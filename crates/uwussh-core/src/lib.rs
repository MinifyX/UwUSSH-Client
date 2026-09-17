//! The UwUSSH session engine.
//!
//! Everything that talks to a terminal lives here: local PTY sessions and a
//! synthetic load source, and SSH sessions — plus what rides along on an SSH
//! connection: file access over SFTP and finding out what the server runs. All of them produce
//! the same thing — a stream of bytes — so they share [`stream`] and [`flow`],
//! the parts that decide how those bytes reach the UI without melting the IPC
//! boundary.
//!
//! This crate deliberately knows nothing about Tauri. The UI layer implements
//! [`stream::FrameSink`] over whatever transport it has, which keeps the engine
//! testable without a window.

pub mod flow;
pub mod metrics;
pub mod os;
pub mod pty;
pub mod session;
pub mod sftp;
pub mod ssh;
pub mod stream;
pub mod synthetic;

pub use flow::FlowControl;
pub use metrics::{Metrics, MetricsSnapshot};
pub use session::{FilesError, SessionId, SessionManager};
pub use sftp::{CancelToken, Elevation, Entry, EntryKind, SftpError};
pub use ssh::{
    public_key_fingerprint, FileSession, ObservedHostKey, SshAuth, SshConnection, SshError,
    SshTarget,
};
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
