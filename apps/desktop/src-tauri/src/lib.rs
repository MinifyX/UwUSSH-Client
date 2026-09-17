//! The Tauri host.
//!
//! This layer is thin on purpose: it turns IPC calls into calls on the crates
//! and pipes frames back. The engine lives in `uwussh-core`, the data in
//! `uwussh-store`, and both are tested without a window.
//!
//! - [`sessions`] — terminal I/O for any session
//! - [`hosts`] — the host list, groups, connecting, host key decisions
//! - [`files`] — the file browser: SFTP, this computer, SMB shares
//! - [`keys`] — keys in the vault
//! - [`keygen`] — UwUKeygen, shared with the standalone app
//! - [`import`] — the vault and importing other clients' setups
//! - [`backup`] — exporting to and importing from `.uwussh` files
//! - [`system`] — updates, links, a fresh start for a reloaded page
//! - [`m0`] — the throughput measurement

mod backup;
mod device;
mod dialogs;
mod files;
mod hosts;
mod import;
mod keygen;
mod keys;
mod m0;
mod sessions;
mod system;
mod updates;

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::Manager;
use uwussh_core::{FrameSink, ObservedHostKey, SessionId, SessionManager, SinkError};
use uwussh_store::Store;

pub(crate) struct AppState {
    pub sessions: Arc<SessionManager>,
    pub store: Arc<Store>,
    /// Host keys a server presented in the last connection attempt, per
    /// address and port. Trusting a key is only possible for a key in here,
    /// so a compromised webview cannot hand in a key of its own choosing. Each
    /// entry expires, so a key the user declined doesn't stay trustable.
    pub presented_keys: Mutex<HashMap<(String, u16), (ObservedHostKey, std::time::Instant)>>,
    /// The password each open terminal logged in with, for typing it when
    /// `sudo` asks. Wiped when the terminal closes.
    pub session_passwords: Mutex<HashMap<SessionId, zeroize::Zeroizing<String>>>,
    /// Which host each open terminal belongs to.
    pub session_hosts: Mutex<HashMap<SessionId, uuid::Uuid>>,
    pub transfers: files::Transfers,
    pub picked_export: backup::PickedExport,
    pub picked_key: keys::Picked,
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
    system::restrict_dll_search();

    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("UWUSSH_LOG").unwrap_or_else(|_| "uwussh=debug,warn".to_string()),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            if updates::apply_pending_on_start(app.handle()) {
                // The downloaded setup replaces this version and starts UwUSSH again.
                std::process::exit(0);
            }

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
            // A vault this device keeps the key for opens right away, so the
            // master password is a once-per-device thing.
            if let Err(error) = store.unlock_remembered_vault(device::unprotect) {
                tracing::warn!(%error, "could not open the vault with this device's key");
            }

            app.manage(AppState {
                sessions: Arc::new(SessionManager::new()),
                store: Arc::new(store),
                presented_keys: Mutex::new(HashMap::new()),
                session_passwords: Mutex::new(HashMap::new()),
                session_hosts: Mutex::new(HashMap::new()),
                transfers: files::Transfers::default(),
                picked_export: backup::PickedExport::default(),
                picked_key: keys::Picked::default(),
            });
            app.manage(keygen::Generated::default());
            updates::start(app.handle());
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
            hosts::set_host_password,
            hosts::list_groups,
            hosts::create_group,
            hosts::rename_group,
            hosts::delete_group,
            hosts::move_group,
            hosts::move_host,
            hosts::connect_host,
            hosts::cancel_connect,
            hosts::trust_host_key,
            hosts::session_can_type_password,
            hosts::type_session_password,
            files::open_files,
            files::close_files,
            files::remote_list,
            files::remote_canonicalize,
            files::remote_mkdir,
            files::remote_rename,
            files::remote_remove,
            files::remote_chmod,
            files::transfer,
            files::cancel_transfer,
            files::local_places,
            files::local_list,
            files::local_parent,
            files::local_mkdir,
            files::local_rename,
            files::local_trash,
            files::local_copy,
            files::smb_connect,
            keys::list_keys,
            keys::rename_key,
            keys::delete_key,
            keys::pick_key_file,
            keys::import_picked_key,
            keys::forget_picked_key,
            keys::export_key_file,
            keys::key_public_line,
            keys::keygen_store,
            keygen::keygen_generate,
            keygen::keygen_encode,
            keygen::keygen_copy_private,
            keygen::keygen_save,
            keygen::keygen_save_public,
            keygen::keygen_discard,
            import::vault_status,
            import::vault_state,
            import::create_vault,
            import::unlock_vault,
            import::set_vault_remembered,
            import::lock_vault,
            import::available_imports,
            import::scan_import,
            import::run_import,
            backup::export_hosts,
            backup::pick_export_file,
            backup::read_export_file,
            backup::import_export_file,
            system::close_all_sessions,
            system::set_update_channel,
            system::update_status,
            system::check_for_updates,
            system::install_update,
            system::open_project_page,
            system::open_terminal_link,
            m0::spawn_m0_session,
            m0::m0_autorun,
            m0::m0_finish,
        ])
        .run(tauri::generate_context!())
        .expect("failed to start UwUSSH");
}
