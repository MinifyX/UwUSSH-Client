/**
 * The shortcut table ({@link ./keymap}) for this system and in the app's
 * language: labels for tooltips and the settings.
 */

import { t } from './i18n';
import {
  allChordLabels,
  primaryModifierLabel,
  shortcutLabel,
  tabLabel,
  type BoundAction,
  type KeyNames,
} from './keymap';
import { platform } from './platform';

export function isMac(): boolean {
  return platform() === 'macos';
}

function keyNames(): KeyNames {
  return {
    ctrl: t('Strg'),
    shift: t('Umschalt'),
    alt: t('Alt'),
    pageUp: t('Bild auf'),
    pageDown: t('Bild ab'),
  };
}

/** `⌘T` on a Mac, `Strg+Umschalt+T` elsewhere. */
export function keysFor(action: BoundAction): string {
  return shortcutLabel(action, platform(), keyNames());
}

/** Every way to do it: `⇧⌘] · ⌃⇥ · ⌥⌘→`. */
export function allKeysFor(action: BoundAction): string {
  return allChordLabels(action, platform(), keyNames()).join(' · ');
}

/** The shortcut that shows tab `number`. */
export function keysForTab(number: number | string): string {
  return tabLabel(number, platform(), keyNames());
}

/** `⌘` or `Strg`, for "hold while scrolling or clicking". */
export function primaryModifier(): string {
  return primaryModifierLabel(platform(), keyNames());
}
