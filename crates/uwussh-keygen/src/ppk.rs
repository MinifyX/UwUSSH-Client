//! PuTTY's `.ppk` files, versions 3 and 2.
//!
//! ssh-key reads these but cannot write them, so they are written here, after
//! Appendix C of the PuTTY manual. A `.ppk` is plain text: a header, the public
//! key in base64, the private key in base64 — encrypted when there is a
//! passphrase — and a MAC over the algorithm, the encryption, the comment and
//! both halves of the key, so none of them can be swapped unnoticed.
//!
//! The versions differ in how a passphrase becomes keys. Version 3 (PuTTY 0.75)
//! runs Argon2id. Version 2 hashes the passphrase once with SHA-1, which a GPU
//! guesses through by the billion per second; it is only for tools that never
//! learned version 3.

use crate::{encode_error, KeygenError, Result};
use hmac::digest::KeyInit;
use hmac::{Hmac, Mac};
use russh::keys::ssh_key::encoding::base64::{Base64, Encoding};
use russh::keys::ssh_key::private::KeypairData;
use russh::keys::ssh_key::sha2::Sha256;
use russh::keys::ssh_key::Cipher;
use russh::keys::PrivateKey;
use sha1::{Digest, Sha1};
use zeroize::Zeroizing;

/// PuTTYgen's memory and parallelism. PuTTYgen times its passes to take about
/// a tenth of a second on the machine it runs on; a fixed count instead makes
/// a file equally hard to guess wherever it was written.
const ARGON2_MEMORY_KIB: u32 = 8192;
const ARGON2_PASSES: u32 = 13;
const ARGON2_PARALLELISM: u32 = 1;
const ARGON2_SALT_LEN: usize = 16;

const AES_BLOCK: usize = 16;
const LINE_WIDTH: usize = 64;
const V2_MAC_KEY_LABEL: &[u8] = b"putty-private-key-file-mac-key";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Version {
    V2,
    V3,
}

struct Secrets {
    cipher_key: Zeroizing<[u8; 32]>,
    iv: Zeroizing<[u8; 16]>,
    mac_key: Zeroizing<Vec<u8>>,
}

pub(crate) fn encode(
    key: &PrivateKey,
    version: Version,
    passphrase: Option<&str>,
) -> Result<Zeroizing<String>> {
    let algorithm = key.algorithm();
    let algorithm = algorithm.as_str();
    let comment = key.comment().as_str_lossy();
    let public_blob = key.public_key().to_bytes().map_err(encode_error)?;
    let mut private_blob = private_blob(key)?;

    let (encryption, kdf_lines, secrets) = match (passphrase, version) {
        (None, _) => ("none", String::new(), None),
        (Some(passphrase), Version::V3) => {
            let mut salt = [0u8; ARGON2_SALT_LEN];
            getrandom::fill(&mut salt).map_err(|_| KeygenError::Random)?;
            let lines = format!(
                "Key-Derivation: Argon2id\n\
                 Argon2-Memory: {ARGON2_MEMORY_KIB}\n\
                 Argon2-Passes: {ARGON2_PASSES}\n\
                 Argon2-Parallelism: {ARGON2_PARALLELISM}\n\
                 Argon2-Salt: {}\n",
                hex(&salt)
            );
            ("aes256-cbc", lines, Some(derive_v3(passphrase, &salt)?))
        }
        (Some(passphrase), Version::V2) => {
            ("aes256-cbc", String::new(), Some(derive_v2(passphrase)))
        }
    };

    if secrets.is_some() {
        pad(&mut private_blob, version)?;
    }

    let mac_key = match (&secrets, version) {
        (Some(secrets), _) => secrets.mac_key.clone(),
        (None, Version::V3) => Zeroizing::new(Vec::new()),
        (None, Version::V2) => v2_mac_key(""),
    };
    let mut mac_input = Zeroizing::new(Vec::with_capacity(
        5 * 4
            + algorithm.len()
            + encryption.len()
            + comment.len()
            + public_blob.len()
            + private_blob.len(),
    ));
    for field in [
        algorithm.as_bytes(),
        encryption.as_bytes(),
        comment.as_bytes(),
        &public_blob,
        &private_blob,
    ] {
        put_string(&mut mac_input, field);
    }
    let mac = match version {
        Version::V3 => {
            let mut mac = Hmac::<Sha256>::new_from_slice(&mac_key).map_err(encode_error)?;
            mac.update(&mac_input);
            hex(&mac.finalize().into_bytes())
        }
        Version::V2 => {
            let mut mac = Hmac::<Sha1>::new_from_slice(&mac_key).map_err(encode_error)?;
            mac.update(&mac_input);
            hex(&mac.finalize().into_bytes())
        }
    };

    if let Some(secrets) = &secrets {
        Cipher::Aes256Cbc
            .encrypt(&secrets.cipher_key[..], &secrets.iv[..], &mut private_blob)
            .map_err(encode_error)?;
    }

    let header = match version {
        Version::V3 => 3,
        Version::V2 => 2,
    };
    // Sized up front: a String that grows leaves copies of the old buffer
    // behind, unwiped.
    let base64_len = |bytes: usize| bytes.div_ceil(3) * 4;
    let capacity = 256
        + algorithm.len()
        + comment.len()
        + kdf_lines.len()
        + base64_len(public_blob.len()) * 2
        + base64_len(private_blob.len()) * 2
        + mac.len();
    let mut out = Zeroizing::new(String::with_capacity(capacity));
    out.push_str(&format!(
        "PuTTY-User-Key-File-{header}: {algorithm}\n\
         Encryption: {encryption}\n\
         Comment: {comment}\n"
    ));
    push_base64_lines(&mut out, "Public-Lines", &public_blob)?;
    out.push_str(&kdf_lines);
    push_base64_lines(&mut out, "Private-Lines", &private_blob)?;
    out.push_str("Private-MAC: ");
    out.push_str(&mac);
    out.push('\n');
    Ok(out)
}

/// The private half, in PuTTY's layout for each algorithm. Room for the
/// padding is reserved now, so padding never reallocates.
fn private_blob(key: &PrivateKey) -> Result<Zeroizing<Vec<u8>>> {
    match key.key_data() {
        KeypairData::Rsa(pair) => {
            let private = pair.private();
            let fields = [private.d(), private.p(), private.q(), private.iqmp()];
            let len: usize = fields.iter().map(|m| 4 + m.as_bytes().len()).sum();
            let mut blob = Zeroizing::new(Vec::with_capacity(len + AES_BLOCK));
            for field in fields {
                put_string(&mut blob, field.as_bytes());
            }
            Ok(blob)
        }
        // PuTTY documents this as an mpint, but it treats the 32 secret bytes
        // as a little-endian number and writes them as they are, which is also
        // what PuTTYgen's own files contain.
        KeypairData::Ed25519(pair) => {
            let secret = pair.private.as_ref();
            let mut blob = Zeroizing::new(Vec::with_capacity(4 + secret.len() + AES_BLOCK));
            put_string(&mut blob, secret);
            Ok(blob)
        }
        KeypairData::Ecdsa(pair) => {
            let scalar = pair.private_key_bytes();
            let mut blob = Zeroizing::new(Vec::with_capacity(5 + scalar.len() + AES_BLOCK));
            put_mpint(&mut blob, scalar);
            Ok(blob)
        }
        _ => Err(KeygenError::Unsupported {
            reason: format!(
                "{} keys cannot be written for PuTTY",
                key.algorithm().as_str()
            ),
        }),
    }
}

/// AES needs whole blocks. Version 3 pads with random bytes, as the manual
/// says; version 2 with the SHA-1 of the unpadded blob, as PuTTY did.
fn pad(blob: &mut Zeroizing<Vec<u8>>, version: Version) -> Result<()> {
    let missing = (AES_BLOCK - blob.len() % AES_BLOCK) % AES_BLOCK;
    let mut padding = Zeroizing::new([0u8; AES_BLOCK]);
    match version {
        Version::V3 => {
            getrandom::fill(&mut padding[..missing]).map_err(|_| KeygenError::Random)?;
        }
        Version::V2 => {
            let hash = Sha1::digest(&blob[..]);
            padding[..missing].copy_from_slice(&hash[..missing]);
        }
    }
    blob.extend_from_slice(&padding[..missing]);
    Ok(())
}

fn derive_v3(passphrase: &str, salt: &[u8]) -> Result<Secrets> {
    let params = argon2::Params::new(
        ARGON2_MEMORY_KIB,
        ARGON2_PASSES,
        ARGON2_PARALLELISM,
        Some(80),
    )
    .map_err(encode_error)?;
    let mut out = Zeroizing::new([0u8; 80]);
    argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), salt, &mut out[..])
        .map_err(encode_error)?;

    let mut secrets = Secrets {
        cipher_key: Zeroizing::new([0u8; 32]),
        iv: Zeroizing::new([0u8; 16]),
        mac_key: Zeroizing::new(out[48..80].to_vec()),
    };
    secrets.cipher_key.copy_from_slice(&out[..32]);
    secrets.iv.copy_from_slice(&out[32..48]);
    Ok(secrets)
}

fn derive_v2(passphrase: &str) -> Secrets {
    let mut secrets = Secrets {
        cipher_key: Zeroizing::new([0u8; 32]),
        iv: Zeroizing::new([0u8; 16]),
        mac_key: v2_mac_key(passphrase),
    };
    let first = Sha1::new()
        .chain_update([0, 0, 0, 0])
        .chain_update(passphrase)
        .finalize();
    let second = Sha1::new()
        .chain_update([0, 0, 0, 1])
        .chain_update(passphrase)
        .finalize();
    secrets.cipher_key[..20].copy_from_slice(&first);
    secrets.cipher_key[20..].copy_from_slice(&second[..12]);
    secrets
}

fn v2_mac_key(passphrase: &str) -> Zeroizing<Vec<u8>> {
    Zeroizing::new(
        Sha1::new()
            .chain_update(V2_MAC_KEY_LABEL)
            .chain_update(passphrase)
            .finalize()
            .to_vec(),
    )
}

fn push_base64_lines(out: &mut String, label: &str, bytes: &[u8]) -> Result<()> {
    let mut buffer = Zeroizing::new(vec![0u8; Base64::encoded_len(bytes)]);
    let mut rest = Base64::encode(bytes, &mut buffer).map_err(encode_error)?;
    out.push_str(&format!("{label}: {}\n", rest.len().div_ceil(LINE_WIDTH)));
    while !rest.is_empty() {
        let (line, tail) = rest.split_at(rest.len().min(LINE_WIDTH));
        out.push_str(line);
        out.push('\n');
        rest = tail;
    }
    Ok(())
}

/// SSH's `string`: a big-endian u32 length, then the bytes.
fn put_string(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
}

/// SSH's `mpint` for a non-negative big-endian number: no leading zeros, and
/// one zero byte in front if the top bit is set.
fn put_mpint(out: &mut Vec<u8>, big_endian: &[u8]) {
    let start = big_endian
        .iter()
        .position(|&byte| byte != 0)
        .unwrap_or(big_endian.len());
    let digits = &big_endian[start..];
    let sign_byte = digits.first().is_some_and(|&byte| byte >= 0x80);
    out.extend_from_slice(&((digits.len() + usize::from(sign_byte)) as u32).to_be_bytes());
    if sign_byte {
        out.push(0);
    }
    out.extend_from_slice(digits);
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::{KeyKind, KeyPair, PrivateFormat};
    use crate::{generate_with_rng, GenerateOptions};
    use chacha20::ChaCha20Rng;
    use russh::keys::decode_secret_key;
    use russh::keys::ssh_key::rand_core::SeedableRng;

    // No key files in this repository, not even test ones: every key here is
    // generated from a fixed seed. During development the writer was checked
    // byte for byte against PuTTYgen's own files (ssh-key's test vectors):
    // unencrypted v3 for Ed25519 and ECDSA, and RSA as v2, encrypted too.

    fn key(kind: KeyKind) -> KeyPair {
        let options = GenerateOptions {
            kind,
            comment: "user@example.com".into(),
            extra_entropy: Vec::new(),
        };
        generate_with_rng(&options, &mut ChaCha20Rng::from_seed([5; 32])).unwrap()
    }

    /// The bytes of a `*-Lines` section.
    fn section(ppk: &str, label: &str) -> Vec<u8> {
        let mut lines = ppk.lines();
        let count: usize = lines
            .find_map(|line| line.strip_prefix(&format!("{label}: ")))
            .unwrap()
            .parse()
            .unwrap();
        let base64: String = lines.take(count).collect();
        Base64::decode_vec(&base64).unwrap()
    }

    #[test]
    fn an_unencrypted_file_is_laid_out_like_puttygens() {
        let key = key(KeyKind::Ed25519);
        let ppk = key.encode(PrivateFormat::PuttyV3, None).unwrap();
        let lines: Vec<&str> = ppk.lines().collect();

        assert_eq!(lines[0], "PuTTY-User-Key-File-3: ssh-ed25519");
        assert_eq!(lines[1], "Encryption: none");
        assert_eq!(lines[2], "Comment: user@example.com");
        assert_eq!(lines[3], "Public-Lines: 2");
        assert_eq!((lines[4].len(), lines[5].len()), (64, 4));
        assert_eq!(lines[6], "Private-Lines: 1");
        assert_eq!(lines[7].len(), 48);
        assert_eq!(lines[8].len(), "Private-MAC: ".len() + 64);
        assert_eq!(lines.len(), 9);
        assert!(ppk.ends_with('\n') && !ppk.contains('\r'));

        // The Ed25519 secret as its 32 bytes, in their own order.
        let secret = match russh::keys::decode_secret_key(&ppk, None)
            .unwrap()
            .key_data()
        {
            KeypairData::Ed25519(pair) => pair.private.to_bytes(),
            _ => unreachable!(),
        };
        assert_eq!(
            section(&ppk, "Private-Lines"),
            [&[0, 0, 0, 32], &secret[..]].concat()
        );
    }

    #[test]
    fn an_rsa_blob_holds_d_p_q_and_iqmp_in_that_order() {
        let key = key(KeyKind::Rsa { bits: 1024 });
        let ppk = key.encode(PrivateFormat::PuttyV2, None).unwrap();
        let KeypairData::Rsa(pair) = decode_secret_key(&ppk, None).unwrap().key_data().clone()
        else {
            unreachable!()
        };
        let private = pair.private();
        let mut expected = Vec::new();
        for field in [private.d(), private.p(), private.q(), private.iqmp()] {
            put_string(&mut expected, field.as_bytes());
        }
        assert_eq!(section(&ppk, "Private-Lines"), expected);
    }

    #[test]
    fn version_2_encryption_is_deterministic_and_version_3_is_not() {
        let key = key(KeyKind::EcdsaP256);
        // PuTTY's v2 has no salt, a zero IV and hash padding: the same key and
        // passphrase always give the same file, as they do in PuTTYgen.
        assert_eq!(
            *key.encode(PrivateFormat::PuttyV2, Some("pw")).unwrap(),
            *key.encode(PrivateFormat::PuttyV2, Some("pw")).unwrap()
        );
        let salt_and_after = |ppk: &str| ppk.split("Argon2-Salt: ").nth(1).unwrap().to_string();
        let a = key.encode(PrivateFormat::PuttyV3, Some("pw")).unwrap();
        let b = key.encode(PrivateFormat::PuttyV3, Some("pw")).unwrap();
        assert_ne!(salt_and_after(&a), salt_and_after(&b));
    }

    #[test]
    fn a_key_written_as_ppk_v3_reads_back_with_its_passphrase() {
        let key = key(KeyKind::Ed25519);
        let ppk = key
            .encode(PrivateFormat::PuttyV3, Some("hunter2 but longer"))
            .unwrap();

        assert!(ppk.starts_with(
            "PuTTY-User-Key-File-3: ssh-ed25519\n\
             Encryption: aes256-cbc\n\
             Comment: user@example.com\n"
        ));
        assert!(ppk.contains(
            "\nKey-Derivation: Argon2id\n\
             Argon2-Memory: 8192\n\
             Argon2-Passes: 13\n\
             Argon2-Parallelism: 1\n\
             Argon2-Salt: "
        ));

        // ssh-key's own reader, the one russh hands `.ppk` files to.
        let back = PrivateKey::from_ppk(&*ppk, Some("hunter2 but longer".into())).unwrap();
        assert_eq!(back.public_key().key_data(), key.public_key().key_data());
        assert_eq!(back.comment().as_str_lossy(), "user@example.com");
        assert!(PrivateKey::from_ppk(&*ppk, Some("hunter3".into())).is_err());
        assert!(PrivateKey::from_ppk(&*ppk, None).is_err());
    }

    #[test]
    fn the_mac_covers_the_comment() {
        let ppk = key(KeyKind::Ed25519)
            .encode(PrivateFormat::PuttyV3, None)
            .unwrap();
        let forged = ppk.replace("Comment: user@example.com", "Comment: root@example.com");
        assert!(decode_secret_key(&ppk, None).is_ok());
        assert!(decode_secret_key(&forged, None).is_err());
    }

    #[test]
    fn base64_wraps_at_64_characters() {
        let ppk = key(KeyKind::EcdsaP521)
            .encode(PrivateFormat::PuttyV3, Some("pw"))
            .unwrap();
        let base64: Vec<&str> = ppk.lines().filter(|line| !line.contains(": ")).collect();
        assert!(base64.len() > 4);
        assert!(base64.iter().all(|line| line.len() <= 64));
        assert_eq!(base64[0].len(), 64);
    }

    #[test]
    fn mpints_carry_no_leading_zeros_and_a_sign_byte_when_needed() {
        let encode = |bytes: &[u8]| {
            let mut out = Vec::new();
            put_mpint(&mut out, bytes);
            out
        };
        assert_eq!(encode(&[0, 0, 0x12]), [0, 0, 0, 1, 0x12]);
        assert_eq!(encode(&[0x80, 1]), [0, 0, 0, 3, 0, 0x80, 1]);
        assert_eq!(encode(&[0, 0]), [0, 0, 0, 0]);
    }
}
