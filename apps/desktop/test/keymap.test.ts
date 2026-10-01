// The shortcut table, on every system. Runs with `node --test` (Node strips the types).

import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  allChordLabels,
  hasPrimaryModifier,
  shortcutFor,
  shortcutLabel,
  tabLabel,
  terminalKey,
  type KeyLike,
} from '../src/lib/keymap.ts';
import { platformOf } from '../src/lib/platform.ts';

const NAMES = {
  ctrl: 'Strg',
  shift: 'Umschalt',
  alt: 'Alt',
  pageUp: 'Bild auf',
  pageDown: 'Bild ab',
};
const ON = { ctrlCCopies: true, ctrlVPastes: true };
const OFF = { ctrlCCopies: false, ctrlVPastes: false };

/** A keydown; `mods` is any of "ctrl shift alt meta". */
function key(code: string, mods = '', extra: Partial<KeyLike> = {}): KeyLike {
  const letter = /^Key(\w)$/.exec(code)?.[1]?.toLowerCase();
  const digit = /^Digit(\d)$/.exec(code)?.[1];
  return {
    type: 'keydown',
    code,
    key: letter ?? digit ?? code,
    ctrlKey: mods.includes('ctrl'),
    shiftKey: mods.includes('shift'),
    altKey: mods.includes('alt'),
    metaKey: mods.includes('meta'),
    ...extra,
  };
}

test('the system is read from the user agent, macOS before Windows', () => {
  const safari =
    'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) MacIntel';
  assert.equal(platformOf(safari), 'macos');
  assert.equal(platformOf('Darwin MacIntel'), 'macos');
  assert.equal(platformOf('Mozilla/5.0 (Windows NT 10.0; Win64; x64) Edg/130 Win32'), 'windows');
  assert.equal(
    platformOf('Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 Linux x86_64'),
    'linux',
  );
});

test('on the Mac the tab shortcuts are ⌘ ones, like in Terminal.app', () => {
  const at = (event: KeyLike) => shortcutFor(event, ON, false, 'macos');
  assert.deepEqual(at(key('KeyT', 'meta')), { kind: 'new-shell' });
  assert.deepEqual(at(key('KeyW', 'meta')), { kind: 'close-tab' });
  assert.deepEqual(at(key('KeyD', 'meta')), { kind: 'duplicate-tab' });
  assert.deepEqual(at(key('Digit1', 'meta')), { kind: 'select-tab', index: 0 });
  assert.deepEqual(at(key('Digit9', 'meta')), { kind: 'select-tab', index: 8 });
  assert.deepEqual(at(key('Comma', 'meta')), { kind: 'settings' });
  assert.deepEqual(at(key('BracketRight', 'meta shift', { key: '}' })), { kind: 'next-tab' });
  assert.deepEqual(at(key('BracketLeft', 'meta shift', { key: '{' })), { kind: 'previous-tab' });
  assert.deepEqual(at(key('Tab', 'ctrl')), { kind: 'next-tab' });
  assert.deepEqual(at(key('Tab', 'ctrl shift')), { kind: 'previous-tab' });
  assert.deepEqual(at(key('ArrowRight', 'meta alt')), { kind: 'next-tab' });
  assert.deepEqual(at(key('KeyF', 'meta shift')), { kind: 'open-files' });
  assert.deepEqual(at(key('KeyP', 'meta shift')), { kind: 'type-password' });
  assert.deepEqual(at(key('KeyA', 'meta')), { kind: 'select-all' });
});

test('on the Mac Ctrl stays with the terminal and ⌘K and ⌘F stay free', () => {
  const at = (event: KeyLike, selection = true) => shortcutFor(event, ON, selection, 'macos');
  assert.equal(at(key('KeyC', 'ctrl')), null, '⌃C is the interrupt, selection or not');
  assert.equal(at(key('KeyT', 'ctrl shift')), null);
  assert.equal(at(key('KeyW', 'ctrl')), null);
  assert.equal(at(key('Digit1', 'ctrl shift')), null);
  assert.equal(at(key('KeyK', 'meta')), null, '⌘K is kept for the AI command');
  assert.equal(at(key('KeyF', 'meta')), null);
  assert.equal(at(key('KeyV', 'meta')), null, '⌘V is pasted by the webview');
});

test('⌘C copies a selection and is left alone without one', () => {
  assert.deepEqual(shortcutFor(key('KeyC', 'meta'), OFF, true, 'macos'), { kind: 'copy' });
  assert.equal(shortcutFor(key('KeyC', 'meta'), OFF, false, 'macos'), null);
});

test('on Windows and Linux everything stays as it was', () => {
  for (const system of ['windows', 'linux'] as const) {
    const at = (event: KeyLike, selection = false, settings = ON) =>
      shortcutFor(event, settings, selection, system);
    assert.deepEqual(at(key('KeyT', 'ctrl shift')), { kind: 'new-shell' });
    assert.deepEqual(at(key('KeyW', 'ctrl shift')), { kind: 'close-tab' });
    assert.deepEqual(at(key('KeyD', 'ctrl shift')), { kind: 'duplicate-tab' });
    assert.deepEqual(at(key('Digit3', 'ctrl shift')), { kind: 'select-tab', index: 2 });
    assert.deepEqual(at(key('Tab', 'ctrl')), { kind: 'next-tab' });
    assert.deepEqual(at(key('Tab', 'ctrl shift')), { kind: 'previous-tab' });
    assert.deepEqual(at(key('PageDown', 'ctrl')), { kind: 'next-tab' });
    assert.deepEqual(at(key('PageUp', 'ctrl')), { kind: 'previous-tab' });
    assert.deepEqual(at(key('Comma', 'ctrl')), { kind: 'settings' });
    assert.deepEqual(at(key('KeyF', 'ctrl shift')), { kind: 'open-files' });
    assert.deepEqual(at(key('KeyP', 'ctrl shift')), { kind: 'type-password' });
    assert.deepEqual(at(key('KeyC', 'ctrl shift')), null, 'nothing selected, nothing to copy');
    assert.deepEqual(at(key('KeyC', 'ctrl shift'), true), { kind: 'copy' });
    assert.deepEqual(at(key('KeyC', 'ctrl'), true), { kind: 'copy' });
    assert.equal(at(key('KeyC', 'ctrl'), true, OFF), null, 'the setting is off');
    assert.equal(at(key('KeyC', 'ctrl'), false), null, 'without a selection it is ^C');
    assert.equal(at(key('KeyW', 'ctrl')), null, 'Ctrl+W deletes a word in bash');
    assert.equal(at(key('Digit7', 'ctrl alt')), null, 'AltGr+7 is a {');
    assert.equal(at(key('KeyT', 'meta')), null);
    assert.equal(at(key('KeyA', 'meta')), null);
  }
});

test('a key the input method is composing is never a shortcut', () => {
  const composing = key('KeyT', 'meta', { isComposing: true });
  assert.equal(shortcutFor(composing, ON, false, 'macos'), null);
  const ime = key('KeyT', 'ctrl shift', { keyCode: 229 });
  assert.equal(shortcutFor(ime, ON, false, 'windows'), null);
});

test('no ⌘ combination reaches the program on the Mac', () => {
  const at = (event: KeyLike) => terminalKey(event, ON, 'macos');
  assert.deepEqual(at(key('ArrowLeft', 'meta')), { kind: 'send', data: '\x01' });
  assert.deepEqual(at(key('ArrowRight', 'meta')), { kind: 'send', data: '\x05' });
  assert.deepEqual(at(key('Backspace', 'meta')), { kind: 'send', data: '\x15' });
  assert.deepEqual(at(key('Backspace', 'meta', { type: 'keyup' })), { kind: 'browser' });
  for (const code of ['Enter', 'Escape', 'Delete', 'KeyV', 'KeyC', 'KeyK', 'F5', 'Digit1']) {
    assert.deepEqual(at(key(code, 'meta')), { kind: 'browser' }, code);
    assert.deepEqual(at(key(code, 'meta shift')), { kind: 'browser' }, code);
  }
  assert.deepEqual(at(key('KeyC', 'ctrl')), { kind: 'pass' }, '⌃C goes on as ^C');
  assert.deepEqual(at(key('KeyV', 'ctrl')), { kind: 'pass' }, '⌃V goes on as ^V');
  assert.deepEqual(
    at(key('KeyL', 'alt', { key: '@' })),
    { kind: 'pass' },
    '⌥L is @ on German keys',
  );
});

test('pasting on Windows and Linux follows the setting', () => {
  const at = (event: KeyLike, settings = ON) => terminalKey(event, settings, 'linux');
  assert.deepEqual(at(key('KeyV', 'ctrl shift')), { kind: 'browser' });
  assert.deepEqual(at(key('Insert', 'shift')), { kind: 'browser' });
  assert.deepEqual(at(key('KeyV', 'ctrl')), { kind: 'browser' });
  assert.deepEqual(at(key('KeyV', 'ctrl'), OFF), { kind: 'pass' });
  assert.deepEqual(at(key('KeyC', 'ctrl')), { kind: 'pass' });
});

test('links and multiple selection follow ⌘ on the Mac and Ctrl elsewhere', () => {
  assert.equal(hasPrimaryModifier({ ctrlKey: false, metaKey: true }, 'macos'), true);
  assert.equal(hasPrimaryModifier({ ctrlKey: true, metaKey: false }, 'macos'), false);
  assert.equal(hasPrimaryModifier({ ctrlKey: true, metaKey: false }, 'windows'), true);
});

test('labels speak the system’s language', () => {
  assert.equal(shortcutLabel('new-shell', 'macos', NAMES), '⌘T');
  assert.equal(shortcutLabel('open-files', 'macos', NAMES), '⇧⌘F');
  assert.equal(shortcutLabel('settings', 'macos', NAMES), '⌘,');
  assert.equal(shortcutLabel('new-shell', 'windows', NAMES), 'Strg+Umschalt+T');
  assert.equal(shortcutLabel('settings', 'linux', NAMES), 'Strg+,');
  assert.equal(
    shortcutLabel('close-tab', 'windows', { ...NAMES, ctrl: 'Ctrl', shift: 'Shift' }),
    'Ctrl+Shift+W',
  );
  assert.equal(tabLabel(3, 'macos', NAMES), '⌘3');
  assert.equal(tabLabel(3, 'linux', NAMES), 'Strg+Umschalt+3');
  assert.deepEqual(allChordLabels('next-tab', 'macos', NAMES), ['⇧⌘]', '⌃⇥', '⌥⌘→']);
  assert.deepEqual(allChordLabels('next-tab', 'windows', NAMES), ['Strg+Tab', 'Strg+Bild ab']);
  assert.deepEqual(allChordLabels('copy', 'windows', NAMES), ['Strg+Umschalt+C', 'Strg+C']);
});
