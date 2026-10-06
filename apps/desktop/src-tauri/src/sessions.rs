//! Terminal I/O, the same for every kind of session.

use crate::{err, AppState, ChannelSink, CommandResult};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::State;
use uwussh_core::{MetricsSnapshot, SessionId};

/// Why the Mac App Store build starts no program on this computer.
pub(crate) const NO_LOCAL_PROCESSES: &str =
    "the App Store version of UwUSSH runs no programs on this Mac, only SSH connections";

/// A local shell. Not in the Mac App Store build: a shell started from a
/// sandboxed app inherits its sandbox and sees little more than the app's own
/// container, which is no use as a terminal — the page offers no local tab
/// there (`localShellAvailable()`), and this refuses in case it asks anyway.
#[tauri::command]
pub(crate) async fn spawn_shell_session(
    state: State<'_, AppState>,
    cols: u16,
    rows: u16,
    on_data: Channel<InvokeResponseBody>,
) -> CommandResult<SessionId> {
    if cfg!(feature = "mas") {
        return Err(NO_LOCAL_PROCESSES.into());
    }
    state
        .sessions
        .spawn_shell(cols, rows, ChannelSink { channel: on_data })
        .map_err(err)
}

/// Deliberately not `async`. Async commands run concurrently on the runtime,
/// so two quick keystrokes could overtake each other. Synchronous commands run
/// on the main thread in the order they were invoked, and all this does is put
/// the bytes on the session's input queue, so it never blocks.
#[tauri::command]
pub(crate) fn write_session(
    state: State<'_, AppState>,
    id: SessionId,
    data: String,
) -> CommandResult<()> {
    state.sessions.write(id, data.as_bytes()).map_err(err)
}

/// Synchronous for the same reason as [`write_session`]: a resize must not
/// overtake the keystrokes typed before it.
#[tauri::command]
pub(crate) fn resize_session(
    state: State<'_, AppState>,
    id: SessionId,
    cols: u16,
    rows: u16,
) -> CommandResult<()> {
    state.sessions.resize(id, cols, rows).map_err(err)
}

/// The renderer has processed `bytes` more bytes. This is what lets the engine
/// pause before the webview drowns.
#[tauri::command]
pub(crate) async fn ack_session(
    state: State<'_, AppState>,
    id: SessionId,
    bytes: u64,
) -> CommandResult<()> {
    state.sessions.ack(id, bytes).map_err(err)
}

#[tauri::command]
pub(crate) async fn close_session(state: State<'_, AppState>, id: SessionId) -> CommandResult<()> {
    crate::hosts::forget_session(&state, id);
    // The tunnels that started with this terminal run on its connection.
    state.tunnels.stop_session(id).await;
    state.sessions.close(id).map_err(err)
}

#[tauri::command]
pub(crate) async fn session_metrics(
    state: State<'_, AppState>,
    id: SessionId,
) -> CommandResult<MetricsSnapshot> {
    state.sessions.metrics(id).map_err(err)
}
