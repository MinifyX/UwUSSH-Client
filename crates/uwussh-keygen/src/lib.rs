//! UwUKeygen: SSH keys made on this machine, written in whichever format the
//! other side wants.
//!
//! - [`generate`] makes a new key: RSA in one of the [`RSA_BITS`] sizes,
//!   Ed25519, or ECDSA on P-256, P-384 or P-521.
//! - [`load`] reads an existing one, so it can be shown, converted into another
//!   format (PuTTYgen's "Conversions"), or put into the vault.
//! - [`KeyPair::encode`] writes it as OpenSSH, PuTTY `.ppk` version 3 or 2, or
//!   PKCS#8 PEM, encrypted when there is a passphrase.
//!
//! The keys themselves are ssh-key's, reached through russh, and every file
//! written here is read back in the tests by `russh::keys::decode_secret_key`.
//! That is the reader that will later log in with the key, so it is the one
//! whose opinion counts.
//!
//! Private key material leaves this crate only as a [`Zeroizing`] string, and
//! no `Debug` output here contains any of it.

mod costs;
mod generate;
mod key;
mod pem;
mod ppk;

pub use costs::check_costs;
pub use generate::{generate, generate_with_rng, GenerateOptions};
pub use key::{load, KeyInfo, KeyKind, KeyPair, PrivateFormat, RSA_BITS};
pub use zeroize::Zeroizing;

use serde::Serialize;

#[derive(Debug, thiserror::Error, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum KeygenError {
    #[error("{bits} bits is not an RSA key size UwUKeygen offers")]
    RsaSize { bits: u32 },

    #[error("this key is protected by a passphrase")]
    PassphraseRequired,

    #[error("the passphrase is wrong, or the key file is damaged")]
    PassphraseWrong,

    #[error("not a private key that can be read: {reason}")]
    Unreadable { reason: String },

    #[error("{reason}")]
    Unsupported { reason: String },

    #[error("the system's random number generator failed")]
    Random,

    #[error("could not generate the key: {reason}")]
    Generate { reason: String },

    #[error("could not write the key: {reason}")]
    Encode { reason: String },
}

pub type Result<T> = std::result::Result<T, KeygenError>;

/// For failures deep inside an encoder, which are bugs rather than anything
/// the user can act on — but still errors, not panics.
fn encode_error(err: impl std::fmt::Display) -> KeygenError {
    KeygenError::Encode {
        reason: err.to_string(),
    }
}
