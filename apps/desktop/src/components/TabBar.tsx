import type { Tab } from '../lib/tabs';

type Props = {
  tabs: Tab[];
  activeId: string | null;
  onSelect: (id: string) => void;
  onClose: (id: string) => void;
  onNewShell: () => void;
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
export function TabBar({ tabs, activeId, onSelect, onClose, onNewShell }: Props) {
  return (
    <div className="tabbar">
      <div className="tabs" role="tablist" aria-label="Offene Sitzungen">
        {tabs.map((tab, index) => {
          const active = tab.id === activeId;
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
                title={`${tab.subtitle ? `${tab.title} · ${tab.subtitle}` : tab.title}${
                  index < 9 ? ` (Strg+Umschalt+${index + 1})` : ''
                }`}
                onClick={() => onSelect(tab.id)}
              >
                <i className="dot" data-state={stateOf(tab)} aria-hidden />
                <span className="tab-title">{tab.title}</span>
                {tab.ordinal > 1 && <span className="tab-ordinal">{tab.ordinal}</span>}
              </button>
              <button
                className="tab-close"
                onClick={() => onClose(tab.id)}
                title="Tab schließen (Strg+Umschalt+W)"
                aria-label={`${tab.title} schließen`}
              >
                ×
              </button>
            </div>
          );
        })}
      </div>
      <button
        className="icon-button tab-new"
        onClick={onNewShell}
        title="Neue lokale Shell (Strg+Umschalt+T)"
        aria-label="Neue lokale Shell"
      >
        +
      </button>
    </div>
  );
}
