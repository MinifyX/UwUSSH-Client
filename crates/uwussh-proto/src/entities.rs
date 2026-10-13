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
use std::collections::BTreeMap;
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
/// changing one would make every sealed record of that kind unreadable. They
/// are written out so that reordering the list can't do it by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum EntityKind {
    Host = 0,
    Group = 1,
    Identity = 2,
    Key = 3,
    Snippet = 4,
    PortForward = 5,
    KnownHost = 6,
    TerminalProfile = 7,
    /// A password, private key or passphrase, which records point at by id.
    /// Its payload is the secret itself, not JSON.
    Secret = 8,
    /// What one device holds: every record's id and version, so another
    /// device can tell whether the server handed it everything. See
    /// [`crate::manifest`].
    Manifest = 9,
    /// The command assistant's settings: which provider, which model, which
    /// address. One record per vault, under a fixed id. The API keys are
    /// [`EntityKind::Secret`] records it points at, never part of it.
    AssistConfig = 10,
    /// One slot of the command assistant's answer cache. There is a fixed
    /// number of slots with fixed ids, and a new answer takes the oldest one,
    /// so the cache never grows past them on the server either.
    AssistCache = 11,
    /// The user's own keyboard shortcuts for one family of keyboards. One
    /// record per family, each under a fixed id. Texts a shortcut types are
    /// [`EntityKind::Secret`] records it points at, never part of it.
    Keymap = 12,
}

impl EntityKind {
    /// Every kind, in discriminant order.
    pub const ALL: [Self; 13] = [
        Self::Host,
        Self::Group,
        Self::Identity,
        Self::Key,
        Self::Snippet,
        Self::PortForward,
        Self::KnownHost,
        Self::TerminalProfile,
        Self::Secret,
        Self::Manifest,
        Self::AssistConfig,
        Self::AssistCache,
        Self::Keymap,
    ];

    /// The kind a discriminant stands for, or `None` for one a newer build
    /// added.
    pub fn from_discriminant(value: u8) -> Option<Self> {
        Self::ALL.get(usize::from(value)).copied()
    }

    /// Kinds a record can point at, before the kinds that point at them. A
    /// batch applied in this order needs the fewest placeholder rows.
    ///
    /// Manifests are not in here on purpose: this list doubles as the list of
    /// record tables, and a manifest sorts last anyway, after the records it
    /// talks about.
    pub const APPLY_ORDER: [Self; 11] = [
        Self::Secret,
        Self::Key,
        Self::Identity,
        Self::Group,
        Self::Host,
        Self::PortForward,
        Self::Snippet,
        Self::KnownHost,
        Self::AssistConfig,
        Self::AssistCache,
        Self::Keymap,
    ];

    /// Kinds a build before 0.3 does not know: the command assistant's two
    /// and the keymap. They travel apart from the rest, in requests of their
    /// own, so a server that refuses them refuses only them (see
    /// `uwussh_sync::engine`), and a pull asks for them.
    pub const APART: [Self; 3] = [Self::AssistConfig, Self::AssistCache, Self::Keymap];

    /// Whether this kind is one of [`Self::APART`].
    pub fn travels_apart(self) -> bool {
        Self::APART.contains(&self)
    }

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

/// A tunnel of a host: `ssh -L` or `ssh -R`. `kind` is a string for the same
/// reason as a host's workspace: a kind a newer build adds (`dynamic`, say)
/// is kept and shown, not run, instead of refusing the record.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortForwardPayload {
    pub host_id: Uuid,
    pub name: String,
    /// `local` or `remote`.
    pub kind: String,
    /// Where it listens: on this computer for `local`, on the server for
    /// `remote`.
    pub bind_address: String,
    pub bind_port: u16,
    /// Where it leads: as the server sees it for `local`, as this computer
    /// sees it for `remote`.
    pub target_host: String,
    pub target_port: u16,
    /// Starts along with a terminal to its host.
    #[serde(default)]
    pub autostart: bool,
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

/// The command assistant's settings. `provider` is the one in use (`ollama`,
/// `openai-compatible`, `openai`, `anthropic`, `mistral`, or empty for off);
/// `providers` keeps what was set for each, so switching back and forth loses
/// nothing. Strings rather than enums for the same reason as
/// [`HostPayload::workspace`]: a build that meets a provider it does not know
/// shows it as off instead of refusing the record.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssistConfigPayload {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub providers: BTreeMap<String, AssistProviderPayload>,
    #[serde(flatten, default, skip_serializing_if = "extra_is_empty")]
    pub extra: Extra,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssistProviderPayload {
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub base_url: Option<String>,
    /// Points at a [`EntityKind::Secret`] record. Never the key itself.
    #[serde(default)]
    pub key_secret_id: Option<Uuid>,
    #[serde(flatten, default, skip_serializing_if = "extra_is_empty")]
    pub extra: Extra,
}

/// One cached answer. A slot that was cleared has an empty `command`: a
/// tombstone would beat every later answer written into the same slot.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssistCachePayload {
    /// The system the command is for, like `linux/bash/apt`.
    #[serde(default)]
    pub platform: String,
    /// What was asked, as typed.
    #[serde(default)]
    pub request: String,
    /// What was asked, as the cache compares it.
    #[serde(default)]
    pub normalized: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub explanation: String,
    #[serde(default)]
    pub dangerous: bool,
    #[serde(default)]
    pub created_ms: u64,
    #[serde(flatten, default, skip_serializing_if = "extra_is_empty")]
    pub extra: Extra,
}

/// The user's keyboard shortcuts for one family of keyboards (`mac`, or
/// `other` for Windows and Linux — ⌘ exists on one and not the other, so the
/// two are kept apart). `bindings` maps an action's id to the chords that
/// trigger it; an action that is not in it keeps the shortcut the app ships
/// with, and one with no chords has none. `texts` are the user's own actions:
/// a text to type into the terminal, under the action id `text:<id>`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeymapPayload {
    #[serde(default)]
    pub bindings: BTreeMap<String, Vec<ChordPayload>>,
    /// By the text's id, a UUID. A string rather than a [`Uuid`] so that one
    /// entry a build cannot read does not cost the whole record.
    #[serde(default)]
    pub texts: BTreeMap<String, KeymapTextPayload>,
    #[serde(flatten, default, skip_serializing_if = "extra_is_empty")]
    pub extra: Extra,
}

/// One key with its modifiers. `code` is the physical key as the page names
/// it (`KeyC`, `F5`, `Digit1`), so a shortcut stays on the same key whatever
/// layout is active.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChordPayload {
    #[serde(default, skip_serializing_if = "is_false")]
    pub ctrl: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub shift: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub alt: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub meta: bool,
    #[serde(default)]
    pub code: String,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeymapTextPayload {
    #[serde(default)]
    pub label: String,
    /// Press Enter after typing the text.
    #[serde(default)]
    pub enter: bool,
    /// Points at a [`EntityKind::Secret`] record. Never the text itself: it
    /// may well be a password.
    #[serde(default)]
    pub secret_id: Option<Uuid>,
    /// Where the text stands in the user's list, which the map it lives in
    /// cannot say.
    #[serde(default)]
    pub position: u32,
    #[serde(flatten, default, skip_serializing_if = "extra_is_empty")]
    pub extra: Extra,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_an_older_build_does_not_know_survives_a_round_trip() {
        // What a newer build wrote: a host with tags.
        let written = r#"{"name":"prox-1","address":"192.0.2.12","port":22,
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
    fn a_tunnel_of_a_kind_this_build_does_not_run_still_reads() {
        let written = format!(
            r#"{{"host_id":"{}","name":"socks","kind":"dynamic","bind_address":"127.0.0.1",
                "bind_port":1080,"target_host":"","target_port":0,"autostart":true,
                "via":"bastion"}}"#,
            Uuid::nil()
        );
        let tunnel: PortForwardPayload = serde_json::from_str(&written).expect("parse");
        assert_eq!(tunnel.kind, "dynamic");
        assert!(tunnel.autostart);
        let json = serde_json::to_value(&tunnel).unwrap();
        assert_eq!(
            json["via"], "bastion",
            "a field from a newer build survives"
        );
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
        assert!(EntityKind::Host.apply_rank() < EntityKind::PortForward.apply_rank());
        assert_eq!(
            EntityKind::TerminalProfile.apply_rank(),
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
        assert_eq!(EntityKind::Manifest as u8, 9);
        assert_eq!(EntityKind::AssistConfig as u8, 10);
        assert_eq!(EntityKind::AssistCache as u8, 11);
        assert_eq!(EntityKind::Keymap as u8, 12);
        for (index, kind) in EntityKind::ALL.iter().enumerate() {
            assert_eq!(*kind as usize, index, "ALL is in discriminant order");
            assert_eq!(EntityKind::from_discriminant(index as u8), Some(*kind));
        }
        assert_eq!(EntityKind::from_discriminant(13), None);
    }

    #[test]
    fn the_assistant_kinds_have_the_names_the_wire_carries() {
        assert_eq!(
            serde_json::to_string(&EntityKind::AssistConfig).unwrap(),
            r#""assist_config""#
        );
        assert_eq!(
            serde_json::to_string(&EntityKind::AssistCache).unwrap(),
            r#""assist_cache""#
        );
        assert!(EntityKind::Secret.apply_rank() < EntityKind::AssistConfig.apply_rank());
        assert!(EntityKind::AssistCache.travels_apart());
        assert!(!EntityKind::Host.travels_apart());
    }

    #[test]
    fn a_keymap_has_its_wire_name_and_keeps_what_a_newer_build_added() {
        assert_eq!(
            serde_json::to_string(&EntityKind::Keymap).unwrap(),
            r#""keymap""#
        );
        assert!(EntityKind::Keymap.travels_apart());
        assert!(EntityKind::Secret.apply_rank() < EntityKind::Keymap.apply_rank());

        let id = Uuid::nil();
        let written = format!(
            r#"{{"bindings":{{"tab:new":[{{"ctrl":true,"shift":true,"code":"KeyT"}}]}},
                "texts":{{"{id}":{{"label":"sudo","enter":true,"secret_id":null,"colour":"red"}}}},
                "sequences":[]}}"#
        );
        let keymap: KeymapPayload = serde_json::from_str(&written).unwrap();
        let chord = &keymap.bindings["tab:new"][0];
        assert!(chord.ctrl && chord.shift && !chord.alt && !chord.meta);
        assert_eq!(chord.code, "KeyT");
        assert!(keymap.texts[&id.to_string()].enter);
        let json = serde_json::to_value(&keymap).unwrap();
        assert_eq!(json["sequences"], serde_json::json!([]));
        assert_eq!(json["texts"][id.to_string()]["colour"], "red");

        // A modifier that is not held is not written at all.
        assert_eq!(
            serde_json::to_string(&ChordPayload {
                meta: true,
                code: "KeyK".into(),
                ..Default::default()
            })
            .unwrap(),
            r#"{"meta":true,"code":"KeyK"}"#
        );
        let empty: KeymapPayload = serde_json::from_str("{}").unwrap();
        assert!(empty.bindings.is_empty() && empty.texts.is_empty());
    }

    #[test]
    fn assistant_settings_keep_what_a_newer_build_added() {
        let written = r#"{"provider":"anthropic","providers":{"anthropic":
            {"model":"claude-sonnet-5-5","base_url":null,"key_secret_id":null,"effort":"low"}},
            "temperature":0.2}"#;
        let config: AssistConfigPayload = serde_json::from_str(written).unwrap();
        assert_eq!(config.provider, "anthropic");
        assert!(config.extra.contains_key("temperature"));
        assert!(config.providers["anthropic"].extra.contains_key("effort"));
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["providers"]["anthropic"]["effort"], "low");
        // An empty slot is still a record, and reads back as one.
        let empty: AssistCachePayload = serde_json::from_str("{}").unwrap();
        assert!(empty.command.is_empty());
    }

    #[test]
    fn a_manifest_is_applied_after_everything_it_lists() {
        for kind in EntityKind::APPLY_ORDER {
            assert!(kind.apply_rank() < EntityKind::Manifest.apply_rank());
        }
        assert_eq!(
            serde_json::to_string(&EntityKind::Manifest).unwrap(),
            r#""manifest""#,
            "the name the server stores it under"
        );
    }
}
