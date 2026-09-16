//! Termius' sealed fields and the key that opens them.
//!
//! Each sensitive field is sealed on its own with NaCl's secretbox
//! (XSalsa20-Poly1305) under a 32-byte local key, and stored as base64 of a
//! short format header, the 24-byte nonce and the box. The key sits in the
//! operating system's credential store, where Termius put it through `keytar`:
//! on Windows a generic credential named `Termius/localKey`, holding the key as
//! base64 text.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use crypto_secretbox::aead::Aead;
use crypto_secretbox::{KeyInit, XSalsa20Poly1305};
use zeroize::Zeroizing;

/// `keytar` names a credential `service/account`.
pub const CREDENTIAL_TARGET: &str = "Termius/localKey";

/// The only sealed format seen so far.
const FORMAT_VERSION: u8 = 4;
const HEADER_LEN: usize = 2;
const NONCE_LEN: usize = 24;
const TAG_LEN: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    #[error(
        "Termius' local key is not in the credential store — is Termius installed for this user?"
    )]
    NotFound,
    #[error("the credential store refused to hand out Termius' local key: {0}")]
    Store(String),
    #[error("Termius' local key is not in the expected format")]
    Format,
    #[error("reading Termius' local key is not supported on this operating system yet")]
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SealError {
    #[error("not base64")]
    Encoding,
    #[error("unknown sealed format {0}")]
    Version(u8),
    #[error("too short to be sealed")]
    Truncated,
    #[error("does not open with this key")]
    Rejected,
}

pub struct LocalKey(Zeroizing<[u8; 32]>);

impl std::fmt::Debug for LocalKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LocalKey(..)")
    }
}

impl LocalKey {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        LocalKey(Zeroizing::new(bytes))
    }

    /// The key as the credential store holds it: base64 text.
    pub fn from_base64(encoded: &[u8]) -> Result<Self, KeyError> {
        let decoded = Zeroizing::new(
            BASE64
                .decode(trim_ascii(encoded))
                .map_err(|_| KeyError::Format)?,
        );
        let bytes: [u8; 32] = decoded
            .as_slice()
            .try_into()
            .map_err(|_| KeyError::Format)?;
        Ok(LocalKey::from_bytes(bytes))
    }

    /// Termius' key for the user running this process.
    pub fn from_credential_store() -> Result<Self, KeyError> {
        #[cfg(windows)]
        {
            let stored = windows::read_generic_credential(CREDENTIAL_TARGET)?;
            LocalKey::from_base64(&stored)
        }
        #[cfg(not(windows))]
        {
            Err(KeyError::Unsupported)
        }
    }

    /// Open a sealed field.
    pub fn open(&self, sealed: &str) -> Result<Zeroizing<Vec<u8>>, SealError> {
        let raw = Zeroizing::new(BASE64.decode(sealed).map_err(|_| SealError::Encoding)?);
        let version = *raw.first().ok_or(SealError::Truncated)?;
        if version != FORMAT_VERSION {
            return Err(SealError::Version(version));
        }
        let body = raw.get(HEADER_LEN..).ok_or(SealError::Truncated)?;
        if body.len() < NONCE_LEN + TAG_LEN {
            return Err(SealError::Truncated);
        }
        let (nonce, sealed_box) = body.split_at(NONCE_LEN);
        self.open_box(nonce.try_into().unwrap(), sealed_box)
    }

    /// Open a secretbox: the 16-byte tag, then the ciphertext, as libsodium's
    /// `crypto_secretbox_easy` lays it out.
    pub fn open_box(
        &self,
        nonce: &[u8; NONCE_LEN],
        sealed_box: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, SealError> {
        XSalsa20Poly1305::new(self.0.as_ref().into())
            .decrypt(nonce.into(), sealed_box)
            .map(Zeroizing::new)
            .map_err(|_| SealError::Rejected)
    }
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map_or(start, |i| i + 1);
    &bytes[start..end]
}

#[cfg(windows)]
mod windows {
    use super::KeyError;
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND};
    use windows_sys::Win32::Security::Credentials::{
        CredFree, CredReadW, CREDENTIALW, CRED_TYPE_GENERIC,
    };
    use zeroize::Zeroizing;

    /// The blob of a generic credential, copied into memory that wipes itself.
    pub fn read_generic_credential(target: &str) -> Result<Zeroizing<Vec<u8>>, KeyError> {
        let target: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
        let mut credential: *mut CREDENTIALW = std::ptr::null_mut();

        // SAFETY: `target` is NUL-terminated and outlives the call, and
        // `credential` is a valid place for Windows to write a pointer to.
        let found = unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) };
        if found == 0 {
            // SAFETY: no preconditions; read right after the failing call.
            let error = unsafe { GetLastError() };
            return Err(if error == ERROR_NOT_FOUND {
                KeyError::NotFound
            } else {
                KeyError::Store(std::io::Error::from_raw_os_error(error as i32).to_string())
            });
        }

        // SAFETY: on success `credential` points at one CREDENTIALW that Windows
        // allocated for us, whose blob pointer and size describe a buffer that
        // stays valid until `CredFree`. It is copied, wiped and freed exactly
        // once, and nothing refers to it afterwards.
        unsafe {
            let blob = (*credential).CredentialBlob;
            let len = (*credential).CredentialBlobSize as usize;
            let copy = if blob.is_null() || len == 0 {
                Zeroizing::new(Vec::new())
            } else {
                let buffer = std::slice::from_raw_parts_mut(blob, len);
                let copy = Zeroizing::new(buffer.to_vec());
                buffer.fill(0);
                copy
            };
            CredFree(credential.cast());
            Ok(copy)
        }
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// Seal like Termius does, for fixtures.
    pub fn seal(key: &[u8; 32], nonce: [u8; 24], plaintext: &[u8]) -> String {
        let sealed_box = XSalsa20Poly1305::new(key.into())
            .encrypt(&nonce.into(), plaintext)
            .unwrap();
        let mut raw = vec![FORMAT_VERSION, 0];
        raw.extend_from_slice(&nonce);
        raw.extend_from_slice(&sealed_box);
        BASE64.encode(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::testing::seal;
    use super::*;

    const KEY: [u8; 32] = [7; 32];

    #[test]
    fn a_sealed_field_opens_with_its_key() {
        let sealed = seal(&KEY, [1; 24], b"10.0.0.12");
        assert!(
            sealed.starts_with("BA"),
            "Termius' fields all start with BA"
        );
        let opened = LocalKey::from_bytes(KEY).open(&sealed).unwrap();
        assert_eq!(opened.as_slice(), b"10.0.0.12");
    }

    #[test]
    fn the_wrong_key_or_a_flipped_bit_is_rejected() {
        let sealed = seal(&KEY, [1; 24], b"hunter2");
        assert_eq!(
            LocalKey::from_bytes([8; 32]).open(&sealed),
            Err(SealError::Rejected)
        );

        let mut raw = BASE64.decode(&sealed).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 1;
        assert_eq!(
            LocalKey::from_bytes(KEY).open(&BASE64.encode(raw)),
            Err(SealError::Rejected)
        );
    }

    #[test]
    fn malformed_fields_say_what_is_wrong() {
        let key = LocalKey::from_bytes(KEY);
        assert_eq!(key.open("not base64!"), Err(SealError::Encoding));
        assert_eq!(
            key.open(&BASE64.encode([4, 0, 1, 2])),
            Err(SealError::Truncated)
        );
        assert_eq!(
            key.open(&BASE64.encode([9; 60])),
            Err(SealError::Version(9))
        );
    }

    #[test]
    fn the_stored_key_is_base64_text() {
        let encoded = format!("{}\n", BASE64.encode(KEY));
        let key = LocalKey::from_base64(encoded.as_bytes()).unwrap();
        let sealed = seal(&KEY, [2; 24], b"ok");
        assert_eq!(key.open(&sealed).unwrap().as_slice(), b"ok");

        assert_eq!(
            LocalKey::from_base64(BASE64.encode([1; 16]).as_bytes()).unwrap_err(),
            KeyError::Format
        );
    }

    #[test]
    fn the_key_never_shows_up_in_debug_output() {
        assert_eq!(format!("{:?}", LocalKey::from_bytes(KEY)), "LocalKey(..)");
    }
}
