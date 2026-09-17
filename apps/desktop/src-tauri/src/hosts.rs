//! The host list, connecting to a host, and deciding about host keys.
//!
//! Connecting is a conversation rather than a call. Most errors from
//! [`connect_host`] are the next question for the user — trust this key? what
//! is the password? — and the webview asks it and calls again with the answer.
//! Secrets only ever travel inbound, for the single call that uses them.
//!
//! One exception, on purpose: the password a terminal logged in with stays in
//! Rust's memory while that terminal is open, so it can be typed again when
//! `sudo` asks (see [`type_session_password`]). It never goes back to the page,
//! and it is wiped when the session closes.

use crate::{err, AppState, ChannelSink, CommandResult};
use serde::Serialize;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;
use uwussh_core::{SessionId, SshAuth, SshError, SshTarget};
use uwussh_store::{
    CredentialSource, GroupRecord, HostDraft, HostRecord, PasswordChange, Revealed, RevealedKey,
    SecretText, StoreError, Workspace,
};
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
    /// Storing a password needs the vault open (or created) first.
    VaultLocked,
    Error {
        message: String,
    },
}

impl From<StoreError> for SaveFailure {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Invalid { field, problem } => Self::Invalid { field, problem },
            StoreError::VaultLocked => Self::VaultLocked,
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
    let forget = matches!(draft.password, PasswordChange::Forget);
    let saved = state.store.save_host(draft)?;
    if forget {
        forget_typed_passwords(&state, saved.id);
    }
    Ok(saved)
}

#[tauri::command]
pub(crate) fn delete_host(state: State<'_, AppState>, id: Uuid) -> CommandResult<()> {
    state.store.delete_host(id).map_err(err)?;
    forget_typed_passwords(&state, id);
    Ok(())
}

/// A forgotten password is forgotten by open terminals too: the password
/// helper stops offering the copy their login kept.
fn forget_typed_passwords(state: &AppState, host: Uuid) {
    let sessions: Vec<SessionId> = state
        .session_hosts
        .lock()
        .iter()
        .filter(|(_, h)| **h == host)
        .map(|(session, _)| *session)
        .collect();
    let mut passwords = state.session_passwords.lock();
    for session in sessions {
        passwords.remove(&session);
    }
}

/// Store a password for a host (`Some`), or forget the stored one (`None`).
#[tauri::command]
pub(crate) fn set_host_password(
    state: State<'_, AppState>,
    id: Uuid,
    password: Option<String>,
) -> Result<HostRecord, SaveFailure> {
    let change = match password {
        Some(value) => PasswordChange::Set {
            value: SecretText::new(value),
        },
        None => PasswordChange::Forget,
    };
    let forget = matches!(change, PasswordChange::Forget);
    let saved = state.store.set_host_password(id, change)?;
    if forget {
        forget_typed_passwords(&state, id);
    }
    Ok(saved)
}

// ── Groups and order ────────────────────────────────────────────────────────

#[tauri::command]
pub(crate) fn list_groups(state: State<'_, AppState>) -> CommandResult<Vec<GroupRecord>> {
    state.store.list_groups().map_err(err)
}

#[tauri::command]
pub(crate) fn create_group(
    state: State<'_, AppState>,
    workspace: Workspace,
    name: String,
) -> Result<GroupRecord, SaveFailure> {
    Ok(state.store.create_group(workspace, &name)?)
}

#[tauri::command]
pub(crate) fn rename_group(
    state: State<'_, AppState>,
    workspace: Workspace,
    from: String,
    to: String,
) -> Result<(), SaveFailure> {
    Ok(state.store.rename_group(workspace, &from, &to)?)
}

#[tauri::command]
pub(crate) fn delete_group(
    state: State<'_, AppState>,
    workspace: Workspace,
    name: String,
) -> CommandResult<()> {
    state.store.delete_group(workspace, &name).map_err(err)
}

#[tauri::command]
pub(crate) fn move_group(
    state: State<'_, AppState>,
    workspace: Workspace,
    name: String,
    to: Workspace,
    before: Option<String>,
) -> CommandResult<()> {
    state
        .store
        .move_group(workspace, &name, to, before.as_deref())
        .map_err(err)
}

#[tauri::command]
pub(crate) fn move_host(
    state: State<'_, AppState>,
    id: Uuid,
    to: Workspace,
    group: Option<String>,
    before: Option<Uuid>,
) -> CommandResult<HostRecord> {
    state
        .store
        .move_host(id, to, group.as_deref(), before)
        .map_err(err)
}

// ── Connecting ──────────────────────────────────────────────────────────────

/// Everything that can come back from a connection attempt. SSH and SFTP
/// errors keep their own `kind` tag; everything else is `internal` or
/// `vault-locked`.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum ConnectFailure {
    Ssh(SshError),
    Files(uwussh_core::SftpError),
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

pub(crate) fn internal(message: impl std::fmt::Display) -> ConnectFailure {
    ConnectFailure::Internal {
        kind: "internal",
        message: message.to_string(),
    }
}

/// A locked vault is a question for the user; any other failure while revealing
/// a secret is internal.
pub(crate) fn from_vault<T>(result: Result<T, StoreError>) -> Result<T, ConnectFailure> {
    result.map_err(|error| match error {
        StoreError::VaultLocked => ConnectFailure::VaultLocked {
            kind: "vault-locked",
        },
        other => internal(other),
    })
}

/// A stored secret is text (a password, a PEM key). Turn the revealed bytes
/// into the zeroizing string the engine wants.
pub(crate) fn as_secret(bytes: Revealed) -> Result<Zeroizing<String>, ConnectFailure> {
    String::from_utf8(bytes.to_vec())
        .map(Zeroizing::new)
        .map_err(|_| internal("a stored secret is not valid text"))
}

/// How a connection attempt is named: the tab it belongs to. Short and plain,
/// since it only keys a map.
pub(crate) fn check_attempt(attempt: &str) -> Result<(), ConnectFailure> {
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

/// How long a key the server presented can be trusted from the dialog. Long
/// enough to compare fingerprints calmly, short enough that a declined key
/// doesn't stay trustable for the rest of the session.
const PRESENTED_KEY_TTL: std::time::Duration = std::time::Duration::from_secs(10 * 60);

fn key_for(address: &str, port: u16) -> (String, u16) {
    (address.trim().to_ascii_lowercase(), port)
}

/// A host, its target for the engine, and the password its login used (if it
/// used one), so it can be kept for `sudo`.
pub(crate) struct Prepared {
    pub host: HostRecord,
    pub target: SshTarget,
    pub login_password: Option<Zeroizing<String>>,
}

/// Look up a host and resolve how it logs in. A vault secret is revealed here,
/// just before connecting; the host key is still checked first inside the
/// engine, so nothing is sent to an unverified server.
pub(crate) fn prepare(
    state: &AppState,
    id: Uuid,
    secret: Option<Zeroizing<String>>,
) -> Result<Prepared, ConnectFailure> {
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

    let (auth, login_password) = match from_vault(state.store.host_credential_source(id))? {
        CredentialSource::AskPassword => (SshAuth::Password(secret.clone()), secret),
        CredentialSource::KeyFile { path } => (
            SshAuth::Key {
                path,
                passphrase: secret,
            },
            None,
        ),
        CredentialSource::VaultPassword => {
            let password = as_secret(from_vault(state.store.reveal_host_password(id))?)?;
            (SshAuth::Password(Some(password.clone())), Some(password))
        }
        CredentialSource::VaultKey => {
            let RevealedKey {
                private_key,
                passphrase,
            } = from_vault(state.store.reveal_host_key(id))?;
            (
                SshAuth::KeyContents {
                    private_key: as_secret(private_key)?,
                    passphrase: passphrase.map(as_secret).transpose()?,
                },
                None,
            )
        }
    };

    let target = SshTarget {
        address: host.address.clone(),
        port: host.port,
        username: host.username.clone(),
        auth,
        trusted_fingerprint,
    };
    Ok(Prepared {
        host,
        target,
        login_password,
    })
}

/// Remember what a failed attempt's server presented, so the dialog can trust
/// exactly that key and nothing else.
pub(crate) fn note_presented_key(state: &AppState, host: &HostRecord, error: &SshError) {
    if let SshError::UnknownHostKey { observed } | SshError::HostKeyChanged { observed, .. } = error
    {
        state.presented_keys.lock().insert(
            key_for(&host.address, host.port),
            (observed.clone(), std::time::Instant::now()),
        );
    }
}

/// What the host list learns after a terminal opened: the system it runs.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct HostOs {
    id: Uuid,
    os: &'static str,
}

// A Tauri command takes its arguments flat, as the page sends them.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub(crate) async fn connect_host(
    app: AppHandle,
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
    let Prepared {
        host,
        target,
        login_password,
    } = prepare(&state, id, secret)?;

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
            state.session_hosts.lock().insert(session, host.id);
            if let Some(password) = login_password {
                state.session_passwords.lock().insert(session, password);
            }
            // Once per host: the probe is a command, and a key with a forced
            // command in authorized_keys would run that command again.
            let probe = if host.os.is_none() {
                state.sessions.os_probe(session)
            } else {
                None
            };
            if let Some(probe) = probe {
                let known = host.os.clone();
                tauri::async_runtime::spawn(async move {
                    let Some(os) = probe.await else { return };
                    if known.as_deref() == Some(os) {
                        return;
                    }
                    let state = app.state::<AppState>();
                    if state.store.set_host_os(host.id, Some(os)).is_ok() {
                        let _ = app.emit("host:os", HostOs { id: host.id, os });
                    }
                });
            }
            Ok(session)
        }
        Err(error) => {
            note_presented_key(&state, &host, &error);
            Err(ConnectFailure::Ssh(error))
        }
    }
}

/// Trust the key the server at `address:port` just presented.
///
/// Replacing a key that was already trusted additionally needs `replace`, the
/// user's explicit "accept the new key" from the warning — never a side effect
/// of trusting a first key.
#[tauri::command]
pub(crate) fn trust_host_key(
    state: State<'_, AppState>,
    address: String,
    port: u16,
    fingerprint: String,
    replace: bool,
) -> CommandResult<()> {
    let slot = key_for(&address, port);

    let presented = state
        .presented_keys
        .lock()
        .get(&slot)
        .filter(|(_, seen)| seen.elapsed() < PRESENTED_KEY_TTL)
        .map(|(observed, _)| observed.clone())
        .filter(|observed| observed.fingerprint == fingerprint)
        .ok_or("this key was not presented by the server in the last connection attempt")?;

    let replacing = state
        .store
        .known_host(&address, port)
        .map_err(err)?
        .is_some_and(|known| known.fingerprint != presented.fingerprint);

    if replacing && !replace {
        return Err("replacing a trusted host key needs the user's confirmation".into());
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
// Async on purpose: closing spawns a task on the runtime, and sync commands
// run on the main thread, outside it.
#[tauri::command]
pub(crate) async fn cancel_connect(state: State<'_, AppState>, attempt: String) -> Result<(), ()> {
    state.sessions.abandon_ssh(&attempt);
    Ok(())
}

// ── Typing the password for sudo ────────────────────────────────────────────

/// The password a terminal can type for its user: the one it logged in with,
/// or else the one stored for its host.
fn session_password(
    state: &AppState,
    session: SessionId,
) -> Result<Option<Zeroizing<String>>, ConnectFailure> {
    if let Some(password) = state.session_passwords.lock().get(&session) {
        return Ok(Some(password.clone()));
    }
    let Some(host) = state.session_hosts.lock().get(&session).copied() else {
        return Ok(None);
    };
    let has = state
        .store
        .get_host(host)
        .map_err(internal)?
        .is_some_and(|h| h.has_password);
    if !has {
        return Ok(None);
    }
    Ok(Some(as_secret(from_vault(
        state.store.reveal_host_password(host),
    )?)?))
}

/// Whether [`type_session_password`] has something to type for this terminal.
#[tauri::command]
pub(crate) fn session_can_type_password(state: State<'_, AppState>, id: SessionId) -> bool {
    if state.session_passwords.lock().contains_key(&id) {
        return true;
    }
    let Some(host) = state.session_hosts.lock().get(&id).copied() else {
        return false;
    };
    state
        .store
        .get_host(host)
        .ok()
        .flatten()
        .is_some_and(|h| h.has_password)
}

/// Type the terminal's password, followed by Enter, into the terminal — after
/// the user said yes to the prompt the page spotted. The page never sees it.
#[tauri::command]
pub(crate) fn type_session_password(
    state: State<'_, AppState>,
    id: SessionId,
) -> Result<(), ConnectFailure> {
    let password = session_password(&state, id)?
        .ok_or_else(|| internal("there is no password for this terminal"))?;
    let mut line = Zeroizing::new(Vec::with_capacity(password.len() + 1));
    line.extend_from_slice(password.as_bytes());
    line.push(b'\r');
    state.sessions.write(id, &line).map_err(internal)
}

/// Forget what belonged to a session that closed.
pub(crate) fn forget_session(state: &AppState, id: SessionId) {
    state.session_passwords.lock().remove(&id);
    state.session_hosts.lock().remove(&id);
}
