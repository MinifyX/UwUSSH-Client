/**
 * Tabs: one session each, as plain data. The terminals themselves live in
 * {@link TerminalDriver}s outside React; a tab only says what it is, how it is
 * doing and what to tell the user.
 */

import type { AssistTarget } from './assist';
import { t } from './i18n';
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

/** A live terminal: a local shell or an SSH session the assistant can type into. */
export function canAssist(tab: Tab): boolean {
  return (tab.kind === 'shell' || tab.kind === 'ssh') && tab.status === 'live';
}

/** Which system the assistant writes a command for: this computer, or the host. */
export function assistTargetOf(tab: Tab): AssistTarget {
  return tab.kind === 'ssh' ? { kind: 'host', os: tab.host.os } : { kind: 'local' };
}
