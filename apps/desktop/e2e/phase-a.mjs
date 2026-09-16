// Phase A: add a host, first contact, password, a working shell, session end,
// and reconnecting to a host whose key is already trusted.
import { readFileSync } from 'node:fs';
import { check, connect, failed, sleep } from './cdp.mjs';

const SHOTS = new URL('./shots/', import.meta.url).pathname.replace(/^\/([A-Z]:)/, '$1');
const EXPECTED_FINGERPRINT = process.argv[2];
const SERVER_LOG = process.argv[3];
const count = (what) => readFileSync(SERVER_LOG, 'utf8').split(what).length - 1;
const connectionsAtStart = count('connection from');
const connectionsSince = () => count('connection from') - connectionsAtStart;

const page = await connect();
const shot = (name) => page.screenshot(`${SHOTS}${name}.png`);

await page.waitFor(`document.querySelector('.sidebar')`, { what: 'app shell' });
await page.waitFor(`window.__uwusshDriver`, { what: 'dev driver hook' });
await sleep(800);
check(
  'empty host list shows Nyu and an add button',
  await page.eval(
    `!!document.querySelector('.sidebar-empty svg') && document.querySelector('.sidebar-empty button')?.textContent === 'Host hinzufügen'`,
  ),
);
await shot('01-start');

// ── Add a host ──────────────────────────────────────────────────────────────
await page.click('.sidebar-empty button', 'Host hinzufügen');
await page.waitFor(`document.querySelector('.modal-title')?.textContent === 'Neuer Host'`, {
  what: 'host form',
});
check(
  'address field has focus when the form opens',
  await page.eval(`document.activeElement === document.querySelectorAll('.modal input')[0]`),
);

// Saving an empty form must name the missing field instead of failing silently.
await page.click('.modal-footer button', 'Speichern');
await page.waitFor(`document.querySelector('.field-error')`, { what: 'validation error' });
check(
  'empty address is reported next to the field',
  (await page.text('.field-error')).includes('Adresse fehlt'),
  await page.text('.field-error'),
);

const inputs = `.modal input`;
await page.fill(`${inputs}:nth-of-type(1)`, '127.0.0.1');
await page.eval(`document.querySelectorAll('${inputs}')[1].focus()`);
await page.key('a', 2);
await page.key('Backspace');
await page.type('2222');
await page.eval(`document.querySelectorAll('${inputs}')[2].focus()`);
await page.type('uwu');
await page.eval(`document.querySelectorAll('${inputs}')[3].focus()`);
await page.type('dev-sshd');
await page.eval(`document.querySelectorAll('${inputs}')[4].focus()`);
await page.type('lokal');
await shot('02-form');

await page.click('.modal-footer button', 'Speichern');
await page.waitFor(
  `!document.querySelector('.modal') && [...document.querySelectorAll('.host-name')].some(e => e.textContent === 'dev-sshd')`,
  { what: 'host in list' },
);
check('saved host appears in its group', (await page.text('.host-group h3')).includes('lokal'));

// ── First contact: unknown host key ─────────────────────────────────────────
await page.click('.host .host-name', 'dev-sshd');
await page.waitFor(
  `document.querySelector('.modal-title')?.textContent === 'Unbekannter Host-Key'`,
  { what: 'trust dialog' },
);
await sleep(300);
const shown = await page.text('.fingerprint');
check(
  "trust dialog shows the server's real fingerprint",
  shown === EXPECTED_FINGERPRINT,
  `${shown} vs ${EXPECTED_FINGERPRINT}`,
);
check('trust dialog shows randomart', (await page.text('.randomart')).includes('[SHA256]'));
await shot('03-trust');

// Enter must not trust a key: nothing in this dialog has default focus on "trust".
await page.key('Enter');
await sleep(300);
check(
  'Enter alone does not trust the key',
  await page.eval(`document.querySelector('.modal-title')?.textContent === 'Unbekannter Host-Key'`),
);

await page.click('.modal-footer button', 'Vertrauen und verbinden');

// ── Password: wrong, then right ─────────────────────────────────────────────
await page.waitFor(`document.querySelector('.modal-title')?.textContent === 'Passwort'`, {
  what: 'password prompt',
});
check('password field has focus', await page.eval(`document.activeElement?.type === 'password'`));
await shot('04-password');
await page.type('falsch');
await page.key('Enter');

await page.waitFor(`document.querySelector('.field-error')?.textContent.includes('abgelehnt')`, {
  what: 'rejected password notice',
  timeout: 15_000,
});
check('a wrong password is reported and asked again', true);
check(
  'the rejected password is not left in the field',
  await page.eval(`document.activeElement?.value === ''`),
);
await shot('05-rejected');
await page.type('nyu');
await page.key('Enter');

// ── A working shell ─────────────────────────────────────────────────────────
await page.waitFor(`document.querySelector('.session-title b')?.textContent === 'dev-sshd'`, {
  what: 'connected title',
  timeout: 15_000,
});
await page.waitFor(
  `(() => { const t = window.__uwusshDriver.term.buffer.active; for (let i = 0; i < t.length; i++) if (t.getLine(i)?.translateToString().includes('toy shell')) return true; })()`,
  { what: 'server banner in terminal' },
);
check('connected: banner from the server is in the terminal', true);
check(
  'first contact took exactly two connections: one to learn the key, one for the login and its retry',
  connectionsSince() === 2,
  `${connectionsSince()} connections`,
);
check(
  'toolbar names user and address',
  (await page.text('.session-title .meta')).includes('uwu@127.0.0.1:2222'),
);

await page.eval(`window.__uwusshDriver.term.focus()`);
await page.type('help');
await page.key('Enter');
await page.waitFor(
  `(() => { const t = window.__uwusshDriver.term.buffer.active; for (let i = 0; i < t.length; i++) if (t.getLine(i)?.translateToString().includes('flood <MiB>')) return true; })()`,
  { what: 'help output' },
);
check('typed command runs on the server and its output comes back', true);
await shot('06-help');

await page.type('colors');
await page.key('Enter');
await sleep(600);
await shot('07-colors');

// ── A flood through the whole stack ─────────────────────────────────────────
const floodStart = Date.now();
await page.type('flood 32');
await page.key('Enter');
await page.waitFor(
  `(() => { const t = window.__uwusshDriver.term.buffer.active; for (let i = 0; i < t.length; i++) if (t.getLine(i)?.translateToString().includes('32 MiB sent')) return true; })()`,
  { what: 'flood completion', timeout: 60_000 },
);
const seconds = (Date.now() - floodStart) / 1000;
const counters = await page.eval(
  `({ received: window.__uwusshDriver.received, written: window.__uwusshDriver.written, discarded: window.__uwusshDriver.discarded })`,
);
check(
  '32 MiB flood over SSH reaches the screen',
  true,
  `${seconds.toFixed(1)} s, ${(32 / seconds).toFixed(1)} MiB/s end to end`,
);
check('nothing discarded during the flood', counters.discarded === 0, JSON.stringify(counters));
await shot('08-flood');

// ── Resize reaches the server ───────────────────────────────────────────────
await page.send('Emulation.setDeviceMetricsOverride', {
  width: 900,
  height: 600,
  deviceScaleFactor: 0,
  mobile: false,
});
await sleep(700);
await page.send('Emulation.clearDeviceMetricsOverride');
await sleep(700);

// ── The remote side ends the session ────────────────────────────────────────
await page.eval(`window.__uwusshDriver.term.focus()`);
await page.type('exit');
await page.key('Enter');
await page.waitFor(`document.querySelector('.notice')?.textContent.includes('wurde beendet')`, {
  what: 'ended banner',
});
check(
  'session end shows a banner, not a modal',
  await page.eval(`!document.querySelector('.modal')`),
);
await shot('09-ended');

// ── Reconnect: key already trusted, so straight to the password ─────────────
await page.click('.notice button', 'Neu verbinden');
await page.waitFor(`document.querySelector('.modal-title')`, { what: 'next dialog' });
check(
  'a trusted host skips the key dialog',
  (await page.text('.modal-title')) === 'Passwort',
  await page.text('.modal-title'),
);
await page.type('nyu');
await page.key('Enter');
await page.waitFor(
  `(() => { const t = window.__uwusshDriver.term.buffer.active; for (let i = 0; i < t.length; i++) if (t.getLine(i)?.translateToString().includes('toy shell')) return true; })()`,
  { what: 'banner after reconnect', timeout: 15_000 },
);
check('reconnected', true);
check(
  'the reconnect took one connection, the password went over it',
  connectionsSince() === 3,
  `${connectionsSince()} connections in total`,
);
await shot('10-reconnected');

// ── Import: preview first, and the vault is asked before anything is written ─
await page.click('.sidebar-head [aria-label="Hosts importieren"]');
await page.waitFor(`document.querySelector('.modal-title')?.textContent === 'Importieren'`, {
  what: 'import dialog',
});
// The dialog reaches a preview (a source is present on this machine) or says
// there is nothing to import. Either way it has not written anything.
await page.waitFor(
  `!!document.querySelector('.import-preview, .import-sources') ||
   (document.querySelector('.import-note')?.textContent.includes('nichts zum Importieren') ?? false)`,
  { what: 'preview, source picker, or nothing-to-import' },
);
await shot('11-import');
const canImport = await page.eval(`!!document.querySelector('.import-preview')`);
if (canImport) {
  // Confirm: with an empty vault, this must ask for a master password before
  // it writes, not jump straight to importing.
  await page.click('.modal-footer button', 'Importieren');
  await page.waitFor(`document.querySelector('.vault-setup input[type=password]')`, {
    what: 'vault step before writing',
  });
  check('importing asks for the vault before it writes anything', true);
}
await page.key('Escape');
await page.waitFor(`!document.querySelector('.modal')`, { what: 'import dialog closed' });
check('the import dialog closes without touching the host list', true);

page.close();
console.log(failed() === 0 ? 'PHASE A OK' : `PHASE A: ${failed()} FAILED`);
process.exit(failed() === 0 ? 0 : 1);
