// Phase D: import from ~/.ssh/config, then connect to what it brought.
//
// UWUSSH_SSH_CONFIG points the app at a fixture config that describes the dev
// server. Termius is also present, so the import dialog shows its source
// picker — the one path no other phase exercises. The OpenSSH source has no
// secrets, so the import needs no vault; the imported host then connects with
// a password, trusting the server's key on the way.
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

// ── Import: pick a source, preview it, write it — no vault ──────────────────
await page.click('.sidebar-head [aria-label="Hosts importieren"]');
await page.waitFor(`document.querySelector('.modal-title')?.textContent === 'Importieren'`, {
  what: 'import dialog',
});
await page.waitFor(`document.querySelector('.import-sources')`, { what: 'source picker' });
check(
  'the import dialog offers a source picker when more than one is available',
  await page.eval(`document.querySelectorAll('.import-source').length >= 2`),
);
await shot('d1-sources');

await page.click('.import-source', 'OpenSSH');
await page.waitFor(`document.querySelector('.import-preview')`, { what: 'ssh_config preview' });
check(
  'the ssh_config preview counts the fixture host',
  await page.eval(
    `[...document.querySelectorAll('.import-counts b')].some(e => e.textContent === '1')`,
  ),
);
await shot('d2-preview');

await page.click('.modal-footer button', 'Importieren');
await page.waitFor(
  `document.querySelector('.modal-title')?.textContent === 'Import abgeschlossen'`,
  {
    what: 'import done',
  },
);
check(
  'a secretless import never asked for the vault',
  await page.eval(`!document.querySelector('.vault-setup')`),
);
check('nothing sensitive was skipped', !log().includes('password for uwu'));
await page.click('.modal-footer button', 'Fertig');

await page.waitFor(
  `!document.querySelector('.modal') && [...document.querySelectorAll('.host-name')].some(e => e.textContent === 'dev-sshd')`,
  { what: 'imported host in list' },
);
check('the imported host is in the list', true);
await shot('d3-imported');

// ── Connect: trust the key, then the password from ssh_config's User ────────
await page.click('.host .host-name', 'dev-sshd');
await page.waitFor(
  `document.querySelector('.modal-title')?.textContent === 'Unbekannter Host-Key'`,
  { what: 'trust dialog' },
);
await page.click('.modal-footer button', 'Vertrauen und verbinden');

await page.waitFor(`document.querySelector('.modal-title')?.textContent === 'Passwort'`, {
  what: 'password prompt',
  timeout: 15_000,
});
check('an imported host with no key asks for a password', true);
await page.type('nyu');
await page.key('Enter');

await page.waitFor(terminalHas('toy shell'), { what: 'connected banner', timeout: 15_000 });
check('connected to the imported host', true);
check(
  'the server accepted the password',
  log().includes('password for uwu: accepted'),
  'no accepted password in the server log',
);
await shot('d4-connected');

page.close();
console.log(failed() === 0 ? 'PHASE D OK' : `PHASE D: ${failed()} FAILED`);
process.exit(failed() === 0 ? 0 : 1);
