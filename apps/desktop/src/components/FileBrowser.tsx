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
  return new Date(ms).toLocaleString('de-DE', { dateStyle: 'short', timeStyle: 'short' });
}

/** Local paths use backslashes on Windows, remote ones slashes; SMB shares are local paths. */
function isLocalStyle(side: Side, remote: Remote | null) {
  return side === 'local' || remote?.kind === 'smb';
}

export function FileBrowser({ host, open, onLive }: Props) {
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
      list.map((t) => {
        if (t.id !== item.id) return t;
        switch (event.kind) {
          case 'started':
            return { ...t, total: event.totalBytes };
          case 'progress':
            return { ...t, done: event.doneBytes };
          case 'item':
            return { ...t, item: event.name };
          case 'done':
            return { ...t, state: 'done', done: Math.max(t.done, t.total) };
          case 'failed':
            return event.error.kind === 'cancelled'
              ? { ...t, state: 'cancelled' }
              : { ...t, state: 'failed', error: describeFilesFailure(event.error) };
        }
        return t;
      }),
    );
    if (event.kind === 'done' || event.kind === 'failed') {
      refresh(item.refresh);
      window.setTimeout(
        () => setTransfers((list) => list.filter((t) => t.id !== item.id || t.state === 'failed')),
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
    const label = names.length === 1 ? names[0]! : `${names.length} Elemente`;
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
        setTransfers((list) => list.filter((t) => t.id !== id));
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

  const running = transfers.filter((t) => t.state === 'running');
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
            aria-label="Hinweis schließen"
          >
            ×
          </button>
        </div>
      )}

      <div className="file-panes">
        <FilePane
          side="local"
          title="Dieser Computer"
          icon="drive"
          pane={local}
          dropping={dropSide === 'local'}
          toolbar={
            <select
              className="select places"
              value=""
              onChange={(event) => event.target.value && void list('local', event.target.value)}
              aria-label="Ort wählen"
            >
              <option value="">Orte…</option>
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
          transferLabel="Hochladen"
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
                  title={remote.root ? 'Zum Wurzelverzeichnis /' : 'Zum Home-Verzeichnis'}
                  aria-label="Start"
                >
                  <Icon name="home" size={15} />
                </button>
              )}
              {remote?.kind === 'sftp' && (
                <button
                  className="icon-button"
                  onClick={() => void list('remote', '/')}
                  title="Dateisystem-Wurzel /"
                  aria-label="Wurzelverzeichnis"
                >
                  /
                </button>
              )}
              <button
                className="root-toggle"
                data-on={remote?.kind === 'sftp' && remote.root}
                disabled={opening}
                onClick={() => void connect(!(remote?.kind === 'sftp' && remote.root))}
                title="Die Dateien des Servers als root öffnen, über sudo"
              >
                <Icon name="crown" size={14} />
                {remote?.kind === 'sftp' && remote.root ? 'root' : 'Als root'}
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
                    ? 'Zurück zu SFTP'
                    : 'Eine SMB-Freigabe auf diesem Host öffnen'
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
          transferLabel="Herunterladen"
          transferIcon="download"
          canTransfer={remoteReady}
          onDragOut={(sources, target) => copy('remote', sources, target.side, target.folder)}
          placeholder={
            opening ? (
              <div className="file-pane-empty">
                <NyuScene name="connecting" className="file-scene" />
                <p>Verbinde mit {host.name}…</p>
              </div>
            ) : !remoteReady ? (
              <div className="file-pane-empty">
                <NyuScene name="puzzled" className="file-scene" />
                <p>Nicht verbunden.</p>
                <button className="primary" onClick={() => void connect(false)}>
                  Verbinden
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
            {transfers.map((t) => (
              <li key={t.id} data-state={t.state}>
                <Icon
                  name={
                    t.direction === 'upload'
                      ? 'upload'
                      : t.direction === 'download'
                        ? 'download'
                        : 'copy'
                  }
                  size={15}
                />
                <span className="transfer-name" title={t.item}>
                  {t.label}
                </span>
                <span className="transfer-bar">
                  <span
                    style={{
                      width: `${t.total ? Math.min(100, (t.done / t.total) * 100) : t.state === 'done' ? 100 : 4}%`,
                    }}
                  />
                </span>
                <span className="transfer-meta">
                  {t.state === 'failed'
                    ? t.error
                    : t.state === 'cancelled'
                      ? 'abgebrochen'
                      : t.state === 'done'
                        ? 'fertig ✧'
                        : `${formatSize(t.done)} / ${formatSize(t.total)}`}
                </span>
                {t.state === 'running' ? (
                  <button
                    className="icon-button"
                    onClick={() => void cancelTransfer(t.id)}
                    aria-label={`${t.label} abbrechen`}
                  >
                    <Icon name="stop" size={14} />
                  </button>
                ) : (
                  <button
                    className="icon-button"
                    onClick={() => setTransfers((list) => list.filter((x) => x.id !== t.id))}
                    aria-label="Ausblenden"
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
              ? 'Neuer Ordner'
              : ask.kind === 'rename'
                ? `${ask.entry.name} umbenennen`
                : ask.kind === 'chmod'
                  ? `Rechte von ${ask.entry.name}`
                  : ask.kind === 'smb'
                    ? 'SMB-Freigabe öffnen'
                    : ask.kind === 'overwrite'
                      ? 'Ersetzen?'
                      : ask.side === 'local'
                        ? 'In den Papierkorb legen?'
                        : 'Endgültig löschen?'
          }
          tone={ask.kind === 'delete' && ask.side === 'remote' ? 'warning' : 'default'}
          onCancel={() => setAsk(null)}
          footer={
            <>
              <span className="spacer" />
              <button data-secondary onClick={() => setAsk(null)}>
                Abbrechen
              </button>
              <button
                className={ask.kind === 'delete' || ask.kind === 'overwrite' ? 'danger' : 'primary'}
                data-secondary={ask.kind === 'delete' || ask.kind === 'overwrite' || undefined}
                onClick={submitAsk}
              >
                {ask.kind === 'delete'
                  ? ask.side === 'local'
                    ? 'In den Papierkorb'
                    : 'Löschen'
                  : ask.kind === 'smb'
                    ? 'Öffnen'
                    : ask.kind === 'overwrite'
                      ? 'Ersetzen'
                      : 'OK'}
              </button>
            </>
          }
        >
          {ask.kind === 'overwrite' ? (
            <p className="dialog-lead">
              <code>{ask.path.split(/[\\/]/).filter(Boolean).pop() ?? ask.path}</code> gibt es dort
              schon. Beim Ersetzen werden Dateien überschrieben und Ordner zusammengeführt.
            </p>
          ) : ask.kind === 'delete' ? (
            <p className="dialog-lead">
              {ask.entries.length === 1 ? (
                <code>{ask.entries[0]!.name}</code>
              ) : (
                `${ask.entries.length} Elemente`
              )}
              {ask.side === 'local'
                ? ' landet im Papierkorb.'
                : ' wird auf dem Server gelöscht, Ordner mit allem darin. Das lässt sich nicht rückgängig machen.'}
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
                    Windows meldet sich an <code>\\{host.address}\…</code> mit dem Benutzer{' '}
                    <code>{host.username}</code> an.
                  </p>
                  <label className="field">
                    <span>Freigabe</span>
                    <input
                      data-autofocus
                      value={ask.share}
                      placeholder="z. B. daten"
                      onChange={(e) => setAsk({ ...ask, share: e.target.value })}
                    />
                  </label>
                  <label className="field">
                    <span>Passwort für die Freigabe</span>
                    <input
                      type="password"
                      value={ask.password}
                      autoComplete="off"
                      onChange={(e) => setAsk({ ...ask, password: e.target.value })}
                    />
                    <em className="field-hint">
                      Leer lassen für Gastzugriff. Das gespeicherte SSH-Passwort wird hier nie
                      verwendet: SMB prüft keinen Host-Key.
                    </em>
                  </label>
                </>
              ) : (
                <label className="field">
                  <span>
                    {ask.kind === 'chmod' ? 'Rechte, oktal (z. B. 644 oder 755)' : 'Name'}
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
                        : 'Drei oder vier Ziffern von 0 bis 7'}
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
    { home: 'Benutzerordner', desktop: 'Desktop', documents: 'Dokumente', downloads: 'Downloads' }[
      place.kind as 'home'
    ] ?? place.label
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
      label: dragged.length === 1 ? dragged[0]!.name : `${dragged.length} Elemente`,
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
          title="Ein Verzeichnis nach oben"
          aria-label="Nach oben"
        >
          <Icon name="up" size={15} />
        </button>
        {pathInput === null ? (
          <button
            className="file-path"
            disabled={props.disabled}
            onClick={() => setPathInput(pane.path)}
            title="Klicken, um einen Pfad einzugeben"
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
            aria-label="Pfad"
          />
        )}
        <button
          className="icon-button"
          onClick={props.onRefresh}
          disabled={props.disabled}
          title="Neu laden"
          aria-label="Neu laden"
        >
          <Icon name="refresh" size={15} />
        </button>
        <button
          className="icon-button"
          onClick={props.onMkdir}
          disabled={props.disabled}
          title="Neuer Ordner"
          aria-label="Neuer Ordner"
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
            <span role="columnheader">Name</span>
            <span role="columnheader">Größe</span>
            <span role="columnheader">Geändert</span>
            {side === 'remote' && <span role="columnheader">Rechte</span>}
          </div>
          {pane.loading && pane.entries.length === 0 && <p className="file-note">Lade…</p>}
          {pane.error && (
            <p className="file-note" data-tone="error">
              {pane.error}
            </p>
          )}
          {!pane.loading && !pane.error && pane.entries.length === 0 && (
            <p className="file-note">Leerer Ordner</p>
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
                  <i className="file-link" title="Symbolischer Link" aria-label="Link">
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
              {pane.entries.length - 3000} weitere Einträge – bitte in einem Unterordner
              weitersuchen.
            </p>
          )}
        </div>
      )}

      <footer className="file-actions">
        <span className="file-count">
          {selectedEntries.length > 0
            ? `${selectedEntries.length} ausgewählt`
            : `${pane.entries.length} ${pane.entries.length === 1 ? 'Eintrag' : 'Einträge'}`}
        </span>
        <span className="spacer" />
        <button
          className="icon-button"
          disabled={selectedEntries.length !== 1}
          onClick={() => props.onRename(selectedEntries[0]!)}
          title="Umbenennen (F2)"
          aria-label="Umbenennen"
        >
          <Icon name="pencil" size={15} />
        </button>
        {props.onChmod && (
          <button
            className="icon-button"
            disabled={selectedEntries.length !== 1 || selectedEntries[0]!.link}
            onClick={() => props.onChmod?.(selectedEntries[0]!)}
            title={selectedEntries[0]?.link ? 'Ein Link hat keine eigenen Rechte' : 'Rechte ändern'}
            aria-label="Rechte ändern"
          >
            <Icon name="shield" size={15} />
          </button>
        )}
        <button
          className="icon-button"
          disabled={selectedEntries.length === 0}
          onClick={() => props.onDelete(selectedEntries)}
          title="Löschen (Entf)"
          aria-label="Löschen"
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
