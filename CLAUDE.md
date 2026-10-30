# UwUSSH-Client

SSH-Client (Tauri 2 + React/TS, russh/SFTP, xterm.js) mit verschlüsseltem Vault und optionalem Sync zum UwUSync-Server. Version steht aktuell bei `0.4.0-beta.1`.

## Layout
- `apps/desktop` (`@uwussh/desktop`): Frontend `src/`, Tests `test/*.test.ts`, Tauri-Seite `src-tauri/`, E2E `e2e/`
- `apps/setup`, `apps/keygen`: eigener Installer bzw. UwUKeygen (je mit `src-tauri`)
- `crates/uwussh-core` (Sessions, PTY/SSH, SFTP), `-vault` (KDF, Verschlüsselung), `-store` (SQLite), `-sync` (Outbox, Clocks), `-import` (PuTTY/KiTTY/OpenSSH/Termius), `-keygen`, `-assist` (Befehls-Assistent), `-proto` (Typen Client/Server)
- `scripts/` Release/AUR/MAS/i18n-Check, `packaging/aur`, `release-notes/`, `docs/`

## Befehle
- Gezielt: `cargo test -p uwussh-<crate> [filter]`, `pnpm --filter @uwussh/desktop test|typecheck`
- `pnpm lint` = prettier + `node scripts/check-i18n.mjs`; i18n-Check nach jeder neuen `t()`/`N_()`-Zeichenkette ausführen (EN-Katalog `apps/desktop/src/i18n/en/*.json` muss sie haben)
- CI (`ci.yml`) zusätzlich: `pnpm -r build` und MAS-Clippy `cargo clippy -p uwussh-desktop --no-default-features --features mas --all-targets --locked -- -D warnings`
- E2E `node apps/desktop/e2e/run.mjs [--only=e]`: nur Windows (WebView2/CDP), Phase E braucht `../UwUSync-Server` als gebautes Binary
- Tauri-Crates (`apps/*/src-tauri`) auf dem Linux-Host nicht baubar, siehe ~/Projects/CLAUDE.md

## Fallen
- `apps/*/src-tauri/gen/schemas/` ist generiert, nie von Hand ändern
- `uwulock-core` in `Cargo.toml` ist auf einen Commit von UwULock-Client gepinnt (`client-0.3`); Anheben nur bewusst
- `@uwusuite/design` kommt als Release-Tarball-URL (aktuell v1.6.1): UI-Änderungen zuerst in UwUSuite-Design
- Öffentliches Repo: keine Schlüssel, `.env`, echte Hostdaten; Importer-Fixtures nur anonymisiert unter `crates/uwussh-import/tests/fixtures/`
- Rust-Mindestversion 1.89 (russh), `rust-toolchain.toml` beachten

## Plan/Stand
- Einstieg: `KONZEPT.md`, `docs/roadmap.md`, `docs/architecture.md`; `docs/vision.md`, `docs/design.md` bei Bedarf
- Nicht freigegebene Änderungen sammeln in `release-notes/unreleased.json` (de + en, siehe `release-notes/README.md`)

## Release
- Version in `Cargo.toml` (Workspace), `tauri.conf.json` von desktop/setup/keygen und allen `package.json` anheben; `unreleased.json` zu `release-notes/<version>.json` umbenennen
- Nach Merge Tag `v<version>` auf den Merge-Commit; CI (`installers.yml`) baut unsigniert, danach `pnpm release` (nur auf der Maschine mit Update-Signing-Key; `--dry-run` testet ohne Veröffentlichung)
- AUR via `aur.yml`/`scripts/aur.mjs`, Mac App Store via `mas.yml`/`pnpm build:mas`

## Security
- Kritische Pfade: `crates/uwussh-vault`, `uwussh-sync`, `uwussh-import` (Parser für fremde Formate), `uwussh-core` (Host-Key-Prüfung, SSH), `uwussh-keygen`, Updater/Installer in `apps/setup`
- Vor jedem Feature-Release den globalen Subagent `security-reviewer` über den Bereich seit dem letzten Tag laufen lassen; Ergebnis nach `docs/security-review-<version>.md` (bestehend: `docs/security-review-2026-09.md`, Schema `YYYY-MM`)
