// Two servers: what the pages work out from `spaces_status`. Runs with
// `node --test` (Node strips the types).

import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  asksOwnPassword,
  blankStatus,
  canKeepEntries,
  canMakeMain,
  chosenSpace,
  defaultSpace,
  draftSpace,
  hasTwoServers,
  mainSpace,
  moveBlocker,
  movesHost,
  otherSpace,
  serverHost,
  spaceName,
} from '../src/lib/spaces.ts';
import type { SpaceInfo, SpacesStatus, SyncStatus } from '../src/lib/sync.ts';

const STATUS: SyncStatus = {
  paired: true,
  serverUrl: 'https://sync.example.com:8443',
  tlsFingerprint: 'SHA256:abc',
  deviceId: 'd1',
  pairedMs: 1,
  lastSyncMs: 2,
  last: null,
  pending: 3,
  running: false,
  vault: 'unlocked',
  deviceName: 'Laptop',
  offering: false,
  withheld: { records: 0, hostKeys: false },
  backend: 'uwusync',
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

const ONE: SpacesStatus = {
  spaces: [space('a', { main: true })],
  defaults: { private: 'a', business: 'a' },
  canAdd: true,
};

const TWO: SpacesStatus = {
  spaces: [space('a', { main: true }), space('b')],
  defaults: { private: 'a', business: 'b' },
  canAdd: false,
};

const withSecond = (over: Partial<SpaceInfo>): SpacesStatus => ({
  ...TWO,
  spaces: [TWO.spaces[0]!, space('b', over)],
});

test('one server is not two, and nothing is known is not two either', () => {
  assert.equal(hasTwoServers(null), false);
  assert.equal(hasTwoServers(ONE), false);
  assert.equal(hasTwoServers(TWO), true);
});

test('with one server save_host gets no space at all', () => {
  assert.equal(draftSpace(null, null), undefined);
  assert.equal(draftSpace(ONE, 'a'), undefined);
  assert.equal(draftSpace(TWO, 'b'), 'b');
  assert.equal(draftSpace(TWO, null), undefined);
});

test('a server is called by its label, else by the host of its address', () => {
  assert.equal(spaceName(space('a')), 'a.example.com');
  assert.equal(spaceName(space('a', { label: '' })), 'sync.example.com:8443');
  const lock = { ...STATUS.lock, serverUrl: 'https://lock.example.org/' };
  assert.equal(
    spaceName(space('a', { label: ' ', status: { ...STATUS, serverUrl: null, lock } })),
    'lock.example.org',
  );
  assert.equal(spaceName(space('a', { label: '', status: { ...STATUS, serverUrl: null } })), '');
});

test('the host of an address drops scheme, path and the default port', () => {
  assert.equal(serverHost('https://lock.example.com:443/base?x=1'), 'lock.example.com');
  assert.equal(serverHost('sync.example.net:8443'), 'sync.example.net:8443');
  assert.equal(serverHost('  '), '');
  assert.equal(serverHost(null), '');
});

test('main is the one marked so, the other is the one that stays', () => {
  assert.equal(mainSpace(TWO)?.id, 'a');
  assert.equal(mainSpace({ ...TWO, spaces: [space('x'), space('y', { main: true })] })?.id, 'y');
  assert.equal(otherSpace(TWO, 'a')?.id, 'b');
  assert.equal(otherSpace(TWO, 'b')?.id, 'a');
  assert.equal(otherSpace(ONE, 'a'), null);
});

test('a default that names a server that is gone falls back to main', () => {
  assert.equal(defaultSpace(TWO, 'business'), 'b');
  assert.equal(
    defaultSpace({ ...TWO, defaults: { private: 'a', business: 'gone' } }, 'business'),
    'a',
  );
});

test('a new host follows the default of its workspace until a server is picked', () => {
  assert.equal(chosenSpace(TWO, null, 'private', null), 'a');
  assert.equal(chosenSpace(TWO, null, 'business', null), 'b');
  assert.equal(chosenSpace(TWO, null, 'business', 'a'), 'a');
  // A pick that names no server counts as none.
  assert.equal(chosenSpace(TWO, null, 'private', 'gone'), 'a');
});

test('a new host avoids a server that cannot take it', () => {
  assert.equal(chosenSpace(withSecond({ locked: true }), null, 'business', null), 'a');
  assert.equal(chosenSpace(withSecond({ readOnly: true }), null, 'business', null), 'a');
});

test('a stored host shows where it lives, whatever its workspace defaults to', () => {
  const host = { space: 'a' };
  assert.equal(chosenSpace(TWO, host, 'business', null), 'a');
  assert.equal(chosenSpace(TWO, host, 'business', 'b'), 'b');
  // Even on a locked server: it is not moved behind the person's back.
  assert.equal(chosenSpace(withSecond({ locked: true }), { space: 'b' }, 'private', null), 'b');
});

test('only a stored host on another server than the chosen one moves', () => {
  assert.equal(movesHost(null, 'b'), false);
  assert.equal(movesHost({ space: 'a' }, 'a'), false);
  assert.equal(movesHost({ space: 'a' }, null), false);
  assert.equal(movesHost({ space: 'a' }, 'b'), true);
});

test('a locked or read-only server keeps hosts from moving', () => {
  assert.equal(moveBlocker(TWO), null);
  assert.equal(moveBlocker(withSecond({ locked: true }))?.id, 'b');
  assert.equal(moveBlocker(withSecond({ readOnly: true }))?.id, 'b');
});

test('a second server asks for its own password only while the app is open', () => {
  const locked = withSecond({ locked: true });
  assert.equal(asksOwnPassword(locked.spaces[1]!, locked), true);
  assert.equal(asksOwnPassword(TWO.spaces[1]!, TWO), false);
  // Main never does: the unlock dialog is for that.
  assert.equal(asksOwnPassword(space('a', { main: true, locked: true }), locked), false);
  const shut: SpacesStatus = {
    ...locked,
    spaces: [space('a', { main: true, status: { ...STATUS, vault: 'locked' } }), locked.spaces[1]!],
  };
  assert.equal(asksOwnPassword(shut.spaces[1]!, shut), false);
});

test('making a server main needs both open and a server behind it', () => {
  assert.equal(canMakeMain(TWO.spaces[1]!, TWO), true);
  assert.equal(canMakeMain(TWO.spaces[0]!, TWO), false);
  const locked = withSecond({ locked: true });
  assert.equal(canMakeMain(locked.spaces[1]!, locked), false);
  const shut = withSecond({ status: { ...STATUS, vault: 'locked' } });
  assert.equal(canMakeMain(shut.spaces[1]!, shut), false);
  const loose = withSecond({ status: { ...STATUS, backend: 'none', paired: false } });
  assert.equal(canMakeMain(loose.spaces[1]!, loose), false);
});

test('entries can be taken over only while both servers are open', () => {
  assert.equal(canKeepEntries(TWO.spaces[0]!, TWO), true);
  assert.equal(canKeepEntries(TWO.spaces[1]!, TWO), true);
  const locked = withSecond({ locked: true });
  assert.equal(canKeepEntries(locked.spaces[0]!, locked), false);
  assert.equal(canKeepEntries(locked.spaces[1]!, locked), false);
  assert.equal(canKeepEntries(ONE.spaces[0]!, ONE), false);
});

test('the forms for a second server start empty, with this device’s name', () => {
  const blank = blankStatus({ ...STATUS, lock: { ...STATUS.lock, email: 'me@example.com' } });
  assert.equal(blank.deviceName, 'Laptop');
  assert.equal(blank.vault, 'absent');
  assert.equal(blank.backend, 'none');
  assert.equal(blank.paired, false);
  assert.equal(blank.serverUrl, null);
  assert.equal(blank.lock.email, null);
  assert.equal(blank.pending, 0);
});
