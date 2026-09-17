//! Keys kept in the vault.
//!
//! A key is a label, its type, its public half in the clear — so the list can
//! show fingerprints and copy the line for `authorized_keys` without unlocking
//! anything — and its private half and passphrase sealed as secrets. Hosts
//! point at a key through their identity, so one key serves any number of
//! hosts, and a key still in use can't be deleted from under them.

use crate::credentials::RevealedKey;
use crate::secret::SecretText;
use crate::vault::{forget_secret, seal_secret};
use crate::{tick, vault_id, Result, Store, StoreError};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyRecord {
    pub id: Uuid,
    pub label: String,
    /// `ssh-ed25519`, `ssh-rsa`, … or what an importer called it.
    pub key_type: String,
    /// The OpenSSH public key line, empty when the source didn't have one.
    pub public_key: String,
    pub has_passphrase: bool,
    /// How many hosts log in with this key.
    pub hosts: usize,
}

/// A key to put into the vault.
#[derive(Debug)]
pub struct KeyDraft {
    pub label: String,
    pub key_type: String,
    pub public_key: String,
    pub private_key: SecretText,
    pub passphrase: Option<SecretText>,
}

const KEY_SELECT: &str = "SELECT k.id, k.label, k.key_type, k.public_key,
            k.passphrase_secret_id IS NOT NULL,
            (SELECT count(*) FROM identities i JOIN hosts h ON h.identity_id = i.id
              WHERE i.key_id = k.id AND i.deleted = 0 AND h.deleted = 0)
       FROM keys k";

fn key_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<KeyRecord> {
    let id: String = row.get(0)?;
    let hosts: i64 = row.get(5)?;
    Ok(KeyRecord {
        id: Uuid::parse_str(&id).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?,
        label: row.get(1)?,
        key_type: row.get(2)?,
        public_key: row.get(3)?,
        has_passphrase: row.get(4)?,
        hosts: hosts as usize,
    })
}

fn label_or_type(label: &str, key_type: &str) -> Result<String> {
    let label = label.trim();
    if label.chars().any(char::is_control) || label.chars().count() > 120 {
        return Err(StoreError::Invalid {
            field: "label",
            problem: "invalid",
        });
    }
    Ok(if label.is_empty() {
        key_type.to_string()
    } else {
        label.to_string()
    })
}

impl Store {
    pub fn list_keys(&self) -> Result<Vec<KeyRecord>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&format!(
            "{KEY_SELECT} WHERE k.deleted = 0 ORDER BY lower(k.label), k.id"
        ))?;
        let keys = stmt
            .query_map([], key_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(keys)
    }

    pub fn get_key(&self, id: Uuid) -> Result<Option<KeyRecord>> {
        Ok(self
            .conn
            .lock()
            .query_row(
                &format!("{KEY_SELECT} WHERE k.id = ?1 AND k.deleted = 0"),
                [id.to_string()],
                key_from_row,
            )
            .optional()?)
    }

    /// Seal a key into the vault. Needs the vault unlocked.
    pub fn add_key(&self, draft: KeyDraft) -> Result<KeyRecord> {
        if draft.private_key.expose().trim().is_empty() {
            return Err(StoreError::Invalid {
                field: "privateKey",
                problem: "required",
            });
        }
        let label = label_or_type(&draft.label, &draft.key_type)?;
        let mut conn = self.conn.lock();
        let vault_guard = self.vault.lock();
        let vault = vault_guard.as_ref().ok_or(StoreError::VaultLocked)?;
        let tx = conn.transaction()?;
        let private = seal_secret(
            &tx,
            self.device,
            vault,
            draft.private_key.expose().as_bytes(),
        )?;
        let passphrase = draft
            .passphrase
            .as_ref()
            .filter(|p| !p.expose().is_empty())
            .map(|p| seal_secret(&tx, self.device, vault, p.expose().as_bytes()))
            .transpose()?;
        let id = Uuid::now_v7();
        let clock = tick(&tx, self.device)?;
        tx.execute(
            "INSERT INTO keys
                (id, vault_id, label, key_type, public_key,
                 private_secret_id, passphrase_secret_id,
                 hlc_wall_ms, hlc_counter, hlc_device)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                id.to_string(),
                vault_id(&tx)?,
                label,
                draft.key_type,
                draft.public_key.trim(),
                private,
                passphrase,
                clock.wall_ms as i64,
                clock.counter,
                clock.device,
            ],
        )?;
        let record = tx.query_row(
            &format!("{KEY_SELECT} WHERE k.id = ?1"),
            [id.to_string()],
            key_from_row,
        )?;
        tx.commit()?;
        Ok(record)
    }

    pub fn rename_key(&self, id: Uuid, label: &str) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let key_type: String = tx
            .query_row(
                "SELECT key_type FROM keys WHERE id = ?1 AND deleted = 0",
                [id.to_string()],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(StoreError::UnknownKey(id))?;
        let label = label_or_type(label, &key_type)?;
        let clock = tick(&tx, self.device)?;
        tx.execute(
            "UPDATE keys
                SET label = ?2, rev = rev + 1,
                    hlc_wall_ms = ?3, hlc_counter = ?4, hlc_device = ?5
              WHERE id = ?1",
            params![
                id.to_string(),
                label,
                clock.wall_ms as i64,
                clock.counter,
                clock.device
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Delete a key and its sealed halves. Refused while a host logs in with it.
    pub fn delete_key(&self, id: Uuid) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let secrets: (Option<String>, Option<String>) = tx
            .query_row(
                "SELECT private_secret_id, passphrase_secret_id FROM keys
                  WHERE id = ?1 AND deleted = 0",
                [id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .ok_or(StoreError::UnknownKey(id))?;
        let hosts: i64 = tx.query_row(
            "SELECT count(*) FROM identities i JOIN hosts h ON h.identity_id = i.id
              WHERE i.key_id = ?1 AND i.deleted = 0 AND h.deleted = 0",
            [id.to_string()],
            |row| row.get(0),
        )?;
        if hosts > 0 {
            return Err(StoreError::KeyInUse {
                hosts: hosts as usize,
            });
        }
        for secret in [secrets.0, secrets.1].into_iter().flatten() {
            forget_secret(&tx, self.device, &secret)?;
        }
        let clock = tick(&tx, self.device)?;
        tx.execute(
            "UPDATE keys
                SET deleted = 1, rev = rev + 1,
                    hlc_wall_ms = ?2, hlc_counter = ?3, hlc_device = ?4
              WHERE id = ?1",
            params![
                id.to_string(),
                clock.wall_ms as i64,
                clock.counter,
                clock.device
            ],
        )?;
        tx.commit()?;
        crate::vault::truncate_wal(&conn);
        Ok(())
    }

    /// A key's private half and passphrase, opened. Fails while locked.
    pub fn reveal_key(&self, id: Uuid) -> Result<RevealedKey> {
        let (private, passphrase): (Option<String>, Option<String>) = self
            .conn
            .lock()
            .query_row(
                "SELECT private_secret_id, passphrase_secret_id
                   FROM keys WHERE id = ?1 AND deleted = 0",
                [id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .ok_or(StoreError::UnknownKey(id))?;
        let parse = |s: String| Uuid::parse_str(&s).map_err(|_| StoreError::UnknownKey(id));
        let private_id = parse(private.ok_or(StoreError::UnknownKey(id))?)?;
        let passphrase_id = passphrase.map(parse).transpose()?;
        Ok(RevealedKey {
            private_key: self.reveal_secret(private_id)?,
            passphrase: passphrase_id.map(|id| self.reveal_secret(id)).transpose()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hosts::{tests::draft, AuthMethod};
    use uwussh_vault::KdfParams;

    fn unlocked() -> Store {
        let store = Store::open_in_memory().unwrap();
        store
            .create_vault_with(b"master", KdfParams::INSECURE_FOR_TESTS)
            .unwrap();
        store
    }

    fn key_draft(label: &str) -> KeyDraft {
        KeyDraft {
            label: label.into(),
            key_type: "ssh-ed25519".into(),
            public_key: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIA nyu".into(),
            private_key: SecretText::new("-----BEGIN OPENSSH PRIVATE KEY-----\nbody\n"),
            passphrase: Some(SecretText::new("meow")),
        }
    }

    #[test]
    fn a_key_goes_in_sealed_and_comes_out_whole() {
        let store = unlocked();
        let key = store.add_key(key_draft("laptop")).unwrap();
        assert_eq!(key.label, "laptop");
        assert!(key.has_passphrase);
        assert_eq!(store.list_keys().unwrap(), vec![key.clone()]);

        let revealed = store.reveal_key(key.id).unwrap();
        assert!(revealed.private_key.starts_with(b"-----BEGIN OPENSSH"));
        assert_eq!(revealed.passphrase.unwrap().as_slice(), b"meow");

        store.lock_vault();
        assert!(matches!(
            store.reveal_key(key.id),
            Err(StoreError::VaultLocked)
        ));
        assert!(matches!(
            store.add_key(key_draft("x")),
            Err(StoreError::VaultLocked)
        ));
        // The list needs nothing unlocked.
        assert_eq!(store.list_keys().unwrap().len(), 1);
    }

    #[test]
    fn a_key_in_use_cannot_be_deleted_and_an_unused_one_leaves_nothing() {
        let store = unlocked();
        let key = store.add_key(key_draft("laptop")).unwrap();
        let mut host = draft("pve", "10.0.0.6");
        host.auth = AuthMethod::Key;
        host.key_id = Some(key.id);
        let host = store.save_host(host).unwrap();
        assert_eq!(host.key_id, Some(key.id));
        assert_eq!(host.key_label.as_deref(), Some("laptop"));
        assert_eq!(store.get_key(key.id).unwrap().unwrap().hosts, 1);
        assert_eq!(
            store.host_credential_source(host.id).unwrap(),
            crate::CredentialSource::VaultKey
        );

        assert!(matches!(
            store.delete_key(key.id),
            Err(StoreError::KeyInUse { hosts: 1 })
        ));
        store.delete_host(host.id).unwrap();
        store.delete_key(key.id).unwrap();
        assert!(store.list_keys().unwrap().is_empty());
        let sealed: i64 = store
            .conn
            .lock()
            .query_row(
                "SELECT count(*) FROM secrets WHERE length(blob) > 0",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sealed, 0);
    }

    #[test]
    fn a_key_without_a_label_is_named_by_its_type() {
        let store = unlocked();
        let key = store.add_key(key_draft("  ")).unwrap();
        assert_eq!(key.label, "ssh-ed25519");
        store.rename_key(key.id, "nas").unwrap();
        assert_eq!(store.get_key(key.id).unwrap().unwrap().label, "nas");
    }
}
