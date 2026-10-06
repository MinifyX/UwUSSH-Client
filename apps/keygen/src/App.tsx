import { getCurrentWindow } from '@tauri-apps/api/window';
import {
  Icon,
  ICONS,
  TitleBar,
  TitleBarAction,
  Wordmark,
  type WindowControls,
} from '@uwusuite/design';
import { setMacMenu, useTauriWindow } from '@uwusuite/design/tauri';
import { KeygenPanel, type Step } from '@desktop/components/keygen/KeygenPanel';
import { useCloseGuard } from '@desktop/components/CloseGuard';
import { useAppAppearance } from '@desktop/lib/appearance';
import { language, N_, t } from '@desktop/lib/i18n';
import { updateSettings, useSettings } from '@desktop/lib/settings';
import { desktop } from '@desktop/lib/shortcuts';
import { useEffect, useMemo, useRef, useState, type MouseEvent } from 'react';

const TITLES: Record<Step, string> = {
  settings: N_('Neuer SSH-Schlüssel'),
  entropy: N_('Zufall sammeln'),
  generating: N_('Zufall sammeln'),
  done: N_('Dein neuer Schlüssel'),
};

/**
 * Tauri maximises on a double-click of a drag region by itself, and the
 * package's title bar does it again in React; the two would cancel out. Same
 * guard as UwUSSH's title bar (apps/desktop/src/components/TitleBar.tsx).
 */
function leaveDoubleClickToTauri(event: MouseEvent) {
  if ((event.target as HTMLElement).hasAttribute('data-tauri-drag-region')) {
    event.stopPropagation();
  }
}

/**
 * UwUKeygen on its own: the key generator from UwUSSH, for people who only
 * want a key — PuTTYgen, but with Nyu. No vault here; keys leave as files or
 * through the clipboard.
 *
 * Windows and Linux get the suite's title bar; macOS keeps its own title bar
 * (tauri.macos.conf.json) and a minimal menu bar, so ⌘C, ⌘V and ⌘Q work.
 */
export function App() {
  const settings = useSettings();
  const appearance = useAppAppearance(settings);
  const [step, setStep] = useState<Step>('settings');
  const lang = language(settings);

  // Every way to close asks first while something would be lost: the title
  // bar's X, and on macOS the red light and ⌘W, which reach the page as a
  // close request. Once the person said yes, the request goes through.
  const confirmed = useRef(false);
  const guard = useCloseGuard(
    step !== 'settings',
    () => {
      confirmed.current = true;
      void getCurrentWindow().close();
    },
    step === 'done'
      ? t('Der neue Schlüssel ist noch nicht gespeichert und geht dabei verloren.')
      : t('Der gesammelte Zufall geht dabei verloren.'),
  );
  const request = useRef(guard.request);
  request.current = guard.request;
  const unsaved = useRef(false);
  unsaved.current = step !== 'settings';

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let stopped = false;
    void getCurrentWindow()
      .onCloseRequested((event) => {
        if (confirmed.current || !unsaved.current) return;
        event.preventDefault();
        request.current();
      })
      .then((stop) => {
        if (stopped) stop();
        else unlisten = stop;
      })
      .catch(() => undefined);
    return () => {
      stopped = true;
      unlisten?.();
    };
  }, []);

  const dark = appearance.theme === 'dark';
  const themeLabel = dark ? t('Helles Farbschema') : t('Dunkles Farbschema');
  const toggleTheme = () => updateSettings({ theme: dark ? 'light' : 'dark' });
  const toggleRef = useRef(toggleTheme);
  toggleRef.current = toggleTheme;

  // macOS: the standard menus (⌘C, ⌘V, ⌘W, ⌘Q work through them), and the
  // theme switch from the title bar under Darstellung.
  useEffect(() => {
    void setMacMenu(
      {
        appName: 'UwUKeygen',
        lang,
        view: [{ text: themeLabel, action: () => toggleRef.current() }],
      },
      desktop,
    ).catch(() => undefined);
  }, [lang, themeLabel]);

  const window = useTauriWindow();
  const controls = useMemo<WindowControls>(
    () => ({ ...window, close: () => request.current() }),
    [window],
  );

  return (
    <div className="keygen-app">
      {desktop !== 'mac' && (
        <div onDoubleClickCapture={leaveDoubleClickToTauri}>
          <TitleBar
            platform={desktop}
            controls={controls}
            brand={<Wordmark product="Keygen" shell="terminal" className="text-body" />}
            actions={
              <TitleBarAction label={themeLabel} onClick={toggleTheme}>
                <Icon icon={dark ? ICONS.lightTheme : ICONS.darkTheme} size="md" />
              </TitleBarAction>
            }
          />
        </div>
      )}

      <main className="keygen-app-main">
        <section className="keygen-card" aria-labelledby="keygen-title">
          <h1 id="keygen-title" className="keygen-card-title">
            {t(TITLES[step])}
          </h1>
          <KeygenPanel onStep={setStep} />
          {guard.dialog}
        </section>
        <p className="keygen-app-note">
          {t(
            'Teil von UwUSSH · Keys entstehen nur auf diesem Rechner und werden nirgends hochgeladen.',
          )}
        </p>
      </main>
    </div>
  );
}
