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

use crate::{now_ms, tick, vault_id, Result, Store, StoreError};
use rusqlite::{params, Transaction};
use uuid::Uuid;
use uwussh_proto::{EntityKind, Hlc};
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
pub struct IdentityInput {
    pub label: Option<String>,
    pub username: Option<String>,
    pub password: Option<Secret>,
    /// Index into [`ImportSet::keys`].
    pub key: Option<usize>,
}

pub struct HostInput {
    pub name: String,
    pub address: String,
    pub port: u16,
    pub group_path: Option<String>,
    /// Index into [`ImportSet::identities`].
    pub identity: Option<usize>,
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
    pub hosts: Vec<HostInput>,
    pub identities: Vec<IdentityInput>,
    pub keys: Vec<KeyInput>,
    pub known_hosts: Vec<KnownHostInput>,
    pub snippets: Vec<SnippetInput>,
}

/// What an import changed. Duplicates are hosts whose `address:port` was
/// already present; they are left untouched.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImportOutcome {
    pub hosts_added: usize,
    pub hosts_skipped: usize,
    pub identities_added: usize,
    pub keys_added: usize,
    pub known_hosts_added: usize,
    pub snippets_added: usize,
}

impl Store {
    /// Write an import into the store under the unlocked vault. The vault must
    /// be unlocked: an import brings secrets, and without a vault there is
    /// nowhere safe to put them.
    pub fn import(&self, set: ImportSet) -> Result<ImportOutcome> {
        let mut conn = self.conn.lock();
        let vault_guard = self.vault.lock();
        let vault = vault_guard.as_ref().ok_or(StoreError::VaultLocked)?;

        let tx = conn.transaction()?;
        let vault_uuid = vault_id(&tx)?;
        let mut writer = Writer {
            tx: &tx,
            device: self.device,
            vault,
            vault_uuid,
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
    vault: &'a UnlockedVault,
    vault_uuid: String,
}

impl Writer<'_> {
    fn write(&mut self, set: &ImportSet) -> Result<ImportOutcome> {
        let mut outcome = ImportOutcome::default();

        // Keys, then identities that point at them, then hosts that point at
        // identities — the same order the importer built them in.
        let mut key_ids = Vec::with_capacity(set.keys.len());
        for key in &set.keys {
            key_ids.push(self.write_key(key)?);
            outcome.keys_added += 1;
        }

        let mut identity_ids = Vec::with_capacity(set.identities.len());
        for identity in &set.identities {
            let key_id = identity
                .key
                .and_then(|i| key_ids.get(i))
                .map(String::as_str);
            identity_ids.push(self.write_identity(identity, key_id)?);
            outcome.identities_added += 1;
        }

        for host in &set.hosts {
            if self.host_exists(&host.address, host.port)? {
                outcome.hosts_skipped += 1;
                continue;
            }
            let identity_id = host
                .identity
                .and_then(|i| identity_ids.get(i))
                .map(String::as_str);
            self.write_host(host, identity_id)?;
            outcome.hosts_added += 1;
        }

        for known in &set.known_hosts {
            if self.write_known_host(known)? {
                outcome.known_hosts_added += 1;
            }
        }

        for snippet in &set.snippets {
            self.write_snippet(snippet)?;
            outcome.snippets_added += 1;
        }

        Ok(outcome)
    }

    fn clock(&self) -> Result<Hlc> {
        tick(self.tx, self.device)
    }

    /// Seal a secret and store it, returning its id.
    fn write_secret(&self, plaintext: &[u8]) -> Result<String> {
        let id = Uuid::now_v7();
        let sealed = self.vault.seal(id, EntityKind::Secret, plaintext)?;
        let clock = self.clock()?;
        self.tx.execute(
            "INSERT INTO secrets
                (id, vault_id, nonce, blob, hlc_wall_ms, hlc_counter, hlc_device)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id.to_string(),
                self.vault_uuid,
                sealed.nonce,
                sealed.blob,
                clock.wall_ms as i64,
                clock.counter,
                clock.device,
            ],
        )?;
        Ok(id.to_string())
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

        let auth_type = if key_id.is_some() {
            "key"
        } else if password_secret_id.is_some() {
            "password"
        } else {
            // A login with neither, e.g. one that only names a username for the
            // agent to match. Password is the method every server offers.
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
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7, ?8, ?9, ?10)",
            params![
                id,
                self.vault_uuid,
                label,
                username,
                auth_type,
                password_secret_id,
                key_id,
                clock.wall_ms as i64,
                clock.counter,
                clock.device,
            ],
        )?;
        Ok(id)
    }

    fn host_exists(&self, address: &str, port: u16) -> Result<bool> {
        let count: i64 = self.tx.query_row(
            "SELECT count(*) FROM hosts
              WHERE deleted = 0 AND lower(address) = lower(?1) AND port = ?2",
            params![address, port],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    fn write_host(&self, host: &HostInput, identity_id: Option<&str>) -> Result<()> {
        let clock = self.clock()?;
        self.tx.execute(
            "INSERT INTO hosts
                (id, vault_id, name, address, port, identity_id, group_path,
                 hlc_wall_ms, hlc_counter, hlc_device)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                Uuid::now_v7().to_string(),
                self.vault_uuid,
                host.name,
                host.address,
                host.port,
                identity_id,
                host.group_path,
                clock.wall_ms as i64,
                clock.counter,
                clock.device,
            ],
        )?;
        Ok(())
    }

    /// Trust an imported host key, unless that address already has one — an
    /// import must never quietly replace a key the user is relying on.
    fn write_known_host(&self, known: &KnownHostInput) -> Result<bool> {
        let address = known.address.trim().to_ascii_lowercase();
        let taken: i64 = self.tx.query_row(
            "SELECT count(*) FROM known_hosts WHERE address = ?1 AND port = ?2 AND deleted = 0",
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
             ON CONFLICT (address, port) DO UPDATE SET
                algorithm = excluded.algorithm,
                fingerprint_sha256 = excluded.fingerprint_sha256,
                public_key = excluded.public_key,
                rev = rev + 1,
                deleted = 0",
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

    fn write_snippet(&self, snippet: &SnippetInput) -> Result<()> {
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
        Ok(())
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

    fn sample() -> ImportSet {
        ImportSet {
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
                },
                IdentityInput {
                    label: None,
                    username: Some("uwu".into()),
                    password: Some(secret("hunter2")),
                    key: None,
                },
            ],
            hosts: vec![
                HostInput {
                    name: "web".into(),
                    address: "10.0.0.5".into(),
                    port: 22,
                    group_path: Some("Homelab".into()),
                    identity: Some(1),
                },
                HostInput {
                    name: "pve".into(),
                    address: "10.0.0.6".into(),
                    port: 2222,
                    group_path: Some("Homelab/Proxmox".into()),
                    identity: Some(0),
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
    fn a_failure_partway_rolls_the_whole_import_back() {
        let store = unlocked_store();
        let mut set = sample();
        // An impossible port for the second host makes its INSERT fail the
        // CHECK constraint, after the first host and all secrets were written.
        set.hosts[1].port = 0;
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
