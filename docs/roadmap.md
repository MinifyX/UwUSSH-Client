# Roadmap

My wish list, without dates. The order is deliberate: the biggest risk goes
first, not the nicest feature.

## M0 · Foundation

Tauri shell, `xterm.js`, `russh` connect, password and key auth, the SQLite
schema, a host list.

**It starts with a throughput spike.** Tauri shell + xterm.js + `russh` against
a test host, `yes` as the load, measuring frame timing. That decides whether the
IPC channel carries a real terminal or whether the local WebSocket fallback is
needed — and it shapes the whole session architecture, so it cannot wait.

## M1 · Daily driver

The point where I can stop using anything else.

- Tabs and splits, `known_hosts` with TOFU, agent auth (Pageant,
  `\\.\pipe\openssh-ssh-agent`, 1Password), ProxyJump chains
- Snippets, broadcast input, themes, the command palette
- **Import: PuTTY, KiTTY, `ssh_config`, Termius**, plus the `.ppk` parser
- First Nyu scenes and the playful/neutral string split

## M2 · Vault and sync

- Vault crypto, recovery kit, OS keychain and biometric unlock
- `UwUSSH-Server` v1: Axum, SQLite, Docker image, admin CLI
- Device pairing (password and QR), device revocation, conflict resolution

## M3 · SFTP and tunnels

- SFTP browser, two columns, drag and drop
- Open a remote file in the local editor and write it back
- Port forwarding manager: local, remote, dynamic, with autostart per host

## M4 · Polish

- Updater and `UwUSSH-Releases`, portable build
- Linux and macOS builds
- Onboarding, accessibility pass, the full Nyu scene set

## M5 · Homelab

Where the distance to the commercial clients actually opens up.

- **Tailscale / Headscale import** — tailnet devices as a host list, MagicDNS
  names, live reachability. Probably the strongest single feature for this
  audience.
- **Proxmox import** — nodes, LXC and VMs via the API, groups mirroring the
  cluster
- **Netbox import** — inventory as the source of truth
- Local shell tabs (PowerShell, WSL, cmd) and serial console
- Session recording with asciinema export, persistent searchable scrollback
- **UwUSSH as an SSH agent** — other programs use the vault's keys through a
  named pipe or unix socket, with a confirmation prompt per signature
- SSH CA certificates; FIDO2 (`ed25519-sk`) — needs a look at `russh` support
  first

## M6 · Mobile and teams

- Android and iOS, since Tauri 2 does mobile and `russh` runs there
- Shared vaults with per-host ACLs

## Open questions

Things I haven't decided, roughly in the order they'll bite:

1. **PPK parser** — is there a maintained crate for PPK v2/v3, or do I write it
   with PuTTYgen test vectors?
2. **What can the Termius import actually reach?** Verify against a real export
   before promising it in the README. If it's CSV only, that belongs in the
   README honestly.
3. **Server default database** — SQLite (one volume, one backup) with Postgres
   as an option, most likely.
4. **Sync `known_hosts`?** — probably yes, on by default; it's the most common
   friction point when switching devices.
5. **File-based sync as a third option** — would Syncthing or Nextcloud as a
   transport, next to "local only" and "own server", be the shortest path for
   most homelabs?
6. **Shared vaults** — leave them out of v1 entirely, or keep the data model
   ready? `vault_id` already exists, so keeping the door open costs nothing.
