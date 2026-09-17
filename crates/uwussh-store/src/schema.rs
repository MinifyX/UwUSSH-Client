//! Migrations, tracked in SQLite's own `user_version`.
//!
//! Each version is one SQL batch applied in one transaction, so an interrupted
//! upgrade leaves the previous schema intact rather than half of the new one.

use crate::{Result, StoreError};
use rusqlite::{params, Connection};
use uuid::Uuid;

pub const SCHEMA_VERSION: i64 = 3;

/// The sync header (`id`, `vault_id`, clock, `rev`, `deleted`) is on every
/// syncable table from the start; see the crate docs for why.
const V1: &str = r#"
CREATE TABLE meta (
    id              INTEGER PRIMARY KEY CHECK (id = 1),
    device_id       INTEGER NOT NULL,
    vault_id        TEXT    NOT NULL,
    clock_wall_ms   INTEGER NOT NULL DEFAULT 0,
    clock_counter   INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE identities (
    id              TEXT    PRIMARY KEY,
    vault_id        TEXT    NOT NULL,
    label           TEXT    NOT NULL,
    username        TEXT    NOT NULL,
    auth_type       TEXT    NOT NULL
                    CHECK (auth_type IN ('password', 'key', 'agent', 'keyboard-interactive', 'cert')),
    -- Local only, never synced: until the vault exists a key is a file on
    -- this machine, and its path means nothing on another one.
    key_path        TEXT,
    hlc_wall_ms     INTEGER NOT NULL,
    hlc_counter     INTEGER NOT NULL,
    hlc_device      INTEGER NOT NULL,
    rev             INTEGER NOT NULL DEFAULT 1,
    deleted         INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE hosts (
    id                  TEXT    PRIMARY KEY,
    vault_id            TEXT    NOT NULL,
    name                TEXT    NOT NULL,
    address             TEXT    NOT NULL,
    port                INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
    identity_id         TEXT    REFERENCES identities (id),
    group_path          TEXT,
    hlc_wall_ms         INTEGER NOT NULL,
    hlc_counter         INTEGER NOT NULL,
    hlc_device          INTEGER NOT NULL,
    rev                 INTEGER NOT NULL DEFAULT 1,
    deleted             INTEGER NOT NULL DEFAULT 0,
    -- Local only: when *this* device last connected.
    last_connected_ms   INTEGER
);

CREATE INDEX hosts_live ON hosts (deleted, name);

CREATE TABLE known_hosts (
    id                  TEXT    PRIMARY KEY,
    vault_id            TEXT    NOT NULL,
    address             TEXT    NOT NULL,
    port                INTEGER NOT NULL,
    algorithm           TEXT    NOT NULL,
    fingerprint_sha256  TEXT    NOT NULL,
    public_key          TEXT    NOT NULL,
    first_seen_ms       INTEGER NOT NULL,
    hlc_wall_ms         INTEGER NOT NULL,
    hlc_counter         INTEGER NOT NULL,
    hlc_device          INTEGER NOT NULL,
    rev                 INTEGER NOT NULL DEFAULT 1,
    deleted             INTEGER NOT NULL DEFAULT 0,
    UNIQUE (address, port)
);
"#;

/// Secrets, keys and snippets, plus the vault that seals them. V2 is where
/// UwUSSH first keeps anything secret — everything before it referenced keys
/// by path and asked for passwords every time.
const V2: &str = r#"
CREATE TABLE vault (
    id                  INTEGER PRIMARY KEY CHECK (id = 1),
    vault_id            TEXT    NOT NULL,
    kdf_memory_kib      INTEGER NOT NULL,
    kdf_time_cost       INTEGER NOT NULL,
    kdf_parallelism     INTEGER NOT NULL,
    salt                BLOB    NOT NULL,
    wrapped_nonce       BLOB    NOT NULL,
    wrapped_blob        BLOB    NOT NULL
);

-- One sealed value: a password, a private key or a passphrase. The plaintext
-- is only ever XChaCha20-Poly1305 ciphertext here; the vault key that opens it
-- never touches the database.
CREATE TABLE secrets (
    id                  TEXT    PRIMARY KEY,
    vault_id            TEXT    NOT NULL,
    nonce               BLOB    NOT NULL,
    blob                BLOB    NOT NULL,
    hlc_wall_ms         INTEGER NOT NULL,
    hlc_counter         INTEGER NOT NULL,
    hlc_device          INTEGER NOT NULL,
    rev                 INTEGER NOT NULL DEFAULT 1,
    deleted             INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE keys (
    id                      TEXT    PRIMARY KEY,
    vault_id                TEXT    NOT NULL,
    label                   TEXT    NOT NULL,
    key_type                TEXT    NOT NULL,
    public_key              TEXT    NOT NULL DEFAULT '',
    private_secret_id       TEXT    REFERENCES secrets (id),
    passphrase_secret_id    TEXT    REFERENCES secrets (id),
    hlc_wall_ms             INTEGER NOT NULL,
    hlc_counter             INTEGER NOT NULL,
    hlc_device              INTEGER NOT NULL,
    rev                     INTEGER NOT NULL DEFAULT 1,
    deleted                 INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE snippets (
    id                  TEXT    PRIMARY KEY,
    vault_id            TEXT    NOT NULL,
    label               TEXT    NOT NULL,
    body                TEXT    NOT NULL,
    group_path          TEXT,
    hlc_wall_ms         INTEGER NOT NULL,
    hlc_counter         INTEGER NOT NULL,
    hlc_device          INTEGER NOT NULL,
    rev                 INTEGER NOT NULL DEFAULT 1,
    deleted             INTEGER NOT NULL DEFAULT 0
);

-- An identity can now carry a stored password and point at a key in the vault,
-- next to the file-path key it already had.
ALTER TABLE identities ADD COLUMN password_secret_id TEXT REFERENCES secrets (id);
ALTER TABLE identities ADD COLUMN key_id TEXT REFERENCES keys (id);
"#;

/// Workspaces, groups of their own and a manual order; what a connection found
/// out about a server; and the vault key kept for this user on this device.
const V3: &str = r#"
-- Private or business, like UwUMail's workspaces. Every host is in one.
ALTER TABLE hosts ADD COLUMN workspace TEXT NOT NULL DEFAULT 'private'
    CHECK (workspace IN ('private', 'business'));
-- The order the user dragged hosts into within their group. Ties sort by name.
ALTER TABLE hosts ADD COLUMN position INTEGER NOT NULL DEFAULT 0;
-- Local only: the system the last connection found, like 'ubuntu'.
ALTER TABLE hosts ADD COLUMN os_id TEXT;

-- A group is a record of its own, so an empty one stays and groups keep the
-- order they were dragged into. Hosts still name their group by `group_path`.
CREATE TABLE host_groups (
    id                  TEXT    PRIMARY KEY,
    vault_id            TEXT    NOT NULL,
    workspace           TEXT    NOT NULL CHECK (workspace IN ('private', 'business')),
    name                TEXT    NOT NULL,
    position            INTEGER NOT NULL DEFAULT 0,
    hlc_wall_ms         INTEGER NOT NULL,
    hlc_counter         INTEGER NOT NULL,
    hlc_device          INTEGER NOT NULL,
    rev                 INTEGER NOT NULL DEFAULT 1,
    deleted             INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX host_groups_live ON host_groups (deleted, workspace, position);

-- Local only, never synced: the vault key sealed by the operating system for
-- this user, so the vault opens without the master password. The check value
-- was sealed with the real vault key and tells a stale entry from a good one.
CREATE TABLE device_unlock (
    id                  INTEGER PRIMARY KEY CHECK (id = 1),
    vault_id            TEXT    NOT NULL,
    protected           BLOB    NOT NULL,
    check_nonce         BLOB    NOT NULL,
    check_blob          BLOB    NOT NULL,
    created_ms          INTEGER NOT NULL
);
"#;

pub fn migrate(conn: &mut Connection) -> Result<()> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;

    if version > SCHEMA_VERSION {
        // Refusing is the only safe option: an older build writing into a newer
        // schema can corrupt data it does not understand.
        return Err(StoreError::SchemaTooNew {
            found: version,
            known: SCHEMA_VERSION,
        });
    }

    if version < 1 {
        let tx = conn.transaction()?;
        tx.execute_batch(V1)?;
        tx.execute(
            "INSERT INTO meta (id, device_id, vault_id) VALUES (1, ?1, ?2)",
            params![i64::from(rand::random::<u32>()), Uuid::now_v7().to_string()],
        )?;
        tx.pragma_update(None, "user_version", 1)?;
        tx.commit()?;
        tracing::info!("store created at schema 1");
    }

    if version < 2 {
        let tx = conn.transaction()?;
        tx.execute_batch(V2)?;
        tx.pragma_update(None, "user_version", 2)?;
        tx.commit()?;
        tracing::info!("store migrated to schema 2");
    }

    if version < 3 {
        let tx = conn.transaction()?;
        tx.execute_batch(V3)?;
        // Every group a host already names becomes a group record, in
        // alphabetical order — the order the list showed until now.
        let names = {
            let mut stmt = tx.prepare(
                "SELECT DISTINCT group_path FROM hosts
                  WHERE deleted = 0 AND group_path IS NOT NULL
                  ORDER BY lower(group_path)",
            )?;
            let names = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            names
        };
        let (vault_id, device): (String, i64) = tx.query_row(
            "SELECT vault_id, device_id FROM meta WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        for (position, name) in names.iter().enumerate() {
            tx.execute(
                "INSERT INTO host_groups
                    (id, vault_id, workspace, name, position, hlc_wall_ms, hlc_counter, hlc_device)
                 VALUES (?1, ?2, 'private', ?3, ?4, 0, 0, ?5)",
                params![
                    Uuid::now_v7().to_string(),
                    vault_id,
                    name,
                    position as i64,
                    device
                ],
            )?;
        }
        // Repairs for what 0.1.0-beta.1 left behind. Deleting a host tombstoned
        // its login even when other hosts still used it: those come back.
        // And a deleted host's password stayed, as ciphertext: every secret
        // no live login or key points at goes now.
        tx.execute_batch(
            "UPDATE identities SET deleted = 0
              WHERE deleted = 1
                AND id IN (SELECT identity_id FROM hosts
                            WHERE deleted = 0 AND identity_id IS NOT NULL);
             UPDATE secrets SET deleted = 1, nonce = x'', blob = x'', rev = rev + 1
              WHERE deleted = 0
                AND id NOT IN (SELECT password_secret_id FROM identities
                                WHERE deleted = 0 AND password_secret_id IS NOT NULL)
                AND id NOT IN (SELECT private_secret_id FROM keys
                                WHERE deleted = 0 AND private_secret_id IS NOT NULL)
                AND id NOT IN (SELECT passphrase_secret_id FROM keys
                                WHERE deleted = 0 AND passphrase_secret_id IS NOT NULL);",
        )?;
        tx.pragma_update(None, "user_version", 3)?;
        tx.commit()?;
        tracing::info!("store migrated to schema 3");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrating_twice_is_harmless() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        migrate(&mut conn).unwrap();
        let rows: i64 = conn
            .query_row("SELECT count(*) FROM meta", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1);
    }

    #[test]
    fn a_v1_database_upgrades_to_v2_without_losing_its_hosts() {
        let mut conn = Connection::open_in_memory().unwrap();
        // Bring it up as a V1 database would have been.
        conn.execute_batch(V1).unwrap();
        conn.execute(
            "INSERT INTO meta (id, device_id, vault_id) VALUES (1, 1, ?1)",
            [Uuid::now_v7().to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO hosts (id, vault_id, name, address, port,
                                hlc_wall_ms, hlc_counter, hlc_device)
             VALUES (?1, ?2, 'nas', 'nas.lan', 22, 0, 0, 1)",
            [Uuid::now_v7().to_string(), Uuid::now_v7().to_string()],
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();

        migrate(&mut conn).unwrap();

        let version: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let hosts: i64 = conn
            .query_row("SELECT count(*) FROM hosts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(hosts, 1, "the existing host survived the migration");
        // The V2 tables and columns exist now.
        conn.query_row("SELECT count(*) FROM secrets", [], |r| r.get::<_, i64>(0))
            .unwrap();
        conn.query_row("SELECT count(key_id) FROM identities", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap();
    }

    #[test]
    fn a_v2_database_upgrades_to_v3_with_its_groups_as_records() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(V1).unwrap();
        conn.execute_batch(V2).unwrap();
        conn.execute(
            "INSERT INTO meta (id, device_id, vault_id) VALUES (1, 1, ?1)",
            [Uuid::now_v7().to_string()],
        )
        .unwrap();
        for (name, group) in [("a", Some("Homelab")), ("b", Some("Work")), ("c", None)] {
            conn.execute(
                "INSERT INTO hosts (id, vault_id, name, address, port, group_path,
                                    hlc_wall_ms, hlc_counter, hlc_device)
                 VALUES (?1, 'v', ?2, '10.0.0.1', 22, ?3, 0, 0, 1)",
                params![Uuid::now_v7().to_string(), name, group],
            )
            .unwrap();
        }
        conn.pragma_update(None, "user_version", 2).unwrap();

        migrate(&mut conn).unwrap();

        let groups: Vec<(String, String, i64)> = conn
            .prepare("SELECT workspace, name, position FROM host_groups ORDER BY position")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            groups,
            vec![
                ("private".into(), "Homelab".into(), 0),
                ("private".into(), "Work".into(), 1)
            ]
        );
        let private: i64 = conn
            .query_row(
                "SELECT count(*) FROM hosts WHERE workspace = 'private'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(private, 3, "existing hosts start out private");
    }

    #[test]
    fn upgrading_repairs_what_the_first_beta_left_behind() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(V1).unwrap();
        conn.execute_batch(V2).unwrap();
        conn.execute(
            "INSERT INTO meta (id, device_id, vault_id) VALUES (1, 1, 'v')",
            [],
        )
        .unwrap();
        // Two secrets: one a live login uses, one a deleted host left behind.
        conn.execute_batch(
            "INSERT INTO secrets (id, vault_id, nonce, blob, hlc_wall_ms, hlc_counter, hlc_device)
                  VALUES ('kept', 'v', x'01', x'02', 0, 0, 1),
                         ('orphan', 'v', x'01', x'02', 0, 0, 1);
             INSERT INTO identities (id, vault_id, label, username, auth_type, password_secret_id,
                                     hlc_wall_ms, hlc_counter, hlc_device, deleted)
                  VALUES ('shared', 'v', '', 'uwu', 'password', 'kept', 0, 0, 1, 1),
                         ('gone', 'v', '', 'old', 'password', 'orphan', 0, 0, 1, 1);
             INSERT INTO hosts (id, vault_id, name, address, port, identity_id,
                                hlc_wall_ms, hlc_counter, hlc_device, deleted)
                  VALUES ('a', 'v', 'a', '10.0.0.1', 22, 'shared', 0, 0, 1, 1),
                         ('b', 'v', 'b', '10.0.0.2', 22, 'shared', 0, 0, 1, 0),
                         ('c', 'v', 'c', '10.0.0.3', 22, 'gone', 0, 0, 1, 1);",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 2).unwrap();

        migrate(&mut conn).unwrap();

        let login: i64 = conn
            .query_row(
                "SELECT deleted FROM identities WHERE id = 'shared'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(login, 0, "b's login came back");
        let secrets: Vec<(String, i64, Vec<u8>)> = conn
            .prepare("SELECT id, deleted, blob FROM secrets ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            secrets,
            vec![
                ("kept".into(), 0, vec![2]),
                ("orphan".into(), 1, Vec::new())
            ]
        );
    }

    #[test]
    fn a_newer_schema_is_refused_rather_than_trampled() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
        assert!(matches!(
            migrate(&mut conn),
            Err(StoreError::SchemaTooNew { .. })
        ));
    }
}
