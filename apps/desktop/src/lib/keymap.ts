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
 * Those are the defaults. Every action can be moved to other keys, and actions
 * without a default (a host to connect to, a text to type) can be given some:
 * the user's own keys are {@link Overrides}, one list of chords per action,
 * which replaces that action's defaults — an empty list unbinds it. They are
 * kept apart for the Mac and for everything else, because ⌘ exists only there.
 *
 * Adding a shortcut is one row in {@link DEFAULTS}, one case in the switch in
 * App.tsx, and its name in lib/shortcuts.ts. ⌘K (Ctrl+Shift+K elsewhere —
 * plain Ctrl+K belongs to the shell) opens the command assistant.
 *
 * This file only imports types, so `node --test` can run its tests as they are.
 */

import type { Platform } from './platform';
import type { Settings } from './settings';

/** The actions that are a plain key combination; in the order the settings list them. */
export const ACTION_GROUPS = {
  tabs: [
    'new-shell',
    'close-tab',
    'duplicate-tab',
    'next-tab',
    'previous-tab',
    'move-tab-left',
    'move-tab-right',
  ],
  terminal: [
    'copy',
    'select-all',
    'clear-terminal',
    'type-password',
    'assist',
    'open-files',
    'font-bigger',
    'font-smaller',
    'font-reset',
    'reconnect',
  ],
  app: ['settings', 'shortcuts', 'toggle-sidebar', 'focus-search', 'new-host', 'tunnels'],
} as const;

export type ActionGroup = keyof typeof ACTION_GROUPS;
/** The shortcuts that are a plain key combination; `select-tab` takes a digit. */
export type BoundAction = (typeof ACTION_GROUPS)[ActionGroup][number];

export type ShortcutAction =
  | { kind: BoundAction }
  | { kind: 'select-tab'; index: number }
  /** Connects to this host in a new tab. */
  | { kind: 'host'; id: string }
  /** Types one of the user's texts into the terminal in front. */
  | { kind: 'text'; id: string };

/** `mac` has ⌘; `other` is Windows and Linux. The user's keys are kept per family. */
export type Family = 'mac' | 'other';

export function familyOf(system: Platform): Family {
  return system === 'macos' ? 'mac' : 'other';
}

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
   * Only defaults have these.
   */
  keys?: string[];
};

/** The user's own keys: an action's id and the chords it has instead of its defaults. */
export type Overrides = Readonly<Record<string, readonly Chord[]>>;

const cmd = (code: string, keys?: string[]): Chord => ({ meta: true, code, keys });
const shiftCmd = (code: string, keys?: string[]): Chord => ({
  meta: true,
  shift: true,
  code,
  keys,
});
const ctrl = (code: string): Chord => ({ ctrl: true, code });
const ctrlShift = (code: string): Chord => ({ ctrl: true, shift: true, code });

const TAB_NUMBERS = [1, 2, 3, 4, 5, 6, 7, 8, 9] as const;
/** `select-tab-1` … `select-tab-9`. */
export const TAB_ACTIONS: readonly string[] = TAB_NUMBERS.map((n) => `select-tab-${n}`);

/** The first chord of each list is the one shown in tooltips. */
const DEFAULTS: Record<string, { mac: Chord[]; other: Chord[] }> = {
  'new-shell': { mac: [cmd('KeyT')], other: [ctrlShift('KeyT')] },
  'close-tab': { mac: [cmd('KeyW')], other: [ctrlShift('KeyW')] },
  'duplicate-tab': { mac: [cmd('KeyD')], other: [ctrlShift('KeyD')] },
  'next-tab': {
    mac: [
      shiftCmd('BracketRight', [']', '}']),
      ctrl('Tab'),
      { meta: true, alt: true, code: 'ArrowRight' },
    ],
    other: [ctrl('Tab'), ctrl('PageDown')],
  },
  'previous-tab': {
    mac: [
      shiftCmd('BracketLeft', ['[', '{']),
      ctrlShift('Tab'),
      { meta: true, alt: true, code: 'ArrowLeft' },
    ],
    other: [ctrlShift('Tab'), ctrl('PageUp')],
  },
  settings: { mac: [cmd('Comma')], other: [ctrl('Comma')] },
  // ⌘F stays free for finding text, as in every Mac app.
  'open-files': { mac: [shiftCmd('KeyF')], other: [ctrlShift('KeyF')] },
  'type-password': { mac: [shiftCmd('KeyP')], other: [ctrlShift('KeyP')] },
  // Copying needs a selection; without one the key goes on to the terminal.
  // Plain Ctrl+C is not in the table: it copies when the setting says so, see
  // shortcutFor.
  copy: { mac: [cmd('KeyC')], other: [ctrlShift('KeyC')] },
  // xterm.js selects everything on ⌘A by itself, but only while it has the
  // keyboard; through the table it works the same from anywhere.
  'select-all': { mac: [cmd('KeyA')], other: [] },
  // The command assistant, over the terminal it types into.
  assist: { mac: [cmd('KeyK')], other: [ctrlShift('KeyK')] },
  // As in VS Code. ⌘B means nothing to a terminal.
  'toggle-sidebar': { mac: [cmd('KeyB')], other: [ctrlShift('KeyB')] },
  'new-host': { mac: [cmd('KeyN')], other: [] },
  ...Object.fromEntries(
    TAB_NUMBERS.map((n) => [
      `select-tab-${n}`,
      { mac: [cmd(`Digit${n}`)], other: [ctrlShift(`Digit${n}`)] },
    ]),
  ),
};

const BUILT_IN: readonly string[] = [...Object.values(ACTION_GROUPS).flat(), ...TAB_ACTIONS];

/** What an id in the keymap stands for, or `null` for one this version does not know. */
export function actionOf(id: string): ShortcutAction | null {
  if (id.startsWith('host:')) return { kind: 'host', id: id.slice(5) };
  if (id.startsWith('text:')) return { kind: 'text', id: id.slice(5) };
  const tab = /^select-tab-([1-9])$/.exec(id);
  if (tab) return { kind: 'select-tab', index: Number(tab[1]) - 1 };
  return BUILT_IN.includes(id) ? { kind: id as BoundAction } : null;
}

export function actionId(action: ShortcutAction): string {
  if (action.kind === 'select-tab') return `select-tab-${action.index + 1}`;
  if (action.kind === 'host' || action.kind === 'text') return `${action.kind}:${action.id}`;
  return action.kind;
}

/** The chords an action has out of the box. */
export function defaultChords(id: string, family: Family): readonly Chord[] {
  return DEFAULTS[id]?.[family] ?? [];
}

/** The chords an action has now: the user's, or the defaults. */
export function chordsFor(id: string, family: Family, overrides: Overrides): readonly Chord[] {
  return overrides[id] ?? defaultChords(id, family);
}

/** `ctrl+shift+KeyT`: a chord as one string, to compare and to look up. */
export function chordKey(chord: Chord): string {
  return `${chord.ctrl ? 'ctrl+' : ''}${chord.alt ? 'alt+' : ''}${chord.shift ? 'shift+' : ''}${chord.meta ? 'meta+' : ''}${chord.code}`;
}

/** The chord a key event is, as the keymap stores one. */
export function chordOf(event: KeyLike): Chord {
  return {
    ...(event.ctrlKey ? { ctrl: true } : {}),
    ...(event.altKey ? { alt: true } : {}),
    ...(event.shiftKey ? { shift: true } : {}),
    ...(event.metaKey ? { meta: true } : {}),
    code: event.code,
  };
}

const MODIFIER_CODES = /^(?:Control|Shift|Alt|Meta|OS)(?:Left|Right)$|^(?:CapsLock|Fn|AltGraph)$/;

/** Ctrl, Shift and their kind: keys that are held, not pressed. */
export function isModifierCode(code: string): boolean {
  return MODIFIER_CODES.test(code);
}

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

/** Every action that has keys: the user's first, so their choice wins over a default. */
function boundIds(overrides: Overrides): string[] {
  const own = Object.keys(overrides);
  return [...own, ...BUILT_IN.filter((id) => !(id in overrides))];
}

/** The action that has this chord, if any. `except` leaves one action out. */
export function holderOf(
  chord: Chord,
  family: Family,
  overrides: Overrides,
  except?: string,
): string | null {
  const key = chordKey(chord);
  for (const id of boundIds(overrides)) {
    if (id === except) continue;
    if (chordsFor(id, family, overrides).some((held) => chordKey(held) === key)) return id;
  }
  return null;
}

const plain = (chord: Chord): Chord => {
  const { keys: _keys, ...rest } = chord;
  return rest;
};

/** Same chords, in the same order: then an override says nothing the defaults do not. */
function sameChords(a: readonly Chord[], b: readonly Chord[]): boolean {
  return a.length === b.length && a.every((chord, i) => chordKey(chord) === chordKey(b[i]!));
}

function withChords(
  overrides: Overrides,
  family: Family,
  id: string,
  chords: readonly Chord[],
): Overrides {
  const next: Record<string, readonly Chord[]> = { ...overrides };
  const defaults = defaultChords(id, family);
  // Back at the defaults, or a host or text with no key left: no entry needed.
  if (sameChords(chords, defaults) && (defaults.length > 0 || !(id in DEFAULTS))) delete next[id];
  else next[id] = chords.map(plain);
  return next;
}

/** Takes a chord away from an action. */
export function unbind(overrides: Overrides, family: Family, id: string, chord: Chord): Overrides {
  const key = chordKey(chord);
  const left = chordsFor(id, family, overrides).filter((held) => chordKey(held) !== key);
  return withChords(overrides, family, id, left);
}

/**
 * Gives an action a chord. Whatever had the chord before loses it, so one key
 * combination never means two things.
 */
export function bind(overrides: Overrides, family: Family, id: string, chord: Chord): Overrides {
  let next = overrides;
  for (let holder = holderOf(chord, family, next, id); holder;) {
    next = unbind(next, family, holder, chord);
    holder = holderOf(chord, family, next, id);
  }
  const key = chordKey(chord);
  const held = chordsFor(id, family, next);
  if (held.some((one) => chordKey(one) === key)) return next;
  return withChords(next, family, id, [...held, plain(chord)]);
}

/**
 * An action back to its defaults. A default another action has taken since
 * stays with that action.
 */
export function resetAction(overrides: Overrides, family: Family, id: string): Overrides {
  const next: Record<string, readonly Chord[]> = { ...overrides };
  delete next[id];
  const free = defaultChords(id, family).filter((chord) => !holderOf(chord, family, next, id));
  return withChords(next, family, id, free);
}

/** Drops what belongs to hosts and texts that no longer exist. */
export function pruneOverrides(overrides: Overrides, known: (id: string) => boolean): Overrides {
  return Object.fromEntries(
    Object.entries(overrides).filter(
      ([id]) => !(id.startsWith('host:') || id.startsWith('text:')) || known(id),
    ),
  );
}

/**
 * Why a chord may be a bad idea; the settings say so before it is taken.
 *
 * - `typing` — no Ctrl, Alt or ⌘: the key could no longer be typed
 * - `terminal` — the program in the terminal uses it (Ctrl+C, Alt+B, ⌃A)
 * - `altgr` — Ctrl+Alt is AltGr on Windows: it types `{`, `@`, `\` on many layouts
 * - `option` — ⌥ types special characters on a Mac
 * - `system` — the system keeps it for itself (⌘Q, Alt+F4)
 */
export type ChordWarning = 'typing' | 'terminal' | 'altgr' | 'option' | 'system';

export function chordWarning(chord: Chord, family: Family): ChordWarning | null {
  const functionKey = /^F\d{1,2}$/.test(chord.code);
  if (!chord.ctrl && !chord.alt && !chord.meta) return functionKey ? null : 'typing';
  if (family === 'mac') {
    if (chord.meta) {
      const reserved = ['KeyQ', 'KeyH', 'KeyM', 'Tab', 'Space', 'Backquote'];
      return !chord.ctrl && reserved.includes(chord.code) ? 'system' : null;
    }
    if (chord.ctrl) return chord.shift || chord.alt || functionKey ? null : 'terminal';
    return functionKey ? null : 'option';
  }
  if (chord.ctrl && chord.alt) return chord.code === 'Delete' ? 'system' : 'altgr';
  if (chord.alt) {
    if (chord.code === 'F4' || chord.code === 'Tab' || chord.code === 'Space') return 'system';
    return functionKey ? null : 'terminal';
  }
  // Ctrl+Tab, Ctrl+PageDown and Ctrl+, mean nothing to a shell.
  if (chord.shift || functionKey) return null;
  return /^(?:Key[A-Z]|Bracket(?:Left|Right)|Backslash|Space|Digit\d)$/.test(chord.code)
    ? 'terminal'
    : null;
}

/** The app's shortcut for this key, or `null` when the key belongs to whatever has focus. */
export function shortcutFor(
  event: KeyLike,
  settings: Pick<Settings, 'ctrlCCopies'>,
  hasSelection: boolean,
  system: Platform,
  overrides: Overrides = {},
): ShortcutAction | null {
  if (composing(event) || isModifierCode(event.code)) return null;
  const family = familyOf(system);
  for (const id of boundIds(overrides)) {
    if (!chordsFor(id, family, overrides).some((chord) => matches(event, chord))) continue;
    // Only with text selected in the terminal; otherwise the key goes on to it.
    if (id === 'copy' && !hasSelection) return null;
    const action = actionOf(id);
    if (action) return action;
  }
  // Ctrl+C copies while text is selected, and is the interrupt otherwise.
  if (family === 'other' && settings.ctrlCCopies && hasSelection && matches(event, ctrl('KeyC')))
    return { kind: 'copy' };
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
  /** Names for more keys by code (`Space`, `Delete`, …), off the Mac. */
  more?: Readonly<Record<string, string>>;
  /** What the connected keyboard has printed on a key, if known: `Z` for KeyY on a German one. */
  printed?: (code: string) => string | undefined;
};

const KEY_SYMBOLS: Record<string, string> = {
  Comma: ',',
  Period: '.',
  Slash: '/',
  Semicolon: ';',
  Quote: "'",
  Backquote: '`',
  Minus: '-',
  Equal: '=',
  Backslash: '\\',
  IntlBackslash: '<',
  Tab: '⇥',
  BracketLeft: '[',
  BracketRight: ']',
  ArrowLeft: '←',
  ArrowRight: '→',
  ArrowUp: '↑',
  ArrowDown: '↓',
  PageUp: '⇞',
  PageDown: '⇟',
  Home: '↖',
  End: '↘',
  Enter: '↩',
  Backspace: '⌫',
  Delete: '⌦',
  Escape: '⎋',
  Space: '␣',
};

/** A key's name for a label: what is printed on it, or what the code says. */
export function keyName(code: string, mac: boolean, names: KeyNames): string {
  const digit = /^Digit(\d)$/.exec(code);
  if (digit) return digit[1]!;
  const printed = names.printed?.(code);
  if (printed) return printed.length === 1 ? printed.toUpperCase() : printed;
  const letter = /^Key(\w)$/.exec(code);
  if (letter) return letter[1]!;
  const numpad = /^Numpad(\d)$/.exec(code);
  if (numpad) return `Num ${numpad[1]}`;
  if (!mac) {
    if (code === 'Tab') return 'Tab';
    if (code === 'PageUp') return names.pageUp;
    if (code === 'PageDown') return names.pageDown;
    const more = names.more?.[code];
    if (more) return more;
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
    chord.meta ? 'Win' : null,
    key,
  ];
  return parts.filter(Boolean).join('+');
}

/** The label of an action's main chord, for tooltips: `⌘T`, `Strg+Umschalt+T`. Empty without one. */
export function shortcutLabel(
  id: string,
  system: Platform,
  names: KeyNames,
  overrides: Overrides = {},
): string {
  const chord = chordsFor(id, familyOf(system), overrides)[0];
  return chord ? chordLabel(chord, system, names) : '';
}

/** `⌘3`, `Strg+Umschalt+3`. */
export function tabLabel(
  number: number | string,
  system: Platform,
  names: KeyNames,
  overrides: Overrides = {},
): string {
  return shortcutLabel(`select-tab-${number}`, system, names, overrides);
}

/** Every chord of an action: `⇧⌘] · ⌃⇥ · ⌥⌘→`. */
export function allChordLabels(
  id: string,
  system: Platform,
  names: KeyNames,
  overrides: Overrides = {},
): string[] {
  return chordsFor(id, familyOf(system), overrides).map((chord) =>
    chordLabel(chord, system, names),
  );
}

/** The modifier for "hold while scrolling" and "hold while clicking": `⌘` or `Strg`. */
export function primaryModifierLabel(system: Platform, names: KeyNames): string {
  return system === 'macos' ? '⌘' : names.ctrl;
}

const ACCELERATOR_KEYS: Record<string, string> = {
  Comma: ',',
  Period: '.',
  Slash: '/',
  Semicolon: ';',
  Minus: '-',
  Equal: '=',
  BracketLeft: '[',
  BracketRight: ']',
  Backslash: '\\',
  Backquote: '`',
  Space: 'Space',
  Enter: 'Enter',
  Tab: 'Tab',
  Backspace: 'Backspace',
  Delete: 'Delete',
  ArrowLeft: 'Left',
  ArrowRight: 'Right',
  ArrowUp: 'Up',
  ArrowDown: 'Down',
  PageUp: 'PageUp',
  PageDown: 'PageDown',
  Home: 'Home',
  End: 'End',
};

/**
 * An action's main chord as a menu accelerator for the macOS menu bar
 * (`Cmd+Shift+F`), or `undefined` when it has none the menu could show: the
 * page handles the key itself then. Only ⌘ chords go to the menu — WebKit
 * hands the page those first, see App.tsx.
 */
export function menuAccelerator(id: string, overrides: Overrides = {}): string | undefined {
  const chord = chordsFor(id, 'mac', overrides)[0];
  if (!chord?.meta) return undefined;
  const key =
    /^(?:Key|Digit)(\w)$/.exec(chord.code)?.[1] ??
    (/^F\d{1,2}$/.test(chord.code) ? chord.code : ACCELERATOR_KEYS[chord.code]);
  if (!key) return undefined;
  return `${chord.ctrl ? 'Ctrl+' : ''}${chord.alt ? 'Alt+' : ''}${chord.shift ? 'Shift+' : ''}Cmd+${key}`;
}
