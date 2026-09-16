//! Who wins when two devices edited the same record.
//!
//! Last-writer-wins, decided by the hybrid logical clock rather than wall time.
//! One asymmetry is deliberate: **a delete beats a concurrent edit**. If one
//! device removed a host while another renamed it, resurrecting the host is
//! the worse outcome — the rename is cheap to redo, an unexpectedly returning
//! host you thought you had removed is confusing and, for a bastion you
//! decommissioned, actively wrong.

use uwussh_proto::Envelope;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// Keep ours, push it.
    Local,
    /// Take theirs, write it to the store.
    Remote,
    /// Same revision of the same record — nothing to do.
    Identical,
}

pub fn resolve(local: &Envelope, remote: &Envelope) -> Resolution {
    debug_assert_eq!(
        local.id, remote.id,
        "resolve compares two versions of one record"
    );

    if local.updated_at == remote.updated_at && local.deleted == remote.deleted {
        return Resolution::Identical;
    }

    // Tombstones win over concurrent edits regardless of clock.
    match (local.deleted, remote.deleted) {
        (true, false) => return Resolution::Local,
        (false, true) => return Resolution::Remote,
        _ => {}
    }

    if local.updated_at > remote.updated_at {
        Resolution::Local
    } else {
        Resolution::Remote
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;
    use uwussh_proto::{EntityKind, Hlc};

    fn env(wall: u64, counter: u32, device: u32, deleted: bool) -> Envelope {
        Envelope {
            id: Uuid::from_u128(1),
            vault_id: Uuid::from_u128(9),
            kind: EntityKind::Host,
            updated_at: Hlc::new(wall, counter, device),
            base_rev: 1,
            deleted,
            nonce: vec![0; 24],
            blob: vec![1, 2, 3],
            seq: None,
        }
    }

    #[test]
    fn the_later_edit_wins() {
        assert_eq!(
            resolve(&env(200, 0, 1, false), &env(100, 0, 2, false)),
            Resolution::Local
        );
        assert_eq!(
            resolve(&env(100, 0, 1, false), &env(200, 0, 2, false)),
            Resolution::Remote
        );
    }

    #[test]
    fn same_millisecond_is_broken_by_the_counter() {
        assert_eq!(
            resolve(&env(100, 5, 1, false), &env(100, 2, 2, false)),
            Resolution::Local
        );
    }

    #[test]
    fn identical_versions_need_no_work() {
        assert_eq!(
            resolve(&env(100, 1, 1, false), &env(100, 1, 1, false)),
            Resolution::Identical
        );
    }

    #[test]
    fn a_delete_beats_a_newer_edit() {
        // Remote edited later, but we deleted. The host stays deleted.
        assert_eq!(
            resolve(&env(100, 0, 1, true), &env(900, 0, 2, false)),
            Resolution::Local
        );
        assert_eq!(
            resolve(&env(900, 0, 1, false), &env(100, 0, 2, true)),
            Resolution::Remote
        );
    }

    #[test]
    fn two_deletes_fall_back_to_the_clock() {
        assert_eq!(
            resolve(&env(200, 0, 1, true), &env(100, 0, 2, true)),
            Resolution::Local
        );
    }
}
