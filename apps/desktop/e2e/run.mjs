// The whole end-to-end run, in one command:
//
//   node apps/desktop/e2e/run.mjs
//
// Starts the dev SSH server with a fresh host key, starts the app in dev mode
// against a throwaway database with WebView2's DevTools port open on
// 127.0.0.1, runs phase A, rebuilds the server's host key the way a
// reinstalled server would, runs phase B, and stops everything again.
//
// Windows only: it drives WebView2 over the Chrome DevTools Protocol.

import { execSync, spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, openSync, readFileSync, rmSync } from 'node:fs';
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

rmSync(runDir, { recursive: true, force: true });
mkdirSync(runDir, { recursive: true });
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
  const sshd = start(name, sshdExe, [], { cwd: repo, env: { ...process.env, ...env } });
  await until(
    () => existsSync(sshd.log) && readFileSync(sshd.log, 'utf8').includes('listening on'),
    'dev_sshd',
  );
  const fingerprint = readFileSync(sshd.log, 'utf8').match(/SHA256:[A-Za-z0-9+/]+/)?.[0];
  if (!fingerprint) throw new Error('dev_sshd printed no fingerprint');
  return { ...sshd, fingerprint };
}

/** Wait until the app's DevTools endpoint answers with the app's page. */
async function waitForApp() {
  await until(async () => {
    try {
      const list = await (await fetch('http://127.0.0.1:9223/json/list')).json();
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

function startApp(name, db) {
  return start(name, 'pnpm', ['tauri', 'dev'], {
    cwd: desktop,
    env: {
      ...process.env,
      UWUSSH_DB: db,
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:
        '--remote-debugging-port=9223 --remote-debugging-address=127.0.0.1',
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

try {
  if (!existsSync(sshdExe) || !existsSync(seedExe)) {
    execSync('cargo build -p uwussh-core --example dev_sshd --example seed_vault_key', {
      cwd: repo,
      stdio: 'inherit',
    });
  }

  let sshd = await startSshd('sshd-a');
  let app = startApp('app', join(runDir, 'e2e.db'));

  console.log('waiting for the app (the first build takes a while)…');
  await waitForApp();

  const passedA = await phase('phase-a.mjs', [sshd.fingerprint, sshd.log]);

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

  const ok = passedA && passedB && passedC;
  console.log(ok ? '\nEND TO END OK' : '\nEND TO END FAILED');
  process.exitCode = ok ? 0 : 1;
} catch (error) {
  console.error(error);
  process.exitCode = 1;
} finally {
  for (const child of children) stop(child);
}
