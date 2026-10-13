//! Several stores side by side: one per server.
//!
//! A space is one store file. The main one is the database UwUSSH always had;
//! a second server is a second [`Store`] in `spaces/<uuid>.db` beside it, with
//! its own vault, its own sync state and its own cursor. Nothing about how a
//! store syncs or seals changes — this module is everything that reaches
//! across two of them:
//!
//! - the **registry**, a small file beside the database that says which file
//!   is main, which others there are, where new entries of a workspace go, and
//!   which host is half way between two stores;
//! - the **key chain**: a store that is not main keeps its vault key in its
//!   `device_unlock` row, sealed by the main vault's key instead of by the
//!   operating system, so one password opens all of them;
//! - **moving a host** from one store to the other with everything it needs;
//! - the lists the interface shows as one: hosts, groups, keys, tunnels.
//!
//! Personal things — snippets, key bindings, the command assistant — are only
//! ever read and written in the main store; callers use [`Spaces::main`].

use crate::backup::{
    Backup, BackupGroup, BackupHost, BackupKey, BackupKnownHost, BackupSnippet, BackupTunnel,
};
use crate::credentials::RevealedKey;
use crate::groups::GroupRecord;
use crate::hosts::{ensure_group, group_name, HostDraft, HostRecord, PasswordChange, Workspace};
use crate::import::{
    GroupInput, HostInput, IdentityInput, ImportOutcome, ImportSet, KeyInput, KnownHostInput,
    SnippetInput,
};
use crate::keys::{KeyDraft, KeyRecord};
use crate::known_hosts::KnownHostRecord;
use crate::secret::SecretText;
use crate::sync::clock_of;
use crate::tunnels::{TunnelDraft, TunnelRecord};
use crate::vault::{forget_secret, seal_secret, truncate_wal, Revealed};
use crate::{tick, vault_id, Result, Store, StoreError, VaultStatus};
use parking_lot::{Mutex, RwLock};
use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;
use uwussh_proto::{AssistConfigPayload, EntityKind, Hlc, KeymapPayload};
use uwussh_vault::{Sealed, UnlockedVault};
use zeroize::Zeroizing;

/// The id of the database UwUSSH starts with. Ids are opaque to everything
/// outside this module.
const FIRST: &str = "first";

/// What the interface calls the server that is being set up. Never the id of
/// a space.
pub const NEW_SPACE: &str = "new";

/// The file name the default database has; its registry is `spaces.json`.
const DEFAULT_BASE: &str = "uwussh.db";

const REGISTRY_VERSION: u32 = 1;

/// Mixed into the id a chained key is sealed under, so no record of the main
/// vault can share its associated data.
const CHAIN: &[u8] = b"uwussh/space-chain/v1\0";

/// How many servers there can be.
pub const MAX_SERVERS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SpaceKind {
    /// A store that syncs with a server of its own.
    Server,
    /// A team's vault, reached through the connection of the server space
    /// it is a child of.
    Team,
}

/// What this account may do in a team. The server enforces it; here it
/// decides whether a team's space takes writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TeamRole {
    Owner,
    Admin,
    Editor,
    Reader,
}

impl TeamRole {
    /// Whether this role pushes records.
    pub fn writes(self) -> bool {
        !matches!(self, Self::Reader)
    }

    /// Whether this role invites, confirms, renames and rotates.
    pub fn manages(self) -> bool {
        matches!(self, Self::Owner | Self::Admin)
    }
}

/// The longest team name kept, in characters: the server's bound.
const MAX_TEAM_NAME: usize = 80;

/// A team's name as it is kept and shown: no control characters, no more
/// than the server allows.
fn clean_label(label: &str) -> String {
    label
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_TEAM_NAME)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Where a new entry of each workspace goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceDefaults {
    pub private: String,
    pub business: String,
}

/// One space, with its store.
#[derive(Clone)]
pub struct SpaceHandle {
    pub id: String,
    pub main: bool,
    pub kind: SpaceKind,
    pub read_only: bool,
    /// For a team: the server space it is reached through.
    pub parent: Option<String>,
    /// For a team: its id on that server.
    pub team: Option<Uuid>,
    /// For a team: its name.
    pub label: Option<String>,
    /// For a team: what this account may do in it.
    pub role: Option<TeamRole>,
    pub store: Arc<Store>,
}

/// A host with the space it lives in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SpaceHost {
    #[serde(flatten)]
    pub host: HostRecord,
    pub space: String,
}

/// What the host form submits: the draft, and the server it should be on.
/// Without one, a new host goes to its workspace's default and an existing
/// one stays where it is.
#[derive(Debug, Clone, Deserialize)]
pub struct SpaceHostDraft {
    #[serde(flatten)]
    pub draft: HostDraft,
    #[serde(default)]
    pub space: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegistryEntry {
    id: String,
    /// The database's own file name, or `spaces/<uuid>.db`. Nothing else is
    /// ever turned into a path.
    file: String,
    kind: SpaceKind,
    #[serde(default)]
    read_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    team: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    role: Option<TeamRole>,
}

/// A host on its way from one store to another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Journal {
    host: Uuid,
    /// The id the host has in the target: its own, unless a tombstone there
    /// already holds that.
    target_host: Uuid,
    from: String,
    to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegistryFile {
    version: u32,
    main: String,
    spaces: Vec<RegistryEntry>,
    defaults: SpaceDefaults,
    #[serde(default)]
    journal: Option<Journal>,
    /// The server that was main until the swap this file was written by, for
    /// as long as the keys kept on this device are not all in their new
    /// place. See [`Spaces::make_main`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    handover: Option<String>,
    /// A server whose entries began to move away because it is being taken
    /// from this device, and are not all gone yet. See [`Spaces::remove`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    leaving: Option<String>,
}

struct Paths {
    base: PathBuf,
    base_name: String,
    registry: PathBuf,
    folder: PathBuf,
}

impl Paths {
    fn of(base: &Path) -> Result<Self> {
        let name = base
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| invalid("space", "path"))?;
        // The usual database gets the plain names. One somebody pointed
        // `UWUSSH_DB` at gets names of its own, so two databases in one
        // folder never read each other's registry.
        let (registry, folder) = if name == DEFAULT_BASE {
            ("spaces.json".to_string(), "spaces".to_string())
        } else {
            let stem = name.strip_suffix(".db").unwrap_or(name);
            (format!("{stem}.spaces.json"), format!("{stem}.spaces"))
        };
        Ok(Self {
            base: base.to_path_buf(),
            base_name: name.to_string(),
            registry: base.with_file_name(registry),
            folder: base.with_file_name(folder),
        })
    }

    /// The path a registry entry names — only ever the database itself or a
    /// `<uuid>.db` directly inside the spaces folder. A name from the file is
    /// never joined into a path as it stands.
    fn resolve(&self, file: &str) -> Option<PathBuf> {
        if file == self.base_name {
            return Some(self.base.clone());
        }
        let id = file.strip_prefix("spaces/")?.strip_suffix(".db")?;
        let uuid = Uuid::parse_str(id).ok()?;
        (uuid.hyphenated().to_string() == id).then(|| self.folder.join(format!("{uuid}.db")))
    }

    /// The file that says a store in the spaces folder is still being set
    /// up. Only a store with this beside it is ever cleared away unasked.
    fn marker(&self, id: &str) -> Option<PathBuf> {
        let uuid = Uuid::parse_str(id).ok()?;
        (uuid.hyphenated().to_string() == id).then(|| self.folder.join(format!("{uuid}.{PENDING}")))
    }

    /// Where an unreadable registry is set aside.
    fn aside(&self) -> PathBuf {
        let mut aside = self.registry.as_os_str().to_owned();
        aside.push(".unreadable");
        PathBuf::from(aside)
    }
}

/// The ending of the file [`Paths::marker`] names.
const PENDING: &str = "pending";

struct Entry {
    id: String,
    file: String,
    kind: SpaceKind,
    read_only: bool,
    parent: Option<String>,
    team: Option<Uuid>,
    label: Option<String>,
    role: Option<TeamRole>,
    store: Arc<Store>,
}

struct Inner {
    main: String,
    entries: Vec<Entry>,
    defaults: SpaceDefaults,
    journal: Option<Journal>,
    handover: Option<String>,
    leaving: Option<String>,
    /// The registry on disk could not be read. It is set aside, not
    /// overwritten, the next time there is something to write.
    unreadable: bool,
}

impl Inner {
    fn handle(&self, entry: &Entry) -> SpaceHandle {
        SpaceHandle {
            id: entry.id.clone(),
            main: entry.id == self.main,
            kind: entry.kind,
            read_only: entry.read_only,
            parent: entry.parent.clone(),
            team: entry.team,
            label: entry.label.clone(),
            role: entry.role,
            store: entry.store.clone(),
        }
    }

    fn file(&self) -> RegistryFile {
        RegistryFile {
            version: REGISTRY_VERSION,
            main: self.main.clone(),
            spaces: self
                .entries
                .iter()
                .map(|entry| RegistryEntry {
                    id: entry.id.clone(),
                    file: entry.file.clone(),
                    kind: entry.kind,
                    read_only: entry.read_only,
                    parent: entry.parent.clone(),
                    team: entry.team,
                    label: entry.label.clone(),
                    role: entry.role,
                })
                .collect(),
            defaults: self.defaults.clone(),
            journal: self.journal.clone(),
            handover: self.handover.clone(),
            leaving: self.leaving.clone(),
        }
    }
}

pub struct Spaces {
    paths: Paths,
    inner: RwLock<Inner>,
    /// One operation that spans stores at a time.
    ops: Mutex<()>,
}

fn invalid(field: &'static str, problem: &'static str) -> StoreError {
    StoreError::Invalid { field, problem }
}

fn unknown_space() -> StoreError {
    invalid("space", "unknown")
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id != NEW_SPACE
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Write the registry so that a crash leaves either the old one or the new
/// one: a temporary file beside it, synced, then renamed over it.
fn write_registry(path: &Path, file: &RegistryFile) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(file).map_err(std::io::Error::other)?;
    let mut name = path.as_os_str().to_owned();
    name.push(".tmp");
    let temporary = PathBuf::from(name);
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut out = options.open(&temporary)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            out.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        out.write_all(&bytes)?;
        out.sync_all()?;
    }
    std::fs::rename(&temporary, path)?;
    // The rename itself is only on disk once the folder is.
    #[cfg(unix)]
    if let Some(folder) = path
        .parent()
        .filter(|folder| !folder.as_os_str().is_empty())
    {
        std::fs::File::open(folder)?.sync_all()?;
    }
    Ok(())
}

/// The registry as it is on disk. `Ok(None)` when there is none.
fn read_registry(path: &Path) -> std::result::Result<Option<RegistryFile>, ()> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            tracing::warn!(%error, "the list of servers could not be read");
            return Err(());
        }
    };
    // A registry is a few hundred bytes. Something far larger is not one.
    if bytes.len() > 256 * 1024 {
        tracing::warn!("the list of servers is not a registry");
        return Err(());
    }
    match serde_json::from_slice::<RegistryFile>(&bytes) {
        Ok(file) if file.version == REGISTRY_VERSION => Ok(Some(file)),
        Ok(file) => {
            tracing::warn!(
                version = file.version,
                "the list of servers is from another build"
            );
            Err(())
        }
        Err(error) => {
            tracing::warn!(%error, "the list of servers does not read");
            Err(())
        }
    }
}

/// The id a space's key is sealed under by the main vault. Derived from the
/// space id, so a blob made for one space opens for no other.
fn chain_id(space: &str) -> Uuid {
    let digest = Sha256::new()
        .chain_update(CHAIN)
        .chain_update(space)
        .finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes)
}

impl Store {
    /// Seal another space's vault key with this vault's key, bound to that
    /// space's id. This vault being locked is an error that may pass.
    fn seal_for_space(&self, space: &str, key: &[u8]) -> std::io::Result<Vec<u8>> {
        let guard = self.vault.lock();
        let vault = guard
            .as_ref()
            .ok_or_else(|| std::io::Error::other("the main vault is locked"))?;
        let sealed = vault
            .seal(chain_id(space), EntityKind::Secret, key)
            .map_err(|_| std::io::Error::other("the key could not be sealed"))?;
        let nonce = u8::try_from(sealed.nonce.len())
            .map_err(|_| std::io::Error::other("the key could not be sealed"))?;
        let mut out = Vec::with_capacity(1 + sealed.nonce.len() + sealed.blob.len());
        out.push(nonce);
        out.extend_from_slice(&sealed.nonce);
        out.extend_from_slice(&sealed.blob);
        Ok(out)
    }

    /// Open what [`Self::seal_for_space`] made. `InvalidData` when it will
    /// never open — another space's blob, another main vault, a damaged one.
    fn open_for_space(&self, space: &str, blob: &[u8]) -> std::io::Result<Zeroizing<Vec<u8>>> {
        let never = || std::io::Error::new(std::io::ErrorKind::InvalidData, "not for this vault");
        let guard = self.vault.lock();
        let vault = guard
            .as_ref()
            .ok_or_else(|| std::io::Error::other("the main vault is locked"))?;
        let (&nonce, rest) = blob.split_first().ok_or_else(never)?;
        if rest.len() < nonce as usize {
            return Err(never());
        }
        let (nonce, blob) = rest.split_at(nonce as usize);
        vault
            .open(
                chain_id(space),
                EntityKind::Secret,
                &Sealed {
                    nonce: nonce.to_vec(),
                    blob: blob.to_vec(),
                },
            )
            .map_err(|_| never())
    }

    /// Put a host at a place: into a workspace and group (`None` keeps both)
    /// and at a position. Nothing is written when nothing changes, so a list
    /// renumbered across two stores only touches the rows that moved.
    pub fn place_host(
        &self,
        id: Uuid,
        place: Option<(Workspace, Option<&str>)>,
        position: i64,
    ) -> Result<HostRecord> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let current: Option<(String, Option<String>, i64)> = tx
            .query_row(
                "SELECT workspace, group_id, position FROM hosts WHERE id = ?1 AND deleted = 0",
                [id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let (workspace, group, old_position) = current.ok_or(StoreError::UnknownHost(id))?;
        let (to, to_group) = match place {
            Some((to, name)) => {
                let name = group_name(name.map(str::to_string))?;
                let group = match name {
                    Some(name) => {
                        let vault = vault_id(&tx)?;
                        Some(ensure_group(&tx, self.device, &vault, to, &name)?)
                    }
                    None => None,
                };
                (to, group)
            }
            None => (Workspace::parse(&workspace), group.clone()),
        };
        if Workspace::parse(&workspace) != to || group != to_group || old_position != position {
            let clock = tick(&tx, self.device)?;
            tx.execute(
                "UPDATE hosts
                    SET workspace = ?2, group_id = ?3, position = ?4, rev = rev + 1,
                        dirty = 1, hlc_wall_ms = ?5, hlc_counter = ?6, hlc_device = ?7
                  WHERE id = ?1",
                params![
                    id.to_string(),
                    to.as_str(),
                    to_group,
                    position,
                    clock.wall_ms as i64,
                    clock.counter,
                    clock.device
                ],
            )?;
        }
        let record = crate::hosts::read_host(&tx, id)?.ok_or(StoreError::UnknownHost(id))?;
        tx.commit()?;
        Ok(record)
    }
}

// ---------------------------------------------------------------------------
// A host in transit
// ---------------------------------------------------------------------------

/// A key with its sealed halves opened, between two vaults.
struct MovedKey {
    id: Uuid,
    label: String,
    key_type: String,
    public_key: String,
    private_key: Revealed,
    passphrase: Option<Revealed>,
    /// Fields a newer build wrote that this one does not know.
    extra: Option<String>,
}

/// A host with everything it takes along. The secrets are in memory that
/// wipes itself and live only for the length of the move.
struct MovedHost {
    host: HostRecord,
    password: Option<Revealed>,
    key: Option<MovedKey>,
    tunnels: Vec<TunnelRecord>,
    known: Option<KnownHostRecord>,
    /// Fields a newer build wrote that this one does not know: the host's,
    /// its login's, and each tunnel's by its id.
    extra: Option<String>,
    login_extra: Option<String>,
    tunnel_extra: HashMap<Uuid, String>,
}

/// What a newer build wrote into a row and this one only carries.
fn extra_of(conn: &rusqlite::Connection, table: &str, id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            &format!("SELECT sync_extra FROM {table} WHERE id = ?1"),
            [id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}

fn keep_extra(tx: &Transaction, table: &str, id: &str, extra: Option<&String>) -> Result<()> {
    if let Some(extra) = extra {
        tx.execute(
            &format!("UPDATE {table} SET sync_extra = ?2 WHERE id = ?1"),
            params![id, extra],
        )?;
    }
    Ok(())
}

/// Whether a row exists, and if so whether it is a tombstone.
fn row_state(tx: &Transaction, table: &str, id: &str) -> Result<Option<bool>> {
    Ok(tx
        .query_row(
            &format!("SELECT deleted FROM {table} WHERE id = ?1"),
            [id],
            |row| row.get(0),
        )
        .optional()?)
}

/// Put a key into this store, sealed under its vault, and say which id it has
/// here. A key that is already here is used as it is. Where a tombstone holds
/// the key's id — it was here once and deleted — the key gets a new one: a
/// tombstone beats every later write of the same record on every other
/// device, so bringing the old id back would only ever show here.
fn import_key(
    tx: &Transaction,
    device: u32,
    vault: &UnlockedVault,
    vault_uuid: &str,
    key: &MovedKey,
) -> Result<String> {
    let id = match row_state(tx, "keys", &key.id.to_string())? {
        Some(false) => return Ok(key.id.to_string()),
        None => key.id,
        Some(true) => {
            // The same key may have come back under a new id before.
            let twin: Option<String> = tx
                .query_row(
                    "SELECT id FROM keys
                      WHERE deleted = 0 AND public_key != '' AND public_key = ?1
                        AND key_type = ?2 AND label = ?3
                      ORDER BY id",
                    params![key.public_key, key.key_type, key.label],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(twin) = twin {
                return Ok(twin);
            }
            Uuid::now_v7()
        }
    };
    let private = seal_secret(tx, device, vault, &key.private_key)?;
    let passphrase = key
        .passphrase
        .as_ref()
        .map(|passphrase| seal_secret(tx, device, vault, passphrase))
        .transpose()?;
    let clock = tick(tx, device)?;
    tx.execute(
        "INSERT INTO keys
            (id, vault_id, label, key_type, public_key,
             private_secret_id, passphrase_secret_id,
             hlc_wall_ms, hlc_counter, hlc_device)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            id.to_string(),
            vault_uuid,
            key.label,
            key.key_type,
            key.public_key,
            private,
            passphrase,
            clock.wall_ms as i64,
            clock.counter,
            clock.device,
        ],
    )?;
    keep_extra(tx, "keys", &id.to_string(), key.extra.as_ref())?;
    Ok(id.to_string())
}

impl Store {
    fn export_moved_key(&self, id: Uuid) -> Result<MovedKey> {
        let record = self.get_key(id)?.ok_or(StoreError::UnknownKey(id))?;
        let RevealedKey {
            private_key,
            passphrase,
        } = self.reveal_key(id)?;
        let extra = extra_of(&self.conn.lock(), "keys", &id.to_string())?;
        Ok(MovedKey {
            extra,
            id,
            label: record.label,
            key_type: record.key_type,
            public_key: record.public_key,
            private_key,
            passphrase,
        })
    }

    fn import_moved_key(&self, key: &MovedKey) -> Result<Uuid> {
        let mut conn = self.conn.lock();
        let guard = self.vault.lock();
        let vault = guard.as_ref().ok_or(StoreError::VaultLocked)?;
        let tx = conn.transaction()?;
        let vault_uuid = vault_id(&tx)?;
        let id = import_key(&tx, self.device, vault, &vault_uuid, key)?;
        tx.commit()?;
        crate::sync::parse_uuid(&id)
    }

    /// Everything a host takes along when it moves. Needs the vault open when
    /// the host has a stored password or a vault key.
    fn export_moved_host(&self, id: Uuid) -> Result<MovedHost> {
        let host = self.get_host(id)?.ok_or(StoreError::UnknownHost(id))?;
        let password = if host.has_password {
            Some(self.reveal_host_password(id)?)
        } else {
            None
        };
        let key = match host.key_id {
            Some(key) => Some(self.export_moved_key(key)?),
            None => None,
        };
        let tunnels: Vec<TunnelRecord> = self
            .list_tunnels()?
            .into_iter()
            .filter(|tunnel| tunnel.host_id == id)
            .collect();
        let known = self.moved_known_host(&host.address, host.port)?;
        let (extra, login_extra, tunnel_extra) = {
            let conn = self.conn.lock();
            let login: Option<String> = conn
                .query_row(
                    "SELECT identity_id FROM hosts WHERE id = ?1",
                    [id.to_string()],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?
                .flatten();
            let mut of_tunnels = HashMap::new();
            for tunnel in &tunnels {
                if let Some(extra) = extra_of(&conn, "port_forwards", &tunnel.id.to_string())? {
                    of_tunnels.insert(tunnel.id, extra);
                }
            }
            (
                extra_of(&conn, "hosts", &id.to_string())?,
                match login {
                    Some(login) => extra_of(&conn, "identities", &login)?,
                    None => None,
                },
                of_tunnels,
            )
        };
        Ok(MovedHost {
            extra,
            login_extra,
            tunnel_extra,
            host,
            password,
            key,
            tunnels,
            known,
        })
    }

    /// The host key trusted for an address, whoever decided on it: a move
    /// takes the row as it is, not what a connection would be answered with.
    fn moved_known_host(&self, address: &str, port: u16) -> Result<Option<KnownHostRecord>> {
        Ok(self
            .conn
            .lock()
            .query_row(
                "SELECT address, port, algorithm, fingerprint_sha256, public_key, first_seen_ms
                   FROM known_hosts WHERE address = ?1 AND port = ?2 AND deleted = 0",
                params![address.trim().to_ascii_lowercase(), port],
                |row| {
                    Ok(KnownHostRecord {
                        address: row.get(0)?,
                        port: row.get::<_, i64>(1)? as u16,
                        algorithm: row.get(2)?,
                        fingerprint: row.get(3)?,
                        public_key: row.get(4)?,
                        first_seen_ms: row.get::<_, i64>(5)? as u64,
                    })
                },
            )
            .optional()?)
    }

    /// Whether a host row with this id exists here, and whether it is a
    /// tombstone.
    fn host_row(&self, id: Uuid) -> Result<Option<bool>> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        row_state(&tx, "hosts", &id.to_string())
    }

    /// Write a moved host into this store, in one transaction: the key (or
    /// the one already here), the login with its password sealed under this
    /// vault, the host under `id` at the place it had, its tunnels, and the
    /// host key unless this store already trusts one for the address.
    fn import_moved_host(&self, moved: &MovedHost, id: Uuid) -> Result<()> {
        let mut conn = self.conn.lock();
        let guard = self.vault.lock();
        let needs_vault = moved.password.is_some() || moved.key.is_some();
        if needs_vault && guard.is_none() {
            return Err(StoreError::VaultLocked);
        }
        let tx = conn.transaction()?;
        let vault_uuid = vault_id(&tx)?;
        let host = &moved.host;

        let key_id = match (&moved.key, guard.as_ref()) {
            (Some(key), Some(vault)) => {
                Some(import_key(&tx, self.device, vault, &vault_uuid, key)?)
            }
            _ => None,
        };
        let password_secret_id = match (&moved.password, guard.as_ref()) {
            (Some(password), Some(vault)) => Some(seal_secret(&tx, self.device, vault, password)?),
            _ => None,
        };
        drop(guard);

        let clock = tick(&tx, self.device)?;
        let identity = Uuid::now_v7().to_string();
        tx.execute(
            "INSERT INTO identities
                (id, vault_id, label, username, auth_type, key_path, key_id, password_secret_id,
                 hlc_wall_ms, hlc_counter, hlc_device)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                identity,
                vault_uuid,
                format!("{}@{}", host.username, host.address),
                host.username,
                host.auth.as_str(),
                host.key_path,
                key_id,
                password_secret_id,
                clock.wall_ms as i64,
                clock.counter,
                clock.device,
            ],
        )?;

        keep_extra(&tx, "identities", &identity, moved.login_extra.as_ref())?;

        let group = match &host.group_path {
            Some(name) => Some(ensure_group(
                &tx,
                self.device,
                &vault_uuid,
                host.workspace,
                name,
            )?),
            None => None,
        };
        tx.execute(
            "INSERT INTO hosts
                (id, vault_id, name, address, port, identity_id, group_id, workspace, position,
                 os_id, last_connected_ms, hlc_wall_ms, hlc_counter, hlc_device)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                id.to_string(),
                vault_uuid,
                host.name,
                host.address,
                host.port,
                identity,
                group,
                host.workspace.as_str(),
                host.position,
                host.os,
                host.last_connected_ms.map(|ms| ms as i64),
                clock.wall_ms as i64,
                clock.counter,
                clock.device,
            ],
        )?;

        keep_extra(&tx, "hosts", &id.to_string(), moved.extra.as_ref())?;

        for tunnel in &moved.tunnels {
            // A tunnel keeps its id unless a row here already holds it — its
            // own tombstone from an earlier stay, which no later write beats.
            let tunnel_id = match row_state(&tx, "port_forwards", &tunnel.id.to_string())? {
                None => tunnel.id,
                Some(_) => Uuid::now_v7(),
            };
            tx.execute(
                "INSERT INTO port_forwards
                    (id, vault_id, host_id, name, kind, bind_address, bind_port,
                     target_host, target_port, autostart,
                     hlc_wall_ms, hlc_counter, hlc_device)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    tunnel_id.to_string(),
                    vault_uuid,
                    id.to_string(),
                    tunnel.name,
                    tunnel.kind,
                    tunnel.bind_address,
                    tunnel.bind_port,
                    tunnel.target_host,
                    tunnel.target_port,
                    tunnel.autostart,
                    clock.wall_ms as i64,
                    clock.counter,
                    clock.device,
                ],
            )?;
            keep_extra(
                &tx,
                "port_forwards",
                &tunnel_id.to_string(),
                moved.tunnel_extra.get(&tunnel.id),
            )?;
        }

        if let Some(known) = &moved.known {
            // What this store already trusts for the address stays. Otherwise
            // the key comes along the way trusting it here would write it.
            let trusted: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM known_hosts
                                 WHERE address = ?1 AND port = ?2 AND deleted = 0)",
                params![known.address, known.port],
                |row| row.get(0),
            )?;
            if !trusted {
                tx.execute(
                    "INSERT INTO known_hosts
                        (id, vault_id, address, port, algorithm, fingerprint_sha256, public_key,
                         first_seen_ms, hlc_wall_ms, hlc_counter, hlc_device)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                     ON CONFLICT (address, port) DO UPDATE
                        SET algorithm = excluded.algorithm,
                            fingerprint_sha256 = excluded.fingerprint_sha256,
                            public_key = excluded.public_key,
                            first_seen_ms = excluded.first_seen_ms,
                            dirty = 1,
                            hlc_wall_ms = excluded.hlc_wall_ms,
                            hlc_counter = excluded.hlc_counter,
                            hlc_device = excluded.hlc_device,
                            rev = rev + 1,
                            deleted = 0",
                    params![
                        Uuid::now_v7().to_string(),
                        vault_uuid,
                        known.address,
                        known.port,
                        known.algorithm,
                        known.fingerprint,
                        known.public_key,
                        known.first_seen_ms as i64,
                        clock.wall_ms as i64,
                        clock.counter,
                        clock.device,
                    ],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Take a host that now lives in another store out of this one: the host
    /// with its login, password and tunnels, and its key and host key unless
    /// another host here still uses them. Needs no vault.
    fn drop_moved_host(&self, id: Uuid) -> Result<()> {
        // Read from the row whether or not the host is still there: stopped
        // after the host went and before the rest did, this is asked again,
        // and the row of a deleted host still says what it used.
        let row: Option<(Option<String>, String, u16)> = self
            .conn
            .lock()
            .query_row(
                "SELECT i.key_id, h.address, h.port FROM hosts h
                   LEFT JOIN identities i ON i.id = h.identity_id
                  WHERE h.id = ?1",
                [id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get::<_, i64>(2)? as u16)),
            )
            .optional()?;
        let Some((key, address, port)) = row else {
            return Ok(());
        };
        match self.delete_host(id) {
            Ok(()) | Err(StoreError::UnknownHost(_)) => {}
            Err(error) => return Err(error),
        }
        if let Some(key) = key.and_then(|key| Uuid::parse_str(&key).ok()) {
            match self.delete_key(key) {
                Ok(()) | Err(StoreError::KeyInUse { .. }) | Err(StoreError::UnknownKey(_)) => {}
                Err(error) => return Err(error),
            }
        }
        let address = address.trim().to_ascii_lowercase();
        let shared = self.list_hosts()?.iter().any(|other| {
            other.port == port && other.address.trim().to_ascii_lowercase() == address
        });
        if !shared {
            self.forget_host_key(&address, port)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Personal records, for a change of main
// ---------------------------------------------------------------------------

/// The tables that hold one JSON record under a fixed id.
const DOC_TABLES: [&str; 2] = ["keymaps", "assist_config"];

struct Doc {
    table: &'static str,
    id: String,
    body: String,
    clock: Hlc,
}

struct SnippetRow {
    id: String,
    label: String,
    body: String,
    group_path: Option<String>,
    clock: Hlc,
}

struct CacheRow {
    id: String,
    platform: String,
    request: String,
    normalized: String,
    command: String,
    explanation: String,
    dangerous: bool,
    created_ms: i64,
    used_ms: i64,
    hits: i64,
    clock: Hlc,
}

/// What belongs to the person rather than to a server, read out of the old
/// main store with its secrets opened.
struct Personal {
    snippets: Vec<SnippetRow>,
    docs: Vec<Doc>,
    cache: Vec<CacheRow>,
    secrets: HashMap<Uuid, Revealed>,
}

/// The secrets a record points at.
fn doc_secrets(table: &str, body: &str) -> Vec<Uuid> {
    match table {
        "keymaps" => serde_json::from_str::<KeymapPayload>(body)
            .map(|keymap| keymap.texts.values().filter_map(|t| t.secret_id).collect())
            .unwrap_or_default(),
        _ => serde_json::from_str::<AssistConfigPayload>(body)
            .map(|config| {
                config
                    .providers
                    .values()
                    .filter_map(|p| p.key_secret_id)
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// The record with its secrets pointing at their copies in another vault. One
/// that has no copy is let go of. `None` for a record that does not read: it
/// is carried as it is.
fn doc_with_secrets(table: &str, body: &str, copies: &HashMap<Uuid, Uuid>) -> Option<String> {
    match table {
        "keymaps" => {
            let mut keymap: KeymapPayload = serde_json::from_str(body).ok()?;
            for text in keymap.texts.values_mut() {
                text.secret_id = text.secret_id.and_then(|id| copies.get(&id).copied());
            }
            serde_json::to_string(&keymap).ok()
        }
        _ => {
            let mut config: AssistConfigPayload = serde_json::from_str(body).ok()?;
            for provider in config.providers.values_mut() {
                provider.key_secret_id = provider
                    .key_secret_id
                    .and_then(|id| copies.get(&id).copied());
            }
            serde_json::to_string(&config).ok()
        }
    }
}

/// The record as it is when nothing was ever set.
fn empty_doc(table: &str) -> String {
    match table {
        "keymaps" => serde_json::to_string(&KeymapPayload::default()),
        _ => serde_json::to_string(&AssistConfigPayload::default()),
    }
    .unwrap_or_else(|_| "{}".to_string())
}

fn doc_is_empty(table: &str, body: &str) -> bool {
    match table {
        "keymaps" => {
            serde_json::from_str::<KeymapPayload>(body).is_ok_and(|k| k == KeymapPayload::default())
        }
        _ => serde_json::from_str::<AssistConfigPayload>(body)
            .is_ok_and(|c| c == AssistConfigPayload::default()),
    }
}

fn write_doc(tx: &Transaction, device: u32, table: &str, id: &str, body: &str) -> Result<()> {
    let clock = tick(tx, device)?;
    tx.execute(
        &format!(
            "INSERT INTO {table}
                (id, vault_id, body, hlc_wall_ms, hlc_counter, hlc_device, deleted, dirty)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 1)
             ON CONFLICT (id) DO UPDATE SET
                body = excluded.body, hlc_wall_ms = excluded.hlc_wall_ms,
                hlc_counter = excluded.hlc_counter, hlc_device = excluded.hlc_device,
                dirty = 1, rev = rev + 1"
        ),
        params![
            id,
            vault_id(tx)?,
            body,
            clock.wall_ms as i64,
            clock.counter,
            clock.device,
        ],
    )?;
    Ok(())
}

impl Store {
    fn export_personal(&self) -> Result<Personal> {
        let mut sealed: Vec<(Uuid, Sealed)> = Vec::new();
        let (snippets, docs, cache) = {
            let conn = self.conn.lock();
            let snippets = conn
                .prepare(
                    "SELECT id, label, body, group_path, hlc_wall_ms, hlc_counter, hlc_device
                       FROM snippets WHERE deleted = 0",
                )?
                .query_map([], |row| {
                    Ok(SnippetRow {
                        id: row.get(0)?,
                        label: row.get(1)?,
                        body: row.get(2)?,
                        group_path: row.get(3)?,
                        clock: clock_of(row.get(4)?, row.get(5)?, row.get(6)?),
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut docs = Vec::new();
            for table in DOC_TABLES {
                let rows = conn
                    .prepare(&format!(
                        "SELECT id, body, hlc_wall_ms, hlc_counter, hlc_device
                           FROM {table} WHERE deleted = 0"
                    ))?
                    .query_map([], |row| {
                        Ok(Doc {
                            table,
                            id: row.get(0)?,
                            body: row.get(1)?,
                            clock: clock_of(row.get(2)?, row.get(3)?, row.get(4)?),
                        })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                docs.extend(
                    rows.into_iter()
                        .filter(|doc| !doc_is_empty(table, &doc.body)),
                );
            }
            for doc in &docs {
                for secret in doc_secrets(doc.table, &doc.body) {
                    let row = conn
                        .query_row(
                            "SELECT nonce, blob FROM secrets
                              WHERE id = ?1 AND deleted = 0 AND length(blob) > 0",
                            [secret.to_string()],
                            |row| {
                                Ok(Sealed {
                                    nonce: row.get(0)?,
                                    blob: row.get(1)?,
                                })
                            },
                        )
                        .optional()?;
                    if let Some(row) = row {
                        sealed.push((secret, row));
                    }
                }
            }
            let cache = conn
                .prepare(
                    "SELECT id, platform, request, normalized, command, explanation, dangerous,
                            created_ms, used_ms, hits, hlc_wall_ms, hlc_counter, hlc_device
                       FROM assist_cache WHERE deleted = 0 AND command != ''",
                )?
                .query_map([], |row| {
                    Ok(CacheRow {
                        id: row.get(0)?,
                        platform: row.get(1)?,
                        request: row.get(2)?,
                        normalized: row.get(3)?,
                        command: row.get(4)?,
                        explanation: row.get(5)?,
                        dangerous: row.get(6)?,
                        created_ms: row.get(7)?,
                        used_ms: row.get(8)?,
                        hits: row.get(9)?,
                        clock: clock_of(row.get(10)?, row.get(11)?, row.get(12)?),
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            (snippets, docs, cache)
        };

        let mut secrets = HashMap::new();
        if !sealed.is_empty() {
            let guard = self.vault.lock();
            let vault = guard.as_ref().ok_or(StoreError::VaultLocked)?;
            for (id, sealed) in sealed {
                // One that does not open is left behind, not a reason to stop.
                if let Ok(plain) = vault.open(id, EntityKind::Secret, &sealed) {
                    secrets.insert(id, plain);
                }
            }
        }
        Ok(Personal {
            snippets,
            docs,
            cache,
            secrets,
        })
    }

    /// Take the personal records of another store in, keeping their ids.
    /// Where this store has the same record, the newer one stays; a record
    /// that was only ever emptied here gives way.
    fn import_personal(&self, personal: &Personal) -> Result<()> {
        let mut conn = self.conn.lock();
        let guard = self.vault.lock();
        if !personal.secrets.is_empty() && guard.is_none() {
            return Err(StoreError::VaultLocked);
        }
        let tx = conn.transaction()?;
        let vault_uuid = vault_id(&tx)?;

        for snippet in &personal.snippets {
            let local: Option<(bool, i64, i64, i64)> = tx
                .query_row(
                    "SELECT deleted, hlc_wall_ms, hlc_counter, hlc_device
                       FROM snippets WHERE id = ?1",
                    [&snippet.id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()?;
            match local {
                Some((false, wall, counter, device)) => {
                    if snippet.clock > clock_of(wall, counter, device) {
                        let clock = tick(&tx, self.device)?;
                        tx.execute(
                            "UPDATE snippets
                                SET label = ?2, body = ?3, group_path = ?4, rev = rev + 1,
                                    dirty = 1, hlc_wall_ms = ?5, hlc_counter = ?6, hlc_device = ?7
                              WHERE id = ?1",
                            params![
                                snippet.id,
                                snippet.label,
                                snippet.body,
                                snippet.group_path,
                                clock.wall_ms as i64,
                                clock.counter,
                                clock.device
                            ],
                        )?;
                    }
                }
                state => {
                    // A tombstone here keeps its id for good; the snippet
                    // comes in under a new one, unless it is here already.
                    let id = if state.is_some() {
                        let twin: bool = tx.query_row(
                            "SELECT EXISTS (SELECT 1 FROM snippets
                                             WHERE deleted = 0 AND label = ?1 AND body = ?2)",
                            params![snippet.label, snippet.body],
                            |row| row.get(0),
                        )?;
                        if twin {
                            continue;
                        }
                        Uuid::now_v7().to_string()
                    } else {
                        snippet.id.clone()
                    };
                    let clock = tick(&tx, self.device)?;
                    tx.execute(
                        "INSERT INTO snippets
                            (id, vault_id, label, body, group_path,
                             hlc_wall_ms, hlc_counter, hlc_device)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        params![
                            id,
                            vault_uuid,
                            snippet.label,
                            snippet.body,
                            snippet.group_path,
                            clock.wall_ms as i64,
                            clock.counter,
                            clock.device,
                        ],
                    )?;
                }
            }
        }

        for doc in &personal.docs {
            let local: Option<(bool, String, i64, i64, i64)> = tx
                .query_row(
                    &format!(
                        "SELECT deleted, body, hlc_wall_ms, hlc_counter, hlc_device
                           FROM {} WHERE id = ?1",
                        doc.table
                    ),
                    [&doc.id],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    },
                )
                .optional()?;
            let old_body = match local {
                None => None,
                // Nothing written over a tombstone would ever reach another
                // device. These records are never deleted by UwUSSH itself.
                Some((true, ..)) => continue,
                Some((false, body, wall, counter, device)) => {
                    if !doc_is_empty(doc.table, &body)
                        && doc.clock <= clock_of(wall, counter, device)
                    {
                        continue;
                    }
                    Some(body)
                }
            };
            let mut copies = HashMap::new();
            if let Some(vault) = guard.as_ref() {
                for secret in doc_secrets(doc.table, &doc.body) {
                    if let Some(plain) = personal.secrets.get(&secret) {
                        let copy = seal_secret(&tx, self.device, vault, plain)?;
                        copies.insert(secret, crate::sync::parse_uuid(&copy)?);
                    }
                }
            }
            let body =
                doc_with_secrets(doc.table, &doc.body, &copies).unwrap_or_else(|| doc.body.clone());
            if let Some(old) = old_body {
                for secret in doc_secrets(doc.table, &old) {
                    forget_secret(&tx, self.device, &secret.to_string())?;
                }
            }
            write_doc(&tx, self.device, doc.table, &doc.id, &body)?;
        }

        for row in &personal.cache {
            let local: Option<(bool, String, i64, i64, i64)> = tx
                .query_row(
                    "SELECT deleted, command, hlc_wall_ms, hlc_counter, hlc_device
                       FROM assist_cache WHERE id = ?1",
                    [&row.id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )
                .optional()?;
            match local {
                Some((true, ..)) => continue,
                Some((false, command, wall, counter, device))
                    if !command.is_empty() && row.clock <= clock_of(wall, counter, device) =>
                {
                    continue
                }
                _ => {}
            }
            let clock = tick(&tx, self.device)?;
            tx.execute(
                "INSERT INTO assist_cache
                    (id, vault_id, platform, request, normalized, command, explanation,
                     dangerous, created_ms, used_ms, hits,
                     hlc_wall_ms, hlc_counter, hlc_device, deleted, dirty)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, 0, 1)
                 ON CONFLICT (id) DO UPDATE SET
                    platform = excluded.platform, request = excluded.request,
                    normalized = excluded.normalized, command = excluded.command,
                    explanation = excluded.explanation, dangerous = excluded.dangerous,
                    created_ms = excluded.created_ms, used_ms = excluded.used_ms,
                    hits = excluded.hits, hlc_wall_ms = excluded.hlc_wall_ms,
                    hlc_counter = excluded.hlc_counter, hlc_device = excluded.hlc_device,
                    dirty = 1, rev = rev + 1",
                params![
                    row.id,
                    vault_uuid,
                    row.platform,
                    row.request,
                    row.normalized,
                    row.command,
                    row.explanation,
                    row.dangerous,
                    row.created_ms,
                    row.used_ms,
                    row.hits,
                    clock.wall_ms as i64,
                    clock.counter,
                    clock.device,
                ],
            )?;
        }
        tx.commit()?;
        truncate_wal(&conn);
        Ok(())
    }

    /// Take the personal records out of a store that is no longer main.
    /// Snippets are deleted. The records under fixed ids — key bindings, the
    /// assistant's settings, its cache — are emptied instead: a tombstone
    /// there would keep them from ever being used again, should this server
    /// become the main one once more.
    fn clear_personal(&self) -> Result<()> {
        {
            let mut conn = self.conn.lock();
            let tx = conn.transaction()?;
            let clock = tick(&tx, self.device)?;
            tx.execute(
                "UPDATE snippets
                    SET deleted = 1, rev = rev + 1,
                        dirty = 1, hlc_wall_ms = ?1, hlc_counter = ?2, hlc_device = ?3
                  WHERE deleted = 0",
                params![clock.wall_ms as i64, clock.counter, clock.device],
            )?;
            for table in DOC_TABLES {
                let rows: Vec<(String, String)> = tx
                    .prepare(&format!("SELECT id, body FROM {table} WHERE deleted = 0"))?
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                for (id, body) in rows {
                    if doc_is_empty(table, &body) {
                        continue;
                    }
                    for secret in doc_secrets(table, &body) {
                        forget_secret(&tx, self.device, &secret.to_string())?;
                    }
                    write_doc(&tx, self.device, table, &id, &empty_doc(table))?;
                }
            }
            tx.commit()?;
            truncate_wal(&conn);
        }
        self.clear_assist_cache()?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Spaces
// ---------------------------------------------------------------------------

fn host_match(address: &str, port: u16, username: &str) -> (String, u16, String) {
    (
        address.trim().to_ascii_lowercase(),
        port,
        username.trim().to_string(),
    )
}

fn host_order(host: &HostRecord) -> (&'static str, String, i64, String, Uuid) {
    (
        host.workspace.as_str(),
        host.group_path.clone().unwrap_or_default().to_lowercase(),
        host.position,
        host.name.to_lowercase(),
        host.id,
    )
}

/// What of an import goes to one server, with where the file's logins and
/// keys ended up in it.
#[derive(Default)]
struct ImportPart {
    set: ImportSet,
    identities: HashMap<usize, usize>,
    keys: HashMap<usize, usize>,
}

fn add_outcome(total: &mut ImportOutcome, part: ImportOutcome) {
    total.hosts_added += part.hosts_added;
    total.hosts_skipped += part.hosts_skipped;
    total.identities_added += part.identities_added;
    total.keys_added += part.keys_added;
    total.known_hosts_added += part.known_hosts_added;
    total.snippets_added += part.snippets_added;
    total.tunnels_added += part.tunnels_added;
}

fn copy_secret(secret: &SecretText) -> SecretText {
    SecretText::new(secret.expose())
}

fn copy_backup_key(key: &BackupKey) -> BackupKey {
    BackupKey {
        label: key.label.clone(),
        key_type: key.key_type.clone(),
        public_key: key.public_key.clone(),
        private_key: key.private_key.as_ref().map(copy_secret),
        passphrase: key.passphrase.as_ref().map(copy_secret),
    }
}

fn copy_key_input(key: &KeyInput) -> KeyInput {
    KeyInput {
        label: key.label.clone(),
        key_type: key.key_type.clone(),
        public_key: key.public_key.clone(),
        private_key: key.private_key.clone(),
        passphrase: key.passphrase.clone(),
    }
}

impl Spaces {
    /// Open the database at `base` and every other space its registry names.
    /// Without a registry there is one space, as there always was.
    pub fn open(base: &Path) -> Result<Self> {
        let paths = Paths::of(base)?;
        let read = read_registry(&paths.registry);
        let mut unreadable = read.is_err();
        let single = || RegistryFile {
            version: REGISTRY_VERSION,
            main: FIRST.to_string(),
            spaces: vec![RegistryEntry {
                id: FIRST.to_string(),
                file: paths.base_name.clone(),
                kind: SpaceKind::Server,
                read_only: false,
                parent: None,
                team: None,
                label: None,
                role: None,
            }],
            defaults: SpaceDefaults {
                private: FIRST.to_string(),
                business: FIRST.to_string(),
            },
            journal: None,
            handover: None,
            leaving: None,
        };
        let mut file = read.ok().flatten().unwrap_or_else(single);

        // Only entries whose id and file are what this module would have
        // written are opened. The rest is left alone on disk.
        let mut planned: Vec<(RegistryEntry, PathBuf)> = Vec::new();
        for entry in std::mem::take(&mut file.spaces) {
            let Some(path) = paths.resolve(&entry.file) else {
                tracing::warn!("a server in the list names a file that is not one of ours");
                unreadable = true;
                continue;
            };
            if !valid_id(&entry.id)
                || planned
                    .iter()
                    .any(|(other, at)| other.id == entry.id || *at == path)
            {
                tracing::warn!("a server in the list is there twice or has no usable id");
                unreadable = true;
                continue;
            }
            planned.push((entry, path));
        }
        // A team hangs on a server that is in the list, and names its team.
        // One that does not is left alone on disk like any other stray.
        let servers: HashSet<String> = planned
            .iter()
            .filter(|(entry, _)| entry.kind == SpaceKind::Server)
            .map(|(entry, _)| entry.id.clone())
            .collect();
        planned.retain(|(entry, _)| match entry.kind {
            SpaceKind::Server => true,
            SpaceKind::Team => {
                let whole = entry.team.is_some()
                    && entry.role.is_some()
                    && entry
                        .parent
                        .as_ref()
                        .is_some_and(|parent| servers.contains(parent));
                if !whole {
                    tracing::warn!("a team in the list belongs to no server, left out");
                }
                whole
            }
        });
        if !planned
            .iter()
            .any(|(entry, _)| entry.id == file.main && entry.kind == SpaceKind::Server)
        {
            tracing::warn!("the list of servers names no main one, starting with one server");
            unreadable = true;
            file = single();
            planned = file
                .spaces
                .drain(..)
                .map(|entry| (entry, paths.base.clone()))
                .collect();
        }

        let mut entries = Vec::with_capacity(planned.len());
        for (entry, path) in planned {
            entries.push(Entry {
                store: Arc::new(Store::open(&path)?),
                id: entry.id,
                file: entry.file,
                kind: entry.kind,
                read_only: match entry.kind {
                    SpaceKind::Server => entry.read_only,
                    SpaceKind::Team => !entry.role.is_some_and(TeamRole::writes),
                },
                parent: entry.parent.filter(|_| entry.kind == SpaceKind::Team),
                team: entry.team.filter(|_| entry.kind == SpaceKind::Team),
                label: entry.label.map(|label| clean_label(&label)),
                role: entry.role.filter(|_| entry.kind == SpaceKind::Team),
            });
        }
        let known = |id: &String| {
            entries
                .iter()
                .any(|entry| entry.id == *id && entry.kind == SpaceKind::Server)
        };
        let any = |id: &String| entries.iter().any(|entry| entry.id == *id);
        let defaults = SpaceDefaults {
            private: Some(file.defaults.private)
                .filter(known)
                .unwrap_or_else(|| file.main.clone()),
            business: Some(file.defaults.business)
                .filter(known)
                .unwrap_or_else(|| file.main.clone()),
        };
        let journal = file
            .journal
            .filter(|journal| journal.from != journal.to && any(&journal.from) && any(&journal.to));

        let handover = file.handover.filter(|old| *old != file.main && known(old));

        let leaving = file.leaving.filter(known);

        let spaces = Self {
            inner: RwLock::new(Inner {
                main: file.main,
                entries,
                defaults,
                journal,
                handover,
                leaving,
                unreadable,
            }),
            paths,
            ops: Mutex::new(()),
        };
        if !unreadable {
            spaces.sweep_leftovers();
        }
        {
            let _ops = spaces.ops.lock();
            spaces.settle_handover()?;
            spaces.recover()?;
        }
        Ok(spaces)
    }

    /// Stores in the spaces folder whose setup never finished: the ones that
    /// still have their marker beside them and that no registry entry names.
    /// A store file without a marker is never touched, whoever it belongs to
    /// — and nothing is while a registry that could not be read lies beside
    /// the one in use, because that one may name what this one forgot.
    fn sweep_leftovers(&self) {
        if self.paths.aside().exists() {
            return;
        }
        let Ok(folder) = std::fs::read_dir(&self.paths.folder) else {
            return;
        };
        let kept: HashSet<PathBuf> = self
            .inner
            .read()
            .entries
            .iter()
            .filter_map(|entry| self.paths.resolve(&entry.file))
            .collect();
        for file in folder.flatten() {
            let name = file.file_name();
            let Some(id) = name
                .to_str()
                .and_then(|name| name.strip_suffix(&format!(".{PENDING}")))
            else {
                continue;
            };
            let Some(path) = self.paths.resolve(&format!("spaces/{id}.db")) else {
                continue;
            };
            if !kept.contains(&path) {
                tracing::info!("a server that was never set up to the end is cleared away");
                remove_store_files(&path);
            }
            let _ = std::fs::remove_file(file.path());
        }
    }

    fn persist(&self, inner: &mut Inner) -> Result<()> {
        if inner.unreadable {
            let _ = std::fs::rename(&self.paths.registry, self.paths.aside());
            inner.unreadable = false;
        }
        write_registry(&self.paths.registry, &inner.file())?;
        Ok(())
    }

    // -- which spaces there are ------------------------------------------

    pub fn main(&self) -> Arc<Store> {
        let inner = self.inner.read();
        inner
            .entries
            .iter()
            .find(|entry| entry.id == inner.main)
            .or(inner.entries.first())
            .map(|entry| entry.store.clone())
            .expect("there is always a main space")
    }

    pub fn main_id(&self) -> String {
        self.inner.read().main.clone()
    }

    /// Every space, the main one first.
    pub fn all(&self) -> Vec<SpaceHandle> {
        let inner = self.inner.read();
        let mut all: Vec<SpaceHandle> = inner.entries.iter().map(|e| inner.handle(e)).collect();
        all.sort_by_key(|space| !space.main);
        all
    }

    pub fn get(&self, id: &str) -> Option<SpaceHandle> {
        let inner = self.inner.read();
        inner
            .entries
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| inner.handle(entry))
    }

    /// The store of a space; without an id, the main one.
    pub fn store(&self, id: Option<&str>) -> Result<Arc<Store>> {
        match id {
            None => Ok(self.main()),
            Some(id) => Ok(self.get(id).ok_or_else(unknown_space)?.store),
        }
    }

    /// The one other space there is beside `id`, while there are two.
    pub fn other(&self, id: &str) -> Option<SpaceHandle> {
        self.all().into_iter().find(|space| space.id != id)
    }

    pub fn defaults(&self) -> SpaceDefaults {
        self.inner.read().defaults.clone()
    }

    pub fn default_for(&self, workspace: Workspace) -> SpaceHandle {
        let defaults = self.defaults();
        let id = match workspace {
            Workspace::Private => defaults.private,
            Workspace::Business => defaults.business,
        };
        self.get(&id)
            .or_else(|| self.all().into_iter().next())
            .expect("there is always a main space")
    }

    pub fn set_default(&self, workspace: Workspace, id: &str) -> Result<()> {
        let mut inner = self.inner.write();
        if !inner.entries.iter().any(|entry| entry.id == id) {
            return Err(unknown_space());
        }
        match workspace {
            Workspace::Private => inner.defaults.private = id.to_string(),
            Workspace::Business => inner.defaults.business = id.to_string(),
        }
        self.persist(&mut inner)
    }

    /// Whether another server may be added: there is room for one.
    pub fn has_room(&self) -> bool {
        let inner = self.inner.read();
        inner
            .entries
            .iter()
            .filter(|entry| entry.kind == SpaceKind::Server)
            .count()
            < MAX_SERVERS
    }

    // -- adding a space ---------------------------------------------------

    /// A new, empty store for a server that is being set up. It is not a
    /// space yet: [`Self::register`] makes it one, [`Self::discard`] throws
    /// it away, and one that is neither is cleared away on the next start.
    pub fn prepare(&self) -> Result<(String, Arc<Store>)> {
        if !self.has_room() {
            return Err(invalid("space", "limit"));
        }
        let id = Uuid::new_v4().to_string();
        Ok((id.clone(), Arc::new(self.begin_store(&id)?)))
    }

    /// A new store file in the spaces folder, marked as being set up before
    /// it exists.
    fn begin_store(&self, id: &str) -> Result<Store> {
        let path = self
            .paths
            .resolve(&format!("spaces/{id}.db"))
            .ok_or_else(unknown_space)?;
        let marker = self.paths.marker(id).ok_or_else(unknown_space)?;
        std::fs::create_dir_all(&self.paths.folder)?;
        std::fs::write(&marker, b"")?;
        Store::open(&path)
    }

    /// A store that is in the registry is no longer being set up.
    fn settled(&self, id: &str) {
        if let Some(marker) = self.paths.marker(id) {
            let _ = std::fs::remove_file(marker);
        }
    }

    /// Make a prepared store the second server and chain its key.
    pub fn register(&self, id: &str, store: Arc<Store>) -> Result<()> {
        let _ops = self.ops.lock();
        let file = format!("spaces/{id}.db");
        if !valid_id(id) || self.paths.resolve(&file).is_none() {
            return Err(unknown_space());
        }
        {
            let mut inner = self.inner.write();
            let servers = inner
                .entries
                .iter()
                .filter(|entry| entry.kind == SpaceKind::Server)
                .count();
            if servers >= MAX_SERVERS {
                return Err(invalid("space", "limit"));
            }
            if inner.entries.iter().any(|entry| entry.id == id) {
                return Err(invalid("space", "exists"));
            }
            inner.entries.push(Entry {
                id: id.to_string(),
                file,
                kind: SpaceKind::Server,
                read_only: false,
                parent: None,
                team: None,
                label: None,
                role: None,
                store,
            });
            if let Err(error) = self.persist(&mut inner) {
                inner.entries.retain(|entry| entry.id != id);
                return Err(error);
            }
        }
        self.settled(id);
        self.reseal_chain();
        Ok(())
    }

    /// Throw a prepared store away. The caller drops its own handle first.
    pub fn discard(&self, id: &str) {
        if self.get(id).is_some() {
            return;
        }
        if let Some(path) = self.paths.resolve(&format!("spaces/{id}.db")) {
            remove_store_files(&path);
        }
        self.settled(id);
    }

    // -- one password -----------------------------------------------------

    /// Whether a space's vault could not be opened with the main key: main is
    /// open, this one is not.
    pub fn is_locked(&self, id: &str) -> bool {
        let Some(space) = self.get(id) else {
            return false;
        };
        if space.main {
            return false;
        }
        let open = |store: &Store| matches!(store.vault_status(), Ok(VaultStatus::Unlocked));
        open(&self.main()) && matches!(space.store.vault_status(), Ok(VaultStatus::Locked))
    }

    /// After the main vault was opened: open every other one with the key
    /// kept for it. One whose chain does not open stays locked and says so.
    pub fn unlock_chain(&self) {
        let main = self.main();
        for space in self.all().into_iter().filter(|space| !space.main) {
            if !matches!(space.store.vault_status(), Ok(VaultStatus::Locked)) {
                continue;
            }
            let opened = space
                .store
                .unlock_remembered_vault(|blob| main.open_for_space(&space.id, blob));
            if !matches!(opened, Ok(true)) {
                tracing::warn!("a second server's vault did not open with the main key");
            }
        }
    }

    /// Seal every other open vault's key under the main vault's again. Called
    /// whenever a vault key changed or was opened by its own password.
    pub fn reseal_chain(&self) {
        let main = self.main();
        if !matches!(main.vault_status(), Ok(VaultStatus::Unlocked)) {
            return;
        }
        for space in self.all().into_iter().filter(|space| !space.main) {
            if !matches!(space.store.vault_status(), Ok(VaultStatus::Unlocked)) {
                continue;
            }
            if let Err(error) = space
                .store
                .remember_vault(|key| main.seal_for_space(&space.id, key))
            {
                tracing::warn!(%error, "a second server's key could not be kept");
            }
        }
    }

    /// Lock every vault.
    pub fn lock_all(&self) {
        for space in self.all() {
            space.store.lock_vault();
        }
    }

    // -- hosts --------------------------------------------------------------

    fn journal(&self) -> Option<Journal> {
        self.inner.read().journal.clone()
    }

    /// Every host of every space, in the order of the list.
    pub fn list_hosts(&self) -> Result<Vec<SpaceHost>> {
        let mut hosts = Vec::new();
        for space in self.all() {
            for host in space.store.list_hosts()? {
                hosts.push(SpaceHost {
                    host,
                    space: space.id.clone(),
                });
            }
        }
        // A host caught half way is shown where it was going.
        if let Some(journal) = self.journal() {
            let arrived = hosts
                .iter()
                .any(|h| h.space == journal.to && h.host.id == journal.target_host);
            if arrived {
                hosts.retain(|h| !(h.space == journal.from && h.host.id == journal.host));
            }
        }
        let mut seen = HashSet::new();
        hosts.retain(|h| seen.insert(h.host.id));
        hosts.sort_by_cached_key(|h| host_order(&h.host));
        Ok(hosts)
    }

    fn host_space(&self, id: Uuid) -> Result<Option<SpaceHandle>> {
        let mut found = None;
        for space in self.all() {
            if space.store.get_host(id)?.is_some() {
                found = Some(space);
                break;
            }
        }
        if let (Some(journal), Some(space)) = (self.journal(), &found) {
            if journal.host == id && journal.target_host == id && space.id == journal.from {
                if let Some(target) = self.get(&journal.to) {
                    if target.store.get_host(id)?.is_some() {
                        return Ok(Some(target));
                    }
                }
            }
        }
        Ok(found)
    }

    /// The space a host lives in.
    pub fn space_of_host(&self, id: Uuid) -> Result<Option<String>> {
        Ok(self.host_space(id)?.map(|space| space.id))
    }

    /// The store a host lives in — the one its host key, its tunnels and its
    /// password are in too.
    pub fn store_of_host(&self, id: Uuid) -> Result<Arc<Store>> {
        Ok(self
            .host_space(id)?
            .ok_or(StoreError::UnknownHost(id))?
            .store)
    }

    pub fn get_host(&self, id: Uuid) -> Result<Option<SpaceHost>> {
        let Some(space) = self.host_space(id)? else {
            return Ok(None);
        };
        Ok(space.store.get_host(id)?.map(|host| SpaceHost {
            host,
            space: space.id,
        }))
    }

    /// Save the host form. A new host goes to the server the draft names or
    /// to its workspace's default; an existing one that names another server
    /// than the one it is on moves there first, with everything it uses.
    pub fn save_host(&self, draft: SpaceHostDraft) -> Result<SpaceHost> {
        let _ops = self.ops.lock();
        let SpaceHostDraft { draft, space } = draft;
        // Refused before anything moves, not after.
        let mut draft = draft.validated()?;
        if let Some(space) = &space {
            if self.get(space).is_none() {
                return Err(unknown_space());
            }
        }

        let target = match draft.id {
            Some(id) => {
                let current = self.host_space(id)?.ok_or(StoreError::UnknownHost(id))?;
                match space.as_deref().filter(|space| *space != current.id) {
                    Some(to) => {
                        draft.id = Some(self.move_between(id, &current.id, to)?);
                        self.get(to).ok_or_else(unknown_space)?
                    }
                    None => current,
                }
            }
            None => match space.as_deref() {
                Some(space) => self.get(space).ok_or_else(unknown_space)?,
                None => self.default_for(draft.workspace.unwrap_or_default()),
            },
        };

        // A key that only lives on the other server comes along.
        if let Some(key) = draft.key_id {
            draft.key_id = Some(self.key_in(key, &target)?);
        }
        let new = draft.id.is_none();
        let mut host = target.store.save_host(draft)?;
        if new {
            // The end of its group, across both servers.
            let last = self
                .list_hosts()?
                .into_iter()
                .filter(|other| {
                    other.host.id != host.id
                        && other.host.workspace == host.workspace
                        && other.host.group_path == host.group_path
                })
                .map(|other| other.host.position)
                .max();
            if let Some(last) = last.filter(|last| *last >= host.position) {
                host = target.store.place_host(host.id, None, last + 1)?;
            }
        }
        Ok(SpaceHost {
            host,
            space: target.id,
        })
    }

    pub fn delete_host(&self, id: Uuid) -> Result<()> {
        let _ops = self.ops.lock();
        self.store_of_host(id)?.delete_host(id)
    }

    pub fn set_host_password(&self, id: Uuid, change: PasswordChange) -> Result<SpaceHost> {
        let space = self.host_space(id)?.ok_or(StoreError::UnknownHost(id))?;
        Ok(SpaceHost {
            host: space.store.set_host_password(id, change)?,
            space: space.id,
        })
    }

    pub fn mark_connected(&self, id: Uuid) -> Result<()> {
        self.store_of_host(id)?.mark_connected(id)
    }

    pub fn set_host_os(&self, id: Uuid, os: Option<&str>) -> Result<()> {
        self.store_of_host(id)?.set_host_os(id, os)
    }

    /// Move a host to another server. Returns the id it has there: its own,
    /// unless it had been on that server before — see [`import_key`] for why
    /// a tombstone's id is not taken up again.
    pub fn move_host_to(&self, id: Uuid, to: &str) -> Result<Uuid> {
        let _ops = self.ops.lock();
        let current = self.host_space(id)?.ok_or(StoreError::UnknownHost(id))?;
        if current.id == to {
            return Ok(id);
        }
        self.move_between(id, &current.id, to)
    }

    /// The move itself. The journal entry is on disk before the target is
    /// written and gone after the source was cleaned up; whatever happens in
    /// between, [`Self::recover`] finishes it or forgets it, so the host is
    /// never lost and never there twice.
    fn move_between(&self, id: Uuid, from: &str, to: &str) -> Result<Uuid> {
        self.recover()?;
        let source = self.get(from).ok_or_else(unknown_space)?;
        let target = self.get(to).ok_or_else(unknown_space)?;
        if target.read_only {
            return Err(invalid("space", "read-only"));
        }
        let moved = source.store.export_moved_host(id)?;
        let needs_vault = moved.password.is_some() || moved.key.is_some();
        if needs_vault && !matches!(target.store.vault_status()?, VaultStatus::Unlocked) {
            return Err(StoreError::VaultLocked);
        }
        let target_host = match target.store.host_row(id)? {
            None => id,
            Some(_) => Uuid::now_v7(),
        };

        {
            let mut inner = self.inner.write();
            inner.journal = Some(Journal {
                host: id,
                target_host,
                from: from.to_string(),
                to: to.to_string(),
            });
            self.persist(&mut inner)?;
        }
        if let Err(error) = target.store.import_moved_host(&moved, target_host) {
            // Nothing was written there: the transaction is gone with it.
            let mut inner = self.inner.write();
            inner.journal = None;
            self.persist(&mut inner)?;
            return Err(error);
        }
        drop(moved);
        source.store.drop_moved_host(id)?;
        let mut inner = self.inner.write();
        inner.journal = None;
        self.persist(&mut inner)?;
        Ok(target_host)
    }

    /// Finish or forget a move that was interrupted. Needs no vault: if the
    /// host arrived, the source lets go of it; if it did not, it never left.
    fn recover(&self) -> Result<()> {
        let Some(journal) = self.journal() else {
            return Ok(());
        };
        // A move is between two servers. A note that names one twice would
        // take the host away from the only place it is in.
        let arrived = journal.from != journal.to
            && match self.get(&journal.to) {
                Some(target) => target.store.get_host(journal.target_host)?.is_some(),
                None => false,
            };
        if arrived {
            if let Some(source) = self.get(&journal.from) {
                source.store.drop_moved_host(journal.host)?;
            }
            tracing::info!("a host's move to another server was finished");
        }
        let mut inner = self.inner.write();
        inner.journal = None;
        self.persist(&mut inner)
    }

    /// Put a host into a workspace and group, before another host of that
    /// group or at its end. The group is numbered across every server, and
    /// only hosts whose place changes are written.
    pub fn move_host(
        &self,
        id: Uuid,
        to: Workspace,
        group: Option<&str>,
        before: Option<Uuid>,
    ) -> Result<SpaceHost> {
        let _ops = self.ops.lock();
        let space = self.host_space(id)?.ok_or(StoreError::UnknownHost(id))?;
        let group = group_name(group.map(str::to_string))?;
        let members: Vec<SpaceHost> = self
            .list_hosts()?
            .into_iter()
            .filter(|other| {
                other.host.id != id && other.host.workspace == to && other.host.group_path == group
            })
            .collect();
        let at = before
            .and_then(|before| members.iter().position(|other| other.host.id == before))
            .unwrap_or(members.len());
        let host = space
            .store
            .place_host(id, Some((to, group.as_deref())), at as i64)?;
        for (index, other) in members.iter().enumerate() {
            let position = if index >= at { index + 1 } else { index } as i64;
            if other.host.position != position {
                self.store(Some(&other.space))?
                    .place_host(other.host.id, None, position)?;
            }
        }
        Ok(SpaceHost {
            host,
            space: space.id,
        })
    }

    // -- groups -------------------------------------------------------------

    /// Every group there is on any server, once.
    pub fn list_groups(&self) -> Result<Vec<GroupRecord>> {
        let mut seen = HashSet::new();
        let mut groups = Vec::new();
        for space in self.all() {
            for group in space.store.list_groups()? {
                if seen.insert((group.workspace, group.name.clone())) {
                    groups.push(group);
                }
            }
        }
        groups.sort_by_cached_key(|group| {
            (
                group.workspace.as_str(),
                group.position,
                group.name.to_lowercase(),
            )
        });
        Ok(groups)
    }

    fn spaces_with_group(&self, workspace: Workspace, name: &str) -> Result<Vec<SpaceHandle>> {
        let mut found = Vec::new();
        for space in self.all() {
            let has = space
                .store
                .list_groups()?
                .iter()
                .any(|group| group.workspace == workspace && group.name == name);
            if has {
                found.push(space);
            }
        }
        if found.is_empty() {
            return Err(invalid("groupPath", "unknown"));
        }
        Ok(found)
    }

    pub fn create_group(&self, workspace: Workspace, name: &str) -> Result<GroupRecord> {
        let _ops = self.ops.lock();
        let wanted =
            group_name(Some(name.to_string()))?.ok_or_else(|| invalid("groupPath", "required"))?;
        if self
            .list_groups()?
            .iter()
            .any(|group| group.workspace == workspace && group.name == wanted)
        {
            return Err(invalid("groupPath", "exists"));
        }
        self.default_for(workspace)
            .store
            .create_group(workspace, &wanted)
    }

    pub fn rename_group(&self, workspace: Workspace, from: &str, to: &str) -> Result<()> {
        let _ops = self.ops.lock();
        for space in self.spaces_with_group(workspace, from)? {
            space.store.rename_group(workspace, from, to)?;
        }
        Ok(())
    }

    pub fn delete_group(&self, workspace: Workspace, name: &str) -> Result<()> {
        let _ops = self.ops.lock();
        for space in self.spaces_with_group(workspace, name)? {
            space.store.delete_group(workspace, name)?;
        }
        Ok(())
    }

    pub fn move_group(
        &self,
        workspace: Workspace,
        name: &str,
        to: Workspace,
        before: Option<&str>,
    ) -> Result<()> {
        let _ops = self.ops.lock();
        for space in self.spaces_with_group(workspace, name)? {
            space.store.move_group(workspace, name, to, before)?;
        }
        Ok(())
    }

    // -- keys ---------------------------------------------------------------

    /// Every key, once: the same key on both servers is one key, used by the
    /// hosts of both.
    pub fn list_keys(&self) -> Result<Vec<KeyRecord>> {
        let mut keys: Vec<KeyRecord> = Vec::new();
        for space in self.all() {
            for key in space.store.list_keys()? {
                match keys.iter_mut().find(|other| other.id == key.id) {
                    Some(other) => other.hosts += key.hosts,
                    None => keys.push(key),
                }
            }
        }
        keys.sort_by_cached_key(|key| (key.label.to_lowercase(), key.id));
        Ok(keys)
    }

    fn spaces_with_key(&self, id: Uuid) -> Result<Vec<SpaceHandle>> {
        let mut found = Vec::new();
        for space in self.all() {
            if space.store.get_key(id)?.is_some() {
                found.push(space);
            }
        }
        Ok(found)
    }

    pub fn space_of_key(&self, id: Uuid) -> Result<Option<String>> {
        Ok(self.spaces_with_key(id)?.into_iter().next().map(|s| s.id))
    }

    pub fn get_key(&self, id: Uuid) -> Result<Option<KeyRecord>> {
        Ok(self.list_keys()?.into_iter().find(|key| key.id == id))
    }

    pub fn add_key(&self, draft: KeyDraft) -> Result<KeyRecord> {
        self.main().add_key(draft)
    }

    pub fn rename_key(&self, id: Uuid, label: &str) -> Result<()> {
        let _ops = self.ops.lock();
        let spaces = self.spaces_with_key(id)?;
        if spaces.is_empty() {
            return Err(StoreError::UnknownKey(id));
        }
        for space in spaces {
            space.store.rename_key(id, label)?;
        }
        Ok(())
    }

    /// Delete a key everywhere. Refused while a host on any server uses it.
    pub fn delete_key(&self, id: Uuid) -> Result<()> {
        let _ops = self.ops.lock();
        let key = self.get_key(id)?.ok_or(StoreError::UnknownKey(id))?;
        if key.hosts > 0 {
            return Err(StoreError::KeyInUse { hosts: key.hosts });
        }
        for space in self.spaces_with_key(id)? {
            space.store.delete_key(id)?;
        }
        Ok(())
    }

    pub fn reveal_key(&self, id: Uuid) -> Result<RevealedKey> {
        let space = self
            .spaces_with_key(id)?
            .into_iter()
            .next()
            .ok_or(StoreError::UnknownKey(id))?;
        space.store.reveal_key(id)
    }

    /// The id a key has in `target`, copying it there from the server that
    /// has it when it is missing.
    fn key_in(&self, key: Uuid, target: &SpaceHandle) -> Result<Uuid> {
        if target.store.get_key(key)?.is_some() {
            return Ok(key);
        }
        let Some(source) = self.spaces_with_key(key)?.into_iter().next() else {
            return Err(invalid("keyId", "unknown"));
        };
        let moved = source.store.export_moved_key(key)?;
        target.store.import_moved_key(&moved)
    }

    // -- tunnels and host keys -----------------------------------------------

    pub fn list_tunnels(&self) -> Result<Vec<TunnelRecord>> {
        let mut tunnels = Vec::new();
        let mut seen = HashSet::new();
        for space in self.all() {
            for tunnel in space.store.list_tunnels()? {
                if seen.insert(tunnel.id) {
                    tunnels.push(tunnel);
                }
            }
        }
        tunnels.sort_by_cached_key(|t| (t.host_id, t.name.to_lowercase(), t.id));
        Ok(tunnels)
    }

    fn tunnel_space(&self, id: Uuid) -> Result<Option<SpaceHandle>> {
        for space in self.all() {
            if space.store.get_tunnel(id)?.is_some() {
                return Ok(Some(space));
            }
        }
        Ok(None)
    }

    pub fn space_of_tunnel(&self, id: Uuid) -> Result<Option<String>> {
        Ok(self.tunnel_space(id)?.map(|space| space.id))
    }

    pub fn get_tunnel(&self, id: Uuid) -> Result<Option<TunnelRecord>> {
        match self.tunnel_space(id)? {
            Some(space) => space.store.get_tunnel(id),
            None => Ok(None),
        }
    }

    /// A tunnel is saved where its host is.
    pub fn save_tunnel(&self, draft: TunnelDraft) -> Result<TunnelRecord> {
        self.store_of_host(draft.host_id)?.save_tunnel(draft)
    }

    pub fn delete_tunnel(&self, id: Uuid) -> Result<()> {
        match self.tunnel_space(id)? {
            Some(space) => space.store.delete_tunnel(id),
            None => Err(invalid("tunnel", "unknown")),
        }
    }

    // -- which server is main -------------------------------------------------

    /// Make another server the main one. Both vaults have to be open.
    ///
    /// The personal records go over with their secrets, the newer of two
    /// winning; the key this device keeps to open without the password —
    /// sealed by `protect`, the operating system — is from then on the new
    /// main vault's, and the old main's key is chained to it instead. The
    /// registry is swapped in one rename: before it the old server is main in
    /// every respect, after it the new one is — the keys are prepared beside
    /// the ones in use before the swap and take their place after it, which a
    /// start finishes when this did not get to.
    pub fn make_main(
        &self,
        id: &str,
        protect: &dyn Fn(&[u8]) -> std::io::Result<Vec<u8>>,
    ) -> Result<()> {
        let _ops = self.ops.lock();
        self.make_main_locked(id, protect)
    }

    fn make_main_locked(
        &self,
        id: &str,
        protect: &dyn Fn(&[u8]) -> std::io::Result<Vec<u8>>,
    ) -> Result<()> {
        self.swap_main(id, protect)?;
        self.settle_handover()
    }

    /// The first half of [`Self::make_main`]: up to and including the swap.
    fn swap_main(
        &self,
        id: &str,
        protect: &dyn Fn(&[u8]) -> std::io::Result<Vec<u8>>,
    ) -> Result<()> {
        let new = self.get(id).ok_or_else(unknown_space)?;
        if new.main {
            return Ok(());
        }
        if new.kind != SpaceKind::Server || new.read_only {
            return Err(invalid("space", "read-only"));
        }
        let old = self.main();
        for store in [&old, &new.store] {
            if !matches!(store.vault_status()?, VaultStatus::Unlocked) {
                return Err(StoreError::VaultLocked);
            }
        }

        let personal = old.export_personal()?;
        new.store.import_personal(&personal)?;
        drop(personal);

        // Every key this device keeps is prepared for the new order first
        // and put in place after the swap: what opened the old main without
        // a password is to open the new one, and every other open vault's
        // key is sealed by the new main's.
        let prepared = (|| -> Result<()> {
            let remembered = old.vault_is_remembered()?;
            new.store.stage_unlock(remembered.then_some(protect))?;
            for space in self.all().into_iter().filter(|space| space.id != id) {
                if !matches!(space.store.vault_status()?, VaultStatus::Unlocked) {
                    continue;
                }
                let seal = |key: &[u8]| new.store.seal_for_space(&space.id, key);
                space.store.stage_unlock(Some(&seal))?;
            }
            Ok(())
        })();
        if let Err(error) = prepared {
            self.drop_staged();
            return Err(error);
        }
        {
            let mut inner = self.inner.write();
            let before = std::mem::replace(&mut inner.main, id.to_string());
            inner.handover = Some(before.clone());
            if let Err(error) = self.persist(&mut inner) {
                inner.main = before;
                inner.handover = None;
                drop(inner);
                self.drop_staged();
                return Err(error);
            }
        }
        Ok(())
    }

    fn drop_staged(&self) {
        for space in self.all() {
            if let Err(error) = space.store.drop_staged_unlock() {
                tracing::warn!(%error, "a prepared key could not be cleared away");
            }
        }
    }

    /// The second half of [`Self::make_main`], and what a start does when the
    /// first half was the last thing that happened: put every prepared key
    /// in place and take the personal records out of the old main server.
    /// Needs no vault, and doing it again changes nothing. Without a swap to
    /// finish, keys that were prepared for one that never happened go.
    fn settle_handover(&self) -> Result<()> {
        let Some(old) = self.inner.read().handover.clone() else {
            self.drop_staged();
            return Ok(());
        };
        for space in self.all() {
            space.store.promote_staged_unlock()?;
        }
        if let Some(old) = self.get(&old) {
            old.store.clear_personal()?;
        }
        let mut inner = self.inner.write();
        inner.handover = None;
        self.persist(&mut inner)
    }

    /// Whether a server's entries began to move away and are not all gone:
    /// [`Self::remove`] stopped half way. Until it is asked again and gets
    /// through, the server must not sync — what already left it is deleted
    /// on this device only, and a pass would delete it for its other devices
    /// too.
    pub fn is_leaving(&self, id: &str) -> bool {
        self.inner.read().leaving.as_deref() == Some(id)
    }

    /// Take a server away from this device. With `keep`, its hosts — with
    /// their logins, keys, tunnels and host keys — its other keys and its
    /// groups move to the remaining server first. Removing the main server
    /// makes the other one main.
    pub fn remove(
        &self,
        id: &str,
        keep: bool,
        protect: &dyn Fn(&[u8]) -> std::io::Result<Vec<u8>>,
    ) -> Result<()> {
        let _ops = self.ops.lock();
        let gone = self.get(id).ok_or_else(unknown_space)?;
        let rest = self.other(id).ok_or_else(|| invalid("space", "last"))?;

        if keep {
            // Written down before the first entry moves: from then on this
            // server must not sync until it is gone.
            let resumed = self.is_leaving(id);
            if !resumed {
                let mut inner = self.inner.write();
                inner.leaving = Some(id.to_string());
                if let Err(error) = self.persist(&mut inner) {
                    inner.leaving = None;
                    return Err(error);
                }
            }
            let mut moved = resumed;
            let taken = (|| -> Result<()> {
                for host in gone.store.list_hosts()? {
                    self.move_between(host.id, id, &rest.id)?;
                    moved = true;
                }
                for key in gone.store.list_keys()? {
                    self.key_in(key.id, &rest)?;
                }
                for group in gone.store.list_groups()? {
                    match rest.store.create_group(group.workspace, &group.name) {
                        Ok(_) | Err(StoreError::Invalid { .. }) => {}
                        Err(error) => return Err(error),
                    }
                }
                Ok(())
            })();
            if let Err(error) = taken {
                if !moved {
                    // Nothing left it: it is as it was.
                    let mut inner = self.inner.write();
                    inner.leaving = None;
                    let _ = self.persist(&mut inner);
                }
                return Err(error);
            }
        }
        if gone.main {
            self.make_main_locked(&rest.id, protect)?;
        }

        let path = {
            let mut inner = self.inner.write();
            let path = inner
                .entries
                .iter()
                .find(|entry| entry.id == id)
                .and_then(|entry| self.paths.resolve(&entry.file));
            inner.entries.retain(|entry| entry.id != id);
            if inner.defaults.private == id {
                inner.defaults.private = rest.id.clone();
            }
            if inner.defaults.business == id {
                inner.defaults.business = rest.id.clone();
            }
            inner.journal = None;
            if inner.leaving.as_deref() == Some(id) {
                inner.leaving = None;
            }
            self.persist(&mut inner)?;
            path
        };
        // The key stays in memory for as long as somebody holds the store.
        gone.store.lock_vault();
        drop(gone);
        if let Some(path) = path {
            remove_store_files(&path);
        }
        Ok(())
    }

    // -- import and export ------------------------------------------------------

    fn host_owners(&self) -> Result<HashMap<(String, u16, String), String>> {
        let mut owners = HashMap::new();
        for host in self.list_hosts()? {
            owners
                .entry(host_match(
                    &host.host.address,
                    host.host.port,
                    &host.host.username,
                ))
                .or_insert(host.space);
        }
        Ok(owners)
    }

    /// Write an import. Each host goes to the default server of its
    /// workspace — or to the one that already has it, where it is skipped as
    /// it always was. Snippets and keys nothing uses go to the main server.
    pub fn import(&self, set: ImportSet) -> Result<ImportOutcome> {
        let _ops = self.ops.lock();
        let all = self.all();
        if all.len() == 1 {
            return all[0].store.import(set);
        }
        let main = self.main_id();
        let owners = self.host_owners()?;
        let ImportSet {
            groups,
            hosts,
            identities,
            keys,
            known_hosts,
            snippets,
        } = set;

        let mut parts: BTreeMap<String, ImportPart> = BTreeMap::new();
        let mut used_keys = HashSet::new();

        for host in hosts {
            let identity = host.identity.and_then(|index| identities.get(index));
            let username = identity
                .and_then(|identity| identity.username.as_deref())
                .unwrap_or_default();
            let target = owners
                .get(&host_match(&host.address, host.port, username))
                .cloned()
                .unwrap_or_else(|| self.default_for(host.workspace).id);
            let part = parts.entry(target).or_default();
            let identity_index = match (host.identity, identity) {
                (Some(index), Some(identity)) => Some(match part.identities.get(&index) {
                    Some(at) => *at,
                    None => {
                        let key = match identity.key.filter(|key| *key < keys.len()) {
                            Some(key) => {
                                used_keys.insert(key);
                                Some(match part.keys.get(&key) {
                                    Some(at) => *at,
                                    None => {
                                        part.set.keys.push(copy_key_input(&keys[key]));
                                        part.keys.insert(key, part.set.keys.len() - 1);
                                        part.set.keys.len() - 1
                                    }
                                })
                            }
                            None => None,
                        };
                        part.set.identities.push(IdentityInput {
                            label: identity.label.clone(),
                            username: identity.username.clone(),
                            password: identity.password.clone(),
                            key,
                            key_path: identity.key_path.clone(),
                        });
                        part.identities.insert(index, part.set.identities.len() - 1);
                        part.set.identities.len() - 1
                    }
                }),
                _ => None,
            };
            part.set.hosts.push(HostInput {
                identity: identity_index,
                ..host
            });
        }
        for group in groups {
            let target = self.default_for(group.workspace).id;
            parts
                .entry(target)
                .or_default()
                .set
                .groups
                .push(GroupInput {
                    workspace: group.workspace,
                    name: group.name,
                });
        }
        // Keys no login points at still belong in the vault once: the main one.
        for (index, key) in keys.iter().enumerate() {
            if used_keys.contains(&index)
                || identities
                    .iter()
                    .any(|identity| identity.key == Some(index))
            {
                continue;
            }
            parts
                .entry(main.clone())
                .or_default()
                .set
                .keys
                .push(copy_key_input(key));
        }
        {
            let part = parts.entry(main.clone()).or_default();
            part.set
                .snippets
                .extend(snippets.into_iter().map(|snippet| SnippetInput {
                    label: snippet.label,
                    body: snippet.body,
                    group_path: snippet.group_path,
                }));
        }
        for part in parts.values_mut() {
            for known in &known_hosts {
                part.set.known_hosts.push(KnownHostInput {
                    address: known.address.clone(),
                    port: known.port,
                    algorithm: known.algorithm.clone(),
                    public_key: known.public_key.clone(),
                    fingerprint: known.fingerprint.clone(),
                });
            }
        }

        let mut total = ImportOutcome::default();
        for (id, part) in parts {
            add_outcome(&mut total, self.store(Some(&id))?.import(part.set)?);
        }
        Ok(total)
    }

    /// Everything on every server in one backup.
    pub fn export_backup(&self, secrets: bool) -> Result<Backup> {
        let mut total = Backup::default();
        let mut groups = HashSet::new();
        let mut known = HashSet::new();
        for space in self.all() {
            let part = space.store.export_backup(secrets)?;
            for group in part.groups {
                if groups.insert((group.workspace, group.name.clone())) {
                    total.groups.push(group);
                }
            }
            // The same key on both servers is written once.
            let mut key_at = Vec::with_capacity(part.keys.len());
            for key in part.keys {
                let twin = total.keys.iter().position(|other| {
                    other.label == key.label
                        && other.key_type == key.key_type
                        && other.public_key == key.public_key
                        && other.private_key == key.private_key
                });
                key_at.push(twin.unwrap_or_else(|| {
                    total.keys.push(key);
                    total.keys.len() - 1
                }));
            }
            let first_host = total.hosts.len();
            for host in part.hosts {
                total.hosts.push(BackupHost {
                    key: host.key.and_then(|key| key_at.get(key).copied()),
                    ..host
                });
            }
            for tunnel in part.tunnels {
                total.tunnels.push(BackupTunnel {
                    host: tunnel.host + first_host,
                    ..tunnel
                });
            }
            for host in part.known_hosts {
                if known.insert((host.address.clone(), host.port)) {
                    total.known_hosts.push(host);
                }
            }
            if space.main {
                total.snippets = part.snippets;
            }
        }
        Ok(total)
    }

    /// Read a backup in, each host on the default server of its workspace.
    pub fn import_backup(&self, backup: Backup) -> Result<ImportOutcome> {
        let _ops = self.ops.lock();
        let all = self.all();
        if all.len() == 1 {
            return all[0].store.import_backup(backup);
        }
        let main = self.main_id();
        let owners = self.host_owners()?;
        let Backup {
            groups,
            hosts,
            keys,
            known_hosts,
            snippets,
            tunnels,
        } = backup;

        let mut parts: BTreeMap<String, Backup> = BTreeMap::new();
        let mut key_at: HashMap<(String, usize), usize> = HashMap::new();
        // Where each host of the file went: its server and its index there.
        let mut host_at: Vec<(String, usize)> = Vec::with_capacity(hosts.len());
        let mut used_keys = HashSet::new();
        for host in hosts {
            let target = owners
                .get(&host_match(&host.address, host.port, &host.username))
                .cloned()
                .unwrap_or_else(|| self.default_for(host.workspace).id);
            let part = parts.entry(target.clone()).or_default();
            let key = match host.key.filter(|key| *key < keys.len()) {
                Some(key) => {
                    used_keys.insert(key);
                    Some(*key_at.entry((target.clone(), key)).or_insert_with(|| {
                        part.keys.push(copy_backup_key(&keys[key]));
                        part.keys.len() - 1
                    }))
                }
                None => None,
            };
            part.hosts.push(BackupHost { key, ..host });
            host_at.push((target, part.hosts.len() - 1));
        }
        for tunnel in tunnels {
            let Some((target, index)) = host_at.get(tunnel.host) else {
                continue;
            };
            parts
                .entry(target.clone())
                .or_default()
                .tunnels
                .push(BackupTunnel {
                    host: *index,
                    ..tunnel
                });
        }
        for group in groups {
            let target = self.default_for(group.workspace).id;
            parts
                .entry(target)
                .or_default()
                .groups
                .push(BackupGroup { ..group });
        }
        {
            let part = parts.entry(main).or_default();
            for (index, key) in keys.iter().enumerate() {
                if !used_keys.contains(&index) {
                    part.keys.push(copy_backup_key(key));
                }
            }
            part.snippets = snippets
                .into_iter()
                .map(|snippet| BackupSnippet { ..snippet })
                .collect();
        }
        for part in parts.values_mut() {
            for known in &known_hosts {
                part.known_hosts.push(BackupKnownHost {
                    address: known.address.clone(),
                    port: known.port,
                    algorithm: known.algorithm.clone(),
                    fingerprint: known.fingerprint.clone(),
                    public_key: known.public_key.clone(),
                });
            }
        }

        let mut total = ImportOutcome::default();
        for (id, part) in parts {
            add_outcome(&mut total, self.store(Some(&id))?.import_backup(part)?);
        }
        Ok(total)
    }
}

/// Delete a store's file with what SQLite keeps beside it.
fn remove_store_files(path: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        match std::fs::remove_file(PathBuf::from(name)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(%error, "a removed server's file is still there"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hosts::{tests::draft, AuthMethod};
    use crate::keymap::{KeymapDraft, KeymapTextDraft};
    use crate::{AssistProviderDraft, NewCacheEntry};
    use uwussh_vault::KdfParams;

    const FAST: KdfParams = KdfParams::INSECURE_FOR_TESTS;

    /// A folder of its own for one test, gone when the test is.
    struct Folder(PathBuf);

    impl Folder {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("uwussh-spaces-{}", Uuid::now_v7())))
        }
        fn base(&self) -> PathBuf {
            self.0.join("uwussh.db")
        }
        fn registry(&self) -> PathBuf {
            self.0.join("spaces.json")
        }
        fn open(&self) -> Spaces {
            Spaces::open(&self.base()).unwrap()
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// What stands in for the operating system's keyring.
    fn os(key: &[u8]) -> std::io::Result<Vec<u8>> {
        Ok(key.iter().map(|b| b ^ 0x5a).collect())
    }

    fn os_open(blob: &[u8]) -> std::io::Result<Zeroizing<Vec<u8>>> {
        Ok(Zeroizing::new(blob.iter().map(|b| b ^ 0x5a).collect()))
    }

    /// Two servers, both vaults open: main with the password `main`, the
    /// second with `second`.
    fn two(folder: &Folder) -> (Spaces, String) {
        let spaces = folder.open();
        spaces.main().create_vault_with(b"main", FAST).unwrap();
        let (id, store) = spaces.prepare().unwrap();
        store.create_vault_with(b"second", FAST).unwrap();
        spaces.register(&id, store).unwrap();
        (spaces, id)
    }

    fn on(space: &str, draft: HostDraft) -> SpaceHostDraft {
        SpaceHostDraft {
            draft,
            space: Some(space.to_string()),
        }
    }

    fn key_draft(label: &str) -> KeyDraft {
        KeyDraft {
            label: label.into(),
            key_type: "ssh-ed25519".into(),
            public_key: format!("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIA {label}"),
            private_key: SecretText::new("-----BEGIN OPENSSH PRIVATE KEY-----\nbody\n"),
            passphrase: Some(SecretText::new("meow")),
        }
    }

    fn with_password(mut draft: HostDraft, password: &str) -> HostDraft {
        draft.password = PasswordChange::Set {
            value: SecretText::new(password),
        };
        draft
    }

    fn tunnel(host: Uuid) -> TunnelDraft {
        TunnelDraft {
            id: None,
            host_id: host,
            name: "admin".into(),
            kind: "local".into(),
            bind_address: None,
            bind_port: 8080,
            target_host: "localhost".into(),
            target_port: 80,
            autostart: true,
        }
    }

    fn names(spaces: &Spaces) -> Vec<(String, String)> {
        spaces
            .list_hosts()
            .unwrap()
            .into_iter()
            .map(|host| (host.host.name, host.space))
            .collect()
    }

    fn live(store: &Store, table: &str) -> i64 {
        store
            .conn
            .lock()
            .query_row(
                &format!("SELECT count(*) FROM {table} WHERE deleted = 0"),
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    // -- registry -------------------------------------------------------------

    #[test]
    fn without_a_registry_there_is_one_main_space_and_nothing_is_written() {
        let folder = Folder::new();
        let spaces = folder.open();
        let all = spaces.all();
        assert_eq!(all.len(), 1);
        assert!(all[0].main);
        assert_eq!(all[0].kind, SpaceKind::Server);
        assert!(!all[0].read_only);
        assert_eq!(spaces.main_id(), all[0].id);
        assert_eq!(spaces.defaults().private, all[0].id);
        assert!(spaces.has_room());
        assert!(!folder.registry().exists());
        assert!(spaces.get(NEW_SPACE).is_none());
        assert!(matches!(
            spaces.store(Some("nowhere")),
            Err(StoreError::Invalid { field: "space", .. })
        ));
    }

    #[test]
    fn the_registry_survives_a_restart() {
        let folder = Folder::new();
        let (spaces, second) = two(&folder);
        spaces.set_default(Workspace::Business, &second).unwrap();
        assert!(!spaces.has_room());
        assert!(matches!(
            spaces.prepare(),
            Err(StoreError::Invalid {
                problem: "limit",
                ..
            })
        ));
        assert!(spaces.set_default(Workspace::Private, "nowhere").is_err());
        let main = spaces.main_id();
        drop(spaces);

        let spaces = folder.open();
        let all = spaces.all();
        assert_eq!(all.len(), 2);
        assert_eq!((all[0].id.as_str(), all[0].main), (main.as_str(), true));
        assert_eq!((all[1].id.as_str(), all[1].main), (second.as_str(), false));
        assert_eq!(
            spaces.defaults(),
            SpaceDefaults {
                private: main,
                business: second.clone()
            }
        );
        assert_eq!(spaces.default_for(Workspace::Business).id, second);
        assert!(folder
            .0
            .join("spaces")
            .join(format!("{second}.db"))
            .exists());
    }

    #[cfg(unix)]
    #[test]
    fn the_registry_is_written_whole_and_private() {
        use std::os::unix::fs::PermissionsExt;
        let folder = Folder::new();
        let (spaces, _) = two(&folder);
        let mode = std::fs::metadata(folder.registry())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        assert!(!folder.0.join("spaces.json.tmp").exists());
        let second = folder.0.join("spaces");
        for file in std::fs::read_dir(second).unwrap().flatten() {
            let mode = file.metadata().unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{:?}", file.file_name());
        }
        drop(spaces);

        // A write that never got to its rename leaves the old registry.
        std::fs::write(folder.0.join("spaces.json.tmp"), b"{ \"version\": 1, \"ma").unwrap();
        assert_eq!(folder.open().all().len(), 2);
    }

    #[test]
    fn a_registry_that_does_not_read_starts_with_one_server_and_loses_nothing() {
        let folder = Folder::new();
        let (spaces, second) = two(&folder);
        drop(spaces);
        let file = folder.0.join("spaces").join(format!("{second}.db"));
        for garbage in [
            &b"\0\xff not json"[..],
            b"{}",
            b"[]",
            b"{\"version\":99}",
            b"",
        ] {
            std::fs::write(folder.registry(), garbage).unwrap();
            let spaces = folder.open();
            assert_eq!(spaces.all().len(), 1);
            // The other server's data is not cleared away on a guess.
            assert!(file.exists());
        }
        // The next write keeps what did not read beside the new registry.
        let spaces = folder.open();
        let main = spaces.main_id();
        spaces.set_default(Workspace::Business, &main).unwrap();
        assert!(folder.0.join("spaces.json.unreadable").exists());
        drop(spaces);
        assert_eq!(folder.open().all().len(), 1);
    }

    #[test]
    fn a_file_name_from_the_registry_never_leaves_the_spaces_folder() {
        let folder = Folder::new();
        drop(folder.open());
        let outside = folder
            .0
            .parent()
            .unwrap()
            .join(format!("escaped-{}.db", Uuid::now_v7()));
        let escaped = outside.file_name().unwrap().to_str().unwrap().to_string();
        let uuid = Uuid::new_v4();
        for file in [
            format!("../{escaped}"),
            format!("spaces/../../{escaped}"),
            outside.to_str().unwrap().to_string(),
            format!("spaces/{uuid}.db/../../../{escaped}"),
            format!("spaces/{}.db", uuid.simple()),
            format!("spaces/{}.db", uuid.to_string().to_uppercase()),
            format!("spaces/{uuid}.db\0"),
            "spaces/.db".to_string(),
            String::new(),
        ] {
            let registry = serde_json::json!({
                "version": 1,
                "main": "first",
                "spaces": [
                    { "id": "first", "file": "uwussh.db", "kind": "server" },
                    { "id": "other", "file": file, "kind": "server" },
                ],
                "defaults": { "private": "other", "business": "first" },
            });
            std::fs::write(folder.registry(), registry.to_string()).unwrap();
            let spaces = folder.open();
            assert_eq!(spaces.all().len(), 1, "{file:?}");
            assert_eq!(spaces.defaults().private, "first");
            assert!(!outside.exists(), "{file:?}");
        }

        let paths = Paths::of(&folder.base()).unwrap();
        assert_eq!(paths.resolve("uwussh.db"), Some(folder.base()));
        assert_eq!(
            paths.resolve(&format!("spaces/{uuid}.db")),
            Some(folder.0.join("spaces").join(format!("{uuid}.db")))
        );
        // Another database in the same folder has a registry of its own.
        let other = Paths::of(&folder.0.join("work.db")).unwrap();
        assert_eq!(other.registry, folder.0.join("work.spaces.json"));
        assert_eq!(other.folder, folder.0.join("work.spaces"));
    }

    #[test]
    fn a_registry_naming_one_server_twice_or_no_main_is_not_followed() {
        let folder = Folder::new();
        drop(folder.open());
        let uuid = Uuid::new_v4();
        let twice = serde_json::json!({
            "version": 1, "main": "first",
            "spaces": [
                { "id": "first", "file": "uwussh.db", "kind": "server" },
                { "id": "first", "file": format!("spaces/{uuid}.db"), "kind": "server" },
                { "id": "new", "file": format!("spaces/{uuid}.db"), "kind": "server" },
                { "id": "again", "file": "uwussh.db", "kind": "server" },
            ],
            "defaults": { "private": "first", "business": "first" },
            "journal": { "host": Uuid::nil(), "targetHost": Uuid::nil(), "from": "x", "to": "y" },
        });
        std::fs::write(folder.registry(), twice.to_string()).unwrap();
        assert_eq!(folder.open().all().len(), 1);

        let no_main = serde_json::json!({
            "version": 1, "main": "gone",
            "spaces": [{ "id": "first", "file": "uwussh.db", "kind": "server" }],
            "defaults": { "private": "gone", "business": "gone" },
        });
        std::fs::write(folder.registry(), no_main.to_string()).unwrap();
        let spaces = folder.open();
        assert_eq!(spaces.all().len(), 1);
        assert!(spaces.all()[0].main);
        spaces.main().save_host(draft("a", "192.0.2.1")).unwrap();
    }

    #[test]
    fn a_server_that_was_never_set_up_to_the_end_leaves_no_file() {
        let folder = Folder::new();
        let spaces = folder.open();
        let (id, store) = spaces.prepare().unwrap();
        let file = folder.0.join("spaces").join(format!("{id}.db"));
        assert!(file.exists());
        drop(store);
        spaces.discard(&id);
        assert!(!file.exists());
        // Nothing to throw away is nothing to do.
        spaces.discard(&id);
        spaces.discard("../uwussh");

        let (id, store) = spaces.prepare().unwrap();
        let file = folder.0.join("spaces").join(format!("{id}.db"));
        drop(store);
        drop(spaces);
        let stranger = folder.0.join("spaces").join("notes.txt");
        std::fs::write(&stranger, b"mine").unwrap();
        let spaces = folder.open();
        assert!(!file.exists());
        assert!(stranger.exists());

        // A registered one is never thrown away by `discard`.
        let (id, store) = spaces.prepare().unwrap();
        spaces.main().create_vault_with(b"main", FAST).unwrap();
        store.create_vault_with(b"second", FAST).unwrap();
        spaces.register(&id, store).unwrap();
        spaces.discard(&id);
        assert!(folder.0.join("spaces").join(format!("{id}.db")).exists());
        assert_eq!(spaces.all().len(), 2);
    }

    #[test]
    fn a_second_server_outlives_a_list_that_did_not_read_once() {
        let folder = Folder::new();
        let (spaces, second) = two(&folder);
        drop(spaces);
        let file = folder.0.join("spaces").join(format!("{second}.db"));
        std::fs::write(folder.registry(), b"not a list").unwrap();
        // The first start falls back to one server and writes that down.
        let spaces = folder.open();
        let main = spaces.main_id();
        spaces.set_default(Workspace::Business, &main).unwrap();
        drop(spaces);
        // The second one reads a list that does not name the other server.
        let spaces = folder.open();
        assert_eq!(spaces.all().len(), 1);
        assert!(file.exists());
        // Not even a setup that never finished goes while the old list is
        // there to be looked at.
        let (id, store) = spaces.prepare().unwrap();
        drop(store);
        drop(spaces);
        drop(folder.open());
        assert!(file.exists());
        assert!(folder.0.join("spaces").join(format!("{id}.db")).exists());

        // A store file nothing marks as being set up stays, list or no list.
        let folder = Folder::new();
        let (spaces, second) = two(&folder);
        let stray = folder
            .0
            .join("spaces")
            .join(format!("{}.db", Uuid::new_v4()));
        std::fs::copy(folder.0.join("spaces").join(format!("{second}.db")), &stray).unwrap();
        drop(spaces);
        let spaces = folder.open();
        assert_eq!(spaces.all().len(), 2);
        assert!(stray.exists());
        // And a registered one loses its marker, not its file.
        assert!(!folder
            .0
            .join("spaces")
            .join(format!("{second}.pending"))
            .exists());
    }

    // -- one password -----------------------------------------------------------

    #[test]
    fn unlocking_main_opens_the_second_vault() {
        let folder = Folder::new();
        let (spaces, second) = two(&folder);
        let store = spaces.store(Some(&second)).unwrap();
        let host = spaces
            .save_host(on(
                &second,
                with_password(draft("db", "192.0.2.7"), "hunter2"),
            ))
            .unwrap();
        assert!(!spaces.is_locked(&second));

        spaces.lock_all();
        assert_eq!(spaces.main().vault_status().unwrap(), VaultStatus::Locked);
        assert_eq!(store.vault_status().unwrap(), VaultStatus::Locked);
        // While main is closed nothing is "locked": the app's own lock
        // screen is what shows.
        assert!(!spaces.is_locked(&second));
        // Without the main key the chain is left alone.
        spaces.unlock_chain();
        assert_eq!(store.vault_status().unwrap(), VaultStatus::Locked);
        assert!(store.vault_is_remembered().unwrap());

        spaces.main().unlock_vault(b"main").unwrap();
        spaces.unlock_chain();
        assert_eq!(store.vault_status().unwrap(), VaultStatus::Unlocked);
        assert!(!spaces.is_locked(&second));
        assert_eq!(
            &**store.reveal_host_password(host.host.id).unwrap(),
            b"hunter2"
        );
        drop(store);
        drop(spaces);

        // And after a restart.
        let spaces = folder.open();
        spaces.main().unlock_vault(b"main").unwrap();
        spaces.unlock_chain();
        let store = spaces.store(Some(&second)).unwrap();
        assert_eq!(
            &**store.reveal_host_password(host.host.id).unwrap(),
            b"hunter2"
        );
    }

    #[test]
    fn a_broken_chain_reports_locked_and_the_servers_password_repairs_it() {
        let folder = Folder::new();
        let (spaces, second) = two(&folder);
        let store = spaces.store(Some(&second)).unwrap();
        store.forget_remembered_vault().unwrap();
        spaces.lock_all();
        spaces.main().unlock_vault(b"main").unwrap();
        spaces.unlock_chain();
        assert!(spaces.is_locked(&second));
        assert!(!spaces.is_locked(&spaces.main_id()));

        store.unlock_vault(b"second").unwrap();
        spaces.reseal_chain();
        assert!(!spaces.is_locked(&second));
        spaces.lock_all();
        spaces.main().unlock_vault(b"main").unwrap();
        spaces.unlock_chain();
        assert_eq!(store.vault_status().unwrap(), VaultStatus::Unlocked);
    }

    #[test]
    fn a_chained_key_opens_for_its_own_space_only() {
        let folder = Folder::new();
        let (spaces, second) = two(&folder);
        let main = spaces.main();
        let blob = main.seal_for_space(&second, &[7u8; 32]).unwrap();
        assert_eq!(&**main.open_for_space(&second, &blob).unwrap(), &[7u8; 32]);
        // The key is not in the blob as it stands.
        assert!(!blob.windows(8).any(|window| window == [7u8; 8]));

        let never = |result: std::io::Result<Zeroizing<Vec<u8>>>| {
            assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::InvalidData);
        };
        never(main.open_for_space("another", &blob));
        never(main.open_for_space(&second, &blob[..blob.len() - 1]));
        never(main.open_for_space(&second, &[]));
        never(main.open_for_space(&second, &[200, 1, 2]));
        let mut bent = blob.clone();
        *bent.last_mut().unwrap() ^= 1;
        never(main.open_for_space(&second, &bent));

        // Another space's blob in this space's row does not open it, and is
        // forgotten for it.
        let other = main.seal_for_space("another", &[7u8; 32]).unwrap();
        let store = spaces.store(Some(&second)).unwrap();
        store.remember_vault(|_| Ok(other.clone())).unwrap();
        store.lock_vault();
        spaces.unlock_chain();
        assert!(spaces.is_locked(&second));
        assert!(!store.vault_is_remembered().unwrap());

        // With main closed it is an error that may pass.
        main.lock_vault();
        let closed = main.open_for_space(&second, &blob).unwrap_err();
        assert_ne!(closed.kind(), std::io::ErrorKind::InvalidData);
        assert!(main.seal_for_space(&second, &[7u8; 32]).is_err());
    }

    #[test]
    fn a_new_main_key_seals_the_chain_again() {
        let folder = Folder::new();
        let (spaces, second) = two(&folder);
        let main = spaces.main();
        let store = spaces.store(Some(&second)).unwrap();

        // Main joins another vault: its key is a different one from here on.
        let vault = main.vault.lock().as_ref().unwrap().vault_id();
        let install = || {
            *main.vault.lock() = Some(UnlockedVault::from_key(vault, Zeroizing::new([9u8; 32])));
        };
        install();
        spaces.reseal_chain();
        spaces.lock_all();
        install();
        spaces.unlock_chain();
        assert_eq!(store.vault_status().unwrap(), VaultStatus::Unlocked);

        // Under the old key that chain does not open: it really was sealed anew.
        spaces.lock_all();
        main.unlock_vault(b"main").unwrap();
        spaces.unlock_chain();
        assert!(spaces.is_locked(&second));
    }

    // -- moving a host ------------------------------------------------------------

    struct Moved {
        spaces: Spaces,
        main: String,
        second: String,
        a: Uuid,
        b: Uuid,
        key: Uuid,
        tunnel: Uuid,
    }

    /// Host `a` with a password, a key, a tunnel and a trusted host key, and
    /// host `b` that shares the key — all on main.
    fn movable(folder: &Folder) -> Moved {
        let (spaces, second) = two(folder);
        let main = spaces.main_id();
        let key = spaces.add_key(key_draft("laptop")).unwrap().id;
        let mut a = with_password(draft("a", "a.example"), "hunter2");
        a.auth = AuthMethod::Key;
        a.key_id = Some(key);
        a.group_path = Some("web".into());
        let a = spaces.save_host(on(&main, a)).unwrap().host.id;
        let mut b = draft("b", "b.example");
        b.auth = AuthMethod::Key;
        b.key_id = Some(key);
        let b = spaces.save_host(on(&main, b)).unwrap().host.id;
        let tunnel = spaces.save_tunnel(tunnel(a)).unwrap().id;
        let store = spaces.main();
        store
            .trust_host_key("a.example", 22, "ssh-ed25519", "SHA256:aaaa", "AAAAa")
            .unwrap();
        store
            .trust_host_key("b.example", 22, "ssh-ed25519", "SHA256:bbbb", "AAAAb")
            .unwrap();
        store.set_host_os(a, Some("debian")).unwrap();
        store.mark_connected(a).unwrap();
        Moved {
            spaces,
            main,
            second,
            a,
            b,
            key,
            tunnel,
        }
    }

    #[test]
    fn a_host_moves_with_everything_it_uses() {
        let folder = Folder::new();
        let m = movable(&folder);
        let (main, second) = (m.spaces.main(), m.spaces.store(Some(&m.second)).unwrap());
        let before = main.get_host(m.a).unwrap().unwrap();

        assert_eq!(m.spaces.move_host_to(m.a, &m.second).unwrap(), m.a);
        assert_eq!(m.spaces.space_of_host(m.a).unwrap(), Some(m.second.clone()));
        assert_eq!(
            names(&m.spaces),
            vec![("b".into(), m.main.clone()), ("a".into(), m.second.clone())]
        );

        // In the target: the same host, readable under that vault.
        let after = second.get_host(m.a).unwrap().unwrap();
        assert_eq!(after, before);
        assert_eq!(&**second.reveal_host_password(m.a).unwrap(), b"hunter2");
        let key = second.reveal_host_key(m.a).unwrap();
        assert!(key.private_key.starts_with(b"-----BEGIN OPENSSH"));
        assert_eq!(
            key.passphrase.as_deref().map(|p| &p[..]),
            Some(&b"meow"[..])
        );
        assert_eq!(after.key_id, Some(m.key));
        let tunnels = second.list_tunnels().unwrap();
        assert_eq!(tunnels.len(), 1);
        assert_eq!((tunnels[0].id, tunnels[0].host_id), (m.tunnel, m.a));
        assert!(tunnels[0].autostart);
        let known = second.known_host("a.example", 22).unwrap().unwrap();
        assert_eq!(known.fingerprint, "SHA256:aaaa");
        assert!(second.known_host("b.example", 22).unwrap().is_none());
        assert_eq!(
            m.spaces.space_of_tunnel(m.tunnel).unwrap(),
            Some(m.second.clone())
        );
        assert!(second
            .list_groups()
            .unwrap()
            .iter()
            .any(|g| g.name == "web"));

        // In the source: the host, its login, password, tunnel and host key
        // are gone; the key stays, because `b` still uses it.
        assert!(main.get_host(m.a).unwrap().is_none());
        assert!(main.list_tunnels().unwrap().is_empty());
        assert!(main.known_host("a.example", 22).unwrap().is_none());
        assert!(main.known_host("b.example", 22).unwrap().is_some());
        assert_eq!(main.get_key(m.key).unwrap().unwrap().hosts, 1);
        assert_eq!(live(&main, "identities"), 1);
        // The key's two halves, and nothing of `a`.
        assert_eq!(live(&main, "secrets"), 2);
        // One key to the interface, used by both hosts.
        let keys = m.spaces.list_keys().unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].hosts, 2);

        // The last host using it takes the key along for good.
        m.spaces.move_host_to(m.b, &m.second).unwrap();
        assert!(main.get_key(m.key).unwrap().is_none());
        assert_eq!(live(&main, "secrets"), 0);
        assert_eq!(second.get_key(m.key).unwrap().unwrap().hosts, 2);
        assert_eq!(live(&second, "keys"), 1);
        assert!(main.list_known_hosts().unwrap().is_empty());
    }

    #[test]
    fn a_host_that_comes_back_is_whole_again_under_a_new_id() {
        let folder = Folder::new();
        let m = movable(&folder);
        let (main, second) = (m.spaces.main(), m.spaces.store(Some(&m.second)).unwrap());
        m.spaces.move_host_to(m.a, &m.second).unwrap();
        m.spaces.move_host_to(m.b, &m.second).unwrap();

        // Main holds tombstones of the host, its tunnel and the key. A
        // tombstone beats every later write on every other device, so what
        // comes back does so under ids of its own.
        let back = m.spaces.move_host_to(m.a, &m.main).unwrap();
        assert_ne!(back, m.a);
        assert_eq!(m.spaces.space_of_host(back).unwrap(), Some(m.main.clone()));
        assert!(m.spaces.get_host(m.a).unwrap().is_none());
        assert_eq!(
            names(&m.spaces),
            vec![("b".into(), m.second.clone()), ("a".into(), m.main.clone())]
        );
        let host = main.get_host(back).unwrap().unwrap();
        assert_eq!(host.group_path.as_deref(), Some("web"));
        assert_eq!(host.os.as_deref(), Some("debian"));
        assert!(host.last_connected_ms.is_some());
        assert_eq!(&**main.reveal_host_password(back).unwrap(), b"hunter2");
        let key = host.key_id.unwrap();
        assert_ne!(key, m.key);
        assert!(main
            .reveal_host_key(back)
            .unwrap()
            .private_key
            .starts_with(b"-----BEGIN"));
        let tunnels = main.list_tunnels().unwrap();
        assert_eq!(tunnels.len(), 1);
        assert_ne!(tunnels[0].id, m.tunnel);
        assert_eq!(tunnels[0].host_id, back);
        assert_eq!(
            main.known_host("a.example", 22)
                .unwrap()
                .unwrap()
                .fingerprint,
            "SHA256:aaaa"
        );
        // The tombstones are still what they were.
        assert_eq!(main.host_row(m.a).unwrap(), Some(true));

        // `b` follows: the key that came back is the one it uses, not a third.
        let b = m.spaces.move_host_to(m.b, &m.main).unwrap();
        assert_eq!(main.get_host(b).unwrap().unwrap().key_id, Some(key));
        assert_eq!(live(&main, "keys"), 1);
        assert_eq!(live(&second, "keys"), 0);
        assert_eq!(live(&second, "hosts"), 0);
        assert_eq!(live(&second, "secrets"), 0);
    }

    #[test]
    fn a_move_needs_the_target_vault_and_changes_nothing_without_it() {
        let folder = Folder::new();
        let m = movable(&folder);
        let second = m.spaces.store(Some(&m.second)).unwrap();
        second.lock_vault();
        assert!(matches!(
            m.spaces.move_host_to(m.a, &m.second),
            Err(StoreError::VaultLocked)
        ));
        assert_eq!(m.spaces.space_of_host(m.a).unwrap(), Some(m.main.clone()));
        assert_eq!(live(&second, "hosts"), 0);
        assert!(m.spaces.journal().is_none());
        assert!(m.spaces.move_host_to(m.a, "nowhere").is_err());
        assert!(matches!(
            m.spaces.move_host_to(Uuid::now_v7(), &m.second),
            Err(StoreError::UnknownHost(_))
        ));
        // A host with nothing sealed moves without any vault.
        let plain = m
            .spaces
            .save_host(on(&m.main, draft("plain", "192.0.2.9")))
            .unwrap();
        m.spaces.lock_all();
        m.spaces.move_host_to(plain.host.id, &m.second).unwrap();
        assert_eq!(
            m.spaces.space_of_host(plain.host.id).unwrap(),
            Some(m.second.clone())
        );
    }

    #[test]
    fn the_host_form_moves_a_host_and_brings_its_key() {
        let folder = Folder::new();
        let m = movable(&folder);
        let second = m.spaces.store(Some(&m.second)).unwrap();

        // An existing host saved with the other server.
        let mut edit = draft("a renamed", "a.example");
        edit.id = Some(m.a);
        edit.auth = AuthMethod::Key;
        edit.key_id = Some(m.key);
        edit.group_path = Some("web".into());
        let saved = m.spaces.save_host(on(&m.second, edit.clone())).unwrap();
        assert_eq!(
            (saved.space.as_str(), saved.host.id),
            (m.second.as_str(), m.a)
        );
        assert_eq!(saved.host.name, "a renamed");
        assert!(saved.host.has_password);
        assert_eq!(&**second.reveal_host_password(m.a).unwrap(), b"hunter2");
        // Saved again without a server: it stays where it is.
        let again = m
            .spaces
            .save_host(SpaceHostDraft {
                draft: edit,
                space: None,
            })
            .unwrap();
        assert_eq!(again.space, m.second);

        // A draft that does not validate moves nothing.
        let mut bad = draft("b", " ");
        bad.id = Some(m.b);
        assert!(m.spaces.save_host(on(&m.second, bad)).is_err());
        assert_eq!(m.spaces.space_of_host(m.b).unwrap(), Some(m.main.clone()));

        // A new host on the second server with a key only main has.
        let other = m.spaces.add_key(key_draft("desk")).unwrap().id;
        assert_eq!(m.spaces.space_of_key(other).unwrap(), Some(m.main.clone()));
        let mut new = draft("c", "c.example");
        new.auth = AuthMethod::Key;
        new.key_id = Some(other);
        let c = m.spaces.save_host(on(&m.second, new)).unwrap();
        assert_eq!(c.host.key_id, Some(other));
        assert!(second
            .reveal_host_key(c.host.id)
            .unwrap()
            .private_key
            .starts_with(b"-----"));
        assert!(m.spaces.main().get_key(other).unwrap().is_some());
        let mut unknown = draft("d", "d.example");
        unknown.auth = AuthMethod::Key;
        unknown.key_id = Some(Uuid::now_v7());
        assert!(matches!(
            m.spaces.save_host(on(&m.second, unknown)),
            Err(StoreError::Invalid { field: "keyId", .. })
        ));
        assert!(m
            .spaces
            .save_host(on("nowhere", draft("e", "e.example")))
            .is_err());

        // Keys are one list; renaming and deleting reach both servers.
        m.spaces.rename_key(other, "desk 2").unwrap();
        assert_eq!(second.get_key(other).unwrap().unwrap().label, "desk 2");
        assert_eq!(
            m.spaces.main().get_key(other).unwrap().unwrap().label,
            "desk 2"
        );
        assert!(matches!(
            m.spaces.delete_key(other),
            Err(StoreError::KeyInUse { hosts: 1 })
        ));
        m.spaces.delete_host(c.host.id).unwrap();
        m.spaces.delete_key(other).unwrap();
        assert!(m.spaces.get_key(other).unwrap().is_none());
        assert!(second.get_key(other).unwrap().is_none());
        assert!(m.spaces.reveal_key(m.key).is_ok());
    }

    #[test]
    fn a_host_key_another_host_there_needs_stays_and_a_trusted_one_is_not_replaced() {
        let folder = Folder::new();
        let m = movable(&folder);
        let (main, second) = (m.spaces.main(), m.spaces.store(Some(&m.second)).unwrap());
        // A second login on the same machine stays on main.
        let mut twin = draft("a as admin", "A.example");
        twin.username = "admin".into();
        m.spaces.save_host(on(&m.main, twin)).unwrap();
        // The second server already trusts another key for that address.
        second
            .trust_host_key("a.example", 22, "ssh-ed25519", "SHA256:theirs", "AAAAt")
            .unwrap();

        m.spaces.move_host_to(m.a, &m.second).unwrap();
        assert!(main.known_host("a.example", 22).unwrap().is_some());
        assert_eq!(
            second
                .known_host("a.example", 22)
                .unwrap()
                .unwrap()
                .fingerprint,
            "SHA256:theirs"
        );
    }

    #[test]
    fn a_move_cut_short_is_finished_or_forgotten_on_the_next_start() {
        // Cut short after the target was written.
        let folder = Folder::new();
        let m = movable(&folder);
        let moved = m.spaces.main().export_moved_host(m.a).unwrap();
        {
            let mut inner = m.spaces.inner.write();
            inner.journal = Some(Journal {
                host: m.a,
                target_host: m.a,
                from: m.main.clone(),
                to: m.second.clone(),
            });
            m.spaces.persist(&mut inner).unwrap();
        }
        m.spaces
            .store(Some(&m.second))
            .unwrap()
            .import_moved_host(&moved, m.a)
            .unwrap();
        drop(moved);
        // Both stores have it; it is listed once, where it was going.
        assert_eq!(
            names(&m.spaces),
            vec![("b".into(), m.main.clone()), ("a".into(), m.second.clone())]
        );
        assert_eq!(m.spaces.space_of_host(m.a).unwrap(), Some(m.second.clone()));
        let Moved {
            spaces,
            main,
            second,
            a,
            tunnel,
            key,
            ..
        } = m;
        drop(spaces);

        // The next start finishes it, with every vault still closed.
        let spaces = folder.open();
        assert!(spaces.journal().is_none());
        assert!(spaces.main().get_host(a).unwrap().is_none());
        assert!(spaces.main().list_tunnels().unwrap().is_empty());
        assert!(spaces.main().get_key(key).unwrap().is_some());
        assert_eq!(
            names(&spaces),
            vec![("b".into(), main), ("a".into(), second.clone())]
        );
        assert_eq!(spaces.space_of_tunnel(tunnel).unwrap(), Some(second));
        drop(spaces);
        let registry = std::fs::read_to_string(folder.registry()).unwrap();
        assert!(!registry.contains(&a.to_string()));

        // Cut short before the target was written.
        let folder = Folder::new();
        let m = movable(&folder);
        {
            let mut inner = m.spaces.inner.write();
            inner.journal = Some(Journal {
                host: m.a,
                target_host: m.a,
                from: m.main.clone(),
                to: m.second.clone(),
            });
            m.spaces.persist(&mut inner).unwrap();
        }
        assert_eq!(m.spaces.space_of_host(m.a).unwrap(), Some(m.main.clone()));
        let Moved {
            spaces,
            main,
            a,
            tunnel,
            ..
        } = m;
        drop(spaces);
        let spaces = folder.open();
        assert!(spaces.journal().is_none());
        assert_eq!(spaces.space_of_host(a).unwrap(), Some(main.clone()));
        assert_eq!(spaces.space_of_tunnel(tunnel).unwrap(), Some(main));
        assert_eq!(spaces.list_hosts().unwrap().len(), 2);
    }

    #[test]
    fn a_note_of_a_move_from_a_server_to_itself_takes_no_host_away() {
        let folder = Folder::new();
        let m = movable(&folder);
        let (main, a) = (m.main.clone(), m.a);
        {
            let mut inner = m.spaces.inner.write();
            inner.journal = Some(Journal {
                host: a,
                target_host: a,
                from: main.clone(),
                to: main.clone(),
            });
            m.spaces.persist(&mut inner).unwrap();
        }
        // Not followed while running either.
        m.spaces.recover().unwrap();
        assert!(m.spaces.main().get_host(a).unwrap().is_some());
        {
            let mut inner = m.spaces.inner.write();
            inner.journal = Some(Journal {
                host: a,
                target_host: a,
                from: main.clone(),
                to: main.clone(),
            });
            m.spaces.persist(&mut inner).unwrap();
        }
        drop(m);
        let spaces = folder.open();
        assert!(spaces.journal().is_none());
        assert_eq!(spaces.space_of_host(a).unwrap(), Some(main));
        assert_eq!(spaces.list_hosts().unwrap().len(), 2);

        // Nor one that names a server there is none of.
        let mut registry: serde_json::Value =
            serde_json::from_slice(&std::fs::read(folder.registry()).unwrap()).unwrap();
        registry["journal"] = serde_json::json!({
            "host": a, "targetHost": a, "from": "nobody", "to": spaces.main_id(),
        });
        drop(spaces);
        std::fs::write(folder.registry(), registry.to_string()).unwrap();
        let spaces = folder.open();
        assert!(spaces.journal().is_none());
        assert_eq!(spaces.list_hosts().unwrap().len(), 2);
    }

    #[test]
    fn a_move_stopped_after_the_host_left_still_takes_its_key_and_host_key_along() {
        let folder = Folder::new();
        let m = movable(&folder);
        let source = m.spaces.main();
        let moved = source.export_moved_host(m.a).unwrap();
        m.spaces
            .store(Some(&m.second))
            .unwrap()
            .import_moved_host(&moved, m.a)
            .unwrap();
        // Nothing else here uses what the host used.
        source.delete_host(m.b).unwrap();
        // Only the first step of letting go happened.
        source.delete_host(m.a).unwrap();
        assert!(source.get_key(m.key).unwrap().is_some());
        let known = |store: &Store| {
            store
                .moved_known_host(&moved.host.address, moved.host.port)
                .unwrap()
                .is_some()
        };
        let had_host_key = known(&source);
        source.drop_moved_host(m.a).unwrap();
        assert!(source.get_key(m.key).unwrap().is_none());
        assert!(!known(&source));
        assert_eq!(
            had_host_key,
            known(&m.spaces.store(Some(&m.second)).unwrap())
        );
        // Asked once more, it has nothing left to do.
        source.drop_moved_host(m.a).unwrap();
        source.drop_moved_host(Uuid::now_v7()).unwrap();
    }

    #[test]
    fn a_move_keeps_what_a_newer_build_wrote_into_the_rows() {
        let folder = Folder::new();
        let m = movable(&folder);
        let source = m.spaces.main();
        let extra = r#"{"future":"kept"}"#;
        {
            let conn = source.conn.lock();
            for (table, id) in [
                ("hosts", m.a.to_string()),
                ("port_forwards", m.tunnel.to_string()),
                ("keys", m.key.to_string()),
            ] {
                conn.execute(
                    &format!("UPDATE {table} SET sync_extra = ?2 WHERE id = ?1"),
                    params![id, extra],
                )
                .unwrap();
            }
            conn.execute(
                "UPDATE identities SET sync_extra = ?2
                  WHERE id = (SELECT identity_id FROM hosts WHERE id = ?1)",
                params![m.a.to_string(), extra],
            )
            .unwrap();
        }
        let id = m.spaces.move_host_to(m.a, &m.second).unwrap();
        let target = m.spaces.store(Some(&m.second)).unwrap();
        let conn = target.conn.lock();
        let login: String = conn
            .query_row(
                "SELECT identity_id FROM hosts WHERE id = ?1",
                [id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        for (table, id) in [
            ("hosts", id.to_string()),
            ("port_forwards", m.tunnel.to_string()),
            ("keys", m.key.to_string()),
            ("identities", login),
        ] {
            assert_eq!(
                extra_of(&conn, table, &id).unwrap().as_deref(),
                Some(extra),
                "{table}"
            );
        }
    }

    // -- groups and order -----------------------------------------------------------

    fn order(spaces: &Spaces, group: &str) -> Vec<(String, i64)> {
        spaces
            .list_hosts()
            .unwrap()
            .into_iter()
            .filter(|host| host.host.group_path.as_deref() == Some(group))
            .map(|host| (host.host.name, host.host.position))
            .collect()
    }

    fn grouped(name: &str, group: &str) -> HostDraft {
        let mut host = draft(name, &format!("{name}.example"));
        host.group_path = Some(group.into());
        host
    }

    #[test]
    fn a_group_holds_hosts_of_both_servers_in_one_order() {
        let folder = Folder::new();
        let (spaces, second) = two(&folder);
        let main = spaces.main_id();
        let a = spaces.save_host(on(&main, grouped("a", "web"))).unwrap();
        let b = spaces.save_host(on(&second, grouped("b", "web"))).unwrap();
        let c = spaces.save_host(on(&main, grouped("c", "web"))).unwrap();
        let d = spaces.save_host(on(&second, grouped("d", "web"))).unwrap();
        assert_eq!(
            order(&spaces, "web"),
            vec![
                ("a".into(), 0),
                ("b".into(), 1),
                ("c".into(), 2),
                ("d".into(), 3)
            ]
        );

        let revs = |spaces: &Spaces| -> Vec<i64> {
            [&a, &b, &c, &d]
                .iter()
                .map(|host| {
                    spaces
                        .store(Some(&host.space))
                        .unwrap()
                        .conn
                        .lock()
                        .query_row(
                            "SELECT rev FROM hosts WHERE id = ?1",
                            [host.host.id.to_string()],
                            |row| row.get(0),
                        )
                        .unwrap()
                })
                .collect()
        };
        let before = revs(&spaces);

        // `d` (second) before `b` (second), between two hosts of main.
        let moved = spaces
            .move_host(d.host.id, Workspace::Private, Some("web"), Some(b.host.id))
            .unwrap();
        assert_eq!(
            (moved.space.as_str(), moved.host.position),
            (second.as_str(), 1)
        );
        assert_eq!(
            order(&spaces, "web"),
            vec![
                ("a".into(), 0),
                ("d".into(), 1),
                ("b".into(), 2),
                ("c".into(), 3)
            ]
        );
        // `a` kept its place and was not written.
        let after = revs(&spaces);
        assert_eq!(after[0], before[0]);
        assert!(after[1] > before[1] && after[2] > before[2] && after[3] > before[3]);

        // To the end, and into another group and workspace.
        spaces
            .move_host(a.host.id, Workspace::Private, Some("web"), None)
            .unwrap();
        assert_eq!(
            order(&spaces, "web"),
            vec![
                ("d".into(), 0),
                ("b".into(), 1),
                ("c".into(), 2),
                ("a".into(), 3)
            ]
        );
        let out = spaces
            .move_host(b.host.id, Workspace::Business, None, None)
            .unwrap();
        assert_eq!(out.host.workspace, Workspace::Business);
        assert_eq!(out.host.group_path, None);
        assert_eq!(out.space, second);
        assert_eq!(order(&spaces, "web").len(), 3);
        // Putting a host where it is closes the gap `b` left, once, and
        // then writes nothing.
        spaces
            .move_host(a.host.id, Workspace::Private, Some("web"), None)
            .unwrap();
        let before = revs(&spaces);
        spaces
            .move_host(a.host.id, Workspace::Private, Some("web"), None)
            .unwrap();
        assert_eq!(revs(&spaces), before);
    }

    #[test]
    fn groups_are_one_list_and_change_on_every_server_that_has_them() {
        let folder = Folder::new();
        let (spaces, second) = two(&folder);
        let main = spaces.main_id();
        spaces.set_default(Workspace::Business, &second).unwrap();
        spaces.save_host(on(&main, grouped("a", "web"))).unwrap();
        spaces.save_host(on(&second, grouped("b", "web"))).unwrap();
        spaces.save_host(on(&second, grouped("c", "db"))).unwrap();

        let groups = spaces.list_groups().unwrap();
        let listed: Vec<&str> = groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(listed.len(), 2);
        assert!(listed.contains(&"web") && listed.contains(&"db"));

        // Created on the default server of its workspace, once.
        spaces.create_group(Workspace::Business, "office").unwrap();
        let store = spaces.store(Some(&second)).unwrap();
        assert!(store
            .list_groups()
            .unwrap()
            .iter()
            .any(|g| g.name == "office"));
        assert!(!spaces
            .main()
            .list_groups()
            .unwrap()
            .iter()
            .any(|g| g.name == "office"));
        assert!(matches!(
            spaces.create_group(Workspace::Private, "db"),
            Err(StoreError::Invalid {
                problem: "exists",
                ..
            })
        ));

        spaces
            .rename_group(Workspace::Private, "web", "www")
            .unwrap();
        assert_eq!(order(&spaces, "www").len(), 2);
        assert!(order(&spaces, "web").is_empty());
        assert!(spaces.rename_group(Workspace::Private, "web", "x").is_err());

        spaces
            .move_group(Workspace::Private, "www", Workspace::Business, None)
            .unwrap();
        assert!(spaces
            .list_hosts()
            .unwrap()
            .iter()
            .filter(|h| h.host.group_path.as_deref() == Some("www"))
            .all(|h| h.host.workspace == Workspace::Business));

        spaces.delete_group(Workspace::Business, "www").unwrap();
        assert!(!spaces
            .list_groups()
            .unwrap()
            .iter()
            .any(|g| g.name == "www"));
        assert!(spaces.delete_group(Workspace::Business, "www").is_err());
    }

    #[test]
    fn a_new_host_goes_to_the_default_server_of_its_workspace() {
        let folder = Folder::new();
        let (spaces, second) = two(&folder);
        let main = spaces.main_id();
        spaces.set_default(Workspace::Business, &second).unwrap();
        let none = |draft| SpaceHostDraft { draft, space: None };
        let mut work = draft("work", "192.0.2.20");
        work.workspace = Some(Workspace::Business);
        assert_eq!(spaces.save_host(none(work)).unwrap().space, second);
        assert_eq!(
            spaces
                .save_host(none(draft("home", "192.0.2.21")))
                .unwrap()
                .space,
            main
        );
        // The draft's own choice goes first.
        assert_eq!(
            spaces
                .save_host(on(&second, draft("lab", "192.0.2.22")))
                .unwrap()
                .space,
            second
        );
        // What the frontend sends reads.
        let sent: SpaceHostDraft = serde_json::from_value(serde_json::json!({
            "name": "sent", "address": "192.0.2.23", "port": 22, "username": "root",
            "auth": "password", "space": second,
        }))
        .unwrap();
        assert_eq!(sent.space.as_deref(), Some(second.as_str()));
        let saved = spaces.save_host(sent).unwrap();
        let json = serde_json::to_value(&saved).unwrap();
        assert_eq!(json["space"], second);
        assert_eq!(json["name"], "sent");
        let plain: SpaceHostDraft = serde_json::from_value(serde_json::json!({
            "name": "p", "address": "192.0.2.24", "port": 22, "username": "root",
            "auth": "password",
        }))
        .unwrap();
        assert!(plain.space.is_none());
    }

    // -- which server is main ---------------------------------------------------------

    fn snippet(label: &str) -> ImportSet {
        ImportSet {
            snippets: vec![SnippetInput {
                label: label.into(),
                body: format!("echo {label}"),
                group_path: None,
            }],
            ..ImportSet::default()
        }
    }

    #[test]
    fn another_server_becomes_main_and_takes_the_personal_things() {
        let folder = Folder::new();
        let (spaces, second) = two(&folder);
        let first = spaces.main_id();
        let old = spaces.main();
        let family = crate::KEYMAP_FAMILIES[0];
        let text = {
            let main = spaces.main();
            let text = Uuid::now_v7();
            main.save_keymap(
                family,
                KeymapDraft {
                    bindings: BTreeMap::new(),
                    texts: vec![KeymapTextDraft {
                        id: text,
                        label: "sudo".into(),
                        enter: true,
                        text: PasswordChange::Set {
                            value: SecretText::new("s3cret"),
                        },
                    }],
                },
            )
            .unwrap();
            text
        };
        old.save_assist_settings(AssistProviderDraft {
            active: "anthropic".into(),
            kind: "anthropic".into(),
            model: "some-model".into(),
            base_url: None,
            key: PasswordChange::Set {
                value: SecretText::new("api-key"),
            },
        })
        .unwrap();
        old.put_assist_cache(NewCacheEntry {
            platform: "linux".into(),
            request: "list files".into(),
            normalized: "list files".into(),
            command: "ls".into(),
            explanation: "lists".into(),
            dangerous: false,
        })
        .unwrap();
        old.import(snippet("hello")).unwrap();
        // This device opens without the password.
        old.remember_vault(os).unwrap();
        let host = spaces
            .save_host(on(&first, with_password(draft("h", "192.0.2.30"), "pw")))
            .unwrap();

        // Refused while one of the two is closed.
        let new = spaces.store(Some(&second)).unwrap();
        new.lock_vault();
        assert!(matches!(
            spaces.make_main(&second, &os),
            Err(StoreError::VaultLocked)
        ));
        assert_eq!(spaces.main_id(), first);
        spaces.unlock_chain();
        assert!(spaces.make_main("nowhere", &os).is_err());

        spaces.make_main(&second, &os).unwrap();
        assert_eq!(spaces.main_id(), second);
        assert_eq!(spaces.all()[0].id, second);
        assert!(Arc::ptr_eq(&spaces.main(), &new));
        // Making it main again is nothing.
        spaces.make_main(&second, &os).unwrap();

        // The new main has it all, readable under its own vault.
        let (typed, enter) = new.keymap_text(family, text).unwrap().unwrap();
        assert_eq!((typed.as_str(), enter), ("s3cret", true));
        assert_eq!(
            new.assist_api_key("anthropic").unwrap().unwrap().as_str(),
            "api-key"
        );
        assert_eq!(new.assist_settings().unwrap(), {
            let mut expected = new.assist_settings().unwrap();
            expected.provider = "anthropic".into();
            expected
        });
        let cache = new.assist_cache_entries(None).unwrap();
        assert_eq!(cache.len(), 1);
        assert_eq!(cache[0].command, "ls");
        assert_eq!(new.export_backup(false).unwrap().snippets.len(), 1);

        // The old one has none of it left — and no tombstone on the records
        // with fixed ids, so it could be main again.
        assert!(old.keymap(family).unwrap().texts.is_empty());
        assert!(old.keymap_text(family, text).unwrap().is_none());
        assert!(old.assist_api_key("anthropic").unwrap().is_none());
        assert!(old.assist_cache_entries(None).unwrap().is_empty());
        assert!(old.export_backup(false).unwrap().snippets.is_empty());
        assert_eq!(live(&old, "keymaps"), 1);
        assert_eq!(live(&old, "assist_config"), 1);
        // Only the host's password is still sealed there.
        assert_eq!(live(&old, "secrets"), 1);
        // Hosts stay where they are.
        assert_eq!(
            spaces.space_of_host(host.host.id).unwrap(),
            Some(first.clone())
        );

        // The key the system keeps is the new main's; the old main hangs on
        // the chain.
        spaces.lock_all();
        assert!(new.unlock_remembered_vault(os_open).unwrap());
        spaces.unlock_chain();
        assert_eq!(old.vault_status().unwrap(), VaultStatus::Unlocked);
        assert_eq!(&**old.reveal_host_password(host.host.id).unwrap(), b"pw");
        // And the password that opens the app is the new main's.
        spaces.lock_all();
        new.unlock_vault(b"second").unwrap();
        spaces.unlock_chain();
        assert!(!spaces.is_locked(&first));
        drop((old, new));
        drop(spaces);

        let spaces = folder.open();
        assert_eq!(spaces.main_id(), second);
        assert!(!spaces.all()[1].main);

        // And back: the first server takes its records up again.
        spaces.main().unlock_vault(b"second").unwrap();
        spaces.unlock_chain();
        spaces.make_main(&first, &os).unwrap();
        let main = spaces.main();
        assert_eq!(
            main.keymap_text(family, text).unwrap().unwrap().0.as_str(),
            "s3cret"
        );
        assert_eq!(
            main.assist_api_key("anthropic").unwrap().unwrap().as_str(),
            "api-key"
        );
        assert_eq!(main.assist_cache_entries(None).unwrap().len(), 1);
        assert_eq!(main.export_backup(false).unwrap().snippets.len(), 1);
    }

    #[test]
    fn without_a_key_kept_by_the_system_the_new_main_has_none_either() {
        let folder = Folder::new();
        let (spaces, second) = two(&folder);
        let first = spaces.main_id();
        spaces.make_main(&second, &os).unwrap();
        let new = spaces.main();
        // What it had was its chain to the old main: that opens nothing now.
        assert!(!new.vault_is_remembered().unwrap());
        assert!(spaces
            .store(Some(&first))
            .unwrap()
            .vault_is_remembered()
            .unwrap());
    }

    #[test]
    fn a_swap_of_the_main_server_cut_short_opens_with_one_password_afterwards() {
        // Stopped right after the list of servers was swapped: the keys are
        // prepared, none is in place.
        let folder = Folder::new();
        let (spaces, second) = two(&folder);
        let first = spaces.main_id();
        spaces.main().remember_vault(os).unwrap();
        spaces.swap_main(&second, &os).unwrap();
        drop(spaces);
        assert!(std::fs::read_to_string(folder.registry())
            .unwrap()
            .contains("handover"));

        // The next start puts them in place with every vault closed.
        let spaces = folder.open();
        assert_eq!(spaces.main_id(), second);
        assert!(!std::fs::read_to_string(folder.registry())
            .unwrap()
            .contains("handover"));
        assert!(spaces.main().unlock_remembered_vault(os_open).unwrap());
        spaces.unlock_chain();
        assert!(!spaces.is_locked(&first));
        assert!(matches!(
            spaces.store(Some(&first)).unwrap().vault_status().unwrap(),
            VaultStatus::Unlocked
        ));
        drop(spaces);

        // Stopped before the swap: the old order holds, and what was
        // prepared for the new one is gone.
        let folder = Folder::new();
        let (spaces, second) = two(&folder);
        let first = spaces.main_id();
        spaces.main().remember_vault(os).unwrap();
        let new = spaces.store(Some(&second)).unwrap();
        new.stage_unlock(Some(&os)).unwrap();
        let seal = |key: &[u8]| new.seal_for_space(&first, key);
        spaces.main().stage_unlock(Some(&seal)).unwrap();
        drop(new);
        drop(spaces);
        let spaces = folder.open();
        assert_eq!(spaces.main_id(), first);
        assert!(spaces.main().unlock_remembered_vault(os_open).unwrap());
        spaces.unlock_chain();
        assert!(!spaces.is_locked(&second));
        for space in spaces.all() {
            let staged: i64 = space
                .store
                .conn
                .lock()
                .query_row("SELECT count(*) FROM device_unlock_staged", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(staged, 0);
        }
        // And the swap still goes through afterwards.
        spaces.make_main(&second, &os).unwrap();
        drop(spaces);
        let spaces = folder.open();
        assert_eq!(spaces.main_id(), second);
        assert!(spaces.main().unlock_remembered_vault(os_open).unwrap());
        spaces.unlock_chain();
        assert!(!spaces.is_locked(&first));
    }

    // -- removing a server ---------------------------------------------------------

    #[test]
    fn a_removed_server_takes_its_entries_from_this_device() {
        let folder = Folder::new();
        let m = movable(&folder);
        m.spaces
            .set_default(Workspace::Business, &m.second)
            .unwrap();
        m.spaces.move_host_to(m.a, &m.second).unwrap();
        let file = folder.0.join("spaces").join(format!("{}.db", m.second));
        assert!(file.exists());

        m.spaces.remove(&m.second, false, &os).unwrap();
        assert!(!file.exists());
        assert!(!PathBuf::from(format!("{}-wal", file.display())).exists());
        assert_eq!(m.spaces.all().len(), 1);
        assert_eq!(names(&m.spaces), vec![("b".into(), m.main.clone())]);
        assert_eq!(m.spaces.defaults().business, m.main);
        assert!(m.spaces.has_room());
        assert!(m.spaces.get(&m.second).is_none());
        // The last one cannot be removed this way.
        assert!(matches!(
            m.spaces.remove(&m.main, false, &os),
            Err(StoreError::Invalid {
                problem: "last",
                ..
            })
        ));
        let Moved { spaces, main, .. } = m;
        drop(spaces);
        let spaces = folder.open();
        assert_eq!(spaces.all().len(), 1);
        assert_eq!(spaces.main_id(), main);
    }

    #[test]
    fn a_removal_stopped_half_way_keeps_the_server_from_syncing_until_it_is_done() {
        let folder = Folder::new();
        let (spaces, second) = two(&folder);
        let main = spaces.main_id();
        // The other vault is closed: an entry with a password cannot move.
        spaces.main().lock_vault();
        let secret = spaces
            .save_host(on(
                &second,
                with_password(draft("secret", "secret.example"), "hunter2"),
            ))
            .unwrap()
            .host
            .id;
        assert!(matches!(
            spaces.remove(&second, true, &os),
            Err(StoreError::VaultLocked)
        ));
        // Nothing left it, so nothing holds it back.
        assert!(!spaces.is_leaving(&second));

        let plain = spaces
            .save_host(on(&second, draft("plain", "plain.example")))
            .unwrap()
            .host
            .id;
        spaces
            .move_host(plain, Workspace::Private, None, Some(secret))
            .unwrap();
        let there = spaces.store(Some(&second)).unwrap().list_hosts().unwrap();
        assert_eq!(there[0].name, "plain");
        assert!(spaces.remove(&second, true, &os).is_err());
        assert!(spaces.is_leaving(&second));
        assert!(!spaces.is_leaving(&main));
        assert_eq!(spaces.all().len(), 2);
        drop(spaces);

        // A start in between does not forget it.
        let spaces = folder.open();
        assert!(spaces.is_leaving(&second));
        spaces.main().unlock_vault(b"main").unwrap();
        spaces.unlock_chain();
        spaces.remove(&second, true, &os).unwrap();
        assert!(!spaces.is_leaving(&second));
        assert_eq!(spaces.all().len(), 1);
        let mut left: Vec<String> = names(&spaces).into_iter().map(|(name, _)| name).collect();
        left.sort();
        assert_eq!(left, vec!["plain".to_string(), "secret".to_string()]);
        assert!(!std::fs::read_to_string(folder.registry())
            .unwrap()
            .contains("leaving"));
    }

    #[test]
    fn a_removed_servers_entries_can_be_taken_over() {
        let folder = Folder::new();
        let m = movable(&folder);
        let second = m.spaces.store(Some(&m.second)).unwrap();
        m.spaces.move_host_to(m.a, &m.second).unwrap();
        m.spaces
            .save_host(on(&m.second, with_password(grouped("c", "db"), "pw-c")))
            .unwrap();
        second.add_key(key_draft("spare")).unwrap();
        second.create_group(Workspace::Business, "empty").unwrap();
        drop(second);

        m.spaces.remove(&m.second, true, &os).unwrap();
        assert_eq!(m.spaces.all().len(), 1);
        let main = m.spaces.main();
        let hosts = names(&m.spaces);
        assert_eq!(hosts.len(), 3);
        assert!(hosts.iter().all(|(_, space)| *space == m.main));
        let c = main
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|h| h.name == "c")
            .unwrap();
        assert_eq!(&**main.reveal_host_password(c.id).unwrap(), b"pw-c");
        let a = main
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|h| h.name == "a")
            .unwrap();
        assert_eq!(&**main.reveal_host_password(a.id).unwrap(), b"hunter2");
        assert_eq!(main.list_tunnels().unwrap().len(), 1);
        let keys: Vec<String> = main
            .list_keys()
            .unwrap()
            .into_iter()
            .map(|k| k.label)
            .collect();
        assert!(keys.contains(&"spare".to_string()) && keys.contains(&"laptop".to_string()));
        let groups: Vec<String> = main
            .list_groups()
            .unwrap()
            .into_iter()
            .map(|g| g.name)
            .collect();
        for group in ["web", "db", "empty"] {
            assert!(groups.contains(&group.to_string()), "{group}");
        }
    }

    #[test]
    fn removing_the_main_server_makes_the_other_one_main() {
        let folder = Folder::new();
        let m = movable(&folder);
        let old = m.spaces.main();
        old.import(snippet("hello")).unwrap();
        old.remember_vault(os).unwrap();
        drop(old);

        m.spaces.remove(&m.main, true, &os).unwrap();
        assert_eq!(m.spaces.main_id(), m.second);
        assert_eq!(m.spaces.all().len(), 1);
        assert!(!folder.base().exists());
        let main = m.spaces.main();
        assert_eq!(main.list_hosts().unwrap().len(), 2);
        assert_eq!(main.export_backup(false).unwrap().snippets.len(), 1);
        assert_eq!(m.spaces.defaults().private, m.second);
        drop(main);
        let Moved { spaces, second, .. } = m;
        drop(spaces);

        let spaces = folder.open();
        assert_eq!(spaces.all().len(), 1);
        assert_eq!(spaces.main_id(), second);
        // The system's key opens it: one password, still.
        assert!(spaces.main().unlock_remembered_vault(os_open).unwrap());
        assert_eq!(spaces.main().list_hosts().unwrap().len(), 2);
        // Nothing of the old main is left beside it.
        assert!(!folder.base().exists());
        // And there is room for a second server again.
        let (id, store) = spaces.prepare().unwrap();
        store.create_vault_with(b"third", FAST).unwrap();
        spaces.register(&id, store).unwrap();
        assert_eq!(spaces.all().len(), 2);
    }

    // -- import and export -----------------------------------------------------------

    fn secret(text: &str) -> Zeroizing<String> {
        Zeroizing::new(text.to_owned())
    }

    fn sample() -> ImportSet {
        let host = |name: &str, address: &str, identity, workspace| HostInput {
            name: name.into(),
            address: address.into(),
            port: 22,
            group_path: Some("Lab".into()),
            identity,
            workspace,
            position: None,
        };
        ImportSet {
            groups: vec![GroupInput {
                workspace: Workspace::Business,
                name: "Empty".into(),
            }],
            keys: vec![
                KeyInput {
                    label: "shared".into(),
                    key_type: "ed25519".into(),
                    public_key: Some("ssh-ed25519 AAAA".into()),
                    private_key: secret("-----BEGIN OPENSSH PRIVATE KEY-----\n"),
                    passphrase: None,
                },
                KeyInput {
                    label: "loose".into(),
                    key_type: "ed25519".into(),
                    public_key: Some("ssh-ed25519 BBBB".into()),
                    private_key: secret("-----BEGIN OPENSSH PRIVATE KEY-----\n2"),
                    passphrase: None,
                },
            ],
            identities: vec![
                IdentityInput {
                    username: Some("root".into()),
                    key: Some(0),
                    ..IdentityInput::default()
                },
                IdentityInput {
                    username: Some("uwu".into()),
                    password: Some(secret("hunter2")),
                    ..IdentityInput::default()
                },
            ],
            hosts: vec![
                host("home", "home.example", Some(0), Workspace::Private),
                host("work", "work.example", Some(0), Workspace::Business),
                host("office", "office.example", Some(1), Workspace::Business),
                host("bare", "192.0.2.40", None, Workspace::Private),
            ],
            known_hosts: vec![KnownHostInput {
                address: "work.example".into(),
                port: 22,
                algorithm: "ssh-ed25519".into(),
                public_key: "AAAAw".into(),
                fingerprint: "SHA256:work".into(),
            }],
            snippets: vec![SnippetInput {
                label: "hello".into(),
                body: "echo hello".into(),
                group_path: None,
            }],
        }
    }

    #[test]
    fn an_import_puts_each_host_on_the_default_server_of_its_workspace() {
        let folder = Folder::new();
        let (spaces, second) = two(&folder);
        let main = spaces.main_id();
        spaces.set_default(Workspace::Business, &second).unwrap();

        let outcome = spaces.import(sample()).unwrap();
        assert_eq!(outcome.hosts_added, 4);
        assert_eq!(outcome.hosts_skipped, 0);
        assert_eq!(outcome.snippets_added, 1);
        assert_eq!(outcome.known_hosts_added, 1);
        // The shared key once per server that has a host using it, and the
        // loose one on main.
        assert_eq!(outcome.keys_added, 3);

        let mut placed = names(&spaces);
        placed.sort();
        assert_eq!(
            placed,
            vec![
                ("bare".to_string(), main.clone()),
                ("home".to_string(), main.clone()),
                ("office".to_string(), second.clone()),
                ("work".to_string(), second.clone()),
            ]
        );
        let (first, other) = (spaces.main(), spaces.store(Some(&second)).unwrap());
        let labels = |store: &Store| -> Vec<String> {
            store
                .list_keys()
                .unwrap()
                .into_iter()
                .map(|k| k.label)
                .collect()
        };
        assert_eq!(labels(&first), vec!["loose", "shared"]);
        assert_eq!(labels(&other), vec!["shared"]);
        let office = other
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|h| h.name == "office")
            .unwrap();
        assert_eq!(
            &**other.reveal_host_password(office.id).unwrap(),
            b"hunter2"
        );
        let work = other
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|h| h.name == "work")
            .unwrap();
        assert!(other.reveal_host_key(work.id).is_ok());
        // The host key went where its host went, and nowhere else.
        assert!(other.known_host("work.example", 22).unwrap().is_some());
        assert!(first.known_host("work.example", 22).unwrap().is_none());
        assert_eq!(first.export_backup(false).unwrap().snippets.len(), 1);
        assert!(other.export_backup(false).unwrap().snippets.is_empty());
        assert!(other
            .list_groups()
            .unwrap()
            .iter()
            .any(|g| g.name == "Empty"));
        assert!(!first
            .list_groups()
            .unwrap()
            .iter()
            .any(|g| g.name == "Empty"));

        // Again: everything is there already, wherever it is — also after the
        // defaults changed.
        spaces.set_default(Workspace::Business, &main).unwrap();
        let again = spaces.import(sample()).unwrap();
        assert_eq!(again.hosts_added, 0);
        assert_eq!(again.hosts_skipped, 4);
        assert_eq!(again.keys_added, 0);
        assert_eq!(spaces.list_hosts().unwrap().len(), 4);
    }

    #[test]
    fn a_backup_holds_both_servers_and_comes_back_onto_both() {
        let folder = Folder::new();
        let m = movable(&folder);
        m.spaces.move_host_to(m.a, &m.second).unwrap();
        m.spaces.main().import(snippet("hello")).unwrap();
        m.spaces
            .store(Some(&m.second))
            .unwrap()
            .create_group(Workspace::Business, "empty")
            .unwrap();

        let backup = m.spaces.export_backup(true).unwrap();
        assert_eq!(backup.hosts.len(), 2);
        // The key both servers have is in the file once.
        assert_eq!(backup.keys.len(), 1);
        assert_eq!(backup.tunnels.len(), 1);
        let a = backup.hosts.iter().position(|h| h.name == "a").unwrap();
        assert_eq!(backup.tunnels[0].host, a);
        assert_eq!(backup.hosts[a].key, Some(0));
        assert_eq!(
            backup.hosts[a].password.as_ref().unwrap().expose(),
            "hunter2"
        );
        assert_eq!(backup.known_hosts.len(), 2);
        assert_eq!(backup.snippets.len(), 1);
        assert!(backup.groups.iter().any(|g| g.name == "empty"));
        assert!(backup.groups.iter().any(|g| g.name == "web"));
        // Without secrets no vault is needed.
        m.spaces.lock_all();
        assert_eq!(m.spaces.export_backup(false).unwrap().hosts.len(), 2);
        assert!(matches!(
            m.spaces.export_backup(true),
            Err(StoreError::VaultLocked)
        ));

        // Into two fresh servers, with business on the second.
        let fresh = Folder::new();
        let (spaces, second) = two(&fresh);
        let main = spaces.main_id();
        spaces.set_default(Workspace::Private, &second).unwrap();
        let outcome = spaces.import_backup(backup).unwrap();
        assert_eq!(outcome.hosts_added, 2);
        assert_eq!(outcome.tunnels_added, 1);
        assert_eq!(outcome.snippets_added, 1);
        assert_eq!(outcome.keys_added, 1);
        // Both hosts are private: they went to the second server, tunnel,
        // key and all. Snippets stay on main.
        let other = spaces.store(Some(&second)).unwrap();
        assert_eq!(other.list_hosts().unwrap().len(), 2);
        assert!(spaces.main().list_hosts().unwrap().is_empty());
        let a = other
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|h| h.name == "a")
            .unwrap();
        assert_eq!(&**other.reveal_host_password(a.id).unwrap(), b"hunter2");
        assert!(other.reveal_host_key(a.id).is_ok());
        let tunnels = spaces.list_tunnels().unwrap();
        assert_eq!(tunnels.len(), 1);
        assert_eq!(tunnels[0].host_id, a.id);
        assert_eq!(
            spaces.main().export_backup(false).unwrap().snippets.len(),
            1
        );
        assert!(spaces
            .main()
            .list_groups()
            .unwrap()
            .iter()
            .any(|g| g.name == "empty"));
        assert_eq!(spaces.default_for(Workspace::Business).id, main);
    }

    #[test]
    fn one_server_imports_and_exports_as_it_always_did() {
        let folder = Folder::new();
        let spaces = folder.open();
        spaces.main().create_vault_with(b"main", FAST).unwrap();
        let outcome = spaces.import(sample()).unwrap();
        assert_eq!((outcome.hosts_added, outcome.keys_added), (4, 2));
        let direct = spaces.main().export_backup(true).unwrap();
        let through = spaces.export_backup(true).unwrap();
        assert_eq!(direct.summary(), through.summary());
        assert!(!folder.registry().exists());
    }
}
