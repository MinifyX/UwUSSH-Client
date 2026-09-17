//! Keeping track of open sessions.
//!
//! Every session, whatever produces its bytes, has the same surface: write
//! keystrokes, resize, acknowledge what the renderer processed, read metrics,
//! close. A local shell, an SSH connection and the M0 load source all look the
//! same from here.

use crate::metrics::MetricsSnapshot;
use crate::pty::PtySession;
use crate::sftp::{Elevation, SftpError};
use crate::ssh::{FileSession, SshConnection, SshError, SshSession, SshTarget};
use crate::stream::FrameSink;
use crate::synthetic::SyntheticSession;
use crate::{CoreError, Result};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub Uuid);

impl SessionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

enum Session {
    Pty(PtySession),
    Ssh(SshSession),
    Synthetic(SyntheticSession),
}

/// Every open session, whatever runs it.
///
/// Sessions belong to the page that opened them. A reloaded page cannot talk to
/// them any more — it stopped acknowledging, so a flow-controlled session would
/// pause forever while its SSH connection stays open — so the desktop calls
/// [`SessionManager::close_all`] whenever a page starts.
#[derive(Default)]
pub struct SessionManager {
    sessions: RwLock<HashMap<SessionId, Arc<Session>>>,
    /// Verified connections waiting for the user to answer a question —
    /// password, passphrase, another try — keyed by whatever the caller uses to
    /// identify an attempt. The desktop uses the tab, so two tabs connecting to
    /// the same host at the same time never share a half-open connection.
    pending: Mutex<HashMap<String, SshConnection>>,
    /// Open file browsers, each on its own connection.
    files: RwLock<HashMap<SessionId, Arc<FileSession>>>,
}

/// Why file access didn't open: a question about logging in, or one about
/// sudo. Both carry their own `kind` tag for the UI.
#[derive(Debug, thiserror::Error, Serialize)]
#[serde(untagged)]
pub enum FilesError {
    #[error(transparent)]
    Ssh(#[from] SshError),
    #[error(transparent)]
    Sftp(#[from] SftpError),
}

impl SessionManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// The user's default shell, flow-controlled.
    pub fn spawn_shell<S: FrameSink>(&self, cols: u16, rows: u16, sink: S) -> Result<SessionId> {
        let session = PtySession::spawn_shell(cols, rows, sink)?;
        Ok(self.insert(Session::Pty(session), "shell"))
    }

    pub fn spawn_command<S: FrameSink>(
        &self,
        program: &str,
        args: &[String],
        cols: u16,
        rows: u16,
        flow_control: bool,
        sink: S,
    ) -> Result<SessionId> {
        let session = PtySession::spawn_command(program, args, cols, rows, flow_control, sink)?;
        Ok(self.insert(Session::Pty(session), "command"))
    }

    /// Connect to an SSH server and open a shell on it, flow-controlled.
    ///
    /// Errors are the interesting part: most of them are not failures but the
    /// next question to ask the user — trust this key? what is the password?
    /// When the question comes after the host key checked out, the verified
    /// connection is kept under `attempt`, and the next call with the same
    /// `attempt` continues on it instead of connecting again.
    pub async fn spawn_ssh<S: FrameSink>(
        &self,
        attempt: &str,
        target: SshTarget,
        cols: u16,
        rows: u16,
        sink: S,
    ) -> std::result::Result<SessionId, SshError> {
        let connection = self.authenticated(attempt, &target).await?;
        let session = connection.open_shell(cols, rows, true, sink).await?;
        Ok(self.insert(Session::Ssh(session), "ssh"))
    }

    /// Connect, or continue on the connection waiting under `attempt`, and log
    /// in. A question for the user keeps the connection waiting.
    async fn authenticated(
        &self,
        attempt: &str,
        target: &SshTarget,
    ) -> std::result::Result<SshConnection, SshError> {
        let waiting = self.pending.lock().remove(attempt);
        let mut connection = match waiting {
            Some(connection) if connection.is_reusable_for(target) => connection,
            stale => {
                if let Some(stale) = stale {
                    tokio::spawn(stale.close());
                }
                SshConnection::open(target).await?
            }
        };

        if let Err(err) = connection.authenticate(&target.auth).await {
            if err.awaits_answer() && connection.is_reusable_for(target) {
                self.pending.lock().insert(attempt.to_string(), connection);
            }
            return Err(err);
        }
        Ok(connection)
    }

    /// Open a file browser on a connection of its own. Returns the session and
    /// the folder to start in. When sudo asks for a password (or didn't like
    /// the one it got), the logged-in connection waits under `attempt` for
    /// the next try, like a login question does.
    pub async fn open_files(
        &self,
        attempt: &str,
        target: SshTarget,
        elevation: Elevation,
    ) -> std::result::Result<(SessionId, Option<String>), FilesError> {
        let connection = self.authenticated(attempt, &target).await?;
        match connection.open_files(elevation).await {
            Ok((client, home)) => {
                let id = SessionId::new();
                self.files
                    .write()
                    .insert(id, Arc::new(connection.into_files(client)));
                tracing::info!(%id, "file session opened");
                Ok((id, home))
            }
            Err(error) => {
                let ask_again = matches!(
                    error,
                    SftpError::SudoPasswordRequired | SftpError::SudoPasswordRejected
                );
                if ask_again && connection.is_reusable_for(&target) {
                    self.pending.lock().insert(attempt.to_string(), connection);
                } else {
                    tokio::spawn(connection.close());
                }
                Err(error.into())
            }
        }
    }

    /// The SFTP client of an open file browser.
    pub fn files(&self, id: SessionId) -> Result<Arc<FileSession>> {
        let files = self
            .files
            .read()
            .get(&id)
            .cloned()
            .ok_or(CoreError::UnknownSession(id))?;
        if files.is_closed() {
            self.files.write().remove(&id);
            return Err(CoreError::SessionClosed);
        }
        Ok(files)
    }

    pub fn close_files(&self, id: SessionId) {
        if let Some(files) = self.files.write().remove(&id) {
            tokio::spawn(async move { files.close().await });
            tracing::info!(%id, "file session closed");
        }
    }

    /// What an SSH session's server runs, found out on a channel next to the
    /// terminal. `None` for other sessions or when it can't tell.
    pub fn os_probe(
        &self,
        id: SessionId,
    ) -> Option<impl std::future::Future<Output = Option<&'static str>> + Send + 'static> {
        match &*self.get(id).ok()? {
            Session::Ssh(ssh) => Some(ssh.os_probe()),
            _ => None,
        }
    }

    /// Whether `id` is an SSH session.
    pub fn is_ssh(&self, id: SessionId) -> bool {
        matches!(self.get(id).as_deref(), Ok(Session::Ssh(_)))
    }

    /// The user walked away from a question: close the connection that was
    /// waiting for the answer.
    pub fn abandon_ssh(&self, attempt: &str) {
        if let Some(connection) = self.pending.lock().remove(attempt) {
            tokio::spawn(connection.close());
        }
    }

    pub fn spawn_synthetic<S: FrameSink>(
        &self,
        total_bytes: usize,
        flow_control: bool,
        sink: S,
    ) -> SessionId {
        let session = SyntheticSession::spawn(total_bytes, flow_control, sink);
        self.insert(Session::Synthetic(session), "synthetic")
    }

    pub fn write(&self, id: SessionId, data: &[u8]) -> Result<()> {
        match &*self.get(id)? {
            Session::Pty(pty) => pty.write(data),
            Session::Ssh(ssh) => ssh.write(data),
            // Nothing is listening; typing into a benchmark is not an error.
            Session::Synthetic(_) => Ok(()),
        }
    }

    pub fn resize(&self, id: SessionId, cols: u16, rows: u16) -> Result<()> {
        match &*self.get(id)? {
            Session::Pty(pty) => pty.resize(cols, rows),
            Session::Ssh(ssh) => ssh.resize(cols, rows),
            Session::Synthetic(_) => Ok(()),
        }
    }

    /// The renderer has processed `bytes` more bytes of this session's output.
    pub fn ack(&self, id: SessionId, bytes: u64) -> Result<()> {
        match &*self.get(id)? {
            Session::Pty(pty) => pty.ack(bytes),
            Session::Ssh(ssh) => ssh.ack(bytes),
            Session::Synthetic(synthetic) => synthetic.ack(bytes),
        }
        Ok(())
    }

    pub fn metrics(&self, id: SessionId) -> Result<MetricsSnapshot> {
        Ok(match &*self.get(id)? {
            Session::Pty(pty) => pty.metrics(),
            Session::Ssh(ssh) => ssh.metrics(),
            Session::Synthetic(synthetic) => synthetic.metrics(),
        })
    }

    pub fn close(&self, id: SessionId) -> Result<()> {
        let session = self
            .sessions
            .write()
            .remove(&id)
            .ok_or(CoreError::UnknownSession(id))?;
        match &*session {
            Session::Pty(pty) => pty.close(),
            Session::Ssh(ssh) => ssh.close(),
            Session::Synthetic(synthetic) => synthetic.close(),
        }
        tracing::info!(%id, "session closed");
        Ok(())
    }

    /// Close every session and drop every connection waiting for an answer.
    /// Returns how many sessions were open.
    pub fn close_all(&self) -> usize {
        let sessions: Vec<_> = self.sessions.write().drain().collect();
        for (_, session) in &sessions {
            match &**session {
                Session::Pty(pty) => pty.close(),
                Session::Ssh(ssh) => ssh.close(),
                Session::Synthetic(synthetic) => synthetic.close(),
            }
        }
        let pending: Vec<_> = self.pending.lock().drain().map(|(_, c)| c).collect();
        for connection in pending {
            tokio::spawn(connection.close());
        }
        let files: Vec<_> = self.files.write().drain().map(|(_, f)| f).collect();
        for session in files {
            tokio::spawn(async move { session.close().await });
        }
        if !sessions.is_empty() {
            tracing::info!(count = sessions.len(), "all sessions closed");
        }
        sessions.len()
    }

    pub fn ids(&self) -> Vec<SessionId> {
        self.sessions.read().keys().copied().collect()
    }

    fn insert(&self, session: Session, kind: &'static str) -> SessionId {
        let id = SessionId::new();
        self.sessions.write().insert(id, Arc::new(session));
        tracing::info!(%id, kind, "session opened");
        id
    }

    fn get(&self, id: SessionId) -> Result<Arc<Session>> {
        self.sessions
            .read()
            .get(&id)
            .cloned()
            .ok_or(CoreError::UnknownSession(id))
    }
}
