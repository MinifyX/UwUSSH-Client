/**
 * The M0 measurement: does the Rust→WebView boundary carry a real terminal?
 *
 * Every scenario pushes the same 64 MiB of coloured log output and times how
 * long it takes until xterm.js has *parsed* the last byte — not until Rust sent
 * it, which is the number that looked fine and meant nothing.
 *
 * The scenarios answer three separate questions:
 *
 * 1. Direct, no flow control — what happens if the IPC channel is trusted to
 *    cope on its own? This is the design as it stood before M0.
 * 2. Direct, with flow control — the path SSH bytes will take, with the
 *    acknowledgement scheme that makes backpressure reach the webview.
 * 3. ConPTY — the local shell path, where Windows' pseudo console sits in
 *    front of everything and may well be the slowest part.
 *
 * A run only counts as complete if every byte was parsed. Speed is not
 * reported for incomplete runs: 64 MiB "in 1.2 seconds" means nothing if most
 * of it never reached the screen.
 */

import type { Renderer, TerminalDriver } from './driver';
import { trackFrames, type FrameStats } from './frames';
import { sessionMetrics, spawnM0Session, type M0Scenario, type MetricsSnapshot } from './session';

export type ScenarioDef = M0Scenario & { id: string; label: string };

const PAYLOAD_MIB = 64;

export const SCENARIOS: ScenarioDef[] = [
  {
    id: 'direct-raw',
    label: 'Direkt, ohne Flow-Control',
    kind: 'synthetic',
    flowControl: false,
    payloadMib: PAYLOAD_MIB,
  },
  {
    id: 'direct-flow',
    label: 'Direkt, mit Flow-Control',
    kind: 'synthetic',
    flowControl: true,
    payloadMib: PAYLOAD_MIB,
  },
  {
    id: 'conpty-flow',
    label: 'ConPTY (type), mit Flow-Control',
    kind: 'pty',
    flowControl: true,
    payloadMib: PAYLOAD_MIB,
  },
];

export type Outcome = 'complete' | 'lost-data' | 'stalled' | 'timeout';

export type ScenarioResult = {
  id: string;
  label: string;
  kind: M0Scenario['kind'];
  flowControl: boolean;
  outcome: Outcome;
  /** Until xterm.js had parsed the last byte it ever parsed. */
  seconds: number;
  /** Only for complete runs. */
  mibPerSec: number | null;
  sentMib: number;
  receivedMib: number;
  writtenMib: number;
  discardedMib: number;
  frames: number;
  meanFrameKib: number;
  /** Sent but not yet parsed, at worst. How far the screen fell behind. */
  peakLagMib: number;
  /** The same as the engine saw it: sent but not yet acknowledged. */
  peakUnackedKib: number;
  flowPauses: number;
  readerStalls: number;
  ui: FrameStats;
};

export type SuiteReport = {
  startedAt: string;
  renderer: Renderer;
  cols: number;
  rows: number;
  devicePixelRatio: number;
  userAgent: string;
  payloadMib: number;
  results: ScenarioResult[];
};

export type Progress = {
  scenario: ScenarioDef;
  index: number;
  total: number;
  seconds: number;
  sentMib: number;
  writtenMib: number;
};

const MIB = 1024 * 1024;
const POLL_MS = 100;
const TIMEOUT_MS = 120_000;
/** ConPTY gives no EOF; once the child is gone, silence this long means done. */
const PTY_IDLE_MS = 750;
/** Source finished but nothing got parsed for this long: it is not coming. */
const STALL_MS = 3_000;

const sleep = (ms: number) => new Promise((resolve) => window.setTimeout(resolve, ms));

export async function runScenario(
  driver: TerminalDriver,
  def: ScenarioDef,
  onProgress: (seconds: number, metrics: MetricsSnapshot) => void,
): Promise<ScenarioResult> {
  driver.term.reset();
  driver.resetCounters();

  const frames = trackFrames();
  const started = performance.now();
  const id = await driver.attach((onData, onEnd) =>
    spawnM0Session(def, driver.term.cols, driver.term.rows, onData, onEnd),
  );

  let peakLag = 0;
  let ending: Exclude<Outcome, 'complete' | 'lost-data'> | null = null;
  let metrics: MetricsSnapshot;

  for (;;) {
    await sleep(POLL_MS);
    metrics = await sessionMetrics(id);
    const now = performance.now();
    peakLag = Math.max(peakLag, metrics.bytesTotal - driver.written);
    onProgress((now - started) / 1000, metrics);

    const sourceDone =
      metrics.finished || (metrics.childExited && now - driver.lastDataAt > PTY_IDLE_MS);
    const allArrived = driver.received >= metrics.bytesTotal;
    const allAccountedFor = driver.written + driver.discarded >= driver.received;

    if (sourceDone && allArrived && allAccountedFor) break;
    if (sourceDone && now - Math.max(driver.lastWrittenAt, driver.lastDataAt) > STALL_MS) {
      ending = 'stalled';
      break;
    }
    if (now - started > TIMEOUT_MS) {
      ending = 'timeout';
      break;
    }
  }

  const ui = frames.stop();
  const seconds = (driver.lastWrittenAt - started) / 1000;
  const snapshot = {
    received: driver.received,
    written: driver.written,
    discarded: driver.discarded,
  };
  await driver.detach();

  const outcome: Outcome =
    ending ??
    (snapshot.discarded > 0 || snapshot.written < metrics.bytesTotal ? 'lost-data' : 'complete');

  return {
    id: def.id,
    label: def.label,
    kind: def.kind,
    flowControl: def.flowControl,
    outcome,
    seconds: round(seconds, 2),
    mibPerSec:
      outcome === 'complete' ? round(metrics.bytesTotal / MIB / Math.max(seconds, 0.001), 1) : null,
    sentMib: round(metrics.bytesTotal / MIB, 1),
    receivedMib: round(snapshot.received / MIB, 1),
    writtenMib: round(snapshot.written / MIB, 1),
    discardedMib: round(snapshot.discarded / MIB, 1),
    frames: metrics.framesTotal,
    meanFrameKib: round(metrics.meanFrameBytes / 1024, 1),
    peakLagMib: round(peakLag / MIB, 1),
    peakUnackedKib: Math.round(metrics.peakUnacked / 1024),
    flowPauses: metrics.flowPauses,
    readerStalls: metrics.readerStalls,
    ui,
  };
}

export async function runSuite(
  driver: TerminalDriver,
  onProgress: (progress: Progress) => void,
  onResults: (results: ScenarioResult[]) => void,
): Promise<SuiteReport> {
  const startedAt = new Date().toISOString();
  const results: ScenarioResult[] = [];

  for (const [index, scenario] of SCENARIOS.entries()) {
    const result = await runScenario(driver, scenario, (seconds, metrics) =>
      onProgress({
        scenario,
        index,
        total: SCENARIOS.length,
        seconds,
        sentMib: metrics.bytesTotal / MIB,
        writtenMib: driver.written / MIB,
      }),
    );
    results.push(result);
    onResults([...results]);
    // Let the garbage collector and the renderer settle before the next run,
    // so one scenario's leftovers do not show up in the next one's numbers.
    await sleep(1_000);
  }

  return {
    startedAt,
    renderer: driver.renderer,
    cols: driver.term.cols,
    rows: driver.term.rows,
    devicePixelRatio: window.devicePixelRatio,
    userAgent: navigator.userAgent,
    payloadMib: PAYLOAD_MIB,
    results,
  };
}

function round(value: number, digits: number): number {
  const factor = 10 ** digits;
  return Math.round(value * factor) / factor;
}
