import { useEffect, useState } from 'react';
import {
  asBackupFailure,
  importExportFile,
  pickExportFile,
  readExportFile,
  type BackupSummary,
  type PickedExport,
} from '../lib/backup';
import {
  availableImports,
  runImport,
  scanImport,
  type ImportReport,
  type ImportSource,
  type ImportSummary,
} from '../lib/session';
import { Icon } from './Icon';
import { useCloseGuard } from './CloseGuard';
import { Modal } from './Modal';
import { NyuScene } from './nyu/scenes';
import { VaultDialog } from './VaultDialog';

type Props = {
  onClose: () => void;
  /** Called after a successful import, so the host list can refresh. */
  onImported: () => void;
};

const LABEL: Record<ImportSource, string> = {
  termius: 'Termius',
  putty: 'PuTTY',
  kitty: 'KiTTY',
  openssh: 'OpenSSH (~/.ssh/config)',
};

type Step =
  | { kind: 'loading' }
  | { kind: 'pick'; sources: ImportSource[] }
  | { kind: 'preview'; source: ImportSource; summary: ImportSummary }
  | { kind: 'file-password'; file: PickedExport; wrong: boolean }
  | { kind: 'file-preview'; file: PickedExport; summary: BackupSummary; password: string | null }
  | { kind: 'done'; report: ImportReport };

/**
 * Bringing a setup across: another client's (read where it keeps its data) or
 * an UwUSSH export file. Pick a source, see what it holds in counts, and write
 * it. Secrets need the vault; the vault dialog comes in between and the import
 * goes on right after. Previews and results show counts only, never a host or
 * a secret.
 */
export function ImportDialog({ onClose, onImported }: Props) {
  const [step, setStep] = useState<Step>({ kind: 'loading' });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [vaultFor, setVaultFor] = useState<(() => Promise<void>) | null>(null);
  const [password, setPassword] = useState('');
  const closeGuard = useCloseGuard(
    step.kind === 'preview' || step.kind === 'file-password' || step.kind === 'file-preview',
    onClose,
    'Der Import wird dann nicht ausgeführt.',
  );

  useEffect(() => {
    void guard(async () => {
      const sources = await availableImports();
      setStep({ kind: 'pick', sources });
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /** Run an action with the buttons disabled and the error line cleared. */
  async function guard(action: () => Promise<void>) {
    setBusy(true);
    setError(null);
    try {
      await action();
    } catch (e) {
      const failure = asBackupFailure(e);
      setError(failure.kind === 'error' ? failure.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  async function preview(source: ImportSource) {
    setStep({ kind: 'preview', source, summary: await scanImport(source) });
  }

  /**
   * Run `write`. When it has a new secret to seal and the vault is locked, it
   * fails with `vault-locked`, having written nothing: open the vault, then
   * run it again. Hosts already imported bring no secret, so importing the
   * same setup twice never asks.
   */
  async function withVault(write: () => Promise<void>) {
    try {
      await write();
    } catch (e) {
      if (asBackupFailure(e).kind !== 'vault-locked') throw e;
      setVaultFor(() => write);
    }
  }

  async function importSource(source: ImportSource) {
    await withVault(async () => {
      const report = await runImport(source);
      onImported();
      setStep({ kind: 'done', report });
    });
  }

  async function pickFile() {
    const file = await pickExportFile();
    if (!file) return;
    if (file.sealed || !file.summary) setStep({ kind: 'file-password', file, wrong: false });
    else setStep({ kind: 'file-preview', file, summary: file.summary, password: null });
  }

  async function unlockFile(file: PickedExport) {
    const entered = password;
    try {
      const summary = await readExportFile(file.token, entered);
      setStep({ kind: 'file-preview', file, summary, password: entered });
    } catch (e) {
      if (asBackupFailure(e).kind === 'password-wrong') {
        setStep({ kind: 'file-password', file, wrong: true });
        setPassword('');
      } else throw e;
    }
  }

  async function importFile(file: PickedExport, filePassword: string | null) {
    await withVault(async () => {
      const report = await importExportFile(file.token, filePassword);
      setPassword('');
      onImported();
      setStep({ kind: 'done', report });
    });
  }

  const title = step.kind === 'done' ? 'Import abgeschlossen ✧' : 'Importieren';

  return (
    <>
      <Modal title={title} onCancel={closeGuard.request} footer={footer()}>
        {error && (
          <p className="field-error" role="alert">
            {error}
          </p>
        )}
        {body()}
      </Modal>
      {closeGuard.dialog}
      {vaultFor && (
        <VaultDialog
          reason="Die importierten Passwörter und Keys landen verschlüsselt im Tresor."
          onDone={() => {
            const write = vaultFor;
            setVaultFor(null);
            void guard(write);
          }}
          onCancel={() => setVaultFor(null)}
        />
      )}
    </>
  );

  function body() {
    switch (step.kind) {
      case 'loading':
        return <p className="import-note">Wird gesucht…</p>;

      case 'pick':
        return (
          <div className="import-sources">
            <p className="import-note">Woraus möchtest du importieren?</p>
            {step.sources.map((source) => (
              <button
                key={source}
                className="import-source"
                disabled={busy}
                onClick={() => void guard(() => preview(source))}
              >
                {LABEL[source]}
              </button>
            ))}
            <button className="import-source" disabled={busy} onClick={() => void guard(pickFile)}>
              <Icon name="file" size={16} /> UwUSSH-Export (.uwussh)…
            </button>
            {step.sources.length === 0 && (
              <p className="import-note">
                Auf diesem Rechner wurden keine anderen SSH-Clients gefunden. UwUSSH liest Termius,
                PuTTY, KiTTY und <code>~/.ssh/config</code> dort, wo sie ihre Daten ablegen.
              </p>
            )}
          </div>
        );

      case 'preview':
        return (
          <Preview
            source={LABEL[step.source]}
            counts={sourceCounts(step.summary)}
            secrets={step.summary.needsVault}
            skipped={step.summary.skipped}
          />
        );

      case 'file-password':
        return (
          <form
            className="form"
            onSubmit={(event) => {
              event.preventDefault();
              void guard(() => unlockFile(step.file));
            }}
          >
            <p className="dialog-lead">
              <code>{step.file.fileName}</code> ist mit einem Passwort geschützt, weil Passwörter
              und Keys darin stecken.
            </p>
            <label className="field">
              <span>Passwort der Export-Datei</span>
              <input
                type="password"
                data-autofocus
                value={password}
                autoComplete="off"
                aria-invalid={step.wrong}
                onChange={(e) => setPassword(e.target.value)}
              />
              {step.wrong && <em className="field-error">Das Passwort passt nicht.</em>}
            </label>
            <button type="submit" hidden />
          </form>
        );

      case 'file-preview':
        return (
          <Preview
            source={step.file.fileName}
            counts={[
              ['Hosts', step.summary.hosts],
              ['Gruppen', step.summary.groups],
              ['Passwörter', step.summary.passwords],
              ['Keys', step.summary.keys],
              ['Bekannte Host-Keys', step.summary.knownHosts],
              ['Snippets', step.summary.snippets],
            ]}
            secrets={step.summary.passwords > 0 || step.summary.keys > 0}
            skipped={[]}
          />
        );

      case 'done':
        return <Report report={step.report} />;
    }
  }

  function footer() {
    switch (step.kind) {
      case 'preview': {
        const nothing = step.summary.hosts === 0 && step.summary.keys === 0;
        return (
          <>
            <span className="spacer" />
            <button data-secondary onClick={closeGuard.request}>
              Abbrechen
            </button>
            <button
              className="primary"
              disabled={busy || nothing}
              onClick={() => void guard(() => importSource(step.source))}
            >
              {busy ? 'Importiere…' : 'Importieren'}
            </button>
          </>
        );
      }
      case 'file-password':
        return (
          <>
            <span className="spacer" />
            <button data-secondary onClick={closeGuard.request}>
              Abbrechen
            </button>
            <button
              className="primary"
              disabled={busy || !password}
              onClick={() => void guard(() => unlockFile(step.file))}
            >
              Öffnen
            </button>
          </>
        );
      case 'file-preview': {
        const nothing =
          step.summary.hosts === 0 && step.summary.keys === 0 && step.summary.groups === 0;
        return (
          <>
            <span className="spacer" />
            <button data-secondary onClick={closeGuard.request}>
              Abbrechen
            </button>
            <button
              className="primary"
              disabled={busy || nothing}
              onClick={() => void guard(() => importFile(step.file, step.password))}
            >
              {busy ? 'Importiere…' : 'Importieren'}
            </button>
          </>
        );
      }
      case 'done':
        return (
          <>
            <span className="spacer" />
            <button className="primary" onClick={onClose}>
              Fertig
            </button>
          </>
        );
      default:
        return (
          <>
            <span className="spacer" />
            <button data-secondary onClick={closeGuard.request}>
              Abbrechen
            </button>
          </>
        );
    }
  }
}

function sourceCounts(summary: ImportSummary): [string, number][] {
  return [
    ['Hosts', summary.hosts],
    ['Anmeldungen', summary.identities],
    ['Keys', summary.keys],
    ['Bekannte Host-Keys', summary.knownHosts],
    ['Snippets', summary.snippets],
  ];
}

function Preview({
  source,
  counts,
  secrets,
  skipped,
}: {
  source: string;
  counts: [string, number][];
  secrets: boolean;
  skipped: string[];
}) {
  return (
    <div className="import-preview">
      <p className="import-note">
        Das findet UwUSSH in {source}. Schon vorhandene Hosts (gleiche Adresse, gleicher Port,
        gleicher Benutzer) und Host-Keys werden übersprungen, nichts wird überschrieben.
      </p>
      <ul className="import-counts">
        {counts
          .filter(([, count]) => count > 0)
          .map(([label, count]) => (
            <li key={label}>
              <b>{count}</b>
              <span>{label}</span>
            </li>
          ))}
      </ul>
      {secrets && <p className="import-note">Die Secrets landen verschlüsselt im Tresor.</p>}
      <Skipped items={skipped} />
    </div>
  );
}

function Report({ report }: { report: ImportReport }) {
  const rows: [string, number][] = [
    ['Hosts hinzugefügt', report.hostsAdded],
    ['Hosts übersprungen (schon da)', report.hostsSkipped],
    ['Anmeldungen', report.identitiesAdded],
    ['Keys', report.keysAdded],
    ['Host-Keys', report.knownHostsAdded],
    ['Snippets', report.snippetsAdded],
  ];
  return (
    <div className="import-preview">
      <NyuScene name="done" className="dialog-scene" />
      <ul className="import-counts">
        {rows.map(([label, count]) => (
          <li key={label}>
            <b>{count}</b>
            <span>{label}</span>
          </li>
        ))}
      </ul>
      <Skipped items={report.skipped} />
    </div>
  );
}

function Skipped({ items }: { items: string[] }) {
  if (items.length === 0) return null;
  return (
    <details className="import-skipped">
      <summary>
        {items.length} {items.length === 1 ? 'Eintrag' : 'Einträge'} übersprungen
      </summary>
      <ul>
        {items.map((item, i) => (
          <li key={i}>{item}</li>
        ))}
      </ul>
    </details>
  );
}
