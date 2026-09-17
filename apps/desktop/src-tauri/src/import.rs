//! The vault and importing another client's setup.
//!
//! The vault comes first when a source brings secrets — Termius keeps its
//! passwords and keys sealed, and they need somewhere to live — but a PuTTY or
//! KiTTY import is only host names, ports and key-file paths, so it needs no
//! vault at all. [`scan_import`] reports what a source holds (counts only, no
//! secrets) and whether it needs the vault; [`run_import`] writes it.

use crate::backup::BackupFailure;
use crate::{err, AppState, CommandResult};
use serde::Serialize;
use std::sync::Arc;
use tauri::State;
use uwussh_core::public_key_fingerprint;
use uwussh_import::termius::{self, TermiusError};
use uwussh_import::{putty, ssh_config, ImportBundle, ImportResult, Source};
use uwussh_store::Store;
use uwussh_store::{
    HostInput, IdentityInput, ImportSet, KeyInput, KnownHostInput, SnippetInput, VaultStatus,
};
use zeroize::Zeroizing;

// ── Vault ────────────────────────────────────────────────────────────────

#[tauri::command]
pub(crate) fn vault_status(state: State<'_, AppState>) -> CommandResult<VaultStatus> {
    state.store.vault_status().map_err(err)
}

/// The vault's status, and whether this device opens it without the master
/// password.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VaultState {
    status: VaultStatus,
    remembered: bool,
}

#[tauri::command]
pub(crate) fn vault_state(state: State<'_, AppState>) -> CommandResult<VaultState> {
    Ok(VaultState {
        status: state.store.vault_status().map_err(err)?,
        remembered: state.store.vault_is_remembered().map_err(err)?,
    })
}

/// Keep the vault key for this Windows user (`true`), or stop (`false`).
fn set_remembered(store: &Store, remember: bool) -> CommandResult<()> {
    if remember {
        store.remember_vault(crate::device::protect).map_err(err)
    } else {
        store.forget_remembered_vault().map_err(err)
    }
}

/// Run the master password's key derivation — about a second on purpose — on
/// a worker thread, so the window keeps drawing meanwhile.
async fn with_store<T: Send + 'static>(
    state: &AppState,
    work: impl FnOnce(&Store) -> CommandResult<T> + Send + 'static,
) -> CommandResult<T> {
    let store = Arc::clone(&state.store);
    tauri::async_runtime::spawn_blocking(move || work(&store))
        .await
        .map_err(err)?
}

#[tauri::command]
pub(crate) async fn create_vault(
    state: State<'_, AppState>,
    password: String,
    remember: bool,
) -> CommandResult<()> {
    let password = Zeroizing::new(password);
    if password.trim().is_empty() {
        return Err("the master password cannot be empty".into());
    }
    with_store(&state, move |store| {
        store.create_vault(password.as_bytes()).map_err(err)?;
        set_remembered(store, remember)
    })
    .await
}

/// Unlock with the master password. `remember` changes whether this device
/// keeps the key; `None` leaves that as it is.
#[tauri::command]
pub(crate) async fn unlock_vault(
    state: State<'_, AppState>,
    password: String,
    remember: Option<bool>,
) -> CommandResult<()> {
    let password = Zeroizing::new(password);
    with_store(&state, move |store| {
        store.unlock_vault(password.as_bytes()).map_err(err)?;
        match remember {
            Some(remember) => set_remembered(store, remember),
            None => Ok(()),
        }
    })
    .await
}

#[tauri::command]
pub(crate) fn set_vault_remembered(
    state: State<'_, AppState>,
    remember: bool,
) -> CommandResult<()> {
    set_remembered(&state.store, remember)
}

#[tauri::command]
pub(crate) fn lock_vault(state: State<'_, AppState>) {
    state.store.lock_vault();
}

// ── Import ─────────────────────────────────────────────────────────────────

/// The sources UwUSSH can import from, by the id the frontend uses.
const TERMIUS: &str = "termius";
const PUTTY: &str = "putty";
const KITTY: &str = "kitty";
const OPENSSH: &str = "openssh";

/// Which sources have something to import on this machine.
#[tauri::command]
pub(crate) fn available_imports() -> Vec<&'static str> {
    let mut sources = Vec::new();
    if termius::is_installed() {
        sources.push(TERMIUS);
    }
    if putty::has_sessions(putty::PUTTY_REGISTRY_PATH) {
        sources.push(PUTTY);
    }
    if putty::has_sessions(putty::KITTY_REGISTRY_PATH) {
        sources.push(KITTY);
    }
    if ssh_config::has_config() {
        sources.push(OPENSSH);
    }
    sources
}

/// What an import would bring, in counts. Contains no host names, addresses or
/// secrets, so it is safe to hand to the webview for a preview.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImportSummary {
    pub hosts: usize,
    pub identities: usize,
    pub keys: usize,
    pub known_hosts: usize,
    pub snippets: usize,
    /// Whether this import carries secrets, which the vault seals if their
    /// hosts are new. A PuTTY or KiTTY import has none.
    pub needs_vault: bool,
    /// One line per thing that could not be imported, with the reason.
    pub skipped: Vec<String>,
}

/// What an import actually changed.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImportReport {
    pub hosts_added: usize,
    pub hosts_skipped: usize,
    pub identities_added: usize,
    pub keys_added: usize,
    pub known_hosts_added: usize,
    pub snippets_added: usize,
    pub skipped: Vec<String>,
}

impl ImportReport {
    pub(crate) fn of(outcome: uwussh_store::ImportOutcome, skipped: Vec<String>) -> Self {
        Self {
            hosts_added: outcome.hosts_added,
            hosts_skipped: outcome.hosts_skipped,
            identities_added: outcome.identities_added,
            keys_added: outcome.keys_added,
            known_hosts_added: outcome.known_hosts_added,
            snippets_added: outcome.snippets_added,
            skipped,
        }
    }
}

#[tauri::command]
pub(crate) fn scan_import(source: String) -> Result<ImportSummary, String> {
    let (set, mut skipped) = to_import_set(read_bundle(&source)?);
    skipped.sort();
    Ok(ImportSummary {
        hosts: set.hosts.len(),
        identities: set.identities.len(),
        keys: set.keys.len(),
        known_hosts: set.known_hosts.len(),
        snippets: set.snippets.len(),
        needs_vault: needs_vault(&set),
        skipped,
    })
}

#[tauri::command]
pub(crate) fn run_import(
    state: State<'_, AppState>,
    source: String,
) -> Result<ImportReport, BackupFailure> {
    let (set, mut skipped) =
        to_import_set(read_bundle(&source).map_err(|message| BackupFailure::Error { message })?);
    skipped.sort();

    // A secret that has to be written fails with `vault-locked`, and the page
    // asks for the vault and runs the import again. Hosts that are already
    // there bring no secret, so a second import doesn't ask at all.
    let outcome = state.store.import(set)?;
    Ok(ImportReport::of(outcome, skipped))
}

/// Whether the set carries anything that has to be sealed.
fn needs_vault(set: &ImportSet) -> bool {
    set.has_secrets()
}

fn read_bundle(source: &str) -> Result<ImportBundle, String> {
    match source {
        TERMIUS => termius::import_local().map_err(|e| match e {
            TermiusError::NotInstalled => "no Termius data was found for this user".into(),
            other => other.to_string(),
        }),
        PUTTY => sessions_bundle(putty::PUTTY_REGISTRY_PATH, Source::Putty),
        KITTY => sessions_bundle(putty::KITTY_REGISTRY_PATH, Source::Kitty),
        OPENSSH => ssh_config::read_default()
            .map(sessions_result)
            .map_err(|e| e.to_string()),
        other => Err(format!("unknown import source: {other}")),
    }
}

/// PuTTY, KiTTY and ssh_config produce hosts with an inline username and
/// key-file path, and nothing else — no shared identities, no vault keys.
fn sessions_bundle(path: &str, source: Source) -> Result<ImportBundle, String> {
    let result = putty::read_sessions(path, source).map_err(|e| e.to_string())?;
    Ok(sessions_result(result))
}

fn sessions_result(result: ImportResult) -> ImportBundle {
    ImportBundle {
        hosts: result.hosts,
        skipped: result.skipped,
        ..Default::default()
    }
}

/// Map the importer's bundle onto the store's input, collecting the reasons
/// anything was left out. Handles both shapes: Termius hosts that point at a
/// shared identity by index, and PuTTY hosts that carry their username and key
/// file inline (a per-host identity is synthesised for those).
fn to_import_set(bundle: ImportBundle) -> (ImportSet, Vec<String>) {
    let mut skipped: Vec<String> = bundle
        .skipped
        .into_iter()
        .map(|(what, why)| format!("{what}: {why}"))
        .collect();

    let keys = bundle
        .keys
        .into_iter()
        .map(|key| KeyInput {
            label: key.label,
            key_type: key_type_label(key.format),
            public_key: key.public_key,
            private_key: key.private_key,
            passphrase: key.passphrase,
        })
        .collect();

    // Shared identities come first and keep their index; per-host ones append.
    let mut identities: Vec<IdentityInput> = bundle
        .identities
        .into_iter()
        .map(|identity| IdentityInput {
            label: identity.label,
            username: identity.username,
            password: identity.password,
            key: identity.key,
            key_path: None,
        })
        .collect();

    let hosts = bundle
        .hosts
        .into_iter()
        .map(|host| {
            let identity = match host.identity {
                Some(index) => Some(index),
                None if host.username.is_some() || host.key_path.is_some() => {
                    identities.push(IdentityInput {
                        username: host.username,
                        key_path: host.key_path,
                        ..Default::default()
                    });
                    Some(identities.len() - 1)
                }
                None => None,
            };
            HostInput {
                name: host.name,
                address: host.address,
                port: host.port,
                group_path: host.group_path,
                identity,
                workspace: Default::default(),
                position: None,
            }
        })
        .collect();

    let known_hosts = bundle
        .known_hosts
        .into_iter()
        .filter_map(|known| {
            let line = format!("{} {}", known.algorithm, known.key);
            match public_key_fingerprint(&line) {
                Some((algorithm, fingerprint)) => Some(KnownHostInput {
                    address: known.host,
                    port: known.port,
                    algorithm,
                    public_key: line,
                    fingerprint,
                }),
                None => {
                    skipped.push(format!(
                        "known host {}:{}: its key could not be read",
                        known.host, known.port
                    ));
                    None
                }
            }
        })
        .collect();

    let snippets = bundle
        .snippets
        .into_iter()
        .map(|snippet| SnippetInput {
            label: snippet.label,
            body: snippet.script,
            group_path: snippet.group,
        })
        .collect();

    (
        ImportSet {
            groups: Vec::new(),
            hosts,
            identities,
            keys,
            known_hosts,
            snippets,
        },
        skipped,
    )
}

fn key_type_label(format: uwussh_import::KeyFormat) -> String {
    use uwussh_import::KeyFormat::*;
    match format {
        OpenSsh => "openssh",
        Pem => "pem",
        Ppk => "ppk",
    }
    .to_string()
}
