import { useEffect, useState } from 'react';
import {
  createVault,
  importTermius,
  scanTermius,
  termiusAvailable,
  unlockVault,
  vaultStatus,
  type ImportReport,
  type ImportSummary,
} from '../lib/session';
import { Modal } from './Modal';

type Props = {
  onClose: () => void;
  /** Called after a successful import, so the host list can refresh. */
  onImported: () => void;
};

type Step =
  | { kind: 'loading' }
  | { kind: 'no-termius' }
  | { kind: 'create' }
  | { kind: 'unlock' }
  | { kind: 'preview'; summary: ImportSummary }
  | { kind: 'done'; report: ImportReport };

/**
 * Bringing a Termius setup across. The vault comes first — the import carries
 * passwords and keys, and they need a sealed home — so this walks the vault
 * from absent or locked to unlocked, then previews what Termius holds and
 * writes it. The preview and the result show counts, never a host or a secret.
 */
export function ImportDialog({ onClose, onImported }: Props) {
  const [step, setStep] = useState<Step>({ kind: 'loading' });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void (async () => {
      try {
        const [available, status] = await Promise.all([termiusAvailable(), vaultStatus()]);
        if (!available) {
          setStep({ kind: 'no-termius' });
          return;
        }
        if (status === 'absent') setStep({ kind: 'create' });
        else if (status === 'locked') setStep({ kind: 'unlock' });
        else await showPreview();
      } catch (e) {
        setError(String(e));
      }
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function showPreview() {
    setStep({ kind: 'preview', summary: await scanTermius() });
  }

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

  const title = step.kind === 'done' ? 'Import abgeschlossen' : 'Aus Termius importieren';

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
        return <p className="import-note">Termius wird gesucht…</p>;

      case 'no-termius':
        return (
          <p className="import-note">
            Auf diesem Rechner wurden keine Termius-Daten gefunden. UwUSSH liest die lokale
            Termius-Installation des angemeldeten Benutzers — es gibt keine Export-Datei, die du
            vorher erzeugen müsstest.
          </p>
        );

      case 'create':
        return <VaultSetup mode="create" busy={busy} onSubmit={handleVault} />;

      case 'unlock':
        return <VaultSetup mode="unlock" busy={busy} onSubmit={handleVault} />;

      case 'preview':
        return <Preview summary={step.summary} />;

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
              onClick={() =>
                void guard(async () => {
                  const report = await importTermius();
                  onImported();
                  setStep({ kind: 'done', report });
                })
              }
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
      case 'no-termius':
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
    void guard(async () => {
      if (mode === 'create') await createVault(password);
      else await unlockVault(password);
      await showPreview();
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
          Deine Termius-Passwörter und -Keys werden verschlüsselt gespeichert. Dafür brauchst du ein
          Master-Passwort. Es entsperrt den Tresor und verlässt dieses Gerät nie.
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

function Preview({ summary }: { summary: ImportSummary }) {
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
        Das findet UwUSSH in Termius. Schon vorhandene Hosts und Host-Keys werden beim Import
        übersprungen, nichts wird überschrieben.
      </p>
      <ul className="import-counts">
        {rows.map(([label, count]) => (
          <li key={label}>
            <b>{count}</b>
            <span>{label}</span>
          </li>
        ))}
      </ul>
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
