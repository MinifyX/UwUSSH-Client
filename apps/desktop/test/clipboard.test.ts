// Copying: through Rust first, the webview's clipboard only when that fails.
// Runs with `node --test` (Node strips the types).

import assert from 'node:assert/strict';
import { afterEach, test } from 'node:test';
import { copyText } from '../src/lib/clipboard.ts';

const scope = globalThis as Record<string, unknown>;
let written: string[] = [];

Object.defineProperty(globalThis.navigator, 'clipboard', {
  configurable: true,
  value: {
    writeText: async (text: string) => {
      written.push(text);
    },
  },
});

afterEach(() => {
  delete scope.window;
  written = [];
});

test('the text goes to Rust, and the webview is left alone', async () => {
  const calls: unknown[] = [];
  scope.window = {
    __TAURI_INTERNALS__: {
      invoke: async (command: string, args: unknown) => {
        calls.push([command, args]);
      },
    },
  };
  await copyText('ssh-ed25519 AAAA user@host.example');
  assert.deepEqual(calls, [['copy_text', { text: 'ssh-ed25519 AAAA user@host.example' }]]);
  assert.deepEqual(written, []);
});

test('when Rust says no, the webview copies', async () => {
  scope.window = {
    __TAURI_INTERNALS__: {
      invoke: async () => {
        throw new Error("Couldn't copy");
      },
    },
  };
  await copyText('https://example.com/');
  assert.deepEqual(written, ['https://example.com/']);
});
