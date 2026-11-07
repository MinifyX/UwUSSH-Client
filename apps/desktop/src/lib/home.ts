/**
 * What the overview works out from the host list: the quick-connect field and
 * the recently used hosts. Pure functions only, for `node --test`.
 */

import type { HostRecord } from './session';

export type QuickTarget = { username: string | null; address: string; port: number | null };

/**
 * `user@host[:port]`, `host:port` or a bare name. `[v6]:port` keeps the
 * address in brackets apart from its port. `null` for anything with spaces
 * or an empty part.
 */
export function parseQuickConnect(text: string): QuickTarget | null {
  const value = text.trim();
  if (!value || /\s/.test(value)) return null;
  const at = value.lastIndexOf('@');
  const username = at >= 0 ? value.slice(0, at) : null;
  if (username === '') return null;
  let rest = value.slice(at + 1);
  let port: number | null = null;
  const bracket = /^\[([^\]]+)\](?::(\d+))?$/.exec(rest);
  if (bracket) {
    rest = bracket[1]!;
    port = bracket[2] ? Number(bracket[2]) : null;
  } else {
    const colon = /^([^:]+):(\d+)$/.exec(rest);
    if (colon) {
      rest = colon[1]!;
      port = Number(colon[2]);
    }
  }
  if (!rest || rest.endsWith(':') || (port !== null && (port < 1 || port > 65535))) return null;
  return { username, address: rest, port };
}

/**
 * The stored host the field means: one with that name, or with that address
 * (and user and port, where typed). `null` when none or several fit.
 */
export function matchQuickConnect(
  hosts: readonly HostRecord[],
  target: QuickTarget,
): HostRecord | null {
  const lower = (s: string) => s.toLowerCase();
  if (target.username === null && target.port === null) {
    const named = hosts.filter((host) => lower(host.name) === lower(target.address));
    if (named.length === 1) return named[0]!;
  }
  const fits = hosts.filter(
    (host) =>
      lower(host.address) === lower(target.address) &&
      (target.username === null || host.username === target.username) &&
      (target.port === null || host.port === target.port),
  );
  return fits.length === 1 ? fits[0]! : null;
}

/** Up to `count` hosts, the one connected to last first; never-used ones are left out. */
export function recentHosts(hosts: readonly HostRecord[], count = 4): HostRecord[] {
  return hosts
    .filter((host) => (host.lastConnectedMs ?? 0) > 0)
    .sort((a, b) => (b.lastConnectedMs ?? 0) - (a.lastConnectedMs ?? 0))
    .slice(0, count);
}
