//! The Tauri host.
//!
//! This layer is thin on purpose: it turns IPC calls into calls on the crates
//! and pipes frames back. The engine lives in `uwussh-core`, the data in
//! `uwussh-store`, and both are tested without a window.
//!
//! - [`sessions`] — terminal I/O for any session
//! - [`hosts`] — the host list, connecting, and host key decisions
//! - [`m0`] — the throughput measurement

mod hosts;
mod import;
mod m0;
mod sessions;

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::Manager;
use uwussh_core::{FrameSink, ObservedHostKey, SessionManager, SinkError};
use uwussh_store::Store;

pub(crate) struct AppState {
    pub sessions: Arc<SessionManager>,
    pub store: Arc<Store>,
    /// Host keys a server presented in the last connection attempt, per
    /// address and port. Trusting a key is only possible for a key in here,
    /// so a compromised webview cannot hand in a key of its own choosing.
    pub presented_keys: Mutex<HashMap<(String, u16), ObservedHostKey>>,
}

/// Terminal frames on their way to the webview.
///
/// This is the whole Tauri-specific surface of the data path. Everything else —
/// reading, coalescing, flow control — is transport-agnostic in `uwussh-core`.
pub(crate) struct ChannelSink {
    pub channel: Channel<InvokeResponseBody>,
}

impl FrameSink for ChannelSink {
    fn send(&self, frame: &[u8]) -> Result<(), SinkError> {
        self.channel
            .send(InvokeResponseBody::Raw(frame.to_vec()))
            .map_err(|err| SinkError::Other(err.to_string()))
    }

    /// An empty frame is the end-of-stream marker; real frames are never empty.
    fn finish(&self) {
        let _ = self.channel.send(InvokeResponseBody::Raw(Vec::new()));
    }
}

pub(crate) type CommandResult<T> = Result<T, String>;

pub(crate) fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("UWUSSH_LOG").unwrap_or_else(|_| "uwussh=debug,warn".to_string()),
        )
        .init();

    tauri::Builder::default()
        .setup(|app| {
            // Same place as UwUMail keeps its database:
            // %APPDATA%\app.uwussh.desktop\uwussh.db on Windows.
            // UWUSSH_DB points elsewhere, so trying things out never touches
            // the real host list.
            let path = match std::env::var_os("UWUSSH_DB") {
                Some(path) => std::path::PathBuf::from(path),
                None => app.path().app_data_dir()?.join("uwussh.db"),
            };
            let store = Store::open(&path)?;
            tracing::info!(path = %path.display(), "store open");

            app.manage(AppState {
                sessions: Arc::new(SessionManager::new()),
                store: Arc::new(store),
                presented_keys: Mutex::new(HashMap::new()),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            sessions::spawn_shell_session,
            sessions::write_session,
            sessions::resize_session,
            sessions::ack_session,
            sessions::close_session,
            sessions::session_metrics,
            hosts::list_hosts,
            hosts::save_host,
            hosts::delete_host,
            hosts::connect_host,
            hosts::cancel_connect,
            hosts::trust_host_key,
            import::vault_status,
            import::create_vault,
            import::unlock_vault,
            import::lock_vault,
            import::available_imports,
            import::scan_import,
            import::run_import,
            m0::spawn_m0_session,
            m0::m0_autorun,
            m0::m0_finish,
        ])
        .run(tauri::generate_context!())
        .expect("failed to start UwUSSH");
}
