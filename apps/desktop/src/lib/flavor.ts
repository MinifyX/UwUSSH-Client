/**
 * Which build of UwUSSH this is, asked from Rust once before the first render
 * (main.tsx): every download from GitHub (`github`) or the Mac App Store
 * build (`app-store`, the `mas` Cargo feature). The store build updates
 * through the store and has no local shell: the App Sandbox would only let a
 * shell see the app's own container, which is no use as a terminal.
 */

import { invoke } from '@tauri-apps/api/core';

export type Flavor = 'github' | 'app-store';

let current: Flavor = 'github';

/** Asks Rust once; a page without Rust behind it (tests, previews) stays `github`. */
export async function loadFlavor(): Promise<void> {
  try {
    const answer = await invoke<string>('app_flavor');
    current = answer === 'app-store' ? 'app-store' : 'github';
  } catch {
    current = 'github';
  }
}

export function flavor(): Flavor {
  return current;
}

/** The Mac App Store build. */
export function isAppStore(): boolean {
  return current === 'app-store';
}

/** Whether a local shell can be opened: not in the Mac App Store build. */
export function localShellAvailable(): boolean {
  return current !== 'app-store';
}

/** Whether the app looks for and installs its own updates: not when a store does that. */
export function updatesAvailableInApp(): boolean {
  return current !== 'app-store';
}
