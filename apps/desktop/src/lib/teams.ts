/**
 * Teams: what the pages work out from `teams_status` and `spaces_status`
 * before they show anything. A team lives on a UwUSync server and is a space
 * of its own on this device; the server enforces who may do what, and the
 * rules here mirror it so the pages offer only what will be accepted.
 *
 * Without a server that knows teams nothing here adds a control.
 *
 * No Tauri in here, so `node --test` can run it.
 */

// With its extension: Node runs this file as it is.
import { serverSpaces, spaceName } from './spaces.ts';
import type { SpaceInfo, SpacesStatus } from './sync';

export type TeamRole = 'owner' | 'admin' | 'editor' | 'reader';
export type TeamState = 'pending' | 'active';

export type TeamInfo = {
  id: string;
  name: string;
  role: TeamRole;
  /** `pending`: joined with a code, waiting for an admin to confirm. */
  state: TeamState;
  members: number;
  /** A member left: the team key is due to be replaced. */
  needsRotation: boolean;
  /** The local space once the team is open on this device. */
  space: string | null;
};

export type TeamServer = {
  /** The server space the teams live on. */
  space: string;
  label: string;
  /** A UwUSync server that knows teams. */
  supported: boolean;
  enabled: boolean;
  mayCreate: boolean;
  /** This account's member key, null before the first use. */
  fingerprint: string | null;
  teams: TeamInfo[];
};

export type TeamMember = {
  accountId: string;
  displayName: string;
  role: TeamRole;
  state: TeamState;
  fingerprint: string;
  joinedMs: number;
  me: boolean;
};

/** `code` only right after the invite was made. */
export type TeamInvite = { id: string; role: TeamRole; expiresMs: number; code?: string };

/** The team's hosts that carry a stored password or key, after a member went. */
export type TeamRemoval = { hosts: { id: string; name: string }[] };

export type TeamFailure = {
  kind:
    | 'vault-locked'
    | 'offline'
    | 'not-allowed'
    | 'limit'
    | 'invalid-code'
    | 'last-owner'
    | 'unsupported'
    | 'error';
  message?: string;
};

const FAILURES: ReadonlySet<string> = new Set([
  'vault-locked',
  'offline',
  'not-allowed',
  'limit',
  'invalid-code',
  'last-owner',
  'unsupported',
  'error',
]);

export function asTeamFailure(error: unknown): TeamFailure {
  if (typeof error === 'object' && error !== null && 'kind' in error) {
    const { kind, message } = error as { kind: unknown; message?: unknown };
    if (typeof kind === 'string' && FAILURES.has(kind)) {
      return {
        kind: kind as TeamFailure['kind'],
        message: typeof message === 'string' ? message : undefined,
      };
    }
    return { kind: 'error', message: typeof message === 'string' ? message : String(kind) };
  }
  return { kind: 'error', message: String(error) };
}

/** The backend's word, inside an `error`, for a member key that is not the one shown. */
const IDENTITY_CHANGED = 'identity-changed';

/** What a failure is about: its kind, or one of the errors the pages have words for. */
export type TeamFailureCase = TeamFailure['kind'] | 'identity-changed';

/**
 * Confirming hands the team key to exactly the key that was on screen. When
 * the member's key is another one by now, the backend refuses with an
 * `error` that names `identity-changed`: the list has to be loaded again and
 * the new key compared.
 */
export function failureCase(failure: TeamFailure): TeamFailureCase {
  if (failure.kind === 'error' && failure.message?.includes(IDENTITY_CHANGED)) {
    return 'identity-changed';
  }
  return failure.kind;
}

// ── Whether teams show at all ───────────────────────────────────────────────

/** The servers the Teams page has a section for. */
export function teamServers(servers: readonly TeamServer[] | null): TeamServer[] {
  return (servers ?? []).filter((server) => server.supported && server.enabled);
}

/** Settings has a Teams page only with a server that knows teams and has them on. */
export function teamsVisible(servers: readonly TeamServer[] | null): boolean {
  return teamServers(servers).length > 0;
}

// ── Roles ───────────────────────────────────────────────────────────────────

/** The roles an invite can carry: an owner is made from a member, never invited. */
export const INVITE_ROLES: readonly TeamRole[] = ['admin', 'editor', 'reader'];

const RANK: Record<TeamRole, number> = { owner: 0, admin: 1, editor: 2, reader: 3 };

/** Owner and admin run the team: members, invites, its name. */
export function manages(role: TeamRole): boolean {
  return role === 'owner' || role === 'admin';
}

export type TeamAction =
  /** See who is in it. */
  | 'members'
  /** Confirm, invite, change roles, remove. */
  | 'manage'
  | 'rename'
  | 'delete'
  | 'leave'
  /** Add and change its hosts. */
  | 'write';

/**
 * What this account may do in a team. Someone who still waits sees nothing
 * of it: they can only go again – or, as its owner, delete it.
 */
export function may(team: { role: TeamRole; state: TeamState }, action: TeamAction): boolean {
  if (action === 'leave') return true;
  if (action === 'delete') return team.role === 'owner';
  if (team.state !== 'active') return false;
  switch (action) {
    case 'members':
      return true;
    case 'manage':
    case 'rename':
      return manages(team.role);
    case 'write':
      return team.role !== 'reader';
  }
}

/**
 * The roles `mine` may give this member, its present one among them. Empty:
 * the role is not this account's to change. An owner gives any role, but
 * only someone who is in becomes an owner; an admin moves anybody who is not
 * an owner between admin, editor and reader.
 */
export function assignableRoles(
  mine: TeamRole,
  member: { role: TeamRole; state: TeamState },
): TeamRole[] {
  if (mine === 'owner') {
    return member.state === 'active' || member.role === 'owner'
      ? ['owner', 'admin', 'editor', 'reader']
      : ['admin', 'editor', 'reader'];
  }
  if (mine === 'admin' && member.role !== 'owner') return ['admin', 'editor', 'reader'];
  return [];
}

/** Removing someone else; leaving is its own action. */
export function mayRemove(mine: TeamRole, member: { role: TeamRole; me: boolean }): boolean {
  if (member.me) return false;
  return mine === 'owner' || (mine === 'admin' && member.role !== 'owner');
}

/** Letting a waiting member in. Nobody confirms their own key. */
export function mayConfirm(mine: TeamRole, member: { state: TeamState; me: boolean }): boolean {
  return manages(mine) && member.state === 'pending' && !member.me;
}

/** Who waits comes first, then by role, then by name. */
export function sortMembers<T extends Pick<TeamMember, 'state' | 'role' | 'displayName'>>(
  members: readonly T[],
): T[] {
  return [...members].sort(
    (a, b) =>
      Number(b.state === 'pending') - Number(a.state === 'pending') ||
      RANK[a.role] - RANK[b.role] ||
      a.displayName.localeCompare(b.displayName),
  );
}

// ── Invites and fingerprints ────────────────────────────────────────────────

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

export type Remaining = { unit: 'days' | 'hours' | 'minutes'; n: number };

/**
 * How long an invite is still good for, in the largest unit that says
 * something: "7 days" right after it was made, hours on the last day,
 * minutes in the last hour. Null once it has run out.
 */
export function inviteRemaining(expiresMs: number, nowMs: number): Remaining | null {
  const left = expiresMs - nowMs;
  if (!(left > 0)) return null;
  if (left >= DAY - HOUR / 2) return { unit: 'days', n: Math.max(1, Math.round(left / DAY)) };
  if (left >= HOUR - MINUTE / 2) return { unit: 'hours', n: Math.round(left / HOUR) };
  return { unit: 'minutes', n: Math.max(1, Math.ceil(left / MINUTE)) };
}

/**
 * A fingerprint in rows of a few groups, to read out and compare line by
 * line. The groups are the backend's; this only breaks the line.
 */
export function fingerprintRows(fingerprint: string, perRow = 4): string[] {
  const groups = fingerprint.split(/[\s:-]+/).filter(Boolean);
  const rows: string[] = [];
  for (let at = 0; at < groups.length; at += perRow) {
    rows.push(groups.slice(at, at + perRow).join(' '));
  }
  return rows;
}

// ── Where a host is filed ───────────────────────────────────────────────────

export function isTeam(space: SpaceInfo): boolean {
  return space.kind === 'team';
}

/** A team exists on this device: the host dialog then asks where a host is filed. */
export function hasTeams(status: SpacesStatus | null): boolean {
  return status !== null && status.spaces.some(isTeam);
}

export type FilingOption = {
  /** The space a host is saved into. */
  id: string;
  kind: 'personal' | 'team';
  /** The team's name; for the personal vault the server's. */
  name: string;
  /** Why nothing can be filed here right now. */
  blocked: 'read-only' | 'locked' | null;
};

export type FilingGroup = { server: string; name: string; options: FilingOption[] };

function blocked(space: SpaceInfo): FilingOption['blocked'] {
  if (space.readOnly) return 'read-only';
  return space.locked ? 'locked' : null;
}

/**
 * The places a host can be filed: per server its personal vault, then its
 * teams. A team names its server in `parent`; one that does not belongs to
 * the server listed before it.
 */
export function filingGroups(status: SpacesStatus): FilingGroup[] {
  const servers = serverSpaces(status);
  const groups = new Map<string, FilingGroup>();
  for (const server of servers) {
    const name = spaceName(server);
    groups.set(server.id, {
      server: server.id,
      name,
      options: [{ id: server.id, kind: 'personal', name, blocked: blocked(server) }],
    });
  }
  let last: string | null = null;
  for (const space of status.spaces) {
    if (!isTeam(space)) {
      last = space.id;
      continue;
    }
    const home = groups.get(space.parent ?? '') ?? groups.get(last ?? '');
    home?.options.push({
      id: space.id,
      kind: 'team',
      name: space.label.trim(),
      blocked: blocked(space),
    });
  }
  return servers.map((server) => groups.get(server.id)!);
}

/** The spaces nothing may be written into: their hosts cannot be changed. */
export function readOnlySpaces(status: SpacesStatus | null): ReadonlySet<string> {
  return new Set((status?.spaces ?? []).filter((space) => space.readOnly).map((space) => space.id));
}

/** The team a host lives in, if it lives in one. */
export function teamOfHost(
  status: SpacesStatus | null,
  host: { space: string } | null,
): SpaceInfo | null {
  if (!status || !host) return null;
  return status.spaces.find((space) => space.id === host.space && isTeam(space)) ?? null;
}

/** A stored host in a place this account only reads: it opens, but nothing in it changes. */
export function hostReadOnly(status: SpacesStatus | null, host: { space: string } | null): boolean {
  return host !== null && readOnlySpaces(status).has(host.space);
}

/**
 * A group with a host this account only reads: renaming, moving or
 * dissolving the group would change that host, so the group stays as it is.
 * `workspace` null: workspaces are off and the list shows both as one.
 */
export function groupReadOnly(
  hosts: readonly { space: string; workspace: string; groupPath: string | null }[],
  readOnly: ReadonlySet<string>,
  workspace: string | null,
  group: string,
): boolean {
  return hosts.some(
    (host) =>
      host.groupPath === group &&
      (workspace === null || host.workspace === workspace) &&
      readOnly.has(host.space),
  );
}

/**
 * The hosts whose tunnels can be added, changed and deleted. A tunnel is
 * stored with its host, so the ones of a read-only host only start and stop.
 */
export function tunnelHosts<T extends { space: string }>(
  hosts: readonly T[],
  readOnly: ReadonlySet<string>,
): T[] {
  return hosts.filter((host) => !readOnly.has(host.space));
}

/**
 * The teams of one server that are open on this device: they go from it
 * together with the server. `server` undefined means the main one. A team
 * names its server in `parent`; one that does not belongs to the server
 * listed before it.
 */
export function teamsOnServer(
  status: SpacesStatus | null,
  server: string | undefined,
): SpaceInfo[] {
  if (!status) return [];
  const home = server ?? status.spaces.find((space) => space.main && !isTeam(space))?.id;
  if (home === undefined) return [];
  const teams: SpaceInfo[] = [];
  let last: string | null = null;
  for (const space of status.spaces) {
    if (!isTeam(space)) last = space.id;
    else if ((space.parent ?? last) === home) teams.push(space);
  }
  return teams;
}
