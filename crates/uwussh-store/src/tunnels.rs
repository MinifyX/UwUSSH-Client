//! Tunnels: the local (`ssh -L`) and remote (`ssh -R`) port forwards of a host.
//!
//! A tunnel is a record of its own (`port_forwards`) that points at its host by
//! id, synced like a host. Running one is `uwussh-core`'s business; this is
//! only what to run: where it listens, where that leads, and whether it starts
//! along with a terminal to its host.
//!
//! The host reference has no foreign key: a tunnel can arrive from the server
//! before its host, and is only listed once the host is there. Deleting a host
//! deletes its tunnels in the same transaction.

use crate::hosts::blank_to_none;
use crate::{tick, vault_id, Result, Store, StoreError};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The address a tunnel listens on when none is given: this computer only (or
/// the server's loopback, for a remote one) — never every network at once.
pub const DEFAULT_BIND_ADDRESS: &str = "127.0.0.1";

/// A tunnel as the interface sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelRecord {
    pub id: Uuid,
    pub host_id: Uuid,
    pub name: String,
    /// `local` or `remote` — or a kind a newer build added, which this one
    /// keeps and shows but does not run.
    pub kind: String,
    pub bind_address: String,
    pub bind_port: u16,
    pub target_host: String,
    pub target_port: u16,
    pub autostart: bool,
}

/// What the tunnel form submits. No `id` means a new tunnel.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelDraft {
    pub id: Option<Uuid>,
    pub host_id: Uuid,
    #[serde(default)]
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub bind_address: Option<String>,
    pub bind_port: u16,
    pub target_host: String,
    pub target_port: u16,
    #[serde(default)]
    pub autostart: bool,
}

fn invalid(field: &'static str, problem: &'static str) -> StoreError {
    StoreError::Invalid { field, problem }
}

/// A host name or address as typed: one word, no spaces, not a novel.
fn address(value: Option<String>, field: &'static str) -> Result<Option<String>> {
    match blank_to_none(value) {
        None => Ok(None),
        Some(text) if text.chars().any(|c| c.is_whitespace() || c.is_control()) => {
            Err(invalid(field, "whitespace"))
        }
        Some(text) if text.len() > 253 => Err(invalid(field, "too-long")),
        Some(text) => Ok(Some(text)),
    }
}

impl TunnelDraft {
    fn validated(self) -> Result<Self> {
        if !matches!(self.kind.as_str(), "local" | "remote") {
            return Err(invalid("kind", "unknown"));
        }
        let bind_address = address(self.bind_address, "bindAddress")?
            .unwrap_or_else(|| DEFAULT_BIND_ADDRESS.to_string());
        if self.bind_port == 0 {
            return Err(invalid("bindPort", "out-of-range"));
        }
        let target_host = address(Some(self.target_host), "targetHost")?
            .ok_or(invalid("targetHost", "required"))?;
        if self.target_port == 0 {
            return Err(invalid("targetPort", "out-of-range"));
        }
        let name = self.name.trim().to_string();
        if name.chars().any(char::is_control) {
            return Err(invalid("name", "control"));
        }
        if name.chars().count() > 80 {
            return Err(invalid("name", "too-long"));
        }
        // An empty name says what the tunnel does, the way `ssh -L` would.
        let name = if name.is_empty() {
            format!("{} → {}:{}", self.bind_port, target_host, self.target_port)
        } else {
            name
        };
        Ok(Self {
            id: self.id,
            host_id: self.host_id,
            name,
            kind: self.kind,
            bind_address: Some(bind_address),
            bind_port: self.bind_port,
            target_host,
            target_port: self.target_port,
            autostart: self.autostart,
        })
    }
}

const TUNNEL_SELECT: &str = "SELECT t.id, t.host_id, t.name, t.kind, t.bind_address, t.bind_port,
        t.target_host, t.target_port, t.autostart
   FROM port_forwards t
   JOIN hosts h ON h.id = t.host_id AND h.deleted = 0";

fn tunnel_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TunnelRecord> {
    let uuid = |index: usize| -> rusqlite::Result<Uuid> {
        let text: String = row.get(index)?;
        Uuid::parse_str(&text).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                index,
                rusqlite::types::Type::Text,
                Box::new(e),
            )
        })
    };
    let port = |index: usize| -> rusqlite::Result<u16> {
        let value: i64 = row.get(index)?;
        Ok(u16::try_from(value).unwrap_or(0))
    };
    Ok(TunnelRecord {
        id: uuid(0)?,
        host_id: uuid(1)?,
        name: row.get(2)?,
        kind: row.get(3)?,
        bind_address: row.get(4)?,
        bind_port: port(5)?,
        target_host: row.get(6)?,
        target_port: port(7)?,
        autostart: row.get(8)?,
    })
}

fn read_tunnel(conn: &Connection, id: Uuid) -> Result<Option<TunnelRecord>> {
    Ok(conn
        .query_row(
            &format!("{TUNNEL_SELECT} WHERE t.id = ?1 AND t.deleted = 0"),
            [id.to_string()],
            tunnel_from_row,
        )
        .optional()?)
}

fn unknown_tunnel() -> StoreError {
    invalid("tunnel", "unknown")
}

impl Store {
    /// Every tunnel of every host there is, by host and name.
    pub fn list_tunnels(&self) -> Result<Vec<TunnelRecord>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&format!(
            "{TUNNEL_SELECT} WHERE t.deleted = 0 ORDER BY t.host_id, lower(t.name), t.id"
        ))?;
        let tunnels = stmt
            .query_map([], tunnel_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(tunnels)
    }

    pub fn get_tunnel(&self, id: Uuid) -> Result<Option<TunnelRecord>> {
        read_tunnel(&self.conn.lock(), id)
    }

    /// Add a tunnel to a host, or change one when the draft has an id. A
    /// tunnel stays with the host it was made for.
    pub fn save_tunnel(&self, draft: TunnelDraft) -> Result<TunnelRecord> {
        let draft = draft.validated()?;
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let host_exists: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM hosts WHERE id = ?1 AND deleted = 0)",
            [draft.host_id.to_string()],
            |row| row.get(0),
        )?;
        if !host_exists {
            return Err(StoreError::UnknownHost(draft.host_id));
        }
        let clock = tick(&tx, self.device)?;
        let bind_address = draft.bind_address.unwrap_or_default();
        let id = match draft.id {
            Some(id) => {
                let changed = tx.execute(
                    "UPDATE port_forwards
                        SET name = ?3, kind = ?4, bind_address = ?5, bind_port = ?6,
                            target_host = ?7, target_port = ?8, autostart = ?9,
                            rev = rev + 1, dirty = 1,
                            hlc_wall_ms = ?10, hlc_counter = ?11, hlc_device = ?12
                      WHERE id = ?1 AND host_id = ?2 AND deleted = 0",
                    params![
                        id.to_string(),
                        draft.host_id.to_string(),
                        draft.name,
                        draft.kind,
                        bind_address,
                        draft.bind_port,
                        draft.target_host,
                        draft.target_port,
                        draft.autostart,
                        clock.wall_ms as i64,
                        clock.counter,
                        clock.device,
                    ],
                )?;
                if changed == 0 {
                    return Err(unknown_tunnel());
                }
                id
            }
            None => {
                let id = Uuid::now_v7();
                let vault = vault_id(&tx)?;
                tx.execute(
                    "INSERT INTO port_forwards
                        (id, vault_id, host_id, name, kind, bind_address, bind_port,
                         target_host, target_port, autostart,
                         hlc_wall_ms, hlc_counter, hlc_device)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                    params![
                        id.to_string(),
                        vault,
                        draft.host_id.to_string(),
                        draft.name,
                        draft.kind,
                        bind_address,
                        draft.bind_port,
                        draft.target_host,
                        draft.target_port,
                        draft.autostart,
                        clock.wall_ms as i64,
                        clock.counter,
                        clock.device,
                    ],
                )?;
                id
            }
        };
        let record = read_tunnel(&tx, id)?.ok_or_else(unknown_tunnel)?;
        tx.commit()?;
        Ok(record)
    }

    /// Tombstone, like a host, so a device that was offline learns about it.
    pub fn delete_tunnel(&self, id: Uuid) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let clock = tick(&tx, self.device)?;
        let changed = tx.execute(
            "UPDATE port_forwards
                SET deleted = 1, rev = rev + 1, dirty = 1,
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
            return Err(unknown_tunnel());
        }
        tx.commit()?;
        Ok(())
    }
}

/// A deleted host's tunnels go with it, under the host's clock.
pub(crate) fn delete_host_tunnels(
    tx: &Transaction,
    host: Uuid,
    clock: uwussh_proto::Hlc,
) -> Result<()> {
    tx.execute(
        "UPDATE port_forwards
            SET deleted = 1, rev = rev + 1, dirty = 1,
                hlc_wall_ms = ?2, hlc_counter = ?3, hlc_device = ?4
          WHERE host_id = ?1 AND deleted = 0",
        params![
            host.to_string(),
            clock.wall_ms as i64,
            clock.counter,
            clock.device
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hosts::tests::draft as host_draft;

    fn tunnel(host: Uuid) -> TunnelDraft {
        TunnelDraft {
            id: None,
            host_id: host,
            name: String::new(),
            kind: "local".into(),
            bind_address: None,
            bind_port: 8080,
            target_host: "localhost".into(),
            target_port: 80,
            autostart: false,
        }
    }

    #[test]
    fn a_tunnel_is_saved_with_sensible_defaults_and_listed_with_its_host() {
        let store = Store::open_in_memory().unwrap();
        let host = store.save_host(host_draft("web", "192.0.2.10")).unwrap();
        let saved = store.save_tunnel(tunnel(host.id)).unwrap();
        assert_eq!(saved.bind_address, DEFAULT_BIND_ADDRESS);
        assert_eq!(saved.name, "8080 → localhost:80");
        assert!(!saved.autostart);
        assert_eq!(store.list_tunnels().unwrap(), vec![saved.clone()]);

        let edited = store
            .save_tunnel(TunnelDraft {
                id: Some(saved.id),
                name: "Admin UI".into(),
                kind: "remote".into(),
                bind_address: Some("0.0.0.0".into()),
                autostart: true,
                ..tunnel(host.id)
            })
            .unwrap();
        assert_eq!(edited.id, saved.id);
        assert_eq!(edited.kind, "remote");
        assert_eq!(edited.bind_address, "0.0.0.0");
        assert!(edited.autostart);
        assert_eq!(store.get_tunnel(saved.id).unwrap(), Some(edited));
    }

    #[test]
    fn a_tunnel_needs_a_kind_ports_a_target_and_a_host() {
        let store = Store::open_in_memory().unwrap();
        let host = store.save_host(host_draft("web", "192.0.2.10")).unwrap();
        let problem = |draft: TunnelDraft| match store.save_tunnel(draft) {
            Err(StoreError::Invalid { field, problem }) => (field, problem),
            other => panic!("expected invalid, got {other:?}"),
        };
        assert_eq!(
            problem(TunnelDraft {
                kind: "dynamic".into(),
                ..tunnel(host.id)
            }),
            ("kind", "unknown")
        );
        assert_eq!(
            problem(TunnelDraft {
                bind_port: 0,
                ..tunnel(host.id)
            }),
            ("bindPort", "out-of-range")
        );
        assert_eq!(
            problem(TunnelDraft {
                target_host: "  ".into(),
                ..tunnel(host.id)
            }),
            ("targetHost", "required")
        );
        assert_eq!(
            problem(TunnelDraft {
                bind_address: Some("127.0.0.1 evil".into()),
                ..tunnel(host.id)
            }),
            ("bindAddress", "whitespace")
        );
        assert!(matches!(
            store.save_tunnel(tunnel(Uuid::now_v7())),
            Err(StoreError::UnknownHost(_))
        ));
    }

    #[test]
    fn deleting_a_host_deletes_its_tunnels_and_both_wait_to_be_pushed() {
        let store = Store::open_in_memory().unwrap();
        let host = store.save_host(host_draft("web", "192.0.2.10")).unwrap();
        let other = store.save_host(host_draft("db", "192.0.2.11")).unwrap();
        let gone = store.save_tunnel(tunnel(host.id)).unwrap();
        let kept = store.save_tunnel(tunnel(other.id)).unwrap();

        store.delete_host(host.id).unwrap();
        assert_eq!(store.list_tunnels().unwrap(), vec![kept]);
        let (deleted, dirty): (bool, bool) = store
            .conn
            .lock()
            .query_row(
                "SELECT deleted, dirty FROM port_forwards WHERE id = ?1",
                [gone.id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(
            deleted && dirty,
            "a tombstone the other devices learn about"
        );
    }

    #[test]
    fn deleting_a_tunnel_leaves_a_tombstone() {
        let store = Store::open_in_memory().unwrap();
        let host = store.save_host(host_draft("web", "192.0.2.10")).unwrap();
        let saved = store.save_tunnel(tunnel(host.id)).unwrap();
        store.delete_tunnel(saved.id).unwrap();
        assert!(store.list_tunnels().unwrap().is_empty());
        assert!(store.delete_tunnel(saved.id).is_err());
    }
}
