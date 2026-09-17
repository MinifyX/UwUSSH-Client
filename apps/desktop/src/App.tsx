import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  HostKeyChanged,
  SecretPrompt,
  TrustHostKey,
  UnlockVault,
} from './components/ConnectDialogs';
import { HostForm } from './components/HostForm';
import { HostList } from './components/HostList';
import { ImportDialog } from './components/ImportDialog';
import { M0Results, M0Status } from './components/M0Panel';
import { Modal } from './components/Modal';
import { NyuScene } from './components/nyu/scenes';
import { SettingsDialog, type SettingsSection } from './components/SettingsDialog';
import { TabBar } from './components/TabBar';
import { TerminalView } from './components/Terminal';
import { TitleBar } from './components/TitleBar';
import { UpdateHint } from './components/UpdateHint';
import type { Renderer, TerminalDriver } from './lib/driver';
import { runSuite, type Progress, type ScenarioResult } from './lib/m0';
import {
  asConnectFailure,
  cancelConnect,
  closeAllSessions,
  connectHost,
  installUpdate,
  listHosts,
  m0Autorun,
  m0Finish,
  setUpdateChannel,
  spawnShellSession,
  trustHostKey,
  unlockVault,
  updateStatus,
  type ConnectFailure,
  type HostRecord,
  type ObservedHostKey,
  type UpdateInfo,
} from './lib/session';
import { getSettings, useSettings } from './lib/settings';
import {
  createTab,
  isPasteKey,
  neighbourAfterClose,
  newTabId,
  shortcutFor,
  type Notice,
  type Tab,
  type TabKind,
} from './lib/tabs';

/** Long enough for layout and the WebGL context to settle before measuring. */
const AUTORUN_DELAY_MS = 1_500;

/** A question one tab's connection needs answered. Asked one at a time, in order. */
type Dialog = { tabId: string; cancel: () => void } & (
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
    }
  | { kind: 'unlock'; host: HostRecord; retry: boolean; resolve: (value: string | null) => void }
);

/** Failures that are not a question for the user, in words the user can act on. */
function describeFailure(failure: ConnectFailure, host: HostRecord): string {
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

/** Copies text, with the old way as a fallback where the clipboard API says no. */
async function copyText(text: string) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const area = document.createElement('textarea');
    area.value = text;
    document.body.append(area);
    area.select();
    document.execCommand('copy');
    area.remove();
  }
}

/**
 * Whatever a page before this one left open is closed first, exactly once —
 * StrictMode runs effects twice, and a second close would take the new tabs.
 */
let boot: Promise<boolean> | null = null;

export function App() {
  const settings = useSettings();

  // ── Tabs ──────────────────────────────────────────────────────────────────
  const [tabs, setTabs] = useState<Tab[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const tabsRef = useRef<Tab[]>([]);
  tabsRef.current = tabs;
  const activeRef = useRef<string | null>(null);
  activeRef.current = activeId;
  /** The terminal of each tab, outside React. */
  const drivers = useRef(new Map<string, TerminalDriver>());
  /** Tabs with a start in flight, so a double click on "reconnect" starts one. */
  const starting = useRef(new Set<string>());

  const [hosts, setHosts] = useState<HostRecord[]>([]);
  const hostsRef = useRef<HostRecord[]>([]);
  hostsRef.current = hosts;
  const [appNotice, setAppNotice] = useState<Notice | null>(null);
  const [dialogs, setDialogs] = useState<Dialog[]>([]);
  const dialogsRef = useRef<Dialog[]>([]);
  dialogsRef.current = dialogs;
  const [form, setForm] = useState<{ host: HostRecord | null } | null>(null);
  const [importing, setImporting] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState<SettingsSection | null>(null);
  const [confirmClose, setConfirmClose] = useState(false);
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  const [updateDismissed, setUpdateDismissed] = useState(false);
  const backgroundRef = useRef<HTMLDivElement>(null);

  const [renderer, setRenderer] = useState<Renderer | null>(null);
  const [m0TabId, setM0TabId] = useState<string | null>(null);
  const [progress, setProgress] = useState<Progress | null>(null);
  const [results, setResults] = useState<ScenarioResult[]>([]);
  const [reportPath, setReportPath] = useState<string | null>(null);
  const [m0Error, setM0Error] = useState<string | null>(null);

  const patchTab = useCallback((id: string, patch: Partial<Tab>) => {
    setTabs((current) =>
      current.map((tab) => (tab.id === id ? ({ ...tab, ...patch } as Tab) : tab)),
    );
  }, []);

  /** Still the same terminal in the same open tab? Anything else means: stop. */
  const alive = (id: string, driver: TerminalDriver) => drivers.current.get(id) === driver;

  const refreshHosts = useCallback(async () => {
    try {
      setHosts(await listHosts());
    } catch (e) {
      setAppNotice({ tone: 'error', text: `Hosts konnten nicht geladen werden: ${String(e)}` });
    }
  }, []);

  /** Show a dialog for a tab and wait for its answer. */
  function ask<T>(
    tabId: string,
    cancelValue: T,
    build: (resolve: (value: T) => void) => Omit<Dialog, 'tabId' | 'cancel'>,
  ): Promise<T> {
    return new Promise<T>((resolve) => {
      let done = false;
      const finish = (value: T) => {
        if (done) return;
        done = true;
        setDialogs((current) => current.filter((dialog) => dialog !== entry));
        resolve(value);
      };
      const entry = {
        ...build(finish),
        tabId,
        cancel: () => finish(cancelValue),
      } as Dialog;
      setDialogs((current) => [...current, entry]);
    });
  }

  // ── Starting what a tab shows ─────────────────────────────────────────────

  const runShell = async (id: string, driver: TerminalDriver) => {
    patchTab(id, { status: 'connecting', notice: null });
    driver.term.reset();
    try {
      await driver.attach(
        (onData, onEnd) => spawnShellSession(driver.term.cols, driver.term.rows, onData, onEnd),
        () =>
          patchTab(id, {
            status: 'ended',
            notice: {
              tone: 'info',
              text: 'Die lokale Shell wurde beendet.',
              action: { label: 'Neu starten', run: () => void restart(id) },
            },
          }),
      );
      if (!alive(id, driver)) return;
      patchTab(id, { status: 'live' });
      if (activeRef.current === id) driver.term.focus();
    } catch (e) {
      // A superseded driver (StrictMode's second mount, a closed tab) failing is expected.
      if (alive(id, driver)) {
        patchTab(id, {
          status: 'failed',
          notice: { tone: 'error', text: `Die lokale Shell startet nicht: ${String(e)}` },
        });
      }
    }
  };

  const runConnect = async (id: string, driver: TerminalDriver, host: HostRecord) => {
    patchTab(id, { status: 'connecting', notice: null });
    // The secret lives in this one variable, for the one attempt that needs
    // it, and is dropped the moment that attempt is over.
    let secret: string | null = null;
    let lastKind: ConnectFailure['kind'] | null = null;
    const gone = () => {
      if (alive(id, driver)) return false;
      void cancelConnect(id).catch(() => undefined);
      return true;
    };
    const fail = (notice: Notice) => {
      if (!gone()) patchTab(id, { status: 'failed', notice });
    };
    const reconnect = { label: 'Neu verbinden', run: () => void restart(id) };

    try {
      for (;;) {
        if (gone()) return;
        driver.term.reset();
        try {
          await driver.attach(
            (onData, onEnd) =>
              connectHost(host.id, id, driver.term.cols, driver.term.rows, secret, onData, onEnd),
            () =>
              patchTab(id, {
                status: 'ended',
                notice: {
                  tone: 'info',
                  text: `Die Verbindung zu ${host.name} wurde beendet.`,
                  action: reconnect,
                },
              }),
          );
          secret = null;
          if (gone()) return;
          patchTab(id, { status: 'live' });
          if (activeRef.current === id) driver.term.focus();
          void refreshHosts();
          return;
        } catch (raw) {
          secret = null;
          if (gone()) return;
          const failure = asConnectFailure(raw);
          const retry = lastKind === failure.kind;
          lastKind = failure.kind;

          switch (failure.kind) {
            case 'unknown-host-key': {
              const trusted = await ask<boolean>(id, false, (resolve) => ({
                kind: 'trust',
                host,
                observed: failure.observed,
                resolve,
              }));
              if (!trusted) {
                fail({
                  tone: 'info',
                  text: 'Nicht verbunden: Der Host-Key wurde nicht bestätigt.',
                  action: reconnect,
                });
                return;
              }
              await trustHostKey(host.address, host.port, failure.observed.fingerprint);
              continue;
            }
            case 'host-key-changed': {
              const confirmation = await ask<string | null>(id, null, (resolve) => ({
                kind: 'changed',
                host,
                trustedFingerprint: failure.trustedFingerprint,
                observed: failure.observed,
                resolve,
              }));
              if (confirmation === null) {
                fail({
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
            case 'vault-locked': {
              // Unlock, retrying inside the prompt on a wrong master
              // password, then connect again — no wasted round trip.
              let unlocked = false;
              let wrong = false;
              while (!unlocked) {
                const password = await ask<string | null>(id, null, (resolve) => ({
                  kind: 'unlock',
                  host,
                  retry: wrong,
                  resolve,
                }));
                if (password === null) {
                  fail({
                    tone: 'info',
                    text: 'Nicht verbunden: Der Tresor ist gesperrt.',
                    action: reconnect,
                  });
                  return;
                }
                try {
                  await unlockVault(password);
                  unlocked = true;
                } catch {
                  wrong = true;
                }
              }
              continue;
            }
            case 'password-required':
            case 'passphrase-required':
            case 'passphrase-rejected': {
              secret = await ask<string | null>(id, null, (resolve) => ({
                kind: 'secret',
                host,
                secret: failure.kind === 'password-required' ? 'password' : 'passphrase',
                retry: failure.kind === 'passphrase-rejected',
                resolve,
              }));
              if (secret === null) {
                void cancelConnect(id);
                fail({ tone: 'info', text: 'Nicht verbunden.', action: reconnect });
                return;
              }
              continue;
            }
            case 'auth-rejected':
              // A rejected password gets another try; a rejected key would be
              // rejected again, so that one is reported instead.
              if (host.auth === 'password') {
                secret = await ask<string | null>(id, null, (resolve) => ({
                  kind: 'secret',
                  host,
                  secret: 'password',
                  retry: true,
                  resolve,
                }));
                if (secret === null) {
                  void cancelConnect(id);
                  fail({ tone: 'info', text: 'Nicht verbunden.', action: reconnect });
                  return;
                }
                continue;
              }
              fail({ tone: 'error', text: describeFailure(failure, host) });
              return;
            default:
              fail({
                tone: 'error',
                text: describeFailure(failure, host),
                action: retry ? undefined : { label: 'Nochmal', run: () => void restart(id) },
              });
              return;
          }
        }
      }
    } catch (e) {
      fail({ tone: 'error', text: String(e) });
    } finally {
      secret = null;
    }
  };

  const runM0 = async (id: string, driver: TerminalDriver) => {
    setM0TabId(id);
    setResults([]);
    setReportPath(null);
    setM0Error(null);
    patchTab(id, { status: 'live', notice: null });
    try {
      const report = await runSuite(driver, setProgress, setResults);
      setProgress(null);
      setReportPath(await m0Finish(report));
    } catch (e) {
      if (alive(id, driver)) setM0Error(String(e));
    } finally {
      setProgress(null);
      if (alive(id, driver)) patchTab(id, { status: 'ended' });
    }
  };

  /** Starts whatever the tab is for, on its current terminal. */
  const start = async (id: string) => {
    const driver = drivers.current.get(id);
    const tab = tabsRef.current.find((candidate) => candidate.id === id);
    if (!driver || !tab || starting.current.has(id)) return;
    starting.current.add(id);
    try {
      if (tab.kind === 'shell') await runShell(id, driver);
      else if (tab.kind === 'm0') await runM0(id, driver);
      else {
        // The newest record for the host, in case it was edited meanwhile.
        const host = hostsRef.current.find((candidate) => candidate.id === tab.host.id) ?? tab.host;
        await runConnect(id, driver, host);
      }
    } finally {
      starting.current.delete(id);
    }
  };
  const startRef = useRef(start);
  startRef.current = start;

  const restart = (id: string) => startRef.current(id);

  // ── Opening, closing, switching ───────────────────────────────────────────

  const openTab = useCallback((kind: TabKind) => {
    setAppNotice(null);
    // Functional updates only: a status change another tab queued in the
    // same tick must not be overwritten by a stale list.
    const id = newTabId();
    setTabs((current) => [...current, createTab(current, kind, id)]);
    setActiveId(id);
    return id;
  }, []);

  const closeTab = useCallback((id: string) => {
    const current = tabsRef.current;
    if (!current.some((tab) => tab.id === id)) return;
    // Questions this tab was waiting for are answered with "cancel".
    for (const dialog of dialogsRef.current) if (dialog.tabId === id) dialog.cancel();
    if (activeRef.current === id) setActiveId(neighbourAfterClose(current, id));
    setTabs((list) => list.filter((tab) => tab.id !== id));
    // Unmounting the terminal disposes its driver, which closes the session.
  }, []);

  const onDriverReady = useCallback((id: string, driver: TerminalDriver) => {
    drivers.current.set(id, driver);
    setRenderer(driver.renderer);
    driver.term.attachCustomKeyEventHandler((event) => !isPasteKey(event, getSettings()));
    // Wait a tick: StrictMode disposes a first driver right away, and only the
    // one that is still there should start anything.
    window.setTimeout(() => {
      if (drivers.current.get(id) === driver) void startRef.current(id);
    }, 0);
  }, []);

  const onDriverDispose = useCallback((id: string, driver: TerminalDriver) => {
    if (drivers.current.get(id) === driver) drivers.current.delete(id);
  }, []);

  const connect = useCallback((host: HostRecord) => openTab({ kind: 'ssh', host }), [openTab]);
  const openShell = useCallback(() => openTab({ kind: 'shell' }), [openTab]);

  const duplicate = useCallback(
    (id: string | null) => {
      const tab = tabsRef.current.find((candidate) => candidate.id === id);
      if (!tab) return;
      if (tab.kind === 'ssh') openTab({ kind: 'ssh', host: tab.host });
      else openTab({ kind: 'shell' });
    },
    [openTab],
  );

  const runM0InNewTab = useCallback(() => {
    setSettingsOpen(null);
    openTab({ kind: 'm0' });
  }, [openTab]);

  // ── Start-up ──────────────────────────────────────────────────────────────

  useEffect(() => {
    void refreshHosts();
    boot ??= closeAllSessions()
      .catch(() => 0)
      .then(() => m0Autorun().catch(() => false));
    let cancelled = false;
    void boot.then(async (autorun) => {
      if (cancelled) return;
      if (autorun) {
        await new Promise((resolve) => window.setTimeout(resolve, AUTORUN_DELAY_MS));
        if (!cancelled && tabsRef.current.length === 0) openTab({ kind: 'm0' });
      } else if (getSettings().openShellOnStart && tabsRef.current.length === 0) {
        openTab({ kind: 'shell' });
      }
    });
    return () => {
      cancelled = true;
    };
  }, [openTab, refreshHosts]);

  // Dev builds only: lets end-to-end tests read the active terminal, whose
  // text never reaches the DOM with the WebGL renderer. Stripped from release.
  useEffect(() => {
    if (!import.meta.env.DEV) return;
    Object.defineProperty(window, '__uwusshDriver', {
      configurable: true,
      get: () => (activeRef.current ? drivers.current.get(activeRef.current) : undefined),
    });
  }, []);

  // ── Updates ───────────────────────────────────────────────────────────────

  useEffect(() => {
    void setUpdateChannel(settings.updateChannel).catch(() => undefined);
  }, [settings.updateChannel]);

  useEffect(() => {
    void updateStatus()
      .then((ready) => ready && setUpdate(ready))
      .catch(() => undefined);
    const stop = listen<UpdateInfo>('update:ready', (event) => {
      setUpdate(event.payload);
      setUpdateDismissed(false);
    });
    return () => void stop.then((unlisten) => unlisten());
  }, []);

  // ── Derived state ─────────────────────────────────────────────────────────

  const activeTab = tabs.find((tab) => tab.id === activeId) ?? null;
  const liveConnections = tabs.filter((tab) => tab.kind === 'ssh' && tab.status === 'live').length;
  const liveRef = useRef(0);
  liveRef.current = liveConnections;
  const onlineIds = useMemo(
    () =>
      new Set(
        tabs.flatMap((tab) => (tab.kind === 'ssh' && tab.status === 'live' ? [tab.host.id] : [])),
      ),
    [tabs],
  );
  const connectingIds = useMemo(
    () =>
      new Set(
        tabs.flatMap((tab) =>
          tab.kind === 'ssh' && tab.status === 'connecting' ? [tab.host.id] : [],
        ),
      ),
    [tabs],
  );
  const dialog = dialogs[0] ?? null;
  const modalOpen = Boolean(dialog || form || importing || settingsOpen || confirmClose);
  const modalRef = useRef(false);
  modalRef.current = modalOpen;

  useEffect(() => {
    if (backgroundRef.current) backgroundRef.current.inert = modalOpen;
  }, [modalOpen]);

  // A question belongs to its tab: show that tab while it is asked.
  useEffect(() => {
    if (dialog && dialog.tabId !== activeRef.current) setActiveId(dialog.tabId);
  }, [dialog]);

  // The shown tab gets the keyboard.
  useEffect(() => {
    if (!activeId || modalOpen) return;
    const frame = window.requestAnimationFrame(() => drivers.current.get(activeId)?.term.focus());
    return () => window.cancelAnimationFrame(frame);
  }, [activeId, modalOpen]);

  // ── Keyboard ──────────────────────────────────────────────────────────────

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (modalRef.current || event.type !== 'keydown') return;
      const id = activeRef.current;
      const driver = id ? drivers.current.get(id) : undefined;
      const action = shortcutFor(event, getSettings(), Boolean(driver?.term.hasSelection()));
      if (!action) return;
      event.preventDefault();
      event.stopPropagation();

      const list = tabsRef.current;
      const index = list.findIndex((tab) => tab.id === id);
      switch (action.kind) {
        case 'new-shell':
          openTab({ kind: 'shell' });
          break;
        case 'close-tab':
          if (id) closeTab(id);
          break;
        case 'duplicate-tab':
          duplicate(id);
          break;
        case 'next-tab':
        case 'previous-tab': {
          if (list.length < 2) break;
          const step = action.kind === 'next-tab' ? 1 : -1;
          setActiveId(list[(index + step + list.length) % list.length]!.id);
          break;
        }
        case 'select-tab': {
          const target = list[action.index];
          if (target) setActiveId(target.id);
          break;
        }
        case 'settings':
          setSettingsOpen('appearance');
          break;
        case 'copy': {
          const selection = driver?.term.getSelection();
          if (selection) {
            void copyText(selection);
            driver?.term.clearSelection();
          }
          break;
        }
      }
    };
    // Capture phase: the terminal must not see the app's own shortcuts.
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [closeTab, duplicate, openTab]);

  // ── Closing the window ────────────────────────────────────────────────────

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let stopped = false;
    void getCurrentWindow()
      .onCloseRequested((event) => {
        if (getSettings().confirmCloseWithSessions && liveRef.current > 0) {
          event.preventDefault();
          setConfirmClose(true);
        }
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

  // ── Render ────────────────────────────────────────────────────────────────

  const sidebarActive =
    activeTab?.kind === 'ssh' ? activeTab.host.id : activeTab?.kind === 'shell' ? 'shell' : null;
  const notice = activeTab?.notice ?? appNotice;
  const showM0 = activeTab !== null && activeTab.id === m0TabId;

  return (
    <div className="shell">
      <div ref={backgroundRef} className="background">
        <TitleBar onSettings={() => setSettingsOpen('appearance')} />

        <div className="body">
          <HostList
            hosts={hosts}
            activeId={sidebarActive}
            onlineIds={onlineIds}
            connectingIds={connectingIds}
            onConnect={connect}
            onLocalShell={openShell}
            onAdd={() => setForm({ host: null })}
            onEdit={(host) => setForm({ host })}
            onImport={() => setImporting(true)}
          />

          <main className="main">
            <TabBar
              tabs={tabs}
              activeId={activeId}
              onSelect={setActiveId}
              onClose={closeTab}
              onNewShell={openShell}
            />

            {activeTab && (
              <div className="toolbar">
                <span className="session-title">
                  <b>{activeTab.title}</b>
                  {activeTab.status === 'connecting' && activeTab.kind === 'ssh' ? (
                    <span className="meta">verbindet…</span>
                  ) : (
                    activeTab.subtitle && <span className="meta">{activeTab.subtitle}</span>
                  )}
                </span>
                <span className="spacer" />
                {showM0 && (
                  <M0Status
                    renderer={renderer}
                    progress={progress}
                    reportPath={reportPath}
                    error={m0Error}
                  />
                )}
              </div>
            )}

            {notice ? (
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
                  onClick={() =>
                    activeTab?.notice
                      ? patchTab(activeTab.id, { notice: null })
                      : setAppNotice(null)
                  }
                  aria-label="Hinweis schließen"
                >
                  ×
                </button>
              </div>
            ) : null}

            {showM0 && <M0Results results={results} />}

            <div className="terminal-wrap">
              {tabs.map((tab) => (
                <div
                  key={tab.id}
                  className="terminal-pane"
                  hidden={tab.id !== activeId}
                  role="tabpanel"
                  aria-label={tab.title}
                >
                  <TerminalView
                    onReady={(driver) => onDriverReady(tab.id, driver)}
                    onDispose={(driver) => onDriverDispose(tab.id, driver)}
                  />
                </div>
              ))}
              {tabs.length === 0 && (
                <div className="no-tabs">
                  <NyuScene name="pick" className="no-tabs-scene" />
                  <p className="no-tabs-title">Kein Tab offen</p>
                  <p className="no-tabs-text">
                    Klick links einen Host an – jede Verbindung bekommt ihren eigenen Tab, auch
                    mehrere zum selben Server.
                  </p>
                  <button className="primary" onClick={openShell}>
                    Lokale Shell öffnen
                  </button>
                </div>
              )}
            </div>
          </main>
        </div>
      </div>

      {update && !updateDismissed && !settingsOpen && (
        <UpdateHint
          update={update}
          openConnections={liveConnections}
          onLater={() => setUpdateDismissed(true)}
          onRestart={installUpdate}
        />
      )}

      {form && (
        <HostForm
          host={form.host}
          onCancel={() => setForm(null)}
          onSaved={(saved) => {
            setForm(null);
            void refreshHosts();
            // Open tabs of this host show the new name; they reconnect with the new data.
            setTabs((current) =>
              current.map((tab) =>
                tab.kind === 'ssh' && tab.host.id === saved.id
                  ? { ...tab, host: saved, title: saved.name }
                  : tab,
              ),
            );
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

      {settingsOpen && (
        <SettingsDialog
          initial={settingsOpen}
          onClose={() => setSettingsOpen(null)}
          update={update}
          onUpdateFound={(found) => {
            setUpdate(found);
            setUpdateDismissed(false);
          }}
          onInstallUpdate={() => {
            setSettingsOpen(null);
            setUpdateDismissed(false);
          }}
          onRunM0={runM0InNewTab}
        />
      )}

      {confirmClose && (
        <Modal
          title="UwUSSH schließen?"
          onCancel={() => setConfirmClose(false)}
          footer={
            <>
              <span className="spacer" />
              <button data-autofocus onClick={() => setConfirmClose(false)}>
                Abbrechen
              </button>
              <button
                className="primary"
                data-secondary
                onClick={() => void getCurrentWindow().destroy()}
              >
                Schließen
              </button>
            </>
          }
        >
          <p className="dialog-lead">
            {liveConnections === 1
              ? 'Eine SSH-Verbindung ist noch offen und wird getrennt.'
              : `${liveConnections} SSH-Verbindungen sind noch offen und werden getrennt.`}
          </p>
        </Modal>
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
      {dialog?.kind === 'unlock' && (
        <UnlockVault
          host={dialog.host}
          retry={dialog.retry}
          onSubmit={(value) => dialog.resolve(value)}
          onCancel={() => dialog.resolve(null)}
        />
      )}
    </div>
  );
}
