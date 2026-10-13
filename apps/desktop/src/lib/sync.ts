/**
 * Settings → Sync, the page's side: connecting a UwUSync server, pairing
 * devices, the device list — or signing in to UwULock instead, and the move
 * from one to the other.
 *
 * Every secret the page types — the master password, the codes — goes
 * straight into one command and is not kept. What comes back holds no secret
 * except the recovery code: right after the account was made, and when asked
 * for again with the master password.
 */

import { invoke } from '@tauri-apps/api/core';
import type { VaultStatus } from './session';

export type SyncReport = {
  pulled: number;
  pushed: number;
  conflicts: number;
  rounds: number;
  apply: { applied: number; rejected: number; hostKeyConflicts: number };
  withheld: Withheld;
};

/**
 * What the other devices' manifests say this one should have and does not:
 * the server is keeping records back or handing out old versions. While
 * `hostKeys` is set, host keys from other devices are not trusted.
 */
export type Withheld = { records: number; hostKeys: boolean };

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
  withheld: Withheld;
  backend: Backend;
  lock: LockStatus;
};

/** Where this device syncs, if anywhere. */
export type Backend = 'none' | 'uwusync' | 'uwulock';

export type LockStatus = {
  serverUrl: string | null;
  email: string | null;
  /** The session ended: the master password is needed to go on syncing. */
  needsSignIn: boolean;
  /** UwULock's realtime channel is open. */
  live: boolean;
  liveRefused: boolean;
  signedInMs: number | null;
  /** A move from UwUSync began and did not finish. */
  moveStartedMs: number | null;
  /** Moved, and this device may still be removed from UwUSync. */
  leftBehind: boolean;
  /** The server has app sync switched off: syncing waits, the session stays. */
  switchedOff: boolean;
};

/** One way of two-step login the UwULock account has set up. */
export type TwoFactorMethod = {
  provider: number;
  kind: 'authenticator' | 'email' | 'yubikey' | 'duo' | 'u2f' | 'webauthn' | 'other';
  supported: boolean;
  /** For email codes: the masked address the code goes to. */
  hint: string | null;
};

export type TwoFactorInput = { provider: number; code: string; remember: boolean };

export type MoveReport = {
  read: number;
  copied: number;
  alreadyThere: number;
  newerThere: number;
  unreadable: number;
};

export type Difference = { id: string; kind: string; problem: 'missing' | 'older' };

export type LockOutcome =
  | { kind: 'signed-in'; madeSpace: boolean }
  | { kind: 'moved'; report: MoveReport; lastDevice: boolean }
  | { kind: 'two-factor'; methods: TwoFactorMethod[]; message: string | null }
  /** Not the space this device used on the account; `now` null: there is none. */
  | { kind: 'space-changed'; was: string; now: string | null };

/** Take the account's space after all: its id, or null to make a new one. */
export type AcceptSpace = { id: string | null };

export type SyncFailure =
  | { kind: 'vault-locked' }
  | { kind: 'password-wrong' }
  | { kind: 'bad-code' }
  | { kind: 'unreachable'; message: string }
  | { kind: 'refused'; message: string }
  | { kind: 'pairing-failed'; message: string }
  | { kind: 'sign-in' }
  | { kind: 'switched-off' }
  | { kind: 'login-refused'; message: string }
  | { kind: 'keys-lost' }
  | { kind: 'no-key-pair' }
  | { kind: 'weaker-kdf'; message: string }
  | { kind: 'space-left' }
  | { kind: 'move-check'; differences: Difference[] }
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

/** One server this device syncs with: a store of its own. The id is opaque. */
export type SpaceInfo = {
  id: string;
  main: boolean;
  kind: 'server';
  /** The server's host name, '' while it is connected to none. */
  label: string;
  /** Its vault could not be opened with the main server's key. */
  locked: boolean;
  readOnly: boolean;
  status: SyncStatus;
};

export type SpacesStatus = {
  /** The main server first. */
  spaces: SpaceInfo[];
  /** Where new entries of each workspace go. */
  defaults: { private: string; business: string };
  /** A second server can be connected now. */
  canAdd: boolean;
};

/**
 * Which server a command means: nothing for the main one, a space's id, or
 * `'new'` for the second server while it is being set up.
 */
export type SpaceRef = string | undefined;
export const NEW_SPACE = 'new';

export const syncStatus = (space?: SpaceRef) => invoke<SyncStatus>('sync_status', { space });

export const spacesStatus = () => invoke<SpacesStatus>('spaces_status');
export const spaceSetDefault = (workspace: 'private' | 'business', space: string) =>
  invoke<void>('space_set_default', { workspace, space });
/** Personal things move to this server, and its password opens the app from then on. */
export const spaceMakeMain = (space: string) => invoke<void>('space_make_main', { space });
/** Open a server's vault with its own password, when the main key did not. */
export const spaceUnlock = (space: string, password: string) =>
  invoke<void>('space_unlock', { space, password });
/** Disconnect a server: `keep` takes its entries over to the remaining one. */
export const spaceRemove = (space: string, password: string, keep: boolean) =>
  invoke<void>('space_remove', { space, password, keep });
/** Throw away the second server that was being set up. */
export const spaceDiscardNew = () => invoke<void>('space_discard_new');

export const syncConnect = (code: string, password: string, name: string, space?: SpaceRef) =>
  invoke<Connected>('sync_connect', { code, password, name, space });

export const syncJoin = (
  code: string,
  password: string,
  name: string,
  serverUrl: string | null,
  fingerprint: string | null,
  space?: SpaceRef,
) => invoke<void>('sync_join', { code, password, name, serverUrl, fingerprint, space });

export const syncOffer = (password: string, space?: SpaceRef) =>
  invoke<Offer>('sync_offer', { password, space });
export const syncWaitForDevice = (space?: SpaceRef) =>
  invoke<{ name: string }>('sync_wait_for_device', { space });
export const syncCancelOffer = (space?: SpaceRef) => invoke<void>('sync_cancel_offer', { space });
export const syncDevices = (space?: SpaceRef) => invoke<Device[]>('sync_devices', { space });
export const syncRevoke = (deviceId: string, password: string, space?: SpaceRef) =>
  invoke<void>('sync_revoke', { deviceId, password, space });
export const syncNow = (space?: SpaceRef) => invoke<void>('sync_now', { space });
/** A pass now, waited for. False when this device doesn't sync. */
export const syncPassNow = (space?: SpaceRef) => invoke<boolean>('sync_pass_now', { space });
export const syncDisconnect = (password: string, space?: SpaceRef) =>
  invoke<void>('sync_disconnect', { password, space });
/** The recovery kit again, on a paired device that kept the account key. */
export const syncRecoveryCode = (password: string, space?: SpaceRef) =>
  invoke<Connected>('sync_recovery_code', { password, space });

/**
 * Sign in to UwULock and sync through it. `twoFactor` once the server asked
 * for a code, `acceptSpace` once the person agreed to another space.
 */
export const lockSignIn = (
  serverUrl: string,
  email: string,
  password: string,
  twoFactor: TwoFactorInput | null,
  acceptSpace: AcceptSpace | null,
  space?: SpaceRef,
) =>
  invoke<LockOutcome>('lock_sign_in', {
    serverUrl,
    email,
    password,
    twoFactor,
    acceptSpace,
    space,
  });

/**
 * Whether the UwULock Server at this address has app sync switched off. False
 * for a server that says nothing about it or can't be reached.
 */
export const lockAppSyncOff = (serverUrl: string, space?: SpaceRef) =>
  invoke<boolean>('lock_app_sync_off', { serverUrl, space });

export const lockSendEmailCode = (
  serverUrl: string,
  email: string,
  password: string,
  space?: SpaceRef,
) => invoke<void>('lock_send_email_code', { serverUrl, email, password, space });

/** The one-click move from UwUSync to UwULock. */
export const lockMove = (
  serverUrl: string,
  email: string,
  password: string,
  twoFactor: TwoFactorInput | null,
  acceptSpace: AcceptSpace | null,
  space?: SpaceRef,
) =>
  invoke<LockOutcome>('lock_move', { serverUrl, email, password, twoFactor, acceptSpace, space });

/** After the move: remove this device from UwUSync, or leave it listed there. */
export const lockLeaveUwusync = (revoke: boolean, space?: SpaceRef) =>
  invoke<void>('lock_leave_uwusync', { revoke, space });
export const lockForgetMove = (space?: SpaceRef) => invoke<void>('lock_forget_move', { space });
export const lockSignOut = (password: string, space?: SpaceRef) =>
  invoke<void>('lock_sign_out', { password, space });

/** A pasted setup code, from `uwusync-server invite`. */
export const isSetupCode = (text: string) => text.trim().startsWith('uwu1_');
/** A pasted pairing code, from "add a device" on another device. */
export const isPasteablePairing = (text: string) => text.trim().startsWith('uwu2_');
