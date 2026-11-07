/**
 * Two tabs side by side. The split is a pair of tab ids on top of the tab
 * list, not a tab of its own: it shows while one of its tabs is in front,
 * stays when another tab or the overview is shown in between, and ends when
 * one of its tabs closes.
 *
 * Pure functions only, so `node --test` runs them as they are.
 */

export type Split = { a: string; b: string; vertical: boolean };

/** `active` next to `other`; `null` when there is nothing to put side by side. */
export function splitWith(active: string | null, other: string, vertical = false): Split | null {
  if (!active || active === other) return null;
  return { a: active, b: other, vertical };
}

/** Whether `split` is on screen with `activeId` in front. */
export function splitShown(split: Split | null, activeId: string | null): boolean {
  return split !== null && activeId !== null && (split.a === activeId || split.b === activeId);
}

/** The split once the open tabs are `ids`: it ends when one of its tabs is gone. */
export function pruneSplit(split: Split | null, ids: readonly string[]): Split | null {
  if (!split) return null;
  return ids.includes(split.a) && ids.includes(split.b) ? split : null;
}

/**
 * The keyboard's "split": a shown split ends; otherwise the tab in front goes
 * next to its neighbour (the next tab, or the previous one at the end).
 */
export function toggleSplit(
  split: Split | null,
  activeId: string | null,
  ids: readonly string[],
): Split | null {
  if (splitShown(split, activeId)) return null;
  const index = activeId ? ids.indexOf(activeId) : -1;
  if (index < 0) return split;
  const other = ids[index + 1] ?? ids[index - 1];
  return other ? splitWith(activeId, other, split?.vertical) : split;
}
