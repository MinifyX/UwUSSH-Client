// Screenshots of every screen, for comparing the UI before and after a change:
//
//   node apps/desktop/e2e/shots.mjs [--app desktop|keygen|setup|all]
//                                   [--out <dir>] [--label <name>] [--only <a,b>]
//
// Writes <out>/ssh-<app>/<label>/<shot>-<variant>.png and a report.txt there
// (default out: apps/desktop/e2e/shots, label: current). `--only` takes
// scenario names (see SCENARIOS below) to run a subset.
//
// Starts each app's page with Vite in-process (127.0.0.1, port 0) and plays
// Rust: a fake of Tauri's IPC answers every command with plausible data, sends
// terminal output through the Channel like the engine does, and records what
// it was asked. Nothing reaches a server or the internet. The setup app needs
// no fake: without __TAURI_INTERNALS__ it runs its own preview API.
//
// Variants: light, dark, hc (light + contrast high), hcdark (dark + contrast
// high); the main window also as macOS (light, dark). Setup is always light.
//
// Needs Playwright 1.63 with Chromium (`npm i --no-save playwright@1.63.0`
// somewhere and NODE_PATH pointing at its node_modules, or the Playwright
// Docker image). Any OS. See the end of this file for the Docker command.

import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

// ── Selectors that depend on CSS classes or app internals ───────────────────
// Everything else goes by visible German text, roles and aria-labels. After a
// UI migration, these are the ones to check first.
const CSS = {
  /** xterm.js' own root element (library class, not the app's). */
  xterm: '.xterm',
  /** The file browser's panes (data attribute set by FileBrowser.tsx). */
  filePane: '[data-file-pane]',
  /** The remote pane of the file browser. */
  remotePane: '[data-file-pane="remote"]',
  /** UwUKeygen's laser pad, the area to move the mouse over. */
  laserPad: '.nyu-laser, .laser-pad, canvas',
};
/**
 * Dev-only hook in App.tsx: the active tab's TerminalDriver, whose buffer the
 * script reads to know the fake shell output arrived (with the WebGL renderer
 * the text never reaches the DOM).
 */
const DRIVER_HOOK = '__uwusshDriver';
/** Where the apps keep their settings and the onboarding flag. */
const SETTINGS_KEY = 'uwussh.settings';
const ONBOARDING_KEY = 'uwussh.onboarding';

// ── Arguments ───────────────────────────────────────────────────────────────

const here = dirname(fileURLToPath(import.meta.url));
const appsDir = resolve(here, '..', '..');
const args = process.argv.slice(2);
const arg = (name, fallback) => {
  const at = args.indexOf(`--${name}`);
  if (at >= 0 && args[at + 1]) return args[at + 1];
  const eq = args.find((a) => a.startsWith(`--${name}=`));
  return eq ? eq.slice(name.length + 3) : fallback;
};
const which = arg('app', 'all');
const outBase = resolve(arg('out', join(here, 'shots')));
const label = arg('label', 'current');
const only = arg('only', '')
  .split(',')
  .map((s) => s.trim())
  .filter(Boolean);
const apps = which === 'all' ? ['desktop', 'keygen', 'setup'] : [which];

// Playwright from wherever it is: a normal import, else NODE_PATH via require.
async function loadPlaywright() {
  try {
    return await import('playwright');
  } catch {
    return createRequire(import.meta.url)('playwright');
  }
}
const { chromium } = await loadPlaywright();

// Vite from the app's own node_modules, so its config's plugins match.
async function startVite(root) {
  const require = createRequire(join(root, 'package.json'));
  const pkgFile = require.resolve('vite/package.json');
  const pkg = JSON.parse(readFileSync(pkgFile, 'utf8'));
  let entry = pkg.exports?.['.']?.import ?? pkg.module ?? 'dist/node/index.js';
  if (typeof entry === 'object') entry = entry.default;
  const vite = await import(pathToFileURL(join(dirname(pkgFile), entry)).href);
  const server = await vite.createServer({
    root,
    configFile: join(root, 'vite.config.ts'),
    logLevel: 'error',
    // The repo root too: the fonts live in its node_modules/.pnpm, which the
    // apps' own `fs.allow: ['..']` leaves out (merged, not replaced).
    server: {
      port: 0,
      strictPort: false,
      host: '127.0.0.1',
      hmr: false,
      fs: { allow: [resolve(root, '..', '..')] },
    },
  });
  await server.listen();
  const { port } = server.httpServer.address();
  return { server, origin: `http://127.0.0.1:${port}` };
}

// ── The fake Rust side, run inside the page ─────────────────────────────────

function fakeTauri(config) {
  const calls = [];
  const listeners = new Map();
  let next = 1;
  const now = new Date('2026-10-06T09:30:00Z').getTime();
  const minutes = (n) => now - n * 60_000;

  const host = (id, name, address, extra) => ({
    id,
    name,
    address,
    port: 22,
    username: 'nyu',
    auth: 'key',
    keyPath: null,
    groupPath: null,
    lastConnectedMs: minutes(90),
    workspace: 'private',
    position: 0,
    os: 'linux',
    hasPassword: false,
    keyId: 'k1',
    keyLabel: 'nyu@laptop',
    ...extra,
  });
  const hosts = [
    host('h-nas', 'nas', '192.0.2.10', {
      groupPath: 'Homelab',
      os: 'synology',
      auth: 'password',
      hasPassword: true,
      keyId: null,
      keyLabel: null,
      username: 'admin',
      position: 0,
    }),
    host('h-pve', 'pve', 'pve.home.example', {
      groupPath: 'Homelab',
      os: 'proxmox',
      username: 'root',
      port: 22,
      position: 1,
    }),
    host('h-pi', 'pi-hole', 'pi.home.example', {
      groupPath: 'Homelab',
      os: 'raspberry',
      username: 'pi',
      position: 2,
    }),
    host('h-vps', 'vps', 'vps.example.org', { os: 'arch', port: 2222, position: 3 }),
    host('h-web', 'web-01', 'web-01.example.com', {
      workspace: 'business',
      groupPath: 'Produktion',
      os: 'ubuntu',
      username: 'deploy',
      position: 0,
    }),
    host('h-db', 'db-01', 'db-01.example.com', {
      workspace: 'business',
      groupPath: 'Produktion',
      os: 'debian',
      username: 'deploy',
      position: 1,
    }),
    host('h-router', 'edge-router', '198.51.100.1', {
      workspace: 'business',
      groupPath: 'Produktion',
      os: 'mikrotik',
      username: 'admin',
      position: 2,
    }),
    host('h-win', 'build-win', '2001:db8::20', {
      workspace: 'business',
      os: 'windows',
      username: 'builder',
      position: 3,
      lastConnectedMs: null,
    }),
  ];
  const groups = [
    { workspace: 'private', name: 'Homelab', position: 0 },
    { workspace: 'business', name: 'Produktion', position: 0 },
  ];
  const tunnels = [
    {
      id: 't1',
      hostId: 'h-pi',
      name: 'Pi-hole Admin',
      kind: 'local',
      bindAddress: '127.0.0.1',
      bindPort: 8080,
      targetHost: '127.0.0.1',
      targetPort: 80,
      autostart: true,
    },
    {
      id: 't2',
      hostId: 'h-web',
      name: 'Postgres',
      kind: 'local',
      bindAddress: '127.0.0.1',
      bindPort: 15432,
      targetHost: 'db-01.example.com',
      targetPort: 5432,
      autostart: false,
    },
    {
      id: 't3',
      hostId: 'h-nas',
      name: 'Webhook',
      kind: 'remote',
      bindAddress: '127.0.0.1',
      bindPort: 9000,
      targetHost: '127.0.0.1',
      targetPort: 9000,
      autostart: false,
    },
  ];
  const tunnelStatuses = [
    {
      id: 't1',
      hostId: 'h-pi',
      session: null,
      connections: 2,
      active: 1,
      bytesOut: 182_340,
      bytesIn: 2_349_812,
      state: 'running',
      boundPort: 8080,
    },
  ];
  const keys = [
    {
      id: 'k1',
      label: 'nyu@laptop',
      keyType: 'ssh-ed25519',
      publicKey:
        'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFakeFakeFakeFakeFakeFakeFakeFakeFakeFake nyu@laptop',
      hasPassphrase: true,
      hosts: 6,
    },
    {
      id: 'k2',
      label: 'deploy (CI)',
      keyType: 'ssh-rsa',
      publicKey:
        'ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQFakeFakeFakeFakeFakeFake deploy@ci.example.com',
      hasPassphrase: false,
      hosts: 0,
    },
  ];
  const observed = {
    algorithm: 'ssh-ed25519',
    fingerprint: 'SHA256:q3Xv9Lr0KxM2b7cT4nYwZp1sQe8uVhGfD6aJkNo5RtI',
    publicKey: 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExampleHostKeyExampleHostKeyExample',
    randomart: [
      '+--[ED25519 256]--+',
      '|      .o+=*o.    |',
      '|     . +=*+o     |',
      '|      o.B=o .    |',
      '|     . *.B.o     |',
      '|      = S.* .    |',
      '|     . = = o     |',
      '|      o . E      |',
      '|       . o .     |',
      '|        .        |',
      '+----[SHA256]-----+',
    ].join('\n'),
  };
  const entry = (name, kind, size, mode, ago) => ({
    name,
    path: '',
    kind,
    size,
    modifiedMs: minutes(ago),
    permissions: mode,
    owner: 'nyu',
    link: false,
  });
  const remoteEntries = (path) =>
    [
      entry('.config', 'dir', 4096, 0o755, 3000),
      entry('backups', 'dir', 4096, 0o750, 600),
      entry('docker', 'dir', 4096, 0o755, 120),
      entry('www', 'dir', 4096, 0o755, 45),
      entry('.bashrc', 'file', 3771, 0o644, 90000),
      entry('deploy.sh', 'file', 2_184, 0o755, 300),
      entry('docker-compose.yml', 'file', 1_402, 0o644, 120),
      entry('notes.md', 'file', 812, 0o600, 20),
      entry('release-0.3.0.tar.gz', 'file', 48_734_221, 0o644, 4000),
    ].map((e) => ({ ...e, path: `${path.replace(/\/$/, '')}/${e.name}` }));
  const localEntries = (path) =>
    [
      entry('Desktop', 'dir', 0, 0o755, 400),
      entry('Documents', 'dir', 0, 0o755, 800),
      entry('Downloads', 'dir', 0, 0o755, 30),
      entry('projects', 'dir', 0, 0o755, 10),
      entry('id_ed25519.pub', 'file', 96, 0o644, 50000),
      entry('screenshot.png', 'file', 284_112, 0o644, 60),
    ].map((e) => ({ ...e, path: `${path.replace(/\/$/, '')}/${e.name}` }));

  const vaultState = () => ({
    status: config.vault,
    remembered: config.vault === 'unlocked',
    needsRecoveryCode: false,
    stranded: false,
    lockEmail: null,
  });
  const syncStatus = () => ({
    paired: true,
    serverUrl: 'https://sync.example.com',
    tlsFingerprint: 'SHA256:9fT1eXampleFingerprintOnlyForScreenshots0000',
    deviceId: 'dev-1',
    pairedMs: minutes(60 * 24 * 20),
    lastSyncMs: minutes(2),
    last: {
      atMs: minutes(2),
      report: {
        pulled: 3,
        pushed: 1,
        conflicts: 0,
        rounds: 1,
        apply: { applied: 3, rejected: 0, hostKeyConflicts: 0 },
        withheld: { records: 0, hostKeys: false },
      },
      error: null,
    },
    pending: 0,
    running: false,
    vault: config.vault,
    deviceName: 'Nyus Laptop',
    offering: false,
    withheld: { records: 0, hostKeys: false },
    backend: 'uwusync',
    lock: {
      serverUrl: null,
      email: null,
      needsSignIn: false,
      live: false,
      liveRefused: false,
      signedInMs: null,
      moveStartedMs: null,
      leftBehind: false,
      switchedOff: false,
    },
  });

  // What connecting to a host does, by host id: 'ok' (default), 'hang',
  // 'unreachable', 'unknown-key', 'changed', 'password', 'vault-locked'.
  const behavior = {};
  let sessionCounter = 0;
  const esc = '\x1b[';
  const shellText = (h) => {
    const prompt = `${esc}01;32m${h.username}@${h.name}${esc}0m:${esc}01;34m~${esc}0m$ `;
    return [
      `Linux ${h.name} 6.8.12-4-pve #1 SMP PREEMPT_DYNAMIC x86_64\r\n`,
      '\r\n',
      `Last login: Mon Oct  5 21:14:03 2026 from 192.0.2.50\r\n`,
      `${prompt}ls --color\r\n`,
      `${esc}01;34mbackups${esc}0m  ${esc}01;34mdocker${esc}0m  ${esc}01;32mdeploy.sh${esc}0m  ` +
        `${esc}01;36mcurrent${esc}0m  notes.md  ${esc}01;31mrelease-0.3.0.tar.gz${esc}0m\r\n`,
      `${prompt}systemctl status nginx --no-pager | head -3\r\n`,
      `${esc}32m●${esc}0m nginx.service - A high performance web server\r\n`,
      `     Loaded: loaded (/lib/systemd/system/nginx.service; enabled)\r\n`,
      `     Active: ${esc}01;32mactive (running)${esc}0m since Tue 2026-10-06 07:02:11 UTC; 2h 28min ago\r\n`,
      `${prompt}tail -n 3 /var/log/app.log\r\n`,
      `2026-10-06 09:28:40 INFO  backup finished OK in 42s\r\n`,
      `2026-10-06 09:29:02 WARN  disk usage at 81% on /var\r\n`,
      `2026-10-06 09:29:15 ERROR upstream 203.0.113.7:8443 failed: connection refused\r\n`,
      `${prompt}ping -c 1 web-01.example.com\r\n`,
      `PING web-01.example.com (198.51.100.20) 56(84) bytes of data.\r\n`,
      `64 bytes from 198.51.100.20: icmp_seq=1 ttl=57 time=11.8 ms\r\n`,
      prompt,
    ].join('');
  };
  const sendFrames = (channel, text) => {
    const bytes = Array.from(new TextEncoder().encode(text));
    const callback = window[`_${channel.id}`];
    if (typeof callback === 'function') callback({ message: bytes, index: 0 });
  };
  const connect = ({ id, onData }) => {
    const h = hosts.find((x) => x.id === id);
    const mode = behavior[id] ?? 'ok';
    const fail = (error) => Promise.reject(error);
    switch (mode) {
      case 'hang':
        return new Promise(() => undefined);
      case 'unreachable':
        return fail({
          kind: 'unreachable',
          address: h.address,
          reason: 'Zeitüberschreitung nach 10 s (keine Antwort auf Port ' + h.port + ')',
        });
      case 'unknown-key':
        return fail({ kind: 'unknown-host-key', observed });
      case 'changed':
        return fail({
          kind: 'host-key-changed',
          trustedFingerprint: 'SHA256:Old0ldOldKeyFingerprintForScreenshots00000',
          observed,
        });
      case 'password':
        return fail({ kind: 'password-required' });
      case 'vault-locked':
        return fail({ kind: 'vault-locked' });
      default: {
        sessionCounter += 1;
        const session = `s-${sessionCounter}`;
        setTimeout(() => sendFrames(onData, shellText(h)), 50);
        return Promise.resolve(session);
      }
    }
  };

  const answers = {
    // App start
    close_all_sessions: () => 0,
    m0_autorun: () => false,
    take_link: () => null,
    update_status: () => null,
    check_for_updates: () => null,
    set_update_channel: () => null,
    // Hosts and groups
    list_hosts: () => hosts,
    list_groups: () => groups,
    save_host: ({ draft }) => ({ ...hosts[0], ...draft, id: draft.id ?? 'h-new' }),
    // Sessions
    connect_host: connect,
    spawn_shell_session: ({ onData }) => {
      sessionCounter += 1;
      setTimeout(
        () => sendFrames(onData, `\x1b[01;32mnyu@laptop\x1b[0m:\x1b[01;34m~\x1b[0m$ `),
        50,
      );
      return `local-${sessionCounter}`;
    },
    session_can_type_password: () => false,
    trust_host_key: () => null,
    // Vault and keys
    vault_status: () => config.vault,
    vault_state: vaultState,
    list_keys: () => keys,
    key_public_line: ({ id }) => keys.find((k) => k.id === id)?.publicKey ?? '',
    // Tunnels
    list_tunnels: () => tunnels,
    tunnel_statuses: () => tunnelStatuses,
    // Sync
    sync_status: syncStatus,
    sync_devices: () => [
      {
        id: 'dev-1',
        name: 'Nyus Laptop',
        createdMs: minutes(60 * 24 * 20),
        lastSeenMs: minutes(2),
        revokedMs: null,
        current: true,
      },
      {
        id: 'dev-2',
        name: 'Arbeits-PC',
        createdMs: minutes(60 * 24 * 12),
        lastSeenMs: minutes(45),
        revokedMs: null,
        current: false,
      },
      {
        id: 'dev-3',
        name: 'Altes Notebook',
        createdMs: minutes(60 * 24 * 300),
        lastSeenMs: minutes(60 * 24 * 90),
        revokedMs: minutes(60 * 24 * 80),
        current: false,
      },
    ],
    // Command assistant
    assist_settings: () => ({
      provider: 'ollama',
      providers: {
        ollama: { model: 'qwen2.5-coder:7b', baseUrl: 'http://127.0.0.1:11434', hasKey: false },
      },
    }),
    assist_detect_ollama: () => ['qwen2.5-coder:7b', 'llama3.2:3b'],
    assist_models: () => ['qwen2.5-coder:7b', 'llama3.2:3b'],
    assist_cache_list: () => [
      {
        id: 'c1',
        platform: 'Ubuntu · bash',
        request: 'größte Ordner in /var',
        normalized: 'größte ordner in /var',
        command: 'du -xh /var --max-depth=1 | sort -rh | head',
        explanation: 'Listet die Unterordner von /var nach Größe, die größten zuerst.',
        dangerous: false,
        createdMs: minutes(600),
        usedMs: minutes(30),
        hits: 3,
      },
      {
        id: 'c2',
        platform: 'Proxmox VE · bash',
        request: 'alle VMs neu starten',
        normalized: 'alle vms neu starten',
        command: "qm list | awk 'NR>1 {print $1}' | xargs -n1 qm reboot",
        explanation: 'Startet jede VM auf diesem Knoten neu.',
        dangerous: true,
        createdMs: minutes(3000),
        usedMs: minutes(3000),
        hits: 1,
      },
    ],
    assist_platform: ({ shell }) => ({
      key: 'proxmox/bash',
      shell: shell ?? 'bash',
      shells: ['bash', 'sh', 'zsh'],
      os: 'proxmox',
    }),
    assist_generate: () => ({
      command: 'df -h --output=source,size,used,avail,pcent /',
      explanation: 'Zeigt, wie voll das Root-Dateisystem ist – Größe, belegt, frei und Prozent.',
      dangerous: false,
      cached: false,
      platform: 'Proxmox VE · bash',
    }),
    // Import
    available_imports: () => ['termius', 'putty', 'openssh', 'folder'],
    scan_import: () => ({
      hosts: 12,
      identities: 2,
      keys: 3,
      knownHosts: 9,
      snippets: 4,
      needsVault: true,
      skipped: [],
    }),
    // Files
    open_files: () => ({ session: 'files-1', home: '/home/deploy', root: false }),
    remote_list: ({ path }) => remoteEntries(path),
    remote_canonicalize: ({ path }) => path,
    local_places: () => [
      { label: 'Persönlicher Ordner', path: '/home/nyu', kind: 'home' },
      { label: 'Schreibtisch', path: '/home/nyu/Desktop', kind: 'desktop' },
      { label: 'Dokumente', path: '/home/nyu/Documents', kind: 'documents' },
      { label: 'Downloads', path: '/home/nyu/Downloads', kind: 'downloads' },
    ],
    local_list: ({ path }) => localEntries(path),
    local_parent: ({ path }) => (path === '/' ? null : path.replace(/\/[^/]*$/, '') || '/'),
    // UwUKeygen
    keygen_generate: ({ request }) => ({
      token: 'tok-1',
      info: {
        algorithm: request?.kind?.type === 'rsa' ? 'ssh-rsa' : 'ssh-ed25519',
        label: request?.kind?.type === 'rsa' ? 'RSA' : 'ED25519',
        bits: request?.kind?.type === 'rsa' ? request.kind.bits : 256,
        comment: request?.comment || 'nyu@laptop',
        publicOpenssh:
          request?.kind?.type === 'rsa'
            ? 'ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQCGeneratedRsaKeyForScreenshotsOnly0000000000000000 nyu@laptop'
            : 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGeneratedKeyForScreenshotsOnly000000 nyu@laptop',
        fingerprintSha256: 'SHA256:Zk3vJ7m2QxA1bR9cD4eF6gH8iK0lM2nO4pQ6rS8tU0w',
        fingerprintMd5: 'MD5:3c:9a:1f:22:7e:4b:90:aa:5d:61:0e:c8:73:14:bf:02',
        randomart:
          request?.kind?.type === 'rsa'
            ? observed.randomart.replace('[ED25519 256]--', '-[RSA 2048]----')
            : observed.randomart,
      },
    }),
    keygen_encode: () =>
      '-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\nQyNTUxOQAAACBFakeKeyForScreenshotsOnlyFakeKeyForScreenshotsOnly000\n-----END OPENSSH PRIVATE KEY-----\n',
    // Window and events
    'plugin:window|is_maximized': () => false,
    'plugin:window|is_fullscreen': () => false,
    'plugin:event|listen': ({ event, handler }) => {
      const id = next++;
      listeners.set(id, { event, handler });
      return id;
    },
    'plugin:event|unlisten': ({ eventId }) => {
      listeners.delete(eventId);
      return null;
    },
  };

  window.__fake = {
    calls,
    behavior,
    hosts,
    emit(event, payload) {
      for (const [id, l] of listeners) {
        if (l.event === event) window[`_${l.handler}`]?.({ event, id, payload });
      }
    },
  };
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => undefined };
  window.__TAURI_INTERNALS__ = {
    metadata: {
      currentWindow: { label: 'main' },
      currentWebview: { label: 'main', windowLabel: 'main' },
    },
    plugins: {},
    transformCallback: (callback, once) => {
      const id = next++;
      window[`_${id}`] = (payload) => {
        if (once) delete window[`_${id}`];
        return callback?.(payload);
      };
      return id;
    },
    unregisterCallback: (id) => delete window[`_${id}`],
    convertFileSrc: (path) => path,
    invoke: async (cmd, args) => {
      calls.push({ cmd, args });
      const answer = answers[cmd];
      return answer ? answer(args ?? {}) : null;
    },
  };
}

// ── Variants ────────────────────────────────────────────────────────────────

const VARIANTS = {
  light: { theme: 'light', contrast: 'normal' },
  dark: { theme: 'dark', contrast: 'normal' },
  hc: { theme: 'light', contrast: 'high' },
  hcdark: { theme: 'dark', contrast: 'high' },
};
const FIXED_NOW = '2026-10-06T09:32:00Z';
const MAC_UA =
  'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)';

// ── Running scenarios ───────────────────────────────────────────────────────

const browser = await chromium.launch({ executablePath: process.env.CHROMIUM || undefined });

/**
 * One page per scenario and variant. `shot(name)` saves `<name>-<variant>.png`;
 * a failed step is noted in the report and the scenario's later shots skipped.
 */
async function runApp({ app, origin, scenarios, variants, dir, fake = true }) {
  const report = [];
  for (const scenario of scenarios) {
    if (only.length && !only.includes(scenario.name)) continue;
    for (const variantName of scenario.variants ?? variants) {
      const variant = VARIANTS[variantName.replace(/^mac-/, '')] ?? VARIANTS.light;
      const errors = [];
      const context = await browser.newContext({
        viewport: scenario.viewport ?? { width: 1280, height: 800 },
        deviceScaleFactor: 1,
        locale: 'de-DE',
        reducedMotion: 'reduce',
        colorScheme: variant.theme,
        userAgent: variantName.startsWith('mac-') ? MAC_UA : undefined,
      });
      if (fake) {
        await context.addInitScript(fakeTauri, { vault: 'unlocked', ...scenario.fake });
      }
      await context.addInitScript(
        ({ key, onboardingKey, settings }) => {
          try {
            window.localStorage.setItem(key, JSON.stringify(settings));
            window.localStorage.setItem(onboardingKey, JSON.stringify({ doneMs: 1 }));
          } catch {
            // Storage off: the app runs with its defaults.
          }
        },
        {
          key: SETTINGS_KEY,
          onboardingKey: ONBOARDING_KEY,
          settings: {
            language: 'de',
            theme: variant.theme,
            contrast: variant.contrast,
            motion: 'system',
            workspaces: true,
            activeWorkspace: 'private',
            ...scenario.settings,
          },
        },
      );
      const page = await context.newPage();
      // A fixed clock: "vor 2 Minuten" and file dates stay the same between runs.
      await page.clock.setFixedTime(new Date(FIXED_NOW));
      page.on('pageerror', (error) => errors.push(`page error: ${error.message}`));
      page.on('console', (message) => {
        if (message.type() === 'error') errors.push(`console: ${message.text().split('\n')[0]}`);
      });
      const suffix = variantName;
      const taken = [];
      const shot = async (name) => {
        await settle(page);
        const file = `${name}-${suffix}.png`;
        await page.screenshot({ path: join(dir, file) });
        taken.push(file);
        report.push(`ok      ${file}`);
      };
      try {
        await page.goto(`${origin}${scenario.path ?? '/'}`);
        await scenario.run(page, shot);
      } catch (error) {
        const message = String(error?.message ?? error).split('\n')[0];
        report.push(
          `FAILED  ${scenario.name}-${suffix} (after ${taken.length} shot(s)): ${message}`,
        );
        try {
          await page.screenshot({ path: join(dir, `_failed-${scenario.name}-${suffix}.png`) });
        } catch {
          // The page is gone; the report says enough.
        }
      }
      for (const e of [...new Set(errors)]) report.push(`        ${scenario.name}-${suffix}: ${e}`);
      await context.close();
    }
  }
  writeFileSync(
    join(dir, 'report.txt'),
    `${app} — ${new Date().toISOString()}\n${report.join('\n')}\n`,
  );
  const failed = report.filter((l) => l.startsWith('FAILED')).length;
  const ok = report.filter((l) => l.startsWith('ok')).length;
  console.log(`${app}: ${ok} shots, ${failed} failed scenario runs → ${dir}`);
  return failed;
}

/** Fonts loaded, two frames drawn, nothing in flight. */
async function settle(page) {
  await page.evaluate(async () => {
    await document.fonts?.ready;
    await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
  });
  await page.waitForTimeout(150);
}

const dialog = (page, name) => page.getByRole('dialog', name ? { name } : undefined);
const hostButton = (page, name) =>
  page
    .locator('aside')
    .getByRole('button', { name: new RegExp(`^${name}\\b`) })
    .first();
const terminalShows = (page, text) =>
  page.waitForFunction(
    ({ hook, text }) => {
      const driver = window[hook];
      const buffer = driver?.term?.buffer?.active;
      if (!buffer) return false;
      for (let y = 0; y < buffer.length; y += 1) {
        if (buffer.getLine(y)?.translateToString(true).includes(text)) return true;
      }
      return false;
    },
    { hook: DRIVER_HOOK, text },
    { timeout: 10_000 },
  );
const ready = (page) => page.getByText('Kein Tab offen').waitFor();
const hostsLoaded = (page) => hostButton(page, 'nas').waitFor();
const openSettings = async (page) => {
  await page
    .getByRole('button', { name: /^Einstellungen\b/ })
    .first()
    .click();
  await dialog(page, 'Einstellungen').waitFor();
};
const setBehavior = (page, id, mode) =>
  page.evaluate(([id, mode]) => (window.__fake.behavior[id] = mode), [id, mode]);

// ── Desktop ─────────────────────────────────────────────────────────────────

const SETTINGS_SECTIONS = [
  ['appearance', 'Darstellung'],
  ['terminal', 'Terminal'],
  ['highlight', 'Hervorhebung'],
  ['vault', 'Tresor & Keys'],
  ['assist', 'KI'],
  ['sync', 'Sync'],
  ['data', 'Import & Export'],
  ['updates', 'Updates'],
  ['about', 'Über UwUSSH'],
];

const DESKTOP = [
  {
    name: 'main',
    run: async (page, shot) => {
      await ready(page);
      await hostsLoaded(page);
      await shot('main-empty');
      await hostButton(page, 'pve').click({ button: 'right' });
      await page.getByRole('menu').waitFor();
      await shot('host-menu');
      await page.keyboard.press('Escape');
      await page.getByRole('menu').waitFor({ state: 'detached' });
      await page.getByRole('radio', { name: /Business/ }).click();
      await hostButton(page, 'web-01').waitFor();
      await page.mouse.move(1000, 400);
      await shot('main-business');
    },
  },
  {
    name: 'mac',
    variants: ['mac-light', 'mac-dark'],
    run: async (page, shot) => {
      await ready(page);
      await hostsLoaded(page);
      await shot('main-empty');
    },
  },
  {
    name: 'terminal',
    run: async (page, shot) => {
      await ready(page);
      await hostsLoaded(page);
      await hostButton(page, 'pve').click();
      await terminalShows(page, 'icmp_seq=1');
      await page.mouse.move(1000, 400);
      await shot('terminal-live');
      await page.getByRole('button', { name: 'Befehl aus Worten' }).click();
      const popup = dialog(page);
      await popup.waitFor();
      await popup.getByRole('textbox').first().waitFor();
      await shot('assist-popup');
      await popup.getByRole('textbox').first().fill('wie voll ist die Festplatte');
      await popup.getByRole('textbox').first().press('Enter');
      await popup.getByText('Zeigt, wie voll das Root-Dateisystem ist').waitFor();
      await shot('assist-suggestion');
    },
  },
  {
    name: 'failed',
    run: async (page, shot) => {
      await ready(page);
      await hostsLoaded(page);
      await setBehavior(page, 'h-vps', 'unreachable');
      await hostButton(page, 'vps').click();
      await page.getByRole('alert').waitFor();
      await page.getByText('Nicht verbunden.').first().waitFor();
      await page.mouse.move(1000, 400);
      await shot('notice-error');
      await setBehavior(page, 'h-pi', 'hang');
      await hostButton(page, 'pi-hole').click();
      await page.getByText('Verbinde mit pi-hole').waitFor();
      await page.mouse.move(1000, 400);
      await shot('tab-connecting');
    },
  },
  {
    name: 'hostform',
    run: async (page, shot) => {
      await ready(page);
      await hostsLoaded(page);
      await page.getByRole('button', { name: 'Host hinzufügen' }).click();
      await dialog(page, 'Neuer Host').waitFor();
      await shot('hostform-new');
      await page.keyboard.press('Escape');
      await dialog(page, 'Neuer Host').waitFor({ state: 'detached' });
      await hostButton(page, 'nas').hover();
      await page.getByRole('button', { name: 'nas bearbeiten' }).click();
      await dialog(page, 'nas bearbeiten').waitFor();
      await shot('hostform-edit');
    },
  },
  {
    name: 'settings',
    run: async (page, shot) => {
      await ready(page);
      await openSettings(page);
      const nav = page.getByRole('navigation', { name: 'Bereiche' });
      for (const [id, text] of SETTINGS_SECTIONS) {
        await nav.getByRole('button', { name: text, exact: true }).click();
        // Sections load their data on open (vault, keys, sync, AI).
        await page.waitForTimeout(250);
        await shot(`settings-${id}`);
      }
    },
  },
  {
    name: 'dialogs',
    run: async (page, shot) => {
      await ready(page);
      await hostsLoaded(page);
      await page.locator('aside').getByRole('button', { name: 'Tunnel', exact: true }).click();
      await dialog(page, 'Tunnel').waitFor();
      await page.getByText('Pi-hole Admin').first().waitFor();
      await shot('tunnels');
      await page.keyboard.press('Escape');
      await dialog(page, 'Tunnel').waitFor({ state: 'detached' });
      await page.locator('aside').getByRole('button', { name: 'Importieren', exact: true }).click();
      await dialog(page).waitFor();
      await page.getByText('Termius').first().waitFor();
      await shot('import');
      await page.keyboard.press('Escape');
      await dialog(page).waitFor({ state: 'detached' });
      await openSettings(page);
      await page
        .getByRole('navigation', { name: 'Bereiche' })
        .getByRole('button', { name: 'Import & Export' })
        .click();
      await page.getByRole('button', { name: 'Exportieren…' }).click();
      await dialog(page, 'Exportieren').waitFor();
      await shot('export');
    },
  },
  {
    name: 'hostkeys',
    run: async (page, shot) => {
      await ready(page);
      await hostsLoaded(page);
      await setBehavior(page, 'h-pve', 'unknown-key');
      await hostButton(page, 'pve').click();
      await dialog(page, 'Unbekannter Host-Key').waitFor();
      await shot('trust-hostkey');
      await page.keyboard.press('Escape');
      await dialog(page, 'Unbekannter Host-Key').waitFor({ state: 'detached' });
      await setBehavior(page, 'h-vps', 'changed');
      await hostButton(page, 'vps').click();
      await dialog(page, 'Der Host-Key hat sich geändert').waitFor();
      await shot('hostkey-changed');
      await page.keyboard.press('Escape');
      await setBehavior(page, 'h-nas', 'password');
      await hostButton(page, 'nas').click();
      await page
        .getByRole('dialog')
        .getByLabel(/Passwort/)
        .first()
        .waitFor();
      await shot('password-prompt');
    },
  },
  {
    name: 'vault',
    fake: { vault: 'locked' },
    run: async (page, shot) => {
      await hostsLoaded(page);
      await dialog(page).waitFor();
      await shot('vault-unlock');
    },
  },
  {
    name: 'files',
    run: async (page, shot) => {
      await ready(page);
      await hostsLoaded(page);
      await page.getByRole('radio', { name: /Business/ }).click();
      await hostButton(page, 'web-01').hover();
      await page.getByRole('button', { name: 'Dateien auf web-01' }).click();
      await page.locator(CSS.remotePane).getByText('docker-compose.yml').waitFor();
      await page.locator(CSS.filePane).first().getByText('screenshot.png').waitFor();
      await page.mouse.move(640, 790);
      await shot('files');
    },
  },
  {
    name: 'onboarding',
    run: async (page, shot) => {
      await ready(page);
      await openSettings(page);
      await page.getByRole('button', { name: 'Einrichtung erneut starten' }).click();
      await dialog(page).waitFor();
      await shot('onboarding');
    },
  },
];

// ── UwUKeygen ───────────────────────────────────────────────────────────────

const KEYGEN = [
  {
    name: 'keygen',
    // UwUKeygen's window size (apps/keygen/src-tauri/tauri.conf.json).
    viewport: { width: 720, height: 860 },
    run: async (page, shot) => {
      await page.getByRole('heading', { name: 'Neuer SSH-Schlüssel' }).waitFor();
      await shot('keygen-settings');
      await page
        .getByRole('radio', { name: 'Erweitert' })
        .or(page.getByRole('tab', { name: 'Erweitert' }))
        .first()
        .click();
      await shot('keygen-advanced');
      await page.getByRole('button', { name: 'Weiter' }).click();
      await page.getByRole('heading', { name: 'Zufall sammeln' }).waitFor();
      await shot('keygen-entropy');
      await page.getByRole('button', { name: 'Ohne Maus erzeugen' }).click();
      await page.getByRole('heading', { name: 'Dein neuer Schlüssel' }).waitFor();
      await shot('keygen-done');
    },
  },
];

// ── Setup ───────────────────────────────────────────────────────────────────

const SETUP = [
  {
    name: 'setup-install',
    viewport: { width: 720, height: 520 },
    run: async (page, shot) => {
      await page.getByRole('button', { name: 'Installieren' }).waitFor();
      await shot('setup-install');
      await page.getByRole('button', { name: 'Optionen' }).click();
      await shot('setup-options');
      await page.getByRole('button', { name: 'Installieren' }).click();
      await page.getByText('Nyu richtet alles ein').waitFor();
      await page.waitForTimeout(900);
      await shot('setup-working');
      await page.getByRole('button', { name: 'UwUSSH starten' }).waitFor({ timeout: 15_000 });
      await shot('setup-done');
    },
  },
  {
    name: 'setup-update',
    path: '/?mode=update',
    viewport: { width: 720, height: 520 },
    run: async (page, shot) => {
      // Update mode starts on its own (no welcome screen) and reloads when done.
      await page.getByText('Nyu bringt UwUSSH auf den neuesten Stand').waitFor();
      await page.waitForTimeout(900);
      await shot('setup-update-working');
      await page.getByText('Alles frisch!').waitFor({ timeout: 15_000 });
      await shot('setup-update-done');
    },
  },
  {
    name: 'setup-running',
    path: '/?running',
    viewport: { width: 720, height: 520 },
    run: async (page, shot) => {
      await page.getByRole('button', { name: 'Installieren' }).click();
      await page.getByText('UwUSSH ist gerade offen').waitFor();
      await shot('setup-running');
    },
  },
  {
    name: 'setup-uninstall',
    path: '/?mode=uninstall',
    viewport: { width: 720, height: 520 },
    run: async (page, shot) => {
      await page.getByText('Schade, dass du gehst').waitFor();
      await shot('setup-uninstall');
    },
  },
  {
    name: 'setup-error',
    path: '/?fail',
    viewport: { width: 720, height: 520 },
    run: async (page, shot) => {
      await page.getByRole('button', { name: 'Installieren' }).click();
      await page.getByText('Hoppla, das hat nicht geklappt').waitFor({ timeout: 15_000 });
      await shot('setup-error');
    },
  },
];

// ── Main ────────────────────────────────────────────────────────────────────

const APPS = {
  desktop: { scenarios: DESKTOP, variants: ['light', 'dark', 'hc', 'hcdark'] },
  keygen: { scenarios: KEYGEN, variants: ['light', 'dark', 'hc', 'hcdark'] },
  setup: { scenarios: SETUP, variants: ['light'], fake: false },
};

let failures = 0;
for (const app of apps) {
  const spec = APPS[app];
  if (!spec) throw new Error(`unknown app ${app}`);
  const dir = join(outBase, `ssh-${app}`, label);
  mkdirSync(dir, { recursive: true });
  const { server, origin } = await startVite(join(appsDir, app));
  try {
    failures += await runApp({ app, origin, dir, ...spec });
  } finally {
    await server.close();
  }
}
await browser.close();
process.exit(failures ? 1 : 0);

// Docker (from the host; CHECKOUT is any UwUSSH-Client checkout with
// `pnpm install --frozen-lockfile` done, OUT the folder for the PNGs):
//
//   docker run --rm --init --ipc=host --user "$(id -u):$(id -g)" -e HOME=/tmp \
//     -v "$CHECKOUT":/work -v "$OUT":/out -w /work \
//     mcr.microsoft.com/playwright:v1.63.0-noble bash -c '
//       npm i --no-save --prefix /tmp/pw playwright@1.63.0 >/dev/null &&
//       NODE_PATH=/tmp/pw/node_modules node apps/desktop/e2e/shots.mjs \
//         --app all --out /out --label before'
