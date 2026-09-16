//! Throughput counters for the M0 spike.
//!
//! These exist to answer one question: does the Rust→WebView boundary carry a
//! real terminal? The UI polls [`Metrics::snapshot`] a few times a second, so
//! everything here is a relaxed atomic — measuring must not itself cost
//! throughput.
//!
//! Everything counted here is the *sending* side. What the webview actually
//! processed is counted over there and compared in the report; the difference
//! between the two is the lag, and the lag is the whole story.

use crate::flow::FlowControl;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

#[derive(Debug)]
pub struct Metrics {
    bytes_total: AtomicU64,
    frames_total: AtomicU64,
    /// How often the PTY reader found the queue to the batcher full.
    reader_stalls: AtomicU64,
    /// How often the batcher paused because the renderer was too far behind.
    flow_pauses: AtomicU64,
    largest_frame: AtomicU64,
    /// The source is exhausted and every byte has been handed to the sink.
    finished: AtomicBool,
    /// The process behind a PTY session has exited. On Windows this is the
    /// only reliable end signal: ConPTY keeps its output pipe open after the
    /// child is gone, so the reader never sees EOF on its own.
    child_exited: AtomicBool,
    started: Instant,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            bytes_total: AtomicU64::new(0),
            frames_total: AtomicU64::new(0),
            reader_stalls: AtomicU64::new(0),
            flow_pauses: AtomicU64::new(0),
            largest_frame: AtomicU64::new(0),
            finished: AtomicBool::new(false),
            child_exited: AtomicBool::new(false),
            started: Instant::now(),
        }
    }

    pub fn record_frame(&self, bytes: usize) {
        let bytes = bytes as u64;
        self.bytes_total.fetch_add(bytes, Ordering::Relaxed);
        self.frames_total.fetch_add(1, Ordering::Relaxed);
        self.largest_frame.fetch_max(bytes, Ordering::Relaxed);
    }

    pub fn record_stall(&self) {
        self.reader_stalls.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_pause(&self) {
        self.flow_pauses.fetch_add(1, Ordering::Relaxed);
    }

    pub fn mark_finished(&self) {
        self.finished.store(true, Ordering::Relaxed);
    }

    pub fn mark_child_exited(&self) {
        self.child_exited.store(true, Ordering::Relaxed);
    }

    pub fn snapshot(&self, flow: &FlowControl) -> MetricsSnapshot {
        let elapsed = self.started.elapsed().as_secs_f64().max(0.001);
        let bytes = self.bytes_total.load(Ordering::Relaxed);
        let frames = self.frames_total.load(Ordering::Relaxed);

        MetricsSnapshot {
            bytes_total: bytes,
            frames_total: frames,
            reader_stalls: self.reader_stalls.load(Ordering::Relaxed),
            flow_pauses: self.flow_pauses.load(Ordering::Relaxed),
            largest_frame: self.largest_frame.load(Ordering::Relaxed),
            elapsed_secs: elapsed,
            bytes_per_sec: bytes as f64 / elapsed,
            frames_per_sec: frames as f64 / elapsed,
            mean_frame_bytes: if frames == 0 {
                0.0
            } else {
                bytes as f64 / frames as f64
            },
            flow_control: flow.enabled(),
            unacked: flow.unacked(),
            peak_unacked: flow.peak_unacked(),
            finished: self.finished.load(Ordering::Relaxed),
            child_exited: self.child_exited.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricsSnapshot {
    pub bytes_total: u64,
    pub frames_total: u64,
    pub reader_stalls: u64,
    pub flow_pauses: u64,
    pub largest_frame: u64,
    pub elapsed_secs: f64,
    pub bytes_per_sec: f64,
    pub frames_per_sec: f64,
    pub mean_frame_bytes: f64,
    pub flow_control: bool,
    pub unacked: u64,
    pub peak_unacked: u64,
    pub finished: bool,
    pub child_exited: bool,
}
