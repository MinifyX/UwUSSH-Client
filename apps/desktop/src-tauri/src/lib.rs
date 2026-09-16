//! The Tauri host.
//!
//! This layer is thin on purpose: it turns IPC calls into `uwussh-core` calls
//! and pipes frames back. All the engine logic lives in the crates, so it can
//! be tested without a window — and so the one piece that is genuinely
//! Tauri-shaped, [`ChannelSink`], stays small enough to replace in a minute if
//! the M0 measurement says the IPC channel cannot keep up.

use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, State};
use uwussh_core::{FrameSink, MetricsSnapshot, SessionId, SessionManager, SinkError};

struct AppState {
    sessions: Arc<SessionManager>,
}

/// Terminal frames on their way to the webview.
///
/// This is the whole Tauri-specific surface of the data path. Everything else —
/// reading, coalescing, flow control — is transport-agnostic in `uwussh-core`.
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

type CommandResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[tauri::command]
async fn spawn_shell_session(
    state: State<'_, AppState>,
    cols: u16,
    rows: u16,
    on_data: Channel<InvokeResponseBody>,
) -> CommandResult<SessionId> {
    state
        .sessions
        .spawn_shell(cols, rows, ChannelSink { channel: on_data })
        .map_err(err)
}

#[tauri::command]
async fn write_session(
    state: State<'_, AppState>,
    id: SessionId,
    data: String,
) -> CommandResult<()> {
    state.sessions.write(id, data.as_bytes()).map_err(err)
}

#[tauri::command]
async fn resize_session(
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
async fn ack_session(state: State<'_, AppState>, id: SessionId, bytes: u64) -> CommandResult<()> {
    state.sessions.ack(id, bytes).map_err(err)
}

#[tauri::command]
async fn close_session(state: State<'_, AppState>, id: SessionId) -> CommandResult<()> {
    state.sessions.close(id).map_err(err)
}

#[tauri::command]
async fn session_metrics(
    state: State<'_, AppState>,
    id: SessionId,
) -> CommandResult<MetricsSnapshot> {
    state.sessions.metrics(id).map_err(err)
}

// ── M0 ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct M0Scenario {
    kind: M0Kind,
    flow_control: bool,
    payload_mib: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum M0Kind {
    /// Generated in Rust, no process, no ConPTY — the path SSH bytes take.
    Synthetic,
    /// `type bigfile.log` through ConPTY — the local shell path.
    Pty,
}

#[tauri::command]
async fn spawn_m0_session(
    state: State<'_, AppState>,
    scenario: M0Scenario,
    cols: u16,
    rows: u16,
    on_data: Channel<InvokeResponseBody>,
) -> CommandResult<SessionId> {
    let sink = ChannelSink { channel: on_data };
    let bytes = scenario.payload_mib as usize * 1024 * 1024;
    tracing::info!(?scenario, "M0 scenario starting");

    match scenario.kind {
        M0Kind::Synthetic => Ok(state
            .sessions
            .spawn_synthetic(bytes, scenario.flow_control, sink)),
        M0Kind::Pty => {
            let path = flood_file(bytes).map_err(err)?;
            let (program, args) = flood_command(&path);
            state
                .sessions
                .spawn_command(&program, &args, cols, rows, scenario.flow_control, sink)
                .map_err(err)
        }
    }
}

/// The log file for the PTY scenario, written once and reused.
fn flood_file(bytes: usize) -> std::io::Result<PathBuf> {
    let path =
        std::env::temp_dir().join(format!("uwussh-m0-flood-{}mib.log", bytes / (1024 * 1024)));
    let complete = std::fs::metadata(&path)
        .map(|m| m.len() as usize >= bytes)
        .unwrap_or(false);
    if !complete {
        uwussh_core::synthetic::write_flood_file(&path, bytes)?;
    }
    Ok(path)
}

fn flood_command(path: &Path) -> (String, Vec<String>) {
    let path = path.to_string_lossy().into_owned();
    if cfg!(windows) {
        ("cmd.exe".into(), vec!["/c".into(), "type".into(), path])
    } else {
        ("cat".into(), vec![path])
    }
}

/// Set `UWUSSH_M0_AUTORUN` to run the measurement on launch and quit.
#[tauri::command]
fn m0_autorun() -> bool {
    std::env::var_os("UWUSSH_M0_AUTORUN").is_some()
}

/// Write the report — to `UWUSSH_M0_REPORT` if set, else the temp directory —
/// and quit if this was an autorun. A file rather than stdout, because a
/// release build on Windows has no console to print to.
#[tauri::command]
async fn m0_finish(app: AppHandle, report: serde_json::Value) -> CommandResult<String> {
    let path = std::env::var_os("UWUSSH_M0_REPORT")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("uwussh-m0-report.json"));
    let text = serde_json::to_string_pretty(&report).map_err(err)?;
    std::fs::write(&path, text).map_err(err)?;
    tracing::info!(path = %path.display(), "M0 report written");

    if m0_autorun() {
        app.exit(0);
    }
    Ok(path.display().to_string())
}

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("UWUSSH_LOG").unwrap_or_else(|_| "uwussh=debug,warn".to_string()),
        )
        .init();

    tauri::Builder::default()
        .manage(AppState {
            sessions: Arc::new(SessionManager::new()),
        })
        .invoke_handler(tauri::generate_handler![
            spawn_shell_session,
            write_session,
            resize_session,
            ack_session,
            close_session,
            session_metrics,
            spawn_m0_session,
            m0_autorun,
            m0_finish,
        ])
        .run(tauri::generate_context!())
        .expect("failed to start UwUSSH");
}
