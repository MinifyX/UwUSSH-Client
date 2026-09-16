//! A load source that needs no shell and no test host.
//!
//! It produces what a busy server log looks like in a terminal — timestamps,
//! coloured levels, request lines, the odd line long enough to wrap — as fast
//! as the pipeline will take it.
//!
//! It stands in for an SSH session in the M0 measurement. SSH bytes reach the
//! batcher straight from the socket; so do these. A local shell on Windows goes
//! through ConPTY first, which re-renders everything and has limits of its own,
//! so measuring only through a PTY would blame the IPC boundary for ConPTY's
//! ceiling.
//!
//! Escape sequences are deliberate: plain ASCII is the cheapest thing xterm.js
//! can parse, and a benchmark made of it would flatter every number.

use crate::flow::FlowControl;
use crate::metrics::{Metrics, MetricsSnapshot};
use crate::stream::{self, FrameSink};
use std::io::{self, Write};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Chunk size the generator pushes into the batcher queue. Roughly what one
/// socket read delivers under load.
const CHUNK_BYTES: usize = 64 * 1024;

/// Deterministic log lines: the same run produces the same bytes, so two
/// measurements differ because the code changed, not because the input did.
#[derive(Debug, Default)]
pub struct LogLines {
    n: u64,
}

impl LogLines {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn write_line(&mut self, out: &mut Vec<u8>) {
        let n = self.n;
        self.n += 1;

        let (level, colour, status) = if n % 97 == 0 {
            ("ERROR", "31", 502)
        } else if n % 17 == 0 {
            (" WARN", "33", 404)
        } else {
            (" INFO", "32", 200)
        };

        // Writing into a Vec cannot fail.
        let _ = write!(
            out,
            "\x1b[2m2026-09-16T10:{min:02}:{sec:02}.{ms:03}Z\x1b[0m \x1b[{colour}m{level}\x1b[0m \
             \x1b[36mhttp\x1b[0m GET /api/v1/hosts/{host} status=\x1b[1m{status}\x1b[0m \
             dur={dur}ms req={req:016x}",
            min = (n / 60_000) % 60,
            sec = (n / 1_000) % 60,
            ms = n % 1_000,
            host = n % 240,
            dur = (n * 7) % 900,
            req = n.wrapping_mul(0x9E37_79B9_7F4A_7C15),
        );

        // Every 31st line is long enough to wrap on a normal terminal width,
        // because reflowing wrapped lines is real work for the renderer.
        if n % 31 == 0 {
            let _ = write!(
                out,
                " \x1b[35mtrace\x1b[0m={a:016x}{b:016x}{c:016x}{d:016x}",
                a = n.wrapping_mul(0xA24B_AED4_963E_E407),
                b = n.wrapping_mul(0x9FB2_1C65_1E98_DF25),
                c = n.rotate_left(17),
                d = !n,
            );
        }

        out.extend_from_slice(b"\r\n");
    }
}

/// Push at least `total_bytes` of log output into `tx`, as fast as it is taken.
pub async fn generate(tx: mpsc::Sender<Vec<u8>>, total_bytes: usize) {
    let mut lines = LogLines::new();
    let mut produced = 0;

    while produced < total_bytes {
        let mut chunk = Vec::with_capacity(CHUNK_BYTES + 512);
        while chunk.len() < CHUNK_BYTES {
            lines.write_line(&mut chunk);
        }
        produced += chunk.len();
        if tx.send(chunk).await.is_err() {
            return;
        }
    }
}

/// Write the same log output to a file, for the `type` / `cat` PTY scenario.
pub fn write_flood_file(path: &Path, total_bytes: usize) -> io::Result<()> {
    let mut file = io::BufWriter::new(std::fs::File::create(path)?);
    let mut lines = LogLines::new();
    let mut buf = Vec::with_capacity(CHUNK_BYTES + 512);
    let mut written = 0;

    while written < total_bytes {
        buf.clear();
        while buf.len() < CHUNK_BYTES {
            lines.write_line(&mut buf);
        }
        file.write_all(&buf)?;
        written += buf.len();
    }
    file.flush()
}

/// A session whose output is generated rather than read from a process.
pub struct SyntheticSession {
    metrics: Arc<Metrics>,
    flow: Arc<FlowControl>,
}

impl SyntheticSession {
    pub fn spawn<S: FrameSink>(total_bytes: usize, flow_control: bool, sink: S) -> Self {
        let (tx, rx) = mpsc::channel(stream::CHANNEL_CAPACITY);
        let metrics = Arc::new(Metrics::new());
        let flow = Arc::new(FlowControl::new(flow_control));

        tokio::spawn(generate(tx, total_bytes));
        tokio::spawn(stream::run_batcher(
            rx,
            sink,
            Arc::clone(&metrics),
            Arc::clone(&flow),
        ));

        Self { metrics, flow }
    }

    pub fn metrics(&self) -> MetricsSnapshot {
        self.metrics.snapshot(&self.flow)
    }

    pub fn ack(&self, bytes: u64) {
        self.flow.ack(bytes);
    }

    pub fn close(&self) {
        self.flow.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_end_in_crlf_because_there_is_no_tty_to_add_the_cr() {
        let mut lines = LogLines::new();
        let mut out = Vec::new();
        for _ in 0..100 {
            lines.write_line(&mut out);
        }
        let text = String::from_utf8(out).expect("utf-8");
        assert_eq!(text.matches("\r\n").count(), 100);
        assert_eq!(text.matches('\n').count(), 100, "a bare LF slipped in");
    }

    #[test]
    fn output_is_deterministic() {
        let render = || {
            let mut lines = LogLines::new();
            let mut out = Vec::new();
            for _ in 0..500 {
                lines.write_line(&mut out);
            }
            out
        };
        assert_eq!(render(), render());
    }

    #[test]
    fn the_payload_is_not_plain_ascii() {
        let mut lines = LogLines::new();
        let mut out = Vec::new();
        for _ in 0..100 {
            lines.write_line(&mut out);
        }
        let escapes = out.iter().filter(|&&b| b == 0x1b).count();
        assert!(
            escapes > 500,
            "only {escapes} escape sequences in 100 lines"
        );
    }

    #[tokio::test]
    async fn the_generator_delivers_at_least_what_was_asked_for() {
        let (tx, mut rx) = mpsc::channel(4);
        let wanted = 300_000;
        let producer = tokio::spawn(generate(tx, wanted));

        let mut got = 0;
        while let Some(chunk) = rx.recv().await {
            got += chunk.len();
        }
        producer.await.expect("generator");
        assert!(got >= wanted);
        assert!(got < wanted + 2 * CHUNK_BYTES, "overshot by far too much");
    }
}
