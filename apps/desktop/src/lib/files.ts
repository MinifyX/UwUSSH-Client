/**
 * The file browser's IPC: a server over SFTP (as the user or as root), this
 * computer, and SMB shares, which Windows reads like local folders.
 */

import { Channel, invoke } from '@tauri-apps/api/core';
import type { ConnectFailure, SessionId } from './session';

export type EntryKind = 'file' | 'dir' | 'link' | 'other';

export type Entry = {
  name: string;
  path: string;
  kind: EntryKind;
  size: number;
  modifiedMs: number | null;
  permissions: number | null;
  owner: string | null;
  /** A symbolic link; one to a folder lists as a folder. */
  link: boolean;
};

export type FilesFailure =
  | ConnectFailure
  | { kind: 'not-found'; path: string }
  | { kind: 'permission-denied'; path: string }
  | { kind: 'already-exists'; path: string }
  | { kind: 'link'; path: string }
  | { kind: 'failed'; message: string }
  | { kind: 'unsafe-name'; name: string }
  | { kind: 'cancelled' }
  | { kind: 'local'; message: string };

export function asFilesFailure(error: unknown): FilesFailure {
  if (typeof error === 'object' && error !== null && 'kind' in error) {
    return error as FilesFailure;
  }
  return { kind: 'failed', message: String(error) };
}

/** In words for a notice. Connecting questions are handled before this. */
export function describeFilesFailure(failure: FilesFailure): string {
  switch (failure.kind) {
    case 'not-found':
      return `${failure.path} gibt es nicht.`;
    case 'permission-denied':
      return `Keine Berechtigung für ${failure.path}.`;
    case 'already-exists':
      return `${failure.path} gibt es schon.`;
    case 'unsafe-name':
      return `Der Name „${failure.name}“ vom Server ist hier kein gültiger Dateiname, die Übertragung wurde abgebrochen.`;
    case 'link':
      return `${failure.path} ist ein Link. Seine Rechte sind die seines Ziels – ändere sie dort.`;
    case 'cancelled':
      return 'Abgebrochen.';
    case 'local':
    case 'failed':
      return failure.message;
    case 'refused':
      return `Der Server erlaubt keinen Dateizugriff: ${failure.reason}`;
    case 'sudo-refused':
      return `sudo hat abgelehnt: ${failure.message}`;
    case 'no-sftp-server':
      return 'Auf dem Server wurde kein sftp-server gefunden, der als root laufen könnte.';
    case 'sudo-password-rejected':
      return 'sudo hat das Passwort nicht angenommen.';
    case 'sudo-password-required':
      return 'sudo braucht ein Passwort.';
    case 'internal':
      return failure.message;
    default:
      return `Fehler (${failure.kind}).`;
  }
}

export type OpenedFiles = { session: SessionId; home: string | null; root: boolean };

export function openFiles(
  id: string,
  attempt: string,
  secret: string | null,
  root: boolean,
  sudoPassword: string | null,
): Promise<OpenedFiles> {
  return invoke<OpenedFiles>('open_files', { id, attempt, secret, root, sudoPassword });
}

export function closeFiles(session: SessionId): Promise<void> {
  return invoke('close_files', { session });
}

export function remoteList(session: SessionId, path: string): Promise<Entry[]> {
  return invoke<Entry[]>('remote_list', { session, path });
}

export function remoteCanonicalize(session: SessionId, path: string): Promise<string> {
  return invoke<string>('remote_canonicalize', { session, path });
}

export function remoteMkdir(session: SessionId, path: string): Promise<void> {
  return invoke('remote_mkdir', { session, path });
}

export function remoteRename(session: SessionId, from: string, to: string): Promise<void> {
  return invoke('remote_rename', { session, from, to });
}

export function remoteRemove(session: SessionId, paths: string[]): Promise<void> {
  return invoke('remote_remove', { session, paths });
}

export function remoteChmod(session: SessionId, path: string, mode: number): Promise<void> {
  return invoke('remote_chmod', { session, path, mode });
}

export type TransferEvent =
  | { kind: 'started'; totalBytes: number }
  | { kind: 'progress'; doneBytes: number }
  | { kind: 'item'; name: string }
  | { kind: 'done' }
  | { kind: 'failed'; error: FilesFailure };

export type Direction = 'download' | 'upload';

let transferCounter = 0;

export function newTransferId(): string {
  transferCounter += 1;
  return `transfer-${Date.now().toString(36)}-${transferCounter}`;
}

/**
 * Remote → local folder, or local → remote folder, over SFTP. Without
 * `overwrite`, anything already there fails the transfer with
 * `already-exists` before a byte is written.
 */
export function transfer(
  id: string,
  session: SessionId,
  direction: Direction,
  sources: string[],
  target: string,
  overwrite: boolean,
  onEvent: (event: TransferEvent) => void,
): Promise<void> {
  const events = new Channel<TransferEvent>();
  events.onmessage = onEvent;
  return invoke('transfer', {
    transfer: id,
    session,
    direction,
    sources,
    target,
    overwrite,
    events,
  });
}

/** Local (or SMB) → local (or SMB) folder. `overwrite` as for {@link transfer}. */
export function localCopy(
  id: string,
  sources: string[],
  target: string,
  overwrite: boolean,
  onEvent: (event: TransferEvent) => void,
): Promise<void> {
  const events = new Channel<TransferEvent>();
  events.onmessage = onEvent;
  return invoke('local_copy', { transfer: id, sources, target, overwrite, events });
}

export function cancelTransfer(id: string): Promise<void> {
  return invoke('cancel_transfer', { transfer: id });
}

export type Place = {
  label: string;
  path: string;
  kind: 'home' | 'desktop' | 'documents' | 'downloads' | 'drive';
};

export function localPlaces(): Promise<Place[]> {
  return invoke<Place[]>('local_places');
}

export function localList(path: string): Promise<Entry[]> {
  return invoke<Entry[]>('local_list', { path });
}

export function localParent(path: string): Promise<string | null> {
  return invoke<string | null>('local_parent', { path });
}

export function localMkdir(path: string): Promise<void> {
  return invoke('local_mkdir', { path });
}

export function localRename(from: string, to: string): Promise<void> {
  return invoke('local_rename', { from, to });
}

/** To the recycle bin. */
export function localTrash(paths: string[]): Promise<void> {
  return invoke('local_trash', { paths });
}

/** `password` is always typed; an empty one means none (guest access). */
export function smbConnect(id: string, share: string, password: string): Promise<{ path: string }> {
  return invoke<{ path: string }>('smb_connect', { id, share, password });
}

// ── Paths ───────────────────────────────────────────────────────────────────

export function remoteJoin(dir: string, name: string): string {
  if (!dir) return name;
  return dir.endsWith('/') ? `${dir}${name}` : `${dir}/${name}`;
}

export function remoteParent(path: string): string | null {
  if (path === '/' || path === '') return null;
  const trimmed = path.replace(/\/+$/, '');
  const index = trimmed.lastIndexOf('/');
  return index <= 0 ? '/' : trimmed.slice(0, index);
}

export function localJoin(dir: string, name: string): string {
  return /[\\/]$/.test(dir) ? `${dir}${name}` : `${dir}\\${name}`;
}

const UNITS = ['B', 'KB', 'MB', 'GB', 'TB'];

export function formatSize(bytes: number): string {
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const digits = unit === 0 || value >= 100 ? 0 : 1;
  return `${value.toLocaleString('de-DE', { maximumFractionDigits: digits })} ${UNITS[unit]}`;
}

/** `rwxr-xr-x` from mode bits. */
export function formatMode(mode: number | null, kind: EntryKind, link = false): string {
  if (mode === null) return '';
  const bits = ['r', 'w', 'x'];
  let out = link || kind === 'link' ? 'l' : kind === 'dir' ? 'd' : '-';
  for (let shift = 6; shift >= 0; shift -= 3) {
    for (let bit = 0; bit < 3; bit += 1) {
      out += mode & (1 << (shift + 2 - bit)) ? bits[bit] : '-';
    }
  }
  return out;
}
