//! Native file dialogs, opened from Rust.
//!
//! The page asks for "a file to export to", never hands in a path of its own
//! for things that write secrets, and gets back only what it needs to show.
//!
//! Debug builds read two environment variables instead of showing a dialog, so
//! the end-to-end run can export and import without clicking through Windows:
//! `UWUSSH_E2E_SAVE_DIR` (a save goes there, under the suggested name) and
//! `UWUSSH_E2E_OPEN_FILE` (an open picks that file), and `UWUSSH_E2E_OPEN_FOLDER`
//! for a folder. Release builds ignore them.

use std::path::PathBuf;
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

pub(crate) struct Filter<'a> {
    pub name: &'a str,
    pub extensions: &'a [&'a str],
}

pub(crate) async fn save(
    app: &AppHandle,
    title: &str,
    file_name: &str,
    filter: Filter<'_>,
) -> Option<PathBuf> {
    if cfg!(debug_assertions) {
        if let Some(dir) = std::env::var_os("UWUSSH_E2E_SAVE_DIR") {
            return Some(PathBuf::from(dir).join(file_name));
        }
    }
    let mut dialog = app
        .dialog()
        .file()
        .set_title(title)
        .set_file_name(file_name);
    // A filter makes Windows append its extension; a file that has none (an
    // OpenSSH key) gets no filter, or it would be saved as "id_ed25519.*".
    if !filter.extensions.is_empty() {
        dialog = dialog.add_filter(filter.name, filter.extensions);
    }
    tauri::async_runtime::spawn_blocking(move || dialog.blocking_save_file())
        .await
        .ok()
        .flatten()
        .and_then(|path| path.into_path().ok())
}

pub(crate) async fn folder(app: &AppHandle, title: &str) -> Option<PathBuf> {
    if cfg!(debug_assertions) {
        if let Some(dir) = std::env::var_os("UWUSSH_E2E_OPEN_FOLDER") {
            return Some(PathBuf::from(dir));
        }
    }
    let dialog = app.dialog().file().set_title(title);
    tauri::async_runtime::spawn_blocking(move || dialog.blocking_pick_folder())
        .await
        .ok()
        .flatten()
        .and_then(|path| path.into_path().ok())
}

pub(crate) async fn open(app: &AppHandle, title: &str, filter: Filter<'_>) -> Option<PathBuf> {
    if cfg!(debug_assertions) {
        if let Some(file) = std::env::var_os("UWUSSH_E2E_OPEN_FILE") {
            return Some(PathBuf::from(file));
        }
    }
    let dialog = app
        .dialog()
        .file()
        .set_title(title)
        .add_filter(filter.name, filter.extensions)
        .add_filter("Alle Dateien", &["*"]);
    tauri::async_runtime::spawn_blocking(move || dialog.blocking_pick_file())
        .await
        .ok()
        .flatten()
        .and_then(|path| path.into_path().ok())
}
