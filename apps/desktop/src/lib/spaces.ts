/**
 * Two servers: what the pages work out from `spaces_status` before they show
 * anything. A space is one server this device syncs with; with one space the
 * app looks as it always did, and nothing here adds a control.
 *
 * No Tauri in here, so `node --test` can run it.
 */

import type { Workspace } from './session';
import type { SpaceInfo, SpacesStatus, SyncStatus } from './sync';

/** A second server is connected: only then do the pages say "server" at all. */
export function hasTwoServers(status: SpacesStatus | null): boolean {
  return status !== null && status.spaces.length > 1;
}

/** The host part of a server address, without scheme, path or the default port. */
export function serverHost(url: string | null): string {
  const text = url?.trim() ?? '';
  if (!text) return '';
  const rest = text.replace(/^[a-z][a-z0-9+.-]*:\/\//i, '');
  const host = rest.split(/[/?#]/)[0] ?? '';
  return host.replace(/:443$/, '');
}

/**
 * What a server is called: the label the backend gives, else the host of the
 * address it knows, else nothing (the page then says "not connected").
 */
export function spaceName(space: SpaceInfo): string {
  if (space.label.trim()) return space.label.trim();
  return serverHost(space.status.serverUrl ?? space.status.lock.serverUrl);
}

export function mainSpace(status: SpacesStatus): SpaceInfo | null {
  return status.spaces.find((space) => space.main) ?? status.spaces[0] ?? null;
}

/** The server that stays when `id` goes. */
export function otherSpace(status: SpacesStatus, id: string): SpaceInfo | null {
  return status.spaces.find((space) => space.id !== id) ?? null;
}

/** Where a new entry of this workspace goes; the main server if the default is gone. */
export function defaultSpace(status: SpacesStatus, workspace: Workspace): string | null {
  const wanted = status.defaults[workspace];
  if (status.spaces.some((space) => space.id === wanted)) return wanted;
  return mainSpace(status)?.id ?? null;
}

/** A server takes entries only while its vault is open and it may be written. */
export function takesEntries(space: SpaceInfo): boolean {
  return !space.locked && !space.readOnly;
}

/**
 * The server the host dialog shows as chosen. Someone who picked one keeps
 * it; a stored host shows where it lives; a new one follows the default of
 * the workspace it is being filed under – or the other server while that
 * one cannot take it.
 */
export function chosenSpace(
  status: SpacesStatus,
  host: { space: string } | null,
  workspace: Workspace,
  picked: string | null,
): string | null {
  const find = (id: string | null | undefined) =>
    id == null ? null : (status.spaces.find((space) => space.id === id) ?? null);
  const stored = find(host?.space);
  if (host && stored) return find(picked)?.id ?? stored.id;
  const wanted = find(picked) ?? find(defaultSpace(status, workspace));
  if (wanted && takesEntries(wanted)) return wanted.id;
  return status.spaces.find(takesEntries)?.id ?? wanted?.id ?? null;
}

/** Saving would move a stored host to another server. */
export function movesHost(host: { space: string } | null, chosen: string | null): boolean {
  return host !== null && chosen !== null && host.space !== chosen;
}

/**
 * What `save_host` gets as `space`: nothing with one server (the backend
 * files it as before), the chosen server with two.
 */
export function draftSpace(status: SpacesStatus | null, chosen: string | null): string | undefined {
  if (!hasTwoServers(status) || chosen === null) return undefined;
  return chosen;
}

/**
 * The server that keeps a stored host from moving, if any: a move needs both
 * vaults open, since the host's secrets are sealed again under the other one.
 */
export function moveBlocker(status: SpacesStatus): SpaceInfo | null {
  return status.spaces.find((space) => !takesEntries(space)) ?? null;
}

/**
 * A server shows its "locked, type this server's password" page only while
 * the app itself is unlocked: with the main vault shut, opening that one
 * comes first and usually opens the other with it.
 */
export function asksOwnPassword(space: SpaceInfo, status: SpacesStatus): boolean {
  if (space.main || !space.locked) return false;
  return mainSpace(status)?.status.vault === 'unlocked';
}

/** Making this server the main one needs both vaults open and a server behind it. */
export function canMakeMain(space: SpaceInfo, status: SpacesStatus): boolean {
  if (space.main || space.readOnly) return false;
  if (space.status.backend === 'none') return false;
  if (status.spaces.some((other) => other.locked)) return false;
  return status.spaces.every((other) => other.status.vault === 'unlocked');
}

/** Entries can be taken over to the remaining server only while both are open. */
export function canKeepEntries(space: SpaceInfo, status: SpacesStatus): boolean {
  const other = otherSpace(status, space.id);
  if (!other || other.locked || other.readOnly || space.locked) return false;
  return space.status.vault === 'unlocked' && other.status.vault === 'unlocked';
}

/**
 * The status the forms for a second server start from: this device's name,
 * and nothing else. The new server's store does not exist yet and has no
 * vault, whatever the main one looks like.
 */
export function blankStatus(main: SyncStatus): SyncStatus {
  return {
    paired: false,
    serverUrl: null,
    tlsFingerprint: null,
    deviceId: null,
    pairedMs: null,
    lastSyncMs: null,
    last: null,
    pending: 0,
    running: false,
    vault: 'absent',
    deviceName: main.deviceName,
    offering: false,
    withheld: { records: 0, hostKeys: false },
    backend: 'none',
    lock: {
      serverUrl: null,
      email: null,
      needsSignIn: false,
      live: false,
      liveRefused: false,
      signedInMs: null,
      moveStartedMs: null,
      leftBehind: false,
      switchedOff: false,
    },
  };
}
