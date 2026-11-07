import {
  Button,
  ICONS,
  Menu,
  StatusLine,
  type MenuEntry,
  type StatusState,
} from '@uwusuite/design';
import type { ReactNode } from 'react';
import { N_, t, useLanguage } from '../lib/i18n';
import { keysLabel, withKeysFor } from '../lib/shortcuts';
import type { Split } from '../lib/split';
import type { Tab } from '../lib/tabs';

type Props = {
  tab: Tab;
  tabs: Tab[];
  split: Split | null;
  /** The files panel next to this ssh tab is open. */
  filesOpen: boolean;
  onToggleFiles: () => void;
  onTypePassword: () => void;
  /** Tunnels of this tab's host running right now. */
  tunnels: number;
  onTunnels: () => void;
  canAssist: boolean;
  onAssist: () => void;
  onSplitNew: (() => void) | null;
  onSplitWith: (id: string) => void;
  onSplitTurn: () => void;
  onUnsplit: () => void;
  /** The throughput test's own status, for its tab. */
  children?: ReactNode;
};

const STATES: Record<Tab['status'], { state: StatusState; label: string }> = {
  connecting: { state: 'connecting', label: N_('verbindet…') },
  live: { state: 'online', label: N_('verbunden') },
  failed: { state: 'error', label: N_('nicht verbunden') },
  ended: { state: 'offline', label: N_('getrennt') },
};

/** The bar above the tab in front: what it is, and what can be done with it. */
export function SessionToolbar(props: Props) {
  useLanguage();
  const { tab, split } = props;
  const remote = tab.kind === 'ssh' || tab.kind === 'files';
  const status = STATES[tab.status];
  const shown = split !== null && (split.a === tab.id || split.b === tab.id);
  const others = props.tabs.filter((other) => other.id !== tab.id);

  const splitItems: MenuEntry[] = [
    ...(props.onSplitNew
      ? [{ label: t('Neue Sitzung daneben'), icon: ICONS.add, onSelect: props.onSplitNew }]
      : []),
    ...(others.length > 0
      ? [
          { heading: t('Tab daneben anzeigen') },
          ...others.map((other) => ({
            label: other.ordinal > 1 ? `${other.title} ${other.ordinal}` : other.title,
            onSelect: () => props.onSplitWith(other.id),
          })),
        ]
      : []),
    ...(shown
      ? [
          'separator' as const,
          {
            label: split.vertical ? t('Nebeneinander') : t('Untereinander'),
            icon: ICONS.layout,
            onSelect: props.onSplitTurn,
          },
          { label: t('Teilung aufheben'), icon: ICONS.close, onSelect: props.onUnsplit },
        ]
      : []),
  ];

  return (
    <div className="toolbar">
      <span className="session-title">
        <b>{tab.title}</b>
        {tab.subtitle && <span className="meta">{tab.subtitle}</span>}
        {remote && <StatusLine state={status.state}>{t(status.label)}</StatusLine>}
      </span>
      <span className="spacer" />
      {tab.kind === 'ssh' && tab.canTypePassword && tab.status === 'live' && (
        <Button
          variant="ghost"
          size="sm"
          icon={ICONS.secret}
          className="toolbar-button"
          onClick={props.onTypePassword}
          title={t(
            'Das Passwort des Hosts ins Terminal tippen ({keys}). Enter kommt nur dazu, wenn gerade etwas nach einer Eingabe fragt.',
            { keys: keysLabel('type-password') },
          )}
        >
          {t('Passwort eintippen')}
        </Button>
      )}
      {tab.kind === 'ssh' && (
        <Button
          variant="ghost"
          size="sm"
          icon={ICONS.files}
          className="toolbar-button"
          aria-pressed={props.filesOpen}
          data-on={props.filesOpen || undefined}
          onClick={props.onToggleFiles}
          title={withKeysFor(t('Dateien dieses Hosts neben dem Terminal'), 'open-files')}
        >
          {t('Dateien')}
        </Button>
      )}
      {remote && (
        <Button
          variant="ghost"
          size="sm"
          icon={ICONS.tunnel}
          className="toolbar-button"
          onClick={props.onTunnels}
          title={withKeysFor(t('Tunnel dieses Hosts'), 'tunnels')}
        >
          {t('Tunnel')}
          {props.tunnels > 0 && (
            <span className="toolbar-count" aria-label={t('{n} laufen', { n: props.tunnels })}>
              {props.tunnels}
            </span>
          )}
        </Button>
      )}
      {props.canAssist && (
        <Button
          variant="ghost"
          size="sm"
          icon={ICONS.ai}
          className="toolbar-button"
          onClick={props.onAssist}
          title={withKeysFor(t('Befehl aus Worten'), 'assist')}
        >
          {t('Befehl aus Worten')}
        </Button>
      )}
      {splitItems.length > 0 && (
        <Menu
          align="end"
          items={splitItems}
          trigger={({ toggle, open: _open, ...aria }) => (
            <Button
              variant="ghost"
              size="sm"
              icon={ICONS.compare}
              className="toolbar-button"
              data-on={shown || undefined}
              onClick={toggle}
              title={withKeysFor(t('Zwei Tabs nebeneinander zeigen'), 'split-view')}
              {...aria}
            >
              {t('Teilen')}
            </Button>
          )}
        />
      )}
      {props.children}
    </div>
  );
}
