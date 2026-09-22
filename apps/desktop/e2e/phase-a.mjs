// Phase A: add a host, first contact, password, a working shell, session end,
// reconnecting to a host whose key is already trusted, tabs, window controls
// and settings — then what beta.2 added: saving the password into a new vault,
// the sudo password helper, keyword highlighting, Ctrl+wheel zoom, the detected
// system, workspaces and dragging, the file browser, and an export.
import { existsSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { check, connect, failed, sleep } from './cdp.mjs';

const SHOTS = new URL('./shots/', import.meta.url).pathname.replace(/^\/([A-Z]:)/, '$1');
const EXPECTED_FINGERPRINT = process.argv[2];
const SERVER_LOG = process.argv[3];
/** The folder dev_sshd serves as the server's `/`. */
const SERVER_FILES = process.argv[4];
/** A scratch folder for downloads, and where the export lands. */
const WORK_DIR = process.argv[5];
const count = (what) => readFileSync(SERVER_LOG, 'utf8').split(what).length - 1;
const connectionsAtStart = count('connection from');
const connectionsSince = () => count('connection from') - connectionsAtStart;

const page = await connect();
const shot = (name) => page.screenshot(`${SHOTS}${name}.png`);
const terminalHas = (text) =>
  `(() => { const t = window.__uwusshDriver?.term.buffer.active; if (!t) return false; for (let i = 0; i < t.length; i++) if (t.getLine(i)?.translateToString().includes(${JSON.stringify(text)})) return true; })()`;

await page.waitFor(`document.querySelector('.sidebar')`, { what: 'app shell' });
await page.waitFor(`'__uwusshDriver' in window`, { what: 'dev driver hook' });
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
await page.waitFor(
  `[...document.querySelectorAll('.modal-title')].pop()?.textContent === 'Neuer Host'`,
  {
    what: 'host form',
  },
);
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
await page.eval(`document.querySelector('.modal input[list]').focus()`);
await page.type('lokal');
check(
  'the password stays empty, so it is asked on connect',
  await page.eval(`document.querySelector('.modal input[type=password]').value === ''`),
);
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
  `[...document.querySelectorAll('.modal-title')].pop()?.textContent === 'Unbekannter Host-Key'`,
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
  await page.eval(
    `[...document.querySelectorAll('.modal-title')].pop()?.textContent === 'Unbekannter Host-Key'`,
  ),
);

await page.click('.modal-footer button', 'Vertrauen und verbinden');

// ── Password: wrong, then right ─────────────────────────────────────────────
await page.waitFor(
  `[...document.querySelectorAll('.modal-title')].pop()?.textContent === 'Passwort'`,
  {
    what: 'password prompt',
  },
);
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
check(
  'the password prompt offers to keep it in the vault',
  await page.eval(`document.querySelector('.modal .check input')?.checked === true`),
);
// Not this time: the vault comes later, on purpose.
await page.click('.modal .check input');
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
await page.waitFor(`[...document.querySelectorAll('.modal-title')].pop()`, { what: 'next dialog' });
check(
  'a trusted host skips the key dialog',
  (await page.text('.modal-title')) === 'Passwort',
  await page.text('.modal-title'),
);
await page.click('.modal .check input');
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
check('the terminal fits: no row or column is cut off', await terminalFits());

// ── Tabs: a second connection to the same server, side by side ─────────────
/** Every row and column the terminal has is inside the box it is shown in. */
async function terminalFits() {
  return page.eval(`(() => {
    const term = window.__uwusshDriver.term;
    const cell = term._core._renderService.dimensions.css.cell;
    const screen = term.element.parentElement.getBoundingClientRect();
    return cell.height > 0
      && term.rows * cell.height <= screen.height + 0.5
      && term.cols * cell.width <= screen.width + 0.5;
  })()`);
}

const tabCount = () => page.eval(`document.querySelectorAll('.tab').length`);
const tabsBefore = await tabCount();
check(
  'nothing opened on start: the SSH connection is the only tab',
  tabsBefore === 1,
  `${tabsBefore} tabs`,
);

// A click on a host with an open tab brings that tab back; another tab to the
// same host is in the host's context menu.
await page.click('.host .host-name', 'dev-sshd');
await sleep(300);
check('clicking the connected host again opens no second tab', (await tabCount()) === tabsBefore);
await page.rightClick('.host .host-name', 'dev-sshd');
await page.waitFor(`document.querySelector('.context-menu')`, { what: 'host menu' });
check(
  'the host menu offers another tab',
  (await page.text('.context-menu [role=menuitem]')).includes('Weiteren Tab öffnen'),
  await page.text('.context-menu [role=menuitem]'),
);
await shot('10a-host-menu');
await page.click('.context-menu [role=menuitem]', 'Weiteren Tab öffnen');
await page.waitFor(
  `[...document.querySelectorAll('.modal-title')].pop()?.textContent === 'Passwort'`,
  {
    what: 'password prompt for the second tab',
    timeout: 15_000,
  },
);
check('"another tab" opens another tab', (await tabCount()) === tabsBefore + 1);
check(
  'the second tab to the same host is numbered',
  (await page.text('.tab[data-active="true"] .tab-ordinal')) === '2',
  await page.text('.tab[data-active="true"]'),
);
await page.click('.modal .check input');
await page.type('nyu');
await page.key('Enter');
await page.waitFor(terminalHas('toy shell'), { what: 'banner in the second tab', timeout: 15_000 });
check(
  'two sessions to the same server are live at once',
  (await page.eval(
    `[...document.querySelectorAll('.tab[data-status="live"]')].filter(t => t.textContent.includes('dev-sshd')).length`,
  )) === 2,
);

await page.eval(`window.__uwusshDriver.term.focus()`);
await page.type('help');
await page.key('Enter');
await page.waitFor(terminalHas('flood <MiB>'), { what: 'help output in the second tab' });
await shot('10b-second-tab');

// Back to the first connection: its terminal is its own.
const secondSize = await page.eval(
  `(window.__uwusshSecond = window.__uwusshDriver.term, [window.__uwusshSecond.cols, window.__uwusshSecond.rows])`,
);
await page.click('.tab .tab-select', 'dev-sshd');
await sleep(300);
check(
  'switching tabs shows the other terminal',
  !(await page.text('.tab[data-active="true"] .tab-ordinal')) &&
    !(await page.eval(terminalHas('flood <MiB>'))),
);
// Hidden, it has no size to fit to; it once shrank to ten columns there and
// the server wrapped all its output to that width.
const hiddenSize = await page.eval(`[window.__uwusshSecond.cols, window.__uwusshSecond.rows]`);
check(
  'a tab in the background keeps its size',
  secondSize[0] > 20 && hiddenSize[0] === secondSize[0] && hiddenSize[1] === secondSize[1],
  `${secondSize.join('x')} → ${hiddenSize.join('x')}`,
);

// Closing the second tab leaves the first one working.
await page.eval(
  `[...document.querySelectorAll('.tab')].find(t => t.querySelector('.tab-ordinal')?.textContent === '2').querySelector('.tab-close').click()`,
);
await sleep(300);
check('closing a tab removes it', (await tabCount()) === tabsBefore);
await page.eval(`window.__uwusshDriver.term.focus()`);
await page.type('help');
await page.key('Enter');
await page.waitFor(terminalHas('flood <MiB>'), { what: 'the first tab still answering' });
check('the other connection keeps working after a tab closed', true);

// ── Window controls ─────────────────────────────────────────────────────────
const controls = await page.eval(
  `[...document.querySelectorAll('.window-control')].map(b => b.getAttribute('aria-label')).join(',')`,
);
check(
  'the title bar has minimize, maximize and close',
  controls === 'Minimieren,Maximieren,Schließen',
  controls,
);
await page.click('.window-control[aria-label="Maximieren"]');
await page.waitFor(`document.querySelector('.window-control[aria-label="Verkleinern"]')`, {
  what: 'maximized window',
});
check('maximize works and offers to restore', true);
await page.click('.window-control[aria-label="Verkleinern"]');
await page.waitFor(`document.querySelector('.window-control[aria-label="Maximieren"]')`, {
  what: 'restored window',
});
await page.click('.window-control[aria-label="Schließen"]');
await page.waitFor(
  `[...document.querySelectorAll('.modal-title')].pop()?.textContent === 'UwUSSH schließen?'`,
  {
    what: 'close confirmation',
  },
);
check('closing with an open connection asks first', true);
await shot('10c-close');
await page.click('.modal-footer button', 'Abbrechen');
await page.waitFor(`!document.querySelector('.modal')`, { what: 'confirmation dismissed' });

// ── Settings ────────────────────────────────────────────────────────────────
await page.click('.titlebar-action[aria-label="Einstellungen"]');
await page.waitFor(
  `[...document.querySelectorAll('.modal-title')].pop()?.textContent === 'Einstellungen'`,
  {
    what: 'settings',
  },
);
await page.click('.settings-nav button', 'Terminal');
await page.click('.segmented button', 'Strich');
await sleep(200);
check(
  'a terminal setting reaches the open terminals right away',
  (await page.eval(`window.__uwusshDriver.term.options.cursorStyle`)) === 'bar',
);
await page.click('.segmented button', 'Block');
await page.click('.settings-nav button', 'Updates');
// A beta build starts on the beta channel, a release on the stable one.
const { version } = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8'));
const channel = version.includes('-') ? 'Beta' : 'Stabil';
check(
  `a ${version.includes('-') ? 'beta build' : 'release'} is on the ${channel} channel`,
  await page.eval(
    `[...document.querySelectorAll('.segmented button')].find(b => b.textContent === '${channel}')?.getAttribute('aria-checked') === 'true'`,
  ),
);
await shot('10d-settings');

// ── English: the whole app switches at once, and back ─────────────────────
const topTitle = `[...document.querySelectorAll('.modal-title')].pop()?.textContent`;
await page.click('.settings-nav button', 'Darstellung');
await page.click('.segmented button', 'English');
await page.waitFor(`${topTitle} === 'Settings'`, { what: 'settings in English' });
check(
  'switching to English translates the open dialog right away',
  await page.eval(
    `document.documentElement.lang === 'en' && [...document.querySelectorAll('.settings-nav button')].some(b => b.textContent.includes('Appearance'))`,
  ),
);
await shot('10e-english-settings');
await page.key('Escape');
await page.waitFor(`!document.querySelector('.modal')`, { what: 'English settings closed' });
check(
  'the window behind it speaks English too',
  await page.eval(
    `!!document.querySelector('.titlebar-action[aria-label="Settings"]') && !!document.querySelector('.sidebar-head [aria-label="Add host"]')`,
  ),
);
await shot('10f-english-main');
await page.click('.sidebar-head [aria-label="Add host"]');
await page.waitFor(`${topTitle} === 'New host'`, { what: 'host form in English' });
check('the host form is in English', true);
await shot('10g-english-form');
await page.key('Escape');
await page.waitFor(`!document.querySelector('.modal')`, { what: 'English host form closed' });
await page.click('.titlebar-action[aria-label="Settings"]');
await page.waitFor(`${topTitle} === 'Settings'`, { what: 'settings again' });
await page.click('.settings-nav button', 'Appearance');
await page.click('.segmented button', 'Deutsch');
await page.waitFor(`${topTitle} === 'Einstellungen'`, { what: 'settings back in German' });
check('and back to German', await page.eval(`document.documentElement.lang === 'de'`));
await page.key('Escape');
await page.waitFor(`!document.querySelector('.modal')`, { what: 'settings closed' });

// ── The system the server runs ─────────────────────────────────────────────
const invoke = (command, args = {}) =>
  page.eval(
    `window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)}, ${JSON.stringify(args)})`,
  );
const devHost = async () => (await invoke('list_hosts')).find((h) => h.name === 'dev-sshd');
await page.waitFor(
  `window.__TAURI_INTERNALS__.invoke('list_hosts').then(h => h.some(x => x.name === 'dev-sshd' && x.os === 'ubuntu'))`,
  { what: 'detected system' },
);
check('the server was recognised as Ubuntu', (await devHost()).os === 'ubuntu');
check(
  'the host list shows the system icon',
  await page.eval(
    `[...document.querySelectorAll('.host-row')].find(r => r.textContent.includes('dev-sshd'))?.querySelector('.host-icon svg')?.getAttribute('aria-hidden') === 'true'`,
  ),
);

// ── Keyword highlighting ────────────────────────────────────────────────────
await page.eval(`window.__uwusshDriver.term.focus()`);
await page.type('error 10.0.0.12 active');
await page.key('Enter');
await page.waitFor(terminalHas('command not found'), { what: 'unknown command output' });
await page.waitFor(`window.__uwusshDriver.highlighter.entries.size > 0`, {
  what: 'highlight decorations',
});
check('keywords and addresses in the output get colours', true);
await shot('12-highlight');

// ── Ctrl + mouse wheel ──────────────────────────────────────────────────────
const fontBefore = await page.eval(`window.__uwusshDriver.term.options.fontSize`);
const { x: tx, y: ty } = await page.locate('.terminal-pane:not([hidden]) .terminal-host');
for (let i = 0; i < 2; i += 1) {
  await page.send('Input.dispatchMouseEvent', {
    type: 'mouseWheel',
    x: tx,
    y: ty,
    deltaX: 0,
    deltaY: -100,
    modifiers: 2,
  });
  await sleep(80);
}
await sleep(200);
const fontAfter = await page.eval(`window.__uwusshDriver.term.options.fontSize`);
check(
  'Ctrl + wheel makes the terminal text bigger',
  fontAfter === fontBefore + 2,
  `${fontBefore} → ${fontAfter}`,
);
// Two notches back: each notch is one step.
for (let i = 0; i < 2; i += 1) {
  await page.send('Input.dispatchMouseEvent', {
    type: 'mouseWheel',
    x: tx,
    y: ty,
    deltaX: 0,
    deltaY: 100,
    modifiers: 2,
  });
  await sleep(80);
}
await sleep(300);
check('the terminal still fits after zooming in and out', await terminalFits());
check(
  'Ctrl + wheel back brings the old size back',
  (await page.eval(`window.__uwusshDriver.term.options.fontSize`)) === fontBefore,
);

// ── sudo asks: the helper types the password the login used ────────────────
await page.eval(`window.__uwusshDriver.term.focus()`);
await page.type('sudo whoami');
await page.key('Enter');
await page.waitFor(`document.querySelector('.password-helper')`, {
  what: 'password helper',
  timeout: 5_000,
});
check('a sudo prompt brings up the password helper', true);
await shot('13-sudo');
await page.click('.password-helper button', 'Eintippen');
await page.waitFor(terminalHas('root access granted'), { what: 'sudo accepted' });
check('the helper typed the right password, and only after asking', count('sudo: accepted') === 1);
check(
  'the helper is gone after typing',
  await page.eval(`!document.querySelector('.password-helper')`),
);

// sudo-rs asks without naming a user; the helper knows it too, and the
// toolbar button types the password and Enter at such a question.
await page.eval(`window.__uwusshDriver.term.focus()`);
await page.type('sudo -i');
await page.key('Enter');
await page.waitFor(`document.querySelector('.password-helper')`, {
  what: 'password helper for sudo-rs',
  timeout: 5_000,
});
check("sudo-rs' prompt without a user name brings up the helper", true);
await page.click('.toolbar button', 'Passwort eintippen');
await sleep(800);
check(
  'the toolbar button typed the password and Enter at the prompt',
  count('sudo: accepted') === 2,
);

// At a shell prompt the button still types the password — but no Enter, so
// nothing runs.
await page.eval(`window.__uwusshDriver.term.focus()`);
check(
  'the toolbar button is always there to click',
  await page.eval(
    `[...document.querySelectorAll('.toolbar button')].some(b => b.textContent.includes('Passwort eintippen') && !b.disabled)`,
  ),
);
await page.click('.toolbar button', 'Passwort eintippen');
await sleep(600);
check(
  'at a shell prompt it types without Enter',
  await page.eval(`(() => {
    const t = window.__uwusshDriver.term.buffer.active;
    return t.getLine(t.baseY + t.cursorY)?.translateToString(true).endsWith('$ nyu');
  })()`),
);
for (let i = 0; i < 3; i += 1) await page.key('Backspace');

// ── Save the password: the vault is created on the way ──────────────────────
await page.eval(
  `[...document.querySelectorAll('.host-row')].find(r => r.textContent.includes('dev-sshd')).querySelector('.host-actions button[aria-label$="bearbeiten"]').click()`,
);
await page.waitFor(
  `[...document.querySelectorAll('.modal-title')].pop()?.textContent === 'dev-sshd bearbeiten'`,
  {
    what: 'edit form',
  },
);
await page.click('.modal input[type=password]');
await page.type('nyu');
await page.click('.modal-footer button', 'Speichern');
await page.waitFor(
  `[...document.querySelectorAll('.modal-title')].pop()?.textContent === 'Tresor anlegen'`,
  {
    what: 'vault creation for the password',
  },
);
check('saving a password asks for a vault first', true);
await page.waitFor(`document.activeElement?.type === 'password'`, {
  what: 'the master password field has the cursor',
});
check('the master password field has the cursor', true);
await page.type('e2e-master');
await page.eval(
  `[...document.querySelectorAll('.modal')].pop().querySelectorAll('input[type=password]')[1].focus()`,
);
await page.type('e2e-master');
await shot('14-vault');
await page.key('Enter');
await page.waitFor(`!document.querySelector('.modal')`, {
  what: 'form saved after the vault',
  timeout: 20_000,
});
const saved = await devHost();
check('the password is stored', saved.hasPassword === true);
const vault = await invoke('vault_state');
check(
  'the vault is open and remembered on this device',
  vault.status === 'unlocked' && vault.remembered === true,
  JSON.stringify(vault),
);
check(
  'the password is not in the page',
  !JSON.stringify(await invoke('list_hosts')).includes('"nyu"'),
);

// ── A stored password: connecting asks nothing ──────────────────────────────
await page.openHost('dev-sshd');
await page.waitFor(terminalHas('toy shell'), {
  what: 'connected without a prompt',
  timeout: 15_000,
});
check(
  'a host with a stored password connects without asking',
  await page.eval(`!document.querySelector('.modal')`),
);

// ── Workspaces and groups: drag a host into Business ───────────────────────
await invoke('save_host', {
  draft: {
    id: null,
    name: 'nas',
    address: '10.99.0.5',
    port: 22,
    username: 'root',
    auth: 'password',
    keyPath: null,
    groupPath: 'lokal',
    workspace: 'private',
    keyId: null,
    password: { kind: 'keep' },
  },
});
await page.send('Page.reload');
await page.waitFor(
  `[...document.querySelectorAll('.host-name')].some(e => e.textContent === 'nas')`,
  {
    what: 'second host',
  },
);
await sleep(600);
async function drag(fromSelector, fromText, toSelector, toText, dy = 0) {
  const from = await page.locate(fromSelector, fromText);
  const to = await page.locate(toSelector, toText);
  await page.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: from.x, y: from.y });
  await page.send('Input.dispatchMouseEvent', {
    type: 'mousePressed',
    x: from.x,
    y: from.y,
    button: 'left',
    buttons: 1,
    clickCount: 1,
  });
  for (let i = 1; i <= 12; i += 1) {
    await page.send('Input.dispatchMouseEvent', {
      type: 'mouseMoved',
      x: from.x + ((to.x - from.x) * i) / 12,
      y: from.y + ((to.y + dy - from.y) * i) / 12,
      button: 'left',
      buttons: 1,
    });
    await sleep(25);
  }
  await page.send('Input.dispatchMouseEvent', {
    type: 'mouseReleased',
    x: to.x,
    y: to.y + dy,
    button: 'left',
    buttons: 0,
    clickCount: 1,
  });
  await sleep(500);
}
// Reorder: nas above dev-sshd.
await drag('.host', 'nas', '.host', 'dev-sshd', -8);
const order = await page.eval(
  `[...document.querySelectorAll('.host-group .host-name')].map(e => e.textContent).join(',')`,
);
check('dragging a host between hosts reorders the group', order.startsWith('nas,dev-sshd'), order);
check(
  'a drag does not open a connection',
  (await tabCount()) === 0 && (await page.eval(`!document.querySelector('.modal')`)),
);
// Into the other workspace.
await drag('.host', 'nas', '.workspace-switch button', 'Business');
const nas = (await invoke('list_hosts')).find((h) => h.name === 'nas');
check('dropping a host on "Business" moves it there', nas.workspace === 'business', nas.workspace);
await page.click('.workspace-switch button', 'Business');
await page.waitFor(
  `[...document.querySelectorAll('.host-name')].some(e => e.textContent === 'nas')`,
  {
    what: 'business list',
  },
);
check(
  'the Business workspace lists it, with its group',
  (await page.text('.host-group h3')).includes('lokal'),
);
await shot('15-business');
await page.click('.workspace-switch button', 'Privat');
await sleep(300);

// ── Files: the server's side, a download and an upload by dragging ─────────
await page.openHost('dev-sshd');
await page.waitFor(terminalHas('toy shell'), {
  what: 'terminal for the files test',
  timeout: 15_000,
});
await page.click('.toolbar button', 'Dateien');
await page.waitFor(
  `[...document.querySelectorAll('[data-file-pane=remote] .file-row')].some(r => r.textContent.includes('welcome.txt'))`,
  { what: 'remote listing', timeout: 20_000 },
);
check('the file tab lists the home folder on the server', true);
check(
  'the file tab starts in the home folder',
  (await page.text('[data-file-pane=remote] .file-path')).includes('/home/uwu'),
);
await page.click('[data-file-pane=local] .file-path');
await page.key('a', 2);
await page.type(WORK_DIR.replaceAll('\\', '/'));
await page.key('Enter');
await sleep(800);
await page.click('[data-file-pane=remote] .file-row', 'welcome.txt');
await page.click('[data-file-pane=remote] .file-actions button', 'Herunterladen');
await page.waitFor(
  `[...document.querySelectorAll('.transfers li')].some(l => l.dataset.state === 'done')`,
  {
    what: 'download done',
  },
);
check('a download arrives on this computer', existsSync(`${WORK_DIR}/welcome.txt`));
check(
  'the download is marked as coming from elsewhere',
  readFileSync(`${WORK_DIR}/welcome.txt:Zone.Identifier`, 'utf8').includes('ZoneId=3'),
);
await shot('16-files');

// A new file goes up by dragging it over.
writeFileSync(`${WORK_DIR}/nyu-upload.txt`, 'hochgeladen');
await page.click('[data-file-pane=local] button[aria-label="Neu laden"]');
await page.waitFor(
  `[...document.querySelectorAll('[data-file-pane=local] .file-row')].some(r => r.textContent.includes('nyu-upload.txt'))`,
  { what: 'local refresh' },
);
await drag(
  '[data-file-pane=local] .file-row',
  'nyu-upload.txt',
  '[data-file-pane=remote] .file-list',
  '',
  60,
);
await page.waitFor(
  `[...document.querySelectorAll('[data-file-pane=remote] .file-row')].some(r => r.textContent.includes('nyu-upload.txt'))`,
  { what: 'uploaded file listed', timeout: 15_000 },
);
check(
  'dragging a local file onto the server uploads it',
  readFileSync(`${SERVER_FILES}/home/uwu/nyu-upload.txt`, 'utf8') === 'hochgeladen',
);

// A name that is taken asks first, and replaces only on yes.
const serverWelcome = `${SERVER_FILES}/home/uwu/welcome.txt`;
const original = readFileSync(serverWelcome, 'utf8');
writeFileSync(`${WORK_DIR}/welcome.txt`, 'neu von Nyu');
await drag(
  '[data-file-pane=local] .file-row',
  'welcome.txt',
  '[data-file-pane=remote] .file-list',
  '',
  60,
);
await page.waitFor(
  `[...document.querySelectorAll('.modal-title')].pop()?.textContent === 'Ersetzen?'`,
  { what: 'replace question', timeout: 15_000 },
);
check('an upload onto a taken name asks first', readFileSync(serverWelcome, 'utf8') === original);
await shot('16b-replace');
await page.click('.modal-footer button', 'Ersetzen');
await page.waitFor(`!document.querySelector('.modal')`, { what: 'replace question closed' });
await page.waitFor(
  `[...document.querySelectorAll('.transfers li')].every(l => l.dataset.state !== 'running')`,
  { what: 'replacing upload done', timeout: 15_000 },
);
await sleep(600);
check(
  'replacing overwrites the file on the server',
  readFileSync(serverWelcome, 'utf8') === 'neu von Nyu',
);

// ── Export: everything, with secrets, into a file ───────────────────────────
await page.click('.titlebar-action[aria-label="Einstellungen"]');
await page.waitFor(
  `[...document.querySelectorAll('.modal-title')].pop()?.textContent === 'Einstellungen'`,
  {
    what: 'settings for export',
  },
);
await page.click('.settings-nav button', 'Import & Export');
await page.click('.settings-content button', 'Exportieren');
await page.waitFor(
  `[...document.querySelectorAll('.modal-title')].pop()?.textContent === 'Exportieren'`,
  {
    what: 'export dialog',
  },
);
await page.click('.modal input[type=password]');
await page.type('export-pw-123');
await page.eval(
  `[...document.querySelectorAll('.modal')].pop().querySelectorAll('input[type=password]')[1].focus()`,
);
await page.type('export-pw-123');
await page.click('.modal-footer button', 'Speichern unter');
await page.waitFor(
  `[...document.querySelectorAll('.modal-title')].pop()?.textContent.startsWith('Export gespeichert')`,
  {
    what: 'export saved',
    timeout: 20_000,
  },
);
const exported = readdirSync(WORK_DIR).filter((name) => name.endsWith('.uwussh'));
check('the export file exists', exported.length === 1, exported.join(','));
const exportText = readFileSync(`${WORK_DIR}/${exported[0]}`, 'utf8');
check(
  'the export file is sealed: no host name, no password in it',
  !exportText.includes('dev-sshd') && !exportText.includes('"nyu"'),
);
await shot('17-export');
await page.click('.modal-footer button', 'Fertig');
await page.key('Escape');
await page.waitFor(`!document.querySelector('.modal')`, { what: 'settings closed after export' });

// ── Import: preview first, and the vault is asked before anything is written ─
await page.click('.sidebar-head [aria-label="Importieren"]');
await page.waitFor(
  `[...document.querySelectorAll('.modal-title')].pop()?.textContent === 'Importieren'`,
  {
    what: 'import dialog',
  },
);
// The dialog reaches a preview (a source is present on this machine) or says
// there is nothing to import. Either way it has not written anything.
await page.waitFor(`!!document.querySelector('.import-sources')`, { what: 'source picker' });
check(
  'the import dialog always offers an UwUSSH export file',
  (await page.text('.import-source')).includes('UwUSSH-Export'),
);
await shot('18-import');
await page.key('Escape');
await page.waitFor(`!document.querySelector('.modal')`, { what: 'import dialog closed' });
check('the import dialog closes without touching the host list', true);

page.close();
console.log(failed() === 0 ? 'PHASE A OK' : `PHASE A: ${failed()} FAILED`);
process.exit(failed() === 0 ? 0 : 1);
