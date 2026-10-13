//! Team keys.
//!
//! A team's records are sealed under one team key, exactly as a vault's are
//! under its vault key. What is new is how that key reaches the members:
//!
//! - Every account has one **member key**, an X25519 pair made on a device.
//!   The public half goes to the server as it is. The private half goes there
//!   too, sealed under the vault key of the account's own vault on that
//!   server, so the account's other devices get it and the server cannot
//!   read it.
//! - The team key is **wrapped** once per member in a sealed box: a fresh
//!   X25519 pair, a shared secret with the member's public key, HKDF-SHA256,
//!   XChaCha20-Poly1305. The associated data names the team, the epoch, the
//!   member's account and the member's public key, so the server can neither
//!   hand a member the key of another team, nor an old epoch's key as the
//!   current one, nor a key wrapped for somebody else.
//! - Anybody can seal a box to a public key, the server too. So a wrap also
//!   carries the keys that came **before** — as many as [`MAX_EARLIER`] —
//!   and a member who already holds a key takes a newer one only when the
//!   key it holds is among them at the right distance. A server cannot make
//!   that up without a key it never had.
//! - The **fingerprint** of a public key is what two people compare before a
//!   newcomer is given the key. The server hands out public keys; a server
//!   that lied about one would otherwise be handed the team key.

use crate::{decrypt_keyed, encrypt_keyed, Result, Sealed, UnlockedVault, VaultError};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use curve25519_dalek::montgomery::MontgomeryPoint;
use hkdf::Hkdf;
use rand::RngCore;
use sha2::{Digest, Sha256};
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

/// The first byte of everything this module writes.
const VERSION: u8 = 1;

const WRAP_LABEL: &[u8] = b"uwussh/team/wrap/v1";
const MEMBER_LABEL: &[u8] = b"uwussh/team/member-key/v1";
const FINGERPRINT_LABEL: &[u8] = b"uwussh/team/fingerprint/v1";

/// `version || ephemeral public key || nonce || sealed team key`, without
/// earlier keys. Each of those adds 32 bytes.
pub const WRAPPED_LEN: usize = 1 + 32 + 24 + 32 + 16;
/// As many earlier keys as a wrap carries: a member that was away for more
/// rotations than this cannot tell the new key from a made-up one.
pub const MAX_EARLIER: usize = 8;
/// `version || nonce || sealed private key`.
pub const SEALED_MEMBER_LEN: usize = 1 + 24 + 32 + 16;

/// An account's private member key.
pub struct MemberKey {
    secret: Zeroizing<[u8; 32]>,
}

impl std::fmt::Debug for MemberKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemberKey").finish_non_exhaustive()
    }
}

impl MemberKey {
    pub fn generate() -> Self {
        let mut secret = Zeroizing::new([0u8; 32]);
        rand::thread_rng().fill_bytes(secret.as_mut());
        Self { secret }
    }

    pub fn from_secret(secret: Zeroizing<[u8; 32]>) -> Self {
        Self { secret }
    }

    pub fn public(&self) -> [u8; 32] {
        MontgomeryPoint::mul_base_clamped(*self.secret).to_bytes()
    }

    /// The private key sealed under the account's vault on that server, as
    /// the server keeps it (`sealed_private`).
    pub fn seal(&self, vault: &UnlockedVault, account: Uuid) -> Result<Vec<u8>> {
        let key = vault.export_key();
        let sealed = encrypt_keyed(
            &key,
            &member_label(vault.vault_id(), account),
            self.secret.as_ref(),
        )?;
        let mut out = Vec::with_capacity(SEALED_MEMBER_LEN);
        out.push(VERSION);
        out.extend_from_slice(&sealed.nonce);
        out.extend_from_slice(&sealed.blob);
        Ok(out)
    }

    /// Open what [`MemberKey::seal`] wrote. Fails for another vault, another
    /// account, and anything changed on the way.
    pub fn open(vault: &UnlockedVault, account: Uuid, sealed: &[u8]) -> Result<Self> {
        if sealed.len() != SEALED_MEMBER_LEN || sealed[0] != VERSION {
            return Err(VaultError::Decrypt);
        }
        let key = vault.export_key();
        let mut plain = decrypt_keyed(
            &key,
            &member_label(vault.vault_id(), account),
            &Sealed {
                nonce: sealed[1..25].to_vec(),
                blob: sealed[25..].to_vec(),
            },
        )?;
        let secret = <[u8; 32]>::try_from(plain.as_slice()).map(Zeroizing::new);
        plain.zeroize();
        secret
            .map(|secret| Self { secret })
            .map_err(|_| VaultError::Decrypt)
    }
}

fn member_label(vault_id: Uuid, account: Uuid) -> Vec<u8> {
    [MEMBER_LABEL, vault_id.as_bytes(), account.as_bytes()].concat()
}

/// Whom a wrapped team key is for, and which key of which team it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WrapContext {
    pub team: Uuid,
    pub epoch: u64,
    pub account: Uuid,
}

/// What a wrap holds: the team key and the ones before it, newest first.
pub struct TeamKeys {
    pub key: Zeroizing<[u8; 32]>,
    pub earlier: Vec<Zeroizing<[u8; 32]>>,
}

impl std::fmt::Debug for TeamKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TeamKeys").finish_non_exhaustive()
    }
}

impl TeamKeys {
    /// Whether these keys, being those of `epoch`, come after `held`, the
    /// key of `held_epoch`: the same key at the same epoch, or `held` where
    /// it has to be among the earlier ones.
    pub fn follows(&self, epoch: u64, held_epoch: u64, held: &[u8; 32]) -> bool {
        if epoch == held_epoch {
            return same(&self.key, held);
        }
        let Some(back) = epoch.checked_sub(held_epoch) else {
            return false;
        };
        usize::try_from(back - 1)
            .ok()
            .and_then(|index| self.earlier.get(index))
            .is_some_and(|earlier| same(earlier, held))
    }

    /// The earlier keys a wrap of the key after this one carries.
    pub fn handed_on(&self) -> Vec<Zeroizing<[u8; 32]>> {
        std::iter::once(self.key.clone())
            .chain(self.earlier.iter().cloned())
            .take(MAX_EARLIER)
            .collect()
    }
}

fn same(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A fresh team key.
pub fn new_team_key() -> Zeroizing<[u8; 32]> {
    let mut key = Zeroizing::new([0u8; 32]);
    rand::thread_rng().fill_bytes(key.as_mut());
    key
}

/// Wrap a team key for one member, with the keys before it (newest first;
/// no more than [`MAX_EARLIER`] are taken).
pub fn wrap_team_key(
    team_key: &[u8; 32],
    earlier: &[Zeroizing<[u8; 32]>],
    context: WrapContext,
    recipient: &[u8; 32],
) -> Result<Vec<u8>> {
    let mut ephemeral = Zeroizing::new([0u8; 32]);
    rand::thread_rng().fill_bytes(ephemeral.as_mut());
    let mut nonce = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut nonce);
    let earlier = &earlier[..earlier.len().min(MAX_EARLIER)];
    let mut keys = Zeroizing::new(Vec::with_capacity(32 * (1 + earlier.len())));
    keys.extend_from_slice(team_key);
    for key in earlier {
        keys.extend_from_slice(key.as_ref());
    }
    wrap_with(&keys, context, recipient, &ephemeral, &nonce)
}

fn wrap_with(
    team_key: &[u8],
    context: WrapContext,
    recipient: &[u8; 32],
    ephemeral: &[u8; 32],
    nonce: &[u8; 24],
) -> Result<Vec<u8>> {
    let ephemeral_public = MontgomeryPoint::mul_base_clamped(*ephemeral).to_bytes();
    let key = box_key(ephemeral, recipient, &ephemeral_public, recipient)?;
    let blob = XChaCha20Poly1305::new(key.as_ref().into())
        .encrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: team_key,
                aad: &wrap_aad(context, recipient, &ephemeral_public),
            },
        )
        .map_err(|e| VaultError::Encrypt(e.to_string()))?;
    let mut out = Vec::with_capacity(WRAPPED_LEN - 32 + team_key.len());
    out.push(VERSION);
    out.extend_from_slice(&ephemeral_public);
    out.extend_from_slice(nonce);
    out.extend_from_slice(&blob);
    Ok(out)
}

/// Open a team key wrapped for this member. Fails unless it was wrapped for
/// exactly this team, epoch, account and key.
pub fn unwrap_team_key(
    member: &MemberKey,
    context: WrapContext,
    wrapped: &[u8],
) -> Result<TeamKeys> {
    let extra = wrapped.len().wrapping_sub(WRAPPED_LEN);
    if wrapped.len() < WRAPPED_LEN
        || !extra.is_multiple_of(32)
        || extra / 32 > MAX_EARLIER
        || wrapped[0] != VERSION
    {
        return Err(VaultError::Decrypt);
    }
    let mut ephemeral_public = [0u8; 32];
    ephemeral_public.copy_from_slice(&wrapped[1..33]);
    let public = member.public();
    let key = box_key(
        &member.secret,
        &ephemeral_public,
        &ephemeral_public,
        &public,
    )?;
    let plain = Zeroizing::new(
        XChaCha20Poly1305::new(key.as_ref().into())
            .decrypt(
                XNonce::from_slice(&wrapped[33..57]),
                Payload {
                    msg: &wrapped[57..],
                    aad: &wrap_aad(context, &public, &ephemeral_public),
                },
            )
            .map_err(|_| VaultError::Decrypt)?,
    );
    let mut keys = plain.as_chunks::<32>().0.iter().map(|chunk| {
        let mut key = Zeroizing::new([0u8; 32]);
        key.copy_from_slice(chunk);
        key
    });
    let key = keys.next().ok_or(VaultError::Decrypt)?;
    Ok(TeamKeys {
        key,
        earlier: keys.collect(),
    })
}

/// The key of one sealed box: the shared secret, bound to both public keys.
///
/// A shared secret of all zeroes means the other side sent a point of small
/// order — a key that gives every sender the same secret. Refused.
fn box_key(
    secret: &[u8; 32],
    their_public: &[u8; 32],
    ephemeral_public: &[u8; 32],
    recipient_public: &[u8; 32],
) -> Result<Zeroizing<[u8; 32]>> {
    let shared = Zeroizing::new(
        MontgomeryPoint(*their_public)
            .mul_clamped(*secret)
            .to_bytes(),
    );
    if shared.iter().fold(0u8, |acc, byte| acc | byte) == 0 {
        return Err(VaultError::Decrypt);
    }
    let info = [WRAP_LABEL, &ephemeral_public[..], &recipient_public[..]].concat();
    let mut key = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(None, shared.as_ref())
        .expand(&info, key.as_mut())
        .map_err(|e| VaultError::Kdf(e.to_string()))?;
    Ok(key)
}

fn wrap_aad(context: WrapContext, recipient: &[u8; 32], ephemeral_public: &[u8; 32]) -> Vec<u8> {
    let mut aad = Vec::with_capacity(WRAP_LABEL.len() + 16 + 8 + 16 + 32 + 32);
    aad.extend_from_slice(WRAP_LABEL);
    aad.extend_from_slice(context.team.as_bytes());
    aad.extend_from_slice(&context.epoch.to_be_bytes());
    aad.extend_from_slice(context.account.as_bytes());
    aad.extend_from_slice(recipient);
    aad.extend_from_slice(ephemeral_public);
    aad
}

/// What two people compare: the first 16 bytes of a SHA-256 over the public
/// key, as eight groups of four hex digits.
pub fn fingerprint(public: &[u8; 32]) -> String {
    let digest = Sha256::new()
        .chain_update(FINGERPRINT_LABEL)
        .chain_update(public)
        .finalize();
    let mut out = String::with_capacity(8 * 5);
    for (index, pair) in digest[..16].chunks(2).enumerate() {
        if index > 0 {
            out.push(' ');
        }
        for byte in pair {
            out.push_str(&format!("{byte:02X}"));
        }
    }
    out
}

/// Whether two fingerprints name the same key, whatever the spacing or case
/// they were typed or passed in.
pub fn same_fingerprint(a: &str, b: &str) -> bool {
    let norm = |s: &str| -> Vec<u8> {
        s.bytes()
            .filter(u8::is_ascii_alphanumeric)
            .map(|b| b.to_ascii_uppercase())
            .collect()
    };
    let (a, b) = (norm(a), norm(b));
    a.len() == 32 && a == b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    fn hex32(text: &str) -> [u8; 32] {
        hex(text).try_into().unwrap()
    }

    fn to_hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn vault(id: u128, key: u8) -> UnlockedVault {
        UnlockedVault::from_key(Uuid::from_u128(id), Zeroizing::new([key; 32]))
    }

    fn context() -> WrapContext {
        WrapContext {
            team: Uuid::from_u128(0x7ea0),
            epoch: 3,
            account: Uuid::from_u128(0xacc0),
        }
    }

    /// RFC 7748, section 6.1: the public keys and the shared secret.
    #[test]
    fn x25519_matches_the_rfc() {
        let alice = hex32("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a");
        let bob = hex32("5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb");
        let alice_public = MemberKey::from_secret(Zeroizing::new(alice)).public();
        let bob_public = MemberKey::from_secret(Zeroizing::new(bob)).public();
        assert_eq!(
            to_hex(&alice_public),
            "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a"
        );
        assert_eq!(
            to_hex(&bob_public),
            "de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f"
        );
        let shared = MontgomeryPoint(bob_public).mul_clamped(alice).to_bytes();
        assert_eq!(
            to_hex(&shared),
            "4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742"
        );
    }

    /// A fixed wrap, so a change to the format or to what it binds shows.
    #[test]
    fn a_wrapped_key_has_this_form() {
        let member = MemberKey::from_secret(Zeroizing::new([0x11; 32]));
        let wrapped = wrap_with(
            &[0x42; 32],
            context(),
            &member.public(),
            &[0x22; 32],
            &[0x33; 24],
        )
        .unwrap();
        assert_eq!(wrapped.len(), WRAPPED_LEN);
        assert_eq!(to_hex(&wrapped), WRAP_VECTOR);
        let keys = unwrap_team_key(&member, context(), &hex(WRAP_VECTOR)).unwrap();
        assert_eq!(*keys.key, [0x42; 32]);
        assert!(keys.earlier.is_empty());
    }

    const WRAP_VECTOR: &str = "010faa684ed28867b97f4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f20\
        333333333333333333333333333333333333333333333333\
        c7571020eb5f8db73bd72e90d2e0e3f5f253052b40decb3c4744b283a9302c20\
        e5cd71b13ff5686fab1b3b1413cf90d1";

    #[test]
    fn a_fingerprint_has_this_form() {
        let member = MemberKey::from_secret(Zeroizing::new([0x11; 32]));
        let print = fingerprint(&member.public());
        assert_eq!(print, FINGERPRINT_VECTOR);
        assert_eq!(print.split(' ').count(), 8);
        assert!(same_fingerprint(
            &print,
            &print.to_lowercase().replace(' ', "-")
        ));
        assert!(!same_fingerprint(&print, &fingerprint(&[9u8; 32])));
        assert!(!same_fingerprint("", ""));
    }

    const FINGERPRINT_VECTOR: &str = "1BE5 76C2 9378 27DA 4450 CA59 5253 5232";

    #[test]
    fn a_wrap_opens_only_for_what_it_names() {
        let member = MemberKey::generate();
        let team_key = new_team_key();
        let wrapped = wrap_team_key(&team_key, &[], context(), &member.public()).unwrap();
        assert_eq!(
            *unwrap_team_key(&member, context(), &wrapped).unwrap().key,
            *team_key
        );

        let other_team = WrapContext {
            team: Uuid::from_u128(1),
            ..context()
        };
        let other_epoch = WrapContext {
            epoch: 2,
            ..context()
        };
        let other_account = WrapContext {
            account: Uuid::from_u128(1),
            ..context()
        };
        for wrong in [other_team, other_epoch, other_account] {
            assert!(unwrap_team_key(&member, wrong, &wrapped).is_err());
        }
        assert!(unwrap_team_key(&MemberKey::generate(), context(), &wrapped).is_err());
    }

    #[test]
    fn a_changed_wrap_does_not_open() {
        let member = MemberKey::generate();
        let wrapped = wrap_team_key(&[5u8; 32], &[], context(), &member.public()).unwrap();
        for index in 0..wrapped.len() {
            let mut bad = wrapped.clone();
            bad[index] ^= 1;
            assert!(
                unwrap_team_key(&member, context(), &bad).is_err(),
                "byte {index}"
            );
        }
        assert!(unwrap_team_key(&member, context(), &wrapped[..wrapped.len() - 1]).is_err());
        assert!(unwrap_team_key(&member, context(), &[]).is_err());
        let mut long = wrapped.clone();
        long.push(0);
        assert!(unwrap_team_key(&member, context(), &long).is_err());
    }

    #[test]
    fn a_key_of_small_order_is_refused() {
        let zero = [0u8; 32];
        assert!(wrap_team_key(&[5u8; 32], &[], context(), &zero).is_err());
        let member = MemberKey::generate();
        let mut wrapped = wrap_team_key(&[5u8; 32], &[], context(), &member.public()).unwrap();
        wrapped[1..33].copy_from_slice(&zero);
        assert!(unwrap_team_key(&member, context(), &wrapped).is_err());
    }

    #[test]
    fn a_new_key_is_taken_only_when_it_comes_after_the_one_held() {
        let member = MemberKey::generate();
        let key = |byte: u8| Zeroizing::new([byte; 32]);
        // Epoch 5, carrying the keys of 4, 3 and 2.
        let earlier = [key(4), key(3), key(2)];
        let wrapped = wrap_team_key(&[5; 32], &earlier, context(), &member.public()).unwrap();
        assert_eq!(wrapped.len(), WRAPPED_LEN + 3 * 32);
        let keys = unwrap_team_key(&member, context(), &wrapped).unwrap();
        assert_eq!(keys.earlier.len(), 3);

        assert!(keys.follows(5, 5, &[5; 32]));
        assert!(keys.follows(5, 4, &[4; 32]));
        assert!(keys.follows(5, 2, &[2; 32]));
        // Another key at the same epoch, a key that was never the team's,
        // one from further back than the wrap reaches, and an epoch that
        // went backwards.
        assert!(!keys.follows(5, 5, &[6; 32]));
        assert!(!keys.follows(5, 4, &[9; 32]));
        assert!(!keys.follows(5, 3, &[4; 32]));
        assert!(!keys.follows(5, 1, &[1; 32]));
        assert!(!keys.follows(5, 6, &[5; 32]));

        let on = keys.handed_on();
        assert_eq!(on.len(), 4);
        assert_eq!((*on[0], *on[3]), ([5; 32], [2; 32]));
        // No more than a wrap holds are handed on or taken.
        let many: Vec<_> = (0..12).map(key).collect();
        let wrapped = wrap_team_key(&[5; 32], &many, context(), &member.public()).unwrap();
        let keys = unwrap_team_key(&member, context(), &wrapped).unwrap();
        assert_eq!(keys.earlier.len(), MAX_EARLIER);
        assert_eq!(keys.handed_on().len(), MAX_EARLIER);
        let mut long = wrapped.clone();
        long.extend_from_slice(&[0; 32]);
        assert!(unwrap_team_key(&member, context(), &long).is_err());
        assert!(!format!("{keys:?}").contains("5, 5"));
    }

    #[test]
    fn two_wraps_of_one_key_differ() {
        let member = MemberKey::generate();
        let a = wrap_team_key(&[5u8; 32], &[], context(), &member.public()).unwrap();
        let b = wrap_team_key(&[5u8; 32], &[], context(), &member.public()).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn a_member_key_opens_only_in_its_vault_and_account() {
        let member = MemberKey::generate();
        let account = Uuid::from_u128(7);
        let sealed = member.seal(&vault(1, 3), account).unwrap();
        assert_eq!(sealed.len(), SEALED_MEMBER_LEN);
        let back = MemberKey::open(&vault(1, 3), account, &sealed).unwrap();
        assert_eq!(back.public(), member.public());

        assert!(MemberKey::open(&vault(2, 3), account, &sealed).is_err());
        assert!(MemberKey::open(&vault(1, 4), account, &sealed).is_err());
        assert!(MemberKey::open(&vault(1, 3), Uuid::from_u128(8), &sealed).is_err());
        for index in 0..sealed.len() {
            let mut bad = sealed.clone();
            bad[index] ^= 1;
            assert!(MemberKey::open(&vault(1, 3), account, &bad).is_err());
        }
        assert!(MemberKey::open(&vault(1, 3), account, &[]).is_err());
    }

    #[test]
    fn a_member_key_does_not_print() {
        let text = format!("{:?}", MemberKey::from_secret(Zeroizing::new([0x11; 32])));
        assert!(!text.contains("11"));
    }
}
