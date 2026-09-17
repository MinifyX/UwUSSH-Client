//! Files over SSH: SFTP, as the logged-in user or as root.
//!
//! As the user, this is the ordinary `sftp` subsystem. As root, there is no
//! standard way — SFTP has no "sudo" — so UwUSSH does what people do by hand:
//! it runs the server's own `sftp-server` through `sudo` and speaks SFTP to it.
//!
//! That runs on a pseudo-terminal, because `sudo` asks for its password on
//! one. The terminal is requested raw from the start (no echo, no line
//! editing, no newline translation, no signals), so once `sftp-server` runs,
//! the bytes pass through unchanged. Before that, UwUSSH reads what comes back:
//! its own prompt marker means sudo wants the password, which is written once
//! and only then; a second prompt means it was wrong; a ready marker, printed
//! by the wrapper right before it starts `sftp-server`, means the SFTP stream
//! begins. The password is never typed into anything but sudo's prompt.
//!
//! As root, deleting is not done over SFTP at all: walking a tree from the
//! client and deleting what it lists can be raced — a user on the server swaps
//! a listed folder for a link to `/etc` — and SFTP has no way to open a folder
//! without following links. So a root delete runs the server's own
//! `rm -rf` through `sudo`, which walks the tree without ever following one.
//!
//! Nothing is overwritten unless the caller says so: files are created
//! exclusively (`O_EXCL`, which also refuses a link in their place), and
//! replacing one removes what is there — a link itself, never its target —
//! first.
//!
//! Remote names are data from the server. A download never lets one reach
//! outside the folder it was asked to write into (see [`safe_local_name`]).

use russh::client::{Handle, Handler};
use russh::Pty;
use russh_sftp::client::fs::File as RemoteFile;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileType, OpenFlags, StatusCode};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zeroize::Zeroizing;

/// How long opening file access may take, sudo included.
const OPEN_TIMEOUT: Duration = Duration::from_secs(40);
/// How long sudo and the wrapper get to start `sftp-server`.
const ELEVATION_TIMEOUT: Duration = Duration::from_secs(25);
/// How long a root delete may run.
const REMOVE_TIMEOUT: Duration = Duration::from_secs(600);
/// The most a failed elevation prints before UwUSSH stops reading it.
const ELEVATION_OUTPUT_LIMIT: usize = 16 * 1024;
const CHUNK: usize = 256 * 1024;

const SUDO_PROMPT: &str = "UWUSSH-SUDO-PROMPT:";
const READY: &str = "UWUSSH-SFTP-READY";
const MISSING: &str = "UWUSSH-SFTP-MISSING";
const REMOVED: &str = "UWUSSH-REMOVED";
const NOT_REMOVED: &str = "UWUSSH-NOT-REMOVED";

/// What a sudo conversation waits for: one marker for success, one for failure.
struct Markers {
    done: &'static str,
    failed: &'static str,
}

const ELEVATE: Markers = Markers {
    done: READY,
    failed: MISSING,
};
const REMOVE: Markers = Markers {
    done: REMOVED,
    failed: NOT_REMOVED,
};

/// Where distributions keep `sftp-server`, most common first.
const ELEVATED_COMMAND: &str = "sudo -p 'UWUSSH-SUDO-PROMPT:' -- sh -c '\
for p in /usr/lib/openssh/sftp-server /usr/libexec/openssh/sftp-server \
/usr/lib/ssh/sftp-server /usr/libexec/sftp-server /usr/lib/sftp-server \
/usr/sbin/sftp-server $(command -v sftp-server 2>/dev/null); do \
if [ -x \"$p\" ]; then printf \"\\nUWUSSH-SFTP-READY\\n\"; exec \"$p\"; fi; done; \
printf \"\\nUWUSSH-SFTP-MISSING\\n\"; exit 127'";

/// Terminal modes for a binary-clean pseudo-terminal.
const RAW_MODES: &[(Pty, u32)] = &[
    (Pty::ECHO, 0),
    (Pty::ECHOE, 0),
    (Pty::ECHOK, 0),
    (Pty::ECHONL, 0),
    (Pty::ECHOCTL, 0),
    (Pty::ECHOKE, 0),
    (Pty::ICANON, 0),
    (Pty::ISIG, 0),
    (Pty::IEXTEN, 0),
    (Pty::ICRNL, 0),
    (Pty::INLCR, 0),
    (Pty::IGNCR, 0),
    (Pty::ISTRIP, 0),
    (Pty::IXON, 0),
    (Pty::IXOFF, 0),
    (Pty::IXANY, 0),
    (Pty::PARMRK, 0),
    (Pty::INPCK, 0),
    (Pty::IUCLC, 0),
    (Pty::IMAXBEL, 0),
    (Pty::OPOST, 0),
    (Pty::ONLCR, 0),
    (Pty::OCRNL, 0),
    (Pty::OLCUC, 0),
    (Pty::CS8, 1),
    (Pty::PARENB, 0),
];

/// How to open files on the server.
pub enum Elevation {
    /// As the logged-in user, through the `sftp` subsystem.
    User,
    /// As root, through `sudo`. The password is only used if sudo asks.
    Root {
        sudo_password: Option<Zeroizing<String>>,
    },
}

#[derive(Debug, Clone, thiserror::Error, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SftpError {
    #[error("the server refused file access: {reason}")]
    Refused { reason: String },
    #[error("sudo needs a password")]
    SudoPasswordRequired,
    #[error("sudo did not accept the password")]
    SudoPasswordRejected,
    #[error("sudo refused: {message}")]
    SudoRefused { message: String },
    #[error("the server has no sftp-server to run as root")]
    NoSftpServer,
    #[error("{path} does not exist")]
    NotFound { path: String },
    #[error("no permission for {path}")]
    PermissionDenied { path: String },
    #[error("{path} already exists")]
    AlreadyExists { path: String },
    #[error("{path} is a link; its permissions are those of what it points to")]
    Link { path: String },
    #[error("{message}")]
    Failed { message: String },
    #[error("a name from the server can't be used on this computer: {name}")]
    UnsafeName { name: String },
    #[error("cancelled")]
    Cancelled,
    #[error("{message}")]
    Local { message: String },
}

type Result<T> = std::result::Result<T, SftpError>;

fn failed(error: impl std::fmt::Display) -> SftpError {
    SftpError::Failed {
        message: error.to_string(),
    }
}

fn local(error: impl std::fmt::Display) -> SftpError {
    SftpError::Local {
        message: error.to_string(),
    }
}

/// A local file error, with "already exists" named as such.
fn local_at(path: &Path, error: std::io::Error) -> SftpError {
    if error.kind() == std::io::ErrorKind::AlreadyExists {
        SftpError::AlreadyExists {
            path: path.display().to_string(),
        }
    } else {
        local(format!("{}: {error}", path.display()))
    }
}

fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(SftpError::Cancelled)
    } else {
        Ok(())
    }
}

/// An SFTP error, named after the path it happened to.
fn remote(path: &str, error: russh_sftp::client::error::Error) -> SftpError {
    match &error {
        russh_sftp::client::error::Error::Status(status) => match status.status_code {
            StatusCode::NoSuchFile => SftpError::NotFound { path: path.into() },
            StatusCode::PermissionDenied => SftpError::PermissionDenied { path: path.into() },
            _ if status.error_message.to_ascii_lowercase().contains("exist") => {
                SftpError::AlreadyExists { path: path.into() }
            }
            _ => failed(format!("{path}: {}", status.error_message)),
        },
        _ => failed(format!("{path}: {error}")),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EntryKind {
    File,
    Dir,
    Link,
    Other,
}

/// A file or folder, local or remote, as the file browser lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub kind: EntryKind,
    pub size: u64,
    pub modified_ms: Option<u64>,
    /// Unix mode bits, remote only.
    pub permissions: Option<u32>,
    pub owner: Option<String>,
    /// A symbolic link. A link to a folder lists as a folder, so it can be
    /// browsed, but says it is a link: as root, what it points to may belong
    /// to someone else entirely.
    pub link: bool,
}

/// An open SFTP session. Cheap to share: operations take `&self`.
pub struct SftpClient {
    session: SftpSession,
    root: bool,
    /// Kept for a root session's deletes, which run `sudo` again.
    sudo_password: Option<Zeroizing<String>>,
}

impl SftpClient {
    /// Open file access on an authenticated connection.
    pub async fn open<H: Handler>(
        handle: &Handle<H>,
        elevation: Elevation,
    ) -> Result<(Self, Option<String>)> {
        tokio::time::timeout(OPEN_TIMEOUT, Self::open_on(handle, elevation))
            .await
            .map_err(|_| SftpError::Refused {
                reason: format!("no answer within {} s", OPEN_TIMEOUT.as_secs()),
            })?
    }

    async fn open_on<H: Handler>(
        handle: &Handle<H>,
        elevation: Elevation,
    ) -> Result<(Self, Option<String>)> {
        let refused = |e: russh::Error| SftpError::Refused {
            reason: e.to_string(),
        };
        let channel = handle.channel_open_session().await.map_err(refused)?;
        match elevation {
            Elevation::User => {
                channel
                    .request_subsystem(true, "sftp")
                    .await
                    .map_err(refused)?;
                let session = SftpSession::new(channel.into_stream()).await.map_err(|e| {
                    SftpError::Refused {
                        reason: e.to_string(),
                    }
                })?;
                let home = session.canonicalize(".").await.ok();
                Ok((
                    Self {
                        session,
                        root: false,
                        sudo_password: None,
                    },
                    home,
                ))
            }
            Elevation::Root { sudo_password } => {
                channel
                    .request_pty(true, "dumb", 200, 24, 0, 0, RAW_MODES)
                    .await
                    .map_err(refused)?;
                channel
                    .exec(true, ELEVATED_COMMAND)
                    .await
                    .map_err(refused)?;
                let mut stream = channel.into_stream();
                tokio::time::timeout(
                    ELEVATION_TIMEOUT,
                    elevate(&mut stream, sudo_password.as_ref()),
                )
                .await
                .map_err(|_| SftpError::SudoRefused {
                    message: format!("no answer within {} s", ELEVATION_TIMEOUT.as_secs()),
                })??;
                let session = SftpSession::new(stream)
                    .await
                    .map_err(|e| SftpError::Refused {
                        reason: e.to_string(),
                    })?;
                Ok((
                    Self {
                        session,
                        root: true,
                        sudo_password,
                    },
                    Some("/".to_string()),
                ))
            }
        }
    }

    pub fn is_root(&self) -> bool {
        self.root
    }

    /// The password sudo took when this root session opened, if it asked.
    pub fn sudo_password(&self) -> Option<&Zeroizing<String>> {
        self.sudo_password.as_ref()
    }

    /// Whether anything — a file, a folder, a link — is at `path`.
    pub async fn exists(&self, path: &str) -> bool {
        self.session.symlink_metadata(path).await.is_ok()
    }

    pub async fn list(&self, path: &str) -> Result<Vec<Entry>> {
        let dir = self
            .session
            .read_dir(path)
            .await
            .map_err(|e| remote(path, e))?;
        let mut entries: Vec<Entry> = dir
            .map(|entry| {
                let metadata = entry.metadata();
                let kind = match metadata.file_type() {
                    FileType::Dir => EntryKind::Dir,
                    FileType::File => EntryKind::File,
                    FileType::Symlink => EntryKind::Link,
                    FileType::Other => EntryKind::Other,
                };
                Entry {
                    kind,
                    link: kind == EntryKind::Link,
                    size: metadata.len(),
                    modified_ms: metadata.mtime.map(|t| u64::from(t) * 1000),
                    permissions: metadata.permissions.map(|p| p & 0o7777),
                    owner: metadata
                        .user
                        .clone()
                        .or_else(|| metadata.uid.map(|u| u.to_string())),
                    path: join(path, &entry.file_name()),
                    name: entry.file_name(),
                }
            })
            .collect();
        // A link to a folder browses like a folder, and still says it is a link.
        for entry in entries.iter_mut().filter(|e| e.kind == EntryKind::Link) {
            if let Ok(target) = self.session.metadata(entry.path.as_str()).await {
                if target.is_dir() {
                    entry.kind = EntryKind::Dir;
                }
            }
        }
        entries.sort_by(|a, b| {
            (a.kind != EntryKind::Dir, a.name.to_lowercase())
                .cmp(&(b.kind != EntryKind::Dir, b.name.to_lowercase()))
        });
        Ok(entries)
    }

    /// The absolute form of a path, `..` and links resolved by the server.
    pub async fn canonicalize(&self, path: &str) -> Result<String> {
        self.session
            .canonicalize(path)
            .await
            .map_err(|e| remote(path, e))
    }

    pub async fn mkdir(&self, path: &str) -> Result<()> {
        self.session
            .create_dir(path)
            .await
            .map_err(|e| remote(path, e))
    }

    pub async fn rename(&self, from: &str, to: &str) -> Result<()> {
        if self.session.try_exists(to).await.unwrap_or(false) {
            return Err(SftpError::AlreadyExists { path: to.into() });
        }
        self.session
            .rename(from, to)
            .await
            .map_err(|e| remote(from, e))
    }

    /// Set a file's or folder's permissions. Never on a link: the server's
    /// SETSTAT follows it, so the change would land on whatever it points to —
    /// as root, possibly `/root` behind a link someone else made.
    pub async fn chmod(&self, path: &str, mode: u32) -> Result<()> {
        let mut metadata = self
            .session
            .symlink_metadata(path)
            .await
            .map_err(|e| remote(path, e))?;
        if metadata.file_type() == FileType::Symlink {
            return Err(SftpError::Link { path: path.into() });
        }
        let kind = metadata.permissions.unwrap_or(0) & !0o7777;
        metadata = russh_sftp::protocol::FileAttributes {
            permissions: Some(kind | (mode & 0o7777)),
            ..Default::default()
        };
        self.session
            .set_metadata(path, metadata)
            .await
            .map_err(|e| remote(path, e))
    }

    /// Delete a file, a link, or a folder with everything in it, as the
    /// logged-in user. Links are removed, never followed. A root session
    /// deletes with [`remove_as_root`] instead.
    pub async fn remove(&self, path: &str) -> Result<()> {
        let metadata = self
            .session
            .symlink_metadata(path)
            .await
            .map_err(|e| remote(path, e))?;
        if metadata.file_type() != FileType::Dir {
            return self
                .session
                .remove_file(path)
                .await
                .map_err(|e| remote(path, e));
        }
        let mut pending = vec![(path.to_string(), false)];
        while let Some((dir, emptied)) = pending.pop() {
            if emptied {
                self.session
                    .remove_dir(dir.as_str())
                    .await
                    .map_err(|e| remote(&dir, e))?;
                continue;
            }
            pending.push((dir.clone(), true));
            let entries = self
                .session
                .read_dir(dir.as_str())
                .await
                .map_err(|e| remote(&dir, e))?;
            for entry in entries {
                let child = entry.path();
                if entry.file_type() == FileType::Dir {
                    pending.push((child, false));
                } else {
                    self.session
                        .remove_file(child.as_str())
                        .await
                        .map_err(|e| remote(&child, e))?;
                }
            }
        }
        Ok(())
    }

    /// The total size of what a download of `path` would fetch.
    pub async fn size_of(&self, path: &str, cancel: &AtomicBool) -> Result<u64> {
        let metadata = self
            .session
            .metadata(path)
            .await
            .map_err(|e| remote(path, e))?;
        if !metadata.is_dir() {
            return Ok(metadata.len());
        }
        let mut total: u64 = 0;
        let mut pending = vec![path.to_string()];
        while let Some(dir) = pending.pop() {
            check_cancel(cancel)?;
            for entry in self
                .session
                .read_dir(dir.as_str())
                .await
                .map_err(|e| remote(&dir, e))?
            {
                match entry.file_type() {
                    FileType::Dir => pending.push(entry.path()),
                    FileType::File => total = total.saturating_add(entry.metadata().len()),
                    _ => {}
                }
            }
        }
        Ok(total)
    }

    /// Copy a remote file or folder into the local folder `into`. Returns the
    /// local path it was written to. `progress` gets every chunk's size.
    ///
    /// Without `overwrite`, something already at the target is an
    /// [`SftpError::AlreadyExists`] before anything is written; with it, files
    /// are replaced and folders merged.
    pub async fn download(
        &self,
        path: &str,
        into: &Path,
        overwrite: bool,
        cancel: &AtomicBool,
        progress: &(dyn Fn(u64) + Send + Sync),
    ) -> Result<PathBuf> {
        let target = download_target(path, into)?;
        if !overwrite && tokio::fs::symlink_metadata(&target).await.is_ok() {
            return Err(SftpError::AlreadyExists {
                path: target.display().to_string(),
            });
        }
        let metadata = self
            .session
            .metadata(path)
            .await
            .map_err(|e| remote(path, e))?;
        if !metadata.is_dir() {
            self.download_file(path, &target, overwrite, cancel, progress)
                .await?;
            return Ok(target);
        }
        let mut pending = vec![(path.to_string(), target.clone())];
        while let Some((dir, local_dir)) = pending.pop() {
            check_cancel(cancel)?;
            if let Err(error) = tokio::fs::create_dir(&local_dir).await {
                let merge = overwrite
                    && tokio::fs::symlink_metadata(&local_dir)
                        .await
                        .is_ok_and(|m| m.is_dir());
                if !merge {
                    return Err(local_at(&local_dir, error));
                }
            }
            for entry in self
                .session
                .read_dir(dir.as_str())
                .await
                .map_err(|e| remote(&dir, e))?
            {
                check_cancel(cancel)?;
                let child = local_dir.join(safe_local_name(&entry.file_name())?);
                match entry.file_type() {
                    FileType::Dir => pending.push((entry.path(), child)),
                    FileType::File => {
                        self.download_file(&entry.path(), &child, overwrite, cancel, progress)
                            .await?
                    }
                    // Links and devices are not followed out of the tree.
                    _ => {}
                }
            }
        }
        Ok(target)
    }

    async fn download_file(
        &self,
        path: &str,
        target: &Path,
        overwrite: bool,
        cancel: &AtomicBool,
        progress: &(dyn Fn(u64) + Send + Sync),
    ) -> Result<()> {
        let mut source = self.session.open(path).await.map_err(|e| remote(path, e))?;
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true);
        if overwrite {
            options.create(true).truncate(true);
        } else {
            options.create_new(true);
        }
        let mut sink = match options.open(target).await {
            Ok(sink) => sink,
            Err(error) => {
                let _ = source.shutdown().await;
                return Err(local_at(target, error));
            }
        };
        let mut buffer = vec![0u8; CHUNK];
        let copied = async {
            loop {
                check_cancel(cancel)?;
                let read = source
                    .read(&mut buffer)
                    .await
                    .map_err(|e| failed(format!("{path}: {e}")))?;
                if read == 0 {
                    break;
                }
                sink.write_all(&buffer[..read]).await.map_err(local)?;
                progress(read as u64);
            }
            sink.flush().await.map_err(local)?;
            Ok(())
        }
        .await;
        let _ = source.shutdown().await;
        drop(sink);
        if copied.is_err() {
            let _ = tokio::fs::remove_file(target).await;
        } else {
            mark_as_downloaded(target).await;
        }
        copied
    }

    /// Copy a local file or folder into the remote folder `into`. Returns the
    /// remote path it was written to.
    ///
    /// Without `overwrite`, something already at the target is an
    /// [`SftpError::AlreadyExists`] before anything is written; with it, files
    /// are replaced and folders merged — never through a link.
    pub async fn upload(
        &self,
        source: &Path,
        into: &str,
        overwrite: bool,
        cancel: &AtomicBool,
        progress: &(dyn Fn(u64) + Send + Sync),
    ) -> Result<String> {
        let name = source
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| local("the file has no usable name"))?;
        let target = join(into, name);
        if !overwrite && self.exists(&target).await {
            return Err(SftpError::AlreadyExists { path: target });
        }
        let metadata = tokio::fs::symlink_metadata(source).await.map_err(local)?;
        if !metadata.is_dir() {
            self.upload_file(source, &target, overwrite, cancel, progress)
                .await?;
            return Ok(target);
        }
        let mut pending = vec![(source.to_path_buf(), target.clone())];
        while let Some((dir, remote_dir)) = pending.pop() {
            check_cancel(cancel)?;
            if let Err(error) = self.session.create_dir(remote_dir.as_str()).await {
                // Merging writes into a real folder only, never a link to one.
                let existing = self.session.symlink_metadata(remote_dir.as_str()).await;
                match existing {
                    Ok(found) if overwrite && found.file_type() == FileType::Dir => {}
                    Ok(_) => {
                        return Err(SftpError::AlreadyExists {
                            path: remote_dir.clone(),
                        })
                    }
                    Err(_) => return Err(remote(&remote_dir, error)),
                }
            }
            let mut children = tokio::fs::read_dir(&dir).await.map_err(local)?;
            while let Some(child) = children.next_entry().await.map_err(local)? {
                check_cancel(cancel)?;
                let Some(child_name) = child.file_name().to_str().map(str::to_string) else {
                    continue;
                };
                let kind = child.file_type().await.map_err(local)?;
                let remote_child = join(&remote_dir, &child_name);
                if kind.is_dir() {
                    pending.push((child.path(), remote_child));
                } else if kind.is_file() {
                    self.upload_file(&child.path(), &remote_child, overwrite, cancel, progress)
                        .await?;
                }
            }
        }
        Ok(target)
    }

    /// Create a remote file that wasn't there. `O_EXCL` also refuses a link in
    /// its place, dangling or not, so a write never lands where a link points.
    async fn create_new(&self, path: &str) -> std::result::Result<RemoteFile, SftpError> {
        match self
            .session
            .open_with_flags(
                path,
                OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE,
            )
            .await
        {
            Ok(file) => Ok(file),
            // SFTP v3 has no "exists" status; OpenSSH says "Failure".
            Err(error) if self.exists(path).await => {
                drop(error);
                Err(SftpError::AlreadyExists { path: path.into() })
            }
            Err(error) => Err(remote(path, error)),
        }
    }

    async fn upload_file(
        &self,
        source: &Path,
        target: &str,
        overwrite: bool,
        cancel: &AtomicBool,
        progress: &(dyn Fn(u64) + Send + Sync),
    ) -> Result<()> {
        let mut input = tokio::fs::File::open(source).await.map_err(local)?;
        let mut output = match self.create_new(target).await {
            Err(SftpError::AlreadyExists { .. }) if overwrite => {
                let existing = self
                    .session
                    .symlink_metadata(target)
                    .await
                    .map_err(|e| remote(target, e))?;
                if existing.file_type() == FileType::Dir {
                    return Err(SftpError::AlreadyExists {
                        path: target.into(),
                    });
                }
                // Replace what is there: a link is removed itself, not followed.
                self.session
                    .remove_file(target)
                    .await
                    .map_err(|e| remote(target, e))?;
                self.create_new(target).await?
            }
            other => other?,
        };
        let mut buffer = vec![0u8; CHUNK];
        let copied = async {
            loop {
                check_cancel(cancel)?;
                let read = input.read(&mut buffer).await.map_err(local)?;
                if read == 0 {
                    break;
                }
                output
                    .write_all(&buffer[..read])
                    .await
                    .map_err(|e| failed(format!("{target}: {e}")))?;
                progress(read as u64);
            }
            Ok(())
        }
        .await;
        let closed = output
            .shutdown()
            .await
            .map_err(|e| failed(format!("{target}: {e}")));
        match (copied, closed) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), _) | (Ok(()), Err(error)) => {
                let _ = self.session.remove_file(target).await;
                Err(error)
            }
        }
    }

    pub async fn close(&self) {
        let _ = self.session.close().await;
    }
}

/// Delete a file, link or folder as root, with the server's own `rm -rf`
/// through `sudo` on a pseudo-terminal of its own.
///
/// `rm` walks the tree relative to folders it already opened and never
/// follows a link, so nobody on the server can steer it elsewhere mid-way —
/// which a walk over SFTP can't promise. The password is used the same way as
/// when the root session opened: once, and only at sudo's own prompt.
pub async fn remove_as_root<H: Handler>(
    handle: &Handle<H>,
    path: &str,
    password: Option<&Zeroizing<String>>,
) -> Result<()> {
    let target = root_removal_target(path)?;
    let command = format!(
        "sudo -p '{SUDO_PROMPT}' -- sh -c 'rm -rf -- \"$1\" && printf \"\\n{REMOVED}\\n\" \
         || printf \"\\n{NOT_REMOVED}\\n\"' uwussh {target}"
    );
    let refused = |e: russh::Error| SftpError::Refused {
        reason: e.to_string(),
    };
    let run = async {
        let channel = handle.channel_open_session().await.map_err(refused)?;
        channel
            .request_pty(true, "dumb", 200, 24, 0, 0, RAW_MODES)
            .await
            .map_err(refused)?;
        channel.exec(true, command).await.map_err(refused)?;
        let mut stream = channel.into_stream();
        match sudo_conversation(&mut stream, password, &REMOVE).await? {
            Outcome::Done(_) => Ok(()),
            Outcome::Failed(seen) => Err(SftpError::Failed {
                message: last_words(&seen, "rm ended without saying why"),
            }),
        }
    };
    tokio::time::timeout(REMOVE_TIMEOUT, run)
        .await
        .map_err(|_| SftpError::Failed {
            message: format!(
                "the delete did not finish within {} s",
                REMOVE_TIMEOUT.as_secs()
            ),
        })?
}

/// An absolute remote path, quoted for a POSIX shell — or an error for
/// anything a root delete should never be pointed at: a relative path, `/`
/// itself, `..`, or characters a shell line can't carry safely.
fn root_removal_target(path: &str) -> Result<String> {
    let parts: Vec<&str> = path
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    if !path.starts_with('/')
        || parts.is_empty()
        || parts.contains(&"..")
        || path.chars().any(|c| c.is_control())
    {
        return Err(SftpError::Failed {
            message: format!("{path}: not something to delete as root"),
        });
    }
    Ok(format!("'{}'", path.replace('\'', r"'\''")))
}

/// Read what sudo and the wrapper print until the SFTP stream starts.
async fn elevate<S>(stream: &mut S, password: Option<&Zeroizing<String>>) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    match sudo_conversation(stream, password, &ELEVATE).await? {
        Outcome::Failed(_) => Err(SftpError::NoSftpServer),
        Outcome::Done(seen) => {
            // The SFTP stream starts right after the marker's line. The
            // server says nothing before the client's first packet, so
            // anything else there means this is not the stream it should be.
            let mut rest = after_marker(&seen, READY).to_vec();
            while rest.is_empty() {
                let mut byte = [0u8; 1];
                if stream.read(&mut byte).await.map_err(local)? == 0 {
                    return Err(SftpError::NoSftpServer);
                }
                rest.push(byte[0]);
            }
            if rest == b"\r" {
                let mut byte = [0u8; 1];
                if stream.read(&mut byte).await.map_err(local)? == 0 {
                    return Err(SftpError::NoSftpServer);
                }
                rest.push(byte[0]);
            }
            if rest == b"\n" || rest == b"\r\n" {
                Ok(())
            } else {
                Err(SftpError::Refused {
                    reason: "unexpected output before the SFTP stream".into(),
                })
            }
        }
    }
}

/// How a sudo conversation ended, with everything it read.
enum Outcome {
    Done(Vec<u8>),
    Failed(Vec<u8>),
}

/// Answer sudo's prompt, once, and read until the command says it is done or
/// that it failed.
async fn sudo_conversation<S>(
    stream: &mut S,
    password: Option<&Zeroizing<String>>,
    markers: &Markers,
) -> Result<Outcome>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut seen = Vec::new();
    let mut answered = false;
    // One byte at a time: nothing after the done marker's line may be read
    // here, since that is where the SFTP stream begins.
    let mut byte = [0u8; 1];
    loop {
        let read = stream.read(&mut byte).await.map_err(local)?;
        if read == 0 {
            return Err(SftpError::SudoRefused {
                message: last_words(&seen, "sudo ended without starting the command"),
            });
        }
        seen.push(byte[0]);
        match elevation_step(&seen, answered, markers) {
            ElevationStep::Wait if seen.len() > ELEVATION_OUTPUT_LIMIT => {
                return Err(SftpError::SudoRefused {
                    message: last_words(&seen, "sudo said too much"),
                })
            }
            ElevationStep::Wait => {}
            ElevationStep::Ready => return Ok(Outcome::Done(seen)),
            ElevationStep::Missing => return Ok(Outcome::Failed(seen)),
            ElevationStep::Rejected => return Err(SftpError::SudoPasswordRejected),
            ElevationStep::Password => {
                let Some(password) = password else {
                    return Err(SftpError::SudoPasswordRequired);
                };
                let mut line = Zeroizing::new(Vec::with_capacity(password.len() + 1));
                line.extend_from_slice(password.as_bytes());
                line.push(b'\n');
                stream.write_all(&line).await.map_err(local)?;
                stream.flush().await.map_err(local)?;
                answered = true;
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ElevationStep {
    Wait,
    Password,
    Rejected,
    Ready,
    Missing,
}

fn elevation_step(seen: &[u8], answered: bool, markers: &Markers) -> ElevationStep {
    let text = String::from_utf8_lossy(seen);
    if text.contains(markers.done) {
        return ElevationStep::Ready;
    }
    if text.contains(markers.failed) {
        return ElevationStep::Missing;
    }
    // sudo prints its prompt and then waits for the answer, so a prompt is
    // only one when nothing follows it — not the marker inside some echoed
    // command line.
    if !text
        .trim_end_matches([' ', '\r', '\n'])
        .ends_with(SUDO_PROMPT)
    {
        return ElevationStep::Wait;
    }
    match text.matches(SUDO_PROMPT).count() {
        1 if !answered => ElevationStep::Password,
        1 => ElevationStep::Wait,
        _ => ElevationStep::Rejected,
    }
}

/// What followed `marker` in `seen`; empty when nothing did yet.
fn after_marker<'a>(seen: &'a [u8], marker: &str) -> &'a [u8] {
    seen.windows(marker.len())
        .position(|window| window == marker.as_bytes())
        .map_or(&[], |at| &seen[at + marker.len()..])
}

/// The last lines of what the server printed, for an error message.
fn last_words(seen: &[u8], otherwise: &str) -> String {
    let mut text = String::from_utf8_lossy(seen).into_owned();
    for marker in [SUDO_PROMPT, READY, MISSING, REMOVED, NOT_REMOVED] {
        text = text.replace(marker, "");
    }
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let tail = lines[lines.len().saturating_sub(3)..].join(" ");
    let tail: String = tail.chars().filter(|c| !c.is_control()).take(300).collect();
    if tail.is_empty() {
        otherwise.into()
    } else {
        tail
    }
}

/// Mark a download as coming from elsewhere, the way a browser does, so
/// SmartScreen and Office's Protected View treat it as such. Best effort: a
/// drive that has no alternate data streams simply goes without.
async fn mark_as_downloaded(path: &Path) {
    #[cfg(windows)]
    {
        let mut stream = path.as_os_str().to_owned();
        stream.push(":Zone.Identifier");
        let _ = tokio::fs::write(stream, "[ZoneTransfer]\r\nZoneId=3\r\n").await;
    }
    #[cfg(not(windows))]
    let _ = path;
}

/// A POSIX path joined with a name.
pub fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// Where [`SftpClient::download`] puts the remote `path` inside `into`.
pub fn download_target(path: &str, into: &Path) -> Result<PathBuf> {
    Ok(into.join(safe_local_name(base_name(path))?))
}

fn base_name(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|n| !n.is_empty())
        .unwrap_or("download")
}

/// A remote file name as a local one, or an error when it could reach outside
/// its folder or isn't a valid name here. A server decides these names, and a
/// download must never write where it wasn't asked to.
pub fn safe_local_name(name: &str) -> Result<&str> {
    // Windows' device names, with or without an extension — superscript
    // digits included, which Windows counts as digits here.
    const RESERVED: &[&str] = &[
        "con",
        "prn",
        "aux",
        "nul",
        "conin$",
        "conout$",
        "clock$",
        "com0",
        "com1",
        "com2",
        "com3",
        "com4",
        "com5",
        "com6",
        "com7",
        "com8",
        "com9",
        "com\u{b9}",
        "com\u{b2}",
        "com\u{b3}",
        "lpt0",
        "lpt1",
        "lpt2",
        "lpt3",
        "lpt4",
        "lpt5",
        "lpt6",
        "lpt7",
        "lpt8",
        "lpt9",
        "lpt\u{b9}",
        "lpt\u{b2}",
        "lpt\u{b3}",
    ];
    let unsafe_name = || SftpError::UnsafeName {
        name: name.chars().filter(|c| !c.is_control()).take(120).collect(),
    };
    // "com1 .txt" is still COM1: spaces before the dot don't count.
    let stem = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches(' ')
        .to_lowercase();
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.len() > 255
        || name.ends_with(' ')
        || name.ends_with('.')
        || name.chars().any(|c| {
            c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
        || RESERVED.contains(&stem.as_str())
    {
        return Err(unsafe_name());
    }
    Ok(name)
}

/// Stops a running transfer from another task.
#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn flag(&self) -> &AtomicBool {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remote_name_never_reaches_outside_its_folder() {
        for bad in [
            "..",
            ".",
            "../evil",
            "a/b",
            "a\\b",
            "C:evil",
            "con",
            "NUL.txt",
            "com1 .txt",
            "COM\u{b9}",
            "lpt0.log",
            "CONOUT$",
            "trailing.",
            "trailing ",
            "bell\u{7}",
            "",
        ] {
            assert!(safe_local_name(bad).is_err(), "{bad:?}");
        }
        for good in ["notes.txt", ".bashrc", "ünïcode ✧.log", "con-fig"] {
            assert_eq!(safe_local_name(good).unwrap(), good);
        }
    }

    #[test]
    fn sudo_gets_the_password_once_and_only_at_its_prompt() {
        let step = |seen: &[u8], answered| elevation_step(seen, answered, &ELEVATE);
        assert_eq!(step(b"", false), ElevationStep::Wait);
        assert_eq!(
            step(b"We trust you have received the usual lecture", false),
            ElevationStep::Wait
        );
        assert_eq!(step(b"UWUSSH-SUDO-PROMPT:", false), ElevationStep::Password);
        assert_eq!(step(b"UWUSSH-SUDO-PROMPT:", true), ElevationStep::Wait);
        assert_eq!(
            step(
                b"UWUSSH-SUDO-PROMPT:\nSorry, try again.\nUWUSSH-SUDO-PROMPT:",
                true
            ),
            ElevationStep::Rejected
        );
        assert_eq!(
            step(b"UWUSSH-SUDO-PROMPT:\n\nUWUSSH-SFTP-READY", true),
            ElevationStep::Ready
        );
        assert_eq!(
            step(b"\nUWUSSH-SFTP-READY", false),
            ElevationStep::Ready,
            "no password needed"
        );
        assert_eq!(
            step(b"\nUWUSSH-SFTP-MISSING", false),
            ElevationStep::Missing
        );
        assert_eq!(
            step(b"sudo -p 'UWUSSH-SUDO-PROMPT:' -- sh -c", false),
            ElevationStep::Wait,
            "the marker inside an echoed command line is no prompt"
        );
        assert_eq!(
            elevation_step(b"\nUWUSSH-NOT-REMOVED", false, &REMOVE),
            ElevationStep::Missing
        );
    }

    #[test]
    fn a_failed_sudo_says_why_in_a_few_words() {
        let said = last_words(
            b"UWUSSH-SUDO-PROMPT:\nuwu is not in the sudoers file.  This incident will be reported.\n",
            "nothing",
        );
        assert_eq!(
            said,
            "uwu is not in the sudoers file.  This incident will be reported."
        );
    }

    #[test]
    fn paths_join_the_posix_way() {
        assert_eq!(join("/", "etc"), "/etc");
        assert_eq!(join("/home/uwu", "notes"), "/home/uwu/notes");
        assert_eq!(join("", "notes"), "notes");
        assert_eq!(base_name("/var/log/"), "log");
        assert_eq!(base_name("/"), "download");
    }

    #[tokio::test]
    async fn the_elevation_handshake_writes_the_password_after_the_prompt() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        let password = Zeroizing::new("nyu".to_string());
        let server_side = tokio::spawn(async move {
            server.write_all(b"UWUSSH-SUDO-PROMPT:").await.unwrap();
            let mut line = [0u8; 4];
            server.read_exact(&mut line).await.unwrap();
            assert_eq!(&line, b"nyu\n");
            // The marker's newline arrives on its own, and the SFTP stream
            // right behind it must not be touched.
            server.write_all(b"\nUWUSSH-SFTP-READY").await.unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
            server.write_all(b"\nSFTP").await.unwrap();
        });
        elevate(&mut client, Some(&password)).await.unwrap();
        let mut stream = [0u8; 4];
        client.read_exact(&mut stream).await.unwrap();
        assert_eq!(&stream, b"SFTP");
        server_side.await.unwrap();
    }

    #[test]
    fn a_root_delete_is_only_ever_aimed_at_something_below_the_root() {
        for bad in [
            "/",
            "//",
            "/./",
            "relative/path",
            "/home/../etc",
            "/tmp/a\nb",
            "",
        ] {
            assert!(root_removal_target(bad).is_err(), "{bad:?}");
        }
        assert_eq!(
            root_removal_target("/home/uwu/old stuff").unwrap(),
            "'/home/uwu/old stuff'"
        );
        assert_eq!(root_removal_target("/srv/it's").unwrap(), r"'/srv/it'\''s'");
    }

    #[tokio::test]
    async fn a_root_delete_reports_what_rm_said() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        let server_side = tokio::spawn(async move {
            server
                .write_all(
                    b"rm: cannot remove '/srv/x': Read-only file system\r\n\nUWUSSH-NOT-REMOVED\n",
                )
                .await
                .unwrap();
        });
        let outcome = sudo_conversation(&mut client, None, &REMOVE).await.unwrap();
        let Outcome::Failed(seen) = outcome else {
            panic!("rm failed, so the conversation must say so");
        };
        assert_eq!(
            last_words(&seen, "nothing"),
            "rm: cannot remove '/srv/x': Read-only file system"
        );
        server_side.await.unwrap();
    }
}
