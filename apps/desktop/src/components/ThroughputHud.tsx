import type { MetricsSnapshot } from '../lib/session';

/**
 * The M0 readout.
 *
 * Four numbers answer the question "does the IPC boundary carry a real
 * terminal?": throughput, how many crossings that took, how big each crossing
 * was, and how often the reader had to wait. The last one is the ceiling —
 * stalls mean backpressure reached the shell, which is correct behaviour and
 * also exactly where the limit sits.
 */

function mib(bytes: number): string {
  return (bytes / 1024 / 1024).toFixed(1);
}

function kib(bytes: number): string {
  return (bytes / 1024).toFixed(1);
}

type Props = {
  metrics: MetricsSnapshot | null;
  renderer: 'webgl' | 'canvas' | null;
};

export function ThroughputHud({ metrics, renderer }: Props) {
  if (!metrics) {
    return (
      <div className="hud">
        <span className="stat">warte auf Session…</span>
      </div>
    );
  }

  return (
    <div className="hud">
      <span className="stat" title="Durchsatz über die IPC-Grenze">
        <b>{mib(metrics.bytesPerSec)}</b> MiB/s
      </span>
      <span className="stat" title="Frames pro Sekunde — bei 8-ms-Fenstern sind ~125 das Maximum">
        <b>{metrics.framesPerSec.toFixed(0)}</b> Frames/s
      </span>
      <span className="stat" title="Mittlere Framegröße — klein heißt, das Bündeln greift nicht">
        ø <b>{kib(metrics.meanFrameBytes)}</b> KiB
      </span>
      <span
        className="stat"
        data-alarm={metrics.readerStalls > 0}
        title="Wie oft der PTY-Reader auf die UI warten musste. Das ist die Decke."
      >
        <b>{metrics.readerStalls}</b> Stalls
      </span>
      <span className="stat" title="Gesamt übertragen">
        <b>{mib(metrics.bytesTotal)}</b> MiB gesamt
      </span>
      {renderer && (
        <span
          className="stat"
          data-alarm={renderer === 'canvas'}
          title="Ohne WebGL misst man die Renderer-Grenze statt der IPC-Grenze"
        >
          {renderer}
        </span>
      )}
    </div>
  );
}
