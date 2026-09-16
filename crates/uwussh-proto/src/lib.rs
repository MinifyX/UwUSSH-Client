//! Types shared by the UwUSSH client and the sync server.
//!
//! This crate is the single definition of what a host, an identity or a key
//! *is*. The server depends on it as a git dependency, so a schema change is
//! one edit in one place instead of two that drift apart.
//!
//! Nothing in here touches the network, the database or the filesystem — it is
//! pure data, so both sides can depend on it without inheriting each other's
//! runtime.

pub mod clock;
pub mod entities;
pub mod sync;

pub use clock::Hlc;
pub use entities::*;
pub use sync::{Envelope, SyncCursor};

/// Bumped whenever the wire format changes in a way older peers cannot read.
/// The server rejects envelopes carrying a schema it does not know.
pub const SCHEMA_VERSION: u32 = 1;
