# Installing UwUSSH

[Deutsch weiter unten](#uwussh-installieren)

UwUSSH is in beta. It runs on **Windows 10 and 11 (64-bit)**, **macOS 11 or
newer** (Apple silicon and Intel) and **Linux** (x86_64). The setup installs for
your user only — no admin rights. UwUSSH speaks English or German, following
the system; **Settings → Appearance → Language** switches.

Every system gets the same setup with Nyu in it. Download it from the
[releases](https://github.com/MinifyX/UwUSSH-Client/releases): take the newest
one at the top. Right now every version is a beta, so it is marked
**Pre-release** — that's expected.

| System                   | File under **Assets**                        |
| ------------------------ | -------------------------------------------- |
| Windows 10/11            | `UwUSSH-Setup-<version>.exe`                 |
| macOS, Apple silicon (M) | `UwUSSH-Setup-<version>-macos-arm64.dmg`     |
| macOS, Intel             | `UwUSSH-Setup-<version>-macos-x64.dmg`       |
| Linux, x86_64            | `UwUSSH-Setup-<version>-linux-x64.AppImage`  |
| Linux, as a package      | `UwUSSH-<version>-linux-x64.deb` (see below) |

**Checking the download (optional).** Each release has a `SHA256SUMS.txt`. On
macOS and Linux: `shasum -a 256 -c SHA256SUMS.txt --ignore-missing` in the
download folder. On Windows, in PowerShell:
`Get-FileHash "$env:USERPROFILE\Downloads\UwUSSH-Setup-<version>.exe"` and
compare with the line in the file.

## Windows

Double-click the setup. Windows will most likely show **"Windows protected your
PC"**: the setup isn't signed with a paid code-signing certificate, so
SmartScreen doesn't know it yet. Click **More info**, then **Run anyway**. Your
browser may also say the file is "not commonly downloaded"; keep it anyway (in
Edge: `…` → **Keep** → **Show more** → **Keep anyway**).

- **Install** sets everything up in a few seconds.
- **Options** lets you change the folder (default
  `%LOCALAPPDATA%\Programs\UwUSSH`), turn off the desktop shortcut, or leave out
  **UwUKeygen**, the SSH key generator that comes along.
- If Microsoft Edge WebView2 is missing (Windows 11 always has it), the setup
  offers to download and install it.

Uninstall from **Windows Settings → Apps → Installed apps → UwUSSH**.

## macOS

Open the `.dmg` and double-click **UwUSSH Setup**. UwUSSH isn't notarized by
Apple (that needs a paid developer account), so the first time macOS says it
can't check the app. Then:

1. Open **System Settings → Privacy & Security**.
2. Scroll down: next to "UwUSSH Setup was blocked", click **Open Anyway** and
   confirm.

(On macOS 14 and older, right-clicking the setup and choosing **Open** works
too.) The setup installs **UwUSSH** and **UwUKeygen** into `/Applications`, or
into `~/Applications` if your user may not write to `/Applications`. The apps
it installs start without that question.

To uninstall, run the setup again and choose **Uninstall …** — it asks whether
to keep your hosts and vault. Dragging the apps to the Trash works too, but
leaves the data in `~/Library/Application Support/app.uwussh.desktop`.

## Linux

```bash
chmod +x UwUSSH-Setup-*-linux-x64.AppImage
./UwUSSH-Setup-*-linux-x64.AppImage
```

It installs into `~/.local/share/uwussh`, with **UwUSSH** and **UwUKeygen** in
the application menu and, if you like, on the desktop. It brings its own WebKit,
so nothing needs installing first. If the AppImage won't start because FUSE is
missing (Ubuntu 22.04 and newer lack `libfuse2`), start it with
`./UwUSSH-Setup-*.AppImage --appimage-extract-and-run`. The installed app runs
unpacked and needs no FUSE at all.

To uninstall, run the setup again and choose **Uninstall …**.

**Rather have a package?** The `.deb` installs the app alone through apt
(`sudo apt install ./UwUSSH-<version>-linux-x64.deb`), using the system's
WebKitGTK 4.1. It has no setup and no automatic updates: install the next
`.deb` to update.

**Remembering the vault on Linux** uses the Secret Service (GNOME Keyring,
KWallet). Without one — a bare window manager — UwUSSH keeps its key in a file
only your user can read, which protects less against someone with your disk.

## First steps

- **Add a host** with `+` in the sidebar: address, user, and a password or a
  key.
- **Or bring your hosts along**: the import button next to it reads Termius,
  PuTTY, KiTTY (from the registry, or a portable KiTTY's `Sessions` folder and
  `.reg` exports) and `~/.ssh/config`.
- **Passwords and keys** go into an encrypted vault. The first time you save
  one, you pick a master password. Tick "remember on this device" if your
  account should open the vault on its own.
- On first contact with a server you are shown its host key fingerprint. Trust
  it only if it is the one you expect.
- A click on a host opens a connection in a tab — or shows the tab that is
  already open. Right-click the host for **Open another tab**.
- Nothing opens on its own when UwUSSH starts. **Settings → Terminal → Open on
  start** can open a local shell or chosen hosts instead.
- **Several computers?** Settings → Sync connects a
  [UwUSSH server](https://github.com/MinifyX/UwUSSH-Server) of your own, and
  keeps hosts, keys and passwords the same everywhere, end-to-end encrypted.

## Updates

UwUSSH updates itself: about 20 seconds after it starts, and every six hours,
it looks for a newer version, downloads it quietly (signed and checked) and
offers a restart. **Settings → Updates** switches between the Beta and Stable
channels. As long as there are only betas, stay on Beta.

A newer setup can also simply be run over an installed UwUSSH. Hosts, the vault
and settings stay.

## Where your data lives

| What                                | Windows                              | macOS                                              | Linux                               |
| ----------------------------------- | ------------------------------------ | -------------------------------------------------- | ----------------------------------- |
| Hosts, groups, host keys, the vault | `%APPDATA%\app.uwussh.desktop\`      | `~/Library/Application Support/app.uwussh.desktop` | `~/.local/share/app.uwussh.desktop` |
| App settings (look, terminal, …)    | `%LOCALAPPDATA%\app.uwussh.desktop\` | `~/Library/WebKit/app.uwussh.desktop`              | `~/.local/share/app.uwussh.desktop` |
| The program                         | `%LOCALAPPDATA%\Programs\UwUSSH\`    | `/Applications/UwUSSH.app`                         | `~/.local/share/uwussh`             |

Passwords and private keys are only stored encrypted. To move to another
computer without a sync server: **Settings → Import & Export** writes everything
into one `.uwussh` file, sealed with a password of its own, which UwUSSH on the
other computer reads back in.

## If something goes wrong

- **"WebView2 couldn't be installed"** (Windows): install the Evergreen WebView2
  Runtime from [Microsoft](https://developer.microsoft.com/microsoft-edge/webview2/)
  and run the setup again.
- **An antivirus program blocks the setup**: that is the same missing
  certificate as SmartScreen's warning. The source of every release is in this
  repository, and the checksums tell you the file is the published one.
- **macOS says the app is damaged**: that happens when the quarantine mark
  survives on the installed app, which the setup avoids. Running
  `xattr -dr com.apple.quarantine /Applications/UwUSSH.app` clears it.
- **Windows on ARM and Linux on ARM** haven't been built yet.
- Something else? [Open an issue](https://github.com/MinifyX/UwUSSH-Client/issues)
  — no promises on how fast, see the README.

Building it yourself instead: [Development](../README.md#development).

---

# UwUSSH installieren

UwUSSH ist in der Beta. Es läuft unter **Windows 10 und 11 (64 Bit)**, **macOS
11 oder neuer** (Apple-Chip und Intel) und **Linux** (x86_64). Das Setup
installiert nur für deinen Benutzer — ohne Adminrechte. UwUSSH spricht Deutsch
oder Englisch, je nach System; **Einstellungen → Darstellung → Sprache**
schaltet um.

Jedes System bekommt dasselbe Setup mit Nyu. Lade es von den
[Releases](https://github.com/MinifyX/UwUSSH-Client/releases) herunter: das
neueste ganz oben. Im Moment ist jede Version eine Beta und deshalb als
**Pre-release** markiert — das ist so gewollt.

| System                | Datei unter **Assets**                         |
| --------------------- | ---------------------------------------------- |
| Windows 10/11         | `UwUSSH-Setup-<Version>.exe`                   |
| macOS, Apple-Chip (M) | `UwUSSH-Setup-<Version>-macos-arm64.dmg`       |
| macOS, Intel          | `UwUSSH-Setup-<Version>-macos-x64.dmg`         |
| Linux, x86_64         | `UwUSSH-Setup-<Version>-linux-x64.AppImage`    |
| Linux, als Paket      | `UwUSSH-<Version>-linux-x64.deb` (siehe unten) |

**Download prüfen (optional).** Jedes Release hat eine `SHA256SUMS.txt`. Unter
macOS und Linux im Download-Ordner: `shasum -a 256 -c SHA256SUMS.txt
--ignore-missing`. Unter Windows in PowerShell:
`Get-FileHash "$env:USERPROFILE\Downloads\UwUSSH-Setup-<Version>.exe"` und mit
der Zeile in der Datei vergleichen.

## Windows

Doppelklick auf das Setup. Windows zeigt sehr wahrscheinlich **„Der Computer
wurde durch Windows geschützt“**: Das Setup ist nicht mit einem
kostenpflichtigen Code-Signing-Zertifikat signiert, deshalb kennt SmartScreen es
noch nicht. Klick auf **Weitere Informationen**, dann auf **Trotzdem
ausführen**. Der Browser meldet vielleicht, die Datei werde „nicht häufig
heruntergeladen“; behalte sie trotzdem (in Edge: `…` → **Beibehalten** → **Mehr
anzeigen** → **Trotzdem beibehalten**).

- **Installieren** richtet alles in ein paar Sekunden ein.
- Unter **Optionen** änderst du den Ordner (Standard
  `%LOCALAPPDATA%\Programs\UwUSSH`), schaltest die Desktop-Verknüpfung ab oder
  lässt **UwUKeygen** weg, den SSH-Schlüssel-Generator, der mitkommt.
- Fehlt Microsoft Edge WebView2 (Windows 11 hat es immer), bietet das Setup an,
  es herunterzuladen und zu installieren.

Deinstallieren über **Windows-Einstellungen → Apps → Installierte Apps →
UwUSSH**.

## macOS

Die `.dmg` öffnen und **UwUSSH Setup** doppelklicken. UwUSSH ist nicht bei
Apple notarisiert (das braucht einen kostenpflichtigen Entwickler-Account),
deshalb sagt macOS beim ersten Mal, es könne die App nicht prüfen. Dann:

1. **Systemeinstellungen → Datenschutz & Sicherheit** öffnen.
2. Nach unten scrollen: neben „UwUSSH Setup wurde blockiert“ auf **Trotzdem
   öffnen** klicken und bestätigen.

(Unter macOS 14 und älter geht auch Rechtsklick auf das Setup → **Öffnen**.) Das
Setup installiert **UwUSSH** und **UwUKeygen** nach `/Applications`, oder nach
`~/Applications`, wenn dein Benutzer nicht in `/Applications` schreiben darf.
Die installierten Apps starten ohne diese Rückfrage.

Deinstallieren: das Setup noch einmal starten und **Deinstallieren …** wählen —
es fragt, ob Hosts und Tresor bleiben sollen. Die Apps in den Papierkorb ziehen
geht auch, lässt aber die Daten in
`~/Library/Application Support/app.uwussh.desktop` liegen.

## Linux

```bash
chmod +x UwUSSH-Setup-*-linux-x64.AppImage
./UwUSSH-Setup-*-linux-x64.AppImage
```

Es installiert nach `~/.local/share/uwussh`, mit **UwUSSH** und **UwUKeygen** im
Anwendungsmenü und auf Wunsch auf dem Schreibtisch. Es bringt sein eigenes
WebKit mit, vorher muss nichts installiert werden. Startet das AppImage nicht,
weil FUSE fehlt (Ubuntu ab 22.04 hat kein `libfuse2`), dann mit
`./UwUSSH-Setup-*.AppImage --appimage-extract-and-run`. Die installierte App
läuft entpackt und braucht gar kein FUSE.

Deinstallieren: das Setup noch einmal starten und **Deinstallieren …** wählen.

**Lieber ein Paket?** Die `.deb` installiert nur die App über apt
(`sudo apt install ./UwUSSH-<Version>-linux-x64.deb`) und nutzt das
WebKitGTK 4.1 des Systems. Sie hat kein Setup und keine automatischen Updates:
zum Aktualisieren die nächste `.deb` installieren.

**Tresor merken unter Linux** nutzt den Secret Service (GNOME Keyring, KWallet).
Ohne einen — etwa unter einem reinen Fenstermanager — legt UwUSSH seinen
Schlüssel in eine Datei, die nur dein Benutzer lesen kann; das schützt weniger
gegen jemanden mit deiner Festplatte.

## Erste Schritte

- **Host anlegen** mit `+` in der Seitenleiste: Adresse, Benutzer und ein
  Passwort oder ein Key.
- **Oder Hosts mitbringen**: Der Import-Knopf daneben liest Termius, PuTTY,
  KiTTY (aus der Registry oder aus dem `Sessions`-Ordner eines portablen KiTTY
  und aus `.reg`-Exporten) und `~/.ssh/config`.
- **Passwörter und Keys** landen in einem verschlüsselten Tresor. Beim ersten
  Speichern legst du ein Master-Passwort fest. Mit „Auf diesem Gerät merken“
  öffnet dein Benutzerkonto den Tresor von selbst.
- Beim ersten Kontakt mit einem Server siehst du den Fingerprint seines
  Host-Keys. Vertrau ihm nur, wenn es der erwartete ist.
- Ein Klick auf einen Host öffnet eine Verbindung in einem Tab — oder zeigt den
  Tab, der schon offen ist. Rechtsklick auf den Host → **Weiteren Tab öffnen**.
- Beim Start öffnet UwUSSH nichts von selbst. **Einstellungen → Terminal → Beim
  Start öffnen** kann stattdessen eine lokale Shell oder bestimmte Hosts öffnen.
- **Mehrere Rechner?** Einstellungen → Sync verbindet einen eigenen
  [UwUSSH-Server](https://github.com/MinifyX/UwUSSH-Server) und hält Hosts, Keys
  und Passwörter überall gleich, Ende-zu-Ende-verschlüsselt.

## Updates

UwUSSH aktualisiert sich selbst: etwa 20 Sekunden nach dem Start und danach
alle sechs Stunden sucht es nach einer neuen Version, lädt sie still herunter
(signiert und geprüft) und bietet einen Neustart an. **Einstellungen → Updates**
wechselt zwischen den Kanälen Beta und Stabil. Solange es nur Betas gibt, bleib
auf Beta.

Ein neueres Setup kann auch einfach über ein installiertes UwUSSH laufen. Hosts,
Tresor und Einstellungen bleiben.

## Wo deine Daten liegen

| Was                               | Windows                              | macOS                                              | Linux                               |
| --------------------------------- | ------------------------------------ | -------------------------------------------------- | ----------------------------------- |
| Hosts, Gruppen, Host-Keys, Tresor | `%APPDATA%\app.uwussh.desktop\`      | `~/Library/Application Support/app.uwussh.desktop` | `~/.local/share/app.uwussh.desktop` |
| App-Einstellungen (Aussehen, …)   | `%LOCALAPPDATA%\app.uwussh.desktop\` | `~/Library/WebKit/app.uwussh.desktop`              | `~/.local/share/app.uwussh.desktop` |
| Das Programm                      | `%LOCALAPPDATA%\Programs\UwUSSH\`    | `/Applications/UwUSSH.app`                         | `~/.local/share/uwussh`             |

Passwörter und private Keys werden nur verschlüsselt gespeichert. Für einen
Umzug ohne Sync-Server: **Einstellungen → Import & Export** schreibt alles in
eine `.uwussh`-Datei, versiegelt mit einem eigenen Passwort, die UwUSSH auf dem
anderen Rechner wieder einliest.

## Wenn etwas nicht klappt

- **„WebView2 couldn't be installed“** (Windows): Installiere die Evergreen
  WebView2 Runtime von
  [Microsoft](https://developer.microsoft.com/microsoft-edge/webview2/) und
  starte das Setup noch einmal.
- **Ein Virenscanner blockiert das Setup**: Das ist dasselbe fehlende Zertifikat
  wie bei SmartScreen. Der Quellcode jedes Releases liegt in diesem Repository,
  und die Prüfsummen zeigen dir, dass die Datei die veröffentlichte ist.
- **macOS sagt, die App sei beschädigt**: Das passiert, wenn die
  Quarantäne-Markierung an der installierten App hängen bleibt, was das Setup
  vermeidet. `xattr -dr com.apple.quarantine /Applications/UwUSSH.app` entfernt
  sie.
- **Windows und Linux auf ARM** sind noch nicht gebaut.
- Etwas anderes? [Issue aufmachen](https://github.com/MinifyX/UwUSSH-Client/issues)
  — ohne Versprechen, wie schnell, siehe README.
