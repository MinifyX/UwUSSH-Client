//! A key pair, what the UI shows about it, and the formats it is written in.

use crate::{encode_error, pem, ppk, KeygenError, Result};
use russh::keys::ssh_key::private::{EcdsaKeypair, KeypairData};
use russh::keys::ssh_key::LineEnding;
use russh::keys::{self, HashAlg, PrivateKey, PublicKey};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// The RSA sizes on offer.
///
/// 1024 is only for devices too old for anything else. Nothing above 4096:
/// 8192 takes the better part of half a minute to generate, which looks like a
/// hang, and the `rsa` crate russh signs with refuses anything larger.
pub const RSA_BITS: &[u32] = &[1024, 2048, 3072, 4096];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum KeyKind {
    Rsa { bits: u32 },
    Ed25519,
    EcdsaP256,
    EcdsaP384,
    EcdsaP521,
}

impl Default for KeyKind {
    /// RSA 2048: PuTTYgen's default, and the one key every server accepts,
    /// however old. Ed25519 is the better key wherever it is supported.
    fn default() -> Self {
        Self::Rsa { bits: 2048 }
    }
}

impl KeyKind {
    /// "RSA 2048", "Ed25519", "ECDSA P-256".
    pub fn label(self) -> String {
        match self {
            Self::Rsa { bits } => format!("RSA {bits}"),
            Self::Ed25519 => "Ed25519".into(),
            Self::EcdsaP256 => "ECDSA P-256".into(),
            Self::EcdsaP384 => "ECDSA P-384".into(),
            Self::EcdsaP521 => "ECDSA P-521".into(),
        }
    }

    pub fn bits(self) -> u32 {
        match self {
            Self::Rsa { bits } => bits,
            Self::Ed25519 | Self::EcdsaP256 => 256,
            Self::EcdsaP384 => 384,
            Self::EcdsaP521 => 521,
        }
    }

    /// The file name ssh-keygen would pick: `id_rsa`, `id_ed25519`, `id_ecdsa`.
    /// Add [`PrivateFormat::file_extension`] for the other formats.
    pub fn file_stem(self) -> &'static str {
        match self {
            Self::Rsa { .. } => "id_rsa",
            Self::Ed25519 => "id_ed25519",
            Self::EcdsaP256 | Self::EcdsaP384 | Self::EcdsaP521 => "id_ecdsa",
        }
    }

    /// The randomart title, as ssh-keygen prints it: `[ED25519 256]`.
    fn randomart_header(self) -> String {
        let family = match self {
            Self::Rsa { .. } => "RSA",
            Self::Ed25519 => "ED25519",
            Self::EcdsaP256 | Self::EcdsaP384 | Self::EcdsaP521 => "ECDSA",
        };
        format!("[{family} {}]", self.bits())
    }

    fn of(key: &PrivateKey) -> Option<Self> {
        match key.key_data() {
            KeypairData::Rsa(pair) => Some(Self::Rsa {
                bits: modulus_bits(pair.public().n().as_positive_bytes()?),
            }),
            KeypairData::Ed25519(_) => Some(Self::Ed25519),
            KeypairData::Ecdsa(EcdsaKeypair::NistP256 { .. }) => Some(Self::EcdsaP256),
            KeypairData::Ecdsa(EcdsaKeypair::NistP384 { .. }) => Some(Self::EcdsaP384),
            KeypairData::Ecdsa(EcdsaKeypair::NistP521 { .. }) => Some(Self::EcdsaP521),
            _ => None,
        }
    }
}

/// The exact size of a big-endian modulus. ssh-key rounds to whole bytes, so a
/// 2047-bit key from elsewhere would otherwise show up as "RSA 2048".
fn modulus_bits(big_endian: &[u8]) -> u32 {
    let significant = big_endian
        .iter()
        .position(|&byte| byte != 0)
        .map_or(&[][..], |start| &big_endian[start..]);
    match significant.first() {
        Some(first) => (significant.len() as u32 * 8).saturating_sub(first.leading_zeros()),
        None => 0,
    }
}

/// How a private key is written to a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PrivateFormat {
    /// `BEGIN OPENSSH PRIVATE KEY`, as ssh-keygen writes it. With a passphrase:
    /// bcrypt-pbkdf and AES-256-CTR, ssh-keygen's defaults.
    #[serde(rename = "openssh")]
    OpenSsh,
    /// PuTTY 0.75 and later, and current WinSCP. With a passphrase: Argon2id
    /// and AES-256-CBC.
    PuttyV3,
    /// For PuTTY-based tools older than that. Its passphrase protection is one
    /// round of SHA-1, which is quick to guess — use it only where v3 fails.
    PuttyV2,
    /// PKCS#8, for OpenSSL and everything built on it: `BEGIN PRIVATE KEY`, or
    /// `BEGIN ENCRYPTED PRIVATE KEY` with a passphrase.
    Pem,
}

impl PrivateFormat {
    /// Nothing for OpenSSH, whose keys are plain `id_ed25519` files.
    pub fn file_extension(self) -> &'static str {
        match self {
            Self::OpenSsh => "",
            Self::PuttyV3 | Self::PuttyV2 => "ppk",
            Self::Pem => "pem",
        }
    }
}

/// Everything PuTTYgen shows about a key, and nothing secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyInfo {
    /// The SSH name: `ssh-rsa`, `ssh-ed25519`, `ecdsa-sha2-nistp256`.
    pub algorithm: String,
    /// For people: `RSA 2048`, `Ed25519`, `ECDSA P-256`.
    pub label: String,
    pub bits: u32,
    pub comment: String,
    /// One line, comment included, ready for `authorized_keys`.
    pub public_openssh: String,
    /// `SHA256:…`, what OpenSSH shows.
    pub fingerprint_sha256: String,
    /// `MD5:aa:bb:…`, what older tools show, PuTTY before 0.75 among them.
    pub fingerprint_md5: String,
    pub randomart: String,
}

/// A private key, decrypted, in memory. ssh-key wipes its secret parts when
/// it is dropped.
pub struct KeyPair {
    key: PrivateKey,
    kind: KeyKind,
}

impl std::fmt::Debug for KeyPair {
    /// Public facts only, never key material.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "KeyPair({}, {})",
            self.kind.label(),
            self.key.fingerprint(HashAlg::Sha256)
        )
    }
}

impl KeyPair {
    pub(crate) fn new(mut key: PrivateKey) -> Result<Self> {
        let Some(kind) = KeyKind::of(&key) else {
            return Err(KeygenError::Unsupported {
                reason: format!("{} keys are not supported", key.algorithm().as_str()),
            });
        };
        let comment = single_line(key.comment().as_str_lossy());
        key.set_comment(comment);
        Ok(Self { key, kind })
    }

    pub fn kind(&self) -> KeyKind {
        self.kind
    }

    pub fn comment(&self) -> &str {
        self.key.comment().as_str_lossy()
    }

    /// A comment is one line in every format that has one; line breaks become
    /// spaces rather than breaking a `.ppk` file.
    pub fn set_comment(&mut self, comment: &str) {
        self.key.set_comment(single_line(comment));
    }

    pub fn public_key(&self) -> &PublicKey {
        self.key.public_key()
    }

    pub fn info(&self) -> KeyInfo {
        let public = self.key.public_key();
        let sha256 = public.fingerprint(HashAlg::Sha256);
        KeyInfo {
            algorithm: public.algorithm().as_str().to_string(),
            label: self.kind.label(),
            bits: self.kind.bits(),
            comment: self.comment().to_string(),
            public_openssh: public.to_openssh().unwrap_or_default(),
            fingerprint_sha256: sha256.to_string(),
            fingerprint_md5: md5_fingerprint(public),
            randomart: sha256.to_randomart(&self.kind.randomart_header()),
        }
    }

    /// The private key as file contents. An empty passphrase means none, as
    /// in ssh-keygen and PuTTYgen.
    pub fn encode(
        &self,
        format: PrivateFormat,
        passphrase: Option<&str>,
    ) -> Result<Zeroizing<String>> {
        let passphrase = passphrase.filter(|p| !p.is_empty());
        match format {
            PrivateFormat::OpenSsh => {
                let text = match passphrase {
                    None => self.key.to_openssh(LineEnding::LF),
                    Some(passphrase) => self
                        .key
                        .encrypt(&mut getrandom::SysRng, passphrase)
                        .and_then(|encrypted| encrypted.to_openssh(LineEnding::LF)),
                };
                text.map_err(encode_error)
            }
            PrivateFormat::PuttyV3 => ppk::encode(&self.key, ppk::Version::V3, passphrase),
            PrivateFormat::PuttyV2 => ppk::encode(&self.key, ppk::Version::V2, passphrase),
            PrivateFormat::Pem => pem::encode(&self.key, passphrase),
        }
    }
}

fn single_line(comment: &str) -> String {
    comment
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn md5_fingerprint(public: &PublicKey) -> String {
    let blob = public.to_bytes().unwrap_or_default();
    let digest = md5::compute(blob);
    let pairs: Vec<String> = digest.0.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("MD5:{}", pairs.join(":"))
}

/// Read an existing private key: OpenSSH, PEM (PKCS#1, SEC1, PKCS#8, also
/// encrypted) or PuTTY `.ppk` v2 and v3 — whatever russh reads, because russh
/// is what will use it.
///
/// A key that turns out to be encrypted ends in
/// [`KeygenError::PassphraseRequired`] when no passphrase came with it, so the
/// UI can ask and call again.
pub fn load(text: &str, passphrase: Option<&str>) -> Result<KeyPair> {
    // First without a passphrase, whatever the caller brought: russh treats a
    // passphrase as an instruction to decrypt, and fails on a PKCS#8 key that
    // is not encrypted.
    let first = match keys::decode_secret_key(text, None) {
        Ok(key) => return KeyPair::new(key),
        Err(err) => err,
    };
    if !is_encrypted(text, &first) {
        return Err(KeygenError::Unreadable {
            reason: first.to_string(),
        });
    }
    let Some(passphrase) = passphrase else {
        return Err(KeygenError::PassphraseRequired);
    };
    // The file's own key derivation settings, before they cost anything.
    crate::check_costs(text)?;
    match keys::decode_secret_key(text, Some(passphrase)) {
        Ok(key) => KeyPair::new(key),
        Err(_) if passphrase.is_empty() => Err(KeygenError::PassphraseRequired),
        Err(_) => Err(KeygenError::PassphraseWrong),
    }
}

fn is_encrypted(text: &str, err: &keys::Error) -> bool {
    // OpenSSH, and the old `Proc-Type: 4,ENCRYPTED` PEM files.
    if matches!(err, keys::Error::KeyIsEncrypted) {
        return true;
    }
    if text.contains("-----BEGIN ENCRYPTED PRIVATE KEY-----") {
        return true;
    }
    text.trim_start().starts_with("PuTTY-User-Key-File-")
        && text.lines().any(|line| {
            line.strip_prefix("Encryption:")
                .is_some_and(|value| value.trim() != "none")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{generate, GenerateOptions};

    const PASSPHRASE: &str = "correct horse battery staple";

    const FORMATS: [PrivateFormat; 4] = [
        PrivateFormat::OpenSsh,
        PrivateFormat::PuttyV3,
        PrivateFormat::PuttyV2,
        PrivateFormat::Pem,
    ];

    fn key(kind: KeyKind, comment: &str) -> KeyPair {
        generate(&GenerateOptions {
            kind,
            comment: comment.into(),
            extra_entropy: Vec::new(),
        })
        .unwrap()
    }

    #[test]
    fn every_kind_generates_with_the_right_algorithm_size_and_label() {
        let cases = [
            (KeyKind::Rsa { bits: 2048 }, "ssh-rsa", 2048, "RSA 2048"),
            (KeyKind::Ed25519, "ssh-ed25519", 256, "Ed25519"),
            (
                KeyKind::EcdsaP256,
                "ecdsa-sha2-nistp256",
                256,
                "ECDSA P-256",
            ),
            (
                KeyKind::EcdsaP384,
                "ecdsa-sha2-nistp384",
                384,
                "ECDSA P-384",
            ),
            (
                KeyKind::EcdsaP521,
                "ecdsa-sha2-nistp521",
                521,
                "ECDSA P-521",
            ),
        ];
        for (kind, algorithm, bits, label) in cases {
            let pair = key(kind, "lorin@uwussh");
            assert_eq!(pair.kind(), kind);
            let info = pair.info();
            assert_eq!(info.algorithm, algorithm);
            assert_eq!(info.bits, bits, "{label}");
            assert_eq!(info.label, label);
            assert_eq!(info.comment, "lorin@uwussh");
        }
    }

    #[test]
    fn the_default_is_rsa_2048() {
        assert_eq!(KeyKind::default(), KeyKind::Rsa { bits: 2048 });
        assert_eq!(GenerateOptions::default().kind, KeyKind::Rsa { bits: 2048 });
    }

    #[test]
    fn every_kind_in_every_format_reads_back_through_russh() {
        let kinds = [
            KeyKind::Rsa { bits: 2048 },
            KeyKind::Ed25519,
            KeyKind::EcdsaP256,
            KeyKind::EcdsaP384,
            KeyKind::EcdsaP521,
        ];
        for kind in kinds {
            let pair = key(kind, "round@trip");
            for format in FORMATS {
                for passphrase in [None, Some(PASSPHRASE)] {
                    let case = format!(
                        "{} as {format:?}, passphrase {:?}",
                        kind.label(),
                        passphrase.is_some()
                    );
                    let text = pair.encode(format, passphrase).unwrap();

                    let read = keys::decode_secret_key(&text, passphrase)
                        .unwrap_or_else(|err| panic!("{case}: {err}"));
                    assert_eq!(
                        read.public_key().key_data(),
                        pair.public_key().key_data(),
                        "{case}"
                    );

                    if passphrase.is_some() {
                        assert!(
                            keys::decode_secret_key(&text, Some("wrong")).is_err(),
                            "{case}: a wrong passphrase must fail"
                        );
                        assert!(
                            matches!(load(&text, None), Err(KeygenError::PassphraseRequired)),
                            "{case}"
                        );
                        assert!(
                            matches!(
                                load(&text, Some("wrong")),
                                Err(KeygenError::PassphraseWrong)
                            ),
                            "{case}"
                        );
                    }
                    let loaded =
                        load(&text, passphrase).unwrap_or_else(|err| panic!("{case}: {err}"));
                    assert_eq!(
                        loaded.public_key().key_data(),
                        pair.public_key().key_data(),
                        "{case}"
                    );
                }
            }
        }
    }

    #[test]
    fn an_empty_passphrase_writes_an_unencrypted_key() {
        let pair = key(KeyKind::Ed25519, "");
        for format in FORMATS {
            let text = pair.encode(format, Some("")).unwrap();
            assert!(keys::decode_secret_key(&text, None).is_ok(), "{format:?}");
        }
    }

    #[test]
    fn a_passphrase_given_for_an_unencrypted_key_is_ignored() {
        let pair = key(KeyKind::EcdsaP256, "");
        for format in FORMATS {
            let text = pair.encode(format, None).unwrap();
            let loaded = load(&text, Some(PASSPHRASE)).unwrap();
            assert_eq!(loaded.public_key().key_data(), pair.public_key().key_data());
        }
    }

    #[test]
    fn an_openssh_key_converts_to_ppk_v3_with_its_comment() {
        let original = key(KeyKind::Ed25519, "nas@home");
        let openssh = original
            .encode(PrivateFormat::OpenSsh, Some(PASSPHRASE))
            .unwrap();

        let loaded = load(&openssh, Some(PASSPHRASE)).unwrap();
        assert_eq!(loaded.comment(), "nas@home");
        let ppk = loaded.encode(PrivateFormat::PuttyV3, None).unwrap();

        assert!(ppk.contains("\nComment: nas@home\n"));
        let read = keys::decode_secret_key(&ppk, None).unwrap();
        assert_eq!(
            read.public_key().key_data(),
            original.public_key().key_data()
        );
        assert_eq!(read.comment().as_str_lossy(), "nas@home");
    }

    #[test]
    fn the_public_line_parses_and_the_fingerprints_match_ssh_key() {
        let pair = key(KeyKind::EcdsaP384, "lorin@uwussh");
        let info = pair.info();

        let public = PublicKey::from_openssh(&info.public_openssh).unwrap();
        assert_eq!(public.key_data(), pair.public_key().key_data());
        assert_eq!(public.comment().as_str_lossy(), "lorin@uwussh");
        assert!(info.public_openssh.starts_with("ecdsa-sha2-nistp384 AAAA"));
        assert!(!info.public_openssh.contains('\n'));

        assert_eq!(
            info.fingerprint_sha256,
            public.fingerprint(HashAlg::Sha256).to_string()
        );
        assert!(info.randomart.starts_with("+---[ECDSA 384]---+"));

        let md5 = md5::compute(public.to_bytes().unwrap());
        assert_eq!(info.fingerprint_md5.len(), "MD5:".len() + 16 * 3 - 1);
        assert!(info
            .fingerprint_md5
            .starts_with(&format!("MD5:{:02x}:", md5.0[0])));
    }

    #[test]
    fn line_breaks_in_a_comment_become_spaces() {
        let mut pair = key(KeyKind::Ed25519, "two\nlines");
        assert_eq!(pair.comment(), "two lines");
        pair.set_comment("injected\nPrivate-MAC: 00");
        let ppk = pair.encode(PrivateFormat::PuttyV3, None).unwrap();
        assert!(keys::decode_secret_key(&ppk, None).is_ok());
        assert_eq!(pair.info().comment, "injected Private-MAC: 00");
    }

    #[test]
    fn garbage_is_unreadable_not_a_passphrase_prompt() {
        for text in [
            "",
            "hello",
            "-----BEGIN OPENSSH PRIVATE KEY-----\nnope\n-----END OPENSSH PRIVATE KEY-----\n",
            "PuTTY-User-Key-File-3: ssh-ed25519\nEncryption: none\n",
        ] {
            assert!(
                matches!(
                    load(text, Some(PASSPHRASE)),
                    Err(KeygenError::Unreadable { .. })
                ),
                "{text:?}"
            );
        }
    }

    #[test]
    fn debug_output_is_the_label_and_fingerprint_and_nothing_else() {
        let pair = key(KeyKind::Ed25519, "");
        assert_eq!(
            format!("{pair:?}"),
            format!(
                "KeyPair(Ed25519, {})",
                pair.public_key().fingerprint(HashAlg::Sha256)
            )
        );
    }

    #[test]
    fn a_modulus_is_measured_to_the_bit() {
        assert_eq!(modulus_bits(&[0x80, 0, 0]), 24);
        assert_eq!(modulus_bits(&[0x7f, 0xff]), 15);
        assert_eq!(modulus_bits(&[0, 0x01]), 1);
        assert_eq!(modulus_bits(&[]), 0);
    }

    #[test]
    fn formats_serialise_as_the_ui_names_them() {
        assert_eq!(
            serde_json::to_value(PrivateFormat::OpenSsh).unwrap(),
            "openssh"
        );
        assert_eq!(
            serde_json::to_value(PrivateFormat::PuttyV3).unwrap(),
            "putty-v3"
        );
        let kind: KeyKind = serde_json::from_str(r#"{"type":"rsa","bits":4096}"#).unwrap();
        assert_eq!(kind, KeyKind::Rsa { bits: 4096 });
        let kind: KeyKind = serde_json::from_str(r#"{"type":"ecdsa-p521"}"#).unwrap();
        assert_eq!(kind, KeyKind::EcdsaP521);
        assert_eq!(PrivateFormat::PuttyV2.file_extension(), "ppk");
        assert_eq!(PrivateFormat::OpenSsh.file_extension(), "");
        assert_eq!(KeyKind::EcdsaP384.file_stem(), "id_ecdsa");
    }
}
