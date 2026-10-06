import { Button, Icon, IconButton, ICONS } from '@uwusuite/design';
import { useMemo, useState, type KeyboardEvent as ReactKeyboardEvent } from 'react';
import { beginDrag, edgeByHalf, type DropTarget } from '../lib/dnd';
import { localShellAvailable } from '../lib/flavor';
import { t } from '../lib/i18n';
import {
  createGroup,
  deleteGroup,
  moveGroup,
  moveHost,
  renameGroup,
  type GroupRecord,
  type HostRecord,
  type Workspace,
} from '../lib/session';
import {
  passwordLoginWarningOn,
  updateSettings,
  useSettings,
  workspaceName,
} from '../lib/settings';
import { ContextMenu, type MenuItem } from './ContextMenu';
import { Nyu } from './nyu/Nyu';
import { OsIcon } from './OsIcon';
import { Tooltip } from './Tooltip';

type Props = {
  hosts: HostRecord[];
  groups: GroupRecord[];
  /** `'shell'` for the local shell, a host id, or nothing: what the active tab shows. */
  activeId: string | null;
  /** Hosts with at least one live session in some tab. */
  onlineIds: ReadonlySet<string>;
  /** Hosts a tab is connecting to right now. */
  connectingIds: ReadonlySet<string>;
  /** Hosts with a terminal tab open, live or not. */
  openIds: ReadonlySet<string>;
  /** A local shell tab is open. */
  shellOpen: boolean;
  /** Hosts with a tunnel running right now. */
  tunnelIds: ReadonlySet<string>;
  /** Opens the tunnels: of one host, or of all of them. */
  onTunnels: (host: HostRecord | null) => void;
  /** Brings the host's open tab to the front, or opens one. */
  onConnect: (host: HostRecord) => void;
  /** Always a new tab, next to the ones already open. */
  onConnectAnother: (host: HostRecord) => void;
  onOpenFiles: (host: HostRecord) => void;
  /** Brings the open local shell to the front, or opens one. */
  onLocalShell: () => void;
  onAnotherShell: () => void;
  onAdd: (workspace: Workspace, group: string | null) => void;
  onEdit: (host: HostRecord) => void;
  onImport: () => void;
  /** Something moved or changed: load hosts and groups again. */
  onChanged: () => void;
  onError: (text: string) => void;
};

const WORKSPACES: Workspace[] = ['private', 'business'];

type Section = { name: string | null; hosts: HostRecord[] };

function byPlace(a: HostRecord, b: HostRecord) {
  return a.position - b.position || a.name.localeCompare(b.name, 'de');
}

/** The ungrouped hosts, then every group in its order, for one workspace. */
function sectionsOf(hosts: HostRecord[], groups: GroupRecord[], workspace: Workspace): Section[] {
  const mine = hosts.filter((host) => host.workspace === workspace);
  const names = [...new Set(groups.filter((g) => g.workspace === workspace).map((g) => g.name))];
  for (const host of mine) {
    if (host.groupPath && !names.includes(host.groupPath)) names.push(host.groupPath);
  }
  return [
    { name: null, hosts: mine.filter((h) => !h.groupPath).sort(byPlace) },
    ...names.map((name) => ({
      name,
      hosts: mine.filter((h) => h.groupPath === name).sort(byPlace),
    })),
  ];
}

function matches(host: HostRecord, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return [host.name, host.address, host.username, host.groupPath ?? '', host.os ?? ''].some(
    (field) => field.toLowerCase().includes(q),
  );
}

function errorText(error: unknown): string {
  if (typeof error === 'object' && error !== null && 'kind' in error) {
    const failure = error as { kind: string; problem?: string; message?: string };
    if (failure.problem === 'exists') return t('Eine Gruppe mit diesem Namen gibt es schon.');
    if (failure.problem === 'required') return t('Die Gruppe braucht einen Namen.');
    if (failure.message) return failure.message;
  }
  return String(error);
}

export function HostList(props: Props) {
  const { hosts, groups, activeId, onlineIds, connectingIds, openIds } = props;
  const settings = useSettings();
  const workspace: Workspace = settings.workspaces ? settings.activeWorkspace : 'private';
  const [query, setQuery] = useState('');
  const [menu, setMenu] = useState<{ x: number; y: number; items: MenuItem[] } | null>(null);
  const [editing, setEditing] = useState<{ from: string | null; value: string } | null>(null);
  const [dragging, setDragging] = useState(false);

  // With workspaces off, everything shows as one list.
  const sections = useMemo(
    () =>
      settings.workspaces
        ? sectionsOf(hosts, groups, workspace)
        : sectionsOf(
            hosts.map((host) => ({ ...host, workspace: 'private' as const })),
            groups.map((group) => ({ ...group, workspace: 'private' as const })),
            'private',
          ),
    [hosts, groups, workspace, settings.workspaces],
  );
  const found = useMemo(
    () => (query.trim() ? hosts.filter((host) => matches(host, query)) : []),
    [hosts, query],
  );
  const counts = useMemo(() => {
    const count: Record<Workspace, number> = { private: 0, business: 0 };
    for (const host of hosts) count[host.workspace] += 1;
    return count;
  }, [hosts]);

  const collapsed = (name: string) => settings.collapsedGroups.includes(`${workspace}/${name}`);
  const toggle = (name: string) => {
    const key = `${workspace}/${name}`;
    updateSettings({
      collapsedGroups: collapsed(name)
        ? settings.collapsedGroups.filter((g) => g !== key)
        : [...settings.collapsedGroups, key],
    });
  };

  const run = (action: () => Promise<unknown>) =>
    void action()
      .then(props.onChanged)
      .catch((error) => props.onError(errorText(error)));

  // ── Dragging ──────────────────────────────────────────────────────────────

  const dragHost = (event: React.PointerEvent, host: HostRecord) => {
    beginDrag(event, {
      label: host.name,
      onStart: () => setDragging(true),
      onEnd: () => setDragging(false),
      accept: (element, y) => {
        const kind = element.dataset.drop;
        if (kind === 'host')
          return element.dataset.hostId === host.id ? null : edgeByHalf(element, y);
        if (kind === 'group' || kind === 'workspace') return 'inside';
        return null;
      },
      onDrop: (target: DropTarget) => {
        const data = target.data;
        const to = settings.workspaces
          ? ((data.workspace as Workspace) ?? workspace)
          : host.workspace;
        if (data.drop === 'workspace') {
          if (to === host.workspace) return;
          run(() => moveHost(host.id, to, host.groupPath, null));
          return;
        }
        const group = data.group ? data.group : null;
        let before: string | null = null;
        if (data.drop === 'host') {
          before = target.edge === 'before' ? (data.hostId ?? null) : data.next || null;
          if (before === host.id) before = null;
        }
        run(() => moveHost(host.id, to, group, before));
      },
    });
  };

  const dragGroup = (event: React.PointerEvent, name: string) => {
    beginDrag(event, {
      label: name,
      onStart: () => setDragging(true),
      onEnd: () => setDragging(false),
      accept: (element, y) => {
        const kind = element.dataset.drop;
        if (kind === 'group' && element.dataset.head === 'true' && element.dataset.group) {
          return element.dataset.group === name ? null : edgeByHalf(element, y);
        }
        if (kind === 'workspace' && element.dataset.workspace !== workspace) return 'inside';
        return null;
      },
      onDrop: (target) => {
        const data = target.data;
        const to = (data.workspace as Workspace) ?? workspace;
        if (data.drop === 'workspace') {
          run(() => moveGroup(workspace, name, to, null));
          return;
        }
        const before = target.edge === 'before' ? (data.group ?? null) : data.nextGroup || null;
        run(() => moveGroup(workspace, name, to, before === name ? null : before));
      },
    });
  };

  // ── Menus ─────────────────────────────────────────────────────────────────

  const other: Workspace = workspace === 'private' ? 'business' : 'private';

  const hostMenu = (x: number, y: number, host: HostRecord) =>
    setMenu({
      x,
      y,
      items: [
        ...(openIds.has(host.id)
          ? [
              {
                label: t('Zum offenen Tab'),
                icon: ICONS.terminal,
                onSelect: () => props.onConnect(host),
              },
              {
                label: t('Weiteren Tab öffnen'),
                icon: ICONS.add,
                onSelect: () => props.onConnectAnother(host),
              },
            ]
          : [
              {
                label: t('Verbinden'),
                icon: ICONS.terminal,
                onSelect: () => props.onConnect(host),
              },
            ]),
        { label: t('Dateien öffnen'), icon: ICONS.files, onSelect: () => props.onOpenFiles(host) },
        { label: t('Tunnel…'), icon: ICONS.tunnel, onSelect: () => props.onTunnels(host) },
        { label: t('Bearbeiten'), icon: ICONS.edit, onSelect: () => props.onEdit(host) },
        'separator',
        ...(settings.workspaces
          ? [
              {
                label: t('Nach {workspace} verschieben', {
                  workspace: workspaceName(other, settings),
                }),
                icon: other === 'private' ? ICONS.home : ICONS.work,
                onSelect: () => run(() => moveHost(host.id, other, host.groupPath, null)),
              },
            ]
          : []),
        ...(host.groupPath
          ? [
              {
                label: t('Aus der Gruppe nehmen'),
                icon: ICONS.parentFolder,
                onSelect: () => run(() => moveHost(host.id, host.workspace, null, null)),
              },
            ]
          : []),
      ],
    });

  const shellMenu = (x: number, y: number) =>
    setMenu({
      x,
      y,
      items: props.shellOpen
        ? [
            { label: t('Zum offenen Tab'), icon: ICONS.terminal, onSelect: props.onLocalShell },
            {
              label: t('Weitere lokale Shell öffnen'),
              icon: ICONS.add,
              onSelect: props.onAnotherShell,
            },
          ]
        : [{ label: t('Lokale Shell öffnen'), icon: ICONS.terminal, onSelect: props.onLocalShell }],
    });

  const groupMenu = (x: number, y: number, name: string) =>
    setMenu({
      x,
      y,
      items: [
        {
          label: t('Host hier hinzufügen'),
          icon: ICONS.add,
          onSelect: () => props.onAdd(workspace, name),
        },
        {
          label: t('Umbenennen'),
          icon: ICONS.edit,
          onSelect: () => setEditing({ from: name, value: name }),
        },
        ...(settings.workspaces
          ? [
              {
                label: t('Nach {workspace} verschieben', {
                  workspace: workspaceName(other, settings),
                }),
                icon: other === 'private' ? ICONS.home : ICONS.work,
                onSelect: () => run(() => moveGroup(workspace, name, other, null)),
              },
            ]
          : []),
        'separator',
        {
          label: t('Gruppe auflösen (Hosts bleiben)'),
          icon: ICONS.delete,
          danger: true,
          onSelect: () => run(() => deleteGroup(workspace, name)),
        },
      ],
    });

  const menuKey = (event: ReactKeyboardEvent, open: (x: number, y: number) => void) => {
    if (event.key === 'ContextMenu' || (event.shiftKey && event.key === 'F10')) {
      event.preventDefault();
      const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
      open(rect.left + 24, rect.bottom);
    }
  };

  const saveGroupName = () => {
    if (!editing) return;
    const value = editing.value.trim();
    const from = editing.from;
    setEditing(null);
    if (!value || value === from) return;
    run(() =>
      from === null ? createGroup(workspace, value) : renameGroup(workspace, from, value),
    );
  };

  // ── Rendering ─────────────────────────────────────────────────────────────

  const warnPassword = passwordLoginWarningOn(settings);

  const hostRow = (host: HostRecord, next: HostRecord | undefined, draggable: boolean) => {
    const online = onlineIds.has(host.id);
    const connecting = connectingIds.has(host.id);
    const tunnel = props.tunnelIds.has(host.id);
    const password = warnPassword && host.auth === 'password';
    return (
      <li
        key={host.id}
        className="host-row"
        data-drop={draggable ? 'host' : undefined}
        data-host-id={host.id}
        data-next={next?.id ?? ''}
        data-workspace={host.workspace}
        data-group={host.groupPath ?? ''}
      >
        <button
          className="host"
          aria-current={activeId === host.id}
          aria-busy={connecting}
          onClick={() => props.onConnect(host)}
          onPointerDown={draggable ? (event) => dragHost(event, host) : undefined}
          onContextMenu={(event) => {
            event.preventDefault();
            hostMenu(event.clientX, event.clientY, host);
          }}
          onKeyDown={(event) => menuKey(event, (x, y) => hostMenu(x, y, host))}
          title={
            openIds.has(host.id)
              ? t(
                  '{user}@{address}:{port} · zeigt den offenen Tab, Rechtsklick für einen weiteren',
                  {
                    user: host.username,
                    address: host.address,
                    port: host.port,
                  },
                )
              : t('{user}@{address}:{port} · öffnet einen neuen Tab', {
                  user: host.username,
                  address: host.address,
                  port: host.port,
                })
          }
        >
          <span className="host-icon">
            <OsIcon os={host.os} size={20} title="" />
            <i
              className="dot"
              data-state={online ? 'online' : connecting ? 'connecting' : 'idle'}
            />
          </span>
          <span className="host-text">
            <span className="host-line">
              <span className="host-name">{host.name}</span>
              {tunnel && (
                <span
                  className="host-badge"
                  data-kind="tunnel"
                  role="img"
                  aria-label={t('Ein Tunnel läuft')}
                >
                  <Icon icon={ICONS.tunnel} size="xs" />
                </span>
              )}
              {password && (
                <Tooltip className="host-badge" content={<PasswordWarning />}>
                  <span
                    data-kind="password"
                    role="img"
                    aria-label={t('Meldet sich mit Passwort statt mit einem SSH-Key an')}
                  >
                    <Icon icon={ICONS.warning} size="xs" />
                  </span>
                </Tooltip>
              )}
            </span>
            <span className="meta">
              {connecting
                ? t('verbindet…')
                : host.port === 22
                  ? `${host.username}@${host.address}`
                  : `${host.username}@${host.address}:${host.port}`}
            </span>
          </span>
        </button>
        <span className="host-actions">
          <IconButton
            size="sm"
            icon={ICONS.files}
            label={t('Dateien auf {name}', { name: host.name })}
            onClick={() => props.onOpenFiles(host)}
          />
          <IconButton
            size="sm"
            icon={ICONS.edit}
            label={t('{name} bearbeiten', { name: host.name })}
            onClick={() => props.onEdit(host)}
          />
        </span>
      </li>
    );
  };

  const groupNames = sections.flatMap((section) => (section.name ? [section.name] : []));

  return (
    <aside className="sidebar" data-dragging={dragging || undefined}>
      <div className="sidebar-head">
        <h2>{t('Hosts')}</h2>
        <span className="spacer" />
        {/* Green while a tunnel runs: the list shows which host, this shows that one does. */}
        <IconButton
          size="sm"
          icon={ICONS.tunnel}
          label={t('Tunnel')}
          onClick={() => props.onTunnels(null)}
          data-active={props.tunnelIds.size > 0 || undefined}
          className={props.tunnelIds.size > 0 ? 'text-success-ink!' : undefined}
        />
        <IconButton
          size="sm"
          icon={ICONS.import}
          label={t('Importieren')}
          onClick={props.onImport}
        />
        <IconButton
          size="sm"
          icon={ICONS.newFolder}
          label={t('Neue Gruppe')}
          onClick={() => setEditing({ from: null, value: '' })}
        />
        <IconButton
          size="sm"
          icon={ICONS.add}
          label={t('Host hinzufügen')}
          onClick={() => props.onAdd(workspace, null)}
        />
      </div>

      {settings.workspaces && (
        <div className="workspace-switch" role="radiogroup" aria-label={t('Bereich')}>
          {WORKSPACES.map((id) => (
            <button
              key={id}
              type="button"
              role="radio"
              aria-checked={id === workspace}
              data-drop="workspace"
              data-workspace={id}
              onClick={() => updateSettings({ activeWorkspace: id })}
              title={t('{workspace} · Hosts hierher ziehen, um sie zu verschieben', {
                workspace: workspaceName(id, settings),
              })}
            >
              <Icon icon={id === 'private' ? ICONS.home : ICONS.work} />
              <span className="workspace-name">{workspaceName(id, settings)}</span>
              {counts[id] > 0 && <span className="workspace-count">{counts[id]}</span>}
            </button>
          ))}
        </div>
      )}

      {hosts.length > 0 && (
        <input
          className="search"
          type="search"
          placeholder={t('Suchen')}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          aria-label={t('Hosts durchsuchen')}
        />
      )}

      <div className="host-scroll">
        {/* The Mac App Store build has no local shell (lib/flavor.ts). */}
        {localShellAvailable() && (
          <ul className="host-list">
            <li>
              <button
                className="host"
                aria-current={activeId === 'shell'}
                onClick={props.onLocalShell}
                onContextMenu={(event) => {
                  event.preventDefault();
                  shellMenu(event.clientX, event.clientY);
                }}
                onKeyDown={(event) => menuKey(event, shellMenu)}
                title={
                  props.shellOpen
                    ? t('Zeigt die offene lokale Shell, Rechtsklick für eine weitere')
                    : t('Öffnet eine lokale Shell')
                }
              >
                <span className="host-icon host-glyph" aria-hidden>
                  <Icon icon={ICONS.terminal} size="md" />
                </span>
                <span className="host-name">{t('Lokale Shell')}</span>
              </button>
            </li>
          </ul>
        )}

        {query.trim() ? (
          <>
            <ul className="host-list">{found.map((host) => hostRow(host, undefined, false))}</ul>
            {found.length === 0 && (
              <p className="sidebar-note">{t('Kein Host passt zu „{query}".', { query })}</p>
            )}
          </>
        ) : (
          <>
            {editing?.from === null && (
              <input
                className="group-input"
                autoFocus
                placeholder={t('Name der neuen Gruppe')}
                value={editing.value}
                onChange={(e) => setEditing({ from: null, value: e.target.value })}
                onBlur={saveGroupName}
                onKeyDown={(event) => {
                  if (event.key === 'Enter') saveGroupName();
                  if (event.key === 'Escape') setEditing(null);
                }}
                aria-label={t('Name der neuen Gruppe')}
              />
            )}

            {sections.map((section, index) => {
              if (section.name === null) {
                return (
                  <ul
                    key="ungrouped"
                    className="host-list host-dropzone"
                    data-drop="group"
                    data-workspace={workspace}
                    data-group=""
                  >
                    {section.hosts.map((host, i) => hostRow(host, section.hosts[i + 1], true))}
                  </ul>
                );
              }
              const name = section.name;
              const isCollapsed = collapsed(name);
              const nextGroup = groupNames[groupNames.indexOf(name) + 1] ?? '';
              return (
                <section
                  key={`${workspace}/${name}`}
                  className="host-group"
                  data-drop="group"
                  data-workspace={workspace}
                  data-group={name}
                  style={{ animationDelay: `${Math.min(index, 8) * 18}ms` }}
                >
                  <div
                    className="group-head"
                    data-drop="group"
                    data-head="true"
                    data-workspace={workspace}
                    data-group={name}
                    data-next-group={nextGroup}
                    onPointerDown={(event) => {
                      if ((event.target as HTMLElement).closest('input')) return;
                      dragGroup(event, name);
                    }}
                    onContextMenu={(event) => {
                      event.preventDefault();
                      groupMenu(event.clientX, event.clientY, name);
                    }}
                  >
                    {editing?.from === name ? (
                      <input
                        className="group-input"
                        autoFocus
                        value={editing.value}
                        onChange={(e) => setEditing({ from: name, value: e.target.value })}
                        onBlur={saveGroupName}
                        onKeyDown={(event) => {
                          if (event.key === 'Enter') saveGroupName();
                          if (event.key === 'Escape') setEditing(null);
                        }}
                        aria-label={t('{name} umbenennen', { name })}
                      />
                    ) : (
                      <button
                        className="group-toggle"
                        aria-expanded={!isCollapsed}
                        onClick={() => toggle(name)}
                        onDoubleClick={() => setEditing({ from: name, value: name })}
                        onKeyDown={(event) => menuKey(event, (x, y) => groupMenu(x, y, name))}
                        title={t(
                          'Klicken zum Auf-/Zuklappen, doppelklicken zum Umbenennen, ziehen zum Sortieren',
                        )}
                      >
                        <Icon icon={ICONS.expand} size="xs" className="group-chevron" />
                        <h3>{name}</h3>
                        <span className="group-count">{section.hosts.length}</span>
                      </button>
                    )}
                    <IconButton
                      size="sm"
                      icon={ICONS.more}
                      label={t('Menü für {name}', { name })}
                      className="group-more size-6!"
                      onClick={(event) => {
                        const rect = event.currentTarget.getBoundingClientRect();
                        groupMenu(rect.left, rect.bottom, name);
                      }}
                    />
                  </div>
                  {!isCollapsed && (
                    <ul className="host-list">
                      {section.hosts.map((host, i) => hostRow(host, section.hosts[i + 1], true))}
                      {section.hosts.length === 0 && (
                        <li className="group-empty">{t('Hosts hierher ziehen')}</li>
                      )}
                    </ul>
                  )}
                </section>
              );
            })}
          </>
        )}

        {!query.trim() && sections.every((section) => section.hosts.length === 0) && (
          <div className="sidebar-empty">
            <Nyu size={72} mood={hosts.length === 0 ? 'uwu' : 'sleepy'} />
            <p>
              {hosts.length === 0
                ? t('Noch keine Hosts.')
                : t('Noch keine Hosts in {workspace}.', {
                    workspace: workspaceName(workspace, settings),
                  })}
            </p>
            <Button variant="primary" icon={ICONS.add} onClick={() => props.onAdd(workspace, null)}>
              {t('Host hinzufügen')}
            </Button>
            {/* A sentence, not a label: it wraps inside the narrow sidebar. */}
            <Button
              variant="ghost"
              size="sm"
              icon={hosts.length === 0 ? ICONS.import : undefined}
              className="h-auto! min-h-8 py-1.5 whitespace-normal! text-muted!"
              onClick={props.onImport}
            >
              {hosts.length === 0
                ? t('Aus Termius, PuTTY, KiTTY, ssh_config oder einer UwUSSH-Datei importieren')
                : t('Hosts aus dem anderen Bereich hierher ziehen')}
            </Button>
          </div>
        )}
      </div>

      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          items={menu.items}
          label={t('Aktionen')}
          onClose={() => setMenu(null)}
        />
      )}
    </aside>
  );
}

/** Why a password login gets a warning sign, and how to get rid of it. */
function PasswordWarning() {
  return (
    <div className="grid gap-1.5">
      <b className="font-semibold">{t('Anmeldung mit Passwort')}</b>
      <p className="m-0">
        {t(
          'Passwörter lassen sich erraten, durchprobieren oder abphishen. Ein SSH-Key ist viel stärker: Er verlässt nie deinen Computer, und ohne ihn kommt niemand rein.',
        )}
      </p>
      <p className="m-0">
        {t(
          'So wechselst du: Host bearbeiten, bei Anmeldung „SSH-Key“ wählen und mit „Neuen Key erzeugen…“ (UwUKeygen) einen Key anlegen. Dann „Public Key kopieren“ und auf dem Server in ~/.ssh/authorized_keys eintragen.',
        )}
      </p>
      <p className="m-0 opacity-75">{t('Ausschalten unter Einstellungen → Darstellung.')}</p>
    </div>
  );
}
