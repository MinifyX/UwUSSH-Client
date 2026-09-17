//! The file browser: this computer on the left, a server on the right.
//!
//! The server side is SFTP on a connection of its own, opened through the same
//! conversation as a terminal (host key, password, vault) — plus, for root,
//! sudo's password. The local side is plain file system access. Transfers run
//! in the background and report progress over a channel; each can be
//! cancelled.
//!
//! SMB shares are the local side's business: Windows speaks SMB itself, so a
//! share is a `\\server\share` path once [`smb_connect`] has signed in to it
//! with the host's user and a password typed for it. Never the stored SSH
//! password: SMB has nothing like a host key, so it would go to whoever
//! answers to the name.

use crate::hosts::{check_attempt, from_vault, internal, note_presented_key, prepare, Prepared};
use crate::AppState;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};
use uuid::Uuid;
use uwussh_core::{CancelToken, Elevation, Entry, EntryKind, FilesError, SessionId, SftpError};
use zeroize::Zeroizing;

use crate::hosts::ConnectFailure;

/// Transfers that are running, by the id the page gave them.
#[derive(Default)]
pub(crate) struct Transfers(Mutex<HashMap<String, CancelToken>>);

impl Transfers {
    /// Stop every running transfer: the page that started them is gone.
    pub(crate) fn cancel_all(&self) {
        for (_, token) in self.0.lock().drain() {
            token.cancel();
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OpenedFiles {
    session: SessionId,
    home: Option<String>,
    root: bool,
}

/// Open file access on a host. With `root`, sudo's password is, in this
/// order: the one the user typed for it, the host's stored password, or the
/// password the login itself used.
#[tauri::command]
pub(crate) async fn open_files(
    state: State<'_, AppState>,
    id: Uuid,
    attempt: String,
    secret: Option<String>,
    root: bool,
    sudo_password: Option<String>,
) -> Result<OpenedFiles, ConnectFailure> {
    check_attempt(&attempt)?;
    let Prepared {
        host,
        target,
        login_password,
    } = prepare(&state, id, secret.map(Zeroizing::new))?;

    let elevation = if root {
        let stored = if host.has_password {
            Some(crate::hosts::as_secret(from_vault(
                state.store.reveal_host_password(host.id),
            )?)?)
        } else {
            None
        };
        Elevation::Root {
            sudo_password: sudo_password
                .map(Zeroizing::new)
                .or(stored)
                .or(login_password),
        }
    } else {
        Elevation::User
    };

    match state.sessions.open_files(&attempt, target, elevation).await {
        Ok((session, home)) => Ok(OpenedFiles {
            session,
            home,
            root,
        }),
        Err(FilesError::Ssh(error)) => {
            note_presented_key(&state, &host, &error);
            Err(ConnectFailure::Ssh(error))
        }
        Err(FilesError::Sftp(error)) => Err(ConnectFailure::Files(error)),
    }
}

// Async on purpose: closing spawns a task on the runtime, and sync commands
// run on the main thread, outside it.
#[tauri::command]
pub(crate) async fn close_files(state: State<'_, AppState>, session: SessionId) -> Result<(), ()> {
    state.sessions.close_files(session);
    Ok(())
}

fn client(
    state: &AppState,
    session: SessionId,
) -> Result<Arc<uwussh_core::FileSession>, SftpError> {
    state
        .sessions
        .files(session)
        .map_err(|_| SftpError::Failed {
            message: "the connection to the server is closed".into(),
        })
}

#[tauri::command]
pub(crate) async fn remote_list(
    state: State<'_, AppState>,
    session: SessionId,
    path: String,
) -> Result<Vec<Entry>, SftpError> {
    client(&state, session)?.client.list(&path).await
}

#[tauri::command]
pub(crate) async fn remote_canonicalize(
    state: State<'_, AppState>,
    session: SessionId,
    path: String,
) -> Result<String, SftpError> {
    client(&state, session)?.client.canonicalize(&path).await
}

#[tauri::command]
pub(crate) async fn remote_mkdir(
    state: State<'_, AppState>,
    session: SessionId,
    path: String,
) -> Result<(), SftpError> {
    client(&state, session)?.client.mkdir(&path).await
}

#[tauri::command]
pub(crate) async fn remote_rename(
    state: State<'_, AppState>,
    session: SessionId,
    from: String,
    to: String,
) -> Result<(), SftpError> {
    client(&state, session)?.client.rename(&from, &to).await
}

#[tauri::command]
pub(crate) async fn remote_remove(
    state: State<'_, AppState>,
    session: SessionId,
    paths: Vec<String>,
) -> Result<(), SftpError> {
    let files = client(&state, session)?;
    for path in paths {
        files.remove(&path).await?;
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn remote_chmod(
    state: State<'_, AppState>,
    session: SessionId,
    path: String,
    mode: u32,
) -> Result<(), SftpError> {
    client(&state, session)?.client.chmod(&path, mode).await
}

// ── Transfers ───────────────────────────────────────────────────────────────

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum TransferEvent {
    #[serde(rename_all = "camelCase")]
    Started {
        total_bytes: u64,
    },
    #[serde(rename_all = "camelCase")]
    Progress {
        done_bytes: u64,
    },
    #[serde(rename_all = "camelCase")]
    Item {
        name: String,
    },
    Done,
    Failed {
        error: SftpError,
    },
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Direction {
    Download,
    Upload,
}

fn check_transfer_id(id: &str) -> Result<(), SftpError> {
    let plain = !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if plain {
        Ok(())
    } else {
        Err(SftpError::Failed {
            message: "invalid transfer id".into(),
        })
    }
}

/// Copy `sources` into `target`: remote paths into a local folder
/// (`download`), or local paths into a remote folder (`upload`). Returns
/// immediately; the channel tells how it goes.
///
/// Without `overwrite`, a source whose name is already taken in `target`
/// fails the whole transfer with `already-exists` before anything is
/// written, so the page can ask.
// A Tauri command takes its arguments flat, as the page sends them.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub(crate) fn transfer(
    app: AppHandle,
    state: State<'_, AppState>,
    transfer: String,
    session: SessionId,
    direction: Direction,
    sources: Vec<String>,
    target: String,
    overwrite: bool,
    events: Channel<TransferEvent>,
) -> Result<(), SftpError> {
    check_transfer_id(&transfer)?;
    let files = client(&state, session)?;
    if direction == Direction::Download {
        check_local_path(&target)?;
    } else {
        for source in &sources {
            check_local_path(source)?;
        }
    }
    let token = CancelToken::default();
    state
        .transfers
        .0
        .lock()
        .insert(transfer.clone(), token.clone());

    tauri::async_runtime::spawn(async move {
        let done = Arc::new(AtomicU64::new(0));
        let result = async {
            if !overwrite {
                for source in &sources {
                    let taken = match direction {
                        Direction::Download => {
                            let local =
                                uwussh_core::sftp::download_target(source, Path::new(&target))?;
                            tokio::fs::symlink_metadata(&local)
                                .await
                                .is_ok()
                                .then(|| local.display().to_string())
                        }
                        Direction::Upload => {
                            let name = Path::new(source)
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or_default();
                            let remote = uwussh_core::sftp::join(&target, name);
                            files.client.exists(&remote).await.then_some(remote)
                        }
                    };
                    if let Some(path) = taken {
                        return Err(SftpError::AlreadyExists { path });
                    }
                }
            }
            let mut total: u64 = 0;
            for source in &sources {
                total = total.saturating_add(match direction {
                    Direction::Download => files.client.size_of(source, token.flag()).await?,
                    Direction::Upload => local_size(Path::new(source)),
                });
            }
            let _ = events.send(TransferEvent::Started { total_bytes: total });
            let progress = {
                let done = Arc::clone(&done);
                let events = events.clone();
                let last = AtomicU64::new(0);
                move |bytes: u64| {
                    let now = done.fetch_add(bytes, Ordering::Relaxed) + bytes;
                    // Every 256 KiB is plenty for a progress bar.
                    if now - last.load(Ordering::Relaxed) >= 256 * 1024 {
                        last.store(now, Ordering::Relaxed);
                        let _ = events.send(TransferEvent::Progress { done_bytes: now });
                    }
                }
            };
            for source in &sources {
                let name = source
                    .trim_end_matches(['/', '\\'])
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or(source)
                    .to_string();
                let _ = events.send(TransferEvent::Item { name });
                match direction {
                    Direction::Download => {
                        files
                            .client
                            .download(
                                source,
                                Path::new(&target),
                                overwrite,
                                token.flag(),
                                &progress,
                            )
                            .await?;
                    }
                    Direction::Upload => {
                        files
                            .client
                            .upload(
                                Path::new(source),
                                &target,
                                overwrite,
                                token.flag(),
                                &progress,
                            )
                            .await?;
                    }
                }
            }
            Ok::<_, SftpError>(())
        }
        .await;
        let _ = events.send(TransferEvent::Progress {
            done_bytes: done.load(Ordering::Relaxed),
        });
        let _ = events.send(match result {
            Ok(()) => TransferEvent::Done,
            Err(error) => TransferEvent::Failed { error },
        });
        app.state::<AppState>().transfers.0.lock().remove(&transfer);
    });
    Ok(())
}

#[tauri::command]
pub(crate) fn cancel_transfer(state: State<'_, AppState>, transfer: String) {
    if let Some(token) = state.transfers.0.lock().remove(&transfer) {
        token.cancel();
    }
}

fn local_size(path: &Path) -> u64 {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if !metadata.is_dir() {
        return metadata.len();
    }
    std::fs::read_dir(path)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| local_size(&entry.path()))
                .sum()
        })
        .unwrap_or(0)
}

// ── This computer ───────────────────────────────────────────────────────────

/// A local path must be absolute. `\\server\share` is allowed here: the user
/// browses to it, or opened an SMB share on purpose.
fn check_local_path(path: &str) -> Result<(), SftpError> {
    if Path::new(path).is_absolute() && !path.chars().any(char::is_control) {
        Ok(())
    } else {
        Err(SftpError::Local {
            message: format!("not an absolute path: {path}"),
        })
    }
}

fn local_error(error: std::io::Error) -> SftpError {
    SftpError::Local {
        message: error.to_string(),
    }
}

/// A local file error at `path`, with "already exists" named as such.
fn local_at(path: &Path, error: std::io::Error) -> SftpError {
    if error.kind() == std::io::ErrorKind::AlreadyExists {
        SftpError::AlreadyExists {
            path: path.display().to_string(),
        }
    } else {
        local_error(error)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Place {
    label: String,
    path: String,
    kind: &'static str,
}

/// Where the local side can start: home, the usual folders, and every drive.
#[tauri::command]
pub(crate) fn local_places(app: AppHandle) -> Vec<Place> {
    let mut places = Vec::new();
    let paths = app.path();
    for (label, kind, path) in [
        ("Home", "home", paths.home_dir()),
        ("Desktop", "desktop", paths.desktop_dir()),
        ("Documents", "documents", paths.document_dir()),
        ("Downloads", "downloads", paths.download_dir()),
    ] {
        if let Ok(path) = path {
            if path.is_dir() {
                places.push(Place {
                    label: label.into(),
                    path: path.display().to_string(),
                    kind,
                });
            }
        }
    }
    #[cfg(windows)]
    for letter in b'A'..=b'Z' {
        let root = format!("{}:\\", letter as char);
        if Path::new(&root).is_dir() {
            places.push(Place {
                label: root.clone(),
                path: root,
                kind: "drive",
            });
        }
    }
    #[cfg(not(windows))]
    places.push(Place {
        label: "/".into(),
        path: "/".into(),
        kind: "drive",
    });
    places
}

#[tauri::command]
pub(crate) async fn local_list(path: String) -> Result<Vec<Entry>, SftpError> {
    check_local_path(&path)?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&path).map_err(local_error)? {
            let Ok(entry) = entry else { continue };
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let link = entry.file_type().is_ok_and(|t| t.is_symlink());
            let name = entry.file_name().to_string_lossy().into_owned();
            let kind = if metadata.is_dir() {
                EntryKind::Dir
            } else if metadata.is_file() {
                EntryKind::File
            } else {
                EntryKind::Other
            };
            entries.push(Entry {
                path: entry.path().display().to_string(),
                name,
                kind,
                size: if metadata.is_file() {
                    metadata.len()
                } else {
                    0
                },
                modified_ms: metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as u64),
                permissions: None,
                owner: None,
                link,
            });
        }
        entries.sort_by(|a, b| {
            (a.kind != EntryKind::Dir, a.name.to_lowercase())
                .cmp(&(b.kind != EntryKind::Dir, b.name.to_lowercase()))
        });
        Ok(entries)
    })
    .await
    .map_err(|e| SftpError::Local {
        message: e.to_string(),
    })?
}

/// The folder above `path`, or nothing at the top of a drive.
#[tauri::command]
pub(crate) fn local_parent(path: String) -> Option<String> {
    Path::new(&path)
        .parent()
        .map(|parent| parent.display().to_string())
        .filter(|parent| !parent.is_empty())
}

#[tauri::command]
pub(crate) fn local_mkdir(path: String) -> Result<(), SftpError> {
    check_local_path(&path)?;
    std::fs::create_dir(&path).map_err(local_error)
}

#[tauri::command]
pub(crate) fn local_rename(from: String, to: String) -> Result<(), SftpError> {
    check_local_path(&from)?;
    check_local_path(&to)?;
    if Path::new(&to).exists() {
        return Err(SftpError::AlreadyExists { path: to });
    }
    std::fs::rename(&from, &to).map_err(local_error)
}

/// Local files go to the recycle bin, not into nothing.
#[tauri::command]
pub(crate) async fn local_trash(paths: Vec<String>) -> Result<(), SftpError> {
    for path in &paths {
        check_local_path(path)?;
    }
    tauri::async_runtime::spawn_blocking(move || {
        trash::delete_all(paths.iter().map(PathBuf::from)).map_err(|e| SftpError::Local {
            message: e.to_string(),
        })
    })
    .await
    .map_err(|e| SftpError::Local {
        message: e.to_string(),
    })?
}

/// Copy local files or folders (an SMB share included) into a local folder.
/// `overwrite` as for [`transfer`].
#[tauri::command]
pub(crate) fn local_copy(
    app: AppHandle,
    state: State<'_, AppState>,
    transfer: String,
    sources: Vec<String>,
    target: String,
    overwrite: bool,
    events: Channel<TransferEvent>,
) -> Result<(), SftpError> {
    check_transfer_id(&transfer)?;
    check_local_path(&target)?;
    for source in &sources {
        check_local_path(source)?;
        if is_within(Path::new(&target), Path::new(source)) {
            return Err(SftpError::Local {
                message: "a folder can't be copied into itself".into(),
            });
        }
    }
    let token = CancelToken::default();
    state
        .transfers
        .0
        .lock()
        .insert(transfer.clone(), token.clone());
    tauri::async_runtime::spawn_blocking(move || {
        let total = sources.iter().map(|s| local_size(Path::new(s))).sum();
        let _ = events.send(TransferEvent::Started { total_bytes: total });
        let mut done = 0u64;
        let result = (|| {
            let mut names = Vec::new();
            for source in &sources {
                let source = Path::new(source);
                let name = source.file_name().ok_or_else(|| SftpError::Local {
                    message: "the file has no name".into(),
                })?;
                let to = Path::new(&target).join(name);
                if !overwrite && std::fs::symlink_metadata(&to).is_ok() {
                    return Err(SftpError::AlreadyExists {
                        path: to.display().to_string(),
                    });
                }
                names.push((source, name, to));
            }
            for (source, name, to) in names {
                let _ = events.send(TransferEvent::Item {
                    name: name.to_string_lossy().into_owned(),
                });
                copy_tree(source, &to, overwrite, &token, &mut done, &events)?;
            }
            Ok(())
        })();
        let _ = events.send(TransferEvent::Progress { done_bytes: done });
        let _ = events.send(match result {
            Ok(()) => TransferEvent::Done,
            Err(error) => TransferEvent::Failed { error },
        });
        app.state::<AppState>().transfers.0.lock().remove(&transfer);
    });
    Ok(())
}

/// How deep a local copy goes: deeper than any real tree, and far short of
/// what would overflow a thread's stack.
const COPY_DEPTH: usize = 128;

/// Whether `inner` is `outer` or somewhere inside it, as Windows sees it:
/// case doesn't matter, and a mapped drive is the share it maps.
fn is_within(inner: &Path, outer: &Path) -> bool {
    let (Ok(inner), Ok(outer)) = (std::fs::canonicalize(inner), std::fs::canonicalize(outer))
    else {
        return false;
    };
    let parts = |path: &Path| -> Vec<String> {
        path.components()
            .map(|part| part.as_os_str().to_string_lossy().to_lowercase())
            .collect()
    };
    parts(&inner).starts_with(&parts(&outer))
}

/// Copy a file or a folder tree, without following links or junctions.
fn copy_tree(
    source: &Path,
    target: &Path,
    overwrite: bool,
    token: &CancelToken,
    done: &mut u64,
    events: &Channel<TransferEvent>,
) -> Result<(), SftpError> {
    let mut pending = vec![(source.to_path_buf(), target.to_path_buf(), 0usize)];
    while let Some((from, to, depth)) = pending.pop() {
        if token.flag().load(Ordering::Relaxed) {
            return Err(SftpError::Cancelled);
        }
        let metadata = std::fs::symlink_metadata(&from).map_err(local_error)?;
        if metadata.is_dir() {
            if depth >= COPY_DEPTH {
                return Err(SftpError::Local {
                    message: format!("{} is nested too deep to copy", from.display()),
                });
            }
            if let Err(error) = std::fs::create_dir(&to) {
                let merge = overwrite && std::fs::symlink_metadata(&to).is_ok_and(|m| m.is_dir());
                if !merge {
                    return Err(local_at(&to, error));
                }
            }
            for entry in std::fs::read_dir(&from).map_err(local_error)? {
                let entry = entry.map_err(local_error)?;
                pending.push((entry.path(), to.join(entry.file_name()), depth + 1));
            }
            continue;
        }
        if metadata.is_file() {
            copy_file(&from, &to, overwrite, token, done, events)?;
        }
    }
    Ok(())
}

fn copy_file(
    source: &Path,
    target: &Path,
    overwrite: bool,
    token: &CancelToken,
    done: &mut u64,
    events: &Channel<TransferEvent>,
) -> Result<(), SftpError> {
    use std::io::{Read, Write};
    let mut input = std::fs::File::open(source).map_err(local_error)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true);
    if overwrite {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    let mut output = options.open(target).map_err(|e| local_at(target, e))?;
    let mut buffer = vec![0u8; 256 * 1024];
    let copied = (|| loop {
        if token.flag().load(Ordering::Relaxed) {
            return Err(SftpError::Cancelled);
        }
        let read = input.read(&mut buffer).map_err(local_error)?;
        if read == 0 {
            return Ok(());
        }
        output.write_all(&buffer[..read]).map_err(local_error)?;
        *done += read as u64;
        let _ = events.send(TransferEvent::Progress { done_bytes: *done });
    })();
    if copied.is_err() {
        drop(output);
        let _ = std::fs::remove_file(target);
    }
    copied
}

// ── SMB ─────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SmbShare {
    path: String,
}

/// Sign in to `\\host\share` with the host's user and the password typed for
/// it — empty for none, as a guest — so the local side can browse it.
#[tauri::command]
pub(crate) async fn smb_connect(
    state: State<'_, AppState>,
    id: Uuid,
    share: String,
    password: String,
) -> Result<SmbShare, ConnectFailure> {
    let share = share.trim().trim_matches(['/', '\\']).to_string();
    if share.is_empty()
        || share
            .chars()
            .any(|c| c.is_control() || "\\/:*?\"<>|".contains(c))
    {
        return Err(internal("that is not a share name"));
    }
    let host = state
        .store
        .get_host(id)
        .map_err(internal)?
        .ok_or_else(|| internal("this host no longer exists"))?;
    if !smb_server_name(&host.address) {
        return Err(internal("the host address can't name an SMB server"));
    }
    let password = Some(Zeroizing::new(password));
    let path = format!(r"\\{}\{}", host.address, share);
    let user = host.username.clone();
    let remote = path.clone();
    tauri::async_runtime::spawn_blocking(move || smb_sign_in(&remote, &user, password))
        .await
        .map_err(internal)?
        .map_err(|message| ConnectFailure::Files(SftpError::Refused { reason: message }))?;
    Ok(SmbShare { path })
}

/// A host name or an IPv4 address, and nothing Windows would read as more:
/// no `@SSL`, port or path.
fn smb_server_name(address: &str) -> bool {
    !address.is_empty()
        && address.len() <= 253
        && !address.starts_with(['.', '-'])
        && address
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

#[cfg(windows)]
fn smb_sign_in(
    remote: &str,
    user: &str,
    password: Option<Zeroizing<String>>,
) -> Result<(), String> {
    use windows_sys::Win32::NetworkManagement::WNet::{
        WNetAddConnection2W, NETRESOURCEW, RESOURCETYPE_DISK,
    };
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let mut remote_w = wide(remote);
    let user_w = wide(user);
    let password_w = password.as_ref().map(|p| Zeroizing::new(wide(p)));
    let resource = NETRESOURCEW {
        dwScope: 0,
        dwType: RESOURCETYPE_DISK,
        dwDisplayType: 0,
        dwUsage: 0,
        lpLocalName: std::ptr::null_mut(),
        lpRemoteName: remote_w.as_mut_ptr(),
        lpComment: std::ptr::null_mut(),
        lpProvider: std::ptr::null_mut(),
    };
    // SAFETY: every pointer is a NUL-terminated UTF-16 buffer that outlives
    // the call.
    let code = unsafe {
        WNetAddConnection2W(
            &resource,
            password_w.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
            user_w.as_ptr(),
            0,
        )
    };
    // 0: signed in. 1219: already connected to this server with other
    // credentials, which Windows will keep using — fine for browsing.
    match code {
        0 | 1219 => Ok(()),
        86 | 1326 => Err("the server did not accept the user or password".into()),
        53 | 67 => Err("the server or the share was not found".into()),
        other => Err(std::io::Error::from_raw_os_error(other as i32).to_string()),
    }
}

#[cfg(not(windows))]
fn smb_sign_in(
    _remote: &str,
    _user: &str,
    _password: Option<Zeroizing<String>>,
) -> Result<(), String> {
    Err("SMB shares need Windows for now".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_absolute_local_paths_are_used() {
        assert!(check_local_path("relative\\x").is_err());
        #[cfg(windows)]
        {
            assert!(check_local_path(r"C:\Users").is_ok());
            assert!(check_local_path(r"\\nas\share").is_ok());
        }
        assert!(check_local_path("/tmp/\u{7}").is_err());
    }

    #[test]
    fn transfer_ids_are_plain() {
        assert!(check_transfer_id("t-1_a").is_ok());
        assert!(check_transfer_id("").is_err());
        assert!(check_transfer_id("../x").is_err());
    }

    #[test]
    fn a_folder_is_never_copied_into_itself() {
        let root = std::env::temp_dir().join(format!("uwussh-copy-{}", Uuid::now_v7()));
        std::fs::create_dir_all(root.join("inside")).unwrap();
        assert!(is_within(&root, &root));
        assert!(is_within(&root.join("inside"), &root));
        #[cfg(windows)]
        assert!(
            is_within(&root.join("INSIDE"), &root),
            "the same folder in other letters"
        );
        assert!(!is_within(&root, &root.join("inside")));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_copy_replaces_only_when_asked() {
        let root = std::env::temp_dir().join(format!("uwussh-copy-{}", Uuid::now_v7()));
        std::fs::create_dir_all(root.join("from/inner")).unwrap();
        std::fs::write(root.join("from/inner/a.txt"), "new").unwrap();
        std::fs::create_dir_all(root.join("to/inner")).unwrap();
        std::fs::write(root.join("to/inner/a.txt"), "old").unwrap();
        let token = CancelToken::default();
        let mut done = 0;
        let channel: Channel<TransferEvent> = Channel::new(|_| Ok(()));

        let refused = copy_tree(
            &root.join("from"),
            &root.join("to"),
            false,
            &token,
            &mut done,
            &channel,
        );
        assert!(matches!(refused, Err(SftpError::AlreadyExists { .. })));
        copy_tree(
            &root.join("from"),
            &root.join("to"),
            true,
            &token,
            &mut done,
            &channel,
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("to/inner/a.txt")).unwrap(),
            "new"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_smb_server_is_a_plain_name() {
        for good in ["nas", "nas.lan", "192.168.1.20", "files-01.example.org"] {
            assert!(smb_server_name(good), "{good}");
        }
        for bad in [
            "",
            "nas@SSL@443",
            "nas:445",
            "nas\\x",
            "-nas",
            ".nas",
            "fe80::1",
        ] {
            assert!(!smb_server_name(bad), "{bad}");
        }
    }
}
