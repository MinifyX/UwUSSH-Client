// Teams: who may do what, where a host can be filed, what is read-only and
// how long an invite still runs. Runs with `node --test`.

import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  chosenSpace,
  defaultSpace,
  draftSpace,
  hasPlaces,
  hasTwoServers,
  mainSpace,
  moveBlocker,
  otherSpace,
  serverSpaces,
} from '../src/lib/spaces.ts';
import type { SpaceInfo, SpacesStatus, SyncStatus } from '../src/lib/sync.ts';
import {
  asTeamFailure,
  assignableRoles,
  attentionNotice,
  confirmedBefore,
  failureCase,
  filingGroups,
  fingerprintRows,
  groupReadOnly,
  hasTeams,
  hostReadOnly,
  inviteRemaining,
  may,
  mayConfirm,
  mayReconfirm,
  mayRemove,
  readOnlySpaces,
  sortMembers,
  teamOfHost,
  teamServers,
  teamsOnServer,
  teamsVisible,
  tunnelHosts,
  type TeamRole,
  type TeamServer,
} from '../src/lib/teams.ts';

const STATUS = { vault: 'unlocked', serverUrl: null, lock: { serverUrl: null } } as SyncStatus;

const space = (id: string, over: Partial<SpaceInfo> = {}): SpaceInfo => ({
  id,
  main: false,
  kind: 'server',
  label: `${id}.example.com`,
  locked: false,
  readOnly: false,
  status: STATUS,
  ...over,
});

const team = (id: string, parent: string, over: Partial<SpaceInfo> = {}): SpaceInfo =>
  space(id, { kind: 'team', label: `Team ${id}`, parent, ...over });

const status = (...spaces: SpaceInfo[]): SpacesStatus => ({
  spaces,
  defaults: { private: 'a', business: 'a' },
  canAdd: true,
});

const ONE = status(space('a', { main: true }));
const TEAMS = status(
  space('a', { main: true }),
  team('ops', 'a'),
  team('audit', 'a', { readOnly: true }),
  space('b'),
  team('lab', 'b', { locked: true }),
);

const server = (over: Partial<TeamServer> = {}): TeamServer => ({
  space: 'a',
  label: 'a.example.com',
  supported: true,
  enabled: true,
  mayCreate: true,
  fingerprint: null,
  teams: [],
  ...over,
});

const ROLES: TeamRole[] = ['owner', 'admin', 'editor', 'reader'];

test('the Teams page exists only with a server that knows teams and has them on', () => {
  assert.equal(teamsVisible(null), false);
  assert.equal(teamsVisible([]), false);
  assert.equal(teamsVisible([server({ supported: false })]), false);
  assert.equal(teamsVisible([server({ enabled: false })]), false);
  assert.equal(teamsVisible([server({ supported: false }), server({ space: 'b' })]), true);
  assert.deepEqual(
    teamServers([server({ enabled: false }), server({ space: 'b' })]).map((s) => s.space),
    ['b'],
  );
});

test('without a team nothing about the two servers changes', () => {
  assert.equal(hasTeams(null), false);
  assert.equal(hasTeams(ONE), false);
  assert.equal(hasPlaces(ONE), false);
  assert.equal(draftSpace(ONE, 'a'), undefined);
  assert.equal(readOnlySpaces(ONE).size, 0);
});

test('a team is a place for hosts but never one of the two servers', () => {
  const one = status(space('a', { main: true }), team('ops', 'a'));
  assert.equal(hasTeams(one), true);
  assert.equal(hasTwoServers(one), false);
  assert.equal(hasPlaces(one), true);
  assert.deepEqual(
    serverSpaces(TEAMS).map((s) => s.id),
    ['a', 'b'],
  );
  assert.equal(mainSpace(status(team('ops', 'a', { main: true }), space('a')))?.id, 'a');
  assert.equal(otherSpace(TEAMS, 'a')?.id, 'b');
  // A default that somehow names a team falls back to the main server.
  assert.equal(
    defaultSpace({ ...TEAMS, defaults: { private: 'ops', business: 'b' } }, 'private'),
    'a',
  );
  assert.equal(draftSpace(one, 'ops'), 'ops');
});

test('a new host never lands in a team unasked, a stored one shows its team', () => {
  assert.equal(chosenSpace(TEAMS, null, 'private', null), 'a');
  assert.equal(chosenSpace(TEAMS, null, 'private', 'ops'), 'ops');
  // The personal vault is shut: the other server takes it, not a team.
  const shut = status(space('a', { main: true, locked: true }), team('ops', 'a'), space('b'));
  assert.equal(chosenSpace(shut, null, 'private', null), 'b');
  assert.equal(chosenSpace(TEAMS, { space: 'audit' }, 'private', null), 'audit');
  assert.equal(chosenSpace(TEAMS, { space: 'ops' }, 'private', 'a'), 'a');
});

test('with teams only the host’s own locked place keeps it from moving', () => {
  assert.equal(moveBlocker(TEAMS, { space: 'a' }), null);
  assert.equal(moveBlocker(TEAMS, { space: 'lab' })?.id, 'lab');
  // Two servers and no team: as before, either one shut blocks the move.
  const two = status(space('a', { main: true }), space('b', { locked: true }));
  assert.equal(moveBlocker(two, { space: 'a' })?.id, 'b');
});

test('filing options: per server its personal vault, then its teams', () => {
  const groups = filingGroups(TEAMS);
  assert.deepEqual(
    groups.map((g) => [g.server, g.name]),
    [
      ['a', 'a.example.com'],
      ['b', 'b.example.com'],
    ],
  );
  assert.deepEqual(groups[0]!.options, [
    { id: 'a', kind: 'personal', name: 'a.example.com', blocked: null },
    { id: 'ops', kind: 'team', name: 'Team ops', blocked: null },
    { id: 'audit', kind: 'team', name: 'Team audit', blocked: 'read-only' },
  ]);
  assert.deepEqual(groups[1]!.options, [
    { id: 'b', kind: 'personal', name: 'b.example.com', blocked: null },
    { id: 'lab', kind: 'team', name: 'Team lab', blocked: 'locked' },
  ]);
});

test('filing options: a team goes by its parent, else by the server before it', () => {
  // Out of order, but each names its server.
  const mixed = status(space('a'), space('b'), team('x', 'b'), team('y', 'a'));
  assert.deepEqual(
    filingGroups(mixed).map((g) => g.options.map((o) => o.id)),
    [
      ['a', 'y'],
      ['b', 'x'],
    ],
  );
  const bare = status(space('a'), team('x', ''), space('b'), team('y', 'gone'));
  assert.deepEqual(
    filingGroups(bare).map((g) => g.options.map((o) => o.id)),
    [
      ['a', 'x'],
      ['b', 'y'],
    ],
  );
  // A team before any server has nowhere to go and is left out.
  assert.deepEqual(filingGroups(status(team('x', 'gone'), space('a')))[0]!.options.length, 1);
  assert.deepEqual(filingGroups(status()), []);
});

test('a host in a team this account only reads cannot be changed', () => {
  assert.deepEqual([...readOnlySpaces(TEAMS)], ['audit']);
  assert.equal(hostReadOnly(TEAMS, { space: 'audit' }), true);
  assert.equal(hostReadOnly(TEAMS, { space: 'ops' }), false);
  assert.equal(hostReadOnly(TEAMS, { space: 'lab' }), false);
  assert.equal(hostReadOnly(TEAMS, null), false);
  assert.equal(hostReadOnly(null, { space: 'audit' }), false);
  assert.equal(teamOfHost(TEAMS, { space: 'audit' })?.label, 'Team audit');
  assert.equal(teamOfHost(TEAMS, { space: 'a' }), null);
  assert.equal(teamOfHost(TEAMS, null), null);
});

test('what a role may do in a team', () => {
  const can = (role: TeamRole, state: 'active' | 'pending' = 'active') =>
    (['members', 'manage', 'rename', 'delete', 'leave', 'write'] as const).filter((action) =>
      may({ role, state }, action),
    );
  assert.deepEqual(can('owner'), ['members', 'manage', 'rename', 'delete', 'leave', 'write']);
  assert.deepEqual(can('admin'), ['members', 'manage', 'rename', 'leave', 'write']);
  assert.deepEqual(can('editor'), ['members', 'leave', 'write']);
  assert.deepEqual(can('reader'), ['members', 'leave']);
  // Still waiting: nothing of the team shows, they can only go again.
  assert.deepEqual(can('admin', 'pending'), ['leave']);
  assert.deepEqual(can('owner', 'pending'), ['delete', 'leave']);
});

test('which roles someone may give', () => {
  const active = (role: TeamRole) => ({ role, state: 'active' as const });
  for (const role of ROLES) assert.deepEqual(assignableRoles('owner', active(role)), ROLES);
  // Only someone who is in becomes an owner.
  assert.deepEqual(assignableRoles('owner', { role: 'editor', state: 'pending' }), [
    'admin',
    'editor',
    'reader',
  ]);
  assert.deepEqual(assignableRoles('admin', active('owner')), []);
  for (const role of ['admin', 'editor', 'reader'] as const) {
    assert.deepEqual(assignableRoles('admin', active(role)), ['admin', 'editor', 'reader']);
  }
  for (const mine of ['editor', 'reader'] as const) {
    for (const role of ROLES) assert.deepEqual(assignableRoles(mine, active(role)), []);
  }
});

test('who may be removed and confirmed', () => {
  assert.equal(mayRemove('owner', { role: 'owner', me: false }), true);
  assert.equal(mayRemove('owner', { role: 'owner', me: true }), false);
  assert.equal(mayRemove('admin', { role: 'owner', me: false }), false);
  assert.equal(mayRemove('admin', { role: 'admin', me: false }), true);
  assert.equal(mayRemove('admin', { role: 'reader', me: true }), false);
  assert.equal(mayRemove('editor', { role: 'reader', me: false }), false);
  assert.equal(mayRemove('reader', { role: 'reader', me: false }), false);

  assert.equal(mayConfirm('owner', { state: 'pending', me: false }), true);
  assert.equal(mayConfirm('admin', { state: 'pending', me: false }), true);
  assert.equal(mayConfirm('admin', { state: 'active', me: false }), false);
  assert.equal(mayConfirm('admin', { state: 'pending', me: true }), false);
  assert.equal(mayConfirm('editor', { state: 'pending', me: false }), false);
  assert.equal(mayConfirm('reader', { state: 'pending', me: false }), false);
});

test('a member this account never compared is confirmed by who manages the team', () => {
  const member = { state: 'active', me: false, confirmed: false } as const;
  assert.equal(mayReconfirm('owner', member), true);
  assert.equal(mayReconfirm('admin', member), true);
  assert.equal(mayReconfirm('editor', member), false);
  assert.equal(mayReconfirm('reader', member), false);
  assert.equal(mayReconfirm('owner', { ...member, confirmed: true }), false);
  // An older backend says nothing: nobody is marked.
  assert.equal(mayReconfirm('owner', { state: 'active', me: false }), false);
  assert.equal(mayReconfirm('owner', { ...member, me: true }), false);
  // Somebody who waits is let in, not confirmed again.
  assert.equal(mayReconfirm('owner', { ...member, state: 'pending' }), false);
});

test('what stands in the way of a team, and the way out', () => {
  const team = (
    attention: TeamInfo['attention'],
    role: TeamRole = 'owner',
  ): Pick<TeamInfo, 'role' | 'state' | 'attention'> => ({ role, state: 'active', attention });
  assert.equal(attentionNotice(team(undefined)), null);
  assert.deepEqual(attentionNotice(team({ kind: 'unavailable' })), {
    kind: 'unavailable',
    member: null,
    confirm: null,
    leave: false,
  });
  // The way out of a key that does not follow is to leave. A name or an
  // account that came along means nothing there.
  assert.deepEqual(attentionNotice(team({ kind: 'key-refused', account: 'a', member: 'Ben' })), {
    kind: 'key-refused',
    member: null,
    confirm: null,
    leave: true,
  });
  assert.deepEqual(
    attentionNotice(team({ kind: 'member-unconfirmed', account: 'a', member: ' Cleo ' })),
    {
      kind: 'member-unconfirmed',
      member: 'Cleo',
      confirm: { account: 'a', again: false },
      leave: false,
    },
  );
  for (const kind of ['member-key-changed', 'member-returned'] as const) {
    const attention = { kind, account: 'a', member: 'Ben' };
    assert.deepEqual(attentionNotice(team(attention, 'admin'))?.confirm, {
      account: 'a',
      again: true,
    });
    // Nothing to press for who does not manage the team, or without an account.
    assert.equal(attentionNotice(team(attention, 'editor'))?.confirm, null);
    assert.equal(attentionNotice(team({ kind, member: 'Ben' }))?.confirm, null);
    assert.equal(attentionNotice(team({ kind, account: 'a' }))?.member, null);
  }

  assert.equal(confirmedBefore({ kind: 'member-key-changed', account: 'a' }, 'a'), true);
  assert.equal(confirmedBefore({ kind: 'member-returned', account: 'a' }, 'a'), true);
  assert.equal(confirmedBefore({ kind: 'member-returned', account: 'a' }, 'b'), false);
  assert.equal(confirmedBefore({ kind: 'member-unconfirmed', account: 'a' }, 'a'), false);
  assert.equal(confirmedBefore(undefined, 'a'), false);
});

test('members: who waits first, then by role and name', () => {
  const member = (displayName: string, role: TeamRole, state: 'active' | 'pending' = 'active') => ({
    displayName,
    role,
    state,
  });
  const sorted = sortMembers([
    member('Zoe', 'reader'),
    member('Ben', 'editor'),
    member('Ada', 'editor'),
    member('Kim', 'owner'),
    member('Uli', 'reader', 'pending'),
    member('Max', 'admin'),
  ]);
  assert.deepEqual(
    sorted.map((m) => m.displayName),
    ['Uli', 'Kim', 'Max', 'Ada', 'Ben', 'Zoe'],
  );
});

test('how long an invite still runs', () => {
  const MIN = 60_000;
  const HOUR = 60 * MIN;
  const DAY = 24 * HOUR;
  const now = 1_700_000_000_000;
  const left = (ms: number) => inviteRemaining(now + ms, now);
  assert.deepEqual(left(7 * DAY), { unit: 'days', n: 7 });
  assert.deepEqual(left(7 * DAY - 5 * MIN), { unit: 'days', n: 7 });
  assert.deepEqual(left(6 * DAY + 11 * HOUR), { unit: 'days', n: 6 });
  assert.deepEqual(left(36 * HOUR), { unit: 'days', n: 2 });
  assert.deepEqual(left(DAY), { unit: 'days', n: 1 });
  assert.deepEqual(left(23 * HOUR), { unit: 'hours', n: 23 });
  assert.deepEqual(left(90 * MIN), { unit: 'hours', n: 2 });
  assert.deepEqual(left(HOUR), { unit: 'hours', n: 1 });
  assert.deepEqual(left(59 * MIN), { unit: 'minutes', n: 59 });
  assert.deepEqual(left(1), { unit: 'minutes', n: 1 });
  assert.equal(left(0), null);
  assert.equal(left(-DAY), null);
  assert.equal(inviteRemaining(Number.NaN, now), null);
});

test('a fingerprint breaks into rows of four groups', () => {
  assert.deepEqual(fingerprintRows('a1b2 c3d4 e5f6 0718 293a 4b5c 6d7e 8f90'), [
    'a1b2 c3d4 e5f6 0718',
    '293a 4b5c 6d7e 8f90',
  ]);
  assert.deepEqual(fingerprintRows('a1b2-c3d4-e5f6:0718-293a'), ['a1b2 c3d4 e5f6 0718', '293a']);
  assert.deepEqual(fingerprintRows('aa bb cc', 2), ['aa bb', 'cc']);
  assert.deepEqual(fingerprintRows(''), []);
});

test('failures: the known kinds pass, anything else is an error with its text', () => {
  assert.deepEqual(asTeamFailure({ kind: 'last-owner' }), {
    kind: 'last-owner',
    message: undefined,
  });
  assert.deepEqual(asTeamFailure({ kind: 'limit', message: 'x' }), { kind: 'limit', message: 'x' });
  assert.deepEqual(asTeamFailure({ kind: 'nope' }), { kind: 'error', message: 'nope' });
  assert.deepEqual(asTeamFailure({ kind: 'nope', message: 'why' }), {
    kind: 'error',
    message: 'why',
  });
  assert.deepEqual(asTeamFailure('boom'), { kind: 'error', message: 'boom' });
});

test('a wrong fingerprint on confirm is its own case, every other failure its kind', () => {
  assert.equal(
    failureCase({ kind: 'error', message: 'team: identity-changed for account 7' }),
    'identity-changed',
  );
  assert.equal(failureCase({ kind: 'error', message: 'identity-changed' }), 'identity-changed');
  assert.equal(failureCase({ kind: 'error', message: 'disk full' }), 'error');
  assert.equal(failureCase({ kind: 'error' }), 'error');
  // Only an `error` carries it: the other kinds keep their own sentence.
  assert.equal(failureCase({ kind: 'offline', message: 'identity-changed' }), 'offline');
  assert.equal(
    failureCase(asTeamFailure({ kind: 'error', message: 'identity-changed' })),
    'identity-changed',
  );
  // Too many tries has a sentence of its own, too.
  assert.equal(failureCase({ kind: 'error', message: 'rate-limited: slow down' }), 'rate-limited');
  assert.equal(failureCase({ kind: 'limit', message: 'rate-limited' }), 'limit');
});

test('a group with a read-only host cannot be renamed, moved or dissolved', () => {
  const locked = readOnlySpaces(TEAMS);
  const hosts = [
    { space: 'a', workspace: 'private', groupPath: 'Web' },
    { space: 'ops', workspace: 'private', groupPath: 'Web' },
    { space: 'audit', workspace: 'private', groupPath: 'Audit' },
    { space: 'a', workspace: 'business', groupPath: 'Audit' },
    { space: 'audit', workspace: 'business', groupPath: null },
  ];
  assert.equal(groupReadOnly(hosts, locked, 'private', 'Web'), false);
  assert.equal(groupReadOnly(hosts, locked, 'private', 'Audit'), true);
  // The same name in the other workspace is another group.
  assert.equal(groupReadOnly(hosts, locked, 'business', 'Audit'), false);
  // Workspaces off: the list shows both as one group.
  assert.equal(groupReadOnly(hosts, locked, null, 'Audit'), true);
  assert.equal(groupReadOnly(hosts, locked, null, 'Web'), false);
  assert.equal(groupReadOnly(hosts, locked, 'private', 'Empty'), false);
  assert.equal(groupReadOnly(hosts, readOnlySpaces(null), 'private', 'Audit'), false);
});

test('tunnels are added and changed only on hosts that can be written', () => {
  const hosts = [
    { id: '1', space: 'a' },
    { id: '2', space: 'audit' },
    { id: '3', space: 'ops' },
  ];
  assert.deepEqual(
    tunnelHosts(hosts, readOnlySpaces(TEAMS)).map((host) => host.id),
    ['1', '3'],
  );
  assert.equal(tunnelHosts(hosts, readOnlySpaces(null)).length, 3);
  assert.deepEqual(tunnelHosts([hosts[1]], readOnlySpaces(TEAMS)), []);
});

test('the teams that go from this device with their server', () => {
  const ids = (server: string | undefined, from: SpacesStatus | null = TEAMS) =>
    teamsOnServer(from, server).map((entry) => entry.id);
  assert.deepEqual(ids('a'), ['ops', 'audit']);
  assert.deepEqual(ids('b'), ['lab']);
  // No id: the main server, as every sync command means it.
  assert.deepEqual(ids(undefined), ['ops', 'audit']);
  assert.deepEqual(ids('new'), []);
  assert.deepEqual(ids('a', ONE), []);
  assert.deepEqual(ids(undefined, null), []);
  // A team without `parent` belongs to the server listed before it.
  const loose = status(
    space('a', { main: true }),
    space('b'),
    space('t', { kind: 'team', label: 'Team t' }),
  );
  assert.deepEqual(ids('b', loose), ['t']);
  assert.deepEqual(ids('a', loose), []);
});
