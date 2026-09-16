# M0 — the throughput spike

The one measurement that could invalidate the whole design, done before any of
the nice parts get built.

## Verdict

**The IPC channel stays. The WebSocket fallback is dropped. End-to-end flow
control is mandatory — not for speed, but because without it output is lost.**

Measured on 2026-09-16, Windows 11, release build, WebView2 153, WebGL renderer,
143×31 terminal:

| Scenario                      | Result                       | Rounds (MiB/s)     | Screen behind at worst | Longest UI frame |
| ----------------------------- | ---------------------------- | ------------------ | ---------------------- | ---------------- |
| Direct, **no** flow control   | **lost 8.7 / 8.9 MiB** of 64 | —                  | 56.5 / 56.8 MiB        | 42 / 43 ms       |
| Direct, with flow control     | complete                     | 43.0 / 46.1 / 41.5 | 0.5 MiB                | 6 ms             |
| ConPTY (`type`), flow control | complete                     | 1.7 / 1.8 / 1.7    | 0 MiB                  | 6–7 ms           |

"Direct" is the path SSH bytes will take: generated in Rust, no process, no
ConPTY. The raw reports are the `m0-report*.json` files the app writes.

What the numbers say:

1. **The IPC channel is not the bottleneck.** Without flow control, xterm.js
   ended up with more than 50 MB of unparsed data queued — the channel delivered
   far faster than the terminal could parse. With flow control, end-to-end
   throughput (41.5–46.1 MiB/s) sits right at xterm.js' own parse rate in the
   uncontrolled run (55 MiB parsed in ~1.15 s). A WebSocket would move bytes
   into the same parser at the same speed, so it cannot help.
2. **Without flow control, output disappears.** xterm.js refuses writes once
   more than 50 MB are pending. From the shipped 5.5.0 bundle:

   ```js
   if (this._pendingData > 5e7)
     throw new Error('write data discarded, use flow control to avoid losing data');
   ```

   Both uncontrolled rounds lost about 9 MiB of a 64 MiB flood. The UI stayed
   smooth while it happened, so nobody would even have noticed.

3. **With flow control, the UI never stutters.** Not one frame above 50 ms in
   any controlled run; the longest gap was 7 ms. The screen was never more than
   half a mebibyte behind, which is what keeps `Ctrl+C` during a flood
   responsive.
4. **Local shells on Windows are capped by ConPTY at ~1.7 MiB/s** — about 25×
   below the direct path. The engine never paused and the reader never stalled
   in that scenario, so nothing downstream was waiting: the pseudo console is
   the ceiling. UwUSSH cannot change that, and SSH sessions do not go through
   ConPTY at all.

41–46 MiB/s is roughly 350 Mbit/s of terminal output — beyond anything a person
reads, and in the same place as every other xterm.js-based terminal, since the
parser is the limit. What mattered was that a flood neither freezes the window
nor loses data, and with flow control it does neither.

## The question

Tauri's classic `emit` events serialise to JSON. A terminal under load pushes
several MB/s. Does the Rust→WebView boundary carry that, or does UwUSSH need a
local WebSocket instead?

Everything else in the concept assumed the answer was "yes, with framing". If it
were "no", the session architecture would change, and it is much cheaper to find
that out before the vault is built on top of it.

## How it is measured

Each scenario pushes the same 64 MiB of deterministic, coloured log output —
timestamps, levels, request lines, a wrapping line every 31 — because plain
ASCII is the cheapest thing a terminal can parse and would flatter every number.

The clock stops when **xterm.js has parsed the last byte**, taken from its write
callback — not when Rust sent it. A run only counts as complete if every byte
was parsed; incomplete runs report what was lost instead of a speed.

Alongside the bytes, the gaps between animation frames are recorded: at 60 Hz a
healthy gap is ~17 ms, above 50 ms is visible stutter, above 250 ms a freeze.

### Running it

```powershell
pnpm tauri build --no-bundle
$env:UWUSSH_M0_AUTORUN = "1"
$env:UWUSSH_M0_REPORT = "$env:TEMP\m0-report.json"
.\target\release\uwussh-desktop.exe
```

The window runs all three scenarios, writes the report and closes itself. Keep
it visible while it runs — WebView2 throttles rendering for minimised or covered
windows, and the numbers stop meaning anything. Without the variables, the
button **M0-Messung starten** runs the same suite and shows the table in the
app.

Measure release builds. A debug build with the Vite dev server measures the dev
tooling as much as the app.

## Two corrections this spike forced

### Bytes are never dropped

`KONZEPT.md` v0.1 said frames would be dropped on overflow, with xterm.js
keeping the scrollback truth. That was wrong before a single number existed:
dropping bytes before xterm.js cuts escape sequences in half, so the buffer ends
up holding corrupted output rather than truth. The engine applies backpressure
instead, the way a real terminal does.

### Backpressure has to reach the webview

The first scaffold had backpressure only between the PTY reader and the batcher,
and its documentation claimed that was enough. It was not: `Channel::send`
returns as soon as Tauri has queued a frame, long before the webview has parsed
it. Every Rust-side counter would have looked healthy while xterm.js drowned —
and, as the measurement showed, started throwing data away.

The fix is the scheme VS Code's terminal uses. The renderer acknowledges bytes
from xterm.js' write callback; the batcher pauses once 512 KiB are
unacknowledged and resumes at 128 KiB. Acknowledgements go out every 64 KiB, and
that size must stay below the resume threshold, or a paused stream would wait
forever on a remainder that is never acknowledged — `uwussh-core` checks this at
compile time. The measured peak of outstanding bytes was 513 KiB in every
controlled run, which is the watermark doing exactly its job.

## What is deliberately not here

- **SSH.** The synthetic source stands in for it and needs no test host.
  `russh` is next, still within M0, onto the path measured here.
- **Session lifetime.** A reloaded webview stops acknowledging, and a
  flow-controlled session then pauses forever. Nothing reloads during a
  measurement; M1 ties sessions to the window that opened them.
- **Pinned versions for the volatile crates.** `russh`, `rusqlite` and friends
  are added with `cargo add` when their milestone arrives, so a guessed version
  cannot break the workspace build.
