//! Making new keys.
//!
//! PuTTYgen asks for mouse movement before it generates a key, a habit from a
//! time when an operating system's randomness could not be taken for granted.
//! Here the key rests on the OS alone: 32 bytes from the system's generator
//! seed the ChaCha20 stream that every random bit of the key is drawn from.
//!
//! A UI may still collect movement and pass it as `extra_entropy`. It is hashed
//! in with the OS bytes — the seed is SHA-256(OS bytes ‖ extra entropy) — so it
//! can add unpredictability but never take any away: empty, constant, even
//! attacker-chosen input leaves the key exactly as strong as the OS bytes make
//! it. It is there because people expect to see it, not because the key needs
//! it.

use crate::key::{KeyKind, KeyPair, RSA_BITS};
use crate::{KeygenError, Result};
use chacha20::ChaCha20Rng;
use russh::keys::ssh_key::private::{EcdsaKeypair, Ed25519Keypair, KeypairData, RsaKeypair};
use russh::keys::ssh_key::rand_core::{CryptoRng, SeedableRng};
use russh::keys::ssh_key::sha2::{Digest, Sha256};
use russh::keys::{EcdsaCurve, PrivateKey};
use serde::Deserialize;
use zeroize::Zeroizing;

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GenerateOptions {
    pub kind: KeyKind,
    pub comment: String,
    /// Mouse movement and the like from the UI, mixed into the seed. Optional,
    /// and never what the key's strength depends on — see the module docs.
    pub extra_entropy: Vec<u8>,
}

/// Generate a key from the OS's randomness, with the options' extra entropy
/// mixed in.
pub fn generate(options: &GenerateOptions) -> Result<KeyPair> {
    let mut os_random = Zeroizing::new([0u8; 32]);
    getrandom::fill(&mut *os_random).map_err(|_| KeygenError::Random)?;
    let mut rng = seeded_rng(&os_random, &options.extra_entropy);
    generate_with_rng(options, &mut rng)
}

/// Generate a key from the given generator, ignoring `extra_entropy`. For
/// tests, which need the same key twice.
pub fn generate_with_rng<R: CryptoRng + ?Sized>(
    options: &GenerateOptions,
    rng: &mut R,
) -> Result<KeyPair> {
    let key_data = match options.kind {
        KeyKind::Rsa { bits } => {
            if !RSA_BITS.contains(&bits) {
                return Err(KeygenError::RsaSize { bits });
            }
            KeypairData::from(RsaKeypair::random(rng, bits as usize).map_err(generate_error)?)
        }
        KeyKind::Ed25519 => KeypairData::from(ed25519(rng)),
        KeyKind::EcdsaP256 => ecdsa(rng, EcdsaCurve::NistP256)?,
        KeyKind::EcdsaP384 => ecdsa(rng, EcdsaCurve::NistP384)?,
        KeyKind::EcdsaP521 => ecdsa(rng, EcdsaCurve::NistP521)?,
    };
    let key = PrivateKey::new(key_data, options.comment.as_str()).map_err(generate_error)?;
    KeyPair::new(key)
}

fn seeded_rng(os_random: &[u8; 32], extra_entropy: &[u8]) -> ChaCha20Rng {
    let seed: Zeroizing<[u8; 32]> = Zeroizing::new(
        Sha256::new()
            .chain_update(os_random)
            .chain_update(extra_entropy)
            .finalize()
            .into(),
    );
    ChaCha20Rng::from_seed(*seed)
}

fn ed25519<R: CryptoRng + ?Sized>(rng: &mut R) -> Ed25519Keypair {
    // ssh-key, and so russh, reads the Ed25519 secret in a `.ppk` as an SSH
    // mpint, and rejects the 1 in 512 secrets that start with a zero byte and
    // then one below 0x80 as having "an unnecessary leading zero". PuTTY reads
    // those files fine; UwUSSH itself could not. So such a secret is drawn
    // again, which costs less than 0.003 of its 256 bits.
    loop {
        let pair = Ed25519Keypair::random(rng);
        let secret = pair.private.as_ref();
        if !(secret[0] == 0 && secret[1] < 0x80) {
            return pair;
        }
    }
}

fn ecdsa<R: CryptoRng + ?Sized>(rng: &mut R, curve: EcdsaCurve) -> Result<KeypairData> {
    EcdsaKeypair::random(rng, curve)
        .map(KeypairData::from)
        .map_err(generate_error)
}

fn generate_error(err: impl std::fmt::Display) -> KeygenError {
    KeygenError::Generate {
        reason: err.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::PrivateFormat;
    use russh::keys::ssh_key::rand_core::{Infallible, TryCryptoRng, TryRng};

    fn options(kind: KeyKind, extra_entropy: &[u8]) -> GenerateOptions {
        GenerateOptions {
            kind,
            comment: "seed@test".into(),
            extra_entropy: extra_entropy.to_vec(),
        }
    }

    #[test]
    fn the_same_seed_gives_the_same_key() {
        for kind in [
            KeyKind::Ed25519,
            KeyKind::EcdsaP256,
            KeyKind::Rsa { bits: 1024 },
        ] {
            let a = generate_with_rng(&options(kind, b""), &mut ChaCha20Rng::from_seed([7; 32]))
                .unwrap();
            let b = generate_with_rng(&options(kind, b""), &mut ChaCha20Rng::from_seed([7; 32]))
                .unwrap();
            assert_eq!(a.public_key().key_data(), b.public_key().key_data());
        }
    }

    #[test]
    fn two_generated_keys_differ() {
        let a = generate(&options(KeyKind::Ed25519, b"same")).unwrap();
        let b = generate(&options(KeyKind::Ed25519, b"same")).unwrap();
        assert_ne!(a.public_key().key_data(), b.public_key().key_data());
    }

    #[test]
    fn the_same_os_bytes_with_different_extra_entropy_give_a_different_key() {
        let os_random = [42u8; 32];
        let key_from = |entropy: &[u8]| {
            generate_with_rng(
                &options(KeyKind::Ed25519, b""),
                &mut seeded_rng(&os_random, entropy),
            )
            .unwrap()
        };
        let still = key_from(b"");
        let wiggled = key_from(b"mouse at 120,48 then 133,52");
        let wiggled_again = key_from(b"mouse at 120,48 then 133,52");

        assert_ne!(
            still.public_key().key_data(),
            wiggled.public_key().key_data()
        );
        assert_eq!(
            wiggled.public_key().key_data(),
            wiggled_again.public_key().key_data()
        );
    }

    #[test]
    fn an_rsa_size_that_is_not_offered_is_rejected() {
        let mut rng = ChaCha20Rng::from_seed([1; 32]);
        for bits in [0, 512, 1023, 2047, 8192, 16384] {
            assert!(matches!(
                generate_with_rng(&options(KeyKind::Rsa { bits }, b""), &mut rng),
                Err(KeygenError::RsaSize { bits: rejected }) if rejected == bits
            ));
        }
    }

    #[test]
    fn rsa_1024_is_still_there_for_old_devices() {
        let pair = generate_with_rng(
            &options(KeyKind::Rsa { bits: 1024 }, b""),
            &mut ChaCha20Rng::from_seed([3; 32]),
        )
        .unwrap();
        assert_eq!(pair.info().bits, 1024);
        assert_eq!(pair.info().label, "RSA 1024");
        let text = pair.encode(PrivateFormat::PuttyV3, None).unwrap();
        let read = russh::keys::decode_secret_key(&text, None).unwrap();
        assert_eq!(read.public_key().key_data(), pair.public_key().key_data());
    }

    /// Hands out a fixed prefix, then zeros forever.
    struct Scripted(Vec<u8>);

    impl TryRng for Scripted {
        type Error = Infallible;

        fn try_next_u32(&mut self) -> std::result::Result<u32, Infallible> {
            let mut bytes = [0; 4];
            self.try_fill_bytes(&mut bytes)?;
            Ok(u32::from_le_bytes(bytes))
        }

        fn try_next_u64(&mut self) -> std::result::Result<u64, Infallible> {
            let mut bytes = [0; 8];
            self.try_fill_bytes(&mut bytes)?;
            Ok(u64::from_le_bytes(bytes))
        }

        fn try_fill_bytes(&mut self, dst: &mut [u8]) -> std::result::Result<(), Infallible> {
            for byte in dst {
                *byte = if self.0.is_empty() {
                    0
                } else {
                    self.0.remove(0)
                };
            }
            Ok(())
        }
    }

    impl TryCryptoRng for Scripted {}

    #[test]
    fn an_ed25519_secret_russh_could_not_read_from_a_ppk_is_drawn_again() {
        let mut unreadable = [0x5a; 32];
        unreadable[0] = 0x00;
        unreadable[1] = 0x7f;
        let readable = [0x11; 32];

        // The reason for the redraw: ssh-key refuses exactly such a file. When
        // this starts to pass, ssh-key is fixed and the redraw can go.
        let stuck = KeyPair::new(PrivateKey::from(Ed25519Keypair::from_seed(&unreadable))).unwrap();
        let ppk = stuck.encode(PrivateFormat::PuttyV3, None).unwrap();
        assert!(russh::keys::decode_secret_key(&ppk, None).is_err());

        let script = [unreadable, readable].concat();
        let pair =
            generate_with_rng(&options(KeyKind::Ed25519, b""), &mut Scripted(script)).unwrap();
        let expected = PrivateKey::from(Ed25519Keypair::from_seed(&readable));
        assert_eq!(
            pair.public_key().key_data(),
            expected.public_key().key_data()
        );
    }
}
