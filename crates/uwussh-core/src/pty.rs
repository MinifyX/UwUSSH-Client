//! Local PTY sessions.
//!
//! On Windows this is ConPTY, elsewhere a normal pty, both behind
//! `portable-pty`.
//!
//! One Windows quirk shapes this file: ConPTY keeps its output pipe open after
//! the child process has exited, so the reader never sees EOF on its own. The
//! end of a session is therefore detected by waiting on the child, not on the
//! pipe, and the pipe only closes once the session — and with it the pseudo
//! console — is dropped.

use crate::flow::FlowControl;
use crate::metrics::{Metrics, MetricsSnapshot};
use crate::stream::{self, FrameSink};
use crate::{CoreError, Result};
use parking_lot::Mutex;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Size of one read from the pty. Large enough that a flood does not turn into
/// a syscall storm, small enough that an interactive line arrives immediately.
const READ_BUFFER: usize = 64 * 1024;

pub struct PtySession {
    /// `Box<dyn MasterPty + Send>` is `Send` but not `Sync`, and the session
    /// map is shared across threads — so the handle lives behind a lock. It is
    /// only touched on resize, so the lock is never contended.
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    /// The child itself lives on the watcher thread, blocked in `wait()`; this
    /// is the handle that can still end it from here.
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    metrics: Arc<Metrics>,
    flow: Arc<FlowControl>,
}

impl PtySession {
    /// Start the user's default shell.
    pub fn spawn_shell<S: FrameSink>(cols: u16, rows: u16, sink: S) -> Result<Self> {
        Self::spawn(CommandBuilder::new(default_shell()), cols, rows, true, sink)
    }

    /// Run one program in a pty — for M0, `type bigfile` as the realistic
    /// "someone cats a huge log" case.
    pub fn spawn_command<S: FrameSink>(
        program: &str,
        args: &[String],
        cols: u16,
        rows: u16,
        flow_control: bool,
        sink: S,
    ) -> Result<Self> {
        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);
        Self::spawn(cmd, cols, rows, flow_control, sink)
    }

    fn spawn<S: FrameSink>(
        cmd: CommandBuilder,
        cols: u16,
        rows: u16,
        flow_control: bool,
        sink: S,
    ) -> Result<Self> {
        let pair = native_pty_system()
            .openpty(size(cols, rows))
            .map_err(CoreError::Pty)?;

        let mut child = pair.slave.spawn_command(cmd).map_err(CoreError::Pty)?;
        let killer = child.clone_killer();

        // Drop the slave handle: holding it would keep our own end of the pty
        // open after the child is gone.
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().map_err(CoreError::Pty)?;
        let writer = pair.master.take_writer().map_err(CoreError::Pty)?;

        let metrics = Arc::new(Metrics::new());
        let flow = Arc::new(FlowControl::new(flow_control));
        let (tx, rx) = mpsc::channel::<Vec<u8>>(stream::CHANNEL_CAPACITY);

        let watcher_metrics = Arc::clone(&metrics);
        std::thread::Builder::new()
            .name("uwussh-pty-child".into())
            .spawn(move || {
                let status = child.wait();
                tracing::debug!(?status, "pty child exited");
                watcher_metrics.mark_child_exited();
            })
            .map_err(CoreError::Io)?;

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
                            // A full queue means everything downstream is
                            // behind. Blocking here is the point: backpressure
                            // travels back to the program, as on a real tty.
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

        tokio::spawn(stream::run_batcher(
            rx,
            sink,
            Arc::clone(&metrics),
            Arc::clone(&flow),
        ));

        Ok(Self {
            master: Mutex::new(pair.master),
            writer: Mutex::new(writer),
            killer: Mutex::new(killer),
            metrics,
            flow,
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
            .lock()
            .resize(size(cols, rows))
            .map_err(CoreError::Pty)
    }

    pub fn metrics(&self) -> MetricsSnapshot {
        self.metrics.snapshot(&self.flow)
    }

    pub fn ack(&self, bytes: u64) {
        self.flow.ack(bytes);
    }

    pub fn close(&self) {
        // A dead child is not an error here — the user may simply have typed
        // `exit` before hitting the close button.
        let _ = self.killer.lock().kill();
        self.flow.close();
    }
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// PowerShell on Windows rather than whatever `COMSPEC` points at.
fn default_shell() -> String {
    if cfg!(windows) {
        "powershell.exe".into()
    } else {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into())
    }
}
