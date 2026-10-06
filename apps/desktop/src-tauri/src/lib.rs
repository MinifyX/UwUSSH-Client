//! The Tauri host.
//!
//! This layer is thin on purpose: it turns IPC calls into calls on the crates
//! and pipes frames back. The engine lives in `uwussh-core`, the data in
//! `uwussh-store`, and both are tested without a window.
//!
//! - [`sessions`] — terminal I/O for any session
//! - [`assist`] — the command assistant: a request in words to one command
//! - [`hosts`] — the host list, groups, connecting, host key decisions
//! - [`files`] — the file browser: SFTP, this computer, SMB shares
//! - [`tunnels`] — local and remote port forwards
//! - [`keys`] — keys in the vault
//! - [`keygen`] — UwUKeygen, shared with the standalone app
//! - [`import`] — the vault and importing other clients' setups
//! - [`backup`] — exporting to and importing from `.uwussh` files
//! - [`sync`] — Settings → Sync and the thread that keeps devices in step
//! - [`lock`] — the same through UwULock: signing in, the move from UwUSync,
//!   the realtime channel
//! - [`system`] — updates, links, a fresh start for a reloaded page
//! - [`updates`] — finding, downloading and installing new versions; only in
//!   builds with the `self-update` feature, which the Mac App Store's is not
//! - [`links`] — `uwussh://connect/<host id>` links
//! - [`sandbox_access`] — the Mac App Store sandbox's bookmarks, so folders
//!   and files the person picked stay reachable after a restart
//! - [`m0`] — the throughput measurement
//!
//! The macOS menu bar is the page's (`setMacMenu` in App.tsx).

mod assist;
mod backup;
mod device;
mod dialogs;
mod files;
mod hosts;
mod import;
mod keygen;
mod keys;
mod links;
mod lock;
mod m0;
mod sandbox_access;
mod sessions;
mod sync;
mod system;
mod tunnels;
#[cfg(feature = "self-update")]
mod updates;

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::Manager;
use uwussh_core::{
    FrameSink, ObservedHostKey, SessionId, SessionManager, SinkError, TunnelManager,
};
use uwussh_store::Store;

pub(crate) struct AppState {
    pub sessions: Arc<SessionManager>,
    pub store: Arc<Store>,
    /// Running tunnels, with or without a terminal.
    pub tunnels: Arc<TunnelManager>,
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
    /// The folder the last "sessions from a folder" import picked. The page
    /// never names a path itself; it can only read what the person chose.
    pub picked_folder: Mutex<Option<std::path::PathBuf>>,
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

    let builder = tauri::Builder::default();

    // One UwUSSH per user, registered first: a second start hands its link
    // over and quits before anything else of it runs. A run with a database
    // of its own (UWUSSH_DB, for trying things out and the end-to-end tests)
    // stays a separate window.
    //
    // Not in the Mac App Store build: on a Mac, Launch Services already keeps
    // an app bundle to one copy and hands a `uwussh://` link to the running
    // one (the deep-link plugin gets it either way), and the plugin's socket
    // in /tmp is outside what the sandbox lets the app create.
    let single_instance =
        std::env::var_os("UWUSSH_DB").is_none() && !cfg!(all(target_os = "macos", feature = "mas"));
    let builder = if single_instance {
        builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            links::second_instance(app)
        }))
    } else {
        builder
    };

    let builder = builder
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init());
    // The updater plugin's own commands are in no capability, so the page
    // cannot reach them; the feed stays what tauri.conf.json says.
    #[cfg(feature = "self-update")]
    let builder = builder.plugin(tauri_plugin_updater::Builder::new().build());

    builder
        .setup(|app| {
            std::thread::spawn(sync::device_name);
            // macOS ends an app without asking the window; this asks the page
            // first (`onMacQuit` in App.tsx, answered through `finish_quit`),
            // which asks the person while connections are open.
            #[cfg(target_os = "macos")]
            {
                use tauri::Emitter;
                let handle = app.handle().clone();
                if let Err(error) = uwu_macos::install_quit_guard(move || {
                    handle.emit(uwu_macos::QUIT_EVENT, ()).is_ok()
                }) {
                    tracing::warn!(%error, "quit guard");
                }
            }

            #[cfg(feature = "self-update")]
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
            // Before anything reads a key file or the page lists a folder: the
            // folders and files the person picked in an earlier run, next to
            // the database so UWUSSH_DB moves them too.
            if let Some(directory) = path.parent() {
                sandbox_access::restore(directory);
            }
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
                tunnels: Arc::new(TunnelManager::new(tunnels::listener(app.handle().clone()))),
                presented_keys: Mutex::new(HashMap::new()),
                session_passwords: Mutex::new(HashMap::new()),
                session_hosts: Mutex::new(HashMap::new()),
                transfers: files::Transfers::default(),
                picked_export: backup::PickedExport::default(),
                picked_key: keys::Picked::default(),
                picked_folder: Mutex::new(None),
            });
            app.manage(keygen::Generated::default());
            #[cfg(feature = "self-update")]
            updates::start(app.handle());
            sync::start(app.handle());
            links::setup(app);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            finish_quit,
            quit_app,
            sessions::spawn_shell_session,
            sessions::write_session,
            sessions::resize_session,
            sessions::ack_session,
            sessions::close_session,
            sessions::session_metrics,
            assist::assist_platform,
            assist::assist_generate,
            assist::assist_type_command,
            assist::assist_settings,
            assist::assist_save_settings,
            assist::assist_models,
            assist::assist_detect_ollama,
            assist::assist_cache_list,
            assist::assist_cache_delete,
            assist::assist_cache_clear,
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
            files::local_pick_folder,
            files::local_forget_folder,
            files::smb_connect,
            tunnels::list_tunnels,
            tunnels::save_tunnel,
            tunnels::delete_tunnel,
            tunnels::tunnel_statuses,
            tunnels::start_tunnel,
            tunnels::stop_tunnel,
            keys::list_keys,
            keys::rename_key,
            keys::delete_key,
            keys::pick_key_file,
            keys::pick_key_path,
            keys::grant_ssh_folder,
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
            import::repair_vault,
            import::set_vault_remembered,
            import::lock_vault,
            import::available_imports,
            import::pick_import_folder,
            import::scan_import,
            import::run_import,
            backup::export_hosts,
            backup::pick_export_file,
            backup::read_export_file,
            backup::import_export_file,
            sync::sync_status,
            sync::sync_connect,
            sync::sync_join,
            sync::sync_offer,
            sync::sync_wait_for_device,
            sync::sync_cancel_offer,
            sync::sync_devices,
            sync::sync_revoke,
            sync::sync_now,
            sync::sync_pass_now,
            sync::sync_disconnect,
            sync::sync_recovery_code,
            lock::lock_sign_in,
            lock::lock_send_email_code,
            lock::lock_move,
            lock::lock_leave_uwusync,
            lock::lock_forget_move,
            lock::lock_app_sync_off,
            lock::lock_sign_out,
            system::app_flavor,
            system::close_all_sessions,
            system::set_update_channel,
            system::update_status,
            system::check_for_updates,
            system::install_update,
            system::open_project_page,
            system::open_terminal_link,
            links::take_link,
            m0::spawn_m0_session,
            m0::m0_autorun,
            m0::m0_finish,
        ])
        .build(tauri::generate_context!())
        .expect("failed to start UwUSSH")
        .run(|app, event| {
            // macOS: ⌘W and the red light only hid the window (App.tsx); a
            // click on the Dock icon brings it back.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen {
                has_visible_windows: false,
                ..
            } = event
            {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app, event);
        });
}

/// The page's answer to a quit from the Dock, ⌘Q or a logout (macOS): go ahead
/// or stay. Does nothing elsewhere.
#[tauri::command]
fn finish_quit(proceed: bool) {
    uwu_macos::reply_quit(proceed);
}

/// Ends the app after the person said yes to quitting with connections open.
/// On macOS the quit question answered "stay" first, so macOS is not waiting
/// for anything; this ends the app the way closing its last window would.
#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    app.exit(0);
}
