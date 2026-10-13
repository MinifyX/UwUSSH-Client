//! Teams: vaults several accounts share, on a UwUSync server.
//!
//! The server's side of it is in its `docs/teams.md`. This is the client's:
//!
//! - [`TeamApi`] is what the server offers, [`crate::Server`] speaks it over
//!   HTTP, and the tests have the same rules without a network.
//! - [`Teams`] is everything a device does with it: the pass that runs with
//!   each sync (which teams there are, their keys, their records), and what
//!   a person asks for — create, invite, join, confirm, remove.
//!
//! A team is a space of its own on the device (see `uwussh_store::spaces`),
//! under the server it lives on. Its key reaches this account wrapped to the
//! account's member key; what a wrap is and why it carries the marks of the
//! keys before it is in `uwussh_vault::team`.
//!
//! What an account knows about a team, it keeps where only it can write: in
//! its own vault on that server, synced among its devices (see
//! `uwussh_store::team`). Not in the team's records, which everybody with
//! the team key writes and the server hands out as it likes.
//!
//! What this side holds the server to:
//!
//! - **A newcomer gets the key for the public key somebody compared.** A
//!   confirmation names a fingerprint; the key the server lists has to have
//!   it, or nothing is wrapped.
//! - **A member's key does not change under this account.** The key an
//!   account confirmed a member with, or first saw them with, is written
//!   down. Listed with another, or listed again after this account took
//!   them out, the member stops this account's rotation instead of
//!   receiving the key, until somebody compared fingerprints again.
//! - **A new key has to come after the one this account had.** The epoch
//!   and the mark of the key an account took for a team are written down
//!   too. A wrap that does not carry that mark, an epoch that went
//!   backwards, another key under the same epoch: the team is left as it is
//!   and not synced. That holds on a device that never had the team, and
//!   for a team that was gone from the list in between.
//! - **A team goes from the device when the server says so about the team
//!   itself,** not when a list comes without it.
//!
//! What it does not: a member this account has never seen is taken with the
//! key the server lists for them. Somebody else may have confirmed them;
//! this account cannot tell. And a new key proves that its maker had the
//! one before, which a former member does (see `uwussh_vault::team`).

use crate::engine::{sync_once, SyncError, Transport, TransportError};
use crate::http::{decode_base64, encode_base64, read_limited, Server, MAX_PAGE_BYTES};
use reqwest::blocking::RequestBuilder;
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::collections::HashSet;
use uuid::Uuid;
use uwussh_proto::{Envelope, PullResponse, PushResponse, SyncCursor, MAX_BATCH, SCHEMA_VERSION};
use uwussh_store::{
    MemberPin, Rekey, SpaceHandle, Spaces, Store, StoreError, TeamKeyRecord, TeamRole, VaultStatus,
    MAX_TEAM_NAME,
};
use uwussh_vault::team::{fingerprint, key_mark, new_team_key, same_fingerprint, unwrap_team_key};
use uwussh_vault::team::{wrap_team_key, TeamKeys};
use uwussh_vault::{MemberKey, WrapContext};

/// As many teams as one answer may list. A server allows far fewer.
const MAX_TEAMS: usize = 256;
/// As many members as one answer may list: the server's own bound.
const MAX_MEMBERS: usize = 1_000;
/// The server's bound on a display name, in characters.
pub const MAX_DISPLAY_NAME: usize = 60;
const MAX_WRAPPED: usize = 512;
const MAX_SEALED_PRIVATE: usize = 1_024;
/// The longest answer of a team route that is not a page of records.
const MAX_TEAM_ANSWER: u64 = 1 << 20;
/// How often a rotation that somebody got in the way of is tried again.
const ROTATE_TRIES: usize = 3;
/// Why a key the server hands out is turned down.
const NOT_AFTER: &str = "a team key that does not come after the one held";
/// Why a team's space stays shut.
const NOT_OPENING: &str = "a team's space that the key handed out does not open";

// ── What goes wrong ─────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum TeamError {
    /// A server from before teams, one with teams switched off, or a
    /// UwULock server.
    #[error("this server has no teams")]
    Unsupported,
    #[error("teams need the vault unlocked")]
    VaultLocked,
    #[error("the server could not be reached: {0}")]
    Offline(String),
    /// The server said no. `code` is its word for why.
    #[error("{message}")]
    Refused {
        code: String,
        message: String,
        /// With `stale-epoch`: the team's current one.
        epoch: Option<u64>,
    },
    /// What the server sent does not hold up against what this device
    /// knows. Never carries anything the server sent.
    #[error("the server's answer was not accepted: {0}")]
    Untrusted(&'static str),
    /// A member is listed with another key than the one this account wrote
    /// down for them, or is listed again after this account took them out.
    /// Nothing is wrapped for them until their fingerprint was compared
    /// again. `name` is what the server calls them.
    #[error("identity-changed: {name} has to be confirmed again")]
    MemberChanged {
        account: Uuid,
        name: String,
        /// Listed again after being taken out, rather than with a new key.
        returned: bool,
    },
    #[error(transparent)]
    Store(StoreError),
    #[error(transparent)]
    Sync(SyncError),
}

impl TeamError {
    fn refused(code: &str, message: &str) -> Self {
        Self::Refused {
            code: code.into(),
            message: message.into(),
            epoch: None,
        }
    }

    /// The server's error code, when it was the server that refused.
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Refused { code, .. } => Some(code),
            Self::MemberChanged { .. } => Some("identity-changed"),
            _ => None,
        }
    }

    /// What the interface acts on: one of a handful of words, whatever the
    /// server's own was.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::VaultLocked => "vault-locked",
            Self::Offline(_) => "offline",
            Self::Refused { code, .. } => match code.as_str() {
                "team-limit" | "team-full" | "too-many-invites" => "limit",
                "invalid-invite" => "invalid-code",
                "last-owner" => "last-owner",
                "forbidden" | "team-creation-closed" | "membership-pending" => "not-allowed",
                "teams-disabled" => "unsupported",
                _ => "error",
            },
            Self::Untrusted(_) | Self::MemberChanged { .. } | Self::Store(_) | Self::Sync(_) => {
                "error"
            }
        }
    }

    fn is(&self, wanted: &str) -> bool {
        self.code() == Some(wanted)
    }
}

impl From<StoreError> for TeamError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::VaultLocked => Self::VaultLocked,
            other => Self::Store(other),
        }
    }
}

impl From<uwussh_vault::VaultError> for TeamError {
    fn from(_: uwussh_vault::VaultError) -> Self {
        Self::Untrusted("a key did not open")
    }
}

impl From<SyncError> for TeamError {
    fn from(error: SyncError) -> Self {
        match error {
            SyncError::VaultLocked => Self::VaultLocked,
            SyncError::Transport(TransportError::Unreachable(why)) => Self::Offline(why),
            other => Self::Sync(other),
        }
    }
}

impl From<TransportError> for TeamError {
    fn from(error: TransportError) -> Self {
        match error {
            TransportError::Unreachable(why) => Self::Offline(why),
            TransportError::SwitchedOff => Self::Unsupported,
            TransportError::Refused(why) | TransportError::SignIn(why) => Self::Refused {
                code: "error".into(),
                message: why,
                epoch: None,
            },
        }
    }
}

type Result<T> = std::result::Result<T, TeamError>;

// ── What travels ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemberState {
    Pending,
    Active,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub struct Policy {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub may_create: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WireIdentity {
    pub public_key: String,
    pub sealed_private: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WireTeam {
    pub id: Uuid,
    pub name: String,
    pub role: TeamRole,
    pub state: MemberState,
    pub epoch: u64,
    #[serde(default)]
    pub wrapped_key: Option<String>,
    #[serde(default)]
    pub needs_rotation: bool,
    #[serde(default)]
    pub members: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WireInvite {
    pub id: Uuid,
    pub role: TeamRole,
    pub expires_ms: u64,
    #[serde(default)]
    pub code: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WireJoined {
    pub team_id: Uuid,
    pub name: String,
    pub role: TeamRole,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WireMember {
    pub account_id: Uuid,
    pub display_name: String,
    pub role: TeamRole,
    pub state: MemberState,
    pub public_key: String,
    #[serde(default)]
    pub joined_ms: u64,
}

/// One member's new key in a rotation.
#[derive(Debug, Clone, Serialize)]
pub struct RotateKey {
    pub account_id: Uuid,
    pub wrapped_key: String,
}

/// A page of a team's records, with what the team is as of this pull.
#[derive(Debug, Clone, Deserialize)]
pub struct TeamPull {
    #[serde(flatten)]
    pub page: PullResponse,
    pub epoch: u64,
    pub role: TeamRole,
    #[serde(default)]
    pub needs_rotation: bool,
}

/// What a UwUSync server offers for teams, acting for one account.
pub trait TeamApi {
    /// The account this device signs in to.
    fn account(&self) -> Option<Uuid>;
    fn policy(&self) -> Result<Policy>;
    /// `None` when the account has no member key yet.
    fn identity(&self) -> Result<Option<WireIdentity>>;
    fn put_identity(&self, public_key: &str, sealed_private: &str) -> Result<WireIdentity>;
    fn teams(&self) -> Result<Vec<WireTeam>>;
    fn create(&self, name: &str, display_name: &str, wrapped_key: &str) -> Result<WireTeam>;
    fn rename(&self, team: Uuid, name: &str) -> Result<WireTeam>;
    fn delete(&self, team: Uuid) -> Result<()>;
    fn invite(&self, team: Uuid, role: TeamRole) -> Result<WireInvite>;
    fn invites(&self, team: Uuid) -> Result<Vec<WireInvite>>;
    fn revoke_invite(&self, team: Uuid, invite: Uuid) -> Result<()>;
    fn join(&self, code: &str, display_name: &str) -> Result<WireJoined>;
    fn members(&self, team: Uuid) -> Result<Vec<WireMember>>;
    fn confirm(
        &self,
        team: Uuid,
        account: Uuid,
        epoch: u64,
        public_key: &str,
        wrapped_key: &str,
    ) -> Result<()>;
    fn set_role(&self, team: Uuid, account: Uuid, role: TeamRole) -> Result<()>;
    fn remove_member(&self, team: Uuid, account: Uuid) -> Result<()>;
    fn rotate(&self, team: Uuid, epoch: u64, keys: &[RotateKey], cursor: u64) -> Result<WireTeam>;
    fn pull(&self, team: Uuid, cursor: u64, limit: usize) -> Result<TeamPull>;
    fn push(&self, team: Uuid, epoch: u64, envelopes: Vec<Envelope>) -> Result<PushResponse>;
}

// ── Over HTTP ────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct Refusal {
    #[serde(default)]
    error: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    epoch: Option<u64>,
}

/// What a person may be shown of what a server wrote: one line, no control
/// characters, not longer than a sentence.
fn shown(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(200)
        .collect::<String>()
        .trim()
        .to_string()
}

impl Server {
    fn team_call(&self, build: impl Fn() -> RequestBuilder, limit: u64) -> Result<Option<Vec<u8>>> {
        let response = self.exchange(&build)?;
        let status = response.status();
        if status.is_success() {
            if status == StatusCode::NO_CONTENT {
                return Ok(None);
            }
            return Ok(Some(read_limited(response, limit)?));
        }
        let refusal = read_limited(response, 64 * 1024)
            .ok()
            .and_then(|body| serde_json::from_slice::<Refusal>(&body).ok());
        let (code, message, epoch) = match refusal {
            Some(refusal) if !refusal.error.is_empty() => (
                shown(&refusal.error),
                shown(&refusal.message),
                refusal.epoch,
            ),
            _ => (
                if status == StatusCode::NOT_FOUND {
                    "not-found".to_string()
                } else {
                    "error".to_string()
                },
                format!("HTTP {}", status.as_u16()),
                None,
            ),
        };
        if code == "teams-disabled" {
            return Err(TeamError::Unsupported);
        }
        Err(TeamError::Refused {
            code,
            message,
            epoch,
        })
    }

    fn team_json<T: DeserializeOwned>(&self, build: impl Fn() -> RequestBuilder) -> Result<T> {
        self.team_json_limited(build, MAX_TEAM_ANSWER)
    }

    fn team_json_limited<T: DeserializeOwned>(
        &self,
        build: impl Fn() -> RequestBuilder,
        limit: u64,
    ) -> Result<T> {
        let body = self
            .team_call(build, limit)?
            .ok_or(TeamError::Untrusted("an empty answer"))?;
        serde_json::from_slice(&body).map_err(|_| TeamError::Untrusted("not what was asked for"))
    }

    fn team_done(&self, build: impl Fn() -> RequestBuilder) -> Result<()> {
        self.team_call(build, MAX_TEAM_ANSWER).map(drop)
    }
}

impl TeamApi for Server {
    fn account(&self) -> Option<Uuid> {
        Server::account(self)
    }

    fn policy(&self) -> Result<Policy> {
        match self.team_json(|| self.client().get(self.url("/v1/teams/policy"))) {
            // A server from before teams.
            Err(error) if error.is("not-found") => Err(TeamError::Unsupported),
            other => other,
        }
    }

    fn identity(&self) -> Result<Option<WireIdentity>> {
        match self.team_json(|| self.client().get(self.url("/v1/teams/identity"))) {
            Ok(identity) => Ok(Some(identity)),
            Err(error) if error.is("not-found") => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn put_identity(&self, public_key: &str, sealed_private: &str) -> Result<WireIdentity> {
        let body = serde_json::json!({
            "public_key": public_key,
            "sealed_private": sealed_private,
        });
        self.team_json(|| {
            self.client()
                .put(self.url("/v1/teams/identity"))
                .json(&body)
        })
    }

    fn teams(&self) -> Result<Vec<WireTeam>> {
        self.team_json(|| self.client().get(self.url("/v1/teams")))
    }

    fn create(&self, name: &str, display_name: &str, wrapped_key: &str) -> Result<WireTeam> {
        let body = serde_json::json!({
            "name": name,
            "display_name": display_name,
            "wrapped_key": wrapped_key,
        });
        self.team_json(|| self.client().post(self.url("/v1/teams")).json(&body))
    }

    fn rename(&self, team: Uuid, name: &str) -> Result<WireTeam> {
        let body = serde_json::json!({ "name": name });
        let url = self.url(&format!("/v1/teams/{team}"));
        self.team_json(|| self.client().patch(&url).json(&body))
    }

    fn delete(&self, team: Uuid) -> Result<()> {
        let url = self.url(&format!("/v1/teams/{team}"));
        self.team_done(|| self.client().delete(&url))
    }

    fn invite(&self, team: Uuid, role: TeamRole) -> Result<WireInvite> {
        let body = serde_json::json!({ "role": role });
        let url = self.url(&format!("/v1/teams/{team}/invites"));
        self.team_json(|| self.client().post(&url).json(&body))
    }

    fn invites(&self, team: Uuid) -> Result<Vec<WireInvite>> {
        let url = self.url(&format!("/v1/teams/{team}/invites"));
        self.team_json(|| self.client().get(&url))
    }

    fn revoke_invite(&self, team: Uuid, invite: Uuid) -> Result<()> {
        let url = self.url(&format!("/v1/teams/{team}/invites/{invite}"));
        self.team_done(|| self.client().delete(&url))
    }

    fn join(&self, code: &str, display_name: &str) -> Result<WireJoined> {
        let body = serde_json::json!({ "code": code, "display_name": display_name });
        self.team_json(|| self.client().post(self.url("/v1/teams/join")).json(&body))
    }

    fn members(&self, team: Uuid) -> Result<Vec<WireMember>> {
        let url = self.url(&format!("/v1/teams/{team}/members"));
        self.team_json(|| self.client().get(&url))
    }

    fn confirm(
        &self,
        team: Uuid,
        account: Uuid,
        epoch: u64,
        public_key: &str,
        wrapped_key: &str,
    ) -> Result<()> {
        let body = serde_json::json!({
            "epoch": epoch,
            "public_key": public_key,
            "wrapped_key": wrapped_key,
        });
        let url = self.url(&format!("/v1/teams/{team}/members/{account}/confirm"));
        self.team_done(|| self.client().post(&url).json(&body))
    }

    fn set_role(&self, team: Uuid, account: Uuid, role: TeamRole) -> Result<()> {
        let body = serde_json::json!({ "role": role });
        let url = self.url(&format!("/v1/teams/{team}/members/{account}"));
        self.team_done(|| self.client().patch(&url).json(&body))
    }

    fn remove_member(&self, team: Uuid, account: Uuid) -> Result<()> {
        let url = self.url(&format!("/v1/teams/{team}/members/{account}"));
        self.team_done(|| self.client().delete(&url))
    }

    fn rotate(&self, team: Uuid, epoch: u64, keys: &[RotateKey], cursor: u64) -> Result<WireTeam> {
        let body = serde_json::json!({ "epoch": epoch, "keys": keys, "cursor": cursor });
        let url = self.url(&format!("/v1/teams/{team}/rotate"));
        self.team_json(|| self.client().post(&url).json(&body))
    }

    fn pull(&self, team: Uuid, cursor: u64, limit: usize) -> Result<TeamPull> {
        let url = self.url(&format!("/v1/teams/{team}/records"));
        let query = [
            ("cursor", cursor.to_string()),
            ("limit", limit.min(MAX_BATCH).to_string()),
        ];
        self.team_json_limited(|| self.client().get(&url).query(&query), MAX_PAGE_BYTES)
    }

    fn push(&self, team: Uuid, epoch: u64, envelopes: Vec<Envelope>) -> Result<PushResponse> {
        let body = serde_json::json!({
            "schema": SCHEMA_VERSION,
            "epoch": epoch,
            "envelopes": envelopes,
        });
        let url = self.url(&format!("/v1/teams/{team}/records"));
        self.team_json(|| self.client().post(&url).json(&body))
    }
}

// ── A team's mailbox, as the engine sees it ────────────────────────────────

struct Mailbox<'a, A> {
    api: &'a A,
    team: Uuid,
    epoch: u64,
    writes: bool,
    /// The team as the last pull described it, when it is not as expected.
    moved: Cell<bool>,
    needs_rotation: Cell<Option<bool>>,
}

fn to_transport(error: TeamError) -> TransportError {
    match error {
        TeamError::Offline(why) => TransportError::Unreachable(why),
        TeamError::Refused { code, .. } => TransportError::Refused(code),
        other => TransportError::Refused(other.to_string()),
    }
}

impl<A: TeamApi> Transport for Mailbox<'_, A> {
    fn pull(
        &self,
        since: SyncCursor,
        limit: usize,
    ) -> std::result::Result<PullResponse, TransportError> {
        let pull = self
            .api
            .pull(self.team, since.0, limit)
            .map_err(to_transport)?;
        if pull.epoch != self.epoch || pull.role.writes() != self.writes {
            // Sealed under another key, or this account's role changed: the
            // next pass starts from the list of teams again.
            self.moved.set(true);
            return Err(TransportError::Refused("stale-epoch".into()));
        }
        self.needs_rotation.set(Some(pull.needs_rotation));
        Ok(pull.page)
    }

    fn push(&self, envelopes: Vec<Envelope>) -> std::result::Result<PushResponse, TransportError> {
        self.api
            .push(self.team, self.epoch, envelopes)
            .map_err(|error| {
                if error.is("stale-epoch") || error.is("forbidden") {
                    self.moved.set(true);
                }
                to_transport(error)
            })
    }

    fn manifests(&self) -> bool {
        false
    }

    fn writes(&self) -> bool {
        self.writes
    }
}

// ── What the interface is told ─────────────────────────────────────────────

/// A team, as this account sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamInfo {
    pub id: Uuid,
    pub name: String,
    pub role: TeamRole,
    pub state: MemberState,
    pub members: u32,
    pub needs_rotation: bool,
    /// The space the team is on this device, once it is open here.
    pub space: Option<String>,
    /// What stands in the way of this team, when something does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attention: Option<Attention>,
}

/// Why a team is not what it should be on this device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Attention {
    pub kind: AttentionKind,
    /// With a member's key: who.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AttentionKind {
    /// The server no longer lists the team and has not said it is gone.
    /// What this device has of it is kept.
    Unavailable,
    /// The key the server hands out does not come after the one this
    /// account had, or does not open what this device has.
    KeyRefused,
    /// A member's key is not the one this account wrote down: no new team
    /// key until somebody confirmed them again.
    MemberKeyChanged,
    /// A member this account took out is listed again.
    MemberReturned,
}

impl Attention {
    fn of(kind: AttentionKind) -> Self {
        Self {
            kind,
            account: None,
            member: None,
        }
    }

    fn from_error(error: &TeamError) -> Option<Self> {
        match error {
            TeamError::Untrusted(why) if *why == NOT_AFTER || *why == NOT_OPENING => {
                Some(Self::of(AttentionKind::KeyRefused))
            }
            TeamError::MemberChanged {
                account,
                name,
                returned,
            } => Some(Self {
                kind: if *returned {
                    AttentionKind::MemberReturned
                } else {
                    AttentionKind::MemberKeyChanged
                },
                account: Some(*account),
                member: Some(name.clone()),
            }),
            _ => None,
        }
    }
}

/// What taking a member out came to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Removal {
    /// The hosts whose stored logins that member may still know: a new team
    /// key does not change a password on a machine.
    pub hosts: Vec<HostRef>,
    /// The team has a new key, or needed none. `false`: it gets one with
    /// the first sync that reaches the server.
    pub rotated: bool,
}

/// The teams on one server.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerTeams {
    pub supported: bool,
    pub enabled: bool,
    pub may_create: bool,
    /// The fingerprint of this account's member key, once it has one.
    pub fingerprint: Option<String>,
    pub teams: Vec<TeamInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamMember {
    pub account_id: Uuid,
    pub display_name: String,
    pub role: TeamRole,
    pub state: MemberState,
    pub fingerprint: String,
    pub joined_ms: u64,
    pub me: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamInvite {
    pub id: Uuid,
    pub role: TeamRole,
    pub expires_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// A host whose stored login somebody who left may still know.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostRef {
    pub id: Uuid,
    pub name: String,
}

/// What one pass over a server's teams came to.
#[derive(Debug, Default)]
pub struct TeamPass {
    pub status: ServerTeams,
    /// A team's space appeared, went, or changed its name or role.
    pub spaces_changed: bool,
    /// Records came in or went out.
    pub records_changed: bool,
}

fn clean(text: &str, most: usize) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(most)
        .collect()
}

fn key32(text: &str) -> Result<[u8; 32]> {
    decode_base64(text)
        .and_then(|bytes| <[u8; 32]>::try_from(bytes.as_slice()).ok())
        .ok_or(TeamError::Untrusted("a public key that is not one"))
}

impl From<WireInvite> for TeamInvite {
    fn from(wire: WireInvite) -> Self {
        Self {
            id: wire.id,
            role: wire.role,
            expires_ms: wire.expires_ms,
            code: wire.code.map(|code| clean(&code, 64)),
        }
    }
}

// ── What a device does ─────────────────────────────────────────────────────

/// The teams of one server, for the account this device has there.
pub struct Teams<'a, A> {
    spaces: &'a Spaces,
    /// The space id of the server.
    server: &'a str,
    api: &'a A,
}

impl<'a, A: TeamApi> Teams<'a, A> {
    pub fn new(spaces: &'a Spaces, server: &'a str, api: &'a A) -> Self {
        Self {
            spaces,
            server,
            api,
        }
    }

    fn account(&self) -> Result<Uuid> {
        self.api
            .account()
            .ok_or_else(|| TeamError::refused("error", "this device has no account here"))
    }

    fn own_store(&self) -> Result<std::sync::Arc<Store>> {
        Ok(self.spaces.store(Some(self.server))?)
    }

    /// This account's member key. With `make`, one is made when the account
    /// has none; two devices doing that at once end with the same one.
    fn member(&self, make: bool) -> Result<Option<MemberKey>> {
        let account = self.account()?;
        let store = self.own_store()?;
        let identity = match self.api.identity()? {
            Some(identity) => identity,
            None if !make => return Ok(None),
            None => {
                let fresh = MemberKey::generate();
                let sealed = store.seal_member_key(&fresh, account)?;
                let put = self
                    .api
                    .put_identity(&encode_base64(fresh.public()), &encode_base64(&sealed));
                match put {
                    Ok(identity) => identity,
                    Err(error) if error.is("identity-exists") => self
                        .api
                        .identity()?
                        .ok_or(TeamError::Untrusted("a member key that is and is not"))?,
                    Err(error) => return Err(error),
                }
            }
        };
        let sealed = decode_base64(&identity.sealed_private)
            .filter(|sealed| sealed.len() <= MAX_SEALED_PRIVATE)
            .ok_or(TeamError::Untrusted("a member key that is not one"))?;
        let member = store
            .open_member_key(account, &sealed)
            .map_err(|error| match error {
                StoreError::VaultLocked => TeamError::VaultLocked,
                _ => TeamError::Untrusted("the member key does not open with this vault"),
            })?;
        // The key everybody else is told is this account's has to be the
        // one this account can open boxes for.
        if key32(&identity.public_key)? != member.public() {
            return Err(TeamError::Untrusted(
                "another public key than this account's",
            ));
        }
        Ok(Some(member))
    }

    pub(crate) fn listed(&self) -> Result<Vec<WireTeam>> {
        let mut teams = self.api.teams()?;
        if teams.len() > MAX_TEAMS {
            return Err(TeamError::Untrusted("more teams than there can be"));
        }
        let mut seen = HashSet::new();
        teams.retain(|team| seen.insert(team.id));
        Ok(teams)
    }

    fn one(&self, team: Uuid) -> Result<WireTeam> {
        self.listed()?
            .into_iter()
            .find(|listed| listed.id == team)
            .ok_or_else(|| TeamError::refused("not-found", "no such team"))
    }

    fn info(&self, team: &WireTeam) -> TeamInfo {
        TeamInfo {
            id: team.id,
            name: clean(&team.name, MAX_TEAM_NAME),
            role: team.role,
            state: team.state,
            members: team.members,
            needs_rotation: team.needs_rotation,
            space: self
                .spaces
                .team_space(self.server, team.id)
                .map(|space| space.id),
            attention: None,
        }
    }

    pub(crate) fn member_key(&self) -> Result<MemberKey> {
        self.member(false)?
            .ok_or(TeamError::Untrusted("no member key"))
    }

    /// The keys the server holds for this account in a team, opened.
    pub(crate) fn keys(&self, member: &MemberKey, team: &WireTeam) -> Result<TeamKeys> {
        let wrapped = team
            .wrapped_key
            .as_deref()
            .and_then(decode_base64)
            .filter(|wrapped| wrapped.len() <= MAX_WRAPPED)
            .ok_or(TeamError::Untrusted("a team without its key"))?;
        let context = WrapContext {
            team: team.id,
            epoch: team.epoch,
            account: self.account()?,
        };
        Ok(unwrap_team_key(member, context, &wrapped)?)
    }

    /// Bring the team's space on this device to the key the server hands
    /// out, making the space when there is none. Refuses a key that does not
    /// come after the one this account wrote down, or after the one this
    /// device holds.
    fn settle(&self, member: &MemberKey, team: &WireTeam) -> Result<(SpaceHandle, TeamKeys, bool)> {
        let keys = self.keys(member, team)?;
        let name = clean(&team.name, MAX_TEAM_NAME);
        let own = self.own_store()?;

        // What this account wrote down, on whichever of its devices. It is
        // what a device goes by that never had the team, or lost it.
        let record = own.team_key_record(team.id)?;
        if let Some(record) = &record {
            if !keys.follows_mark(team.epoch, record.epoch, &record.mark) {
                return Err(TeamError::Untrusted(NOT_AFTER));
            }
        }

        let (space, changed) = match self.spaces.team_space(self.server, team.id) {
            Some(space) => {
                if !matches!(space.store.vault_status()?, VaultStatus::Unlocked) {
                    // The key this device kept for it is gone. The server's
                    // opens it when it is still the same one. When not, what
                    // is here stays as it is: nothing the server says takes
                    // it away.
                    if !space.store.team_unlock(team.epoch, keys.key.clone())? {
                        return Err(TeamError::Untrusted(NOT_OPENING));
                    }
                    self.spaces.reseal_chain();
                }
                let state = space
                    .store
                    .team_state()?
                    .ok_or(TeamError::Untrusted("a space that is not a team's"))?;
                let held = space.store.team_key()?;
                if !keys.follows(team.epoch, state.epoch, &held) {
                    return Err(TeamError::Untrusted(NOT_AFTER));
                }
                let pending = space.store.team_pending_rotation()?;
                if team.epoch > state.epoch {
                    let mine = pending.as_ref().is_some_and(|(epoch, key)| {
                        *epoch == team.epoch && key.as_ref() == keys.key.as_ref()
                    });
                    let how = match (mine, team.role.writes()) {
                        (true, _) => Rekey::Lead,
                        (false, true) => Rekey::Follow,
                        (false, false) => Rekey::Read,
                    };
                    space.store.team_rekey(team.epoch, keys.key.clone(), how)?;
                    if mine {
                        // The rotation this device began did happen.
                        space.store.team_owe_rotation(false)?;
                    }
                    self.spaces.reseal_chain();
                } else if pending.is_some() {
                    // A rotation that was noted and never happened.
                    space.store.team_forget_rotation()?;
                }
                let changed = self.spaces.update_team(&space.id, &name, team.role)?;
                (self.spaces.get(&space.id).unwrap_or(space), changed)
            }
            None => {
                // With nothing written down either, this is the first key
                // this account sees for the team, and it is taken as it is.
                let space = self.spaces.add_team(
                    self.server,
                    team.id,
                    &name,
                    team.role,
                    team.epoch,
                    keys.key.clone(),
                )?;
                (space, true)
            }
        };
        if record.is_none_or(|record| record.epoch < team.epoch) {
            own.team_note_key(
                team.id,
                TeamKeyRecord {
                    epoch: team.epoch,
                    mark: key_mark(&keys.key),
                },
            )?;
        }
        Ok((space, keys, changed))
    }

    /// Push and pull one team to the end. `Ok(None)` when the team turned
    /// out to have moved on — another key, another role — and the next pass
    /// has to look again.
    fn sync(&self, team: &WireTeam, space: &SpaceHandle) -> Result<Option<Synced>> {
        let mailbox = Mailbox {
            api: self.api,
            team: team.id,
            epoch: team.epoch,
            writes: team.role.writes(),
            moved: Cell::new(false),
            needs_rotation: Cell::new(None),
        };
        let mut changed = false;
        // Twice at most: what a change of key by somebody else left to send
        // is only known after the first pull.
        for _ in 0..2 {
            let report = match sync_once(&space.store, &mailbox) {
                Ok(report) => report,
                Err(_) if mailbox.moved.get() => return Ok(None),
                Err(error) => return Err(error.into()),
            };
            changed |= report.pulled > 0 || report.pushed > 0;
            if !report.complete {
                return Ok(None);
            }
            if !team.role.writes() || space.store.team_requeue()? == 0 {
                break;
            }
        }
        Ok(Some(Synced {
            changed,
            needs_rotation: mailbox.needs_rotation.get().unwrap_or(team.needs_rotation),
        }))
    }

    /// What runs with every sync of the server: which teams there are, their
    /// keys, their records. A team leaves the device when the server says,
    /// asked about that team, that this account is not in it.
    pub fn pass(&self) -> Result<TeamPass> {
        let mut pass = TeamPass::default();
        let policy = match self.api.policy() {
            Ok(policy) => policy,
            Err(TeamError::Unsupported) => return Ok(pass),
            Err(error) => return Err(error),
        };
        pass.status.supported = true;
        pass.status.enabled = policy.enabled;
        pass.status.may_create = policy.enabled && policy.may_create;
        if !policy.enabled {
            return Ok(pass);
        }
        let member = self.member(false)?;
        pass.status.fingerprint = member.as_ref().map(|member| fingerprint(&member.public()));
        let teams = self.listed()?;

        for team in &teams {
            let active = team.state == MemberState::Active;
            let mut info = self.info(team);
            if let (true, Some(member)) = (active, &member) {
                match self.pass_one(member, team) {
                    Ok(done) => {
                        pass.spaces_changed |= done.spaces_changed;
                        pass.records_changed |= done.records_changed;
                        info.needs_rotation = done.needs_rotation;
                        info.attention = done.attention;
                    }
                    Err(TeamError::VaultLocked) => return Err(TeamError::VaultLocked),
                    Err(TeamError::Offline(why)) => return Err(TeamError::Offline(why)),
                    Err(error) => {
                        tracing::warn!(%error, "a team was not synced");
                        info.attention = Attention::from_error(&error);
                    }
                }
                info.space = self
                    .spaces
                    .team_space(self.server, team.id)
                    .map(|space| space.id);
            }
            pass.status.teams.push(info);
        }

        // A list without a team is no word about the team. The server is
        // asked for the team itself, and only its "no such team for you"
        // takes what is here away. What this account wrote down about the
        // team's key stays either way: a team that comes back is held to it.
        let known: HashSet<Uuid> = teams.iter().map(|team| team.id).collect();
        for space in self.spaces.teams_of(self.server) {
            let Some(team) = space.team.filter(|team| !known.contains(team)) else {
                continue;
            };
            let cursor = space.store.sync_state()?.cursor;
            match self.api.pull(team, cursor, 1) {
                Err(error) if error.is("not-found") => {
                    self.spaces.remove_team(&space.id)?;
                    pass.spaces_changed = true;
                }
                Err(TeamError::Offline(why)) => return Err(TeamError::Offline(why)),
                _ => {
                    tracing::warn!("a team is not listed any more and is kept");
                    pass.status.teams.push(TeamInfo {
                        id: team,
                        name: space.label.clone().unwrap_or_default(),
                        role: space.role.unwrap_or(TeamRole::Reader),
                        state: MemberState::Active,
                        members: 0,
                        needs_rotation: false,
                        space: Some(space.id.clone()),
                        attention: Some(Attention::of(AttentionKind::Unavailable)),
                    });
                }
            }
        }
        Ok(pass)
    }

    fn pass_one(&self, member: &MemberKey, team: &WireTeam) -> Result<Passed> {
        let (space, _, spaces_changed) = self.settle(member, team)?;
        let mut done = Passed {
            spaces_changed,
            records_changed: false,
            needs_rotation: team.needs_rotation,
            attention: None,
        };
        let Some(synced) = self.sync(team, &space)? else {
            // The team moved on while this ran. The next pass has it.
            return Ok(done);
        };
        done.records_changed = synced.changed;
        // What this device owes the team it owes whatever the server says.
        let owed = space
            .store
            .team_state()?
            .is_some_and(|state| state.rotation_owed);
        done.needs_rotation = synced.needs_rotation || (owed && team.role.manages());
        if done.needs_rotation && team.role.manages() {
            // Somebody who had the key left. Whoever manages the team and
            // comes by first gives it a new one.
            match self.rotate(member, team.id) {
                Ok(()) => {
                    done.needs_rotation = false;
                    done.records_changed = true;
                }
                Err(error @ (TeamError::VaultLocked | TeamError::Offline(_))) => return Err(error),
                Err(error) => {
                    tracing::warn!(%error, "a team's key was not rotated");
                    done.attention = Attention::from_error(&error);
                }
            }
        }
        Ok(done)
    }

    /// Give a team a new key: pull to the end, wrap a fresh key for every
    /// active member, have the server switch, and send every record again
    /// under the new key. Stops, and wraps for nobody, when a member is not
    /// who this account took them to be.
    fn rotate(&self, member: &MemberKey, team: Uuid) -> Result<()> {
        let account = self.account()?;
        let own = self.own_store()?;
        let mut last = TeamError::refused("stale-cursor", "the team kept changing");
        for _ in 0..ROTATE_TRIES {
            let wire = self.one(team)?;
            if wire.state != MemberState::Active || !wire.role.manages() {
                return Err(TeamError::refused(
                    "forbidden",
                    "not this account's to rotate",
                ));
            }
            let (space, keys, _) = self.settle(member, &wire)?;
            if self.sync(&wire, &space)?.is_none() {
                continue;
            }
            let members = self.active_members(wire.id)?;
            let epoch = wire
                .epoch
                .checked_add(1)
                .ok_or(TeamError::Untrusted("an epoch nothing comes after"))?;
            // Everybody is looked at before anything is wrapped or written.
            let mut unseen = Vec::new();
            for listed in &members {
                if listed.account == account {
                    if listed.public != member.public() {
                        return Err(TeamError::Untrusted(
                            "another public key than this account's",
                        ));
                    }
                    continue;
                }
                match own.team_pin(wire.id, listed.account)? {
                    Some(MemberPin::Confirmed(public)) if public == listed.public => {}
                    Some(pin) => {
                        return Err(TeamError::MemberChanged {
                            account: listed.account,
                            name: listed.name.clone(),
                            returned: pin == MemberPin::Removed,
                        });
                    }
                    // Never seen by this account: somebody else let them
                    // in. Their key is the one the server lists, and from
                    // here on no other.
                    None => unseen.push(listed),
                }
            }
            for listed in unseen {
                own.team_pin_member(wire.id, listed.account, &listed.public)?;
            }
            let fresh = new_team_key();
            let earlier = keys.handed_on();
            let mut wrapped = Vec::with_capacity(members.len());
            for listed in &members {
                // What an earlier build kept in the team's records about
                // who is confirmed counts for nothing and goes.
                space.store.team_drop_mailbox_pin(listed.account)?;
                let context = WrapContext {
                    team: wire.id,
                    epoch,
                    account: listed.account,
                };
                let sealed = wrap_team_key(&fresh, &earlier, context, &listed.public)?;
                wrapped.push(RotateKey {
                    account_id: listed.account,
                    wrapped_key: encode_base64(sealed),
                });
            }
            // Written down first: should this stop after the server
            // switched, the next start knows the key and that everything
            // has to go back.
            space.store.team_note_rotation(epoch, &fresh)?;
            let cursor = space.store.sync_state()?.cursor;
            match self.api.rotate(wire.id, epoch, &wrapped, cursor) {
                Ok(_) => {
                    let mark = key_mark(&fresh);
                    space.store.team_rekey(epoch, fresh, Rekey::Lead)?;
                    space.store.team_owe_rotation(false)?;
                    own.team_note_key(wire.id, TeamKeyRecord { epoch, mark })?;
                    self.spaces.reseal_chain();
                    let now = WireTeam {
                        epoch,
                        needs_rotation: false,
                        ..wire
                    };
                    self.sync(&now, &space)?;
                    return Ok(());
                }
                Err(error)
                    if error.is("stale-cursor")
                        || error.is("members-mismatch")
                        || error.is("stale-epoch") =>
                {
                    space.store.team_forget_rotation()?;
                    last = error;
                }
                // It may have happened all the same: the note stays, and
                // the next pass sees which key the team has.
                Err(error @ TeamError::Offline(_)) => return Err(error),
                Err(error) => {
                    space.store.team_forget_rotation()?;
                    return Err(error);
                }
            }
        }
        Err(last)
    }

    fn active_members(&self, team: Uuid) -> Result<Vec<Listed>> {
        let members = self.api.members(team)?;
        if members.len() > MAX_MEMBERS {
            return Err(TeamError::Untrusted("more members than there can be"));
        }
        members
            .iter()
            .filter(|member| member.state == MemberState::Active)
            .map(|member| {
                Ok(Listed {
                    account: member.account_id,
                    name: clean(&member.display_name, MAX_DISPLAY_NAME),
                    public: key32(&member.public_key)?,
                })
            })
            .collect()
    }

    // ── What a person asks for ──────────────────────────────────────────────

    /// Make a team. This account is its owner and only member.
    pub fn create(&self, name: &str, display_name: &str) -> Result<TeamInfo> {
        let account = self.account()?;
        let member = self
            .member(true)?
            .ok_or(TeamError::Untrusted("no member key"))?;
        let key = new_team_key();
        // The server names the team; the key is wrapped for the id it gave.
        // So the team is made with a key nobody can open and given its real
        // one before anything is written: epoch 1 has no records.
        let name = clean(name, MAX_TEAM_NAME);
        let display = clean(display_name, MAX_DISPLAY_NAME);
        let placeholder = encode_base64([0u8; 1]);
        let made = self.api.create(&name, &display, &placeholder)?;
        let finish = || -> Result<TeamInfo> {
            let epoch = made
                .epoch
                .checked_add(1)
                .ok_or(TeamError::Untrusted("an epoch nothing comes after"))?;
            let context = WrapContext {
                team: made.id,
                epoch,
                account,
            };
            let wrapped = wrap_team_key(&key, &[], context, &member.public())?;
            let keys = [RotateKey {
                account_id: account,
                wrapped_key: encode_base64(wrapped),
            }];
            let team = self.api.rotate(made.id, epoch, &keys, 0)?;
            let space = self.spaces.add_team(
                self.server,
                made.id,
                &name,
                TeamRole::Owner,
                epoch,
                key.clone(),
            )?;
            let mark = key_mark(&key);
            self.own_store()?
                .team_note_key(made.id, TeamKeyRecord { epoch, mark })?;
            let now = WireTeam { epoch, ..team };
            self.sync(&now, &space)?;
            Ok(self.info(&now))
        };
        finish().inspect_err(|_| {
            // Half a team is no team.
            let _ = self.api.delete(made.id);
        })
    }

    pub fn rename(&self, team: Uuid, name: &str) -> Result<()> {
        let name = clean(name, MAX_TEAM_NAME);
        let renamed = self.api.rename(team, &name)?;
        if let Some(space) = self.spaces.team_space(self.server, team) {
            self.spaces.update_team(&space.id, &name, renamed.role)?;
        }
        Ok(())
    }

    /// This account is out of the team because it said so: what the device
    /// has of the team goes, and so does what the account wrote down about
    /// it. `members` are the accounts it may have written something about.
    fn forget(&self, team: Uuid, members: &[Uuid]) -> Result<()> {
        if let Some(space) = self.spaces.team_space(self.server, team) {
            self.spaces.remove_team(&space.id)?;
        }
        let own = self.own_store()?;
        own.team_forget_key(team)?;
        for member in members {
            own.team_unpin_member(team, *member)?;
        }
        Ok(())
    }

    fn accounts(&self, team: Uuid) -> Vec<Uuid> {
        let listed = self.api.members(team).unwrap_or_default();
        listed
            .iter()
            .take(MAX_MEMBERS)
            .map(|member| member.account_id)
            .collect()
    }

    /// Delete the team for everybody.
    pub fn delete(&self, team: Uuid) -> Result<()> {
        let members = self.accounts(team);
        self.api.delete(team)?;
        self.forget(team, &members)
    }

    /// Leave the team. What it holds goes from this device.
    pub fn leave(&self, team: Uuid) -> Result<()> {
        let account = self.account()?;
        let members = self.accounts(team);
        self.api.remove_member(team, account)?;
        self.forget(team, &members)
    }

    pub fn invite(&self, team: Uuid, role: TeamRole) -> Result<TeamInvite> {
        Ok(self.api.invite(team, role)?.into())
    }

    pub fn invites(&self, team: Uuid) -> Result<Vec<TeamInvite>> {
        Ok(self
            .api
            .invites(team)?
            .into_iter()
            .take(64)
            .map(TeamInvite::from)
            .collect())
    }

    pub fn revoke_invite(&self, team: Uuid, invite: Uuid) -> Result<()> {
        self.api.revoke_invite(team, invite)
    }

    /// Redeem an invite. The account then waits until somebody who manages
    /// the team has compared its fingerprint and confirmed it.
    pub fn join(&self, code: &str, display_name: &str) -> Result<TeamInfo> {
        self.member(true)?;
        let joined = self
            .api
            .join(&clean(code, 64), &clean(display_name, MAX_DISPLAY_NAME))?;
        // Coming in by an invite is starting over: an account that was out
        // of the team for long has no key any more that the next one could
        // come after. What a device still holds of the team it goes on
        // holding the server to.
        self.own_store()?.team_forget_key(joined.team_id)?;
        Ok(TeamInfo {
            id: joined.team_id,
            name: clean(&joined.name, MAX_TEAM_NAME),
            role: joined.role,
            state: MemberState::Pending,
            members: 0,
            needs_rotation: false,
            space: None,
            attention: None,
        })
    }

    pub fn members(&self, team: Uuid) -> Result<Vec<TeamMember>> {
        let account = self.account()?;
        let members = self.api.members(team)?;
        if members.len() > MAX_MEMBERS {
            return Err(TeamError::Untrusted("more members than there can be"));
        }
        members
            .into_iter()
            .map(|member| {
                Ok(TeamMember {
                    account_id: member.account_id,
                    display_name: clean(&member.display_name, MAX_DISPLAY_NAME),
                    role: member.role,
                    state: member.state,
                    fingerprint: fingerprint(&key32(&member.public_key)?),
                    joined_ms: member.joined_ms,
                    me: member.account_id == account,
                })
            })
            .collect()
    }

    /// Let a waiting member in. `seen` is the fingerprint the person who
    /// confirms compared with the newcomer: the team key is wrapped for the
    /// key that has it, or not at all.
    ///
    /// For a member who is in already it is this account taking their key
    /// as compared: after the key changed, or after they came back. Nothing
    /// is wrapped then; the next rotation is.
    pub fn confirm(&self, team: Uuid, account: Uuid, seen: &str) -> Result<()> {
        let member = self.member_key()?;
        let own = self.own_store()?;
        let mut last = TeamError::refused("stale-epoch", "the team kept changing");
        for _ in 0..ROTATE_TRIES {
            let wire = self.one(team)?;
            let (space, keys, _) = self.settle(&member, &wire)?;
            let listed = self.api.members(team)?;
            let newcomer = listed
                .iter()
                .find(|listed| listed.account_id == account)
                .ok_or_else(|| TeamError::refused("not-found", "no such member"))?;
            let public = key32(&newcomer.public_key)?;
            if !same_fingerprint(&fingerprint(&public), seen) {
                return Err(TeamError::refused(
                    "identity-changed",
                    "identity-changed: that is not the key that was compared",
                ));
            }
            if wire.role.writes() {
                space.store.team_drop_mailbox_pin(account)?;
            }
            if newcomer.state == MemberState::Active {
                own.team_pin_member(team, account, &public)?;
                return Ok(());
            }
            let context = WrapContext {
                team,
                epoch: wire.epoch,
                account,
            };
            let wrapped = wrap_team_key(&keys.key, &keys.earlier, context, &public)?;
            // Written down before the server hears of it: from here on this
            // account wraps for this key, and for no other under that
            // account. It is the key that was compared either way.
            own.team_pin_member(team, account, &public)?;
            let sent = self.api.confirm(
                team,
                account,
                wire.epoch,
                &encode_base64(public),
                &encode_base64(wrapped),
            );
            match sent {
                Ok(()) => {
                    self.sync(&wire, &space)?;
                    return Ok(());
                }
                Err(error) if error.is("stale-epoch") => last = error,
                Err(error) => return Err(error),
            }
        }
        Err(last)
    }

    pub fn set_role(&self, team: Uuid, account: Uuid, role: TeamRole) -> Result<()> {
        self.api.set_role(team, account, role)
    }

    /// Take a member out and, when they had the key, give the team a new
    /// one. That this device owes the team a key is written down before the
    /// server hears of the removal, and holds until the device gave it one,
    /// whatever the server says the team needs.
    pub fn remove(&self, team: Uuid, account: Uuid) -> Result<Removal> {
        let own = self.own_store()?;
        // Somebody who was only waiting never had the key. Not knowing is
        // taken for having had it.
        let had_key = match self.api.members(team) {
            Ok(members) => members
                .iter()
                .find(|member| member.account_id == account)
                .is_none_or(|member| member.state == MemberState::Active),
            Err(error @ TeamError::Offline(_)) => return Err(error),
            Err(_) => true,
        };
        let space = || self.spaces.team_space(self.server, team);
        let owe = |owed: bool| -> Result<bool> {
            let Some(space) = space() else {
                return Ok(false);
            };
            let Some(state) = space.store.team_state()? else {
                return Ok(false);
            };
            space.store.team_owe_rotation(owed)?;
            Ok(state.rotation_owed)
        };

        // No key for them from this account any more, first of all.
        let pin = own.team_pin(team, account)?;
        own.team_bar_member(team, account)?;
        let owed_before = if had_key { owe(true)? } else { false };
        match self.api.remove_member(team, account) {
            Ok(()) => {}
            // It may have happened all the same.
            Err(error @ TeamError::Offline(_)) => return Err(error),
            Err(error) => {
                // Still in, as they were.
                match pin {
                    Some(MemberPin::Confirmed(public)) => {
                        own.team_pin_member(team, account, &public)?
                    }
                    Some(MemberPin::Removed) => {}
                    None => own.team_unpin_member(team, account)?,
                }
                if had_key && !owed_before {
                    owe(false)?;
                }
                return Err(error);
            }
        }

        let asked = self
            .one(team)
            .map(|wire| wire.needs_rotation)
            .unwrap_or(false);
        if !had_key && !asked {
            return Ok(Removal {
                hosts: Vec::new(),
                rotated: true,
            });
        }
        owe(true)?;
        let rotated = self
            .member_key()
            .and_then(|member| self.rotate(&member, team));
        if let Err(error) = &rotated {
            // Owed from here on: the next pass that reaches the server
            // pays, whoever of those who manage the team gets there first.
            tracing::warn!(%error, "a team's key was not rotated after a member left");
            owe(true)?;
        }
        let hosts = match space() {
            Some(space) => space
                .store
                .team_credential_hosts()?
                .into_iter()
                .map(|(id, name)| HostRef { id, name })
                .collect(),
            None => Vec::new(),
        };
        Ok(Removal {
            hosts,
            rotated: rotated.is_ok(),
        })
    }
}

/// An active member, as the server lists them.
struct Listed {
    account: Uuid,
    name: String,
    public: [u8; 32],
}

struct Synced {
    changed: bool,
    needs_rotation: bool,
}

struct Passed {
    spaces_changed: bool,
    records_changed: bool,
    needs_rotation: bool,
    attention: Option<Attention>,
}
