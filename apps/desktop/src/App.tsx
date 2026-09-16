import { useCallback, useRef, useState } from 'react';
import { M0Results, M0Status } from './components/M0Panel';
import { Nyu } from './components/nyu/Nyu';
import { TerminalView } from './components/Terminal';
import type { Renderer, TerminalDriver } from './lib/driver';
import { runSuite, type Progress, type ScenarioResult } from './lib/m0';
import { m0Autorun, m0Finish, spawnShellSession } from './lib/session';

/**
 * Example hosts, so the first launch shows what the app is for instead of an
 * empty shell. They are not connectable yet — SSH comes next — and the sidebar
 * says so rather than pretending otherwise.
 */
const DEMO_HOSTS = [
  { name: 'prox-1', meta: '10.0.0.12', online: true },
  { name: 'nas', meta: '10.0.0.20', online: true },
  { name: 'db-01', meta: ':2222', online: false },
  { name: 'edge-bastion', meta: 'bastion…', online: false },
];

/** Long enough for layout and the WebGL context to settle before measuring. */
const AUTORUN_DELAY_MS = 1_500;

export function App() {
  const driverRef = useRef<TerminalDriver | null>(null);
  const [renderer, setRenderer] = useState<Renderer | null>(null);
  const [running, setRunning] = useState(false);
  const [progress, setProgress] = useState<Progress | null>(null);
  const [results, setResults] = useState<ScenarioResult[]>([]);
  const [reportPath, setReportPath] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const openShell = useCallback(async (driver: TerminalDriver) => {
    driver.term.reset();
    try {
      await driver.attach((onData) =>
        spawnShellSession(driver.term.cols, driver.term.rows, onData),
      );
    } catch (e) {
      // A superseded driver (StrictMode's second mount) failing is expected.
      if (driverRef.current === driver) setError(String(e));
    }
  }, []);

  const runM0 = useCallback(async () => {
    const driver = driverRef.current;
    if (!driver) return;

    setRunning(true);
    setResults([]);
    setReportPath(null);
    setError(null);
    try {
      const report = await runSuite(driver, setProgress, setResults);
      setProgress(null);
      setReportPath(await m0Finish(report));
    } catch (e) {
      setError(String(e));
    } finally {
      setProgress(null);
      setRunning(false);
      if (driverRef.current === driver) void openShell(driver);
    }
  }, [openShell]);

  const onReady = useCallback(
    (driver: TerminalDriver) => {
      driverRef.current = driver;
      setRenderer(driver.renderer);

      void (async () => {
        if (await m0Autorun()) {
          await new Promise((resolve) => window.setTimeout(resolve, AUTORUN_DELAY_MS));
          if (driverRef.current === driver) await runM0();
        } else {
          await openShell(driver);
        }
      })();
    },
    [openShell, runM0],
  );

  return (
    <div className="shell">
      <header className="titlebar" data-tauri-drag-region>
        <Nyu size={22} blink={false} title="UwUSSH" />
        <span className="wordmark">
          <span>UwU</span>SSH
        </span>
        <span className="hint">M0 · Durchsatz-Spike</span>
      </header>

      <div className="body">
        <aside className="sidebar">
          <h2>Hosts</h2>
          <ul className="host-list">
            {DEMO_HOSTS.map((host) => (
              <li key={host.name} className="host" aria-selected={false}>
                <i className="dot" data-state={host.online ? 'online' : 'offline'} />
                <span>{host.name}</span>
                <span className="meta">{host.meta}</span>
              </li>
            ))}
          </ul>
          <p className="demo-note">
            Beispiel-Hosts. Verbinden kommt als Nächstes — dass die IPC-Grenze ein echtes Terminal
            trägt, hat M0 gemessen.
          </p>
        </aside>

        <main className="main">
          <div className="toolbar">
            <button className="primary" onClick={() => void runM0()} disabled={running}>
              {running ? 'Messung läuft…' : 'M0-Messung starten'}
            </button>
            <M0Status
              renderer={renderer}
              progress={progress}
              reportPath={reportPath}
              error={error}
            />
          </div>

          <M0Results results={results} />

          <div className="terminal-wrap">
            <TerminalView onReady={onReady} />
          </div>
        </main>
      </div>
    </div>
  );
}
