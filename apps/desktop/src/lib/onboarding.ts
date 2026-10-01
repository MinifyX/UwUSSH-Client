/**
 * The first-start setup: whether to show it, and the flag that says it ran.
 *
 * Only a truly fresh install sees it. Someone who already has hosts, a vault,
 * a sync connection or settings of their own has set the app up before — the
 * wizard would only be in the way, so it is marked done for them without
 * showing. Settings → Darstellung starts it again on request.
 *
 * The decision is a plain function of what the app found on start, so
 * `node --test` can run its tests; this file only imports types.
 */

import type { VaultStatus } from './session';
import type { Backend } from './sync';

const KEY = 'uwussh.onboarding';

/** What the app knows about this install when it starts. */
export type InstallSigns = {
  /** The wizard ran before: finished, or skipped. */
  done: boolean;
  /** Settings were in storage before this start. */
  settingsStored: boolean;
  /** Hosts in the store, `null` when they could not be read. */
  hosts: number | null;
  /** The vault, `null` when its state could not be read. */
  vault: VaultStatus | null;
  /** Where this device syncs, `null` when that could not be read. */
  sync: Backend | null;
};

/**
 * - `show` — a fresh install: the wizard opens
 * - `settled` — the app was set up before the wizard existed: never show it,
 *   and remember that
 * - `skip` — not now; either it ran before, or the app could not tell, and a
 *   wizard over someone's hosts is worse than none on a fresh install
 */
export type OnboardingDecision = 'show' | 'settled' | 'skip';

export function onboardingDecision(signs: InstallSigns): OnboardingDecision {
  if (signs.done) return 'skip';
  const used =
    signs.settingsStored ||
    (signs.hosts ?? 0) > 0 ||
    (signs.vault !== null && signs.vault !== 'absent') ||
    (signs.sync !== null && signs.sync !== 'none');
  if (used) return 'settled';
  if (signs.hosts === null || signs.vault === null) return 'skip';
  return 'show';
}

/** Whether the wizard ran on this device (the page's own storage, like the settings). */
export function onboardingDone(): boolean {
  try {
    return window.localStorage.getItem(KEY) !== null;
  } catch {
    // No storage, no wizard: it could not remember being done either.
    return true;
  }
}

export function markOnboardingDone() {
  try {
    window.localStorage.setItem(KEY, JSON.stringify({ doneMs: Date.now() }));
  } catch {
    // Private storage can be unavailable; then the wizard is simply gone for this run.
  }
}
