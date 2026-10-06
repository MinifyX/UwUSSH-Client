import { Button, Icon, ICONS, Segmented, Tag } from '@uwusuite/design';
import type { LucideIcon } from 'lucide-react';
import { useCallback, useEffect, useState, type ReactNode } from 'react';
import { localShellAvailable } from '../lib/flavor';
import { N_, t, useLanguage } from '../lib/i18n';
import { systemName } from '../lib/platform';
import {
  availableImports,
  listHosts,
  vaultState,
  type ImportSource,
  type VaultStatus,
} from '../lib/session';
import {
  passwordLoginWarningOn,
  setPasswordLoginWarning,
  updateSettings,
  useSettings,
  type ThemeSetting,
} from '../lib/settings';
import { keysFor } from '../lib/shortcuts';
import { syncStatus, type SyncStatus } from '../lib/sync';
import { AssistProviderSetup } from './AssistSettings';
import { ImportDialog } from './ImportDialog';
import { Modal } from './Modal';
import { NyuScene, type SceneName } from './nyu/scenes';
import { Row, Toggle } from './SettingsDialog';
import { SyncSettings } from './SyncSettings';
import { VaultDialog } from './VaultDialog';

type Props = {
  /** Finished or skipped: either way the wizard does not come back by itself. */
  onClose: () => void;
  /** An import added hosts: the host list reloads. */
  onHostsChanged: () => void;
};

type StepId = 'welcome' | 'design' | 'vault' | 'import' | 'assist' | 'done';

const STEPS: { id: StepId; label: string; scene: SceneName }[] = [
  { id: 'welcome', label: N_('Willkommen'), scene: 'welcome' },
  { id: 'design', label: N_('Design'), scene: 'design' },
  { id: 'vault', label: N_('Tresor & Sync'), scene: 'vault' },
  { id: 'import', label: N_('Import'), scene: 'files' },
  { id: 'assist', label: N_('KI & Warnungen'), scene: 'assist' },
  { id: 'done', label: N_('Fertig'), scene: 'done' },
];

const IMPORT_NAMES: Record<ImportSource, string> = {
  termius: 'Termius',
  putty: 'PuTTY',
  kitty: 'KiTTY',
  openssh: 'OpenSSH',
  folder: 'KiTTY / PuTTY',
};

/**
 * The first start: six short steps, each one skippable, that set up what
 * otherwise hides in the settings — how the app looks, the vault and sync,
 * hosts from another client, the command assistant and the password warning.
 *
 * It reuses the dialogs and setters the settings use, so nothing here can
 * drift from them: the vault dialog, the sync settings, the import dialog and
 * the assistant's provider setup open over the wizard, and the wizard reads
 * the result back when they close. Escape or "Einrichtung überspringen" ends
 * it; Settings → Darstellung starts it again.
 */
export function Onboarding({ onClose, onHostsChanged }: Props) {
  useLanguage();
  const settings = useSettings();
  const [index, setIndex] = useState(0);
  const step = STEPS[index]!;
  const last = index === STEPS.length - 1;

  const [vault, setVault] = useState<VaultStatus | null>(null);
  const [sync, setSync] = useState<SyncStatus | null>(null);
  const [hosts, setHosts] = useState<number | null>(null);
  const [sources, setSources] = useState<ImportSource[] | null>(null);
  const [assistSaved, setAssistSaved] = useState(false);
  const [open, setOpen] = useState<'vault' | 'sync' | 'import' | null>(null);

  const refresh = useCallback(() => {
    void vaultState()
      .then((state) => setVault(state.status))
      .catch(() => undefined);
    void syncStatus()
      .then(setSync)
      .catch(() => undefined);
    void listHosts()
      .then((list) => setHosts(list.length))
      .catch(() => undefined);
  }, []);

  useEffect(() => {
    refresh();
    void availableImports()
      .then(setSources)
      .catch(() => setSources([]));
  }, [refresh]);

  const synced = Boolean(sync && (sync.paired || sync.backend === 'uwulock'));
  const done: Record<StepId, boolean> = {
    welcome: true,
    design: true,
    vault: vault === 'unlocked' || vault === 'locked' || synced,
    import: (hosts ?? 0) > 0,
    assist: assistSaved,
    done: true,
  };

  const close = (which: 'vault' | 'sync' | 'import') => {
    if (open === which) setOpen(null);
    refresh();
  };

  let page: ReactNode;
  switch (step.id) {
    case 'welcome':
      page = <Welcome />;
      break;
    case 'design':
      page = <Design />;
      break;
    case 'vault':
      page = (
        <VaultStep
          vault={vault}
          sync={sync}
          onVault={() => setOpen('vault')}
          onSync={() => setOpen('sync')}
        />
      );
      break;
    case 'import':
      page = <ImportStep sources={sources} hosts={hosts} onImport={() => setOpen('import')} />;
      break;
    case 'assist':
      page = <AssistStep saved={assistSaved} onSaved={() => setAssistSaved(true)} />;
      break;
    case 'done':
      page = (
        <Summary
          items={[
            {
              label: t('Design'),
              ok: true,
              note: t(THEMES.find((theme) => theme.value === settings.theme)?.label ?? ''),
            },
            {
              label: t('Tresor'),
              ok: vault === 'unlocked' || vault === 'locked',
              note:
                vault === 'absent' ? t('wird angelegt, sobald du ein Passwort speicherst') : null,
            },
            {
              label: t('Sync'),
              ok: synced,
              note: synced ? (sync?.backend === 'uwulock' ? 'UwULock' : 'UwUSync') : null,
            },
            {
              label: t('Hosts'),
              ok: (hosts ?? 0) > 0,
              note: hosts ? t('{count} in der Liste', { count: hosts }) : null,
            },
            { label: t('KI-Assistent'), ok: assistSaved, note: null },
          ]}
        />
      );
      break;
  }

  const primaryLabel = last ? t('Los geht’s') : done[step.id] ? t('Weiter') : t('Überspringen');

  return (
    <Modal
      title={stepTitle(step.id)}
      size="wizard"
      onCancel={onClose}
      // One height for every step, so the buttons stay where the pointer is;
      // only the page between title and buttons scrolls (down to 720 × 480).
      className="onboarding-dialog"
      footer={
        <>
          {!last && (
            <Button variant="ghost" data-secondary onClick={onClose}>
              {t('Einrichtung überspringen')}
            </Button>
          )}
          <span className="spacer" />
          {index > 0 && (
            <Button data-secondary onClick={() => setIndex(index - 1)}>
              {t('Zurück')}
            </Button>
          )}
          <Button
            variant="primary"
            data-autofocus
            onClick={() => (last ? onClose() : setIndex(index + 1))}
          >
            {primaryLabel}
          </Button>
        </>
      }
    >
      <div className="onboarding">
        <ol className="onboarding-steps" aria-label={t('Schritte der Einrichtung')}>
          {STEPS.map((candidate, at) => (
            <li key={candidate.id}>
              <button
                type="button"
                aria-current={at === index ? 'step' : undefined}
                data-state={at < index ? 'past' : at === index ? 'current' : 'next'}
                onClick={() => setIndex(at)}
              >
                <span className="onboarding-dot" aria-hidden>
                  {at < index ? <Icon icon={ICONS.done} size="xs" /> : at + 1}
                </span>
                <span className="onboarding-step-label">{t(candidate.label)}</span>
              </button>
            </li>
          ))}
        </ol>
        <p className="sr-only" aria-live="polite">
          {t('Schritt {step} von {count}: {name}', {
            step: index + 1,
            count: STEPS.length,
            name: t(step.label),
          })}
        </p>
        <div className="onboarding-page" key={step.id}>
          <NyuScene name={step.scene} className="onboarding-scene" />
          <div className="onboarding-content">{page}</div>
        </div>
      </div>

      {open === 'vault' && (
        <VaultDialog
          cancelLabel={t('Später')}
          onDone={() => close('vault')}
          onCancel={() => close('vault')}
        />
      )}
      {open === 'sync' && (
        <Modal
          title={t('Sync einrichten')}
          onCancel={() => close('sync')}
          footer={
            <Button variant="primary" data-secondary onClick={() => close('sync')}>
              {t('Fertig')}
            </Button>
          }
        >
          <div className="onboarding-sync">
            <SyncSettings />
          </div>
        </Modal>
      )}
      {open === 'import' && (
        <ImportDialog
          onClose={() => close('import')}
          onImported={() => {
            onHostsChanged();
            refresh();
          }}
        />
      )}
    </Modal>
  );
}

function stepTitle(id: StepId): string {
  switch (id) {
    case 'welcome':
      return t('Willkommen bei UwUSSH');
    case 'design':
      return t('Wie soll UwUSSH aussehen?');
    case 'vault':
      return t('Tresor und Sync');
    case 'import':
      return t('Hosts mitbringen');
    case 'assist':
      return t('KI-Assistent und Warnungen');
    case 'done':
      return t('Alles bereit!');
  }
}

function Welcome() {
  const settings = useSettings();
  return (
    <>
      <p className="dialog-lead">
        {t(
          'Schön, dass du da bist! Nyu hilft dir in ein paar Schritten beim Einrichten: Aussehen, Tresor und Sync, Hosts aus anderen Programmen und auf Wunsch der KI-Assistent.',
        )}
      </p>
      <p className="setting-description">
        {t('Jeder Schritt lässt sich überspringen und später unter Einstellungen nachholen.')}
      </p>
      <Row label="Sprache · Language">
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
    </>
  );
}

const THEMES: { value: ThemeSetting; label: string }[] = [
  { value: 'system', label: N_('System') },
  { value: 'light', label: N_('Hell') },
  { value: 'dark', label: N_('Dunkel') },
];

function Design() {
  const settings = useSettings();
  return (
    <>
      <p className="text-body font-semibold">{t('Farbschema')}</p>
      <div className="theme-cards" role="radiogroup" aria-label={t('Farbschema')}>
        {THEMES.map(({ value, label }) => (
          <button
            key={value}
            type="button"
            role="radio"
            className="theme-card"
            aria-checked={settings.theme === value}
            onClick={() => {
              updateSettings({ theme: value });
            }}
          >
            <span className="theme-preview" data-preview={value} aria-hidden>
              <span className="theme-preview-bar" />
              <span className="theme-preview-line" />
              <span className="theme-preview-line short" />
            </span>
            <span>{t(label)}</span>
          </button>
        ))}
      </div>
      <p className="setting-description">
        {t('Das Terminal bleibt in jedem Schema dunkel.')}{' '}
        {t('„System“ folgt der Einstellung von {system}.', { system: systemName() })}
      </p>
      <Row
        label={t('Animationen')}
        description={t('Nyu bewegt sich, Tabs pulsieren beim Verbinden. Aus ist ruhiger.')}
      >
        <Segmented
          label={t('Animationen')}
          value={settings.motion}
          onChange={(motion) => {
            updateSettings({ motion });
          }}
          options={[
            { value: 'system', label: t('System') },
            { value: 'on', label: t('An') },
            { value: 'off', label: t('Aus') },
          ]}
        />
      </Row>
    </>
  );
}

function Choice({
  icon,
  title,
  text,
  ok,
  okText,
  onClick,
}: {
  icon: LucideIcon;
  title: string;
  text: string;
  ok: boolean;
  okText: string;
  onClick: () => void;
}) {
  return (
    <button className="sync-choice onboarding-choice" data-ok={ok || undefined} onClick={onClick}>
      <Icon icon={ok ? ICONS.done : icon} size="lg" />
      <span>
        <b>{ok ? okText : title}</b>
        <small>{text}</small>
      </span>
    </button>
  );
}

function VaultStep({
  vault,
  sync,
  onVault,
  onSync,
}: {
  vault: VaultStatus | null;
  sync: SyncStatus | null;
  onVault: () => void;
  onSync: () => void;
}) {
  const hasVault = vault === 'unlocked' || vault === 'locked';
  const synced = Boolean(sync && (sync.paired || sync.backend === 'uwulock'));
  return (
    <>
      <p className="dialog-lead">
        {t(
          'Passwörter und Keys liegen verschlüsselt im Tresor. Mit Sync sind Hosts, Keys und Passwörter auf allen deinen Geräten gleich – Ende-zu-Ende-verschlüsselt.',
        )}
      </p>
      <div className="sync-choices">
        <Choice
          icon={ICONS.masterPassword}
          title={t('Master-Passwort festlegen')}
          okText={t('Tresor ist angelegt')}
          text={t(
            'Neu hier: ein Tresor für dieses Gerät. Auf Wunsch öffnet {system} ihn, ohne jedes Mal zu fragen.',
            { system: systemName() },
          )}
          ok={hasVault}
          onClick={onVault}
        />
        <Choice
          icon={ICONS.sync}
          title={t('Sync verbinden')}
          okText={
            sync?.backend === 'uwulock'
              ? t('Mit UwULock verbunden')
              : t('Mit {server} verbunden', { server: sync?.serverUrl ?? 'UwUSync' })
          }
          text={t(
            'Schon ein UwULock-Konto oder ein UwUSync-Server? Dann gleich hier verbinden – der Tresor kommt dabei mit.',
          )}
          ok={synced}
          onClick={onSync}
        />
      </div>
      <p className="setting-description">
        {t('Beides geht auch später unter Einstellungen → Tresor & Keys und → Sync.')}
      </p>
    </>
  );
}

function ImportStep({
  sources,
  hosts,
  onImport,
}: {
  sources: ImportSource[] | null;
  hosts: number | null;
  onImport: () => void;
}) {
  const found = (sources ?? []).filter((source) => source !== 'folder');
  return (
    <>
      <p className="dialog-lead">
        {t(
          'Hosts aus Termius, PuTTY, KiTTY oder ~/.ssh/config übernehmen – oder aus einer .uwussh-Exportdatei. Schon vorhandene Hosts werden übersprungen.',
        )}
      </p>
      {sources === null ? (
        <p className="setting-description">{t('Suche nach anderen Programmen…')}</p>
      ) : found.length > 0 ? (
        <p className="onboarding-found">
          <span>{t('Auf diesem Rechner gefunden:')}</span>
          {found.map((source) => (
            <Tag key={source} tone="pink">
              {IMPORT_NAMES[source]}
            </Tag>
          ))}
        </p>
      ) : (
        <p className="setting-description">
          {t('Kein anderes Programm gefunden – ein Ordner oder eine Exportdatei geht trotzdem.')}
        </p>
      )}
      <div className="onboarding-actions">
        <Button icon={ICONS.import} onClick={onImport}>
          {t('Importieren…')}
        </Button>
        {(hosts ?? 0) > 0 && (
          <span className="onboarding-ok">
            <Icon icon={ICONS.done} size="xs" />
            {t('{count} Hosts in der Liste', { count: hosts ?? 0 })}
          </span>
        )}
      </div>
    </>
  );
}

function AssistStep({ saved, onSaved }: { saved: boolean; onSaved: () => void }) {
  const settings = useSettings();
  const [setup, setSetup] = useState(false);
  return (
    <>
      <p className="dialog-lead">
        {t(
          'Beschreibe in Worten, was du willst – der Assistent schreibt einen passenden Befehl und tippt ihn ins Terminal. Ausgeführt wird erst mit deinem Enter. Öffnen mit {shortcut}.',
          { shortcut: keysFor('assist') },
        )}
      </p>
      {setup ? (
        <AssistProviderSetup
          saveLabel={t('Speichern')}
          onSaved={() => {
            onSaved();
            setSetup(false);
          }}
        />
      ) : (
        <div className="onboarding-actions">
          <Button icon={ICONS.ai} onClick={() => setSetup(true)}>
            {saved ? t('Anbieter ändern…') : t('KI einrichten…')}
          </Button>
          {saved ? (
            <span className="onboarding-ok">
              <Icon icon={ICONS.done} size="xs" />
              {t('Gespeichert')}
            </span>
          ) : (
            <span className="setting-description">
              {t('Optional – geht auch später unter Einstellungen → KI.')}
            </span>
          )}
        </div>
      )}
      <Row
        label={t('Warnung bei Passwort-Login anzeigen')}
        description={t(
          'Ein Warnzeichen in der Hostliste neben Hosts, die sich mit Benutzer und Passwort statt mit einem SSH-Key anmelden.',
        )}
      >
        <Toggle
          label={t('Warnung bei Passwort-Login anzeigen')}
          checked={passwordLoginWarningOn(settings)}
          onChange={setPasswordLoginWarning}
        />
      </Row>
    </>
  );
}

function Summary({ items }: { items: { label: string; ok: boolean; note: string | null }[] }) {
  return (
    <>
      <p className="dialog-lead">
        {t(
          'UwUSSH ist eingerichtet. Was noch fehlt, holst du jederzeit in den Einstellungen nach.',
        )}
      </p>
      <ul className="onboarding-summary">
        {items.map((item) => (
          <li key={item.label} data-ok={item.ok || undefined}>
            <span className="onboarding-mark" aria-hidden>
              {item.ok ? <Icon icon={ICONS.done} size="xs" /> : '–'}
            </span>
            <b>{item.label}</b>
            <span className="sr-only">{item.ok ? t('eingerichtet') : t('übersprungen')}</span>
            {item.note && <small>{item.note}</small>}
          </li>
        ))}
      </ul>
      <div className="shortcuts onboarding-tips">
        <dl>
          {localShellAvailable() && (
            <>
              <dt>{keysFor('new-shell')}</dt>
              <dd>{t('Neue lokale Shell')}</dd>
            </>
          )}
          <dt>{keysFor('assist')}</dt>
          <dd>{t('Befehl aus Worten')}</dd>
          <dt>{keysFor('settings')}</dt>
          <dd>{t('Einstellungen')}</dd>
        </dl>
      </div>
    </>
  );
}
