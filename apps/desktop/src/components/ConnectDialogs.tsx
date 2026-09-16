import { useState, type FormEvent } from 'react';
import type { HostRecord, ObservedHostKey } from '../lib/session';
import { Modal } from './Modal';

/*
 * The questions connecting can raise.
 *
 * Wording rule from docs/design.md, and not negotiable: dialogs about host keys
 * are plain. No kaomoji, no Nyu, no jokes — a possible man in the middle is the
 * one moment the app must not sound like it is playing.
 */

// ── Password / passphrase ───────────────────────────────────────────────────

type SecretPromptProps = {
  host: HostRecord;
  secret: 'password' | 'passphrase';
  /** The previous attempt was rejected. */
  retry: boolean;
  onSubmit: (value: string) => void;
  onCancel: () => void;
};

export function SecretPrompt({ host, secret, retry, onSubmit, onCancel }: SecretPromptProps) {
  const [value, setValue] = useState('');

  const submit = (event?: FormEvent) => {
    event?.preventDefault();
    const entered = value;
    // Do not keep the secret in component state any longer than the submit.
    setValue('');
    onSubmit(entered);
  };

  const password = secret === 'password';
  return (
    <Modal
      title={password ? 'Passwort' : 'Passphrase'}
      onCancel={onCancel}
      footer={
        <>
          <span className="spacer" />
          <button data-secondary onClick={onCancel}>
            Abbrechen
          </button>
          <button className="primary" onClick={() => submit()}>
            Verbinden
          </button>
        </>
      }
    >
      <form className="form" onSubmit={submit}>
        <p className="dialog-lead">
          {password ? (
            <>
              für{' '}
              <code>
                {host.username}@{host.address}
              </code>
            </>
          ) : (
            <>
              für den Key <code>{host.keyPath}</code>
            </>
          )}
        </p>
        <label className="field">
          <span className="sr-only">{password ? 'Passwort' : 'Passphrase'}</span>
          <input
            type="password"
            value={value}
            onChange={(e) => setValue(e.target.value)}
            autoComplete="off"
            aria-invalid={retry}
          />
          {retry && (
            <em className="field-error">
              {password
                ? 'Der Server hat das Passwort abgelehnt.'
                : 'Die Passphrase passt nicht zu diesem Key.'}
            </em>
          )}
        </label>
        <p className="field-hint">
          Wird nur für diese eine Verbindung verwendet und nicht gespeichert.
        </p>
        <button type="submit" hidden />
      </form>
    </Modal>
  );
}

// ── First contact ───────────────────────────────────────────────────────────

type KeyFactsProps = { observed: ObservedHostKey };

function KeyFacts({ observed }: KeyFactsProps) {
  return (
    <div className="key-facts">
      <pre className="randomart" aria-label="Randomart des Schlüssels">
        {observed.randomart}
      </pre>
      <dl>
        <dt>Typ</dt>
        <dd>
          <code>{observed.algorithm}</code>
        </dd>
        <dt>Fingerprint</dt>
        <dd>
          <code className="fingerprint">{observed.fingerprint}</code>
        </dd>
      </dl>
    </div>
  );
}

type TrustProps = {
  host: HostRecord;
  observed: ObservedHostKey;
  onTrust: () => void;
  onCancel: () => void;
};

export function TrustHostKey({ host, observed, onTrust, onCancel }: TrustProps) {
  return (
    <Modal
      title="Unbekannter Host-Key"
      onCancel={onCancel}
      footer={
        <>
          <span className="spacer" />
          <button data-secondary onClick={onCancel}>
            Abbrechen
          </button>
          <button className="primary" data-secondary onClick={onTrust}>
            Vertrauen und verbinden
          </button>
        </>
      }
    >
      <p className="dialog-lead">
        Erste Verbindung zu{' '}
        <code>
          {host.address}:{host.port}
        </code>
        . Bisher wurde nichts gesendet — auch kein Passwort.
      </p>
      <KeyFacts observed={observed} />
      <p className="field-hint">
        Vergleiche den Fingerprint mit dem, was der Server über sich selbst sagt, etwa per{' '}
        <code>ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub</code> auf dem Server. Stimmt er,
        merkt sich UwUSSH den Schlüssel und fragt beim nächsten Mal nicht mehr.
      </p>
    </Modal>
  );
}

// ── The key changed ─────────────────────────────────────────────────────────

type ChangedProps = {
  host: HostRecord;
  trustedFingerprint: string;
  observed: ObservedHostKey;
  onReplace: (confirmation: string) => void;
  onCancel: () => void;
};

export function HostKeyChanged({
  host,
  trustedFingerprint,
  observed,
  onReplace,
  onCancel,
}: ChangedProps) {
  const [typed, setTyped] = useState('');
  const confirmed = typed.trim().toLowerCase() === host.address.trim().toLowerCase();

  return (
    <Modal
      title="Der Host-Key hat sich geändert"
      tone="warning"
      onCancel={onCancel}
      footer={
        <>
          <button
            className="danger"
            data-secondary
            disabled={!confirmed}
            onClick={() => onReplace(typed)}
          >
            Neuen Schlüssel übernehmen
          </button>
          <span className="spacer" />
          <button className="primary" data-autofocus onClick={onCancel}>
            Nicht verbinden
          </button>
        </>
      }
    >
      <p className="dialog-lead">
        <code>
          {host.address}:{host.port}
        </code>{' '}
        zeigt einen anderen Schlüssel als beim letzten Mal. Das kann ein neu aufgesetzter Server
        sein — oder jemand, der sich zwischen dich und den Server schaltet.
      </p>
      <p className="dialog-lead">
        <strong>Die Verbindung wurde abgebrochen. Es wurde nichts gesendet.</strong>
      </p>

      <dl className="key-compare">
        <dt>Bisher vertraut</dt>
        <dd>
          <code className="fingerprint">{trustedFingerprint}</code>
        </dd>
        <dt>Jetzt präsentiert</dt>
        <dd>
          <code className="fingerprint">{observed.fingerprint}</code>
        </dd>
      </dl>
      <pre className="randomart">{observed.randomart}</pre>

      <label className="field">
        <span>
          Nur wenn du weißt, dass der Server neu aufgesetzt wurde: tippe <code>{host.address}</code>{' '}
          ein, um den neuen Schlüssel zu übernehmen.
        </span>
        <input
          value={typed}
          onChange={(e) => setTyped(e.target.value)}
          autoComplete="off"
          spellCheck={false}
        />
      </label>
    </Modal>
  );
}
