//! What actually travels between client and server.
//!
//! The server only ever sees an [`Envelope`]: an id, a sequence number, a kind
//! hint and an opaque blob. It cannot read the blob, so everything it does —
//! ordering, paging, conflict detection — has to work on the header alone.
//! That constraint is deliberate, and it is why the cursor is a server sequence
//! number rather than a timestamp the client could lie about.

use crate::clock::Hlc;
use crate::entities::EntityKind;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One encrypted record in transit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub id: Uuid,
    pub vault_id: Uuid,
    pub kind: EntityKind,
    pub updated_at: Hlc,
    /// The revision this edit was based on. The server compares it to what it
    /// holds and answers 409 if someone else got there first.
    pub base_rev: u64,
    #[serde(default)]
    pub deleted: bool,
    /// XChaCha20-Poly1305 nonce, 24 bytes.
    #[serde(with = "serde_bytes_vec")]
    pub nonce: Vec<u8>,
    /// Ciphertext plus tag. AAD is `id || kind || vault_id`, which is what
    /// stops a hostile server from swapping blobs between records.
    #[serde(with = "serde_bytes_vec")]
    pub blob: Vec<u8>,
    /// Assigned by the server on write; `None` on the way up.
    pub seq: Option<u64>,
}

/// Where a device left off. Monotonic and server-assigned, so resuming a sync
/// never depends on clocks agreeing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncCursor(pub u64);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullResponse {
    pub envelopes: Vec<Envelope>,
    pub cursor: SyncCursor,
    /// True when the server has more waiting — page again rather than assume
    /// one round trip was enough.
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushRequest {
    pub schema: u32,
    pub envelopes: Vec<Envelope>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushResponse {
    pub cursor: SyncCursor,
    /// Records the server refused because someone else wrote first. The client
    /// merges field-wise and tries again.
    pub conflicts: Vec<Envelope>,
}

/// Base64 for byte vectors, so envelopes stay readable in a JSON body.
mod serde_bytes_vec {
    use serde::{Deserialize, Deserializer, Serializer};

    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
            out.push(ALPHABET[(n >> 18 & 63) as usize] as char);
            out.push(ALPHABET[(n >> 12 & 63) as usize] as char);
            out.push(if chunk.len() > 1 {
                ALPHABET[(n >> 6 & 63) as usize] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                ALPHABET[(n & 63) as usize] as char
            } else {
                '='
            });
        }
        s.serialize_str(&out)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        let mut out = Vec::with_capacity(text.len() / 4 * 3);
        let mut acc = 0u32;
        let mut bits = 0u32;
        for c in text.bytes() {
            if c == b'=' {
                break;
            }
            let Some(v) = ALPHABET.iter().position(|&a| a == c) else {
                continue;
            };
            acc = acc << 6 | v as u32;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((acc >> bits) as u8);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelopes_round_trip_through_json() {
        let env = Envelope {
            id: Uuid::nil(),
            vault_id: Uuid::nil(),
            kind: EntityKind::Host,
            updated_at: Hlc::new(1_700_000_000_000, 0, 1),
            base_rev: 3,
            deleted: false,
            nonce: vec![1, 2, 3, 4, 5],
            blob: vec![9, 8, 7, 6, 5, 4, 3, 2, 1, 0],
            seq: Some(42),
        };

        let json = serde_json::to_string(&env).expect("serialise");
        let back: Envelope = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(env, back);
    }
}
