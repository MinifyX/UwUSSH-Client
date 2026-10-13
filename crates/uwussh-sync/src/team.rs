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
//! account's member key; what a wrap is and why it carries the keys before
//! it is in `uwussh_vault::team`.
//!
//! What this side holds the server to:
//!
//! - **A newcomer gets the key for the public key somebody compared.** A
//!   confirmation names a fingerprint; the key the server lists has to have
//!   it, or nothing is wrapped.
//! - **A rotation wraps for confirmed keys only.** Every confirmed key is a
//!   record in the team's vault. A member the server lists as active with a
//!   key nobody confirmed stops the rotation instead of receiving the key.
//! - **A new key has to come after the one held.** A wrap that does not
//!   carry the key this device has, an epoch that went backwards, another
//!   key under the same epoch: the team is left as it is and not synced.

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
    Rekey, SpaceHandle, Spaces, Store, StoreError, TeamRole, VaultStatus, MAX_TEAM_NAME,
};
use uwussh_vault::team::{fingerprint, new_team_key, same_fingerprint, unwrap_team_key};
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
            Self::Untrusted(_) | Self::Store(_) | Self::Sync(_) => "error",
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

    fn listed(&self) -> Result<Vec<WireTeam>> {
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
        }
    }

    /// The keys the server holds for this account in a team, opened.
    fn keys(&self, member: &MemberKey, team: &WireTeam) -> Result<TeamKeys> {
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
    /// come after the one held.
    fn settle(&self, member: &MemberKey, team: &WireTeam) -> Result<(SpaceHandle, TeamKeys, bool)> {
        let keys = self.keys(member, team)?;
        let name = clean(&team.name, MAX_TEAM_NAME);
        let mut changed = false;
        let mut space = self.spaces.team_space(self.server, team.id);

        if let Some(found) = &space {
            if !matches!(found.store.vault_status()?, VaultStatus::Unlocked) {
                // The key this device kept for it is gone. The server's
                // opens it when it is still the same one.
                if !found.store.team_unlock(team.epoch, keys.key.clone())? {
                    tracing::warn!("a team's space did not open any more and is fetched again");
                    self.spaces.remove_team(&found.id)?;
                    space = None;
                    changed = true;
                } else {
                    self.spaces.reseal_chain();
                }
            }
        }

        let space = match space {
            Some(space) => {
                let state = space
                    .store
                    .team_state()?
                    .ok_or(TeamError::Untrusted("a space that is not a team's"))?;
                let held = space.store.team_key()?;
                if !keys.follows(team.epoch, state.epoch, &held) {
                    return Err(TeamError::Untrusted(
                        "a team key that does not come after the one held",
                    ));
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
                    self.spaces.reseal_chain();
                } else if pending.is_some() {
                    // A rotation that was noted and never happened.
                    space.store.team_forget_rotation()?;
                }
                changed |= self.spaces.update_team(&space.id, &name, team.role)?;
                self.spaces.get(&space.id).unwrap_or(space)
            }
            None => {
                changed = true;
                self.spaces.add_team(
                    self.server,
                    team.id,
                    &name,
                    team.role,
                    team.epoch,
                    keys.key.clone(),
                )?
            }
        };
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
    /// keys, their records. Teams this account is no longer in leave the
    /// device.
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
                    }
                    Err(TeamError::VaultLocked) => return Err(TeamError::VaultLocked),
                    Err(TeamError::Offline(why)) => return Err(TeamError::Offline(why)),
                    Err(error) => {
                        tracing::warn!(%error, "a team was not synced");
                    }
                }
                info.space = self
                    .spaces
                    .team_space(self.server, team.id)
                    .map(|space| space.id);
            }
            pass.status.teams.push(info);
        }

        // What this account is no longer in is gone from here too.
        let known: HashSet<Uuid> = teams.iter().map(|team| team.id).collect();
        for space in self.spaces.teams_of(self.server) {
            if space.team.is_some_and(|team| !known.contains(&team)) {
                self.spaces.remove_team(&space.id)?;
                pass.spaces_changed = true;
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
        };
        let Some(synced) = self.sync(team, &space)? else {
            // The team moved on while this ran. The next pass has it.
            return Ok(done);
        };
        done.records_changed = synced.changed;
        done.needs_rotation = synced.needs_rotation;
        if synced.needs_rotation && team.role.manages() {
            // Somebody who had the key left. Whoever manages the team and
            // comes by first gives it a new one.
            match self.rotate(member, team.id) {
                Ok(()) => {
                    done.needs_rotation = false;
                    done.records_changed = true;
                }
                Err(error @ (TeamError::VaultLocked | TeamError::Offline(_))) => return Err(error),
                Err(error) => tracing::warn!(%error, "a team's key was not rotated"),
            }
        }
        Ok(done)
    }

    /// Give a team a new key: pull to the end, wrap a fresh key for every
    /// active member whose key was confirmed, have the server switch, and
    /// send every record again under the new key.
    fn rotate(&self, member: &MemberKey, team: Uuid) -> Result<()> {
        let account = self.account()?;
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
            let mut wrapped = Vec::with_capacity(members.len());
            let fresh = new_team_key();
            let epoch = wire.epoch + 1;
            let earlier = keys.handed_on();
            for (id, public) in &members {
                let confirmed = if *id == account {
                    *public == member.public()
                } else {
                    space.store.team_pinned(*id)?.as_ref() == Some(public)
                };
                if !confirmed {
                    return Err(TeamError::Untrusted("a member whose key nobody confirmed"));
                }
                let context = WrapContext {
                    team: wire.id,
                    epoch,
                    account: *id,
                };
                wrapped.push(RotateKey {
                    account_id: *id,
                    wrapped_key: encode_base64(wrap_team_key(&fresh, &earlier, context, public)?),
                });
            }
            // Written down first: should this stop after the server
            // switched, the next start knows the key and that everything
            // has to go back.
            space.store.team_note_rotation(epoch, &fresh)?;
            let cursor = space.store.sync_state()?.cursor;
            match self.api.rotate(wire.id, epoch, &wrapped, cursor) {
                Ok(_) => {
                    space.store.team_rekey(epoch, fresh, Rekey::Lead)?;
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

    fn active_members(&self, team: Uuid) -> Result<Vec<(Uuid, [u8; 32])>> {
        let members = self.api.members(team)?;
        if members.len() > MAX_MEMBERS {
            return Err(TeamError::Untrusted("more members than there can be"));
        }
        members
            .iter()
            .filter(|member| member.state == MemberState::Active)
            .map(|member| Ok((member.account_id, key32(&member.public_key)?)))
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
            let epoch = made.epoch + 1;
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
            space.store.team_pin_member(account, &member.public())?;
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

    fn forget(&self, team: Uuid) -> Result<()> {
        if let Some(space) = self.spaces.team_space(self.server, team) {
            self.spaces.remove_team(&space.id)?;
        }
        Ok(())
    }

    /// Delete the team for everybody.
    pub fn delete(&self, team: Uuid) -> Result<()> {
        self.api.delete(team)?;
        self.forget(team)
    }

    /// Leave the team. What it holds goes from this device.
    pub fn leave(&self, team: Uuid) -> Result<()> {
        let account = self.account()?;
        // Taken off the list of confirmed keys first, while this account
        // may still write: a server that listed it again would otherwise
        // get it the next key.
        let space = self
            .spaces
            .team_space(self.server, team)
            .filter(|space| !space.read_only);
        let mut pinned = None;
        if let Some(space) = &space {
            pinned = space.store.team_pinned(account).ok().flatten();
            if pinned.is_some() && space.store.team_unpin_member(account).is_ok() {
                if let Ok(wire) = self.one(team) {
                    let _ = self.sync(&wire, space);
                }
            }
        }
        if let Err(error) = self.api.remove_member(team, account) {
            // Still in: the key stays confirmed.
            if let (Some(space), Some(public)) = (&space, pinned) {
                let _ = space.store.team_pin_member(account, &public);
            }
            return Err(error);
        }
        self.forget(team)
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
        Ok(TeamInfo {
            id: joined.team_id,
            name: clean(&joined.name, MAX_TEAM_NAME),
            role: joined.role,
            state: MemberState::Pending,
            members: 0,
            needs_rotation: false,
            space: None,
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
    pub fn confirm(&self, team: Uuid, account: Uuid, seen: &str) -> Result<()> {
        let member = self
            .member(false)?
            .ok_or(TeamError::Untrusted("no member key"))?;
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
                    "that is not the key that was compared",
                ));
            }
            let context = WrapContext {
                team,
                epoch: wire.epoch,
                account,
            };
            let wrapped = wrap_team_key(&keys.key, &keys.earlier, context, &public)?;
            // Written down before the server hears of it: from here on a
            // rotation wraps for this key, and for no other under this
            // account. It is the key that was compared either way.
            space.store.team_pin_member(account, &public)?;
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

    /// Take a member out and give the team a new key. Returns the hosts
    /// whose stored logins that member may still know: a new team key does
    /// not change a password on a machine.
    pub fn remove(&self, team: Uuid, account: Uuid) -> Result<Vec<HostRef>> {
        self.api.remove_member(team, account)?;
        let Some(space) = self.spaces.team_space(self.server, team) else {
            return Ok(Vec::new());
        };
        space.store.team_unpin_member(account)?;
        let rotated = self
            .member(false)
            .and_then(|member| member.ok_or(TeamError::Untrusted("no member key")))
            .and_then(|member| self.rotate(&member, team));
        if let Err(error) = rotated {
            // The next pass tries again: the team says it needs it.
            tracing::warn!(%error, "a team's key was not rotated after a member left");
        }
        Ok(space
            .store
            .team_credential_hosts()?
            .into_iter()
            .map(|(id, name)| HostRef { id, name })
            .collect())
    }
}

struct Synced {
    changed: bool,
    needs_rotation: bool,
}

struct Passed {
    spaces_changed: bool,
    records_changed: bool,
    needs_rotation: bool,
}
