# UwUSSH — Konzept

> SSH-Client mit Termius-Ruhe, PuTTY-Tiefe und einem Sync-Server, der dir gehört.

|                 |                                                                             |
| --------------- | --------------------------------------------------------------------------- |
| **Stand**       | 2026-09-16 · Entwurf v0.2                                                   |
| **Basis**       | Tauri 2 + React + SQLite — identisch zu UwUMail                             |
| **Bundle-ID**   | `app.uwussh.desktop`                                                        |
| **Repo**        | [MinifyX/UwUSSH-Client](https://github.com/MinifyX/UwUSSH-Client) · GPL-3.0 |
| **Maskottchen** | Nyu, jetzt als Terminal-Katze                                               |
| **Plattformen** | Windows zuerst, dann Linux/macOS, später Android/iOS                        |

---

## 1. Positionierung

Der Markt ist zweigeteilt:

- **PuTTY / KiTTY / MobaXterm** — mächtig, lokal, kein Sync, UI aus 2004.
- **Termius / Tabby Cloud / Secure Shell** — schöne UI, aber Sync nur über deren Cloud, oft im Abo, deine Host-Liste liegt bei einem Dritten.

UwUSSH besetzt die Lücke: **Termius-Optik, PuTTY-Funktionsumfang, Sync auf deinem eigenen Server — Zero-Knowledge verschlüsselt.**

**Zielgruppe:** Homelabber und Admins mit 20–200 Hosts, drei Geräten und einer gesunden Abneigung gegen fremde Clouds.

**Versprechen in einem Satz:** _Deine Hosts, deine Keys, dein Server._

### Nicht-Ziele (v1)

- Kein Team-/Enterprise-Produkt (Shared Vaults kommen später, nicht zuerst).
- Kein Browser-Terminal im Server (optionales Modul, nach v1).
- Kein RDP/VNC. Das ist ein SSH-Client, kein Remmina-Klon.
- Keine Telemetrie. Keine Accounts bei uns. Kein Abo.

---

## 2. Tech-Stack

| Schicht      | Wahl                                                 | Warum                                                                                                                        |
| ------------ | ---------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| App-Shell    | **Tauri 2**                                          | Wie UwUMail. ~12 MB Binary statt 150 MB Electron, WebView2 auf Windows, Mobile-Support in v2.                                |
| Frontend     | **React + TypeScript**, Node 24, pnpm 11             | Exakt der UwUMail-Stack. Tokens, Komponenten, Nyu und die Build-Pipeline lassen sich übernehmen.                             |
| UI-Font      | **Manrope** (variabel, gebündelt)                    | Wie UwUMail — keine Netzwerk-Fonts. Terminal-Font separat: JetBrains Mono.                                                   |
| Terminal     | **xterm.js** + `@xterm/addon-webgl`                  | Das, was VS Code und Termius benutzen. WebGL-Renderer, Fallback auf Canvas.                                                  |
| SSH          | **`russh`** (+ `russh-sftp`)                         | Pure Rust, async/Tokio, kein libssh2-FFI-Schmerz. Liest OpenSSH-, PEM- und `.ppk`-Keys selbst; Agent, Port-Forwarding, SFTP. |
| Lokale Shell | **`portable-pty`** (WezTerm-Crate)                   | ConPTY auf Windows, PTY auf Unix — lokale Tabs für PowerShell/WSL/bash.                                                      |
| Seriell      | **`serialport`**                                     | PuTTY-Parität für COM-Ports.                                                                                                 |
| Store        | **SQLite** (`rusqlite`, WAL)                         | Wie UwUMail. Eine Datei, offline-first, einfach zu sichern.                                                                  |
| Krypto       | `argon2`, `chacha20poly1305`, `zeroize` (RustCrypto) | Etablierte Crates. Nichts selbst bauen.                                                                                      |
| Sync-Server  | **Rust + Axum**, SQLite (optional Postgres)          | Eine Sprache, ein Docker-Image, wenig RAM im Leerlauf.                                                                       |

### Terminal-Durchsatz — gemessen, entschieden

Das war der eine Punkt, an dem Tauri hätte wehtun können: Terminal-Ausgabe über die IPC-Grenze. **M0 hat ihn gemessen, und die Antwort ist eindeutig** (Details: [`docs/m0-spike.md`](docs/m0-spike.md)):

| Szenario (64 MiB farbige Log-Ausgabe) | Ergebnis                     | Längster UI-Frame |
| ------------------------------------- | ---------------------------- | ----------------- |
| Direkt, **ohne** Flow-Control         | **8,7 / 8,9 MiB verloren**   | 42 / 43 ms        |
| Direkt, mit Flow-Control (= SSH-Pfad) | vollständig, **41–46 MiB/s** | 6 ms              |
| ConPTY (`type bigfile`), Flow-Control | vollständig, **1,7 MiB/s**   | 6–7 ms            |

**Entscheidung: Der `tauri::ipc::Channel` mit Raw-Bytes bleibt, der WebSocket-Fallback fliegt raus.** Der Channel liefert schneller, als xterm.js parsen kann — die 41–46 MiB/s _sind_ die Parse-Geschwindigkeit von xterm.js, und die kann kein anderer Transport anheben. Lokale Shells unter Windows deckelt ConPTY bei ~1,7 MiB/s; SSH-Sessions laufen nie durch ConPTY.

**End-to-End-Flow-Control ist Pflicht — nicht wegen Tempo, sondern weil sonst Daten verloren gehen.** `Channel::send` kehrt zurück, sobald Tauri den Frame eingereiht hat, nicht wenn die WebView ihn verarbeitet hat. Ohne Rückmeldung staut sich alles in der WebView, und xterm.js verwirft ab 50 MB Rückstand hart („write data discarded, use flow control to avoid losing data"). Die UI lief dabei flüssig — der Verlust wäre niemandem aufgefallen. Gebaut ist deshalb das Schema von VS Codes Terminal: Die WebView quittiert verarbeitete Bytes aus dem Write-Callback von xterm.js, die Engine pausiert bei 512 KiB Rückstand und macht bei 128 KiB weiter.

> **Zwei Korrekturen gegenüber v0.1**, beide durch M0 erzwungen:
>
> 1. Hier stand, bei Overflow würden **Frames bewusst verworfen**. Falsch — Bytes vor xterm.js zu verwerfen zerschneidet Escape-Sequenzen. Die Engine verwirft nie, sie lässt nur die Quelle warten.
> 2. Das erste Gerüst hatte **Backpressure nur bis zur IPC-Grenze** und die Doku behauptete, das reiche. Die Messung hat gezeigt, dass genau dort die Daten verloren gingen.

---

## 3. Architektur

```
┌─ WebView ─────────────────────────┐
│  UI (Host-Tree, Tabs, Inspector)  │
│  xterm.js (WebGL)                 │
└──────────────┬────────────────────┘
               │ Tauri IPC
               │  ↓ Commands (JSON)   ↑ Channel (raw bytes)
┌──────────────┴────────────────────────────────────────┐
│  Rust Core                                            │
│                                                       │
│  SessionManager ──► russh ──────────► SSH-Host        │
│       │        └──► portable-pty ───► lokale Shell    │
│       │        └──► serialport ─────► COM/tty         │
│                                                       │
│  VaultService  (Argon2id, XChaCha20, zeroize)         │
│  SyncEngine    (HLC, Outbox, Konfliktauflösung)       │
│  KnownHosts    (TOFU + Change-Detection)              │
│  Store         (SQLite, WAL)                          │
└──────────────┬────────────────────────────────────────┘
               │ HTTPS + WSS (nur Chiffrat)
┌──────────────┴────────────────────────────────────────┐
│  UwUSSH Sync Server (Axum, selfhosted)                │
│  sieht: id, seq, updated_at, blob                     │
│  sieht nicht: Hostnamen, Keys, Passwörter             │
└───────────────────────────────────────────────────────┘
```

**Harte Regel:** Private Keys und Passwörter verlassen den Rust-Core nie im Klartext. Die WebView bekommt Terminal-Bytes und Metadaten, nie Secrets. Auth passiert komplett in Rust. Das begrenzt den Schaden einer XSS-Lücke im Frontend auf "kann Sessions stören" statt "kann alle Keys exfiltrieren".

---

## 4. Datenmodell

Alle Entitäten teilen sich denselben Sync-Kopf:

```
id          UUIDv7          -- zeitsortiert, kollisionsfrei ohne Koordination
updated_at  HLC             -- Hybrid Logical Clock (wall_ms, counter, device_id)
rev         u64             -- lokaler Revisionszähler
deleted     bool            -- Tombstone, TTL 90 Tage
vault_id    UUID            -- Personal / Work / Shared
```

| Entität             | Felder (Auszug)                                                                                                                                                            |
| ------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Host**            | name, address, port, group_id, identity_id, jump_host_id, tags[], color, charset, env[], keepalive, agent_forward, startup_snippet_id, terminal_profile_id, backspace_mode |
| **Group**           | name, parent_id, icon, sort                                                                                                                                                |
| **Identity**        | label, username, auth_type (`password` \| `key` \| `agent` \| `keyboard-interactive` \| `cert`), secret_ref                                                                |
| **Key**             | label, type (`ed25519` \| `rsa` \| `ecdsa`), private_pem (verschlüsselt), public, passphrase_ref, certificate                                                              |
| **Snippet**         | label, body, shell, targets[] (Host/Gruppe/Tag)                                                                                                                            |
| **PortForward**     | host_id, kind (`local` \| `remote` \| `dynamic`), bind, target, autostart                                                                                                  |
| **KnownHost**       | hostname, port, key_type, fingerprint_sha256, first_seen, verified_by                                                                                                      |
| **TerminalProfile** | font, size, theme, cursor, scrollback, bell                                                                                                                                |
| **SessionLog**      | host_id, started_at, duration, bytes — **lokal, synct per Default nicht**                                                                                                  |

`jump_host_id` als Selbstreferenz gibt ProxyJump-Ketten umsonst: `laptop → bastion → db-01` ist eine verkettete Liste, die der SessionManager rekursiv auflöst (max. Tiefe 8, Zyklenerkennung).

---

## 5. Vault & Krypto

Das Herzstück — hier entscheidet sich, ob "selfhosted Sync" ein Feature oder ein Risiko ist.

### Schlüsselableitung

```
Master-Passwort
   └─ Argon2id (m=64 MiB, t=3, p=4, salt = per-Vault random)
        ├─ Bytes 0..32   →  Master Key (bleibt IMMER lokal)
        └─ Bytes 32..64  →  Auth Secret → HKDF → Server-Login-Hash

Vault Key (32 B, random, einmalig erzeugt)
   └─ wird mit Master Key umschlossen (key wrapping) → wrapped_vault_key
```

Zwei Konsequenzen, die diesen Aufbau rechtfertigen:

1. **Passwortwechsel = nur neu umschließen.** Kein Re-Encrypt von 200 Records.
2. **Der Server kennt nur den Login-Hash**, nie den Master Key. Das ist das Bitwarden-Modell — gut verstanden und vielfach geprüft.

### Record-Verschlüsselung

`XChaCha20-Poly1305`, Nonce pro Record neu, **AAD = `record_id || entity_type || vault_id`**. Damit kann ein bösartiger Server weder Blobs zwischen Records vertauschen noch ihren Typ umdeuten.

### Keys und Anmeldedaten in der Praxis

Das ist der Teil, den man täglich merkt — deshalb ausformuliert statt nur "Vault":

- **Keys leben im Vault, nicht auf der Platte.** Erzeugen (ed25519 per Klick) oder importieren (OpenSSH-PEM, `.ppk` aus PuTTY/KiTTY). Der private Teil liegt nur verschlüsselt im Vault; es muss keine Key-Datei mehr im Dateisystem herumliegen.
- **Passphrase liegt mit im Vault.** Einmal den Vault entsperren, danach keine Passphrase-Abfrage mehr pro Verbindung. Genau der Reibungspunkt, der einen sonst dazu bringt, Keys ohne Passphrase zu erzeugen.
- **Identity statt Host-Duplikat.** Benutzername + Auth-Methode sind eine eigene Entität. Ein Key gilt für 40 Hosts, ohne 40-mal kopiert zu werden — und ein Key-Wechsel ist eine Änderung, nicht vierzig.
- **Auch Passwörter und Antworten.** Passwort-Auth, keyboard-interactive-Antworten und optional das sudo-Passwort (opt-in pro Host, deutlich als solches markiert).
- **Alles synct verschlüsselt.** Neues Gerät heißt: Vault entsperren, fertig. Kein Key-Datei-Rumkopieren, kein USB-Stick, kein "der Key ist auf dem anderen Rechner".
- **Export bleibt möglich**, aber als bewusste Aktion mit Warnung — ein Vault, aus dem man nicht wieder herauskommt, ist ein Lock-in und kein Feature.
- **v2: UwUSSH als SSH-Agent.** Der Vault tritt gegenüber anderen Programmen als Agent auf (Named Pipe unter Windows, Unix-Socket sonst), sodass auch `git push` im normalen Terminal die Keys aus dem Vault benutzt — mit Bestätigungsdialog pro Signatur-Anfrage.

### Unlock-Wege

- Master-Passwort (immer)
- **Windows Hello / Touch ID** → OS-Keychain hält den umschlossenen Vault Key, biometrisch freigegeben
- Auto-Lock nach N Minuten Inaktivität, bei Standby, bei Lock-Screen
- **Recovery Kit:** 24-Wort-BIP39-Phrase beim Setup, entschlüsselt den Vault Key unabhängig vom Passwort. Einmal anzeigen, zum Ausdrucken, nie synchronisieren.

### Neues Gerät koppeln

Zwei Wege, beide ohne das Master-Passwort über einen Kanal zu schicken:

1. **Passwort + Server-URL** — Standardfall, funktioniert ohne zweites Gerät.
2. **QR-Pairing** — bestehendes Gerät zeigt QR mit kurzlebigem Transport-Key; SPAKE2 mit 6-stelligem Short Auth String gegen MITM. Angenehm auf dem Handy.

Jedes Gerät bekommt eine `device_id` + eigenes Keypair für Server-Auth, einzeln widerrufbar. Ein verlorenes Notebook sperrt man aus, ohne dass alle anderen Geräte neu eingerichtet werden müssen.

---

## 6. Sync-Engine

**Grundhaltung: offline-first.** Alles landet zuerst in SQLite; der Server ist ein Verteiler, kein Gatekeeper. Ohne Netz funktioniert die App vollständig.

### Protokoll

| Endpoint                   | Zweck                                                     |
| -------------------------- | --------------------------------------------------------- |
| `GET /v1/sync?since=<seq>` | Alle Blobs mit `seq > since`, paginiert                   |
| `POST /v1/sync`            | Batch-Push, jeder Record mit `base_rev`                   |
| `WS /v1/stream`            | Push-Notify: "es gibt Änderungen ab seq N" → Client pullt |
| `POST /v1/auth/login`      | Login-Hash → Session-Token + `wrapped_vault_key`          |
| `GET/DELETE /v1/devices`   | Geräte listen, widerrufen                                 |
| `GET /healthz`, `/metrics` | Ops, Prometheus                                           |

Der Cursor ist eine **monotone Server-Sequenznummer**, kein Zeitstempel. Zeitstempel über Geräte hinweg sind eine Fehlerquelle, Sequenznummern nicht.

### Konflikte

Last-Writer-Wins **pro Feld**, entschieden über die HLC. Bei `409 Conflict` liefert der Server den aktuellen Stand, der Client merged feldweise und pusht erneut (max. 3 Versuche, dann Konflikt-Banner in der UI).

Das ist bewusst kein volles CRDT: Host-Einträge werden fast nie gleichzeitig auf zwei Geräten am selben Feld geändert — der Aufwand zahlt sich nicht aus. Ausnahme: **Snippet-Bodies**, wo Text echt kollidieren kann. Dafür ein 3-Wege-Merge mit Konfliktmarkern.

### Was synct

- ✅ Hosts, Gruppen, Identities, Keys, Snippets, Port-Forwards, Terminal-Profile, UI-Prefs, Tags
- ⚙️ `known_hosts` — optional, Default an
- ❌ Session-Historie, lokale Logs, Scrollback, Fenstergeometrie

---

## 7. Der Sync-Server

Ein Binary. Ein Docker-Image. Eine SQLite-Datei.

```yaml
services:
  uwussh:
    image: ghcr.io/<user>/uwussh-server:latest
    ports: ['8080:8080']
    volumes: ['./data:/data']
    environment:
      UWUSSH_DB: /data/uwussh.db
      UWUSSH_REGISTRATION: invite # open | invite | closed
```

- **TLS macht der Reverse-Proxy** (Caddy/Traefik/nginx). Der Server spricht HTTP und wertet `X-Forwarded-*` aus.
- **Admin-CLI:** `uwussh-server user add`, `invite create`, `device list`, `backup`.
- **Backup** = die SQLite-Datei kopieren. Mehr nicht. Plus `GET /v1/export` für ein vollständiges, weiterhin verschlüsseltes Archiv.
- **Postgres** als optionaler Treiber für Leute, die schon einen haben.
- **Update-Feed** für den Tauri-Updater — die eigene Instanz verteilt auch die App-Updates.

### OIDC (Authentik, Keycloak) — mit einer ehrlichen Einschränkung

Homelab-Setups haben oft schon einen IdP, und Login per SSO ist bequem. Aber: **OIDC authentifiziert nur den Transport.** Der Vault bleibt hinter dem Master-Passwort, sonst wäre Zero-Knowledge weg — der IdP könnte sonst Vault-Zugriff ausstellen. Also: SSO ersetzt den Login-Hash, nicht das Unlock. Das muss in der UI klar dastehen, sonst ist die Erwartung falsch.

---

## 8. Features

### Import — Priorität 1, nicht "nice to have"

Niemand tippt 80 Hosts neu ab. Der Import entscheidet, ob die App am ersten Abend benutzbar ist oder wieder zugemacht wird — deshalb steht er **in M1, nicht in M5**.

| Quelle                       | Wo die Daten liegen                                                                                                                                                                                                       | Aufwand                                                     |
| ---------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------- |
| **PuTTY**                    | Registry `HKCU\Software\SimonTatham\PuTTY\Sessions`, ein Schlüssel pro Session, URL-enkodierte Namen                                                                                                                      | klein — reines Registry-Lesen                               |
| **KiTTY**                    | PuTTY-Fork: Registry unter `HKCU\Software\9bis.com\KiTTY\Sessions`, im Portable-Modus stattdessen ein `Sessions\`-Ordner mit einer Datei pro Session (**beides am echten KiTTY verifizieren**)                            | klein — selbes Key/Value-Format wie PuTTY, nur anderer Pfad |
| **OpenSSH**                  | `~/.ssh/config` inkl. `Host`-Patterns, `ProxyJump`, `IdentityFile`, `Match`-Blöcke                                                                                                                                        | mittel — echter Parser nötig                                |
| **Termius**                  | Je nach Version JSON- oder CSV-Export; die lokale Termius-Datenbank ist verschlüsselt und kommt nur in Frage, wenn der Nutzer sein Termius-Passwort eingibt (**am echten Export verifizieren, bevor wir es versprechen**) | mittel — Format hängt an der Version                        |
| WinSCP, mRemoteNG, MobaXterm | INI- bzw. XML-Exportdateien                                                                                                                                                                                               | klein, wenn die Grundstruktur steht                         |

**Gemeinsame Mechanik statt vier Einzellösungen:** jeder Importer ist ein Adapter, der in ein neutrales `ImportedHost`-Zwischenformat schreibt. Danach läuft für alle derselbe Weg — Vorschau mit Checkboxen, Duplikaterkennung über `address:port`, Zuordnung zu Gruppen, und erst dann der Schreibvorgang. Neue Quellen kosten dann nur noch einen Adapter.

**Was PuTTY und KiTTY mitbringen, das man nicht verlieren darf:** zugeordnete Key-Dateien (`PublicKeyFile`, `.ppk`), Proxy-Einstellungen, Terminal-Farbschemata, `RemoteCommand`, Port-Forwards und die Backspace-/Zeichensatz-Eigenheiten. `.ppk`-Dateien brauchen **weder Konvertierung noch eigenen Parser**: russh liest PuTTY-Keys (v2 und v3, auch verschlüsselt) direkt — beim Bau der SSH-Anbindung im Quelltext gefunden.

### MVP — muss drin sein, damit es täglich nutzbar ist

- Tabs + Splits (horizontal/vertikal), Host-Tree mit Suche
- **Command Palette (Ctrl+K)**: verbinden, Snippet senden, SFTP öffnen, Theme wechseln
- Auth: Passwort, Key (ed25519/RSA/ECDSA), **Agent** (Pageant, `\\.\pipe\openssh-ssh-agent`, 1Password), keyboard-interactive inkl. 2FA-Prompt
- `known_hosts` mit TOFU; geänderter Host-Key = harte Blockade mit explizitem Override
- **ProxyJump-Ketten** über `jump_host_id`
- Port-Forwarding: local / remote / dynamic (SOCKS5) mit eigenem Manager-Panel
- **SFTP-Browser**: Zwei-Spalten, Drag & Drop, Remote-Datei im lokalen Editor öffnen und zurückschreiben
- Snippets, Broadcast-Input auf alle Panes
- Themes, Fonts, Keymaps, echtes True-Color

### v2 — Homelab-Superkräfte

Hier entsteht der Abstand zu Termius:

- **Tailscale/Headscale-Import** — Tailnet-Geräte als Host-Liste, MagicDNS-Namen, Online-Status live. Für die Zielgruppe wahrscheinlich das stärkste Einzelfeature.
- **Proxmox-Import** — Nodes und LXC/VMs per API, Gruppen spiegeln den Cluster
- **Netbox-Import** — Inventar als Single Source of Truth
- Lokale Shell-Tabs (PowerShell, WSL, cmd) + serielle Konsole
- Session-Recording mit asciinema-Export
- Persistenter Scrollback mit Volltextsuche über alte Sessions
- Auto-Reconnect mit Zustandsanzeige statt Modal
- SSH-CA-Zertifikate; FIDO2 (`ed25519-sk`) — **Library-Support in `russh` vorher prüfen**

### v3

- Mobile Clients (Tauri v2 kann iOS/Android, `russh` läuft dort)
- Shared Vaults mit Per-Host-ACL
- Optionales Web-Terminal im Server

---

## 9. UI, Nyu & Tonfall

**Leitbild:** Termius' Ruhe — aber die Dichte, die 200 Hosts brauchen, und die UwU-Handschrift von UwUMail.

### Layout

```
┌────────────┬──────────────────────────────────┬──────────┐
│ Sidebar    │  Tab-Bar                         │ Inspector│
│            ├──────────────────────────────────┤          │
│ Suche      │                                  │ Host-    │
│ ▸ Homelab  │      Terminal / SFTP             │ Details  │
│   ● prox-1 │                                  │ Forwards │
│   ● nas    │                                  │ Snippets │
│ ▸ Hetzner  │                                  │          │
└────────────┴──────────────────────────────────┴──────────┘
```

- Custom Titlebar, keine OS-Chrome-Kante
- Status-Punkt pro Host (Reachability), "zuletzt verbunden"
- **Reconnect als Banner, nie als Modal.** Ein Modal über einem laufenden Terminal ist ein UX-Fehler.
- Inspector einklappbar — der Vollbild-Terminal-Modus muss einen Tastendruck entfernt sein

### Visuell — das UwUMail-Designsystem, unverändert übernommen

Kein zweites Designsystem. Die Tokens aus `apps/desktop/src/styles/tokens.css` von UwUMail werden 1:1 übernommen, inklusive Namensschema `--uwu-*`:

| Token              | Hell      | Dunkel    |
| ------------------ | --------- | --------- |
| `--uwu-canvas`     | `#f8f4f6` | `#141016` |
| `--uwu-surface`    | `#ffffff` | `#1c171f` |
| `--uwu-ink`        | `#1c1420` | `#f8f2f6` |
| `--uwu-pink`       | `#ff4d8d` | `#ff7fac` |
| `--uwu-pink-solid` | `#e11d74` | `#ff7fac` |

Zwei Pinks aus demselben Grund wie bei UwUMail: weißer Text auf `#ff4d8d` schafft nur 3,1:1, gefüllte Buttons brauchen deshalb `#e11d74`.

- **Manrope** für die Oberfläche, Radien 10 px für Controls, 16 px für Karten, 999 px für Pills, 4-px-Raster.
- **JetBrains Mono** im Terminal — als einziger Bereich, der bewusst nicht dem UwU-Look folgt. Ein Terminal ist ein Terminal.
- Neu gegenüber UwUMail: **Terminal-Themes** sind ein eigener Satz Tokens. Der Default heißt "Nyu" und übersetzt die 16 ANSI-Farben in die UwU-Palette; klassische Schemata (Solarized, Gruvbox, Campbell) liegen daneben.
- Dunkel ist Default, hell gleichwertig gepflegt. Keyboard-first, sichtbare Focus-Ringe.

### Nyu

Nyu bleibt dieselbe Katze — nur ist der Briefumschlag jetzt ein **Terminalfenster**: die Fensterchrome mit drei Punkten oben, die Ohren gucken darüber hinaus, und der Bildschirm ist das Gesicht mit UwU-Augen, `w`-Mund und Blush. Sticker-Stil unverändert: Pflaumen-Outlines `#4B1D3F`, pinker Body `#FF6FA6`, heller Bildschirm `#FFB8D3`, weiße Die-Cut-Kante. Als App-Icon leicht gekippt auf pastellpinker Kachel, mit zwei gelben Sparkles — und statt des Herzens von UwUMail bewacht sie hier **einen Schlüssel**.

Quellen in `brand/` (Icon, Symbol, Mono-Symbol) und `apps/desktop/src/components/nyu/` (React), analog zu UwUMail.

**Szenen** (`NyuScene`, 320 × 220) für die Leerzustände, die es in einem SSH-Client gibt:

| Szene            | Wann                                           |
| ---------------- | ---------------------------------------------- |
| Willkommen       | Erststart, noch kein Host                      |
| Import geschafft | Nach PuTTY/KiTTY/Termius-Import, mit Anzahl    |
| Vault schläft    | Vault gesperrt — Nyu schläft auf dem Schlüssel |
| Nichts gefunden  | Suche ohne Treffer                             |
| Verbindung weg   | Reconnect-Banner, Nyu wartet mit Kabel         |
| Alles offline    | Kein Host erreichbar                           |
| Tunnel läuft     | Port-Forward-Panel ohne aktive Weiterleitung   |
| Ordner leer      | Leerer SFTP-Ordner                             |

**Bewegung:** Nyu blinzelt in Szenen, zuckt bei Hover mit den Ohren, und der Cursor auf ihrem Bildschirm blinkt im Terminal-Takt. Einstellungen → Darstellung → Animationen (System / An / Aus) löst wie bei UwUMail nach `<html data-motion="full|reduced">` auf; bei `reduced` steht Nyu still.

### Tonfall

Verspielt als Default, **Einstellungen → Tonfall → Neutral** tauscht die Worte, nie Layout oder Farben. Strings liegen in `locales/<lang>/neutral.json` und `playful.json`, wie bei UwUMail.

| Situation           | Neutral                                   | Verspielt                                                         |
| ------------------- | ----------------------------------------- | ----------------------------------------------------------------- |
| Kein Host           | Noch keine Hosts                          | Ganz schön leer hier (・_・;) Lass uns deine PuTTY-Sessions holen |
| Verbunden           | Verbunden mit prox-1                      | Drin! ✨                                                          |
| Import fertig       | 47 Hosts importiert                       | 47 Hosts eingesammelt (๑˃ᴗ˂)ﻭ                                     |
| Verbindung verloren | Verbindung getrennt. Neuer Versuch in 5 s | Ups, weg (╥﹏╥) Ich probier's in 5 s nochmal                      |
| Vault gesperrt      | Vault gesperrt                            | Nyu passt auf deine Schlüssel auf ᶻ 𝗓 𐰁                           |

**Die eine harte Ausnahme: Sicherheitswarnungen sind nie verspielt.** Geänderter Host-Key, fehlgeschlagene Vault-Entsperrung, Zustimmung zu Agent-Forwarding — dort verschwinden Kaomoji und Nyu vollständig, in beiden Tonfällen. Ein `(╥﹏╥)` neben einer möglichen Man-in-the-Middle-Warnung macht genau das kaputt, was die Warnung leisten soll. Gleiches gilt weiterhin für Buttons, die auf Daten wirken: `Löschen` bleibt `Löschen`.

### Onboarding

Drei Schritte, nicht mehr: **Vault anlegen → "Nur lokal" oder Server verbinden → Hosts importieren.** "Nur lokal" muss eine gleichwertige, nicht abgewertete Option sein — viele wollen erst mal keinen Server.

---

## 10. Sicherheit

### Maßnahmen

- Keys nur im RAM entschlüsselt, `zeroize` beim Drop, nie in Logs
- **Agent-Forwarding per Host opt-in** — nicht global. Ein kompromittierter Host mit weitergereichtem Agent ist ein Seitwärtsbewegungs-Vektor.
- Host-Key-Änderung = Verbindung blockiert, Fingerprint-Diff im Klartext, Override nur mit Tippbestätigung
- Zwischenablage-Auto-Clear nach 45 s bei Passwort-Kopie
- Tauri: strikte CSP, Capability-Allowlist für Commands, kein Remote-Content im Hauptfenster
- Signierte Installer, reproduzierbare Builds

### Threat Model — was bekommt ein Angreifer?

| Szenario                             | Bekommt                                                                  | Bekommt **nicht**                                      |
| ------------------------------------ | ------------------------------------------------------------------------ | ------------------------------------------------------ |
| **Sync-Server kompromittiert**       | Chiffrat-Blobs, Anzahl Records, Änderungszeiten, Geräte-IDs              | Hostnamen, Adressen, Keys, Passwörter, Snippet-Inhalte |
| **Netzwerk-MITM**                    | Nichts über TLS hinaus; Blobs sind zusätzlich Ende-zu-Ende verschlüsselt | —                                                      |
| **Gerät gestohlen, Vault gesperrt**  | SQLite-Datei mit Chiffrat                                                | Klartext — Argon2id (64 MiB) macht Brute-Force teuer   |
| **Gerät gestohlen, Vault entsperrt** | Alles                                                                    | — _(deshalb Auto-Lock als Default)_                    |
| **XSS in der WebView**               | Terminal-Bytes, Metadaten, kann Sessions stören                          | Private Keys, Passwörter — die liegen im Rust-Core     |

Die ehrliche Zeile ist die vierte: Gegen ein entsperrtes, entwendetes Gerät hilft Krypto nicht. Deshalb ist der Auto-Lock keine Komforteinstellung, sondern die eigentliche Verteidigung.

---

## 11. Roadmap

| Meilenstein            | Inhalt                                                                                                                                                                           | Grob       |
| ---------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------- |
| **M0 · Fundament**     | Tauri-Shell, xterm.js, `russh` connect, Passwort+Key-Auth, Host-Key-Prüfung, SQLite-Schema, Host-Liste. **Zuerst: Durchsatz messen.** — **erledigt**                             | 2–3 Wochen |
| **M1 · Daily Driver**  | Tabs/Splits, Agent, ProxyJump, Snippets, Themes, **Import aus PuTTY, KiTTY, `ssh_config` und Termius** samt der Host-Keys, denen diese Clients schon vertrauen, erste Nyu-Szenen | 3–4 Wochen |
| **M2 · Vault & Sync**  | Vault-Krypto, Server v1, Device-Pairing, Konfliktauflösung, Recovery Kit                                                                                                         | 4–5 Wochen |
| **M3 · SFTP & Tunnel** | SFTP-Browser, Port-Forward-Manager, Remote-Edit                                                                                                                                  | 3 Wochen   |
| **M4 · Politur**       | Updater, Portable-Build, Linux/macOS, Onboarding, Accessibility                                                                                                                  | 2–3 Wochen |
| **M5 · Homelab**       | Tailscale-, Proxmox-, Netbox-Import, lokale Shells, Recording                                                                                                                    | offen      |
| **M6 · Mobil & Teams** | iOS/Android, Shared Vaults                                                                                                                                                       | offen      |

**Die Risikoreihenfolge ist Absicht:** M0 klärt zuerst die einzige Frage, die das ganze Konzept kippen könnte — ob die IPC-Grenze den Terminal-Durchsatz trägt. Krypto und Sync kommen erst, wenn das steht.

---

## 12. Repos

Gleiche Aufteilung wie bei UwUMail — drei Repos, GPL-3.0:

| Repo                | Inhalt                                      | Status                                                                                            |
| ------------------- | ------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| **UwUSSH-Client**   | Die App: React-UI, Rust-Engine, Brand, Docs | angelegt                                                                                          |
| **UwUSSH-Server**   | Der Sync-Server (Axum, Docker)              | kommt mit M2                                                                                      |
| ~~UwUSSH-Releases~~ | Downloads und Update-Feed                   | entfällt: Releases und Feeds (Branch `updates`) liegen im Client-Repo, wie inzwischen bei UwUMail |

```
UwUSSH-Client/
├─ apps/desktop/
│  ├─ src/                 # React-UI, inkl. components/nyu/
│  └─ src-tauri/           # Tauri-Host, Commands, Channels
├─ crates/
│  ├─ uwussh-core/         # SessionManager, russh, pty, forwards, sftp
│  ├─ uwussh-vault/        # Argon2id, XChaCha20, Keychain, Recovery
│  ├─ uwussh-sync/         # HLC, Outbox, Merge, HTTP/WS-Client
│  ├─ uwussh-import/       # PuTTY, KiTTY, ssh_config, Termius
│  ├─ uwussh-store/        # SQLite: Hosts, Identities, Known Hosts
│  └─ uwussh-proto/        # Shared Types (serde), Schema-Version
├─ brand/                  # Nyu: App-Icon, Symbol, Mono-Symbol
├─ docs/                   # vision, architecture, design, roadmap
├─ KONZEPT.md              # dieses Dokument
└─ LICENSE                 # GPL-3.0
```

Zwei Entscheidungen, die sich später auszahlen:

- **`uwussh-proto` als eigenes Crate** — Client und Server teilen exakt eine Typdefinition, damit Schema-Migrationen nicht still auseinanderbrechen. Deshalb liegt es im Client-Repo und wird vom Server als Git-Dependency gezogen.
- **`uwussh-import` als eigenes Crate** — die Importer sind die Komponente mit den meisten Sonderfällen und dem größten Testbedarf. Getrennt lassen sie sich gegen echte Registry-Dumps und Beispiel-Configs testen, ohne die App zu starten.

---

## 13. Offene Entscheidungen

~~Frontend-Framework~~ — **geklärt: React + TypeScript, Node 24, pnpm 11**, exakt wie UwUMail.
~~Lizenz~~ — **geklärt: GPL-3.0**, wie UwUMail. Wer eine geänderte Version weitergibt, gibt den Quelltext mit weiter.

1. **Server-Default-DB** — Empfehlung SQLite (ein Volume, ein Backup); Postgres optional.
2. **`known_hosts` synchronisieren?** — Empfehlung ja, Default an; es ist der häufigste Reibungspunkt beim Gerätewechsel.
3. **Team-Vaults** — v1 bewusst raus, oder gleich im Datenmodell vorsehen? (`vault_id` ist ohnehin drin, es offen zu lassen kostet nichts.)
4. ~~**PPK-Parser selbst schreiben?**~~ — **geklärt: nicht nötig.** russh liest `.ppk` v2/v3 nativ, verschlüsselt oder nicht.
5. **Termius-Import: was geht wirklich?** — vor dem Versprechen an einem echten Termius-Export verifizieren. Falls nur CSV herauskommt, ist der Import dünner als gedacht und das gehört ehrlich ins README.
6. **Vault-Sync ohne Server** — lohnt sich ein reiner Datei-Sync über Syncthing/Nextcloud als dritte Option neben "nur lokal" und "eigener Server"? Wäre für viele Homelabs der kürzeste Weg.

---

## Nächster Schritt

~~M0-Durchsatz-Spike~~ — **erledigt am 2026-09-16.** Der IPC-Channel trägt, mit End-to-End-Flow-Control verlustfrei und ohne Ruckler; der WebSocket-Fallback ist gestrichen.

~~Rest von M0~~ — **ebenfalls erledigt.** SSH-Sessions laufen auf dem gemessenen Datenpfad, mit Passwort- und Key-Login (OpenSSH, PEM, PuTTY-`.ppk`, auch verschlüsselt). Hosts und vertraute Host-Keys liegen in SQLite, die Host-Liste ist echt. Die Host-Key-Prüfung ist dabei von M1 nach M0 gewandert: Ein Client, der sich verbindet, ohne den Server-Key zu prüfen, wäre schlimmer als einer, der sich gar nicht verbindet.

Ein Ende-zu-Ende-Lauf (`node apps/desktop/e2e/run.mjs`) klickt die echte App gegen einen echten SSH-Server durch und hat vier Fehler gefunden, die kein anderer Test sehen konnte. Der wichtigste: Die Passwortabfrage kam **vor** der Warnung über einen geänderten Host-Key. Gesendet wurde nie etwas, aber die Reihenfolge war falsch — jetzt verbindet UwUSSH zuerst, prüft den Key und fragt erst dann nach dem Passwort, auf derselben Verbindung.

~~Import aus Termius~~ — **erledigt.** Termius hat keinen Export mehr, also liest UwUSSH die lokale Electron-Datenbank direkt: Hosts, Gruppen, Anmeldungen, Keys samt Passphrase, die schon vertrauten Host-Keys und Snippets. Die Secrets landen im Vault, dessen lokale Krypto dafür aus M2 vorgezogen wurde (Master-Passwort → Argon2id → umschlossener Vault-Key, XChaCha20-Poly1305 pro Record). Beim ersten echten Lauf: 14 Hosts, 16 Anmeldungen, 2 Keys, 138 Host-Keys, nichts Lesbares übersprungen. Details in [`docs/architecture.md`](docs/architecture.md#termius-which-has-no-export). Der Ende-zu-Ende-Lauf öffnet den Import-Dialog und prüft, dass er zuerst nach dem Vault fragt, bevor irgendetwas geschrieben wird.

~~Verbinden mit Keys aus dem Vault~~ — **erledigt.** Ein importierter Host meldet sich mit dem Passwort oder Key aus dem Vault an; Key-Material geht als Bytes an die Engine, keine Datei auf der Platte. Der Host-Key wird weiterhin zuerst geprüft, bevor ein Secret gesendet wird. Ist der Vault gesperrt, fragt die App nach dem Master-Passwort und verbindet neu.

~~Import aus PuTTY, KiTTY und `ssh_config`~~ — **erledigt.** PuTTY/KiTTY-Sessions kommen direkt aus der Registry (`.ppk`-Pfade, gefakte Ordner als Gruppen); `~/.ssh/config` wird mitsamt `Include`-Direktiven gelesen. Alle drei nutzen Datei-Keys und tippen Passwörter — kein Vault nötig. Der Import ist quellenunabhängig: Quelle wählen, Vorschau, schreiben.

~~Tabs, Einstellungen, Installer und Updates — die erste Beta~~ — **erledigt, als 0.1.0-beta.1.** Jede Verbindung bekommt ihren eigenen Tab, auch mehrere zum selben Server; jeder Tab hat sein eigenes Terminal und seinen eigenen Login-Versuch, Rückfragen kommen der Reihe nach. Das Fenster hat eigene Knöpfe zum Minimieren, Maximieren und Schließen (vorher fehlten sie, weil das Fenster ohne Systemrahmen läuft), und fragt vor dem Schließen nach, wenn noch Verbindungen offen sind. Die Einstellungen decken Darstellung, Terminal, Tresor und Updates ab. Windows bekommt den eigenen Nyu-Installer wie UwUMail (pro Benutzer, ohne Adminrechte, ersetzt die alte NSIS-Installation) und signierte automatische Updates mit Kanal Stabil oder Beta. Dazu eine Sicherheitsrunde: DLLs nur aus System32, strengere CSP, M0-Befehle begrenzt, alte Sessions werden beim Neuladen der Seite geschlossen.

Als Nächstes im Rest von **M1**: Splits, Agent-Login, ProxyJump-Ketten (der Import merkt sich den Jump-Host, verknüpft die Kette aber noch nicht). Offen: die von PuTTY schon vertrauten Host-Keys (eigenes Registry-Format, braucht einen echten Dump zum Verifizieren).
