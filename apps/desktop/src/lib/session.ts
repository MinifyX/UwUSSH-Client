/**
 * The IPC surface, in one file.
 *
 * Everything the UI knows about Tauri lives here.
 */

import { Channel, invoke } from '@tauri-apps/api/core';

export type SessionId = string;

/** Must match `ACK_CHUNK` in `crates/uwussh-core/src/flow.rs`. */
export const ACK_CHUNK = 64 * 1024;

export type MetricsSnapshot = {
  bytesTotal: number;
  framesTotal: number;
  readerStalls: number;
  flowPauses: number;
  largestFrame: number;
  elapsedSecs: number;
  bytesPerSec: number;
  framesPerSec: number;
  meanFrameBytes: number;
  flowControl: boolean;
  unacked: number;
  peakUnacked: number;
  finished: boolean;
  childExited: boolean;
};

export type DataHandler = (bytes: Uint8Array) => void;
export type EndHandler = () => void;

/** How a session is created: give it somewhere to put bytes and a way to say it ended. */
export type Spawner = (onData: DataHandler, onEnd: EndHandler) => Promise<SessionId>;

/**
 * Frames arrive as raw bytes. Depending on the Tauri version they land as an
 * ArrayBuffer or as a plain number array, so normalise both rather than
 * guessing — a wrong guess here shows up as an empty terminal with no error.
 */
function toBytes(message: unknown): Uint8Array | null {
  if (message instanceof ArrayBuffer) return new Uint8Array(message);
  if (ArrayBuffer.isView(message)) {
    return new Uint8Array(message.buffer, message.byteOffset, message.byteLength);
  }
  if (Array.isArray(message)) return new Uint8Array(message as number[]);
  return null;
}

function channelFor(onData: DataHandler, onEnd: EndHandler): Channel<unknown> {
  const channel = new Channel<unknown>();
  channel.onmessage = (message) => {
    const bytes = toBytes(message);
    if (!bytes) return;
    // Real frames are never empty; an empty one is the engine saying the
    // stream is over.
    if (bytes.length === 0) onEnd();
    else onData(bytes);
  };
  return channel;
}

// ── Sessions ────────────────────────────────────────────────────────────────

export function spawnShellSession(
  cols: number,
  rows: number,
  onData: DataHandler,
  onEnd: EndHandler,
): Promise<SessionId> {
  return invoke<SessionId>('spawn_shell_session', {
    cols,
    rows,
    onData: channelFor(onData, onEnd),
  });
}

export function writeSession(id: SessionId, data: string): Promise<void> {
  return invoke('write_session', { id, data });
}

export function resizeSession(id: SessionId, cols: number, rows: number): Promise<void> {
  return invoke('resize_session', { id, cols, rows });
}

/** Tell the engine the renderer has processed `bytes` more bytes. */
export function ackSession(id: SessionId, bytes: number): Promise<void> {
  return invoke('ack_session', { id, bytes });
}

export function closeSession(id: SessionId): Promise<void> {
  return invoke('close_session', { id });
}

export function sessionMetrics(id: SessionId): Promise<MetricsSnapshot> {
  return invoke<MetricsSnapshot>('session_metrics', { id });
}

// ── Hosts ───────────────────────────────────────────────────────────────────

export type AuthMethod = 'password' | 'key';

/** Private or business, like UwUMail's workspaces. */
export type Workspace = 'private' | 'business';

export type HostRecord = {
  id: string;
  name: string;
  address: string;
  port: number;
  username: string;
  auth: AuthMethod;
  keyPath: string | null;
  groupPath: string | null;
  lastConnectedMs: number | null;
  workspace: Workspace;
  position: number;
  /** What the last connection found the server to run, like `ubuntu`. */
  os: string | null;
  /** A password is stored in the vault: for logging in, or for sudo. */
  hasPassword: boolean;
  keyId: string | null;
  keyLabel: string | null;
};

export type PasswordChange = { kind: 'keep' } | { kind: 'set'; value: string } | { kind: 'forget' };

export type HostDraft = {
  id: string | null;
  name: string;
  address: string;
  port: number;
  username: string;
  auth: AuthMethod;
  keyPath: string | null;
  groupPath: string | null;
  workspace: Workspace | null;
  keyId: string | null;
  password: PasswordChange;
};

export type SaveFailure =
  | { kind: 'invalid'; field: string; problem: string }
  | { kind: 'vault-locked' }
  | { kind: 'error'; message: string };

export function listHosts(): Promise<HostRecord[]> {
  return invoke<HostRecord[]>('list_hosts');
}

export function saveHost(draft: HostDraft): Promise<HostRecord> {
  return invoke<HostRecord>('save_host', { draft });
}

export function deleteHost(id: string): Promise<void> {
  return invoke('delete_host', { id });
}

// ── Groups and order ────────────────────────────────────────────────────────

export type GroupRecord = { workspace: Workspace; name: string; position: number };

export function listGroups(): Promise<GroupRecord[]> {
  return invoke<GroupRecord[]>('list_groups');
}

export function createGroup(workspace: Workspace, name: string): Promise<GroupRecord> {
  return invoke<GroupRecord>('create_group', { workspace, name });
}

export function renameGroup(workspace: Workspace, from: string, to: string): Promise<void> {
  return invoke('rename_group', { workspace, from, to });
}

export function deleteGroup(workspace: Workspace, name: string): Promise<void> {
  return invoke('delete_group', { workspace, name });
}

export function moveGroup(
  workspace: Workspace,
  name: string,
  to: Workspace,
  before: string | null,
): Promise<void> {
  return invoke('move_group', { workspace, name, to, before });
}

export function moveHost(
  id: string,
  to: Workspace,
  group: string | null,
  before: string | null,
): Promise<HostRecord> {
  return invoke<HostRecord>('move_host', { id, to, group, before });
}

// ── Connecting ──────────────────────────────────────────────────────────────

export type ObservedHostKey = {
  algorithm: string;
  fingerprint: string;
  publicKey: string;
  randomart: string;
};

/** Mirrors `SshError` plus the desktop layer's `internal`. */
export type ConnectFailure =
  | { kind: 'unreachable'; address: string; reason: string }
  | { kind: 'unknown-host-key'; observed: ObservedHostKey }
  | { kind: 'host-key-changed'; trustedFingerprint: string; observed: ObservedHostKey }
  | { kind: 'password-required' }
  | { kind: 'passphrase-required'; keyPath: string }
  | { kind: 'passphrase-rejected'; keyPath: string }
  | { kind: 'key-unreadable'; keyPath: string; reason: string }
  | { kind: 'auth-rejected'; remaining: string[] }
  | { kind: 'session-refused'; reason: string }
  | { kind: 'protocol'; reason: string }
  | { kind: 'vault-locked' }
  | { kind: 'internal'; message: string }
  // File access (SFTP and sudo)
  | { kind: 'refused'; reason: string }
  | { kind: 'sudo-password-required' }
  | { kind: 'sudo-password-rejected' }
  | { kind: 'sudo-refused'; message: string }
  | { kind: 'no-sftp-server' };

export function asConnectFailure(error: unknown): ConnectFailure {
  if (typeof error === 'object' && error !== null && 'kind' in error) {
    return error as ConnectFailure;
  }
  return { kind: 'internal', message: String(error) };
}

/**
 * Open a shell on a host. `attempt` names the tab that connects, so two tabs to
 * the same host never share a half-open connection. `secret` is the password
 * or the key passphrase, depending on the host — sent for this one call and
 * not kept anywhere.
 */
export function connectHost(
  id: string,
  attempt: string,
  cols: number,
  rows: number,
  secret: string | null,
  onData: DataHandler,
  onEnd: EndHandler,
): Promise<SessionId> {
  return invoke<SessionId>('connect_host', {
    id,
    attempt,
    cols,
    rows,
    secret,
    onData: channelFor(onData, onEnd),
  });
}

/** The user closed a password prompt or the tab: drop the connection that was waiting. */
export function cancelConnect(attempt: string): Promise<void> {
  return invoke('cancel_connect', { attempt });
}

/**
 * Trust the key the server just presented. Replacing a key that was already
 * trusted needs `replace`: the user's explicit yes in the warning.
 */
export function trustHostKey(
  address: string,
  port: number,
  fingerprint: string,
  replace = false,
): Promise<void> {
  return invoke('trust_host_key', { address, port, fingerprint, replace });
}

/** Whether the terminal has a password it can type (for sudo and friends). */
export function sessionCanTypePassword(id: SessionId): Promise<boolean> {
  return invoke<boolean>('session_can_type_password', { id });
}

/** Types the terminal's password and Enter. The password never reaches the page. */
/** Type the host's password into the terminal; with `enter`, press Enter too. */
export function typeSessionPassword(id: SessionId, enter: boolean): Promise<void> {
  return invoke('type_session_password', { id, enter });
}

/** Store (or forget, with `null`) a host's password in the vault. */
export function setHostPassword(id: string, password: string | null): Promise<HostRecord> {
  return invoke<HostRecord>('set_host_password', { id, password });
}

export type HostOsEvent = { id: string; os: string };

// ── Vault ─────────────────────────────────────────────────────────────────

export type VaultStatus = 'absent' | 'locked' | 'unlocked';

export function vaultStatus(): Promise<VaultStatus> {
  return invoke<VaultStatus>('vault_status');
}

export type VaultState = { status: VaultStatus; remembered: boolean };

export function vaultState(): Promise<VaultState> {
  return invoke<VaultState>('vault_state');
}

/** `remember`: this Windows user opens the vault without the master password. */
export function createVault(password: string, remember: boolean): Promise<void> {
  return invoke('create_vault', { password, remember });
}

/** `remember` changes whether this device keeps the key; `null` leaves it. */
export function unlockVault(password: string, remember: boolean | null = null): Promise<void> {
  return invoke('unlock_vault', { password, remember });
}

export function setVaultRemembered(remember: boolean): Promise<void> {
  return invoke('set_vault_remembered', { remember });
}

export function lockVault(): Promise<void> {
  return invoke('lock_vault');
}

// ── Import ────────────────────────────────────────────────────────────────

/** The sources UwUSSH can import from, by id. */
export type ImportSource = 'termius' | 'putty' | 'kitty' | 'openssh';

export type ImportSummary = {
  hosts: number;
  identities: number;
  keys: number;
  knownHosts: number;
  snippets: number;
  /** Writing this import needs an unlocked vault (it has secrets to seal). */
  needsVault: boolean;
  skipped: string[];
};

export type ImportReport = {
  hostsAdded: number;
  hostsSkipped: number;
  identitiesAdded: number;
  keysAdded: number;
  knownHostsAdded: number;
  snippetsAdded: number;
  skipped: string[];
};

export function availableImports(): Promise<ImportSource[]> {
  return invoke<ImportSource[]>('available_imports');
}

export function scanImport(source: ImportSource): Promise<ImportSummary> {
  return invoke<ImportSummary>('scan_import', { source });
}

export function runImport(source: ImportSource): Promise<ImportReport> {
  return invoke<ImportReport>('run_import', { source });
}

// ── App ─────────────────────────────────────────────────────────────────────

/** A page just started: close whatever an earlier page left open. */
export function closeAllSessions(): Promise<number> {
  return invoke<number>('close_all_sessions');
}

export type UpdateInfo = { version: string; notes: string | null };

export function setUpdateChannel(channel: 'stable' | 'beta'): Promise<void> {
  return invoke('set_update_channel', { channel });
}

export function updateStatus(): Promise<UpdateInfo | null> {
  return invoke<UpdateInfo | null>('update_status');
}

export function checkForUpdates(): Promise<UpdateInfo | null> {
  return invoke<UpdateInfo | null>('check_for_updates');
}

/** Hands over to the downloaded setup; the app quits on success. */
export function installUpdate(): Promise<void> {
  return invoke('install_update');
}

export type ProjectPage = 'source' | 'releases' | 'issues' | 'license';

/** A web link a program printed in the terminal; Rust only opens http and https. */
export function openTerminalLink(url: string): Promise<void> {
  return invoke('open_terminal_link', { url });
}

export function openProjectPage(page: ProjectPage): Promise<void> {
  return invoke('open_project_page', { page });
}

// ── M0 ──────────────────────────────────────────────────────────────────────

export type M0Kind = 'synthetic' | 'pty';

export type M0Scenario = {
  kind: M0Kind;
  flowControl: boolean;
  payloadMib: number;
};

export function spawnM0Session(
  scenario: M0Scenario,
  cols: number,
  rows: number,
  onData: DataHandler,
  onEnd: EndHandler,
): Promise<SessionId> {
  const { kind, flowControl, payloadMib } = scenario;
  return invoke<SessionId>('spawn_m0_session', {
    scenario: { kind, flowControl, payloadMib },
    cols,
    rows,
    onData: channelFor(onData, onEnd),
  });
}

export function m0Autorun(): Promise<boolean> {
  return invoke<boolean>('m0_autorun');
}

/** Writes the report file and returns its path. Quits the app on autorun. */
export function m0Finish(report: unknown): Promise<string> {
  return invoke<string>('m0_finish', { report });
}
