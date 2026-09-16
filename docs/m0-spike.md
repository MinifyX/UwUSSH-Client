# M0 — the throughput spike

The one measurement that could invalidate the whole design, done before any of
the nice parts get built.

## The question

Tauri's classic `emit` events serialise to JSON. A terminal under load pushes
several MB/s. Does the Rust→WebView boundary carry that, or does UwUSSH need a
local WebSocket instead?

Everything else in the concept assumes the answer is "yes, with framing". If it
is "no", the session architecture changes, and it is much cheaper to find that
out now than after the vault is built on top of it.

## Prerequisites

Not installed on Lorin's machine as of 2026-09-16 — this is the list that has
to happen first:

- **Rust stable** via [rustup](https://rustup.rs)
- **Visual Studio C++ Build Tools** with the Windows 10/11 SDK (Tauri links
  against MSVC)
- **WebView2 runtime** — already present on Windows 11
- **Node 24** (the machine has 22) and **pnpm 11** via `corepack enable`

## Running it

```bash
pnpm install
pnpm tauri dev
```

The window opens on a local shell — PowerShell on Windows. Press
**Lasttest starten**, which writes a shell-appropriate flood command
(`yes …` on Unix, a `while ($true)` loop on Windows) into the PTY, and watch the
readout in the toolbar.

## Reading the numbers

| Reading            | What it tells you                                                                                                             |
| ------------------ | ----------------------------------------------------------------------------------------------------------------------------- |
| **MiB/s**          | Throughput actually crossing the boundary.                                                                                    |
| **Frames/s**       | Crossings per second. With an 8 ms window the ceiling is ~125; sitting at the ceiling means the batcher is working.           |
| **ø KiB**          | Mean frame size. Small frames under load mean coalescing is _not_ working — that is the failure mode to look for.             |
| **Stalls**         | How often the PTY reader waited for the UI. This is the real ceiling.                                                         |
| **webgl / canvas** | Which renderer won. On `canvas` you are measuring xterm.js, not the IPC path, and the result means nothing for this question. |

**Verdict:** if a sustained flood holds a comfortable margin above what a real
session produces, with frames near the size cap and stalls only under
deliberate flooding, the IPC channel is fine and M1 can start. If frames stay
small, or stalls appear at modest rates, the fallback is the local WebSocket on
`127.0.0.1` with a one-time token.

Either way, only `ChannelSink` in `apps/desktop/src-tauri/src/lib.rs` and
`spawnLocalSession` in `apps/desktop/src/lib/session.ts` change. The reading,
coalescing and backpressure in `uwussh-core` are transport-agnostic, which was
the point of splitting them out.

## One correction to the concept

`KONZEPT.md` originally said frames would be **dropped** on overflow, with
xterm.js keeping the scrollback truth. Building it made clear that this is
wrong: dropping bytes before xterm.js cuts escape sequences in half, so the
buffer ends up holding corrupted output rather than truth.

What the implementation does instead is what a real terminal does — apply
backpressure. The bounded channel makes the reader wait, the PTY buffer fills,
and the program slows down, exactly as it already does when you `cat` a large
file into a slow terminal. Nothing is dropped, and the stall counter measures
how often that happened. That counter is a better answer to the M0 question
than a drop counter would have been anyway: it measures the ceiling instead of
hiding it.

## What is deliberately not here

- **SSH.** A local PTY saturates the boundary harder than SSH ever will, and it
  needs no test host. `russh` lands in M1.
- **Persistence.** No SQLite yet; nothing here is worth keeping.
- **Pinned versions for the volatile crates.** `russh`, `rusqlite` and friends
  are added with `cargo add` when their milestone arrives, so a guessed version
  cannot break the workspace build today.
