// Kept free of imports so `node --test` can load it.

/** Next/previous tab: the overview (`null`) is the first stop in the round. */
export function stepTab(
  ids: readonly string[],
  current: string | null,
  step: 1 | -1,
): string | null {
  const round = [null, ...ids];
  const at = current === null ? 0 : round.indexOf(current);
  return round[(Math.max(at, 0) + step + round.length) % round.length]!;
}

/** `list` with the item `id` moved to position `to` (clamped); unchanged if `id` is missing. */
export function moveTo<T extends { id: string }>(list: readonly T[], id: string, to: number): T[] {
  const from = list.findIndex((item) => item.id === id);
  if (from < 0) return [...list];
  const next = list.filter((item) => item.id !== id);
  next.splice(Math.max(0, Math.min(to, next.length)), 0, list[from]!);
  return next;
}

/**
 * Where a dragged tab belongs: after every other tab whose middle lies left of
 * the pointer. Only the other tabs count, so the order cannot flip back and forth.
 */
export function dropIndex(otherMiddles: readonly number[], x: number): number {
  return otherMiddles.filter((middle) => middle < x).length;
}
