import { useEffect, useState, type ReactNode } from 'react';
import pkg from '../../package.json';
import { customRegex, HIGHLIGHT_HEX } from '../lib/highlight';
import {
  passphraseNote,
  asKeyFailure,
  deleteKey,
  exportKeyFile,
  FORMAT_LABELS,
  keyPublicLine,
  listKeys,
  renameKey,
  type KeyRecord,
  type PrivateFormat,
} from '../lib/keys';
import {
  checkForUpdates,
  lockVault,
  openProjectPage,
  setVaultRemembered,
  vaultState,
  type ProjectPage,
  type UpdateInfo,
  type VaultState,
} from '../lib/session';
import {
  FONT_SIZE_MAX,
  FONT_SIZE_MIN,
  HIGHLIGHT_COLORS,
  SCROLLBACK_CHOICES,
  updateSettings,
  useSettings,
  workspaceName,
  type CursorStyle,
  type HighlightColor,
  type HighlightRule,
} from '../lib/settings';
import { ExportDialog } from './ExportDialog';
import { Icon } from './Icon';
import { KeyImportDialog, KeygenDialog } from './keygen/KeyDialogs';
import { Modal } from './Modal';
import { Nyu } from './nyu/Nyu';
import { VaultDialog } from './VaultDialog';

export type SettingsSection =
  'appearance' | 'terminal' | 'highlight' | 'vault' | 'data' | 'updates' | 'about';

const SECTIONS: { id: SettingsSection; label: string }[] = [
  { id: 'appearance', label: 'Darstellung' },
  { id: 'terminal', label: 'Terminal' },
  { id: 'highlight', label: 'Hervorhebung' },
  { id: 'vault', label: 'Tresor & Keys' },
  { id: 'data', label: 'Import & Export' },
  { id: 'updates', label: 'Updates' },
  { id: 'about', label: 'Über UwUSSH' },
];

type Props = {
  initial?: SettingsSection;
  onClose: () => void;
  /** A downloaded update, if one is waiting. */
  update: UpdateInfo | null;
  onUpdateFound: (update: UpdateInfo) => void;
  onInstallUpdate: () => void;
  onRunM0: () => void;
  onImport: () => void;
  /** Keys or hosts changed from here: the host list reloads. */
  onChanged: () => void;
};

/** One setting: a label, an optional explanation and its control. */
function Row({
  label,
  description,
  children,
}: {
  label: string;
  description?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="setting-row">
      <div className="setting-text">
        <p className="setting-label">{label}</p>
        {description && <p className="setting-description">{description}</p>}
      </div>
      <div className="setting-control">{children}</div>
    </div>
  );
}

function Segmented<T extends string | number>({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: T;
  options: { value: T; label: string }[];
  onChange: (value: T) => void;
}) {
  return (
    <div className="segmented" role="radiogroup" aria-label={label}>
      {options.map((option) => (
        <button
          key={String(option.value)}
          type="button"
          role="radio"
          aria-checked={option.value === value}
          onClick={() => onChange(option.value)}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}

function Toggle({
  label,
  checked,
  onChange,
}: {
  label: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <button
      type="button"
      role="switch"
      className="toggle"
      aria-checked={checked}
      aria-label={label}
      onClick={() => onChange(!checked)}
    >
      <span className="toggle-thumb" />
    </button>
  );
}

function Appearance() {
  const settings = useSettings();
  return (
    <>
      <Row label="Farbschema" description="Das Terminal bleibt in jedem Schema dunkel.">
        <Segmented
          label="Farbschema"
          value={settings.theme}
          onChange={(theme) => updateSettings({ theme })}
          options={[
            { value: 'system', label: 'System' },
            { value: 'light', label: 'Hell' },
            { value: 'dark', label: 'Dunkel' },
          ]}
        />
      </Row>
      <Row label="Animationen" description="„System“ folgt der Windows-Einstellung.">
        <Segmented
          label="Animationen"
          value={settings.motion}
          onChange={(motion) => updateSettings({ motion })}
          options={[
            { value: 'system', label: 'System' },
            { value: 'on', label: 'An' },
            { value: 'off', label: 'Aus' },
          ]}
        />
      </Row>
      <Row
        label="Privat und Business"
        description="Zwei Bereiche in der Hostliste, wie bei UwUMail. Hosts und Gruppen ziehst du einfach in den anderen Bereich."
      >
        <Toggle
          label="Privat und Business"
          checked={settings.workspaces}
          onChange={(workspaces) => updateSettings({ workspaces })}
        />
      </Row>
      {settings.workspaces && (
        <Row label="Namen der Bereiche" description="Leer lassen für „Privat“ und „Business“.">
          <div className="workspace-names">
            {(['private', 'business'] as const).map((id) => (
              <input
                key={id}
                className="search"
                value={settings.workspaceNames[id]}
                placeholder={id === 'private' ? 'Privat' : 'Business'}
                maxLength={24}
                aria-label={`Name für ${workspaceName(id, settings)}`}
                onChange={(e) =>
                  updateSettings({
                    workspaceNames: { ...settings.workspaceNames, [id]: e.target.value },
                  })
                }
              />
            ))}
          </div>
        </Row>
      )}
    </>
  );
}

function TerminalSettings() {
  const settings = useSettings();
  const sizes = Array.from(
    { length: FONT_SIZE_MAX - FONT_SIZE_MIN + 1 },
    (_, i) => FONT_SIZE_MIN + i,
  );
  return (
    <>
      <Row label="Schriftgröße" description="Oder Strg + Mausrad über dem Terminal.">
        <select
          className="select"
          aria-label="Schriftgröße"
          value={settings.fontSize}
          onChange={(event) => updateSettings({ fontSize: Number(event.target.value) })}
        >
          {sizes.map((size) => (
            <option key={size} value={size}>
              {size} px
            </option>
          ))}
        </select>
      </Row>
      <Row label="Cursor">
        <Segmented<CursorStyle>
          label="Cursorform"
          value={settings.cursorStyle}
          onChange={(cursorStyle) => updateSettings({ cursorStyle })}
          options={[
            { value: 'block', label: 'Block' },
            { value: 'bar', label: 'Strich' },
            { value: 'underline', label: 'Unterstrich' },
          ]}
        />
      </Row>
      <Row label="Cursor blinkt">
        <Toggle
          label="Cursor blinkt"
          checked={settings.cursorBlink}
          onChange={(cursorBlink) => updateSettings({ cursorBlink })}
        />
      </Row>
      <Row label="Scrollback" description="So viele Zeilen hält jeder Tab zum Zurückscrollen.">
        <Segmented
          label="Scrollback"
          value={settings.scrollback}
          onChange={(scrollback) => updateSettings({ scrollback })}
          options={SCROLLBACK_CHOICES.map((lines) => ({
            value: lines,
            label: lines.toLocaleString('de-DE'),
          }))}
        />
      </Row>
      <Row
        label="Passwort-Helfer"
        description="Fragt sudo oder su im Terminal nach dem Passwort, bietet UwUSSH an, das Passwort des Hosts einzutippen (Strg+Umschalt+P)."
      >
        <Toggle
          label="Passwort-Helfer"
          checked={settings.passwordHelper}
          onChange={(passwordHelper) => updateSettings({ passwordHelper })}
        />
      </Row>
      <Row
        label="Strg+C kopiert markierten Text"
        description="Ohne Markierung geht Strg+C wie immer als Abbruch an das Programm."
      >
        <Toggle
          label="Strg+C kopiert markierten Text"
          checked={settings.ctrlCCopies}
          onChange={(ctrlCCopies) => updateSettings({ ctrlCCopies })}
        />
      </Row>
      <Row
        label="Strg+V fügt ein"
        description="Aus: Strg+V geht als ^V an das Programm. Strg+Umschalt+V fügt immer ein."
      >
        <Toggle
          label="Strg+V fügt ein"
          checked={settings.ctrlVPastes}
          onChange={(ctrlVPastes) => updateSettings({ ctrlVPastes })}
        />
      </Row>
      <Row label="Beim Start eine lokale Shell öffnen">
        <Toggle
          label="Beim Start eine lokale Shell öffnen"
          checked={settings.openShellOnStart}
          onChange={(openShellOnStart) => updateSettings({ openShellOnStart })}
        />
      </Row>
      <Row
        label="Vor dem Schließen nachfragen"
        description="Wenn noch SSH-Verbindungen offen sind."
      >
        <Toggle
          label="Vor dem Schließen nachfragen"
          checked={settings.confirmCloseWithSessions}
          onChange={(confirmCloseWithSessions) => updateSettings({ confirmCloseWithSessions })}
        />
      </Row>
      <div className="shortcuts">
        <p className="setting-label">Tastenkürzel</p>
        <dl>
          <dt>Strg+Umschalt+T</dt>
          <dd>Neue lokale Shell</dd>
          <dt>Strg+Umschalt+D</dt>
          <dd>Tab duplizieren (neue Verbindung zum selben Host)</dd>
          <dt>Strg+Umschalt+W</dt>
          <dd>Tab schließen</dd>
          <dt>Strg+Tab · Strg+Umschalt+Tab</dt>
          <dd>Nächster · vorheriger Tab</dd>
          <dt>Strg+Umschalt+1 … 9</dt>
          <dd>Zu Tab 1 … 9</dd>
          <dt>Strg+Umschalt+F</dt>
          <dd>Dateien des Hosts öffnen</dd>
          <dt>Strg+Umschalt+P</dt>
          <dd>Passwort eintippen, wenn danach gefragt wird</dd>
          <dt>Strg+Umschalt+C · Strg+Umschalt+V</dt>
          <dd>Kopieren · Einfügen</dd>
          <dt>Strg+Mausrad</dt>
          <dd>Schrift größer · kleiner</dd>
          <dt>Strg+,</dt>
          <dd>Einstellungen</dd>
        </dl>
      </div>
    </>
  );
}

const COLOR_NAMES: Record<HighlightColor, string> = {
  red: 'Rot',
  yellow: 'Gelb',
  green: 'Grün',
  blue: 'Blau',
  magenta: 'Pink',
  cyan: 'Türkis',
};

function Swatch({ color }: { color: HighlightColor }) {
  return <i className="swatch" style={{ background: HIGHLIGHT_HEX[color] }} aria-hidden />;
}

function Highlighting() {
  const settings = useSettings();
  const highlight = settings.highlight;
  const set = (patch: Partial<typeof highlight>) =>
    updateSettings({ highlight: { ...highlight, ...patch } });
  const [pattern, setPattern] = useState('');
  const [color, setColor] = useState<HighlightColor>('magenta');
  const [regex, setRegex] = useState(false);
  const valid = pattern.length > 0 && customRegex(pattern, regex, false) !== null;

  const add = () => {
    if (!valid) return;
    const rule: HighlightRule = {
      id: `rule-${Date.now().toString(36)}`,
      pattern,
      color,
      regex,
      caseSensitive: false,
    };
    set({ custom: [...highlight.custom, rule] });
    setPattern('');
  };

  const builtin: {
    key: 'errors' | 'warnings' | 'success' | 'network';
    label: string;
    colors: HighlightColor[];
    example: string;
  }[] = [
    { key: 'errors', label: 'Fehler', colors: ['red'], example: 'error, failed, denied, fatal …' },
    { key: 'warnings', label: 'Warnungen', colors: ['yellow'], example: 'warning, deprecated …' },
    { key: 'success', label: 'Erfolg', colors: ['green'], example: 'ok, active, running, done …' },
    {
      key: 'network',
      label: 'Adressen',
      colors: ['blue', 'cyan'],
      example: 'IPv4, IPv6, http(s)-Links',
    },
  ];

  return (
    <>
      <Row
        label="Schlüsselwörter hervorheben"
        description="Wie bei Termius: Wörter wie „error“ oder „active“ und IP-Adressen bekommen im Terminal eine Farbe. Vollbild-Programme wie vim oder htop bleiben unberührt."
      >
        <Toggle
          label="Schlüsselwörter hervorheben"
          checked={highlight.enabled}
          onChange={(enabled) => set({ enabled })}
        />
      </Row>
      {highlight.enabled && (
        <>
          {builtin.map((item) => (
            <Row key={item.key} label={item.label} description={item.example}>
              <span className="swatches">
                {item.colors.map((c) => (
                  <Swatch key={c} color={c} />
                ))}
              </span>
              <Toggle
                label={item.label}
                checked={highlight[item.key]}
                onChange={(on) => set({ [item.key]: on })}
              />
            </Row>
          ))}

          <div className="highlight-custom">
            <p className="setting-label">Eigene Regeln</p>
            {highlight.custom.length === 0 && (
              <p className="setting-description">
                Noch keine. Zum Beispiel den Namen deiner Server, „prod“ oder eine Regex für
                Ticketnummern.
              </p>
            )}
            <ul>
              {highlight.custom.map((rule) => (
                <li key={rule.id}>
                  <Swatch color={rule.color} />
                  <code style={{ color: HIGHLIGHT_HEX[rule.color] }}>{rule.pattern}</code>
                  {rule.regex && <span className="rule-tag">Regex</span>}
                  <span className="spacer" />
                  <button
                    className="icon-button"
                    onClick={() =>
                      set({ custom: highlight.custom.filter((r) => r.id !== rule.id) })
                    }
                    aria-label={`Regel ${rule.pattern} löschen`}
                  >
                    <Icon name="trash" size={15} />
                  </button>
                </li>
              ))}
            </ul>
            <form
              className="highlight-add"
              onSubmit={(event) => {
                event.preventDefault();
                add();
              }}
            >
              <input
                className="search"
                value={pattern}
                placeholder={regex ? 'Regex, z. B. INC-\\d+' : 'Wort, z. B. prod'}
                spellCheck={false}
                aria-invalid={pattern.length > 0 && !valid}
                onChange={(e) => setPattern(e.target.value)}
                aria-label="Muster"
              />
              <select
                className="select"
                value={color}
                onChange={(e) => setColor(e.target.value as HighlightColor)}
                aria-label="Farbe"
              >
                {HIGHLIGHT_COLORS.map((c) => (
                  <option key={c} value={c}>
                    {COLOR_NAMES[c]}
                  </option>
                ))}
              </select>
              <label className="check inline">
                <input
                  type="checkbox"
                  checked={regex}
                  onChange={(e) => setRegex(e.target.checked)}
                />
                <span>Regex</span>
              </label>
              <button type="submit" className="primary" disabled={!valid}>
                Hinzufügen
              </button>
            </form>
          </div>

          <pre className="highlight-preview" aria-label="Vorschau">
            <span>systemctl status nginx</span>
            {'\n'}● nginx.service – <span style={{ color: HIGHLIGHT_HEX.green }}>active</span> (
            <span style={{ color: HIGHLIGHT_HEX.green }}>running</span>) on{' '}
            <span style={{ color: HIGHLIGHT_HEX.blue }}>10.0.0.12:443</span>
            {'\n'}
            <span style={{ color: HIGHLIGHT_HEX.yellow }}>warning</span>: certificate expires soon
            {'\n'}
            <span style={{ color: HIGHLIGHT_HEX.red }}>error</span>: upstream{' '}
            <span style={{ color: HIGHLIGHT_HEX.red }}>timed out</span>
          </pre>
        </>
      )}
    </>
  );
}

function Vault({ onChanged }: { onChanged: () => void }) {
  const [state, setState] = useState<VaultState | null>(null);
  const [keys, setKeys] = useState<KeyRecord[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [dialog, setDialog] = useState<
    | { kind: 'vault' }
    | { kind: 'keygen' }
    | { kind: 'import' }
    | { kind: 'export'; key: KeyRecord; format: PrivateFormat; passphrase: string }
    | { kind: 'rename'; key: KeyRecord; label: string }
    | null
  >(null);
  const [copied, setCopied] = useState<string | null>(null);

  const load = () => {
    void vaultState()
      .then(setState)
      .catch((e) => setError(String(e)));
    void listKeys()
      .then(setKeys)
      .catch((e) => setError(String(e)));
  };

  useEffect(load, []);

  const guard = async (action: () => Promise<unknown>) => {
    setError(null);
    try {
      await action();
    } catch (e) {
      const failure = asKeyFailure(e);
      if (failure.kind === 'vault-locked') setDialog({ kind: 'vault' });
      else if (failure.kind === 'in-use')
        setError(`Dieser Key wird noch von ${failure.hosts} Host(s) benutzt.`);
      else setError(failure.kind === 'error' ? failure.message : `Fehler (${failure.kind})`);
    }
    load();
    onChanged();
  };

  const status = state?.status;
  const text: Record<VaultState['status'], string> = {
    absent: 'Noch kein Tresor. Er entsteht, sobald du ein Passwort oder einen Key speicherst.',
    locked: 'Gesperrt. UwUSSH fragt nach dem Master-Passwort, sobald etwas daraus gebraucht wird.',
    unlocked: 'Entsperrt. Gespeicherte Passwörter und Keys können benutzt werden.',
  };

  return (
    <>
      <Row
        label="Status"
        description="Passwörter und Keys liegen verschlüsselt im Tresor, mit Argon2id und XChaCha20-Poly1305. Das Master-Passwort verlässt dieses Gerät nie."
      >
        <span className="vault-status" data-status={status ?? 'loading'}>
          {status ? text[status] : error ? error : 'Wird geprüft …'}
        </span>
        {status !== 'unlocked' && (
          <button className="primary" onClick={() => setDialog({ kind: 'vault' })}>
            {status === 'absent' ? 'Anlegen' : 'Entsperren'}
          </button>
        )}
      </Row>
      {status !== 'absent' && (
        <Row
          label="Auf diesem Gerät merken"
          description="Windows öffnet den Tresor beim Start für dein Benutzerkonto, ohne Master-Passwort. Andere Konten und andere Rechner brauchen es weiter."
        >
          <Toggle
            label="Auf diesem Gerät merken"
            checked={Boolean(state?.remembered)}
            onChange={(remember) =>
              void guard(async () => {
                if (remember && status !== 'unlocked') {
                  setDialog({ kind: 'vault' });
                  return;
                }
                await setVaultRemembered(remember);
              })
            }
          />
        </Row>
      )}
      {status === 'unlocked' && (
        <Row
          label="Tresor sperren"
          description={
            state?.remembered
              ? 'Bis zum nächsten Start. Offene Verbindungen bleiben bestehen.'
              : 'Offene Verbindungen bleiben bestehen; neue fragen wieder nach dem Master-Passwort.'
          }
        >
          <button onClick={() => void guard(lockVault)}>Jetzt sperren</button>
        </Row>
      )}

      <div className="key-list">
        <div className="key-list-head">
          <p className="setting-label">SSH-Keys im Tresor</p>
          <span className="spacer" />
          <button onClick={() => setDialog({ kind: 'import' })}>
            <Icon name="import" size={15} />
            Importieren…
          </button>
          <button className="primary" onClick={() => setDialog({ kind: 'keygen' })}>
            <Icon name="sparkles" size={15} />
            Erzeugen…
          </button>
        </div>
        {keys.length === 0 ? (
          <p className="setting-description">
            Noch keine Keys. Erzeuge einen mit UwUKeygen oder importiere eine Key-Datei (OpenSSH,
            PEM oder PuTTY).
          </p>
        ) : (
          <ul>
            {keys.map((key) => (
              <li key={key.id}>
                <Icon name="key" size={16} />
                <span className="key-list-text">
                  <b>{key.label}</b>
                  <small>
                    {key.keyType.replace(/^ssh-|^ecdsa-sha2-/, '')}
                    {key.hasPassphrase ? ' · mit Passphrase' : ''}
                    {key.hosts > 0 ? ` · ${key.hosts} Host${key.hosts === 1 ? '' : 's'}` : ''}
                  </small>
                </span>
                <span className="spacer" />
                <button
                  className="icon-button"
                  title="Public Key kopieren"
                  aria-label={`Public Key von ${key.label} kopieren`}
                  onClick={() =>
                    void guard(async () => {
                      await navigator.clipboard.writeText(await keyPublicLine(key.id));
                      setCopied(key.id);
                      window.setTimeout(() => setCopied(null), 1600);
                    })
                  }
                >
                  <Icon name={copied === key.id ? 'check' : 'copy'} size={15} />
                </button>
                <button
                  className="icon-button"
                  title="Als Datei exportieren"
                  aria-label={`${key.label} exportieren`}
                  onClick={() =>
                    setDialog({ kind: 'export', key, format: 'openssh', passphrase: '' })
                  }
                >
                  <Icon name="export" size={15} />
                </button>
                <button
                  className="icon-button"
                  title="Umbenennen"
                  aria-label={`${key.label} umbenennen`}
                  onClick={() => setDialog({ kind: 'rename', key, label: key.label })}
                >
                  <Icon name="pencil" size={15} />
                </button>
                <button
                  className="icon-button"
                  title={key.hosts > 0 ? 'Wird noch benutzt' : 'Löschen'}
                  aria-label={`${key.label} löschen`}
                  disabled={key.hosts > 0}
                  onClick={() => void guard(() => deleteKey(key.id))}
                >
                  <Icon name="trash" size={15} />
                </button>
              </li>
            ))}
          </ul>
        )}
        {error && (
          <p className="setting-result" data-tone="error">
            {error}
          </p>
        )}
      </div>

      {dialog?.kind === 'vault' && (
        <VaultDialog
          onDone={() => {
            setDialog(null);
            load();
          }}
          onCancel={() => setDialog(null)}
        />
      )}
      {dialog?.kind === 'keygen' && (
        <KeygenDialog onStored={() => load()} onClose={() => setDialog(null)} />
      )}
      {dialog?.kind === 'import' && (
        <KeyImportDialog onImported={() => load()} onClose={() => setDialog(null)} />
      )}
      {dialog?.kind === 'rename' && (
        <Modal
          title="Key umbenennen"
          onCancel={() => setDialog(null)}
          footer={
            <>
              <span className="spacer" />
              <button data-secondary onClick={() => setDialog(null)}>
                Abbrechen
              </button>
              <button
                className="primary"
                onClick={() => {
                  const { key, label } = dialog;
                  setDialog(null);
                  void guard(() => renameKey(key.id, label));
                }}
              >
                Speichern
              </button>
            </>
          }
        >
          <label className="field">
            <span>Name</span>
            <input
              data-autofocus
              value={dialog.label}
              onChange={(e) => setDialog({ ...dialog, label: e.target.value })}
            />
          </label>
        </Modal>
      )}
      {dialog?.kind === 'export' && (
        <Modal
          title={`${dialog.key.label} exportieren`}
          onCancel={() => setDialog(null)}
          footer={
            <>
              <span className="spacer" />
              <button data-secondary onClick={() => setDialog(null)}>
                Abbrechen
              </button>
              <button
                className="primary"
                onClick={() => {
                  const { key, format, passphrase } = dialog;
                  setDialog(null);
                  void guard(() => exportKeyFile(key.id, format, passphrase || null));
                }}
              >
                Speichern unter…
              </button>
            </>
          }
        >
          <label className="field">
            <span>Format</span>
            <select
              className="select"
              value={dialog.format}
              onChange={(e) => setDialog({ ...dialog, format: e.target.value as PrivateFormat })}
            >
              {(Object.keys(FORMAT_LABELS) as PrivateFormat[]).map((value) => (
                <option key={value} value={value}>
                  {FORMAT_LABELS[value]}
                </option>
              ))}
            </select>
          </label>
          <label className="field">
            <span>Passphrase für die Datei (optional)</span>
            <input
              type="password"
              value={dialog.passphrase}
              autoComplete="new-password"
              onChange={(e) => setDialog({ ...dialog, passphrase: e.target.value })}
            />
            <em className="field-hint">
              {dialog.passphrase
                ? passphraseNote(dialog.format, true)
                : 'Ohne Passphrase liegt der private Key unverschlüsselt in der Datei.'}
            </em>
          </label>
        </Modal>
      )}
    </>
  );
}

function Data({ onImport }: { onImport: () => void }) {
  const [exporting, setExporting] = useState(false);
  return (
    <>
      <Row
        label="Exportieren"
        description="Alle Hosts mit Bereichen, Gruppen und Host-Keys in eine .uwussh-Datei – auf Wunsch mit Passwörtern und Keys, dann mit eigenem Passwort verschlüsselt."
      >
        <button onClick={() => setExporting(true)}>
          <Icon name="export" size={15} />
          Exportieren…
        </button>
      </Row>
      <Row
        label="Importieren"
        description="Aus einer .uwussh-Datei, aus Termius, PuTTY, KiTTY oder ~/.ssh/config. Schon vorhandene Hosts werden übersprungen."
      >
        <button onClick={onImport}>
          <Icon name="import" size={15} />
          Importieren…
        </button>
      </Row>
      {exporting && <ExportDialog onClose={() => setExporting(false)} />}
    </>
  );
}

function Updates({
  update,
  onUpdateFound,
  onInstallUpdate,
}: Pick<Props, 'update' | 'onUpdateFound' | 'onInstallUpdate'>) {
  const settings = useSettings();
  const [checking, setChecking] = useState(false);
  const [result, setResult] = useState<{ tone: 'info' | 'error'; text: string } | null>(null);

  return (
    <>
      <Row
        label="Update-Kanal"
        description={
          settings.updateChannel === 'beta'
            ? 'Beta bekommt neue Versionen früher. Es kann mal etwas wackeln.'
            : 'Stabil bekommt nur fertige Versionen.'
        }
      >
        <Segmented
          label="Update-Kanal"
          value={settings.updateChannel}
          onChange={(updateChannel) => {
            setResult(null);
            updateSettings({ updateChannel });
          }}
          options={[
            { value: 'stable', label: 'Stabil' },
            { value: 'beta', label: 'Beta' },
          ]}
        />
      </Row>
      <Row
        label={`Version ${pkg.version}`}
        description="UwUSSH lädt neue Versionen still herunter und installiert sie beim nächsten Start. Jedes Update ist signiert und wird vor dem Start geprüft."
      >
        {update ? (
          <button className="primary" onClick={onInstallUpdate}>
            {update.version} installieren
          </button>
        ) : (
          <button
            disabled={checking}
            onClick={async () => {
              setChecking(true);
              setResult(null);
              try {
                const found = await checkForUpdates();
                if (found) onUpdateFound(found);
                else setResult({ tone: 'info', text: 'UwUSSH ist auf dem neuesten Stand. ✧' });
              } catch (e) {
                setResult({ tone: 'error', text: `Suche fehlgeschlagen: ${String(e)}` });
              } finally {
                setChecking(false);
              }
            }}
          >
            {checking ? 'Sucht …' : 'Nach Updates suchen'}
          </button>
        )}
      </Row>
      {result && (
        <p className="setting-result" data-tone={result.tone} role="status">
          {result.text}
        </p>
      )}
    </>
  );
}

function About({ onRunM0 }: { onRunM0: () => void }) {
  const open = (page: ProjectPage) => void openProjectPage(page).catch(() => undefined);
  return (
    <div className="about">
      <Nyu size={88} mood="happy" title="Nyu" />
      <p className="about-name">
        <span>UwU</span>SSH
      </p>
      <p className="about-version">Version {pkg.version}</p>
      <p className="about-text">
        Freie Software unter der GNU GPL v3.0. Nutzen, ändern, weitergeben – nur geänderte Versionen
        müssen offen bleiben. Kein Tracking, kein Konto.
      </p>
      <div className="about-actions">
        <button onClick={() => open('source')}>Quellcode auf GitHub</button>
        <button onClick={() => open('releases')}>Versionen</button>
        <button onClick={() => open('license')}>Lizenz</button>
      </div>
      <details className="about-diagnostics">
        <summary>Diagnose</summary>
        <p className="setting-description">
          Misst in einem eigenen Tab, wie schnell das Terminal Ausgabe verarbeitet (Meilenstein M0).
        </p>
        <button onClick={onRunM0}>Durchsatz messen</button>
      </details>
    </div>
  );
}

export function SettingsDialog({
  initial = 'appearance',
  onClose,
  update,
  onUpdateFound,
  onInstallUpdate,
  onRunM0,
  onImport,
  onChanged,
}: Props) {
  const [section, setSection] = useState<SettingsSection>(initial);
  return (
    <Modal title="Einstellungen" size="wide" onCancel={onClose}>
      <div className="settings">
        <nav className="settings-nav" aria-label="Bereiche">
          {SECTIONS.map(({ id, label }) => (
            <button
              key={id}
              type="button"
              aria-current={section === id ? 'page' : undefined}
              onClick={() => setSection(id)}
            >
              {label}
            </button>
          ))}
        </nav>
        <div className="settings-content">
          {section === 'appearance' && <Appearance />}
          {section === 'terminal' && <TerminalSettings />}
          {section === 'highlight' && <Highlighting />}
          {section === 'vault' && <Vault onChanged={onChanged} />}
          {section === 'data' && <Data onImport={onImport} />}
          {section === 'updates' && (
            <Updates
              update={update}
              onUpdateFound={onUpdateFound}
              onInstallUpdate={onInstallUpdate}
            />
          )}
          {section === 'about' && <About onRunM0={onRunM0} />}
        </div>
      </div>
      <button className="settings-close icon-button" onClick={onClose} aria-label="Schließen">
        ×
      </button>
    </Modal>
  );
}
