import { Button, ICONS } from '@uwusuite/design';
import { useEffect, useId, useRef, useState } from 'react';
import { t, useLanguage } from '../../lib/i18n';
import {
  forgetPickedKey,
  asKeyFailure,
  importPickedKey,
  keygenStore,
  pasteKeyText,
  pickKeyFile,
  type KeyRecord,
  type PickedKey,
} from '../../lib/keys';
import { useCloseGuard } from '../CloseGuard';
import { Modal } from '../Modal';
import { VaultDialog } from '../VaultDialog';
import { KeygenPanel, type Step } from './KeygenPanel';

/**
 * UwUKeygen in a dialog. Storing needs the vault; when it is locked (or not
 * there yet) the vault dialog comes first and the key is stored right after.
 */
export function KeygenDialog({
  comment,
  storeLabel,
  onStored,
  onClose,
}: {
  comment?: string;
  storeLabel?: string;
  onStored?: (key: KeyRecord) => void;
  onClose: () => void;
}) {
  useLanguage();
  const [step, setStep] = useState<Step>('settings');
  const [vault, setVault] = useState<{ resolve: (open: boolean) => void } | null>(null);
  const guard = useCloseGuard(
    step !== 'settings',
    onClose,
    step === 'done'
      ? t('Der neue Schlüssel ist noch nicht gespeichert und geht dabei verloren.')
      : t('Der gesammelte Zufall geht dabei verloren.'),
  );

  const store = async (token: string, label: string, passphrase: string | null) => {
    for (;;) {
      try {
        return await keygenStore(token, label, passphrase);
      } catch (error) {
        if (asKeyFailure(error).kind !== 'vault-locked') throw error;
        const opened = await new Promise<boolean>((resolve) => setVault({ resolve }));
        setVault(null);
        if (!opened) throw error;
      }
    }
  };

  return (
    <>
      <Modal
        title={
          step === 'done'
            ? t('Dein neuer Schlüssel')
            : step === 'settings'
              ? t('UwUKeygen – neuer SSH-Schlüssel')
              : t('Zufall sammeln')
        }
        size="wide"
        onCancel={guard.request}
      >
        <KeygenPanel
          comment={comment}
          onStore={store}
          storeLabel={storeLabel}
          onStep={setStep}
          onStored={(key) => {
            onStored?.(key);
            onClose();
          }}
        />
      </Modal>
      {guard.dialog}
      {vault && (
        <VaultDialog
          reason={t('Der neue Schlüssel wird verschlüsselt im Tresor abgelegt.')}
          onDone={() => vault.resolve(true)}
          onCancel={() => vault.resolve(false)}
        />
      )}
    </>
  );
}

/**
 * Put an existing private key into the vault: a file, or the key pasted as
 * text (password managers such as Vaultwarden keep keys as fields, not files).
 */
export function KeyImportDialog({
  onImported,
  onClose,
}: {
  onImported: (key: KeyRecord) => void;
  onClose: () => void;
}) {
  useLanguage();
  const [picked, setPicked] = useState<PickedKey | null>(null);
  const [label, setLabel] = useState('');
  const [passphrase, setPassphrase] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [vault, setVault] = useState(false);
  const [pasted, setPasted] = useState('');
  // A paste error shows once the field was left or pasted into, not mid-typing.
  const [committed, setCommitted] = useState(false);
  const [pasteError, setPasteError] = useState<string | null>(null);
  const reading = useRef(0);
  const ids = useId();
  const guard = useCloseGuard(
    picked !== null || pasted.trim() !== '',
    onClose,
    t('Der Key wird dann nicht importiert.'),
  );

  const typing = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => () => clearTimeout(typing.current), []);

  // Read once typing pauses, not per keystroke: each call carries the whole key.
  const onPasted = (text: string) => {
    setPasted(text);
    setPicked(null);
    setPasteError(null);
    const run = ++reading.current;
    clearTimeout(typing.current);
    if (text.trim()) typing.current = setTimeout(() => void readPasted(text, run), 300);
  };

  const readPasted = async (text: string, run: number) => {
    try {
      const key = await pasteKeyText(text);
      if (run !== reading.current) return;
      setPicked(key);
      setLabel((current) => current || key.info?.comment || '');
    } catch (e) {
      if (run !== reading.current) return;
      const failure = asKeyFailure(e);
      setPasteError(
        t('Das ist kein lesbarer Private Key ({reason}).', {
          reason: failure.kind === 'error' ? failure.message : failure.kind,
        }),
      );
    }
  };

  const pick = async () => {
    setError(null);
    try {
      const file = await pickKeyFile();
      if (!file) return;
      reading.current++;
      clearTimeout(typing.current);
      setPasted('');
      setPasteError(null);
      setPicked(file);
      setLabel(file.fileName.replace(/\.(ppk|pem|key)$/i, ''));
    } catch (e) {
      const failure = asKeyFailure(e);
      setError(
        failure.kind === 'error' ? failure.message : t('Diese Datei ist kein lesbarer Key.'),
      );
    }
  };

  // The key file stays in memory only while this dialog is open.
  useEffect(() => () => void forgetPickedKey().catch(() => {}), []);

  const submit = async () => {
    if (!picked) return;
    setBusy(true);
    setError(null);
    try {
      const key = await importPickedKey(picked.token, label || null, passphrase || null);
      setPassphrase('');
      onImported(key);
      onClose();
    } catch (e) {
      const failure = asKeyFailure(e);
      if (failure.kind === 'vault-locked') setVault(true);
      else if (failure.kind === 'passphrase-wrong' || failure.kind === 'passphrase-required')
        setError(t('Die Passphrase passt nicht zu diesem Key.'));
      else
        setError(
          failure.kind === 'error' ? failure.message : t('Fehler ({kind})', { kind: failure.kind }),
        );
    } finally {
      setBusy(false);
    }
  };

  if (vault) {
    return (
      <VaultDialog
        reason={t('Der Key wird verschlüsselt im Tresor abgelegt.')}
        onDone={() => {
          setVault(false);
          void submit();
        }}
        onCancel={() => setVault(false)}
      />
    );
  }

  return (
    <>
      <Modal
        title={t('Key importieren')}
        onCancel={guard.request}
        footer={
          <>
            <Button
              variant="ghost"
              data-secondary
              className="mr-auto"
              icon={ICONS.file}
              onClick={() => void pick()}
              disabled={busy}
            >
              {t('Datei wählen…')}
            </Button>
            <Button data-secondary onClick={guard.request} disabled={busy}>
              {t('Abbrechen')}
            </Button>
            <Button
              variant="primary"
              icon={ICONS.vault}
              busy={busy}
              disabled={!picked || (picked.encrypted && !passphrase)}
              onClick={() => void submit()}
            >
              {t('In den Tresor legen')}
            </Button>
          </>
        }
      >
        <form
          className="form"
          onSubmit={(event) => {
            event.preventDefault();
            void submit();
          }}
        >
          {!picked?.fileName && (
            <label className="field">
              <span>{t('Private Key einfügen')}</span>
              <em className="field-hint" id={`${ids}-hint`}>
                {t(
                  'OpenSSH, PEM oder PuTTY, z. B. aus Vaultwarden kopiert – oder eine Datei wählen.',
                )}
              </em>
              <textarea
                className="sync-code-input"
                rows={6}
                value={pasted}
                autoComplete="off"
                autoCapitalize="off"
                spellCheck={false}
                placeholder="-----BEGIN OPENSSH PRIVATE KEY-----"
                aria-describedby={
                  pasteError && committed ? `${ids}-hint ${ids}-error` : `${ids}-hint`
                }
                aria-invalid={pasteError !== null && committed}
                onPaste={() => setCommitted(true)}
                onBlur={() => setCommitted(true)}
                onChange={(e) => onPasted(e.target.value)}
              />
              {pasteError && committed && (
                <em className="field-error" id={`${ids}-error`} role="alert">
                  {pasteError}
                </em>
              )}
            </label>
          )}
          {picked && (
            <p className="dialog-lead">
              {picked.fileName ? <code>{picked.fileName}</code> : t('Eingefügter Key')}
              {picked.info ? ` · ${picked.info.label}` : ` · ${t('mit Passphrase geschützt')}`}
            </p>
          )}
          {picked && (
            <>
              {picked.info && (
                <p className="field-hint">
                  <code className="fingerprint">{picked.info.fingerprintSha256}</code>
                </p>
              )}
              <label className="field">
                <span>{t('Name im Tresor')}</span>
                <input value={label} onChange={(e) => setLabel(e.target.value)} />
              </label>
              {picked.encrypted && (
                <label className="field">
                  <span>{t('Passphrase')}</span>
                  <input
                    type="password"
                    data-autofocus
                    value={passphrase}
                    autoComplete="off"
                    onChange={(e) => setPassphrase(e.target.value)}
                  />
                  <em className="field-hint">{t('Wird mit dem Key im Tresor gespeichert.')}</em>
                </label>
              )}
            </>
          )}
          <button type="submit" hidden />
        </form>
        {error && (
          <p className="field-error" role="alert">
            {error}
          </p>
        )}
      </Modal>
      {guard.dialog}
    </>
  );
}
