//! End-to-end flow control between the session engine and whatever renders it.
//!
//! The first M0 draft only had backpressure between the PTY reader and the
//! batcher. That stops at the IPC boundary: `Channel::send` returns as soon as
//! Tauri has queued a frame, long before the webview has parsed it. A flood
//! could push hundreds of megabytes into the webview's queue while every
//! Rust-side counter looked perfectly healthy.
//!
//! So the renderer acknowledges what it has actually processed, and the batcher
//! stops sending once too much is outstanding. This is the scheme VS Code's
//! terminal uses: pause at a high watermark, resume at a lower one, so the
//! stream does not flap on every single acknowledgement.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::Notify;

/// Outstanding bytes at which the batcher pauses.
pub const HIGH_WATERMARK: u64 = 512 * 1024;

/// Outstanding bytes at which a paused batcher resumes.
pub const LOW_WATERMARK: u64 = 128 * 1024;

/// How much the renderer processes before it acknowledges.
///
/// The renderer may hold back up to one chunk unacknowledged. If that remainder
/// could exceed [`LOW_WATERMARK`], a paused stream would never resume, with both
/// sides waiting on each other — hence the compile-time check below.
pub const ACK_CHUNK: u64 = 64 * 1024;

const _: () = assert!(
    ACK_CHUNK < LOW_WATERMARK,
    "ACK_CHUNK must stay below LOW_WATERMARK"
);

#[derive(Debug)]
pub struct FlowControl {
    enabled: bool,
    unacked: AtomicU64,
    peak_unacked: AtomicU64,
    closed: AtomicBool,
    changed: Notify,
}

impl FlowControl {
    /// A disabled controller still counts outstanding bytes — that is what makes
    /// "with" and "without" comparable in the M0 measurement — it just never
    /// pauses anything.
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            unacked: AtomicU64::new(0),
            peak_unacked: AtomicU64::new(0),
            closed: AtomicBool::new(false),
            changed: Notify::new(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Call *before* handing a frame to the sink, so an acknowledgement can
    /// never arrive for bytes that were not counted yet.
    pub fn on_sent(&self, bytes: u64) {
        let now = self.unacked.fetch_add(bytes, Ordering::Relaxed) + bytes;
        self.peak_unacked.fetch_max(now, Ordering::Relaxed);
    }

    pub fn ack(&self, bytes: u64) {
        // Saturating: a renderer that reloaded and acknowledges twice must not
        // wrap the counter around to eighteen quintillion outstanding bytes.
        let _ = self
            .unacked
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                Some(v.saturating_sub(bytes))
            });
        self.changed.notify_waiters();
    }

    /// Release a waiting batcher for good, e.g. when the session is closed.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
        self.changed.notify_waiters();
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    pub fn unacked(&self) -> u64 {
        self.unacked.load(Ordering::Relaxed)
    }

    pub fn peak_unacked(&self) -> u64 {
        self.peak_unacked.load(Ordering::Relaxed)
    }

    /// Wait until the renderer has caught up. Returns whether it had to wait.
    pub async fn wait_for_capacity(&self) -> bool {
        if !self.enabled || self.unacked() <= HIGH_WATERMARK {
            return false;
        }
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            // Register interest *before* re-checking, so an acknowledgement
            // landing between the check and the await cannot be missed.
            notified.as_mut().enable();
            if self.is_closed() || self.unacked() <= LOW_WATERMARK {
                return true;
            }
            notified.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::time::timeout;

    #[tokio::test]
    async fn below_the_high_watermark_nothing_waits() {
        let flow = FlowControl::new(true);
        flow.on_sent(HIGH_WATERMARK);
        assert!(!flow.wait_for_capacity().await);
    }

    #[tokio::test]
    async fn pauses_above_high_and_resumes_only_below_low() {
        let flow = Arc::new(FlowControl::new(true));
        flow.on_sent(HIGH_WATERMARK + 1);

        let waiter = tokio::spawn({
            let flow = Arc::clone(&flow);
            async move { flow.wait_for_capacity().await }
        });
        // Let it actually park. Acknowledging before that would make it never
        // pause in the first place, which is correct but not what is tested.
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Dropping just under the high watermark is not enough — that is the
        // hysteresis that stops the stream from flapping.
        flow.ack(2);
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(
            !waiter.is_finished(),
            "resumed without reaching the low watermark"
        );

        flow.ack(flow.unacked() - LOW_WATERMARK);
        let waited = timeout(Duration::from_secs(1), waiter)
            .await
            .expect("still paused after reaching the low watermark")
            .expect("waiter task");
        assert!(waited);
    }

    #[tokio::test]
    async fn a_disabled_controller_never_pauses_but_still_counts() {
        let flow = FlowControl::new(false);
        flow.on_sent(10 * HIGH_WATERMARK);
        assert!(!flow.wait_for_capacity().await);
        assert_eq!(flow.peak_unacked(), 10 * HIGH_WATERMARK);
    }

    #[tokio::test]
    async fn closing_releases_a_waiting_batcher() {
        let flow = Arc::new(FlowControl::new(true));
        flow.on_sent(HIGH_WATERMARK * 2);

        let waiter = tokio::spawn({
            let flow = Arc::clone(&flow);
            async move { flow.wait_for_capacity().await }
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
        flow.close();

        timeout(Duration::from_secs(1), waiter)
            .await
            .expect("close did not release the waiter")
            .expect("waiter task");
    }

    #[test]
    fn over_acknowledging_saturates_at_zero() {
        let flow = FlowControl::new(true);
        flow.on_sent(100);
        flow.ack(1_000);
        assert_eq!(flow.unacked(), 0);
    }
}
