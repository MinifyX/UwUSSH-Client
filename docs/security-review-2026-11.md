# Security-Review UwUSSH-Client – Redesign (2026-11)

- Branch: `feat/redesign` (Worktree `wt/ssh-redesign`), HEAD `baab1de`
- Bereich: `v0.4.0-beta.1...HEAD` (8 Commits, 35 Dateien, ohne `pnpm-lock.yaml`)
- Nur das Frontend `apps/desktop/src` hat sich geändert, dazu Doku/i18n/CSS und der Design-Bump (1.6.1 → 1.11.0) in den drei `package.json`.
- **In diesem Bereich keine Änderungen an** `crates/*` (Host-Key-Prüfung, Vault, Sync, Import, Keygen), `apps/*/src-tauri` und Updater/Installer.
- **Der UwUAuth-SSO-Commit liegt nicht in diesem Bereich.** Callback, State/PKCE und Token-Ablage sind deshalb hier nicht geprüft und brauchen ein eigenes Review, sobald der Commit auf dem Branch ist.

Ergebnis: kein Befund mit Schwere Kritisch, Hoch oder Mittel. Es gibt 3 Befunde mit Niedrig und 2 mit Info.

## Befunde

### N1 – Niedrig: Quick-Connect wertet einen passenden Host-Namen höher als die eingetippte Adresse

- **Ort:** `apps/desktop/src/lib/home.ts:46-50` (`matchQuickConnect`), aufgerufen aus `apps/desktop/src/components/Home.tsx:108-111`
- **Ursache:** Eine Eingabe ohne Benutzer und Port wird zuerst mit `host.name` verglichen. Passt genau ein Name, wird dieser Host sofort verbunden, auch wenn ein anderer gespeicherter Host genau die eingetippte Adresse hat oder die Eingabe selbst wie eine Adresse aussieht.
- **Auswirkung:** Host-Namen kommen auch aus dem Sync und aus Importen. Heißt ein Host so wie die Adresse eines anderen Servers (z. B. ein Name in der Form einer IP oder eines FQDN), dann verbindet die Eingabe dieser Adresse still mit einem anderen Ziel. Dabei werden die Zugangsdaten und der bekannte Host-Key **dieses** Eintrags benutzt. Es gibt dann keine TOFU-Warnung, weil der Key zum Eintrag passt. Wer danach im Terminal ein Passwort eintippt, gibt es dem falschen Server.
- **Fix:** Der Name soll nur dann zählen, wenn kein Host diese Adresse hat. Alternativ gilt die Eingabe als mehrdeutig (`null`, dann öffnet sich die HostForm), wenn Name-Treffer und Adress-Treffer verschiedene Hosts sind. Dazu einen Test in `test/home.test.ts` ergänzen.

### N2 – Niedrig: Wird das Dateien-Panel geschlossen, während es sich verbindet, bleibt die SFTP-Verbindung offen

- **Ort:** `apps/desktop/src/App.tsx:774-792` (`openPanelFiles`). Dasselbe Muster gab es schon vorher in `openFilesIn`, `App.tsx:752-771`.
- **Ursache:** Wird das Panel geschlossen, während `openFiles` läuft, liefert `negotiate` trotzdem `ok`. Danach gibt `if (!panelOpen(id)) return null;` nur `null` zurück und ruft nie `closeFiles(result.value.session)` auf. Der `FileBrowser` ist zu diesem Zeitpunkt schon unmountet und kann die Session auch nicht mehr schließen.
- **Auswirkung:** Es bleibt eine angemeldete SFTP-Session übrig, die nie benutzt wird. Wurde mit Root/sudo geöffnet, ist sie erhöht. Sie lebt bis zum Ende der App oder der Verbindung. Mit dem Panel-Toggle (Taste/Toolbar) passiert das leicht.
- **Fix:** Vor dem `return null` bei `result.ok` die Session schließen: `void closeFiles(result.value.session).catch(() => undefined)`. Dasselbe in `openFilesIn` einbauen.

### N3 – Niedrig: In der Befehlspalette schickt Enter die Suchanfrage an den KI-Anbieter, wenn sonst nichts passt

- **Ort:** `apps/desktop/src/components/Palette.tsx:161-181`, dazu `AssistPopup.tsx:126-128` (fragt beim Öffnen sofort an)
- **Ursache:** Der Eintrag „… als Befehl für …“ erscheint bei jeder Eingabe, wenn ein Terminal im Vordergrund ist. Passt sonst kein Host, Tab oder Befehl, ist er der einzige Eintrag und damit vorausgewählt. Ein Enter öffnet dann den Assistenten, und der sendet den Text sofort an den eingestellten Anbieter. Das kann ein Cloud-Anbieter sein.
- **Auswirkung:** Datenschutz: Was in die Palette getippt wurde (ein vertippter Host-Name, eine interne Adresse oder versehentlich eingefügter Text), kann ungewollt beim KI-Anbieter landen. **Ausgeführt wird dabei nichts** (siehe unten).
- **Fix:** Beim Weg über die Palette nur das Feld im Assistenten vorbefüllen und erst nach einem eigenen Enter fragen. Alternativ den Eintrag nie automatisch vorauswählen.

### I1 – Info: Der Design-Tarball wird ohne Integritäts-Hash bezogen

- **Ort:** `apps/{desktop,setup,keygen}/package.json` (`@uwusuite/design` als Release-URL), `pnpm-lock.yaml:766-768`. Dort steht nur `resolution: {tarball: …}` und kein `integrity`.
- **Ursache:** pnpm speichert für Tarball-URLs keinen Hash. GitHub-Release-Assets lassen sich nachträglich austauschen.
- **Auswirkung:** Wird das Release-Asset ausgetauscht (z. B. bei kompromittiertem Repo oder Token), landet der neue Code ohne Warnung im nächsten Build. Dieser Code läuft im WebView, der Zugriff auf die Tauri-IPC hat. Das Problem bestand schon vorher; dieser Bump hat es nur weitergetragen.
- **Fix (später, suite-weit):** In CI den Tarball gegen eine SHA-256 prüfen, die im Repo liegt. Alternativ das Paket aus einer Registry beziehen, die `integrity` liefert.

### I2 – Info: Downloads aus dem Panel landen direkt im Benutzerordner

- **Ort:** `apps/desktop/src/components/FileBrowser.tsx:53,191-192` (compact: die lokale Seite ist der Benutzerordner)
- **Ursache:** Das Ziel ist `~` statt eines Download-Ordners. Ein entfernter Ordner, der so heißt wie ein Ordner in `~` (`.config`, `.ssh` usw.), wird nach einem bestätigten „Ersetzen“ in den vorhandenen Ordner gemischt (`sftp.rs` merge bei `overwrite`).
- **Auswirkung:** Pfad-Traversal ist **nicht** möglich (siehe unten). Bestätigt der Nutzer aber „Ersetzen“ für einen Punkt-Ordner, kann ein bösartiger Server Dateien in Konfigurations- und Autostart-Ordner legen.
- **Fix (optional):** Als Ziel den Downloads-Ordner (`place.kind === 'downloads'`) nehmen. Alternativ im Überschreiben-Dialog den vollen lokalen Pfad zeigen und Punkt-Ordner deutlich markieren.

## Geprüft, ohne Befund

- **„Befehl aus Worten“ führt nichts aus:** `assist_type_command` (`src-tauri/src/assist.rs:237-252`) entfernt alle Steuerzeichen. Damit fallen CR/LF, ESC und Bracketed-Paste-Sequenzen weg, und es wird nur die Zeile geschrieben, ohne Enter. Auch der neue Weg über die Palette (`initialRequest`) erzeugt nur einen Vorschlag. Getippt wird erst mit einem eigenen Enter oder Klick (`insert`), und ausgeführt erst mit dem Enter des Nutzers im Terminal. Die Anfrage geht als Parameter an `assistGenerate` und wird nirgends in HTML oder Shell eingesetzt.
- **Dateien-Panel, Vertrauenskette:** Das Panel nutzt dasselbe `negotiate` wie Terminal und Datei-Tab (Host-Key, Vault entsperren, Passwort, sudo) mit eigener Attempt-ID `<tab>-files`. Dialoge werden dem Tab zugeordnet (`App.tsx:1333`) und beim Schließen von Panel oder Tab abgebrochen (`App.tsx:798, 848`). Es gibt keinen eigenen Weg, auf dem ein Host-Key automatisch akzeptiert wird.
- **Pfad-Traversal bei Downloads:** Diese Prüfung ist unverändert und gilt auch für das Panel. `download_target` und `safe_local_name` in `crates/uwussh-core/src/sftp.rs:1109-1124` lehnen Namen mit Trennern, `..` und Windows-Gerätenamen ab. Das gilt auch rekursiv (`sftp.rs:564`). Vorhandene Ziele führen zu `AlreadyExists` und damit zu einer Rückfrage. Symlinks werden nicht als Ordner zum Zusammenführen genommen.
- **Quick-Connect-Parser** (`lib/home.ts:15-36`): Er lehnt Leerzeichen, leere Teile und Ports außerhalb 1–65535 ab. Mit einer unbekannten Adresse öffnet sich immer zuerst die HostForm und es wird nicht direkt verbunden. Verbunden wird über russh und nicht über ein `ssh`-Binary, eine Argument-Injection gibt es also nicht. Die Tests decken das ab.
- **Split-Ansicht:** `type-password` und der Assistent schreiben immer in die Session des eigenen Tabs (`App.tsx:963-975`). Das Passwort wird nur mit Enter abgeschickt, wenn eine Passwortfrage erkannt wurde. Der Fokus folgt dem angeklickten Bereich (`App.tsx:1977`).
- **Keymap (synchronisiert, nicht vertrauenswürdig):** Neu sind nur `command-palette` (übernimmt die alte ⌘K-Belegung) und `split-view` (ohne Standardtaste). `actionOf` lässt nur bekannte IDs durch. `DEFAULTS[id]?.[family]` kommt auch mit Prototyp-Namen zurecht. Die Palette bietet nur eingebaute Aktionen an, keine synchronisierten `text:`-Snippets. Neue Wege, über die ein synchronisierter Wert etwas ausführt, gibt es nicht.
- **Rendering:** Im Diff gibt es kein `dangerouslySetInnerHTML`, `innerHTML`, `eval`, keine neuen `invoke`-Aufrufe und keine URL-Öffner. Host-Namen und -Adressen werden als React-Text gerendert.
- **Settings:** `filesPanelWidth` wird in `sanitize` geprüft und begrenzt und nur pro Gerät gespeichert, nicht synchronisiert.
- **Rust-Crates und SSO:** In diesem Bereich unverändert bzw. nicht enthalten (siehe oben).

## Behoben

Alle Fixes liegen auf `feat/redesign`.

| Befund | Fix                                                                                                                                                                                                                                   | Commit  |
| ------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------- |
| N1     | Ein Name zählt nur noch, wenn die Eingabe nicht wie eine Adresse aussieht (kein `.`/`:`) und kein Host diese Adresse hat. Außerdem wird ein Port abgelehnt, der keine Zahl ist (`host:abc`). Beides hat Tests in `test/home.test.ts`. | 8bea0c4 |
| N2     | `openPanelFiles` und `openFilesIn` schließen die Session, wenn sie erst ankommt, nachdem Panel oder Tab schon zu sind.                                                                                                                | 284edff |
| N3     | Die Palette füllt „Befehl aus Worten“ nur noch vor. Gefragt wird erst, wenn im Assistenten Enter gedrückt wird.                                                                                                                       | 7b8efe7 |
| I2     | Teilweise: Der Überschreiben-Dialog zeigt den vollen Zielpfad. Fehlt der lokale Ordner (Store-Sandbox), ist der Download im Panel aus und es erscheint ein Hinweis. Ziel bleibt der Benutzerordner.                                   | 31ada7d |
| I1     | Offen: Das ist suite-weit zu lösen (SHA-256 der Design-Tarballs in CI prüfen).                                                                                                                                                        | –       |
