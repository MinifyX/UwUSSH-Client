import {
  Button,
  Icon,
  IconButton,
  ICONS,
  StatusDot,
  StatusLine,
  type StatusState,
} from '@uwusuite/design';
import { listen } from '@tauri-apps/api/event';
import { useEffect, useMemo, useState, type KeyboardEvent, type MouseEvent } from 'react';
import { matchQuickConnect, parseQuickConnect, recentHosts, type QuickTarget } from '../lib/home';
import { t, useLanguage } from '../lib/i18n';
import { vaultState, type GroupRecord, type HostRecord, type Workspace } from '../lib/session';
import { useSettings } from '../lib/settings';
import { syncStatus, type SyncStatus } from '../lib/sync';
import type { Tab } from '../lib/tabs';
import type { TunnelStatus } from '../lib/tunnels';
import { ContextMenu, type MenuItem } from './ContextMenu';
import { shownSections } from './HostList';
import { NyuScene } from './nyu/scenes';
import { OsIcon } from './OsIcon';
import { ago, syncDot } from './SyncParts';

type Props = {
  hosts: HostRecord[];
  groups: GroupRecord[];
  tabs: Tab[];
  onlineIds: ReadonlySet<string>;
  connectingIds: ReadonlySet<string>;
  /** Hosts with a terminal tab open. */
  openIds: ReadonlySet<string>;
  tunnels: ReadonlyMap<string, TunnelStatus>;
  onConnect: (host: HostRecord) => void;
  onConnectAnother: (host: HostRecord) => void;
  onOpenFiles: (host: HostRecord) => void;
  onTunnels: (host: HostRecord | null) => void;
  onEdit: (host: HostRecord) => void;
  onNewHost: (workspace: Workspace, draft?: QuickTarget) => void;
  onImport: () => void;
  onSelectTab: (id: string) => void;
  onUnlock: () => void;
  onSyncSettings: () => void;
};

/**
 * The overview, the first tab that never closes: every host as a card, a
 * field to connect by typing, what needs attention, and on wide windows the
 * open sessions, running tunnels and sync on the right.
 */
// With few hosts the group cards below already show every one of them.
const RECENT_MIN_HOSTS = 8;

export function Home(props: Props) {
  useLanguage();
  const { hosts, groups, tabs, onlineIds, connectingIds, openIds } = props;
  const settings = useSettings();
  const workspace: Workspace = settings.workspaces ? settings.activeWorkspace : 'private';
  const [quick, setQuick] = useState('');
  const [quickError, setQuickError] = useState(false);
  const [menu, setMenu] = useState<{ x: number; y: number; items: MenuItem[] } | null>(null);
  const [vault, setVault] = useState<string | null>(null);
  const [sync, setSync] = useState<SyncStatus | null>(null);

  // What the warnings say comes from the app's own state, read again whenever sync reports.
  useEffect(() => {
    let gone = false;
    const load = () => {
      void vaultState()
        .then((state) => !gone && setVault(state.status))
        .catch(() => undefined);
      void syncStatus()
        .then((status) => !gone && setSync(status))
        .catch(() => undefined);
    };
    load();
    const stop = listen('sync:changed', load);
    window.addEventListener('focus', load);
    return () => {
      gone = true;
      void stop.then((unlisten) => unlisten()).catch(() => undefined);
      window.removeEventListener('focus', load);
    };
  }, [hosts]);

  const sections = useMemo(
    () => shownSections(hosts, groups, settings.workspaces, workspace),
    [hosts, groups, settings.workspaces, workspace],
  );
  const recent = useMemo(() => {
    const own = hosts.filter((host) => !settings.workspaces || host.workspace === workspace);
    return own.length > RECENT_MIN_HOSTS ? recentHosts(own) : [];
  }, [hosts, settings.workspaces, workspace]);
  const failed = tabs.filter((tab) => tab.kind === 'ssh' && tab.status === 'failed');
  const running = [...props.tunnels.values()].filter((status) => status.state === 'running');
  const syncTrouble =
    sync !== null &&
    sync.backend !== 'none' &&
    (Boolean(sync.last?.error) || sync.withheld.records > 0 || sync.lock.needsSignIn);

  const submitQuick = () => {
    const target = parseQuickConnect(quick);
    if (!target) {
      setQuickError(quick.trim() !== '');
      return;
    }
    setQuickError(false);
    const host = matchQuickConnect(hosts, target);
    setQuick('');
    // A connection needs a stored host: an unknown address becomes one first.
    if (host) props.onConnect(host);
    else props.onNewHost(workspace, target);
  };

  const hostState = (host: HostRecord): { state: StatusState; label: string } =>
    onlineIds.has(host.id)
      ? { state: 'online', label: t('verbunden') }
      : connectingIds.has(host.id)
        ? { state: 'connecting', label: t('verbindet…') }
        : { state: 'offline', label: t('nicht verbunden') };

  const cardMenu = (x: number, y: number, host: HostRecord) =>
    setMenu({
      x,
      y,
      items: [
        openIds.has(host.id)
          ? {
              label: t('Zum offenen Tab'),
              icon: ICONS.terminal,
              onSelect: () => props.onConnect(host),
            }
          : { label: t('Verbinden'), icon: ICONS.terminal, onSelect: () => props.onConnect(host) },
        ...(openIds.has(host.id)
          ? [
              {
                label: t('Weiteren Tab öffnen'),
                icon: ICONS.add,
                onSelect: () => props.onConnectAnother(host),
              },
            ]
          : []),
        {
          label: t('Dateien als Tab öffnen'),
          icon: ICONS.files,
          onSelect: () => props.onOpenFiles(host),
        },
        { label: t('Tunnel…'), icon: ICONS.tunnel, onSelect: () => props.onTunnels(host) },
        { label: t('Bearbeiten'), icon: ICONS.edit, onSelect: () => props.onEdit(host) },
      ],
    });

  const card = (host: HostRecord, where: string) => {
    const state = hostState(host);
    const open = openIds.has(host.id);
    return (
      <li
        key={`${where}-${host.id}`}
        className="host-card"
        onContextMenu={(event: MouseEvent) => {
          event.preventDefault();
          cardMenu(event.clientX, event.clientY, host);
        }}
        onKeyDown={(event: KeyboardEvent<HTMLLIElement>) => {
          if (event.key === 'ContextMenu' || (event.shiftKey && event.key === 'F10')) {
            event.preventDefault();
            const rect = event.currentTarget.getBoundingClientRect();
            cardMenu(rect.left + 24, rect.top + 40, host);
          }
        }}
      >
        <div className="host-card-head">
          <OsIcon os={host.os} size={22} />
          <span className="host-card-name">{host.name}</span>
        </div>
        <span className="host-card-login">
          {host.username}@{host.address}
          {host.port !== 22 ? `:${host.port}` : ''}
        </span>
        <StatusLine state={state.state}>{state.label}</StatusLine>
        <div className="host-card-actions">
          <IconButton
            size="sm"
            icon={ICONS.files}
            label={t('Dateien von {name} als Tab öffnen', { name: host.name })}
            onClick={() => props.onOpenFiles(host)}
          />
          <span className="spacer" />
          <Button
            size="sm"
            // Pink is for what is selected: a wall of pink buttons would drown it.
            variant="secondary"
            icon={open ? ICONS.terminal : ICONS.connect}
            onClick={() => props.onConnect(host)}
            aria-label={
              open
                ? t('Zum Tab von {name}', { name: host.name })
                : t('Mit {name} verbinden', { name: host.name })
            }
          >
            {open ? t('Zum Tab') : t('Verbinden')}
          </Button>
        </div>
      </li>
    );
  };

  const shownHosts = sections.reduce((sum, section) => sum + section.hosts.length, 0);

  return (
    <div id="pane-home" className="home" role="tabpanel" aria-label={t('Übersicht')}>
      <div className="home-main">
        <header className="home-head">
          <div className="home-title">
            <h1>{t('Hosts')}</h1>
            <p className="home-count">
              {shownHosts === 1 ? t('1 Host') : t('{n} Hosts', { n: shownHosts })}
              {' · '}
              {t('{n} offen', { n: openIds.size })}
            </p>
          </div>
          <span className="spacer" />
          <Button variant="primary" icon={ICONS.add} onClick={() => props.onNewHost(workspace)}>
            {t('Neuer Host')}
          </Button>
        </header>

        <form
          className="home-quick"
          onSubmit={(event) => {
            event.preventDefault();
            submitQuick();
          }}
        >
          <Icon icon={ICONS.connect} />
          <input
            type="text"
            value={quick}
            onChange={(event) => {
              setQuick(event.target.value);
              setQuickError(false);
            }}
            placeholder={t('user@host:port oder Name eines Hosts')}
            aria-label={t('Schnell verbinden')}
            aria-invalid={quickError || undefined}
            aria-describedby="home-quick-hint"
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
          />
        </form>
        <p id="home-quick-hint" className="home-quick-hint" data-error={quickError || undefined}>
          {quickError
            ? t('Das ist keine Adresse: user@host, host:port oder der Name eines Hosts.')
            : t(
                'Enter verbindet einen gespeicherten Host; eine neue Adresse wird erst als Host angelegt.',
              )}
        </p>

        {(vault === 'locked' || syncTrouble || failed.length > 0) && (
          <ul className="home-warnings">
            {vault === 'locked' && (
              <li>
                <Icon icon={ICONS.locked} size="sm" />
                <span>
                  {t('Der Tresor ist gesperrt: gespeicherte Passwörter und Keys fehlen.')}
                </span>
                <Button size="sm" variant="ghost" onClick={props.onUnlock}>
                  {t('Entsperren')}
                </Button>
              </li>
            )}
            {syncTrouble && (
              <li>
                <Icon icon={ICONS.sync} size="sm" />
                <span>{t('Sync braucht dich.')}</span>
                <Button size="sm" variant="ghost" onClick={props.onSyncSettings}>
                  {t('Ansehen')}
                </Button>
              </li>
            )}
            {failed.map((tab) => (
              <li key={tab.id}>
                <Icon icon={ICONS.warning} size="sm" />
                <span>{t('{name}: nicht verbunden', { name: tab.title })}</span>
                <Button size="sm" variant="ghost" onClick={() => props.onSelectTab(tab.id)}>
                  {t('Zum Tab')}
                </Button>
              </li>
            ))}
          </ul>
        )}

        {hosts.length === 0 ? (
          <div className="home-empty">
            <NyuScene name="pick" className="home-empty-scene" />
            <p className="home-empty-title">{t('Noch keine Hosts')}</p>
            <p className="home-empty-text">
              {t(
                'Leg deinen ersten Server an oder hol deine Hosts aus PuTTY, Termius oder ~/.ssh/config.',
              )}
            </p>
            <Button icon={ICONS.import} onClick={props.onImport}>
              {t('Importieren')}
            </Button>
          </div>
        ) : (
          <>
            {recent.length > 0 && (
              <section className="home-section" aria-labelledby="home-recent">
                <h2 id="home-recent">{t('Zuletzt')}</h2>
                <ul className="home-grid">{recent.map((host) => card(host, 'recent'))}</ul>
              </section>
            )}
            {sections
              .filter((section) => section.hosts.length > 0)
              .map((section) => (
                <section
                  key={section.name ?? ''}
                  className="home-section"
                  aria-label={section.name ?? t('Ohne Gruppe')}
                >
                  <h2>{section.name ?? t('Ohne Gruppe')}</h2>
                  <ul className="home-grid">
                    {section.hosts.map((host) => card(host, section.name ?? ''))}
                  </ul>
                </section>
              ))}
          </>
        )}
      </div>

      <aside className="home-aside" aria-label={t('Überblick')}>
        <section className="home-section" aria-labelledby="home-sessions">
          <h2 id="home-sessions">{t('Offene Sitzungen')}</h2>
          {tabs.length === 0 ? (
            <p className="home-muted">{t('Keine.')}</p>
          ) : (
            <ul className="home-list">
              {tabs.map((tab) => (
                <li key={tab.id}>
                  <button type="button" onClick={() => props.onSelectTab(tab.id)}>
                    <StatusDot state={tabDot(tab)} />
                    <span className="home-list-name">
                      {tab.ordinal > 1 ? `${tab.title} ${tab.ordinal}` : tab.title}
                    </span>
                    <span className="home-muted">{tabLabel(tab)}</span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </section>
        <section className="home-section" aria-labelledby="home-tunnels">
          <h2 id="home-tunnels">{t('Tunnel')}</h2>
          {running.length === 0 ? (
            <p className="home-muted">{t('Keiner läuft.')}</p>
          ) : (
            <ul className="home-list">
              {running.map((status) => (
                <li key={status.id}>
                  <span className="home-list-row">
                    <StatusDot state="online" />
                    <span className="home-list-name">
                      {hosts.find((host) => host.id === status.hostId)?.name ?? status.hostId}
                    </span>
                    <span className="home-muted">
                      {status.state === 'running' &&
                        t('läuft auf Port {port}', { port: status.boundPort })}
                    </span>
                  </span>
                </li>
              ))}
            </ul>
          )}
          <Button size="sm" icon={ICONS.tunnel} onClick={() => props.onTunnels(null)}>
            {t('Tunnel verwalten')}
          </Button>
        </section>
        {sync && sync.backend !== 'none' && (
          <section className="home-section" aria-labelledby="home-sync">
            <h2 id="home-sync">{t('Sync')}</h2>
            <StatusLine state={syncDot(sync)}>
              {syncTrouble
                ? t('braucht dich')
                : sync.running
                  ? t('gleicht ab…')
                  : sync.lastSyncMs
                    ? t('abgeglichen {when}', { when: ago(sync.lastSyncMs) })
                    : t('noch nicht abgeglichen')}
            </StatusLine>
          </section>
        )}
      </aside>

      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          items={menu.items}
          label={t('Aktionen')}
          onClose={() => setMenu(null)}
        />
      )}
    </div>
  );
}

export function tabDot(tab: Tab): StatusState {
  if (tab.status === 'live') return 'online';
  if (tab.status === 'connecting') return 'connecting';
  return tab.status === 'failed' ? 'error' : 'offline';
}

function tabLabel(tab: Tab): string {
  if (tab.status === 'live') return tab.kind === 'shell' ? t('läuft') : t('verbunden');
  if (tab.status === 'connecting') return t('verbindet…');
  return tab.status === 'failed' ? t('nicht verbunden') : t('getrennt');
}
