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

async function startSshd(name) {
  rmSync(hostKey, { force: true });
  const sshd = start(name, sshdExe, [], { cwd: repo });
  await until(
    () => existsSync(sshd.log) && readFileSync(sshd.log, 'utf8').includes('listening on'),
    'dev_sshd',
  );
  const fingerprint = readFileSync(sshd.log, 'utf8').match(/SHA256:[A-Za-z0-9+/]+/)?.[0];
  if (!fingerprint) throw new Error('dev_sshd printed no fingerprint');
  return { ...sshd, fingerprint };
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
  if (!existsSync(sshdExe)) {
    execSync('cargo build -p uwussh-core --example dev_sshd', { cwd: repo, stdio: 'inherit' });
  }

  let sshd = await startSshd('sshd-a');
  start('app', 'pnpm', ['tauri', 'dev'], {
    cwd: desktop,
    env: {
      ...process.env,
      UWUSSH_DB: join(runDir, 'e2e.db'),
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:
        '--remote-debugging-port=9223 --remote-debugging-address=127.0.0.1',
    },
  });

  console.log('waiting for the app (the first build takes a while)…');
  await until(async () => {
    try {
      const list = await (await fetch('http://127.0.0.1:9223/json/list')).json();
      return list.some((target) => target.url.startsWith('http://localhost:1420'));
    } catch {
      return false;
    }
  }, 'the app');

  const passedA = await phase('phase-a.mjs', [sshd.fingerprint, sshd.log]);

  // The server is "reinstalled": same address, new host key.
  const trusted = sshd.fingerprint;
  stop(sshd.child);
  sshd = await startSshd('sshd-b');
  const passedB = passedA && (await phase('phase-b.mjs', [trusted, sshd.fingerprint]));

  console.log(passedA && passedB ? '\nEND TO END OK' : '\nEND TO END FAILED');
  process.exitCode = passedA && passedB ? 0 : 1;
} catch (error) {
  console.error(error);
  process.exitCode = 1;
} finally {
  for (const child of children) stop(child);
}
