// The whole end-to-end run, in one command:
//
//   node apps/desktop/e2e/run.mjs
//
// Starts the dev SSH server with a fresh host key, starts the app in dev mode
// against a throwaway database with WebView2's DevTools port open on
// 127.0.0.1, runs phase A, rebuilds the server's host key the way a
// reinstalled server would, runs phase B, then swaps in a seeded database and a
// key-authorizing server for phase C (logging in with a key from the vault),
// then reads phase A's export back into a fresh database and imports a
// fixture ~/.ssh/config for phase D, then connects two app instances to a
// real UwUSSH server for phase E (sync), and stops everything again.
//
//   node apps/desktop/e2e/run.mjs --only=e   just phase E
//
// Phase E needs the server built next to this repository:
// ../UwUSSH-Server/target/debug/uwussh-server.exe (or UWUSSH_SERVER_EXE).
//
// Windows only: it drives WebView2 over the Chrome DevTools Protocol.

import { execSync, spawn, spawnSync } from 'node:child_process';
import {
  existsSync,
  mkdirSync,
  openSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const desktop = join(here, '..');
const repo = join(desktop, '..', '..');
const runDir = join(here, '.run');
const hostKey = join(tmpdir(), 'uwussh-dev-sshd-host-ed25519');
const sshdExe = join(repo, 'target', 'debug', 'examples', 'dev_sshd.exe');
const seedExe = join(repo, 'target', 'debug', 'examples', 'seed_vault_key.exe');
const authorizedKeys = join(runDir, 'authorized_key.pub');
/** What dev_sshd serves over SFTP, and a folder for downloads and the export. */
const filesDir = join(runDir, 'files');
const workDir = join(runDir, 'work');
const only = process.argv.find((arg) => arg.startsWith('--only='))?.slice('--only='.length);
const serverExe =
  process.env.UWUSSH_SERVER_EXE ??
  join(repo, '..', 'UwUSSH-Server', 'target', 'debug', 'uwussh-server.exe');
const appExe = join(repo, 'target', 'debug', 'uwussh-desktop.exe');

rmSync(runDir, { recursive: true, force: true });
mkdirSync(runDir, { recursive: true });
mkdirSync(workDir, { recursive: true });
mkdirSync(join(here, 'shots'), { recursive: true });

const children = [];

function start(name, command, args, options = {}) {
  const log = join(runDir, `${name}.log`);
  // Straight into the file, not piped through this process. A pipe is only
  // drained while this event loop runs, so the phases saw a stale server log —
  // and a busy server would eventually block on a full pipe.
  const fd = openSync(log, 'w');
  const child = spawn(command, args, {
    ...options,
    shell: command === 'pnpm',
    stdio: ['ignore', fd, fd],
  });
  children.push(child);
  return { child, log };
}

function stop(child) {
  // /T takes the whole tree: pnpm → vite and cargo → the app.
  spawnSync('taskkill', ['/PID', String(child.pid), '/T', '/F']);
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function until(condition, what, timeout = 600_000) {
  const started = Date.now();
  while (!(await condition())) {
    if (Date.now() - started > timeout) throw new Error(`gave up waiting for ${what}`);
    await sleep(500);
  }
}

async function startSshd(name, env = {}) {
  rmSync(hostKey, { force: true });
  const sshd = start(name, sshdExe, [], {
    cwd: repo,
    env: { ...process.env, UWUSSH_DEV_SSHD_FILES: filesDir, ...env },
  });
  await until(
    () => existsSync(sshd.log) && readFileSync(sshd.log, 'utf8').includes('listening on'),
    'dev_sshd',
  );
  const fingerprint = readFileSync(sshd.log, 'utf8').match(/SHA256:[A-Za-z0-9+/]+/)?.[0];
  if (!fingerprint) throw new Error('dev_sshd printed no fingerprint');
  return { ...sshd, fingerprint };
}

/** Wait until the app's DevTools endpoint answers with the app's page. */
async function waitForApp(port = 9223) {
  await until(async () => {
    try {
      const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
      return list.some((target) => target.url.startsWith('http://localhost:1420'));
    } catch {
      return false;
    }
  }, 'the app');
}

/** Wait until the DevTools endpoint is gone, so a fresh app can take the port. */
async function waitForNoApp() {
  await until(async () => {
    try {
      await fetch('http://127.0.0.1:9223/json/list');
      return false;
    } catch {
      return true;
    }
  }, 'the app to stop');
}

function startApp(name, db, extraEnv = {}) {
  return start(name, 'pnpm', ['tauri', 'dev'], {
    cwd: desktop,
    env: {
      ...process.env,
      UWUSSH_DB: db,
      ...extraEnv,
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:
        '--remote-debugging-port=9223 --remote-debugging-address=127.0.0.1',
      // A WebView2 folder of its own: an installed UwUSSH that is running
      // shares the default one, and its browser process would ignore the
      // debugging port above.
      WEBVIEW2_USER_DATA_FOLDER: join(runDir, `webview-${name}`),
    },
  });
}

/** Run one phase without blocking this event loop, and report whether it passed. */
function phase(script, args) {
  console.log(`\n── ${script} ──`);
  return new Promise((resolve) => {
    spawn(process.execPath, [join(here, script), ...args], { stdio: 'inherit' }).on(
      'exit',
      (code) => resolve(code === 0),
    );
  });
}

/**
 * Phase E: a UwUSSH server on this machine with its own certificate, and two
 * app instances — one through `pnpm tauri dev`, one straight from the debug
 * binary that build left behind, on its own DevTools port and its own
 * database, against the same dev server page.
 */
async function phaseE() {
  if (!existsSync(serverExe)) {
    console.log(`\nphase E skipped: no ${serverExe} (build UwUSSH-Server first)`);
    return false;
  }
  const data = join(runDir, 'server');
  mkdirSync(data, { recursive: true });
  const serverEnv = {
    ...process.env,
    UWUSSH_DATA: data,
    UWUSSH_LISTEN: '127.0.0.1:18443',
    UWUSSH_PUBLIC: 'https://127.0.0.1:18443',
    UWUSSH_UPDATE_CHECK: 'off',
  };
  const invite = execSync(`"${serverExe}" invite`, { env: serverEnv, encoding: 'utf8' });
  const setupCode = invite.match(/uwu1_[A-Za-z0-9_-]+/)?.[0];
  if (!setupCode) throw new Error(`the server printed no setup code:\n${invite}`);
  const server = start('uwussh-server', serverExe, [], { env: serverEnv });
  await until(async () => {
    try {
      // Its own certificate: nothing here trusts it, which is the point.
      return readFileSync(server.log, 'utf8').includes('server ready');
    } catch {
      return false;
    }
  }, 'the UwUSSH server');

  const sshConfig = join(runDir, 'ssh_config-e');
  writeFileSync(sshConfig, 'Host dev-sshd\n  HostName 127.0.0.1\n  Port 2222\n  User uwu\n');
  const first = startApp('app-e1', join(runDir, 'e1.db'), { UWUSSH_SSH_CONFIG: sshConfig });
  await waitForApp();
  const second = start('app-e2', appExe, [], {
    cwd: desktop,
    env: {
      ...process.env,
      UWUSSH_DB: join(runDir, 'e2.db'),
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:
        '--remote-debugging-port=9224 --remote-debugging-address=127.0.0.1',
      WEBVIEW2_USER_DATA_FOLDER: join(runDir, 'webview-app-e2'),
    },
  });
  await waitForApp(9224);
  const passed = await phase('phase-e.mjs', [setupCode, '9223', '9224']);
  stop(second.child);
  stop(first.child);
  stop(server.child);
  return passed;
}

try {
  if (only === 'e') {
    const ok = await phaseE();
    console.log(ok ? '\nEND TO END OK' : '\nEND TO END FAILED');
    for (const child of children) stop(child);
    spawnSync('taskkill', ['/IM', 'uwussh-desktop.exe', '/T', '/F']);
    process.exit(ok ? 0 : 1);
  }
  // Always: cargo only rebuilds what changed, and a stale server tests nothing.
  execSync('cargo build -p uwussh-core --example dev_sshd --example seed_vault_key', {
    cwd: repo,
    stdio: 'inherit',
  });

  let sshd = await startSshd('sshd-a');
  // Debug builds read UWUSSH_E2E_SAVE_DIR instead of showing a save dialog.
  let app = startApp('app', join(runDir, 'e2e.db'), { UWUSSH_E2E_SAVE_DIR: workDir });

  console.log('waiting for the app (the first build takes a while)…');
  await waitForApp();

  const passedA = await phase('phase-a.mjs', [sshd.fingerprint, sshd.log, filesDir, workDir]);

  // The server is "reinstalled": same address, new host key.
  const trusted = sshd.fingerprint;
  stop(sshd.child);
  sshd = await startSshd('sshd-b');
  const passedB = passedA && (await phase('phase-b.mjs', [trusted, sshd.fingerprint]));

  // Phase C: a host that logs in with a key from the vault. A separate app on
  // a separate database, seeded to hold one such host with the vault locked —
  // exactly what the app finds on start — against a server that authorizes the
  // seeded key.
  let passedC = passedA && passedB;
  if (passedC) {
    stop(app.child);
    stop(sshd.child);
    await waitForNoApp();

    const vaultDb = join(runDir, 'vault.db');
    for (const suffix of ['', '-wal', '-shm']) rmSync(vaultDb + suffix, { force: true });
    execSync(`"${seedExe}" "${vaultDb}" hunter2 dev-sshd 127.0.0.1 2222 uwu "${authorizedKeys}"`, {
      cwd: repo,
      stdio: 'inherit',
    });

    sshd = await startSshd('sshd-c', { UWUSSH_DEV_SSHD_AUTHORIZED_KEYS: authorizedKeys });
    app = startApp('app-c', vaultDb);
    await waitForApp();
    passedC = await phase('phase-c.mjs', [sshd.log]);
  }

  // Phase D: import from ~/.ssh/config and connect to what it brought. A
  // secretless source, so it needs no vault; UWUSSH_SSH_CONFIG points the app
  // at a fixture instead of the real file. With Termius also present the import
  // dialog shows its source picker, which nothing else exercises.
  let passedD = passedC;
  if (passedD) {
    stop(app.child);
    stop(sshd.child);
    await waitForNoApp();

    const sshConfig = join(runDir, 'ssh_config');
    writeFileSync(sshConfig, 'Host dev-sshd\n  HostName 127.0.0.1\n  Port 2222\n  User uwu\n');
    const opensshDb = join(runDir, 'openssh.db');
    for (const suffix of ['', '-wal', '-shm']) rmSync(opensshDb + suffix, { force: true });

    const exported = readdirSync(workDir).find((name) => name.endsWith('.uwussh'));
    if (!exported) throw new Error('phase A left no export file');
    sshd = await startSshd('sshd-d');
    app = startApp('app-d', opensshDb, {
      UWUSSH_SSH_CONFIG: sshConfig,
      // Debug builds read UWUSSH_E2E_OPEN_FILE instead of showing an open dialog.
      UWUSSH_E2E_OPEN_FILE: join(workDir, exported),
    });
    await waitForApp();
    passedD = await phase('phase-d.mjs', [sshd.log]);
  }

  let passedE = passedD;
  if (passedE) {
    stop(app.child);
    stop(sshd.child);
    await waitForNoApp();
    passedE = await phaseE();
  }

  const ok = passedA && passedB && passedC && passedD && passedE;
  console.log(ok ? '\nEND TO END OK' : '\nEND TO END FAILED');
  process.exitCode = ok ? 0 : 1;
} catch (error) {
  console.error(error);
  process.exitCode = 1;
} finally {
  for (const child of children) stop(child);
  // `pnpm tauri dev` launches the app binary and its WebView2 through cargo, so
  // they are not descendants of the pnpm process a tree kill sees. Sweep the
  // app binary by name, which takes its WebView2 with it, or the next run finds
  // port 9223 still held.
  spawnSync('taskkill', ['/IM', 'uwussh-desktop.exe', '/T', '/F']);
}
