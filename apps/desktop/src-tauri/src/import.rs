//! The vault and importing another client's setup.
//!
//! Two steps the UI drives in order. First the vault: importing brings
//! passwords and keys, and they need somewhere sealed to live, so a locked or
//! absent vault has to be dealt with before anything is written. Then the
//! import: [`scan_termius`] reads the local Termius install and reports what it
//! found — counts only, no secrets — for a preview, and [`import_termius`]
//! writes it under the unlocked vault.

use crate::{err, AppState, CommandResult};
use serde::Serialize;
use tauri::State;
use uwussh_core::public_key_fingerprint;
use uwussh_import::termius::{self, TermiusError};
use uwussh_import::ImportBundle;
use uwussh_store::{
    HostInput, IdentityInput, ImportSet, KeyInput, KnownHostInput, SnippetInput, VaultStatus,
};
use zeroize::Zeroizing;

// ── Vault ────────────────────────────────────────────────────────────────

#[tauri::command]
pub(crate) fn vault_status(state: State<'_, AppState>) -> CommandResult<VaultStatus> {
    state.store.vault_status().map_err(err)
}

#[tauri::command]
pub(crate) fn create_vault(state: State<'_, AppState>, password: String) -> CommandResult<()> {
    let password = Zeroizing::new(password);
    if password.trim().is_empty() {
        return Err("the master password cannot be empty".into());
    }
    state.store.create_vault(password.as_bytes()).map_err(err)
}

#[tauri::command]
pub(crate) fn unlock_vault(state: State<'_, AppState>, password: String) -> CommandResult<()> {
    let password = Zeroizing::new(password);
    state.store.unlock_vault(password.as_bytes()).map_err(err)
}

#[tauri::command]
pub(crate) fn lock_vault(state: State<'_, AppState>) {
    state.store.lock_vault();
}

// ── Termius ──────────────────────────────────────────────────────────────

/// What a Termius import would bring, in counts. Contains no host names,
/// addresses or secrets, so it is safe to hand to the webview for a preview.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImportSummary {
    pub hosts: usize,
    pub identities: usize,
    pub keys: usize,
    pub known_hosts: usize,
    pub snippets: usize,
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

#[tauri::command]
pub(crate) fn termius_available() -> bool {
    termius::is_installed()
}

#[tauri::command]
pub(crate) fn scan_termius() -> Result<ImportSummary, String> {
    let bundle = read_termius()?;
    let (set, mut skipped) = to_import_set(bundle);
    skipped.sort();
    Ok(ImportSummary {
        hosts: set.hosts.len(),
        identities: set.identities.len(),
        keys: set.keys.len(),
        known_hosts: set.known_hosts.len(),
        snippets: set.snippets.len(),
        skipped,
    })
}

#[tauri::command]
pub(crate) fn import_termius(state: State<'_, AppState>) -> Result<ImportReport, String> {
    if state.store.vault_status().map_err(err)? != VaultStatus::Unlocked {
        return Err("unlock the vault before importing".into());
    }
    let bundle = read_termius()?;
    let (set, mut skipped) = to_import_set(bundle);
    skipped.sort();

    let outcome = state.store.import(set).map_err(err)?;
    Ok(ImportReport {
        hosts_added: outcome.hosts_added,
        hosts_skipped: outcome.hosts_skipped,
        identities_added: outcome.identities_added,
        keys_added: outcome.keys_added,
        known_hosts_added: outcome.known_hosts_added,
        snippets_added: outcome.snippets_added,
        skipped,
    })
}

fn read_termius() -> Result<ImportBundle, String> {
    termius::import_local().map_err(|e| match e {
        TermiusError::NotInstalled => "no Termius data was found for this user".into(),
        other => other.to_string(),
    })
}

/// Map the importer's bundle onto the store's input, and collect the reasons
/// anything was left out. The two crates deliberately do not share these types,
/// so this is the one place they meet.
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

    let identities = bundle
        .identities
        .into_iter()
        .map(|identity| IdentityInput {
            label: identity.label,
            username: identity.username,
            password: identity.password,
            key: identity.key,
        })
        .collect();

    let hosts = bundle
        .hosts
        .into_iter()
        .map(|host| HostInput {
            name: host.name,
            address: host.address,
            port: host.port,
            group_path: host.group_path,
            identity: host.identity,
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
