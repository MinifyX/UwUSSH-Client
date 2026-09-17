//! Export files: every host, group, key and trusted host key in one file, and
//! back in.
//!
//! A file without secrets is plain JSON, readable and diffable. A file with
//! them — stored passwords, private keys and their passphrases — is sealed as a
//! whole under a password of its own (see `uwussh_vault::password`): not the
//! master password, because the file may go to another device, another vault,
//! or another person. Nothing is ever written with secrets in the clear.
//!
//! Importing goes through [`Store::import`], so an export read back into the
//! same store adds nothing twice, and one read into another store keeps
//! workspaces, groups, their order and every login.

use crate::hosts::{AuthMethod, Workspace};
use crate::import::{
    GroupInput, HostInput, IdentityInput, ImportOutcome, ImportSet, KeyInput, KnownHostInput,
    SnippetInput,
};
use crate::secret::SecretText;
use crate::{now_ms, Result, Store, StoreError};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;
use uwussh_vault::{KdfParams, PasswordSealed};
use zeroize::Zeroizing;

const FORMAT: &str = "uwussh-export";
const VERSION: u32 = 1;
/// Larger than any real host list by orders of magnitude; a file bigger than
/// this is not an export.
pub const MAX_EXPORT_BYTES: usize = 64 * 1024 * 1024;

/// The most key derivation work a file may ask for. UwUSSH writes 64 MiB and
/// three passes; a file asking for much more wants to stall whoever opens it.
fn reasonable_for_a_file(kdf: &KdfParams) -> bool {
    kdf.within_limits()
        && kdf.memory_kib <= 256 * 1024
        && kdf.time_cost <= 8
        && kdf.parallelism <= 8
}

/// Secrets in a sealed file are base64: without escapes, the JSON reader hands
/// them over in place instead of copying them into a buffer nobody wipes.
mod secret_base64 {
    use super::{SecretText, BASE64};
    use base64::Engine as _;
    use serde::de::{self, Deserializer, Visitor};
    use serde::Serializer;
    use zeroize::Zeroizing;

    pub fn serialize<S: Serializer>(
        value: &Option<SecretText>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(secret) => {
                let encoded = Zeroizing::new(BASE64.encode(secret.expose().as_bytes()));
                serializer.serialize_some(encoded.as_str())
            }
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<SecretText>, D::Error> {
        struct Optional;
        struct Encoded;

        impl<'de> Visitor<'de> for Optional {
            type Value = Option<SecretText>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a base64 secret or null")
            }
            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(None)
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(None)
            }
            fn visit_some<D2: Deserializer<'de>>(
                self,
                inner: D2,
            ) -> Result<Self::Value, D2::Error> {
                inner.deserialize_str(Encoded).map(Some)
            }
        }

        impl<'de> Visitor<'de> for Encoded {
            type Value = SecretText;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a base64 secret")
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                let bytes = Zeroizing::new(
                    BASE64
                        .decode(value)
                        .map_err(|_| E::custom("a secret is damaged"))?,
                );
                let text =
                    std::str::from_utf8(&bytes).map_err(|_| E::custom("a secret is damaged"))?;
                Ok(SecretText::new(text.to_string()))
            }
        }

        deserializer.deserialize_option(Optional)
    }
}

/// JSON in a buffer allocated once, at its final size: a growing buffer
/// leaves copies of what it held behind, and this one holds secrets.
fn json_exact<T: Serialize>(value: &T) -> Result<Zeroizing<Vec<u8>>> {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let failed = |e: serde_json::Error| StoreError::Export(e.to_string());
    let mut count = Count(0);
    serde_json::to_writer(&mut count, value).map_err(failed)?;
    let mut out = Zeroizing::new(Vec::with_capacity(count.0));
    serde_json::to_writer(&mut *out, value).map_err(failed)?;
    Ok(out)
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Backup {
    #[serde(default)]
    pub groups: Vec<BackupGroup>,
    #[serde(default)]
    pub hosts: Vec<BackupHost>,
    #[serde(default)]
    pub keys: Vec<BackupKey>,
    #[serde(default)]
    pub known_hosts: Vec<BackupKnownHost>,
    #[serde(default)]
    pub snippets: Vec<BackupSnippet>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupGroup {
    pub workspace: Workspace,
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupHost {
    pub name: String,
    pub address: String,
    pub port: u16,
    pub username: String,
    #[serde(default)]
    pub workspace: Workspace,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub position: i64,
    pub auth: AuthMethod,
    #[serde(default)]
    pub key_path: Option<String>,
    /// Index into [`Backup::keys`].
    #[serde(default)]
    pub key: Option<usize>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "secret_base64"
    )]
    pub password: Option<SecretText>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupKey {
    pub label: String,
    pub key_type: String,
    #[serde(default)]
    pub public_key: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "secret_base64"
    )]
    pub private_key: Option<SecretText>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "secret_base64"
    )]
    pub passphrase: Option<SecretText>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupKnownHost {
    pub address: String,
    pub port: u16,
    pub algorithm: String,
    pub fingerprint: String,
    pub public_key: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupSnippet {
    pub label: String,
    pub body: String,
    #[serde(default)]
    pub group: Option<String>,
}

/// What a file holds, in counts, for the preview. Nothing identifying.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupSummary {
    pub hosts: usize,
    pub groups: usize,
    pub keys: usize,
    pub known_hosts: usize,
    pub snippets: usize,
    pub passwords: usize,
}

impl Backup {
    pub fn summary(&self) -> BackupSummary {
        BackupSummary {
            hosts: self.hosts.len(),
            groups: self.groups.len(),
            keys: self.keys.iter().filter(|k| k.private_key.is_some()).count(),
            known_hosts: self.known_hosts.len(),
            snippets: self.snippets.len(),
            passwords: self.hosts.iter().filter(|h| h.password.is_some()).count(),
        }
    }

    pub fn has_secrets(&self) -> bool {
        self.hosts.iter().any(|h| h.password.is_some())
            || self.keys.iter().any(|k| k.private_key.is_some())
    }
}

impl Store {
    /// Everything, ready to write. With `secrets`, stored passwords and vault
    /// keys come along, which needs the vault unlocked; without, hosts that
    /// log in with a vault key keep their name of it but not the key.
    pub fn export_backup(&self, secrets: bool) -> Result<Backup> {
        if secrets && self.vault.lock().is_none() {
            return Err(StoreError::VaultLocked);
        }
        let groups = self
            .list_groups()?
            .into_iter()
            .map(|g| BackupGroup {
                workspace: g.workspace,
                name: g.name,
            })
            .collect();

        let mut keys = Vec::new();
        let mut key_index: HashMap<Uuid, usize> = HashMap::new();
        for key in self.list_keys()? {
            let (private_key, passphrase) = if secrets {
                let revealed = self.reveal_key(key.id)?;
                (
                    Some(text(revealed.private_key)?),
                    revealed.passphrase.map(text).transpose()?,
                )
            } else {
                (None, None)
            };
            key_index.insert(key.id, keys.len());
            keys.push(BackupKey {
                label: key.label,
                key_type: key.key_type,
                public_key: key.public_key,
                private_key,
                passphrase,
            });
        }

        let mut hosts = Vec::new();
        for host in self.list_hosts()? {
            let password = if secrets && host.has_password {
                Some(text(self.reveal_host_password(host.id)?)?)
            } else {
                None
            };
            hosts.push(BackupHost {
                key: host.key_id.and_then(|id| key_index.get(&id).copied()),
                name: host.name,
                address: host.address,
                port: host.port,
                username: host.username,
                workspace: host.workspace,
                group: host.group_path,
                position: host.position,
                auth: host.auth,
                key_path: host.key_path,
                password,
            });
        }

        let known_hosts = self
            .list_known_hosts()?
            .into_iter()
            .map(|k| BackupKnownHost {
                address: k.address,
                port: k.port,
                algorithm: k.algorithm,
                fingerprint: k.fingerprint,
                public_key: k.public_key,
            })
            .collect();

        let snippets = self
            .list_snippets()?
            .into_iter()
            .map(|(label, body, group)| BackupSnippet { label, body, group })
            .collect();

        Ok(Backup {
            groups,
            hosts,
            keys,
            known_hosts,
            snippets,
        })
    }

    /// Read a backup into the store, skipping what is already there.
    pub fn import_backup(&self, backup: Backup) -> Result<ImportOutcome> {
        let Backup {
            groups,
            hosts,
            keys,
            known_hosts,
            snippets,
        } = backup;

        // Keys without their private half can't be used; hosts that pointed
        // at one ask for a password instead.
        let mut set_keys = Vec::new();
        let mut key_map: HashMap<usize, usize> = HashMap::new();
        for (index, key) in keys.into_iter().enumerate() {
            let Some(private_key) = key.private_key else {
                continue;
            };
            key_map.insert(index, set_keys.len());
            set_keys.push(KeyInput {
                label: key.label,
                key_type: key.key_type,
                public_key: Some(key.public_key).filter(|p| !p.is_empty()),
                private_key: private_key.into_inner(),
                passphrase: key.passphrase.map(SecretText::into_inner),
            });
        }

        // A file's host keys only for the hosts it brings: a host list must
        // not pre-trust keys for addresses the user adds some other day.
        let addresses: HashSet<(String, u16)> = hosts
            .iter()
            .map(|h| (h.address.trim().to_ascii_lowercase(), h.port))
            .collect();

        let mut identities = Vec::new();
        let mut set_hosts = Vec::new();
        for host in hosts {
            let key = match host.auth {
                AuthMethod::Key => host.key.and_then(|k| key_map.get(&k).copied()),
                AuthMethod::Password => None,
            };
            identities.push(IdentityInput {
                label: None,
                username: Some(host.username),
                password: host.password.map(SecretText::into_inner),
                key,
                key_path: match host.auth {
                    AuthMethod::Key if key.is_none() => host.key_path,
                    _ => None,
                },
            });
            set_hosts.push(HostInput {
                name: host.name,
                address: host.address,
                port: host.port,
                group_path: host.group,
                identity: Some(identities.len() - 1),
                workspace: host.workspace,
                position: None,
            });
        }

        self.import(ImportSet {
            groups: groups
                .into_iter()
                .map(|g| GroupInput {
                    workspace: g.workspace,
                    name: g.name,
                })
                .collect(),
            hosts: set_hosts,
            identities,
            keys: set_keys,
            known_hosts: known_hosts
                .into_iter()
                .filter(|k| addresses.contains(&(k.address.trim().to_ascii_lowercase(), k.port)))
                .map(|k| KnownHostInput {
                    address: k.address,
                    port: k.port,
                    algorithm: k.algorithm,
                    public_key: k.public_key,
                    fingerprint: k.fingerprint,
                })
                .collect(),
            snippets: snippets
                .into_iter()
                .map(|s| SnippetInput {
                    label: s.label,
                    body: s.body,
                    group_path: s.group,
                })
                .collect(),
        })
    }
}

impl Store {
    fn list_snippets(&self) -> Result<Vec<(String, String, Option<String>)>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT label, body, group_path FROM snippets WHERE deleted = 0 ORDER BY lower(label)",
        )?;
        let snippets = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(snippets)
    }
}

fn text(bytes: Zeroizing<Vec<u8>>) -> Result<SecretText> {
    std::str::from_utf8(&bytes)
        .map(SecretText::new)
        .map_err(|_| StoreError::Export("a stored secret is not text".into()))
}

// ── The file ────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Envelope {
    format: String,
    version: u32,
    #[serde(default)]
    created_ms: u64,
    #[serde(default)]
    app: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sealed: Option<SealedBody>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    data: Option<Backup>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SealedBody {
    kdf: String,
    memory_kib: u32,
    time_cost: u32,
    parallelism: u32,
    salt: String,
    nonce: String,
    blob: String,
}

/// Write a backup as a file. Secrets need a password; a backup without them is
/// written in the clear, whatever `password` says, so nobody locks away a file
/// that has nothing to hide and then forgets its password.
pub fn encode_export(
    backup: &Backup,
    password: Option<&[u8]>,
    app_version: &str,
) -> Result<Zeroizing<Vec<u8>>> {
    encode_export_with(backup, password, app_version, KdfParams::RECOMMENDED)
}

pub(crate) fn encode_export_with(
    backup: &Backup,
    password: Option<&[u8]>,
    app_version: &str,
    kdf: KdfParams,
) -> Result<Zeroizing<Vec<u8>>> {
    let mut envelope = Envelope {
        format: FORMAT.into(),
        version: VERSION,
        created_ms: now_ms(),
        app: app_version.into(),
        sealed: None,
        data: None,
    };
    let json = |value: &Envelope| {
        serde_json::to_vec_pretty(value)
            .map(Zeroizing::new)
            .map_err(|e| StoreError::Export(e.to_string()))
    };
    // A password always seals, secrets or not: whoever typed one was told
    // the file would be encrypted.
    let Some(password) = password.filter(|p| !p.is_empty()) else {
        if backup.has_secrets() {
            return Err(StoreError::ExportPasswordRequired);
        }
        return serialize_plain(&envelope, backup);
    };
    let inner = json_exact(backup)?;
    let sealed = uwussh_vault::seal_with_password(password, &inner, kdf)?;
    envelope.sealed = Some(SealedBody {
        kdf: "argon2id".into(),
        memory_kib: sealed.kdf.memory_kib,
        time_cost: sealed.kdf.time_cost,
        parallelism: sealed.kdf.parallelism,
        salt: BASE64.encode(sealed.salt),
        nonce: BASE64.encode(sealed.nonce),
        blob: BASE64.encode(&sealed.blob),
    });
    json(&envelope)
}

fn serialize_plain(envelope: &Envelope, backup: &Backup) -> Result<Zeroizing<Vec<u8>>> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct View<'a> {
        format: &'a str,
        version: u32,
        created_ms: u64,
        app: &'a str,
        data: &'a Backup,
    }
    serde_json::to_vec_pretty(&View {
        format: &envelope.format,
        version: envelope.version,
        created_ms: envelope.created_ms,
        app: &envelope.app,
        data: backup,
    })
    .map(Zeroizing::new)
    .map_err(|e| StoreError::Export(e.to_string()))
}

/// Whether a file needs a password to read, without reading it all.
pub fn export_is_sealed(bytes: &[u8]) -> Result<bool> {
    Ok(parse_envelope(bytes)?.sealed.is_some())
}

/// Read a file. A sealed one without a password is
/// [`StoreError::ExportPasswordRequired`]; with the wrong one,
/// [`StoreError::ExportPasswordWrong`].
pub fn decode_export(bytes: &[u8], password: Option<&[u8]>) -> Result<Backup> {
    let envelope = parse_envelope(bytes)?;
    match (envelope.sealed, envelope.data) {
        (Some(sealed), _) => {
            let password = password
                .filter(|p| !p.is_empty())
                .ok_or(StoreError::ExportPasswordRequired)?;
            let bytes = |field: &str| {
                BASE64
                    .decode(field)
                    .map_err(|_| StoreError::Export("the sealed part is damaged".into()))
            };
            if sealed.kdf != "argon2id" {
                return Err(StoreError::Export(format!(
                    "unknown key derivation {}",
                    sealed.kdf
                )));
            }
            let salt: [u8; 16] = bytes(&sealed.salt)?
                .try_into()
                .map_err(|_| StoreError::Export("the sealed part is damaged".into()))?;
            let nonce: [u8; 24] = bytes(&sealed.nonce)?
                .try_into()
                .map_err(|_| StoreError::Export("the sealed part is damaged".into()))?;
            let kdf = KdfParams {
                memory_kib: sealed.memory_kib,
                time_cost: sealed.time_cost,
                parallelism: sealed.parallelism,
            };
            if !reasonable_for_a_file(&kdf) {
                return Err(StoreError::Export(
                    "the file asks for far more key derivation work than an export needs".into(),
                ));
            }
            let sealed = PasswordSealed {
                kdf,
                salt,
                nonce,
                blob: bytes(&sealed.blob)?,
            };
            let inner = match uwussh_vault::open_with_password(password, &sealed) {
                Ok(inner) => inner,
                Err(uwussh_vault::VaultError::Decrypt) => {
                    return Err(StoreError::ExportPasswordWrong)
                }
                Err(other) => return Err(other.into()),
            };
            serde_json::from_slice(&inner).map_err(|e| StoreError::Export(e.to_string()))
        }
        (None, Some(data)) => Ok(data),
        (None, None) => Err(StoreError::Export("the file holds no data".into())),
    }
}

fn parse_envelope(bytes: &[u8]) -> Result<Envelope> {
    if bytes.len() > MAX_EXPORT_BYTES {
        return Err(StoreError::Export("the file is too large".into()));
    }
    let envelope: Envelope = serde_json::from_slice(bytes)
        .map_err(|_| StoreError::Export("this is not an UwUSSH export".into()))?;
    if envelope.format != FORMAT {
        return Err(StoreError::Export("this is not an UwUSSH export".into()));
    }
    if envelope.version > VERSION {
        return Err(StoreError::Export(format!(
            "the file was written by a newer UwUSSH (format {})",
            envelope.version
        )));
    }
    Ok(envelope)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hosts::{tests::draft, PasswordChange};
    use crate::keys::KeyDraft;

    const FAST: KdfParams = KdfParams::INSECURE_FOR_TESTS;

    /// A store with a bit of everything: two workspaces, a group, a password
    /// host, a vault-key host, a trusted host key.
    fn full_store() -> Store {
        let store = Store::open_in_memory().unwrap();
        store.create_vault_with(b"master", FAST).unwrap();
        store.create_group(Workspace::Business, "Clients").unwrap();
        store.create_group(Workspace::Private, "Empty").unwrap();

        let mut web = draft("web", "10.0.0.5");
        web.workspace = Some(Workspace::Business);
        web.group_path = Some("Clients".into());
        web.password = PasswordChange::Set {
            value: SecretText::new("hunter2"),
        };
        store.save_host(web).unwrap();

        let key = store
            .add_key(KeyDraft {
                label: "laptop".into(),
                key_type: "ssh-ed25519".into(),
                public_key: "ssh-ed25519 AAAA nyu".into(),
                private_key: SecretText::new("-----BEGIN OPENSSH PRIVATE KEY-----\nx\n"),
                passphrase: Some(SecretText::new("meow")),
            })
            .unwrap();
        let mut pve = draft("pve", "10.0.0.6");
        pve.auth = AuthMethod::Key;
        pve.key_id = Some(key.id);
        store.save_host(pve).unwrap();

        store
            .trust_host_key(
                "10.0.0.5",
                22,
                "ssh-ed25519",
                "SHA256:web",
                "ssh-ed25519 AAAA",
            )
            .unwrap();
        store
    }

    #[test]
    fn an_export_with_secrets_reads_back_into_a_new_store_whole() {
        let source = full_store();
        let backup = source.export_backup(true).unwrap();
        assert_eq!(
            backup.summary(),
            BackupSummary {
                hosts: 2,
                groups: 2,
                keys: 1,
                known_hosts: 1,
                snippets: 0,
                passwords: 1
            }
        );
        let file = encode_export_with(&backup, Some(b"file-pw"), "test", FAST).unwrap();
        let text = String::from_utf8(file.to_vec()).unwrap();
        assert!(!text.contains("hunter2"), "no secret in the clear");
        assert!(!text.contains("web"), "a sealed file hides host names too");
        assert!(export_is_sealed(&file).unwrap());

        let target = Store::open_in_memory().unwrap();
        target.create_vault_with(b"other", FAST).unwrap();
        let read = decode_export(&file, Some(b"file-pw")).unwrap();
        let outcome = target.import_backup(read).unwrap();
        assert_eq!(outcome.hosts_added, 2);

        let hosts = target.list_hosts().unwrap();
        let web = hosts.iter().find(|h| h.name == "web").unwrap();
        assert_eq!(web.workspace, Workspace::Business);
        assert_eq!(web.group_path.as_deref(), Some("Clients"));
        assert_eq!(
            target.reveal_host_password(web.id).unwrap().as_slice(),
            b"hunter2"
        );
        let pve = hosts.iter().find(|h| h.name == "pve").unwrap();
        assert_eq!(pve.key_label.as_deref(), Some("laptop"));
        let key = target.reveal_host_key(pve.id).unwrap();
        assert_eq!(key.passphrase.unwrap().as_slice(), b"meow");
        assert!(target
            .list_groups()
            .unwrap()
            .iter()
            .any(|g| g.name == "Empty"));
        assert!(target.known_host("10.0.0.5", 22).unwrap().is_some());

        // Reading it again adds nothing.
        let again = target
            .import_backup(decode_export(&file, Some(b"file-pw")).unwrap())
            .unwrap();
        assert_eq!(again.hosts_added, 0);
        assert_eq!(target.list_keys().unwrap().len(), 1);
    }

    #[test]
    fn a_sealed_file_needs_its_password() {
        let backup = full_store().export_backup(true).unwrap();
        assert!(matches!(
            encode_export_with(&backup, None, "test", FAST),
            Err(StoreError::ExportPasswordRequired)
        ));
        let file = encode_export_with(&backup, Some(b"file-pw"), "test", FAST).unwrap();
        assert!(matches!(
            decode_export(&file, None),
            Err(StoreError::ExportPasswordRequired)
        ));
        assert!(matches!(
            decode_export(&file, Some(b"wrong")),
            Err(StoreError::ExportPasswordWrong)
        ));
    }

    #[test]
    fn a_file_brings_host_keys_only_for_its_own_hosts() {
        let store = full_store();
        let mut backup = store.export_backup(true).unwrap();
        backup.known_hosts.push(BackupKnownHost {
            address: "git.example.org".into(),
            port: 22,
            algorithm: "ssh-ed25519".into(),
            fingerprint: "SHA256:someone-elses".into(),
            public_key: "ssh-ed25519 AAAA".into(),
        });
        let target = Store::open_in_memory().unwrap();
        target
            .create_vault_with(b"m", KdfParams::INSECURE_FOR_TESTS)
            .unwrap();
        target.import_backup(backup).unwrap();
        assert!(target.known_host("git.example.org", 22).unwrap().is_none());
    }

    #[test]
    fn a_file_asking_for_absurd_key_derivation_is_refused() {
        let store = full_store();
        let backup = store.export_backup(true).unwrap();
        let heavy = KdfParams {
            memory_kib: 1024 * 1024,
            time_cost: 16,
            parallelism: 16,
        };
        // Writing with such costs is possible; reading one back is not.
        let file = encode_export_with(&backup, Some(b"pw"), "test", FAST).unwrap();
        let text = String::from_utf8(file.to_vec()).unwrap().replace(
            &format!("\"memoryKib\": {}", FAST.memory_kib),
            &format!("\"memoryKib\": {}", heavy.memory_kib),
        );
        assert!(matches!(
            decode_export(text.as_bytes(), Some(b"pw")),
            Err(StoreError::Export(_))
        ));
    }

    #[test]
    fn an_export_without_secrets_is_plain_and_needs_no_vault() {
        let store = full_store();
        store.lock_vault();
        assert!(matches!(
            store.export_backup(true),
            Err(StoreError::VaultLocked)
        ));
        let backup = store.export_backup(false).unwrap();
        assert!(!backup.has_secrets());
        // A password seals even a file without secrets: whoever typed one
        // expects an encrypted file.
        let sealed = encode_export_with(&backup, Some(b"pw"), "test", FAST).unwrap();
        assert!(export_is_sealed(&sealed).unwrap());
        let file = encode_export_with(&backup, None, "test", FAST).unwrap();
        assert!(!export_is_sealed(&file).unwrap());
        let text = String::from_utf8(file.to_vec()).unwrap();
        assert!(text.contains("\"web\""));

        let target = Store::open_in_memory().unwrap();
        let outcome = target
            .import_backup(decode_export(&file, None).unwrap())
            .unwrap();
        assert_eq!(outcome.hosts_added, 2);
        let pve = target
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|h| h.name == "pve")
            .unwrap();
        assert_eq!(
            pve.auth,
            AuthMethod::Password,
            "without its key, the host asks for a password"
        );
    }

    #[test]
    fn something_else_is_not_read_as_an_export() {
        assert!(decode_export(b"{\"format\":\"other\",\"version\":1}", None).is_err());
        assert!(decode_export(b"not json", None).is_err());
        assert!(decode_export(b"{\"format\":\"uwussh-export\",\"version\":99}", None).is_err());
    }
}
