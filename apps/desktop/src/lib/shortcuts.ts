/**
 * The shortcut table ({@link ./keymap}) for this system and in the app's
 * language: labels for tooltips and the settings.
 */

import { detectPlatform, shortcutText, withShortcut } from '@uwusuite/design';
import { language, t } from './i18n';
import { getKeyboard } from './keyboard';
import {
  allChordLabels,
  chordLabel,
  primaryModifierLabel,
  shortcutLabel,
  tabLabel,
  type BoundAction,
  type Chord,
  type KeyNames,
} from './keymap';
import { getKeymap } from './keymapStore';
import { platform } from './platform';

export function isMac(): boolean {
  return platform() === 'macos';
}

/** The desktop platform for chrome decisions (@uwusuite/design): title bar or menu bar. */
export const desktop = detectPlatform();

/** A Tauri accelerator (`CmdOrCtrl+,`) as this platform writes it. */
export function keys(accelerator: string): string {
  return shortcutText(accelerator, desktop, language());
}

/** A tooltip with its shortcut: `Einstellungen (⌘,)`. */
export function withKeys(label: string, accelerator: string): string {
  return withShortcut(label, accelerator, desktop, language());
}

export function keyNames(): KeyNames {
  const printed = getKeyboard().printed;
  return {
    ctrl: t('Strg'),
    shift: t('Umschalt'),
    alt: t('Alt'),
    pageUp: t('Bild auf'),
    pageDown: t('Bild ab'),
    more: {
      Space: t('Leertaste'),
      Enter: t('Eingabe'),
      Backspace: t('Rücktaste'),
      Delete: t('Entf'),
      Insert: t('Einfg'),
      Home: t('Pos1'),
      End: t('Ende'),
      Escape: 'Esc',
    },
    printed: (code) => printed[code],
  };
}

/** `⌘T` on a Mac, `Strg+Umschalt+T` elsewhere; empty when the action has no key. */
export function keysFor(action: BoundAction): string {
  return shortcutLabel(action, platform(), keyNames(), getKeymap().bindings);
}

/** The same inside a sentence, where an action the user took the key from still needs a word. */
export function keysLabel(action: BoundAction): string {
  return keysFor(action) || t('kein Tastenkürzel');
}

/** Every way to do it: `⇧⌘] · ⌃⇥ · ⌥⌘→`. */
export function allKeysFor(action: string): string {
  return allChordLabels(action, platform(), keyNames(), getKeymap().bindings).join(' · ');
}

/** The shortcut that shows tab `number`. */
export function keysForTab(number: number | string): string {
  return tabLabel(number, platform(), keyNames(), getKeymap().bindings);
}

/** One chord as this system writes it. */
export function keysOf(chord: Chord): string {
  return chordLabel(chord, platform(), keyNames());
}

/** A tooltip with an action's key, when it has one: `Seitenleiste ausblenden (⌘B)`. */
export function withKeysFor(label: string, action: BoundAction): string {
  const keys = keysFor(action);
  return keys ? `${label} (${keys})` : label;
}

/** `⌘` or `Strg`, for "hold while scrolling or clicking". */
export function primaryModifier(): string {
  return primaryModifierLabel(platform(), keyNames());
}

/** What an action is called, in the settings and in the keyboard's picture. */
export function actionName(action: BoundAction): string {
  switch (action) {
    case 'new-shell':
      return t('Neue lokale Shell');
    case 'close-tab':
      return t('Tab schließen');
    case 'duplicate-tab':
      return t('Tab duplizieren');
    case 'next-tab':
      return t('Nächster Tab');
    case 'previous-tab':
      return t('Vorheriger Tab');
    case 'move-tab-left':
      return t('Tab nach links schieben');
    case 'move-tab-right':
      return t('Tab nach rechts schieben');
    case 'copy':
      return t('Kopieren');
    case 'select-all':
      return t('Alles auswählen');
    case 'clear-terminal':
      return t('Terminal leeren');
    case 'type-password':
      return t('Passwort eintippen');
    case 'assist':
      return t('Befehl aus Worten');
    case 'open-files':
      return t('Dateien öffnen');
    case 'font-bigger':
      return t('Schrift größer');
    case 'font-smaller':
      return t('Schrift kleiner');
    case 'font-reset':
      return t('Schriftgröße zurücksetzen');
    case 'reconnect':
      return t('Neu verbinden');
    case 'settings':
      return t('Einstellungen');
    case 'shortcuts':
      return t('Tastenkürzel anzeigen');
    case 'toggle-sidebar':
      return t('Seitenleiste ein- und ausblenden');
    case 'focus-search':
      return t('Hosts durchsuchen');
    case 'new-host':
      return t('Neuer Host');
    case 'tunnels':
      return t('Tunnel');
  }
}
