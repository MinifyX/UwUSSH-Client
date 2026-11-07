import {
  CommandPalette,
  filterCommands,
  Icon,
  ICONS,
  StatusDot,
  type CommandGroup,
  type CommandItem,
} from '@uwusuite/design';
import { useEffect, useState } from 'react';
import { t, useLanguage } from '../lib/i18n';
import { ACTION_GROUPS, type BoundAction } from '../lib/keymap';
import { lockVault, vaultState, type HostRecord } from '../lib/session';
import { useSettings, workspaceName } from '../lib/settings';
import { actionName, isMac, keysFor } from '../lib/shortcuts';
import { syncNow, syncStatus } from '../lib/sync';
import { canAssist, type Tab } from '../lib/tabs';
import { tabDot } from './Home';
import { OsIcon } from './OsIcon';

type Props = {
  open: boolean;
  onClose: () => void;
  hosts: HostRecord[];
  tabs: Tab[];
  activeId: string | null;
  /** Whether an action does anything right now; the others are left out. */
  canPerform: (action: BoundAction) => boolean;
  onPerform: (action: BoundAction) => void;
  onConnect: (host: HostRecord) => void;
  /** Ctrl/Cmd+Enter: a new session to the host, next to the tab in front. */
  onConnectBeside: (host: HostRecord) => void;
  onSelectTab: (id: string | null) => void;
  onImport: () => void;
  /** Opens "Befehl aus Worten" for the tab in front with this request. */
  onAssist: (request: string) => void;
};

const HIDDEN: readonly BoundAction[] = ['command-palette'];

/**
 * ⌘K: hosts, open tabs and actions in one searchable list, and for the
 * terminal in front the query as a request for "Befehl aus Worten".
 */
export function Palette(props: Props) {
  useLanguage();
  const { open, onClose, hosts, tabs, activeId } = props;
  const settings = useSettings();
  const [query, setQuery] = useState('');
  const [canSync, setCanSync] = useState(false);
  const [unlocked, setUnlocked] = useState(false);

  // Each opening starts empty and asks what sync and the vault can do now.
  useEffect(() => {
    if (!open) return;
    setQuery('');
    void syncStatus()
      .then((status) => setCanSync(status.backend !== 'none' && !status.running))
      .catch(() => setCanSync(false));
    void vaultState()
      .then((state) => setUnlocked(state.status === 'unlocked'))
      .catch(() => setUnlocked(false));
  }, [open]);

  const done = (run: () => void) => () => {
    onClose();
    run();
  };
  const active = tabs.find((tab) => tab.id === activeId) ?? null;
  const workspace = settings.workspaces ? settings.activeWorkspace : null;

  const hostItems: CommandItem[] = [...hosts]
    // Stable sort: the active workspace's hosts first, each side in list order.
    .sort((a, b) => Number(b.workspace === workspace) - Number(a.workspace === workspace))
    .map((host) => {
      const index = tabs.findIndex((tab) => tab.kind === 'ssh' && tab.host.id === host.id);
      const tab = tabs[index];
      const state = tab
        ? tab.status === 'connecting'
          ? t('verbindet')
          : t('offen in Tab {number}', { number: index + 1 })
        : null;
      return {
        id: `host:${host.id}`,
        label: host.name,
        text: host.name,
        icon: <OsIcon os={host.os} size={18} title="" />,
        detail: (
          <span className="uwu-mono">
            {host.username}@{host.address}
            {host.port === 22 ? '' : `:${host.port}`}
          </span>
        ),
        trailing: tab && (
          <>
            <StatusDot state={tabDot(tab)} />
            {state}
          </>
        ),
        keywords: [
          host.address,
          host.username,
          host.groupPath ?? '',
          settings.workspaces ? workspaceName(host.workspace, settings) : '',
        ],
        onSelect: ({ modifier }) => {
          onClose();
          if (modifier) props.onConnectBeside(host);
          else props.onConnect(host);
        },
      };
    });

  const tabItems: CommandItem[] = [
    {
      id: 'tab:home',
      label: t('Übersicht'),
      text: t('Übersicht'),
      icon: <Icon icon={ICONS.home} />,
      onSelect: done(() => props.onSelectTab(null)),
    },
    ...tabs.map((tab, index) => {
      const title = tab.ordinal > 1 ? `${tab.title} ${tab.ordinal}` : tab.title;
      return {
        id: `tab:${tab.id}`,
        label: title,
        text: title,
        detail: tab.subtitle && <span className="uwu-mono">{tab.subtitle}</span>,
        trailing: t('Tab {number}', { number: index + 1 }),
        onSelect: done(() => props.onSelectTab(tab.id)),
      };
    }),
  ];

  const action = (id: string, label: string, run: () => void, keys = ''): CommandItem => ({
    id,
    label,
    text: label,
    trailing: keys && <kbd>{keys}</kbd>,
    onSelect: done(run),
  });
  const actionItems: CommandItem[] = [
    ...Object.values(ACTION_GROUPS)
      .flat()
      .filter((id) => !HIDDEN.includes(id) && props.canPerform(id))
      .map((id) => action(`action:${id}`, actionName(id), () => props.onPerform(id), keysFor(id))),
    action('action:import', t('Importieren …'), props.onImport),
    ...(canSync ? [action('action:sync', t('Jetzt abgleichen'), () => void syncNow())] : []),
    ...(unlocked
      ? [action('action:lock', t('Tresor sperren'), () => void lockVault().catch(() => undefined))]
      : []),
  ];

  const groups: CommandGroup[] = filterCommands(
    [
      { id: 'hosts', title: t('Hosts'), items: hostItems },
      { id: 'tabs', title: t('Offene Tabs'), items: tabItems },
      { id: 'actions', title: t('Aktionen'), items: actionItems },
    ],
    query,
  );
  const request = query.trim();
  if (request && active && canAssist(active)) {
    const text = t('„{request}“ als Befehl für {tab}', {
      request,
      tab: active.kind === 'ssh' ? active.title : t('diesen Rechner'),
    });
    groups.push({
      id: 'assist',
      title: t('Befehl aus Worten'),
      items: [
        {
          id: 'assist:query',
          label: text,
          text,
          icon: <Icon icon={ICONS.ai} />,
          onSelect: done(() => props.onAssist(request)),
        },
      ],
    });
  }

  return (
    <CommandPalette
      open={open}
      onClose={onClose}
      groups={groups}
      query={query}
      onQueryChange={setQuery}
      filter={false}
      placeholder={t('Host, Tab oder Aktion suchen …')}
      footer={t('↑ ↓ wählen · Enter ausführen · {mod}+Enter daneben öffnen · Esc schließen', {
        mod: isMac() ? '⌘' : t('Strg'),
      })}
    />
  );
}
