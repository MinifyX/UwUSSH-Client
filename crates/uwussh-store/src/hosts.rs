//! Hosts and the identity each one logs in with.
//!
//! The data model keeps identities separate from hosts, so one key can serve
//! forty machines. The UI of M0 does not expose that yet: every host gets its
//! own identity row behind the scenes, and [`HostRecord`] presents the pair as
//! one flat thing. Sharing identities later changes the UI, not the schema.

use crate::{now_ms, tick, vault_id, Result, Store, StoreError};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthMethod {
    /// Asked for on every connect until the vault exists — never stored.
    Password,
    /// A private key file, referenced by path.
    Key,
}

impl AuthMethod {
    fn as_str(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::Key => "key",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "key" => Self::Key,
            // agent, keyboard-interactive and cert arrive with later milestones;
            // until then they fall back to the one method every server offers.
            _ => Self::Password,
        }
    }
}

/// A host as the UI sees it: the host and its identity, flattened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostRecord {
    pub id: Uuid,
    pub name: String,
    pub address: String,
    pub port: u16,
    pub username: String,
    pub auth: AuthMethod,
    pub key_path: Option<String>,
    pub group_path: Option<String>,
    pub last_connected_ms: Option<u64>,
}

/// What the host form submits. No `id` means a new host.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostDraft {
    pub id: Option<Uuid>,
    pub name: String,
    pub address: String,
    pub port: u16,
    pub username: String,
    pub auth: AuthMethod,
    pub key_path: Option<String>,
    pub group_path: Option<String>,
}

fn invalid(field: &'static str, problem: &'static str) -> StoreError {
    StoreError::Invalid { field, problem }
}

fn blank_to_none(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

impl HostDraft {
    fn validated(self) -> Result<Self> {
        let address = self.address.trim().to_string();
        if address.is_empty() {
            return Err(invalid("address", "required"));
        }
        if address.chars().any(char::is_whitespace) {
            return Err(invalid("address", "whitespace"));
        }
        if self.port == 0 {
            return Err(invalid("port", "out-of-range"));
        }

        let username = self.username.trim().to_string();
        if username.is_empty() {
            return Err(invalid("username", "required"));
        }
        if username.chars().any(char::is_whitespace) {
            return Err(invalid("username", "whitespace"));
        }

        let key_path = match self.auth {
            AuthMethod::Key => {
                Some(blank_to_none(self.key_path).ok_or_else(|| invalid("keyPath", "required"))?)
            }
            // Switching a host to password auth must not leave a stale key
            // path behind that the next edit would silently resurrect.
            AuthMethod::Password => None,
        };

        let name = match self.name.trim() {
            "" => address.clone(),
            name => name.to_string(),
        };

        Ok(Self {
            id: self.id,
            name,
            address,
            port: self.port,
            username,
            auth: self.auth,
            key_path,
            group_path: blank_to_none(self.group_path),
        })
    }
}

impl Store {
    pub fn list_hosts(&self) -> Result<Vec<HostRecord>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&format!(
            "{HOST_SELECT} WHERE h.deleted = 0 ORDER BY lower(coalesce(h.group_path, '')), lower(h.name)"
        ))?;
        let hosts = stmt
            .query_map([], host_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(hosts)
    }

    pub fn get_host(&self, id: Uuid) -> Result<Option<HostRecord>> {
        read_host(&self.conn.lock(), id)
    }

    /// Insert a new host, or update an existing one when the draft has an id.
    pub fn save_host(&self, draft: HostDraft) -> Result<HostRecord> {
        let draft = draft.validated()?;
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let clock = tick(&tx, self.device)?;
        let vault = vault_id(&tx)?;
        let label = format!("{}@{}", draft.username, draft.address);

        let id = match draft.id {
            Some(id) => {
                let identity: Option<Option<String>> = tx
                    .query_row(
                        "SELECT identity_id FROM hosts WHERE id = ?1 AND deleted = 0",
                        [id.to_string()],
                        |row| row.get(0),
                    )
                    .optional()?;
                let identity = identity.ok_or(StoreError::UnknownHost(id))?;

                let identity_id = match identity {
                    Some(identity_id) => {
                        tx.execute(
                            "UPDATE identities
                                SET label = ?2, username = ?3, auth_type = ?4, key_path = ?5,
                                    hlc_wall_ms = ?6, hlc_counter = ?7, hlc_device = ?8,
                                    rev = rev + 1
                              WHERE id = ?1",
                            params![
                                identity_id,
                                label,
                                draft.username,
                                draft.auth.as_str(),
                                draft.key_path,
                                clock.wall_ms as i64,
                                clock.counter,
                                clock.device,
                            ],
                        )?;
                        identity_id
                    }
                    None => insert_identity(&tx, &vault, &label, &draft, clock)?,
                };

                tx.execute(
                    "UPDATE hosts
                        SET name = ?2, address = ?3, port = ?4, identity_id = ?5, group_path = ?6,
                            hlc_wall_ms = ?7, hlc_counter = ?8, hlc_device = ?9,
                            rev = rev + 1
                      WHERE id = ?1",
                    params![
                        id.to_string(),
                        draft.name,
                        draft.address,
                        draft.port,
                        identity_id,
                        draft.group_path,
                        clock.wall_ms as i64,
                        clock.counter,
                        clock.device,
                    ],
                )?;
                id
            }
            None => {
                let identity_id = insert_identity(&tx, &vault, &label, &draft, clock)?;
                let id = Uuid::now_v7();
                tx.execute(
                    "INSERT INTO hosts
                        (id, vault_id, name, address, port, identity_id, group_path,
                         hlc_wall_ms, hlc_counter, hlc_device)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![
                        id.to_string(),
                        vault,
                        draft.name,
                        draft.address,
                        draft.port,
                        identity_id,
                        draft.group_path,
                        clock.wall_ms as i64,
                        clock.counter,
                        clock.device,
                    ],
                )?;
                id
            }
        };

        let record = read_host(&tx, id)?.ok_or(StoreError::UnknownHost(id))?;
        tx.commit()?;
        Ok(record)
    }

    /// Tombstone, not a hard delete: a device that was offline when this
    /// happened must learn about it on the next sync instead of resurrecting
    /// the host.
    pub fn delete_host(&self, id: Uuid) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let clock = tick(&tx, self.device)?;

        let changed = tx.execute(
            "UPDATE hosts
                SET deleted = 1, rev = rev + 1,
                    hlc_wall_ms = ?2, hlc_counter = ?3, hlc_device = ?4
              WHERE id = ?1 AND deleted = 0",
            params![
                id.to_string(),
                clock.wall_ms as i64,
                clock.counter,
                clock.device
            ],
        )?;
        if changed == 0 {
            return Err(StoreError::UnknownHost(id));
        }

        tx.execute(
            "UPDATE identities
                SET deleted = 1, rev = rev + 1,
                    hlc_wall_ms = ?2, hlc_counter = ?3, hlc_device = ?4
              WHERE id = (SELECT identity_id FROM hosts WHERE id = ?1)",
            params![
                id.to_string(),
                clock.wall_ms as i64,
                clock.counter,
                clock.device
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Record a successful connection. Local bookkeeping, so it neither bumps
    /// the revision nor ticks the sync clock.
    pub fn mark_connected(&self, id: Uuid) -> Result<()> {
        self.conn.lock().execute(
            "UPDATE hosts SET last_connected_ms = ?2 WHERE id = ?1",
            params![id.to_string(), now_ms() as i64],
        )?;
        Ok(())
    }
}

const HOST_SELECT: &str = "SELECT h.id, h.name, h.address, h.port,
            coalesce(i.username, ''), coalesce(i.auth_type, 'password'), i.key_path,
            h.group_path, h.last_connected_ms
       FROM hosts h
       LEFT JOIN identities i ON i.id = h.identity_id AND i.deleted = 0";

fn read_host(conn: &Connection, id: Uuid) -> Result<Option<HostRecord>> {
    Ok(conn
        .query_row(
            &format!("{HOST_SELECT} WHERE h.id = ?1 AND h.deleted = 0"),
            [id.to_string()],
            host_from_row,
        )
        .optional()?)
}

fn host_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<HostRecord> {
    let id: String = row.get(0)?;
    let port: i64 = row.get(3)?;
    let auth: String = row.get(5)?;
    let last: Option<i64> = row.get(8)?;
    Ok(HostRecord {
        id: Uuid::parse_str(&id).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?,
        name: row.get(1)?,
        address: row.get(2)?,
        port: port as u16,
        username: row.get(4)?,
        auth: AuthMethod::parse(&auth),
        key_path: row.get(6)?,
        group_path: row.get(7)?,
        last_connected_ms: last.map(|v| v as u64),
    })
}

fn insert_identity(
    tx: &rusqlite::Transaction,
    vault: &str,
    label: &str,
    draft: &HostDraft,
    clock: uwussh_proto::Hlc,
) -> Result<String> {
    let id = Uuid::now_v7().to_string();
    tx.execute(
        "INSERT INTO identities
            (id, vault_id, label, username, auth_type, key_path, hlc_wall_ms, hlc_counter, hlc_device)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            id,
            vault,
            label,
            draft.username,
            draft.auth.as_str(),
            draft.key_path,
            clock.wall_ms as i64,
            clock.counter,
            clock.device,
        ],
    )?;
    Ok(id)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn draft(name: &str, address: &str) -> HostDraft {
        HostDraft {
            id: None,
            name: name.into(),
            address: address.into(),
            port: 22,
            username: "root".into(),
            auth: AuthMethod::Password,
            key_path: None,
            group_path: None,
        }
    }

    fn store() -> Store {
        Store::open_in_memory().unwrap()
    }

    #[test]
    fn a_saved_host_comes_back_as_saved() {
        let store = store();
        let saved = store.save_host(draft("prox-1", "10.0.0.12")).unwrap();
        assert_eq!(store.list_hosts().unwrap(), vec![saved.clone()]);
        assert_eq!(store.get_host(saved.id).unwrap(), Some(saved));
    }

    #[test]
    fn an_empty_name_defaults_to_the_address() {
        let saved = store().save_host(draft("  ", "nas.lan")).unwrap();
        assert_eq!(saved.name, "nas.lan");
    }

    #[test]
    fn updating_keeps_the_id_and_changes_the_fields() {
        let store = store();
        let saved = store.save_host(draft("old", "10.0.0.1")).unwrap();

        let mut edit = draft("new", "10.0.0.2");
        edit.id = Some(saved.id);
        edit.port = 2222;
        edit.username = "lorin".into();
        let updated = store.save_host(edit).unwrap();

        assert_eq!(updated.id, saved.id);
        assert_eq!(
            (
                updated.name.as_str(),
                updated.address.as_str(),
                updated.port
            ),
            ("new", "10.0.0.2", 2222)
        );
        assert_eq!(updated.username, "lorin");
        assert_eq!(
            store.list_hosts().unwrap().len(),
            1,
            "an update must not insert"
        );
    }

    #[test]
    fn updates_bump_the_revision_for_sync() {
        let store = store();
        let saved = store.save_host(draft("a", "10.0.0.1")).unwrap();
        let mut edit = draft("b", "10.0.0.1");
        edit.id = Some(saved.id);
        store.save_host(edit).unwrap();

        let rev: i64 = store
            .conn
            .lock()
            .query_row(
                "SELECT rev FROM hosts WHERE id = ?1",
                [saved.id.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rev, 2);
    }

    #[test]
    fn validation_names_the_field_that_is_wrong() {
        let store = store();

        let spaced = draft("x", "10.0.0 .1");
        assert!(matches!(
            store.save_host(spaced),
            Err(StoreError::Invalid {
                field: "address",
                problem: "whitespace"
            })
        ));

        let mut no_user = draft("x", "10.0.0.1");
        no_user.username = " ".into();
        assert!(matches!(
            store.save_host(no_user),
            Err(StoreError::Invalid {
                field: "username",
                problem: "required"
            })
        ));

        let mut keyless = draft("x", "10.0.0.1");
        keyless.auth = AuthMethod::Key;
        assert!(matches!(
            store.save_host(keyless),
            Err(StoreError::Invalid {
                field: "keyPath",
                problem: "required"
            })
        ));

        let mut port_zero = draft("x", "10.0.0.1");
        port_zero.port = 0;
        assert!(matches!(
            store.save_host(port_zero),
            Err(StoreError::Invalid {
                field: "port",
                problem: "out-of-range"
            })
        ));
    }

    #[test]
    fn switching_to_password_auth_drops_the_key_path() {
        let store = store();
        let mut keyed = draft("x", "10.0.0.1");
        keyed.auth = AuthMethod::Key;
        keyed.key_path = Some("~/.ssh/id_ed25519".into());
        let saved = store.save_host(keyed).unwrap();
        assert_eq!(saved.key_path.as_deref(), Some("~/.ssh/id_ed25519"));

        let mut edit = draft("x", "10.0.0.1");
        edit.id = Some(saved.id);
        edit.key_path = Some("~/.ssh/id_ed25519".into());
        let updated = store.save_host(edit).unwrap();
        assert_eq!(updated.auth, AuthMethod::Password);
        assert_eq!(updated.key_path, None);
    }

    #[test]
    fn deleted_hosts_disappear_but_stay_as_tombstones() {
        let store = store();
        let saved = store.save_host(draft("x", "10.0.0.1")).unwrap();
        store.delete_host(saved.id).unwrap();

        assert!(store.list_hosts().unwrap().is_empty());
        assert_eq!(store.get_host(saved.id).unwrap(), None);

        let tombstones: i64 = store
            .conn
            .lock()
            .query_row("SELECT count(*) FROM hosts WHERE deleted = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(tombstones, 1, "sync needs the tombstone");
    }

    #[test]
    fn deleting_or_updating_an_unknown_host_is_an_error() {
        let store = store();
        let ghost = Uuid::now_v7();
        assert!(matches!(
            store.delete_host(ghost),
            Err(StoreError::UnknownHost(_))
        ));

        let mut edit = draft("x", "10.0.0.1");
        edit.id = Some(ghost);
        assert!(matches!(
            store.save_host(edit),
            Err(StoreError::UnknownHost(_))
        ));
    }

    #[test]
    fn marking_a_connection_does_not_count_as_an_edit() {
        let store = store();
        let saved = store.save_host(draft("x", "10.0.0.1")).unwrap();
        store.mark_connected(saved.id).unwrap();

        let host = store.get_host(saved.id).unwrap().unwrap();
        assert!(host.last_connected_ms.is_some());
        let rev: i64 = store
            .conn
            .lock()
            .query_row(
                "SELECT rev FROM hosts WHERE id = ?1",
                [saved.id.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rev, 1);
    }
}
