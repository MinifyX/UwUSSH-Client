// The shortcut table, on every system. Runs with `node --test` (Node strips the types).

import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  actionId,
  actionOf,
  allChordLabels,
  bind,
  chordsFor,
  chordWarning,
  holderOf,
  menuAccelerator,
  pruneOverrides,
  resetAction,
  unbind,
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

test('on the Mac Ctrl stays with the terminal and ⌘F stays free', () => {
  const at = (event: KeyLike, selection = true) => shortcutFor(event, ON, selection, 'macos');
  assert.equal(at(key('KeyC', 'ctrl')), null, '⌃C is the interrupt, selection or not');
  assert.equal(at(key('KeyT', 'ctrl shift')), null);
  assert.equal(at(key('KeyW', 'ctrl')), null);
  assert.equal(at(key('Digit1', 'ctrl shift')), null);
  assert.equal(at(key('KeyF', 'meta')), null);
  assert.equal(at(key('KeyV', 'meta')), null, '⌘V is pasted by the webview');
});

test('the command assistant is ⌘K on the Mac and Ctrl+Shift+K elsewhere', () => {
  assert.deepEqual(shortcutFor(key('KeyK', 'meta'), ON, false, 'macos'), { kind: 'assist' });
  assert.equal(shortcutFor(key('KeyK', 'ctrl'), ON, false, 'macos'), null, '⌃K kills the line');
  assert.equal(shortcutFor(key('KeyK', 'ctrl shift'), ON, false, 'macos'), null);
  for (const system of ['windows', 'linux'] as const) {
    assert.deepEqual(shortcutFor(key('KeyK', 'ctrl shift'), ON, false, system), {
      kind: 'assist',
    });
    assert.equal(
      shortcutFor(key('KeyK', 'ctrl'), ON, false, system),
      null,
      'Ctrl+K is the shell’s',
    );
    assert.equal(shortcutFor(key('KeyK', 'ctrl shift alt'), ON, false, system), null);
  }
  assert.equal(shortcutLabel('assist', 'macos', NAMES), '⌘K');
  assert.equal(shortcutLabel('assist', 'linux', NAMES), 'Strg+Umschalt+K');
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
  assert.deepEqual(allChordLabels('copy', 'windows', NAMES), ['Strg+Umschalt+C']);
});

test('the user’s own keys replace the defaults', () => {
  const chord = { ctrl: true, alt: true, code: 'KeyT' };
  const own = bind({}, 'other', 'new-shell', chord);
  assert.deepEqual(shortcutFor(key('KeyT', 'ctrl alt'), ON, false, 'linux', own), {
    kind: 'new-shell',
  });
  // The default stays beside it until it is taken away.
  const bare = unbind(own, 'other', 'new-shell', { ctrl: true, shift: true, code: 'KeyT' });
  assert.equal(shortcutFor(key('KeyT', 'ctrl shift'), ON, false, 'linux', bare), null);
  assert.equal(chordsFor('new-shell', 'other', bare).length, 1);
  assert.equal(
    chordsFor('new-shell', 'other', unbind(bare, 'other', 'new-shell', chord)).length,
    0,
  );
  assert.deepEqual(resetAction(bare, 'other', 'new-shell'), {});
});

test('a combination belongs to one action: binding it takes it over', () => {
  const taken = { ctrl: true, shift: true, code: 'KeyW' };
  assert.equal(holderOf(taken, 'other', {}), 'close-tab');
  const own = bind({}, 'other', 'tunnels', taken);
  assert.equal(holderOf(taken, 'other', own), 'tunnels');
  assert.deepEqual(chordsFor('close-tab', 'other', own), []);
  assert.deepEqual(shortcutFor(key('KeyW', 'ctrl shift'), ON, false, 'windows', own), {
    kind: 'tunnels',
  });
  // Putting the default back does not steal the key from its new owner.
  assert.deepEqual(chordsFor('close-tab', 'other', resetAction(own, 'other', 'close-tab')), []);
});

test('hosts and texts are actions of their own', () => {
  const host = 'host:8f0c1d2e-0000-4000-8000-000000000001';
  const text = 'text:8f0c1d2e-0000-4000-8000-000000000002';
  assert.deepEqual(actionOf(host), { kind: 'host', id: host.slice(5) });
  assert.equal(actionId({ kind: 'text', id: text.slice(5) }), text);
  assert.equal(actionOf('nonsense'), null);
  let own = bind({}, 'mac', host, { meta: true, alt: true, code: 'Digit1' });
  own = bind(own, 'mac', text, { ctrl: true, shift: true, code: 'KeyL' });
  assert.deepEqual(shortcutFor(key('Digit1', 'meta alt'), ON, false, 'macos', own), {
    kind: 'host',
    id: host.slice(5),
  });
  assert.deepEqual(shortcutFor(key('KeyL', 'ctrl shift'), ON, false, 'macos', own), {
    kind: 'text',
    id: text.slice(5),
  });
  assert.deepEqual(
    Object.keys(pruneOverrides(own, (id) => id === host)),
    [host],
    'a deleted text loses its key',
  );
});

test('risky combinations are named', () => {
  assert.equal(chordWarning({ code: 'KeyA' }, 'other'), 'typing');
  assert.equal(chordWarning({ code: 'F5' }, 'other'), null);
  assert.equal(chordWarning({ ctrl: true, code: 'KeyR' }, 'other'), 'terminal');
  assert.equal(chordWarning({ ctrl: true, shift: true, code: 'KeyR' }, 'other'), null);
  assert.equal(chordWarning({ ctrl: true, alt: true, code: 'KeyQ' }, 'other'), 'altgr');
  assert.equal(chordWarning({ alt: true, code: 'F4' }, 'other'), 'system');
  assert.equal(chordWarning({ meta: true, code: 'KeyQ' }, 'mac'), 'system');
  assert.equal(chordWarning({ alt: true, code: 'KeyL' }, 'mac'), 'option');
  assert.equal(chordWarning({ meta: true, code: 'KeyB' }, 'mac'), null);
});

test('the menu bar shows the key that is bound', () => {
  assert.equal(menuAccelerator('close-tab'), 'Cmd+W');
  assert.equal(menuAccelerator('open-files'), 'Shift+Cmd+F');
  const own = bind({}, 'mac', 'tunnels', { meta: true, alt: true, code: 'KeyU' });
  assert.equal(menuAccelerator('tunnels', own), 'Alt+Cmd+U');
  // ⌃ alone is no menu key: the page handles it.
  const ctrl = bind({}, 'mac', 'reconnect', { ctrl: true, shift: true, code: 'KeyR' });
  assert.equal(menuAccelerator('reconnect', ctrl), undefined);
});
