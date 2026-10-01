/**
 * Every keyboard shortcut of the app, per system, in one table.
 *
 * On Windows and Linux the app's shortcuts are Ctrl+Shift+letter, like in
 * Windows Terminal: plain Ctrl+letter belongs to the program in the terminal
 * (Ctrl+W deletes a word in bash), and Alt is never used, because AltGr is
 * Ctrl+Alt on a German keyboard and AltGr+7 has to stay a `{`.
 *
 * On macOS they are ⌘+letter, like in Terminal.app: ⌘ never reaches the
 * program in the terminal, so ⌘T, ⌘W, ⌘1 … ⌘9 can be had without Shift, and
 * Ctrl stays the terminal's — ⌃C is always an interrupt there.
 *
 * Adding a shortcut is one row in {@link BINDINGS}, one case in the switch in
 * App.tsx, and {@link shortcutLabel} has its label for tooltips. ⌘K
 * (Ctrl+Shift+K elsewhere — plain Ctrl+K belongs to the shell) opens the
 * command assistant.
 *
 * This file only imports types, so `node --test` can run its tests as they are.
 */

import type { Platform } from './platform';
import type { Settings } from './settings';

export type ShortcutAction =
  | { kind: 'new-shell' }
  | { kind: 'type-password' }
  | { kind: 'open-files' }
  | { kind: 'close-tab' }
  | { kind: 'duplicate-tab' }
  | { kind: 'next-tab' }
  | { kind: 'previous-tab' }
  | { kind: 'select-tab'; index: number }
  | { kind: 'settings' }
  | { kind: 'copy' }
  | { kind: 'select-all' }
  | { kind: 'assist' };

/** The shortcuts that are a plain key combination; `select-tab` takes a digit. */
export type BoundAction = Exclude<ShortcutAction, { kind: 'select-tab' }>['kind'];

export type KeyLike = Pick<
  KeyboardEvent,
  'key' | 'code' | 'ctrlKey' | 'shiftKey' | 'altKey' | 'metaKey'
> &
  Partial<Pick<KeyboardEvent, 'isComposing' | 'keyCode' | 'type'>>;

/** Which modifiers to hold, exactly, and which physical key (`KeyboardEvent.code`). */
export type Chord = {
  ctrl?: boolean;
  shift?: boolean;
  alt?: boolean;
  meta?: boolean;
  code: string;
  /**
   * Characters that count as this key too. `[` and `]` sit elsewhere on other
   * layouts; ⇧⌘] is what Terminal.app says, whatever key makes the `}`.
   */
  keys?: string[];
};

const cmd = (code: string, keys?: string[]): Chord => ({ meta: true, code, keys });
const shiftCmd = (code: string, keys?: string[]): Chord => ({
  meta: true,
  shift: true,
  code,
  keys,
});
const ctrl = (code: string): Chord => ({ ctrl: true, code });
const ctrlShift = (code: string): Chord => ({ ctrl: true, shift: true, code });

type Binding = {
  action: BoundAction;
  mac: Chord[];
  other: Chord[];
  /** Only with text selected in the terminal; otherwise the key goes on to the terminal. */
  needsSelection?: 'always' | 'plain-ctrl';
};

/** The first chord of each list is the one shown in tooltips and in the settings. */
const BINDINGS: Binding[] = [
  { action: 'new-shell', mac: [cmd('KeyT')], other: [ctrlShift('KeyT')] },
  { action: 'close-tab', mac: [cmd('KeyW')], other: [ctrlShift('KeyW')] },
  { action: 'duplicate-tab', mac: [cmd('KeyD')], other: [ctrlShift('KeyD')] },
  {
    action: 'next-tab',
    mac: [
      shiftCmd('BracketRight', [']', '}']),
      ctrl('Tab'),
      { meta: true, alt: true, code: 'ArrowRight' },
    ],
    other: [ctrl('Tab'), ctrl('PageDown')],
  },
  {
    action: 'previous-tab',
    mac: [
      shiftCmd('BracketLeft', ['[', '{']),
      ctrlShift('Tab'),
      { meta: true, alt: true, code: 'ArrowLeft' },
    ],
    other: [ctrlShift('Tab'), ctrl('PageUp')],
  },
  { action: 'settings', mac: [cmd('Comma')], other: [ctrl('Comma')] },
  // ⌘F stays free for finding text, as in every Mac app.
  { action: 'open-files', mac: [shiftCmd('KeyF')], other: [ctrlShift('KeyF')] },
  { action: 'type-password', mac: [shiftCmd('KeyP')], other: [ctrlShift('KeyP')] },
  // ⌘C copies only a selection; without one there is nothing to copy, and ⌃C
  // is the interrupt either way. Ctrl+C copies only when the setting says so.
  { action: 'copy', mac: [cmd('KeyC')], other: [ctrlShift('KeyC')], needsSelection: 'always' },
  { action: 'copy', mac: [], other: [ctrl('KeyC')], needsSelection: 'plain-ctrl' },
  // xterm.js selects everything on ⌘A by itself, but only while it has the
  // keyboard; through the table it works the same from anywhere.
  { action: 'select-all', mac: [cmd('KeyA')], other: [] },
  // The command assistant, over the terminal it types into.
  { action: 'assist', mac: [cmd('KeyK')], other: [ctrlShift('KeyK')] },
];

function matches(event: KeyLike, chord: Chord): boolean {
  if (event.ctrlKey !== Boolean(chord.ctrl)) return false;
  if (event.altKey !== Boolean(chord.alt)) return false;
  if (event.metaKey !== Boolean(chord.meta)) return false;
  if (chord.keys?.includes(event.key)) return true;
  if (event.shiftKey !== Boolean(chord.shift)) return false;
  return event.code === chord.code;
}

/** The IME is composing (dead keys, Japanese input): the key is not ours. */
function composing(event: KeyLike): boolean {
  return Boolean(event.isComposing) || event.keyCode === 229;
}

/** The app's shortcut for this key, or `null` when the key belongs to whatever has focus. */
export function shortcutFor(
  event: KeyLike,
  settings: Pick<Settings, 'ctrlCCopies'>,
  hasSelection: boolean,
  system: Platform,
): ShortcutAction | null {
  if (composing(event)) return null;
  const mac = system === 'macos';
  for (const binding of BINDINGS) {
    const chords = mac ? binding.mac : binding.other;
    if (!chords.some((chord) => matches(event, chord))) continue;
    if (binding.needsSelection && !hasSelection) continue;
    if (binding.needsSelection === 'plain-ctrl' && !settings.ctrlCCopies) continue;
    return { kind: binding.action } as ShortcutAction;
  }
  const digit = /^Digit([1-9])$/.exec(event.code);
  const tabChord = mac ? cmd(event.code) : ctrlShift(event.code);
  if (digit && matches(event, tabChord)) return { kind: 'select-tab', index: Number(digit[1]) - 1 };
  return null;
}

/**
 * What the terminal does with a key before xterm.js sees it:
 *
 * - `pass` — xterm.js handles it as usual
 * - `browser` — xterm.js keeps out and the webview does its default: pasting
 *   from the clipboard (no clipboard permission needed, and xterm.js still
 *   wraps it in bracketed paste), the native Edit menu on macOS
 * - `send` — these bytes go to the program instead
 *
 * On macOS no ⌘ combination reaches the program as it is: xterm.js would send
 * ⌘⌫ as a plain Backspace, ⌘↩ as Enter, ⌘Entf as `ESC[3;9~`. The three that
 * mean something in every Mac text field mean the same in the shell, as with
 * iTerm2's "natural text editing": ⌘← and ⌘→ jump to the start and end of the
 * line (^A, ^E), ⌘⌫ deletes back to its start (^U).
 */
export type TerminalKey = { kind: 'pass' } | { kind: 'browser' } | { kind: 'send'; data: string };

const MAC_LINE_KEYS: Record<string, string> = {
  ArrowLeft: '\x01',
  ArrowRight: '\x05',
  Backspace: '\x15',
};

export function terminalKey(
  event: KeyLike,
  settings: Pick<Settings, 'ctrlVPastes'>,
  system: Platform,
): TerminalKey {
  if (system === 'macos') {
    if (!event.metaKey) return { kind: 'pass' };
    const plain = !event.ctrlKey && !event.altKey && !event.shiftKey;
    const line = plain && event.type === 'keydown' ? MAC_LINE_KEYS[event.code] : undefined;
    if (line) return { kind: 'send', data: line };
    return { kind: 'browser' };
  }
  if (isPasteKey(event, settings)) return { kind: 'browser' };
  return { kind: 'pass' };
}

/** Ctrl+Shift+V, Shift+Insert and, when the setting says so, Ctrl+V (Windows and Linux). */
function isPasteKey(event: KeyLike, settings: Pick<Settings, 'ctrlVPastes'>): boolean {
  if (event.altKey || event.metaKey) return false;
  if (event.shiftKey && !event.ctrlKey && event.code === 'Insert') return true;
  if (!event.ctrlKey || event.code !== 'KeyV') return false;
  return event.shiftKey || settings.ctrlVPastes;
}

/** The modifier key with the system's "copy a link / several items" meaning: ⌘ or Ctrl. */
export function hasPrimaryModifier(
  event: Pick<KeyLike, 'ctrlKey' | 'metaKey'>,
  system: Platform,
): boolean {
  return system === 'macos' ? event.metaKey : event.ctrlKey;
}

// ── Labels ──────────────────────────────────────────────────────────────────

/** What the keys are called in the app's language; the Mac uses symbols. */
export type KeyNames = {
  ctrl: string;
  shift: string;
  alt: string;
  pageUp: string;
  pageDown: string;
};

const KEY_SYMBOLS: Record<string, string> = {
  Comma: ',',
  Tab: '⇥',
  BracketLeft: '[',
  BracketRight: ']',
  ArrowLeft: '←',
  ArrowRight: '→',
  PageUp: '⇞',
  PageDown: '⇟',
};

function keyName(code: string, mac: boolean, names: KeyNames): string {
  const letter = /^(?:Key|Digit)(\w)$/.exec(code);
  if (letter) return letter[1]!;
  if (!mac) {
    if (code === 'Tab') return 'Tab';
    if (code === 'PageUp') return names.pageUp;
    if (code === 'PageDown') return names.pageDown;
  }
  return KEY_SYMBOLS[code] ?? code;
}

/** `⇧⌘F` on a Mac (in Apple's order ⌃⌥⇧⌘), `Strg+Umschalt+F` elsewhere. */
export function chordLabel(chord: Chord, system: Platform, names: KeyNames): string {
  const mac = system === 'macos';
  const key = keyName(chord.code, mac, names);
  if (mac) {
    return `${chord.ctrl ? '⌃' : ''}${chord.alt ? '⌥' : ''}${chord.shift ? '⇧' : ''}${chord.meta ? '⌘' : ''}${key}`;
  }
  const parts = [
    chord.ctrl ? names.ctrl : null,
    chord.shift ? names.shift : null,
    chord.alt ? names.alt : null,
    key,
  ];
  return parts.filter(Boolean).join('+');
}

/** The label of an action's main chord, for tooltips: `⌘T`, `Strg+Umschalt+T`. */
export function shortcutLabel(action: BoundAction, system: Platform, names: KeyNames): string {
  const binding = BINDINGS.find((candidate) => candidate.action === action);
  const chord = (system === 'macos' ? binding?.mac : binding?.other)?.[0];
  return chord ? chordLabel(chord, system, names) : '';
}

/** `⌘3`, `Strg+Umschalt+3`. */
export function tabLabel(number: number | string, system: Platform, names: KeyNames): string {
  const chord = system === 'macos' ? cmd(`Digit${number}`) : ctrlShift(`Digit${number}`);
  return chordLabel(chord, system, names);
}

/** Every chord of an action, for the list in the settings: `⇧⌘] · ⌃⇥ · ⌥⌘→`. */
export function allChordLabels(action: BoundAction, system: Platform, names: KeyNames): string[] {
  return BINDINGS.filter((binding) => binding.action === action)
    .flatMap((binding) => (system === 'macos' ? binding.mac : binding.other))
    .map((chord) => chordLabel(chord, system, names));
}

/** The modifier for "hold while scrolling" and "hold while clicking": `⌘` or `Strg`. */
export function primaryModifierLabel(system: Platform, names: KeyNames): string {
  return system === 'macos' ? '⌘' : names.ctrl;
}
