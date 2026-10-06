/**
 * UwUSSH's macOS menu bar: @uwusuite/design's skeleton (`macMenuSpec`, the
 * suite's menus in Apple's order), with one change for a tabbed app.
 *
 * ⌘W closes the tab in front, as in Terminal.app and every browser, and only
 * with no tab left does it hide the window (the app stays in the Dock). The
 * package's Ablage menu ends in AppKit's own "Fenster schließen" on ⌘W, which
 * would take the key before the page sees it; here it is the app's own entry
 * on ⇧⌘W instead, and "Tab schließen" has ⌘W. Everything else is the package's.
 *
 * Off macOS this does nothing: the window has its own title bar there.
 */

import type { MacMenuEntry, MacMenuOptions } from '@uwusuite/design/tauri';
import { macMenuSpec } from '@uwusuite/design/tauri';
import { Menu, Submenu } from '@tauri-apps/api/menu';
import { desktop } from './shortcuts';

type Options = MacMenuOptions & {
  /** "Tab schließen", on ⌘W: closes the tab in front, or hides the window when there is none. */
  closeTab: { text: string; action: () => void };
  /** "Fenster schließen", on ⇧⌘W. */
  closeWindow: { text: string; action: () => void };
};

/** Ablage's entries before the closing pair. */
function fileEntries(options: Options): MacMenuEntry[] {
  return [
    ...(options.file ?? []),
    ...(options.file?.length ? (['separator'] as const) : []),
    { text: options.closeTab.text, accelerator: 'CmdOrCtrl+W', action: options.closeTab.action },
    {
      text: options.closeWindow.text,
      accelerator: 'CmdOrCtrl+Shift+W',
      action: options.closeWindow.action,
    },
  ];
}

/**
 * The bar as data, as `setMacMenu` would build it, with the Ablage menu's
 * predefined "Fenster schließen" (⌘W) replaced by the app's two entries.
 */
export function sshMenuSpec(options: Options) {
  const spec = macMenuSpec({ ...options, file: fileEntries(options) });
  const file = spec[1];
  if (file) {
    // AppKit's own item, and the separator the package put before it.
    const predefined = (item: unknown, name: string) =>
      typeof item === 'object' && item !== null && 'item' in item && item.item === name;
    file.items = file.items.filter(
      (item, index, items) =>
        !predefined(item, 'CloseWindow') &&
        !(predefined(item, 'Separator') && predefined(items[index + 1], 'CloseWindow')),
    );
  }
  return spec;
}

/** The bar that is set now, and its submenus: closed once the next one is set. */
let shown: Array<{ close: () => Promise<void> }> = [];
/** One build at a time, in the order asked: a late older bar must not win. */
let queue: Promise<void> = Promise.resolve();

/** Sets the menu bar; call again when the language or an entry's state changes. */
export function setSshMacMenu(options: Options): Promise<void> {
  if (desktop !== 'mac') return Promise.resolve();
  const next = queue.then(() => build(options));
  queue = next.catch(() => undefined);
  return next;
}

async function build(options: Options): Promise<void> {
  const submenus = await Promise.all(
    sshMenuSpec(options).map(async (spec) => ({
      spec,
      submenu: await Submenu.new({ text: spec.text, items: spec.items }),
    })),
  );
  const menu = await Menu.new({ items: submenus.map(({ submenu }) => submenu) });
  await menu.setAsAppMenu();
  for (const { spec, submenu } of submenus) {
    if (spec.role === 'window') await submenu.setAsWindowsMenuForNSApp();
    if (spec.role === 'help') await submenu.setAsHelpMenuForNSApp();
  }
  // Every rebuild (a tab switch, the language) made new menu resources; the
  // old ones would pile up in the app's resource table.
  const old = shown;
  shown = [menu, ...submenus.map(({ submenu }) => submenu)];
  await Promise.all(old.map((resource) => resource.close().catch(() => undefined)));
}
