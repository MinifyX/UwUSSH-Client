//! Keeping track of open sessions.
//!
//! Every session, whatever produces its bytes, has the same surface: write
//! keystrokes, resize, acknowledge what the renderer processed, read metrics,
//! close. A local shell, an SSH connection and the M0 load source all look the
//! same from here.

use crate::metrics::MetricsSnapshot;
use crate::pty::PtySession;
use crate::ssh::{SshError, SshSession, SshTarget};
use crate::stream::FrameSink;
use crate::synthetic::SyntheticSession;
use crate::{CoreError, Result};
use parking_lot::RwLock;
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

// Sessions currently outlive a webview reload: a reloaded page stops
// acknowledging, and a flow-controlled session then pauses forever. Harmless
// for M0, where nothing reloads mid-measurement; M1 ties sessions to the window
// that opened them.
#[derive(Default)]
pub struct SessionManager {
    sessions: RwLock<HashMap<SessionId, Arc<Session>>>,
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
    pub async fn spawn_ssh<S: FrameSink>(
        &self,
        target: SshTarget,
        cols: u16,
        rows: u16,
        sink: S,
    ) -> std::result::Result<SessionId, SshError> {
        let session = SshSession::connect(target, cols, rows, true, sink).await?;
        Ok(self.insert(Session::Ssh(session), "ssh"))
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
