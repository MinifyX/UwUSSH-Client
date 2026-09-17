import { getCurrentWindow } from '@tauri-apps/api/window';
import { KeygenPanel, type Step } from '@desktop/components/keygen/KeygenPanel';
import { updateSettings, useSettings } from '@desktop/lib/settings';
import { useCloseGuard } from '@desktop/components/CloseGuard';
import { useState } from 'react';

const TITLES: Record<Step, string> = {
  settings: 'Neuer SSH-Schlüssel',
  entropy: 'Zufall sammeln',
  generating: 'Zufall sammeln',
  done: 'Dein neuer Schlüssel',
};

/**
 * UwUKeygen on its own: the key generator from UwUSSH, for people who only
 * want a key — PuTTYgen, but with Nyu. No vault here; keys leave as files or
 * through the clipboard.
 */
export function App() {
  const settings = useSettings();
  const [step, setStep] = useState<Step>('settings');
  const window = () => getCurrentWindow();
  const dark = settings.theme === 'dark';
  const guard = useCloseGuard(
    step !== 'settings',
    () => void window().close(),
    step === 'done'
      ? 'Der neue Schlüssel ist noch nicht gespeichert und geht dabei verloren.'
      : 'Der gesammelte Zufall geht dabei verloren.',
  );

  return (
    <div className="keygen-app">
      <header className="titlebar" data-tauri-drag-region>
        <span className="titlebar-brand" data-tauri-drag-region>
          <img src="/icon.svg" width={22} height={22} alt="" data-tauri-drag-region />
          <span className="wordmark" data-tauri-drag-region>
            <span>UwU</span>Keygen
          </span>
        </span>
        <span className="spacer" data-tauri-drag-region />
        <button
          className="titlebar-action"
          onClick={() => updateSettings({ theme: dark ? 'light' : 'dark' })}
          title={dark ? 'Helles Farbschema' : 'Dunkles Farbschema'}
          aria-label={dark ? 'Helles Farbschema' : 'Dunkles Farbschema'}
        >
          <svg viewBox="0 0 24 24" aria-hidden>
            {dark ? (
              <path d="M12 4v2 M12 18v2 M4 12h2 M18 12h2 M6.3 6.3l1.4 1.4 M16.3 16.3l1.4 1.4 M6.3 17.7l1.4-1.4 M16.3 7.7l1.4-1.4 M12 16a4 4 0 1 0 0-8 4 4 0 0 0 0 8Z" />
            ) : (
              <path d="M19.5 14.5A8 8 0 0 1 9.5 4.5a8 8 0 1 0 10 10Z" />
            )}
          </svg>
        </button>
        <div className="window-controls">
          <button
            className="window-control"
            onClick={() => void window().minimize()}
            title="Minimieren"
            aria-label="Minimieren"
          >
            <svg viewBox="0 0 10 10" aria-hidden>
              <path d="M0 5.5h10" />
            </svg>
          </button>
          <button
            className="window-control close"
            onClick={guard.request}
            title="Schließen"
            aria-label="Schließen"
          >
            <svg viewBox="0 0 10 10" aria-hidden>
              <path d="M.5.5l9 9 M9.5.5l-9 9" />
            </svg>
          </button>
        </div>
      </header>

      <main className="keygen-app-main">
        <section className="keygen-card" aria-labelledby="keygen-title">
          <h1 id="keygen-title" className="modal-title">
            {TITLES[step]}
          </h1>
          <KeygenPanel onStep={setStep} />
          {guard.dialog}
        </section>
        <p className="keygen-app-note">
          Teil von UwUSSH · Keys entstehen nur auf diesem Rechner und werden nirgends hochgeladen.
        </p>
      </main>
    </div>
  );
}
