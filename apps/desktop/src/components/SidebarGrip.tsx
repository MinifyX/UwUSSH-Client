import { useRef } from 'react';
import { t, useLanguage } from '../lib/i18n';
import {
  getSettings,
  SIDEBAR_MAX,
  SIDEBAR_MIN,
  SIDEBAR_WIDTH,
  updateSettings,
} from '../lib/settings';

/** Dragged this far below the smallest width, the host list folds away. */
const COLLAPSE_AT = SIDEBAR_MIN - 70;
const STEP = 16;

const clamp = (width: number) => Math.round(Math.min(SIDEBAR_MAX, Math.max(SIDEBAR_MIN, width)));

/**
 * The edge between the host list and the tabs: drag it to make the list wider
 * or narrower, drag it to the window's edge to fold the list away, and drag
 * it back out from there. A double click is the default width again.
 *
 * While dragging, the width goes straight onto the body's style; the settings
 * (and with them the whole app) only hear of it when the drag ends.
 */
export function SidebarGrip() {
  useLanguage();
  const drag = useRef<{ body: HTMLElement; left: number; moved: boolean } | null>(null);
  const settings = getSettings();

  const measure = (clientX: number, left: number) => {
    const width = clientX - left;
    return width < COLLAPSE_AT ? null : clamp(width);
  };

  return (
    <div
      className="sidebar-grip"
      role="separator"
      aria-orientation="vertical"
      aria-label={t('Breite der Seitenleiste')}
      aria-valuemin={SIDEBAR_MIN}
      aria-valuemax={SIDEBAR_MAX}
      aria-valuenow={settings.sidebarCollapsed ? 0 : settings.sidebarWidth}
      tabIndex={0}
      title={t('Ziehen ändert die Breite, Doppelklick setzt sie zurück')}
      onPointerDown={(event) => {
        if (event.button !== 0) return;
        const body = event.currentTarget.parentElement;
        if (!body) return;
        event.preventDefault();
        event.currentTarget.setPointerCapture(event.pointerId);
        drag.current = { body, left: body.getBoundingClientRect().left, moved: false };
        body.dataset.resizing = '';
      }}
      onPointerMove={(event) => {
        const state = drag.current;
        if (!state) return;
        state.moved = true;
        const width = measure(event.clientX, state.left);
        if (width === null) state.body.dataset.sidebar = 'collapsed';
        else {
          delete state.body.dataset.sidebar;
          state.body.style.setProperty('--sidebar-width', `${width}px`);
        }
      }}
      onPointerUp={(event) => {
        const state = drag.current;
        drag.current = null;
        if (!state) return;
        delete state.body.dataset.resizing;
        if (!state.moved) return;
        const width = measure(event.clientX, state.left);
        updateSettings(
          width === null
            ? { sidebarCollapsed: true }
            : { sidebarCollapsed: false, sidebarWidth: width },
        );
      }}
      onPointerCancel={() => {
        const state = drag.current;
        drag.current = null;
        if (!state) return;
        delete state.body.dataset.resizing;
        // Back to what the settings say.
        const now = getSettings();
        if (now.sidebarCollapsed) state.body.dataset.sidebar = 'collapsed';
        else delete state.body.dataset.sidebar;
        state.body.style.setProperty('--sidebar-width', `${now.sidebarWidth}px`);
      }}
      onDoubleClick={() => updateSettings({ sidebarCollapsed: false, sidebarWidth: SIDEBAR_WIDTH })}
      onKeyDown={(event) => {
        const now = getSettings();
        if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') {
          event.preventDefault();
          const step = event.key === 'ArrowLeft' ? -STEP : STEP;
          if (now.sidebarCollapsed) {
            if (step > 0) updateSettings({ sidebarCollapsed: false });
          } else if (step < 0 && now.sidebarWidth <= SIDEBAR_MIN) {
            updateSettings({ sidebarCollapsed: true });
          } else {
            updateSettings({ sidebarWidth: clamp(now.sidebarWidth + step) });
          }
        } else if (event.key === 'Enter') {
          event.preventDefault();
          updateSettings({ sidebarCollapsed: !now.sidebarCollapsed });
        }
      }}
    />
  );
}
