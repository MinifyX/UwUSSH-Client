import { Icon, ICONS, TitleBar as SuiteTitleBar, TitleBarAction, Wordmark } from '@uwusuite/design';
import { useTauriWindow } from '@uwusuite/design/tauri';
import type { MouseEvent, ReactNode } from 'react';
import { t, useLanguage } from '../lib/i18n';
import { desktop, withKeys } from '../lib/shortcuts';

type Props = {
  onSettings: () => void;
  /** More actions before the gear. */
  children?: ReactNode;
};

/**
 * Tauri maximises on a double-click of a drag region by itself (its drag
 * script), and the package's title bar does it again in React — the two would
 * cancel out. The capture phase runs first and keeps the second one away; the
 * window buttons are not drag regions, so they are never touched by this.
 */
function leaveDoubleClickToTauri(event: MouseEvent) {
  if ((event.target as HTMLElement).hasAttribute('data-tauri-drag-region')) {
    event.stopPropagation();
  }
}

/**
 * The window's own title bar on Windows and Linux (@uwusuite/design's
 * TitleBar): the window has no system frame there (tauri.conf.json), so
 * moving, minimizing, maximizing and closing all happen here. Double-clicking
 * the empty bar maximizes, as everywhere on Windows.
 *
 * macOS draws the window's title bar itself (tauri.macos.conf.json), with the
 * traffic lights, and the actions live in the menu bar (App.tsx, setMacMenu),
 * so there is nothing to draw there.
 */
export function TitleBar({ onSettings, children }: Props) {
  useLanguage();
  const controls = useTauriWindow();
  if (desktop === 'mac') return null;
  return (
    <div className="titlebar-frame" onDoubleClickCapture={leaveDoubleClickToTauri}>
      <SuiteTitleBar
        platform={desktop}
        controls={controls}
        brand={<Wordmark product="SSH" shell="terminal" className="text-body" />}
        actions={
          <>
            {children}
            <TitleBarAction
              label={withKeys(t('Einstellungen'), 'CmdOrCtrl+,')}
              onClick={onSettings}
            >
              <Icon icon={ICONS.settings} size="md" />
            </TitleBarAction>
          </>
        }
      />
    </div>
  );
}
