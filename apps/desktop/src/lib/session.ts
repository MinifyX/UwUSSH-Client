/**
 * The IPC surface, in one file.
 *
 * Everything the UI knows about Tauri lives here. If the M0 measurement sends
 * us to the WebSocket fallback, this is the only module that changes.
 */

import { Channel, invoke } from '@tauri-apps/api/core';

export type SessionId = string;

export type MetricsSnapshot = {
  bytesTotal: number;
  framesTotal: number;
  /** How often the PTY reader had to wait for the UI. The M0 number. */
  readerStalls: number;
  largestFrame: number;
  elapsedSecs: number;
  bytesPerSec: number;
  framesPerSec: number;
  meanFrameBytes: number;
};

/**
 * Frames arrive as raw bytes. Depending on the Tauri version they land as an
 * ArrayBuffer or as a plain number array, so normalise both rather than
 * guessing — a wrong guess here shows up as an empty terminal with no error.
 */
function toBytes(message: unknown): Uint8Array {
  if (message instanceof ArrayBuffer) return new Uint8Array(message);
  if (ArrayBuffer.isView(message)) {
    const view = message as ArrayBufferView;
    return new Uint8Array(view.buffer, view.byteOffset, view.byteLength);
  }
  if (Array.isArray(message)) return new Uint8Array(message as number[]);
  return new Uint8Array();
}

export async function spawnLocalSession(
  cols: number,
  rows: number,
  onData: (bytes: Uint8Array) => void,
): Promise<SessionId> {
  const channel = new Channel<unknown>();
  channel.onmessage = (message) => {
    const bytes = toBytes(message);
    if (bytes.length > 0) onData(bytes);
  };

  return invoke<SessionId>('spawn_local_session', { cols, rows, onData: channel });
}

export function writeSession(id: SessionId, data: string): Promise<void> {
  return invoke('write_session', { id, data });
}

export function resizeSession(id: SessionId, cols: number, rows: number): Promise<void> {
  return invoke('resize_session', { id, cols, rows });
}

export function closeSession(id: SessionId): Promise<void> {
  return invoke('close_session', { id });
}

export function sessionMetrics(id: SessionId): Promise<MetricsSnapshot> {
  return invoke<MetricsSnapshot>('session_metrics', { id });
}

/** Flood the session's stdout, so the IPC path has something to choke on. */
export function startLoadTest(id: SessionId): Promise<void> {
  return invoke('start_load_test', { id });
}
