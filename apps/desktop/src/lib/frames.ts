/**
 * Does the UI stay usable while a flood is on screen?
 *
 * Throughput alone does not answer that: a terminal can swallow 100 MiB/s and
 * still freeze the window for half a second at a time. So alongside the byte
 * counters, count the gaps between animation frames. At 60 Hz a healthy gap is
 * ~17 ms; anything past 50 ms is visible stutter, past 250 ms a freeze.
 */

export type FrameStats = {
  frames: number;
  maxGapMs: number;
  over50ms: number;
  over250ms: number;
};

export function trackFrames(): { stop(): FrameStats } {
  const stats: FrameStats = { frames: 0, maxGapMs: 0, over50ms: 0, over250ms: 0 };
  let last = performance.now();
  let running = true;

  const tick = (now: number) => {
    const gap = now - last;
    last = now;
    stats.frames += 1;
    stats.maxGapMs = Math.max(stats.maxGapMs, gap);
    if (gap > 50) stats.over50ms += 1;
    if (gap > 250) stats.over250ms += 1;
    if (running) requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);

  return {
    stop() {
      running = false;
      return { ...stats, maxGapMs: Math.round(stats.maxGapMs) };
    },
  };
}
