// The overview's quick connect and recent hosts (lib/home.ts). Runs with `node --test`.

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { matchQuickConnect, parseQuickConnect, recentHosts } from '../src/lib/home.ts';
import type { HostRecord } from '../src/lib/session.ts';

const host = (id: string, extra: Partial<HostRecord> = {}): HostRecord =>
  ({
    id,
    name: id,
    address: `${id}.example.com`,
    port: 22,
    username: 'admin',
    lastConnectedMs: null,
    ...extra,
  }) as HostRecord;

test('quick connect reads user@host:port', () => {
  assert.deepEqual(parseQuickConnect(' root@db.example.com:2222 '), {
    username: 'root',
    address: 'db.example.com',
    port: 2222,
  });
  assert.deepEqual(parseQuickConnect('192.0.2.10'), {
    username: null,
    address: '192.0.2.10',
    port: null,
  });
  assert.deepEqual(parseQuickConnect('me@[2001:db8::1]:22'), {
    username: 'me',
    address: '2001:db8::1',
    port: 22,
  });
  assert.deepEqual(parseQuickConnect('2001:db8::1'), {
    username: null,
    address: '2001:db8::1',
    port: null,
  });
});

test('quick connect refuses what is no target', () => {
  for (const text of ['', '  ', 'a b', '@host', 'user@', 'host:0', 'host:70000', 'host:']) {
    assert.equal(parseQuickConnect(text), null, text);
  }
});

test('quick connect finds a stored host by name or address', () => {
  const hosts = [
    host('web'),
    host('db', { port: 2222 }),
    host('db2', { address: 'db.example.com', username: 'root' }),
  ];
  assert.equal(matchQuickConnect(hosts, parseQuickConnect('WEB')!)?.id, 'web');
  assert.equal(matchQuickConnect(hosts, parseQuickConnect('web.example.com')!)?.id, 'web');
  // Two hosts share the address: user or port tell them apart.
  assert.equal(matchQuickConnect(hosts, parseQuickConnect('db.example.com')!), null);
  assert.equal(matchQuickConnect(hosts, parseQuickConnect('root@db.example.com')!)?.id, 'db2');
  assert.equal(matchQuickConnect(hosts, parseQuickConnect('db.example.com:2222')!)?.id, 'db');
  assert.equal(matchQuickConnect(hosts, parseQuickConnect('other.example.com')!), null);
});

test('recent hosts: last connected first, at most four, never-used left out', () => {
  const hosts = [
    host('a', { lastConnectedMs: 10 }),
    host('b'),
    host('c', { lastConnectedMs: 30 }),
    host('d', { lastConnectedMs: 20 }),
    host('e', { lastConnectedMs: 5 }),
    host('f', { lastConnectedMs: 1 }),
  ];
  assert.deepEqual(
    recentHosts(hosts).map((h) => h.id),
    ['c', 'd', 'a', 'e'],
  );
});
