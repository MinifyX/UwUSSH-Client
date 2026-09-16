// Phase C: a host that logs in with a key from the vault.
//
// The database was seeded with one such host and the vault left locked, and
// dev_sshd authorizes the seeded key. So connecting has to unlock the vault
// first (wrong master password, then right), trust the new host key, and then
// log in with the key — never a password. The server's log proves it.
import { readFileSync } from 'node:fs';
import { check, connect, failed, sleep } from './cdp.mjs';

const SHOTS = new URL('./shots/', import.meta.url).pathname.replace(/^\/([A-Z]:)/, '$1');
const SERVER_LOG = process.argv[2];
const log = () => readFileSync(SERVER_LOG, 'utf8');

const terminalHas = (text) =>
  `(() => { const t = window.__uwusshDriver.term.buffer.active; for (let i = 0; i < t.length; i++) if (t.getLine(i)?.translateToString().includes(${JSON.stringify(text)})) return true; })()`;

const page = await connect();
const shot = (name) => page.screenshot(`${SHOTS}${name}.png`);

await page.waitFor(`document.querySelector('.sidebar')`, { what: 'app shell' });
await page.waitFor(`window.__uwusshDriver`, { what: 'dev driver hook' });
await sleep(500);

check(
  'the seeded vault host is in the list',
  await page.eval(
    `[...document.querySelectorAll('.host-name')].some(e => e.textContent === 'dev-sshd')`,
  ),
);
await shot('c1-start');

// ── Connect: the vault is locked, so it must be unlocked first ──────────────
await page.click('.host .host-name', 'dev-sshd');
await page.waitFor(`document.querySelector('.modal-title')?.textContent === 'Tresor entsperren'`, {
  what: 'unlock dialog',
});
check('a vault host asks to unlock before connecting', true);
check(
  'no password went to the server yet',
  !log().includes('password for uwu'),
  'server saw a password attempt',
);
await shot('c2-unlock');

// A wrong master password is reported, and the prompt comes back.
await page.type('nope');
await page.key('Enter');
await page.waitFor(`document.querySelector('.field-error')?.textContent.includes('falsch')`, {
  what: 'wrong master password notice',
});
check('a wrong master password is reported', true);

await page.type('hunter2');
await page.key('Enter');

// ── Trust the host key, then the key login goes through ─────────────────────
await page.waitFor(
  `document.querySelector('.modal-title')?.textContent === 'Unbekannter Host-Key'`,
  { what: 'trust dialog after unlock' },
);
check('after unlocking, the host key is offered for trust', true);
await page.click('.modal-footer button', 'Vertrauen und verbinden');

await page.waitFor(`document.querySelector('.session-title b')?.textContent === 'dev-sshd'`, {
  what: 'connected title',
  timeout: 15_000,
});
await page.waitFor(terminalHas('toy shell'), { what: 'banner from the server', timeout: 15_000 });
check('connected with the key from the vault', true);
await shot('c3-connected');

// ── The server logged a key login, never a password ─────────────────────────
check(
  'the server accepted the public key',
  log().includes('publickey for uwu: accepted'),
  'no accepted publickey in the server log',
);
check(
  'the server was never sent a password',
  !log().includes('password for uwu'),
  'the server saw a password attempt',
);

// A typed command still works over the key session.
await page.type('whoami');
await page.key('Enter');
await page.waitFor(terminalHas('uwu'), { what: 'command output', timeout: 15_000 });
check('a command runs over the key session', true);

page.close();
console.log(failed() === 0 ? 'PHASE C OK' : `PHASE C: ${failed()} FAILED`);
process.exit(failed() === 0 ? 0 : 1);
