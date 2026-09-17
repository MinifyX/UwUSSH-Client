//! Automatic updates on Windows, the same way UwUMail does them.
//!
//! UwUSSH looks for a new version shortly after starting and every six hours,
//! downloads the signed `UwUSSH-Setup-<version>.exe` quietly and tells the page.
//! "Restart now" hands over to that setup in `--update` mode; otherwise the
//! update is applied the next time UwUSSH starts.
//!
//! Two channels, both feeds on the `updates` branch of the public repository:
//! `stable.json` carries plain versions only, `beta.json` carries betas too.
//! Only the setup is signed, not the feed, so the signature is checked when the
//! update is downloaded and again right before the setup runs, and the setup
//! itself refuses to install anything older than what is there.

use std::path::{Path, PathBuf};
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;

/// The `updates` branch of the public repo, written by `pnpm release`.
const FEED: &str = "https://raw.githubusercontent.com/MinifyX/UwUSSH-Client/updates";
const FIRST_CHECK_AFTER: Duration = Duration::from_secs(20);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
const PENDING: &str = "pending.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Stable,
    Beta,
}

/// Until the page says otherwise: a beta build stays on Beta, everything else on Stable.
impl Default for Channel {
    fn default() -> Self {
        if env!("CARGO_PKG_VERSION").contains('-') {
            Channel::Beta
        } else {
            Channel::Stable
        }
    }
}

/// A downloaded update waiting to be installed, as `pending.json` keeps it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadyUpdate {
    version: String,
    /// Release notes; JSON with `de` and `en` when written for UwUSSH.
    notes: Option<String>,
    /// The downloaded setup.
    file: PathBuf,
    /// The release signature, checked again right before the setup runs.
    signature: String,
}

/// What the page learns about a waiting update. No file paths.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub notes: Option<String>,
}

impl From<&ReadyUpdate> for UpdateInfo {
    fn from(update: &ReadyUpdate) -> Self {
        Self {
            version: update.version.clone(),
            notes: update.notes.clone(),
        }
    }
}

#[derive(Default)]
pub struct Updates {
    channel: Mutex<Channel>,
    ready: Mutex<Option<ReadyUpdate>>,
    checking: tokio::sync::Mutex<()>,
}

fn updates_dir(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_local_data_dir()
        .ok()
        .map(|dir| dir.join("updates"))
}

fn is_newer(app: &AppHandle, version: &str) -> bool {
    semver::Version::parse(version).is_ok_and(|v| v > app.package_info().version)
}

fn setup_file(dir: &Path, version: &str) -> PathBuf {
    dir.join(format!("UwUSSH-Setup-{version}.exe"))
}

/// A waiting update, if it is where UwUSSH put it and still carries a valid
/// release signature. `pending.json` lives in a folder any program of the user
/// can write to, so neither its path nor the file is trusted blindly.
fn read_pending(app: &AppHandle) -> Option<ReadyUpdate> {
    let dir = updates_dir(app)?;
    let raw = std::fs::read(dir.join(PENDING)).ok()?;
    let update = serde_json::from_slice::<ReadyUpdate>(&raw).ok()?;
    semver::Version::parse(&update.version).ok()?;
    let expected = setup_file(&dir, &update.version);
    if update.file != expected {
        return None;
    }
    let bytes = std::fs::read(&expected).ok()?;
    verify(app, &bytes, &update.signature).then_some(update)
}

/// Checks a setup against the release key from `tauri.conf.json`.
fn verify(app: &AppHandle, bytes: &[u8], signature: &str) -> bool {
    let Some(pubkey) = app
        .config()
        .plugins
        .0
        .get("updater")
        .and_then(|updater| updater.get("pubkey"))
        .and_then(|key| key.as_str())
    else {
        return false;
    };
    verify_with(pubkey, bytes, signature)
}

/// Both the key and the signature come base64-wrapped, the way Tauri's signer
/// writes them.
fn verify_with(pubkey: &str, bytes: &[u8], signature: &str) -> bool {
    use base64::Engine as _;
    let decode = |text: &str| {
        base64::engine::general_purpose::STANDARD
            .decode(text.trim())
            .ok()
            .and_then(|raw| String::from_utf8(raw).ok())
    };
    let (Some(pubkey), Some(signature)) = (decode(pubkey), decode(signature)) else {
        return false;
    };
    let (Ok(key), Ok(signature)) = (
        minisign_verify::PublicKey::decode(&pubkey),
        minisign_verify::Signature::decode(&signature),
    ) else {
        return false;
    };
    key.verify(bytes, &signature, false).is_ok()
}

/// Whether another UwUSSH window is running from the same program file. Its
/// sessions would die with the update, so nothing hands over while it runs.
#[cfg(windows)]
fn other_instances_running() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let Ok(me) = std::env::current_exe() else {
        return false;
    };
    let wanted = me.to_string_lossy().to_lowercase();
    let own_pid = std::process::id();
    let path_of = |pid: u32| -> Option<String> {
        // SAFETY: plain Win32 calls with a buffer we own; the handle is closed.
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                return None;
            }
            let mut buffer = [0u16; 1024];
            let mut size = buffer.len() as u32;
            let ok = QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                buffer.as_mut_ptr(),
                &mut size,
            );
            CloseHandle(process);
            (ok != 0).then(|| String::from_utf16_lossy(&buffer[..size as usize]).to_lowercase())
        }
    };

    // SAFETY: the snapshot handle is checked and closed; the entry's size is set.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = false;
        let mut more = Process32FirstW(snapshot, &mut entry) != 0;
        while more && !found {
            if entry.th32ProcessID != own_pid
                && path_of(entry.th32ProcessID).as_deref() == Some(&*wanted)
            {
                found = true;
            }
            more = Process32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
        found
    }
}

#[cfg(not(windows))]
fn other_instances_running() -> bool {
    false
}

/// Starts the downloaded setup to replace this UwUSSH, which then quits.
fn hand_over(update: &ReadyUpdate, relaunch: bool) -> Result<(), String> {
    // Each download gets one attempt. If the setup refuses (e.g. an older
    // version), the next start must not hand over again and again.
    if let Some(dir) = update.file.parent() {
        let _ = std::fs::remove_file(dir.join(PENDING));
    }
    let pid = std::process::id().to_string();
    let mut args = vec!["--update", "--wait-pid", pid.as_str()];
    if relaunch {
        args.push("--relaunch");
    }
    std::process::Command::new(&update.file)
        .args(&args)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Couldn't start the update: {e}"))
}

/// Called first thing on start: installs a waiting update, or cleans up after
/// one. Returns true when UwUSSH must quit right away because the setup takes
/// over.
pub fn apply_pending_on_start(app: &AppHandle) -> bool {
    if !cfg!(windows) || cfg!(debug_assertions) {
        return false;
    }
    match read_pending(app) {
        Some(update) if is_newer(app, &update.version) => {
            // Another window still has sessions open; the update waits.
            !other_instances_running() && hand_over(&update, true).is_ok()
        }
        _ => {
            if let Some(dir) = updates_dir(app) {
                let _ = std::fs::remove_dir_all(dir);
            }
            false
        }
    }
}

pub fn set_channel(app: &AppHandle, channel: Channel) {
    let state = app.state::<Updates>();
    let mut current = state.channel.lock();
    if *current != channel {
        *current = channel;
        tracing::info!(?channel, "update channel");
    }
}

pub fn ready(app: &AppHandle) -> Option<UpdateInfo> {
    app.state::<Updates>()
        .ready
        .lock()
        .as_ref()
        .map(UpdateInfo::from)
}

/// Looks for a newer version and downloads it. Returns the waiting update, if any.
pub async fn check(app: &AppHandle) -> Result<Option<UpdateInfo>, String> {
    let state = app.state::<Updates>();
    let _one_at_a_time = state.checking.lock().await;
    if let Some(update) = ready(app) {
        return Ok(Some(update));
    }
    if !cfg!(windows) {
        return Ok(None);
    }
    let channel = *state.channel.lock();
    let feed = match channel {
        Channel::Stable => format!("{FEED}/stable.json"),
        Channel::Beta => format!("{FEED}/beta.json"),
    };
    let fail = |e: tauri_plugin_updater::Error| format!("Update check failed: {e}");
    let updater = app
        .updater_builder()
        .endpoints(vec![feed
            .parse()
            .map_err(|_| "Bad update address".to_string())?])
        .map_err(fail)?
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(fail)?;
    let Some(found) = updater.check().await.map_err(fail)? else {
        return Ok(None);
    };
    if semver::Version::parse(&found.version).is_err() {
        return Err(format!(
            "The update feed names a bad version: {}",
            found.version
        ));
    }

    // The plugin checks the signature against the public key before handing out the bytes.
    let bytes = found.download(|_, _| {}, || {}).await.map_err(fail)?;
    let dir = updates_dir(app).ok_or("No folder for updates")?;
    let save = |e: std::io::Error| format!("Couldn't save the update: {e}");
    std::fs::create_dir_all(&dir).map_err(save)?;
    let file = setup_file(&dir, &found.version);
    std::fs::write(&file, &bytes).map_err(save)?;
    let update = ReadyUpdate {
        version: found.version.clone(),
        notes: found.body.clone(),
        file,
        signature: found.signature.clone(),
    };
    let json = serde_json::to_vec(&update).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(PENDING), json).map_err(save)?;

    let info = UpdateInfo::from(&update);
    *state.ready.lock() = Some(update);
    let _ = app.emit("update:ready", &info);
    Ok(Some(info))
}

/// "Restart now".
pub fn install_now(app: &AppHandle) -> Result<(), String> {
    if cfg!(debug_assertions) {
        return Err("Development builds don't install updates.".into());
    }
    if ready(app).is_none() {
        return Err("There's no update waiting.".into());
    }
    if other_instances_running() {
        return Err(
            "Another UwUSSH window is still open. Close it first, so its connections aren't cut off."
                .into(),
        );
    }
    // Read it back from disk, so the signature is checked on the file that runs.
    let update = read_pending(app).ok_or("The downloaded update is damaged.")?;
    hand_over(&update, true)?;
    app.exit(0);
    Ok(())
}

/// Checks in the background for as long as UwUSSH runs.
pub fn start(app: &AppHandle) {
    app.manage(Updates::default());
    if let Some(update) = read_pending(app).filter(|update| is_newer(app, &update.version)) {
        *app.state::<Updates>().ready.lock() = Some(update);
    }
    if cfg!(debug_assertions) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK_AFTER).await;
        loop {
            if let Err(error) = check(&app).await {
                tracing::info!("{error}");
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_beta_build_starts_on_the_beta_channel() {
        let expected = if env!("CARGO_PKG_VERSION").contains('-') {
            Channel::Beta
        } else {
            Channel::Stable
        };
        assert_eq!(Channel::default(), expected);
        assert_eq!(serde_json::to_string(&Channel::Beta).unwrap(), "\"beta\"");
    }

    #[test]
    fn garbage_never_verifies() {
        assert!(!verify_with("", b"setup", ""));
        assert!(!verify_with(
            "bm90IGEga2V5",
            b"setup",
            "bm90IGEgc2lnbmF0dXJl"
        ));
    }

    #[test]
    fn a_setup_signed_with_another_key_is_refused() {
        // The shipped key, and a signature that is well-formed but belongs to
        // nothing this key signed.
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let pubkey = conf["plugins"]["updater"]["pubkey"].as_str().unwrap();
        assert!(!pubkey.is_empty());
        assert!(!verify_with(pubkey, b"a setup nobody signed", "AAAA"));
    }

    #[test]
    fn only_the_file_uwussh_names_counts_as_a_setup() {
        let dir = Path::new(r"C:\Users\nyu\AppData\Local\app.uwussh.desktop\updates");
        assert_eq!(
            setup_file(dir, "0.1.0-beta.2"),
            dir.join("UwUSSH-Setup-0.1.0-beta.2.exe")
        );
    }
}
