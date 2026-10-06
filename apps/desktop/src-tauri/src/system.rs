//! App-level commands: updates, links out of the app, and a fresh start for a
//! page that (re)loaded.

#[cfg(feature = "self-update")]
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
    // Tunnels on the old page's terminals stop with them; the ones on a
    // connection of their own keep running, and the new page lists them.
    state.tunnels.stop_terminals().await;
    Ok(state.sessions.close_all())
}

// The update commands exist in every build, so a page that asks anyway gets
// an answer instead of an unknown command. Without `self-update` (the Mac App
// Store build, where the store updates the app) they find nothing and install
// nothing; the page hides its update UI there (`updatesAvailableInApp()`).

#[tauri::command]
pub(crate) fn set_update_channel(app: AppHandle, channel: Channel) {
    #[cfg(feature = "self-update")]
    updates::set_channel(&app, channel);
    #[cfg(not(feature = "self-update"))]
    let _ = (app, channel);
}

/// A downloaded update waiting for a restart, if any.
#[tauri::command]
pub(crate) fn update_status(app: AppHandle) -> Option<UpdateInfo> {
    #[cfg(feature = "self-update")]
    return updates::ready(&app);
    #[cfg(not(feature = "self-update"))]
    {
        let _ = app;
        None
    }
}

#[tauri::command]
pub(crate) async fn check_for_updates(app: AppHandle) -> CommandResult<Option<UpdateInfo>> {
    #[cfg(feature = "self-update")]
    return updates::check(&app).await;
    #[cfg(not(feature = "self-update"))]
    {
        let _ = app;
        Ok(None)
    }
}

/// Async: a Linux package waits for the password prompt and the package
/// manager, which must not hold up the main thread.
#[tauri::command]
pub(crate) async fn install_update(app: AppHandle) -> CommandResult<()> {
    #[cfg(feature = "self-update")]
    return updates::install_now(&app).await;
    #[cfg(not(feature = "self-update"))]
    {
        let _ = app;
        Err("this copy of UwUSSH is updated by the App Store".into())
    }
}

/// What the update commands take and give in a build without an updater: the
/// same channel names, and an update that never exists.
#[cfg(not(feature = "self-update"))]
mod no_updates {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "lowercase")]
    pub(crate) enum Channel {
        Stable,
        Beta,
    }

    #[derive(serde::Serialize)]
    pub(crate) enum UpdateInfo {}
}
#[cfg(not(feature = "self-update"))]
use no_updates::{Channel, UpdateInfo};

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

/// Which build this is: `"github"` for every download from GitHub (it updates
/// itself and has the local shell), `"app-store"` for the Mac App Store build
/// (the `mas` feature: no updater, no local shell, the sandbox decides what it
/// may open). The page asks once, before its first render (`lib/flavor.ts`).
#[tauri::command]
pub(crate) fn app_flavor() -> &'static str {
    if cfg!(feature = "mas") {
        "app-store"
    } else {
        "github"
    }
}
