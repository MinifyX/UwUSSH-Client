import { useEffect, useState } from 'react';
import {
  availableImports,
  createVault,
  runImport,
  scanImport,
  unlockVault,
  vaultStatus,
  type ImportReport,
  type ImportSource,
  type ImportSummary,
} from '../lib/session';
import { Modal } from './Modal';

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
  | { kind: 'none' }
  | { kind: 'pick'; sources: ImportSource[] }
  | { kind: 'preview'; source: ImportSource; summary: ImportSummary }
  | { kind: 'vault'; source: ImportSource; mode: 'create' | 'unlock' }
  | { kind: 'done'; report: ImportReport };

/**
 * Bringing another client's setup across. Pick a source, see what it holds in
 * counts, and write it. A source with secrets (Termius) needs the vault first;
 * one without (PuTTY, KiTTY) is written straight away. Previews and results
 * show counts only, never a host or a secret.
 */
export function ImportDialog({ onClose, onImported }: Props) {
  const [step, setStep] = useState<Step>({ kind: 'loading' });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void guard(async () => {
      const sources = await availableImports();
      if (sources.length === 0) setStep({ kind: 'none' });
      else if (sources.length === 1) await preview(sources[0]!);
      else setStep({ kind: 'pick', sources });
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /** Run an action with the button disabled and the error line cleared. */
  async function guard(action: () => Promise<void>) {
    setBusy(true);
    setError(null);
    try {
      await action();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function preview(source: ImportSource) {
    setStep({ kind: 'preview', source, summary: await scanImport(source) });
  }

  /** From the preview: import now, stepping through the vault first if needed. */
  async function confirm(source: ImportSource, summary: ImportSummary) {
    if (summary.needsVault) {
      const status = await vaultStatus();
      if (status === 'absent') return setStep({ kind: 'vault', source, mode: 'create' });
      if (status === 'locked') return setStep({ kind: 'vault', source, mode: 'unlock' });
    }
    const report = await runImport(source);
    onImported();
    setStep({ kind: 'done', report });
  }

  const title = step.kind === 'done' ? 'Import abgeschlossen' : 'Importieren';

  return (
    <Modal title={title} onCancel={onClose} footer={footer()}>
      {error && (
        <p className="field-error" role="alert">
          {error}
        </p>
      )}
      {body()}
    </Modal>
  );

  function body() {
    switch (step.kind) {
      case 'loading':
        return <p className="import-note">Wird gesucht…</p>;

      case 'none':
        return (
          <p className="import-note">
            Auf diesem Rechner wurde nichts zum Importieren gefunden. UwUSSH liest Termius, PuTTY,
            KiTTY und <code>~/.ssh/config</code> dort, wo sie ihre Daten ablegen — es gibt keine
            Export-Datei, die du vorher erzeugen müsstest.
          </p>
        );

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
          </div>
        );

      case 'preview':
        return <Preview source={step.source} summary={step.summary} />;

      case 'vault':
        return <VaultSetup mode={step.mode} busy={busy} onSubmit={handleVault} />;

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
            <button data-secondary onClick={onClose}>
              Abbrechen
            </button>
            <button
              className="primary"
              disabled={busy || nothing}
              onClick={() => void guard(() => confirm(step.source, step.summary))}
            >
              {busy ? 'Importiere…' : 'Importieren'}
            </button>
          </>
        );
      }
      case 'done':
        return (
          <button className="primary" onClick={onClose}>
            Fertig
          </button>
        );
      case 'none':
        return (
          <button className="primary" onClick={onClose}>
            Schließen
          </button>
        );
      default:
        return (
          <button data-secondary onClick={onClose}>
            Abbrechen
          </button>
        );
    }
  }

  function handleVault(password: string, mode: 'create' | 'unlock') {
    if (step.kind !== 'vault') return;
    const source = step.source;
    void guard(async () => {
      if (mode === 'create') await createVault(password);
      else await unlockVault(password);
      const report = await runImport(source);
      onImported();
      setStep({ kind: 'done', report });
    });
  }
}

function VaultSetup({
  mode,
  busy,
  onSubmit,
}: {
  mode: 'create' | 'unlock';
  busy: boolean;
  onSubmit: (password: string, mode: 'create' | 'unlock') => void;
}) {
  const [password, setPassword] = useState('');
  const [confirm, setConfirm] = useState('');

  const mismatch = mode === 'create' && confirm.length > 0 && password !== confirm;
  const ready = password.length > 0 && (mode === 'unlock' || password === confirm);

  function submit(event: React.FormEvent) {
    event.preventDefault();
    if (ready && !busy) onSubmit(password, mode);
  }

  return (
    <form className="vault-setup" onSubmit={submit}>
      {mode === 'create' ? (
        <p className="import-note">
          Die importierten Passwörter und Keys werden verschlüsselt gespeichert. Dafür brauchst du
          ein Master-Passwort. Es entsperrt den Tresor und verlässt dieses Gerät nie.
        </p>
      ) : (
        <p className="import-note">Entsperre den Tresor, um den Import fortzusetzen.</p>
      )}

      <label className="field">
        <span>Master-Passwort</span>
        <input
          type="password"
          data-autofocus
          value={password}
          autoComplete={mode === 'create' ? 'new-password' : 'current-password'}
          onChange={(e) => setPassword(e.target.value)}
        />
      </label>

      {mode === 'create' && (
        <label className="field">
          <span>Wiederholen</span>
          <input
            type="password"
            value={confirm}
            autoComplete="new-password"
            onChange={(e) => setConfirm(e.target.value)}
          />
        </label>
      )}

      {mismatch && <p className="field-error">Die Passwörter stimmen nicht überein.</p>}

      {mode === 'create' && (
        <p className="import-warning">
          Es gibt noch keine Wiederherstellung: Vergisst du das Master-Passwort, sind die
          gespeicherten Secrets verloren. Ein Recovery-Kit kommt mit dem Sync (M2).
        </p>
      )}

      {/* A submit input so Enter works; the footer button posts the same form. */}
      <button type="submit" hidden disabled={!ready || busy} />
    </form>
  );
}

function Preview({ source, summary }: { source: ImportSource; summary: ImportSummary }) {
  const rows: [string, number][] = [
    ['Hosts', summary.hosts],
    ['Anmeldungen', summary.identities],
    ['Keys', summary.keys],
    ['Bekannte Host-Keys', summary.knownHosts],
    ['Snippets', summary.snippets],
  ];
  return (
    <div className="import-preview">
      <p className="import-note">
        Das findet UwUSSH in {LABEL[source]}. Schon vorhandene Hosts und Host-Keys werden beim
        Import übersprungen, nichts wird überschrieben.
      </p>
      <ul className="import-counts">
        {rows
          .filter(([, count]) => count > 0)
          .map(([label, count]) => (
            <li key={label}>
              <b>{count}</b>
              <span>{label}</span>
            </li>
          ))}
      </ul>
      {summary.needsVault && (
        <p className="import-note">Die Secrets landen verschlüsselt im Tresor.</p>
      )}
      <Skipped items={summary.skipped} />
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
