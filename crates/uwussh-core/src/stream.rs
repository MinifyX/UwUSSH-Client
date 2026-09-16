//! Turning a byte stream into frames the UI can survive.
//!
//! A PTY hands you bytes in whatever sizes the OS feels like — often a few
//! hundred at a time. Forwarding each read across the IPC boundary means tens
//! of thousands of crossings per second, and that is what kills terminal
//! throughput in a webview app. So we coalesce: collect for [`FRAME_INTERVAL`],
//! then send one frame.
//!
//! ## Nothing is ever dropped
//!
//! `KONZEPT.md` v0.1 said frames would be dropped on overflow, with xterm.js
//! keeping the scrollback truth. That is wrong: dropping bytes *before* xterm.js
//! cuts escape sequences in half, and the buffer then holds garbage rather than
//! truth.
//!
//! A real terminal applies backpressure instead, and so does this — in two
//! stages. The bounded channel makes the reader wait when the batcher is behind,
//! and [`FlowControl`] makes the batcher wait when the *renderer* is behind.
//! The second stage is the one that matters: without it, backpressure stopped
//! at the IPC boundary, because handing a frame to Tauri says nothing about
//! whether the webview has processed it.

use crate::flow::FlowControl;
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
/// Returns when the source closes, the sink goes away, or the session is
/// closed through `flow`.
pub async fn run_batcher<S: FrameSink>(
    mut rx: mpsc::Receiver<Vec<u8>>,
    sink: S,
    metrics: Arc<Metrics>,
    flow: Arc<FlowControl>,
) {
    loop {
        // Not reading while paused is the point: the queue fills, the reader
        // blocks, and the program on the far end slows down.
        if flow.wait_for_capacity().await {
            metrics.record_pause();
        }
        if flow.is_closed() {
            break;
        }

        // Block until there is anything at all — an idle terminal should cost
        // nothing, not wake up 125 times a second.
        let Some(first) = rx.recv().await else {
            break;
        };

        let mut frame = first;
        let deadline = Instant::now() + FRAME_INTERVAL;
        let mut source_closed = false;

        // Keep absorbing until the window closes or the frame is big enough.
        while frame.len() < MAX_FRAME_BYTES {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match timeout(remaining, rx.recv()).await {
                Ok(Some(chunk)) => frame.extend_from_slice(&chunk),
                Ok(None) => {
                    source_closed = true;
                    break;
                }
                Err(_elapsed) => break,
            }
        }

        flow.on_sent(frame.len() as u64);
        metrics.record_frame(frame.len());
        if sink.send(&frame).is_err() || source_closed {
            break;
        }
    }
    metrics.mark_finished();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::{ACK_CHUNK, HIGH_WATERMARK};
    use parking_lot::Mutex;

    #[derive(Clone, Default)]
    struct Collector(Arc<Mutex<Vec<Vec<u8>>>>);

    impl Collector {
        fn total(&self) -> usize {
            self.0.lock().iter().map(Vec::len).sum()
        }
    }

    impl FrameSink for Collector {
        fn send(&self, frame: &[u8]) -> Result<(), SinkError> {
            self.0.lock().push(frame.to_vec());
            Ok(())
        }
    }

    /// Hands frames to a separate "renderer" task, like the IPC channel does.
    struct QueueSink(mpsc::UnboundedSender<Vec<u8>>);

    impl FrameSink for QueueSink {
        fn send(&self, frame: &[u8]) -> Result<(), SinkError> {
            self.0.send(frame.to_vec()).map_err(|_| SinkError::Closed)
        }
    }

    fn no_flow_control() -> Arc<FlowControl> {
        Arc::new(FlowControl::new(false))
    }

    #[tokio::test]
    async fn many_small_chunks_become_few_frames() {
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        let sink = Collector::default();
        let metrics = Arc::new(Metrics::new());

        let batcher = tokio::spawn(run_batcher(
            rx,
            sink.clone(),
            metrics.clone(),
            no_flow_control(),
        ));

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
        let batcher = tokio::spawn(run_batcher(
            rx,
            sink.clone(),
            Arc::new(Metrics::new()),
            no_flow_control(),
        ));

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
        let batcher = tokio::spawn(run_batcher(
            rx,
            sink.clone(),
            Arc::new(Metrics::new()),
            no_flow_control(),
        ));

        for _ in 0..8 {
            tx.send(vec![b'x'; 100 * 1024]).await.expect("send");
        }
        drop(tx);
        batcher.await.expect("batcher");

        for frame in sink.0.lock().iter() {
            assert!(
                frame.len() <= MAX_FRAME_BYTES + 100 * 1024,
                "frame of {} bytes blew past the cap",
                frame.len()
            );
        }
    }

    #[tokio::test]
    async fn a_renderer_that_never_acknowledges_caps_what_is_sent() {
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        let sink = Collector::default();
        let flow = Arc::new(FlowControl::new(true));
        let batcher = tokio::spawn(run_batcher(
            rx,
            sink.clone(),
            Arc::new(Metrics::new()),
            flow.clone(),
        ));

        let feeder = tokio::spawn(async move {
            for _ in 0..64 {
                if tx.send(vec![b'x'; 64 * 1024]).await.is_err() {
                    break;
                }
            }
        });

        tokio::time::sleep(Duration::from_millis(200)).await;
        let sent = sink.total() as u64;
        assert!(sent > 0, "nothing was sent at all");
        assert!(
            sent <= HIGH_WATERMARK + MAX_FRAME_BYTES as u64,
            "sent {sent} bytes to a renderer that acknowledged none of them"
        );

        flow.close();
        batcher.await.expect("batcher");
        feeder.await.expect("feeder");
    }

    #[tokio::test]
    async fn a_slow_renderer_gets_every_byte_without_deadlock() {
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        let (frame_tx, mut frame_rx) = mpsc::unbounded_channel();
        let flow = Arc::new(FlowControl::new(true));
        let metrics = Arc::new(Metrics::new());
        let batcher = tokio::spawn(run_batcher(
            rx,
            QueueSink(frame_tx),
            metrics.clone(),
            flow.clone(),
        ));

        // Acknowledges in chunks and holds back the remainder, exactly like the
        // real renderer — which is what would deadlock if ACK_CHUNK were too big.
        let renderer = tokio::spawn({
            let flow = Arc::clone(&flow);
            async move {
                let mut total = 0usize;
                let mut pending = 0u64;
                while let Some(frame) = frame_rx.recv().await {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    total += frame.len();
                    pending += frame.len() as u64;
                    if pending >= ACK_CHUNK {
                        flow.ack(pending);
                        pending = 0;
                    }
                }
                total
            }
        });

        // Odd sizes, so the renderer ends up holding an unacknowledged remainder.
        let chunk = 50_000;
        let chunks = 90;
        for _ in 0..chunks {
            tx.send(vec![b'y'; chunk]).await.expect("send");
        }
        drop(tx);

        timeout(Duration::from_secs(10), batcher)
            .await
            .expect("batcher deadlocked")
            .expect("batcher");
        let total = renderer.await.expect("renderer");

        assert_eq!(total, chunk * chunks, "flow control lost bytes");
        assert!(
            metrics.snapshot(&flow).flow_pauses > 0,
            "a renderer this slow should have forced at least one pause"
        );
    }
}
