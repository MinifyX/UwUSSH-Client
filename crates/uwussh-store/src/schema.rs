//! Migrations, tracked in SQLite's own `user_version`.
//!
//! Each version is one SQL batch applied in one transaction, so an interrupted
//! upgrade leaves the previous schema intact rather than half of the new one.

use crate::{Result, StoreError};
use rusqlite::{params, Connection};
use uuid::Uuid;

pub const SCHEMA_VERSION: i64 = 1;

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
