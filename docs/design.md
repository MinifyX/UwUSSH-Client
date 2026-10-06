# Design

UwUSSH looks like every UwU app because it is built from the suite's design
package, [@uwusuite/design](https://github.com/MinifyX/UwUSuite-Design): its
tokens, UwU Sans and the font picker, light, dark and high contrast, the
motion rules, the icons (Lucide through `Icon` and `ICONS`), the components
(Button, IconButton, Dialog, Switch, Segmented, SettingRow, Hint, Tag, Card,
TitleBar, Wordmark, …), the macOS menu bar and Nyu. The rules for all of that
live there, in its `docs/` (color, typography, icons, components, window,
macos, motion, nyu, tone), and the way an app moves onto it in its
[docs/migration.md](https://github.com/MinifyX/UwUSuite-Design/blob/main/docs/migration.md).

This page is only about what is special about UwUSSH: the terminal is the one
place that stays a terminal.

## Where things are

- `apps/desktop/src/styles/index.css` imports Tailwind, the package's
  `tailwind.css` and `font-picker.css`, then the app's own sheets into
  Tailwind's `components` layer (a utility class always wins over them):
  `base.css` (form pieces and checkboxes), `shell.css` (body, tabs, toolbar,
  notices, the terminal panes and their overlays), `sidebar.css` (workspaces,
  groups, host rows, badges, the context menu, the password-login tooltip),
  `dialogs.css` (host form, host keys, import, vault, export, tunnels),
  `settings.css` (settings, sync, the command assistant, the setup wizard,
  the update hint) and `files.css` (the file browser).
- Theme, contrast and motion: Settings → Darstellung, through the package's
  `useAppearance()` (`lib/appearance.ts`); `/boot.js` (the package's
  `bootScript()`, a file because the CSP allows no inline script) puts them on
  `<html>` before the first paint. UwUSSH is dark until the person picks
  something.
- The interface font: Settings → Darstellung → Schrift, the package's choices
  and `applyUiFont()`. A stored font that is no longer offered falls back to
  UwU Sans. The terminal's font is its own setting.
- UwUKeygen (`apps/keygen`) uses the desktop's components and sheets; the
  installer (`apps/setup`) is the package's installer look: always light, the
  tile gradient, plum text, Nyu packing the box.

## What is UwUSSH's own

- **The terminal** sits on the package's stage (`--uwu-stage`, dark in both
  themes), and xterm.js' background and foreground are the stage colours. The
  16 ANSI colours (`NYU_THEME` in `lib/driver.ts`) are UwUSSH's: a terminal
  that fights the program's own colours is a broken terminal. Overlays over a
  pane (connecting, failed) use `stage-overlay`.
- **Tabs** sit above the terminal, one per session: an icon for the system
  with a state dot (connecting pulses pink, online mint, ended grey), the
  active tab has the pink top inset, and a second tab to the same host gets a
  small number. The tab bar is the app's, built from tokens.
- **Reconnect is a banner, never a modal.** A modal over a running terminal is
  a UX bug, not a safety feature.
- **The sidebar** has two workspaces, Privat and Business (renamable), groups
  that fold, and hosts and groups that move by dragging (pointer events, not
  HTML5 drag and drop, which Tauri's file drop swallows on Windows).
- **System icons** (`OsIcon`): a small rounded Nyu sticker per system —
  Ubuntu, Debian, Fedora, Arch, Windows, macOS, MikroTik, Proxmox and more —
  in its brand colour with two little cat ears. Lucide has no system logos and
  the suite uses no company logos as icons; these only evoke the system, so
  they stay in the app.
- **Nyu** is the package's `terminal` shell. The scenes (`components/nyu/scenes.tsx`,
  the installer's and UwUKeygen's laser pad, where Nyu chases the cursor while
  the key's randomness is collected) are UwUSSH's.

## macOS

The system draws the title bar; the gear and the app's actions live in the
menu bar (`lib/macMenu.ts`, the package's `setMacMenu` skeleton): Einstellungen
(⌘,), Neue lokale Shell (⌘T), Neuer Host (⌘N), Tab duplizieren (⌘D), Dateien
öffnen (⇧⌘F), and a Sitzung menu with Passwort eintippen (⇧⌘P), Befehl aus
Worten (⌘K) and Tunnel.

One difference to the package: ⌘W closes the **tab** in front, as in
Terminal.app, and only with no tab left does it hide the window. "Fenster
schließen" is ⇧⌘W. Hidden, UwUSSH keeps its connections and tunnels and stays
in the Dock; the Dock icon brings the window back. ⌘Q, the Dock and logging out
ask first while connections are open (`uwu-macos`, `onMacQuit`).

## App icons

`brand/` has UwUSSH's and UwUKeygen's app, taskbar, small and symbol icons.
`pnpm exec uwu-icons --brand ../../brand --name uwussh` in `apps/desktop` (and
`--name uwukeygen` in `apps/keygen`) regenerates every platform file, the Dock
icon in Apple's grid.

## Security is never playful

The suite's tone rules apply (playful by default, information first, one
kaomoji at most). UwUSSH adds one that is not negotiable: a changed host key, a
failed vault unlock, a request to forward your agent get no kaomoji and no Nyu,
in both tones, and the warning dialog's focus sits on the safe choice. A sad
face next to a possible man-in-the-middle warning destroys exactly what the
warning is for.
