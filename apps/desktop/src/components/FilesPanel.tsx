import { IconButton, ICONS } from '@uwusuite/design';
import { useRef, type CSSProperties } from 'react';
import type { OpenedFiles } from '../lib/files';
import { t, useLanguage } from '../lib/i18n';
import type { HostRecord } from '../lib/session';
import {
  FILES_PANEL_MAX,
  FILES_PANEL_MIN,
  FILES_PANEL_WIDTH,
  getSettings,
  updateSettings,
  useSettings,
} from '../lib/settings';
import { FileBrowser } from './FileBrowser';

const STEP = 16;
const clamp = (width: number) =>
  Math.round(Math.min(FILES_PANEL_MAX, Math.max(FILES_PANEL_MIN, width)));

type Props = {
  host: HostRecord;
  open: (root: boolean) => Promise<OpenedFiles | null>;
  onClose: () => void;
};

/**
 * The server's files next to its terminal: the file browser's server side in
 * one column, on a connection of its own. Mounted only while it is open, so
 * closing it closes that connection. The terminal refits by itself (its
 * resize observer) when the panel comes, goes or changes width.
 */
export function FilesPanel({ host, open, onClose }: Props) {
  const settings = useSettings();
  return (
    <aside
      className="files-panel"
      aria-label={t('Dateien von {name}', { name: host.name })}
      style={{ '--files-width': `${settings.filesPanelWidth}px` } as CSSProperties}
    >
      <FilesPanelGrip />
      <div className="files-panel-head">
        <b>{t('Dateien')}</b>
        <span className="spacer" />
        <IconButton size="sm" icon={ICONS.close} label={t('Dateien schließen')} onClick={onClose} />
      </div>
      <FileBrowser host={host} open={open} onLive={() => undefined} compact />
    </aside>
  );
}

/** The panel's left edge, as the sidebar's grip: drag, arrow keys, double click resets. */
function FilesPanelGrip() {
  useLanguage();
  const drag = useRef<{ panel: HTMLElement; right: number; moved: boolean } | null>(null);
  const width = getSettings().filesPanelWidth;
  const end = (keep: boolean, clientX = 0) => {
    const state = drag.current;
    drag.current = null;
    if (!state) return;
    delete state.panel.dataset.resizing;
    if (keep && state.moved) updateSettings({ filesPanelWidth: clamp(state.right - clientX) });
    else state.panel.style.setProperty('--files-width', `${getSettings().filesPanelWidth}px`);
  };
  return (
    <div
      className="files-grip"
      role="separator"
      aria-orientation="vertical"
      aria-label={t('Breite der Dateien')}
      aria-valuemin={FILES_PANEL_MIN}
      aria-valuemax={FILES_PANEL_MAX}
      aria-valuenow={width}
      tabIndex={0}
      title={t('Ziehen ändert die Breite, Doppelklick setzt sie zurück')}
      onPointerDown={(event) => {
        if (event.button !== 0) return;
        const panel = event.currentTarget.parentElement;
        if (!panel) return;
        event.preventDefault();
        event.currentTarget.setPointerCapture(event.pointerId);
        drag.current = { panel, right: panel.getBoundingClientRect().right, moved: false };
        panel.dataset.resizing = '';
      }}
      onPointerMove={(event) => {
        const state = drag.current;
        if (!state) return;
        state.moved = true;
        state.panel.style.setProperty('--files-width', `${clamp(state.right - event.clientX)}px`);
      }}
      onPointerUp={(event) => end(true, event.clientX)}
      onPointerCancel={() => end(false)}
      onDoubleClick={() => updateSettings({ filesPanelWidth: FILES_PANEL_WIDTH })}
      onKeyDown={(event) => {
        if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return;
        event.preventDefault();
        // The grip is on the left edge: left makes the panel wider.
        const step = event.key === 'ArrowLeft' ? STEP : -STEP;
        updateSettings({ filesPanelWidth: clamp(getSettings().filesPanelWidth + step) });
      }}
    />
  );
}
