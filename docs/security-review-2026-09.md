# Security review, September 2026

Four rounds: the first [before the first beta](#the-trust-boundary), the second
[before the second](#second-round-before-010-beta2), the third
[before beta 8](#third-round-before-010-beta8), the fourth
[after 0.1.3](#fourth-round-2026-09-23-after-013).

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

## Second round: before 0.1.0-beta.2

The second beta brought a lot of new surface: file access over SFTP and SMB,
root through `sudo`, the password helper, passwords and keys per host, the
vault remembered with DPAPI, export files, UwUKeygen and its place in the
installer. Three independent reviews went over it, split by area — files and
the command surface, store and crypto, installer and key generation — against
the same trust boundary as above. Nothing High was left standing; one High
(shared logins) was found while it was already being fixed.

### Fixed

| Severity | Where           | What                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| -------- | --------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| High     | Store           | Logins a Termius import shares between hosts were changed in place: saving a password for one host gave it to every host with that login — and connecting to one of them sent it there — and deleting a host took the others' login with it. A shared login is now copied before it changes, and a password or login goes only when no host uses it. The upgrade to schema 3 repairs logins the first beta tombstoned.                                             |
| Medium   | Files, root     | Deleting a folder as root walked the tree over SFTP. A user on the server could swap a listed subfolder for a link to `/etc` between listing and deleting, and the delete went on in there, as root. Root deletes now run the server's own `rm -rf` through `sudo`, which never follows a link; the path is quoted and must be absolute, not `/`, without `..`.                                                                                                    |
| Medium   | Files, root     | Links to folders listed as plain folders, chmod followed links (SETSTAT does), and uploads followed a link in the target's place. Links are shown as links, chmod refuses them, and files are created with `O_EXCL`; replacing removes the link itself first.                                                                                                                                                                                                      |
| Medium   | Files, SMB      | Opening an SMB share fell back to the host's stored SSH password. SMB has no host key, so a spoofed name (LLMNR on a hotel network) would have received a crackable hash of it. The password is now always typed for the share; the address must be a plain name or IPv4 address.                                                                                                                                                                                  |
| Medium   | Export          | Reading an export trusted every host key in it, for any address — a shared host list could pre-trust a key for a server the user adds next month — and brought back keys the user had removed, without checking a fingerprint against its key. Only keys for the file's own hosts are taken, only when the fingerprint is that key's, and never over a key that exists or existed.                                                                                 |
| Low      | Password helper | The button and Ctrl+Shift+P typed the password plus Enter without a prompt on screen — into bash history, if sudo had timed out — and the prompt patterns also matched `user@other-host's password:`, git's `Password for 'https://…'` and a bare `Password:`. The helper now offers itself only for sudo, sudo-rs or doas asking for this login; the button always types, but adds Enter only when the cursor sits after a question, checked right before typing. |
| Low      | Key files       | A key file's own key derivation settings were used as written: a `.ppk` asking for four billion Argon2 passes froze the window once a passphrase was typed, and PKCS#8 asking scrypt for a terabyte ended the app. Costs far beyond any real key file are refused before the passphrase is tried, and opening encrypted keys runs off the UI thread.                                                                                                               |
| Low      | Key files       | The network share check for key paths looked at the path as written: `\??\UNC\server\share` and `~/\\server\share` got through. It now checks the path as it will be opened, and only a drive letter counts as local; export files lose key paths that aren't.                                                                                                                                                                                                     |
| Low      | Files           | Downloads and uploads replaced existing files and merged folders silently — a server-chosen tree could overwrite `.git/config` in a local folder of the same name. Everything now stops before writing and asks.                                                                                                                                                                                                                                                   |
| Low      | Files           | Opening file access had no timeout; the "copy into itself" check compared path text, so a mapped drive and its share recursed until the stack overflowed; cancel didn't reach the folder walks. Now a 40 second timeout, resolved case-insensitive paths with a depth limit, and a cancel check in every loop; a reloaded page cancels running transfers.                                                                                                          |
| Low      | Export          | With "take passwords and keys" ticked but nothing secret stored, the file was written in plain JSON despite the password. A password now always seals. Files asking for more than 256 MiB or eight passes of key derivation are refused.                                                                                                                                                                                                                           |
| Low      | Store           | Forgotten passwords and a forgotten device key stayed in SQLite's free pages and the write-ahead log. `secure_delete` is on and the log is truncated after forgetting; the upgrade wipes passwords of hosts the first beta deleted.                                                                                                                                                                                                                                |
| Low      | System probe    | The probe ran on every connect, and a key with a forced command in `authorized_keys` ran that command again each time; on a timeout its channel stayed open. It runs once per host now and always closes its channel.                                                                                                                                                                                                                                              |
| Low      | Files, root     | The sudo handshake took the prompt marker anywhere in the output, and could read into the SFTP stream when the ready marker's newline came separately. The prompt counts only at the very end, and the handshake reads byte by byte up to the stream.                                                                                                                                                                                                              |
| Low      | Setup           | The setup closed a running UwUKeygen without asking, losing a generated key that wasn't saved yet. It now counts as running, and the question says what is lost.                                                                                                                                                                                                                                                                                                   |

Smaller hardening in the same pass:

- A private key copied from UwUKeygen goes through Rust with the formats that
  keep it out of Windows' clipboard history and cloud clipboard, and is cleared
  after a minute if nothing else was copied. Saved private key files get an
  access list for this user, SYSTEM and Administrators only.
- Argon2's working memory is wiped after every derivation; secrets inside a
  sealed export are base64, so the JSON reader never copies them into a buffer
  nobody wipes, and the file is written into a buffer sized once.
- More Windows device names are refused for downloaded names (`COM0`, `COM¹`,
  `CONIN$`, `com1 .txt`); downloads carry the "from the internet" mark.
- Imports skip hosts the host form would refuse (spaces or control characters
  in address or user) and take at most 50,000 entries of a kind.
- `.ppk` v2 with a passphrase carries a warning: one SHA-1 round protects
  little against guessing.
- Forgetting a host's password also forgets the copy an open terminal kept;
  custom highlight rules with nested repetition like `(a+)+` are refused; the
  master password's and the export's key derivation run off the UI thread; a
  picked key file is dropped from memory when its dialog closes.

### Accepted, for now

- **The vault can open with the Windows account.** Remembering the vault key
  with DPAPI makes the vault exactly as strong as the Windows account: anything
  running as that user can open it, and so can someone with the account's
  password and a copy of the disk. That is the user the trust boundary already
  trusts, it is one checkbox, and Settings turns it off.
- **A changed host key is accepted with one click.** Typing the address
  protected nothing the warning doesn't; focus sits on Reject.
- **Listing a huge remote folder is slow.** `russh-sftp` collects directory
  batches quadratically; a folder with a million entries holds a worker thread
  for a long time. Cancel reaches everything around it; a streaming listing
  needs a change in the library.
- **Uploading as root into a folder another user can write** can still be
  raced on folders created on the way; only the final name is `O_EXCL`. Deletes,
  the dangerous part, don't walk at all.
- **The login password stays in memory while its terminal is open**, for the
  helper. It is wiped when the tab closes or the password is forgotten.
- **UwUKeygen's window keeps Tauri's default permissions.** The page is
  UwUSSH's own code under the same content security policy; trimming the list
  gains nothing that boundary doesn't already give.

## Third round: before 0.1.0-beta.8

This release wires the sync client into the app — Settings → Sync, a worker
thread, pairing — and brings UwUSSH, its setup and its updater to macOS and
Linux. An independent review went over the whole client first, with the sync
server added to the trust boundary as hostile: it may lie, stall, answer with
anything, and it must still learn nothing and break nothing. No Critical or
High finding; the sync transport was where the work was, and it was fixed
before a single build shipped it.

### Fixed

| Severity   | Where          | What                                                                                                                                                                                                                                                                                                                                                                   |
| ---------- | -------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Medium     | Sync transport | The "plain HTTP only to this machine" check read the host by hand: `http://localhost:1@evil.example` passed as localhost, and reqwest sent the invite, the login key and every token in the clear to `evil.example`. Addresses are now parsed the way reqwest parses them; `http` needs a loopback host, and user names, passwords, queries and fragments are refused. |
| Low–Medium | Sync transport | Redirects were followed, with the body sent again — a login key included — to wherever the server pointed, past the pin and possibly down to plain HTTP. The protocol has no redirects; none are followed now.                                                                                                                                                         |
| Low–Medium | Sync transport | Answers were read without a limit, so a hostile server could stream gigabytes into memory until the app died. Record pages are capped at twice a full batch plus a megabyte, everything else at 256 KiB, error bodies at 64 KiB.                                                                                                                                       |
| Low        | Sync engine    | A server that always said "there is more" kept the sync thread pulling forever. A pass pulls at most a thousand pages, and stops at a page where every record failed its seal.                                                                                                                                                                                         |
| Low        | Pairing        | A pairing that failed — wrong words, a timeout, a relay error — left its session open on the server for its ten minutes. It is closed on every path now; a retry always makes new words.                                                                                                                                                                               |
| Low        | ssh_config     | The network-share check for `Include` looked at the pattern as written, so `~/\server\share\x` only became a UNC path after `~` was expanded — and reading it sends the Windows login hash to that server. The expanded path is checked too, and only a drive letter counts as local.                                                                                  |
| Info       | Sync transport | reqwest honours `HTTP_PROXY`, which for an `http://localhost` server would have sent the plain-text requests to the proxy. Plain HTTP now never uses one.                                                                                                                                                                                                              |
| Info       | Device seal    | The vault key and the pairing's keys were to be sealed with the same DPAPI entropy. Each purpose has its own label now, so a blob made for one can't be opened as the other.                                                                                                                                                                                           |

Also found while building, not by the review: the export dialog's "close
anyway?" question was asked but never shown, so Escape did nothing while a file
password was typed.

### New surface, and how it is held

- **Settings → Sync** never hands a secret to the page except the recovery
  code, once, right after the account was made. Master password and pairing
  codes go straight into one command each and are not kept. Revoking another
  device needs the master password (the login key comes from it); leaving
  re-wraps the vault without the account key only after the password opened
  it.
- **The worker thread** syncs only while the vault is open, and drops its
  connection after any failure, so a pin, token or address problem is met
  afresh on the next pass.
- **Sealing on macOS and Linux** uses a random key of UwUSSH's own in the
  Keychain or the Secret Service, and XChaCha20-Poly1305 with a label per
  purpose. Where no Secret Service answers, the key falls back to a file only
  this user can read (0600, not a link, refused if anyone else may read it).
- **The macOS and Linux setup** unpacks only the two app folders it knows,
  entry by entry, refusing any path outside them; replaces an app by moving the
  old one aside first; removes only those folders on uninstall — never
  `/Applications` or a folder that holds anything else.
- **Updates on macOS and Linux** run the downloaded setup only after the same
  two signature checks as on Windows, and only for a copy of UwUSSH the setup
  installed (its `install.json` names the folder the running app is in). An app
  from the `.deb` updates the way it came.

### Accepted, for now

- **Pairing messages share one seal label** in both directions and all steps.
  A replayed message fails to parse where it doesn't belong, so nothing is
  exploitable; changing the label would break pairing between this version
  and the last.
- **The WebView2 bootstrapper** is trusted on HTTPS alone. Checking its
  Authenticode signature would add something only on networks that intercept
  TLS with their own trusted root — which could sign code just as well.
- **Nothing is notarized on macOS, nothing is code-signed on Windows.** Both
  need paid certificates; the updater's own signature is what protects
  updates, and each release lists SHA-256 sums for the first download.
- **The seal key file on Linux without a Secret Service** is as strong as the
  file permissions: anything running as this user, or anyone with the disk,
  can read it. The install guide says so.
- **The unsealed update on macOS and Linux could be swapped** by a process of
  the same user between the check and the start. That process can already
  replace the app itself.

## Fourth round: 2026-09-23, after 0.1.3

A read-only review of 0.1.3, in four parts — the SSH engine and file access,
vault, store and sync, the importers, and updater, setup, keygen and CI — plus
the Tauri command surface, against the same trust boundary. It also covered
`0671b0a` (sync manifests, the KDF floor, pairing labels), which has no section
of its own here. No Critical or High finding; four Medium, fixed below.

### Fixed

| Severity | Where       | What                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| -------- | ----------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Medium   | Files, root | Root deletes and chmod "never follow a link" held only for the last name in the path: the kernel still followed a folder further up that someone had swapped for a link after it was listed, and chmod checked for a link over SFTP and changed permissions with SETSTAT, which follows one, a moment later. Both now run through `sudo sh`, change into the folder the target is in with `cd -P` and refuse unless `pwd -P` is that folder, then act on `./name`. chmod changes a folder as `.` from inside it, and a file only where nobody but root can write to the folder. A trailing slash is refused, and a root session lists folders under their real path so paths through a link like `/var/run` keep working. (`1f689ab`) |
| Medium   | Sync engine | The manifest check only ran after a pull that reached the end; a server that always said "there is more" kept every pass incomplete, the check never ran, and host keys from other devices stayed trusted — even on a device whose pairing named a manifest to wait for. A pull that stops short is now written down as something kept back (with host keys among it) until a complete one clears it, a paired device distrusts other devices' host keys until the manifest from the pairing has been checked, and the report says whether the pass was complete. (`a54a987`)                                                                                                                                                         |
| Medium   | Updater     | The Linux setup, an AppImage run with `APPIMAGE_EXTRACT_AND_RUN`, unpacked into `/tmp/appimage_extracted_<hash>` — a name anyone could work out from the public release — and ran what it found there, so another local user could put their `AppRun` there first. The setup now unpacks into a fresh 0700 folder in UwUSSH's local data folder, and hands the `TMPDIR` from before back to the UwUSSH it starts again. Protects updates applied by this version on. (`15a0ecc`)                                                                                                                                                                                                                                                      |
| Medium   | Device seal | A keychain, Secret Service or DPAPI that did not answer — locked, slow, a macOS prompt denied after an update — counted as "this no longer opens": the pairing's account key and the remembered vault key were deleted for good, and on macOS and Linux a new seal key was made on the way. Only a blob that fails its seal against every key there is (`InvalidData`) is forgotten now; anything else is an error and everything stays. Opening never makes a key, and a keychain entry that isn't a key is no longer overwritten. (`3e2a994`)                                                                                                                                                                                       |

### Not fixed: Low and Info

Listed for a later round; none is exploitable without a precondition named in
the review.

- **L1** An import can pre-trust a host key for a host the user already has, by
  using another user name for it.
- **L2** Channels the server opens towards the client (agent, X11, forwarded
  ports) are accepted and never closed.
- **L3** The key-file cost check can be bypassed: duplicate PPK headers, and
  PEM files the check can't parse but russh can.
- **L4** Windows: a custom install folder inherits a writable ACL; an existing
  folder's owner and ACL are not checked.
- **L5** `local_size` recurses without a depth limit.
- **L6** Copying from an SMB share skips the download name checks and the
  Mark-of-the-Web.
- **L7** `ProxyJump` and PuTTY proxy settings are dropped silently on import.
- **L8** Push answers are capped at 256 KiB although they carry whole conflict
  records; sync can stall.
- **L9** The server can flip `needs_account_key` on join.
- **L10** Joining the same account again after leaving keeps a password-only
  header.
- **L11** Revoking a device rotates nothing, and the advice in the UI doesn't
  help.
- **L12** The V8 reader's budget counts values, not bytes.
- **L13** The folder import has no total cap and runs on the main thread.
- **L14** One invalid group path aborts a whole import.
- **L15** The `Include` wildcard matcher takes exponential time.
- **L16** The Linux setup keeps its WebView data in a fixed path in `/tmp`.
- **I1–I16** Fixed names in `/tmp` for M0; the local shell started by name;
  no size limit on update downloads; a waiting beta kept after switching to
  stable; saving a key over an existing file; release builds without
  `--locked`; one host key per host and no algorithm preference; waiting
  connections never expire; zeroisation gaps; server KDF bounds looser than
  file bounds; a server cursor near `u64::MAX`; a pairing session left open on
  one error path; symlinks followed in the folder import; network key paths
  kept on import; re-trusting reviving a tombstone; rm markers matched
  anywhere in the output.
- **I17** `rsa` 0.10.0-rc.18 carries RUSTSEC-2023-0071 (Marvin): its private-key
  operations are not constant-time, and no fixed version exists. UwUSSH uses it
  to sign one login with an RSA key and to make keys in UwUKeygen; a server would
  have to time a great many signatures of the same key, which a client that
  signs once per login does not give it. Watched, to be taken when a fix exists.

Also noted: `rename` as root has the same issue as M1 for folders further up
the path, with less at stake; it still goes over SFTP.

### Earlier entries, corrected

- "Pairing messages share one seal label" (third round, accepted) no longer
  holds: since `0671b0a` each direction has its own label.
- "UwUKeygen's window keeps Tauri's default permissions" (second round,
  accepted) no longer holds: its capabilities are trimmed.
- "Owner and field go into the associated data" (first round) was done
  differently: the pointers between records live inside sealed payloads, which
  a server can't change either.
