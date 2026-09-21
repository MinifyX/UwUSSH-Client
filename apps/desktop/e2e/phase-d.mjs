// Phase D: a fresh database reads back phase A's export — sealed file,
// password, vault, workspaces, the stored password — and then imports from
// ~/.ssh/config, which brings the same server again and is skipped.
//
// UWUSSH_E2E_OPEN_FILE hands the export to the app's file dialog, and
// UWUSSH_SSH_CONFIG points it at a fixture config for the dev server. The dev
// server has a new host key since phase A, so the key from the export shows up
// as changed, and is accepted with the button.
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
await page.waitFor(`'__uwusshDriver' in window`, { what: 'dev driver hook' });
await sleep(500);

// ── Import the export file ──────────────────────────────────────────────────
await page.click('.sidebar-head [aria-label="Importieren"]');
await page.waitFor(`document.querySelector('.import-sources')`, { what: 'source picker' });
await page.click('.import-source', 'UwUSSH-Export');
await page.waitFor(
  `[...document.querySelectorAll('.modal-title')].pop()?.textContent === 'Importieren' && document.querySelector('.modal input[type=password]')`,
  {
    what: 'export password prompt',
  },
);
check('a sealed export asks for its password', true);
await page.click('.modal input[type=password]');
await page.type('wrong-password');
await page.key('Enter');
await page.waitFor(`document.querySelector('.field-error')?.textContent.includes('passt nicht')`, {
  what: 'wrong export password',
  timeout: 20_000,
});
check('a wrong export password is reported', true);
await page.click('.modal input[type=password]');
await page.type('export-pw-123');
await page.key('Enter');
await page.waitFor(`document.querySelector('.import-counts')`, {
  what: 'export preview',
  timeout: 20_000,
});
check(
  'the preview counts hosts and a stored password',
  (await page.text('.import-counts li')).includes('Passwörter'),
  await page.text('.import-counts li'),
);
await shot('d0-export-preview');
await page.click('.modal-footer button', 'Importieren');
await page.waitFor(
  `[...document.querySelectorAll('.modal-title')].pop()?.textContent === 'Tresor anlegen'`,
  {
    what: 'vault for the imported secrets',
  },
);
check('secrets from the export need a vault first', true);
await page.waitFor(`document.activeElement?.type === 'password'`, {
  what: 'master password focus',
});
await page.type('d-master');
await page.eval(
  `[...document.querySelectorAll('.modal')].pop().querySelectorAll('input[type=password]')[1].focus()`,
);
await page.type('d-master');
await page.key('Enter');
await page.waitFor(
  `[...document.querySelectorAll('.modal-title')].pop()?.textContent.startsWith('Import abgeschlossen')`,
  {
    what: 'export imported',
    timeout: 20_000,
  },
);
const restored = await page.eval(`window.__TAURI_INTERNALS__.invoke('list_hosts')`);
check(
  'hosts, workspaces and the stored password came back',
  restored.some((h) => h.name === 'dev-sshd' && h.hasPassword) &&
    restored.some((h) => h.name === 'nas' && h.workspace === 'business'),
  JSON.stringify(restored.map((h) => [h.name, h.workspace, h.hasPassword])),
);
await page.click('.modal-footer button', 'Fertig');
await page.waitFor(`!document.querySelector('.modal')`, { what: 'import dialog closed' });

// ── Import: pick a source, preview it, write it — no vault ──────────────────
await page.click('.sidebar-head [aria-label="Importieren"]');
await page.waitFor(
  `[...document.querySelectorAll('.modal-title')].pop()?.textContent === 'Importieren'`,
  {
    what: 'import dialog',
  },
);
await page.waitFor(`document.querySelector('.import-sources')`, { what: 'source picker' });
check(
  'the import dialog offers the sources it found and an export file',
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
  `[...document.querySelectorAll('.modal-title')].pop()?.textContent.startsWith('Import abgeschlossen')`,
  {
    what: 'import done',
  },
);
check(
  'the same server from ssh_config is skipped, not added twice',
  (await page.eval(`window.__TAURI_INTERNALS__.invoke('list_hosts')`)).filter(
    (h) => h.name === 'dev-sshd',
  ).length === 1,
);
check('nothing sensitive was skipped', !log().includes('password for uwu'));
await page.click('.modal-footer button', 'Fertig');

await page.waitFor(
  `!document.querySelector('.modal') && [...document.querySelectorAll('.host-name')].some(e => e.textContent === 'dev-sshd')`,
  { what: 'imported host in list' },
);
check('the imported host is in the list', true);
await shot('d3-imported');

// ── Connect: the exported key changed, the stored password logs in ──────────
await page.click('.host .host-name', 'dev-sshd');
await page.waitFor(
  `[...document.querySelectorAll('.modal-title')].pop()?.textContent === 'Der Host-Key hat sich geändert'`,
  { what: 'changed key from the export', timeout: 15_000 },
);
check('the exported trusted key came back and is checked', true);
await page.click('.modal-footer button', 'Neuen Schlüssel akzeptieren');

await page.waitFor(terminalHas('toy shell'), { what: 'connected banner', timeout: 15_000 });
check('connected to the imported host', true);
const accepted = log().includes('password for uwu: accepted');
check('the server accepted the password', accepted, accepted ? '' : 'none in the server log');
await shot('d4-connected');

page.close();
console.log(failed() === 0 ? 'PHASE D OK' : `PHASE D: ${failed()} FAILED`);
process.exit(failed() === 0 ? 0 : 1);
