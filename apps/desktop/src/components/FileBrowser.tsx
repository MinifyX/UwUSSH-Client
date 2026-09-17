import { getCurrentWebview } from '@tauri-apps/api/webview';
import { useCallback, useEffect, useRef, useState } from 'react';
import { beginDrag } from '../lib/dnd';
import {
  asFilesFailure,
  cancelTransfer,
  closeFiles,
  describeFilesFailure,
  formatMode,
  formatSize,
  localCopy,
  localJoin,
  localList,
  localMkdir,
  localParent,
  localPlaces,
  localRename,
  localTrash,
  newTransferId,
  remoteChmod,
  remoteJoin,
  remoteList,
  remoteMkdir,
  remoteParent,
  remoteRemove,
  remoteRename,
  smbConnect,
  transfer,
  type Entry,
  type OpenedFiles,
  type Place,
  type TransferEvent,
} from '../lib/files';
import { locale, t, useLanguage } from '../lib/i18n';
import type { HostRecord } from '../lib/session';
import { Icon } from './Icon';
import { Modal } from './Modal';
import { NyuScene } from './nyu/scenes';

type Side = 'local' | 'remote';

type Props = {
  host: HostRecord;
  /** Opens the server side through the app's connection questions. `null`: not opened. */
  open: (root: boolean) => Promise<OpenedFiles | null>;
  /** The tab's state for the tab bar. */
  onLive: (live: boolean) => void;
};

type PaneState = {
  path: string;
  entries: Entry[];
  loading: boolean;
  error: string | null;
  selected: Set<string>;
  /** The last row clicked, for Shift+click ranges. */
  anchor: string | null;
};

const emptyPane = (path = ''): PaneState => ({
  path,
  entries: [],
  loading: false,
  error: null,
  selected: new Set(),
  anchor: null,
});

type Remote =
  { kind: 'sftp'; session: string; root: boolean; home: string } | { kind: 'smb'; share: string };

type Transfer = {
  id: string;
  label: string;
  direction: 'upload' | 'download' | 'copy';
  total: number;
  done: number;
  item: string;
  state: 'running' | 'done' | 'failed' | 'cancelled';
  error?: string;
  /** Which pane to refresh when it ends. */
  refresh: Side;
};

type Ask =
  | { kind: 'mkdir'; side: Side; value: string }
  | { kind: 'rename'; side: Side; entry: Entry; value: string }
  | { kind: 'delete'; side: Side; entries: Entry[] }
  | { kind: 'chmod'; entry: Entry; value: string }
  | { kind: 'smb'; share: string; password: string }
  | { kind: 'overwrite'; path: string; retry: () => void };

function modified(ms: number | null) {
  if (!ms) return '';
  return new Date(ms).toLocaleString(locale(), { dateStyle: 'short', timeStyle: 'short' });
}

function withCode(text: string, codes: Record<string, string>) {
  return text.split(/(\{\w+\})/).map((part, index) => {
    const code = codes[part.slice(1, -1)];
    return /^\{\w+\}$/.test(part) && code !== undefined ? <code key={index}>{code}</code> : part;
  });
}

/** Local paths use backslashes on Windows, remote ones slashes; SMB shares are local paths. */
function isLocalStyle(side: Side, remote: Remote | null) {
  return side === 'local' || remote?.kind === 'smb';
}

export function FileBrowser({ host, open, onLive }: Props) {
  useLanguage();
  const [places, setPlaces] = useState<Place[]>([]);
  const [local, setLocal] = useState<PaneState>(emptyPane());
  const [remotePane, setRemotePane] = useState<PaneState>(emptyPane());
  const [remote, setRemote] = useState<Remote | null>(null);
  const [opening, setOpening] = useState(false);
  const [transfers, setTransfers] = useState<Transfer[]>([]);
  const [ask, setAsk] = useState<Ask | null>(null);
  const [dropSide, setDropSide] = useState<Side | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const remoteRef = useRef<Remote | null>(null);
  remoteRef.current = remote;
  /** Still mounted: a connection that opens after the tab closed is closed again. */
  const mounted = useRef(false);
  const panesRef = useRef({ local, remote: remotePane });
  panesRef.current = { local, remote: remotePane };

  // ── Listing ───────────────────────────────────────────────────────────────

  const setPane = (side: Side, update: (pane: PaneState) => PaneState) =>
    side === 'local' ? setLocal(update) : setRemotePane(update);

  const list = useCallback(async (side: Side, path: string, keepSelection = false) => {
    const target = remoteRef.current;
    setPane(side, (pane) => ({ ...pane, path, loading: true, error: null }));
    try {
      const entries =
        side === 'local' || target?.kind === 'smb'
          ? await localList(path)
          : target?.kind === 'sftp'
            ? await remoteList(target.session, path)
            : [];
      setPane(side, (pane) =>
        pane.path === path
          ? {
              ...pane,
              entries,
              loading: false,
              selected: keepSelection
                ? new Set([...pane.selected].filter((name) => entries.some((e) => e.name === name)))
                : new Set(),
              anchor: keepSelection ? pane.anchor : null,
            }
          : pane,
      );
    } catch (error) {
      setPane(side, (pane) =>
        pane.path === path
          ? { ...pane, loading: false, error: describeFilesFailure(asFilesFailure(error)) }
          : pane,
      );
    }
  }, []);

  const refresh = (side: Side) => {
    const pane = panesRef.current[side];
    if (pane.path) void list(side, pane.path, true);
  };

  useEffect(() => {
    void localPlaces()
      .then((found) => {
        setPlaces(found);
        const home = found.find((p) => p.kind === 'home') ?? found[0];
        if (home) void list('local', home.path);
      })
      .catch((e) => setLocal((pane) => ({ ...pane, error: String(e) })));
  }, [list]);

  // ── The server side ───────────────────────────────────────────────────────

  const connect = useCallback(
    async (root: boolean) => {
      setOpening(true);
      setNotice(null);
      const previous = remoteRef.current;
      try {
        const opened = await open(root);
        if (!opened) return;
        if (!mounted.current) {
          void closeFiles(opened.session).catch(() => undefined);
          return;
        }
        if (previous?.kind === 'sftp') void closeFiles(previous.session).catch(() => undefined);
        const home = opened.home ?? '/';
        const next: Remote = { kind: 'sftp', session: opened.session, root: opened.root, home };
        remoteRef.current = next;
        setRemote(next);
        onLive(true);
        await list('remote', home);
      } finally {
        setOpening(false);
      }
    },
    [list, onLive, open],
  );

  useEffect(() => {
    mounted.current = true;
    // A tick later: StrictMode mounts twice in dev, and only the mount that
    // stays should connect (and ask its questions).
    const timer = window.setTimeout(() => void connect(false), 0);
    return () => {
      mounted.current = false;
      window.clearTimeout(timer);
      const current = remoteRef.current;
      if (current?.kind === 'sftp') void closeFiles(current.session).catch(() => undefined);
    };
    // Once per tab; switching root or SMB goes through `connect` and `openSmb`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const openSmb = async (share: string, password: string) => {
    setAsk(null);
    setOpening(true);
    try {
      const { path } = await smbConnect(host.id, share, password);
      if (!mounted.current) return;
      const current = remoteRef.current;
      if (current?.kind === 'sftp') void closeFiles(current.session).catch(() => undefined);
      const next: Remote = { kind: 'smb', share: path };
      remoteRef.current = next;
      setRemote(next);
      onLive(true);
      await list('remote', path);
    } catch (error) {
      setNotice(describeFilesFailure(asFilesFailure(error)));
    } finally {
      setOpening(false);
    }
  };

  // ── Transfers ─────────────────────────────────────────────────────────────

  const track = (item: Transfer, event: TransferEvent) => {
    setTransfers((list) =>
      list.map((job) => {
        if (job.id !== item.id) return job;
        switch (event.kind) {
          case 'started':
            return { ...job, total: event.totalBytes };
          case 'progress':
            return { ...job, done: event.doneBytes };
          case 'item':
            return { ...job, item: event.name };
          case 'done':
            return { ...job, state: 'done', done: Math.max(job.done, job.total) };
          case 'failed':
            return event.error.kind === 'cancelled'
              ? { ...job, state: 'cancelled' }
              : { ...job, state: 'failed', error: describeFilesFailure(event.error) };
        }
        return job;
      }),
    );
    if (event.kind === 'done' || event.kind === 'failed') {
      refresh(item.refresh);
      window.setTimeout(
        () =>
          setTransfers((list) =>
            list.filter((job) => job.id !== item.id || job.state === 'failed'),
          ),
        event.kind === 'done' ? 2600 : 0,
      );
    }
  };

  /**
   * Copy paths from one side into a folder on the other (or the same) side.
   * Something already there is asked about first, and only replaced on yes.
   */
  const copy = (from: Side, sources: string[], to: Side, folder: string, overwrite = false) => {
    const target = remoteRef.current;
    if (sources.length === 0 || !folder) return;
    const id = newTransferId();
    const names = sources.map((s) => s.split(/[\\/]/).filter(Boolean).pop() ?? s);
    const label = names.length === 1 ? names[0]! : t('{n} Elemente', { n: names.length });
    const viaSftp = target?.kind === 'sftp' && from !== to;
    const item: Transfer = {
      id,
      label,
      direction: !viaSftp ? 'copy' : to === 'remote' ? 'upload' : 'download',
      total: 0,
      done: 0,
      item: names[0] ?? '',
      state: 'running',
      refresh: to,
    };
    setTransfers((list) => [...list, item]);
    const onEvent = (event: TransferEvent) => {
      if (!overwrite && event.kind === 'failed' && event.error.kind === 'already-exists') {
        const path = event.error.path;
        setTransfers((list) => list.filter((job) => job.id !== id));
        setAsk({ kind: 'overwrite', path, retry: () => copy(from, sources, to, folder, true) });
        return;
      }
      track(item, event);
    };
    const started =
      viaSftp && target?.kind === 'sftp'
        ? transfer(
            id,
            target.session,
            item.direction as 'upload' | 'download',
            sources,
            folder,
            overwrite,
            onEvent,
          )
        : localCopy(id, sources, folder, overwrite, onEvent);
    void started.catch((error) => track(item, { kind: 'failed', error: asFilesFailure(error) }));
  };

  const selectedPaths = (side: Side) => {
    const pane = panesRef.current[side];
    return pane.entries.filter((e) => pane.selected.has(e.name)).map((e) => e.path);
  };

  // Files dragged in from Explorer: onto a pane, they are copied there.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let stopped = false;
    const sideAt = (x: number, y: number): Side | null => {
      const scale = window.devicePixelRatio || 1;
      const element = document.elementFromPoint(x / scale, y / scale);
      const pane = element?.closest<HTMLElement>('[data-file-pane]');
      return (pane?.dataset.filePane as Side | undefined) ?? null;
    };
    void getCurrentWebview()
      .onDragDropEvent((event) => {
        const payload = event.payload;
        if (payload.type === 'over') setDropSide(sideAt(payload.position.x, payload.position.y));
        else if (payload.type === 'leave') setDropSide(null);
        else if (payload.type === 'drop') {
          setDropSide(null);
          const side = sideAt(payload.position.x, payload.position.y);
          const folder = side ? panesRef.current[side].path : '';
          if (side && folder && payload.paths.length > 0)
            copy('local', payload.paths, side, folder);
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
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // ── Actions ───────────────────────────────────────────────────────────────

  const act = async (action: () => Promise<unknown>, side: Side) => {
    setAsk(null);
    try {
      await action();
    } catch (error) {
      setNotice(describeFilesFailure(asFilesFailure(error)));
    }
    refresh(side);
  };

  const join = (side: Side, dir: string, name: string) =>
    isLocalStyle(side, remoteRef.current) ? localJoin(dir, name) : remoteJoin(dir, name);

  const submitAsk = () => {
    if (!ask) return;
    const target = remoteRef.current;
    const sftp = target?.kind === 'sftp' ? target.session : null;
    switch (ask.kind) {
      case 'mkdir': {
        const name = ask.value.trim();
        if (!name) return;
        const path = join(ask.side, panesRef.current[ask.side].path, name);
        void act(
          () =>
            isLocalStyle(ask.side, target) || !sftp ? localMkdir(path) : remoteMkdir(sftp, path),
          ask.side,
        );
        return;
      }
      case 'rename': {
        const name = ask.value.trim();
        if (!name || name === ask.entry.name) return setAsk(null);
        const to = join(ask.side, panesRef.current[ask.side].path, name);
        void act(
          () =>
            isLocalStyle(ask.side, target) || !sftp
              ? localRename(ask.entry.path, to)
              : remoteRename(sftp, ask.entry.path, to),
          ask.side,
        );
        return;
      }
      case 'delete': {
        const paths = ask.entries.map((e) => e.path);
        void act(
          () => (ask.side === 'local' || !sftp ? localTrash(paths) : remoteRemove(sftp, paths)),
          ask.side,
        );
        return;
      }
      case 'chmod': {
        const mode = Number.parseInt(ask.value, 8);
        if (!sftp || !/^[0-7]{3,4}$/.test(ask.value) || Number.isNaN(mode)) return;
        void act(() => remoteChmod(sftp, ask.entry.path, mode), 'remote');
        return;
      }
      case 'smb':
        if (ask.share.trim()) void openSmb(ask.share, ask.password);
        return;
      case 'overwrite':
        setAsk(null);
        ask.retry();
        return;
    }
  };

  const up = async (side: Side) => {
    const pane = panesRef.current[side];
    const parent = isLocalStyle(side, remoteRef.current)
      ? await localParent(pane.path).catch(() => null)
      : remoteParent(pane.path);
    if (parent && parent !== pane.path) void list(side, parent);
  };

  // ── Render ────────────────────────────────────────────────────────────────

  const running = transfers.filter((job) => job.state === 'running');
  const remoteReady = remote !== null;
  const remoteTitle =
    remote?.kind === 'smb'
      ? `SMB · ${remote.share}`
      : `${remote?.kind === 'sftp' && remote.root ? 'root' : host.username}@${host.address}`;

  return (
    <div className="file-browser" data-drop-over={dropSide ?? undefined}>
      {notice && (
        <div className="notice" data-tone="error" role="alert">
          <span>{notice}</span>
          <span className="spacer" />
          <button
            className="icon-button"
            onClick={() => setNotice(null)}
            aria-label={t('Hinweis schließen')}
          >
            ×
          </button>
        </div>
      )}

      <div className="file-panes">
        <FilePane
          side="local"
          title={t('Dieser Computer')}
          icon="drive"
          pane={local}
          dropping={dropSide === 'local'}
          toolbar={
            <select
              className="select places"
              value=""
              onChange={(event) => event.target.value && void list('local', event.target.value)}
              aria-label={t('Ort wählen')}
            >
              <option value="">{t('Orte…')}</option>
              {places.map((place) => (
                <option key={place.path} value={place.path}>
                  {place.kind === 'drive' ? place.label : placeLabel(place)}
                </option>
              ))}
            </select>
          }
          onNavigate={(path) => void list('local', path)}
          onUp={() => void up('local')}
          onRefresh={() => refresh('local')}
          onChange={(update) => setLocal(update)}
          onMkdir={() => setAsk({ kind: 'mkdir', side: 'local', value: '' })}
          onRename={(entry) => setAsk({ kind: 'rename', side: 'local', entry, value: entry.name })}
          onDelete={(entries) => setAsk({ kind: 'delete', side: 'local', entries })}
          onTransfer={() =>
            remoteReady && copy('local', selectedPaths('local'), 'remote', remotePane.path)
          }
          transferLabel={t('Hochladen')}
          transferIcon="upload"
          canTransfer={remoteReady}
          onDragOut={(sources, target) => copy('local', sources, target.side, target.folder)}
        />

        <FilePane
          side="remote"
          title={remoteTitle}
          icon={remote?.kind === 'sftp' && remote.root ? 'crown' : 'network'}
          root={remote?.kind === 'sftp' && remote.root}
          pane={remotePane}
          dropping={dropSide === 'remote'}
          disabled={!remoteReady}
          toolbar={
            <>
              {remote?.kind === 'sftp' && (
                <button
                  className="icon-button"
                  onClick={() => void list('remote', remote.root ? '/' : remote.home)}
                  title={remote.root ? t('Zum Wurzelverzeichnis /') : t('Zum Home-Verzeichnis')}
                  aria-label={t('Start')}
                >
                  <Icon name="home" size={15} />
                </button>
              )}
              {remote?.kind === 'sftp' && (
                <button
                  className="icon-button"
                  onClick={() => void list('remote', '/')}
                  title={t('Dateisystem-Wurzel /')}
                  aria-label={t('Wurzelverzeichnis')}
                >
                  /
                </button>
              )}
              <button
                className="root-toggle"
                data-on={remote?.kind === 'sftp' && remote.root}
                disabled={opening}
                onClick={() => void connect(!(remote?.kind === 'sftp' && remote.root))}
                title={t('Die Dateien des Servers als root öffnen, über sudo')}
              >
                <Icon name="crown" size={14} />
                {remote?.kind === 'sftp' && remote.root ? 'root' : t('Als root')}
              </button>
              <button
                className="quiet smb-button"
                disabled={opening}
                onClick={() =>
                  remote?.kind === 'smb'
                    ? void connect(false)
                    : setAsk({ kind: 'smb', share: '', password: '' })
                }
                title={
                  remote?.kind === 'smb'
                    ? t('Zurück zu SFTP')
                    : t('Eine SMB-Freigabe auf diesem Host öffnen')
                }
              >
                {remote?.kind === 'smb' ? 'SFTP' : 'SMB'}
              </button>
            </>
          }
          onNavigate={(path) => void list('remote', path)}
          onUp={() => void up('remote')}
          onRefresh={() => refresh('remote')}
          onChange={(update) => setRemotePane(update)}
          onMkdir={() => setAsk({ kind: 'mkdir', side: 'remote', value: '' })}
          onRename={(entry) => setAsk({ kind: 'rename', side: 'remote', entry, value: entry.name })}
          onDelete={(entries) => setAsk({ kind: 'delete', side: 'remote', entries })}
          onChmod={
            remote?.kind === 'sftp'
              ? (entry) =>
                  setAsk({
                    kind: 'chmod',
                    entry,
                    value: ((entry.permissions ?? 0o644) & 0o7777).toString(8).padStart(3, '0'),
                  })
              : undefined
          }
          onTransfer={() => copy('remote', selectedPaths('remote'), 'local', local.path)}
          transferLabel={t('Herunterladen')}
          transferIcon="download"
          canTransfer={remoteReady}
          onDragOut={(sources, target) => copy('remote', sources, target.side, target.folder)}
          placeholder={
            opening ? (
              <div className="file-pane-empty">
                <NyuScene name="connecting" className="file-scene" />
                <p>{t('Verbinde mit {name}…', { name: host.name })}</p>
              </div>
            ) : !remoteReady ? (
              <div className="file-pane-empty">
                <NyuScene name="puzzled" className="file-scene" />
                <p>{t('Nicht verbunden.')}</p>
                <button className="primary" onClick={() => void connect(false)}>
                  {t('Verbinden')}
                </button>
              </div>
            ) : null
          }
        />
      </div>

      {transfers.length > 0 && (
        <div className="transfers" aria-live="polite">
          {running.length > 0 && <NyuScene name="files" className="transfer-scene" />}
          <ul>
            {transfers.map((job) => (
              <li key={job.id} data-state={job.state}>
                <Icon
                  name={
                    job.direction === 'upload'
                      ? 'upload'
                      : job.direction === 'download'
                        ? 'download'
                        : 'copy'
                  }
                  size={15}
                />
                <span className="transfer-name" title={job.item}>
                  {job.label}
                </span>
                <span className="transfer-bar">
                  <span
                    style={{
                      width: `${job.total ? Math.min(100, (job.done / job.total) * 100) : job.state === 'done' ? 100 : 4}%`,
                    }}
                  />
                </span>
                <span className="transfer-meta">
                  {job.state === 'failed'
                    ? job.error
                    : job.state === 'cancelled'
                      ? t('abgebrochen')
                      : job.state === 'done'
                        ? t('fertig ✧')
                        : `${formatSize(job.done)} / ${formatSize(job.total)}`}
                </span>
                {job.state === 'running' ? (
                  <button
                    className="icon-button"
                    onClick={() => void cancelTransfer(job.id)}
                    aria-label={t('{label} abbrechen', { label: job.label })}
                  >
                    <Icon name="stop" size={14} />
                  </button>
                ) : (
                  <button
                    className="icon-button"
                    onClick={() => setTransfers((list) => list.filter((x) => x.id !== job.id))}
                    aria-label={t('Ausblenden')}
                  >
                    ×
                  </button>
                )}
              </li>
            ))}
          </ul>
        </div>
      )}

      {ask && (
        <Modal
          title={
            ask.kind === 'mkdir'
              ? t('Neuer Ordner')
              : ask.kind === 'rename'
                ? t('{name} umbenennen', { name: ask.entry.name })
                : ask.kind === 'chmod'
                  ? t('Rechte von {name}', { name: ask.entry.name })
                  : ask.kind === 'smb'
                    ? t('SMB-Freigabe öffnen')
                    : ask.kind === 'overwrite'
                      ? t('Ersetzen?')
                      : ask.side === 'local'
                        ? t('In den Papierkorb legen?')
                        : t('Endgültig löschen?')
          }
          tone={ask.kind === 'delete' && ask.side === 'remote' ? 'warning' : 'default'}
          onCancel={() => setAsk(null)}
          footer={
            <>
              <span className="spacer" />
              <button data-secondary onClick={() => setAsk(null)}>
                {t('Abbrechen')}
              </button>
              <button
                className={ask.kind === 'delete' || ask.kind === 'overwrite' ? 'danger' : 'primary'}
                data-secondary={ask.kind === 'delete' || ask.kind === 'overwrite' || undefined}
                onClick={submitAsk}
              >
                {ask.kind === 'delete'
                  ? ask.side === 'local'
                    ? t('In den Papierkorb')
                    : t('Löschen')
                  : ask.kind === 'smb'
                    ? t('Öffnen')
                    : ask.kind === 'overwrite'
                      ? t('Ersetzen')
                      : 'OK'}
              </button>
            </>
          }
        >
          {ask.kind === 'overwrite' ? (
            <p className="dialog-lead">
              {withCode(
                t(
                  '{name} gibt es dort schon. Beim Ersetzen werden Dateien überschrieben und Ordner zusammengeführt.',
                ),
                { name: ask.path.split(/[\\/]/).filter(Boolean).pop() ?? ask.path },
              )}
            </p>
          ) : ask.kind === 'delete' ? (
            <p className="dialog-lead">
              {ask.entries.length === 1
                ? withCode(
                    ask.side === 'local'
                      ? t('{name} landet im Papierkorb.')
                      : t(
                          '{name} wird auf dem Server gelöscht, Ordner mit allem darin. Das lässt sich nicht rückgängig machen.',
                        ),
                    { name: ask.entries[0]!.name },
                  )
                : ask.side === 'local'
                  ? t('{n} Elemente landet im Papierkorb.', { n: ask.entries.length })
                  : t(
                      '{n} Elemente wird auf dem Server gelöscht, Ordner mit allem darin. Das lässt sich nicht rückgängig machen.',
                      { n: ask.entries.length },
                    )}
            </p>
          ) : (
            <form
              className="form"
              onSubmit={(event) => {
                event.preventDefault();
                submitAsk();
              }}
            >
              {ask.kind === 'smb' ? (
                <>
                  <p className="dialog-lead">
                    {withCode(t('Windows meldet sich an {share} mit dem Benutzer {user} an.'), {
                      share: `\\\\${host.address}\\…`,
                      user: host.username,
                    })}
                  </p>
                  <label className="field">
                    <span>{t('Freigabe')}</span>
                    <input
                      data-autofocus
                      value={ask.share}
                      placeholder={t('z. B. daten')}
                      onChange={(e) => setAsk({ ...ask, share: e.target.value })}
                    />
                  </label>
                  <label className="field">
                    <span>{t('Passwort für die Freigabe')}</span>
                    <input
                      type="password"
                      value={ask.password}
                      autoComplete="off"
                      onChange={(e) => setAsk({ ...ask, password: e.target.value })}
                    />
                    <em className="field-hint">
                      {t(
                        'Leer lassen für Gastzugriff. Das gespeicherte SSH-Passwort wird hier nie verwendet: SMB prüft keinen Host-Key.',
                      )}
                    </em>
                  </label>
                </>
              ) : (
                <label className="field">
                  <span>
                    {ask.kind === 'chmod' ? t('Rechte, oktal (z. B. 644 oder 755)') : t('Name')}
                  </span>
                  <input
                    data-autofocus
                    value={ask.value}
                    spellCheck={false}
                    onChange={(e) => setAsk({ ...ask, value: e.target.value })}
                  />
                  {ask.kind === 'chmod' && (
                    <em className="field-hint">
                      {/^[0-7]{3,4}$/.test(ask.value)
                        ? formatMode(Number.parseInt(ask.value, 8), ask.entry.kind)
                        : t('Drei oder vier Ziffern von 0 bis 7')}
                    </em>
                  )}
                </label>
              )}
              <button type="submit" hidden />
            </form>
          )}
        </Modal>
      )}
    </div>
  );
}

function placeLabel(place: Place) {
  return (
    {
      home: t('Benutzerordner'),
      desktop: t('Desktop'),
      documents: t('Dokumente'),
      downloads: t('Downloads'),
    }[place.kind as 'home'] ?? place.label
  );
}

// ── One side ────────────────────────────────────────────────────────────────

type PaneProps = {
  side: Side;
  title: string;
  icon: 'drive' | 'network' | 'crown';
  root?: boolean;
  pane: PaneState;
  disabled?: boolean;
  dropping: boolean;
  toolbar?: React.ReactNode;
  placeholder?: React.ReactNode;
  onNavigate: (path: string) => void;
  onUp: () => void;
  onRefresh: () => void;
  onChange: (update: (pane: PaneState) => PaneState) => void;
  onMkdir: () => void;
  onRename: (entry: Entry) => void;
  onDelete: (entries: Entry[]) => void;
  onChmod?: (entry: Entry) => void;
  onTransfer: () => void;
  transferLabel: string;
  transferIcon: 'upload' | 'download';
  canTransfer: boolean;
  onDragOut: (sources: string[], target: { side: Side; folder: string }) => void;
};

function FilePane(props: PaneProps) {
  useLanguage();
  const { side, pane } = props;
  const [pathInput, setPathInput] = useState<string | null>(null);
  const selectedEntries = pane.entries.filter((e) => pane.selected.has(e.name));

  const select = (entry: Entry, event: React.MouseEvent | React.KeyboardEvent) => {
    props.onChange((current) => {
      const selected = new Set(current.selected);
      if (event.shiftKey && current.anchor) {
        const names = current.entries.map((e) => e.name);
        const from = names.indexOf(current.anchor);
        const to = names.indexOf(entry.name);
        if (!event.ctrlKey) selected.clear();
        for (const name of names.slice(Math.min(from, to), Math.max(from, to) + 1)) {
          selected.add(name);
        }
        return { ...current, selected };
      }
      if (event.ctrlKey || event.metaKey) {
        if (selected.has(entry.name)) selected.delete(entry.name);
        else selected.add(entry.name);
      } else {
        selected.clear();
        selected.add(entry.name);
      }
      return { ...current, selected, anchor: entry.name };
    });
  };

  const startDrag = (event: React.PointerEvent, entry: Entry) => {
    const dragged = pane.selected.has(entry.name) ? selectedEntries : [entry];
    beginDrag(event, {
      label: dragged.length === 1 ? dragged[0]!.name : t('{n} Elemente', { n: dragged.length }),
      accept: (element) => {
        const kind = element.dataset.drop;
        if (kind === 'file-pane') return element.dataset.side !== side ? 'inside' : null;
        if (kind === 'file-folder') {
          const path = element.dataset.path ?? '';
          return dragged.some((d) => d.path === path) ? null : 'inside';
        }
        return null;
      },
      onDrop: (target) => {
        const toSide = target.data.side as Side;
        const folder = target.data.drop === 'file-folder' ? target.data.path : target.data.folder;
        if (toSide && folder)
          props.onDragOut(
            dragged.map((d) => d.path),
            { side: toSide, folder },
          );
      },
    });
  };

  return (
    <section
      className="file-pane"
      data-file-pane={side}
      data-root={props.root || undefined}
      data-dropping={props.dropping || undefined}
      data-drop="file-pane"
      data-side={side}
      data-folder={pane.path}
      aria-label={props.title}
    >
      <header className="file-pane-head">
        <Icon name={props.icon} size={16} />
        <b className="file-pane-title" title={props.title}>
          {props.title}
        </b>
        {props.root && <span className="root-badge">root</span>}
        <span className="spacer" />
        {props.toolbar}
      </header>

      <div className="file-pathbar">
        <button
          className="icon-button"
          onClick={props.onUp}
          disabled={props.disabled}
          title={t('Ein Verzeichnis nach oben')}
          aria-label={t('Nach oben')}
        >
          <Icon name="up" size={15} />
        </button>
        {pathInput === null ? (
          <button
            className="file-path"
            disabled={props.disabled}
            onClick={() => setPathInput(pane.path)}
            title={t('Klicken, um einen Pfad einzugeben')}
          >
            <bdi>{pane.path || '—'}</bdi>
          </button>
        ) : (
          <input
            className="file-path-input"
            autoFocus
            value={pathInput}
            spellCheck={false}
            onChange={(e) => setPathInput(e.target.value)}
            onBlur={() => setPathInput(null)}
            onKeyDown={(event) => {
              if (event.key === 'Enter' && pathInput.trim()) {
                props.onNavigate(pathInput.trim());
                setPathInput(null);
              }
              if (event.key === 'Escape') setPathInput(null);
            }}
            aria-label={t('Pfad')}
          />
        )}
        <button
          className="icon-button"
          onClick={props.onRefresh}
          disabled={props.disabled}
          title={t('Neu laden')}
          aria-label={t('Neu laden')}
        >
          <Icon name="refresh" size={15} />
        </button>
        <button
          className="icon-button"
          onClick={props.onMkdir}
          disabled={props.disabled}
          title={t('Neuer Ordner')}
          aria-label={t('Neuer Ordner')}
        >
          <Icon name="folderPlus" size={15} />
        </button>
      </div>

      {props.placeholder ?? (
        <div
          className="file-list"
          role="grid"
          aria-multiselectable="true"
          tabIndex={0}
          onKeyDown={(event) => {
            if (event.key === 'Delete' && selectedEntries.length > 0)
              props.onDelete(selectedEntries);
            if (event.key === 'F2' && selectedEntries.length === 1)
              props.onRename(selectedEntries[0]!);
            if (event.key === 'Backspace') props.onUp();
            if (event.key === 'a' && event.ctrlKey) {
              event.preventDefault();
              props.onChange((current) => ({
                ...current,
                selected: new Set(current.entries.map((e) => e.name)),
              }));
            }
          }}
        >
          <div className="file-row file-row-head" role="row">
            <span role="columnheader">{t('Name')}</span>
            <span role="columnheader">{t('Größe')}</span>
            <span role="columnheader">{t('Geändert')}</span>
            {side === 'remote' && <span role="columnheader">{t('Rechte')}</span>}
          </div>
          {pane.loading && pane.entries.length === 0 && <p className="file-note">{t('Lade…')}</p>}
          {pane.error && (
            <p className="file-note" data-tone="error">
              {pane.error}
            </p>
          )}
          {!pane.loading && !pane.error && pane.entries.length === 0 && (
            <p className="file-note">{t('Leerer Ordner')}</p>
          )}
          {pane.entries.slice(0, 3000).map((entry) => (
            <div
              key={entry.name}
              className="file-row"
              role="row"
              aria-selected={pane.selected.has(entry.name)}
              data-kind={entry.kind}
              data-drop={entry.kind === 'dir' ? 'file-folder' : undefined}
              data-path={entry.path}
              data-side={side}
              onPointerDown={(event) => startDrag(event, entry)}
              onClick={(event) => select(entry, event)}
              onDoubleClick={() => entry.kind === 'dir' && props.onNavigate(entry.path)}
              onContextMenu={(event) => {
                event.preventDefault();
                if (!pane.selected.has(entry.name)) select(entry, event);
              }}
            >
              <span className="file-name" role="gridcell">
                <Icon name={entry.kind === 'dir' ? 'folder' : 'file'} size={15} />
                <span>{entry.name}</span>
                {entry.link && (
                  <i className="file-link" title={t('Symbolischer Link')} aria-label={t('Link')}>
                    ↪
                  </i>
                )}
              </span>
              <span role="gridcell">{entry.kind === 'dir' ? '' : formatSize(entry.size)}</span>
              <span role="gridcell">{modified(entry.modifiedMs)}</span>
              {side === 'remote' && (
                <span role="gridcell" className="file-mode" title={entry.owner ?? undefined}>
                  {formatMode(entry.permissions, entry.kind, entry.link)}
                </span>
              )}
            </div>
          ))}
          {pane.entries.length > 3000 && (
            <p className="file-note">
              {t('{n} weitere Einträge – bitte in einem Unterordner weitersuchen.', {
                n: pane.entries.length - 3000,
              })}
            </p>
          )}
        </div>
      )}

      <footer className="file-actions">
        <span className="file-count">
          {selectedEntries.length > 0
            ? t('{n} ausgewählt', { n: selectedEntries.length })
            : pane.entries.length === 1
              ? t('1 Eintrag')
              : t('{n} Einträge', { n: pane.entries.length })}
        </span>
        <span className="spacer" />
        <button
          className="icon-button"
          disabled={selectedEntries.length !== 1}
          onClick={() => props.onRename(selectedEntries[0]!)}
          title={t('Umbenennen (F2)')}
          aria-label={t('Umbenennen')}
        >
          <Icon name="pencil" size={15} />
        </button>
        {props.onChmod && (
          <button
            className="icon-button"
            disabled={selectedEntries.length !== 1 || selectedEntries[0]!.link}
            onClick={() => props.onChmod?.(selectedEntries[0]!)}
            title={
              selectedEntries[0]?.link ? t('Ein Link hat keine eigenen Rechte') : t('Rechte ändern')
            }
            aria-label={t('Rechte ändern')}
          >
            <Icon name="shield" size={15} />
          </button>
        )}
        <button
          className="icon-button"
          disabled={selectedEntries.length === 0}
          onClick={() => props.onDelete(selectedEntries)}
          title={t('Löschen (Entf)')}
          aria-label={t('Löschen')}
        >
          <Icon name="trash" size={15} />
        </button>
        <button
          className="primary transfer-button"
          disabled={!props.canTransfer || selectedEntries.length === 0}
          onClick={props.onTransfer}
        >
          <Icon name={props.transferIcon} size={15} />
          {props.transferLabel}
        </button>
      </footer>
    </section>
  );
}
