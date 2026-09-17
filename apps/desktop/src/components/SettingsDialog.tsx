import { useEffect, useState, type ReactNode } from 'react';
import pkg from '../../package.json';
import {
  checkForUpdates,
  lockVault,
  openProjectPage,
  vaultStatus,
  type ProjectPage,
  type UpdateInfo,
  type VaultStatus,
} from '../lib/session';
import {
  FONT_SIZES,
  SCROLLBACK_CHOICES,
  updateSettings,
  useSettings,
  type CursorStyle,
} from '../lib/settings';
import { Modal } from './Modal';
import { Nyu } from './nyu/Nyu';

export type SettingsSection = 'appearance' | 'terminal' | 'vault' | 'updates' | 'about';

const SECTIONS: { id: SettingsSection; label: string }[] = [
  { id: 'appearance', label: 'Darstellung' },
  { id: 'terminal', label: 'Terminal' },
  { id: 'vault', label: 'Tresor' },
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
    </>
  );
}

function TerminalSettings() {
  const settings = useSettings();
  return (
    <>
      <Row label="Schriftgröße">
        <select
          className="select"
          aria-label="Schriftgröße"
          value={settings.fontSize}
          onChange={(event) => updateSettings({ fontSize: Number(event.target.value) })}
        >
          {FONT_SIZES.map((size) => (
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
          <dt>Strg+Umschalt+C · Strg+Umschalt+V</dt>
          <dd>Kopieren · Einfügen</dd>
          <dt>Strg+,</dt>
          <dd>Einstellungen</dd>
        </dl>
      </div>
    </>
  );
}

function Vault() {
  const [status, setStatus] = useState<VaultStatus | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void vaultStatus()
      .then(setStatus)
      .catch((e) => setError(String(e)));
  }, []);

  const text: Record<VaultStatus, string> = {
    absent: 'Noch kein Tresor. Er entsteht, sobald ein Import Passwörter oder Schlüssel mitbringt.',
    locked: 'Gesperrt. UwUSSH fragt nach dem Master-Passwort, sobald ein Host es braucht.',
    unlocked: 'Entsperrt. Gespeicherte Passwörter und Schlüssel können benutzt werden.',
  };

  return (
    <>
      <Row
        label="Status"
        description="Passwörter und Schlüssel liegen verschlüsselt im Tresor, mit Argon2id und XChaCha20-Poly1305. Das Master-Passwort verlässt dieses Gerät nie."
      >
        <span className="vault-status" data-status={status ?? 'loading'}>
          {status ? text[status] : error ? error : 'Wird geprüft …'}
        </span>
      </Row>
      {status === 'unlocked' && (
        <Row
          label="Tresor sperren"
          description="Offene Verbindungen bleiben bestehen; neue fragen wieder nach dem Master-Passwort."
        >
          <button
            onClick={() => {
              void lockVault()
                .then(vaultStatus)
                .then(setStatus)
                .catch((e) => setError(String(e)));
            }}
          >
            Jetzt sperren
          </button>
        </Row>
      )}
    </>
  );
}

function Updates({ update, onUpdateFound, onInstallUpdate }: Omit<Props, 'onClose' | 'onRunM0'>) {
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
          {section === 'vault' && <Vault />}
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
