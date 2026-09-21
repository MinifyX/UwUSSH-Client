import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  HostKeyChanged,
  SecretPrompt,
  TrustHostKey,
  type SecretKind,
} from './components/ConnectDialogs';
import { FileBrowser } from './components/FileBrowser';
import { HostForm } from './components/HostForm';
import { HostList } from './components/HostList';
import { Icon } from './components/Icon';
import { ImportDialog } from './components/ImportDialog';
import { M0Results, M0Status } from './components/M0Panel';
import { Modal } from './components/Modal';
import { NyuScene } from './components/nyu/scenes';
import { SettingsDialog, type SettingsSection } from './components/SettingsDialog';
import { TabBar } from './components/TabBar';
import { TerminalView } from './components/Terminal';
import { TitleBar } from './components/TitleBar';
import { UpdateHint } from './components/UpdateHint';
import { VaultDialog } from './components/VaultDialog';
import type { Renderer, TerminalDriver } from './lib/driver';
import type { PasswordPrompt } from './lib/highlight';
import { openFiles, type OpenedFiles } from './lib/files';
import { runSuite, type Progress, type ScenarioResult } from './lib/m0';
import {
  asConnectFailure,
  cancelConnect,
  closeAllSessions,
  connectHost,
  installUpdate,
  listGroups,
  listHosts,
  m0Autorun,
  m0Finish,
  sessionCanTypePassword,
  setHostPassword,
  setUpdateChannel,
  spawnShellSession,
  trustHostKey,
  typeSessionPassword,
  updateStatus,
  vaultState,
  type ConnectFailure,
  type GroupRecord,
  type HostOsEvent,
  type HostRecord,
  type ObservedHostKey,
  type UpdateInfo,
  type Workspace,
} from './lib/session';
import { language, t } from './lib/i18n';
import { getSettings, useSettings } from './lib/settings';
import {
  createTab,
  describe,
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

type SecretAnswer = { value: string; save: boolean };

/** A question one tab's connection needs answered. Asked one at a time, in order. */
type Dialog = { tabId: string; cancel: () => void } & (
  | {
      kind: 'secret';
      host: HostRecord;
      secret: SecretKind;
      retry: boolean;
      canSave: boolean;
      resolve: (value: SecretAnswer | null) => void;
    }
  | { kind: 'trust'; host: HostRecord; observed: ObservedHostKey; resolve: (ok: boolean) => void }
  | {
      kind: 'changed';
      host: HostRecord;
      trustedFingerprint: string;
      observed: ObservedHostKey;
      resolve: (accept: boolean) => void;
    }
  | { kind: 'unlock'; reason: string; resolve: (unlocked: boolean) => void }
);

/** What connecting ended with: the thing it opened, or a notice for the tab. */
type Negotiated<T> = { ok: true; value: T } | { ok: false; notice: Notice | null };

/** Failures that are not a question for the user, in words the user can act on. */
function describeFailure(failure: ConnectFailure, host: HostRecord): string {
  switch (failure.kind) {
    case 'unreachable':
      return t('{address} ist nicht erreichbar: {reason}', {
        address: host.address,
        reason: failure.reason,
      });
    case 'key-unreadable':
      return failure.keyPath === '<vault>'
        ? t('Der Key aus dem Tresor ließ sich nicht lesen: {reason}', { reason: failure.reason })
        : t('Der Key {path} ließ sich nicht lesen: {reason}', {
            path: failure.keyPath,
            reason: failure.reason,
          });
    case 'auth-rejected':
      return failure.remaining.length > 0
        ? t('Der Server hat die Anmeldung abgelehnt. Er würde akzeptieren: {methods}.', {
            methods: failure.remaining.join(', '),
          })
        : t('Der Server hat die Anmeldung abgelehnt.');
    case 'session-refused':
      return t('Angemeldet, aber der Server hat kein Terminal geöffnet: {reason}', {
        reason: failure.reason,
      });
    case 'protocol':
      return t('SSH-Fehler: {reason}', { reason: failure.reason });
    case 'refused':
      return t('Der Server erlaubt keinen Dateizugriff: {reason}', { reason: failure.reason });
    case 'sudo-refused':
      return t('sudo hat abgelehnt: {message}', { message: failure.message });
    case 'no-sftp-server':
      return t('Auf dem Server gibt es kein sftp-server, das als root laufen könnte.');
    case 'internal':
      return failure.message;
    default:
      return t('Verbindung fehlgeschlagen ({kind}).', { kind: failure.kind });
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
  /** When each tab was last in front, so a click on its host can bring back the right one. */
  const lastShown = useRef(new Map<string, number>());
  useEffect(() => {
    if (activeId) lastShown.current.set(activeId, Date.now());
  }, [activeId]);
  /** The terminal of each tab, outside React. */
  const drivers = useRef(new Map<string, TerminalDriver>());
  /** Tabs with a start in flight, so a double click on "reconnect" starts one. */
  const starting = useRef(new Set<string>());

  const [hosts, setHosts] = useState<HostRecord[]>([]);
  const [groups, setGroups] = useState<GroupRecord[]>([]);
  const hostsRef = useRef<HostRecord[]>([]);
  hostsRef.current = hosts;
  const [appNotice, setAppNotice] = useState<Notice | null>(null);
  const [dialogs, setDialogs] = useState<Dialog[]>([]);
  const dialogsRef = useRef<Dialog[]>([]);
  dialogsRef.current = dialogs;
  const [form, setForm] = useState<{
    host: HostRecord | null;
    workspace?: Workspace;
    group?: string | null;
  } | null>(null);
  const [importing, setImporting] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState<SettingsSection | null>(null);
  const [confirmClose, setConfirmClose] = useState(false);
  const [startupVault, setStartupVault] = useState(false);
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
  const tabOpen = (id: string) => tabsRef.current.some((tab) => tab.id === id);

  const refreshHosts = useCallback(async () => {
    try {
      const [loadedHosts, loadedGroups] = await Promise.all([listHosts(), listGroups()]);
      setHosts(loadedHosts);
      setGroups(loadedGroups);
      // Open tabs show the newest record: name, icon, what it logs in with.
      setTabs((current) =>
        current.map((tab) => {
          if (tab.kind !== 'ssh' && tab.kind !== 'files') return tab;
          const fresh = loadedHosts.find((h) => h.id === tab.host.id);
          return fresh && fresh !== tab.host
            ? ({ ...tab, host: fresh, title: fresh.name } as Tab)
            : tab;
        }),
      );
    } catch (e) {
      setAppNotice({
        tone: 'error',
        text: t('Hosts konnten nicht geladen werden: {error}', { error: String(e) }),
      });
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

  const unlock = (tabId: string, reason: string) =>
    ask<boolean>(tabId, false, (resolve) => ({ kind: 'unlock', reason, resolve }));

  // ── Connecting: the conversation every server connection goes through ─────

  /**
   * Try `attempt` until it opens or the user stops: trust a new host key,
   * accept or reject a changed one, open the vault, type a password or
   * passphrase, give sudo its password. Secrets live in local variables for
   * the one attempt that needs them.
   */
  const negotiate = async <T,>(
    id: string,
    host: HostRecord,
    attempt: (answers: { secret: string | null; sudo: string | null }) => Promise<T>,
    gone: () => boolean,
  ): Promise<Negotiated<T>> => {
    let secret: string | null = null;
    let save = false;
    let sudo: string | null = null;
    let lastKind: ConnectFailure['kind'] | null = null;
    const reconnect = { label: t('Neu verbinden'), run: () => void restart(id) };
    const stop = (notice: Notice | null): Negotiated<T> => ({ ok: false, notice });

    const askSecret = async (kind: SecretKind, retry: boolean) => {
      const answer = await ask<SecretAnswer | null>(id, null, (resolve) => ({
        kind: 'secret',
        host,
        secret: kind,
        retry,
        canSave: kind === 'password',
        resolve,
      }));
      return answer;
    };

    for (;;) {
      if (gone()) return stop(null);
      try {
        const value = await attempt({ secret, sudo });
        if (secret !== null && save) {
          const typed = secret;
          void (async () => {
            for (let tries = 0; tries < 2; tries += 1) {
              try {
                await setHostPassword(host.id, typed);
                void refreshHosts();
                return;
              } catch (error) {
                const failure = error as { kind?: string };
                if (failure?.kind !== 'vault-locked') throw error;
                const opened = await unlock(
                  id,
                  t('Das Passwort wird verschlüsselt im Tresor gespeichert.'),
                );
                if (!opened) return;
              }
            }
          })().catch((e) =>
            setAppNotice({
              tone: 'error',
              text: t('Passwort nicht gespeichert: {error}', { error: String(e) }),
            }),
          );
        }
        return { ok: true, value };
      } catch (raw) {
        if (gone()) {
          void cancelConnect(id).catch(() => undefined);
          return stop(null);
        }
        const failure = asConnectFailure(raw);
        const retry = lastKind === failure.kind;
        lastKind = failure.kind;
        const keepSecret =
          failure.kind === 'sudo-password-required' || failure.kind === 'sudo-password-rejected';
        if (!keepSecret) {
          secret = null;
          save = false;
        }
        sudo = null;

        switch (failure.kind) {
          case 'unknown-host-key': {
            const trusted = await ask<boolean>(id, false, (resolve) => ({
              kind: 'trust',
              host,
              observed: failure.observed,
              resolve,
            }));
            if (!trusted) {
              return stop({
                tone: 'info',
                text: t('Nicht verbunden: Der Host-Key wurde nicht bestätigt.'),
                action: reconnect,
              });
            }
            await trustHostKey(host.address, host.port, failure.observed.fingerprint);
            continue;
          }
          case 'host-key-changed': {
            const accepted = await ask<boolean>(id, false, (resolve) => ({
              kind: 'changed',
              host,
              trustedFingerprint: failure.trustedFingerprint,
              observed: failure.observed,
              resolve,
            }));
            if (!accepted) {
              return stop({
                tone: 'error',
                text: t('Nicht verbunden: Der Host-Key von {address} hat sich geändert.', {
                  address: host.address,
                }),
              });
            }
            await trustHostKey(host.address, host.port, failure.observed.fingerprint, true);
            continue;
          }
          case 'vault-locked': {
            const opened = await unlock(
              id,
              t('Die Anmeldedaten für {name} liegen im Tresor.', { name: host.name }),
            );
            if (!opened) {
              return stop({
                tone: 'info',
                text: t('Nicht verbunden: Der Tresor ist gesperrt.'),
                action: reconnect,
              });
            }
            continue;
          }
          case 'password-required':
          case 'passphrase-required':
          case 'passphrase-rejected': {
            const answer = await askSecret(
              failure.kind === 'password-required' ? 'password' : 'passphrase',
              failure.kind === 'passphrase-rejected',
            );
            if (!answer) {
              void cancelConnect(id);
              return stop({ tone: 'info', text: t('Nicht verbunden.'), action: reconnect });
            }
            secret = answer.value;
            save = answer.save;
            continue;
          }
          case 'auth-rejected':
            // A rejected password gets another try; a rejected key would be
            // rejected again, so that one is reported instead.
            if (host.auth === 'password') {
              const answer = await askSecret('password', true);
              if (!answer) {
                void cancelConnect(id);
                return stop({ tone: 'info', text: t('Nicht verbunden.'), action: reconnect });
              }
              secret = answer.value;
              save = answer.save;
              continue;
            }
            return stop({ tone: 'error', text: describeFailure(failure, host) });
          case 'sudo-password-required':
          case 'sudo-password-rejected': {
            const answer = await askSecret('sudo', failure.kind === 'sudo-password-rejected');
            if (!answer) {
              void cancelConnect(id);
              return stop({ tone: 'info', text: t('Nicht als root geöffnet.') });
            }
            sudo = answer.value;
            continue;
          }
          default:
            return stop({
              tone: 'error',
              text: describeFailure(failure, host),
              action: retry ? undefined : { label: t('Nochmal'), run: () => void restart(id) },
            });
        }
      }
    }
  };

  // ── Starting what a tab shows ─────────────────────────────────────────────

  /**
   * The helper offers itself on its own only to sudo or doas asking for this
   * login's password — a prompt that names another user wants another one.
   */
  const asksForLogin = (id: string, prompt: PasswordPrompt | null) => {
    const tab = tabsRef.current.find((candidate) => candidate.id === id);
    return (
      tab?.kind === 'ssh' &&
      prompt !== null &&
      (prompt.user === null || prompt.user === tab.host.username)
    );
  };

  const watchPrompts = (id: string, driver: TerminalDriver) =>
    driver.onPasswordPrompt((prompt) => {
      if (alive(id, driver)) patchTab(id, { prompt: asksForLogin(id, prompt) });
    });

  const runShell = async (id: string, driver: TerminalDriver) => {
    patchTab(id, { status: 'connecting', notice: null });
    driver.resetScreen();
    try {
      await driver.attach(
        (onData, onEnd) => spawnShellSession(driver.term.cols, driver.term.rows, onData, onEnd),
        () =>
          patchTab(id, {
            status: 'ended',
            notice: {
              tone: 'info',
              text: t('Die lokale Shell wurde beendet.'),
              action: { label: t('Neu starten'), run: () => void restart(id) },
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
          notice: {
            tone: 'error',
            text: t('Die lokale Shell startet nicht: {error}', { error: String(e) }),
          },
        });
      }
    }
  };

  const runConnect = async (id: string, driver: TerminalDriver, host: HostRecord) => {
    patchTab(id, { status: 'connecting', notice: null, prompt: false, canTypePassword: false });
    const reconnect = { label: t('Neu verbinden'), run: () => void restart(id) };
    try {
      const result = await negotiate(
        id,
        host,
        ({ secret }) => {
          driver.resetScreen();
          return driver.attach(
            (onData, onEnd) =>
              connectHost(host.id, id, driver.term.cols, driver.term.rows, secret, onData, onEnd),
            () =>
              patchTab(id, {
                status: 'ended',
                prompt: false,
                canTypePassword: false,
                notice: {
                  tone: 'info',
                  text: t('Die Verbindung zu {name} wurde beendet.', { name: host.name }),
                  action: reconnect,
                },
              }),
          );
        },
        () => !alive(id, driver),
      );
      if (!alive(id, driver)) return;
      if (!result.ok) {
        patchTab(id, { status: 'failed', notice: result.notice });
        return;
      }
      patchTab(id, { status: 'live' });
      if (activeRef.current === id) driver.term.focus();
      void refreshHosts();
      const canType = await sessionCanTypePassword(result.value).catch(() => false);
      if (alive(id, driver)) patchTab(id, { canTypePassword: canType });
    } catch (e) {
      if (alive(id, driver))
        patchTab(id, { status: 'failed', notice: { tone: 'error', text: String(e) } });
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
      else if (tab.kind === 'ssh') {
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

  const restart = (id: string) => {
    const tab = tabsRef.current.find((candidate) => candidate.id === id);
    if (tab?.kind === 'files') {
      // A file tab reconnects by mounting its browser anew.
      patchTab(id, { reload: (tab.reload ?? 0) + 1, notice: null, status: 'connecting' });
      return;
    }
    void startRef.current(id);
  };

  /** Opening a file tab's server side, through the same conversation. */
  const openFilesIn =
    (id: string, host: HostRecord) =>
    async (root: boolean): Promise<OpenedFiles | null> => {
      patchTab(id, { status: 'connecting', notice: null });
      const fresh = hostsRef.current.find((candidate) => candidate.id === host.id) ?? host;
      const result = await negotiate(
        id,
        fresh,
        ({ secret, sudo }) => openFiles(fresh.id, id, secret, root, sudo),
        () => !tabOpen(id),
      );
      if (!tabOpen(id)) return null;
      if (!result.ok) {
        patchTab(id, { status: 'failed', notice: result.notice });
        return null;
      }
      patchTab(id, { status: 'live' });
      return result.value;
    };

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
    watchPrompts(id, driver);
    // Wait a tick: StrictMode disposes a first driver right away, and only the
    // one that is still there should start anything.
    window.setTimeout(() => {
      if (drivers.current.get(id) === driver) void startRef.current(id);
    }, 0);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const onDriverDispose = useCallback((id: string, driver: TerminalDriver) => {
    if (drivers.current.get(id) === driver) drivers.current.delete(id);
  }, []);

  /** The open tab of this kind (and host) that was in front last, if there is one. */
  const recentTab = (match: (tab: Tab) => boolean) =>
    tabsRef.current
      .filter(match)
      .sort((a, b) => (lastShown.current.get(b.id) ?? 0) - (lastShown.current.get(a.id) ?? 0))[0];

  /**
   * A click in the sidebar: a host that already has a terminal tab gets that
   * tab brought to the front, one without gets a new one. Another tab to the
   * same host is in the host's context menu.
   */
  const connect = useCallback(
    (host: HostRecord) => {
      const open = recentTab((tab) => tab.kind === 'ssh' && tab.host.id === host.id);
      if (open) {
        setAppNotice(null);
        setActiveId(open.id);
        // A tab whose connection ended or never came up connects again: a
        // click on the host means "connect me", not "show me the error".
        if (open.status === 'failed' || open.status === 'ended') restart(open.id);
        return open.id;
      }
      return openTab({ kind: 'ssh', host });
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [openTab],
  );
  const connectAnother = useCallback(
    (host: HostRecord) => openTab({ kind: 'ssh', host }),
    [openTab],
  );
  const showShell = useCallback(() => {
    const open = recentTab((tab) => tab.kind === 'shell');
    if (open) {
      setAppNotice(null);
      setActiveId(open.id);
      return open.id;
    }
    return openTab({ kind: 'shell' });
  }, [openTab]);
  const openShell = useCallback(() => openTab({ kind: 'shell' }), [openTab]);
  const openFilesTab = useCallback(
    (host: HostRecord) => openTab({ kind: 'files', host }),
    [openTab],
  );

  const duplicate = useCallback(
    (id: string | null) => {
      const tab = tabsRef.current.find((candidate) => candidate.id === id);
      if (!tab) return;
      if (tab.kind === 'ssh') openTab({ kind: 'ssh', host: tab.host });
      else if (tab.kind === 'files') openTab({ kind: 'files', host: tab.host });
      else openTab({ kind: 'shell' });
    },
    [openTab],
  );

  const runM0InNewTab = useCallback(() => {
    setSettingsOpen(null);
    openTab({ kind: 'm0' });
  }, [openTab]);

  const typePassword = useCallback(
    (id: string) => {
      const driver = drivers.current.get(id);
      const session = driver?.session;
      if (!driver || !session) return;
      patchTab(id, { prompt: false });
      // Always typed, whatever asks. Enter only follows when the cursor sits
      // after a question (`Password:`), checked right before typing: at a
      // shell prompt — sudo timed out, the vault took a while — the password
      // must never run as a command or land in the history.
      const type = () =>
        typeSessionPassword(session, driver.waitsForAnswer()).then(() => driver.term.focus());
      void type().catch((error) => {
        const failure = error as { kind?: string };
        if (failure?.kind === 'vault-locked') {
          void unlock(id, t('Das gespeicherte Passwort liegt im Tresor.')).then(
            (opened) => opened && void type(),
          );
        } else {
          patchTab(id, {
            notice: {
              tone: 'error',
              text: t('Passwort nicht eingegeben: {error}', { error: String(error) }),
            },
          });
        }
      });
      // eslint-disable-next-line react-hooks/exhaustive-deps
    },
    [patchTab],
  );

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
        return;
      }
      // A vault this device doesn't open on its own asks once, now, instead
      // of on the first host that needs it.
      const vault = await vaultState().catch(() => null);
      if (!cancelled && vault?.status === 'locked' && !vault.remembered) setStartupVault(true);
      // Nothing opens on its own unless the settings say so: a local shell,
      // or the chosen hosts, each in its own tab, the first one in front.
      if (cancelled || tabsRef.current.length > 0) return;
      const { startup, startupHosts } = getSettings();
      if (startup === 'shell') {
        openTab({ kind: 'shell' });
      } else if (startup === 'hosts' && startupHosts.length > 0) {
        const known = await listHosts().catch(() => [] as HostRecord[]);
        if (cancelled || tabsRef.current.length > 0) return;
        const chosen = startupHosts.flatMap((id) => known.filter((host) => host.id === id));
        const ids = chosen.map((host) => openTab({ kind: 'ssh', host }));
        if (ids[0]) setActiveId(ids[0]);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [openTab, refreshHosts]);

  // The server told what it runs: the host list gets its icon.
  useEffect(() => {
    const stop = listen<HostOsEvent>('host:os', () => void refreshHosts());
    return () => void stop.then((unlisten) => unlisten());
  }, [refreshHosts]);

  // Another device changed hosts or groups: the list loads again.
  useEffect(() => {
    const stop = listen('sync:changed', () => void refreshHosts());
    return () => void stop.then((unlisten) => unlisten());
  }, [refreshHosts]);

  // Dev builds only: lets end-to-end tests read the active terminal, whose
  // text never reaches the DOM with the WebGL renderer. Stripped from release.
  useEffect(() => {
    if (!import.meta.env.DEV) return;
    Object.defineProperty(window, '__uwusshDriver', {
      configurable: true,
      get: () => (activeRef.current ? drivers.current.get(activeRef.current) : undefined),
    });
  }, []);

  // Tab names follow the language.
  const lang = language(settings);
  const tabsLanguage = useRef(lang);
  useEffect(() => {
    if (tabsLanguage.current === lang) return;
    tabsLanguage.current = lang;
    setTabs((current) => current.map((tab) => ({ ...tab, ...describe(tab) }) as Tab));
  }, [lang]);

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
  const liveConnections = tabs.filter(
    (tab) => (tab.kind === 'ssh' || tab.kind === 'files') && tab.status === 'live',
  ).length;
  const liveRef = useRef(0);
  liveRef.current = liveConnections;
  const openHostIds = useMemo(
    () => new Set(tabs.flatMap((tab) => (tab.kind === 'ssh' ? [tab.host.id] : []))),
    [tabs],
  );
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
          (tab.kind === 'ssh' || tab.kind === 'files') && tab.status === 'connecting'
            ? [tab.host.id]
            : [],
        ),
      ),
    [tabs],
  );
  const dialog = dialogs[0] ?? null;
  const modalOpen = Boolean(
    dialog || form || importing || settingsOpen || confirmClose || startupVault,
  );
  const modalRef = useRef(false);
  modalRef.current = modalOpen;

  useEffect(() => {
    if (backgroundRef.current) backgroundRef.current.inert = modalOpen;
  }, [modalOpen]);

  // A question belongs to its tab: show that tab while it is asked.
  useEffect(() => {
    if (dialog && dialog.tabId !== activeRef.current && tabOpen(dialog.tabId)) {
      setActiveId(dialog.tabId);
    }
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
      const tab = list[index];
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
        case 'type-password':
          if (tab?.kind === 'ssh' && tab.canTypePassword && id) typePassword(id);
          break;
        case 'open-files':
          if (tab?.kind === 'ssh' || tab?.kind === 'files') openFilesTab(tab.host);
          break;
      }
    };
    // Capture phase: the terminal must not see the app's own shortcuts.
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [closeTab, duplicate, openFilesTab, openTab, typePassword]);

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
    activeTab?.kind === 'ssh' || activeTab?.kind === 'files'
      ? activeTab.host.id
      : activeTab?.kind === 'shell'
        ? 'shell'
        : null;
  const notice = activeTab?.notice ?? appNotice;
  const showM0 = activeTab !== null && activeTab.id === m0TabId;
  const helperOn = settings.passwordHelper;
  const [helperBefore, helperAfter] = t('Passwort für {login} eintippen?').split('{login}');

  return (
    <div className="shell">
      <div ref={backgroundRef} className="background">
        <TitleBar onSettings={() => setSettingsOpen('appearance')} />

        <div className="body">
          <HostList
            hosts={hosts}
            groups={groups}
            activeId={sidebarActive}
            onlineIds={onlineIds}
            connectingIds={connectingIds}
            openIds={openHostIds}
            shellOpen={tabs.some((tab) => tab.kind === 'shell')}
            onConnect={connect}
            onConnectAnother={connectAnother}
            onOpenFiles={openFilesTab}
            onLocalShell={showShell}
            onAnotherShell={openShell}
            onAdd={(workspace, group) => setForm({ host: null, workspace, group })}
            onEdit={(host) => setForm({ host })}
            onImport={() => setImporting(true)}
            onChanged={() => void refreshHosts()}
            onError={(text) => setAppNotice({ tone: 'error', text })}
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
                    <span className="meta">{t('verbindet…')}</span>
                  ) : (
                    activeTab.subtitle && <span className="meta">{activeTab.subtitle}</span>
                  )}
                </span>
                <span className="spacer" />
                {activeTab.kind === 'ssh' &&
                  activeTab.canTypePassword &&
                  activeTab.status === 'live' && (
                    <button
                      className="quiet toolbar-button"
                      onClick={() => typePassword(activeTab.id)}
                      title={t(
                        'Das Passwort des Hosts ins Terminal tippen (Strg+Umschalt+P). Enter kommt nur dazu, wenn gerade etwas nach einer Eingabe fragt.',
                      )}
                    >
                      <Icon name="key" size={15} />
                      {t('Passwort eintippen')}
                    </button>
                  )}
                {activeTab.kind === 'ssh' && (
                  <button
                    className="quiet toolbar-button"
                    onClick={() => openFilesTab(activeTab.host)}
                    title={t('Dateien dieses Hosts in einem neuen Tab (Strg+Umschalt+F)')}
                  >
                    <Icon name="files" size={15} />
                    {t('Dateien')}
                  </button>
                )}
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
                  aria-label={t('Hinweis schließen')}
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
                  data-kind={tab.kind}
                  hidden={tab.id !== activeId}
                  role="tabpanel"
                  aria-label={tab.title}
                >
                  {tab.kind === 'files' ? (
                    <FileBrowser
                      key={tab.reload ?? 0}
                      host={tab.host}
                      open={openFilesIn(tab.id, tab.host)}
                      onLive={(live) => patchTab(tab.id, { status: live ? 'live' : 'ended' })}
                    />
                  ) : (
                    <TerminalView
                      onReady={(driver) => onDriverReady(tab.id, driver)}
                      onDispose={(driver) => onDriverDispose(tab.id, driver)}
                    />
                  )}
                  {tab.kind === 'ssh' &&
                    tab.status === 'connecting' &&
                    !dialogs.some((d) => d.tabId === tab.id) && (
                      <div className="pane-overlay" aria-live="polite">
                        <NyuScene name="connecting" className="pane-scene" />
                        <p>{t('Verbinde mit {name}…', { name: tab.host.name })}</p>
                      </div>
                    )}
                  {tab.kind === 'ssh' && tab.status === 'failed' && (
                    <div className="pane-overlay" data-tone="failed">
                      <NyuScene name="loadError" className="pane-scene" />
                      <p>{t('Nicht verbunden.')}</p>
                      <button className="primary" onClick={() => restart(tab.id)}>
                        {t('Neu verbinden')}
                      </button>
                    </div>
                  )}
                  {tab.kind === 'ssh' &&
                    helperOn &&
                    tab.prompt &&
                    tab.canTypePassword &&
                    tab.status === 'live' && (
                      <div className="password-helper" role="status">
                        <Icon name="key" size={16} />
                        <span>
                          {helperBefore}
                          <code>
                            {tab.host.username}@{tab.host.address}
                          </code>
                          {helperAfter}
                        </span>
                        <button className="primary" onClick={() => typePassword(tab.id)}>
                          {t('Eintippen')}
                        </button>
                        <kbd>{t('Strg+Umschalt+P')}</kbd>
                        <button
                          className="icon-button"
                          onClick={() => {
                            patchTab(tab.id, { prompt: false });
                            drivers.current.get(tab.id)?.term.focus();
                          }}
                          aria-label={t('Nicht eintippen')}
                        >
                          ×
                        </button>
                      </div>
                    )}
                </div>
              ))}
              {tabs.length === 0 && (
                <div className="no-tabs">
                  <NyuScene name="pick" className="no-tabs-scene" />
                  <p className="no-tabs-title">{t('Kein Tab offen')}</p>
                  <p className="no-tabs-text">
                    {t(
                      'Klick links einen Host an – jede Verbindung bekommt ihren eigenen Tab, auch mehrere zum selben Server.',
                    )}
                  </p>
                  <button className="primary" onClick={openShell}>
                    {t('Lokale Shell öffnen')}
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
          workspace={form.workspace}
          group={form.group}
          groups={groups}
          onCancel={() => setForm(null)}
          onSaved={() => {
            setForm(null);
            // Open tabs of this host show the new record; they reconnect with the new data.
            void refreshHosts();
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
          onImport={() => {
            setSettingsOpen(null);
            setImporting(true);
          }}
          onChanged={() => void refreshHosts()}
        />
      )}

      {startupVault && !dialog && (
        <VaultDialog
          reason={t(
            'Einmal entsperren – dann verbinden alle Hosts mit gespeicherten Passwörtern und Keys, ohne weiter zu fragen.',
          )}
          cancelLabel={t('Später')}
          onDone={() => {
            setStartupVault(false);
            void refreshHosts();
          }}
          onCancel={() => setStartupVault(false)}
        />
      )}

      {confirmClose && (
        <Modal
          title={t('UwUSSH schließen?')}
          onCancel={() => setConfirmClose(false)}
          footer={
            <>
              <span className="spacer" />
              <button data-autofocus onClick={() => setConfirmClose(false)}>
                {t('Abbrechen')}
              </button>
              <button
                className="primary"
                data-secondary
                onClick={() => void getCurrentWindow().destroy()}
              >
                {t('Schließen')}
              </button>
            </>
          }
        >
          <NyuScene name="goodbye" className="dialog-scene" />
          <p className="dialog-lead">
            {liveConnections === 1
              ? t('Eine Verbindung ist noch offen und wird getrennt.')
              : t('{count} Verbindungen sind noch offen und werden getrennt.', {
                  count: liveConnections,
                })}
          </p>
        </Modal>
      )}

      {dialog?.kind === 'secret' && (
        <SecretPrompt
          host={dialog.host}
          secret={dialog.secret}
          retry={dialog.retry}
          canSave={dialog.canSave}
          onSubmit={(value, save) => dialog.resolve({ value, save })}
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
          onAccept={() => dialog.resolve(true)}
          onReject={() => dialog.resolve(false)}
        />
      )}
      {dialog?.kind === 'unlock' && (
        <VaultDialog
          reason={dialog.reason}
          onDone={() => dialog.resolve(true)}
          onCancel={() => dialog.resolve(false)}
        />
      )}
    </div>
  );
}
