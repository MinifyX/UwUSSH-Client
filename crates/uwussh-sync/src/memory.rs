//! A sync server in memory.
//!
//! This is the whole server, minus the network, the accounts and the disk: it
//! hands out sequence numbers, keeps the newest version of each record, pages
//! through them from a cursor, and refuses a write whose `base_seq` is not the
//! version it holds. Those are exactly the rules the real server must follow,
//! which is why they live here as running code rather than only in the
//! protocol's documentation — the tests in this crate hold two devices against
//! it, and the server's own tests can hold it against them.
//!
//! It also plays the part of a server that misbehaves: [`MemoryServer::store`]
//! writes a record the way a hostile or broken server would hand it back, so
//! the client's defences can be tested.

use parking_lot::Mutex;
use std::collections::BTreeMap;
use uuid::Uuid;
use uwussh_proto::{
    Accepted, Envelope, PullResponse, PushResponse, SyncCursor, MAX_BATCH, MAX_BLOB_BYTES,
};

use crate::{Transport, TransportError};

#[derive(Default)]
struct Inner {
    /// The last sequence number handed out. Monotonic, never reused.
    seq: u64,
    /// The newest version of every record, by id. No history: a device that
    /// needs an older version has it locally or not at all.
    records: BTreeMap<Uuid, Envelope>,
    /// The vault this account holds. Set by the first record that arrives, so
    /// a device pushing another account's records is refused.
    vault_id: Option<Uuid>,
}

#[derive(Default)]
pub struct MemoryServer {
    inner: Mutex<Inner>,
}

impl MemoryServer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Everything the server holds — which is what an attacker who took the
    /// server would get. Tests read it to check that it is unreadable.
    pub fn records(&self) -> Vec<Envelope> {
        self.inner.lock().records.values().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Put a record in as the server itself, bypassing every check: the next
    /// sequence number, whatever header and blob the caller says. This is how
    /// a test plays a server that flips a tombstone flag or replays an old
    /// version.
    pub fn store(&self, mut envelope: Envelope) -> u64 {
        let mut inner = self.inner.lock();
        inner.seq += 1;
        let seq = inner.seq;
        envelope.seq = Some(seq);
        inner.records.insert(envelope.id, envelope);
        seq
    }
}

impl Transport for MemoryServer {
    fn pull(&self, since: SyncCursor, limit: usize) -> Result<PullResponse, TransportError> {
        let inner = self.inner.lock();
        let mut envelopes: Vec<Envelope> = inner
            .records
            .values()
            .filter(|env| env.seq.unwrap_or(0) > since.0)
            .cloned()
            .collect();
        envelopes.sort_by_key(|env| env.seq.unwrap_or(0));

        let limit = limit.min(MAX_BATCH);
        let has_more = envelopes.len() > limit;
        envelopes.truncate(limit);
        let cursor = envelopes
            .last()
            .and_then(|env| env.seq)
            .map(SyncCursor)
            .unwrap_or(since);
        Ok(PullResponse {
            envelopes,
            cursor,
            has_more,
        })
    }

    fn push(&self, envelopes: Vec<Envelope>) -> Result<PushResponse, TransportError> {
        if envelopes.len() > MAX_BATCH {
            return Err(TransportError::Refused(format!(
                "{} records in one request, at most {MAX_BATCH} are allowed",
                envelopes.len()
            )));
        }
        let mut inner = self.inner.lock();
        let mut response = PushResponse::default();
        for mut envelope in envelopes {
            if envelope.nonce.len() != 24 || envelope.blob.len() > MAX_BLOB_BYTES {
                return Err(TransportError::Refused(
                    "a record the protocol does not allow".into(),
                ));
            }
            match inner.vault_id {
                Some(vault) if vault != envelope.vault_id => {
                    return Err(TransportError::Refused(
                        "a record from another vault than this account's".into(),
                    ));
                }
                Some(_) => {}
                None => inner.vault_id = Some(envelope.vault_id),
            }

            // The version check is the whole of conflict detection, and it is
            // all the server can do: it cannot read a record, so it cannot
            // merge one either.
            if let Some(current) = inner.records.get(&envelope.id) {
                if current.seq.unwrap_or(0) != envelope.base_seq {
                    response.conflicts.push(current.clone());
                    continue;
                }
            }
            inner.seq += 1;
            let seq = inner.seq;
            envelope.seq = Some(seq);
            response.accepted.push(Accepted {
                id: envelope.id,
                seq,
            });
            inner.records.insert(envelope.id, envelope);
        }
        response.cursor = SyncCursor(inner.seq);
        Ok(response)
    }
}
