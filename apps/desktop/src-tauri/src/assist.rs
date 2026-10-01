//! The command assistant: Ctrl+Shift+K (Cmd+K on a Mac) over a terminal, a
//! request in words, one command typed into the prompt — never run.
//!
//! The cache is asked first, offline; only a miss goes to the model. Which
//! provider, which model and the API key come from the store, the key opened
//! from the vault for the one request and dropped after it. A locked vault is
//! the page's cue to ask for the master password and try again.

use crate::AppState;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock};
use tauri::State;
use uuid::Uuid;
use uwussh_assist::{
    best_match, normalize, AssistError, Client, Endpoint, Failure, Language, Platform,
    ProviderKind, Shell, OLLAMA_DEFAULT_URL,
};
use uwussh_core::SessionId;
use uwussh_store::{
    AssistCacheEntry, AssistProviderDraft, AssistSettings, NewCacheEntry, Store, StoreError,
};

type AssistResult<T> = Result<T, Failure>;

fn failure(error: AssistError) -> Failure {
    error.failure()
}

fn store_failure(error: StoreError) -> Failure {
    match error {
        StoreError::VaultLocked | StoreError::NoVault => Failure {
            code: "vault-locked",
            detail: None,
        },
        other => Failure {
            code: "store",
            detail: Some(other.to_string()),
        },
    }
}

fn internal(error: impl std::fmt::Display) -> Failure {
    Failure {
        code: "internal",
        detail: Some(error.to_string()),
    }
}

/// One HTTP client for the app's life: reading the system's trust store takes
/// a moment on some systems.
fn client() -> AssistResult<&'static Client> {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client);
    }
    let client = Client::new().map_err(failure)?;
    Ok(CLIENT.get_or_init(|| client))
}

/// What the command is for: this machine, or a host as the connection found it.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum Target {
    Local,
    Host { os: Option<String> },
}

/// This machine: its system, and the shell a local tab opens.
fn local_platform() -> Platform {
    let login_shell = std::env::var("SHELL")
        .ok()
        .and_then(|path| Platform::shell_from_path(&path));
    if cfg!(windows) {
        // A local tab on Windows is PowerShell (`uwussh_core::pty`).
        return Platform::for_os(Some("windows"));
    }
    let platform = if cfg!(target_os = "macos") {
        Platform::for_os(Some("macos"))
    } else {
        let release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
        let os = uwussh_core::os::from_probe(&format!("Linux\n{release}")).unwrap_or("linux");
        Platform::for_os(Some(os))
    };
    match login_shell {
        Some(shell) => platform.with_shell(shell),
        None => platform,
    }
}

fn platform_of(target: &Target, shell: Option<Shell>) -> Platform {
    let platform = match target {
        Target::Local => local_platform(),
        Target::Host { os } => Platform::for_os(os.as_deref()),
    };
    match shell {
        Some(shell) => platform.with_shell(shell),
        None => platform,
    }
}

/// The target as the popup shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlatformInfo {
    key: String,
    shell: Shell,
    shells: Vec<Shell>,
    os: Option<String>,
}

#[tauri::command]
pub(crate) fn assist_platform(target: Target, shell: Option<Shell>) -> PlatformInfo {
    let platform = platform_of(&target, shell);
    PlatformInfo {
        key: platform.key(),
        shell: platform.shell,
        shells: platform.shells(),
        os: platform.os.clone(),
    }
}

/// A command for the popup.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Suggestion {
    command: String,
    explanation: String,
    dangerous: bool,
    /// It came from the cache, not from a model.
    cached: bool,
    platform: String,
}

impl Suggestion {
    fn from_cache(entry: AssistCacheEntry) -> Self {
        Self {
            dangerous: entry.dangerous || uwussh_assist::looks_dangerous(&entry.command),
            command: entry.command,
            explanation: entry.explanation,
            cached: true,
            platform: entry.platform,
        }
    }
}

/// The endpoint the settings describe, with its key opened from the vault.
fn endpoint_from_settings(store: &Store) -> AssistResult<(Endpoint, String)> {
    let settings = store.assist_settings().map_err(store_failure)?;
    let kind = ProviderKind::parse(&settings.provider)
        .ok_or_else(|| failure(AssistError::NotConfigured))?;
    let provider = settings
        .providers
        .get(kind.as_str())
        .cloned()
        .unwrap_or_default();
    if provider.model.trim().is_empty() {
        return Err(failure(AssistError::NoModel));
    }
    let key = if kind.key_allowed() && provider.has_key {
        store.assist_api_key(kind.as_str()).map_err(store_failure)?
    } else {
        None
    };
    let endpoint = Endpoint::new(kind, provider.base_url.as_deref(), key).map_err(failure)?;
    Ok((endpoint, provider.model))
}

/// One command for `request`: from the cache when a close enough request was
/// answered before for the same platform, else from the model — which is also
/// what `fresh` asks for, and its answer then takes the cached one's place.
#[tauri::command]
pub(crate) async fn assist_generate(
    state: State<'_, AppState>,
    sync: State<'_, crate::sync::Sync>,
    target: Target,
    shell: Option<Shell>,
    request: String,
    language: Language,
    fresh: bool,
) -> AssistResult<Suggestion> {
    let store = Arc::clone(&state.store);
    let platform = platform_of(&target, shell);
    let key = platform.key();
    let normalized = normalize(&request);

    if !fresh && !normalized.is_empty() {
        let entries = store
            .assist_cache_entries(Some(&key))
            .map_err(store_failure)?;
        let candidates = entries
            .iter()
            .map(|entry| (entry.normalized.as_str(), entry));
        if let Some((entry, _)) = best_match(&normalized, candidates) {
            let _ = store.touch_assist_cache(entry.id);
            return Ok(Suggestion::from_cache(entry.clone()));
        }
    }

    let (endpoint, model) = endpoint_from_settings(&store)?;
    let client = client()?;
    let asked = request.clone();
    let task_platform = platform.clone();
    let answer = tauri::async_runtime::spawn_blocking(move || {
        client.generate(&endpoint, &model, &task_platform, language, &asked)
    })
    .await
    .map_err(internal)?
    .map_err(failure)?;

    if !answer.command.is_empty() && !normalized.is_empty() {
        let stored = store.put_assist_cache(NewCacheEntry {
            platform: key.clone(),
            request: request.trim().to_string(),
            normalized,
            command: answer.command.clone(),
            explanation: answer.explanation.clone(),
            dangerous: answer.dangerous,
        });
        match stored {
            Ok(_) => sync.poke(),
            Err(error) => tracing::warn!(%error, "an answer could not be cached"),
        }
    }
    Ok(Suggestion {
        command: answer.command,
        explanation: answer.explanation,
        dangerous: answer.dangerous,
        cached: false,
        platform: key,
    })
}

/// Type a command at the terminal's prompt. Without Enter, and without
/// anything that would act like it: every control character is dropped here,
/// whatever the page sent.
#[tauri::command]
pub(crate) fn assist_type_command(
    state: State<'_, AppState>,
    id: SessionId,
    command: String,
) -> AssistResult<()> {
    let line: String = command
        .chars()
        .map(|c| if c == '\t' { ' ' } else { c })
        .filter(|c| !c.is_control())
        .collect();
    let line = line.trim();
    if line.is_empty() {
        return Ok(());
    }
    state.sessions.write(id, line.as_bytes()).map_err(internal)
}

#[tauri::command]
pub(crate) fn assist_settings(state: State<'_, AppState>) -> AssistResult<AssistSettings> {
    state.store.assist_settings().map_err(store_failure)
}

/// Save one provider's settings. An address is checked before it is kept.
#[tauri::command]
pub(crate) fn assist_save_settings(
    state: State<'_, AppState>,
    sync: State<'_, crate::sync::Sync>,
    mut draft: AssistProviderDraft,
) -> AssistResult<AssistSettings> {
    if !draft.active.is_empty() && ProviderKind::parse(&draft.active).is_none() {
        return Err(failure(AssistError::NotConfigured));
    }
    let kind =
        ProviderKind::parse(&draft.kind).ok_or_else(|| failure(AssistError::NotConfigured))?;
    draft.base_url = match draft.base_url.as_deref().map(str::trim) {
        Some(url) if !url.is_empty() && kind.base_url_editable() => {
            Some(uwussh_assist::check_base_url(kind, url).map_err(failure)?)
        }
        _ => None,
    };
    let saved = state
        .store
        .save_assist_settings(draft)
        .map_err(store_failure)?;
    sync.poke();
    Ok(saved)
}

/// The models a provider offers — which is also the connection test. With a
/// key typed into the form, that key; without, the one in the vault.
#[tauri::command]
pub(crate) async fn assist_models(
    state: State<'_, AppState>,
    kind: String,
    base_url: Option<String>,
    api_key: Option<String>,
) -> AssistResult<Vec<String>> {
    let kind = ProviderKind::parse(&kind).ok_or_else(|| failure(AssistError::NotConfigured))?;
    let key = match api_key.filter(|key| !key.trim().is_empty()) {
        Some(key) => Some(zeroize::Zeroizing::new(key)),
        None if kind.key_allowed() => state
            .store
            .assist_api_key(kind.as_str())
            .map_err(store_failure)?,
        None => None,
    };
    let endpoint = Endpoint::new(kind, base_url.as_deref(), key).map_err(failure)?;
    let client = client()?;
    tauri::async_runtime::spawn_blocking(move || client.models(&endpoint))
        .await
        .map_err(internal)?
        .map_err(failure)
}

/// Whether Ollama runs at the address (this machine by default), and which
/// models it has.
#[tauri::command]
pub(crate) async fn assist_detect_ollama(base_url: Option<String>) -> Option<Vec<String>> {
    let client = client().ok()?;
    let base = base_url
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| OLLAMA_DEFAULT_URL.to_string());
    tauri::async_runtime::spawn_blocking(move || client.detect_ollama(&base))
        .await
        .ok()
        .flatten()
}

#[tauri::command]
pub(crate) fn assist_cache_list(state: State<'_, AppState>) -> AssistResult<Vec<AssistCacheEntry>> {
    state
        .store
        .assist_cache_entries(None)
        .map_err(store_failure)
}

#[tauri::command]
pub(crate) fn assist_cache_delete(
    state: State<'_, AppState>,
    sync: State<'_, crate::sync::Sync>,
    id: Uuid,
) -> AssistResult<()> {
    state
        .store
        .clear_assist_cache_entry(id)
        .map_err(store_failure)?;
    sync.poke();
    Ok(())
}

#[tauri::command]
pub(crate) fn assist_cache_clear(
    state: State<'_, AppState>,
    sync: State<'_, crate::sync::Sync>,
) -> AssistResult<usize> {
    let cleared = state.store.clear_assist_cache().map_err(store_failure)?;
    sync.poke();
    Ok(cleared)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_target_follows_what_the_connection_found() {
        let ubuntu = platform_of(
            &Target::Host {
                os: Some("ubuntu".into()),
            },
            None,
        );
        assert_eq!(ubuntu.key(), "linux/bash/apt");
        let zsh = platform_of(
            &Target::Host {
                os: Some("ubuntu".into()),
            },
            Some(Shell::Zsh),
        );
        assert_eq!(zsh.key(), "linux/zsh/apt");
        let unknown = platform_of(&Target::Host { os: None }, None);
        assert_eq!(unknown.key(), "unknown/sh/none");
    }

    #[test]
    fn this_machine_is_a_platform_of_its_own_kind() {
        let local = local_platform();
        if cfg!(windows) {
            assert_eq!(local.shell, Shell::Powershell);
        } else if cfg!(target_os = "macos") {
            assert_eq!(local.family, uwussh_assist::Family::Macos);
        } else {
            assert_eq!(local.family, uwussh_assist::Family::Linux);
        }
    }

    #[test]
    fn a_locked_vault_is_a_code_the_page_acts_on() {
        assert_eq!(store_failure(StoreError::VaultLocked).code, "vault-locked");
        assert_eq!(failure(AssistError::Timeout).code, "timeout");
    }
}
