# Release notes

One file per version, named after it: `0.1.0.json`, `0.1.0-beta.1.json`. `pnpm release` refuses to run
without it. The text appears under "Was ist neu?" in UwUSSH's update hint and on the GitHub release page.

```json
{
  "de": "- Kurze, verständliche Punkte\n- Was Leute merken, nicht wie es gebaut ist",
  "en": "- Short, plain points\n- What people notice, not how it's built"
}
```

## Releasing a version

1. Set the version in `Cargo.toml` (workspace), the `tauri.conf.json` of `apps/desktop`, `apps/setup` and
   `apps/keygen`, and the `package.json` files.
2. Add `release-notes/<version>.json`.
3. Commit, tag `v<version>` and push both. The tag starts `.github/workflows/installers.yml`, which
   checks the workspace on macOS and Linux and builds their setups — unsigned, since CI holds no key.
4. Run `pnpm release` on Windows, with the tag checked out.

`pnpm release` builds the app, packs it into `UwUSSH-Setup-<version>.exe` (`pnpm build:setup`), waits
for the tag's CI run and downloads the macOS disk images, the macOS update programs, the Linux AppImage
and the `.deb`, signs every file the updater runs, checks each signature against the key in
`tauri.conf.json`, creates the GitHub release with all of it and a `SHA256SUMS.txt`, and writes the
update feeds (Windows, macOS on both architectures, Linux) to the `updates` branch. It waits until
everything is online and checks it. If CI can't build for some reason, `pnpm release --windows-only`
publishes Windows alone.

It needs the update signing key, either as `TAURI_SIGNING_PRIVATE_KEY` +
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` or as a folder with `uwussh-update.key` and `PASSWORT.txt` in
`UWUSSH_UPDATE_KEY_DIR` (default: `Documents\UwUSSH-Update-Schluessel`). The key never goes into this
repository, and the app and setup builds never see it: only the signing step does. Losing it means
installed apps can't take updates any more, so keep a copy somewhere safe.

## Channels

Versions with a suffix (`-beta.1`) go to the Beta feed only; plain versions go to Stable and Beta. An
installed beta starts on the Beta channel, everything else on Stable; Settings → Updates switches.
