/**
 * The settings, kept in the page's own storage.
 *
 * Only preferences live here — how things look and behave. Nothing about a
 * host and no secret: those are in the store and the vault, on the Rust side.
 * A setting that Rust needs (the update channel) is handed over on start and
 * on every change.
 */

import { useSyncExternalStore } from 'react';
import pkg from '../../package.json';
import type { Workspace } from './session';

export type ThemeSetting = 'system' | 'light' | 'dark';
/** Animations: follow the system's reduced-motion setting, or override it. */
export type MotionSetting = 'system' | 'on' | 'off';
export type CursorStyle = 'block' | 'bar' | 'underline';
/** Beta gets pre-releases (tags like v0.1.0-beta.1) before everyone else. */
export type UpdateChannel = 'stable' | 'beta';

/** The terminal palette's colours a highlight can use. */
export type HighlightColor = 'red' | 'yellow' | 'green' | 'blue' | 'magenta' | 'cyan';

export type HighlightRule = {
  id: string;
  /** A word, or a regular expression when `regex` is set. */
  pattern: string;
  color: HighlightColor;
  regex: boolean;
  caseSensitive: boolean;
};

export type HighlightSettings = {
  enabled: boolean;
  errors: boolean;
  warnings: boolean;
  success: boolean;
  /** IP addresses and URLs. */
  network: boolean;
  custom: HighlightRule[];
};

export type Settings = {
  theme: ThemeSetting;
  motion: MotionSetting;
  fontSize: number;
  cursorStyle: CursorStyle;
  cursorBlink: boolean;
  scrollback: number;
  /** Ctrl+V pastes, like in Windows Terminal. Off sends ^V to the remote side. */
  ctrlVPastes: boolean;
  /** Ctrl+C copies while text is selected, and sends ^C otherwise. */
  ctrlCCopies: boolean;
  openShellOnStart: boolean;
  confirmCloseWithSessions: boolean;
  updateChannel: UpdateChannel;
  highlight: HighlightSettings;
  /** Offer to type the terminal's password when a prompt asks for one. */
  passwordHelper: boolean;
  /** Private and business hosts apart, like UwUMail's workspaces. */
  workspaces: boolean;
  activeWorkspace: Workspace;
  workspaceNames: Record<Workspace, string>;
  /** Groups folded away in the host list, as `workspace/name`. */
  collapsedGroups: string[];
};

export const FONT_SIZES = [11, 12, 13, 14, 15, 16, 18, 20] as const;
export const FONT_SIZE_MIN = 8;
export const FONT_SIZE_MAX = 32;
export const SCROLLBACK_CHOICES = [1_000, 10_000, 50_000] as const;
export const HIGHLIGHT_COLORS: readonly HighlightColor[] = [
  'red',
  'yellow',
  'green',
  'blue',
  'magenta',
  'cyan',
];

export const DEFAULT_SETTINGS: Settings = {
  theme: 'dark',
  motion: 'system',
  fontSize: 13,
  cursorStyle: 'block',
  cursorBlink: true,
  scrollback: 10_000,
  ctrlVPastes: true,
  ctrlCCopies: true,
  openShellOnStart: true,
  confirmCloseWithSessions: true,
  // Someone who installed a beta wants the next beta too.
  updateChannel: pkg.version.includes('-') ? 'beta' : 'stable',
  highlight: {
    enabled: true,
    errors: true,
    warnings: true,
    success: true,
    network: true,
    custom: [],
  },
  passwordHelper: true,
  workspaces: true,
  activeWorkspace: 'private',
  workspaceNames: { private: '', business: '' },
  collapsedGroups: [],
};

const KEY = 'uwussh.settings';

function sanitizeRules(raw: unknown): HighlightRule[] {
  if (!Array.isArray(raw)) return [];
  return raw
    .filter((rule): rule is Record<string, unknown> => typeof rule === 'object' && rule !== null)
    .map((rule) => ({
      id: typeof rule.id === 'string' ? rule.id.slice(0, 40) : String(Math.random()).slice(2),
      pattern: typeof rule.pattern === 'string' ? rule.pattern.slice(0, 200) : '',
      color: HIGHLIGHT_COLORS.includes(rule.color as HighlightColor)
        ? (rule.color as HighlightColor)
        : 'magenta',
      regex: rule.regex === true,
      caseSensitive: rule.caseSensitive === true,
    }))
    .filter((rule) => rule.pattern.length > 0)
    .slice(0, 50);
}

/** Stored values are checked one by one; anything unexpected falls back to its default. */
export function sanitize(raw: unknown): Settings {
  const input = typeof raw === 'object' && raw !== null ? (raw as Record<string, unknown>) : {};
  const oneOf = <T>(value: unknown, allowed: readonly T[], fallback: T): T =>
    allowed.includes(value as T) ? (value as T) : fallback;
  const bool = (value: unknown, fallback: boolean) =>
    typeof value === 'boolean' ? value : fallback;
  const d = DEFAULT_SETTINGS;
  const highlight =
    typeof input.highlight === 'object' && input.highlight !== null
      ? (input.highlight as Record<string, unknown>)
      : {};
  const names =
    typeof input.workspaceNames === 'object' && input.workspaceNames !== null
      ? (input.workspaceNames as Record<string, unknown>)
      : {};
  const name = (value: unknown) => (typeof value === 'string' ? value.slice(0, 24) : '');
  const fontSize =
    typeof input.fontSize === 'number' && Number.isInteger(input.fontSize)
      ? Math.min(FONT_SIZE_MAX, Math.max(FONT_SIZE_MIN, input.fontSize))
      : d.fontSize;
  return {
    theme: oneOf(input.theme, ['system', 'light', 'dark'] as const, d.theme),
    motion: oneOf(input.motion, ['system', 'on', 'off'] as const, d.motion),
    fontSize,
    cursorStyle: oneOf(input.cursorStyle, ['block', 'bar', 'underline'] as const, d.cursorStyle),
    cursorBlink: bool(input.cursorBlink, d.cursorBlink),
    scrollback: oneOf(
      input.scrollback,
      SCROLLBACK_CHOICES,
      d.scrollback as (typeof SCROLLBACK_CHOICES)[number],
    ),
    ctrlVPastes: bool(input.ctrlVPastes, d.ctrlVPastes),
    ctrlCCopies: bool(input.ctrlCCopies, d.ctrlCCopies),
    openShellOnStart: bool(input.openShellOnStart, d.openShellOnStart),
    confirmCloseWithSessions: bool(input.confirmCloseWithSessions, d.confirmCloseWithSessions),
    updateChannel: oneOf(input.updateChannel, ['stable', 'beta'] as const, d.updateChannel),
    highlight: {
      enabled: bool(highlight.enabled, d.highlight.enabled),
      errors: bool(highlight.errors, d.highlight.errors),
      warnings: bool(highlight.warnings, d.highlight.warnings),
      success: bool(highlight.success, d.highlight.success),
      network: bool(highlight.network, d.highlight.network),
      custom: sanitizeRules(highlight.custom),
    },
    passwordHelper: bool(input.passwordHelper, d.passwordHelper),
    workspaces: bool(input.workspaces, d.workspaces),
    activeWorkspace: oneOf(
      input.activeWorkspace,
      ['private', 'business'] as const,
      d.activeWorkspace,
    ),
    workspaceNames: { private: name(names.private), business: name(names.business) },
    collapsedGroups: Array.isArray(input.collapsedGroups)
      ? input.collapsedGroups
          .filter((g): g is string => typeof g === 'string')
          .map((g) => g.slice(0, 120))
          .slice(0, 500)
      : [],
  };
}

function load(): Settings {
  try {
    const raw = window.localStorage.getItem(KEY);
    return sanitize(raw ? JSON.parse(raw) : {});
  } catch {
    return DEFAULT_SETTINGS;
  }
}

let current = load();
const listeners = new Set<() => void>();

export function getSettings(): Settings {
  return current;
}

export function updateSettings(patch: Partial<Settings>) {
  current = sanitize({ ...current, ...patch });
  try {
    window.localStorage.setItem(KEY, JSON.stringify(current));
  } catch {
    // Private storage can be unavailable; the change still holds for this run.
  }
  for (const listener of listeners) listener();
}

export function subscribeSettings(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function useSettings(): Settings {
  return useSyncExternalStore(subscribeSettings, getSettings);
}

/** "Privat" and "Business", or the names the user gave them. */
export function workspaceName(workspace: Workspace, settings: Settings): string {
  const own = settings.workspaceNames[workspace].trim();
  if (own) return own;
  return workspace === 'private' ? 'Privat' : 'Business';
}

const darkQuery = () => window.matchMedia('(prefers-color-scheme: dark)');
const reducedQuery = () => window.matchMedia('(prefers-reduced-motion: reduce)');

/** Whether animations should play right now, by setting and system. */
export function motionAllowed(): boolean {
  const { motion } = current;
  return motion === 'on' || (motion === 'system' && !reducedQuery().matches);
}

/** Puts theme and motion on <html>, now and whenever the setting or the system changes. */
export function applyAppearance() {
  const apply = () => {
    const { theme } = current;
    const dark = theme === 'dark' || (theme === 'system' && darkQuery().matches);
    document.documentElement.dataset.theme = dark ? 'dark' : 'light';
    if (motionAllowed()) delete document.documentElement.dataset.motion;
    else document.documentElement.dataset.motion = 'reduced';
  };
  apply();
  subscribeSettings(apply);
  darkQuery().addEventListener('change', apply);
  reducedQuery().addEventListener('change', apply);
}
