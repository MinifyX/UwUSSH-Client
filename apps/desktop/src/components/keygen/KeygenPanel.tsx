import { useCallback, useEffect, useRef, useState } from 'react';
import {
  passphraseNote,
  asKeyFailure,
  FORMAT_LABELS,
  keygenDiscard,
  keygenCopyPrivate,
  keygenEncode,
  keygenGenerate,
  keygenSave,
  keygenSavePublic,
  type Generated,
  type KeyKind,
  type KeyRecord,
  type PrivateFormat,
} from '../../lib/keys';
import { Icon } from '../Icon';
import { NyuScene } from '../nyu/scenes';
import { NyuLaserPad } from './NyuLaserPad';
import './keygen.css';

/**
 * UwUKeygen: settings, a round of laser chasing with Nyu for extra
 * randomness, and the finished key — to look at, copy, save as a file in any
 * format, or (inside UwUSSH) put straight into the vault.
 *
 * The same panel runs in the app's dialog and in the standalone UwUKeygen
 * window; the standalone one has no vault, so it passes no `onStore`.
 */

type Props = {
  /** Suggested comment, e.g. `lorin@prox-1`. */
  comment?: string;
  /** Inside UwUSSH: seal the key into the vault. Returns the stored key. */
  onStore?: (token: string, label: string, passphrase: string | null) => Promise<KeyRecord>;
  /** Label of the store button, e.g. "Für diesen Host verwenden". */
  storeLabel?: string;
  /** Called with the stored key after `onStore` succeeded. */
  onStored?: (key: KeyRecord) => void;
  /** Steps change: the host dialog adjusts its title. */
  onStep?: (step: Step) => void;
};

export type Step = 'settings' | 'entropy' | 'generating' | 'done';

type KindChoice = 'rsa' | 'ed25519' | 'ecdsa';

/**
 * Moments of play to collect. A moment is a sample at least `MOMENT_MS`
 * after the last one and `MOMENT_MOVE` away from it: a fast mouse sends a
 * thousand samples a second, and counting those filled the bar before Nyu had
 * a chance to pounce. This is a good quarter of a minute of chasing the dot.
 */
const ENTROPY_TARGET = 360;
const MOMENT_MS = 40;
/** In 1/16 CSS px, like the samples: 6 px. */
const MOMENT_MOVE = 96;
/** The most sent to Rust; older samples fold into it. */
const ENTROPY_BUFFER = 4096;

function kindOf(choice: KindChoice, bits: number, curve: number): KeyKind {
  if (choice === 'rsa') return { type: 'rsa', bits };
  if (choice === 'ed25519') return { type: 'ed25519' };
  return { type: curve === 384 ? 'ecdsa-p384' : curve === 521 ? 'ecdsa-p521' : 'ecdsa-p256' };
}

function describeKind(choice: KindChoice, bits: number, curve: number) {
  if (choice === 'rsa') return `RSA ${bits}`;
  if (choice === 'ed25519') return 'Ed25519';
  return `ECDSA P-${curve}`;
}

async function copy(text: string) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const area = document.createElement('textarea');
    area.value = text;
    document.body.append(area);
    area.select();
    document.execCommand('copy');
    area.remove();
  }
}

export function KeygenPanel({ comment = '', onStore, storeLabel, onStored, onStep }: Props) {
  const [step, setStepState] = useState<Step>('settings');
  const [advanced, setAdvanced] = useState(false);
  const [choice, setChoice] = useState<KindChoice>('rsa');
  const [bits, setBits] = useState(2048);
  const [curve, setCurve] = useState(256);
  const [keyComment, setKeyComment] = useState(comment);
  const [passphrase, setPassphrase] = useState('');
  const [repeat, setRepeat] = useState('');
  const [collected, setCollected] = useState(0);
  const [generated, setGenerated] = useState<Generated | null>(null);
  const [format, setFormat] = useState<PrivateFormat>('openssh');
  const [privateText, setPrivateText] = useState<string | null>(null);
  const [label, setLabel] = useState('');
  const [message, setMessage] = useState<{ tone: 'info' | 'error'; text: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState<string | null>(null);

  const entropy = useRef(new Uint8Array(ENTROPY_BUFFER));
  const moments = useRef(0);
  const lastMoment = useRef({ x: -1e6, y: -1e6, time: -1e6 });
  const written = useRef(0);
  const flush = useRef(0);
  const tokenRef = useRef<string | null>(null);

  const setStep = (next: Step) => {
    setStepState(next);
    onStep?.(next);
  };

  // A key nobody stored or saved is thrown away with the panel.
  useEffect(
    () => () => {
      if (tokenRef.current) void keygenDiscard(tokenRef.current).catch(() => undefined);
      window.clearTimeout(flush.current);
    },
    [],
  );

  const onEntropy = useCallback((sample: Uint8Array) => {
    const buffer = entropy.current;
    for (const byte of sample) {
      const at = written.current % ENTROPY_BUFFER;
      // Past the buffer's end, new samples fold into the old ones.
      buffer[at] = written.current < ENTROPY_BUFFER ? byte : buffer[at]! ^ byte;
      written.current += 1;
    }
    // Every sample goes into the buffer; only real moments of play fill the bar.
    const view = new DataView(sample.buffer, sample.byteOffset, sample.byteLength);
    const x = view.getUint16(0);
    const y = view.getUint16(2);
    const time = view.getUint32(8) / 1000;
    const last = lastMoment.current;
    const moved = Math.hypot(x - last.x, y - last.y);
    if (time - last.time >= MOMENT_MS || time < last.time) {
      if (moved >= MOMENT_MOVE) {
        lastMoment.current = { x, y, time };
        moments.current += 1;
      }
    }
    if (flush.current) return;
    flush.current = window.setTimeout(() => {
      flush.current = 0;
      setCollected(moments.current);
    }, 120);
  }, []);

  const mismatch = passphrase.length > 0 && repeat.length > 0 && passphrase !== repeat;
  const settingsReady = passphrase === repeat;
  const progress = Math.min(1, collected / ENTROPY_TARGET);

  const generate = async () => {
    setStep('generating');
    setMessage(null);
    try {
      const size = Math.min(written.current, ENTROPY_BUFFER);
      const result = await keygenGenerate(
        kindOf(choice, bits, curve),
        keyComment.trim(),
        entropy.current.slice(0, size),
      );
      entropy.current.fill(0);
      written.current = 0;
      moments.current = 0;
      lastMoment.current = { x: -1e6, y: -1e6, time: -1e6 };
      if (tokenRef.current) void keygenDiscard(tokenRef.current).catch(() => undefined);
      tokenRef.current = result.token;
      setGenerated(result);
      setLabel(keyComment.trim() || result.info.label);
      setStep('done');
    } catch (error) {
      const failure = asKeyFailure(error);
      setMessage({
        tone: 'error',
        text:
          failure.kind === 'error' ? failure.message : 'Der Schlüssel ließ sich nicht erzeugen.',
      });
      setStep('entropy');
    }
  };

  const flash = (what: string) => {
    setCopied(what);
    window.setTimeout(() => setCopied((current) => (current === what ? null : current)), 1600);
  };

  const run = async (action: () => Promise<void>) => {
    setBusy(true);
    setMessage(null);
    try {
      await action();
    } catch (error) {
      const failure = asKeyFailure(error);
      setMessage({
        tone: 'error',
        text:
          failure.kind === 'vault-locked'
            ? 'Der Tresor ist gesperrt.'
            : failure.kind === 'error'
              ? failure.message
              : `Fehler (${failure.kind})`,
      });
    } finally {
      setBusy(false);
    }
  };

  const pass = passphrase || null;

  if (step === 'settings') {
    return (
      <div className="keygen" data-step="settings">
        <div className="keygen-intro">
          <NyuScene name="keys" className="keygen-scene" />
          <div>
            <p className="dialog-lead">
              Ein neuer SSH-Schlüssel, erzeugt auf diesem Rechner. Standard ist{' '}
              <b>RSA mit 2048 Bit</b> – den nimmt jeder Server.
            </p>
            <p className="field-hint">Alles andere findest du unter „Erweitert“.</p>
          </div>
        </div>

        <div className="segmented keygen-tabs" role="tablist" aria-label="Ansicht">
          <button
            role="tab"
            aria-selected={!advanced}
            aria-checked={!advanced}
            onClick={() => setAdvanced(false)}
          >
            Einfach
          </button>
          <button
            role="tab"
            aria-selected={advanced}
            aria-checked={advanced}
            onClick={() => setAdvanced(true)}
          >
            Erweitert
          </button>
        </div>

        {advanced && (
          <div className="keygen-advanced">
            <fieldset className="field">
              <span>Schlüsseltyp</span>
              <div className="segmented" role="radiogroup" aria-label="Schlüsseltyp">
                {(
                  [
                    ['rsa', 'RSA'],
                    ['ed25519', 'Ed25519'],
                    ['ecdsa', 'ECDSA'],
                  ] as const
                ).map(([value, text]) => (
                  <button
                    key={value}
                    type="button"
                    role="radio"
                    aria-checked={choice === value}
                    onClick={() => setChoice(value)}
                  >
                    {text}
                  </button>
                ))}
              </div>
            </fieldset>
            {choice === 'rsa' && (
              <fieldset className="field">
                <span>Schlüssellänge</span>
                <div className="segmented" role="radiogroup" aria-label="Schlüssellänge">
                  {[1024, 2048, 3072, 4096].map((value) => (
                    <button
                      key={value}
                      type="button"
                      role="radio"
                      aria-checked={bits === value}
                      onClick={() => setBits(value)}
                    >
                      {value}
                    </button>
                  ))}
                </div>
                {bits === 1024 && (
                  <em className="field-error">
                    1024 Bit gelten als zu schwach – nur für sehr alte Geräte.
                  </em>
                )}
              </fieldset>
            )}
            {choice === 'ecdsa' && (
              <fieldset className="field">
                <span>Kurve</span>
                <div className="segmented" role="radiogroup" aria-label="Kurve">
                  {[256, 384, 521].map((value) => (
                    <button
                      key={value}
                      type="button"
                      role="radio"
                      aria-checked={curve === value}
                      onClick={() => setCurve(value)}
                    >
                      P-{value}
                    </button>
                  ))}
                </div>
              </fieldset>
            )}
            {choice === 'ed25519' && (
              <p className="field-hint">
                Kurz, schnell und modern – von OpenSSH ab 6.5 und PuTTY ab 0.68 unterstützt.
              </p>
            )}
            <p className="field-hint">
              Speicherformate (OpenSSH, PuTTY .ppk v3/v2, PEM) wählst du nach dem Erzeugen.
            </p>
          </div>
        )}

        <label className="field">
          <span>Kommentar</span>
          <input
            value={keyComment}
            placeholder="z. B. lorin@laptop"
            spellCheck={false}
            onChange={(e) => setKeyComment(e.target.value)}
          />
        </label>
        <div className="form-row">
          <label className="field grow">
            <span>Passphrase (optional)</span>
            <input
              type="password"
              value={passphrase}
              autoComplete="new-password"
              onChange={(e) => setPassphrase(e.target.value)}
            />
          </label>
          <label className="field grow">
            <span>Wiederholen</span>
            <input
              type="password"
              value={repeat}
              autoComplete="new-password"
              aria-invalid={mismatch}
              onChange={(e) => setRepeat(e.target.value)}
            />
          </label>
        </div>
        {mismatch && <p className="field-error">Die Passphrasen stimmen nicht überein.</p>}

        <div className="keygen-actions">
          <span className="keygen-summary">{describeKind(choice, bits, curve)}</span>
          <span className="spacer" />
          <button className="primary" disabled={!settingsReady} onClick={() => setStep('entropy')}>
            Weiter
          </button>
        </div>
      </div>
    );
  }

  if (step === 'entropy' || step === 'generating') {
    return (
      <div className="keygen" data-step="entropy">
        <p className="dialog-lead">
          Für einen guten Schlüssel braucht Nyu ein bisschen Zufall. Lass sie den Laserpunkt jagen!
        </p>
        <NyuLaserPad
          progress={progress}
          active={step === 'entropy' && progress < 1}
          onEntropy={onEntropy}
          hint={
            step === 'generating'
              ? 'Nyu schmiedet deinen Schlüssel …'
              : progress >= 1
                ? 'Genug Zufall gesammelt ✧'
                : 'Bewege die Maus über das Feld'
          }
        />
        {message && (
          <p className="field-error" role="alert">
            {message.text}
          </p>
        )}
        <div className="keygen-actions">
          <button
            className="quiet"
            onClick={() => setStep('settings')}
            disabled={step === 'generating'}
          >
            Zurück
          </button>
          <span className="spacer" />
          {progress < 1 && (
            <button
              className="link-button keygen-skip"
              onClick={() => void generate()}
              disabled={step === 'generating'}
              title="Der Zufall des Betriebssystems reicht für einen sicheren Schlüssel; die Mausbewegung kommt nur obendrauf."
            >
              Ohne Maus erzeugen
            </button>
          )}
          <button
            className="primary"
            disabled={progress < 1 || step === 'generating'}
            onClick={() => void generate()}
          >
            {step === 'generating' ? 'Erzeuge …' : 'Schlüssel erzeugen'}
          </button>
        </div>
      </div>
    );
  }

  const info = generated?.info;
  if (!generated || !info) return null;

  return (
    <div className="keygen" data-step="done">
      <div className="keygen-result-head">
        <NyuScene name="done" className="keygen-scene" />
        <div>
          <p className="keygen-title">
            {info.label} <span>ist fertig ✧</span>
          </p>
          <p className="field-hint">
            {info.comment ? <code>{info.comment}</code> : 'ohne Kommentar'}
            {passphrase ? ' · mit Passphrase' : ' · ohne Passphrase'}
          </p>
        </div>
      </div>

      <div className="key-facts">
        <pre className="randomart" aria-label="Randomart des Schlüssels">
          {info.randomart}
        </pre>
        <dl>
          <dt>Fingerprint (SHA-256)</dt>
          <dd>
            <code className="fingerprint">{info.fingerprintSha256}</code>
          </dd>
          <dt>Fingerprint (MD5)</dt>
          <dd>
            <code className="fingerprint">{info.fingerprintMd5}</code>
          </dd>
        </dl>
      </div>

      <label className="field">
        <span>Public Key – gehört auf den Server in ~/.ssh/authorized_keys</span>
        <textarea className="key-text" readOnly rows={3} value={info.publicOpenssh} />
      </label>
      <div className="keygen-row">
        <button onClick={() => void copy(info.publicOpenssh).then(() => flash('public'))}>
          <Icon name={copied === 'public' ? 'check' : 'copy'} size={15} />
          {copied === 'public' ? 'Kopiert' : 'Kopieren'}
        </button>
        <button
          disabled={busy}
          onClick={() =>
            void run(async () => {
              const saved = await keygenSavePublic(generated.token, label || info.label);
              if (saved) setMessage({ tone: 'info', text: `Gespeichert als ${saved}` });
            })
          }
        >
          <Icon name="download" size={15} />
          Als .pub speichern…
        </button>
      </div>

      <fieldset className="field keygen-private">
        <span>Private Key</span>
        <div className="keygen-row">
          <select
            className="select"
            value={format}
            aria-label="Format"
            onChange={(e) => {
              setFormat(e.target.value as PrivateFormat);
              setPrivateText(null);
            }}
          >
            {(Object.keys(FORMAT_LABELS) as PrivateFormat[]).map((value) => (
              <option key={value} value={value}>
                {FORMAT_LABELS[value]}
              </option>
            ))}
          </select>
          <button
            disabled={busy}
            onClick={() =>
              privateText
                ? setPrivateText(null)
                : void run(async () =>
                    setPrivateText(await keygenEncode(generated.token, format, pass)),
                  )
            }
          >
            <Icon name="eye" size={15} />
            {privateText ? 'Verbergen' : 'Anzeigen'}
          </button>
          <button
            disabled={busy}
            onClick={() =>
              void run(async () => {
                await keygenCopyPrivate(generated.token, format, pass);
                flash('private');
              })
            }
          >
            <Icon name={copied === 'private' ? 'check' : 'copy'} size={15} />
            {copied === 'private' ? 'Kopiert' : 'Kopieren'}
          </button>
          <button
            disabled={busy}
            onClick={() =>
              void run(async () => {
                const saved = await keygenSave(generated.token, format, pass, label || info.label);
                if (saved) setMessage({ tone: 'info', text: `Gespeichert als ${saved}` });
              })
            }
          >
            <Icon name="download" size={15} />
            Speichern…
          </button>
        </div>
        {privateText && <textarea className="key-text" readOnly rows={8} value={privateText} />}
        <em
          className="field-hint"
          data-tone={format === 'putty-v2' && passphrase ? 'warning' : undefined}
        >
          {passphraseNote(format, Boolean(passphrase))} Kopiert wird am Verlauf von Windows vorbei,
          nach einer Minute ist die Zwischenablage wieder leer.
        </em>
      </fieldset>

      {onStore && (
        <div className="keygen-store">
          <label className="field grow">
            <span>Name im Tresor</span>
            <input value={label} onChange={(e) => setLabel(e.target.value)} />
          </label>
          <button
            className="primary"
            disabled={busy}
            onClick={() =>
              void run(async () => {
                const stored = await onStore(generated.token, label || info.label, pass);
                tokenRef.current = null;
                onStored?.(stored);
              })
            }
          >
            <Icon name="key" size={15} />
            {storeLabel ?? 'In den Tresor legen'}
          </button>
        </div>
      )}

      {message && (
        <p className={message.tone === 'error' ? 'field-error' : 'field-hint'} role="status">
          {message.text}
        </p>
      )}

      <div className="keygen-actions">
        <button
          className="quiet"
          onClick={() => {
            setGenerated(null);
            setPrivateText(null);
            setCollected(0);
            moments.current = 0;
            lastMoment.current = { x: -1e6, y: -1e6, time: -1e6 };
            setStep('settings');
          }}
        >
          Noch einen erzeugen
        </button>
      </div>
    </div>
  );
}
