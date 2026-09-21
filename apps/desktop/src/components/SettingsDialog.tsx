import { useEffect, useState, type ReactNode } from 'react';
import pkg from '../../package.json';
import { customRegex, HIGHLIGHT_HEX } from '../lib/highlight';
import { locale, N_, t, useLanguage } from '../lib/i18n';
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
  listHosts,
  lockVault,
  openProjectPage,
  setVaultRemembered,
  vaultState,
  type HostRecord,
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
  type StartupSetting,
} from '../lib/settings';
import { systemName } from '../lib/platform';
import { ExportDialog } from './ExportDialog';
import { SyncSettings } from './SyncSettings';
import { Icon } from './Icon';
import { KeyImportDialog, KeygenDialog } from './keygen/KeyDialogs';
import { Modal } from './Modal';
import { Nyu } from './nyu/Nyu';
import { VaultDialog } from './VaultDialog';

export type SettingsSection =
  'appearance' | 'terminal' | 'highlight' | 'vault' | 'sync' | 'data' | 'updates' | 'about';

const SECTIONS: { id: SettingsSection; label: string }[] = [
  { id: 'appearance', label: N_('Darstellung') },
  { id: 'terminal', label: N_('Terminal') },
  { id: 'highlight', label: N_('Hervorhebung') },
  { id: 'vault', label: N_('Tresor & Keys') },
  { id: 'sync', label: N_('Sync') },
  { id: 'data', label: N_('Import & Export') },
  { id: 'updates', label: N_('Updates') },
  { id: 'about', label: N_('Über UwUSSH') },
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
      <Row
        label="Sprache · Language"
        description={`„System“ folgt der Sprache von ${systemName()}. · “System” follows ${systemName()}.`}
      >
        <Segmented
          label="Sprache · Language"
          value={settings.language}
          onChange={(language) => updateSettings({ language })}
          options={[
            { value: 'system', label: 'System' },
            { value: 'de', label: 'Deutsch' },
            { value: 'en', label: 'English' },
          ]}
        />
      </Row>
      <Row label={t('Farbschema')} description={t('Das Terminal bleibt in jedem Schema dunkel.')}>
        <Segmented
          label={t('Farbschema')}
          value={settings.theme}
          onChange={(theme) => updateSettings({ theme })}
          options={[
            { value: 'system', label: t('System') },
            { value: 'light', label: t('Hell') },
            { value: 'dark', label: t('Dunkel') },
          ]}
        />
      </Row>
      <Row
        label={t('Animationen')}
        description={t('„System“ folgt der Einstellung von {system}.', { system: systemName() })}
      >
        <Segmented
          label={t('Animationen')}
          value={settings.motion}
          onChange={(motion) => updateSettings({ motion })}
          options={[
            { value: 'system', label: t('System') },
            { value: 'on', label: t('An') },
            { value: 'off', label: t('Aus') },
          ]}
        />
      </Row>
      <Row
        label={t('Privat und Business')}
        description={t(
          'Zwei Bereiche in der Hostliste, wie bei UwUMail. Hosts und Gruppen ziehst du einfach in den anderen Bereich.',
        )}
      >
        <Toggle
          label={t('Privat und Business')}
          checked={settings.workspaces}
          onChange={(workspaces) => updateSettings({ workspaces })}
        />
      </Row>
      {settings.workspaces && (
        <Row
          label={t('Namen der Bereiche')}
          description={t('Leer lassen für „Privat“ und „Business“.')}
        >
          <div className="workspace-names">
            {(['private', 'business'] as const).map((id) => (
              <input
                key={id}
                className="search"
                value={settings.workspaceNames[id]}
                placeholder={id === 'private' ? t('Privat') : t('Business')}
                maxLength={24}
                aria-label={t('Name für {name}', { name: workspaceName(id, settings) })}
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
      <Row label={t('Schriftgröße')} description={t('Oder Strg + Mausrad über dem Terminal.')}>
        <select
          className="select"
          aria-label={t('Schriftgröße')}
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
      <Row label={t('Cursor')}>
        <Segmented<CursorStyle>
          label={t('Cursorform')}
          value={settings.cursorStyle}
          onChange={(cursorStyle) => updateSettings({ cursorStyle })}
          options={[
            { value: 'block', label: t('Block') },
            { value: 'bar', label: t('Strich') },
            { value: 'underline', label: t('Unterstrich') },
          ]}
        />
      </Row>
      <Row label={t('Cursor blinkt')}>
        <Toggle
          label={t('Cursor blinkt')}
          checked={settings.cursorBlink}
          onChange={(cursorBlink) => updateSettings({ cursorBlink })}
        />
      </Row>
      <Row
        label={t('Scrollback')}
        description={t('So viele Zeilen hält jeder Tab zum Zurückscrollen.')}
      >
        <Segmented
          label={t('Scrollback')}
          value={settings.scrollback}
          onChange={(scrollback) => updateSettings({ scrollback })}
          options={SCROLLBACK_CHOICES.map((lines) => ({
            value: lines,
            label: lines.toLocaleString(locale()),
          }))}
        />
      </Row>
      <Row
        label={t('Passwort-Helfer')}
        description={t(
          'Fragt sudo oder su im Terminal nach dem Passwort, bietet UwUSSH an, das Passwort des Hosts einzutippen (Strg+Umschalt+P).',
        )}
      >
        <Toggle
          label={t('Passwort-Helfer')}
          checked={settings.passwordHelper}
          onChange={(passwordHelper) => updateSettings({ passwordHelper })}
        />
      </Row>
      <Row
        label={t('Strg+C kopiert markierten Text')}
        description={t('Ohne Markierung geht Strg+C wie immer als Abbruch an das Programm.')}
      >
        <Toggle
          label={t('Strg+C kopiert markierten Text')}
          checked={settings.ctrlCCopies}
          onChange={(ctrlCCopies) => updateSettings({ ctrlCCopies })}
        />
      </Row>
      <Row
        label={t('Strg+V fügt ein')}
        description={t('Aus: Strg+V geht als ^V an das Programm. Strg+Umschalt+V fügt immer ein.')}
      >
        <Toggle
          label={t('Strg+V fügt ein')}
          checked={settings.ctrlVPastes}
          onChange={(ctrlVPastes) => updateSettings({ ctrlVPastes })}
        />
      </Row>
      <StartupRow />
      <Row
        label={t('Vor dem Schließen nachfragen')}
        description={t('Wenn noch SSH-Verbindungen offen sind.')}
      >
        <Toggle
          label={t('Vor dem Schließen nachfragen')}
          checked={settings.confirmCloseWithSessions}
          onChange={(confirmCloseWithSessions) => updateSettings({ confirmCloseWithSessions })}
        />
      </Row>
      <div className="shortcuts">
        <p className="setting-label">{t('Tastenkürzel')}</p>
        <dl>
          <dt>{t('Strg+Umschalt+T')}</dt>
          <dd>{t('Neue lokale Shell')}</dd>
          <dt>{t('Strg+Umschalt+D')}</dt>
          <dd>{t('Tab duplizieren (neue Verbindung zum selben Host)')}</dd>
          <dt>{t('Strg+Umschalt+W')}</dt>
          <dd>{t('Tab schließen')}</dd>
          <dt>{t('Strg+Tab · Strg+Umschalt+Tab')}</dt>
          <dd>{t('Nächster · vorheriger Tab')}</dd>
          <dt>{t('Strg+Umschalt+1 … 9')}</dt>
          <dd>{t('Zu Tab 1 … 9')}</dd>
          <dt>{t('Strg+Umschalt+F')}</dt>
          <dd>{t('Dateien des Hosts öffnen')}</dd>
          <dt>{t('Strg+Umschalt+P')}</dt>
          <dd>{t('Passwort eintippen, wenn danach gefragt wird')}</dd>
          <dt>{t('Strg+Umschalt+C · Strg+Umschalt+V')}</dt>
          <dd>{t('Kopieren · Einfügen')}</dd>
          <dt>{t('Strg+Mausrad')}</dt>
          <dd>{t('Schrift größer · kleiner')}</dd>
          <dt>{t('Strg+,')}</dt>
          <dd>{t('Einstellungen')}</dd>
        </dl>
      </div>
    </>
  );
}

/** What opens when UwUSSH starts: nothing (the default), a local shell, or chosen hosts. */
function StartupRow() {
  const settings = useSettings();
  const [hosts, setHosts] = useState<HostRecord[] | null>(null);
  useEffect(() => {
    if (settings.startup !== 'hosts' || hosts) return;
    void listHosts()
      .then(setHosts)
      .catch(() => setHosts([]));
  }, [settings.startup, hosts]);
  const chosen = new Set(settings.startupHosts);
  const toggle = (id: string, on: boolean) =>
    updateSettings({
      startupHosts: on
        ? [...settings.startupHosts.filter((other) => other !== id), id]
        : settings.startupHosts.filter((other) => other !== id),
    });
  return (
    <>
      <Row
        label={t('Beim Start öffnen')}
        description={t(
          'Normalerweise öffnet UwUSSH keine Verbindung von selbst. Hier kannst du eine lokale Shell oder bestimmte Hosts vorgeben.',
        )}
      >
        <Segmented<StartupSetting>
          label={t('Beim Start öffnen')}
          value={settings.startup}
          onChange={(startup) => updateSettings({ startup })}
          options={[
            { value: 'nothing', label: t('Nichts') },
            { value: 'shell', label: t('Lokale Shell') },
            { value: 'hosts', label: t('Hosts') },
          ]}
        />
      </Row>
      {settings.startup === 'hosts' && (
        <div className="startup-hosts" role="group" aria-label={t('Hosts beim Start')}>
          {hosts === null ? (
            <p className="setting-description">{t('Hosts werden geladen…')}</p>
          ) : hosts.length === 0 ? (
            <p className="setting-description">{t('Noch keine Hosts angelegt.')}</p>
          ) : (
            [...hosts]
              .sort((a, b) => a.name.localeCompare(b.name, 'de'))
              .map((host) => (
                <label key={host.id} className="check inline">
                  <input
                    type="checkbox"
                    checked={chosen.has(host.id)}
                    onChange={(event) => toggle(host.id, event.target.checked)}
                  />
                  <span>
                    {host.name}
                    <small>
                      {host.username}@{host.address}
                    </small>
                  </span>
                </label>
              ))
          )}
          {hosts !== null && hosts.length > 0 && chosen.size === 0 && (
            <p className="setting-description">
              {t('Keiner gewählt – dann öffnet beim Start nichts.')}
            </p>
          )}
        </div>
      )}
    </>
  );
}

const COLOR_NAMES: Record<HighlightColor, string> = {
  red: N_('Rot'),
  yellow: N_('Gelb'),
  green: N_('Grün'),
  blue: N_('Blau'),
  magenta: N_('Pink'),
  cyan: N_('Türkis'),
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
    {
      key: 'errors',
      label: t('Fehler'),
      colors: ['red'],
      example: 'error, failed, denied, fatal …',
    },
    {
      key: 'warnings',
      label: t('Warnungen'),
      colors: ['yellow'],
      example: 'warning, deprecated …',
    },
    {
      key: 'success',
      label: t('Erfolg'),
      colors: ['green'],
      example: 'ok, active, running, done …',
    },
    {
      key: 'network',
      label: t('Adressen'),
      colors: ['blue', 'cyan'],
      example: t('IPv4, IPv6, http(s)-Links'),
    },
  ];

  return (
    <>
      <Row
        label={t('Schlüsselwörter hervorheben')}
        description={t(
          'Wie bei Termius: Wörter wie „error“ oder „active“ und IP-Adressen bekommen im Terminal eine Farbe. Vollbild-Programme wie vim oder htop bleiben unberührt.',
        )}
      >
        <Toggle
          label={t('Schlüsselwörter hervorheben')}
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
            <p className="setting-label">{t('Eigene Regeln')}</p>
            {highlight.custom.length === 0 && (
              <p className="setting-description">
                {t(
                  'Noch keine. Zum Beispiel den Namen deiner Server, „prod“ oder eine Regex für Ticketnummern.',
                )}
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
                    aria-label={t('Regel {pattern} löschen', { pattern: rule.pattern })}
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
                placeholder={regex ? t('Regex, z. B. INC-\\d+') : t('Wort, z. B. prod')}
                spellCheck={false}
                aria-invalid={pattern.length > 0 && !valid}
                onChange={(e) => setPattern(e.target.value)}
                aria-label={t('Muster')}
              />
              <select
                className="select"
                value={color}
                onChange={(e) => setColor(e.target.value as HighlightColor)}
                aria-label={t('Farbe')}
              >
                {HIGHLIGHT_COLORS.map((c) => (
                  <option key={c} value={c}>
                    {t(COLOR_NAMES[c])}
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
                {t('Hinzufügen')}
              </button>
            </form>
          </div>

          <pre className="highlight-preview" aria-label={t('Vorschau')}>
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
  useLanguage();
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
        setError(t('Dieser Key wird noch von {hosts} Host(s) benutzt.', { hosts: failure.hosts }));
      else
        setError(
          failure.kind === 'error' ? failure.message : t('Fehler ({kind})', { kind: failure.kind }),
        );
    }
    load();
    onChanged();
  };

  const status = state?.status;
  const text: Record<VaultState['status'], string> = {
    absent: t('Noch kein Tresor. Er entsteht, sobald du ein Passwort oder einen Key speicherst.'),
    locked: t(
      'Gesperrt. UwUSSH fragt nach dem Master-Passwort, sobald etwas daraus gebraucht wird.',
    ),
    unlocked: t('Entsperrt. Gespeicherte Passwörter und Keys können benutzt werden.'),
  };

  return (
    <>
      <Row
        label={t('Status')}
        description={t(
          'Passwörter und Keys liegen verschlüsselt im Tresor, mit Argon2id und XChaCha20-Poly1305. Das Master-Passwort verlässt dieses Gerät nie.',
        )}
      >
        <span className="vault-status" data-status={status ?? 'loading'}>
          {status ? text[status] : error ? error : t('Wird geprüft …')}
        </span>
        {status !== 'unlocked' && (
          <button className="primary" onClick={() => setDialog({ kind: 'vault' })}>
            {status === 'absent' ? t('Anlegen') : t('Entsperren')}
          </button>
        )}
      </Row>
      {status !== 'absent' && (
        <Row
          label={t('Auf diesem Gerät merken')}
          description={t(
            '{system} öffnet den Tresor beim Start für dein Benutzerkonto, ohne Master-Passwort. Andere Konten und andere Rechner brauchen es weiter.',
            { system: systemName() },
          )}
        >
          <Toggle
            label={t('Auf diesem Gerät merken')}
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
          label={t('Tresor sperren')}
          description={
            state?.remembered
              ? t('Bis zum nächsten Start. Offene Verbindungen bleiben bestehen.')
              : t(
                  'Offene Verbindungen bleiben bestehen; neue fragen wieder nach dem Master-Passwort.',
                )
          }
        >
          <button onClick={() => void guard(lockVault)}>{t('Jetzt sperren')}</button>
        </Row>
      )}

      <div className="key-list">
        <div className="key-list-head">
          <p className="setting-label">{t('SSH-Keys im Tresor')}</p>
          <span className="spacer" />
          <button onClick={() => setDialog({ kind: 'import' })}>
            <Icon name="import" size={15} />
            {t('Importieren…')}
          </button>
          <button className="primary" onClick={() => setDialog({ kind: 'keygen' })}>
            <Icon name="sparkles" size={15} />
            {t('Erzeugen…')}
          </button>
        </div>
        {keys.length === 0 ? (
          <p className="setting-description">
            {t(
              'Noch keine Keys. Erzeuge einen mit UwUKeygen oder importiere eine Key-Datei (OpenSSH, PEM oder PuTTY).',
            )}
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
                    {key.hasPassphrase ? ` · ${t('mit Passphrase')}` : ''}
                    {key.hosts > 0
                      ? ` · ${key.hosts === 1 ? t('1 Host') : t('{count} Hosts', { count: key.hosts })}`
                      : ''}
                  </small>
                </span>
                <span className="spacer" />
                <button
                  className="icon-button"
                  title={t('Public Key kopieren')}
                  aria-label={t('Public Key von {label} kopieren', { label: key.label })}
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
                  title={t('Als Datei exportieren')}
                  aria-label={t('{label} exportieren', { label: key.label })}
                  onClick={() =>
                    setDialog({ kind: 'export', key, format: 'openssh', passphrase: '' })
                  }
                >
                  <Icon name="export" size={15} />
                </button>
                <button
                  className="icon-button"
                  title={t('Umbenennen')}
                  aria-label={t('{label} umbenennen', { label: key.label })}
                  onClick={() => setDialog({ kind: 'rename', key, label: key.label })}
                >
                  <Icon name="pencil" size={15} />
                </button>
                <button
                  className="icon-button"
                  title={key.hosts > 0 ? t('Wird noch benutzt') : t('Löschen')}
                  aria-label={t('{label} löschen', { label: key.label })}
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
          title={t('Key umbenennen')}
          onCancel={() => setDialog(null)}
          footer={
            <>
              <span className="spacer" />
              <button data-secondary onClick={() => setDialog(null)}>
                {t('Abbrechen')}
              </button>
              <button
                className="primary"
                onClick={() => {
                  const { key, label } = dialog;
                  setDialog(null);
                  void guard(() => renameKey(key.id, label));
                }}
              >
                {t('Speichern')}
              </button>
            </>
          }
        >
          <label className="field">
            <span>{t('Name')}</span>
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
          title={t('{label} exportieren', { label: dialog.key.label })}
          onCancel={() => setDialog(null)}
          footer={
            <>
              <span className="spacer" />
              <button data-secondary onClick={() => setDialog(null)}>
                {t('Abbrechen')}
              </button>
              <button
                className="primary"
                onClick={() => {
                  const { key, format, passphrase } = dialog;
                  setDialog(null);
                  void guard(() => exportKeyFile(key.id, format, passphrase || null));
                }}
              >
                {t('Speichern unter…')}
              </button>
            </>
          }
        >
          <label className="field">
            <span>{t('Format')}</span>
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
            <span>{t('Passphrase für die Datei (optional)')}</span>
            <input
              type="password"
              value={dialog.passphrase}
              autoComplete="new-password"
              onChange={(e) => setDialog({ ...dialog, passphrase: e.target.value })}
            />
            <em className="field-hint">
              {dialog.passphrase
                ? passphraseNote(dialog.format, true)
                : t('Ohne Passphrase liegt der private Key unverschlüsselt in der Datei.')}
            </em>
          </label>
        </Modal>
      )}
    </>
  );
}

function Data({ onImport }: { onImport: () => void }) {
  useLanguage();
  const [exporting, setExporting] = useState(false);
  return (
    <>
      <Row
        label={t('Exportieren')}
        description={t(
          'Alle Hosts mit Bereichen, Gruppen und Host-Keys in eine .uwussh-Datei – auf Wunsch mit Passwörtern und Keys, dann mit eigenem Passwort verschlüsselt.',
        )}
      >
        <button onClick={() => setExporting(true)}>
          <Icon name="export" size={15} />
          {t('Exportieren…')}
        </button>
      </Row>
      <Row
        label={t('Importieren')}
        description={t(
          'Aus einer .uwussh-Datei, aus Termius, PuTTY, KiTTY oder ~/.ssh/config. Schon vorhandene Hosts werden übersprungen.',
        )}
      >
        <button onClick={onImport}>
          <Icon name="import" size={15} />
          {t('Importieren…')}
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
        label={t('Update-Kanal')}
        description={
          settings.updateChannel === 'beta'
            ? t('Beta bekommt neue Versionen früher. Es kann mal etwas wackeln.')
            : t('Stabil bekommt nur fertige Versionen.')
        }
      >
        <Segmented
          label={t('Update-Kanal')}
          value={settings.updateChannel}
          onChange={(updateChannel) => {
            setResult(null);
            updateSettings({ updateChannel });
          }}
          options={[
            { value: 'stable', label: t('Stabil') },
            { value: 'beta', label: t('Beta') },
          ]}
        />
      </Row>
      <Row
        label={t('Version {version}', { version: pkg.version })}
        description={t(
          'UwUSSH lädt neue Versionen still herunter und installiert sie beim nächsten Start. Jedes Update ist signiert und wird vor dem Start geprüft.',
        )}
      >
        {update ? (
          <button className="primary" onClick={onInstallUpdate}>
            {t('{version} installieren', { version: update.version })}
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
                else setResult({ tone: 'info', text: t('UwUSSH ist auf dem neuesten Stand. ✧') });
              } catch (e) {
                setResult({
                  tone: 'error',
                  text: t('Suche fehlgeschlagen: {error}', { error: String(e) }),
                });
              } finally {
                setChecking(false);
              }
            }}
          >
            {checking ? t('Sucht …') : t('Nach Updates suchen')}
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
  useLanguage();
  const open = (page: ProjectPage) => void openProjectPage(page).catch(() => undefined);
  return (
    <div className="about">
      <Nyu size={88} mood="happy" title="Nyu" />
      <p className="about-name">
        <span>UwU</span>SSH
      </p>
      <p className="about-version">{t('Version {version}', { version: pkg.version })}</p>
      <p className="about-text">
        {t(
          'Freie Software unter der GNU GPL v3.0. Nutzen, ändern, weitergeben – nur geänderte Versionen müssen offen bleiben. Kein Tracking, kein Konto.',
        )}
      </p>
      <div className="about-actions">
        <button onClick={() => open('source')}>{t('Quellcode auf GitHub')}</button>
        <button onClick={() => open('releases')}>{t('Versionen')}</button>
        <button onClick={() => open('license')}>{t('Lizenz')}</button>
      </div>
      <details className="about-diagnostics">
        <summary>{t('Diagnose')}</summary>
        <p className="setting-description">
          {t(
            'Misst in einem eigenen Tab, wie schnell das Terminal Ausgabe verarbeitet (Meilenstein M0).',
          )}
        </p>
        <button onClick={onRunM0}>{t('Durchsatz messen')}</button>
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
  useLanguage();
  const [section, setSection] = useState<SettingsSection>(initial);
  return (
    <Modal title={t('Einstellungen')} size="wide" onCancel={onClose}>
      <div className="settings">
        <nav className="settings-nav" aria-label={t('Bereiche')}>
          {SECTIONS.map(({ id, label }) => (
            <button
              key={id}
              type="button"
              aria-current={section === id ? 'page' : undefined}
              onClick={() => setSection(id)}
            >
              {t(label)}
            </button>
          ))}
        </nav>
        <div className="settings-content">
          {section === 'appearance' && <Appearance />}
          {section === 'terminal' && <TerminalSettings />}
          {section === 'highlight' && <Highlighting />}
          {section === 'vault' && <Vault onChanged={onChanged} />}
          {section === 'sync' && <SyncSettings />}
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
      <button className="settings-close icon-button" onClick={onClose} aria-label={t('Schließen')}>
        ×
      </button>
    </Modal>
  );
}
