//! What UwUSSH stores and syncs.
//!
//! One record is an [`Envelope`](crate::Envelope) — id, kind, clock, tombstone
//! — plus a sealed payload. The payloads live here, and they hold *only* the
//! fields that mean something on another device: no local connection times, no
//! key file paths, no detected system. The envelope carries everything else, so
//! the sync engine can move records around without knowing whether a blob is a
//! host, a key or a snippet.
//!
//! Every payload keeps the fields it did not recognise in [`Extra`]. A device
//! running an older build can therefore edit a host a newer one wrote without
//! dropping the fields it has never heard of.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Fields a payload did not recognise, kept verbatim so they survive an edit
/// on a device whose build is older than the one that wrote them.
pub type Extra = serde_json::Map<String, serde_json::Value>;

fn extra_is_empty(extra: &Extra) -> bool {
    extra.is_empty()
}

/// What kind of record a blob holds. Part of the associated data when
/// encrypting, so a malicious server cannot hand back a key where a snippet was
/// expected.
///
/// **Append only.** The discriminant is what goes into the associated data;
/// reordering would make every sealed record unreadable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
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
    /// A password, private key or passphrase, which records point at by id.
    /// Its payload is the secret itself, not JSON.
    Secret,
}

impl EntityKind {
    /// Kinds a record can point at, before the kinds that point at them. A
    /// batch applied in this order needs the fewest placeholder rows.
    pub const APPLY_ORDER: [Self; 7] = [
        Self::Secret,
        Self::Key,
        Self::Identity,
        Self::Group,
        Self::Host,
        Self::Snippet,
        Self::KnownHost,
    ];

    /// Where this kind sorts when a batch is applied. Unknown-to-us kinds go
    /// last; they are stored and passed on, not understood.
    pub fn apply_rank(self) -> usize {
        Self::APPLY_ORDER
            .iter()
            .position(|kind| *kind == self)
            .unwrap_or(Self::APPLY_ORDER.len())
    }
}

/// A host, as another device needs it. `workspace` is a string rather than an
/// enum on purpose: a build that meets a workspace it does not know shows the
/// host in the private one instead of refusing the whole record.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostPayload {
    pub name: String,
    pub address: String,
    pub port: u16,
    pub workspace: String,
    /// The order the user dragged the host into, within its group.
    pub position: i64,
    pub group_id: Option<Uuid>,
    pub identity_id: Option<Uuid>,
    #[serde(flatten, default, skip_serializing_if = "extra_is_empty")]
    pub extra: Extra,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupPayload {
    pub workspace: String,
    pub name: String,
    pub position: i64,
    #[serde(flatten, default, skip_serializing_if = "extra_is_empty")]
    pub extra: Extra,
}

/// Username plus how to authenticate. Kept separate from [`HostPayload`] on
/// purpose: one key serves forty hosts, and rotating it is one edit rather than
/// forty.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityPayload {
    pub label: String,
    pub username: String,
    /// `password`, `key`, `agent`, `keyboard-interactive` or `cert`.
    pub auth_type: String,
    pub key_id: Option<Uuid>,
    /// Points at a [`EntityKind::Secret`] record. Never the password itself.
    pub password_secret_id: Option<Uuid>,
    #[serde(flatten, default, skip_serializing_if = "extra_is_empty")]
    pub extra: Extra,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyPayload {
    pub label: String,
    /// `ssh-ed25519`, `ssh-rsa`, … or what an importer called it.
    pub key_type: String,
    /// The public half, in the clear: a host form shows which key it uses
    /// without unlocking anything.
    pub public_key: String,
    pub private_secret_id: Option<Uuid>,
    pub passphrase_secret_id: Option<Uuid>,
    #[serde(flatten, default, skip_serializing_if = "extra_is_empty")]
    pub extra: Extra,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnippetPayload {
    pub label: String,
    pub body: String,
    pub group_path: Option<String>,
    #[serde(flatten, default, skip_serializing_if = "extra_is_empty")]
    pub extra: Extra,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownHostPayload {
    pub address: String,
    pub port: u16,
    pub algorithm: String,
    /// `SHA256:…`, as `ssh-keygen -lf` prints it.
    pub fingerprint_sha256: String,
    pub public_key: String,
    pub first_seen_ms: u64,
    #[serde(flatten, default, skip_serializing_if = "extra_is_empty")]
    pub extra: Extra,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_an_older_build_does_not_know_survives_a_round_trip() {
        // What a newer build wrote: a host with tags.
        let written = r#"{"name":"prox-1","address":"10.0.0.12","port":22,
            "workspace":"private","position":0,"group_id":null,"identity_id":null,
            "tags":["homelab"]}"#;

        let host: HostPayload = serde_json::from_str(written).expect("parse");
        assert_eq!(host.name, "prox-1");
        assert!(host.extra.contains_key("tags"));

        // The older build edits the name and writes it back.
        let edited = HostPayload {
            name: "prox-one".into(),
            ..host
        };
        let json = serde_json::to_value(&edited).expect("serialise");
        assert_eq!(json["name"], "prox-one");
        assert_eq!(json["tags"][0], "homelab", "the tags must still be there");
    }

    #[test]
    fn nothing_extra_shows_up_when_there_is_nothing_extra() {
        let json = serde_json::to_string(&GroupPayload {
            workspace: "private".into(),
            name: "Homelab".into(),
            position: 0,
            extra: Extra::new(),
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"workspace":"private","name":"Homelab","position":0}"#
        );
    }

    #[test]
    fn records_are_applied_before_the_records_that_point_at_them() {
        assert!(EntityKind::Secret.apply_rank() < EntityKind::Key.apply_rank());
        assert!(EntityKind::Key.apply_rank() < EntityKind::Identity.apply_rank());
        assert!(EntityKind::Identity.apply_rank() < EntityKind::Host.apply_rank());
        assert!(EntityKind::Group.apply_rank() < EntityKind::Host.apply_rank());
        assert_eq!(
            EntityKind::PortForward.apply_rank(),
            EntityKind::APPLY_ORDER.len(),
            "a kind with no table yet sorts last"
        );
    }

    #[test]
    fn the_discriminants_never_move() {
        // They go into the associated data of every sealed record, so a
        // reordering here would make existing vaults unreadable.
        assert_eq!(EntityKind::Host as u8, 0);
        assert_eq!(EntityKind::Group as u8, 1);
        assert_eq!(EntityKind::Identity as u8, 2);
        assert_eq!(EntityKind::Key as u8, 3);
        assert_eq!(EntityKind::Snippet as u8, 4);
        assert_eq!(EntityKind::PortForward as u8, 5);
        assert_eq!(EntityKind::KnownHost as u8, 6);
        assert_eq!(EntityKind::TerminalProfile as u8, 7);
        assert_eq!(EntityKind::Secret as u8, 8);
    }
}
