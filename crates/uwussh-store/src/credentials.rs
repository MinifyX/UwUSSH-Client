//! How a host logs in, resolved from its identity.
//!
//! A host points at an identity, and the identity says where its secret is: a
//! password to ask for, a key file on disk, or — for anything imported — a
//! password or key sealed in the vault. The connect path asks
//! [`Store::host_credential_source`] what to do, and only reveals a vault
//! secret when it is actually connecting, through [`Store::reveal_host_password`]
//! or [`Store::reveal_host_key`], which fail while the vault is locked.

use crate::vault::Revealed;
use crate::{Result, Store, StoreError};
use rusqlite::OptionalExtension;
use uuid::Uuid;

/// Where a host's login secret comes from. Carries nothing secret itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialSource {
    /// A password the user types on every connect (nothing stored).
    AskPassword,
    /// A private key file on disk.
    KeyFile { path: String },
    /// A password sealed in the vault.
    VaultPassword,
    /// A private key sealed in the vault.
    VaultKey,
}

/// A private key revealed from the vault, with its passphrase if it had one.
pub struct RevealedKey {
    pub private_key: Revealed,
    pub passphrase: Option<Revealed>,
}

/// The identity fields that decide how a host authenticates.
struct IdentityAuth {
    auth_type: Option<String>,
    key_path: Option<String>,
    password_secret_id: Option<String>,
    key_id: Option<String>,
}

impl Store {
    fn identity_auth(&self, host_id: Uuid) -> Result<IdentityAuth> {
        self.conn
            .lock()
            .query_row(
                "SELECT i.auth_type, i.key_path, i.password_secret_id, i.key_id
                   FROM hosts h
                   LEFT JOIN identities i ON i.id = h.identity_id AND i.deleted = 0
                  WHERE h.id = ?1 AND h.deleted = 0",
                [host_id.to_string()],
                |row| {
                    Ok(IdentityAuth {
                        auth_type: row.get(0)?,
                        key_path: row.get(1)?,
                        password_secret_id: row.get(2)?,
                        key_id: row.get(3)?,
                    })
                },
            )
            .optional()?
            .ok_or(StoreError::UnknownHost(host_id))
    }

    pub fn host_credential_source(&self, host_id: Uuid) -> Result<CredentialSource> {
        let auth = self.identity_auth(host_id)?;
        let key_auth = auth.auth_type.as_deref() == Some("key");
        Ok(match (key_auth, auth.key_path, auth.key_id) {
            (true, Some(path), _) => CredentialSource::KeyFile { path },
            (true, None, Some(_)) => CredentialSource::VaultKey,
            _ if auth.password_secret_id.is_some() => CredentialSource::VaultPassword,
            // Key auth without a key, or password auth without a stored one:
            // nothing to use, so ask.
            _ => CredentialSource::AskPassword,
        })
    }

    /// The stored password for a host. Fails if the vault is locked or the host
    /// has no stored password.
    pub fn reveal_host_password(&self, host_id: Uuid) -> Result<Revealed> {
        let id = self
            .identity_auth(host_id)?
            .password_secret_id
            .and_then(|s| Uuid::parse_str(&s).ok())
            .ok_or(StoreError::UnknownHost(host_id))?;
        self.reveal_secret(id)
    }

    /// The stored private key and passphrase for a host.
    pub fn reveal_host_key(&self, host_id: Uuid) -> Result<RevealedKey> {
        let key_id = self
            .identity_auth(host_id)?
            .key_id
            .and_then(|s| Uuid::parse_str(&s).ok())
            .ok_or(StoreError::UnknownHost(host_id))?;

        let (private, passphrase): (String, Option<String>) = self
            .conn
            .lock()
            .query_row(
                "SELECT private_secret_id, passphrase_secret_id
                   FROM keys WHERE id = ?1 AND deleted = 0",
                [key_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .ok_or(StoreError::UnknownHost(host_id))?;

        let private_id = Uuid::parse_str(&private).map_err(|_| StoreError::UnknownHost(host_id))?;
        let passphrase_id = passphrase.and_then(|s| Uuid::parse_str(&s).ok());

        Ok(RevealedKey {
            private_key: self.reveal_secret(private_id)?,
            passphrase: passphrase_id.map(|id| self.reveal_secret(id)).transpose()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::{HostInput, IdentityInput, ImportSet, KeyInput};
    use uwussh_vault::KdfParams;
    use zeroize::Zeroizing;

    fn secret(text: &str) -> Zeroizing<String> {
        Zeroizing::new(text.to_owned())
    }

    /// A store with one password host and one key host, both imported into the
    /// vault, plus one manually-added file-key host.
    fn store() -> Store {
        let store = Store::open_in_memory().unwrap();
        store
            .create_vault_with(b"master", KdfParams::INSECURE_FOR_TESTS)
            .unwrap();
        store
            .import(ImportSet {
                keys: vec![KeyInput {
                    label: "k".into(),
                    key_type: "ed25519".into(),
                    public_key: None,
                    private_key: secret("PRIVATE-KEY-BODY"),
                    passphrase: Some(secret("meow")),
                }],
                identities: vec![
                    IdentityInput {
                        label: None,
                        username: Some("pw".into()),
                        password: Some(secret("hunter2")),
                        key: None,
                        key_path: None,
                    },
                    IdentityInput {
                        label: None,
                        username: Some("keyed".into()),
                        password: None,
                        key: Some(0),
                        key_path: None,
                    },
                ],
                hosts: vec![
                    HostInput {
                        name: "pw-host".into(),
                        address: "10.0.0.1".into(),
                        port: 22,
                        group_path: None,
                        identity: Some(0),
                    },
                    HostInput {
                        name: "key-host".into(),
                        address: "10.0.0.2".into(),
                        port: 22,
                        group_path: None,
                        identity: Some(1),
                    },
                ],
                ..Default::default()
            })
            .unwrap();
        store
    }

    fn host(store: &Store, name: &str) -> Uuid {
        store
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|h| h.name == name)
            .unwrap()
            .id
    }

    #[test]
    fn a_file_key_host_reports_its_path() {
        let store = Store::open_in_memory().unwrap();
        let mut draft = crate::hosts::tests::draft("nas", "nas.lan");
        draft.auth = crate::AuthMethod::Key;
        draft.key_path = Some("~/.ssh/id_ed25519".into());
        let saved = store.save_host(draft).unwrap();
        assert_eq!(
            store.host_credential_source(saved.id).unwrap(),
            CredentialSource::KeyFile {
                path: "~/.ssh/id_ed25519".into()
            }
        );
    }

    #[test]
    fn a_manual_password_host_is_asked_every_time() {
        let store = Store::open_in_memory().unwrap();
        let saved = store
            .save_host(crate::hosts::tests::draft("web", "10.0.0.9"))
            .unwrap();
        assert_eq!(
            store.host_credential_source(saved.id).unwrap(),
            CredentialSource::AskPassword
        );
    }

    #[test]
    fn imported_hosts_resolve_to_the_vault() {
        let store = store();
        assert_eq!(
            store
                .host_credential_source(host(&store, "pw-host"))
                .unwrap(),
            CredentialSource::VaultPassword
        );
        assert_eq!(
            store
                .host_credential_source(host(&store, "key-host"))
                .unwrap(),
            CredentialSource::VaultKey
        );
    }

    #[test]
    fn a_vault_password_reveals_while_unlocked_and_not_after() {
        let store = store();
        let id = host(&store, "pw-host");
        assert_eq!(
            store.reveal_host_password(id).unwrap().as_slice(),
            b"hunter2"
        );

        store.lock_vault();
        assert!(matches!(
            store.reveal_host_password(id),
            Err(StoreError::VaultLocked)
        ));
    }

    #[test]
    fn a_vault_key_comes_back_with_its_passphrase() {
        let store = store();
        let revealed = store.reveal_host_key(host(&store, "key-host")).unwrap();
        assert_eq!(revealed.private_key.as_slice(), b"PRIVATE-KEY-BODY");
        assert_eq!(
            revealed.passphrase.as_ref().map(|p| p.as_slice()),
            Some(&b"meow"[..])
        );
    }

    #[test]
    fn asking_a_key_host_for_a_password_fails_cleanly() {
        let store = store();
        // The key host has no stored password.
        assert!(store
            .reveal_host_password(host(&store, "key-host"))
            .is_err());
    }
}
