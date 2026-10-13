import { listen } from '@tauri-apps/api/event';
import { Button, Card, Hint, Icon, ICONS, Segmented, Select, Tag } from '@uwusuite/design';
import { useCallback, useEffect, useRef, useState } from 'react';
import { t, useLanguage } from '../lib/i18n';
import type { Workspace } from '../lib/session';
import { useSettings, workspaceName } from '../lib/settings';
import {
  asksOwnPassword,
  blankStatus,
  canKeepEntries,
  canMakeMain,
  hasTwoServers,
  mainSpace,
  otherSpace,
  serverSpaces,
  spaceName,
} from '../lib/spaces';
import {
  asSyncFailure,
  isPasteablePairing,
  NEW_SPACE,
  spaceDiscardNew,
  spaceMakeMain,
  spaceRemove,
  spaceSetDefault,
  spacesStatus,
  spaceUnlock,
  syncCancelOffer,
  syncConnect,
  syncDevices,
  syncDisconnect,
  syncJoin,
  syncNow,
  syncOffer,
  syncRecoveryCode,
  syncRevoke,
  syncWaitForDevice,
  lockForgetMove,
  type Connected,
  type Device,
  type LockOutcome,
  type Offer,
  type SpaceInfo,
  type SpacesStatus,
  type SyncStatus,
} from '../lib/sync';
import { LockOverview, LockSignInForm, MoveDone } from './LockSync';
import { Modal } from './Modal';
import { NyuScene } from './nyu/scenes';
import {
  ago,
  APP_SYNC_OFF,
  copy,
  failureText,
  PassDetails,
  PasswordConfirm,
  SpaceProvider,
  syncDot,
  SyncRow,
  useAppSyncOff,
  useSpace,
} from './SyncParts';
import { TeamsGoNote } from './TeamParts';
import { VaultDialog } from './VaultDialog';

/**
 * Settings → Sync. Syncing nowhere yet: pick UwULock (sign in) or UwUSync
 * (connect a server as the first device, or join from another one). On
 * UwUSync: what the last pass did, adding a device, the device list with
 * revoking, the move to UwULock, and leaving. On UwULock: see `LockSync`.
 *
 * With one server that is the whole page, plus the way to a second one once
 * the first is connected. With two, each gets a section of the same rows
 * under its name, and the page adds what only two servers need: which one is
 * the main server, and where new hosts go.
 */
export function SyncSettings({ offerSecond = true }: { offerSecond?: boolean }) {
  useLanguage();
  const [spaces, setSpaces] = useState<SpacesStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [makingMain, setMakingMain] = useState<string | null>(null);
  const [removing, setRemoving] = useState<string | null>(null);
  // The second server while it is set up: thrown away unless it got as far
  // as being connected.
  const pending = useRef(false);

  const refresh = useCallback(() => {
    void spacesStatus()
      .then((found) => {
        setSpaces(found);
        setError(null);
      })
      .catch((e) => setError(String(e)));
  }, []);

  useEffect(() => {
    refresh();
    const stops = [listen('sync:status', refresh), listen('spaces:changed', refresh)];
    // "vor 2 Minuten" moves on by itself.
    const timer = window.setInterval(refresh, 20_000);
    return () => {
      window.clearInterval(timer);
      for (const stop of stops) void stop.then((unlisten) => unlisten());
      // Settings closed halfway through the second server's setup.
      if (pending.current) {
        pending.current = false;
        void spaceDiscardNew().catch(() => undefined);
      }
    };
  }, [refresh]);

  const main = spaces ? mainSpace(spaces) : null;
  if (!spaces || !main) {
    return <p className="setting-description">{error ?? t('Wird geladen…')}</p>;
  }

  if (adding) {
    const finish = () => {
      pending.current = false;
      setAdding(false);
      refresh();
    };
    return (
      <SpaceProvider value={{ space: NEW_SPACE, second: true, leave: null, after: null }}>
        <SpaceView
          key="new"
          status={blankStatus(main.status)}
          refresh={refresh}
          onRegistered={() => {
            pending.current = false;
          }}
          onFinished={finish}
          onCancel={() => {
            pending.current = false;
            void spaceDiscardNew().catch(() => undefined);
            setAdding(false);
          }}
        />
      </SpaceProvider>
    );
  }

  if (!hasTwoServers(spaces)) {
    const second =
      offerSecond && spaces.canAdd ? (
        <SyncRow
          label={t('Zweiter Server')}
          control={
            <Button
              size="sm"
              icon={ICONS.add}
              onClick={() => {
                pending.current = true;
                setAdding(true);
              }}
            >
              {t('Zweiten Server verbinden…')}
            </Button>
          }
        >
          <p className="setting-description">
            {t(
              'Für Hosts, die getrennt bleiben sollen – etwa private und die der Arbeit. Jeder Host liegt auf genau einem Server.',
            )}
          </p>
        </SyncRow>
      ) : null;
    return (
      <SpaceProvider value={{ space: undefined, second: false, leave: null, after: second }}>
        <SpaceView key="main" status={main.status} refresh={refresh} />
      </SpaceProvider>
    );
  }

  const target = (id: string | null) =>
    serverSpaces(spaces).find((space) => space.id === id) ?? null;
  const toMain = target(makingMain);
  const toRemove = target(removing);

  return (
    <div className="sync-spaces">
      {/* Teams are spaces too; they have their own page. */}
      {serverSpaces(spaces).map((space) => (
        <section key={space.id} className="sync-space" aria-label={spaceTitle(space)}>
          <header className="sync-space-head">
            <Icon icon={ICONS.server} className="text-muted" />
            <b>{spaceTitle(space)}</b>
            {space.status.backend !== 'none' && (
              <Tag>{space.status.backend === 'uwulock' ? 'UwULock' : 'UwUSync'}</Tag>
            )}
            {space.main && <Tag tone="pink">{t('Hauptserver')}</Tag>}
            <span className="spacer" />
            {!space.main && (
              <Button
                size="sm"
                disabled={!canMakeMain(space, spaces)}
                title={
                  canMakeMain(space, spaces)
                    ? undefined
                    : t('Dafür müssen beide Server verbunden und entsperrt sein.')
                }
                onClick={() => setMakingMain(space.id)}
              >
                {t('Als Hauptserver festlegen…')}
              </Button>
            )}
          </header>
          <SpaceProvider
            value={{
              space: space.id,
              second: false,
              leave: () => setRemoving(space.id),
              after: null,
            }}
          >
            {asksOwnPassword(space, spaces) ? (
              <LockedSpace space={space} onChanged={refresh} />
            ) : (
              <SpaceView status={space.status} refresh={refresh} />
            )}
          </SpaceProvider>
        </section>
      ))}

      <DefaultServers spaces={spaces} onChanged={refresh} />

      {toMain && (
        <MakeMainDialog
          space={toMain}
          onCancel={() => setMakingMain(null)}
          onDone={() => {
            setMakingMain(null);
            refresh();
          }}
        />
      )}
      {toRemove && (
        <RemoveDialog
          space={toRemove}
          spaces={spaces}
          onCancel={() => setRemoving(null)}
          onDone={() => {
            setRemoving(null);
            refresh();
          }}
        />
      )}
    </div>
  );
}

/** A server's name on the page; one that is connected nowhere says so. */
function spaceTitle(space: SpaceInfo): string {
  return spaceName(space) || t('Nicht verbunden');
}

// ── Two servers ───────────────────────────────────────────────────────────

/**
 * A second server whose vault the main key did not open (its password was
 * changed elsewhere, say): that server's own password opens it once and the
 * app remembers it again.
 */
function LockedSpace({ space, onChanged }: { space: SpaceInfo; onChanged: () => void }) {
  useLanguage();
  const [asking, setAsking] = useState(false);
  const name = spaceTitle(space);
  return (
    <>
      <SyncRow
        label={t('Gesperrt')}
        dot="offline"
        control={
          <Button size="sm" icon={ICONS.unlocked} onClick={() => setAsking(true)}>
            {t('Entsperren…')}
          </Button>
        }
      >
        <p className="setting-description">
          {t(
            'UwUSSH konnte diesen Server nicht mit dem Passwort des Hauptservers öffnen. Gib einmal sein eigenes Master-Passwort ein. Bis dahin fehlen seine Hosts in der Liste.',
          )}
        </p>
      </SyncRow>
      {asking && (
        <PasswordConfirm
          title={t('{server} entsperren', { server: name })}
          lead={t(
            'Das Master-Passwort dieses Servers, nicht das des Hauptservers. Danach öffnet wieder ein Passwort beide.',
          )}
          action={t('Entsperren')}
          tone="normal"
          passwordLabel={t('Master-Passwort von {server}', { server: name })}
          run={(password) => spaceUnlock(space.id, password)}
          onCancel={() => setAsking(false)}
          onDone={() => {
            setAsking(false);
            onChanged();
          }}
        />
      )}
    </>
  );
}

/** Where new hosts go, per workspace. Hosts that exist stay where they are. */
function DefaultServers({ spaces, onChanged }: { spaces: SpacesStatus; onChanged: () => void }) {
  useLanguage();
  const settings = useSettings();
  const [error, setError] = useState<string | null>(null);
  const workspaces: Workspace[] = settings.workspaces ? ['private', 'business'] : ['private'];

  const choose = (workspace: Workspace, space: string) => {
    setError(null);
    void spaceSetDefault(workspace, space)
      .catch((e) => setError(failureText(asSyncFailure(e))))
      .finally(onChanged);
  };

  return (
    <section className="sync-space" aria-label={t('Neue Hosts')}>
      <header className="sync-space-head">
        <b>{t('Neue Hosts')}</b>
      </header>
      {workspaces.map((workspace) => {
        const label = settings.workspaces
          ? t('Neue Hosts in {workspace}', { workspace: workspaceName(workspace, settings) })
          : t('Neue Hosts speichern auf');
        return (
          <SyncRow
            key={workspace}
            label={label}
            control={
              <Select
                className="w-56"
                aria-label={label}
                value={spaces.defaults[workspace]}
                onChange={(event) => choose(workspace, event.target.value)}
              >
                {serverSpaces(spaces).map((space) => (
                  <option key={space.id} value={space.id} disabled={space.readOnly}>
                    {spaceTitle(space)}
                  </option>
                ))}
              </Select>
            }
          />
        );
      })}
      <p className="setting-description">
        {t(
          'Gilt für neue und importierte Hosts. Vorhandene bleiben, wo sie sind – im Host-Dialog ziehst du einen einzelnen um.',
        )}
      </p>
      {error && (
        <p className="field-error" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}

/** What changes when another server becomes the main one, before it does. */
function MakeMainDialog({
  space,
  onCancel,
  onDone,
}: {
  space: SpaceInfo;
  onCancel: () => void;
  onDone: () => void;
}) {
  useLanguage();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const name = spaceTitle(space);

  const run = async () => {
    setBusy(true);
    setError(null);
    try {
      await spaceMakeMain(space.id);
      onDone();
    } catch (e) {
      setError(failureText(asSyncFailure(e)));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      title={t('{server} als Hauptserver festlegen?', { server: name })}
      onCancel={() => !busy && onCancel()}
      footer={
        <>
          <Button data-autofocus onClick={onCancel} disabled={busy}>
            {t('Abbrechen')}
          </Button>
          <Button
            variant="primary"
            data-secondary
            busy={busy}
            disabled={busy}
            onClick={() => void run()}
          >
            {busy ? t('Einen Moment…') : t('Als Hauptserver festlegen')}
          </Button>
        </>
      }
    >
      <p className="dialog-lead">
        {t(
          'Deine persönlichen Dinge ziehen auf diesen Server um: Tastenkürzel, gespeicherte Texte und die KI-Einstellungen. Hosts bleiben, wo sie sind.',
        )}
      </p>
      <Hint tone="warning">
        {t('Ab dann öffnet das Master-Passwort von {server} UwUSSH – nicht mehr das bisherige.', {
          server: name,
        })}
      </Hint>
      <p className="setting-description">
        {t(
          'Lege auf deinen anderen Geräten denselben Hauptserver fest, sonst teilen sie Kürzel und Texte nicht mehr.',
        )}
      </p>
      {error && (
        <p className="field-error" role="alert">
          {error}
        </p>
      )}
    </Modal>
  );
}

/**
 * Disconnecting one of two servers: its entries either go from this device,
 * or come along to the server that stays.
 */
function RemoveDialog({
  space,
  spaces,
  onCancel,
  onDone,
}: {
  space: SpaceInfo;
  spaces: SpacesStatus;
  onCancel: () => void;
  onDone: () => void;
}) {
  useLanguage();
  const keepable = canKeepEntries(space, spaces);
  const [keep, setKeep] = useState(false);
  const name = spaceTitle(space);
  const other = otherSpace(spaces, space.id);
  const otherName = other ? spaceTitle(other) : '';

  return (
    <PasswordConfirm
      title={t('{server} trennen?', { server: name })}
      lead={t(
        'Dieses Gerät synchronisiert dann nicht mehr mit diesem Server. Auf dem Server und auf deinen anderen Geräten bleibt alles, wie es ist.',
      )}
      action={t('Trennen')}
      passwordLabel={t('Master-Passwort von {server}', { server: name })}
      run={(password) => spaceRemove(space.id, password, keep && keepable)}
      onCancel={onCancel}
      onDone={onDone}
    >
      <fieldset className="sync-remove-choice">
        <legend>{t('Was passiert mit den Einträgen dieses Servers?')}</legend>
        <label className="check">
          <input type="radio" name="sync-remove" checked={!keep} onChange={() => setKeep(false)} />
          <span>
            <b>{t('Einträge von diesem Gerät entfernen')}</b>
            <small>{t('Hosts, Keys und Passwörter dieses Servers verschwinden hier.')}</small>
          </span>
        </label>
        <label className="check" data-disabled={keepable ? undefined : true}>
          <input
            type="radio"
            name="sync-remove"
            checked={keep && keepable}
            disabled={!keepable}
            onChange={() => setKeep(true)}
          />
          <span>
            <b>{t('Auf den anderen Server übernehmen')}</b>
            <small>
              {keepable
                ? t('Sie ziehen zu {server} um und synchronisieren von da an dort.', {
                    server: otherName,
                  })
                : t('Dafür müssen beide Server entsperrt sein.')}
            </small>
          </span>
        </label>
      </fieldset>
      <TeamsGoNote server={space.id} />
      {space.main && other && (
        <Hint tone="warning">
          {t(
            'Das ist dein Hauptserver. Danach ist {server} der Hauptserver: Tastenkürzel, Texte und KI-Einstellungen ziehen dorthin, und sein Master-Passwort öffnet UwUSSH.',
            { server: otherName },
          )}
        </Hint>
      )}
    </PasswordConfirm>
  );
}

// ── One server ────────────────────────────────────────────────────────────

/**
 * The pages of one server, whichever the scope around it names. While the
 * second server is being set up (`onCancel` and the others are set), it is
 * the intro and its forms only, and finishing hands back to the page.
 */
function SpaceView({
  status,
  refresh,
  onCancel,
  onRegistered,
  onFinished,
}: {
  status: SyncStatus;
  refresh: () => void;
  onCancel?: () => void;
  /** The second server is connected: nothing to throw away any more. */
  onRegistered?: () => void;
  /** And the person has seen what there was to see. */
  onFinished?: () => void;
}) {
  useLanguage();
  const { second } = useSpace();
  const [mode, setMode] = useState<'overview' | 'connect' | 'join' | 'lock'>('overview');
  const [backend, setBackend] = useState<'uwulock' | 'uwusync'>('uwulock');
  const [kit, setKit] = useState<Connected | null>(null);
  const [moved, setMoved] = useState<Extract<LockOutcome, { kind: 'moved' }> | null>(null);

  const done = () => {
    setMode('overview');
    if (onFinished) onFinished();
    else refresh();
  };

  if (kit) {
    return (
      <RecoveryKit
        kit={kit}
        onDone={() => {
          setKit(null);
          done();
        }}
      />
    );
  }

  if (moved) {
    return (
      <MoveDone
        outcome={moved}
        leftBehind={status.lock.leftBehind}
        onDone={() => {
          setMoved(null);
          refresh();
        }}
      />
    );
  }

  if (status.backend === 'uwulock') {
    return <LockOverview status={status} onChanged={refresh} />;
  }

  if (!status.paired) {
    if (mode === 'lock') {
      return (
        <LockSignInForm
          mode="sign-in"
          status={status}
          onBack={() => setMode('overview')}
          onDone={() => {
            onRegistered?.();
            done();
          }}
        />
      );
    }
    if (mode === 'connect') {
      return (
        <ConnectForm
          status={status}
          onBack={() => setMode('overview')}
          onConnected={(connected) => {
            onRegistered?.();
            setKit(connected);
          }}
        />
      );
    }
    if (mode === 'join') {
      return (
        <JoinForm
          status={status}
          onBack={() => setMode('overview')}
          onJoined={() => {
            onRegistered?.();
            done();
          }}
        />
      );
    }
    return (
      <div className="sync-intro">
        {second ? (
          <>
            <p className="setting-label">{t('Zweiten Server verbinden')}</p>
            <p className="setting-description">
              {t(
                'Der zweite Server hat sein eigenes Konto und sein eigenes Master-Passwort. Im Alltag öffnet das Passwort des Hauptservers beide.',
              )}
            </p>
          </>
        ) : (
          <NyuScene name="welcome" className="sync-scene" />
        )}
        <Segmented
          label={t('Synchronisieren über')}
          value={backend}
          onChange={setBackend}
          options={[
            { value: 'uwulock', label: 'UwULock' },
            { value: 'uwusync', label: 'UwUSync' },
          ]}
        />
        {backend === 'uwulock' ? (
          <>
            <p className="dialog-lead">
              {t(
                'Mit deinem UwULock-Konto bleiben Hosts, Gruppen, Keys und Passwörter auf allen deinen Geräten gleich – im selben Konto wie deine Passwörter, Ende-zu-Ende-verschlüsselt. Der Server sieht davon nur verschlüsselte Blöcke.',
              )}
            </p>
            <div className="sync-choices">
              <button className="sync-choice" onClick={() => setMode('lock')}>
                <Icon icon={ICONS.vault} size="lg" />
                <span>
                  <b>{t('Mit UwULock anmelden')}</b>
                  <small>
                    {t(
                      'Jedes Gerät gleich: Server-Adresse, E-Mail und Master-Passwort deines UwULock-Kontos, dazu die Zwei-Faktor-Anmeldung, falls eingerichtet.',
                    )}
                  </small>
                </span>
              </button>
            </div>
            <p className="setting-description">
              {second
                ? t('Einen Server aufsetzen: siehe github.com/MinifyX/UwULock-Server.')
                : t(
                    'Einen Server aufsetzen: siehe github.com/MinifyX/UwULock-Server. Schon auf UwUSync? Dann dort verbinden und unter Sync → Zu UwULock umziehen.',
                  )}
            </p>
          </>
        ) : (
          <>
            <p className="dialog-lead">
              {t(
                'Mit einem eigenen UwUSync-Server bleiben Hosts, Gruppen, Keys und Passwörter auf allen deinen Geräten gleich. Der Server sieht davon nur verschlüsselte Blöcke.',
              )}
            </p>
            <div className="sync-choices">
              <button className="sync-choice" onClick={() => setMode('connect')}>
                <Icon icon={ICONS.server} size="lg" />
                <span>
                  <b>{t('Server verbinden')}</b>
                  <small>
                    {t(
                      'Das erste Gerät: mit dem Einrichtungscode, den der Server beim ersten Start zeigt.',
                    )}
                  </small>
                </span>
              </button>
              <button className="sync-choice" onClick={() => setMode('join')}>
                <Icon icon={ICONS.link} size="lg" />
                <span>
                  <b>{t('Mit einem Gerät koppeln')}</b>
                  <small>
                    {t(
                      'Ein weiteres Gerät: mit dem Code, den ein schon verbundenes Gerät unter „Gerät hinzufügen“ zeigt.',
                    )}
                  </small>
                </span>
              </button>
            </div>
            <p className="setting-description">
              {t(
                'Einen Server aufsetzen: siehe github.com/MinifyX/UwUSync-Server – ein Befehl, Docker, fertig.',
              )}
            </p>
          </>
        )}
        {onCancel && (
          <div className="sync-actions">
            <Button data-secondary onClick={onCancel}>
              {t('Abbrechen')}
            </Button>
          </div>
        )}
      </div>
    );
  }

  return (
    <Paired
      status={status}
      onChanged={refresh}
      onMoved={(outcome) => {
        if (outcome.kind === 'moved') setMoved(outcome);
        refresh();
      }}
    />
  );
}

// ── First device ──────────────────────────────────────────────────────────

function ConnectForm({
  status,
  onBack,
  onConnected,
}: {
  status: SyncStatus;
  onBack: () => void;
  onConnected: (connected: Connected) => void;
}) {
  useLanguage();
  const { space, second } = useSpace();
  const fresh = status.vault === 'absent';
  const [code, setCode] = useState('');
  const [name, setName] = useState(status.deviceName);
  const [password, setPassword] = useState('');
  const [repeat, setRepeat] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const mismatch = fresh && repeat.length > 0 && password !== repeat;
  const ready =
    !busy &&
    code.trim().startsWith('uwu1_') &&
    name.trim().length > 0 &&
    password.length > 0 &&
    (!fresh || (password.length >= 8 && password === repeat));

  const submit = async () => {
    if (!ready) return;
    setBusy(true);
    setError(null);
    try {
      const connected = await syncConnect(code, password, name, space);
      setPassword('');
      setRepeat('');
      onConnected(connected);
    } catch (e) {
      setError(failureText(asSyncFailure(e)));
      setPassword('');
      setRepeat('');
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      className="form sync-form"
      onSubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
      <p className="setting-label">{t('Server verbinden')}</p>
      <label className="field">
        <span>{t('Einrichtungscode')}</span>
        <textarea
          className="sync-code-input"
          value={code}
          rows={3}
          spellCheck={false}
          autoFocus
          placeholder="uwu1_…"
          onChange={(e) => setCode(e.target.value)}
        />
        <em className="field-hint">
          {t(
            'Steht im Log des Servers, oder: docker compose exec uwusync uwusync-server invite. Er enthält Adresse, Zertifikat-Fingerprint und eine Einladung.',
          )}
        </em>
      </label>
      <label className="field">
        <span>{t('Name dieses Geräts')}</span>
        <input value={name} maxLength={64} onChange={(e) => setName(e.target.value)} />
      </label>
      <label className="field">
        <span>{fresh ? t('Neues Master-Passwort') : t('Master-Passwort deines Tresors')}</span>
        <input
          type="password"
          value={password}
          autoComplete={fresh ? 'new-password' : 'current-password'}
          onChange={(e) => setPassword(e.target.value)}
        />
      </label>
      {fresh && (
        <label className="field">
          <span>{t('Wiederholen')}</span>
          <input
            type="password"
            value={repeat}
            autoComplete="new-password"
            aria-invalid={mismatch}
            onChange={(e) => setRepeat(e.target.value)}
          />
        </label>
      )}
      <p className={mismatch ? 'field-error' : 'field-hint'}>
        {mismatch
          ? t('Die Passwörter stimmen nicht überein.')
          : second
            ? t(
                'Mindestens 8 Zeichen. Das Passwort dieses Servers: Du brauchst es auf weiteren Geräten und zum Trennen.',
              )
            : fresh
              ? t('Mindestens 8 Zeichen. Es verlässt dieses Gerät nie.')
              : t('Deine Hosts und Keys kommen mit – nichts wird neu verschlüsselt.')}
      </p>
      {error && (
        <p className="field-error" role="alert">
          {error}
        </p>
      )}
      <div className="sync-actions">
        <Button data-secondary onClick={onBack} disabled={busy}>
          {t('Zurück')}
        </Button>
        <span className="spacer" />
        <Button type="submit" variant="primary" busy={busy} disabled={!ready}>
          {busy ? t('Verbinde…') : t('Verbinden')}
        </Button>
      </div>
    </form>
  );
}

/** Shown right after the account was made, and again when asked for. */
function RecoveryKit({
  kit,
  again = false,
  onDone,
}: {
  kit: Connected;
  again?: boolean;
  onDone: () => void;
}) {
  useLanguage();
  const [saved, setSaved] = useState(false);
  const [copied, setCopied] = useState(false);
  const text = [
    'UwUSSH Recovery-Kit',
    `Server: ${kit.serverUrl}`,
    kit.tlsFingerprint ? `Fingerprint: ${kit.tlsFingerprint}` : null,
    `Code: ${kit.recoveryCode}`,
  ]
    .filter(Boolean)
    .join('\n');
  return (
    <div className="sync-kit">
      <NyuScene name="keys" className="sync-scene" />
      <p className="setting-label">
        {again ? t('Dein Recovery-Kit') : t('Verbunden ✧ Jetzt das Recovery-Kit sichern')}
      </p>
      <p className="dialog-lead">
        {again
          ? t(
              'Dein Tresor braucht das Master-Passwort und diesen Code. Deine Geräte merken sich den Code – aber sind alle Geräte weg und der Code auch, sind die Daten weg.',
            )
          : t(
              'Dein Tresor braucht ab jetzt das Master-Passwort und diesen Code. Deine Geräte merken sich den Code – aber sind alle Geräte weg und der Code auch, sind die Daten weg. Später zeigt ihn jedes gekoppelte Gerät unter Sync → Recovery-Kit noch einmal.',
            )}
      </p>
      <Card className="sync-kit-card">
        <code className="sync-kit-code">{kit.recoveryCode}</code>
        <dl>
          <dt>{t('Server')}</dt>
          <dd>
            <code>{kit.serverUrl}</code>
          </dd>
          {kit.tlsFingerprint && (
            <>
              <dt>{t('Fingerprint')}</dt>
              <dd>
                <code>{kit.tlsFingerprint}</code>
              </dd>
            </>
          )}
        </dl>
        <Button
          size="sm"
          icon={copied ? ICONS.done : ICONS.copy}
          onClick={() => {
            void copy(text);
            setCopied(true);
          }}
        >
          {copied ? t('Kopiert') : t('Kopieren')}
        </Button>
      </Card>
      {/* Asked for again, the kit was saved once already: no gate. */}
      {!again && (
        <label className="check">
          <input type="checkbox" checked={saved} onChange={(e) => setSaved(e.target.checked)} />
          <span>
            <b>{t('Ich habe den Code sicher aufgeschrieben')}</b>
            <small>
              {t('Auf Papier oder im Passwort-Manager – nicht nur auf diesem Rechner.')}
            </small>
          </span>
        </label>
      )}
      <div className="sync-actions">
        <span className="spacer" />
        <Button variant="primary" disabled={!saved && !again} onClick={onDone}>
          {t('Fertig')}
        </Button>
      </div>
    </div>
  );
}

// ── Joining from another device ───────────────────────────────────────────

function JoinForm({
  status,
  onBack,
  onJoined,
}: {
  status: SyncStatus;
  onBack: () => void;
  onJoined: () => void;
}) {
  useLanguage();
  const { space, second } = useSpace();
  const [code, setCode] = useState('');
  const [server, setServer] = useState('');
  const [fingerprint, setFingerprint] = useState('');
  const [name, setName] = useState(status.deviceName);
  const [password, setPassword] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [unlocking, setUnlocking] = useState(false);

  const pasted = isPasteablePairing(code);
  const spoken = code.trim().length > 0 && !pasted;
  const ready =
    !busy &&
    code.trim().length > 0 &&
    (!spoken || server.trim().length > 0) &&
    name.trim().length > 0 &&
    password.length > 0;

  const submit = async () => {
    if (!ready) return;
    setBusy(true);
    setError(null);
    try {
      await syncJoin(
        code,
        password,
        name,
        spoken ? server : null,
        spoken && fingerprint.trim() ? fingerprint : null,
        space,
      );
      setPassword('');
      onJoined();
    } catch (e) {
      const failure = asSyncFailure(e);
      if (failure.kind === 'vault-locked') setUnlocking(true);
      else {
        setError(failureText(failure));
        setPassword('');
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      className="form sync-form"
      onSubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
      <p className="setting-label">{t('Mit einem Gerät koppeln')}</p>
      <p className="setting-description">
        {t(
          'Auf dem anderen Gerät: Einstellungen → Sync → Gerät hinzufügen. Den langen Code dort kopieren und hier einfügen – oder den kurzen Code abtippen.',
        )}
      </p>
      <label className="field">
        <span>{t('Code vom anderen Gerät')}</span>
        <textarea
          className="sync-code-input"
          value={code}
          rows={2}
          spellCheck={false}
          autoFocus
          placeholder="uwu2_…  ·  K7M4Q-tiger-radio-kiwi"
          onChange={(e) => setCode(e.target.value)}
        />
      </label>
      {spoken && (
        <>
          <label className="field">
            <span>{t('Server-Adresse')}</span>
            <input
              value={server}
              spellCheck={false}
              placeholder="https://sync.example.com:8443"
              onChange={(e) => setServer(e.target.value)}
            />
          </label>
          <label className="field">
            <span>{t('Fingerprint (steht beim anderen Gerät)')}</span>
            <input
              value={fingerprint}
              spellCheck={false}
              placeholder="SHA256:…"
              onChange={(e) => setFingerprint(e.target.value)}
            />
          </label>
        </>
      )}
      <label className="field">
        <span>{t('Name dieses Geräts')}</span>
        <input value={name} maxLength={64} onChange={(e) => setName(e.target.value)} />
      </label>
      <label className="field">
        <span>{t('Master-Passwort des Kontos')}</span>
        <input
          type="password"
          value={password}
          autoComplete="current-password"
          onChange={(e) => setPassword(e.target.value)}
        />
        <em className="field-hint">
          {second
            ? t(
                'Das Passwort, mit dem das erste Gerät diesen Server verbunden hat. Deine bisherigen Hosts bleiben auf dem ersten Server.',
              )
            : t(
                'Das Passwort, mit dem das erste Gerät den Server verbunden hat. Hosts, die schon hier sind, kommen mit.',
              )}
        </em>
      </label>
      {busy && (
        <p className="field-hint" role="status">
          {t('Warte auf das andere Gerät…')}
        </p>
      )}
      {error && (
        <p className="field-error" role="alert">
          {error}
        </p>
      )}
      <div className="sync-actions">
        <Button data-secondary onClick={onBack} disabled={busy}>
          {t('Zurück')}
        </Button>
        <span className="spacer" />
        <Button type="submit" variant="primary" busy={busy} disabled={!ready}>
          {busy ? t('Kopple…') : t('Koppeln')}
        </Button>
      </div>
      {unlocking && (
        <VaultDialog
          reason={t(
            'Zuerst den Tresor dieses Geräts öffnen: seine Passwörter und Keys ziehen mit in den Tresor des Kontos.',
          )}
          onDone={() => {
            setUnlocking(false);
            void submit();
          }}
          onCancel={() => setUnlocking(false)}
        />
      )}
    </form>
  );
}

// ── Paired ────────────────────────────────────────────────────────────────

function Paired({
  status,
  onChanged,
  onMoved,
}: {
  status: SyncStatus;
  onChanged: () => void;
  onMoved: (outcome: LockOutcome) => void;
}) {
  useLanguage();
  const { space, leave, after } = useSpace();
  const [moving, setMoving] = useState(false);
  const [forgettingMove, setForgettingMove] = useState(false);
  const [devices, setDevices] = useState<Device[] | null>(null);
  const [devicesError, setDevicesError] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [offer, setOffer] = useState<Offer | null>(null);
  const [joined, setJoined] = useState<string | null>(null);
  const [revoking, setRevoking] = useState<Device | null>(null);
  const [revoked, setRevoked] = useState<string | null>(null);
  const [leaving, setLeaving] = useState(false);
  const [unlocking, setUnlocking] = useState(false);
  const [askingKit, setAskingKit] = useState(false);
  const [kit, setKit] = useState<Connected | null>(null);
  // A move that began knows its server: said here when it has app sync off.
  const moveServerOff = useAppSyncOff(status.lock.moveStartedMs ? status.lock.serverUrl : null);

  const loadDevices = useCallback(() => {
    void syncDevices(space)
      .then((list) => {
        setDevices(list);
        setDevicesError(null);
      })
      .catch((e) => setDevicesError(failureText(asSyncFailure(e))));
  }, [space]);

  useEffect(() => {
    if (status.vault === 'unlocked') loadDevices();
  }, [loadDevices, status.vault]);

  const locked = status.vault !== 'unlocked';

  if (kit) return <RecoveryKit kit={kit} again onDone={() => setKit(null)} />;
  if (moving) {
    return (
      <LockSignInForm
        mode="move"
        status={status}
        onBack={() => setMoving(false)}
        onDone={(outcome) => {
          setMoving(false);
          onMoved(outcome);
        }}
      />
    );
  }

  return (
    <>
      <SyncRow
        label={t('Verbunden mit {server}', { server: status.serverUrl ?? '' })}
        dot={syncDot(status)}
        control={
          locked ? (
            <Button size="sm" icon={ICONS.unlocked} onClick={() => setUnlocking(true)}>
              {t('Entsperren')}
            </Button>
          ) : (
            <Button
              size="sm"
              icon={ICONS.refresh}
              onClick={() => {
                void syncNow(space).then(() => window.setTimeout(onChanged, 400));
              }}
              disabled={status.running}
            >
              {t('Jetzt synchronisieren')}
            </Button>
          )
        }
      >
        <PassDetails status={status} />
        {status.tlsFingerprint && (
          <p className="setting-description">
            {t('Gepinnt:')} <code className="sync-fingerprint">{status.tlsFingerprint}</code>
          </p>
        )}
      </SyncRow>

      <SyncRow
        label={t('Gerät hinzufügen')}
        control={
          <Button
            size="sm"
            icon={ICONS.add}
            onClick={() => {
              setJoined(null);
              setAdding(true);
            }}
            disabled={locked}
          >
            {t('Gerät hinzufügen…')}
          </Button>
        }
      >
        <p className="setting-description">
          {t(
            'Zeigt einen Code für zehn Minuten. Das neue Gerät gibt ihn unter Sync → Mit einem Gerät koppeln ein.',
          )}
        </p>
        {joined && (
          <p className="setting-result text-success-ink" role="status">
            {t('{name} ist beigetreten ✧', { name: joined })}
          </p>
        )}
      </SyncRow>

      <div className="sync-devices">
        <p className="setting-label">{t('Geräte')}</p>
        {devicesError && <p className="setting-description text-warning-ink">{devicesError}</p>}
        {locked && (
          <p className="setting-description">{t('Die Liste kommt, wenn der Tresor offen ist.')}</p>
        )}
        {devices && (
          <ul className="sync-device-list">
            {devices.map((device) => (
              <li key={device.id} data-revoked={device.revokedMs ? true : undefined}>
                <Icon icon={ICONS.computer} className="text-muted" />
                <span className="sync-device-text">
                  <b>
                    {device.name}
                    {device.current && <Tag tone="pink">{t('dieses Gerät')}</Tag>}
                  </b>
                  <small>
                    {device.revokedMs
                      ? t('Widerrufen {when}', { when: ago(device.revokedMs) })
                      : device.lastSeenMs
                        ? t('Zuletzt gesehen {when}', { when: ago(device.lastSeenMs) })
                        : t('Gekoppelt {when}', { when: ago(device.createdMs) })}
                  </small>
                </span>
                <span className="spacer" />
                {!device.current && !device.revokedMs && (
                  <Button size="sm" variant="ghost" onClick={() => setRevoking(device)}>
                    {t('Widerrufen…')}
                  </Button>
                )}
              </li>
            ))}
          </ul>
        )}
        {revoked && (
          <Hint tone="warning" className="sync-revoked" role="status">
            {t(
              '{name} kommt nicht mehr an den Server. Was es schon heruntergeladen hat, kennt es aber weiter: ändere das Master-Passwort und die Passwörter wichtiger Hosts, wenn das Gerät in falsche Hände geraten ist.',
              { name: revoked },
            )}
          </Hint>
        )}
      </div>

      <SyncRow
        label={t('Recovery-Kit')}
        control={
          <Button size="sm" icon={ICONS.secret} onClick={() => setAskingKit(true)}>
            {t('Anzeigen…')}
          </Button>
        }
      >
        <p className="setting-description">
          {t(
            'Der Code, den dein Tresor neben dem Master-Passwort braucht. Dieses Gerät zeigt ihn nach dem Master-Passwort noch einmal.',
          )}
        </p>
      </SyncRow>

      <SyncRow
        label={t('Zu UwULock umziehen')}
        control={
          <>
            {status.lock.moveStartedMs && (
              <Button
                size="sm"
                variant="ghost"
                disabled={forgettingMove}
                onClick={() => {
                  setForgettingMove(true);
                  void lockForgetMove(space).finally(() => {
                    setForgettingMove(false);
                    onChanged();
                  });
                }}
              >
                {t('Verwerfen')}
              </Button>
            )}
            <Button
              size="sm"
              icon={ICONS.export}
              onClick={() => setMoving(true)}
              disabled={locked || moveServerOff}
              title={moveServerOff ? APP_SYNC_OFF() : undefined}
            >
              {status.lock.moveStartedMs ? t('Fortsetzen…') : t('Umziehen…')}
            </Button>
          </>
        }
      >
        <p className="setting-description">
          {moveServerOff
            ? APP_SYNC_OFF()
            : status.lock.moveStartedMs
              ? t(
                  'Ein Umzug hat {when} begonnen und ist nicht fertig geworden. Bis er fertig ist, synchronisiert dieses Gerät weiter über UwUSync.',
                  { when: ago(status.lock.moveStartedMs) },
                )
              : t(
                  'Hosts, Keys und Passwörter ziehen mit einem Klick auf einen UwULock-Server um. UwUSSH prüft die Kopie, bevor es wechselt – auf UwUSync bleibt alles, wie es ist.',
                )}
        </p>
      </SyncRow>

      <SyncRow
        label={t('Trennen')}
        control={
          <Button
            size="sm"
            variant="danger"
            icon={ICONS.disconnect}
            onClick={() => (leave ? leave() : setLeaving(true))}
          >
            {t('Trennen…')}
          </Button>
        }
      >
        <p className="setting-description">
          {leave
            ? t(
                'Dieses Gerät synchronisiert nicht mehr mit diesem Server. Du entscheidest, ob seine Einträge hier bleiben.',
              )
            : t(
                'Dieses Gerät synchronisiert nicht mehr. Hosts und Tresor bleiben hier, das Master-Passwort allein öffnet ihn wieder.',
              )}
        </p>
      </SyncRow>

      {after}

      {adding && (
        <PasswordConfirm
          title={t('Gerät hinzufügen')}
          lead={t(
            'Das neue Gerät bekommt den Schlüssel zu deinem Tresor. Darum braucht es hier zuerst das Master-Passwort.',
          )}
          action={t('Code zeigen')}
          tone="normal"
          run={async (password) => setOffer(await syncOffer(password, space))}
          onCancel={() => setAdding(false)}
          onDone={() => setAdding(false)}
        />
      )}
      {offer && (
        <AddDevice
          offer={offer}
          onClose={(name) => {
            setOffer(null);
            if (name) {
              setJoined(name);
              loadDevices();
            }
            onChanged();
          }}
        />
      )}
      {revoking && (
        <PasswordConfirm
          title={t('{name} widerrufen?', { name: revoking.name })}
          lead={t(
            'Zum Widerrufen eines anderen Geräts braucht es das Master-Passwort – so kann ein gestohlenes Gerät dich nicht aussperren. Das Gerät bekommt danach nichts Neues mehr, aber was es schon hat, bleibt dort lesbar: Ändere Passwörter und Schlüssel, die darauf lagen, wenn du ihm nicht mehr traust.',
          )}
          action={t('Widerrufen')}
          run={(password) => syncRevoke(revoking.id, password, space)}
          onCancel={() => setRevoking(null)}
          onDone={() => {
            setRevoked(revoking.name);
            setRevoking(null);
            loadDevices();
          }}
        />
      )}
      {leaving && (
        <PasswordConfirm
          title={t('Vom Server trennen?')}
          lead={t(
            'Der Tresor wird wieder nur mit dem Master-Passwort verschlüsselt. Die anderen Geräte synchronisieren weiter.',
          )}
          action={t('Trennen')}
          run={(password) => syncDisconnect(password, space)}
          onCancel={() => setLeaving(false)}
          onDone={() => {
            setLeaving(false);
            onChanged();
          }}
        >
          <TeamsGoNote server={space} />
        </PasswordConfirm>
      )}
      {askingKit && (
        <PasswordConfirm
          title={t('Recovery-Kit anzeigen')}
          lead={t(
            'Der Code öffnet zusammen mit dem Master-Passwort deinen Tresor auf jedem Gerät. Zeig ihn niemandem und schreib ihn nur dorthin, wo er sicher ist.',
          )}
          action={t('Anzeigen')}
          tone="normal"
          run={async (password) => {
            const shown = await syncRecoveryCode(password, space);
            setKit(shown);
          }}
          onCancel={() => setAskingKit(false)}
          onDone={() => setAskingKit(false)}
        />
      )}
      {unlocking && (
        <VaultDialog
          reason={t('Der Sync braucht den offenen Tresor, um zu ver- und entschlüsseln.')}
          onDone={() => {
            setUnlocking(false);
            onChanged();
            void syncNow(space);
          }}
          onCancel={() => setUnlocking(false)}
        />
      )}
    </>
  );
}

/** The code to read out or paste, while this device waits for the other one. */
function AddDevice({ offer, onClose }: { offer: Offer; onClose: (joined: string | null) => void }) {
  useLanguage();
  const { space } = useSpace();
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [now, setNow] = useState(Date.now());
  const closed = useRef(false);
  const mounted = useRef(true);
  const waiting = useRef(false);

  useEffect(() => {
    mounted.current = true;
    // Once per code, however often React runs this: a second wait on the
    // same code would garble the handshake for the other device.
    if (!waiting.current) {
      waiting.current = true;
      void (async () => {
        try {
          const joined = await syncWaitForDevice(space);
          if (mounted.current && !closed.current) onClose(joined.name);
        } catch (e) {
          if (mounted.current && !closed.current) setError(failureText(asSyncFailure(e)));
        }
      })();
    }
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => {
      mounted.current = false;
      window.clearInterval(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const cancel = () => {
    closed.current = true;
    void syncCancelOffer(space).catch(() => undefined);
    onClose(null);
  };

  const left = Math.max(0, Math.round((offer.expiresMs - now) / 1000));
  const minutes = Math.floor(left / 60);
  const seconds = String(left % 60).padStart(2, '0');

  return (
    <Modal
      title={t('Gerät hinzufügen')}
      onCancel={cancel}
      footer={
        <Button data-secondary onClick={cancel}>
          {error ? t('Schließen') : t('Abbrechen')}
        </Button>
      }
    >
      <NyuScene name="connecting" className="dialog-scene" />
      <p className="dialog-lead">
        {t('Auf dem neuen Gerät: Einstellungen → Sync → Mit einem Gerät koppeln.')}
      </p>
      <Card className="sync-offer gap-1 p-4">
        <span className="setting-description">{t('Zum Abtippen')}</span>
        <code className="sync-kit-code">{offer.spoken}</code>
        <span className="setting-description">
          {t('Dazu die Adresse {server}', { server: offer.serverUrl })}
          {offer.tlsFingerprint && (
            <>
              {' · '}
              <code className="sync-fingerprint">{offer.tlsFingerprint}</code>
            </>
          )}
        </span>
      </Card>
      <Button
        className="justify-self-start"
        icon={copied ? ICONS.done : ICONS.copy}
        onClick={() => {
          void copy(offer.pasteable);
          setCopied(true);
        }}
      >
        {copied ? t('Langen Code kopiert') : t('Langen Code zum Einfügen kopieren')}
      </Button>
      {!error && (
        <p className="field-hint" role="status">
          {t('Warte auf das andere Gerät… noch {m}:{s}', { m: minutes, s: seconds })}
        </p>
      )}
      {error && (
        <p className="field-error" role="alert">
          {error}
        </p>
      )}
    </Modal>
  );
}
