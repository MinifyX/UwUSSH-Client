//! How much work a key file may ask for before its passphrase is tried.
//!
//! An encrypted key file carries its own key derivation settings — Argon2's
//! memory and passes in a `.ppk`, bcrypt rounds in an OpenSSH key, PBKDF2
//! iterations or scrypt's `N` in PKCS#8 — and the libraries that open it take
//! them as written. A file asking for four billion passes freezes whoever types
//! a passphrase for it; one asking scrypt for a terabyte ends the process. So
//! the settings are read first, and a file asking for far more than any real
//! key file does is refused.

use crate::{KeygenError, Result};
use pkcs8::der::Decode as _;
use pkcs8::pkcs5::pbes2::Kdf;
use russh::keys::ssh_key;

/// 1 GiB of Argon2 memory; PuTTYgen uses 8 MiB.
const ARGON2_MEMORY_KIB: u64 = 1024 * 1024;
const ARGON2_PASSES: u64 = 1024;
/// Memory times passes: 16 GiB worth of work.
const ARGON2_WORK_KIB: u64 = 16 * 1024 * 1024;
const ARGON2_PARALLELISM: u64 = 16;
/// `ssh-keygen` uses 16; people who raise it with `-a` rarely go past 500.
const BCRYPT_ROUNDS: u32 = 1024;
const PBKDF2_ITERATIONS: u32 = 10_000_000;
/// scrypt's memory: `128 · N · r` bytes.
const SCRYPT_MEMORY_BYTES: u128 = 1024 * 1024 * 1024;
const SCRYPT_PARALLELISM: u16 = 16;

/// `Ok` when opening `text` with a passphrase costs a sane amount of work, or
/// when it isn't a format that says.
pub fn check_costs(text: &str) -> Result<()> {
    let text = text.trim_start();
    if text.starts_with("PuTTY-User-Key-File-") {
        ppk(text)
    } else if text.contains("-----BEGIN OPENSSH PRIVATE KEY-----") {
        openssh(text)
    } else if text.contains("-----BEGIN ENCRYPTED PRIVATE KEY-----") {
        pkcs8_file(text)
    } else {
        Ok(())
    }
}

fn too_costly(what: &str) -> KeygenError {
    KeygenError::Unsupported {
        reason: format!(
            "this key file asks for far more {what} than any real key does, so it is not opened"
        ),
    }
}

fn ppk(text: &str) -> Result<()> {
    let field = |name: &str| -> Option<u64> {
        text.lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))
            .map(|value| value.trim().parse().unwrap_or(u64::MAX))
    };
    let memory = field("Argon2-Memory").unwrap_or(0);
    let passes = field("Argon2-Passes").unwrap_or(0);
    let parallelism = field("Argon2-Parallelism").unwrap_or(0);
    if memory > ARGON2_MEMORY_KIB
        || passes > ARGON2_PASSES
        || memory.saturating_mul(passes) > ARGON2_WORK_KIB
        || parallelism > ARGON2_PARALLELISM
    {
        return Err(too_costly("Argon2 work"));
    }
    Ok(())
}

fn openssh(text: &str) -> Result<()> {
    // A file that doesn't parse fails later, with the library's own words.
    let Ok(key) = ssh_key::PrivateKey::from_openssh(text) else {
        return Ok(());
    };
    match key.kdf() {
        ssh_key::Kdf::Bcrypt { rounds, .. } if *rounds > BCRYPT_ROUNDS => {
            Err(too_costly("bcrypt rounds"))
        }
        _ => Ok(()),
    }
}

fn pkcs8_file(text: &str) -> Result<()> {
    let Ok((_, der)) = pkcs8::der::pem::decode_vec(text.as_bytes()) else {
        return Ok(());
    };
    let Ok(info) = pkcs8::EncryptedPrivateKeyInfoRef::from_der(&der) else {
        return Ok(());
    };
    // PBES1 counts its iterations in 16 bits, which bounds them already.
    let Some(pbes2) = info.encryption_algorithm.pbes2() else {
        return Ok(());
    };
    match &pbes2.kdf {
        Kdf::Pbkdf2(params) if params.iteration_count > PBKDF2_ITERATIONS => {
            Err(too_costly("PBKDF2 iterations"))
        }
        Kdf::Scrypt(params)
            if u128::from(params.cost_parameter)
                .saturating_mul(u128::from(params.block_size))
                .saturating_mul(128)
                > SCRYPT_MEMORY_BYTES
                || params.parallelization > SCRYPT_PARALLELISM =>
        {
            Err(too_costly("scrypt memory"))
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{generate_with_rng, GenerateOptions, KeyKind, PrivateFormat};

    fn sample() -> crate::KeyPair {
        use russh::keys::ssh_key::rand_core::SeedableRng as _;
        let mut rng = chacha20::ChaCha20Rng::from_seed([7; 32]);
        generate_with_rng(
            &GenerateOptions {
                kind: KeyKind::Ed25519,
                comment: "costs".into(),
                extra_entropy: Vec::new(),
            },
            &mut rng,
        )
        .unwrap()
    }

    #[test]
    fn real_key_files_pass() {
        let key = sample();
        for format in [
            PrivateFormat::OpenSsh,
            PrivateFormat::PuttyV3,
            PrivateFormat::PuttyV2,
            PrivateFormat::Pem,
        ] {
            let text = key.encode(format, Some("secret")).unwrap();
            assert!(check_costs(&text).is_ok(), "{format:?}");
        }
    }

    #[test]
    fn a_ppk_asking_for_endless_argon2_is_refused() {
        let text = sample()
            .encode(PrivateFormat::PuttyV3, Some("secret"))
            .unwrap();
        let passes = text
            .lines()
            .find(|line| line.starts_with("Argon2-Passes:"))
            .unwrap()
            .to_string();
        let crafted = text.replace(&passes, "Argon2-Passes: 4294967295");
        assert!(matches!(
            check_costs(&crafted),
            Err(KeygenError::Unsupported { .. })
        ));
        let memory = text
            .lines()
            .find(|line| line.starts_with("Argon2-Memory:"))
            .unwrap()
            .to_string();
        let crafted = text.replace(&memory, "Argon2-Memory: 67108864");
        assert!(check_costs(&crafted).is_err());
    }
}
