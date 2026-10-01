import { useCallback, useEffect, useState } from 'react';
import {
  asAssistFailure,
  assistCacheClear,
  assistCacheDelete,
  assistCacheList,
  assistErrorText,
  assistModels,
  assistSettings,
  detectOllama,
  PROVIDERS,
  providerInfo,
  providerLabel,
  saveAssistSettings,
  type AssistFailure,
  type AssistSettings as Settings,
  type CacheEntry,
  type KeyChange,
  type ProviderKind,
} from '../lib/assist';
import { locale, t, useLanguage } from '../lib/i18n';
import { keysFor } from '../lib/shortcuts';
import { Icon } from './Icon';
import { VaultDialog } from './VaultDialog';

type Result = { tone: 'ok' | 'error'; text: string };

type Props = {
  /** After a save that switched the assistant on, off or to other settings. */
  onSaved?: (settings: Settings) => void;
  /** The primary button; the onboarding wizard may call it "Weiter". */
  saveLabel?: string;
};

/**
 * Picking and testing a provider: which one, the model, the address for
 * servers people run themselves, and the API key — which goes straight into
 * the vault and never comes back to the page. Finds Ollama on this machine by
 * itself. Self-contained so the onboarding wizard can show it as one step.
 */
export function AssistProviderSetup({ onSaved, saveLabel }: Props) {
  useLanguage();
  const [settings, setSettings] = useState<Settings | null>(null);
  const [kind, setKind] = useState<ProviderKind>('ollama');
  const [model, setModel] = useState('');
  const [baseUrl, setBaseUrl] = useState('');
  const [apiKey, setApiKey] = useState('');
  const [forgetKey, setForgetKey] = useState(false);
  const [models, setModels] = useState<string[] | null>(null);
  const [ollama, setOllama] = useState<string[] | null>(null);
  const [busy, setBusy] = useState<'test' | 'save' | null>(null);
  const [result, setResult] = useState<Result | null>(null);
  /** What to do again once the vault is open. */
  const [unlock, setUnlock] = useState<(() => void) | null>(null);

  const info = providerInfo(kind);
  const stored = settings?.providers[kind];

  const pick = useCallback((next: ProviderKind, from: Settings | null) => {
    const saved = from?.providers[next];
    setKind(next);
    setModel(saved?.model ?? '');
    setBaseUrl(saved?.baseUrl ?? '');
    setApiKey('');
    setForgetKey(false);
    setModels(null);
    setResult(null);
  }, []);

  useEffect(() => {
    let current = true;
    void Promise.all([assistSettings(), detectOllama().catch(() => null)])
      .then(([loaded, found]) => {
        if (!current) return;
        setSettings(loaded);
        setOllama(found);
        // Nothing chosen yet: Ollama first, with its models if it runs here.
        pick(loaded.provider || 'ollama', loaded);
        if (!loaded.provider && found) {
          setModels(found);
          if (found[0]) setModel(found[0]);
        }
      })
      .catch((error) => current && setResult(failureResult(asAssistFailure(error))));
    return () => {
      current = false;
    };
  }, [pick]);

  const failureResult = (failure: AssistFailure): Result => ({
    tone: 'error',
    text: assistErrorText(failure),
  });

  const handle = (error: unknown, retry: () => void) => {
    const failure = asAssistFailure(error);
    if (failure.code === 'vault-locked') setUnlock(() => retry);
    else setResult(failureResult(failure));
  };

  const address = info.baseUrlEditable ? baseUrl.trim() || info.defaultBaseUrl : null;

  const test = async () => {
    setBusy('test');
    setResult(null);
    try {
      const found = await assistModels(kind, address, apiKey.trim() || null);
      setModels(found);
      if (kind === 'ollama') setOllama(found);
      if (!model && found[0]) setModel(found[0]);
      setResult({
        tone: 'ok',
        text:
          found.length === 0
            ? t('Verbunden, aber der Anbieter nennt keine Modelle.')
            : found.length === 1
              ? t('Verbunden. 1 Modell verfügbar.')
              : t('Verbunden. {count} Modelle verfügbar.', { count: found.length }),
      });
    } catch (error) {
      handle(error, () => void test());
    } finally {
      setBusy(null);
    }
  };

  const save = async (active: ProviderKind | '') => {
    setBusy('save');
    setResult(null);
    const key: KeyChange = apiKey.trim()
      ? { kind: 'set', value: apiKey.trim() }
      : forgetKey
        ? { kind: 'forget' }
        : { kind: 'keep' };
    try {
      const saved = await saveAssistSettings({
        active,
        kind,
        model: model.trim(),
        baseUrl: info.baseUrlEditable ? baseUrl.trim() || null : null,
        key,
      });
      setSettings(saved);
      setApiKey('');
      setForgetKey(false);
      setResult({
        tone: 'ok',
        text: active
          ? t('Gespeichert. {shortcut} öffnet den Assistenten über dem Terminal.', {
              shortcut: keysFor('assist'),
            })
          : t('Der Assistent ist aus.'),
      });
      onSaved?.(saved);
    } catch (error) {
      handle(error, () => void save(active));
    } finally {
      setBusy(null);
    }
  };

  if (!settings) {
    return (
      <p className="setting-description">
        {result?.tone === 'error' ? result.text : t('Wird geladen…')}
      </p>
    );
  }

  const suggestions = models ?? (kind === 'ollama' ? (ollama ?? []) : info.suggestedModels);
  const keyMissing = info.keyRequired && !apiKey.trim() && (!stored?.hasKey || forgetKey);

  return (
    <div className="assist-provider">
      <label className="field">
        <span>{t('Anbieter')}</span>
        <select
          className="select"
          value={kind}
          onChange={(event) => pick(event.target.value as ProviderKind, settings)}
        >
          {PROVIDERS.map((provider) => (
            <option key={provider.kind} value={provider.kind}>
              {providerLabel(provider.kind)}
              {provider.kind === settings.provider ? ` · ${t('aktiv')}` : ''}
            </option>
          ))}
        </select>
        <em className="field-hint">
          {t(info.hint)}
          {kind === 'ollama' &&
            ollama !== null &&
            ` ${t('Ollama läuft hier ({count} Modelle).', { count: ollama.length })}`}
        </em>
      </label>

      {info.baseUrlEditable && (
        <label className="field">
          <span>{t('Adresse')}</span>
          <input
            value={baseUrl}
            placeholder={info.defaultBaseUrl ?? 'http://192.0.2.10:8080/v1'}
            spellCheck={false}
            autoComplete="off"
            onChange={(event) => setBaseUrl(event.target.value)}
          />
        </label>
      )}

      {info.keyAllowed && (
        <label className="field">
          <span>
            {info.keyRequired ? t('API-Key') : t('API-Key (optional)')}
            {stored?.hasKey && !forgetKey && ` · ${t('im Tresor gespeichert')}`}
          </span>
          <div className="assist-provider-row">
            <input
              type="password"
              value={apiKey}
              autoComplete="off"
              spellCheck={false}
              placeholder={stored?.hasKey && !forgetKey ? t('Leer lassen, um ihn zu behalten') : ''}
              onChange={(event) => setApiKey(event.target.value)}
            />
            {stored?.hasKey && (
              <button type="button" onClick={() => setForgetKey(!forgetKey)}>
                {forgetKey ? t('Doch behalten') : t('Key entfernen')}
              </button>
            )}
          </div>
          <em className="field-hint">
            {t('Der Key liegt verschlüsselt im Tresor und wird mit deinen Geräten synchronisiert.')}
          </em>
        </label>
      )}

      <label className="field">
        <span>{t('Modell')}</span>
        <input
          value={model}
          list="assist-models"
          spellCheck={false}
          autoComplete="off"
          placeholder={suggestions[0] ?? ''}
          onChange={(event) => setModel(event.target.value)}
        />
        <datalist id="assist-models">
          {suggestions.map((name) => (
            <option key={name} value={name} />
          ))}
        </datalist>
      </label>
      {suggestions.length > 0 && (
        <div className="assist-models" aria-label={t('Verfügbare Modelle')}>
          {suggestions.slice(0, 12).map((name) => (
            <button
              key={name}
              type="button"
              aria-pressed={name === model.trim()}
              onClick={() => setModel(name)}
            >
              {name}
            </button>
          ))}
        </div>
      )}

      <div className="assist-provider-row">
        <button type="button" onClick={() => void test()} disabled={busy !== null}>
          {busy === 'test' ? t('Teste…') : t('Verbindung testen')}
        </button>
        <span className="spacer" />
        {settings.provider && (
          <button type="button" onClick={() => void save('')} disabled={busy !== null}>
            {t('Ausschalten')}
          </button>
        )}
        <button
          type="button"
          className="primary"
          onClick={() => void save(kind)}
          disabled={busy !== null || !model.trim() || keyMissing}
        >
          {saveLabel ?? t('Speichern und verwenden')}
        </button>
      </div>
      {result && (
        <p
          className="setting-result"
          data-tone={result.tone === 'error' ? 'error' : undefined}
          role="status"
        >
          {result.text}
        </p>
      )}

      {unlock && (
        <VaultDialog
          reason={t('Der API-Key für die KI liegt im Tresor.')}
          onDone={() => {
            const again = unlock;
            setUnlock(null);
            again();
          }}
          onCancel={() => setUnlock(null)}
        />
      )}
    </div>
  );
}

/**
 * Answers the assistant gave before, which it reuses for close enough
 * requests on the same kind of system — offline and without asking a model.
 * Synced like everything else; deleting one deletes it everywhere.
 */
function AssistCache() {
  const [entries, setEntries] = useState<CacheEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(() => {
    void assistCacheList()
      .then(setEntries)
      .catch((e) => setError(assistErrorText(asAssistFailure(e))));
  }, []);
  useEffect(load, [load]);

  const guard = (action: () => Promise<unknown>) =>
    void action()
      .then(load)
      .catch((e) => setError(assistErrorText(asAssistFailure(e))));

  return (
    <div className="key-list">
      <div className="key-list-head">
        <p className="setting-label">{t('Gemerkte Befehle')}</p>
        <span className="spacer" />
        <button onClick={() => guard(assistCacheClear)} disabled={!entries || entries.length === 0}>
          <Icon name="trash" size={15} />
          {t('Alle löschen')}
        </button>
      </div>
      {entries && entries.length === 0 ? (
        <p className="setting-description">
          {t(
            'Noch nichts gemerkt. Jede Antwort landet hier, und eine ähnliche Frage für dasselbe System kommt danach ohne Modell aus dem Cache.',
          )}
        </p>
      ) : (
        <ul>
          {(entries ?? []).map((entry) => (
            <li key={entry.id}>
              <Icon name="sparkles" size={16} />
              <span className="key-list-text">
                <b className="assist-cache-command" title={entry.command}>
                  {entry.command}
                </b>
                <small title={entry.explanation}>
                  {entry.request} · {entry.platform}
                  {entry.hits > 0 &&
                    ` · ${entry.hits === 1 ? t('1× wiederverwendet') : t('{count}× wiederverwendet', { count: entry.hits })}`}
                  {' · '}
                  {new Date(entry.usedMs || entry.createdMs).toLocaleDateString(locale())}
                  {entry.dangerous && (
                    <span className="assist-cache-danger"> · {t('gefährlich')}</span>
                  )}
                </small>
              </span>
              <span className="spacer" />
              <button
                className="icon-button"
                title={t('Löschen')}
                aria-label={t('{label} löschen', { label: entry.command })}
                onClick={() => guard(() => assistCacheDelete(entry.id))}
              >
                <Icon name="trash" size={15} />
              </button>
            </li>
          ))}
        </ul>
      )}
      {error && (
        <p className="setting-result" data-tone="error">
          {error}
        </p>
      )}
    </div>
  );
}

/** Settings → KI: the provider, and the answers kept for reuse. */
export function AssistSettings() {
  useLanguage();
  return (
    <>
      <p className="setting-description">
        {t(
          'Beschreibe in Worten, was du willst – der Assistent schreibt einen passenden Befehl für das System im Terminal und tippt ihn ein. Ausgeführt wird erst mit deinem Enter. Öffnen mit {shortcut} oder dem Funken über den Tabs.',
          { shortcut: keysFor('assist') },
        )}
      </p>
      <AssistProviderSetup />
      <AssistCache />
    </>
  );
}
