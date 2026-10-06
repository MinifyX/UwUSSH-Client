import { Button, Hint, Nyu } from '@uwusuite/design';
import { useState } from 'react';
import { updatesAvailableInApp } from '../lib/flavor';
import { language, t, useLanguage } from '../lib/i18n';
import type { UpdateInfo } from '../lib/session';

/** Release notes are JSON with `de` and `en` when written for UwUSSH, otherwise plain text. */
export function notesFor(notes: string | null | undefined): string {
  if (!notes) return '';
  try {
    const parsed = JSON.parse(notes) as Record<string, unknown>;
    const text = language() === 'en' ? (parsed.en ?? parsed.de) : (parsed.de ?? parsed.en);
    if (typeof text === 'string') return text;
  } catch {
    // Plain text notes.
  }
  return notes;
}

type Props = {
  update: UpdateInfo;
  /** SSH sessions a restart would cut off. */
  openConnections: number;
  onLater: () => void;
  onRestart: () => Promise<void>;
};

/**
 * Nyu's quiet note that a new version is downloaded and ready. Nothing in a
 * build that doesn't update itself (the Mac App Store's).
 */
export function UpdateHint(props: Props) {
  if (!updatesAvailableInApp()) return null;
  return <ReadyNote {...props} />;
}

function ReadyNote({ update, openConnections, onLater, onRestart }: Props) {
  useLanguage();
  const [showNotes, setShowNotes] = useState(false);
  const [restarting, setRestarting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const notes = notesFor(update.notes);

  return (
    <aside className="update-hint" aria-live="polite">
      <div className="update-hint-head">
        <Nyu shell="terminal" size={40} mood="sparkle" title="Nyu" />
        <div>
          <p className="update-hint-title">{t('Ein Update ist bereit ✧')}</p>
          <p className="update-hint-meta">
            {t('Version {version}', { version: update.version })}
            {notes && (
              <>
                {' · '}
                <button
                  className="link-button"
                  onClick={() => setShowNotes(!showNotes)}
                  aria-expanded={showNotes}
                >
                  {t('Was ist neu?')}
                </button>
              </>
            )}
          </p>
        </div>
      </div>
      {showNotes && <p className="update-hint-notes">{notes}</p>}
      {openConnections > 0 && (
        <p className="update-hint-warning">
          {openConnections === 1
            ? t('Eine Verbindung ist noch offen und wird beim Neustart getrennt.')
            : t('{count} Verbindungen sind noch offen und werden beim Neustart getrennt.', {
                count: openConnections,
              })}
        </p>
      )}
      {error && <Hint tone="danger">{error}</Hint>}
      <div className="update-hint-actions">
        <Button size="sm" variant="ghost" onClick={onLater}>
          {t('Später')}
        </Button>
        <Button
          size="sm"
          variant="primary"
          busy={restarting}
          onClick={async () => {
            setRestarting(true);
            setError(null);
            try {
              await onRestart();
            } catch (e) {
              setError(String(e));
              setRestarting(false);
            }
          }}
        >
          {restarting ? t('Startet neu …') : t('Jetzt neu starten')}
        </Button>
      </div>
    </aside>
  );
}
