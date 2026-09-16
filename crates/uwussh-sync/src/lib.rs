//! Offline-first sync.
//!
//! The rule that shapes this crate: **local writes never wait for the network**.
//! Everything lands in the store first and goes into an [`outbox`]; the network
//! is a background job that drains it. If the server is down, or you are on a
//! train, nothing about using the app changes.

pub mod merge;
pub mod outbox;

pub use merge::{resolve, Resolution};
pub use outbox::{Outbox, OutboxEntry};

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("schema {found} is newer than this client understands ({known})")]
    SchemaTooNew { found: u32, known: u32 },
    #[error("gave up after {0} conflict retries")]
    ConflictLoop(u32),
}

/// How many times a push retries against a conflicting server state before the
/// UI stops pretending it can fix itself and shows a banner.
pub const MAX_CONFLICT_RETRIES: u32 = 3;
