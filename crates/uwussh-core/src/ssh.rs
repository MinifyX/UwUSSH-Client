//! SSH sessions, on the data path M0 measured.
//!
//! Connecting happens in the same order as in PuTTY and OpenSSH, and that order
//! is the security model:
//!
//! 1. **Connect and check the host key.** [`SshConnection::open`] runs the key
//!    exchange and compares the server's key with the trusted fingerprint. An
//!    unknown key ends with [`SshError::UnknownHostKey`], a different one with
//!    [`SshError::HostKeyChanged`] — before anything about the user is sent,
//!    and before the user is asked for anything.
//! 2. **Only then ask for secrets.** [`SshConnection::authenticate`] reports a
//!    missing password or passphrase without contacting the server, and the
//!    verified connection stays open, so the answer — or a second try after a
//!    typo — goes over the same connection rather than a new one.
//! 3. **Open the shell** on the authenticated connection.
//!
//! An earlier version asked for the password before connecting, to save a
//! round trip. Security held — nothing was sent to a changed key — but the user
//! typed the password first and learned about the possible man in the middle
//! second. The end-to-end UI test caught it; this order is the fix.
//!
//! Backpressure reaches the server: when the renderer falls behind, the batcher
//! pauses, the reader stops pulling from russh, russh stops reading the socket
//! (it awaits on a full channel queue rather than dropping), and TCP makes the
//! remote program wait.

use crate::flow::FlowControl;
use crate::metrics::{Metrics, MetricsSnapshot};
use crate::os;
use crate::sftp::{Elevation, SftpClient, SftpError};
use crate::stream::{self, FrameSink};
use crate::{CoreError, Result};
use parking_lot::Mutex;
use russh::client::{self, Handle};
use russh::keys::{
    self, Algorithm, HashAlg, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate,
};
use russh::{ChannelMsg, Disconnect};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use zeroize::Zeroizing;

/// Long enough for a sleepy VPN, short enough that a typo in the address does
/// not leave the user staring at a spinner.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a verified connection waits for the user to type a secret. OpenSSH
/// servers give up on unauthenticated connections after two minutes by default
/// (`LoginGraceTime`), so waiting longer than that would only hand back a dead
/// connection.
const PENDING_TTL: Duration = Duration::from_secs(110);

/// How long the server may take to answer a login or to open the terminal,
/// once connected. A server that accepts the connection and then goes silent
/// must not leave a tab spinning forever.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(30);

/// What the remote side is told it is talking to.
const TERM: &str = "xterm-256color";

pub enum SshAuth {
    /// `None` means "not asked yet".
    Password(Option<Zeroizing<String>>),
    /// A private key file — OpenSSH, PEM or PuTTY `.ppk`. `~` is expanded.
    Key {
        path: String,
        passphrase: Option<Zeroizing<String>>,
    },
    /// Private key material already in memory — from the vault, not a file.
    /// The passphrase, if the key needs one, comes from the vault too, so this
    /// never turns into a prompt: it either decodes or it does not.
    KeyContents {
        private_key: Zeroizing<String>,
        passphrase: Option<Zeroizing<String>>,
    },
}

pub struct SshTarget {
    pub address: String,
    pub port: u16,
    pub username: String,
    pub auth: SshAuth,
    /// The `SHA256:…` fingerprint the user trusted for this address and port.
    pub trusted_fingerprint: Option<String>,
}

/// A server key as the user needs to see it to decide whether to trust it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedHostKey {
    pub algorithm: String,
    pub fingerprint: String,
    pub public_key: String,
    /// OpenSSH-style randomart: people notice a changed picture faster than a
    /// changed line of base64.
    pub randomart: String,
}

/// The algorithm and SHA-256 fingerprint of a public key, for an imported
/// `known_hosts` entry. Takes the OpenSSH one-line form (`algorithm base64` or
/// `algorithm base64 comment`) and returns `None` if it does not parse — an
/// importer skips those rather than trusting something it could not read.
pub fn public_key_fingerprint(openssh_line: &str) -> Option<(String, String)> {
    let key = PublicKey::from_openssh(openssh_line.trim()).ok()?;
    Some((
        key.algorithm().as_str().to_string(),
        key.fingerprint(HashAlg::Sha256).to_string(),
    ))
}

impl ObservedHostKey {
    fn of(key: &PublicKey) -> Self {
        let header = match key.algorithm() {
            Algorithm::Ed25519 => "ED25519 256".to_string(),
            Algorithm::Rsa { .. } => "RSA".to_string(),
            Algorithm::Ecdsa { .. } => "ECDSA".to_string(),
            other => other.as_str().to_uppercase(),
        };
        Self {
            algorithm: key.algorithm().as_str().to_string(),
            fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
            public_key: key.to_openssh().unwrap_or_default(),
            randomart: key.fingerprint(HashAlg::Sha256).to_randomart(&header),
        }
    }
}

#[derive(Debug, thiserror::Error, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SshError {
    #[error("could not reach {address}: {reason}")]
    Unreachable { address: String, reason: String },

    #[error("the host key of this server is not trusted yet")]
    #[serde(rename_all = "camelCase")]
    UnknownHostKey { observed: ObservedHostKey },

    #[error("the host key changed")]
    #[serde(rename_all = "camelCase")]
    HostKeyChanged {
        trusted_fingerprint: String,
        observed: ObservedHostKey,
    },

    #[error("a password is needed")]
    PasswordRequired,

    #[error("the key is protected by a passphrase")]
    #[serde(rename_all = "camelCase")]
    PassphraseRequired { key_path: String },

    #[error("the passphrase did not unlock the key")]
    #[serde(rename_all = "camelCase")]
    PassphraseRejected { key_path: String },

    #[error("the key could not be read: {reason}")]
    #[serde(rename_all = "camelCase")]
    KeyUnreadable { key_path: String, reason: String },

    #[error("the server rejected the login")]
    AuthRejected {
        /// Methods the server said it would accept instead.
        remaining: Vec<String>,
    },

    #[error("the server refused the terminal: {reason}")]
    SessionRefused { reason: String },

    #[error("ssh error: {reason}")]
    Protocol { reason: String },
}

impl SshError {
    /// Errors that are a question for the user, after which trying again on
    /// the same connection makes sense.
    pub fn awaits_answer(&self) -> bool {
        matches!(
            self,
            Self::PasswordRequired
                | Self::PassphraseRequired { .. }
                | Self::PassphraseRejected { .. }
                | Self::AuthRejected { .. }
        )
    }
}

// ── Connection: verified, possibly not yet authenticated ────────────────────

pub struct SshConnection {
    handle: Handle<HostKeyCheck>,
    address: String,
    port: u16,
    username: String,
    opened: Instant,
    /// What the server called itself when connecting, like `SSH-2.0-OpenSSH_9.6`.
    banner: Arc<Mutex<Option<String>>>,
    authenticated: bool,
}

impl SshConnection {
    /// Connect and check the host key. Nothing about the user is sent.
    pub async fn open(target: &SshTarget) -> std::result::Result<Self, SshError> {
        let verdict = Arc::new(Mutex::new(None));
        let banner = Arc::new(Mutex::new(None));
        let handler = HostKeyCheck {
            trusted: target.trusted_fingerprint.clone(),
            verdict: Arc::clone(&verdict),
            banner: Arc::clone(&banner),
        };
        let config = Arc::new(client::Config {
            keepalive_interval: Some(Duration::from_secs(30)),
            keepalive_max: 3,
            nodelay: true,
            ..Default::default()
        });

        let connecting = client::connect(config, (target.address.as_str(), target.port), handler);
        let handle = match tokio::time::timeout(CONNECT_TIMEOUT, connecting).await {
            Ok(Ok(handle)) => handle,
            Ok(Err(err)) => {
                return Err(match verdict.lock().take() {
                    Some(Verdict::Unknown(observed)) => SshError::UnknownHostKey { observed },
                    Some(Verdict::Changed { trusted, observed }) => SshError::HostKeyChanged {
                        trusted_fingerprint: trusted,
                        observed,
                    },
                    None => match err {
                        russh::Error::IO(io) => SshError::Unreachable {
                            address: target.address.clone(),
                            reason: io.to_string(),
                        },
                        other => SshError::Protocol {
                            reason: other.to_string(),
                        },
                    },
                })
            }
            Err(_) => {
                return Err(SshError::Unreachable {
                    address: target.address.clone(),
                    reason: format!("no answer within {} s", CONNECT_TIMEOUT.as_secs()),
                })
            }
        };

        Ok(Self {
            handle,
            address: target.address.clone(),
            port: target.port,
            username: target.username.clone(),
            opened: Instant::now(),
            banner,
            authenticated: false,
        })
    }

    /// Whether this connection can carry the next attempt for `target`: still
    /// open, not too old, and for the same server and user. An edited host
    /// gets a fresh connection — and with it a fresh host key check.
    pub fn is_reusable_for(&self, target: &SshTarget) -> bool {
        !self.handle.is_closed()
            && self.opened.elapsed() < PENDING_TTL
            && self.address.eq_ignore_ascii_case(&target.address)
            && self.port == target.port
            && self.username == target.username
    }

    /// Log in. A missing password or passphrase is reported without
    /// contacting the server; the connection stays usable for the retry.
    pub async fn authenticate(&mut self, auth: &SshAuth) -> std::result::Result<(), SshError> {
        // A connection kept for a second question after logging in (sudo's
        // password for root file access) is not logged in twice.
        if self.authenticated {
            return Ok(());
        }
        let credential = prepare_credential(auth)?;
        let answered = tokio::time::timeout(ANSWER_TIMEOUT, self.login(credential)).await;
        let result = answered.map_err(|_| SshError::Protocol {
            reason: format!(
                "the server did not answer the login within {} s",
                ANSWER_TIMEOUT.as_secs()
            ),
        })??;

        match result {
            client::AuthResult::Success => {
                self.authenticated = true;
                Ok(())
            }
            client::AuthResult::Failure {
                remaining_methods, ..
            } => Err(SshError::AuthRejected {
                remaining: remaining_methods
                    .iter()
                    .map(|method| format!("{method:?}").to_lowercase())
                    .collect(),
            }),
        }
    }

    async fn login(
        &mut self,
        credential: Credential,
    ) -> std::result::Result<client::AuthResult, SshError> {
        let protocol = |e: russh::Error| SshError::Protocol {
            reason: e.to_string(),
        };
        Ok(match credential {
            Credential::Password(password) => self
                .handle
                .authenticate_password(&self.username, password.as_str())
                .await
                .map_err(protocol)?,
            Credential::Key(key) => {
                let hash = if matches!(key.algorithm(), Algorithm::Rsa { .. }) {
                    self.handle
                        .best_supported_rsa_hash()
                        .await
                        .ok()
                        .flatten()
                        .flatten()
                } else {
                    None
                };
                self.handle
                    .authenticate_publickey(
                        &self.username,
                        PrivateKeyWithHashAlg::new(Arc::new(*key), hash),
                    )
                    .await
                    .map_err(protocol)?
            }
        })
    }

    /// Open a shell on an authenticated connection and start streaming it.
    pub async fn open_shell<S: FrameSink>(
        self,
        cols: u16,
        rows: u16,
        flow_control: bool,
        sink: S,
    ) -> std::result::Result<SshSession, SshError> {
        let refused = |e: russh::Error| SshError::SessionRefused {
            reason: e.to_string(),
        };
        let opening = async {
            let channel = self.handle.channel_open_session().await.map_err(refused)?;
            channel
                .request_pty(false, TERM, u32::from(cols), u32::from(rows), 0, 0, &[])
                .await
                .map_err(refused)?;
            channel.request_shell(false).await.map_err(refused)?;
            Ok::<_, SshError>(channel)
        };
        let channel = tokio::time::timeout(ANSWER_TIMEOUT, opening)
            .await
            .map_err(|_| SshError::SessionRefused {
                reason: format!(
                    "the server did not open a terminal within {} s",
                    ANSWER_TIMEOUT.as_secs()
                ),
            })??;
        let (mut reader, writer) = channel.split();
        let handle = Arc::new(self.handle);
        let banner = self.banner.lock().clone();

        let metrics = Arc::new(Metrics::new());
        let flow = Arc::new(FlowControl::new(flow_control));
        let (tx, rx) = mpsc::channel::<Vec<u8>>(stream::CHANNEL_CAPACITY);
        let (input, mut inputs) = mpsc::unbounded_channel::<Input>();

        tokio::spawn(stream::run_batcher(
            rx,
            sink,
            Arc::clone(&metrics),
            Arc::clone(&flow),
        ));

        let reader_metrics = Arc::clone(&metrics);
        tokio::spawn(async move {
            while let Some(message) = reader.wait().await {
                match message {
                    ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
                        // Awaiting here while the batcher is paused is the
                        // backpressure: see the module docs.
                        if tx.send(Vec::from(data)).await.is_err() {
                            break;
                        }
                    }
                    ChannelMsg::ExitStatus { .. } | ChannelMsg::ExitSignal { .. } => {
                        reader_metrics.mark_child_exited();
                    }
                    ChannelMsg::Eof | ChannelMsg::Close => break,
                    _ => {}
                }
            }
            // Dropping `tx` lets the batcher flush the last frame and finish.
        });

        // Inputs go through one queue in one task, so keystrokes reach the
        // server in the order they were typed.
        let input_handle = Arc::clone(&handle);
        tokio::spawn(async move {
            while let Some(next) = inputs.recv().await {
                let sent = match next {
                    Input::Data(bytes) => writer.data_bytes(bytes).await,
                    Input::Resize(cols, rows) => {
                        writer
                            .window_change(u32::from(cols), u32::from(rows), 0, 0)
                            .await
                    }
                    Input::Close => break,
                };
                if let Err(err) = sent {
                    tracing::debug!(?err, "ssh input stopped");
                    break;
                }
            }
            let _ = writer.close().await;
            let _ = input_handle
                .disconnect(Disconnect::ByApplication, "", "en")
                .await;
        });

        tracing::info!(address = %self.address, port = self.port, "ssh session open");
        Ok(SshSession {
            input,
            metrics,
            flow,
            handle,
            banner,
        })
    }

    /// Open file access on this authenticated connection. The connection stays
    /// with the caller, so a sudo that wants a password can be asked again on
    /// it.
    pub async fn open_files(
        &self,
        elevation: Elevation,
    ) -> std::result::Result<(SftpClient, Option<String>), SftpError> {
        SftpClient::open(&self.handle, elevation).await
    }

    /// Hand the connection over to the file session that runs on it.
    pub fn into_files(self, client: SftpClient) -> FileSession {
        FileSession {
            client,
            handle: self.handle,
        }
    }

    /// Give up on this connection, telling the server rather than letting it
    /// time out.
    pub async fn close(self) {
        let _ = self
            .handle
            .disconnect(Disconnect::ByApplication, "", "en")
            .await;
    }
}

// ── Session: an open shell ──────────────────────────────────────────────────

enum Input {
    Data(Vec<u8>),
    Resize(u16, u16),
    Close,
}

pub struct SshSession {
    input: mpsc::UnboundedSender<Input>,
    metrics: Arc<Metrics>,
    flow: Arc<FlowControl>,
    handle: Arc<Handle<HostKeyCheck>>,
    banner: Option<String>,
}

impl SshSession {
    pub fn write(&self, data: &[u8]) -> Result<()> {
        self.input
            .send(Input::Data(data.to_vec()))
            .map_err(|_| CoreError::SessionClosed)
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.input
            .send(Input::Resize(cols, rows))
            .map_err(|_| CoreError::SessionClosed)
    }

    pub fn ack(&self, bytes: u64) {
        self.flow.ack(bytes);
    }

    pub fn metrics(&self) -> MetricsSnapshot {
        self.metrics.snapshot(&self.flow)
    }

    pub fn close(&self) {
        let _ = self.input.send(Input::Close);
        self.flow.close();
    }

    /// Find out what the server runs, on a channel of its own next to the
    /// terminal. `None` when it can't tell; never an error, since this is only
    /// for an icon.
    pub fn os_probe(
        &self,
    ) -> impl std::future::Future<Output = Option<&'static str>> + Send + 'static {
        let handle = Arc::clone(&self.handle);
        let banner = self.banner.clone();
        async move {
            if let Some(found) = banner.as_deref().and_then(os::from_banner) {
                return Some(found);
            }
            let mut channel = tokio::time::timeout(PROBE_TIMEOUT, handle.channel_open_session())
                .await
                .ok()?
                .ok()?;
            let output = tokio::time::timeout(PROBE_TIMEOUT, async {
                channel.exec(true, os::PROBE_COMMAND).await.ok()?;
                let mut output = Vec::new();
                while let Some(message) = channel.wait().await {
                    match message {
                        ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
                            output.extend_from_slice(&data);
                            if output.len() > os::PROBE_LIMIT {
                                break;
                            }
                        }
                        ChannelMsg::Eof | ChannelMsg::Close => break,
                        _ => {}
                    }
                }
                Some(output)
            })
            .await
            .ok()
            .flatten();
            // Closed on every way out, a timeout too: a dropped channel stays
            // open on the server, and so would the command.
            let _ = channel.close().await;
            os::from_probe(&String::from_utf8_lossy(&output?))
        }
    }
}

/// How long the system probe may take before the host just keeps its old icon.
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

// ── Files: an SFTP session on its own connection ────────────────────────────

pub struct FileSession {
    pub client: SftpClient,
    handle: Handle<HostKeyCheck>,
}

impl FileSession {
    pub async fn close(&self) {
        self.client.close().await;
        let _ = self
            .handle
            .disconnect(Disconnect::ByApplication, "", "en")
            .await;
    }

    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }

    /// Delete a file, link or folder. A root session hands this to the
    /// server's own `rm` (see [`crate::sftp::remove_as_root`]).
    pub async fn remove(&self, path: &str) -> std::result::Result<(), SftpError> {
        if self.client.is_root() {
            crate::sftp::remove_as_root(&self.handle, path, self.client.sudo_password()).await
        } else {
            self.client.remove(path).await
        }
    }
}

// ── Host key check ──────────────────────────────────────────────────────────

enum Verdict {
    Unknown(ObservedHostKey),
    Changed {
        trusted: String,
        observed: ObservedHostKey,
    },
}

pub struct HostKeyCheck {
    trusted: Option<String>,
    verdict: Arc<Mutex<Option<Verdict>>>,
    banner: Arc<Mutex<Option<String>>>,
}

impl client::Handler for HostKeyCheck {
    type Error = russh::Error;

    async fn kex_done(
        &mut self,
        _shared_secret: Option<&[u8]>,
        _names: &russh::Names,
        session: &mut client::Session,
    ) -> std::result::Result<(), Self::Error> {
        let banner = String::from_utf8_lossy(session.remote_sshid()).into_owned();
        *self.banner.lock() = Some(banner);
        Ok(())
    }

    async fn check_server_key(
        &mut self,
        presented: &PublicKeyOrCertificate,
    ) -> std::result::Result<bool, Self::Error> {
        let key = match presented {
            PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
            // Checking a host certificate properly needs a trusted CA, which
            // arrives with SSH-CA support. Until then the key inside the
            // certificate is pinned like any other key.
            PublicKeyOrCertificate::Certificate(cert) => PublicKey::from(cert.public_key().clone()),
        };
        let observed = ObservedHostKey::of(&key);

        match &self.trusted {
            Some(trusted) if *trusted == observed.fingerprint => Ok(true),
            Some(trusted) => {
                *self.verdict.lock() = Some(Verdict::Changed {
                    trusted: trusted.clone(),
                    observed,
                });
                Ok(false)
            }
            None => {
                *self.verdict.lock() = Some(Verdict::Unknown(observed));
                Ok(false)
            }
        }
    }
}

// ── Credentials ─────────────────────────────────────────────────────────────

enum Credential {
    Password(Zeroizing<String>),
    Key(Box<keys::PrivateKey>),
}

fn prepare_credential(auth: &SshAuth) -> std::result::Result<Credential, SshError> {
    match auth {
        SshAuth::Password(None) => Err(SshError::PasswordRequired),
        SshAuth::Password(Some(password)) => Ok(Credential::Password(password.clone())),
        SshAuth::Key { path, passphrase } => {
            let key_path = path.clone();
            if is_network_path(path) {
                return Err(SshError::KeyUnreadable {
                    key_path,
                    reason: "keys on network shares are not read: Windows would send your login to that server".into(),
                });
            }
            let file = expand_home(path);
            let contents = std::fs::read_to_string(&file).map_err(|e| SshError::KeyUnreadable {
                key_path: key_path.clone(),
                reason: e.to_string(),
            })?;
            if passphrase.is_some() {
                reasonable_costs(&contents, &key_path)?;
            }

            match keys::decode_secret_key(&contents, passphrase.as_ref().map(|p| p.as_str())) {
                Ok(key) => Ok(Credential::Key(Box::new(key))),
                Err(keys::Error::KeyIsEncrypted) if passphrase.is_none() => {
                    Err(SshError::PassphraseRequired { key_path })
                }
                Err(_) if passphrase.is_some() => Err(SshError::PassphraseRejected { key_path }),
                Err(err) => Err(SshError::KeyUnreadable {
                    key_path,
                    reason: err.to_string(),
                }),
            }
        }
        // A vault key: the passphrase, if any, is stored alongside it, so a
        // failure is a broken or wrongly-sealed key, not a prompt.
        SshAuth::KeyContents {
            private_key,
            passphrase,
        } => {
            reasonable_costs(private_key, "<vault>")?;
            keys::decode_secret_key(private_key, passphrase.as_ref().map(|p| p.as_str()))
                .map(|key| Credential::Key(Box::new(key)))
                .map_err(|err| SshError::KeyUnreadable {
                    key_path: "<vault>".to_string(),
                    reason: match err {
                        keys::Error::KeyIsEncrypted => {
                            "the stored key needs a passphrase that was not saved with it"
                                .to_string()
                        }
                        other => other.to_string(),
                    },
                })
        }
    }
}

/// Whether a key file path could reach beyond this computer. Opening
/// `\\server\share\key` makes Windows authenticate to that server with the
/// user's login hash, so a host entry (imported, or later synced) could use one
/// to collect the hash. Checked on the path as it will be opened, with `~`
/// expanded (`~/\\server\share` is a network path too), and only a path on a
/// drive letter counts as local.
pub fn is_network_path(path: &str) -> bool {
    let expanded = expand_home(path.trim());
    let text = expanded.as_os_str().to_string_lossy();
    let bytes = text.as_bytes();
    // Two leading separators of either kind: a UNC path, however it's spelled.
    if bytes.len() >= 2 && matches!(bytes[0], b'\\' | b'/') && matches!(bytes[1], b'\\' | b'/') {
        return true;
    }
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        // Only a path on a drive letter is local: this also turns away
        // NT-namespace spellings like `\??\UNC\server\share`.
        !matches!(
            expanded.components().next(),
            Some(Component::Prefix(prefix))
                if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
        )
    }
    #[cfg(not(windows))]
    {
        !expanded.is_absolute()
    }
}

/// Refuse a key file whose own key derivation settings would stall or crash
/// the app once a passphrase is tried (see `uwussh_keygen::check_costs`).
fn reasonable_costs(contents: &str, key_path: &str) -> std::result::Result<(), SshError> {
    uwussh_keygen::check_costs(contents).map_err(|err| SshError::KeyUnreadable {
        key_path: key_path.to_string(),
        reason: err.to_string(),
    })
}

/// `~/.ssh/id_ed25519` → the user's home directory, on Windows too.
fn expand_home(path: &str) -> PathBuf {
    let rest = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\"));
    match (rest, home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(path),
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tilde_path_lands_in_the_home_directory() {
        let Some(home) = home_dir() else { return };
        assert_eq!(
            expand_home("~/.ssh/id_ed25519"),
            home.join(".ssh/id_ed25519")
        );
        assert_eq!(expand_home("~\\keys\\nas.ppk"), home.join("keys\\nas.ppk"));
        assert_eq!(
            expand_home("C:\\keys\\nas.ppk"),
            PathBuf::from("C:\\keys\\nas.ppk")
        );
    }

    #[test]
    fn a_public_key_line_yields_its_algorithm_and_fingerprint() {
        let key = keys::PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        let line = key.public_key().to_openssh().unwrap();

        let (algorithm, fingerprint) = public_key_fingerprint(&line).unwrap();
        assert_eq!(algorithm, "ssh-ed25519");
        assert_eq!(
            fingerprint,
            key.public_key().fingerprint(HashAlg::Sha256).to_string()
        );
        // A comment after the blob is fine; garbage is not.
        assert!(public_key_fingerprint(&format!("{line} root@host")).is_some());
        assert!(public_key_fingerprint("ssh-ed25519 not-base64").is_none());
        assert!(public_key_fingerprint("").is_none());
    }

    #[test]
    fn a_missing_password_is_reported_without_asking_the_server() {
        assert!(matches!(
            prepare_credential(&SshAuth::Password(None)),
            Err(SshError::PasswordRequired)
        ));
    }

    #[test]
    fn a_key_in_memory_is_accepted_without_a_file() {
        use keys::ssh_key::LineEnding;
        let key = keys::PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        // The material a vault key arrives as: OpenSSH PEM text, no file.
        let pem = key.to_openssh(LineEnding::LF).unwrap().to_string();

        let credential = prepare_credential(&SshAuth::KeyContents {
            private_key: Zeroizing::new(pem),
            passphrase: None,
        });
        assert!(matches!(credential, Ok(Credential::Key(_))));

        // Broken material is unreadable, and blames the vault, not a file — and
        // it is never turned into a passphrase prompt.
        let broken = prepare_credential(&SshAuth::KeyContents {
            private_key: Zeroizing::new("-----BEGIN OPENSSH PRIVATE KEY-----\nnope\n".into()),
            passphrase: None,
        });
        assert!(
            matches!(broken, Err(SshError::KeyUnreadable { key_path, .. }) if key_path == "<vault>")
        );
    }

    #[test]
    fn a_key_on_a_network_share_is_never_opened() {
        // Backslashes are separators only on Windows; elsewhere these spell
        // plain file names, and only the forward-slash share is a network path.
        let paths: &[&str] = if cfg!(windows) {
            &[
                r"\\attacker\share\id",
                "//attacker/share/id",
                r"\\?\UNC\attacker\share\id",
                r"/\attacker\share\id",
                r"\??\UNC\attacker\share\id",
                r"~/\\attacker\share\id",
                "relative\\id",
            ]
        } else {
            &["//attacker/share/id", "relative/id"]
        };
        for &path in paths {
            assert!(is_network_path(path), "{path}");
            let err = prepare_credential(&SshAuth::Key {
                path: path.into(),
                passphrase: None,
            });
            assert!(matches!(err, Err(SshError::KeyUnreadable { .. })), "{path}");
        }
        #[cfg(windows)]
        assert!(!is_network_path(r"C:\keys\id"));
        #[cfg(not(windows))]
        assert!(!is_network_path("/home/nyu/.ssh/id_ed25519"));
        assert!(!is_network_path("~/.ssh/id_ed25519"));
    }

    #[test]
    fn a_missing_key_file_says_which_file() {
        let err = prepare_credential(&SshAuth::Key {
            path: "C:\\definitely\\not\\here.ppk".into(),
            passphrase: None,
        });
        assert!(
            matches!(err, Err(SshError::KeyUnreadable { key_path, .. }) if key_path.ends_with("here.ppk"))
        );
    }

    #[test]
    fn errors_serialise_with_a_kind_tag_for_the_ui() {
        let json = serde_json::to_value(SshError::PassphraseRequired {
            key_path: "~/.ssh/id".into(),
        })
        .unwrap();
        assert_eq!(json["kind"], "passphrase-required");
        assert_eq!(json["keyPath"], "~/.ssh/id");
    }

    #[test]
    fn only_questions_for_the_user_keep_a_connection_waiting() {
        assert!(SshError::PasswordRequired.awaits_answer());
        assert!(SshError::AuthRejected { remaining: vec![] }.awaits_answer());
        assert!(!SshError::Protocol {
            reason: String::new()
        }
        .awaits_answer());
    }
}
