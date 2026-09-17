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

export type ThemeSetting = 'system' | 'light' | 'dark';
/** Animations: follow the system's reduced-motion setting, or override it. */
export type MotionSetting = 'system' | 'on' | 'off';
export type CursorStyle = 'block' | 'bar' | 'underline';
/** Beta gets pre-releases (tags like v0.1.0-beta.1) before everyone else. */
export type UpdateChannel = 'stable' | 'beta';

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
};

export const FONT_SIZES = [11, 12, 13, 14, 15, 16, 18, 20] as const;
export const SCROLLBACK_CHOICES = [1_000, 10_000, 50_000] as const;

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
};

const KEY = 'uwussh.settings';

/** Stored values are checked one by one; anything unexpected falls back to its default. */
export function sanitize(raw: unknown): Settings {
  const input = typeof raw === 'object' && raw !== null ? (raw as Record<string, unknown>) : {};
  const oneOf = <T>(value: unknown, allowed: readonly T[], fallback: T): T =>
    allowed.includes(value as T) ? (value as T) : fallback;
  const bool = (value: unknown, fallback: boolean) =>
    typeof value === 'boolean' ? value : fallback;
  const d = DEFAULT_SETTINGS;
  return {
    theme: oneOf(input.theme, ['system', 'light', 'dark'] as const, d.theme),
    motion: oneOf(input.motion, ['system', 'on', 'off'] as const, d.motion),
    fontSize: oneOf(input.fontSize, FONT_SIZES, d.fontSize as (typeof FONT_SIZES)[number]),
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

const darkQuery = () => window.matchMedia('(prefers-color-scheme: dark)');
const reducedQuery = () => window.matchMedia('(prefers-reduced-motion: reduce)');

/** Puts theme and motion on <html>, now and whenever the setting or the system changes. */
export function applyAppearance() {
  const apply = () => {
    const { theme, motion } = current;
    const dark = theme === 'dark' || (theme === 'system' && darkQuery().matches);
    document.documentElement.dataset.theme = dark ? 'dark' : 'light';
    const reduced = motion === 'off' || (motion === 'system' && reducedQuery().matches);
    if (reduced) document.documentElement.dataset.motion = 'reduced';
    else delete document.documentElement.dataset.motion;
  };
  apply();
  subscribeSettings(apply);
  darkQuery().addEventListener('change', apply);
  reducedQuery().addEventListener('change', apply);
}
