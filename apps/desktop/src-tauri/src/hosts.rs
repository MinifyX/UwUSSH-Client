//! The host list, connecting to a host, and deciding about host keys.
//!
//! Connecting is a conversation rather than a call. Most errors from
//! [`connect_host`] are the next question for the user — trust this key? what
//! is the password? — and the webview asks it and calls again with the answer.
//! Secrets only ever travel inbound, for the single call that uses them, and
//! are wiped after.

use crate::{err, AppState, ChannelSink, CommandResult};
use serde::Serialize;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::State;
use uuid::Uuid;
use uwussh_core::{SessionId, SshAuth, SshError, SshTarget};
use uwussh_store::{CredentialSource, HostDraft, HostRecord, Revealed, RevealedKey, StoreError};
use zeroize::Zeroizing;

#[tauri::command]
pub(crate) fn list_hosts(state: State<'_, AppState>) -> CommandResult<Vec<HostRecord>> {
    state.store.list_hosts().map_err(err)
}

/// Validation failures carry the field, so the form can mark the right input.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum SaveFailure {
    Invalid {
        field: &'static str,
        problem: &'static str,
    },
    Error {
        message: String,
    },
}

impl From<StoreError> for SaveFailure {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Invalid { field, problem } => Self::Invalid { field, problem },
            other => Self::Error {
                message: other.to_string(),
            },
        }
    }
}

#[tauri::command]
pub(crate) fn save_host(
    state: State<'_, AppState>,
    draft: HostDraft,
) -> Result<HostRecord, SaveFailure> {
    Ok(state.store.save_host(draft)?)
}

#[tauri::command]
pub(crate) fn delete_host(state: State<'_, AppState>, id: Uuid) -> CommandResult<()> {
    state.store.delete_host(id).map_err(err)
}

/// Everything that can come back from a connection attempt. SSH errors keep
/// their own `kind` tag; everything else is `internal` or `vault-locked`.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum ConnectFailure {
    Ssh(SshError),
    /// The host logs in with a vault secret, but the vault is locked. The UI
    /// asks for the master password and retries.
    VaultLocked {
        kind: &'static str,
    },
    Internal {
        kind: &'static str,
        message: String,
    },
}

fn internal(message: impl std::fmt::Display) -> ConnectFailure {
    ConnectFailure::Internal {
        kind: "internal",
        message: message.to_string(),
    }
}

/// A locked vault is a question for the user; any other failure while revealing
/// a secret is internal.
fn from_vault<T>(result: Result<T, StoreError>) -> Result<T, ConnectFailure> {
    result.map_err(|error| match error {
        StoreError::VaultLocked => ConnectFailure::VaultLocked {
            kind: "vault-locked",
        },
        other => internal(other),
    })
}

/// A stored secret is text (a password, a PEM key). Turn the revealed bytes
/// into the zeroizing string the engine wants.
fn as_secret(bytes: Revealed) -> Result<Zeroizing<String>, ConnectFailure> {
    String::from_utf8(bytes.to_vec())
        .map(Zeroizing::new)
        .map_err(|_| internal("a stored secret is not valid text"))
}

/// How a connection attempt is named: the tab it belongs to. Short and plain,
/// since it only keys a map.
fn check_attempt(attempt: &str) -> Result<(), ConnectFailure> {
    let plain = !attempt.is_empty()
        && attempt.len() <= 64
        && attempt
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if plain {
        Ok(())
    } else {
        Err(internal("invalid connection attempt"))
    }
}

fn key_for(address: &str, port: u16) -> (String, u16) {
    (address.trim().to_ascii_lowercase(), port)
}

#[tauri::command]
pub(crate) async fn connect_host(
    state: State<'_, AppState>,
    id: Uuid,
    attempt: String,
    cols: u16,
    rows: u16,
    secret: Option<String>,
    on_data: Channel<InvokeResponseBody>,
) -> Result<SessionId, ConnectFailure> {
    let secret = secret.map(Zeroizing::new);
    check_attempt(&attempt)?;

    let host = state
        .store
        .get_host(id)
        .map_err(internal)?
        .ok_or_else(|| internal("this host no longer exists"))?;

    let trusted_fingerprint = state
        .store
        .known_host(&host.address, host.port)
        .map_err(internal)?
        .map(|known| known.fingerprint);

    // Where this host's login comes from decides what we hand the engine. A
    // vault secret is revealed here, just before connecting; the host key is
    // still checked first inside the engine, so nothing is sent to an
    // unverified server.
    let auth = match from_vault(state.store.host_credential_source(id))? {
        CredentialSource::AskPassword => SshAuth::Password(secret),
        CredentialSource::KeyFile { path } => SshAuth::Key {
            path,
            passphrase: secret,
        },
        CredentialSource::VaultPassword => SshAuth::Password(Some(as_secret(from_vault(
            state.store.reveal_host_password(id),
        )?)?)),
        CredentialSource::VaultKey => {
            let RevealedKey {
                private_key,
                passphrase,
            } = from_vault(state.store.reveal_host_key(id))?;
            SshAuth::KeyContents {
                private_key: as_secret(private_key)?,
                passphrase: passphrase.map(as_secret).transpose()?,
            }
        }
    };

    let target = SshTarget {
        address: host.address.clone(),
        port: host.port,
        username: host.username.clone(),
        auth,
        trusted_fingerprint,
    };

    match state
        .sessions
        .spawn_ssh(
            &attempt,
            target,
            cols,
            rows,
            ChannelSink { channel: on_data },
        )
        .await
    {
        Ok(session) => {
            if let Err(error) = state.store.mark_connected(host.id) {
                tracing::warn!(%error, "could not record the connection time");
            }
            Ok(session)
        }
        Err(error) => {
            if let SshError::UnknownHostKey { observed }
            | SshError::HostKeyChanged { observed, .. } = &error
            {
                state
                    .presented_keys
                    .lock()
                    .insert(key_for(&host.address, host.port), observed.clone());
            }
            Err(ConnectFailure::Ssh(error))
        }
    }
}

/// Trust the key the server at `address:port` just presented.
///
/// Replacing a key that was already trusted for this address additionally
/// needs `confirmation` to be the address itself, typed out. That is the
/// deliberate step between a changed host key and a click on the wrong button.
#[tauri::command]
pub(crate) fn trust_host_key(
    state: State<'_, AppState>,
    address: String,
    port: u16,
    fingerprint: String,
    confirmation: Option<String>,
) -> CommandResult<()> {
    let slot = key_for(&address, port);

    let presented = state
        .presented_keys
        .lock()
        .get(&slot)
        .cloned()
        .filter(|observed| observed.fingerprint == fingerprint)
        .ok_or("this key was not presented by the server in the last connection attempt")?;

    let replacing = state
        .store
        .known_host(&address, port)
        .map_err(err)?
        .is_some_and(|known| known.fingerprint != presented.fingerprint);

    if replacing {
        let typed = confirmation.unwrap_or_default();
        if !typed.trim().eq_ignore_ascii_case(address.trim()) {
            return Err("replacing a trusted host key needs the address typed out".into());
        }
    }

    state
        .store
        .trust_host_key(
            &address,
            port,
            &presented.algorithm,
            &presented.fingerprint,
            &presented.public_key,
        )
        .map_err(err)?;
    state.presented_keys.lock().remove(&slot);
    Ok(())
}

/// The user closed a password or passphrase prompt instead of answering it, or
/// closed the tab: close the verified connection that was waiting for the
/// answer, rather than leaving it for the server to time out.
#[tauri::command]
pub(crate) fn cancel_connect(state: State<'_, AppState>, attempt: String) {
    state.sessions.abandon_ssh(&attempt);
}
