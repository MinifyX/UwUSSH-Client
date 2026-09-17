/**
 * Keys in the vault, and UwUKeygen.
 *
 * A generated key stays in Rust under a token; the page gets its public facts
 * and — only when asked — its private half as text to show or copy.
 */

import { invoke } from '@tauri-apps/api/core';

export type KeyRecord = {
  id: string;
  label: string;
  keyType: string;
  publicKey: string;
  hasPassphrase: boolean;
  hosts: number;
};

export type KeyFailure =
  | { kind: 'vault-locked' }
  | { kind: 'passphrase-required' }
  | { kind: 'passphrase-wrong' }
  | { kind: 'in-use'; hosts: number }
  | { kind: 'error'; message: string };

export function asKeyFailure(error: unknown): KeyFailure {
  if (typeof error === 'object' && error !== null && 'kind' in error) {
    return error as KeyFailure;
  }
  return { kind: 'error', message: String(error) };
}

export function listKeys(): Promise<KeyRecord[]> {
  return invoke<KeyRecord[]>('list_keys');
}

export function renameKey(id: string, label: string): Promise<void> {
  return invoke('rename_key', { id, label });
}

export function deleteKey(id: string): Promise<void> {
  return invoke('delete_key', { id });
}

export function keyPublicLine(id: string): Promise<string> {
  return invoke<string>('key_public_line', { id });
}

export type PickedKey = {
  token: string;
  fileName: string;
  encrypted: boolean;
  info: KeyInfo | null;
};

/** Opens a file dialog and reads the key; `null` when it was cancelled. */
export function pickKeyFile(): Promise<PickedKey | null> {
  return invoke<PickedKey | null>('pick_key_file');
}

/** Drops the picked key file from memory; the import dialog closed. */
export function forgetPickedKey(): Promise<void> {
  return invoke('forget_picked_key');
}

/** Seals the picked key into the vault, with its passphrase if it has one. */
export function importPickedKey(
  token: string,
  label: string | null,
  passphrase: string | null,
): Promise<KeyRecord> {
  return invoke<KeyRecord>('import_picked_key', { token, label, passphrase });
}

export type PrivateFormat = 'openssh' | 'putty-v3' | 'putty-v2' | 'pem';

/**
 * What a passphrase does for a file in this format. PuTTY's v2 files derive
 * their key with a single SHA-1 round, which graphics cards guess billions of
 * times a second — worth saying before someone relies on it.
 */
export function passphraseNote(format: PrivateFormat, passphrase: boolean): string {
  if (!passphrase) return 'Unverschlüsselt – wer die Datei hat, hat den Schlüssel.';
  if (format === 'putty-v2')
    return 'Verschlüsselt, aber schwach: .ppk v2 schützt die Passphrase kaum gegen Raten. Nimm .ppk v3, wenn das Programm es kann.';
  return 'Verschlüsselt mit deiner Passphrase.';
}

export const FORMAT_LABELS: Record<PrivateFormat, string> = {
  openssh: 'OpenSSH',
  'putty-v3': 'PuTTY (.ppk v3)',
  'putty-v2': 'PuTTY (.ppk v2)',
  pem: 'PEM (PKCS#8)',
};

/** Opens a save dialog; returns the file name, or `null` when cancelled. */
export function exportKeyFile(
  id: string,
  format: PrivateFormat,
  passphrase: string | null,
): Promise<string | null> {
  return invoke<string | null>('export_key_file', { id, format, passphrase });
}

// ── UwUKeygen ───────────────────────────────────────────────────────────────

export type KeyKind =
  | { type: 'rsa'; bits: number }
  | { type: 'ed25519' }
  | { type: 'ecdsa-p256' }
  | { type: 'ecdsa-p384' }
  | { type: 'ecdsa-p521' };

export type KeyInfo = {
  algorithm: string;
  label: string;
  bits: number;
  comment: string;
  publicOpenssh: string;
  fingerprintSha256: string;
  fingerprintMd5: string;
  randomart: string;
};

export type Generated = { token: string; info: KeyInfo };

export function keygenGenerate(
  kind: KeyKind,
  comment: string,
  entropy: Uint8Array,
): Promise<Generated> {
  return invoke<Generated>('keygen_generate', {
    request: { kind, comment, entropy: Array.from(entropy) },
  });
}

export function keygenEncode(
  token: string,
  format: PrivateFormat,
  passphrase: string | null,
): Promise<string> {
  return invoke<string>('keygen_encode', { token, format, passphrase });
}

/**
 * Copies the private key through Rust, marked for Windows to keep out of the
 * clipboard history and cloud sync; it is cleared again after a minute.
 */
export function keygenCopyPrivate(
  token: string,
  format: PrivateFormat,
  passphrase: string | null,
): Promise<void> {
  return invoke('keygen_copy_private', { token, format, passphrase });
}

export function keygenSave(
  token: string,
  format: PrivateFormat,
  passphrase: string | null,
  label: string,
): Promise<string | null> {
  return invoke<string | null>('keygen_save', { token, format, passphrase, label });
}

export function keygenStore(
  token: string,
  label: string,
  passphrase: string | null,
): Promise<KeyRecord> {
  return invoke<KeyRecord>('keygen_store', { token, label, passphrase });
}

export function keygenDiscard(token: string): Promise<void> {
  return invoke('keygen_discard', { token });
}

/** Saves the public key line as a `.pub` file; returns the file name or `null`. */
export function keygenSavePublic(token: string, label: string): Promise<string | null> {
  return invoke<string | null>('keygen_save_public', { token, label });
}
