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

// ── What the sandbox needs from the page ────────────────────────────────────
//
// In the store build the App Sandbox lets the app open only what the person
// picked in a panel; Rust keeps each pick across restarts with a
// security-scoped bookmark (src-tauri/src/sandbox_access.rs). The panels work
// in every build, so the page can offer them everywhere and must in the store.

/** A folder the person added to the file browser's local places. */
export type PickedPlace = { label: string; path: string; kind: 'picked' };

/**
 * Let the app into `~/.ssh`: a folder panel that starts there. Afterwards
 * `available_imports` lists `openssh` (store build) and every host whose key
 * file lies in there connects. The folder as shown (`~/.ssh`), or `null` when
 * the panel was cancelled.
 */
export function grantSshFolder(): Promise<string | null> {
  return invoke<string | null>('grant_ssh_folder');
}

/** The host form's key file, picked in a panel (`~/…` when in the home folder). */
export function pickKeyPath(): Promise<string | null> {
  return invoke<string | null>('pick_key_path');
}

/** A folder for the file browser's local side; listed by `local_places` in the store build. */
export function pickLocalFolder(): Promise<PickedPlace | null> {
  return invoke<PickedPlace | null>('local_pick_folder');
}

/** Takes a picked folder off the local places again (store build). */
export function forgetLocalFolder(path: string): Promise<void> {
  return invoke('local_forget_folder', { path });
}
