/**
 * Settings → Sync, the page's side: connecting a UwUSSH server, pairing
 * devices, the device list.
 *
 * Every secret the page types — the master password, the codes — goes
 * straight into one command and is not kept. What comes back holds no secret
 * except the recovery code, once, right after the account was made.
 */

import { invoke } from '@tauri-apps/api/core';
import type { VaultStatus } from './session';

export type SyncReport = {
  pulled: number;
  pushed: number;
  conflicts: number;
  rounds: number;
  apply: { applied: number; rejected: number; hostKeyConflicts: number };
};

export type SyncStatus = {
  paired: boolean;
  serverUrl: string | null;
  tlsFingerprint: string | null;
  deviceId: string | null;
  pairedMs: number | null;
  lastSyncMs: number | null;
  last: { atMs: number; report: SyncReport | null; error: string | null } | null;
  pending: number;
  running: boolean;
  vault: VaultStatus;
  deviceName: string;
  offering: boolean;
};

export type SyncFailure =
  | { kind: 'vault-locked' }
  | { kind: 'password-wrong' }
  | { kind: 'bad-code' }
  | { kind: 'unreachable'; message: string }
  | { kind: 'refused'; message: string }
  | { kind: 'pairing-failed'; message: string }
  | { kind: 'error'; message: string };

export function asSyncFailure(error: unknown): SyncFailure {
  if (typeof error === 'object' && error !== null && 'kind' in error) {
    return error as SyncFailure;
  }
  return { kind: 'error', message: String(error) };
}

export type Connected = {
  recoveryCode: string;
  serverUrl: string;
  tlsFingerprint: string | null;
};

export type Offer = {
  spoken: string;
  pasteable: string;
  serverUrl: string;
  tlsFingerprint: string | null;
  expiresMs: number;
};

export type Device = {
  id: string;
  name: string;
  createdMs: number;
  lastSeenMs: number | null;
  revokedMs: number | null;
  current: boolean;
};

export const syncStatus = () => invoke<SyncStatus>('sync_status');

export const syncConnect = (code: string, password: string, name: string) =>
  invoke<Connected>('sync_connect', { code, password, name });

export const syncJoin = (
  code: string,
  password: string,
  name: string,
  serverUrl: string | null,
  fingerprint: string | null,
) => invoke<void>('sync_join', { code, password, name, serverUrl, fingerprint });

export const syncOffer = () => invoke<Offer>('sync_offer');
export const syncWaitForDevice = () => invoke<{ name: string }>('sync_wait_for_device');
export const syncCancelOffer = () => invoke<void>('sync_cancel_offer');
export const syncDevices = () => invoke<Device[]>('sync_devices');
export const syncRevoke = (deviceId: string, password: string) =>
  invoke<void>('sync_revoke', { deviceId, password });
export const syncNow = () => invoke<void>('sync_now');
export const syncDisconnect = (password: string) => invoke<void>('sync_disconnect', { password });

/** A pasted setup code, from `uwussh-server invite`. */
export const isSetupCode = (text: string) => text.trim().startsWith('uwu1_');
/** A pasted pairing code, from "add a device" on another device. */
export const isPasteablePairing = (text: string) => text.trim().startsWith('uwu2_');
