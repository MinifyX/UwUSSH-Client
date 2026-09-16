//! Record encryption.
//!
//! XChaCha20-Poly1305 with a fresh 24-byte nonce per record. The associated
//! data is `record_id || kind || vault_id`, which is the part that actually
//! matters against a hostile server: without it, a server could hand back the
//! blob of record A when the client asked for record B, and the decryption
//! would succeed. With it, that swap fails the authentication tag.

use crate::{Result, VaultError};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::RngCore;
use uuid::Uuid;
use uwussh_proto::EntityKind;

/// An encrypted record, ready to go into an envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sealed {
    pub nonce: Vec<u8>,
    pub blob: Vec<u8>,
}

/// Bind a ciphertext to the identity of the record it belongs to.
fn associated_data(id: Uuid, kind: EntityKind, vault_id: Uuid) -> Vec<u8> {
    let mut aad = Vec::with_capacity(16 + 1 + 16);
    aad.extend_from_slice(id.as_bytes());
    aad.push(kind as u8);
    aad.extend_from_slice(vault_id.as_bytes());
    aad
}

pub fn encrypt_record(
    vault_key: &[u8; 32],
    id: Uuid,
    kind: EntityKind,
    vault_id: Uuid,
    plaintext: &[u8],
) -> Result<Sealed> {
    let cipher = XChaCha20Poly1305::new(vault_key.into());

    let mut nonce_bytes = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = XNonce::from_slice(&nonce_bytes);

    let aad = associated_data(id, kind, vault_id);
    let blob = cipher
        .encrypt(nonce, Payload { msg: plaintext, aad: &aad })
        .map_err(|e| VaultError::Encrypt(e.to_string()))?;

    Ok(Sealed { nonce: nonce_bytes.to_vec(), blob })
}

pub fn decrypt_record(
    vault_key: &[u8; 32],
    id: Uuid,
    kind: EntityKind,
    vault_id: Uuid,
    sealed: &Sealed,
) -> Result<Vec<u8>> {
    if sealed.nonce.len() != 24 {
        return Err(VaultError::Decrypt);
    }
    let cipher = XChaCha20Poly1305::new(vault_key.into());
    let nonce = XNonce::from_slice(&sealed.nonce);
    let aad = associated_data(id, kind, vault_id);

    cipher
        .decrypt(nonce, Payload { msg: &sealed.blob, aad: &aad })
        .map_err(|_| VaultError::Decrypt)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> [u8; 32] {
        [7u8; 32]
    }

    #[test]
    fn round_trip() {
        let id = Uuid::from_u128(1);
        let vault = Uuid::from_u128(99);
        let sealed = encrypt_record(&key(), id, EntityKind::Host, vault, b"prox-1").unwrap();
        let out = decrypt_record(&key(), id, EntityKind::Host, vault, &sealed).unwrap();
        assert_eq!(out, b"prox-1");
    }

    #[test]
    fn a_swapped_record_id_fails() {
        let vault = Uuid::from_u128(99);
        let sealed =
            encrypt_record(&key(), Uuid::from_u128(1), EntityKind::Host, vault, b"secret").unwrap();

        // The server hands this blob back as if it belonged to another record.
        let out = decrypt_record(&key(), Uuid::from_u128(2), EntityKind::Host, vault, &sealed);
        assert!(out.is_err(), "AAD must bind the ciphertext to its record id");
    }

    #[test]
    fn a_reinterpreted_kind_fails() {
        let id = Uuid::from_u128(1);
        let vault = Uuid::from_u128(99);
        let sealed = encrypt_record(&key(), id, EntityKind::Snippet, vault, b"secret").unwrap();

        let out = decrypt_record(&key(), id, EntityKind::Key, vault, &sealed);
        assert!(out.is_err(), "a snippet must not be readable as a key");
    }

    #[test]
    fn a_tampered_blob_fails() {
        let id = Uuid::from_u128(1);
        let vault = Uuid::from_u128(99);
        let mut sealed = encrypt_record(&key(), id, EntityKind::Host, vault, b"secret").unwrap();
        sealed.blob[0] ^= 0xff;

        assert!(decrypt_record(&key(), id, EntityKind::Host, vault, &sealed).is_err());
    }

    #[test]
    fn nonces_do_not_repeat() {
        let id = Uuid::from_u128(1);
        let vault = Uuid::from_u128(99);
        let a = encrypt_record(&key(), id, EntityKind::Host, vault, b"same").unwrap();
        let b = encrypt_record(&key(), id, EntityKind::Host, vault, b"same").unwrap();
        assert_ne!(a.nonce, b.nonce);
        assert_ne!(a.blob, b.blob, "identical plaintext must not produce identical ciphertext");
    }
}
