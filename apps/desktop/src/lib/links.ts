/**
 * `uwussh://connect/<host id>`: finding the host a link asks for.
 *
 * The app's Rust side checks the link and hands over nothing but the host's
 * record id (`take_link`). Finding the host is this: open the vault first if
 * it is locked (the hosts' sign-in data is in it, and sync needs it open),
 * look in the list, and if the host isn't there yet — the link came from
 * UwULock's web vault for a host another device just added — sync once and
 * look again.
 *
 * A plain function of what it is given, so `node --test` can run its tests;
 * this file only imports types.
 */

import type { HostRecord, VaultStatus } from './session';

/** What the app does for a link. */
export type LinkSteps = {
  vaultStatus: () => Promise<VaultStatus>;
  /** Ask for the master password. False: the person didn't open it. */
  unlock: () => Promise<boolean>;
  listHosts: () => Promise<HostRecord[]>;
  /** One sync pass, waited for. False: sync isn't set up on this device. */
  sync: () => Promise<boolean>;
};

export type LinkOutcome =
  | { kind: 'host'; host: HostRecord }
  | { kind: 'locked' }
  /** Not on this device. `synced`: a pass ran and it still wasn't there. */
  | { kind: 'unknown'; synced: boolean; syncError: string | null };

/** The host ids the Rust side passes on are lowercase, hyphenated UUIDs. */
const sameId = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();

export async function findLinkedHost(id: string, steps: LinkSteps): Promise<LinkOutcome> {
  if ((await steps.vaultStatus()) === 'locked' && !(await steps.unlock())) {
    return { kind: 'locked' };
  }
  const find = async () => (await steps.listHosts()).find((host) => sameId(host.id, id));
  const known = await find();
  if (known) return { kind: 'host', host: known };

  let synced = false;
  let syncError: string | null = null;
  try {
    synced = await steps.sync();
  } catch (error) {
    syncError = describeError(error);
  }
  // Even a failed pass may have brought records before it failed.
  const arrived = await find();
  if (arrived) return { kind: 'host', host: arrived };
  return { kind: 'unknown', synced, syncError };
}

function describeError(error: unknown): string {
  if (error && typeof error === 'object' && 'message' in error) {
    return String((error as { message: unknown }).message);
  }
  if (error && typeof error === 'object' && 'kind' in error) {
    return String((error as { kind: unknown }).kind);
  }
  return String(error);
}
