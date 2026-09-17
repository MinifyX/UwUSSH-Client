//! The vault's life on this device: create it, unlock it, lock it.
//!
//! The header — salt, KDF parameters, wrapped key — lives in the `vault` table;
//! it is not secret and is useless without the master password. The unlocked
//! key lives only in [`Store`]'s memory, held in a mutex, and is wiped when the
//! store is dropped or the vault is locked. Secrets are sealed and opened
//! against that in-memory key, so the plaintext of a password or a private key
//! never reaches SQLite.

use crate::{tick, vault_id, Result, Store, StoreError};
use rusqlite::{params, OptionalExtension, Transaction};
use serde::Serialize;
use uuid::Uuid;
use uwussh_vault::{KdfParams, Sealed, UnlockedVault, VaultHeader};
use zeroize::Zeroizing;

/// What the UI needs to know before it can store or reveal a secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum VaultStatus {
    /// No master password has ever been set.
    Absent,
    /// A vault exists but the key is not in memory.
    Locked,
    Unlocked,
}

impl Store {
    pub fn vault_status(&self) -> Result<VaultStatus> {
        if self.vault.lock().is_some() {
            return Ok(VaultStatus::Unlocked);
        }
        Ok(if self.load_header()?.is_some() {
            VaultStatus::Locked
        } else {
            VaultStatus::Absent
        })
    }

    /// Create the vault with a master password and leave it unlocked. Uses the
    /// device's existing `vault_id`, so records written before the vault
    /// existed keep the same owner.
    pub fn create_vault(&self, password: &[u8]) -> Result<()> {
        self.create_vault_with(password, KdfParams::RECOMMENDED)
    }

    pub fn create_vault_with(&self, password: &[u8], kdf: KdfParams) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        if header_row(&tx)?.is_some() {
            return Err(StoreError::VaultExists);
        }
        let vault_id: String =
            tx.query_row("SELECT vault_id FROM meta WHERE id = 1", [], |r| r.get(0))?;
        let vault_id = Uuid::parse_str(&vault_id).map_err(|_| StoreError::NoVault)?;

        let (header, unlocked) = uwussh_vault::create(password, vault_id, kdf)?;
        store_header(&tx, &header)?;
        tx.commit()?;
        // Locks are always taken connection first, vault second, and never
        // the other way round: reveal and import run at the same time.
        drop(conn);
        *self.vault.lock() = Some(unlocked);
        tracing::info!("vault created");
        Ok(())
    }

    /// Unlock an existing vault. A wrong password is [`StoreError::Vault`].
    pub fn unlock_vault(&self, password: &[u8]) -> Result<()> {
        let header = self.load_header()?.ok_or(StoreError::NoVault)?;
        let unlocked = header.unlock(password)?;
        *self.vault.lock() = Some(unlocked);
        tracing::info!("vault unlocked");
        Ok(())
    }

    /// Drop the key from memory. Idempotent.
    pub fn lock_vault(&self) {
        if self.vault.lock().take().is_some() {
            tracing::info!("vault locked");
        }
    }

    /// Open one stored secret by id. Fails if the vault is locked; the
    /// plaintext comes back in memory that wipes itself.
    pub fn reveal_secret(&self, id: Uuid) -> Result<Revealed> {
        // The row first, with only the connection locked; the vault after.
        // Holding the vault while waiting for the connection deadlocked
        // against an import, which takes them in the other order.
        if self.vault.lock().is_none() {
            return Err(StoreError::VaultLocked);
        }
        let sealed = self
            .conn
            .lock()
            .query_row(
                "SELECT nonce, blob FROM secrets WHERE id = ?1 AND deleted = 0",
                [id.to_string()],
                |row| {
                    Ok(Sealed {
                        nonce: row.get(0)?,
                        blob: row.get(1)?,
                    })
                },
            )
            .optional()?
            .ok_or(StoreError::NoVault)?;
        let guard = self.vault.lock();
        let vault = guard.as_ref().ok_or(StoreError::VaultLocked)?;
        Ok(vault.open(id, uwussh_proto::EntityKind::Secret, &sealed)?)
    }

    /// The vault's header: salt, key derivation costs and the wrapped key.
    ///
    /// None of it is secret and all of it is useless without the master
    /// password, which is what makes it the thing a device uploads when it
    /// creates an account, and the thing a second device downloads before it
    /// asks for that password.
    pub fn vault_header(&self) -> Result<Option<VaultHeader>> {
        self.load_header()
    }

    pub(crate) fn load_header(&self) -> Result<Option<VaultHeader>> {
        header_row(&self.conn.lock())
    }
}

/// Seal a secret with the unlocked vault and store it, returning its id.
pub(crate) fn seal_secret(
    tx: &Transaction,
    device: u32,
    vault: &UnlockedVault,
    plaintext: &[u8],
) -> Result<String> {
    let id = Uuid::now_v7();
    let sealed = vault.seal(id, uwussh_proto::EntityKind::Secret, plaintext)?;
    let clock = tick(tx, device)?;
    tx.execute(
        "INSERT INTO secrets
            (id, vault_id, nonce, blob, hlc_wall_ms, hlc_counter, hlc_device)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            id.to_string(),
            vault_id(tx)?,
            sealed.nonce,
            sealed.blob,
            clock.wall_ms as i64,
            clock.counter,
            clock.device,
        ],
    )?;
    Ok(id.to_string())
}

/// Tombstone a secret and drop its ciphertext with it. A replaced password has
/// no reason to stay around, not even encrypted: whoever learns the master
/// password later would learn the old one too.
pub(crate) fn forget_secret(tx: &Transaction, device: u32, id: &str) -> Result<()> {
    let clock = tick(tx, device)?;
    tx.execute(
        "UPDATE secrets
            SET deleted = 1, nonce = x'', blob = x'', rev = rev + 1,
                dirty = 1, hlc_wall_ms = ?2, hlc_counter = ?3, hlc_device = ?4
          WHERE id = ?1 AND deleted = 0",
        params![id, clock.wall_ms as i64, clock.counter, clock.device],
    )?;
    Ok(())
}

/// Fold the write-ahead log back into the database and empty it. After a
/// secret is forgotten, the log still holds the page it was on; with
/// `secure_delete` on, this is what finally overwrites it. Best effort: a
/// reader holding the log open just means it happens next time.
pub(crate) fn truncate_wal(conn: &rusqlite::Connection) {
    let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
}

fn store_header(conn: &rusqlite::Connection, header: &VaultHeader) -> Result<()> {
    conn.execute(
        "INSERT INTO vault
            (id, vault_id, kdf_memory_kib, kdf_time_cost, kdf_parallelism,
             salt, wrapped_nonce, wrapped_blob)
         VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            header.vault_id.to_string(),
            header.kdf.memory_kib,
            header.kdf.time_cost,
            header.kdf.parallelism,
            header.salt.to_vec(),
            header.wrapped_key.nonce,
            header.wrapped_key.blob,
        ],
    )?;
    Ok(())
}

fn header_row(conn: &rusqlite::Connection) -> Result<Option<VaultHeader>> {
    conn.query_row(
        "SELECT vault_id, kdf_memory_kib, kdf_time_cost, kdf_parallelism,
                salt, wrapped_nonce, wrapped_blob
           FROM vault WHERE id = 1",
        [],
        |row| {
            let vault_id: String = row.get(0)?;
            let salt: Vec<u8> = row.get(4)?;
            Ok((
                vault_id,
                KdfParams {
                    memory_kib: row.get(1)?,
                    time_cost: row.get(2)?,
                    parallelism: row.get(3)?,
                },
                salt,
                Sealed {
                    nonce: row.get(5)?,
                    blob: row.get(6)?,
                },
            ))
        },
    )
    .optional()?
    .map(|(vault_id, kdf, salt, wrapped_key)| {
        let vault_id = Uuid::parse_str(&vault_id).map_err(|_| StoreError::NoVault)?;
        // A header with absurd costs would make every unlock run out of memory.
        if !kdf.within_limits() {
            return Err(StoreError::Vault(uwussh_vault::VaultError::Kdf(
                "the vault header asks for unreasonable key derivation costs".into(),
            )));
        }
        let salt: [u8; 16] = salt
            .as_slice()
            .try_into()
            .map_err(|_| StoreError::NoVault)?;
        Ok(VaultHeader {
            vault_id,
            kdf,
            salt,
            wrapped_key,
        })
    })
    .transpose()
}

/// A secret revealed from the vault: wiped from memory when dropped.
pub type Revealed = Zeroizing<Vec<u8>>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;

    fn store() -> Store {
        Store::open_in_memory().unwrap()
    }

    fn create(store: &Store) {
        store
            .create_vault_with(b"open sesame", KdfParams::INSECURE_FOR_TESTS)
            .unwrap();
    }

    #[test]
    fn status_walks_absent_locked_unlocked() {
        let store = store();
        assert_eq!(store.vault_status().unwrap(), VaultStatus::Absent);
        create(&store);
        assert_eq!(store.vault_status().unwrap(), VaultStatus::Unlocked);
        store.lock_vault();
        assert_eq!(store.vault_status().unwrap(), VaultStatus::Locked);
    }

    #[test]
    fn a_vault_cannot_be_created_twice() {
        let store = store();
        create(&store);
        assert!(matches!(
            store.create_vault_with(b"again", KdfParams::INSECURE_FOR_TESTS),
            Err(StoreError::VaultExists)
        ));
    }

    #[test]
    fn the_right_password_unlocks_a_reopened_vault() {
        let path = std::env::temp_dir().join(format!("uwussh-vault-{}.db", Uuid::now_v7()));
        {
            let store = Store::open(&path).unwrap();
            create(&store);
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(store.vault_status().unwrap(), VaultStatus::Locked);
        assert!(store.unlock_vault(b"wrong").is_err());
        assert_eq!(store.vault_status().unwrap(), VaultStatus::Locked);
        store.unlock_vault(b"open sesame").unwrap();
        assert_eq!(store.vault_status().unwrap(), VaultStatus::Unlocked);

        drop(store);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    #[test]
    fn a_header_with_absurd_kdf_costs_is_refused() {
        let store = store();
        create(&store);
        store.lock_vault();
        store
            .conn
            .lock()
            .execute("UPDATE vault SET kdf_memory_kib = 4294967295", [])
            .unwrap();
        assert!(matches!(
            store.unlock_vault(b"open sesame"),
            Err(StoreError::Vault(uwussh_vault::VaultError::Kdf(_)))
        ));
    }

    #[test]
    fn revealing_and_importing_at_once_does_not_deadlock() {
        use crate::import::ImportSet;
        let store = std::sync::Arc::new(store());
        create(&store);
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(400);
        let revealer = {
            let store = std::sync::Arc::clone(&store);
            std::thread::spawn(move || {
                while std::time::Instant::now() < deadline {
                    let _ = store.reveal_secret(Uuid::now_v7());
                }
            })
        };
        while std::time::Instant::now() < deadline {
            store.import(ImportSet::default()).unwrap();
        }
        revealer.join().unwrap();
    }

    #[test]
    fn locking_without_a_vault_is_harmless() {
        let store = store();
        store.lock_vault();
        assert!(store.unlock_vault(b"x").is_err());
    }
}
