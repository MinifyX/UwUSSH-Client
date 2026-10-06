# Mac App Store (and Microsoft Store)

UwUSSH ships on the Mac in two forms. The **setup / disk image** from GitHub
releases is what it has always been: not sandboxed, updates itself, has a
local shell. The **Mac App Store build** is the same SSH client with the
store's rules applied. This document is how that second one is built and how
to publish it. None of it has been through App Review yet, and none of it has
run on a real Mac yet (see [Before the first review](#before-the-first-review-needs-a-mac)).

The last section is an analysis of the Microsoft Store — no build exists for
it.

## What is different in the store build

|                          | GitHub (setup, DMG)                                 | Mac App Store                                                                                 |
| ------------------------ | --------------------------------------------------- | --------------------------------------------------------------------------------------------- |
| Cargo features           | default (`self-update`, `local-shell`)              | `--no-default-features --features mas`                                                        |
| Config                   | `tauri.conf.json` (+ `tauri.macos.conf.json`)       | + `tauri.mas.conf.json`                                                                       |
| Updates                  | updater plugin, feed on the `updates` branch, setup | none compiled in (`updates.rs` is not built); the store updates it                            |
| `app_flavor`             | `"github"`                                          | `"app-store"` — the page hides the update UI and the local shell                              |
| Sandbox                  | no                                                  | yes, `macos/Entitlements.mas.plist`                                                           |
| Local shell, M0 PTY run  | yes                                                 | not compiled in (no `local-shell`, no PTY code); the command stand-in refuses, no local tab   |
| Key files a host names   | read from where they are                            | readable once picked (`pick_key_path`) or inside a granted folder (`grant_ssh_folder`)        |
| `~/.ssh/config` import   | found on its own                                    | after the person grants `~/.ssh` once in an open panel                                        |
| Termius import           | yes                                                 | no: its data is in another app's folders and its key in Termius' keychain item                |
| File browser, this Mac   | home, Desktop, Documents, Downloads, `/`            | Downloads and folders the person added (`local_pick_folder`), kept by bookmarks               |
| Delete to Trash (local)  | through Finder (`osascript`), "Put Back" works      | through `NSFileManager`, no "Put Back"                                                        |
| This Mac's name for sync | `scutil --get ComputerName`                         | `NSHost.localizedName` (no program is started)                                                |
| Single instance          | plugin (socket in `/tmp`)                           | Launch Services alone (macOS never starts a bundle twice); `uwussh://` links still arrive     |
| Where the data lives     | `~/Library/Application Support/app.uwussh.desktop`  | `~/Library/Containers/app.uwussh.desktop/Data/Library/Application Support/app.uwussh.desktop` |
| Setup app, UwUKeygen app | yes                                                 | not part of it (the store installs; the keygen is the Keys tab)                               |

`~` in a key path or an ssh_config means the real home folder in both builds:
inside the sandbox `HOME` is the container, and `uwussh_core::home_dir` (and
its twin in `uwussh-import`) step back out of it.

## How the sandbox access works

A sandboxed app may open only what the person picked in an open or save panel,
and only until it quits. Everything UwUSSH keeps for later goes through
`src-tauri/src/sandbox_access.rs`: a **security-scoped bookmark** per picked
file or folder in `bookmarks.json` next to `uwussh.db`, resolved and switched
on in `setup` before the page loads (at most 256; a file inside a bookmarked
folder needs none of its own — unless that folder is only a file browser place,
which the person may take off again: `~/.ssh` or a key file inside one gets its
own bookmark, and forgetting the place keeps it). A bookmark that does not
resolve at start-up (an external disk or a share that is not there) is kept in
the file without access, so it works again on the next start with the disk
back; picking the path again replaces it.

| What                                  | How it gets access                                                            | Kept across restarts |
| ------------------------------------- | ----------------------------------------------------------------------------- | -------------------- |
| `~/.ssh` (config import, key files)   | `grant_ssh_folder` — folder panel that starts in `~/.ssh`                     | yes                  |
| A key file elsewhere                  | `pick_key_path` — file panel, returns `~/…` or the absolute path for the form | yes                  |
| Key into the vault (`pick_key_file`)  | file panel, read once, copied into the vault                                  | not needed           |
| Folders for the file browser          | `local_pick_folder`, listed by `local_places` until `local_forget_folder`     | yes                  |
| Downloads                             | `files.downloads.read-write`                                                  | always               |
| Exports, backups, keys saved, imports | save/open panel, used right away                                              | not needed           |
| PuTTY/KiTTY sessions folder           | folder panel, used within the dialog                                          | not needed           |

An export written through a save panel cannot have a temporary file next to it
in the sandbox; `backup.rs` then writes the file directly (works, but not
crash-atomic there).

## Building

On a Mac with Xcode and both Rust targets
(`rustup target add aarch64-apple-darwin x86_64-apple-darwin`):

```sh
pnpm build:mas
```

That builds a universal app (Apple Silicon and Intel), checks it — both
architectures, bundle ID `app.uwussh.desktop`, category developer tools, the
encryption flag (`true`), the privacy manifest, the `uwussh://` scheme, no
update feed and no setup name left in the executable — and packs it with
`productbuild` into `target/release/UwUSSH-<version>-mas-universal.pkg`.
Without signing identities in the environment the app and the package stay
unsigned; that is what CI does on every pull request touching the store build.

The version: App Store Connect takes only three numbers, so `0.3.0-beta.2` is
uploaded as `0.3.0`. `MAS_BUILD_NUMBER` becomes `CFBundleVersion` and has to
grow with every upload of the same version (CI uses the run number). A beta
can go to TestFlight; only a version without a later one under the same three
numbers can go to review, so the first store release should be a plain
`0.3.0` or later.

Signing reads these from the environment (the identities must be in a keychain
`codesign` can use):

| Variable                         | What                                                                                  |
| -------------------------------- | ------------------------------------------------------------------------------------- |
| `APPLE_MAS_APP_IDENTITY`         | `3rd Party Mac Developer Application: <Name> (<TEAMID>)` or `Apple Distribution: …`   |
| `APPLE_MAS_INSTALLER_IDENTITY`   | `3rd Party Mac Developer Installer: <Name> (<TEAMID>)`                                |
| `APPLE_TEAM_ID`                  | the ten-character team ID                                                             |
| `APPLE_MAS_PROVISIONING_PROFILE` | path to the Mac App Store provisioning profile → `Contents/embedded.provisionprofile` |
| `MAS_BUILD_NUMBER`               | build number, required for a signed build                                             |

The team ID goes into the entitlements only at signing time
(`com.apple.application-identifier`, `com.apple.developer.team-identifier`), so
nothing team-specific is committed.

### CI

`.github/workflows/mas.yml` runs on tags (beside `installers.yml`, not inside
it), by hand, and on pull requests that touch the store build. `build` makes
the unsigned app and package. `sign` runs only when these repository secrets
exist, on a fresh runner that has built nothing:

| Secret                            | Content                                                                              |
| --------------------------------- | ------------------------------------------------------------------------------------ |
| `APPLE_MAS_CERTIFICATES_P12`      | base64 of one .p12 with both certificates (application and installer) and their keys |
| `APPLE_MAS_CERTIFICATES_PASSWORD` | its password                                                                         |
| `APPLE_MAS_PROVISIONING_PROFILE`  | base64 of the `.provisionprofile`                                                    |
| `APPLE_MAS_APP_IDENTITY`          | as above                                                                             |
| `APPLE_MAS_INSTALLER_IDENTITY`    | as above                                                                             |
| `APPLE_TEAM_ID`                   | as above                                                                             |

The signed package is the run's `mas-signed` artifact. Uploading stays a manual
step, on purpose: a build reaches review because somebody sent it.

`ci.yml` also runs clippy for the `mas` feature set on Linux on every pull
request, so a change that breaks the store build's Rust is noticed even when it
does not touch the paths `mas.yml` watches.

## Publishing, step by step

1. **Apple Developer Program** membership (99 USD/year) at
   developer.apple.com, as an individual or as MinifyX (an organisation needs
   a D-U-N-S number). The seller name shown in the store comes from this.
2. **App ID**: Certificates, Identifiers & Profiles → Identifiers → `+` → App
   IDs → App, platform macOS, explicit bundle ID `app.uwussh.desktop`. No
   capabilities need ticking: sandbox entitlements need no App ID capability.
3. **Certificates** (Certificates → `+`), each from a certificate signing
   request made in Keychain Access:
   - _Mac App Distribution_ (signs the app; "3rd Party Mac Developer
     Application" or "Apple Distribution"),
   - _Mac Installer Distribution_ (signs the package; "3rd Party Mac Developer
     Installer").

   Export both with their private keys into one .p12 for CI.

4. **Provisioning profile**: Profiles → `+` → Distribution → _Mac App Store
   Connect_, App ID `app.uwussh.desktop`, the distribution certificate.
5. **App Store Connect record**: Apps → `+` → New App: macOS, name "UwUSSH"
   (must be free in the store), bundle ID `app.uwussh.desktop`, SKU e.g.
   `uwussh-mac`.
6. **App information**: category _Developer Tools_ (secondary: _Utilities_),
   content rights (no third-party content), age rating questionnaire —
   everything "None"/"No" except that it is an unrestricted way onto other
   computers, not the web: **4+** is expected. Pricing: free.
7. **App Encryption Documentation** (App information → `+`): see
   [Export compliance](#export-compliance) — answer before the first build is
   submitted, and upload the French declaration if the app is offered in
   France.
8. **App Privacy**: privacy policy URL (required even for "no data"), then
   "Do you or your third-party partners collect data from this app?" → **No**
   → **Data Not Collected**. Matches `macos/PrivacyInfo.xcprivacy`; the
   reasoning (sync and AI go where the person points them, never to MinifyX)
   is in that file's comment. Review this if MinifyX ever runs a sync server
   or an AI endpoint _for_ users.
9. **Screenshots**: 16:10, one of 1280×800, 1440×900, 2560×1600, 2880×1800.
   Host list, a terminal (light and dark), the file browser, tunnels, the
   vault, Nyu. Taken from the store build: no local tab, no update hint.
10. **Version page**: description, keywords, support URL (GitHub issues),
    marketing URL, copyright "© 2026 MinifyX", "What's New".
11. **Build and upload**: run the `Mac App Store` workflow (or `pnpm build:mas`
    with the variables above), then upload the signed `.pkg` with
    **Transporter** or:
    ```sh
    xcrun altool --upload-app --type macos \
      --file UwUSSH-0.3.0-mas-universal.pkg \
      --apiKey <KEY_ID> --apiIssuer <ISSUER_ID>
    ```
12. **TestFlight for Mac**: install the processed build and go through
    [Before the first review](#before-the-first-review-needs-a-mac).
13. **Submit for review** with the notes below and a demo server.

### Review notes (paste into "Notes" for App Review)

The reviewer needs something to connect to. MinifyX runs no public SSH server,
so before submitting set up a throwaway one (a small VM or container with a
user that can do nothing harmful, password login, a few files for SFTP) and
fill in the placeholders. Remove it after the review.

> UwUSSH is an SSH and SFTP client for people who manage their own servers.
> It needs no account and has no in-app purchases.
>
> To try it: Hosts → "+" → address `<demo host, e.g. demo.example.com>`, port
> `<22>`, user `<demo user>`, password `<demo password>`. Connect, accept the
> host key, and a terminal opens. The folder icon opens the file browser:
> the server on the right, this Mac on the left (Downloads, plus any folder
> added with "Ordner wählen…"). Tunnels → "+" creates a local port forward.
>
> Network: it connects to the servers the user adds (SSH), optionally to a
> sync server the user runs themselves (UwUSync or UwULock, end-to-end
> encrypted) and, only if the user sets one up with their own API key, to an
> AI provider for the command assistant. It uses the network-server
> entitlement for local port forwarding: a tunnel listens on a local port
> (127.0.0.1 by default) and forwards it through SSH — the standard `ssh -L`.
>
> Keys and passwords stay on the Mac, sealed in a local vault (the master
> password is never sent anywhere); a key file can also stay where it is, and
> the Mac App Store version reads it only after the user picked it or its
> folder (`~/.ssh`) in an open panel. Because of the App Sandbox this version
> has no local shell and no self-updater, and imports `~/.ssh/config` only
> after the user grants access to `~/.ssh`.

### Review risks

- **2.4.5(v) "other code"**: an SSH client runs commands on _other_ machines,
  which is its purpose and is common in the store (Termius, Prompt, Secure
  ShellFish). It runs nothing on the Mac. Say so if asked.
- **network.server**: the most likely question; the note above explains it.
- **4.2 minimum functionality / demo**: a review without a working demo host
  is usually rejected as "unable to review". Keep it reachable for the whole
  review, from the US.
- **5.1.1 accounts**: none required; sync is optional and self-hosted.
- **Export compliance**: see below; the build waits in "Missing Compliance"
  until it is answered.
- **Links to GitHub releases** (About → "Releases"): a link to the source is
  fine; an "update available" message would not be — the store build shows
  none.
- **AI assistant**: the user's own key, the user's chosen provider; nothing
  passes through MinifyX. Mention it rather than have it found.

## App Review Guidelines, point by point

| Guideline                                                  | How UwUSSH meets it                                                                                         |
| ---------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------- |
| 2.4.5(i) sandboxed, correct entitlements                   | `app-sandbox`; network client/server, user-selected files, Downloads, app-scope bookmarks — each justified. |
| 2.4.5(ii) packaged and submitted with Xcode tools          | `productbuild` package signed with the installer certificate, uploaded with Transporter or `altool`.        |
| 2.4.5(iii) self-contained bundle, no installing other code | One executable, no helpers; the frontend is inside the binary. It downloads no code.                        |
| 2.4.5(iv) no auto-launch / login items without consent     | No login item, launch agent or daemon.                                                                      |
| 2.4.5(v) no running other code / scripts on the Mac        | No local shell, no `osascript`, no `scutil`; links open in the browser.                                     |
| 2.4.5(vi) updates only through the Mac App Store           | Updater plugin, feed and setup download are not compiled in (`self-update` off).                            |
| 2.4.5(vii) runs on the current macOS                       | Universal binary, minimum macOS 11.                                                                         |
| 2.4.5(viii) no license screens / own activation            | None; GPL-3.0 is linked from About.                                                                         |
| 2.4.5(ix) no unrelated permissions                         | No camera, microphone, contacts, location, Bluetooth, USB.                                                  |
| 2.1 completeness, 2.3 accurate metadata                    | Screenshots from this build; demo server in the notes.                                                      |
| 4.1 copycats, 5.2 intellectual property                    | Own name, own artwork (Nyu), own code under GPL-3.0.                                                        |
| 5.1.1 privacy policy, 5.1.2 data use                       | Privacy policy URL; nothing collected by MinifyX (`PrivacyInfo.xcprivacy`).                                 |
| 5.2.1 / GPL                                                | MinifyX owns the copyright and may distribute through the store; the source stays public on GitHub.         |

## Export compliance

`ITSAppUsesNonExemptEncryption` is **`true`** in `src-tauri/Info.plist`.

Why: UwUSSH implements encryption itself rather than using macOS's — SSH
(russh on ring: ChaCha20-Poly1305, AES-GCM/CTR, Curve25519, ECDH, Ed25519,
RSA), TLS for sync and the AI providers (rustls on ring), and the vault
(ChaCha20-Poly1305 under an Argon2 key). Apple's exemptions cover encryption
"limited to that within the Apple operating system" and encryption used only
for authentication; SSH encrypts the whole session, so neither applies. All of
it is **standard** (IETF/NIST algorithms), nothing proprietary.

What that means:

- **US (EAR)**: mass-market encryption software, ECCN **5D992.c**, under
  License Exception ENC **§ 740.17(b)(1)**. Since BIS's rule of 29 March 2021
  no self-classification report is due for mass-market software like this,
  and no CCATS is needed. The source is public on GitHub (GPL-3.0), which on
  its own would also take the source out of the EAR (§ 734.3(b)(3), § 742.15(b));
  the app in the store is distributed as a product, so the mass-market path is
  the one to answer with.
- **France**: Apple asks for the French encryption declaration (ANSSI,
  "déclaration de fourniture d'un moyen de cryptologie") for apps with
  standard non-Apple encryption offered in France. Either file it with ANSSI
  and upload it in App Store Connect, or leave France out of the availability
  until it exists.
- **No CCATS**: only proprietary algorithms would need one.

In App Store Connect (App information → App Encryption Documentation, or the
per-build questions), answer:

1. Does your app use encryption? **Yes.**
2. Does it qualify for an exemption (Category 5, Part 2)? **No** — not
   authentication-only, not Apple's OS encryption only.
3. Which algorithms? **Standard encryption algorithms instead of, or in
   addition to, using or accessing the encryption within Apple's operating
   system.**
4. Available in France? Yes → upload the French declaration; or No.

Once Apple approves the documentation it shows a code. Add it to
`src-tauri/Info.plist` as `ITSEncryptionExportComplianceCode` (a string), and
App Store Connect stops asking per build.

Not certain, and worth a second look before the first submission: whether
Apple's current questionnaire still treats the French declaration as required
for standard algorithms (its own reference table says so as of this writing),
and whether ANSSI's simplified declaration covers a free open-source client
distributed by an individual. This is not legal advice.

## Privacy manifest

`macos/PrivacyInfo.xcprivacy`, copied to `Contents/Resources`: no tracking, no
collected data types, and the required-reason APIs file timestamps (3B52.1,
C617.1), system boot time (35F9.1), user defaults (CA92.1) and disk space
(E174.1). The disk-space entry is there because the bundled SQLite calls
`statfs` on Apple systems to choose its locking style — nothing reads free
space, and E174.1 is the nearest listed reason. If App Store Connect's scan
never flags it (it reports missing reasons by e-mail after upload), it can
go; if it flags another API, add that one.

## Sandbox notes and limitations

- **Key files**: a host whose key file is outside every granted folder fails
  with "key unreadable … Operation not permitted". The page should offer
  "Key-Datei wählen…" (`pick_key_path`) or "~/.ssh freigeben…"
  (`grant_ssh_folder`) there. Hosts synced from another computer usually say
  `~/.ssh/id_…`, which works once `~/.ssh` is granted.
- **ssh_config `Include`**: relative includes and `~/…` ones inside `~/.ssh`
  work after the grant; includes elsewhere need that folder granted too.
- **No Termius import** (other app's container and keychain item).
- **File browser**: only Downloads and added folders; `/` is not offered.
  SFTP downloads land there; uploads start there.
- **Device key**: `device.rs` keeps the seal key as the app's own item in the
  login keychain (`keyring`, apple-native). A sandboxed app may create and
  read its own items without `keychain-access-groups`, so none is declared.
  The store build has its own container, so it starts with its own vault and
  key; moving from the DMG means exporting (`.uwussh`) or syncing.
- **Deep links**: `CFBundleURLTypes` comes from `plugins.deep-link` in both
  builds; Launch Services hands `uwussh://connect/<id>` to the running app or
  starts it, and the deep-link plugin forwards it to `links.rs`. The
  single-instance plugin is not registered in the store build (macOS keeps one
  copy of a bundle anyway, and its socket in `/tmp` is outside the sandbox).
- **Sync and TLS**: rustls with the public roots plus the system's trust
  store (`rustls-native-certs` through the Security framework), which the
  sandbox allows.
- **Ollama on another machine of the LAN**: macOS 15's local-network privacy
  prompt applies to the store build as to the DMG; no entitlement is needed.
- **Data**: in the container, see the table at the top. Moving between the two
  builds carries nothing over.

## Before the first review (needs a Mac)

Nothing below could be tried while this was written. On a real Mac, with a
TestFlight or a locally signed build:

- the window comes up, the host list loads, Console shows no sandbox
  violations at start (`log stream --predicate 'sender == "Sandbox"'`);
- connect by password and by vault key; accept a host key; sync with a UwUSync
  or UwULock server; the AI assistant with Ollama on the same Mac;
- grant `~/.ssh`, import `~/.ssh/config`, connect with a host that names
  `~/.ssh/id_ed25519`; quit, start again: still connects (bookmarks.json in
  the container has an entry);
- pick a key file outside `~/.ssh`, restart, connect;
- file browser: Downloads lists, "Ordner wählen…" adds a folder, it is there
  after a restart, download into it, upload from it, rename, delete to Trash;
- a local tunnel on 127.0.0.1 accepts a connection (network.server);
- export and import a `.uwussh` backup, save a key from UwUKeygen;
- `uwussh://connect/<id>` from UwULock opens the host, cold and while running;
- the device name shown on the other devices after pairing is the Mac's name;
- no update button, no local tab, no update request in Console;
- the Dock icon at the size of its neighbours.

## What stays open

- The page parts for the sandbox (buttons for `grant_ssh_folder`,
  `pick_key_path`, `local_pick_folder` / `local_forget_folder`, hiding Termius
  and offering the grant in the import dialog) — the commands exist, typed in
  `apps/desktop/src/lib/flavor.ts`.
- The French encryption declaration, or France left out.
- A demo server for review.
- Membership, certificates, profile, secrets (see above).
- The hardened-runtime entitlements for a Developer-ID-signed DMG: the GitHub
  build is signed ad hoc today, so there is no `macos/Entitlements.plist`; the
  DMG would need none beyond the defaults (WebKit runs JavaScript in its own
  processes, the local shell needs no exception) when it gets one.

## Microsoft Store

Analysis only — nothing is built for it.

**Two ways in.** Partner Center takes either an **MSIX** package or, since
2021 and much improved in 2025, an **unpackaged Win32 installer** (`.exe` or
`.msi`) that the Store downloads from a versioned HTTPS URL of ours.

|                   | Win32 installer (the existing setup)                                                                                                   | MSIX                                                                                                                        |
| ----------------- | -------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------- |
| What we ship      | `UwUSSH-Setup-<version>.exe` as it is, hosted on GitHub releases                                                                       | a new package: Tauri has no MSIX target; `makeappx` with an `AppxManifest.xml`, or the MSIX Packaging Tool on the setup     |
| Signing           | ours: the installer must be signed with a certificate from a trusted CA (Authenticode) — **the release has no such certificate today** | the Store signs; no certificate needed                                                                                      |
| Silent install    | required (`/S` or similar); the setup's `--update` mode is close                                                                       | n/a                                                                                                                         |
| Updates           | ours — the updater may stay, or the Store can now offer updates from our URL; one mechanism should be chosen                           | the Store's; the updater must be off (`--no-default-features`, like the Mac App Store build)                                |
| Package identity  | none                                                                                                                                   | yes; app data under the package's virtualised `AppData`, registry writes to `HKCU` virtualised                              |
| Capabilities      | n/a                                                                                                                                    | `runFullTrust` (a Win32 desktop app); `internetClient`, `internetClientServer` and `privateNetworkClientServer` for tunnels |
| `uwussh://` links | the setup writes `HKCU\Software\Classes\uwussh` as today                                                                               | a `windows.protocol` extension in the manifest; the deep-link plugin's own registry registration has no effect there        |
| Single instance   | as today (named mutex + window message)                                                                                                | works the same: a full-trust packaged app keeps Win32 semantics; a link starts or reaches the one instance                  |
| WebView2          | the setup brings the bootstrapper                                                                                                      | the Evergreen runtime is part of Windows 11 and current Windows 10; the manifest cannot run the bootstrapper                |
| DPAPI device key  | as today                                                                                                                               | works; the key is tied to the user, not the package                                                                         |
| SMB shares, PuTTY | as today                                                                                                                               | as today (full trust); PuTTY's registry is read from the real `HKCU`                                                        |

**Recommendation:** the Win32 route, once there is an Authenticode certificate
(which would also end SmartScreen warnings for the GitHub setup). It needs no
second package, keeps one update path, and the Store listing is mostly
metadata. MSIX only if the Store-managed updates or the Store's signing
matter more than a second build pipeline.

**What CI would need.** Win32: sign the setup (Azure Trusted Signing or a
certificate in a hardware token / cloud HSM), keep the versioned download URL
stable, then a Partner Center submission per release — by hand or through the
Microsoft Store Submission API (`msstore` CLI) with an Entra app's credentials
as secrets. MSIX: a `--no-default-features` Windows build, an
`AppxManifest.xml` (identity name and publisher from Partner Center, the
protocol extension, capabilities), `makeappx pack` for x64 and ARM64 (and
`makeappx bundle`), no signing (the Store does it; a test-signed copy for
local installs), then the same submission step.

**Cost.** Registration is free: Microsoft dropped the fee for individual
developers (June 2025, globally since September 2025) and for company
accounts (May 2026). An Authenticode certificate for the Win32 route costs
roughly 100–400 EUR a year (Azure Trusted Signing is about 10 USD a month,
where it is offered to the account's country).

**Effort.** Win32 route: about a day for the listing, privacy policy, age
rating and first submission, plus whatever obtaining the signing certificate
takes. MSIX route: two to four days for the manifest, packaging in CI, the
updater-off build and testing links, single instance and SMB under package
identity on a real Windows machine — then the submission as above.
