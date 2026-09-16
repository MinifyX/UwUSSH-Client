/**
 * The IPC surface, in one file.
 *
 * Everything the UI knows about Tauri lives here. If the M0 measurement sends
 * us to the WebSocket fallback, this is the only module that changes.
 */

import { Channel, invoke } from '@tauri-apps/api/core';

export type SessionId = string;

/** Must match `ACK_CHUNK` in `crates/uwussh-core/src/flow.rs`. */
export const ACK_CHUNK = 64 * 1024;

export type MetricsSnapshot = {
  bytesTotal: number;
  framesTotal: number;
  readerStalls: number;
  flowPauses: number;
  largestFrame: number;
  elapsedSecs: number;
  bytesPerSec: number;
  framesPerSec: number;
  meanFrameBytes: number;
  flowControl: boolean;
  unacked: number;
  peakUnacked: number;
  finished: boolean;
  childExited: boolean;
};

export type M0Kind = 'synthetic' | 'pty';

export type M0Scenario = {
  kind: M0Kind;
  flowControl: boolean;
  payloadMib: number;
};

export type DataHandler = (bytes: Uint8Array) => void;

/**
 * Frames arrive as raw bytes. Depending on the Tauri version they land as an
 * ArrayBuffer or as a plain number array, so normalise both rather than
 * guessing — a wrong guess here shows up as an empty terminal with no error.
 */
function toBytes(message: unknown): Uint8Array {
  if (message instanceof ArrayBuffer) return new Uint8Array(message);
  if (ArrayBuffer.isView(message)) {
    return new Uint8Array(message.buffer, message.byteOffset, message.byteLength);
  }
  if (Array.isArray(message)) return new Uint8Array(message as number[]);
  return new Uint8Array();
}

function channelFor(onData: DataHandler): Channel<unknown> {
  const channel = new Channel<unknown>();
  channel.onmessage = (message) => {
    const bytes = toBytes(message);
    if (bytes.length > 0) onData(bytes);
  };
  return channel;
}

export function spawnShellSession(
  cols: number,
  rows: number,
  onData: DataHandler,
): Promise<SessionId> {
  return invoke<SessionId>('spawn_shell_session', { cols, rows, onData: channelFor(onData) });
}

export function spawnM0Session(
  scenario: M0Scenario,
  cols: number,
  rows: number,
  onData: DataHandler,
): Promise<SessionId> {
  const { kind, flowControl, payloadMib } = scenario;
  return invoke<SessionId>('spawn_m0_session', {
    scenario: { kind, flowControl, payloadMib },
    cols,
    rows,
    onData: channelFor(onData),
  });
}

export function writeSession(id: SessionId, data: string): Promise<void> {
  return invoke('write_session', { id, data });
}

export function resizeSession(id: SessionId, cols: number, rows: number): Promise<void> {
  return invoke('resize_session', { id, cols, rows });
}

/** Tell the engine the renderer has processed `bytes` more bytes. */
export function ackSession(id: SessionId, bytes: number): Promise<void> {
  return invoke('ack_session', { id, bytes });
}

export function closeSession(id: SessionId): Promise<void> {
  return invoke('close_session', { id });
}

export function sessionMetrics(id: SessionId): Promise<MetricsSnapshot> {
  return invoke<MetricsSnapshot>('session_metrics', { id });
}

export function m0Autorun(): Promise<boolean> {
  return invoke<boolean>('m0_autorun');
}

/** Writes the report file and returns its path. Quits the app on autorun. */
export function m0Finish(report: unknown): Promise<string> {
  return invoke<string>('m0_finish', { report });
}
