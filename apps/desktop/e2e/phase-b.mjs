// Phase B: the fixes from phase A's screenshots, a changed host key (accepted
// or rejected with a button), and deleting a host.
import { check, connect, failed, sleep } from './cdp.mjs';

const SHOTS = new URL('./shots/', import.meta.url).pathname.replace(/^\/([A-Z]:)/, '$1');
const OLD_FINGERPRINT = process.argv[2];
const NEW_FINGERPRINT = process.argv[3];

const page = await connect();
const shot = (name) => page.screenshot(`${SHOTS}${name}.png`);
const terminalHas = (text) =>
  `(() => { const t = window.__uwusshDriver.term.buffer.active; for (let i = 0; i < t.length; i++) if (t.getLine(i)?.translateToString().includes(${JSON.stringify(text)})) return true; })()`;

// A clean page, so no state from phase A leaks in.
await page.send('Page.reload');
await sleep(1500);
await page.waitFor(
  `window.__uwusshDriver && [...document.querySelectorAll('.host-name')].some(e => e.textContent === 'dev-sshd')`,
  { what: 'host list after reload' },
);
check('hosts survive a reload (they live in SQLite, not the page)', true);

// ── Fix: errors clear on edit, inputs stay aligned ─────────────────────────
await page.click('.sidebar-head [aria-label="Host hinzufügen"]');
await page.waitFor(
  `[...document.querySelectorAll('.modal-title')].pop()?.textContent === 'Neuer Host'`,
  {
    what: 'host form',
  },
);
await page.click('.modal-footer button', 'Speichern');
await page.waitFor(`document.querySelector('.field-error')`, { what: 'validation error' });
const tops = await page.eval(
  `[...document.querySelectorAll('.modal .form-row')[0].querySelectorAll('input')].map((i) => Math.round(i.getBoundingClientRect().top))`,
);
check(
  'port input stays level with address while an error shows',
  tops[0] === tops[1],
  JSON.stringify(tops),
);
await page.eval(`document.querySelectorAll('.modal input')[0].focus()`);
await page.type('1');
await sleep(150);
check(
  'typing into the field clears its error',
  await page.eval(`!document.querySelector('.field-error')`),
);
// A click beside the form keeps it open.
for (const type of ['mousePressed', 'mouseReleased']) {
  await page.send('Input.dispatchMouseEvent', {
    type,
    x: 8,
    y: 700,
    button: 'left',
    clickCount: 1,
  });
}
await sleep(300);
check(
  'a click beside a dialog does not close it',
  await page.eval(
    `[...document.querySelectorAll('.modal-title')].pop()?.textContent === 'Neuer Host'`,
  ),
);
// Closing a form with something typed into it asks first.
await page.key('Escape');
await page.waitFor(
  `[...document.querySelectorAll('.modal-title')].pop()?.textContent === 'Wirklich schließen?'`,
  { what: 'close question' },
);
check('closing a changed form asks first', true);
await page.click('.modal-footer button', 'Weiter bearbeiten');
await sleep(200);
check(
  '"keep editing" keeps what was typed',
  await page.eval(
    `[...document.querySelectorAll('.modal-title')].pop()?.textContent === 'Neuer Host'`,
  ),
);
await page.key('Escape');
await page.waitFor(
  `[...document.querySelectorAll('.modal-title')].pop()?.textContent === 'Wirklich schließen?'`,
  { what: 'close question again' },
);
await page.click('.modal-footer button', 'Schließen');
await page.waitFor(`!document.querySelector('.modal')`, { what: 'form closed' });

// ── The host key changed ────────────────────────────────────────────────────
await page.click('.host .host-name', 'dev-sshd');
await page.waitFor(
  `[...document.querySelectorAll('.modal-title')].pop()?.textContent === 'Der Host-Key hat sich geändert'`,
  { what: 'changed-key warning', timeout: 15_000 },
);
await sleep(300);
check(
  'warning is styled as a warning',
  await page.eval(`document.querySelector('.modal')?.dataset.tone === 'warning'`),
);
const fps = (await page.text('.key-compare .fingerprint')).split(' | ');
check('shows the fingerprint trusted before', fps[0] === OLD_FINGERPRINT, fps[0]);
check('shows the fingerprint presented now', fps[1] === NEW_FINGERPRINT, fps[1]);
check(
  'the warning offers two plain buttons, no typing',
  await page.eval(
    `!document.querySelector('.modal input') && [...document.querySelectorAll('.modal-footer button')].map(b => b.textContent).join('|') === 'Neuen Schlüssel akzeptieren|Ablehnen'`,
  ),
  await page.text('.modal-footer button'),
);
check(
  'focus sits on the safe choice',
  (await page.eval(`document.activeElement?.textContent`)) === 'Ablehnen',
  await page.eval(`document.activeElement?.textContent`),
);
check(
  'toolbar says which host is being connected',
  (await page.text('.session-title')).includes('dev-sshd') &&
    (await page.text('.session-title')).includes('verbindet'),
  await page.text('.session-title'),
);
await shot('11-changed');

// Enter takes the safe way out.
await page.key('Enter');
await page.waitFor(`document.querySelector('.notice')?.textContent.includes('Host-Key')`, {
  what: 'blocked notice',
});
check(
  'Enter refuses the connection and says why',
  (await page.text('.notice')).includes('hat sich geändert'),
  await page.text('.notice'),
);

// Now deliberately accept the new key.
await page.click('.host .host-name', 'dev-sshd');
await page.waitFor(
  `[...document.querySelectorAll('.modal-title')].pop()?.textContent === 'Der Host-Key hat sich geändert'`,
  { what: 'changed-key warning again', timeout: 15_000 },
);
await page.click('.modal-footer button', 'Neuen Schlüssel akzeptieren');

// The password was stored in phase A: after the key, nothing else is asked.
await page.waitFor(terminalHas('toy shell'), { what: 'connected with new key', timeout: 15_000 });
check('connected to the server with its new key', true);
check(
  'the stored password logged in without a prompt',
  await page.eval(`!document.querySelector('.modal')`),
);

// ── Fix: the dot goes out when the session ends ─────────────────────────────
const dot = `[...document.querySelectorAll('.host-row')].find(r => r.textContent.includes('dev-sshd'))?.querySelector('.host-icon .dot')?.dataset.state`;
check('connected host shows as online', (await page.eval(dot)) === 'online', await page.eval(dot));
await page.eval(`window.__uwusshDriver.term.focus()`);
await page.type('exit');
await page.key('Enter');
await page.waitFor(`document.querySelector('.notice')?.textContent.includes('wurde beendet')`, {
  what: 'ended banner',
});
check(
  'after the session ends the host no longer shows as online',
  (await page.eval(dot)) === 'idle',
  await page.eval(dot),
);
await shot('12-ended');

// ── Delete the host ─────────────────────────────────────────────────────────
await page.eval(
  `[...document.querySelectorAll('.host-row')].find(r => r.textContent.includes('dev-sshd')).querySelector('.host-actions button[aria-label$="bearbeiten"]').click()`,
);
await page.waitFor(
  `[...document.querySelectorAll('.modal-title')].pop()?.textContent === 'dev-sshd bearbeiten'`,
  { what: 'edit form' },
);
await page.click('.modal-footer button', 'Löschen');
check(
  'deleting needs a second click',
  await page.eval(
    `[...document.querySelectorAll('.modal-footer button')].some(b => b.textContent === 'Wirklich löschen')`,
  ),
);
await page.click('.modal-footer button', 'Wirklich löschen');
await page.waitFor(
  `!document.querySelector('.modal') && document.querySelector('.sidebar-empty')`,
  { what: 'empty list after delete' },
);
check('host is gone and the empty state is back', true);
await shot('13-deleted');

page.close();
console.log(failed() === 0 ? 'PHASE B OK' : `PHASE B: ${failed()} FAILED`);
process.exit(failed() === 0 ? 0 : 1);
