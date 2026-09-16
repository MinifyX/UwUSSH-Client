# Design

Clean, bright, soft — with a wink. Same design system as
[UwUMail](https://github.com/MinifyX/UwUMail-Client), same cat, one confident
bubblegum pink. The terminal is the one place that stays a terminal.

## Color

Tokens come from UwUMail unchanged, including the `--uwu-*` naming, and live in
`apps/desktop/src/styles/tokens.css`. Components never use raw hex values.

| Token              | Light     | Dark      | Use                                            |
| ------------------ | --------- | --------- | ---------------------------------------------- |
| `--uwu-canvas`     | `#f8f4f6` | `#141016` | App background                                 |
| `--uwu-surface`    | `#ffffff` | `#1c171f` | Host tree, panels, cards                       |
| `--uwu-elevated`   | `#fcf8fa` | `#241e28` | Hover rows, popovers                           |
| `--uwu-ink`        | `#1c1420` | `#f8f2f6` | Primary text                                   |
| `--uwu-muted`      | `#716672` | `#b3a8b3` | Secondary text                                 |
| `--uwu-hairline`   | `#f2e8ee` | `#2c2430` | Dividers                                       |
| `--uwu-border`     | `#e9dde4` | `#3a3040` | Control borders                                |
| `--uwu-pink`       | `#ff4d8d` | `#ff7fac` | **Brand.** Status dots, selection, focus, logo |
| `--uwu-pink-solid` | `#e11d74` | `#ff7fac` | Filled buttons with text                       |
| `--uwu-pink-tint`  | `#ffe4ef` | `#3a1a2a` | Selected host, active row                      |

**Why two pinks?** White text on `#ff4d8d` reaches only 3.1:1. Filled buttons
therefore use `#e11d74` (4.5:1, WCAG AA). The brighter brand pink stays for
everything that is not small text on a pink fill.

Connection state uses semantic color, separate from the brand: reachable is
mint, unreachable is muted grey, and a failed host key is amber — never pink,
because pink means "selected" everywhere else.

## Terminal themes

New compared to UwUMail, and the one part that doesn't follow the UwU look. The
terminal gets its own token set mapping the 16 ANSI colors, because a terminal
that fights the program's own colors is a broken terminal.

- **Nyu** (default) — the 16 ANSI colors translated into the UwU palette, dark
  ground, pink cursor.
- **Classics** — Solarized, Gruvbox, Campbell, shipped as-is and not
  "improved".
- Font: **JetBrains Mono**, bundled. Size, line height, cursor shape, bell and
  scrollback are per terminal profile, assignable per host.

## Type

- **Manrope** (variable, bundled, no network) for the interface.
- Sizes: 12 caption · 13 meta · 14 body/list · 16 panel body · 18 section ·
  22 title. Weights 400, 500, 600 for titles and host names, 700 only for the
  wordmark.

## Shape and space

- Radius: 10px controls, 16px cards and panes, 999px pills and badges.
- Spacing on a 4px grid.
- Shadows only for floating layers: menus, the command palette, toasts.

## Layout

```
┌────────────┬──────────────────────────────────┬──────────┐
│ Sidebar    │ Tab bar                          │ Inspector│
│            ├──────────────────────────────────┤          │
│ Search     │                                  │ Host     │
│ ▸ Homelab  │      Terminal / SFTP             │ Forwards │
│   ● prox-1 │                                  │ Snippets │
│ ▸ Hetzner  │                                  │          │
└────────────┴──────────────────────────────────┴──────────┘
```

- Custom title bar, no OS chrome edge. The inspector folds away, because
  full-screen terminal has to be one keystroke out.
- A status dot and "last connected" sit in the host row itself.
- **Reconnect is a banner, never a modal.** A modal over a running terminal is
  a UX bug, not a safety feature.
- Keyboard-first: everything reachable without the mouse, visible focus rings,
  command palette on `Ctrl+K`.

## Nyu, the mascot

Nyu is the same cat as in UwUMail — the envelope is just a **terminal window**
now. Window chrome with three dots on top, ears poking out above it, and the
screen is the face: UwU eyes, `w` mouth, blush.

- **Sticker style**, unchanged. Plum outlines `#4B1D3F`, pink body `#FF6FA6`,
  light screen `#FFB8D3`, pastel props, a white die-cut edge. The colors are
  fixed artwork and stay the same in dark mode; the white edge keeps the
  outlines readable on dark backgrounds.
- **App icon.** Nyu slightly tilted on a pastel pink tile with two yellow
  sparkles — and where UwUMail has a heart, UwUSSH has the key she guards.
  Never on a saturated pink tile. Regenerate platform icons with
  `pnpm tauri icon ../../brand/uwussh-app-icon.svg` in `apps/desktop`.
- **Sources** in `brand/` (icon, symbol, mono symbol) and
  `apps/desktop/src/components/nyu/` (React).

**Scenes** (`NyuScene`, 320 × 220), for the empty states an SSH client actually
has:

| Scene           | When                                               |
| --------------- | -------------------------------------------------- |
| Welcome         | First start, no hosts yet                          |
| Import done     | After a PuTTY/KiTTY/Termius import, with the count |
| Vault asleep    | Vault locked — Nyu naps on the key                 |
| Nothing found   | Search with no matches                             |
| Connection lost | Reconnect banner, Nyu waiting with a cable         |
| All offline     | No host reachable                                  |
| No tunnels      | Port forwarding panel with nothing running         |
| Empty folder    | Empty SFTP directory                               |

**Motion.** Nyu blinks in scenes, twitches her ears on hover, and the cursor on
her screen blinks at terminal rhythm. Settings → Appearance → Animations
(System / On / Off) resolves to `<html data-motion="full|reduced">`; with
`reduced`, all animation collapses to 1 ms and Nyu holds still.

**Name.** Nyu only appears by name in the playful tone. The neutral tone keeps
the pictures and says "UwUSSH".

## Tone of voice

Playful by default: kaomoji, warm little jokes, soft animation. Settings → Tone
→ **Neutral** replaces the words, never the layout or the colors. Every string
lives in `locales/<lang>/neutral.json`, with the playful variant under the same
key in `playful.json`; missing playful keys fall back to neutral.

| Situation       | Neutral                      | Playful                                                        |
| --------------- | ---------------------------- | -------------------------------------------------------------- |
| No hosts        | No hosts yet                 | Pretty empty in here (・_・;) Let's go get your PuTTY sessions |
| Connected       | Connected to prox-1          | We're in! ✨                                                   |
| Import done     | Imported 47 hosts            | Collected 47 hosts (๑˃ᴗ˂)ﻭ                                     |
| Connection lost | Disconnected. Retrying in 5s | Whoops, gone (╥﹏╥) Trying again in 5s                         |
| Vault locked    | Vault locked                 | Nyu's watching your keys ᶻ 𝗓 𐰁                                 |

Rules for playful copy:

1. **Information first.** The joke never replaces what happened or what to do.
2. **Short.** One kaomoji at most, never in buttons that act on data.
3. **Kind.** Never mock the user; the app laughs at itself.
4. **Security is never playful.** A changed host key, a failed vault unlock, a
   request to forward your agent: no kaomoji, no Nyu, in _both_ tones. A sad
   face next to a possible man-in-the-middle warning destroys exactly what the
   warning is for.

Rule 4 is the one that's new compared to UwUMail, and it is not negotiable:

```
⚠  The host key for prox-1 changed.

  known   SHA256:nThbg6kX…UmcQ2p4   since 2026-03-14
  now     SHA256:7Pq1Zx0v…Kd9Lm3s

This can be a rebuilt server — or a man in the middle.
Connection blocked. Type prox-1 to override.
```

German strings follow the same rules and use "du".
