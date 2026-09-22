//! App-level commands: updates, links out of the app, and a fresh start for a
//! page that (re)loaded.

use crate::updates::{self, Channel, UpdateInfo};
use crate::{AppState, CommandResult};
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

/// A page just started. Whatever an earlier page left open can't be reached
/// from here any more, so it goes.
// Async on purpose: closing spawns a task on the runtime, and sync commands
// run on the main thread, outside it.
#[tauri::command]
pub(crate) async fn close_all_sessions(state: State<'_, AppState>) -> Result<usize, ()> {
    state.presented_keys.lock().clear();
    state.transfers.cancel_all();
    state.session_passwords.lock().clear();
    state.session_hosts.lock().clear();
    Ok(state.sessions.close_all())
}

#[tauri::command]
pub(crate) fn set_update_channel(app: AppHandle, channel: Channel) {
    updates::set_channel(&app, channel);
}

/// A downloaded update waiting for a restart, if any.
#[tauri::command]
pub(crate) fn update_status(app: AppHandle) -> Option<UpdateInfo> {
    updates::ready(&app)
}

#[tauri::command]
pub(crate) async fn check_for_updates(app: AppHandle) -> CommandResult<Option<UpdateInfo>> {
    updates::check(&app).await
}

/// Async: a Linux package waits for the password prompt and the package
/// manager, which must not hold up the main thread.
#[tauri::command]
pub(crate) async fn install_update(app: AppHandle) -> CommandResult<()> {
    updates::install_now(&app).await
}

/// The project pages the app links to. The page names one; it never hands in
/// an address of its own.
#[tauri::command]
pub(crate) fn open_project_page(app: AppHandle, page: String) -> CommandResult<()> {
    let url = match page.as_str() {
        "source" => "https://github.com/MinifyX/UwUSSH-Client",
        "releases" => "https://github.com/MinifyX/UwUSSH-Client/releases",
        "issues" => "https://github.com/MinifyX/UwUSSH-Client/issues",
        "license" => "https://www.gnu.org/licenses/gpl-3.0.html",
        _ => return Err(format!("unknown page: {page}")),
    };
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| format!("Couldn't open the browser: {e}"))
}

/// A link a program in the terminal printed (OSC 8), after a Ctrl+click. The
/// remote side chooses the address, so only plain web links go to the browser:
/// no `file:`, no `ms-settings:`, no other protocol handler on this machine.
#[tauri::command]
pub(crate) fn open_terminal_link(app: AppHandle, url: String) -> CommandResult<()> {
    if !is_web_link(&url) {
        return Err("only http and https links open from the terminal".into());
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| format!("Couldn't open the browser: {e}"))
}

fn is_web_link(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"));
    url.len() <= 2048
        && rest.is_some_and(|rest| !rest.is_empty() && !rest.starts_with('/'))
        && !url
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '"')
}

/// On Windows, DLLs loaded by name at runtime resolve from System32 only, never
/// the install folder or PATH. The runtime half of `/DEPENDENTLOADFLAG` in
/// `build.rs`, which only covers statically imported DLLs. Must run before
/// anything else loads a DLL.
pub(crate) fn restrict_dll_search() {
    #[cfg(windows)]
    // SAFETY: a process-wide flag, set once before any other thread exists.
    unsafe {
        use windows_sys::Win32::System::LibraryLoader::{
            SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_SYSTEM32,
        };
        SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32);
    }
}

#[cfg(test)]
mod tests {
    use super::is_web_link;

    #[test]
    fn only_web_links_leave_the_terminal() {
        assert!(is_web_link("https://github.com/MinifyX/UwUSSH-Client"));
        assert!(is_web_link("HTTP://example.org/a?b=c"));
        for bad in [
            "file:///C:/Windows/System32/calc.exe",
            "ms-settings:privacy",
            "javascript:alert(1)",
            "https://",
            "https:///etc/passwd",
            "https://example.org/\" & calc",
            "https://exa mple.org",
            r"\\attacker\share",
        ] {
            assert!(!is_web_link(bad), "{bad}");
        }
    }
}
