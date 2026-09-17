# Security review, September 2026

Done before the first beta (0.1.0-beta.1), across the whole client: the vault
and store, the SSH engine, the importers, the Tauri commands, the new installer
and updater, and the web page. What was found, what was fixed, and what is left
on purpose.

## The trust boundary

The page inside the window is **inside** the trust boundary. It can start a
local shell and type into it, so a compromised page already runs code as the
user. Everything below that guards against the page (host keys it may trust,
pages it may open, file names it may name) is defence in depth. What really
keeps the page trustworthy is that it only ever renders UwUSSH's own code: a
strict content security policy (`script-src 'self'`, no inline scripts, no
frames, no objects), frozen prototypes, no `dangerouslySetInnerHTML`, and
terminal output that never becomes markup.

## Fixed

| Severity | Where                              | What                                                                                                                                                                                                                                                              |
| -------- | ---------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Medium   | `uwussh-store` vault               | Revealing a secret took the vault lock, then the database; an import took them the other way round. Connecting with a vault password during an import could freeze the app. Both now go database first; a test runs them against each other.                      |
| Low      | `uwussh-import` V8 reader          | A run of object-count markers recursed without a depth limit (stack overflow); back-references copied whole trees, so 40 short levels could claim 2^40 values; sparse arrays allocated megabytes from a few bytes. Now a loop and a budget of 200,000 values.     |
| Low      | `uwussh-import` IndexedDB, LevelDB | Array keys recursed without a limit; a block offset near the end of memory could wrap; keys that grow per entry made copying quadratic; a five-byte Snappy header could ask for 4 GiB. Now a depth limit, checked arithmetic, a copy budget and a 64 MiB ceiling. |
| Low      | `uwussh-import` PuTTY              | A session name like `%1ü` sliced a character in half and panicked; non-ASCII names came back garbled. Decoded on bytes now.                                                                                                                                       |
| Low      | `uwussh-vault`, store              | KDF costs in the vault header were used as stored; a tampered header could make every unlock run out of memory. Costs outside sane limits are refused.                                                                                                            |
| Low      | Updater                            | The signature was checked on bytes in memory and the file started by path afterwards; the file is now held locked against writes and deletes from the check until the setup runs.                                                                                 |
| Low      | Updater                            | The feed is unsigned, so it could offer an older signed setup under a newer version. The signature's trusted comment names the signed file, and the app now requires it to be `UwUSSH-Setup-<the feed's version>.exe`; `pnpm release` checks the same.            |
| Low      | Setup                              | In update mode, an unreadable installed version let any version through. It now refuses when it can't tell.                                                                                                                                                       |
| Low      | Terminal                           | Links a program prints (OSC 8) went to xterm.js' default handler: a `confirm()`, then navigation to whatever the server sent. They now open only on Ctrl+click, only as `http`/`https`, through a Rust command that checks again.                                 |
| Low      | SSH, `ssh_config`                  | Key files and `Include`s on network shares (`\\server\share`) were opened, which makes Windows send the user's login hash to that server. Refused now.                                                                                                            |
| Low      | SSH                                | A server that accepted the connection and then stayed silent kept a tab connecting forever. Login and opening the terminal now time out after 30 seconds.                                                                                                         |
| Low      | Host keys                          | A key the server presented stayed trustable for the rest of the session, even after the user declined it. It now expires after ten minutes.                                                                                                                       |
| Low      | Setup                              | The uninstaller's self-delete put its temp path into a `cmd` line, where `%…%` expands even inside quotes. The path now goes in through an environment variable.                                                                                                  |

Found and fixed while building the beta, before this review:

- Linked and runtime DLLs load from System32 only, in the app and the setup
  (`/DEPENDENTLOADFLAG` and `SetDefaultDllDirectories`), because both run from
  folders the user can write to.
- The content security policy got stricter (no frames, objects, base URIs or
  form targets) and prototypes are frozen.
- A reloaded page closes every session an earlier page left open, instead of
  leaving SSH connections running unreachable in the background.
- Connection attempts are keyed by tab and validated, so two tabs never share a
  half-open connection.
- The M0 measurement refuses payloads above 256 MiB, which would otherwise be a
  way to fill the disk.
- Release builds ship no source maps.

## Accepted, for now

- **A stored secret follows an edited address.** Changing the address of a host
  that logs in with a vault password sends that password to the new address,
  after its host key is trusted. That is also what a user fixing a changed IP
  wants. The host key dialog is the gate; the page that could change the address
  unnoticed is inside the trust boundary anyway.
- **Pointers between records aren't authenticated.** A secret's ciphertext is
  bound to its own id, but which identity points at which secret is not sealed.
  That matters once a sync server (M2) could rewrite records, and gets fixed
  there: owner and field go into the associated data.
- **The input queue to a server is unbounded.** Pasting megabytes into a server
  that stopped reading grows memory until the paste ends. Bounded with the
  paste confirmation that comes with snippets.
- **The same Windows user is trusted.** Anything running as the user can
  replace the installed program itself, so guarding the update folder against
  that user only narrows a window; it doesn't close a door.

## Checked and fine

- Vault crypto: random 24-byte nonces per record, associated data binds record
  id, kind and vault; the wrapped vault key has its own label; keys are wiped
  from memory; a wrong password fails the wrapped key's tag, with no separate
  verifier to attack.
- SQL: every statement is parameterized.
- Host keys are checked before the username or any secret is sent; host
  certificates are pinned by their inner key; a waiting connection is reused only
  for the same address, port and user.
- The setup deletes only files it names and removes folders only when empty.
- `ssh_config` includes can't loop: canonical paths and a depth limit.
