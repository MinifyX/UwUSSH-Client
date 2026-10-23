import { invoke } from '@tauri-apps/api/core';

/**
 * Puts text on the system clipboard, through Rust (`copy_text`). The
 * webview's own clipboard says no without focus or a fresh click or key —
 * one `await` before the copy is enough to lose that — so it is only the
 * fallback, and the old `execCommand` after it.
 */
export async function copyText(text: string): Promise<void> {
  try {
    await invoke('copy_text', { text });
    return;
  } catch {
    // On to the webview's clipboard.
  }
  try {
    await navigator.clipboard.writeText(text);
    return;
  } catch {
    // On to the old way.
  }
  const focused = document.activeElement;
  const area = document.createElement('textarea');
  area.value = text;
  area.setAttribute('readonly', '');
  area.style.position = 'fixed';
  area.style.opacity = '0';
  document.body.append(area);
  area.select();
  const copied = document.execCommand('copy');
  area.remove();
  // The terminal (or field) that had the keyboard keeps it.
  if (focused instanceof HTMLElement) focused.focus();
  if (!copied) throw new Error('the clipboard said no');
}
