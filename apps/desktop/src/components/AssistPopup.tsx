import { useEffect, useRef, useState, type KeyboardEvent } from 'react';
import {
  asAssistFailure,
  assistErrorText,
  assistGenerate,
  assistPlatform,
  assistTypeCommand,
  SHELL_LABELS,
  type AssistFailure,
  type AssistTarget,
  type PlatformInfo,
  type Shell,
  type Suggestion,
} from '../lib/assist';
import { t, useLanguage } from '../lib/i18n';
import type { SessionId } from '../lib/session';
import { Icon } from './Icon';
import { Modal } from './Modal';
import { VaultDialog } from './VaultDialog';

type Props = {
  /** The terminal the command is typed into. */
  session: SessionId;
  target: AssistTarget;
  /** Shown above the field: which host or "this computer". */
  targetName: string;
  onClose: () => void;
  /** The command is at the prompt. */
  onInserted: () => void;
  /** No provider yet: open Settings → KI. */
  onSetUp: () => void;
};

/** The shell picked per target, for as long as the app runs. */
const pickedShells = new Map<string, Shell>();

function targetKey(target: AssistTarget): string {
  return target.kind === 'local' ? 'local' : `host:${target.os ?? ''}`;
}

/**
 * The command assistant over a terminal: a request in words, one command for
 * this system, typed at the prompt with Enter or "Einfügen" — never run. A
 * close enough request asked before on the same kind of system comes from the
 * cache; "Neu generieren" asks the model anyway.
 */
export function AssistPopup({ session, target, targetName, onClose, onInserted, onSetUp }: Props) {
  const language = useLanguage();
  const [platform, setPlatform] = useState<PlatformInfo | null>(null);
  const [shell, setShell] = useState<Shell | null>(pickedShells.get(targetKey(target)) ?? null);
  const [request, setRequest] = useState('');
  const [asked, setAsked] = useState<string | null>(null);
  const [suggestion, setSuggestion] = useState<Suggestion | null>(null);
  const [command, setCommand] = useState('');
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<AssistFailure | null>(null);
  const [unlock, setUnlock] = useState<{ fresh: boolean } | null>(null);
  const requestRef = useRef<HTMLInputElement>(null);
  const run = useRef(0);

  useEffect(() => {
    let current = true;
    void assistPlatform(target, shell)
      .then((info) => current && setPlatform(info))
      .catch(() => undefined);
    return () => {
      current = false;
    };
    // The target does not change while the popup is open.
  }, [shell]);

  const generate = async (fresh: boolean) => {
    const text = request.trim();
    if (!text || busy) return;
    const mine = ++run.current;
    setBusy(true);
    setFailure(null);
    try {
      const found = await assistGenerate(target, shell, text, language, fresh);
      if (mine !== run.current) return;
      setSuggestion(found);
      setCommand(found.command);
      setAsked(text);
    } catch (error) {
      if (mine !== run.current) return;
      const failed = asAssistFailure(error);
      if (failed.code === 'vault-locked') setUnlock({ fresh });
      else setFailure(failed);
    } finally {
      if (mine === run.current) setBusy(false);
    }
  };

  const insert = async () => {
    const line = command.trim();
    if (!line) return;
    try {
      await assistTypeCommand(session, line);
      onInserted();
    } catch (error) {
      setFailure(asAssistFailure(error));
    }
  };

  // Enter asks first; once the answer is for what is in the field, Enter
  // types it.
  const answered = suggestion !== null && asked === request.trim();
  const onRequestKey = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key !== 'Enter' || event.nativeEvent.isComposing) return;
    event.preventDefault();
    if (answered && command.trim()) void insert();
    else void generate(false);
  };

  const pickShell = (next: Shell) => {
    pickedShells.set(targetKey(target), next);
    setShell(next);
    // An answer for another shell does not fit this one.
    setSuggestion(null);
    setAsked(null);
    setCommand('');
  };

  const notSetUp = failure?.code === 'not-configured' || failure?.code === 'no-model';

  return (
    <>
      <Modal
        title={t('Befehl aus Worten')}
        onCancel={onClose}
        footer={
          <>
            <button data-secondary onClick={onClose}>
              {t('Abbrechen')}
            </button>
            <button
              data-secondary
              onClick={() => void generate(true)}
              disabled={busy || !request.trim()}
              title={t('Fragt das Modell neu, ohne den Cache')}
            >
              <Icon name="refresh" size={15} />
              {t('Neu generieren')}
            </button>
            <button
              className="primary"
              onClick={() => (answered ? void insert() : void generate(false))}
              disabled={busy || !request.trim() || (answered && !command.trim())}
            >
              {answered ? t('Einfügen') : t('Erzeugen')}
            </button>
          </>
        }
      >
        <div className="assist">
          <p className="assist-target">
            <span>{t('Für {name}', { name: targetName })}</span>
            {platform && platform.shells.length > 1 ? (
              <select
                className="select assist-shell"
                aria-label={t('Shell')}
                value={platform.shell}
                onChange={(event) => pickShell(event.target.value as Shell)}
              >
                {platform.shells.map((option) => (
                  <option key={option} value={option}>
                    {SHELL_LABELS[option]}
                  </option>
                ))}
              </select>
            ) : (
              platform && <span className="assist-shell-name">{SHELL_LABELS[platform.shell]}</span>
            )}
          </p>
          <label className="field">
            <span>{t('Was soll passieren?')}</span>
            <input
              ref={requestRef}
              data-autofocus
              value={request}
              maxLength={1000}
              placeholder={t('z. B. liste alle Benutzer auf')}
              onChange={(event) => setRequest(event.target.value)}
              onKeyDown={onRequestKey}
              spellCheck={false}
            />
          </label>

          {busy && (
            <p className="assist-status" role="status">
              {t('Denkt nach …')}
            </p>
          )}

          {suggestion && !busy && (
            <div className="assist-result" data-dangerous={suggestion.dangerous}>
              {suggestion.command ? (
                <>
                  <div className="assist-command-row">
                    <input
                      className="assist-command"
                      aria-label={t('Befehl')}
                      value={command}
                      onChange={(event) => setCommand(event.target.value)}
                      onKeyDown={(event) => {
                        if (event.key === 'Enter') {
                          event.preventDefault();
                          void insert();
                        }
                      }}
                      spellCheck={false}
                    />
                    {suggestion.cached && (
                      <span
                        className="assist-badge"
                        title={t('Diese Antwort kam ohne Modell aus dem Cache.')}
                      >
                        {t('aus Cache')}
                      </span>
                    )}
                  </div>
                  {suggestion.explanation && (
                    <p className="assist-explanation">{suggestion.explanation}</p>
                  )}
                  {suggestion.dangerous && (
                    <p className="assist-danger" role="alert">
                      {t(
                        'Vorsicht: Dieser Befehl kann Daten löschen, Dienste stoppen oder das System verändern. Lies ihn, bevor du Enter drückst.',
                      )}
                    </p>
                  )}
                  <p className="field-hint">
                    {t(
                      'Einfügen tippt den Befehl nur ein. Ausgeführt wird er erst mit deinem Enter.',
                    )}
                  </p>
                </>
              ) : (
                <p className="assist-explanation">
                  {suggestion.explanation || t('Dafür gibt es keinen einzelnen Befehl.')}
                </p>
              )}
            </div>
          )}

          {failure && (
            <div className="assist-failure">
              <p className="form-error" role="alert">
                {assistErrorText(failure)}
              </p>
              {notSetUp && <button onClick={onSetUp}>{t('KI einrichten')}</button>}
            </div>
          )}
        </div>
      </Modal>
      {unlock && (
        <VaultDialog
          reason={t('Der API-Key für die KI liegt im Tresor.')}
          onDone={() => {
            const { fresh } = unlock;
            setUnlock(null);
            void generate(fresh);
          }}
          onCancel={() => {
            setUnlock(null);
            setFailure({ code: 'vault-locked', detail: null });
            requestRef.current?.focus();
          }}
        />
      )}
    </>
  );
}
