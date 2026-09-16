//! The Tauri host.
//!
//! This layer is thin on purpose: it turns IPC calls into `uwussh-core` calls
//! and pipes frames back. All the engine logic lives in the crates, so it can
//! be tested without a window — and so the one piece that is genuinely
//! Tauri-shaped, [`ChannelSink`], stays small enough to fix in a minute if the
//! IPC API moves under us.

use std::sync::Arc;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::State;
use uwussh_core::{FrameSink, MetricsSnapshot, SessionId, SessionManager, SinkError};

struct AppState {
    sessions: Arc<SessionManager>,
}

/// Terminal frames on their way to the webview.
///
/// This is the whole Tauri-specific surface of the data path. Everything else —
/// reading, coalescing, backpressure — is transport-agnostic in `uwussh-core`.
/// If the M0 measurement says the IPC channel cannot keep up, the fallback (a
/// local WebSocket on 127.0.0.1) replaces *this struct* and nothing else.
struct ChannelSink {
    channel: Channel<InvokeResponseBody>,
}

impl FrameSink for ChannelSink {
    fn send(&self, frame: &[u8]) -> Result<(), SinkError> {
        self.channel
            .send(InvokeResponseBody::Raw(frame.to_vec()))
            .map_err(|err| SinkError::Other(err.to_string()))
    }
}

#[tauri::command]
async fn spawn_local_session(
    state: State<'_, AppState>,
    cols: u16,
    rows: u16,
    on_data: Channel<InvokeResponseBody>,
) -> Result<SessionId, String> {
    state
        .sessions
        .spawn_local(cols, rows, ChannelSink { channel: on_data })
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn write_session(
    state: State<'_, AppState>,
    id: SessionId,
    data: String,
) -> Result<(), String> {
    state.sessions.write(id, data.as_bytes()).map_err(|e| e.to_string())
}

#[tauri::command]
async fn resize_session(
    state: State<'_, AppState>,
    id: SessionId,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    state.sessions.resize(id, cols, rows).map_err(|e| e.to_string())
}

#[tauri::command]
async fn close_session(state: State<'_, AppState>, id: SessionId) -> Result<(), String> {
    state.sessions.close(id).map_err(|e| e.to_string())
}

#[tauri::command]
async fn session_metrics(
    state: State<'_, AppState>,
    id: SessionId,
) -> Result<MetricsSnapshot, String> {
    state.sessions.metrics(id).map_err(|e| e.to_string())
}

/// The M0 load generator: a command that floods stdout, picked to match the
/// shell we actually spawned (Windows has no `yes`).
#[tauri::command]
async fn start_load_test(state: State<'_, AppState>, id: SessionId) -> Result<(), String> {
    let command = uwussh_core::pty::load_test_command();
    state.sessions.write(id, command.as_bytes()).map_err(|e| e.to_string())
}

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("UWUSSH_LOG").unwrap_or_else(|_| "uwussh=debug,warn".to_string()),
        )
        .init();

    tauri::Builder::default()
        .manage(AppState { sessions: Arc::new(SessionManager::new()) })
        .invoke_handler(tauri::generate_handler![
            spawn_local_session,
            write_session,
            resize_session,
            close_session,
            session_metrics,
            start_load_test,
        ])
        .run(tauri::generate_context!())
        .expect("failed to start UwUSSH");
}
