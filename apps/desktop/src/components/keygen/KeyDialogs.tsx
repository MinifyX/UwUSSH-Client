import { useEffect, useRef, useState } from 'react';
import { t, useLanguage } from '../../lib/i18n';
import {
  forgetPickedKey,
  asKeyFailure,
  importPickedKey,
  keygenStore,
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
        <div className="keygen-dialog">
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
        </div>
        <button
          className="settings-close icon-button"
          onClick={guard.request}
          aria-label={t('Schließen')}
        >
          ×
        </button>
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

/** Put an existing private key file into the vault. */
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
  const started = useRef(false);
  const guard = useCloseGuard(
    picked !== null,
    onClose,
    t('Die gewählte Key-Datei wird dann nicht importiert.'),
  );

  const pick = async () => {
    setError(null);
    try {
      const file = await pickKeyFile();
      if (!file) {
        if (!picked) onClose();
        return;
      }
      setPicked(file);
      setLabel(file.fileName.replace(/\.(ppk|pem|key)$/i, ''));
    } catch (e) {
      const failure = asKeyFailure(e);
      setError(
        failure.kind === 'error' ? failure.message : t('Diese Datei ist kein lesbarer Key.'),
      );
    }
  };

  useEffect(() => {
    // Straight to the file dialog; StrictMode must not open it twice.
    if (started.current) return;
    started.current = true;
    void pick();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

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
            <button data-secondary onClick={() => void pick()} disabled={busy}>
              {t('Andere Datei…')}
            </button>
            <span className="spacer" />
            <button data-secondary onClick={guard.request} disabled={busy}>
              {t('Abbrechen')}
            </button>
            <button
              className="primary"
              disabled={!picked || busy || (picked.encrypted && !passphrase)}
              onClick={() => void submit()}
            >
              {t('In den Tresor legen')}
            </button>
          </>
        }
      >
        {!picked ? (
          <p className="import-note">{t('Wähle eine Key-Datei aus …')}</p>
        ) : (
          <form
            className="form"
            onSubmit={(event) => {
              event.preventDefault();
              void submit();
            }}
          >
            <p className="dialog-lead">
              <code>{picked.fileName}</code>
              {picked.info ? ` · ${picked.info.label}` : ` · ${t('mit Passphrase geschützt')}`}
            </p>
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
            <button type="submit" hidden />
          </form>
        )}
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
