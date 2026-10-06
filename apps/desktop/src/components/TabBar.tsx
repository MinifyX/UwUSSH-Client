import { Icon, IconButton, ICONS } from '@uwusuite/design';
import { t, useLanguage } from '../lib/i18n';
import { keysFor, keysForTab } from '../lib/shortcuts';
import type { Tab } from '../lib/tabs';
import { OsIcon } from './OsIcon';

type Props = {
  tabs: Tab[];
  activeId: string | null;
  onSelect: (id: string) => void;
  onClose: (id: string) => void;
  /** No local shell in the Mac App Store build: then there is no "+" either. */
  onNewShell?: () => void;
  /** Opens the command assistant over the shown terminal. */
  onAssist: () => void;
  /** The shown tab is a live terminal the assistant can type into. */
  canAssist: boolean;
};

function stateOf(tab: Tab): 'online' | 'connecting' | 'idle' {
  if (tab.status === 'connecting') return 'connecting';
  if (tab.status === 'live') return 'online';
  return 'idle';
}

/**
 * One tab per session. Every click on a host opens another one, so several
 * connections to the same server sit side by side; the number after a repeated
 * name tells them apart. Middle click closes, like in a browser.
 */
export function TabBar({
  tabs,
  activeId,
  onSelect,
  onClose,
  onNewShell,
  onAssist,
  canAssist,
}: Props) {
  useLanguage();
  return (
    <div className="tabbar">
      <div className="tabs" role="tablist" aria-label={t('Offene Sitzungen')}>
        {tabs.map((tab, index) => {
          const active = tab.id === activeId;
          const name = tab.subtitle ? `${tab.title} · ${tab.subtitle}` : tab.title;
          return (
            <div
              key={tab.id}
              className="tab"
              data-active={active}
              data-status={tab.status}
              onMouseDown={(event) => {
                // Middle click closes without selecting first.
                if (event.button === 1) {
                  event.preventDefault();
                  onClose(tab.id);
                }
              }}
            >
              <button
                role="tab"
                className="tab-select"
                aria-selected={active}
                title={
                  index < 9 ? t('{name} ({keys})', { name, keys: keysForTab(index + 1) }) : name
                }
                onClick={() => onSelect(tab.id)}
              >
                <span className="tab-icon" aria-hidden>
                  {tab.kind === 'files' ? (
                    <Icon icon={ICONS.files} size="sm" />
                  ) : tab.kind === 'ssh' ? (
                    <OsIcon os={tab.host.os} size={16} title="" />
                  ) : (
                    <Icon icon={ICONS.terminal} size="sm" />
                  )}
                  <i className="dot" data-state={stateOf(tab)} />
                </span>
                <span className="tab-title">{tab.title}</span>
                {tab.ordinal > 1 && <span className="tab-ordinal">{tab.ordinal}</span>}
              </button>
              <button
                className="tab-close"
                onClick={() => onClose(tab.id)}
                title={t('Tab schließen ({keys})', { keys: keysFor('close-tab') })}
                aria-label={t('{name} schließen', { name: tab.title })}
              >
                <Icon icon={ICONS.close} size="xs" />
              </button>
            </div>
          );
        })}
      </div>
      <IconButton
        size="sm"
        className="tab-assist"
        icon={ICONS.ai}
        onClick={onAssist}
        disabled={!canAssist}
        label={
          canAssist
            ? t('Befehl aus Worten ({shortcut})', { shortcut: keysFor('assist') })
            : t('Befehl aus Worten: erst ein Terminal öffnen')
        }
      />
      {onNewShell && (
        <IconButton
          size="sm"
          className="tab-new"
          icon={ICONS.add}
          onClick={onNewShell}
          label={t('Neue lokale Shell ({keys})', { keys: keysFor('new-shell') })}
        />
      )}
    </div>
  );
}
