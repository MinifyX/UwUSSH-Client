import { useRef, useEffect, useState, type FormEvent } from 'react';
import { keyPublicLine, listKeys, type KeyRecord } from '../lib/keys';
import {
  deleteHost,
  saveHost,
  type AuthMethod,
  type GroupRecord,
  type HostRecord,
  type PasswordChange,
  type SaveFailure,
  type Workspace,
} from '../lib/session';
import { useCloseGuard } from './CloseGuard';
import { useSettings, workspaceName } from '../lib/settings';
import { Icon } from './Icon';
import { KeyImportDialog, KeygenDialog } from './keygen/KeyDialogs';
import { Modal } from './Modal';
import { VaultDialog } from './VaultDialog';

type Props = {
  /** The host to edit, or nothing for a new one. */
  host: HostRecord | null;
  /** Where a new host goes. */
  workspace?: Workspace;
  group?: string | null;
  groups: GroupRecord[];
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
  keyId: { unknown: 'Diesen Key gibt es nicht mehr', required: 'Wähle einen Key' },
  port: { 'out-of-range': 'Port zwischen 1 und 65535' },
  password: { required: 'Das Passwort ist leer' },
  groupPath: { 'too-long': 'Der Gruppenname ist zu lang', control: 'Ungültiger Gruppenname' },
};

type Field =
  'address' | 'port' | 'username' | 'keyPath' | 'keyId' | 'password' | 'groupPath' | 'form';
type Errors = Partial<Record<Field, string>>;

type KeySource = 'vault' | 'file';

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

export function HostForm({ host, workspace, group, groups, onSaved, onDeleted, onCancel }: Props) {
  const settings = useSettings();
  const [name, setName] = useState(host?.name ?? '');
  const [address, setAddress] = useState(host?.address ?? '');
  const [port, setPort] = useState(String(host?.port ?? 22));
  const [username, setUsername] = useState(host?.username ?? '');
  const [auth, setAuth] = useState<AuthMethod>(host?.auth ?? 'password');
  const [keySource, setKeySource] = useState<KeySource>(host?.keyPath ? 'file' : 'vault');
  const [keyPath, setKeyPath] = useState(host?.keyPath ?? '');
  const [keyId, setKeyId] = useState<string>(host?.keyId ?? '');
  const [keys, setKeys] = useState<KeyRecord[]>([]);
  const [space, setSpace] = useState<Workspace>(host?.workspace ?? workspace ?? 'private');
  const [groupPath, setGroupPath] = useState(host?.groupPath ?? group ?? '');
  /** The password field: `null` keeps what is stored. */
  const [password, setPassword] = useState<string | null>(host?.hasPassword ? null : '');
  const [forget, setForget] = useState(false);
  const [showPassword, setShowPassword] = useState(false);
  const [errors, setErrors] = useState<Errors>({});
  const [busy, setBusy] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [vault, setVault] = useState(false);
  const [keygen, setKeygen] = useState(false);
  const [keyImport, setKeyImport] = useState(false);
  const [copied, setCopied] = useState(false);

  // What the form holds, compared with what it opened with: closing a form
  // that changed asks first. A key only counts when the host uses keys, since
  // the list picks one on its own once it has loaded.
  const snapshot = JSON.stringify([
    name,
    address,
    port,
    username,
    auth,
    auth === 'key' ? [keySource, keyPath, keyId] : null,
    space,
    groupPath,
    password ?? null,
    forget,
  ]);
  const opened = useRef(snapshot);
  const guard = useCloseGuard(
    snapshot !== opened.current,
    onCancel,
    host
      ? 'Deine Änderungen an diesem Host gehen dabei verloren.'
      : 'Der neue Host ist noch nicht gespeichert und geht dabei verloren.',
  );

  const loadKeys = (select?: string) =>
    void listKeys()
      .then((found) => {
        setKeys(found);
        if (select) setKeyId(select);
        else if (!keyId && found.length === 1 && !host) setKeyId(found[0]!.id);
      })
      .catch(() => undefined);

  useEffect(() => {
    loadKeys();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /** Editing a field clears its error: a stale "Adresse fehlt" under a filled-in address is noise. */
  const edit =
    (field: Field, set: (value: string) => void) => (event: { target: { value: string } }) => {
      set(event.target.value);
      if (errors[field] || errors.form)
        setErrors((current) => ({ ...current, [field]: undefined, form: undefined }));
    };

  const passwordChange = (): PasswordChange => {
    if (forget) return { kind: 'forget' };
    if (password === null || password === '') return { kind: 'keep' };
    return { kind: 'set', value: password };
  };

  const submit = async (event?: FormEvent) => {
    event?.preventDefault();
    const portNumber = Number(port);
    if (!Number.isInteger(portNumber) || portNumber < 1 || portNumber > 65535) {
      setErrors({ port: PROBLEMS.port?.['out-of-range'] });
      return;
    }
    if (auth === 'key' && keySource === 'vault' && !keyId) {
      setErrors({ keyId: PROBLEMS.keyId?.required });
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
        keyPath: auth === 'key' && keySource === 'file' ? keyPath : null,
        keyId: auth === 'key' && keySource === 'vault' ? keyId : null,
        groupPath: groupPath || null,
        workspace: space,
        password: passwordChange(),
      });
      setPassword(null);
      onSaved(saved);
    } catch (raw) {
      const failure = raw as SaveFailure;
      if (failure?.kind === 'vault-locked') {
        setVault(true);
      } else if (failure?.kind === 'invalid') {
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

  const selectedKey = keys.find((k) => k.id === keyId) ?? null;
  const groupNames = [...new Set(groups.filter((g) => g.workspace === space).map((g) => g.name))];
  const storedPassword = Boolean(host?.hasPassword) && !forget && password === null;

  const passwordBlock = (label: string, hint: string) =>
    storedPassword ? (
      <div className="stored-secret">
        <Icon name="lock" size={15} />
        <span>
          <b>{label}</b> ist im Tresor gespeichert
        </span>
        <span className="spacer" />
        <button type="button" className="quiet" onClick={() => setPassword('')}>
          Ändern
        </button>
        <button type="button" className="quiet" onClick={() => setForget(true)}>
          Entfernen
        </button>
      </div>
    ) : forget ? (
      <div className="stored-secret" data-forgotten>
        <Icon name="unlock" size={15} />
        <span>Das gespeicherte Passwort wird beim Speichern entfernt.</span>
        <span className="spacer" />
        <button type="button" className="quiet" onClick={() => setForget(false)}>
          Rückgängig
        </button>
      </div>
    ) : (
      <label className="field">
        <span>{label}</span>
        <span className="input-with-button">
          <input
            type={showPassword ? 'text' : 'password'}
            value={password ?? ''}
            onChange={edit('password', (value) => setPassword(value))}
            placeholder="leer lassen: beim Verbinden fragen"
            autoComplete="new-password"
            aria-invalid={Boolean(errors.password)}
          />
          <button
            type="button"
            className="icon-button"
            onClick={() => setShowPassword((v) => !v)}
            aria-label={showPassword ? 'Passwort verbergen' : 'Passwort anzeigen'}
          >
            <Icon name="eye" size={15} />
          </button>
        </span>
        {errors.password ? (
          <em className="field-error">{errors.password}</em>
        ) : (
          <em className="field-hint">{hint}</em>
        )}
      </label>
    );

  return (
    <>
      <Modal
        title={host ? `${host.name} bearbeiten` : 'Neuer Host'}
        onCancel={guard.request}
        footer={
          <>
            {host && (
              <button className="danger" data-secondary onClick={remove} disabled={busy}>
                {confirmDelete ? 'Wirklich löschen' : 'Löschen'}
              </button>
            )}
            <span className="spacer" />
            <button data-secondary onClick={guard.request} disabled={busy}>
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

          <div className="form-row">
            <label className="field grow">
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
            <label className="field grow">
              <span>Name</span>
              <input
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder={address || 'wie die Adresse'}
                autoComplete="off"
              />
            </label>
          </div>

          <fieldset className="field">
            <span>Anmeldung</span>
            <div className="segmented" role="radiogroup" aria-label="Anmeldung">
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
                SSH-Key
              </button>
            </div>
          </fieldset>

          {auth === 'password' &&
            passwordBlock(
              'Passwort',
              'Gespeichert wird es verschlüsselt im Tresor – UwUSSH verbindet dann ohne zu fragen und tippt es auf Wunsch für sudo.',
            )}

          {auth === 'key' && (
            <div className="key-choice">
              <div className="segmented" role="radiogroup" aria-label="Woher der Key kommt">
                <button
                  type="button"
                  role="radio"
                  aria-checked={keySource === 'vault'}
                  onClick={() => setKeySource('vault')}
                >
                  Aus dem Tresor
                </button>
                <button
                  type="button"
                  role="radio"
                  aria-checked={keySource === 'file'}
                  onClick={() => setKeySource('file')}
                >
                  Key-Datei
                </button>
              </div>

              {keySource === 'vault' ? (
                <>
                  <label className="field">
                    <span>Key</span>
                    <select
                      className="select"
                      value={keyId}
                      onChange={edit('keyId', setKeyId)}
                      aria-invalid={Boolean(errors.keyId)}
                    >
                      <option value="">
                        {keys.length === 0 ? 'Noch keine Keys im Tresor' : 'Key wählen…'}
                      </option>
                      {keys.map((key) => (
                        <option key={key.id} value={key.id}>
                          {key.label} · {key.keyType.replace(/^ssh-|^ecdsa-sha2-/, '')}
                        </option>
                      ))}
                    </select>
                    {errors.keyId && <em className="field-error">{errors.keyId}</em>}
                  </label>
                  <div className="key-buttons">
                    <button type="button" onClick={() => setKeygen(true)}>
                      <Icon name="sparkles" size={15} />
                      Neuen Key erzeugen…
                    </button>
                    <button type="button" onClick={() => setKeyImport(true)}>
                      <Icon name="import" size={15} />
                      Key-Datei importieren…
                    </button>
                    {selectedKey && (
                      <button
                        type="button"
                        className="quiet"
                        onClick={() =>
                          void keyPublicLine(selectedKey.id)
                            .then(copy)
                            .then(() => {
                              setCopied(true);
                              window.setTimeout(() => setCopied(false), 1600);
                            })
                            .catch(() => setVault(true))
                        }
                        title="Für ~/.ssh/authorized_keys auf dem Server"
                      >
                        <Icon name={copied ? 'check' : 'copy'} size={15} />
                        {copied ? 'Kopiert' : 'Public Key kopieren'}
                      </button>
                    )}
                  </div>
                </>
              ) : (
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

              {passwordBlock(
                'Passwort für sudo',
                'Optional. Wenn sudo im Terminal fragt, tippt UwUSSH es auf Knopfdruck ein – und nutzt es für Dateien als root.',
              )}
            </div>
          )}

          <div className="form-row">
            {settings.workspaces && (
              <fieldset className="field">
                <span>Bereich</span>
                <div className="segmented" role="radiogroup" aria-label="Bereich">
                  {(['private', 'business'] as const).map((id) => (
                    <button
                      key={id}
                      type="button"
                      role="radio"
                      aria-checked={space === id}
                      onClick={() => setSpace(id)}
                    >
                      {workspaceName(id, settings)}
                    </button>
                  ))}
                </div>
              </fieldset>
            )}
            <label className="field grow">
              <span>Gruppe</span>
              <input
                value={groupPath}
                onChange={edit('groupPath', setGroupPath)}
                placeholder="optional, z. B. homelab"
                autoComplete="off"
                list="host-form-groups"
                aria-invalid={Boolean(errors.groupPath)}
              />
              <datalist id="host-form-groups">
                {groupNames.map((groupName) => (
                  <option key={groupName} value={groupName} />
                ))}
              </datalist>
              {errors.groupPath && <em className="field-error">{errors.groupPath}</em>}
            </label>
          </div>

          {errors.form && <p className="form-error">{errors.form}</p>}
          {/* Enter in any field submits. */}
          <button type="submit" hidden />
        </form>
      </Modal>

      {guard.dialog}
      {vault && (
        <VaultDialog
          reason="Das Passwort wird verschlüsselt im Tresor gespeichert."
          onDone={() => {
            setVault(false);
            void submit();
          }}
          onCancel={() => setVault(false)}
        />
      )}
      {keygen && (
        <KeygenDialog
          comment={username && address ? `${username}@${address}` : undefined}
          storeLabel="Für diesen Host verwenden"
          onStored={(key) => {
            loadKeys(key.id);
            setKeySource('vault');
          }}
          onClose={() => setKeygen(false)}
        />
      )}
      {keyImport && (
        <KeyImportDialog
          onImported={(key) => {
            loadKeys(key.id);
            setKeySource('vault');
          }}
          onClose={() => setKeyImport(false)}
        />
      )}
    </>
  );
}
