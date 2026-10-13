import { Button, Hint, Icon, IconButton, ICONS, Select, Tag } from '@uwusuite/design';
import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react';
import {
  asAssistFailure,
  assistCacheDelete,
  assistCacheList,
  assistErrorText,
  assistGenerate,
  assistPlatform,
  assistTypeCommand,
  SHELL_LABELS,
  type AssistFailure,
  type AssistTarget,
  type CacheEntry,
  type PlatformInfo,
  type Shell,
  type Suggestion,
} from '../lib/assist';
import { t, useLanguage } from '../lib/i18n';
import type { SessionId } from '../lib/session';
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

/** How many earlier requests the popup lists. */
const HISTORY_LENGTH = 30;

/**
 * The command assistant over a terminal: a request in words, one command for
 * this system, typed at the prompt with Enter or "Einfügen" — never run. A
 * close enough request asked before on the same kind of system comes from the
 * cache; "Neu generieren" asks the model anyway.
 *
 * Below the field is what was asked before, newest first (the cache is the
 * history): a click or ↑/↓ brings a request and its command back, ready for
 * Enter.
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
  const [history, setHistory] = useState<CacheEntry[]>([]);
  /** The entry ↑/↓ stopped at, by id. */
  const [recalled, setRecalled] = useState<string | null>(null);

  const loadHistory = () =>
    assistCacheList()
      .then(setHistory)
      // Locked, or nothing asked yet: no history to show.
      .catch(() => undefined);
  useEffect(() => void loadHistory(), []);

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
      setRecalled(null);
      void loadHistory();
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
  // What was asked before and fits what is typed. A recalled request shows the
  // whole list, so ↑/↓ keep walking through it.
  const shown = useMemo(() => {
    const typed = recalled ? '' : request.trim().toLowerCase();
    const fits = typed
      ? history.filter(
          (entry) =>
            entry.request.toLowerCase().includes(typed) ||
            entry.command.toLowerCase().includes(typed),
        )
      : history;
    return fits.slice(0, HISTORY_LENGTH);
  }, [history, request, recalled]);

  const recall = (entry: CacheEntry) => {
    run.current++;
    setBusy(false);
    setFailure(null);
    setRecalled(entry.id);
    setRequest(entry.request);
    setAsked(entry.request.trim());
    setCommand(entry.command);
    setSuggestion({
      command: entry.command,
      explanation: entry.explanation,
      dangerous: entry.dangerous,
      cached: true,
      platform: entry.platform,
    });
  };

  const forget = (entry: CacheEntry) => {
    setHistory((list) => list.filter((one) => one.id !== entry.id));
    if (recalled === entry.id) setRecalled(null);
    void assistCacheDelete(entry.id).catch(() => void loadHistory());
  };

  const onRequestKey = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.nativeEvent.isComposing) return;
    if ((event.key === 'ArrowUp' || event.key === 'ArrowDown') && shown.length > 0) {
      // ↑ goes back in time like in a shell; the list has the newest on top.
      event.preventDefault();
      const at = shown.findIndex((entry) => entry.id === recalled);
      const next = event.key === 'ArrowUp' ? at + 1 : at - 1;
      const entry = shown[Math.min(shown.length - 1, Math.max(0, next))];
      if (entry) recall(entry);
      return;
    }
    if (event.key !== 'Enter') return;
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
            <Button data-secondary onClick={onClose}>
              {t('Abbrechen')}
            </Button>
            <Button
              data-secondary
              icon={ICONS.refresh}
              onClick={() => void generate(true)}
              disabled={busy || !request.trim()}
              title={t('Fragt das Modell neu, ohne den Cache')}
            >
              {t('Neu generieren')}
            </Button>
            <Button
              variant="primary"
              icon={answered ? undefined : ICONS.ai}
              busy={busy}
              onClick={() => (answered ? void insert() : void generate(false))}
              disabled={busy || !request.trim() || (answered && !command.trim())}
            >
              {answered ? t('Einfügen') : t('Erzeugen')}
            </Button>
          </>
        }
      >
        <div className="assist">
          <p className="assist-target">
            <span>{t('Für {name}', { name: targetName })}</span>
            {platform && platform.shells.length > 1 ? (
              <Select
                className="assist-shell w-40 [&>select]:h-8 [&>select]:text-meta"
                aria-label={t('Shell')}
                value={platform.shell}
                onChange={(event) => pickShell(event.target.value as Shell)}
              >
                {platform.shells.map((option) => (
                  <option key={option} value={option}>
                    {SHELL_LABELS[option]}
                  </option>
                ))}
              </Select>
            ) : (
              platform && <Tag>{SHELL_LABELS[platform.shell]}</Tag>
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
              onChange={(event) => {
                setRequest(event.target.value);
                setRecalled(null);
              }}
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
                      <Tag tone="pink" title={t('Diese Antwort kam ohne Modell aus dem Cache.')}>
                        {t('aus Cache')}
                      </Tag>
                    )}
                  </div>
                  {suggestion.explanation && (
                    <p className="assist-explanation">{suggestion.explanation}</p>
                  )}
                  {suggestion.dangerous && (
                    <Hint tone="danger" className="font-semibold">
                      {t(
                        'Vorsicht: Dieser Befehl kann Daten löschen, Dienste stoppen oder das System verändern. Lies ihn, bevor du Enter drückst.',
                      )}
                    </Hint>
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

          {shown.length > 0 && (
            <section className="assist-history" aria-label={t('Verlauf')}>
              <h3>
                <Icon icon={ICONS.history} size="xs" />
                {t('Verlauf')}
                <span className="assist-history-keys">{t('↑ ↓ blättert')}</span>
              </h3>
              <ul>
                {shown.map((entry) => (
                  <li key={entry.id} data-current={entry.id === recalled || undefined}>
                    <button
                      type="button"
                      className="assist-history-entry"
                      onClick={() => {
                        recall(entry);
                        requestRef.current?.focus();
                      }}
                      title={entry.explanation || undefined}
                    >
                      <span className="assist-history-request">{entry.request}</span>
                      <code data-dangerous={entry.dangerous || undefined}>{entry.command}</code>
                    </button>
                    {platform && entry.platform !== platform.key && (
                      <Tag title={t('Für ein anderes System erzeugt')}>{entry.platform}</Tag>
                    )}
                    <IconButton
                      size="sm"
                      icon={ICONS.delete}
                      label={t('Aus dem Verlauf löschen')}
                      onClick={() => forget(entry)}
                    />
                  </li>
                ))}
              </ul>
            </section>
          )}

          {failure && (
            <div className="assist-failure">
              <p className="form-error" role="alert">
                {assistErrorText(failure)}
              </p>
              {notSetUp && (
                <Button size="sm" icon={ICONS.ai} onClick={onSetUp}>
                  {t('KI einrichten')}
                </Button>
              )}
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
