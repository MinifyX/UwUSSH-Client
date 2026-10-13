/**
 * The user's own keys ({@link ./keymap}'s overrides) and the texts they can
 * put on a key, as the vault has them.
 *
 * They live in the store on the Rust side, sealed, and travel with the sync —
 * one set for the Mac, one for Windows and Linux. A text's wording never comes
 * back to the page: it is written once, and typing it is the backend's job.
 * While the vault is locked nothing can be read, and the defaults apply.
 */

import { invoke } from '@tauri-apps/api/core';
import { useSyncExternalStore } from 'react';
import { familyOf, type Chord, type Family, type Overrides } from './keymap';
import { platform } from './platform';
import type { SessionId } from './session';

/** A text a key can type. `enter` sends it off with a line break. */
export type KeymapText = { id: string; label: string; enter: boolean; hasText: boolean };

export type TextChange = { kind: 'keep' } | { kind: 'set'; value: string } | { kind: 'forget' };
export type TextDraft = { id: string; label: string; enter: boolean; text: TextChange };

export type Keymap = { bindings: Overrides; texts: readonly KeymapText[] };

type View = { bindings: Record<string, Chord[]>; texts: KeymapText[] };

const EMPTY: Keymap = { bindings: {}, texts: [] };

let current: Keymap = EMPTY;
const listeners = new Set<() => void>();

export function keymapFamily(): Family {
  return familyOf(platform());
}

function set(view: View) {
  current = { bindings: view.bindings ?? {}, texts: view.texts ?? [] };
  for (const listener of listeners) listener();
}

export function getKeymap(): Keymap {
  return current;
}

export function subscribeKeymap(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function useKeymap(): Keymap {
  return useSyncExternalStore(subscribeKeymap, getKeymap);
}

/** Reads the keymap from the store: on start, after unlocking, after a sync. */
export async function loadKeymap(): Promise<void> {
  try {
    set(await invoke<View>('keymap_get', { family: keymapFamily() }));
  } catch {
    // Locked, or no store yet: the defaults apply until the next try.
  }
}

/** One save at a time, in the order asked: a late older one must not win. */
let queue: Promise<unknown> = Promise.resolve();

/**
 * Stores the keys and texts. `texts` is the whole list — one that is missing
 * is deleted, with its wording. The page shows the change at once; a save that
 * fails puts back what the store has.
 */
export function saveKeymap(bindings: Overrides, texts: readonly TextDraft[]): Promise<void> {
  current = {
    bindings,
    texts: texts.map((draft) => ({
      id: draft.id,
      label: draft.label,
      enter: draft.enter,
      hasText:
        draft.text.kind === 'set' ||
        (draft.text.kind === 'keep' &&
          Boolean(current.texts.find((text) => text.id === draft.id)?.hasText)),
    })),
  };
  for (const listener of listeners) listener();
  const family = keymapFamily();
  const next = queue.then(async () => {
    try {
      set(await invoke<View>('keymap_save', { family, draft: { bindings, texts } }));
    } catch (error) {
      await loadKeymap();
      throw error;
    }
  });
  queue = next.catch(() => undefined);
  return next;
}

/** The texts as they are, for a save that only changes keys. */
export function keptTexts(texts: readonly KeymapText[] = current.texts): TextDraft[] {
  return texts.map(({ id, label, enter }) => ({ id, label, enter, text: { kind: 'keep' } }));
}

/** Types a text into a session. The backend reads it from the vault. */
export function sendKeymapText(id: SessionId, textId: string): Promise<void> {
  return invoke('keymap_send_text', { id, family: keymapFamily(), textId });
}
