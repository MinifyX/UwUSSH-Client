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

export type SecretKind = 'password' | 'passphrase' | 'sudo';

type SecretPromptProps = {
  host: HostRecord;
  secret: SecretKind;
  /** The previous attempt was rejected. */
  retry: boolean;
  /** Offer to keep a password that works in the vault. */
  canSave?: boolean;
  onSubmit: (value: string, save: boolean) => void;
  onCancel: () => void;
};

const SECRET_TITLES: Record<SecretKind, string> = {
  password: 'Passwort',
  passphrase: 'Passphrase',
  sudo: 'sudo-Passwort',
};

export function SecretPrompt({
  host,
  secret,
  retry,
  canSave = false,
  onSubmit,
  onCancel,
}: SecretPromptProps) {
  const [value, setValue] = useState('');
  const [save, setSave] = useState(true);

  const submit = (event?: FormEvent) => {
    event?.preventDefault();
    const entered = value;
    // Do not keep the secret in component state any longer than the submit.
    setValue('');
    onSubmit(entered, canSave && save);
  };

  const errorText: Record<SecretKind, string> = {
    password: 'Der Server hat das Passwort abgelehnt.',
    passphrase: 'Die Passphrase passt nicht zu diesem Key.',
    sudo: 'sudo hat das Passwort nicht angenommen.',
  };

  return (
    <Modal
      title={SECRET_TITLES[secret]}
      onCancel={onCancel}
      footer={
        <>
          <span className="spacer" />
          <button data-secondary onClick={onCancel}>
            Abbrechen
          </button>
          <button className="primary" onClick={() => submit()}>
            {secret === 'sudo' ? 'Als root öffnen' : 'Verbinden'}
          </button>
        </>
      }
    >
      <form className="form" onSubmit={submit}>
        <p className="dialog-lead">
          {secret === 'passphrase' ? (
            <>
              für den Key <code>{host.keyLabel ?? host.keyPath}</code>
            </>
          ) : secret === 'sudo' ? (
            <>
              <code>sudo</code> auf{' '}
              <code>
                {host.username}@{host.address}
              </code>{' '}
              fragt nach dem Passwort, um die Dateien als root zu öffnen.
            </>
          ) : (
            <>
              für{' '}
              <code>
                {host.username}@{host.address}
              </code>
            </>
          )}
        </p>
        <label className="field">
          <span className="sr-only">{SECRET_TITLES[secret]}</span>
          <input
            type="password"
            value={value}
            onChange={(e) => setValue(e.target.value)}
            autoComplete="off"
            aria-invalid={retry}
          />
          {retry && <em className="field-error">{errorText[secret]}</em>}
        </label>
        {canSave ? (
          <label className="check">
            <input type="checkbox" checked={save} onChange={(e) => setSave(e.target.checked)} />
            <span>
              <b>Im Tresor speichern</b>
              <small>
                Beim nächsten Mal verbindet UwUSSH ohne zu fragen, und tippt es für sudo.
              </small>
            </span>
          </label>
        ) : (
          <p className="field-hint">Wird nur für diese Verbindung verwendet.</p>
        )}
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
  onAccept: () => void;
  onReject: () => void;
};

/**
 * Accept or reject, with two buttons. Rejecting has the focus and is where
 * Enter and Escape lead; accepting needs a deliberate click, and the warning
 * above it says plainly what it can mean.
 */
export function HostKeyChanged({
  host,
  trustedFingerprint,
  observed,
  onAccept,
  onReject,
}: ChangedProps) {
  return (
    <Modal
      title="Der Host-Key hat sich geändert"
      tone="warning"
      onCancel={onReject}
      footer={
        <>
          <button className="danger" data-secondary onClick={onAccept}>
            Neuen Schlüssel akzeptieren
          </button>
          <span className="spacer" />
          <button className="primary" data-autofocus onClick={onReject}>
            Ablehnen
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

      <p className="field-hint">
        Akzeptiere nur, wenn du weißt, dass der Server neu aufgesetzt wurde oder seinen Schlüssel
        gewechselt hat. Im Zweifel: ablehnen und nachfragen.
      </p>
    </Modal>
  );
}
