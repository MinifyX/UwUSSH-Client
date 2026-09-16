# Architecture

How the pieces fit together. The German [KONZEPT.md](../KONZEPT.md) has the
long version.

## The stack

| Layer | Choice | Why |
| --- | --- | --- |
| Shell | Tauri 2 | Same as UwUMail. ~12 MB instead of Electron's 150, WebView2 on Windows, mobile support built in. |
| UI | React + TypeScript, Node 24, pnpm 11 | UwUMail's stack, so tokens, components and Nyu carry over. |
| Terminal | `xterm.js` + WebGL addon | What VS Code and Termius use. Canvas fallback. |
| SSH | `russh`, `russh-keys`, `russh-sftp` | Pure Rust, async, no libssh2 FFI pain. |
| Local shell | `portable-pty` | ConPTY on Windows, PTY elsewhere. |
| Store | `rusqlite` with WAL | One file, offline-first, trivial to back up. |
| Crypto | `argon2`, `chacha20poly1305`, `zeroize` | Established RustCrypto crates. Nothing home-made. |
| Server | Rust + `axum`, SQLite | One language across the stack, one Docker image. |

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

## The one real risk

Terminal throughput across the IPC boundary. A `cat bigfile.log` pushes several
MB/s, and Tauri's classic `emit` events serialise to JSON, which does not carry
that.

The plan is `tauri::ipc::Channel<&[u8]>` with raw bytes, PTY output batched into
8 ms frames in the core instead of per read, backpressure through a bounded
channel, and deliberately dropping frames on overflow — the scrollback truth
lives in xterm.js' buffer anyway. If that measurably isn't enough, the fallback
is a local WebSocket on `127.0.0.1` with a one-time token and binary frames.

**M0 starts with measuring this**, not with the UI. It is the only question that
could invalidate the whole design.

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

| Endpoint | Purpose |
| --- | --- |
| `GET /v1/sync?since=<seq>` | Blobs newer than a cursor, paginated |
| `POST /v1/sync` | Batch push, each record with its `base_rev` |
| `WS /v1/stream` | "Changes from seq N" — the client then pulls |
| `POST /v1/auth/login` | Login hash → session token + `wrapped_vault_key` |
| `GET/DELETE /v1/devices` | List and revoke devices |

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

`.ppk` files from PuTTY and KiTTY need converting to OpenSSH format, which
`russh-keys` does not do, so that is our own parser (v2 and v3, encrypted and
not). It lives in `crates/uwussh-import` so it can be tested against real
registry dumps and sample configs without launching the app.
