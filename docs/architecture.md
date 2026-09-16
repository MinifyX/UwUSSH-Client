# Architecture

How the pieces fit together. The German [KONZEPT.md](../KONZEPT.md) has the
long version.

## The stack

| Layer       | Choice                                  | Why                                                                                              |
| ----------- | --------------------------------------- | ------------------------------------------------------------------------------------------------ |
| Shell       | Tauri 2                                 | Same as UwUMail. ~12 MB instead of Electron's 150, WebView2 on Windows, mobile support built in. |
| UI          | React + TypeScript, Node 24, pnpm 11    | UwUMail's stack, so tokens, components and Nyu carry over.                                       |
| Terminal    | `xterm.js` + WebGL addon                | What VS Code and Termius use. Canvas fallback.                                                   |
| SSH         | `russh`, `russh-keys`, `russh-sftp`     | Pure Rust, async, no libssh2 FFI pain.                                                           |
| Local shell | `portable-pty`                          | ConPTY on Windows, PTY elsewhere.                                                                |
| Store       | `rusqlite` with WAL                     | One file, offline-first, trivial to back up.                                                     |
| Crypto      | `argon2`, `chacha20poly1305`, `zeroize` | Established RustCrypto crates. Nothing home-made.                                                |
| Server      | Rust + `axum`, SQLite                   | One language across the stack, one Docker image.                                                 |

## The secret boundary

```
┌─ WebView ─────────────┐ │ ┌─ Rust Core ───────────────────────┐
│ Host tree, tabs       │ │ │ SessionManager → russh → SSH host  │
│ Inspector, SFTP       │ │ │ VaultService   → keys, passwords   │
│ xterm.js (WebGL)      │ │ │ SyncEngine     → outbox, merging   │
│                       │ │ │ Store          → SQLite            │
│ no access to secrets  │ │ │                                    │
└───────────────────────┘ │ └────────────────────────────────────┘
        ↓ connect, resize (JSON command)
        ↑ PTY bytes (raw channel, batched every 8 ms)
```

Private keys and passwords never cross that line. Authentication happens
entirely in Rust. The WebView gets terminal bytes and metadata, which means an
XSS hole in the frontend costs you disrupted sessions, not your key material.

## Terminal throughput — measured, settled

This was the one risk that could have invalidated the design: terminal output
crossing the IPC boundary. It has been measured, and the answer is in
[the M0 spike](m0-spike.md). Short version: **the IPC channel stays, the
WebSocket fallback is gone, and end-to-end flow control is mandatory.**

The data path, as built:

```
source (SSH socket, PTY reader, synthetic)
  → bounded queue          backpressure to the source when full
  → batcher                8 ms frames, 256 KiB cap, pauses on the renderer
  → tauri::ipc::Channel    raw bytes, no JSON
  → xterm.js write()       acknowledges parsed bytes back to the batcher
```

With flow control, 64 MiB of coloured log output reaches the screen at
41–46 MiB/s with no UI frame above 7 ms. That rate is xterm.js' own parse
speed — the channel delivers faster — so no transport change could raise it.
Local shells on Windows are capped far lower by ConPTY (~1.7 MiB/s), which SSH
sessions never go through.

### Why flow control is not optional

`Channel::send` returns once Tauri has queued a frame, not once the webview has
parsed it. Without acknowledgements, backpressure stops at the IPC boundary, the
webview queues everything — and xterm.js throws away writes once more than
50 MB are pending. Measured: about 9 MiB lost from a 64 MiB flood, twice, with a
perfectly smooth UI that gave no hint of it.

So the renderer acknowledges from xterm.js' write callback, the batcher pauses
at 512 KiB outstanding and resumes at 128 KiB, and acknowledgements go out every
64 KiB. That last size must stay below the resume threshold, or a paused stream
deadlocks on an unacknowledged remainder; `uwussh-core` asserts it at compile
time. It is the scheme VS Code's terminal uses, for the same reason.

### Bytes are never dropped

An early draft of the concept said frames would be dropped on overflow. That was
wrong: dropping bytes before xterm.js cuts escape sequences in half. The engine
never drops; it only ever makes the source wait.

## Data model

Every syncable entity shares one header:

```
id          UUIDv7   time-sortable, no coordination needed
updated_at  HLC      hybrid logical clock (wall_ms, counter, device_id)
rev         u64      local revision counter
deleted     bool     tombstone, 90 day TTL
vault_id    UUID     personal | work | shared
```

Entities: `Host`, `Group`, `Identity`, `Key`, `Snippet`, `PortForward`,
`KnownHost`, `TerminalProfile`. `SessionLog` stays local and does not sync by
default.

`Host.jump_host_id` is a self-reference, which gives ProxyJump chains for free:
`laptop → bastion → db-01` is a linked list the SessionManager resolves
recursively, with a depth limit of 8 and cycle detection.

## Vault

```
Master password
   └─ Argon2id (m=64 MiB, t=3, p=4, per-vault salt)
        ├─ bytes  0..32  →  master key        never leaves the device
        └─ bytes 32..64  →  auth secret → HKDF → server login hash

Vault key (32 random bytes)
   └─ wrapped with the master key → wrapped_vault_key
```

Two consequences justify this shape: changing the password only rewraps one key
instead of re-encrypting every record, and the server only ever learns the login
hash. It is the Bitwarden model, which is well understood and has been looked at
by many more eyes than mine.

Records are encrypted with `XChaCha20-Poly1305`, a fresh nonce each, and
`record_id || entity_type || vault_id` as associated data — so a malicious
server can neither swap blobs between records nor reinterpret their type.

## Sync

Offline-first: everything lands in SQLite first, the server is a relay.

| Endpoint                   | Purpose                                          |
| -------------------------- | ------------------------------------------------ |
| `GET /v1/sync?since=<seq>` | Blobs newer than a cursor, paginated             |
| `POST /v1/sync`            | Batch push, each record with its `base_rev`      |
| `WS /v1/stream`            | "Changes from seq N" — the client then pulls     |
| `POST /v1/auth/login`      | Login hash → session token + `wrapped_vault_key` |
| `GET/DELETE /v1/devices`   | List and revoke devices                          |

The cursor is a monotonic server sequence number, not a timestamp. Clocks across
devices are a bug source; sequence numbers are not.

Conflicts resolve last-writer-wins **per field**, decided by the HLC. On `409`
the server returns current state, the client merges field-wise and retries up to
three times before showing a conflict banner. This is deliberately not a full
CRDT — two devices practically never edit the same field of the same host at the
same moment. The exception is snippet bodies, where real text collisions are
possible; those get a three-way merge with conflict markers.

## Import

Every importer is an adapter that writes into one neutral `ImportedHost`
intermediate form. Everything after that — preview with checkboxes, duplicate
detection on `address:port`, group assignment, then the write — is shared. A new
source costs one adapter, not a new pipeline.

`.ppk` files from PuTTY and KiTTY need no conversion: russh reads PuTTY key files
natively, encrypted or not, so a session imported from PuTTY logs in with its
own key file as it is. The importers live in `crates/uwussh-import`, testable
against real registry dumps and sample configs without launching the app.

## Connecting

The order is the security model, and it is the order PuTTY and OpenSSH use:

1. **Connect and check the host key.** An unknown key ends the attempt with the
   fingerprint and randomart for the trust dialog; a changed one ends it with
   both fingerprints. Nothing about the user has been sent, and the user has not
   been asked for anything.
2. **Then ask for secrets.** A missing password or passphrase is a question for
   the user, and the verified connection waits for the answer — up to 110
   seconds, just under OpenSSH's default `LoginGraceTime` — so the answer, and a
   second try after a typo, go over the same connection.
3. **Open the shell** on the authenticated connection.

An earlier version asked for the password before connecting, to save a round
trip. Nothing was ever sent to a changed key, but the user typed the password
first and read about a possible man in the middle second. The end-to-end run
caught that; the order above is the fix.

Trusting a key goes through the Rust side, which only accepts a fingerprint a
server actually presented in the last attempt, so a compromised webview cannot
hand in a key of its own choosing. Replacing a key that was already trusted
additionally needs the address typed out.

## Storage

`crates/uwussh-store` keeps hosts, identities and trusted host keys in one
SQLite file — bundled SQLite, WAL — in the app data directory, or wherever
`UWUSSH_DB` points. Records carry the sync header from the first row on, so sync
in M2 needs no data migration.

**No secrets are stored** until the vault exists: passwords are asked for on
every connect and never written anywhere, keys are referenced by path.

## How it is tested

- **Unit tests** in every crate: clock, crypto, merge, outbox, parsers, flow
  control, the store.
- **SSH integration tests** in `crates/uwussh-core/tests/ssh.rs` run a real SSH
  server in-process — russh's server half — and check on every commit what
  matters most: no login attempt against an untrusted or changed key, the
  password asked for only after the key checked out and sent over that same
  connection, keystroke order, resize, remote exit, and an 8 MiB flood that
  arrives complete under a deliberately slow renderer.
- **End to end**, `node apps/desktop/e2e/run.mjs`, drives the real app against
  `crates/uwussh-core/examples/dev_sshd.rs` over WebView2's DevTools protocol:
  adding a host, trusting its key, a wrong and a right password, a 32 MiB
  flood, the session ending, a reconnect, the server's key changing, deleting
  the host — and counts connections and password attempts in the server's log.

The end-to-end run earned its place on its first outing. It found four bugs no
other test could see: the password asked for before a changed host key was
shown; Enter in a dialog reaching a button behind it and starting a second
connection; a resize feedback loop that grew the terminal to 5000 columns; and a
validation message that stayed after the field was fixed.
