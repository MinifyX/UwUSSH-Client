//! The things UwUSSH stores and syncs.
//!
//! Every syncable entity carries the same [`SyncHeader`], which is what lets
//! the sync engine stay generic: it moves records around without knowing or
//! caring whether a blob is a host, a key or a snippet.

use crate::clock::Hlc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The header every syncable record shares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncHeader {
    pub id: Uuid,
    pub vault_id: Uuid,
    pub updated_at: Hlc,
    /// Local revision counter, sent back as `base_rev` so the server can spot
    /// a conflicting concurrent write.
    pub rev: u64,
    /// Tombstone. Records are never hard-deleted on sync, or a device that was
    /// offline during the delete would happily resurrect them.
    #[serde(default)]
    pub deleted: bool,
}

/// What kind of record a blob holds. Part of the AAD when encrypting, so a
/// malicious server cannot hand back a key where a snippet was expected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Host,
    Group,
    Identity,
    Key,
    Snippet,
    PortForward,
    KnownHost,
    TerminalProfile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthType {
    Password,
    Key,
    Agent,
    KeyboardInteractive,
    Cert,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KeyType {
    Ed25519,
    Rsa,
    Ecdsa,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForwardKind {
    Local,
    Remote,
    Dynamic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    #[serde(flatten)]
    pub header: SyncHeader,
    pub name: String,
    pub address: String,
    pub port: u16,
    pub group_id: Option<Uuid>,
    pub identity_id: Option<Uuid>,
    /// Self-reference, which is what makes ProxyJump chains free:
    /// `laptop → bastion → db-01` is just a linked list.
    pub jump_host_id: Option<Uuid>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub color: Option<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    pub keepalive_secs: Option<u32>,
    /// Off unless explicitly enabled per host. A compromised host with a
    /// forwarded agent is a lateral movement vector, so this is never global.
    #[serde(default)]
    pub agent_forward: bool,
    pub startup_snippet_id: Option<Uuid>,
    pub terminal_profile_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    #[serde(flatten)]
    pub header: SyncHeader,
    pub name: String,
    pub parent_id: Option<Uuid>,
    pub icon: Option<String>,
    pub sort: i32,
}

/// Username plus how to authenticate. Kept separate from [`Host`] on purpose:
/// one key serves forty hosts, and rotating it is one edit rather than forty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    #[serde(flatten)]
    pub header: SyncHeader,
    pub label: String,
    pub username: String,
    pub auth_type: AuthType,
    /// Points at a secret inside the vault. Never the secret itself — this
    /// struct gets serialised in places a plaintext password must not reach.
    pub secret_ref: Option<Uuid>,
    pub key_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Key {
    #[serde(flatten)]
    pub header: SyncHeader,
    pub label: String,
    pub key_type: KeyType,
    pub public: String,
    /// Reference to the encrypted private key in the vault.
    pub private_ref: Uuid,
    pub passphrase_ref: Option<Uuid>,
    pub certificate: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snippet {
    #[serde(flatten)]
    pub header: SyncHeader,
    pub label: String,
    pub body: String,
    #[serde(default)]
    pub target_tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortForward {
    #[serde(flatten)]
    pub header: SyncHeader,
    pub host_id: Uuid,
    pub kind: ForwardKind,
    pub bind: String,
    pub target: Option<String>,
    #[serde(default)]
    pub autostart: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownHost {
    #[serde(flatten)]
    pub header: SyncHeader,
    pub hostname: String,
    pub port: u16,
    pub key_type: String,
    pub fingerprint_sha256: String,
    pub first_seen_ms: u64,
}
