import { Icon, IconButton, ICONS } from '@uwusuite/design';
import { useEffect, useRef } from 'react';
import { t, useLanguage } from '../lib/i18n';
import { keysForTab, withKeysFor } from '../lib/shortcuts';
import { dropIndex } from '../lib/tabRound';
import type { Tab } from '../lib/tabs';
import { OsIcon } from './OsIcon';

type Props = {
  tabs: Tab[];
  activeId: string | null;
  /** `null` is the overview. */
  onSelect: (id: string | null) => void;
  onClose: (id: string) => void;
  /** Dragging a tab puts it at this place among the tabs. */
  onMove: (id: string, to: number) => void;
  /** No local shell in the Mac App Store build: then there is no "+" either. */
  onNewShell?: () => void;
  /** The two tabs shown side by side, if any. */
  split: readonly string[];
  /** The host list is folded away; the button brings it back. */
  sidebarCollapsed: boolean;
  onToggleSidebar: () => void;
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
  onMove,
  onNewShell,
  split,
  sidebarCollapsed,
  onToggleSidebar,
}: Props) {
  useLanguage();
  const listRef = useRef<HTMLDivElement>(null);
  const drag = useRef<{ id: string; startX: number; moving: boolean } | null>(null);
  const swallowClick = useRef(false);

  // A tab that just opened or got picked by keyboard may sit outside the scrolled part.
  useEffect(() => {
    listRef.current
      ?.querySelector('[data-active="true"]')
      ?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
  }, [activeId, tabs.length]);

  function dragTo(x: number) {
    const list = listRef.current;
    const current = drag.current;
    if (!list || !current) return;
    const bounds = list.getBoundingClientRect();
    // shortcut: scrolls only while the pointer moves, enough for a short hold at the edge.
    if (x < bounds.left + 24) list.scrollLeft -= 12;
    else if (x > bounds.right - 24) list.scrollLeft += 12;
    const middles = [...list.querySelectorAll<HTMLElement>('[data-tab-id]')]
      .filter((el) => el.dataset.tabId !== current.id)
      .map((el) => {
        const rect = el.getBoundingClientRect();
        return rect.left + rect.width / 2;
      });
    const to = dropIndex(middles, x);
    if (tabs.findIndex((tab) => tab.id === current.id) !== to) onMove(current.id, to);
  }

  return (
    <div className="tabbar">
      <IconButton
        className="tab-sidebar"
        size="sm"
        icon={sidebarCollapsed ? ICONS.sidebarShow : ICONS.sidebarHide}
        label={withKeysFor(
          sidebarCollapsed ? t('Seitenleiste einblenden') : t('Seitenleiste ausblenden'),
          'toggle-sidebar',
        )}
        aria-pressed={!sidebarCollapsed}
        onClick={onToggleSidebar}
      />
      <div
        ref={listRef}
        className="tabs"
        role="tablist"
        aria-label={t('Offene Sitzungen')}
        // Move and release are caught here, not on the tab: reordering moves the tab's
        // element, which would drop a pointer capture held by it.
        onPointerMove={(event) => {
          const current = drag.current;
          if (!current) return;
          if (!(event.buttons & 1)) {
            // Released somewhere we did not hear about.
            drag.current = null;
            return;
          }
          if (!current.moving) {
            if (Math.abs(event.clientX - current.startX) < 5) return;
            current.moving = true;
            event.currentTarget.setPointerCapture(event.pointerId);
          }
          dragTo(event.clientX);
        }}
        onPointerUp={() => {
          if (drag.current?.moving) swallowClick.current = true;
          drag.current = null;
        }}
        onPointerCancel={() => {
          drag.current = null;
        }}
        onWheel={(event) => {
          // A plain mouse wheel only scrolls vertically; the tab row runs sideways.
          if (Math.abs(event.deltaY) > Math.abs(event.deltaX)) {
            event.currentTarget.scrollLeft += event.deltaY;
          }
        }}
        onClickCapture={(event) => {
          if (swallowClick.current) {
            swallowClick.current = false;
            event.stopPropagation();
          }
        }}
      >
        {/* The overview: always first, never closes. */}
        <div className="tab tab-home" data-active={activeId === null}>
          <button
            role="tab"
            className="tab-select"
            aria-selected={activeId === null}
            aria-controls="pane-home"
            aria-label={t('Übersicht')}
            title={t('Übersicht')}
            onClick={() => onSelect(null)}
          >
            <span className="tab-icon" aria-hidden>
              <Icon icon={ICONS.home} size="sm" />
            </span>
          </button>
        </div>
        {tabs.map((tab, index) => {
          const active = tab.id === activeId;
          const shared = split.includes(tab.id);
          const name = tab.subtitle ? `${tab.title} · ${tab.subtitle}` : tab.title;
          return (
            <div
              key={tab.id}
              className="tab"
              data-active={active}
              data-split={shared || undefined}
              data-status={tab.status}
              data-tab-id={tab.id}
              onPointerDown={(event) => {
                if (event.button !== 0 || (event.target as Element).closest('.tab-close')) return;
                drag.current = { id: tab.id, startX: event.clientX, moving: false };
                swallowClick.current = false;
              }}
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
                aria-label={
                  shared
                    ? t('{name}, geteilt', {
                        name: tab.ordinal > 1 ? `${tab.title} ${tab.ordinal}` : tab.title,
                      })
                    : undefined
                }
                title={
                  index < 9 && keysForTab(index + 1)
                    ? t('{name} ({keys})', { name, keys: keysForTab(index + 1) })
                    : name
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
                {shared && (
                  <Icon icon={ICONS.compare} size="xs" className="tab-split" aria-hidden />
                )}
                {tab.ordinal > 1 && <span className="tab-ordinal">{tab.ordinal}</span>}
              </button>
              <button
                className="tab-close"
                onClick={() => onClose(tab.id)}
                title={withKeysFor(t('Tab schließen'), 'close-tab')}
                aria-label={t('{name} schließen', { name: tab.title })}
              >
                <Icon icon={ICONS.close} size="xs" />
              </button>
            </div>
          );
        })}
      </div>
      {onNewShell && (
        <IconButton
          size="sm"
          className="tab-new"
          icon={ICONS.add}
          onClick={onNewShell}
          label={withKeysFor(t('Neue lokale Shell'), 'new-shell')}
        />
      )}
    </div>
  );
}
