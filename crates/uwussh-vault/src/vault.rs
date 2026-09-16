//! A vault on one device: created with a master password, unlocked with it,
//! and then sealing and opening secrets under its vault key.
//!
//! ```text
//! master password ──Argon2id(salt, params)──► master key ──wraps──► vault key
//!                                                                     │
//!                         secret ◄──XChaCha20-Poly1305(id, kind)──────┘
//! ```
//!
//! What gets stored is a [`VaultHeader`]: salt, parameters and the wrapped
//! vault key. None of it is secret; all of it is useless without the password.
//! The unwrapped vault key only ever exists inside an [`UnlockedVault`], which
//! wipes it when dropped.

use crate::crypto::{decrypt_record, encrypt_record, Sealed};
use crate::kdf::{derive_master_secrets_with, KdfParams};
use crate::{Result, VaultError};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::RngCore;
use uuid::Uuid;
use uwussh_proto::EntityKind;
use zeroize::Zeroizing;

/// Associated data for wrapping the vault key: a label no record's AAD can
/// collide with (those are 33 bytes of ids and a kind), and the vault's id.
const WRAP_LABEL: &[u8] = b"uwussh/vault-key/v1";

/// Everything stored about a vault. Not secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultHeader {
    pub vault_id: Uuid,
    pub kdf: KdfParams,
    pub salt: [u8; 16],
    pub wrapped_key: Sealed,
}

/// A vault whose key is in memory. Dropping it locks the vault.
pub struct UnlockedVault {
    vault_id: Uuid,
    key: Zeroizing<[u8; 32]>,
}

impl std::fmt::Debug for UnlockedVault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "UnlockedVault({}, key redacted)", self.vault_id)
    }
}

/// Create a vault: a fresh random vault key, wrapped under the password.
pub fn create(
    password: &[u8],
    vault_id: Uuid,
    kdf: KdfParams,
) -> Result<(VaultHeader, UnlockedVault)> {
    let mut salt = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut salt);
    let mut key = Zeroizing::new([0u8; 32]);
    rand::thread_rng().fill_bytes(key.as_mut());

    let master = derive_master_secrets_with(password, &salt, kdf)?;
    let wrapped_key = wrap(&master.master_key, vault_id, &key)?;

    Ok((
        VaultHeader {
            vault_id,
            kdf,
            salt,
            wrapped_key,
        },
        UnlockedVault { vault_id, key },
    ))
}

impl VaultHeader {
    /// Unlock with the master password. A wrong password fails the wrapped
    /// key's authentication tag, so there is no separate verifier to attack.
    pub fn unlock(&self, password: &[u8]) -> Result<UnlockedVault> {
        let master = derive_master_secrets_with(password, &self.salt, self.kdf)?;
        let unwrapped = unwrap(&master.master_key, self.vault_id, &self.wrapped_key)?;
        let key: [u8; 32] = unwrapped
            .as_slice()
            .try_into()
            .map_err(|_| VaultError::Decrypt)?;
        Ok(UnlockedVault {
            vault_id: self.vault_id,
            key: Zeroizing::new(key),
        })
    }
}

impl UnlockedVault {
    pub fn vault_id(&self) -> Uuid {
        self.vault_id
    }

    /// Seal a secret, bound to its record id and kind.
    pub fn seal(&self, id: Uuid, kind: EntityKind, plaintext: &[u8]) -> Result<Sealed> {
        encrypt_record(&self.key, id, kind, self.vault_id, plaintext)
    }

    pub fn open(&self, id: Uuid, kind: EntityKind, sealed: &Sealed) -> Result<Zeroizing<Vec<u8>>> {
        decrypt_record(&self.key, id, kind, self.vault_id, sealed).map(Zeroizing::new)
    }
}

fn wrap_aad(vault_id: Uuid) -> Vec<u8> {
    [WRAP_LABEL, vault_id.as_bytes()].concat()
}

fn wrap(master_key: &[u8; 32], vault_id: Uuid, vault_key: &[u8; 32]) -> Result<Sealed> {
    let mut nonce = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut nonce);
    let blob = XChaCha20Poly1305::new(master_key.into())
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: vault_key,
                aad: &wrap_aad(vault_id),
            },
        )
        .map_err(|e| VaultError::Encrypt(e.to_string()))?;
    Ok(Sealed {
        nonce: nonce.to_vec(),
        blob,
    })
}

fn unwrap(master_key: &[u8; 32], vault_id: Uuid, wrapped: &Sealed) -> Result<Zeroizing<Vec<u8>>> {
    if wrapped.nonce.len() != 24 {
        return Err(VaultError::Decrypt);
    }
    XChaCha20Poly1305::new(master_key.into())
        .decrypt(
            XNonce::from_slice(&wrapped.nonce),
            Payload {
                msg: &wrapped.blob,
                aad: &wrap_aad(vault_id),
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| VaultError::Decrypt)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAST: KdfParams = KdfParams::INSECURE_FOR_TESTS;

    fn vault() -> (VaultHeader, UnlockedVault) {
        create(b"correct horse battery staple", Uuid::from_u128(7), FAST).unwrap()
    }

    #[test]
    fn the_right_password_unlocks_what_creating_sealed() {
        let (header, created) = vault();
        let id = Uuid::from_u128(1);
        let sealed = created.seal(id, EntityKind::Secret, b"nyu").unwrap();
        drop(created);

        let unlocked = header.unlock(b"correct horse battery staple").unwrap();
        assert_eq!(
            unlocked
                .open(id, EntityKind::Secret, &sealed)
                .unwrap()
                .as_slice(),
            b"nyu"
        );
    }

    #[test]
    fn the_wrong_password_does_not_unlock() {
        let (header, _) = vault();
        assert!(matches!(
            header.unlock(b"correct horse battery stapler"),
            Err(VaultError::Decrypt)
        ));
    }

    #[test]
    fn a_header_moved_to_another_vault_id_does_not_unlock() {
        let (mut header, _) = vault();
        header.vault_id = Uuid::from_u128(8);
        assert!(header.unlock(b"correct horse battery staple").is_err());
    }

    #[test]
    fn a_secret_is_bound_to_its_id() {
        let (_, unlocked) = vault();
        let sealed = unlocked
            .seal(Uuid::from_u128(1), EntityKind::Secret, b"nyu")
            .unwrap();
        assert!(unlocked
            .open(Uuid::from_u128(2), EntityKind::Secret, &sealed)
            .is_err());
    }

    #[test]
    fn two_vaults_with_the_same_password_have_different_keys() {
        let (a_header, a) = vault();
        let (b_header, _) = vault();
        assert_ne!(a_header.salt, b_header.salt);

        let sealed = a
            .seal(Uuid::from_u128(1), EntityKind::Secret, b"nyu")
            .unwrap();
        let b = b_header.unlock(b"correct horse battery staple").unwrap();
        assert!(b
            .open(Uuid::from_u128(1), EntityKind::Secret, &sealed)
            .is_err());
    }

    #[test]
    fn the_vault_key_never_shows_up_in_debug_output() {
        let (_, unlocked) = vault();
        let printed = format!("{unlocked:?}");
        assert!(printed.contains("redacted"), "{printed}");
    }
}
