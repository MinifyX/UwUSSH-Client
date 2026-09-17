import { useEffect, useRef, useState, type FormEvent, type ReactNode } from 'react';
import { createVault, unlockVault, vaultState } from '../lib/session';
import { Modal } from './Modal';
import { NyuScene } from './nyu/scenes';

type Props = {
  /** Why the vault is needed right now, in one sentence. */
  reason?: ReactNode;
  /** A button label for the dismissive choice; "Abbrechen" by default. */
  cancelLabel?: string;
  onDone: () => void;
  onCancel: () => void;
};

/**
 * Open the vault — or create it, if there is none yet. Either way the user can
 * let this Windows account open it from now on, so the master password is a
 * once-per-device thing (see `uwussh_store::device`). One dialog for every
 * place that needs the vault: app start, a host with a stored password, the
 * host form, an import, keys.
 */
export function VaultDialog({ reason, cancelLabel = 'Abbrechen', onDone, onCancel }: Props) {
  const [mode, setMode] = useState<'loading' | 'create' | 'unlock'>('loading');
  const [password, setPassword] = useState('');
  const [confirm, setConfirm] = useState('');
  const [remember, setRemember] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const passwordRef = useRef<HTMLInputElement>(null);

  // The fields appear once the status is known; the cursor goes there then.
  useEffect(() => {
    if (mode !== 'loading') passwordRef.current?.focus();
  }, [mode]);

  useEffect(() => {
    void vaultState()
      .then((state) => {
        if (state.status === 'unlocked') onDone();
        else setMode(state.status === 'absent' ? 'create' : 'unlock');
      })
      .catch((e) => setError(String(e)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const mismatch = mode === 'create' && confirm.length > 0 && password !== confirm;
  const ready =
    !busy && password.length > 0 && (mode === 'unlock' || (password === confirm && !mismatch));

  const submit = async (event?: FormEvent) => {
    event?.preventDefault();
    if (!ready) return;
    setBusy(true);
    setError(null);
    try {
      if (mode === 'create') await createVault(password, remember);
      else await unlockVault(password, remember);
      setPassword('');
      setConfirm('');
      onDone();
    } catch (e) {
      setError(
        mode === 'unlock' ? 'Das Master-Passwort war falsch.' : `Hat nicht geklappt: ${String(e)}`,
      );
      setPassword('');
      passwordRef.current?.focus();
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      title={
        mode === 'create' ? 'Tresor anlegen' : mode === 'unlock' ? 'Tresor entsperren' : 'Tresor'
      }
      onCancel={onCancel}
      footer={
        <>
          <span className="spacer" />
          <button data-secondary onClick={onCancel} disabled={busy}>
            {cancelLabel}
          </button>
          <button className="primary" onClick={() => void submit()} disabled={!ready}>
            {busy
              ? mode === 'create'
                ? 'Lege an…'
                : 'Entsperre…'
              : mode === 'create'
                ? 'Anlegen'
                : 'Entsperren'}
          </button>
        </>
      }
    >
      <NyuScene name="vault" className="dialog-scene" />
      {mode !== 'loading' && (
        <form className="form" onSubmit={submit}>
          <p className="dialog-lead">
            {reason ??
              (mode === 'create'
                ? 'Passwörter und Keys liegen verschlüsselt im Tresor. Dafür brauchst du einmal ein Master-Passwort.'
                : 'Passwörter und Keys liegen verschlüsselt im Tresor.')}
          </p>
          <label className="field">
            <span>Master-Passwort</span>
            <input
              ref={passwordRef}
              type="password"
              value={password}
              autoComplete={mode === 'create' ? 'new-password' : 'current-password'}
              onChange={(e) => setPassword(e.target.value)}
              aria-invalid={Boolean(error)}
            />
          </label>
          {mode === 'create' && (
            <label className="field">
              <span>Wiederholen</span>
              <input
                type="password"
                value={confirm}
                autoComplete="new-password"
                onChange={(e) => setConfirm(e.target.value)}
                aria-invalid={mismatch}
              />
              {mismatch && <em className="field-error">Die Passwörter stimmen nicht überein.</em>}
            </label>
          )}
          <label className="check">
            <input
              type="checkbox"
              checked={remember}
              onChange={(e) => setRemember(e.target.checked)}
            />
            <span>
              <b>Auf diesem Gerät merken</b>
              <small>
                Windows öffnet den Tresor für dein Benutzerkonto automatisch – du gibst das
                Master-Passwort hier nicht noch einmal ein.
              </small>
            </span>
          </label>
          {error && (
            <p className="field-error" role="alert">
              {error}
            </p>
          )}
          {mode === 'create' && (
            <p className="import-warning">
              Es gibt noch keine Wiederherstellung: Vergisst du das Master-Passwort, kommst du auf
              einem neuen Gerät nicht mehr an die gespeicherten Passwörter.
            </p>
          )}
          <button type="submit" hidden disabled={!ready} />
        </form>
      )}
    </Modal>
  );
}
