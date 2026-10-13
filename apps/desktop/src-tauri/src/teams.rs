//! Teams: vaults several accounts share, on a UwUSync server.
//!
//! The work is `uwussh_sync::team`'s; this layer adds what belongs to the
//! running app. A team is reached through its server's connection, so its
//! pass runs in that server's worker, right after the server's own, and
//! nothing here opens a connection of its own. What the pass learned is
//! kept on the server's [`Sync`]: `teams_status` answers from that and
//! never waits for the network.
//!
//! One thing at a time per server: a command takes the worker's pass flag
//! for as long as it runs, so a rotation and a pass never cross.

use crate::lock::Running;
use crate::sync::{self, Sync, SyncFailure, Syncs};
use crate::AppState;
use serde::Serialize;
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;
use uwussh_store::{SpaceHandle, SpaceKind, Spaces, Store, TeamRole, VaultStatus};
use uwussh_sync::{
    HostRef, MemberState, Server, ServerTeams, TeamError, TeamInfo, TeamInvite, TeamMember, Teams,
};

/// What the page is told when something went wrong. `kind` is what it acts
/// on; `message` is for showing.
#[derive(Debug, Serialize)]
pub(crate) struct TeamFailure {
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

impl TeamFailure {
    fn of(kind: &'static str) -> Self {
        Self {
            kind,
            message: None,
        }
    }

    fn error(message: impl std::fmt::Display) -> Self {
        Self {
            kind: "error",
            message: Some(message.to_string()),
        }
    }
}

impl From<TeamError> for TeamFailure {
    fn from(error: TeamError) -> Self {
        let kind = error.kind();
        let message = match &error {
            TeamError::VaultLocked | TeamError::Unsupported => None,
            other => Some(other.to_string()),
        };
        Self { kind, message }
    }
}

impl From<SyncFailure> for TeamFailure {
    fn from(failure: SyncFailure) -> Self {
        match failure {
            SyncFailure::VaultLocked => Self::of("vault-locked"),
            SyncFailure::Unreachable { message } => Self {
                kind: "offline",
                message: Some(message),
            },
            other => Self::error(sync::describe(&other)),
        }
    }
}

type TeamResult<T> = Result<T, TeamFailure>;

/// Whether this server can have teams at all: it syncs with UwUSync.
fn on_uwusync(store: &Store) -> bool {
    let on_lock = store.lock_state().map(|s| s.active).unwrap_or(true);
    !on_lock && store.sync_state().map(|s| s.paired()).unwrap_or(false)
}

fn tell(app: &AppHandle) {
    let _ = app.emit("teams:changed", ());
}

/// One pass over a server's teams, with what it learned kept and the page
/// told what changed.
fn pass(app: &AppHandle, sync: &Sync, teams: &Teams<'_, Server>) -> Result<(), TeamError> {
    let pass = teams.pass()?;
    let differs = {
        let mut kept = sync.teams.lock();
        let differs = kept.as_ref() != Some(&pass.status);
        *kept = Some(pass.status);
        differs
    };
    if pass.spaces_changed {
        crate::spaces::changed(app);
    }
    if pass.records_changed || pass.spaces_changed {
        // The host list loads again.
        let _ = app.emit("sync:changed", 1usize);
    }
    if differs || pass.spaces_changed {
        tell(app);
    }
    Ok(())
}

/// The worker's part: after a pass of the server went through.
pub(crate) fn after_pass(app: &AppHandle, id: &str, store: &Store, sync: &Sync) {
    if !on_uwusync(store) {
        return;
    }
    // A command is at it, or another pass: the next round is soon enough.
    let Some(_running) = Running::try_take(sync) else {
        return;
    };
    if sync.gone.load(Ordering::SeqCst) || sync.held.load(Ordering::SeqCst) {
        return;
    }
    let Ok(server) = sync::server(store, sync) else {
        return;
    };
    let state = app.state::<AppState>();
    let teams = Teams::new(&state.spaces, id, server.as_ref());
    match pass(app, sync, &teams) {
        Ok(()) | Err(TeamError::VaultLocked) => {}
        Err(TeamError::Offline(why)) => {
            *sync.server.lock() = None;
            tracing::info!(error = %why, "the teams pass did not reach the server");
        }
        Err(error) => tracing::info!(%error, "the teams pass failed"),
    }
}

/// A server that no longer syncs with UwUSync — disconnected, or moved to
/// UwULock — has no teams: what they held leaves this device.
pub(crate) fn without_server(app: &AppHandle, id: &str, store: &Store, sync: &Sync) {
    let state = app.state::<AppState>();
    // Only when that is known for certain, never on a database error.
    let unpaired = matches!(store.sync_state(), Ok(paired) if !paired.paired());
    let on_lock = matches!(store.lock_state(), Ok(lock) if lock.active);
    if !(unpaired || on_lock) {
        return;
    }
    let teams = state.spaces.teams_of(id);
    let had = sync.teams.lock().take().is_some();
    if teams.is_empty() {
        if had {
            tell(app);
        }
        return;
    }
    for team in teams {
        if let Err(error) = state.spaces.remove_team(&team.id) {
            tracing::warn!(%error, "a team's space could not be removed");
        }
    }
    crate::spaces::changed(app);
    let _ = app.emit("sync:changed", 1usize);
    tell(app);
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TeamServer {
    space: String,
    label: String,
    #[serde(flatten)]
    teams: ServerTeams,
}

/// Before the first pass of a run: what this device has of the server's
/// teams.
fn from_here(spaces: &Spaces, server: &SpaceHandle) -> ServerTeams {
    let teams: Vec<TeamInfo> = spaces
        .teams_of(&server.id)
        .into_iter()
        .filter_map(|space| {
            Some(TeamInfo {
                id: space.team?,
                name: space.label.clone().unwrap_or_default(),
                role: space.role?,
                state: MemberState::Active,
                members: 0,
                needs_rotation: false,
                space: Some(space.id),
            })
        })
        .collect();
    ServerTeams {
        supported: true,
        enabled: true,
        may_create: false,
        fingerprint: None,
        teams,
    }
}

/// The teams on every server, as the last pass found them. Never waits.
#[tauri::command]
pub(crate) fn teams_status(state: State<'_, AppState>, syncs: State<'_, Syncs>) -> Vec<TeamServer> {
    state
        .spaces
        .servers()
        .into_iter()
        .map(|server| {
            let teams = if !on_uwusync(&server.store) {
                ServerTeams::default()
            } else {
                let kept = syncs.of(&server.id).teams.lock().clone();
                kept.unwrap_or_else(|| from_here(&state.spaces, &server))
            };
            TeamServer {
                label: crate::spaces::label(&server.store),
                space: server.id,
                teams,
            }
        })
        .collect()
}

/// Run team work for one server on a worker thread, with no pass of that
/// server in between; `refresh` looks at the teams again afterwards, so the
/// page hears what the work changed.
async fn on_server<T: Send + 'static>(
    app: &AppHandle,
    server: String,
    refresh: bool,
    work: impl FnOnce(&Teams<'_, Server>) -> Result<T, TeamError> + Send + 'static,
) -> TeamResult<T> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let space = state
            .spaces
            .get(&server)
            .filter(|space| space.kind == SpaceKind::Server)
            .ok_or_else(|| TeamFailure::of("unsupported"))?;
        if !on_uwusync(&space.store) {
            return Err(TeamFailure::of("unsupported"));
        }
        if !matches!(space.store.vault_status(), Ok(VaultStatus::Unlocked)) {
            return Err(TeamFailure::of("vault-locked"));
        }
        let sync = app.state::<Syncs>().of(&server);
        if sync.gone.load(Ordering::SeqCst) || sync.held.load(Ordering::SeqCst) {
            return Err(TeamFailure::error("this server is being removed"));
        }
        let _running = Running::take(&sync)?;
        let api = sync::server(&space.store, &sync)?;
        let teams = Teams::new(&state.spaces, &server, api.as_ref());
        let done = work(&teams);
        if matches!(done, Err(TeamError::Offline(_))) {
            // A fresh connection next time.
            *sync.server.lock() = None;
        } else if refresh {
            if let Err(error) = pass(&app, &sync, &teams) {
                tracing::info!(%error, "the teams were not looked at again");
            }
            tell(&app);
        }
        done.map_err(TeamFailure::from)
    })
    .await
    .map_err(TeamFailure::error)?
}

#[tauri::command]
pub(crate) async fn team_create(
    app: AppHandle,
    server: String,
    name: String,
    display_name: String,
) -> TeamResult<TeamInfo> {
    on_server(&app, server, true, move |teams| {
        teams.create(&name, &display_name)
    })
    .await
}

#[tauri::command]
pub(crate) async fn team_rename(
    app: AppHandle,
    server: String,
    team: Uuid,
    name: String,
) -> TeamResult<()> {
    on_server(&app, server, true, move |teams| teams.rename(team, &name)).await
}

/// Delete the team for everybody.
#[tauri::command]
pub(crate) async fn team_delete(app: AppHandle, server: String, team: Uuid) -> TeamResult<()> {
    on_server(&app, server, true, move |teams| teams.delete(team)).await
}

#[tauri::command]
pub(crate) async fn team_leave(app: AppHandle, server: String, team: Uuid) -> TeamResult<()> {
    on_server(&app, server, true, move |teams| teams.leave(team)).await
}

#[tauri::command]
pub(crate) async fn team_invite(
    app: AppHandle,
    server: String,
    team: Uuid,
    role: TeamRole,
) -> TeamResult<TeamInvite> {
    on_server(&app, server, false, move |teams| teams.invite(team, role)).await
}

#[tauri::command]
pub(crate) async fn team_invites(
    app: AppHandle,
    server: String,
    team: Uuid,
) -> TeamResult<Vec<TeamInvite>> {
    on_server(&app, server, false, move |teams| teams.invites(team)).await
}

#[tauri::command]
pub(crate) async fn team_revoke_invite(
    app: AppHandle,
    server: String,
    team: Uuid,
    invite: Uuid,
) -> TeamResult<()> {
    on_server(&app, server, false, move |teams| {
        teams.revoke_invite(team, invite)
    })
    .await
}

#[tauri::command]
pub(crate) async fn team_join(
    app: AppHandle,
    server: String,
    code: String,
    display_name: String,
) -> TeamResult<TeamInfo> {
    on_server(&app, server, true, move |teams| {
        teams.join(&code, &display_name)
    })
    .await
}

#[tauri::command]
pub(crate) async fn team_members(
    app: AppHandle,
    server: String,
    team: Uuid,
) -> TeamResult<Vec<TeamMember>> {
    on_server(&app, server, false, move |teams| teams.members(team)).await
}

/// Let a waiting member in. `fingerprint` is the one the person compared.
#[tauri::command]
pub(crate) async fn team_confirm(
    app: AppHandle,
    server: String,
    team: Uuid,
    account: Uuid,
    fingerprint: String,
) -> TeamResult<()> {
    on_server(&app, server, true, move |teams| {
        teams.confirm(team, account, &fingerprint)
    })
    .await
}

#[tauri::command]
pub(crate) async fn team_set_role(
    app: AppHandle,
    server: String,
    team: Uuid,
    account: Uuid,
    role: TeamRole,
) -> TeamResult<()> {
    on_server(&app, server, true, move |teams| {
        teams.set_role(team, account, role)
    })
    .await
}

#[derive(Serialize)]
pub(crate) struct Removed {
    /// The hosts whose stored logins the member may still know.
    hosts: Vec<HostRef>,
}

#[tauri::command]
pub(crate) async fn team_remove(
    app: AppHandle,
    server: String,
    team: Uuid,
    account: Uuid,
) -> TeamResult<Removed> {
    on_server(&app, server, true, move |teams| {
        teams.remove(team, account).map(|hosts| Removed { hosts })
    })
    .await
}
