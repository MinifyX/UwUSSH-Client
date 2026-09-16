//! Turning a byte stream into frames the UI can survive.
//!
//! A PTY hands you bytes in whatever sizes the OS feels like — often a few
//! hundred at a time. Forwarding each read across the IPC boundary means tens
//! of thousands of crossings per second, and that is what kills terminal
//! throughput in a webview app. So we coalesce: collect for [`FRAME_INTERVAL`],
//! then send one frame.
//!
//! ## A correction to the concept
//!
//! `KONZEPT.md` originally said we would drop frames on overflow and let
//! xterm.js keep the scrollback truth. That is wrong, and building it made it
//! obvious: dropping bytes *before* xterm.js corrupts the stream — escape
//! sequences get cut in half, and the buffer ends up holding garbage rather
//! than truth.
//!
//! What a real terminal does instead is apply backpressure. The bounded channel
//! here does exactly that: when the UI falls behind, the reader blocks, the PTY
//! buffer fills, and the program on the other end slows down — which is what
//! already happens when you `cat` a huge file into a slow terminal. We count
//! those stalls instead of dropping data, and the stall count is the real
//! throughput ceiling M0 is looking for.

use crate::metrics::Metrics;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::time::timeout;

/// How long to collect before emitting a frame. 8 ms is roughly half a frame at
/// 60 Hz: short enough that typing feels immediate, long enough to bundle a
/// flood into a handful of crossings per frame.
pub const FRAME_INTERVAL: Duration = Duration::from_millis(8);

/// Upper bound on a single frame, so one burst cannot produce a multi-megabyte
/// message that blocks the UI thread while it is decoded.
pub const MAX_FRAME_BYTES: usize = 256 * 1024;

/// Queue depth between reader and batcher. Full queue means backpressure.
pub const CHANNEL_CAPACITY: usize = 64;

#[derive(Debug, thiserror::Error)]
pub enum SinkError {
    #[error("the UI channel is gone")]
    Closed,
    #[error("{0}")]
    Other(String),
}

/// Where finished frames go. Implemented by the Tauri layer over an IPC
/// channel; implemented by tests over a `Vec`.
pub trait FrameSink: Send + 'static {
    fn send(&self, frame: &[u8]) -> Result<(), SinkError>;
}

/// Consume raw chunks, emit coalesced frames, keep score.
///
/// Returns when the source closes or the sink goes away.
pub async fn run_batcher<S: FrameSink>(
    mut rx: mpsc::Receiver<Vec<u8>>,
    sink: S,
    metrics: Arc<Metrics>,
) {
    loop {
        // Block until there is anything at all — an idle terminal should cost
        // nothing, not wake up 125 times a second.
        let Some(first) = rx.recv().await else {
            return;
        };

        let mut frame = first;
        let deadline = Instant::now() + FRAME_INTERVAL;

        // Keep absorbing until the window closes or the frame is big enough.
        while frame.len() < MAX_FRAME_BYTES {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match timeout(remaining, rx.recv()).await {
                Ok(Some(chunk)) => frame.extend_from_slice(&chunk),
                // Source closed: flush what we have, then stop.
                Ok(None) => {
                    if !frame.is_empty() {
                        metrics.record_frame(frame.len());
                        let _ = sink.send(&frame);
                    }
                    return;
                }
                Err(_elapsed) => break,
            }
        }

        metrics.record_frame(frame.len());
        if sink.send(&frame).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;

    #[derive(Clone, Default)]
    struct Collector(Arc<Mutex<Vec<Vec<u8>>>>);

    impl FrameSink for Collector {
        fn send(&self, frame: &[u8]) -> Result<(), SinkError> {
            self.0.lock().push(frame.to_vec());
            Ok(())
        }
    }

    #[tokio::test]
    async fn many_small_chunks_become_few_frames() {
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        let sink = Collector::default();
        let metrics = Arc::new(Metrics::new());

        let batcher = tokio::spawn(run_batcher(rx, sink.clone(), metrics.clone()));

        // 200 tiny writes, as a chatty program would produce.
        for _ in 0..200 {
            tx.send(b"tick ".to_vec()).await.expect("send");
        }
        drop(tx);
        batcher.await.expect("batcher");

        let frames = sink.0.lock();
        assert!(!frames.is_empty(), "nothing was emitted");
        assert!(
            frames.len() < 200,
            "coalescing did nothing: {} frames for 200 chunks",
            frames.len()
        );

        let total: usize = frames.iter().map(Vec::len).sum();
        assert_eq!(total, 200 * 5, "coalescing must never lose a byte");
    }

    #[tokio::test]
    async fn bytes_arrive_in_order() {
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        let sink = Collector::default();
        let metrics = Arc::new(Metrics::new());
        let batcher = tokio::spawn(run_batcher(rx, sink.clone(), metrics));

        for i in 0..50u8 {
            tx.send(vec![i]).await.expect("send");
        }
        drop(tx);
        batcher.await.expect("batcher");

        let joined: Vec<u8> = sink.0.lock().iter().flatten().copied().collect();
        assert_eq!(joined, (0..50u8).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn a_single_huge_burst_is_split_at_the_frame_cap() {
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        let sink = Collector::default();
        let metrics = Arc::new(Metrics::new());
        let batcher = tokio::spawn(run_batcher(rx, sink.clone(), metrics));

        for _ in 0..8 {
            tx.send(vec![b'x'; 100 * 1024]).await.expect("send");
        }
        drop(tx);
        batcher.await.expect("batcher");

        let frames = sink.0.lock();
        for frame in frames.iter() {
            assert!(
                frame.len() <= MAX_FRAME_BYTES + 100 * 1024,
                "frame of {} bytes blew past the cap",
                frame.len()
            );
        }
    }
}
