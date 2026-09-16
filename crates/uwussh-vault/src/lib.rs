//! The vault: key derivation and record encryption.
//!
//! The shape is the Bitwarden model, for two concrete reasons rather than
//! because it is popular:
//!
//! 1. The master password derives *two* independent secrets. One never leaves
//!    the device; the other is what the server sees. The server therefore
//!    cannot decrypt anything it stores, even if it wanted to.
//! 2. Records are encrypted with a separate random vault key, which is merely
//!    *wrapped* by the master key. Changing the master password rewraps one
//!    32-byte key instead of re-encrypting every record you own.

pub mod crypto;
pub mod kdf;
pub mod vault;

pub use crypto::{decrypt_record, encrypt_record, Sealed};
pub use kdf::{
    derive_master_secrets, derive_master_secrets_with, KdfParams, MasterSecrets, KDF_MEMORY_KIB,
    KDF_PARALLELISM, KDF_TIME_COST,
};
pub use vault::{create, UnlockedVault, VaultHeader};

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("key derivation failed: {0}")]
    Kdf(String),
    #[error("could not decrypt: wrong key, or the record was tampered with")]
    Decrypt,
    #[error("could not encrypt: {0}")]
    Encrypt(String),
}

pub type Result<T> = std::result::Result<T, VaultError>;
