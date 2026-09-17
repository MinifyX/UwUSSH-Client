//! PKCS#8 PEM: `BEGIN PRIVATE KEY`, or `BEGIN ENCRYPTED PRIVATE KEY`.
//!
//! This is the PEM that OpenSSL writes today and most crypto libraries read;
//! the per-algorithm files (`BEGIN RSA PRIVATE KEY`) are its deprecated
//! predecessors. Every key type comes out the way OpenSSL writes it, down to
//! Ed25519 as PKCS#8 version 1, without the optional public key that older
//! readers trip over.
//!
//! A passphrase encrypts with PBES2: PBKDF2-HMAC-SHA256 over 600,000 rounds,
//! OWASP's current advice, and AES-256-CBC. scrypt would resist guessing
//! better, but not every PKCS#8 reader knows it, and PEM is chosen precisely
//! for readers outside SSH.

use crate::{encode_error, KeygenError, Result};
use pkcs8::der::asn1::OctetStringRef;
use pkcs8::der::SecretDocument;
use pkcs8::pkcs5::pbes2;
use pkcs8::{
    AlgorithmIdentifierRef, EncodePrivateKey, LineEnding, ObjectIdentifier, PrivateKeyInfoRef,
};
use russh::keys::ssh_key::private::{EcdsaKeypair, KeypairData};
use russh::keys::PrivateKey;
use zeroize::Zeroizing;

const PBKDF2_ROUNDS: u32 = 600_000;

/// id-Ed25519, RFC 8410.
const ED25519_OID: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.101.112");

pub(crate) fn encode(key: &PrivateKey, passphrase: Option<&str>) -> Result<Zeroizing<String>> {
    let der = pkcs8_der(key)?;
    let Some(passphrase) = passphrase else {
        return der
            .to_pem("PRIVATE KEY", LineEnding::LF)
            .map_err(encode_error);
    };

    let mut salt = [0u8; 16];
    let mut iv = [0u8; 16];
    getrandom::fill(&mut salt).map_err(|_| KeygenError::Random)?;
    getrandom::fill(&mut iv).map_err(|_| KeygenError::Random)?;
    let params = pbes2::Parameters::generate_pbkdf2_sha256_aes256cbc(PBKDF2_ROUNDS, &salt, iv)
        .map_err(encode_error)?;

    PrivateKeyInfoRef::try_from(der.as_bytes())
        .map_err(encode_error)?
        .encrypt_with_params(params, passphrase)
        .map_err(encode_error)?
        .to_pem("ENCRYPTED PRIVATE KEY", LineEnding::LF)
        .map_err(encode_error)
}

fn pkcs8_der(key: &PrivateKey) -> Result<SecretDocument> {
    let der = match key.key_data() {
        KeypairData::Rsa(pair) => rsa::RsaPrivateKey::try_from(pair)
            .map_err(encode_error)?
            .to_pkcs8_der(),
        KeypairData::Ed25519(pair) => return ed25519_der(pair.private.as_ref()),
        KeypairData::Ecdsa(EcdsaKeypair::NistP256 { private, .. }) => {
            p256::SecretKey::from_slice(private.as_slice())
                .map_err(encode_error)?
                .to_pkcs8_der()
        }
        KeypairData::Ecdsa(EcdsaKeypair::NistP384 { private, .. }) => {
            p384::SecretKey::from_slice(private.as_slice())
                .map_err(encode_error)?
                .to_pkcs8_der()
        }
        KeypairData::Ecdsa(EcdsaKeypair::NistP521 { private, .. }) => {
            p521::SecretKey::from_slice(private.as_slice())
                .map_err(encode_error)?
                .to_pkcs8_der()
        }
        _ => {
            return Err(KeygenError::Unsupported {
                reason: format!("{} keys cannot be written as PEM", key.algorithm().as_str()),
            })
        }
    };
    der.map_err(encode_error)
}

/// The secret, wrapped once more as an OCTET STRING as RFC 8410 has it. Built
/// here rather than by the ed25519 crate, which leaves a copy of the secret
/// behind unless a feature nobody in this build enables is switched on.
fn ed25519_der(secret: &[u8; 32]) -> Result<SecretDocument> {
    let mut inner = Zeroizing::new([0u8; 34]);
    inner[0] = 0x04;
    inner[1] = 0x20;
    inner[2..].copy_from_slice(secret);

    let algorithm = AlgorithmIdentifierRef {
        oid: ED25519_OID,
        parameters: None,
    };
    let octets = OctetStringRef::new(&inner[..]).map_err(encode_error)?;
    SecretDocument::encode_msg(&PrivateKeyInfoRef::new(algorithm, octets)).map_err(encode_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pkcs8::pkcs5::EncryptionScheme;
    use pkcs8::EncryptedPrivateKeyInfoRef;
    use russh::keys::ssh_key::private::Ed25519Keypair;

    #[test]
    fn an_ed25519_key_is_laid_out_as_in_rfc_8410() {
        let secret = [0x42; 32];
        let key = PrivateKey::from(Ed25519Keypair::from_seed(&secret));
        let pem = encode(&key, None).unwrap();

        let (label, der) = SecretDocument::from_pem(&pem).unwrap();
        assert_eq!(label, "PRIVATE KEY");
        // SEQUENCE { INTEGER 0, SEQUENCE { OID 1.3.101.112 },
        //            OCTET STRING { OCTET STRING secret } }
        let header = [
            0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22,
            0x04, 0x20,
        ];
        assert_eq!(der.as_bytes(), [&header[..], &secret[..]].concat());
    }

    #[test]
    fn a_passphrase_means_pbkdf2_with_600000_rounds_and_aes_256() {
        let key = PrivateKey::from(Ed25519Keypair::from_seed(&[9; 32]));
        let pem = encode(&key, Some("pw")).unwrap();
        assert!(pem.starts_with("-----BEGIN ENCRYPTED PRIVATE KEY-----\n"));

        let (label, der) = SecretDocument::from_pem(&pem).unwrap();
        assert_eq!(label, "ENCRYPTED PRIVATE KEY");
        let info = EncryptedPrivateKeyInfoRef::try_from(der.as_bytes()).unwrap();
        let EncryptionScheme::Pbes2(params) = info.encryption_algorithm else {
            panic!("not PBES2");
        };
        let pbes2::Kdf::Pbkdf2(kdf) = params.kdf else {
            panic!("not PBKDF2");
        };
        assert_eq!(kdf.iteration_count, PBKDF2_ROUNDS);
        assert!(matches!(
            params.encryption,
            pbes2::EncryptionScheme::Aes256Cbc { .. }
        ));
    }
}
