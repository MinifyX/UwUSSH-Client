//! Keys in the vault: listing, importing a key file, exporting one, and
//! storing a key UwUKeygen just made. A key from the vault only ever leaves
//! through a save dialog.

use crate::dialogs::{self, Filter};
use crate::hosts::{as_secret, from_vault, ConnectFailure};
use crate::keygen::{save_key_file, Generated, KeygenFailure};
use crate::{err, AppState, CommandResult};
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, State};
use uuid::Uuid;
use uwussh_keygen::{KeyInfo, PrivateFormat};
use uwussh_store::{KeyDraft, KeyRecord, SecretText, StoreError};
use zeroize::Zeroizing;

/// A key file picked for import, waiting for its passphrase.
#[derive(Default)]
pub(crate) struct Picked(Mutex<Option<PickedFile>>);

struct PickedFile {
    token: String,
    name: String,
    text: Zeroizing<String>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum KeyFailure {
    VaultLocked,
    PassphraseRequired,
    PassphraseWrong,
    InUse { hosts: usize },
    Error { message: String },
}

impl From<StoreError> for KeyFailure {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::VaultLocked => Self::VaultLocked,
            StoreError::KeyInUse { hosts } => Self::InUse { hosts },
            other => Self::Error {
                message: other.to_string(),
            },
        }
    }
}

impl From<uwussh_keygen::KeygenError> for KeyFailure {
    fn from(error: uwussh_keygen::KeygenError) -> Self {
        match error {
            uwussh_keygen::KeygenError::PassphraseRequired => Self::PassphraseRequired,
            uwussh_keygen::KeygenError::PassphraseWrong => Self::PassphraseWrong,
            other => Self::Error {
                message: other.to_string(),
            },
        }
    }
}

impl From<KeygenFailure> for KeyFailure {
    fn from(failure: KeygenFailure) -> Self {
        let KeygenFailure::Error { message } = failure;
        Self::Error { message }
    }
}

impl From<ConnectFailure> for KeyFailure {
    fn from(failure: ConnectFailure) -> Self {
        match failure {
            ConnectFailure::VaultLocked { .. } => Self::VaultLocked,
            other => Self::Error {
                message: format!("{other:?}"),
            },
        }
    }
}

fn failure(message: impl std::fmt::Display) -> KeyFailure {
    KeyFailure::Error {
        message: message.to_string(),
    }
}

/// Opening an encrypted key runs its format's key derivation, which takes a
/// moment on purpose; the window keeps drawing meanwhile.
async fn off_the_main_thread<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, KeyFailure> + Send + 'static,
) -> Result<T, KeyFailure> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(failure)?
}

#[tauri::command]
pub(crate) fn list_keys(state: State<'_, AppState>) -> CommandResult<Vec<KeyRecord>> {
    state.store.list_keys().map_err(err)
}

#[tauri::command]
pub(crate) fn rename_key(
    state: State<'_, AppState>,
    id: Uuid,
    label: String,
) -> Result<(), KeyFailure> {
    Ok(state.store.rename_key(id, &label)?)
}

#[tauri::command]
pub(crate) fn delete_key(state: State<'_, AppState>, id: Uuid) -> Result<(), KeyFailure> {
    Ok(state.store.delete_key(id)?)
}

/// The public key line of a vault key, for `authorized_keys`. Needs nothing
/// unlocked when the line was stored; otherwise it is worked out from the
/// private half.
#[tauri::command]
pub(crate) async fn key_public_line(
    state: State<'_, AppState>,
    id: Uuid,
) -> Result<String, KeyFailure> {
    let key = state
        .store
        .get_key(id)?
        .ok_or_else(|| failure("this key no longer exists"))?;
    if !key.public_key.trim().is_empty() {
        return Ok(key.public_key);
    }
    let revealed = from_vault(state.store.reveal_key(id))?;
    let private = as_secret(revealed.private_key)?;
    let passphrase = revealed.passphrase.map(as_secret).transpose()?;
    off_the_main_thread(move || {
        let loaded = uwussh_keygen::load(&private, passphrase.as_ref().map(|p| p.as_str()))?;
        Ok(loaded.info().public_openssh)
    })
    .await
}

const KEY_FILTER: Filter<'static> = Filter {
    name: "Private Keys",
    extensions: &["ppk", "pem", "key", ""],
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PickedKey {
    token: String,
    file_name: String,
    /// Protected by a passphrase: [`import_picked_key`] needs it.
    encrypted: bool,
    /// What the key is, when it could be read without a passphrase.
    info: Option<KeyInfo>,
}

/// Pick a private key file (OpenSSH, PEM or PuTTY) to put into the vault. The
/// file is read now and kept in memory, so asking for its passphrase doesn't
/// mean picking it again.
#[tauri::command]
pub(crate) async fn pick_key_file(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<PickedKey>, KeyFailure> {
    let Some(path) = dialogs::open(&app, "Private Key importieren", KEY_FILTER).await else {
        return Ok(None);
    };
    if uwussh_core::ssh::is_network_path(&path.display().to_string()) {
        return Err(failure("keys on network shares are not read"));
    }
    let metadata = std::fs::metadata(&path).map_err(failure)?;
    if metadata.len() > 64 * 1024 {
        return Err(failure("this file is too large to be a private key"));
    }
    let text = Zeroizing::new(std::fs::read_to_string(&path).map_err(failure)?);
    let (encrypted, info) = match uwussh_keygen::load(&text, None) {
        Ok(key) => (false, Some(key.info())),
        Err(uwussh_keygen::KeygenError::PassphraseRequired) => (true, None),
        Err(other) => return Err(other.into()),
    };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let token = Uuid::now_v7().to_string();
    *state.picked_key.0.lock() = Some(PickedFile {
        token: token.clone(),
        name: name.clone(),
        text,
    });
    Ok(Some(PickedKey {
        token,
        file_name: name,
        encrypted,
        info,
    }))
}

/// The picked key file is no longer wanted: the dialog closed.
#[tauri::command]
pub(crate) fn forget_picked_key(state: State<'_, AppState>) {
    *state.picked_key.0.lock() = None;
}

/// Put the picked key file into the vault, with its passphrase when it has one
/// (stored with it, so connecting never asks).
#[tauri::command]
pub(crate) async fn import_picked_key(
    state: State<'_, AppState>,
    token: String,
    label: Option<String>,
    passphrase: Option<String>,
) -> Result<KeyRecord, KeyFailure> {
    if state.store.vault_status()? != uwussh_store::VaultStatus::Unlocked {
        return Err(KeyFailure::VaultLocked);
    }
    let (name, text) = match &*state.picked_key.0.lock() {
        Some(picked) if picked.token == token => (picked.name.clone(), picked.text.clone()),
        _ => return Err(failure("pick the key file again")),
    };
    let passphrase = passphrase.filter(|p| !p.is_empty()).map(Zeroizing::new);
    let (info, text, passphrase) = off_the_main_thread(move || {
        let loaded = uwussh_keygen::load(&text, passphrase.as_ref().map(|p| p.as_str()))?;
        Ok((loaded.info(), text, passphrase))
    })
    .await?;
    let label = label
        .filter(|l| !l.trim().is_empty())
        .or_else(|| {
            std::path::Path::new(&name)
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| info.label.clone());
    let record = state.store.add_key(KeyDraft {
        label,
        key_type: info.algorithm.clone(),
        public_key: info.public_openssh.clone(),
        private_key: SecretText::from(text),
        passphrase: passphrase.map(SecretText::from),
    })?;
    *state.picked_key.0.lock() = None;
    Ok(record)
}

/// Save a vault key as a file, in the format asked for.
#[tauri::command]
pub(crate) async fn export_key_file(
    app: AppHandle,
    state: State<'_, AppState>,
    id: Uuid,
    format: PrivateFormat,
    passphrase: Option<String>,
) -> Result<Option<String>, KeyFailure> {
    let key = state
        .store
        .get_key(id)?
        .ok_or_else(|| failure("this key no longer exists"))?;
    let revealed = from_vault(state.store.reveal_key(id))?;
    let private = as_secret(revealed.private_key)?;
    let stored_passphrase = revealed.passphrase.map(as_secret).transpose()?;
    let passphrase = passphrase.filter(|p| !p.is_empty()).map(Zeroizing::new);
    let encoded = off_the_main_thread(move || {
        let loaded = uwussh_keygen::load(&private, stored_passphrase.as_ref().map(|p| p.as_str()))?;
        Ok(loaded.encode(format, passphrase.as_ref().map(|p| p.as_str()))?)
    })
    .await?;
    Ok(save_key_file(&app, &key.label, format, &encoded).await?)
}

// ── UwUKeygen, into the vault ──────────────────────────────────────────────

/// Put a generated key into the vault, as OpenSSH, sealed with its passphrase
/// when it has one (stored with it, so connecting never has to ask). The rest
/// of UwUKeygen lives in `keygen`, shared with the standalone app.
#[tauri::command]
pub(crate) async fn keygen_store(
    state: State<'_, AppState>,
    generated: State<'_, Generated>,
    token: String,
    label: String,
    passphrase: Option<String>,
) -> Result<KeyRecord, KeyFailure> {
    let passphrase = passphrase.filter(|p| !p.is_empty()).map(Zeroizing::new);
    let info = generated.with(&token, |key| Ok(key.info()))?;
    let encoded = crate::keygen::encode(
        &generated,
        &token,
        PrivateFormat::OpenSsh,
        passphrase.as_ref().map(|p| p.to_string()),
    )
    .await?;
    let record = state.store.add_key(KeyDraft {
        label: if label.trim().is_empty() {
            info.label.clone()
        } else {
            label
        },
        key_type: info.algorithm,
        public_key: info.public_openssh,
        private_key: SecretText::from(encoded),
        passphrase: passphrase.map(SecretText::from),
    })?;
    generated.forget(&token);
    Ok(record)
}
