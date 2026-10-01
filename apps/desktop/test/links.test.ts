// Following a `uwussh://connect/<id>` link. Runs with `node --test` (Node strips the types).

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { findLinkedHost, type LinkSteps } from '../src/lib/links.ts';
import type { HostRecord, VaultStatus } from '../src/lib/session.ts';

const ID = '0b5f5d3e-8a4c-4a7e-9b1f-2c3d4e5f6a7b';

function host(id: string): HostRecord {
  return {
    id,
    name: 'web',
    address: 'web.example.com',
    port: 22,
    username: 'nyu',
    auth: 'key',
    keyPath: null,
    groupPath: null,
    lastConnectedMs: null,
    workspace: 'private',
    position: 0,
    os: null,
    hasPassword: false,
    keyId: null,
    keyLabel: null,
  };
}

/** A device with these hosts; a sync adds `arriving`. Records what was called. */
function device(options: {
  hosts?: HostRecord[];
  arriving?: HostRecord[];
  vault?: VaultStatus;
  opens?: boolean;
  sync?: 'ok' | 'off' | 'fails';
}) {
  let hosts = options.hosts ?? [];
  let vault = options.vault ?? 'unlocked';
  const calls: string[] = [];
  const steps: LinkSteps = {
    vaultStatus: async () => vault,
    unlock: async () => {
      calls.push('unlock');
      if (options.opens) vault = 'unlocked';
      return options.opens ?? false;
    },
    listHosts: async () => {
      calls.push('list');
      return hosts;
    },
    sync: async () => {
      calls.push('sync');
      if (vault === 'locked') throw { kind: 'vault-locked' };
      if (options.sync === 'off') return false;
      hosts = [...hosts, ...(options.arriving ?? [])];
      if (options.sync === 'fails') throw { kind: 'unreachable', message: 'no route' };
      return true;
    },
  };
  return { steps, calls };
}

test('a known host connects without a sync', async () => {
  const { steps, calls } = device({ hosts: [host('other'), host(ID)] });
  const outcome = await findLinkedHost(ID, steps);
  assert.deepEqual(outcome, { kind: 'host', host: host(ID) });
  assert.deepEqual(calls, ['list']);
});

test('ids compare without regard to case', async () => {
  const { steps } = device({ hosts: [host(ID.toUpperCase())] });
  assert.equal((await findLinkedHost(ID, steps)).kind, 'host');
});

test('an unknown host is looked for again after a sync', async () => {
  const { steps, calls } = device({ hosts: [host('other')], arriving: [host(ID)] });
  const outcome = await findLinkedHost(ID, steps);
  assert.deepEqual(outcome, { kind: 'host', host: host(ID) });
  assert.deepEqual(calls, ['list', 'sync', 'list']);
});

test('still unknown after the sync says so', async () => {
  const { steps } = device({ hosts: [host('other')] });
  assert.deepEqual(await findLinkedHost(ID, steps), {
    kind: 'unknown',
    synced: true,
    syncError: null,
  });
});

test('without sync it says that no sync could help', async () => {
  const { steps } = device({ sync: 'off' });
  assert.deepEqual(await findLinkedHost(ID, steps), {
    kind: 'unknown',
    synced: false,
    syncError: null,
  });
});

test('a failed sync is reported, but what it brought still counts', async () => {
  const failed = device({ sync: 'fails' });
  assert.deepEqual(await findLinkedHost(ID, failed.steps), {
    kind: 'unknown',
    synced: false,
    syncError: 'no route',
  });
  const partly = device({ sync: 'fails', arriving: [host(ID)] });
  assert.equal((await findLinkedHost(ID, partly.steps)).kind, 'host');
});

test('a locked vault is opened first, then the host is found and synced', async () => {
  const { steps, calls } = device({ vault: 'locked', opens: true, arriving: [host(ID)] });
  const outcome = await findLinkedHost(ID, steps);
  assert.equal(outcome.kind, 'host');
  assert.deepEqual(calls, ['unlock', 'list', 'sync', 'list']);
});

test('a vault left locked stops before anything else', async () => {
  const { steps, calls } = device({ vault: 'locked', opens: false, hosts: [host(ID)] });
  assert.deepEqual(await findLinkedHost(ID, steps), { kind: 'locked' });
  assert.deepEqual(calls, ['unlock']);
});

test('no vault at all needs no unlocking', async () => {
  const { steps, calls } = device({ vault: 'absent', hosts: [host(ID)] });
  assert.equal((await findLinkedHost(ID, steps)).kind, 'host');
  assert.deepEqual(calls, ['list']);
});
