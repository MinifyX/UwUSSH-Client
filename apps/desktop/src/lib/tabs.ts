/**
 * Tabs: one session each, as plain data. The terminals themselves live in
 * {@link TerminalDriver}s outside React; a tab only says what it is, how it is
 * doing and what to tell the user.
 */

import { t } from './i18n';
import type { Settings } from './settings';
import type { HostRecord } from './session';

export type TabKind =
  | { kind: 'shell' }
  | { kind: 'ssh'; host: HostRecord }
  | { kind: 'files'; host: HostRecord }
  | { kind: 'm0' };

/**
 * - `connecting` — spawning or logging in, possibly waiting for a dialog
 * - `live` — a session is attached
 * - `ended` — the session stopped on its own; the terminal stays readable
 * - `failed` — never got a session
 */
export type TabStatus = 'connecting' | 'live' | 'ended' | 'failed';

export type Notice = {
  tone: 'info' | 'error';
  text: string;
  action?: { label: string; run: () => void };
};

export type Tab = TabKind & {
  id: string;
  title: string;
  subtitle: string | null;
  /** 2 for the second open tab to the same host, and so on. */
  ordinal: number;
  status: TabStatus;
  notice: Notice | null;
  /** The terminal's cursor sits after a password prompt. */
  prompt: boolean;
  /** The terminal has a password it can type (the login's, or a stored one). */
  canTypePassword: boolean;
  /** Bumped to mount a file tab's browser anew, for a fresh connection. */
  reload?: number;
};

let counter = 0;

/** Tab ids double as connection-attempt names on the Rust side: plain ASCII. */
export function newTabId(): string {
  counter += 1;
  return `tab-${Date.now().toString(36)}-${counter}`;
}

/** What makes two tabs "the same thing": the host, or the kind for local tabs. */
export function sameKey(kind: TabKind): string {
  return kind.kind === 'ssh' || kind.kind === 'files' ? `${kind.kind}:${kind.host.id}` : kind.kind;
}

export function describe(kind: TabKind): { title: string; subtitle: string | null } {
  switch (kind.kind) {
    case 'ssh': {
      const { host } = kind;
      const port = host.port === 22 ? '' : `:${host.port}`;
      return { title: host.name, subtitle: `${host.username}@${host.address}${port}` };
    }
    case 'files': {
      const { host } = kind;
      return {
        title: host.name,
        subtitle: t('Dateien · {login}', { login: `${host.username}@${host.address}` }),
      };
    }
    case 'm0':
      return { title: t('Durchsatz-Messung'), subtitle: null };
    case 'shell':
      return { title: t('Lokale Shell'), subtitle: null };
  }
}

/** The smallest number no open tab of the same kind uses yet. */
export function nextOrdinal(tabs: Tab[], kind: TabKind): number {
  const key = sameKey(kind);
  const used = new Set(tabs.filter((tab) => sameKey(tab) === key).map((tab) => tab.ordinal));
  let ordinal = 1;
  while (used.has(ordinal)) ordinal += 1;
  return ordinal;
}

export function createTab(tabs: Tab[], kind: TabKind, id: string = newTabId()): Tab {
  return {
    ...kind,
    ...describe(kind),
    id,
    ordinal: nextOrdinal(tabs, kind),
    status: 'connecting',
    notice: null,
    prompt: false,
    canTypePassword: false,
  };
}

/** Which tab to show after closing `id`: the one to its right, else to its left. */
export function neighbourAfterClose(tabs: Tab[], id: string): string | null {
  const index = tabs.findIndex((tab) => tab.id === id);
  if (index < 0) return null;
  const rest = tabs.filter((tab) => tab.id !== id);
  return rest[Math.min(index, rest.length - 1)]?.id ?? null;
}

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
  | { kind: 'copy' };

type KeyLike = Pick<KeyboardEvent, 'key' | 'code' | 'ctrlKey' | 'shiftKey' | 'altKey' | 'metaKey'>;

/**
 * The app's own shortcuts. All of them need Ctrl and never Alt: on a German
 * keyboard AltGr is Ctrl+Alt, and AltGr+7 has to stay a `{`. Plain Ctrl+letter
 * belongs to the program in the terminal (Ctrl+W deletes a word in bash), so
 * tab shortcuts take Shift as well, like in Windows Terminal.
 */
export function shortcutFor(
  event: KeyLike,
  settings: Pick<Settings, 'ctrlCCopies'>,
  hasSelection: boolean,
): ShortcutAction | null {
  if (!event.ctrlKey || event.altKey || event.metaKey) return null;
  if (event.key === 'Tab' || event.code === 'PageDown' || event.code === 'PageUp') {
    const back = event.code === 'PageUp' || (event.key === 'Tab' && event.shiftKey);
    return { kind: back ? 'previous-tab' : 'next-tab' };
  }
  if (!event.shiftKey) {
    if (event.code === 'Comma') return { kind: 'settings' };
    if (event.code === 'KeyC' && settings.ctrlCCopies && hasSelection) return { kind: 'copy' };
    return null;
  }
  switch (event.code) {
    case 'KeyT':
      return { kind: 'new-shell' };
    case 'KeyW':
      return { kind: 'close-tab' };
    case 'KeyD':
      return { kind: 'duplicate-tab' };
    case 'KeyC':
      return { kind: 'copy' };
    case 'KeyP':
      return { kind: 'type-password' };
    case 'KeyF':
      return { kind: 'open-files' };
  }
  const digit = /^Digit([1-9])$/.exec(event.code);
  if (digit) return { kind: 'select-tab', index: Number(digit[1]) - 1 };
  return null;
}

/**
 * Keys that must reach the browser instead of the terminal, so the webview
 * pastes natively: xterm.js would otherwise turn Ctrl+V into ^V. Pasting this
 * way needs no clipboard permission, and xterm.js still wraps it in bracketed
 * paste when the program asks for that.
 */
export function isPasteKey(event: KeyLike, settings: Pick<Settings, 'ctrlVPastes'>): boolean {
  if (event.altKey || event.metaKey) return false;
  if (event.shiftKey && !event.ctrlKey && event.code === 'Insert') return true;
  if (!event.ctrlKey || event.code !== 'KeyV') return false;
  return event.shiftKey || settings.ctrlVPastes;
}
