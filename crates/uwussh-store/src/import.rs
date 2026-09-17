//! Writing an imported set into the store, secrets and all.
//!
//! The store defines its own plain input types rather than depending on the
//! importer crate: an importer maps its findings into these, and everything
//! after — sealing secrets, one transaction, deduplicating hosts against what
//! is already there — happens here, in one place, tested here.
//!
//! Secrets are sealed with the unlocked vault as they are written, so a
//! password or a private key is ciphertext by the time it reaches SQLite. The
//! whole set is one transaction: a failure halfway leaves the store as it was,
//! not half-imported.
//!
//! Importing the same source twice adds nothing twice. A host that is already
//! there — same address, port and user — is skipped, and so is the login that
//! only it needed; a key is only written for a login that is written, or, when
//! no login uses it, once per label and type.

use crate::hosts::{ensure_group, next_position, Workspace};
use crate::vault::seal_secret;
use crate::{now_ms, tick, vault_id, Result, Store, StoreError};
use rusqlite::{params, OptionalExtension, Transaction};
use std::collections::HashMap;
use uuid::Uuid;
use uwussh_vault::UnlockedVault;
use zeroize::Zeroizing;

/// A secret as it arrives from an importer.
pub type Secret = Zeroizing<String>;

/// A private key with its passphrase, referenced from identities by index.
pub struct KeyInput {
    pub label: String,
    /// `ed25519`, `rsa`, `ecdsa`, or the source's own word — stored as given.
    pub key_type: String,
    pub public_key: Option<String>,
    pub private_key: Secret,
    pub passphrase: Option<Secret>,
}

/// A username and how it logs in, referenced from hosts by index.
#[derive(Default)]
pub struct IdentityInput {
    pub label: Option<String>,
    pub username: Option<String>,
    pub password: Option<Secret>,
    /// Index into [`ImportSet::keys`] — a key that lives in the vault.
    pub key: Option<usize>,
    /// A private key file on disk (what PuTTY imports bring), used when there
    /// is no vault key. Nothing secret is read or stored: the file is
    /// referenced by path, as the source had it.
    pub key_path: Option<String>,
}

pub struct HostInput {
    pub name: String,
    pub address: String,
    pub port: u16,
    pub group_path: Option<String>,
    /// Index into [`ImportSet::identities`].
    pub identity: Option<usize>,
    pub workspace: Workspace,
    /// The place within its group, when the source has one (an UwUSSH export
    /// does); otherwise the host goes to the end.
    pub position: Option<i64>,
}

/// A group of its own, possibly empty, as an UwUSSH export carries it.
pub struct GroupInput {
    pub workspace: Workspace,
    pub name: String,
}

pub struct KnownHostInput {
    pub address: String,
    pub port: u16,
    pub algorithm: String,
    /// The base64 key blob, as in a `known_hosts` line.
    pub public_key: String,
    pub fingerprint: String,
}

pub struct SnippetInput {
    pub label: String,
    pub body: String,
    pub group_path: Option<String>,
}

/// Everything one import produced. Indices tie hosts to identities to keys, so
/// a key used by forty hosts is written once.
#[derive(Default)]
pub struct ImportSet {
    pub groups: Vec<GroupInput>,
    pub hosts: Vec<HostInput>,
    pub identities: Vec<IdentityInput>,
    pub keys: Vec<KeyInput>,
    pub known_hosts: Vec<KnownHostInput>,
    pub snippets: Vec<SnippetInput>,
}

/// What an import changed. Duplicates are hosts whose address, port and user
/// were already present; they are left untouched.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImportOutcome {
    pub hosts_added: usize,
    pub hosts_skipped: usize,
    pub identities_added: usize,
    pub keys_added: usize,
    pub known_hosts_added: usize,
    pub snippets_added: usize,
}

impl ImportSet {
    /// Whether writing this set has to seal anything. A PuTTY import — key
    /// files and typed passwords — has no secrets and so needs no vault.
    pub fn has_secrets(&self) -> bool {
        !self.keys.is_empty() || self.identities.iter().any(|i| i.password.is_some())
    }
}

impl Store {
    /// Write an import into the store. A secret it actually writes — a stored
    /// password or a key for the vault — needs the vault unlocked, since that
    /// is the only place a secret may go; otherwise the import fails with
    /// [`StoreError::VaultLocked`] and writes nothing. Secrets of hosts that
    /// are already there are never written, so importing the same setup again,
    /// or a set with no secrets at all, needs no vault.
    pub fn import(&self, set: ImportSet) -> Result<ImportOutcome> {
        // Far beyond any real setup; an import runs in one transaction that
        // holds the database, so there has to be an end.
        if set.hosts.len() > MAX_ITEMS
            || set.identities.len() > MAX_ITEMS
            || set.keys.len() > MAX_ITEMS
            || set.known_hosts.len() > MAX_ITEMS
            || set.snippets.len() > MAX_ITEMS
            || set.groups.len() > MAX_ITEMS
        {
            return Err(StoreError::Export(format!(
                "more than {MAX_ITEMS} entries of one kind is not something to import"
            )));
        }
        let mut conn = self.conn.lock();
        let vault_guard = self.vault.lock();
        let vault = vault_guard.as_ref();

        let tx = conn.transaction()?;
        let vault_uuid = vault_id(&tx)?;
        let mut writer = Writer {
            tx: &tx,
            device: self.device,
            vault,
            vault_uuid,
            key_ids: HashMap::new(),
            identity_ids: HashMap::new(),
        };
        let outcome = writer.write(&set)?;
        tx.commit()?;
        tracing::info!(?outcome, "import written");
        Ok(outcome)
    }
}

struct Writer<'a> {
    tx: &'a Transaction<'a>,
    device: u32,
    /// `None` when the set has no secrets to seal.
    vault: Option<&'a UnlockedVault>,
    vault_uuid: String,
    /// Keys and identities written so far, by their index in the set.
    key_ids: HashMap<usize, String>,
    identity_ids: HashMap<usize, String>,
}

/// The most entries of one kind a single import takes.
const MAX_ITEMS: usize = 50_000;

/// Whether an imported host is one the host form would have let through:
/// an address and user without spaces or control characters, a name without
/// control characters. What isn't is skipped, not written.
fn plausible(host: &HostInput, username: &str) -> bool {
    let clean = |text: &str| !text.chars().any(|c| c.is_control());
    let address = host.address.trim();
    !address.is_empty()
        && address.len() <= 253
        && !address.chars().any(|c| c.is_whitespace() || c.is_control())
        && !username
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
        && username.len() <= 256
        && clean(&host.name)
        && host.name.len() <= 256
        && host.port > 0
}

impl Writer<'_> {
    fn write(&mut self, set: &ImportSet) -> Result<ImportOutcome> {
        let mut outcome = ImportOutcome::default();

        for group in &set.groups {
            if let Some(name) = crate::hosts::group_name(Some(group.name.clone()))? {
                ensure_group(
                    self.tx,
                    self.device,
                    &self.vault_uuid,
                    group.workspace,
                    &name,
                )
                .map(drop)?;
            }
        }

        // Hosts pull in the identity they need, which pulls in its key: what
        // a skipped host alone needed is never written.
        for host in &set.hosts {
            let identity = host.identity.and_then(|i| set.identities.get(i));
            let username = identity
                .and_then(|i| i.username.as_deref())
                .unwrap_or_default();
            if !plausible(host, username) || self.host_exists(&host.address, host.port, username)? {
                outcome.hosts_skipped += 1;
                continue;
            }
            let identity_id = match host.identity.filter(|i| *i < set.identities.len()) {
                Some(index) => Some(self.identity(set, index, &mut outcome)?),
                None => None,
            };
            self.write_host(host, identity_id.as_deref())?;
            outcome.hosts_added += 1;
        }

        // Keys no login points at still belong in the vault, once.
        let used: Vec<usize> = set.identities.iter().filter_map(|i| i.key).collect();
        for (index, key) in set.keys.iter().enumerate() {
            if used.contains(&index) || self.key_exists(key)? {
                continue;
            }
            self.key(set, index, &mut outcome)?;
        }

        for known in &set.known_hosts {
            if self.write_known_host(known)? {
                outcome.known_hosts_added += 1;
            }
        }

        for snippet in &set.snippets {
            if self.write_snippet(snippet)? {
                outcome.snippets_added += 1;
            }
        }

        Ok(outcome)
    }

    fn clock(&self) -> Result<uwussh_proto::Hlc> {
        tick(self.tx, self.device)
    }

    /// Seal a secret and store it, returning its id.
    fn write_secret(&self, plaintext: &[u8]) -> Result<String> {
        let vault = self.vault.ok_or(StoreError::VaultLocked)?;
        seal_secret(self.tx, self.device, vault, plaintext)
    }

    fn key(
        &mut self,
        set: &ImportSet,
        index: usize,
        outcome: &mut ImportOutcome,
    ) -> Result<String> {
        if let Some(id) = self.key_ids.get(&index) {
            return Ok(id.clone());
        }
        let id = self.write_key(&set.keys[index])?;
        outcome.keys_added += 1;
        self.key_ids.insert(index, id.clone());
        Ok(id)
    }

    fn identity(
        &mut self,
        set: &ImportSet,
        index: usize,
        outcome: &mut ImportOutcome,
    ) -> Result<String> {
        if let Some(id) = self.identity_ids.get(&index) {
            return Ok(id.clone());
        }
        let identity = &set.identities[index];
        let key_id = match identity.key.filter(|k| *k < set.keys.len()) {
            Some(key) => Some(self.key(set, key, outcome)?),
            None => None,
        };
        let id = self.write_identity(identity, key_id.as_deref())?;
        outcome.identities_added += 1;
        self.identity_ids.insert(index, id.clone());
        Ok(id)
    }

    fn key_exists(&self, key: &KeyInput) -> Result<bool> {
        let count: i64 = self.tx.query_row(
            "SELECT count(*) FROM keys WHERE deleted = 0 AND label = ?1 AND key_type = ?2",
            params![key.label, key.key_type],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    fn write_key(&self, key: &KeyInput) -> Result<String> {
        let private_secret_id = self.write_secret(key.private_key.as_bytes())?;
        let passphrase_secret_id = key
            .passphrase
            .as_ref()
            .map(|p| self.write_secret(p.as_bytes()))
            .transpose()?;

        let id = Uuid::now_v7().to_string();
        let clock = self.clock()?;
        self.tx.execute(
            "INSERT INTO keys
                (id, vault_id, label, key_type, public_key,
                 private_secret_id, passphrase_secret_id,
                 hlc_wall_ms, hlc_counter, hlc_device)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                id,
                self.vault_uuid,
                key.label,
                key.key_type,
                key.public_key.clone().unwrap_or_default(),
                private_secret_id,
                passphrase_secret_id,
                clock.wall_ms as i64,
                clock.counter,
                clock.device,
            ],
        )?;
        Ok(id)
    }

    fn write_identity(&self, identity: &IdentityInput, key_id: Option<&str>) -> Result<String> {
        let password_secret_id = identity
            .password
            .as_ref()
            .map(|p| self.write_secret(p.as_bytes()))
            .transpose()?;

        // A file key is only used when there is no vault key.
        let key_path = key_id
            .is_none()
            .then(|| identity.key_path.clone())
            .flatten();
        let auth_type = if key_id.is_some() || key_path.is_some() {
            "key"
        } else {
            // Either a stored password, or a login that only names a username
            // and asks on connect. Password is the method every server offers.
            "password"
        };
        let username = identity.username.clone().unwrap_or_default();
        let label = identity
            .label
            .clone()
            .filter(|l| !l.trim().is_empty())
            .unwrap_or_else(|| username.clone());

        let id = Uuid::now_v7().to_string();
        let clock = self.clock()?;
        self.tx.execute(
            "INSERT INTO identities
                (id, vault_id, label, username, auth_type, key_path,
                 password_secret_id, key_id, hlc_wall_ms, hlc_counter, hlc_device)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                id,
                self.vault_uuid,
                label,
                username,
                auth_type,
                key_path,
                password_secret_id,
                key_id,
                clock.wall_ms as i64,
                clock.counter,
                clock.device,
            ],
        )?;
        Ok(id)
    }

    fn host_exists(&self, address: &str, port: u16, username: &str) -> Result<bool> {
        let count: i64 = self.tx.query_row(
            "SELECT count(*) FROM hosts h
               LEFT JOIN identities i ON i.id = h.identity_id AND i.deleted = 0
              WHERE h.deleted = 0 AND lower(h.address) = lower(?1) AND h.port = ?2
                AND coalesce(i.username, '') = ?3",
            params![address.trim(), port, username.trim()],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    fn write_host(&self, host: &HostInput, identity_id: Option<&str>) -> Result<()> {
        let group = match crate::hosts::group_name(host.group_path.clone())? {
            Some(name) => Some(ensure_group(
                self.tx,
                self.device,
                &self.vault_uuid,
                host.workspace,
                &name,
            )?),
            None => None,
        };
        let position = match host.position {
            Some(position) => position,
            None => next_position(self.tx, host.workspace, group.as_deref())?,
        };
        let clock = self.clock()?;
        self.tx.execute(
            "INSERT INTO hosts
                (id, vault_id, name, address, port, identity_id, group_id, workspace, position,
                 hlc_wall_ms, hlc_counter, hlc_device)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                Uuid::now_v7().to_string(),
                self.vault_uuid,
                host.name,
                host.address,
                host.port,
                identity_id,
                group,
                host.workspace.as_str(),
                position,
                clock.wall_ms as i64,
                clock.counter,
                clock.device,
            ],
        )?;
        Ok(())
    }

    /// Trust an imported host key, unless that address already has one — an
    /// import must never quietly replace a key the user is relying on, nor
    /// bring back one the user removed.
    fn write_known_host(&self, known: &KnownHostInput) -> Result<bool> {
        let address = known.address.trim().to_ascii_lowercase();
        let taken: i64 = self.tx.query_row(
            "SELECT count(*) FROM known_hosts WHERE address = ?1 AND port = ?2",
            params![address, known.port],
            |row| row.get(0),
        )?;
        if taken > 0 {
            return Ok(false);
        }
        let clock = self.clock()?;
        self.tx.execute(
            "INSERT INTO known_hosts
                (id, vault_id, address, port, algorithm, fingerprint_sha256, public_key,
                 first_seen_ms, hlc_wall_ms, hlc_counter, hlc_device)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT (address, port) DO NOTHING",
            params![
                Uuid::now_v7().to_string(),
                self.vault_uuid,
                address,
                known.port,
                known.algorithm,
                known.fingerprint,
                known.public_key,
                now_ms() as i64,
                clock.wall_ms as i64,
                clock.counter,
                clock.device,
            ],
        )?;
        Ok(true)
    }

    /// A snippet with the same label and body is already there: skip it.
    fn write_snippet(&self, snippet: &SnippetInput) -> Result<bool> {
        let exists = self
            .tx
            .query_row(
                "SELECT 1 FROM snippets WHERE deleted = 0 AND label = ?1 AND body = ?2",
                params![snippet.label, snippet.body],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if exists {
            return Ok(false);
        }
        let clock = self.clock()?;
        self.tx.execute(
            "INSERT INTO snippets
                (id, vault_id, label, body, group_path, hlc_wall_ms, hlc_counter, hlc_device)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                Uuid::now_v7().to_string(),
                self.vault_uuid,
                snippet.label,
                snippet.body,
                snippet.group_path,
                clock.wall_ms as i64,
                clock.counter,
                clock.device,
            ],
        )?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::VaultStatus;
    use uwussh_vault::KdfParams;

    fn unlocked_store() -> Store {
        let store = Store::open_in_memory().unwrap();
        store
            .create_vault_with(b"master", KdfParams::INSECURE_FOR_TESTS)
            .unwrap();
        store
    }

    fn secret(text: &str) -> Secret {
        Secret::new(text.to_owned())
    }

    fn host_id(store: &Store, name: &str) -> Uuid {
        store
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|h| h.name == name)
            .unwrap()
            .id
    }

    fn sample() -> ImportSet {
        ImportSet {
            groups: Vec::new(),
            keys: vec![KeyInput {
                label: "nyu-key".into(),
                key_type: "ed25519".into(),
                public_key: Some("ssh-ed25519 AAAA".into()),
                private_key: secret("-----BEGIN OPENSSH PRIVATE KEY-----\n"),
                passphrase: Some(secret("meow")),
            }],
            identities: vec![
                IdentityInput {
                    label: Some("root".into()),
                    username: Some("root".into()),
                    password: None,
                    key: Some(0),
                    key_path: None,
                },
                IdentityInput {
                    label: None,
                    username: Some("uwu".into()),
                    password: Some(secret("hunter2")),
                    key: None,
                    key_path: None,
                },
            ],
            hosts: vec![
                HostInput {
                    name: "web".into(),
                    address: "10.0.0.5".into(),
                    port: 22,
                    group_path: Some("Homelab".into()),
                    identity: Some(1),
                    workspace: Workspace::Private,
                    position: None,
                },
                HostInput {
                    name: "pve".into(),
                    address: "10.0.0.6".into(),
                    port: 2222,
                    group_path: Some("Homelab/Proxmox".into()),
                    identity: Some(0),
                    workspace: Workspace::Private,
                    position: None,
                },
            ],
            known_hosts: vec![KnownHostInput {
                address: "10.0.0.5".into(),
                port: 22,
                algorithm: "ssh-ed25519".into(),
                public_key: "AAAAC3NzaC1lZDI1NTE5".into(),
                fingerprint: "SHA256:web".into(),
            }],
            snippets: vec![SnippetInput {
                label: "update".into(),
                body: "apt update".into(),
                group_path: Some("maintenance".into()),
            }],
        }
    }

    #[test]
    fn a_whole_set_lands_with_hosts_logins_and_trusted_keys() {
        let store = unlocked_store();
        let outcome = store.import(sample()).unwrap();
        assert_eq!(
            outcome,
            ImportOutcome {
                hosts_added: 2,
                hosts_skipped: 0,
                identities_added: 2,
                keys_added: 1,
                known_hosts_added: 1,
                snippets_added: 1,
            }
        );

        let hosts = store.list_hosts().unwrap();
        assert_eq!(hosts.len(), 2);
        let web = hosts.iter().find(|h| h.name == "web").unwrap();
        assert_eq!((web.address.as_str(), web.port), ("10.0.0.5", 22));
        assert_eq!(web.username, "uwu");
        assert_eq!(web.group_path.as_deref(), Some("Homelab"));

        assert!(store.known_host("10.0.0.5", 22).unwrap().is_some());
    }

    #[test]
    fn a_stored_password_can_be_revealed_only_while_unlocked() {
        let store = unlocked_store();
        store.import(sample()).unwrap();

        let secret_id: String = store
            .conn
            .lock()
            .query_row(
                "SELECT s.id FROM secrets s
                   JOIN identities i ON i.password_secret_id = s.id
                  WHERE i.username = 'uwu'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let id = Uuid::parse_str(&secret_id).unwrap();

        let revealed = store.reveal_secret(id).unwrap();
        assert_eq!(revealed.as_slice(), b"hunter2");

        store.lock_vault();
        assert!(matches!(
            store.reveal_secret(id),
            Err(StoreError::VaultLocked)
        ));
    }

    #[test]
    fn a_key_is_written_once_and_shared_by_index() {
        let store = unlocked_store();
        let mut set = sample();
        // A second identity reusing key 0.
        set.identities.push(IdentityInput {
            label: Some("admin".into()),
            username: Some("admin".into()),
            password: None,
            key: Some(0),
            key_path: None,
        });
        store.import(set).unwrap();

        let keys: i64 = store
            .conn
            .lock()
            .query_row("SELECT count(*) FROM keys WHERE deleted = 0", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(keys, 1, "one key, two identities pointing at it");
    }

    #[test]
    fn importing_a_host_that_already_exists_skips_it() {
        let store = unlocked_store();
        store.import(sample()).unwrap();
        // The same set again: every host is now a duplicate.
        let outcome = store.import(sample()).unwrap();
        assert_eq!(outcome.hosts_added, 0);
        assert_eq!(outcome.hosts_skipped, 2);
        assert_eq!(store.list_hosts().unwrap().len(), 2);
    }

    #[test]
    fn importing_the_same_set_twice_writes_no_second_login_key_or_secret() {
        let store = unlocked_store();
        store.import(sample()).unwrap();
        let count = |table: &str| -> i64 {
            store
                .conn
                .lock()
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap()
        };
        let before = (count("identities"), count("keys"), count("secrets"));
        let again = store.import(sample()).unwrap();
        assert_eq!(
            again,
            ImportOutcome {
                hosts_skipped: 2,
                ..Default::default()
            }
        );
        assert_eq!(
            (count("identities"), count("keys"), count("secrets")),
            before
        );
    }

    #[test]
    fn two_logins_to_the_same_server_are_two_hosts() {
        let store = unlocked_store();
        let mut set = sample();
        set.hosts.push(HostInput {
            name: "web as root".into(),
            address: "10.0.0.5".into(),
            port: 22,
            group_path: None,
            identity: Some(0),
            workspace: Workspace::Business,
            position: None,
        });
        assert_eq!(store.import(set).unwrap().hosts_added, 3);
        let business = host_id(&store, "web as root");
        assert_eq!(
            store.get_host(business).unwrap().unwrap().workspace,
            Workspace::Business
        );
    }

    #[test]
    fn a_key_no_login_uses_is_kept_once() {
        let store = unlocked_store();
        let mut set = sample();
        set.keys.push(KeyInput {
            label: "spare".into(),
            key_type: "ed25519".into(),
            public_key: None,
            private_key: secret(
                "-----BEGIN OPENSSH PRIVATE KEY-----
",
            ),
            passphrase: None,
        });
        assert_eq!(store.import(set).unwrap().keys_added, 2);
        let mut again = sample();
        again.keys.push(KeyInput {
            label: "spare".into(),
            key_type: "ed25519".into(),
            public_key: None,
            private_key: secret(
                "-----BEGIN OPENSSH PRIVATE KEY-----
",
            ),
            passphrase: None,
        });
        assert_eq!(store.import(again).unwrap().keys_added, 0);
        assert_eq!(store.list_keys().unwrap().len(), 2);
    }

    #[test]
    fn groups_arrive_as_records_even_when_empty() {
        let store = Store::open_in_memory().unwrap();
        store
            .import(ImportSet {
                groups: vec![GroupInput {
                    workspace: Workspace::Business,
                    name: "Clients".into(),
                }],
                ..Default::default()
            })
            .unwrap();
        let groups = store.list_groups().unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].workspace, Workspace::Business);
    }

    #[test]
    fn an_import_never_overwrites_a_trusted_host_key() {
        let store = unlocked_store();
        store
            .trust_host_key(
                "10.0.0.5",
                22,
                "ssh-ed25519",
                "SHA256:mine",
                "ssh-ed25519 MINE",
            )
            .unwrap();

        let outcome = store.import(sample()).unwrap();
        assert_eq!(
            outcome.known_hosts_added, 0,
            "the existing key is left alone"
        );
        assert_eq!(
            store
                .known_host("10.0.0.5", 22)
                .unwrap()
                .unwrap()
                .fingerprint,
            "SHA256:mine"
        );
    }

    #[test]
    fn importing_into_a_locked_vault_is_refused_and_writes_nothing() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.vault_status().unwrap(), VaultStatus::Absent);
        assert!(matches!(
            store.import(sample()),
            Err(StoreError::VaultLocked)
        ));
        assert!(store.list_hosts().unwrap().is_empty());
    }

    #[test]
    fn importing_the_same_setup_again_needs_no_vault() {
        let store = unlocked_store();
        store.import(sample()).unwrap();
        let hosts = store.list_hosts().unwrap().len();

        store.lock_vault();
        let again = store.import(sample()).unwrap();
        assert_eq!(again.hosts_added, 0);
        assert_eq!(store.list_hosts().unwrap().len(), hosts);
    }

    #[test]
    fn a_secretless_import_needs_no_vault() {
        // What a PuTTY import looks like: a key-file host and an ask-password
        // host, no vault key, no stored password.
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.vault_status().unwrap(), VaultStatus::Absent);

        let set = ImportSet {
            identities: vec![
                IdentityInput {
                    username: Some("root".into()),
                    key_path: Some("~/.ssh/id_ed25519".into()),
                    ..Default::default()
                },
                IdentityInput {
                    username: Some("deploy".into()),
                    ..Default::default()
                },
            ],
            hosts: vec![
                HostInput {
                    name: "keyed".into(),
                    address: "10.0.0.1".into(),
                    port: 22,
                    group_path: None,
                    identity: Some(0),
                    workspace: Workspace::Private,
                    position: None,
                },
                HostInput {
                    name: "asked".into(),
                    address: "10.0.0.2".into(),
                    port: 22,
                    group_path: None,
                    identity: Some(1),
                    workspace: Workspace::Private,
                    position: None,
                },
            ],
            ..Default::default()
        };

        let outcome = store.import(set).unwrap();
        assert_eq!(outcome.hosts_added, 2);
        assert_eq!(outcome.identities_added, 2);

        let keyed = host_id(&store, "keyed");
        assert_eq!(
            store.host_credential_source(keyed).unwrap(),
            crate::CredentialSource::KeyFile {
                path: "~/.ssh/id_ed25519".into()
            }
        );
        let asked = host_id(&store, "asked");
        assert_eq!(
            store.host_credential_source(asked).unwrap(),
            crate::CredentialSource::AskPassword
        );
    }

    #[test]
    fn a_host_key_the_user_removed_is_not_brought_back() {
        let store = unlocked_store();
        store.import(sample()).unwrap();
        store
            .conn
            .lock()
            .execute("UPDATE known_hosts SET deleted = 1", [])
            .unwrap();
        store.import(sample()).unwrap();
        let live: i64 = store
            .conn
            .lock()
            .query_row(
                "SELECT count(*) FROM known_hosts WHERE deleted = 0",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(live, 0);
    }

    #[test]
    fn hosts_the_form_would_refuse_are_skipped() {
        let store = unlocked_store();
        let mut set = sample();
        set.hosts[0].address = "evil host".into();
        set.hosts[1].name = "bell\u{7}".into();
        let outcome = store.import(set).unwrap();
        assert_eq!(outcome.hosts_added, 0);
        assert_eq!(outcome.hosts_skipped, 2);
    }

    #[test]
    fn a_failure_partway_rolls_the_whole_import_back() {
        let store = unlocked_store();
        let mut set = sample();
        // A group name no group may have fails the second host, after the
        // first host and all secrets were written.
        set.hosts[1].group_path = Some("bad\u{7}group".into());
        assert!(store.import(set).is_err());
        assert!(store.list_hosts().unwrap().is_empty(), "nothing sticks");
        let secrets: i64 = store
            .conn
            .lock()
            .query_row("SELECT count(*) FROM secrets", [], |r| r.get(0))
            .unwrap();
        assert_eq!(secrets, 0, "sealed secrets were rolled back too");
    }
}
