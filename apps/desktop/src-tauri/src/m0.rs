//! The M0 throughput measurement. See `docs/m0-spike.md`.

use crate::{err, AppState, ChannelSink, CommandResult};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, State};
use uwussh_core::SessionId;

const MAX_PAYLOAD_MIB: u32 = 256;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct M0Scenario {
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
pub(crate) async fn spawn_m0_session(
    state: State<'_, AppState>,
    scenario: M0Scenario,
    cols: u16,
    rows: u16,
    on_data: Channel<InvokeResponseBody>,
) -> CommandResult<SessionId> {
    // The measurement uses 64 MiB. Anything far beyond that is not a
    // measurement but a way to fill the disk with flood files.
    if !(1..=MAX_PAYLOAD_MIB).contains(&scenario.payload_mib) {
        return Err(format!("payload must be 1 to {MAX_PAYLOAD_MIB} MiB"));
    }
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
pub(crate) fn m0_autorun() -> bool {
    std::env::var_os("UWUSSH_M0_AUTORUN").is_some()
}

/// Write the report — to `UWUSSH_M0_REPORT` if set, else the temp directory —
/// and quit if this was an autorun. A file rather than stdout, because a
/// release build on Windows has no console to print to.
#[tauri::command]
pub(crate) async fn m0_finish(app: AppHandle, report: serde_json::Value) -> CommandResult<String> {
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
