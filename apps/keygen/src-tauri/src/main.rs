//! UwUKeygen: the key generator from UwUSSH, in a window of its own.
//!
//! The commands and the dialogs are UwUSSH's own files, included by path, so
//! the two can never drift apart. There is no vault and no store here: a key
//! leaves as a file (OpenSSH, PuTTY, PEM, `.pub`) or through the clipboard.

// No console window next to it in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[path = "../../../desktop/src-tauri/src/dialogs.rs"]
#[allow(dead_code)]
mod dialogs;
#[path = "../../../desktop/src-tauri/src/keygen.rs"]
mod keygen;

fn main() {
    restrict_dll_search();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(keygen::Generated::default())
        .invoke_handler(tauri::generate_handler![
            keygen::keygen_generate,
            keygen::keygen_encode,
            keygen::keygen_copy_private,
            keygen::keygen_save,
            keygen::keygen_save_public,
            keygen::keygen_discard,
        ])
        .run(tauri::generate_context!())
        .expect("failed to start UwUKeygen");
}

/// DLLs loaded by name at runtime come from System32 only — the runtime half
/// of `/DEPENDENTLOADFLAG` in `build.rs`. Before anything else loads one.
fn restrict_dll_search() {
    #[cfg(windows)]
    // SAFETY: a process-wide flag, set once before any other thread exists.
    unsafe {
        use windows_sys::Win32::System::LibraryLoader::{
            SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_SYSTEM32,
        };
        SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32);
    }
}
