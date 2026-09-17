# Installing UwUSSH

[Deutsch weiter unten](#uwussh-installieren)

UwUSSH is in beta. It runs on **Windows 10 and 11 (64-bit)**; macOS and Linux
come later. It installs for your Windows user only — no admin rights — and
takes about 25 MB. The installer speaks English on an English Windows; the app
itself is German for now, so menu names below come with their German label.

## 1. Download

1. Open the [releases](https://github.com/MinifyX/UwUSSH-Client/releases).
2. Take the newest one at the top. Right now every version is a beta, so it is
   marked **Pre-release** — that's expected.
3. Under **Assets**, download `UwUSSH-Setup-<version>.exe`.

Your browser may say the file is "not commonly downloaded". Keep it anyway
(in Edge: `…` → **Keep** → **Show more** → **Keep anyway**).

**Checking the download (optional).** Each release lists the setup's SHA-256.
In PowerShell:

```powershell
Get-FileHash "$env:USERPROFILE\Downloads\UwUSSH-Setup-<version>.exe"
```

The hash shown must match the one on the release page.

## 2. Run the setup

Double-click the file. Windows will most likely show **"Windows protected your
PC"**: the setup isn't signed with a paid code-signing certificate, so
SmartScreen doesn't know it yet. Click **More info**, then **Run anyway**.

Nyu opens the installer:

- **Install** sets everything up in a few seconds.
- **Options** lets you change the folder (default
  `%LOCALAPPDATA%\Programs\UwUSSH`), turn off the desktop shortcut, or leave out
  **UwUKeygen**, the SSH key generator that comes along.
- If Microsoft Edge WebView2 is missing (Windows 11 always has it), the setup
  offers to download and install it.

Then start UwUSSH from the setup, the Start menu or the desktop.

## 3. First steps

- **Add a host** with `+` in the sidebar (Host hinzufügen): address, user,
  and a password or a key.
- **Or bring your hosts along**: the import button next to it (Importieren)
  reads Termius, PuTTY, KiTTY and `~/.ssh/config`.
- **Passwords and keys** go into an encrypted vault (Tresor). The first time
  you save one, you pick a master password. Tick **Auf diesem Gerät merken**
  (remember on this device) if your Windows account should open the vault on
  its own.
- On first contact with a server you are shown its host key fingerprint. Trust
  it only if it is the one you expect.

## Updates

UwUSSH updates itself: about 20 seconds after it starts, and every six hours,
it looks for a newer version, downloads it quietly (signed and checked) and
offers a restart. **Settings → Updates** (Einstellungen → Updates) switches
between the Beta and Stable (Stabil) channels. As long as there are only betas, stay on Beta.

A newer setup can also simply be run over an installed UwUSSH. Hosts, the vault
and settings stay.

## Uninstalling

**Windows Settings → Apps → Installed apps → UwUSSH → Uninstall.** The
uninstaller asks whether to keep your hosts, vault and settings, in case you
come back. Without them, the vault with every saved password and key is
deleted too.

## Where your data lives

| What                                        | Where                                |
| ------------------------------------------- | ------------------------------------ |
| Hosts, groups, trusted host keys, the vault | `%APPDATA%\app.uwussh.desktop\`      |
| App settings (look, terminal, highlighting) | `%LOCALAPPDATA%\app.uwussh.desktop\` |
| The program                                 | `%LOCALAPPDATA%\Programs\UwUSSH\`    |

Passwords and private keys are only stored encrypted. To move to another
computer: **Settings → Import & Export** (Einstellungen → Import & Export)
writes everything into one `.uwussh`
file, sealed with a password of its own, which UwUSSH on the other computer
reads back in.

## If something goes wrong

- **"WebView2 couldn't be installed"**: install the Evergreen WebView2 Runtime
  from [Microsoft](https://developer.microsoft.com/microsoft-edge/webview2/)
  and run the setup again.
- **An antivirus program blocks the setup**: that is the same missing
  certificate as SmartScreen's warning. The source of every release is in this
  repository, and the checksum above tells you the file is the published one.
- **Windows on ARM** hasn't been tried.
- Something else? [Open an issue](https://github.com/MinifyX/UwUSSH-Client/issues)
  — no promises on how fast, see the README.

Building it yourself instead: [Development](../README.md#development).

---

# UwUSSH installieren

UwUSSH ist in der Beta. Es läuft unter **Windows 10 und 11 (64 Bit)**; macOS und
Linux kommen später. Es wird nur für deinen Windows-Benutzer installiert — ohne
Adminrechte — und braucht etwa 25 MB.

## 1. Herunterladen

1. Öffne die [Releases](https://github.com/MinifyX/UwUSSH-Client/releases).
2. Nimm das neueste ganz oben. Im Moment ist jede Version eine Beta und deshalb
   als **Pre-release** markiert — das ist so gewollt.
3. Lade unter **Assets** die Datei `UwUSSH-Setup-<Version>.exe` herunter.

Der Browser meldet vielleicht, die Datei werde „nicht häufig heruntergeladen“.
Behalte sie trotzdem (in Edge: `…` → **Beibehalten** → **Mehr anzeigen** →
**Trotzdem beibehalten**).

**Download prüfen (optional).** Jedes Release nennt die SHA-256-Prüfsumme des
Setups. In PowerShell:

```powershell
Get-FileHash "$env:USERPROFILE\Downloads\UwUSSH-Setup-<Version>.exe"
```

Der angezeigte Hash muss mit dem auf der Release-Seite übereinstimmen.

## 2. Setup starten

Doppelklick auf die Datei. Windows zeigt sehr wahrscheinlich **„Der Computer
wurde durch Windows geschützt“**: Das Setup ist nicht mit einem kostenpflichtigen
Code-Signing-Zertifikat signiert, deshalb kennt SmartScreen es noch nicht. Klick
auf **Weitere Informationen**, dann auf **Trotzdem ausführen**.

Nyu öffnet den Installer:

- **Installieren** richtet alles in ein paar Sekunden ein.
- Unter **Optionen** änderst du den Ordner (Standard
  `%LOCALAPPDATA%\Programs\UwUSSH`), schaltest die Desktop-Verknüpfung ab oder
  lässt **UwUKeygen** weg, den SSH-Schlüssel-Generator, der mitkommt.
- Fehlt Microsoft Edge WebView2 (Windows 11 hat es immer), bietet das Setup an,
  es herunterzuladen und zu installieren.

Danach startest du UwUSSH aus dem Setup, dem Startmenü oder vom Desktop.

## 3. Erste Schritte

- **Host anlegen** mit `+` in der Seitenleiste: Adresse, Benutzer und ein
  Passwort oder ein Key.
- **Oder Hosts mitbringen**: Der Import-Knopf daneben liest Termius, PuTTY,
  KiTTY und `~/.ssh/config`.
- **Passwörter und Keys** landen in einem verschlüsselten Tresor. Beim ersten
  Speichern legst du ein Master-Passwort fest. Mit „Auf diesem Gerät merken“
  öffnet dein Windows-Konto den Tresor von selbst.
- Beim ersten Kontakt mit einem Server siehst du den Fingerprint seines
  Host-Keys. Vertrau ihm nur, wenn es der erwartete ist.

## Updates

UwUSSH aktualisiert sich selbst: etwa 20 Sekunden nach dem Start und danach
alle sechs Stunden sucht es nach einer neuen Version, lädt sie still herunter
(signiert und geprüft) und bietet einen Neustart an. **Einstellungen → Updates**
wechselt zwischen den Kanälen Beta und Stabil. Solange es nur Betas gibt, bleib
auf Beta.

Ein neueres Setup kann auch einfach über ein installiertes UwUSSH laufen. Hosts,
Tresor und Einstellungen bleiben.

## Deinstallieren

**Windows-Einstellungen → Apps → Installierte Apps → UwUSSH → Deinstallieren.**
Die Deinstallation fragt, ob Hosts, Tresor und Einstellungen bleiben sollen,
falls du wiederkommst. Ohne sie wird auch der Tresor mit allen gespeicherten
Passwörtern und Keys gelöscht.

## Wo deine Daten liegen

| Was                                                  | Wo                                   |
| ---------------------------------------------------- | ------------------------------------ |
| Hosts, Gruppen, vertraute Host-Keys, der Tresor      | `%APPDATA%\app.uwussh.desktop\`      |
| App-Einstellungen (Aussehen, Terminal, Hervorhebung) | `%LOCALAPPDATA%\app.uwussh.desktop\` |
| Das Programm                                         | `%LOCALAPPDATA%\Programs\UwUSSH\`    |

Passwörter und private Keys werden nur verschlüsselt gespeichert. Für einen
Umzug auf einen anderen Rechner: **Einstellungen → Import & Export** schreibt
alles in eine `.uwussh`-Datei, versiegelt mit einem eigenen Passwort, die UwUSSH
auf dem anderen Rechner wieder einliest.

## Wenn etwas nicht klappt

- **„WebView2 couldn't be installed“**: Installiere die Evergreen WebView2
  Runtime von [Microsoft](https://developer.microsoft.com/microsoft-edge/webview2/)
  und starte das Setup noch einmal.
- **Ein Virenscanner blockiert das Setup**: Das ist dasselbe fehlende Zertifikat
  wie bei SmartScreen. Der Quellcode jedes Releases liegt in diesem Repository,
  und die Prüfsumme oben zeigt dir, dass die Datei die veröffentlichte ist.
- **Windows auf ARM** ist nicht ausprobiert.
- Etwas anderes? [Issue aufmachen](https://github.com/MinifyX/UwUSSH-Client/issues)
  — ohne Versprechen, wie schnell, siehe README.
