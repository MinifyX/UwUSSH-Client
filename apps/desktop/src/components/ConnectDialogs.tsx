import { Fragment, useState, type FormEvent, type ReactNode } from 'react';
import { N_, t, useLanguage } from '../lib/i18n';
import type { HostRecord, ObservedHostKey } from '../lib/session';
import { Modal } from './Modal';

/*
 * The questions connecting can raise.
 *
 * Wording rule from docs/design.md, and not negotiable: dialogs about host keys
 * are plain. No kaomoji, no Nyu, no jokes — a possible man in the middle is the
 * one moment the app must not sound like it is playing.
 */

/** `text` with its `{name}` placeholders replaced by elements. */
function withElements(text: string, elements: Record<string, ReactNode>): ReactNode[] {
  return text
    .split(/\{(\w+)\}/)
    .map((piece, index) =>
      index % 2 === 0 ? piece : <Fragment key={index}>{elements[piece]}</Fragment>,
    );
}

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
  password: N_('Passwort'),
  passphrase: N_('Passphrase'),
  sudo: N_('sudo-Passwort'),
};

export function SecretPrompt({
  host,
  secret,
  retry,
  canSave = false,
  onSubmit,
  onCancel,
}: SecretPromptProps) {
  useLanguage();
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
    password: t('Der Server hat das Passwort abgelehnt.'),
    passphrase: t('Die Passphrase passt nicht zu diesem Key.'),
    sudo: t('sudo hat das Passwort nicht angenommen.'),
  };

  return (
    <Modal
      title={t(SECRET_TITLES[secret])}
      onCancel={onCancel}
      footer={
        <>
          <span className="spacer" />
          <button data-secondary onClick={onCancel}>
            {t('Abbrechen')}
          </button>
          <button className="primary" onClick={() => submit()}>
            {secret === 'sudo' ? t('Als root öffnen') : t('Verbinden')}
          </button>
        </>
      }
    >
      <form className="form" onSubmit={submit}>
        <p className="dialog-lead">
          {secret === 'passphrase'
            ? withElements(t('für den Key {key}'), {
                key: <code>{host.keyLabel ?? host.keyPath}</code>,
              })
            : secret === 'sudo'
              ? withElements(
                  t(
                    '{sudo} auf {target} fragt nach dem Passwort, um die Dateien als root zu öffnen.',
                  ),
                  {
                    sudo: <code>sudo</code>,
                    target: (
                      <code>
                        {host.username}@{host.address}
                      </code>
                    ),
                  },
                )
              : withElements(t('für {target}'), {
                  target: (
                    <code>
                      {host.username}@{host.address}
                    </code>
                  ),
                })}
        </p>
        <label className="field">
          <span className="sr-only">{t(SECRET_TITLES[secret])}</span>
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
              <b>{t('Im Tresor speichern')}</b>
              <small>
                {t('Beim nächsten Mal verbindet UwUSSH ohne zu fragen, und tippt es für sudo.')}
              </small>
            </span>
          </label>
        ) : (
          <p className="field-hint">{t('Wird nur für diese Verbindung verwendet.')}</p>
        )}
        <button type="submit" hidden />
      </form>
    </Modal>
  );
}

// ── First contact ───────────────────────────────────────────────────────────

type KeyFactsProps = { observed: ObservedHostKey };

function KeyFacts({ observed }: KeyFactsProps) {
  useLanguage();
  return (
    <div className="key-facts">
      <pre className="randomart" aria-label={t('Randomart des Schlüssels')}>
        {observed.randomart}
      </pre>
      <dl>
        <dt>{t('Typ')}</dt>
        <dd>
          <code>{observed.algorithm}</code>
        </dd>
        <dt>{t('Fingerprint')}</dt>
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
  useLanguage();
  return (
    <Modal
      title={t('Unbekannter Host-Key')}
      onCancel={onCancel}
      footer={
        <>
          <span className="spacer" />
          <button data-secondary onClick={onCancel}>
            {t('Abbrechen')}
          </button>
          <button className="primary" data-secondary onClick={onTrust}>
            {t('Vertrauen und verbinden')}
          </button>
        </>
      }
    >
      <p className="dialog-lead">
        {withElements(
          t('Erste Verbindung zu {address}. Bisher wurde nichts gesendet — auch kein Passwort.'),
          {
            address: (
              <code>
                {host.address}:{host.port}
              </code>
            ),
          },
        )}
      </p>
      <KeyFacts observed={observed} />
      <p className="field-hint">
        {withElements(
          t(
            'Vergleiche den Fingerprint mit dem, was der Server über sich selbst sagt, etwa per {command} auf dem Server. Stimmt er, merkt sich UwUSSH den Schlüssel und fragt beim nächsten Mal nicht mehr.',
          ),
          { command: <code>ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub</code> },
        )}
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
  useLanguage();
  return (
    <Modal
      title={t('Der Host-Key hat sich geändert')}
      tone="warning"
      onCancel={onReject}
      footer={
        <>
          <button className="danger" data-secondary onClick={onAccept}>
            {t('Neuen Schlüssel akzeptieren')}
          </button>
          <span className="spacer" />
          <button className="primary" data-autofocus onClick={onReject}>
            {t('Ablehnen')}
          </button>
        </>
      }
    >
      <p className="dialog-lead">
        {withElements(
          t(
            '{address} zeigt einen anderen Schlüssel als beim letzten Mal. Das kann ein neu aufgesetzter Server sein — oder jemand, der sich zwischen dich und den Server schaltet.',
          ),
          {
            address: (
              <code>
                {host.address}:{host.port}
              </code>
            ),
          },
        )}
      </p>
      <p className="dialog-lead">
        <strong>{t('Die Verbindung wurde abgebrochen. Es wurde nichts gesendet.')}</strong>
      </p>

      <dl className="key-compare">
        <dt>{t('Bisher vertraut')}</dt>
        <dd>
          <code className="fingerprint">{trustedFingerprint}</code>
        </dd>
        <dt>{t('Jetzt präsentiert')}</dt>
        <dd>
          <code className="fingerprint">{observed.fingerprint}</code>
        </dd>
      </dl>
      <pre className="randomart">{observed.randomart}</pre>

      <p className="field-hint">
        {t(
          'Akzeptiere nur, wenn du weißt, dass der Server neu aufgesetzt wurde oder seinen Schlüssel gewechselt hat. Im Zweifel: ablehnen und nachfragen.',
        )}
      </p>
    </Modal>
  );
}
