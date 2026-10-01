import { getCurrentWindow } from '@tauri-apps/api/window';
import { useEffect, useState, type ReactNode } from 'react';
import { t, useLanguage } from '../lib/i18n';
import { isMac, keysFor } from '../lib/shortcuts';
import { Nyu } from './nyu/Nyu';

type Props = {
  onSettings: () => void;
  children?: ReactNode;
};

const ICONS = {
  settings:
    'M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6Z M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1Z',
};

/**
 * The window's own title bar: on Windows and Linux the window has no system
 * frame (see tauri.conf.json), so moving, minimizing, maximizing and closing
 * all happen here. Double-clicking the empty bar maximizes, as everywhere on
 * Windows.
 *
 * On macOS the system draws its own traffic lights at the left, over this bar
 * (tauri.macos.conf.json: an overlay title bar), so the caption buttons on the
 * right are left out and the bar keeps room for the lights — except in full
 * screen, where macOS hides them and the room would be a gap.
 */
export function TitleBar({ onSettings, children }: Props) {
  useLanguage();
  const mac = isMac();
  const [maximized, setMaximized] = useState(false);
  const [fullscreen, setFullscreen] = useState(false);

  useEffect(() => {
    const window = getCurrentWindow();
    let stopped = false;
    let unlisten: (() => void) | undefined;
    const sync = () => {
      void window
        .isMaximized()
        .then((value) => !stopped && setMaximized(value))
        .catch(() => undefined);
      if (mac)
        void window
          .isFullscreen()
          .then((value) => !stopped && setFullscreen(value))
          .catch(() => undefined);
    };
    sync();
    void window
      .onResized(sync)
      .then((stop) => {
        if (stopped) stop();
        else unlisten = stop;
      })
      .catch(() => undefined);
    return () => {
      stopped = true;
      unlisten?.();
    };
  }, [mac]);

  const window = () => getCurrentWindow();

  return (
    <header
      className="titlebar"
      data-platform={mac ? 'macos' : undefined}
      data-fullscreen={fullscreen || undefined}
      data-tauri-drag-region
    >
      <span className="titlebar-brand" data-tauri-drag-region>
        <Nyu size={22} blink={false} title="UwUSSH" />
        <span className="wordmark" data-tauri-drag-region>
          <span>UwU</span>SSH
        </span>
      </span>
      <span className="spacer" data-tauri-drag-region />
      {children}
      <button
        className="titlebar-action"
        onClick={onSettings}
        title={t('Einstellungen ({keys})', { keys: keysFor('settings') })}
        aria-label={t('Einstellungen')}
      >
        <svg viewBox="0 0 24 24" aria-hidden>
          <path d={ICONS.settings} />
        </svg>
      </button>
      {!mac && (
        <div className="window-controls">
          <button
            className="window-control"
            onClick={() => void window().minimize()}
            title={t('Minimieren')}
            aria-label={t('Minimieren')}
          >
            <svg viewBox="0 0 10 10" aria-hidden>
              <path d="M0 5.5h10" />
            </svg>
          </button>
          <button
            className="window-control"
            onClick={() => void window().toggleMaximize()}
            title={maximized ? t('Verkleinern') : t('Maximieren')}
            aria-label={maximized ? t('Verkleinern') : t('Maximieren')}
          >
            {maximized ? (
              <svg viewBox="0 0 10 10" aria-hidden>
                <path d="M2.5 2.5V.5h7v7h-2 M.5 2.5h7v7h-7z" />
              </svg>
            ) : (
              <svg viewBox="0 0 10 10" aria-hidden>
                <path d="M.5.5h9v9h-9z" />
              </svg>
            )}
          </button>
          <button
            className="window-control close"
            onClick={() => void window().close()}
            title={t('Schließen')}
            aria-label={t('Schließen')}
          >
            <svg viewBox="0 0 10 10" aria-hidden>
              <path d="M.5.5l9 9 M9.5.5l-9 9" />
            </svg>
          </button>
        </div>
      )}
    </header>
  );
}
