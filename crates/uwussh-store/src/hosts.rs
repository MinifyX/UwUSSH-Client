//! Hosts and the identity each one logs in with.
//!
//! The data model keeps identities separate from hosts, so one key can serve
//! forty machines. The UI does not expose that yet: every host gets its own
//! identity row behind the scenes, and [`HostRecord`] presents the pair as one
//! flat thing. Sharing identities later changes the UI, not the schema.
//!
//! A host lives in a workspace (private or business) and optionally a group,
//! at a position the user dragged it to. How it logs in: a password typed on
//! connect or stored in the vault, a key file, or a key from the vault. A
//! stored password is useful next to a key too — the terminal types it when
//! `sudo` asks.

use crate::secret::SecretText;
use crate::vault::{forget_secret, seal_secret, truncate_wal};
use crate::{now_ms, tick, vault_id, Result, Store, StoreError};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use uwussh_proto::Hlc;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Workspace {
    #[default]
    Private,
    Business,
}

impl Workspace {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Business => "business",
        }
    }

    pub(crate) fn parse(value: &str) -> Self {
        match value {
            "business" => Self::Business,
            _ => Self::Private,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthMethod {
    /// Typed on connect, or stored in the vault.
    Password,
    /// A private key: a file referenced by path, or a key kept in the vault.
    Key,
}

impl AuthMethod {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::Key => "key",
        }
    }

    pub(crate) fn parse(value: &str) -> Self {
        match value {
            "key" => Self::Key,
            // agent, keyboard-interactive and cert arrive with later milestones;
            // until then they fall back to the one method every server offers.
            _ => Self::Password,
        }
    }
}

/// A host as the UI sees it: the host and its identity, flattened. Nothing in
/// here is secret; `has_password` only says that a password is stored.
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
    pub workspace: Workspace,
    pub position: i64,
    /// What the last connection found the server to run, like `ubuntu`.
    pub os: Option<String>,
    /// A password for this host is sealed in the vault.
    pub has_password: bool,
    /// The vault key this host logs in with, if it uses one.
    pub key_id: Option<Uuid>,
    pub key_label: Option<String>,
}

/// What happens to a host's stored password when the form is saved.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum PasswordChange {
    #[default]
    Keep,
    Set {
        value: SecretText,
    },
    Forget,
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
    /// `None` keeps an edited host where it is and puts a new one in private.
    #[serde(default)]
    pub workspace: Option<Workspace>,
    /// A key from the vault; with key auth it wins over `key_path`.
    #[serde(default)]
    pub key_id: Option<Uuid>,
    #[serde(default)]
    pub password: PasswordChange,
}

fn invalid(field: &'static str, problem: &'static str) -> StoreError {
    StoreError::Invalid { field, problem }
}

pub(crate) fn blank_to_none(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// A group name as typed: trimmed, not empty, one line, not a novel.
pub(crate) fn group_name(value: Option<String>) -> Result<Option<String>> {
    match blank_to_none(value) {
        None => Ok(None),
        Some(name) if name.chars().any(char::is_control) => Err(invalid("groupPath", "control")),
        Some(name) if name.chars().count() > 80 => Err(invalid("groupPath", "too-long")),
        Some(name) => Ok(Some(name)),
    }
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

        // Switching a host to password auth must not leave a stale key behind
        // that the next edit would silently resurrect.
        let (key_path, key_id) = match self.auth {
            AuthMethod::Key => match self.key_id {
                Some(id) => (None, Some(id)),
                None => (
                    Some(
                        blank_to_none(self.key_path)
                            .ok_or_else(|| invalid("keyPath", "required"))?,
                    ),
                    None,
                ),
            },
            AuthMethod::Password => (None, None),
        };

        if let PasswordChange::Set { value } = &self.password {
            if value.expose().is_empty() {
                return Err(invalid("password", "required"));
            }
        }

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
            group_path: group_name(self.group_path)?,
            workspace: self.workspace,
            key_id,
            password: self.password,
        })
    }
}

impl Store {
    pub fn list_hosts(&self) -> Result<Vec<HostRecord>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&format!(
            "{HOST_SELECT} WHERE h.deleted = 0
              ORDER BY h.workspace, lower(coalesce(h.group_path, '')), h.position, lower(h.name)"
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
    ///
    /// Setting a password seals it in the vault, so that needs the vault
    /// unlocked — checked before anything is written.
    pub fn save_host(&self, draft: HostDraft) -> Result<HostRecord> {
        let draft = draft.validated()?;
        // Connection first, vault second: the order every other path takes.
        let mut conn = self.conn.lock();
        let vault_guard = self.vault.lock();
        if matches!(draft.password, PasswordChange::Set { .. }) && vault_guard.is_none() {
            return Err(StoreError::VaultLocked);
        }

        let tx = conn.transaction()?;
        let clock = tick(&tx, self.device)?;
        let vault = vault_id(&tx)?;
        let label = format!("{}@{}", draft.username, draft.address);

        if let Some(key_id) = draft.key_id {
            let live: i64 = tx.query_row(
                "SELECT count(*) FROM keys WHERE id = ?1 AND deleted = 0",
                [key_id.to_string()],
                |row| row.get(0),
            )?;
            if live == 0 {
                return Err(invalid("keyId", "unknown"));
            }
        }

        // Where the host is now, if it exists: an edit that keeps workspace
        // and group keeps its position too.
        let existing: Option<(Option<String>, String, Option<String>, i64)> = match draft.id {
            Some(id) => Some(
                tx.query_row(
                    "SELECT identity_id, workspace, group_path, position
                       FROM hosts WHERE id = ?1 AND deleted = 0",
                    [id.to_string()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()?
                .ok_or(StoreError::UnknownHost(id))?,
            ),
            None => None,
        };

        let workspace = draft
            .workspace
            .or_else(|| existing.as_ref().map(|(_, w, _, _)| Workspace::parse(w)))
            .unwrap_or_default();
        if let Some(group) = &draft.group_path {
            ensure_group(&tx, self.device, &vault, workspace, group)?;
        }
        let position = match &existing {
            Some((_, w, g, position))
                if Workspace::parse(w) == workspace && *g == draft.group_path =>
            {
                *position
            }
            _ => next_position(&tx, workspace, draft.group_path.as_deref())?,
        };

        // An identity several hosts share (a Termius import makes those) is
        // copied first: editing one host must never change another's login.
        let existing = match (draft.id, existing) {
            (Some(id), Some((Some(identity_id), w, g, p))) => {
                Some((Some(own_identity(&tx, id, &identity_id, clock)?), w, g, p))
            }
            (_, existing) => existing,
        };

        let old_password: Option<String> = match existing.as_ref().and_then(|(i, ..)| i.as_ref()) {
            Some(identity_id) => tx
                .query_row(
                    "SELECT password_secret_id FROM identities WHERE id = ?1",
                    [identity_id],
                    |row| row.get(0),
                )
                .optional()?
                .flatten(),
            None => None,
        };
        let password_secret_id = match &draft.password {
            PasswordChange::Keep => old_password.clone(),
            PasswordChange::Set { value } => {
                let vault_key = vault_guard.as_ref().ok_or(StoreError::VaultLocked)?;
                Some(seal_secret(
                    &tx,
                    self.device,
                    vault_key,
                    value.expose().as_bytes(),
                )?)
            }
            PasswordChange::Forget => None,
        };
        drop(vault_guard);

        let identity = IdentityFields {
            label: &label,
            username: &draft.username,
            auth: draft.auth,
            key_path: draft.key_path.as_deref(),
            key_id: draft.key_id.map(|id| id.to_string()),
            password_secret_id,
        };

        let id = match (draft.id, existing) {
            (Some(id), Some((identity_id, ..))) => {
                let identity_id = match identity_id {
                    Some(identity_id) => {
                        update_identity(&tx, &identity_id, &identity, clock)?;
                        identity_id
                    }
                    None => insert_identity(&tx, &vault, &identity, clock)?,
                };
                tx.execute(
                    "UPDATE hosts
                        SET name = ?2, address = ?3, port = ?4, identity_id = ?5, group_path = ?6,
                            workspace = ?7, position = ?8,
                            hlc_wall_ms = ?9, hlc_counter = ?10, hlc_device = ?11,
                            rev = rev + 1
                      WHERE id = ?1",
                    params![
                        id.to_string(),
                        draft.name,
                        draft.address,
                        draft.port,
                        identity_id,
                        draft.group_path,
                        workspace.as_str(),
                        position,
                        clock.wall_ms as i64,
                        clock.counter,
                        clock.device,
                    ],
                )?;
                id
            }
            _ => {
                let identity_id = insert_identity(&tx, &vault, &identity, clock)?;
                let id = Uuid::now_v7();
                tx.execute(
                    "INSERT INTO hosts
                        (id, vault_id, name, address, port, identity_id, group_path,
                         workspace, position, hlc_wall_ms, hlc_counter, hlc_device)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    params![
                        id.to_string(),
                        vault,
                        draft.name,
                        draft.address,
                        draft.port,
                        identity_id,
                        draft.group_path,
                        workspace.as_str(),
                        position,
                        clock.wall_ms as i64,
                        clock.counter,
                        clock.device,
                    ],
                )?;
                id
            }
        };

        // The old password goes once nothing points at it any more.
        if !matches!(draft.password, PasswordChange::Keep) {
            if let Some(old) = &old_password {
                release_secret(&tx, self.device, old)?;
            }
        }

        let record = read_host(&tx, id)?.ok_or(StoreError::UnknownHost(id))?;
        tx.commit()?;
        if !matches!(draft.password, PasswordChange::Keep) {
            truncate_wal(&conn);
        }
        Ok(record)
    }

    /// Tombstone, not a hard delete: a device that was offline when this
    /// happened must learn about it on the next sync instead of resurrecting
    /// the host. A password stored for the host goes with it.
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

        // The identity, and its password, go with the host — unless another
        // host still logs in with them.
        let identity: Option<(String, Option<String>)> = tx
            .query_row(
                "SELECT i.id, i.password_secret_id FROM hosts h
                   JOIN identities i ON i.id = h.identity_id
                  WHERE h.id = ?1
                    AND NOT EXISTS (SELECT 1 FROM hosts other
                                     WHERE other.identity_id = i.id AND other.deleted = 0)",
                [id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((identity, password)) = identity {
            tx.execute(
                "UPDATE identities
                    SET deleted = 1, rev = rev + 1,
                        hlc_wall_ms = ?2, hlc_counter = ?3, hlc_device = ?4
                  WHERE id = ?1",
                params![identity, clock.wall_ms as i64, clock.counter, clock.device],
            )?;
            if let Some(secret) = password {
                release_secret(&tx, self.device, &secret)?;
            }
        }
        tx.commit()?;
        truncate_wal(&conn);
        Ok(())
    }

    /// Store a new password for a host, or forget the stored one — what "save
    /// this password" after a successful login does, without the whole form.
    pub fn set_host_password(&self, id: Uuid, change: PasswordChange) -> Result<HostRecord> {
        let mut conn = self.conn.lock();
        let vault_guard = self.vault.lock();
        if let PasswordChange::Set { value } = &change {
            if value.expose().is_empty() {
                return Err(invalid("password", "required"));
            }
            if vault_guard.is_none() {
                return Err(StoreError::VaultLocked);
            }
        }
        let tx = conn.transaction()?;
        let (identity, old): (Option<String>, Option<String>) = tx
            .query_row(
                "SELECT h.identity_id, i.password_secret_id FROM hosts h
                   LEFT JOIN identities i ON i.id = h.identity_id
                  WHERE h.id = ?1 AND h.deleted = 0",
                [id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .ok_or(StoreError::UnknownHost(id))?;
        let clock = tick(&tx, self.device)?;
        let identity = match identity {
            Some(identity) => own_identity(&tx, id, &identity, clock)?,
            // A host imported without a login gets one now, to keep the password.
            None => {
                let vault = vault_id(&tx)?;
                let created = insert_identity(
                    &tx,
                    &vault,
                    &IdentityFields {
                        label: "",
                        username: "",
                        auth: AuthMethod::Password,
                        key_path: None,
                        key_id: None,
                        password_secret_id: None,
                    },
                    clock,
                )?;
                tx.execute(
                    "UPDATE hosts SET identity_id = ?2, rev = rev + 1 WHERE id = ?1",
                    params![id.to_string(), created],
                )?;
                created
            }
        };
        let secret = match &change {
            PasswordChange::Keep => old.clone(),
            PasswordChange::Set { value } => {
                let vault = vault_guard.as_ref().ok_or(StoreError::VaultLocked)?;
                Some(seal_secret(
                    &tx,
                    self.device,
                    vault,
                    value.expose().as_bytes(),
                )?)
            }
            PasswordChange::Forget => None,
        };
        drop(vault_guard);
        tx.execute(
            "UPDATE identities
                SET password_secret_id = ?2, rev = rev + 1,
                    hlc_wall_ms = ?3, hlc_counter = ?4, hlc_device = ?5
              WHERE id = ?1",
            params![
                identity,
                secret,
                clock.wall_ms as i64,
                clock.counter,
                clock.device
            ],
        )?;
        if !matches!(change, PasswordChange::Keep) {
            if let Some(old) = &old {
                release_secret(&tx, self.device, old)?;
            }
        }
        let record = read_host(&tx, id)?.ok_or(StoreError::UnknownHost(id))?;
        tx.commit()?;
        if !matches!(change, PasswordChange::Keep) {
            truncate_wal(&conn);
        }
        Ok(record)
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

    /// Remember what system a connection found. Local, like the connection
    /// time: another device finds out for itself.
    pub fn set_host_os(&self, id: Uuid, os: Option<&str>) -> Result<()> {
        self.conn.lock().execute(
            "UPDATE hosts SET os_id = ?2 WHERE id = ?1",
            params![id.to_string(), os],
        )?;
        Ok(())
    }
}

pub(crate) const HOST_SELECT: &str = "SELECT h.id, h.name, h.address, h.port,
            coalesce(i.username, ''), coalesce(i.auth_type, 'password'), i.key_path,
            h.group_path, h.last_connected_ms, h.workspace, h.position, h.os_id,
            i.password_secret_id IS NOT NULL, k.id, k.label
       FROM hosts h
       LEFT JOIN identities i ON i.id = h.identity_id AND i.deleted = 0
       LEFT JOIN keys k ON k.id = i.key_id AND k.deleted = 0";

pub(crate) fn read_host(conn: &Connection, id: Uuid) -> Result<Option<HostRecord>> {
    Ok(conn
        .query_row(
            &format!("{HOST_SELECT} WHERE h.id = ?1 AND h.deleted = 0"),
            [id.to_string()],
            host_from_row,
        )
        .optional()?)
}

fn parse_uuid(index: usize, text: &str) -> rusqlite::Result<Uuid> {
    Uuid::parse_str(text).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(e))
    })
}

fn host_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<HostRecord> {
    let id: String = row.get(0)?;
    let port: i64 = row.get(3)?;
    let auth: String = row.get(5)?;
    let last: Option<i64> = row.get(8)?;
    let workspace: String = row.get(9)?;
    let key_id: Option<String> = row.get(13)?;
    Ok(HostRecord {
        id: parse_uuid(0, &id)?,
        name: row.get(1)?,
        address: row.get(2)?,
        port: port as u16,
        username: row.get(4)?,
        auth: AuthMethod::parse(&auth),
        key_path: row.get(6)?,
        group_path: row.get(7)?,
        last_connected_ms: last.map(|v| v as u64),
        workspace: Workspace::parse(&workspace),
        position: row.get(10)?,
        os: row.get(11)?,
        has_password: row.get(12)?,
        key_id: key_id.map(|id| parse_uuid(13, &id)).transpose()?,
        key_label: row.get(14)?,
    })
}

/// The identity `host` logs in with, as its own: when other hosts share it,
/// the host gets a copy first — same user, key and password — so what
/// happens next changes this host only.
fn own_identity(tx: &Transaction, host: Uuid, identity: &str, clock: Hlc) -> Result<String> {
    let shared: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM hosts
                         WHERE identity_id = ?1 AND id != ?2 AND deleted = 0)",
        params![identity, host.to_string()],
        |row| row.get(0),
    )?;
    if !shared {
        return Ok(identity.to_string());
    }
    let copy = Uuid::now_v7().to_string();
    tx.execute(
        "INSERT INTO identities
            (id, vault_id, label, username, auth_type, key_path, key_id, password_secret_id,
             hlc_wall_ms, hlc_counter, hlc_device)
         SELECT ?2, vault_id, label, username, auth_type, key_path, key_id, password_secret_id,
                ?3, ?4, ?5
           FROM identities WHERE id = ?1",
        params![
            identity,
            copy,
            clock.wall_ms as i64,
            clock.counter,
            clock.device
        ],
    )?;
    tx.execute(
        "UPDATE hosts
            SET identity_id = ?2, rev = rev + 1,
                hlc_wall_ms = ?3, hlc_counter = ?4, hlc_device = ?5
          WHERE id = ?1",
        params![
            host.to_string(),
            copy,
            clock.wall_ms as i64,
            clock.counter,
            clock.device
        ],
    )?;
    Ok(copy)
}

/// Forget a password secret once no identity uses it any more. A copied
/// identity shares its original's secret until one of them changes it.
fn release_secret(tx: &Transaction, device: u32, secret: &str) -> Result<()> {
    let used: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM identities
                         WHERE password_secret_id = ?1 AND deleted = 0)",
        [secret],
        |row| row.get(0),
    )?;
    if used {
        Ok(())
    } else {
        forget_secret(tx, device, secret)
    }
}

/// The identity columns a host form decides.
struct IdentityFields<'a> {
    label: &'a str,
    username: &'a str,
    auth: AuthMethod,
    key_path: Option<&'a str>,
    key_id: Option<String>,
    password_secret_id: Option<String>,
}

fn insert_identity(
    tx: &Transaction,
    vault: &str,
    identity: &IdentityFields,
    clock: Hlc,
) -> Result<String> {
    let id = Uuid::now_v7().to_string();
    tx.execute(
        "INSERT INTO identities
            (id, vault_id, label, username, auth_type, key_path, key_id, password_secret_id,
             hlc_wall_ms, hlc_counter, hlc_device)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            id,
            vault,
            identity.label,
            identity.username,
            identity.auth.as_str(),
            identity.key_path,
            identity.key_id,
            identity.password_secret_id,
            clock.wall_ms as i64,
            clock.counter,
            clock.device,
        ],
    )?;
    Ok(id)
}

fn update_identity(
    tx: &Transaction,
    id: &str,
    identity: &IdentityFields,
    clock: Hlc,
) -> Result<()> {
    tx.execute(
        "UPDATE identities
            SET label = ?2, username = ?3, auth_type = ?4, key_path = ?5, key_id = ?6,
                password_secret_id = ?7,
                hlc_wall_ms = ?8, hlc_counter = ?9, hlc_device = ?10,
                rev = rev + 1
          WHERE id = ?1",
        params![
            id,
            identity.label,
            identity.username,
            identity.auth.as_str(),
            identity.key_path,
            identity.key_id,
            identity.password_secret_id,
            clock.wall_ms as i64,
            clock.counter,
            clock.device,
        ],
    )?;
    Ok(())
}

/// Make sure a group record exists for a group a host names.
pub(crate) fn ensure_group(
    tx: &Transaction,
    device: u32,
    vault: &str,
    workspace: Workspace,
    name: &str,
) -> Result<()> {
    let exists: i64 = tx.query_row(
        "SELECT count(*) FROM host_groups WHERE workspace = ?1 AND name = ?2 AND deleted = 0",
        params![workspace.as_str(), name],
        |row| row.get(0),
    )?;
    if exists > 0 {
        return Ok(());
    }
    let position: i64 = tx.query_row(
        "SELECT coalesce(max(position) + 1, 0) FROM host_groups
          WHERE workspace = ?1 AND deleted = 0",
        [workspace.as_str()],
        |row| row.get(0),
    )?;
    let clock = tick(tx, device)?;
    tx.execute(
        "INSERT INTO host_groups
            (id, vault_id, workspace, name, position, hlc_wall_ms, hlc_counter, hlc_device)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            Uuid::now_v7().to_string(),
            vault,
            workspace.as_str(),
            name,
            position,
            clock.wall_ms as i64,
            clock.counter,
            clock.device,
        ],
    )?;
    Ok(())
}

/// The position after the last host of a group.
pub(crate) fn next_position(
    tx: &Transaction,
    workspace: Workspace,
    group: Option<&str>,
) -> Result<i64> {
    Ok(tx.query_row(
        "SELECT coalesce(max(position) + 1, 0) FROM hosts
          WHERE deleted = 0 AND workspace = ?1 AND group_path IS ?2",
        params![workspace.as_str(), group],
        |row| row.get(0),
    )?)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use uwussh_vault::KdfParams;

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
            workspace: None,
            key_id: None,
            password: PasswordChange::Keep,
        }
    }

    fn store() -> Store {
        Store::open_in_memory().unwrap()
    }

    fn unlocked() -> Store {
        let store = store();
        store
            .create_vault_with(b"master", KdfParams::INSECURE_FOR_TESTS)
            .unwrap();
        store
    }

    fn live_secrets(store: &Store) -> i64 {
        store
            .conn
            .lock()
            .query_row("SELECT count(*) FROM secrets WHERE deleted = 0", [], |r| {
                r.get(0)
            })
            .unwrap()
    }

    #[test]
    fn a_saved_host_comes_back_as_saved() {
        let store = store();
        let saved = store.save_host(draft("prox-1", "10.0.0.12")).unwrap();
        assert_eq!(store.list_hosts().unwrap(), vec![saved.clone()]);
        assert_eq!(store.get_host(saved.id).unwrap(), Some(saved.clone()));
        assert_eq!(saved.workspace, Workspace::Private);
        assert!(!saved.has_password);
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

        let mut unknown_key = draft("x", "10.0.0.1");
        unknown_key.auth = AuthMethod::Key;
        unknown_key.key_id = Some(Uuid::now_v7());
        assert!(matches!(
            store.save_host(unknown_key),
            Err(StoreError::Invalid {
                field: "keyId",
                problem: "unknown"
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
        store.set_host_os(saved.id, Some("debian")).unwrap();

        let host = store.get_host(saved.id).unwrap().unwrap();
        assert!(host.last_connected_ms.is_some());
        assert_eq!(host.os.as_deref(), Some("debian"));
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

    /// Point `host` at the identity `owner` logs in with, the way an import
    /// shares one login between hosts.
    fn share_identity(store: &Store, owner: Uuid, host: Uuid) {
        store
            .conn
            .lock()
            .execute(
                "UPDATE hosts SET identity_id = (SELECT identity_id FROM hosts WHERE id = ?1)
                  WHERE id = ?2",
                params![owner.to_string(), host.to_string()],
            )
            .unwrap();
    }

    #[test]
    fn hosts_sharing_a_login_never_change_each_other() {
        let store = unlocked();
        let mut first = draft("a", "10.0.0.1");
        first.password = PasswordChange::Set {
            value: SecretText::new("shared"),
        };
        let a = store.save_host(first).unwrap();
        let b = store.save_host(draft("b", "10.0.0.2")).unwrap();
        let c = store.save_host(draft("c", "10.0.0.3")).unwrap();
        share_identity(&store, a.id, b.id);
        share_identity(&store, a.id, c.id);
        let password = |id| store.reveal_host_password(id).map(|p| p.to_vec());

        // A new password for b is b's alone.
        store
            .set_host_password(
                b.id,
                PasswordChange::Set {
                    value: SecretText::new("only-b"),
                },
            )
            .unwrap();
        assert_eq!(password(a.id).unwrap(), b"shared");
        assert_eq!(password(c.id).unwrap(), b"shared");
        assert_eq!(password(b.id).unwrap(), b"only-b");

        // Editing a's user, and forgetting c's password, leave the others be.
        let mut edit = draft("a", "10.0.0.1");
        edit.id = Some(a.id);
        edit.username = "admin".into();
        store.save_host(edit).unwrap();
        store
            .set_host_password(c.id, PasswordChange::Forget)
            .unwrap();
        let listed = store.list_hosts().unwrap();
        let named = |id| listed.iter().find(|h| h.id == id).unwrap().clone();
        assert_eq!(named(a.id).username, "admin");
        assert_eq!(named(c.id).username, "root");
        assert!(!named(c.id).has_password);
        assert_eq!(password(a.id).unwrap(), b"shared");

        // Deleting a host keeps a login another host still uses.
        share_identity(&store, b.id, c.id);
        store.delete_host(b.id).unwrap();
        let c_now = store
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|h| h.id == c.id)
            .unwrap();
        assert_eq!(c_now.username, "root");
        assert_eq!(password(c.id).unwrap(), b"only-b");

        // The last host with a login takes it along, password included.
        let before = live_secrets(&store);
        store.delete_host(a.id).unwrap();
        assert_eq!(live_secrets(&store), before - 1);
    }

    #[test]
    fn a_password_set_in_the_form_is_sealed_and_can_be_revealed() {
        let store = unlocked();
        let mut new = draft("web", "10.0.0.5");
        new.password = PasswordChange::Set {
            value: SecretText::new("hunter2"),
        };
        let saved = store.save_host(new).unwrap();
        assert!(saved.has_password);
        assert_eq!(
            store.host_credential_source(saved.id).unwrap(),
            crate::CredentialSource::VaultPassword
        );
        assert_eq!(
            store.reveal_host_password(saved.id).unwrap().as_slice(),
            b"hunter2"
        );
        let plaintext: i64 = store
            .conn
            .lock()
            .query_row(
                "SELECT count(*) FROM secrets WHERE instr(blob, 'hunter2') > 0",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(plaintext, 0, "only ciphertext reaches SQLite");
    }

    #[test]
    fn setting_a_password_needs_the_vault_and_writes_nothing_without_it() {
        let store = store();
        let mut new = draft("web", "10.0.0.5");
        new.password = PasswordChange::Set {
            value: SecretText::new("hunter2"),
        };
        assert!(matches!(store.save_host(new), Err(StoreError::VaultLocked)));
        assert!(store.list_hosts().unwrap().is_empty());
    }

    #[test]
    fn keeping_replacing_and_forgetting_a_password() {
        let store = unlocked();
        let mut new = draft("web", "10.0.0.5");
        new.password = PasswordChange::Set {
            value: SecretText::new("one"),
        };
        let saved = store.save_host(new).unwrap();

        // An edit that doesn't touch the password keeps it, even locked.
        store.lock_vault();
        let mut rename = draft("web-1", "10.0.0.5");
        rename.id = Some(saved.id);
        assert!(store.save_host(rename).unwrap().has_password);
        store.unlock_vault(b"master").unwrap();

        let mut replace = draft("web-1", "10.0.0.5");
        replace.id = Some(saved.id);
        replace.password = PasswordChange::Set {
            value: SecretText::new("two"),
        };
        store.save_host(replace).unwrap();
        assert_eq!(
            store.reveal_host_password(saved.id).unwrap().as_slice(),
            b"two"
        );
        assert_eq!(live_secrets(&store), 1, "the old password is gone");

        let mut forget = draft("web-1", "10.0.0.5");
        forget.id = Some(saved.id);
        forget.password = PasswordChange::Forget;
        let forgotten = store.save_host(forget).unwrap();
        assert!(!forgotten.has_password);
        assert_eq!(live_secrets(&store), 0);
        let leftover: i64 = store
            .conn
            .lock()
            .query_row(
                "SELECT count(*) FROM secrets WHERE length(blob) > 0",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            leftover, 0,
            "not even the ciphertext of an old password stays"
        );
    }

    #[test]
    fn a_key_host_can_keep_a_password_for_sudo() {
        let store = unlocked();
        let mut keyed = draft("pve", "10.0.0.6");
        keyed.auth = AuthMethod::Key;
        keyed.key_path = Some("~/.ssh/id_ed25519".into());
        keyed.password = PasswordChange::Set {
            value: SecretText::new("sudo-pw"),
        };
        let saved = store.save_host(keyed).unwrap();
        assert_eq!(
            store.host_credential_source(saved.id).unwrap(),
            crate::CredentialSource::KeyFile {
                path: "~/.ssh/id_ed25519".into()
            },
            "the key logs in"
        );
        assert_eq!(
            store.reveal_host_password(saved.id).unwrap().as_slice(),
            b"sudo-pw",
            "the password is there for sudo"
        );
    }

    #[test]
    fn a_password_that_worked_can_be_saved_afterwards() {
        let store = unlocked();
        let saved = store.save_host(draft("web", "10.0.0.5")).unwrap();
        let updated = store
            .set_host_password(
                saved.id,
                PasswordChange::Set {
                    value: SecretText::new("typed-at-login"),
                },
            )
            .unwrap();
        assert!(updated.has_password);
        assert_eq!(
            store.reveal_host_password(saved.id).unwrap().as_slice(),
            b"typed-at-login"
        );
        let forgotten = store
            .set_host_password(saved.id, PasswordChange::Forget)
            .unwrap();
        assert!(!forgotten.has_password);
        assert_eq!(live_secrets(&store), 0);
    }

    #[test]
    fn deleting_a_host_forgets_its_password() {
        let store = unlocked();
        let mut new = draft("web", "10.0.0.5");
        new.password = PasswordChange::Set {
            value: SecretText::new("hunter2"),
        };
        let saved = store.save_host(new).unwrap();
        store.delete_host(saved.id).unwrap();
        assert_eq!(live_secrets(&store), 0);
    }

    #[test]
    fn a_new_host_joins_its_group_at_the_end_and_creates_the_group() {
        let store = store();
        let mut a = draft("a", "10.0.0.1");
        a.group_path = Some("Homelab".into());
        a.workspace = Some(Workspace::Business);
        let a = store.save_host(a).unwrap();
        let mut b = draft("b", "10.0.0.2");
        b.group_path = Some("Homelab".into());
        b.workspace = Some(Workspace::Business);
        let b = store.save_host(b).unwrap();

        assert_eq!((a.position, b.position), (0, 1));
        assert_eq!(b.workspace, Workspace::Business);
        let groups = store.list_groups().unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "Homelab");
        assert_eq!(groups[0].workspace, Workspace::Business);

        // An edit without a workspace keeps the host where it is.
        let mut edit = draft("b2", "10.0.0.2");
        edit.id = Some(b.id);
        edit.group_path = Some("Homelab".into());
        let edited = store.save_host(edit).unwrap();
        assert_eq!(
            (edited.workspace, edited.position),
            (Workspace::Business, 1)
        );
    }
}
