/**
 * Tunnels: local (`ssh -L`) and remote (`ssh -R`) port forwards of a host.
 *
 * The records live in the store and sync like hosts; whether one runs right
 * now lives in Rust, which reports every change as a `tunnel:status` event.
 * `useTunnelStatuses()` keeps the page's picture of that, loaded once and then
 * kept up to date by the events.
 */

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useSyncExternalStore } from 'react';
import { t } from './i18n';

export type TunnelKind = 'local' | 'remote';

export type TunnelRecord = {
  id: string;
  hostId: string;
  name: string;
  /** `local` or `remote` — or a kind a newer UwUSSH added, shown but not run. */
  kind: string;
  bindAddress: string;
  bindPort: number;
  targetHost: string;
  targetPort: number;
  /** Starts along with a terminal to its host. */
  autostart: boolean;
};

export type TunnelDraft = {
  id: string | null;
  hostId: string;
  name: string;
  kind: TunnelKind;
  /** `null` listens on 127.0.0.1. */
  bindAddress: string | null;
  bindPort: number;
  targetHost: string;
  targetPort: number;
  autostart: boolean;
};

/** Mirrors `TunnelError` in `crates/uwussh-core/src/tunnel.rs`. */
export type TunnelError =
  | { kind: 'port-in-use'; address: string; port: number }
  | { kind: 'bind-failed'; address: string; port: number; reason: string }
  | { kind: 'forward-refused'; address: string; port: number }
  | { kind: 'connection-lost' }
  | { kind: 'protocol'; reason: string };

export type TunnelState =
  | { state: 'starting' }
  | { state: 'running'; boundPort: number }
  | { state: 'failed'; error: TunnelError }
  | { state: 'stopped' };

export type TunnelStatus = {
  id: string;
  hostId: string;
  /** The terminal whose connection it runs on, if it started with one. */
  session: string | null;
  connections: number;
  active: number;
  bytesOut: number;
  bytesIn: number;
} & TunnelState;

export const TUNNEL_ERROR_KINDS: readonly TunnelError['kind'][] = [
  'port-in-use',
  'bind-failed',
  'forward-refused',
  'connection-lost',
  'protocol',
];

export function isTunnelError(error: unknown): error is TunnelError {
  return (
    typeof error === 'object' &&
    error !== null &&
    TUNNEL_ERROR_KINDS.includes((error as { kind?: unknown }).kind as TunnelError['kind'])
  );
}

/** What went wrong, in words the user can act on. */
export function describeTunnelError(error: TunnelError): string {
  switch (error.kind) {
    case 'port-in-use':
      return t('Port {port} auf {address} ist schon belegt.', {
        port: error.port,
        address: error.address,
      });
    case 'bind-failed':
      return t('Auf {address}:{port} lässt sich nicht lauschen: {reason}', {
        address: error.address,
        port: error.port,
        reason: error.reason,
      });
    case 'forward-refused':
      return t('Der Server lässt auf {address}:{port} nicht lauschen.', {
        address: error.address,
        port: error.port,
      });
    case 'connection-lost':
      return t('Die Verbindung, auf der der Tunnel lief, ist beendet.');
    case 'protocol':
      return t('SSH-Fehler: {reason}', { reason: error.reason });
  }
}

/** `127.0.0.1:8080 → db:5432`, the way round the data flows. */
export function describeRoute(
  tunnel: Pick<TunnelRecord, 'bindAddress' | 'bindPort' | 'targetHost' | 'targetPort'>,
): string {
  const listen = `${tunnel.bindAddress}:${tunnel.bindPort}`;
  const target = `${tunnel.targetHost}:${tunnel.targetPort}`;
  return `${listen} → ${target}`;
}

export function listTunnels(): Promise<TunnelRecord[]> {
  return invoke<TunnelRecord[]>('list_tunnels');
}

export function saveTunnel(draft: TunnelDraft): Promise<TunnelRecord> {
  return invoke<TunnelRecord>('save_tunnel', { draft });
}

export function deleteTunnel(id: string): Promise<void> {
  return invoke('delete_tunnel', { id });
}

/**
 * Start a tunnel on its own connection. `attempt` names the conversation like
 * a tab's; `secret` answers a password or passphrase question.
 */
export function startTunnel(
  id: string,
  attempt: string,
  secret: string | null,
): Promise<TunnelStatus> {
  return invoke<TunnelStatus>('start_tunnel', { id, attempt, secret });
}

/** Stop a tunnel, or clear one that failed. */
export function stopTunnel(id: string): Promise<boolean> {
  return invoke<boolean>('stop_tunnel', { id });
}

// ── What runs right now ─────────────────────────────────────────────────────

let statuses: ReadonlyMap<string, TunnelStatus> = new Map();
const listeners = new Set<() => void>();
let started = false;

/** Fold one reported status into the picture. A stopped tunnel leaves it. */
export function applyStatus(
  current: ReadonlyMap<string, TunnelStatus>,
  status: TunnelStatus,
): ReadonlyMap<string, TunnelStatus> {
  const next = new Map(current);
  if (status.state === 'stopped') next.delete(status.id);
  else next.set(status.id, status);
  return next;
}

function publish(next: ReadonlyMap<string, TunnelStatus>) {
  statuses = next;
  for (const listener of listeners) listener();
}

function begin() {
  if (started) return;
  started = true;
  void listen<TunnelStatus>('tunnel:status', ({ payload }) =>
    publish(applyStatus(statuses, payload)),
  ).catch(() => undefined);
  void invoke<TunnelStatus[]>('tunnel_statuses')
    .then((all) => {
      let next = statuses;
      for (const status of all) if (!next.has(status.id)) next = applyStatus(next, status);
      publish(next);
    })
    .catch(() => undefined);
}

function subscribe(listener: () => void): () => void {
  begin();
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Every tunnel that runs, starts or failed, by id. */
export function useTunnelStatuses(): ReadonlyMap<string, TunnelStatus> {
  return useSyncExternalStore(subscribe, () => statuses);
}
