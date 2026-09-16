import { useCallback, useEffect, useState } from 'react';
import { Nyu } from './components/nyu/Nyu';
import { TerminalView } from './components/Terminal';
import { ThroughputHud } from './components/ThroughputHud';
import { sessionMetrics, startLoadTest, type MetricsSnapshot, type SessionId } from './lib/session';

/**
 * Example hosts, so the first launch shows what the app is for instead of an
 * empty shell. They are not connectable — SSH lands in M1 — and the sidebar
 * says so rather than pretending otherwise.
 */
const DEMO_HOSTS = [
  { name: 'prox-1', meta: '10.0.0.12', online: true },
  { name: 'nas', meta: '10.0.0.20', online: true },
  { name: 'db-01', meta: ':2222', online: false },
  { name: 'edge-bastion', meta: 'bastion…', online: false },
];

export function App() {
  const [session, setSession] = useState<SessionId | null>(null);
  const [metrics, setMetrics] = useState<MetricsSnapshot | null>(null);
  const [renderer, setRenderer] = useState<'webgl' | 'canvas' | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Poll rather than push: metrics must never compete with terminal frames for
  // the channel we are trying to measure.
  useEffect(() => {
    if (!session) return;
    const timer = window.setInterval(() => {
      sessionMetrics(session)
        .then(setMetrics)
        .catch(() => undefined);
    }, 250);
    return () => window.clearInterval(timer);
  }, [session]);

  const runLoadTest = useCallback(() => {
    if (session) void startLoadTest(session);
  }, [session]);

  return (
    <div className="shell">
      <header className="titlebar" data-tauri-drag-region>
        <Nyu size={22} blink={false} title="UwUSSH" />
        <span className="wordmark">
          <span>UwU</span>SSH
        </span>
        <span className="hint">M0 · Durchsatz-Spike</span>
        <span className="spacer" />
        <span className="hint">lokale Shell · noch kein SSH</span>
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
            Beispiel-Hosts. Verbinden geht ab M1 — bis dahin misst dieser Build nur, ob die
            IPC-Grenze ein echtes Terminal trägt.
          </p>
        </aside>

        <main className="main">
          <div className="toolbar">
            <button className="primary" onClick={runLoadTest} disabled={!session}>
              Lasttest starten
            </button>
            <ThroughputHud metrics={metrics} renderer={renderer} />
          </div>

          <div className="terminal-wrap">
            {error ? (
              <div className="empty-state">
                <Nyu size={120} />
                <p>
                  Die Session ist nicht gestartet.
                  <br />
                  <code>{error}</code>
                </p>
              </div>
            ) : (
              <TerminalView onSession={setSession} onRenderer={setRenderer} onError={setError} />
            )}
          </div>
        </main>
      </div>
    </div>
  );
}
