# UwUSSH — Konzept

> SSH-Client mit Termius-Ruhe, PuTTY-Tiefe und einem Sync-Server, der dir gehört.

|                 |                                                                             |
| --------------- | --------------------------------------------------------------------------- |
| **Stand**       | 2026-09-17 · Entwurf v0.3                                                   |
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
Master-Passwort ── Argon2id (m=64 MiB, t=3, p=4, Salt pro Vault) ──┐
Account-Key (128 Bit, zufällig, liegt nur auf gekoppelten Geräten) ┴─ HKDF
        ├─ Master Key   (bleibt IMMER lokal)  →  umschließt den Vault Key
        └─ Auth Secret  →  Server speichert nur SHA-256 davon

Vault Key (32 B, random, einmalig erzeugt)
   └─ wird mit Master Key umschlossen (key wrapping) → wrapped_vault_key
```

Drei Konsequenzen, die diesen Aufbau rechtfertigen:

1. **Passwortwechsel = nur neu umschließen.** Kein Re-Encrypt von 200 Records.
   Deshalb kostet auch der Umstieg eines schon bestehenden lokalen Vaults auf
   den Account-Key nichts: ein Schlüssel wird neu umschlossen, kein Record neu
   verschlüsselt.
2. **Der Server kennt nur den Login-Hash**, nie den Master Key. Das ist das
   Bitwarden-Modell — gut verstanden und vielfach geprüft.
3. **Der Account-Key macht die Server-Datenbank wertlos.** Der Server muss den
   umschlossenen Vault Key speichern, sonst kann kein zweites Gerät ihn holen.
   Ohne zweiten Faktor könnte also jeder, der den Server übernimmt, das
   Master-Passwort offline raten — mit Argon2id teuer, bei einem schwachen
   Passwort aber machbar. Der Account-Key (das 1Password-Modell) nimmt dieser
   Kopie jeden Wert: er steht nur auf den gekoppelten Geräten und im
   Recovery-Kit, nie auf dem Server.

   Der Preis ist ehrlich zu benennen: **alle Geräte weg und das Recovery-Kit
   weg heißt Daten weg.** Passwort plus Server-Adresse allein reichen dann
   nicht mehr. Deshalb zeigt das Setup das Kit genau einmal und fragt nach,
   ob es gesichert ist.

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
- **Recovery Kit:** zum Ausdrucken, einmal angezeigt, nie synchronisiert — Server-Adresse, Zertifikat-Fingerprint und der **Account-Key**, dazu eine 24-Wort-BIP39-Phrase, die den Vault Key unabhängig vom Master-Passwort entschlüsselt.

### Erstes Gerät, weiteres Gerät

Das Master-Passwort geht dabei nie über einen Kanal — es wird auf jedem Gerät
einmal getippt und bleibt dort.

**Erstes Gerät.** Der Server schreibt beim ersten Start einen Einrichtungscode
ins Log: Adresse, Zertifikat-Fingerprint und Einladung in einem String. Den
fügt man in Einstellungen → Sync ein, tippt das Master-Passwort und ist fertig.
Die App erzeugt dabei Account-Key und Geräte-Keypair, legt das Konto an, schiebt
alles hoch und zeigt das Recovery-Kit.

**Weiteres Gerät** — nach dem Vorbild von Magic Wormhole, weil an einem
Desktop-PC selten eine Kamera für einen QR-Code hängt:

1. Gerät A: „Gerät hinzufügen" zeigt einen Code wie `7-nyu-laser-kaffee`,
   zehn Minuten gültig, inklusive Server-Adresse.
2. Gerät B tippt den Code ein. Beide handeln per **SPAKE2** über das Relay des
   Servers einen Schlüssel aus; der Server sieht dabei nichts Brauchbares, und
   ein Angreifer hat pro Code genau einen Rateversuch.
3. Gerät A fragt „LVLaptop möchte beitreten?" und schickt durch diesen Kanal
   Zertifikat-Fingerprint, Account-Key und einen Einmal-Token. B prüft das
   Zertifikat, das es gesehen hat, gegen den Fingerprint.
4. B tippt das Master-Passwort und beweist dem Server damit, dass es das Konto
   kennt. **Erst dann** gibt der Server den umschlossenen Vault Key und die
   Records heraus — ein abgefangener Kopplungscode nützt ohne Passwort nichts.

Hosts, die B schon selbst hatte, laufen durch dieselbe Duplikaterkennung wie
ein Import; seine eigenen Secrets werden dabei unter dem Schlüssel des Kontos
neu versiegelt (`adopt_vault`).

Jedes Gerät bekommt eine `device_id` + eigenes Keypair für Server-Auth, einzeln
widerrufbar. Ein verlorenes Notebook sperrt man aus, ohne dass alle anderen
Geräte neu eingerichtet werden müssen.

---

## 6. Sync-Engine

**Grundhaltung: offline-first.** Alles landet zuerst in SQLite; der Server ist ein Verteiler, kein Gatekeeper. Ohne Netz funktioniert die App vollständig.

### Der Umschlag

Ein Record reist als **Umschlag**: id, Art, Vault, HLC, Tombstone-Flag — und
ein versiegelter Inhalt. Der Server sieht nur den Kopf, und **der Kopf ist
mitversiegelt** (AAD = `Label ‖ id ‖ kind ‖ vault_id ‖ HLC ‖ deleted`). Das ist
kein Detail, sondern die Stelle, an der ein bösartiger Server sonst gewinnt:

- Könnte er `deleted` setzen, wäre jeder Host auf allen Geräten weg — Löschen
  gewinnt gegen gleichzeitige Änderungen.
- Könnte er die Uhr umschreiben, ließe sich eine alte Version als die neueste
  ausgeben.

Beides scheitert jetzt am Authentifizierungs-Tag. Ein Tombstone versiegelt
deshalb einen leeren Inhalt, statt gar keinen zu haben.

### Protokoll

| Endpoint                             | Zweck                                                         |
| ------------------------------------ | ------------------------------------------------------------- |
| `POST /v1/accounts`                  | Konto anlegen (Einladung): Vault-Header, Auth-Verifier, Gerät |
| `POST /v1/session`                   | Gerät signiert eine Challenge (Ed25519) → Token, 1 h          |
| `GET /v1/vault`, `PUT /v1/vault/key` | Vault-Header holen; beim Passwortwechsel neu setzen           |
| `GET /v1/records?since=<seq>`        | Alle Umschläge mit `seq > since`, paginiert                   |
| `POST /v1/records`                   | Batch-Push, jeder Record mit `base_seq`                       |
| `GET /v1/events`                     | Server-Sent Events: „neu ab seq N" → Client pullt             |
| `POST /v1/pair`, `/v1/pair/{id}`     | Relay für die Gerätekopplung (SPAKE2), 10 min                 |
| `GET /v1/devices`, `POST …/revoke`   | Geräte listen, widerrufen (ein anderes nur mit Master-PW)     |
| `GET /healthz`                       | Ops                                                           |

Der Cursor ist eine **monotone Server-Sequenznummer**, kein Zeitstempel.
Zeitstempel über Geräte hinweg sind eine Fehlerquelle, Sequenznummern nicht.
Die Version eines Records ist genau diese Nummer: der Client pusht mit
`base_seq`, und weicht sie von der gespeicherten ab, ist es ein Konflikt. Ein
gerätelokaler Zähler wäre hier falsch — zwei Geräte zählen unabhängig.

**SSE statt WebSocket**, weil es durch jeden Reverse-Proxy kommt und für
„es gibt Neues ab N" kein Rückkanal nötig ist.

### Ein Durchgang

Erst pushen, dann pullen — in dieser Reihenfolge, denn der Cursor bewegt sich
nur beim Pull. Ein Durchgang, der mit einem Push endet, würde das Gerät seine
eigenen Schreibvorgänge beim nächsten Mal herunterladen. So kommen sie einmal
zurück und werden gegen den lokalen Stand geprüft — billig, und es fällt auf,
wenn der Server etwas anderes gespeichert hat.

Konflikte zeigen sich damit **auf dem Weg hinaus**: der Server lehnt ab und
liefert seinen Stand mit, der Client merged ihn und beginnt die Runde neu
(höchstens drei, dann Konflikt-Banner).

### Konflikte

Last-Writer-Wins **pro Record**, entschieden über die HLC, und **Löschen
gewinnt** gegen eine gleichzeitige Änderung: einen wiederauferstandenen Host,
den man für eine stillgelegte Bastion schon entfernt hatte, will niemand.

Pro Record und nicht pro Feld: Zwei Geräte ändern praktisch nie denselben Host
im selben Moment, und ein feldweiser Merge bräuchte für jedes Feld einen
gemeinsamen Vorfahren — also eine zweite Kopie jedes Records für einen Fall,
der nicht eintritt. Snippet-Bodies, wo Text echt kollidieren kann, sind die
einzige Stelle, an der sich später ein 3-Wege-Merge lohnen könnte.

Zwei Sonderfälle mit eigener Regel:

- **Vertraute Host-Keys** teilen sich einen Platz pro `address:port`. Haben
  zwei Geräte unabhängig verschiedene Schlüssel vertraut, bekommt der jüngere
  Eintrag den Platz — auf jedem Gerät derselbe, also einigen sie sich. Bei
  unterschiedlichen Fingerprints zählt die App das mit, damit die UI es sagen
  kann.
- **Keys als Datei** (`key_path`) bleiben lokal. Auf einem anderen Gerät
  bedeutet ein Pfad nichts; dort steht künftig „Key liegt nur auf LVDesk1".

### Was synct

- ✅ Hosts, Gruppen, Anmeldungen, Keys, versiegelte Secrets, Snippets, später
  Port-Forwards und Terminal-Profile
- ⚙️ `known_hosts` — optional, Default an
- ❌ Session-Historie, lokale Logs, Scrollback, Fenstergeometrie — und die
  lokalen Spalten: Key-Pfad, letzte Verbindung, erkanntes System

**Die Outbox ist die Datenbank.** Jeder lokale Schreibvorgang setzt in
derselben Transaktion `dirty = 1`; daneben steht `server_seq`, die Version, die
der Server bestätigt hat. Eine Warteschlange im RAM würde bei einem Absturz
Änderungen verlieren. Dazu `sync_extra`: Felder, die eine **neuere** Version
geschrieben hat, werden unverändert mitgeführt — ein älteres Gerät darf einen
Host bearbeiten, ohne zu löschen, was es nicht kennt.

Eine Umbenennung einer Gruppe ist genau ein Record: Hosts zeigen per
`group_id` auf ihre Gruppe, nicht per Name. Vorher wären es N Hosts gewesen —
und auf zwei Geräten gleichzeitig N Konflikte.

---

## 7. Der Sync-Server

Ein Binary. Ein Docker-Image. Eine SQLite-Datei. Er ist ein **dummer,
verschlüsselter Briefkasten**: er vergibt Sequenznummern, hält von jedem Record
die neueste Version, blättert ab einem Cursor durch sie und lehnt einen Schreib-
vorgang ab, dessen `base_seq` nicht der gespeicherten Version entspricht. Alles
Schlaue — verschlüsseln, mergen, entscheiden — passiert im Client.

```yaml
services:
  uwussh:
    image: ghcr.io/minifyx/uwussh-server:latest
    ports: ['8443:8443']
    volumes: ['./data:/data']
    environment:
      UWUSSH_DATA: /data
      UWUSSH_TLS: auto # auto = eigenes Zertifikat | off = hinter Reverse-Proxy
      UWUSSH_REGISTRATION: invite # open | invite | closed
```

```
UwUSSH-Server/
  src/main.rs      serve | invite | devices | revoke | backup | fingerprint
  src/config.rs    Umgebungsvariablen, nichts anderes
  src/tls.rs       selbstsigniertes Zertifikat beim ersten Start (rustls)
  src/db/          SQLite (WAL), Migrationen
  src/api/         accounts, session, records, events, pairing, devices
  src/limits.rs    Body- und Blob-Größen, Rate-Limits
  tests/           im selben Prozess gegen den echten uwussh-sync-Client
  Dockerfile       statisch (musl), distroless, nicht als root, ein Volume
```

```sql
accounts (id, vault_id, kdf_*, salt, wrapped_key, auth_verifier, created_ms)
devices  (id, account_id, name, public_key, cursor, last_seen_ms, revoked_ms)
records  (account_id, id, kind, seq, hlc, deleted, nonce, blob)  -- nur die neueste Version
invites  (code_hash, expires_ms, used_ms)
```

### TLS gehört dazu, nicht daneben

`UWUSSH_TLS=auto` erzeugt beim ersten Start ein eigenes Zertifikat und schreibt
seinen Fingerprint ins Log. Die App merkt ihn sich bei der Einrichtung und pinnt
ihn — **genau das Modell, das ein SSH-Client sowieso benutzt**, und beim Koppeln
reicht Gerät A den Fingerprint durch den SPAKE2-Kanal weiter. Damit braucht ein
Homelab keine Domain und kein Let's Encrypt, und der Server läuft auch über eine
Tailscale-Adresse. Wer schon einen Reverse-Proxy mit echtem Zertifikat hat,
setzt `UWUSSH_TLS=off` und bekommt die normale Prüfung. Reines HTTP lehnt die
App ab, außer gegen `localhost`.

### Grenzen, die der Server durchsetzt

Er kann keinen Record lesen, aber zählen und messen: höchstens 500 Records pro
Anfrage, 256 KiB pro Umschlag, eine 24-Byte-Nonce, die Schema-Version, und
Rate-Limits auf Kontoanlage, Login und Kopplung.

### Betrieb

- **Admin-CLI:** `uwussh-server invite`, `devices`, `revoke`, `backup`,
  `fingerprint`.
- **Backup** per `VACUUM INTO` — eine laufende WAL-Datenbank einfach
  wegzukopieren ist nicht zuverlässig. Dazu jede Nacht automatisch ein
  Schnappschuss, 14 werden behalten: billige Versicherung gegen einen
  Client-Bug, der Müll auf alle Geräte verteilt.
- **Bewusst nicht in v1:** Postgres, OIDC, Web-Oberfläche, Metriken und ein
  eigener Update-Feed. Updates kommen weiter aus dem Client-Repo.

### OIDC später — mit einer ehrlichen Einschränkung

Homelab-Setups haben oft schon einen IdP, und Login per SSO ist bequem. Aber:
**OIDC authentifiziert nur den Transport.** Der Vault bleibt hinter dem
Master-Passwort, sonst wäre Zero-Knowledge weg — der IdP könnte sonst
Vault-Zugriff ausstellen. Also: SSO ersetzt den Login-Beweis, nicht das Unlock.
Das muss in der UI klar dastehen, sonst ist die Erwartung falsch.

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

| Szenario                                      | Bekommt                                                                            | Bekommt **nicht**                                                                                                                                     |
| --------------------------------------------- | ---------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Sync-Server kompromittiert**                | Chiffrat-Blobs, Anzahl und Art der Records, Änderungszeiten, Geräte-IDs            | Hostnamen, Adressen, Keys, Passwörter, Snippet-Inhalte — und ohne den Account-Key auch keine Chance, das Master-Passwort offline zu raten             |
| **Sync-Server bösartig, nicht nur neugierig** | Kann Records zurückhalten oder einem **frischen** Gerät einen älteren Stand zeigen | Kann nichts fälschen, vertauschen oder löschen: der Kopf ist mitversiegelt. Geräte, die einen Record schon haben, erkennen den Rückschritt an der HLC |
| **Netzwerk-MITM**                             | Nichts über TLS hinaus; Blobs sind zusätzlich Ende-zu-Ende verschlüsselt           | —                                                                                                                                                     |
| **Gerät gestohlen, Vault gesperrt**           | SQLite-Datei mit Chiffrat                                                          | Klartext — Argon2id (64 MiB) macht Brute-Force teuer                                                                                                  |
| **Gerät gestohlen, Vault entsperrt**          | Alles                                                                              | — _(deshalb Auto-Lock als Default)_                                                                                                                   |
| **XSS in der WebView**                        | Terminal-Bytes, Metadaten, kann Sessions stören                                    | Private Keys, Passwörter — die liegen im Rust-Core                                                                                                    |

Die ehrliche Zeile ist die fünfte: Gegen ein entsperrtes, entwendetes Gerät
hilft Krypto nicht. Deshalb ist der Auto-Lock keine Komforteinstellung, sondern
die eigentliche Verteidigung. Und ein verlorenes Gerät am Server zu widerrufen
stoppt künftigen Sync, nicht das, was es schon hat — danach gehören SSH-Keys und
Passwörter getauscht. Das sagt die UI in dem Moment, in dem man widerruft.

Eine Grenze bleibt, und die hat jeder Zero-Knowledge-Sync: Ein Server, der
einfach **schweigt**, lässt sich nicht daran hindern. Er kann Records
zurückhalten oder einem ganz neuen Gerät einen älteren, in sich stimmigen Stand
zeigen (Fork-Consistency). Geräte, die einen Record schon kennen, merken den
Rückschritt an der HLC — ein frisches kann es nicht wissen. Was er dagegen nicht
kann, ist etwas erfinden: Ohne den Vault Key entsteht kein Umschlag, der die
Prüfung übersteht.

---

## 11. Roadmap

| Meilenstein            | Inhalt                                                                                                                                                                           | Grob       |
| ---------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------- |
| **M0 · Fundament**     | Tauri-Shell, xterm.js, `russh` connect, Passwort+Key-Auth, Host-Key-Prüfung, SQLite-Schema, Host-Liste. **Zuerst: Durchsatz messen.** — **erledigt**                             | 2–3 Wochen |
| **M1 · Daily Driver**  | Tabs/Splits, Agent, ProxyJump, Snippets, Themes, **Import aus PuTTY, KiTTY, `ssh_config` und Termius** samt der Host-Keys, denen diese Clients schon vertrauen, erste Nyu-Szenen | 3–4 Wochen |
| **M2 · Vault & Sync**  | Vault-Krypto ✓, **Sync-Fundament ✓**, Server v1, Kopplung, Account-Key, Recovery Kit                                                                                             | 4–5 Wochen |
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
| **UwUSSH-Server**   | Der Sync-Server (Axum, Docker)              | angelegt, erste Hälfte steht                                                                      |
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

~~Server-Default-DB~~ — **geklärt: SQLite**, ein Volume, ein Backup. Postgres wäre ein zweiter Treiber für einen Vorteil, den niemand mit drei Geräten spürt.
~~Zweiter Faktor neben dem Master-Passwort?~~ — **geklärt: Account-Key**, siehe §5. Der Server speichert zwangsläufig den umschlossenen Vault Key; ohne Account-Key wäre eine geklaute Server-Datenbank ein Offline-Angriff auf das Master-Passwort.
~~TLS nur über einen Reverse-Proxy?~~ — **geklärt: eingebaut als Default**, mit gepinntem Fingerprint wie bei einem SSH-Host; Reverse-Proxy bleibt möglich.
~~Konflikte feldweise auflösen?~~ — **geklärt: pro Record**, siehe §6.

1. **Feldweiser Merge für Snippet-Bodies** — lohnt der 3-Wege-Merge, sobald es Snippets gibt?
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

~~Das Fundament für den Sync-Server~~ — **erledigt.** Bevor ein Server Sinn hat,
musste der Client sicher synchronisieren können, und beim Durchlesen fanden sich
dafür sieben Baustellen. Die wichtigste: Der Kopf eines Records war nicht
mitversiegelt, ein Server hätte also jeden Host als gelöscht markieren können —
und weil Löschen gegen Änderungen gewinnt, wäre er überall verschwunden. Jetzt
ist der ganze Kopf Teil der Signatur, Tombstones versiegeln einen leeren Inhalt,
die Version eines Records ist die Sequenznummer des Servers statt eines
gerätelokalen Zählers, die Outbox liegt in der Datenbank statt im RAM, Hosts
zeigen per ID auf ihre Gruppe (eine Umbenennung ist ein Record statt N), und
eine fremde Uhr, die im Jahr 2099 steht, zieht die lokale nicht mehr mit.

Dazu die Engine selbst: ein Durchgang pusht, pullt und merged, und ein
Speicher-Server in `uwussh-sync` hält die Regeln des echten Servers als
lauffähigen Code fest. Sechzehn Tests fahren zwei echte Geräte dagegen — ein
Host, der ankommt, ein Passwort, das sich auf dem zweiten Gerät öffnet, zwei
gleichzeitige Umbenennungen, die beide Seiten gleich entscheiden, ein Löschen,
das nicht zurückkommt, ein Gerät, das mit eigenen Hosts beitritt, und ein
Server, der schwindelt und damit nirgends hinkommt.

~~Der Server~~ — **steht**, in
[MinifyX/UwUSSH-Server](https://github.com/MinifyX/UwUSSH-Server) (public,
GPL-3.0): ein Rust-Binary mit Axum und SQLite, das Sequenznummern vergibt, von
jedem Record die neueste Version hält und einen Schreibvorgang mit falscher
Version ablehnt — lesen kann es keinen. Geräte melden sich per signierter
Challenge an, ein zweites Gerät braucht Einmal-Token **und**
Master-Passwort-Beweis, Widerruf nimmt sofort die Tokens mit, und das letzte
Gerät darf sich nicht selbst aussperren. Dazu SSE-Events, Rate-Limits, CLI,
Docker-Image und nächtliche Backups.

Das eigene TLS ist drin und Standard: Der Server macht sich beim ersten Start
ein Zertifikat und nennt seinen Fingerprint; gepinnt wird der **Schlüssel**,
also übersteht ein gepinnter Fingerprint jedes neue Zertifikat. Und das
Kopplungs-Relay steht — der Server trägt ein paar opake Nachrichten zwischen
zwei Geräten hin und her, ohne den gesprochenen Code je zu sehen. 81 Tests,
davon 17 über echtes HTTP; am laufenden Binary mit `curl` und `openssl`
nachgeprüft, dass der ausgelieferte Schlüssel denselben Fingerprint hat, den die
Kommandozeile nennt.

~~Die Client-Hälfte~~ — **steht ebenfalls**, bis auf die Oberfläche. Der
**Account-Key** (128 Bit, per HKDF nach Argon2id) macht eine geklaute
Server-Datenbank wertlos; Sync einschalten schließt einen Schlüssel neu um und
verschlüsselt keinen Record neu. Im Recovery-Kit steht er als 28 Zeichen in
vier Gruppen mit zwei Prüfzeichen — ein Tippfehler sagt deshalb „Tippfehler"
und nicht „falsches Passwort". Der **Transport** pinnt den Server-Schlüssel wie
einen SSH-Host-Key und meldet sich bei abgelaufenem Token selbst neu an. Die
**Kopplung** läuft per SPAKE2 über das Relay: `K7M4Q-tiger-radio-kiwi`, drei
von 128 Wörtern, und das Geheimnis geht erst raus, nachdem die andere Seite
bewiesen hat, dass sie denselben Schlüssel hat.

Und beides ist gegeneinander geprüft: `UwUSSH-Server/tests/client.rs` fährt die
echten Client-Crates gegen den echten Server — ein Host mit Passwort wandert zu
einem Gerät, dem nur drei Wörter gesagt wurden, ein Gerät mit falsch gehörten
Wörtern bekommt nichts, und in den gespeicherten Records steht nichts Lesbares.
Der Test hat beim ersten Lauf einen echten Fehler gefunden (ein Header mit
eigenem SQL, dem ein Feld fehlte).

**Der Server ist release-fertig.** Installiert wird er mit einem Befehl:
`install.sh` installiert bei Bedarf Docker, fragt nur, wie die Geräte die
Maschine erreichen, startet den Server und zeigt den Einrichtungscode.
`update.sh` folgt dem Verfahren von UwUMail-Server — erst sich selbst
aktualisieren (mit Prüfsumme), dann Backup, neues Image, Healthcheck, und wenn
der nicht grün wird, zurück zur alten Version. Image für amd64 und arm64,
distroless, ohne Root und ohne Capabilities; CI fährt Installation, Update und
Rollback auf echtem Docker durch, bevor ein Image veröffentlicht wird.

Vorher hat ihn jemand geprüft, der ihn nicht geschrieben hat: nichts Kritisches,
kein Weg in ein fremdes Konto, aber umgehbare Rate-Limits, ein Konto, das den
Server erschöpfen kann, und ein Widerruf ohne Passwortbeweis — alles behoben
(`UwUSSH-Server/docs/security-review-2026-09.md`). Drei Fixes haben das
Protokoll berührt: ein **anderes Gerät widerrufen braucht das Master-Passwort**
(ein geklauter Laptop sperrt sonst den Besitzer aus), der Enrolment-Token reist
im Body statt in der URL, und die Seiten einer Kopplung sind gebunden (Seite a
mit Token, Seite b mit einem selbst gewählten Geheimnis). Pushes und Pull-Seiten
enden zusätzlich bei 8 MiB.

~~Die Oberfläche~~ — **steht seit 0.1.0-beta.8**: Einstellungen → Sync verbindet
den Server mit dem Einrichtungscode, zeigt das Recovery-Kit genau einmal (erst
schließbar nach „aufgeschrieben“), koppelt Geräte per langem oder gesprochenem
Code, listet sie mit Widerruf per Master-Passwort samt dem ehrlichen Satz, was
ein widerrufenes Gerät noch weiß, und trennt wieder (der Tresor braucht danach
nur noch das Passwort). Ein eigener Thread synchronisiert, solange der Tresor
offen ist. Die E2E-Phase E fährt zwei App-Instanzen gegen einen echten Server.
Vorher hat ein unabhängiger Review den Transport gehärtet: Adressen wie reqwest
parsen (`http://localhost:1@evil` war ein Loch), keine Redirects, Antworten mit
Obergrenze, endliche Pulls.

Im selben Release kamen **macOS (Apple-Chip und Intel) und Linux** dazu, mit
demselben Setup samt Nyu: unter macOS nach `/Applications`, unter Linux als
entpackte AppDirs nach `~/.local/share/uwussh` (kein FUSE nötig), beide mit
automatischen Updates. Gebaut wird in GitHub Actions, signiert lokal — der
Update-Schlüssel verlässt den Rechner nie. Dazu: KiTTY-Sitzungen aus Ordnern
und `.reg`-Exporten, ein Klick auf einen offenen Host zeigt seinen Tab (ein
weiterer per Rechtsklick), und beim Start öffnet sich nichts mehr von selbst.

Als Nächstes: ein „alles neu hochladen“ für die Zeit nach einem Server-Restore. Danach der Rest von **M1** — Splits, Agent-Login, ProxyJump-Ketten (der
Import merkt sich den Jump-Host, verknüpft die Kette aber noch nicht). Offen:
die von PuTTY schon vertrauten Host-Keys (eigenes Registry-Format, braucht
einen echten Dump zum Verifizieren).
