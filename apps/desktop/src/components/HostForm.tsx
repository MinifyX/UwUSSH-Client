import { useState, type FormEvent } from 'react';
import {
  deleteHost,
  saveHost,
  type AuthMethod,
  type HostRecord,
  type SaveFailure,
} from '../lib/session';
import { Modal } from './Modal';

type Props = {
  /** The host to edit, or nothing for a new one. */
  host: HostRecord | null;
  onSaved: (host: HostRecord) => void;
  onDeleted: (id: string) => void;
  onCancel: () => void;
};

/** What the store's validation codes mean, next to the field they belong to. */
const PROBLEMS: Record<string, Record<string, string>> = {
  address: {
    required: 'Adresse fehlt',
    whitespace: 'Die Adresse darf keine Leerzeichen enthalten',
  },
  username: {
    required: 'Benutzername fehlt',
    whitespace: 'Der Benutzername darf keine Leerzeichen enthalten',
  },
  keyPath: { required: 'Pfad zur Key-Datei fehlt' },
  port: { 'out-of-range': 'Port zwischen 1 und 65535' },
};

type Errors = Partial<Record<'address' | 'port' | 'username' | 'keyPath' | 'form', string>>;

export function HostForm({ host, onSaved, onDeleted, onCancel }: Props) {
  const [name, setName] = useState(host?.name ?? '');
  const [address, setAddress] = useState(host?.address ?? '');
  const [port, setPort] = useState(String(host?.port ?? 22));
  const [username, setUsername] = useState(host?.username ?? '');
  const [auth, setAuth] = useState<AuthMethod>(host?.auth ?? 'password');
  const [keyPath, setKeyPath] = useState(host?.keyPath ?? '');
  const [groupPath, setGroupPath] = useState(host?.groupPath ?? '');
  const [errors, setErrors] = useState<Errors>({});
  const [busy, setBusy] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);

  /** Editing a field clears its error: a stale "Adresse fehlt" under a filled-in address is noise. */
  const edit =
    (field: keyof Errors, set: (value: string) => void) =>
    (event: { target: { value: string } }) => {
      set(event.target.value);
      if (errors[field] || errors.form)
        setErrors((current) => ({ ...current, [field]: undefined, form: undefined }));
    };

  const submit = async (event?: FormEvent) => {
    event?.preventDefault();
    const portNumber = Number(port);
    if (!Number.isInteger(portNumber) || portNumber < 1 || portNumber > 65535) {
      setErrors({ port: PROBLEMS.port?.['out-of-range'] });
      return;
    }

    setBusy(true);
    setErrors({});
    try {
      const saved = await saveHost({
        id: host?.id ?? null,
        name,
        address,
        port: portNumber,
        username,
        auth,
        keyPath: auth === 'key' ? keyPath : null,
        groupPath: groupPath || null,
      });
      onSaved(saved);
    } catch (raw) {
      const failure = raw as SaveFailure;
      if (failure?.kind === 'invalid') {
        setErrors({
          [failure.field]: PROBLEMS[failure.field]?.[failure.problem] ?? failure.problem,
        });
      } else {
        setErrors({ form: failure?.kind === 'error' ? failure.message : String(raw) });
      }
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    if (!host) return;
    if (!confirmDelete) {
      setConfirmDelete(true);
      return;
    }
    setBusy(true);
    try {
      await deleteHost(host.id);
      onDeleted(host.id);
    } catch (raw) {
      setErrors({ form: String(raw) });
      setBusy(false);
    }
  };

  return (
    <Modal
      title={host ? `${host.name} bearbeiten` : 'Neuer Host'}
      onCancel={onCancel}
      footer={
        <>
          {host && (
            <button className="danger" data-secondary onClick={remove} disabled={busy}>
              {confirmDelete ? 'Wirklich löschen' : 'Löschen'}
            </button>
          )}
          <span className="spacer" />
          <button data-secondary onClick={onCancel} disabled={busy}>
            Abbrechen
          </button>
          <button className="primary" onClick={() => void submit()} disabled={busy}>
            Speichern
          </button>
        </>
      }
    >
      <form className="form" onSubmit={submit}>
        <div className="form-row">
          <label className="field grow">
            <span>Adresse</span>
            <input
              value={address}
              onChange={edit('address', setAddress)}
              placeholder="10.0.0.12 oder prox-1.lan"
              aria-invalid={Boolean(errors.address)}
              autoComplete="off"
              spellCheck={false}
            />
            {errors.address && <em className="field-error">{errors.address}</em>}
          </label>
          <label className="field port">
            <span>Port</span>
            <input
              value={port}
              onChange={edit('port', setPort)}
              inputMode="numeric"
              aria-invalid={Boolean(errors.port)}
            />
            {errors.port && <em className="field-error">{errors.port}</em>}
          </label>
        </div>

        <label className="field">
          <span>Benutzer</span>
          <input
            value={username}
            onChange={edit('username', setUsername)}
            placeholder="root"
            aria-invalid={Boolean(errors.username)}
            autoComplete="off"
            spellCheck={false}
          />
          {errors.username && <em className="field-error">{errors.username}</em>}
        </label>

        <fieldset className="field">
          <span>Anmeldung</span>
          <div className="segmented" role="radiogroup">
            <button
              type="button"
              role="radio"
              aria-checked={auth === 'password'}
              onClick={() => setAuth('password')}
            >
              Passwort
            </button>
            <button
              type="button"
              role="radio"
              aria-checked={auth === 'key'}
              onClick={() => setAuth('key')}
            >
              Key-Datei
            </button>
          </div>
          {auth === 'password' && (
            <em className="field-hint">
              Wird bei jeder Verbindung abgefragt und nirgends gespeichert, bis der Vault kommt.
            </em>
          )}
        </fieldset>

        {auth === 'key' && (
          <label className="field">
            <span>Key-Datei</span>
            <input
              value={keyPath}
              onChange={edit('keyPath', setKeyPath)}
              placeholder="~/.ssh/id_ed25519"
              aria-invalid={Boolean(errors.keyPath)}
              autoComplete="off"
              spellCheck={false}
            />
            {errors.keyPath ? (
              <em className="field-error">{errors.keyPath}</em>
            ) : (
              <em className="field-hint">
                OpenSSH, PEM oder PuTTY-.ppk. Die Datei bleibt, wo sie ist.
              </em>
            )}
          </label>
        )}

        <div className="form-row">
          <label className="field grow">
            <span>Name</span>
            <input
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder={address || 'wie die Adresse'}
              autoComplete="off"
            />
          </label>
          <label className="field grow">
            <span>Gruppe</span>
            <input
              value={groupPath}
              onChange={(e) => setGroupPath(e.target.value)}
              placeholder="optional, z. B. homelab"
              autoComplete="off"
            />
          </label>
        </div>

        {errors.form && <p className="form-error">{errors.form}</p>}
        {/* Enter in any field submits. */}
        <button type="submit" hidden />
      </form>
    </Modal>
  );
}
