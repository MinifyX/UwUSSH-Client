import { useState } from 'react';
import { asBackupFailure, exportHosts, type BackupSummary } from '../lib/backup';
import { Modal } from './Modal';
import { useCloseGuard } from './CloseGuard';
import { NyuScene } from './nyu/scenes';
import { VaultDialog } from './VaultDialog';

/**
 * Everything into one `.uwussh` file: hosts, groups, trusted host keys, and —
 * if wanted — stored passwords and keys, then sealed with a password of the
 * file's own. Read back with Importieren → UwUSSH-Export.
 */
export function ExportDialog({ onClose }: { onClose: () => void }) {
  const [secrets, setSecrets] = useState(true);
  const [password, setPassword] = useState('');
  const [repeat, setRepeat] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [vault, setVault] = useState(false);
  const [done, setDone] = useState<{ fileName: string; summary: BackupSummary } | null>(null);
  const guard = useCloseGuard(
    !done && password.length > 0,
    onClose,
    'Das eingegebene Passwort für die Datei geht dabei verloren.',
  );

  const mismatch = secrets && repeat.length > 0 && password !== repeat;
  const ready = !busy && (!secrets || (password.length >= 8 && password === repeat));

  const run = async () => {
    if (!ready) return;
    setBusy(true);
    setError(null);
    try {
      const result = await exportHosts(secrets, secrets ? password : null);
      if (result) {
        setPassword('');
        setRepeat('');
        setDone(result);
      }
    } catch (e) {
      const failure = asBackupFailure(e);
      if (failure.kind === 'vault-locked') setVault(true);
      else setError(failure.kind === 'error' ? failure.message : `Fehler (${failure.kind})`);
    } finally {
      setBusy(false);
    }
  };

  if (vault) {
    return (
      <VaultDialog
        reason="Um Passwörter und Keys mitzunehmen, muss der Tresor offen sein."
        onDone={() => {
          setVault(false);
          void run();
        }}
        onCancel={() => setVault(false)}
      />
    );
  }

  if (done) {
    const { summary } = done;
    return (
      <Modal
        title="Export gespeichert ✧"
        onCancel={onClose}
        footer={
          <>
            <span className="spacer" />
            <button className="primary" onClick={onClose}>
              Fertig
            </button>
          </>
        }
      >
        <NyuScene name="files" className="dialog-scene" />
        <p className="dialog-lead">
          <code>{done.fileName}</code>
        </p>
        <ul className="import-counts">
          {(
            [
              ['Hosts', summary.hosts],
              ['Gruppen', summary.groups],
              ['Passwörter', summary.passwords],
              ['Keys', summary.keys],
              ['Host-Keys', summary.knownHosts],
            ] as const
          ).map(([label, count]) => (
            <li key={label}>
              <b>{count}</b>
              <span>{label}</span>
            </li>
          ))}
        </ul>
      </Modal>
    );
  }

  return (
    <Modal
      title="Exportieren"
      onCancel={guard.request}
      footer={
        <>
          <span className="spacer" />
          <button data-secondary onClick={guard.request} disabled={busy}>
            Abbrechen
          </button>
          <button className="primary" onClick={() => void run()} disabled={!ready}>
            {busy ? 'Exportiere…' : 'Speichern unter…'}
          </button>
        </>
      }
    >
      <form
        className="form"
        onSubmit={(event) => {
          event.preventDefault();
          void run();
        }}
      >
        <p className="dialog-lead">
          Alle Hosts mit Bereichen, Gruppen und bekannten Host-Keys in eine Datei – zum Sichern oder
          für einen anderen Rechner.
        </p>
        <label className="check">
          <input type="checkbox" checked={secrets} onChange={(e) => setSecrets(e.target.checked)} />
          <span>
            <b>Passwörter und Keys mitnehmen</b>
            <small>
              Die ganze Datei wird dann mit einem eigenen Passwort verschlüsselt (Argon2id,
              XChaCha20-Poly1305).
            </small>
          </span>
        </label>
        {secrets && (
          <div className="form-row">
            <label className="field grow">
              <span>Passwort für die Datei</span>
              <input
                type="password"
                data-autofocus
                value={password}
                autoComplete="new-password"
                onChange={(e) => setPassword(e.target.value)}
              />
              {password.length > 0 && password.length < 8 && (
                <em className="field-hint">Mindestens 8 Zeichen.</em>
              )}
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
              {mismatch && <em className="field-error">Stimmt nicht überein.</em>}
            </label>
          </div>
        )}
        {!secrets && (
          <p className="field-hint">
            Ohne Geheimnisse ist die Datei lesbares JSON. Hosts mit gespeichertem Passwort fragen
            nach dem Import wieder beim Verbinden.
          </p>
        )}
        {error && (
          <p className="field-error" role="alert">
            {error}
          </p>
        )}
        <button type="submit" hidden />
      </form>
    </Modal>
  );
}
