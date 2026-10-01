//! Tunnels: saving them, starting and stopping them, and telling the page.
//!
//! A tunnel started from the tunnel manager logs in like a terminal — host key
//! first, then the stored or asked-for secret, the same questions answered the
//! same way — on a connection of its own, which every tunnel of that host
//! started there shares. Tunnels marked to start with their host start on a
//! terminal's connection instead, when it opens, and stop when it closes.
//!
//! Every change of a running tunnel reaches the page as a `tunnel:status`
//! event; [`tunnel_statuses`] is the whole picture for a page that just loaded.

use crate::hosts::{
    check_attempt, internal, note_presented_key, prepare, ConnectFailure, Prepared, SaveFailure,
};
use crate::{err, AppState, CommandResult};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;
use uwussh_core::{SessionId, TunnelError, TunnelKind, TunnelSpec, TunnelStatus};
use uwussh_store::{TunnelDraft, TunnelRecord};
use zeroize::Zeroizing;

/// The listener the tunnel manager reports to: every change goes to the page.
pub(crate) fn listener(app: AppHandle) -> uwussh_core::TunnelListener {
    std::sync::Arc::new(move |status: TunnelStatus| {
        let _ = app.emit("tunnel:status", status);
    })
}

#[tauri::command]
pub(crate) fn list_tunnels(state: State<'_, AppState>) -> CommandResult<Vec<TunnelRecord>> {
    state.store.list_tunnels().map_err(err)
}

#[tauri::command]
pub(crate) fn save_tunnel(
    state: State<'_, AppState>,
    draft: TunnelDraft,
) -> Result<TunnelRecord, SaveFailure> {
    Ok(state.store.save_tunnel(draft)?)
}

/// Delete a tunnel; a running one stops first.
#[tauri::command]
pub(crate) async fn delete_tunnel(state: State<'_, AppState>, id: Uuid) -> CommandResult<()> {
    state.tunnels.stop(id).await;
    state.store.delete_tunnel(id).map_err(err)
}

#[tauri::command]
pub(crate) async fn tunnel_statuses(state: State<'_, AppState>) -> Result<Vec<TunnelStatus>, ()> {
    Ok(state.tunnels.statuses().await)
}

/// Why a tunnel did not start: a question about logging in, or the tunnel's
/// own trouble (a port in use, a server that won't listen). Both carry their
/// own `kind` tag.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum StartFailure {
    Connect(ConnectFailure),
    Tunnel(TunnelError),
}

impl From<ConnectFailure> for StartFailure {
    fn from(failure: ConnectFailure) -> Self {
        Self::Connect(failure)
    }
}

fn spec_of(tunnel: &TunnelRecord) -> Result<TunnelSpec, ConnectFailure> {
    let kind = TunnelKind::parse(&tunnel.kind)
        .ok_or_else(|| internal("this kind of tunnel needs a newer UwUSSH"))?;
    Ok(TunnelSpec {
        kind,
        bind_address: tunnel.bind_address.clone(),
        bind_port: tunnel.bind_port,
        target_host: tunnel.target_host.clone(),
        target_port: tunnel.target_port,
    })
}

/// Start a tunnel without a terminal. `attempt` names the conversation, like a
/// tab's: a question for the user keeps the verified connection waiting under
/// it, and the next call with the answer continues on it.
#[tauri::command]
pub(crate) async fn start_tunnel(
    state: State<'_, AppState>,
    id: Uuid,
    attempt: String,
    secret: Option<String>,
) -> Result<TunnelStatus, StartFailure> {
    check_attempt(&attempt)?;
    let tunnel = state
        .store
        .get_tunnel(id)
        .map_err(internal)?
        .ok_or_else(|| internal("this tunnel no longer exists"))?;
    let spec = spec_of(&tunnel)?;

    let link = match state.tunnels.dedicated_link(tunnel.host_id) {
        Some(link) => link,
        None => {
            let Prepared { host, target, .. } =
                prepare(&state, tunnel.host_id, secret.map(Zeroizing::new))?;
            match state.sessions.open_link(&attempt, target).await {
                Ok(link) => {
                    if let Err(error) = state.store.mark_connected(host.id) {
                        tracing::warn!(%error, "could not record the connection time");
                    }
                    state.tunnels.keep_dedicated(host.id, &link);
                    link
                }
                Err(error) => {
                    note_presented_key(&state, &host, &error);
                    return Err(ConnectFailure::Ssh(error).into());
                }
            }
        }
    };
    state
        .tunnels
        .start(id, tunnel.host_id, None, link, spec)
        .await
        .map_err(StartFailure::Tunnel)
}

/// Stop a tunnel, or clear one that failed.
#[tauri::command]
pub(crate) async fn stop_tunnel(state: State<'_, AppState>, id: Uuid) -> Result<bool, ()> {
    Ok(state.tunnels.stop(id).await)
}

/// A terminal to `host` just opened: start the host's tunnels that start with
/// it, on that terminal's connection. One that already runs is left alone; one
/// that fails says so through its status, like any other.
pub(crate) fn autostart(app: &AppHandle, host: Uuid, session: SessionId) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let tunnels = match state.store.list_tunnels() {
            Ok(tunnels) => tunnels,
            Err(error) => {
                tracing::warn!(%error, "could not read the tunnels to start");
                return;
            }
        };
        let running = state.tunnels.statuses().await;
        for tunnel in tunnels
            .iter()
            .filter(|tunnel| tunnel.host_id == host && tunnel.autostart)
        {
            let busy = running.iter().any(|status| {
                status.id == tunnel.id
                    && matches!(
                        status.state,
                        uwussh_core::TunnelState::Running { .. }
                            | uwussh_core::TunnelState::Starting
                    )
            });
            if busy {
                continue;
            }
            let Ok(spec) = spec_of(tunnel) else { continue };
            let Some(link) = state.sessions.ssh_link(session) else {
                return;
            };
            if let Err(error) = state
                .tunnels
                .start(tunnel.id, host, Some(session), link, spec)
                .await
            {
                tracing::info!(id = %tunnel.id, %error, "a tunnel did not start with its terminal");
            }
        }
    });
}
