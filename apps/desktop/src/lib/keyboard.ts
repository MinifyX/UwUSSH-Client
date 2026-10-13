/**
 * The keyboard that is connected: its layout, for the picture in the shortcut
 * settings and for what a shortcut's label says (`Strg+Umschalt+Z` is KeyY on
 * a German keyboard).
 *
 * Three sources, best first: the webview (`navigator.keyboard`, WebView2
 * only), the system through the backend (`keyboard_layout`), and a guess from
 * the preferred languages. The settings can name a layout instead.
 */

import { invoke } from '@tauri-apps/api/core';
import { useSyncExternalStore } from 'react';
import {
  closestLayout,
  layoutFor,
  layoutForLanguages,
  printedKeys,
  type Layout,
} from './keyboardLayouts';
import { getSettings, subscribeSettings } from './settings';

export type Keyboard = {
  layout: Layout;
  /** What is printed on each key, by code. */
  printed: Readonly<Record<string, string>>;
  /** Read from the system, not guessed and not chosen. */
  detected: boolean;
};

type LayoutMap = { entries(): Iterable<[string, string]> };
type WithKeyboard = Navigator & { keyboard?: { getLayoutMap?: () => Promise<LayoutMap> } };

function guessed(): Keyboard {
  const layout = layoutForLanguages(navigator.languages ?? [navigator.language ?? 'en']);
  return { layout, printed: printedKeys(layout), detected: false };
}

let system: Keyboard = guessed();
let current: Keyboard = resolve();
const listeners = new Set<() => void>();

function resolve(): Keyboard {
  const chosen = layoutFor(getSettings().keyboardLayout);
  if (getSettings().keyboardLayout !== 'auto' && chosen) {
    return { layout: chosen, printed: printedKeys(chosen), detected: false };
  }
  return system;
}

function publish() {
  current = resolve();
  for (const listener of listeners) listener();
}

async function detect(): Promise<Keyboard> {
  try {
    const map = await (navigator as WithKeyboard).keyboard?.getLayoutMap?.();
    if (map) {
      const printed: Record<string, string> = {};
      for (const [code, key] of map.entries()) if (key.trim()) printed[code] = key;
      if (Object.keys(printed).length > 20) {
        const layout = closestLayout(printed);
        return { layout, printed: { ...printedKeys(layout), ...printed }, detected: true };
      }
    }
  } catch {
    // Not allowed here: ask the backend.
  }
  try {
    const layout = layoutFor(await invoke<string | null>('keyboard_layout'));
    if (layout) return { layout, printed: printedKeys(layout), detected: true };
  } catch {
    // No answer from the system: the guess stands.
  }
  return guessed();
}

/** Asks the system again: on start, and when the shortcut settings open (a keyboard may have been plugged in). */
export async function refreshKeyboard(): Promise<void> {
  system = await detect();
  publish();
}

subscribeSettings(() => {
  const next = resolve();
  if (next.layout !== current.layout || next.printed !== current.printed) publish();
});

export function getKeyboard(): Keyboard {
  return current;
}

export function useKeyboard(): Keyboard {
  return useSyncExternalStore((listener) => {
    listeners.add(listener);
    return () => listeners.delete(listener);
  }, getKeyboard);
}
