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
    dir.join(setup_name(version))
}

/// Where the setup keeps what it installed (macOS and Linux; on Windows it is
/// the registry). The same path as in `apps/setup/src-tauri/src/install_unix.rs`.
#[cfg(not(windows))]
fn setup_record() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let config = if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute())
            .unwrap_or_else(|| home.join(".config"))
    };
    Some(config.join("app.uwussh.setup").join("install.json"))
}

/// Whether this copy of UwUSSH is one the setup put there, and so one the
/// setup may replace. On macOS and Linux an app from somewhere else — a
/// `.deb`, a copy out of a disk image — updates the way it came.
fn installed_by_setup() -> bool {
    #[cfg(windows)]
    {
        true
    }
    #[cfg(not(windows))]
    {
        #[derive(Deserialize)]
        struct Record {
            dir: PathBuf,
        }
        let (Some(path), Ok(me)) = (setup_record(), std::env::current_exe()) else {
            return false;
        };
        std::fs::read(path)
            .ok()
            .and_then(|raw| serde_json::from_slice::<Record>(&raw).ok())
            .is_some_and(|record| record.dir.is_absolute() && me.starts_with(&record.dir))
    }
}

/// The feed's platform key for this build, as `pnpm release` writes it.
fn feed_target() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    format!("{os}-{}", std::env::consts::ARCH)
}

/// A waiting update, opened and checked, with the file held so that nobody can
/// change or replace it until the handle is dropped.
struct Pending {
    update: ReadyUpdate,
    _locked: std::fs::File,
}

/// A waiting update, if it is where UwUSSH put it and still carries a valid
/// release signature for exactly that file name. `pending.json` lives in a
/// folder any program of the user can write to, so neither its path nor the
/// file is trusted blindly — and the file stays locked against writing and
/// deleting from the check until the setup has started.
fn open_pending(app: &AppHandle) -> Option<Pending> {
    let dir = updates_dir(app)?;
    let raw = std::fs::read(dir.join(PENDING)).ok()?;
    let update = serde_json::from_slice::<ReadyUpdate>(&raw).ok()?;
    semver::Version::parse(&update.version).ok()?;
    let expected = setup_file(&dir, &update.version);
    if update.file != expected {
        return None;
    }
    let mut file = open_locked(&expected).ok()?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut bytes).ok()?;
    let name = setup_name(&update.version);
    verify(app, &bytes, &update.signature, &name).then_some(Pending {
        update,
        _locked: file,
    })
}

/// Opens a file for reading while refusing everyone else write and delete
/// access. Starting it as a program still works: that only needs reading.
fn open_locked(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x1;
        options.share_mode(FILE_SHARE_READ);
    }
    options.open(path)
}

/// The file the release publishes for this platform's updater, and the name
/// its signature has to carry. Windows and Linux run the same file people
/// download; on macOS people get a disk image, and the updater the bare setup
/// program that is inside it.
fn setup_name(version: &str) -> String {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => format!("UwUSSH-Setup-{version}-macos-arm64-update"),
        ("macos", _) => format!("UwUSSH-Setup-{version}-macos-x64-update"),
        ("linux", "aarch64") => format!("UwUSSH-Setup-{version}-linux-arm64.AppImage"),
        ("linux", _) => format!("UwUSSH-Setup-{version}-linux-x64.AppImage"),
        _ => format!("UwUSSH-Setup-{version}.exe"),
    }
}

fn pubkey(app: &AppHandle) -> Option<String> {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|updater| updater.get("pubkey"))
        .and_then(|key| key.as_str())
        .map(str::to_string)
}

/// Checks a setup against the release key from `tauri.conf.json`.
fn verify(app: &AppHandle, bytes: &[u8], signature: &str, file_name: &str) -> bool {
    pubkey(app).is_some_and(|key| verify_with(&key, bytes, signature, file_name))
}

/// Both the key and the signature come base64-wrapped, the way Tauri's signer
/// writes them. The feed is not signed, only the setup is; the signature's
/// trusted comment names the file that was signed, so checking it ties the
/// version the feed claims to the version that was actually signed.
fn verify_with(pubkey: &str, bytes: &[u8], signature: &str, file_name: &str) -> bool {
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
        && signs_file(signature.trusted_comment(), file_name)
}

/// Tauri's signer writes `timestamp:<secs>\tfile:<name>` as the trusted comment.
fn signs_file(trusted_comment: &str, file_name: &str) -> bool {
    trusted_comment
        .split('\t')
        .filter_map(|part| part.trim().strip_prefix("file:"))
        .any(|name| name == file_name)
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

/// Starts the checked setup to replace this UwUSSH, which then quits. The file
/// stays locked until the setup process exists.
fn hand_over(pending: Pending, relaunch: bool) -> Result<(), String> {
    let update = &pending.update;
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
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&update.file, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("Couldn't start the update: {e}"))?;
    }
    let mut command = std::process::Command::new(&update.file);
    command.args(&args);
    // The Linux setup is an AppImage. Unpacked and run, it needs no FUSE,
    // which many systems no longer have.
    #[cfg(target_os = "linux")]
    command.env("APPIMAGE_EXTRACT_AND_RUN", "1");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group, so it lives on when UwUSSH quits.
        command.process_group(0);
    }
    let started = command
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Couldn't start the update: {e}"));
    drop(pending);
    started
}

/// Called first thing on start: installs a waiting update, or cleans up after
/// one. Returns true when UwUSSH must quit right away because the setup takes
/// over.
pub fn apply_pending_on_start(app: &AppHandle) -> bool {
    if cfg!(debug_assertions) || !installed_by_setup() {
        return false;
    }
    match open_pending(app) {
        Some(pending) if is_newer(app, &pending.update.version) => {
            // Another window still has sessions open; the update waits.
            !other_instances_running() && hand_over(pending, true).is_ok()
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
    if !installed_by_setup() {
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
        // The Linux setup brings its own WebKit and is large; a slow line
        // needs its time.
        .timeout(Duration::from_secs(15 * 60))
        .target(feed_target())
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

    // The plugin checks the signature against the public key before handing
    // out the bytes; the file name in it is checked here, against the version
    // the feed claims.
    let bytes = found.download(|_, _| {}, || {}).await.map_err(fail)?;
    let name = setup_name(&found.version);
    if !verify(app, &bytes, &found.signature, &name) {
        return Err(format!(
            "The update's signature doesn't belong to {name}, so it was not saved."
        ));
    }
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
    // Read it back from disk and keep it locked, so the signature is checked
    // on the file that runs.
    let pending = open_pending(app).ok_or("The downloaded update is damaged.")?;
    hand_over(pending, true)?;
    app.exit(0);
    Ok(())
}

/// Checks in the background for as long as UwUSSH runs.
pub fn start(app: &AppHandle) {
    app.manage(Updates::default());
    if let Some(pending) = open_pending(app).filter(|p| is_newer(app, &p.update.version)) {
        *app.state::<Updates>().ready.lock() = Some(pending.update);
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
        assert!(!verify_with("", b"setup", "", "UwUSSH-Setup-1.0.0.exe"));
        assert!(!verify_with(
            "bm90IGEga2V5",
            b"setup",
            "bm90IGEgc2lnbmF0dXJl",
            "UwUSSH-Setup-1.0.0.exe"
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
        assert!(!verify_with(
            pubkey,
            b"a setup nobody signed",
            "AAAA",
            "UwUSSH-Setup-1.0.0.exe"
        ));
    }

    #[test]
    fn the_signature_must_name_the_setup_the_feed_promised() {
        let comment = "timestamp:1789000000\tfile:UwUSSH-Setup-0.1.0-beta.2.exe";
        assert!(signs_file(comment, "UwUSSH-Setup-0.1.0-beta.2.exe"));
        // An older, validly signed setup offered as a newer version.
        assert!(!signs_file(comment, "UwUSSH-Setup-0.2.0.exe"));
        assert!(!signs_file(
            "timestamp:1789000000",
            "UwUSSH-Setup-0.2.0.exe"
        ));
    }

    #[test]
    fn only_the_file_uwussh_names_counts_as_a_setup() {
        let dir = Path::new("updates");
        let file = setup_file(dir, "0.1.0-beta.2");
        let name = file.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("UwUSSH-Setup-0.1.0-beta.2"), "{name}");
        #[cfg(windows)]
        assert_eq!(name, "UwUSSH-Setup-0.1.0-beta.2.exe");
        #[cfg(target_os = "linux")]
        assert!(name.ends_with(".AppImage"));
        #[cfg(target_os = "macos")]
        assert!(name.ends_with("-update"));
    }

    #[test]
    fn the_feed_key_is_the_one_the_release_writes() {
        let target = feed_target();
        assert!(
            ["windows-", "darwin-", "linux-"]
                .iter()
                .any(|os| target.starts_with(os)),
            "{target}"
        );
        #[cfg(all(windows, target_arch = "x86_64"))]
        assert_eq!(target, "windows-x86_64");
    }
}
