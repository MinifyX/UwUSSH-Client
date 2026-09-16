//! Pending local writes, waiting for a server that may not be there.
//!
//! Backed by SQLite in the real app (M2); in-memory here so the merge logic is
//! testable without a database. The important property either way: a record
//! appears **once**. Editing the same host five times offline must produce one
//! push, not five, or a week of plane trips turns into a stampede on landing.

use parking_lot::Mutex;
use std::collections::HashMap;
use uuid::Uuid;
use uwussh_proto::Envelope;

#[derive(Debug, Clone)]
pub struct OutboxEntry {
    pub envelope: Envelope,
    pub attempts: u32,
}

#[derive(Default)]
pub struct Outbox {
    pending: Mutex<HashMap<Uuid, OutboxEntry>>,
}

impl Outbox {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a write, replacing any earlier unsent version of the same record.
    pub fn enqueue(&self, envelope: Envelope) {
        let id = envelope.id;
        let mut pending = self.pending.lock();
        let attempts = pending.get(&id).map_or(0, |e| e.attempts);
        pending.insert(id, OutboxEntry { envelope, attempts });
    }

    /// Everything waiting, oldest edit first, so the server sees changes in
    /// roughly the order they happened.
    pub fn drain_batch(&self, limit: usize) -> Vec<Envelope> {
        let pending = self.pending.lock();
        let mut entries: Vec<_> = pending.values().cloned().collect();
        entries.sort_by_key(|e| e.envelope.updated_at);
        entries
            .into_iter()
            .take(limit)
            .map(|e| e.envelope)
            .collect()
    }

    /// Drop a record after the server accepted it.
    pub fn acknowledge(&self, id: Uuid) {
        self.pending.lock().remove(&id);
    }

    /// Note a failed attempt, so a record that keeps bouncing can be surfaced
    /// instead of retried forever in silence.
    pub fn record_failure(&self, id: Uuid) -> u32 {
        let mut pending = self.pending.lock();
        match pending.get_mut(&id) {
            Some(entry) => {
                entry.attempts += 1;
                entry.attempts
            }
            None => 0,
        }
    }

    pub fn len(&self) -> usize {
        self.pending.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uwussh_proto::{EntityKind, Hlc};

    fn env(id: u128, wall: u64) -> Envelope {
        Envelope {
            id: Uuid::from_u128(id),
            vault_id: Uuid::from_u128(9),
            kind: EntityKind::Host,
            updated_at: Hlc::new(wall, 0, 1),
            base_rev: 1,
            deleted: false,
            nonce: vec![0; 24],
            blob: vec![],
            seq: None,
        }
    }

    #[test]
    fn repeated_edits_collapse_into_one_push() {
        let outbox = Outbox::new();
        for wall in [100, 200, 300, 400, 500] {
            outbox.enqueue(env(1, wall));
        }
        assert_eq!(outbox.len(), 1);

        let batch = outbox.drain_batch(10);
        assert_eq!(batch.len(), 1);
        assert_eq!(
            batch[0].updated_at.wall_ms, 500,
            "the newest version is the one that ships"
        );
    }

    #[test]
    fn a_batch_is_ordered_oldest_first() {
        let outbox = Outbox::new();
        outbox.enqueue(env(3, 300));
        outbox.enqueue(env(1, 100));
        outbox.enqueue(env(2, 200));

        let walls: Vec<u64> = outbox
            .drain_batch(10)
            .iter()
            .map(|e| e.updated_at.wall_ms)
            .collect();
        assert_eq!(walls, vec![100, 200, 300]);
    }

    #[test]
    fn acknowledging_clears_the_entry() {
        let outbox = Outbox::new();
        outbox.enqueue(env(1, 100));
        outbox.acknowledge(Uuid::from_u128(1));
        assert!(outbox.is_empty());
    }

    #[test]
    fn failures_accumulate_across_re_enqueues() {
        let outbox = Outbox::new();
        outbox.enqueue(env(1, 100));
        assert_eq!(outbox.record_failure(Uuid::from_u128(1)), 1);
        assert_eq!(outbox.record_failure(Uuid::from_u128(1)), 2);

        // A newer edit of the same record must not reset the failure count, or
        // a permanently rejected record would retry forever.
        outbox.enqueue(env(1, 200));
        assert_eq!(outbox.record_failure(Uuid::from_u128(1)), 3);
    }
}
