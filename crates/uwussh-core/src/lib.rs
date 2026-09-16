//! The UwUSSH session engine.
//!
//! Everything that talks to a terminal lives here: local PTY sessions today,
//! SSH sessions from M1 on. Both produce the same thing — a stream of bytes —
//! so they share [`stream`], the part that decides how those bytes reach the
//! UI without melting the IPC boundary.
//!
//! This crate deliberately knows nothing about Tauri. The UI layer implements
//! [`stream::FrameSink`] over whatever transport it has, which keeps the engine
//! testable without a window.

pub mod metrics;
pub mod pty;
pub mod session;
pub mod stream;

pub use metrics::{Metrics, MetricsSnapshot};
pub use session::{SessionId, SessionManager};
pub use stream::{FrameSink, SinkError};

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("no session with id {0}")]
    UnknownSession(SessionId),
    #[error("the session has ended")]
    SessionClosed,
    #[error(transparent)]
    Pty(#[from] anyhow::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, CoreError>;
