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
