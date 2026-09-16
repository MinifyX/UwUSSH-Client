import { useCallback, useEffect, useRef, useState } from 'react';
import { HostKeyChanged, SecretPrompt, TrustHostKey } from './components/ConnectDialogs';
import { HostForm } from './components/HostForm';
import { HostList } from './components/HostList';
import { ImportDialog } from './components/ImportDialog';
import { M0Results, M0Status } from './components/M0Panel';
import { Nyu } from './components/nyu/Nyu';
import { TerminalView } from './components/Terminal';
import type { Renderer, TerminalDriver } from './lib/driver';
import { runSuite, type Progress, type ScenarioResult } from './lib/m0';
import {
  asConnectFailure,
  cancelConnect,
  connectHost,
  listHosts,
  m0Autorun,
  m0Finish,
  spawnShellSession,
  trustHostKey,
  type ConnectFailure,
  type HostRecord,
  type ObservedHostKey,
} from './lib/session';

/** Long enough for layout and the WebGL context to settle before measuring. */
const AUTORUN_DELAY_MS = 1_500;

/** `ended` once the stream has stopped on its own — the terminal stays readable, the session is gone. */
type Active = ({ kind: 'shell' } | { kind: 'ssh'; host: HostRecord }) & { ended?: boolean };

type Dialog =
  | {
      kind: 'secret';
      host: HostRecord;
      secret: 'password' | 'passphrase';
      retry: boolean;
      resolve: (value: string | null) => void;
    }
  | { kind: 'trust'; host: HostRecord; observed: ObservedHostKey; resolve: (ok: boolean) => void }
  | {
      kind: 'changed';
      host: HostRecord;
      trustedFingerprint: string;
      observed: ObservedHostKey;
      resolve: (confirmation: string | null) => void;
    };

type Notice = { tone: 'info' | 'error'; text: string; action?: { label: string; run: () => void } };

/** Failures that are not a question for the user, in words the user can act on. */
function describe(failure: ConnectFailure, host: HostRecord): string {
  switch (failure.kind) {
    case 'unreachable':
      return `${host.address} ist nicht erreichbar: ${failure.reason}`;
    case 'key-unreadable':
      return `Die Key-Datei ${failure.keyPath} ließ sich nicht lesen: ${failure.reason}`;
    case 'auth-rejected':
      return failure.remaining.length > 0
        ? `Der Server hat die Anmeldung abgelehnt. Er würde akzeptieren: ${failure.remaining.join(', ')}.`
        : 'Der Server hat die Anmeldung abgelehnt.';
    case 'session-refused':
      return `Angemeldet, aber der Server hat kein Terminal geöffnet: ${failure.reason}`;
    case 'protocol':
      return `SSH-Fehler: ${failure.reason}`;
    case 'internal':
      return failure.message;
    default:
      return `Verbindung fehlgeschlagen (${failure.kind}).`;
  }
}

export function App() {
  const driverRef = useRef<TerminalDriver | null>(null);
  /** The host an attempt is in flight for. A ref, not state: a double click must see it immediately. */
  const connectingRef = useRef<string | null>(null);
  const backgroundRef = useRef<HTMLDivElement>(null);
  const [renderer, setRenderer] = useState<Renderer | null>(null);
  const [hosts, setHosts] = useState<HostRecord[]>([]);
  const [active, setActive] = useState<Active | null>(null);
  const [connectingId, setConnectingId] = useState<string | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [dialog, setDialog] = useState<Dialog | null>(null);
  const [form, setForm] = useState<{ host: HostRecord | null } | null>(null);
  const [importing, setImporting] = useState(false);

  const [m0Running, setM0Running] = useState(false);
  const [progress, setProgress] = useState<Progress | null>(null);
  const [results, setResults] = useState<ScenarioResult[]>([]);
  const [reportPath, setReportPath] = useState<string | null>(null);
  const [m0Error, setM0Error] = useState<string | null>(null);

  const refreshHosts = useCallback(async () => {
    try {
      setHosts(await listHosts());
    } catch (e) {
      setNotice({ tone: 'error', text: `Hosts konnten nicht geladen werden: ${String(e)}` });
    }
  }, []);

  useEffect(() => {
    void refreshHosts();
  }, [refreshHosts]);

  /** Show a dialog and wait for its answer. */
  function ask<T>(build: (resolve: (value: T) => void) => Dialog): Promise<T> {
    return new Promise<T>((resolve) => {
      setDialog(
        build((value) => {
          setDialog(null);
          resolve(value);
        }),
      );
    });
  }

  const openShell = useCallback(async () => {
    const driver = driverRef.current;
    if (!driver) return;
    setNotice(null);
    setActive({ kind: 'shell' });
    driver.term.reset();
    try {
      await driver.attach(
        (onData, onEnd) => spawnShellSession(driver.term.cols, driver.term.rows, onData, onEnd),
        () => {
          setActive((current) => (current ? { ...current, ended: true } : current));
          setNotice({
            tone: 'info',
            text: 'Die lokale Shell wurde beendet.',
            action: { label: 'Neu starten', run: () => void openShell() },
          });
        },
      );
      driver.term.focus();
    } catch (e) {
      // A superseded driver (StrictMode's second mount) failing is expected.
      if (driverRef.current === driver) {
        setNotice({ tone: 'error', text: `Die lokale Shell startet nicht: ${String(e)}` });
      }
    }
  }, []);

  const connect = useCallback(
    async (host: HostRecord) => {
      const driver = driverRef.current;
      // One attempt at a time: a second click, or an Enter that reaches the
      // host row, must not start a parallel connection.
      if (!driver || connectingRef.current) return;
      connectingRef.current = host.id;

      setNotice(null);
      setConnectingId(host.id);
      // The secret lives in this one variable, for the one attempt that needs
      // it, and is dropped the moment that attempt is over.
      let secret: string | null = null;
      let lastKind: ConnectFailure['kind'] | null = null;

      try {
        for (;;) {
          driver.term.reset();
          try {
            await driver.attach(
              (onData, onEnd) =>
                connectHost(host.id, driver.term.cols, driver.term.rows, secret, onData, onEnd),
              () => {
                setActive((current) => (current ? { ...current, ended: true } : current));
                setNotice({
                  tone: 'info',
                  text: `Die Verbindung zu ${host.name} wurde beendet.`,
                  action: { label: 'Neu verbinden', run: () => void connect(host) },
                });
              },
            );
            secret = null;
            setActive({ kind: 'ssh', host });
            driver.term.focus();
            void refreshHosts();
            return;
          } catch (raw) {
            secret = null;
            const failure = asConnectFailure(raw);
            const retry = lastKind === failure.kind;
            lastKind = failure.kind;

            switch (failure.kind) {
              case 'unknown-host-key': {
                const trusted = await ask<boolean>((resolve) => ({
                  kind: 'trust',
                  host,
                  observed: failure.observed,
                  resolve,
                }));
                if (!trusted) return;
                await trustHostKey(host.address, host.port, failure.observed.fingerprint);
                continue;
              }
              case 'host-key-changed': {
                const confirmation = await ask<string | null>((resolve) => ({
                  kind: 'changed',
                  host,
                  trustedFingerprint: failure.trustedFingerprint,
                  observed: failure.observed,
                  resolve,
                }));
                if (confirmation === null) {
                  setNotice({
                    tone: 'error',
                    text: `Nicht verbunden: Der Host-Key von ${host.address} hat sich geändert.`,
                  });
                  return;
                }
                await trustHostKey(
                  host.address,
                  host.port,
                  failure.observed.fingerprint,
                  confirmation,
                );
                continue;
              }
              case 'password-required':
              case 'passphrase-required':
              case 'passphrase-rejected': {
                secret = await ask<string | null>((resolve) => ({
                  kind: 'secret',
                  host,
                  secret: failure.kind === 'password-required' ? 'password' : 'passphrase',
                  retry: failure.kind === 'passphrase-rejected',
                  resolve,
                }));
                if (secret === null) {
                  void cancelConnect(host.id);
                  return;
                }
                continue;
              }
              case 'auth-rejected':
                // A rejected password gets another try; a rejected key would be
                // rejected again, so that one is reported instead.
                if (host.auth === 'password') {
                  secret = await ask<string | null>((resolve) => ({
                    kind: 'secret',
                    host,
                    secret: 'password',
                    retry: true,
                    resolve,
                  }));
                  if (secret === null) {
                    void cancelConnect(host.id);
                    return;
                  }
                  continue;
                }
                setNotice({ tone: 'error', text: describe(failure, host) });
                return;
              default:
                setNotice({
                  tone: 'error',
                  text: describe(failure, host),
                  action: retry ? undefined : { label: 'Nochmal', run: () => void connect(host) },
                });
                return;
            }
          }
        }
      } catch (e) {
        setNotice({ tone: 'error', text: String(e) });
      } finally {
        secret = null;
        connectingRef.current = null;
        setConnectingId(null);
      }
    },
    [refreshHosts],
  );

  const runM0 = useCallback(async () => {
    const driver = driverRef.current;
    if (!driver) return;

    setM0Running(true);
    setResults([]);
    setReportPath(null);
    setM0Error(null);
    setNotice(null);
    try {
      const report = await runSuite(driver, setProgress, setResults);
      setProgress(null);
      setReportPath(await m0Finish(report));
    } catch (e) {
      setM0Error(String(e));
    } finally {
      setProgress(null);
      setM0Running(false);
      if (driverRef.current === driver) void openShell();
    }
  }, [openShell]);

  const onReady = useCallback(
    (driver: TerminalDriver) => {
      driverRef.current = driver;
      setRenderer(driver.renderer);
      // Dev builds only: lets end-to-end tests read the terminal, whose text
      // never reaches the DOM with the WebGL renderer. Stripped from release.
      if (import.meta.env.DEV) {
        (window as unknown as { __uwusshDriver?: TerminalDriver }).__uwusshDriver = driver;
      }

      void (async () => {
        if (await m0Autorun()) {
          await new Promise((resolve) => window.setTimeout(resolve, AUTORUN_DELAY_MS));
          if (driverRef.current === driver) await runM0();
        } else if (driverRef.current === driver) {
          await openShell();
        }
      })();
    },
    [openShell, runM0],
  );

  const modalOpen = Boolean(dialog || form || importing);
  useEffect(() => {
    if (backgroundRef.current) backgroundRef.current.inert = modalOpen;
  }, [modalOpen]);

  const activeId =
    active?.kind === 'ssh' ? active.host.id : active?.kind === 'shell' ? 'shell' : null;
  const connecting = connectingId ? (hosts.find((host) => host.id === connectingId) ?? null) : null;

  return (
    <div className="shell">
      <div ref={backgroundRef} className="background">
        <header className="titlebar" data-tauri-drag-region>
          <Nyu size={22} blink={false} title="UwUSSH" />
          <span className="wordmark">
            <span>UwU</span>SSH
          </span>
          <span className="spacer" />
          <button
            className="quiet"
            onClick={() => void runM0()}
            disabled={m0Running}
            title="Durchsatz-Messung aus M0 erneut laufen lassen"
          >
            {m0Running ? 'Messung läuft…' : 'M0-Messung'}
          </button>
        </header>

        <div className="body">
          <HostList
            hosts={hosts}
            activeId={activeId}
            activeEnded={Boolean(active?.ended)}
            connectingId={connectingId}
            onConnect={(host) => void connect(host)}
            onLocalShell={() => void openShell()}
            onAdd={() => setForm({ host: null })}
            onEdit={(host) => setForm({ host })}
            onImport={() => setImporting(true)}
          />

          <main className="main">
            <div className="toolbar">
              <span className="session-title">
                {connecting ? (
                  <>
                    <b>{connecting.name}</b>
                    <span className="meta">verbindet…</span>
                  </>
                ) : active?.kind === 'ssh' ? (
                  <>
                    <b>{active.host.name}</b>
                    <span className="meta">
                      {active.host.username}@{active.host.address}
                      {active.host.port === 22 ? '' : `:${active.host.port}`}
                    </span>
                  </>
                ) : (
                  <b>Lokale Shell</b>
                )}
              </span>
              <span className="spacer" />
              {(m0Running || results.length > 0 || m0Error) && (
                <M0Status
                  renderer={renderer}
                  progress={progress}
                  reportPath={reportPath}
                  error={m0Error}
                />
              )}
            </div>

            {notice && (
              <div
                className="notice"
                data-tone={notice.tone}
                role={notice.tone === 'error' ? 'alert' : 'status'}
              >
                <span>{notice.text}</span>
                <span className="spacer" />
                {notice.action && (
                  <button onClick={notice.action.run}>{notice.action.label}</button>
                )}
                <button
                  className="icon-button"
                  onClick={() => setNotice(null)}
                  aria-label="Hinweis schließen"
                >
                  ×
                </button>
              </div>
            )}

            <M0Results results={results} />

            <div className="terminal-wrap">
              <TerminalView onReady={onReady} />
            </div>
          </main>
        </div>
      </div>

      {form && (
        <HostForm
          host={form.host}
          onCancel={() => setForm(null)}
          onSaved={(saved) => {
            setForm(null);
            void refreshHosts();
            if (active?.kind === 'ssh' && active.host.id === saved.id) {
              setActive({ kind: 'ssh', host: saved });
            }
          }}
          onDeleted={() => {
            setForm(null);
            void refreshHosts();
          }}
        />
      )}

      {importing && (
        <ImportDialog onClose={() => setImporting(false)} onImported={() => void refreshHosts()} />
      )}

      {dialog?.kind === 'secret' && (
        <SecretPrompt
          host={dialog.host}
          secret={dialog.secret}
          retry={dialog.retry}
          onSubmit={(value) => dialog.resolve(value)}
          onCancel={() => dialog.resolve(null)}
        />
      )}
      {dialog?.kind === 'trust' && (
        <TrustHostKey
          host={dialog.host}
          observed={dialog.observed}
          onTrust={() => dialog.resolve(true)}
          onCancel={() => dialog.resolve(false)}
        />
      )}
      {dialog?.kind === 'changed' && (
        <HostKeyChanged
          host={dialog.host}
          trustedFingerprint={dialog.trustedFingerprint}
          observed={dialog.observed}
          onReplace={(confirmation) => dialog.resolve(confirmation)}
          onCancel={() => dialog.resolve(null)}
        />
      )}
    </div>
  );
}
