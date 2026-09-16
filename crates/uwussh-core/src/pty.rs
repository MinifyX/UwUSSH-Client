//! Local PTY sessions.
//!
//! On Windows this is ConPTY, elsewhere a normal pty, both behind
//! `portable-pty`. For M0 this is also the load generator: a local shell
//! running `yes` pushes bytes far faster than any SSH connection will, so if
//! the IPC path survives this, it survives production.

use crate::metrics::Metrics;
use crate::stream::{self, FrameSink};
use crate::{CoreError, Result};
use parking_lot::Mutex;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Size of one read from the pty. Large enough that a flood does not turn into
/// a syscall storm, small enough that an interactive line arrives immediately.
const READ_BUFFER: usize = 64 * 1024;

pub struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Mutex<Box<dyn Write + Send>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
    metrics: Arc<Metrics>,
}

impl PtySession {
    /// Start the user's default shell and stream its output into `sink`.
    pub fn spawn<S: FrameSink>(cols: u16, rows: u16, sink: S) -> Result<Self> {
        Self::spawn_command(default_shell(), cols, rows, sink)
    }

    pub fn spawn_command<S: FrameSink>(
        program: String,
        cols: u16,
        rows: u16,
        sink: S,
    ) -> Result<Self> {
        let size = PtySize { rows, cols, pixel_width: 0, pixel_height: 0 };
        let pair = native_pty_system().openpty(size).map_err(CoreError::Pty)?;

        let cmd = CommandBuilder::new(program);
        let child = pair.slave.spawn_command(cmd).map_err(CoreError::Pty)?;

        // Drop the slave handle: otherwise the pty never reports EOF when the
        // child exits, because we would still be holding the other end open.
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().map_err(CoreError::Pty)?;
        let writer = pair.master.take_writer().map_err(CoreError::Pty)?;

        let metrics = Arc::new(Metrics::new());
        let (tx, rx) = mpsc::channel::<Vec<u8>>(stream::CHANNEL_CAPACITY);

        // The reader has to be a real thread: pty reads are blocking, and
        // parking a tokio worker on one would starve everything else.
        let reader_metrics = Arc::clone(&metrics);
        std::thread::Builder::new()
            .name("uwussh-pty-reader".into())
            .spawn(move || {
                let mut buf = vec![0u8; READ_BUFFER];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            // A full queue means the UI is behind. Blocking
                            // here is the point: backpressure travels back to
                            // the program, exactly as it would on a real tty.
                            if tx.capacity() == 0 {
                                reader_metrics.record_stall();
                            }
                            if tx.blocking_send(buf[..n].to_vec()).is_err() {
                                break;
                            }
                        }
                        Err(err) => {
                            tracing::debug!(?err, "pty reader finished");
                            break;
                        }
                    }
                }
            })
            .map_err(CoreError::Io)?;

        let batcher_metrics = Arc::clone(&metrics);
        tokio::spawn(stream::run_batcher(rx, sink, batcher_metrics));

        Ok(Self {
            master: pair.master,
            writer: Mutex::new(writer),
            child: Mutex::new(child),
            metrics,
        })
    }

    pub fn write(&self, data: &[u8]) -> Result<()> {
        let mut writer = self.writer.lock();
        writer.write_all(data)?;
        writer.flush()?;
        Ok(())
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.master
            .resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .map_err(CoreError::Pty)
    }

    pub fn metrics(&self) -> Arc<Metrics> {
        Arc::clone(&self.metrics)
    }

    pub fn kill(&self) -> Result<()> {
        self.child.lock().kill().map_err(CoreError::Io)
    }
}

/// PowerShell on Windows rather than whatever `COMSPEC` points at, so the load
/// test below has a predictable shell to talk to.
fn default_shell() -> String {
    if cfg!(windows) {
        "powershell.exe".into()
    } else {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into())
    }
}

/// A command that floods stdout, for the M0 measurement. Shell-specific,
/// because Windows has no `yes`.
pub fn load_test_command() -> &'static str {
    if cfg!(windows) {
        "while ($true) { 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx' }\r"
    } else {
        "yes xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\n"
    }
}
