//! Keeping track of open sessions.
//!
//! For M0 every session is a local PTY. From M1 this becomes an enum with an
//! SSH variant — the manager, the ids and the write/resize/close surface stay
//! the same, because both kinds are "something that eats keystrokes and emits
//! bytes".

use crate::metrics::MetricsSnapshot;
use crate::pty::PtySession;
use crate::stream::FrameSink;
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
    Local(PtySession),
    // Ssh(SshSession) — M1
}

#[derive(Default)]
pub struct SessionManager {
    sessions: RwLock<HashMap<SessionId, Arc<Session>>>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Open a local shell and start streaming it into `sink`.
    pub fn spawn_local<S: FrameSink>(&self, cols: u16, rows: u16, sink: S) -> Result<SessionId> {
        let session = PtySession::spawn(cols, rows, sink)?;
        let id = SessionId::new();
        self.sessions.write().insert(id, Arc::new(Session::Local(session)));
        tracing::info!(%id, cols, rows, "local session opened");
        Ok(id)
    }

    pub fn write(&self, id: SessionId, data: &[u8]) -> Result<()> {
        match &*self.get(id)? {
            Session::Local(pty) => pty.write(data),
        }
    }

    pub fn resize(&self, id: SessionId, cols: u16, rows: u16) -> Result<()> {
        match &*self.get(id)? {
            Session::Local(pty) => pty.resize(cols, rows),
        }
    }

    pub fn metrics(&self, id: SessionId) -> Result<MetricsSnapshot> {
        match &*self.get(id)? {
            Session::Local(pty) => Ok(pty.metrics().snapshot()),
        }
    }

    pub fn close(&self, id: SessionId) -> Result<()> {
        let session = self.sessions.write().remove(&id).ok_or(CoreError::UnknownSession(id))?;
        match &*session {
            Session::Local(pty) => {
                // A dead child is not an error here — the user may simply have
                // typed `exit` before hitting the close button.
                let _ = pty.kill();
            }
        }
        tracing::info!(%id, "session closed");
        Ok(())
    }

    pub fn ids(&self) -> Vec<SessionId> {
        self.sessions.read().keys().copied().collect()
    }

    fn get(&self, id: SessionId) -> Result<Arc<Session>> {
        self.sessions.read().get(&id).cloned().ok_or(CoreError::UnknownSession(id))
    }
}
