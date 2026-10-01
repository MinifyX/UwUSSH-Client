/**
 * The command assistant: a request in words to one command for the system a
 * terminal is connected to (see `src-tauri/src/assist.rs`). The page only ever
 * sees settings without keys, suggestions and cache entries; an API key goes
 * in once, through the settings form, and lives sealed in the vault.
 */

import { invoke } from '@tauri-apps/api/core';
import { N_, t, type Language } from './i18n';
import type { SessionId } from './session';

export type ProviderKind = 'ollama' | 'openai-compatible' | 'openai' | 'anthropic' | 'mistral';

export type Shell =
  'bash' | 'zsh' | 'fish' | 'sh' | 'busybox' | 'powershell' | 'cmd' | 'ios' | 'routeros';

export type AssistTarget = { kind: 'local' } | { kind: 'host'; os: string | null };

export type PlatformInfo = {
  key: string;
  shell: Shell;
  shells: Shell[];
  os: string | null;
};

export type Suggestion = {
  command: string;
  explanation: string;
  dangerous: boolean;
  cached: boolean;
  platform: string;
};

export type AssistProviderSettings = { model: string; baseUrl: string | null; hasKey: boolean };

export type AssistSettings = {
  /** The provider in use; empty when the assistant is off. */
  provider: ProviderKind | '';
  providers: Partial<Record<ProviderKind, AssistProviderSettings>>;
};

export type KeyChange = { kind: 'keep' } | { kind: 'set'; value: string } | { kind: 'forget' };

export type AssistProviderDraft = {
  active: ProviderKind | '';
  kind: ProviderKind;
  model: string;
  baseUrl: string | null;
  key: KeyChange;
};

export type CacheEntry = {
  id: string;
  platform: string;
  request: string;
  normalized: string;
  command: string;
  explanation: string;
  dangerous: boolean;
  createdMs: number;
  usedMs: number;
  hits: number;
};

/** An error from the Rust side: a code the page has words for, and detail. */
export type AssistFailure = { code: string; detail: string | null };

export type ProviderInfo = {
  kind: ProviderKind;
  label: string;
  /** The person picks the address: servers they run themselves. */
  baseUrlEditable: boolean;
  defaultBaseUrl: string | null;
  keyRequired: boolean;
  keyAllowed: boolean;
  suggestedModels: string[];
  /** One line about it, German, translated where shown. */
  hint: string;
};

/** The providers, in the order the settings offer them. Mirrors `ProviderKind` in Rust. */
export const PROVIDERS: ProviderInfo[] = [
  {
    kind: 'ollama',
    label: 'Ollama',
    baseUrlEditable: true,
    defaultBaseUrl: 'http://localhost:11434',
    keyRequired: false,
    keyAllowed: false,
    suggestedModels: [],
    hint: N_('Läuft auf diesem Rechner oder im Heimnetz. Nichts verlässt dein Netz.'),
  },
  {
    kind: 'openai-compatible',
    label: 'OpenAI-kompatibel',
    baseUrlEditable: true,
    defaultBaseUrl: null,
    keyRequired: false,
    keyAllowed: true,
    suggestedModels: [],
    hint: N_('llama.cpp, LM Studio, vLLM und andere Server mit /v1/chat/completions.'),
  },
  {
    kind: 'openai',
    label: 'OpenAI',
    baseUrlEditable: false,
    defaultBaseUrl: 'https://api.openai.com/v1',
    keyRequired: true,
    keyAllowed: true,
    suggestedModels: ['gpt-5-mini', 'gpt-5-nano', 'gpt-5'],
    hint: N_('Braucht einen API-Key von platform.openai.com.'),
  },
  {
    kind: 'anthropic',
    label: 'Anthropic',
    baseUrlEditable: false,
    defaultBaseUrl: 'https://api.anthropic.com/v1',
    keyRequired: true,
    keyAllowed: true,
    suggestedModels: ['claude-sonnet-5-5', 'claude-haiku-4-5'],
    hint: N_('Braucht einen API-Key von console.anthropic.com.'),
  },
  {
    kind: 'mistral',
    label: 'Mistral',
    baseUrlEditable: false,
    defaultBaseUrl: 'https://api.mistral.ai/v1',
    keyRequired: true,
    keyAllowed: true,
    suggestedModels: ['mistral-small-latest', 'mistral-medium-latest', 'codestral-latest'],
    hint: N_('Braucht einen API-Key von console.mistral.ai.'),
  },
];

export function providerInfo(kind: ProviderKind): ProviderInfo {
  return PROVIDERS.find((provider) => provider.kind === kind) ?? PROVIDERS[0]!;
}

export function providerLabel(kind: ProviderKind): string {
  return kind === 'openai-compatible' ? t('OpenAI-kompatibel') : providerInfo(kind).label;
}

export const SHELL_LABELS: Record<Shell, string> = {
  bash: 'bash',
  zsh: 'zsh',
  fish: 'fish',
  sh: 'sh',
  busybox: 'BusyBox',
  powershell: 'PowerShell',
  cmd: 'cmd',
  ios: 'Cisco IOS',
  routeros: 'RouterOS',
};

export function asAssistFailure(error: unknown): AssistFailure {
  if (error && typeof error === 'object' && 'code' in error) {
    const { code, detail } = error as { code: unknown; detail?: unknown };
    return { code: String(code), detail: typeof detail === 'string' ? detail : null };
  }
  return { code: 'internal', detail: String(error) };
}

const ADDRESS_PROBLEMS: Record<string, string> = {
  missing: N_('Gib die Adresse des Servers ein.'),
  'too-long': N_('Die Adresse ist zu lang.'),
  'not-a-url': N_('Das ist keine Web-Adresse.'),
  scheme: N_('Die Adresse muss mit http:// oder https:// beginnen.'),
  login: N_('Die Adresse darf keine Anmeldedaten enthalten.'),
  query: N_('Die Adresse darf nichts nach ? oder # enthalten.'),
  'no-host': N_('Der Adresse fehlt der Server.'),
  'refused-ip': N_('Diese IP-Adresse kann nicht verwendet werden.'),
  insecure: N_(
    'Unverschlüsseltes http:// geht nur zu diesem Rechner oder ins Heimnetz. Nimm https://.',
  ),
};

/** What went wrong, in plain words. */
export function assistErrorText(failure: AssistFailure): string {
  const detail = failure.detail;
  switch (failure.code) {
    case 'not-configured':
      return t('Noch kein KI-Anbieter eingerichtet. Richte einen unter Einstellungen → KI ein.');
    case 'no-model':
      return t('Kein Modell gewählt. Wähle eines unter Einstellungen → KI.');
    case 'no-key':
      return t('Für diesen Anbieter fehlt der API-Key.');
    case 'address':
      return t(ADDRESS_PROBLEMS[detail ?? ''] ?? N_('Die Adresse kann nicht verwendet werden.'));
    case 'request':
      return t('Die Anfrage ist leer oder zu lang.');
    case 'unreachable':
      return t('Der Anbieter ist nicht erreichbar. Prüfe Adresse und Verbindung.');
    case 'timeout':
      return t('Das Modell hat zu lange gebraucht.');
    case 'unauthorized':
      return detail
        ? t('Der Anbieter hat den API-Key abgelehnt: {detail}', { detail })
        : t('Der Anbieter hat den API-Key abgelehnt.');
    case 'rate-limited':
      return t('Der Anbieter ist gerade ausgelastet. Versuch es gleich noch einmal.');
    case 'not-found':
      return detail
        ? t('Modell oder Adresse nicht gefunden: {detail}', { detail })
        : t('Modell oder Adresse nicht gefunden.');
    case 'provider':
      return t('Der Anbieter meldet einen Fehler: {detail}', { detail: detail ?? '' });
    case 'unreadable':
      return t('Die Antwort des Modells war nicht lesbar. Versuch es noch einmal.');
    case 'multiline':
      return t('Das Modell hat mehrere Zeilen geliefert. Das wird nicht eingefügt.');
    case 'refused':
      return t('Das Modell hat die Antwort verweigert.');
    case 'vault-locked':
      return t('Der API-Key liegt im Tresor, und der ist gesperrt.');
    default:
      return t('Etwas ist schiefgegangen: {detail}', { detail: detail ?? failure.code });
  }
}

export function assistPlatform(target: AssistTarget, shell: Shell | null): Promise<PlatformInfo> {
  return invoke<PlatformInfo>('assist_platform', { target, shell });
}

export function assistGenerate(
  target: AssistTarget,
  shell: Shell | null,
  request: string,
  language: Language,
  fresh: boolean,
): Promise<Suggestion> {
  return invoke<Suggestion>('assist_generate', { target, shell, request, language, fresh });
}

/** Typed at the prompt, never with Enter: Rust drops every control character. */
export function assistTypeCommand(id: SessionId, command: string): Promise<void> {
  return invoke('assist_type_command', { id, command });
}

export function assistSettings(): Promise<AssistSettings> {
  return invoke<AssistSettings>('assist_settings');
}

export function saveAssistSettings(draft: AssistProviderDraft): Promise<AssistSettings> {
  return invoke<AssistSettings>('assist_save_settings', { draft });
}

/** The provider's models; also the connection test. `apiKey` null: the one in the vault. */
export function assistModels(
  kind: ProviderKind,
  baseUrl: string | null,
  apiKey: string | null,
): Promise<string[]> {
  return invoke<string[]>('assist_models', { kind, baseUrl, apiKey });
}

export function detectOllama(baseUrl: string | null = null): Promise<string[] | null> {
  return invoke<string[] | null>('assist_detect_ollama', { baseUrl });
}

export function assistCacheList(): Promise<CacheEntry[]> {
  return invoke<CacheEntry[]>('assist_cache_list');
}

export function assistCacheDelete(id: string): Promise<void> {
  return invoke('assist_cache_delete', { id });
}

export function assistCacheClear(): Promise<number> {
  return invoke<number>('assist_cache_clear');
}
