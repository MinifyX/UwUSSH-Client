//! Tunnels: local (`ssh -L`) and remote (`ssh -R`) port forwards.
//!
//! A tunnel runs on an [`SshLink`] — an authenticated SSH connection, either a
//! terminal's (the tunnels a host starts with its terminal) or one of its own,
//! shared by every tunnel of that host started without a terminal and closed
//! with the last of them.
//!
//! - **Local**: a listener on this computer; every connection to it opens a
//!   `direct-tcpip` channel to the target, as seen from the server.
//! - **Remote**: the server listens (`tcpip-forward`); every connection there
//!   arrives as a `forwarded-tcpip` channel, which the connection's handler
//!   hands to the tunnel registered for that port, and which is connected to
//!   the target as seen from this computer.
//!
//! Stopping is one signal every task of the tunnel waits on: the listener
//! goes, and so does every channel it opened. [`TunnelManager::stop`] returns
//! only once the listener is closed, so the port is free when it does. A
//! connection that ends underneath a tunnel turns it into
//! [`TunnelError::ConnectionLost`] instead of leaving a listener that leads
//! nowhere.

use crate::session::SessionId;
use crate::ssh::HostKeyCheck;
use parking_lot::Mutex;
use russh::client::{Handle, Msg};
use russh::{Channel, Disconnect};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use uuid::Uuid;

/// How long opening a channel, a forward on the server or a connection to the
/// target may take.
const OPEN_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TunnelKind {
    /// A port on this computer leads to a target the server can reach.
    Local,
    /// A port on the server leads to a target this computer can reach.
    Remote,
}

impl TunnelKind {
    /// The kind a stored record names, or `None` for one a newer build added.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "local" => Some(Self::Local),
            "remote" => Some(Self::Remote),
            _ => None,
        }
    }
}

/// What a tunnel forwards: where it listens and where that leads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelSpec {
    pub kind: TunnelKind,
    /// On this computer for a local tunnel, on the server for a remote one.
    pub bind_address: String,
    /// 0 lets the operating system (or the server) pick one.
    pub bind_port: u16,
    pub target_host: String,
    pub target_port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum TunnelError {
    #[error("{address}:{port} is already in use")]
    PortInUse { address: String, port: u16 },

    #[error("could not listen on {address}:{port}: {reason}")]
    BindFailed {
        address: String,
        port: u16,
        reason: String,
    },

    #[error("the server refused to listen on {address}:{port}")]
    ForwardRefused { address: String, port: u16 },

    #[error("the connection the tunnel ran on ended")]
    ConnectionLost,

    #[error("ssh error: {reason}")]
    Protocol { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum TunnelState {
    Starting,
    Running {
        /// The port it really listens on — the picked one, for port 0.
        #[serde(rename = "boundPort")]
        bound_port: u16,
    },
    Failed {
        error: TunnelError,
    },
    Stopped,
}

/// A tunnel as the interface shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelStatus {
    pub id: Uuid,
    pub host_id: Uuid,
    /// The terminal whose connection it runs on, if it runs on one.
    pub session: Option<SessionId>,
    #[serde(flatten)]
    pub state: TunnelState,
    /// Connections forwarded so far, and how many are open right now.
    pub connections: u64,
    pub active: u64,
    /// Bytes towards the target and back, counted when a connection ends.
    pub bytes_out: u64,
    pub bytes_in: u64,
}

/// Told about every change of a tunnel's status.
pub type TunnelListener = Arc<dyn Fn(TunnelStatus) + Send + Sync>;

// ── The connection a tunnel runs on ─────────────────────────────────────────

/// Remote forwards of one connection, by the port the server listens on.
pub(crate) type ForwardTable = Mutex<HashMap<u32, RemoteForward>>;

/// What a `forwarded-tcpip` channel on that port is connected to.
#[derive(Clone)]
pub(crate) struct RemoteForward {
    target_host: String,
    target_port: u16,
    shared: Arc<Shared>,
    stop: watch::Receiver<bool>,
}

/// Set when the connection's handler goes away — that is, when the
/// connection ended, whichever way.
pub(crate) struct ClosedGuard(pub(crate) watch::Sender<bool>);

impl Drop for ClosedGuard {
    fn drop(&mut self) {
        self.0.send_replace(true);
    }
}

/// An authenticated SSH connection that tunnels can run on. Cheap to clone.
#[derive(Clone)]
pub struct SshLink {
    inner: Arc<LinkInner>,
}

pub(crate) struct LinkInner {
    handle: Arc<Handle<HostKeyCheck>>,
    forwards: Arc<ForwardTable>,
    closed: watch::Receiver<bool>,
    /// A connection of the tunnels' own is closed with the last of them; a
    /// terminal's stays the terminal's.
    owned: bool,
}

impl Drop for LinkInner {
    fn drop(&mut self) {
        if !self.owned {
            return;
        }
        let handle = Arc::clone(&self.handle);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = handle.disconnect(Disconnect::ByApplication, "", "en").await;
            });
        }
    }
}

impl SshLink {
    pub(crate) fn new(
        handle: Arc<Handle<HostKeyCheck>>,
        forwards: Arc<ForwardTable>,
        closed: watch::Receiver<bool>,
        owned: bool,
    ) -> Self {
        Self {
            inner: Arc::new(LinkInner {
                handle,
                forwards,
                closed,
                owned,
            }),
        }
    }

    /// Whether the connection underneath has ended.
    pub fn is_closed(&self) -> bool {
        *self.inner.closed.borrow() || self.inner.handle.is_closed()
    }

    fn closed(&self) -> watch::Receiver<bool> {
        self.inner.closed.clone()
    }
}

/// The connection's handler got a `forwarded-tcpip` channel: hand it to the
/// tunnel listening on that port, or turn it away.
pub(crate) fn forwarded(
    forwards: &ForwardTable,
    channel: Channel<Msg>,
    connected_port: u32,
    reply: russh::client::ChannelOpenHandle,
) {
    let Some(forward) = forwards.lock().get(&connected_port).cloned() else {
        tracing::debug!(connected_port, "forwarded channel for no tunnel, refused");
        // Dropping the reply refuses the channel.
        return;
    };
    tokio::spawn(async move {
        let target = (forward.target_host.as_str(), forward.target_port);
        let mut stop = forward.stop.clone();
        let connecting = tokio::time::timeout(OPEN_TIMEOUT, TcpStream::connect(target));
        let socket = tokio::select! {
            _ = raised(&mut stop) => return,
            socket = connecting => socket,
        };
        match socket {
            Ok(Ok(socket)) => {
                let _ = socket.set_nodelay(true);
                reply.accept().await;
                pipe(socket, channel.into_stream(), forward.shared, forward.stop).await;
            }
            // Refused: the server closes the connection it accepted.
            Ok(Err(error)) => {
                tracing::debug!(%error, host = %forward.target_host, port = forward.target_port, "remote tunnel target unreachable");
            }
            Err(_) => {
                tracing::debug!(host = %forward.target_host, port = forward.target_port, "remote tunnel target did not answer");
            }
        }
    });
}

// ── One tunnel ──────────────────────────────────────────────────────────────

struct Shared {
    id: Uuid,
    host_id: Uuid,
    session: Option<SessionId>,
    state: Mutex<TunnelState>,
    connections: AtomicU64,
    active: AtomicU64,
    bytes_out: AtomicU64,
    bytes_in: AtomicU64,
    listener: TunnelListener,
}

impl Shared {
    fn status(&self) -> TunnelStatus {
        TunnelStatus {
            id: self.id,
            host_id: self.host_id,
            session: self.session,
            state: self.state.lock().clone(),
            connections: self.connections.load(Ordering::Relaxed),
            active: self.active.load(Ordering::Relaxed),
            bytes_out: self.bytes_out.load(Ordering::Relaxed),
            bytes_in: self.bytes_in.load(Ordering::Relaxed),
        }
    }

    fn changed(&self) {
        (self.listener)(self.status());
    }

    fn set(&self, state: TunnelState) {
        *self.state.lock() = state;
        self.changed();
    }

    /// Fail a running tunnel. A stopped one stays stopped.
    fn fail(&self, error: TunnelError) {
        {
            let mut state = self.state.lock();
            if matches!(*state, TunnelState::Stopped) {
                return;
            }
            *state = TunnelState::Failed { error };
        }
        self.changed();
    }
}

/// Copy both ways until either side ends or the tunnel stops. The channel
/// closes when its stream is dropped, on every way out.
async fn pipe<S>(
    mut socket: TcpStream,
    mut ssh: S,
    shared: Arc<Shared>,
    mut stop: watch::Receiver<bool>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    shared.connections.fetch_add(1, Ordering::Relaxed);
    shared.active.fetch_add(1, Ordering::Relaxed);
    shared.changed();
    tokio::select! {
        copied = tokio::io::copy_bidirectional(&mut socket, &mut ssh) => {
            if let Ok((out, back)) = copied {
                shared.bytes_out.fetch_add(out, Ordering::Relaxed);
                shared.bytes_in.fetch_add(back, Ordering::Relaxed);
            }
        }
        _ = raised(&mut stop) => {}
    }
    let _ = ssh.shutdown().await;
    drop(ssh);
    drop(socket);
    shared.active.fetch_sub(1, Ordering::Relaxed);
    shared.changed();
}

struct Running {
    shared: Arc<Shared>,
    stop: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
    link: Option<SshLink>,
    /// The forward to cancel on the server, for a remote tunnel.
    remote: Option<(String, u32)>,
}

impl Running {
    /// Stop every task, cancel the forward on the server, and wait until the
    /// listener is gone.
    async fn finish(mut self) {
        self.stop.send_replace(true);
        if let (Some(link), Some((address, port))) = (&self.link, &self.remote) {
            link.inner.forwards.lock().remove(port);
            if !link.is_closed() {
                let cancel = link
                    .inner
                    .handle
                    .cancel_tcpip_forward(address.clone(), *port);
                let _ = tokio::time::timeout(OPEN_TIMEOUT, cancel).await;
            }
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

/// Every tunnel that runs or failed, by its record's id.
pub struct TunnelManager {
    tunnels: tokio::sync::Mutex<HashMap<Uuid, Running>>,
    /// The connection of their own that tunnels started without a terminal
    /// share, per host, for as long as one of them runs.
    dedicated: Mutex<HashMap<Uuid, Weak<LinkInner>>>,
    listener: TunnelListener,
}

impl TunnelManager {
    pub fn new(listener: TunnelListener) -> Self {
        Self {
            tunnels: tokio::sync::Mutex::new(HashMap::new()),
            dedicated: Mutex::new(HashMap::new()),
            listener,
        }
    }

    /// The tunnels' own connection to `host`, if one is open.
    pub fn dedicated_link(&self, host: Uuid) -> Option<SshLink> {
        let mut dedicated = self.dedicated.lock();
        let link = dedicated
            .get(&host)
            .and_then(Weak::upgrade)
            .map(|inner| SshLink { inner })
            .filter(|link| !link.is_closed());
        if link.is_none() {
            dedicated.remove(&host);
        }
        link
    }

    /// Remember a connection opened for `host`'s tunnels, so the next one
    /// started without a terminal runs on it too.
    pub fn keep_dedicated(&self, host: Uuid, link: &SshLink) {
        self.dedicated
            .lock()
            .insert(host, Arc::downgrade(&link.inner));
    }

    /// Start a tunnel on `link`. One that already runs is left as it is; one
    /// that failed starts again. A failure is kept as the tunnel's state, so
    /// the list can show it, and returned.
    pub async fn start(
        &self,
        id: Uuid,
        host_id: Uuid,
        session: Option<SessionId>,
        link: SshLink,
        spec: TunnelSpec,
    ) -> Result<TunnelStatus, TunnelError> {
        let mut tunnels = self.tunnels.lock().await;
        if let Some(running) = tunnels.get(&id) {
            let state = running.shared.state.lock().clone();
            if matches!(state, TunnelState::Running { .. } | TunnelState::Starting) {
                return Ok(running.shared.status());
            }
        }
        if let Some(old) = tunnels.remove(&id) {
            old.finish().await;
        }

        let shared = Arc::new(Shared {
            id,
            host_id,
            session,
            state: Mutex::new(TunnelState::Starting),
            connections: AtomicU64::new(0),
            active: AtomicU64::new(0),
            bytes_out: AtomicU64::new(0),
            bytes_in: AtomicU64::new(0),
            listener: Arc::clone(&self.listener),
        });
        let (stop, stopped) = watch::channel(false);
        shared.changed();

        let started = match spec.kind {
            TunnelKind::Local => start_local(&link, &spec, &shared, stopped).await,
            TunnelKind::Remote => start_remote(&link, &spec, &shared, stopped).await,
        };
        match started {
            Ok((task, bound, remote)) => {
                tunnels.insert(
                    id,
                    Running {
                        shared: Arc::clone(&shared),
                        stop,
                        task: Some(task),
                        link: Some(link),
                        remote,
                    },
                );
                shared.set(TunnelState::Running { bound_port: bound });
                tracing::info!(%id, kind = ?spec.kind, port = bound, "tunnel running");
                Ok(shared.status())
            }
            Err(error) => {
                tracing::info!(%id, %error, "tunnel did not start");
                shared.set(TunnelState::Failed {
                    error: error.clone(),
                });
                tunnels.insert(
                    id,
                    Running {
                        shared,
                        stop,
                        task: None,
                        link: None,
                        remote: None,
                    },
                );
                Err(error)
            }
        }
    }

    /// Stop a tunnel, or forget a failed one. Returns once its port is free.
    pub async fn stop(&self, id: Uuid) -> bool {
        let Some(running) = self.tunnels.lock().await.remove(&id) else {
            return false;
        };
        let shared = Arc::clone(&running.shared);
        running.finish().await;
        shared.set(TunnelState::Stopped);
        tracing::info!(%id, "tunnel stopped");
        true
    }

    /// Stop every tunnel matching `which`.
    async fn stop_where(&self, which: impl Fn(&Shared) -> bool) -> usize {
        let ids: Vec<Uuid> = self
            .tunnels
            .lock()
            .await
            .values()
            .filter(|running| which(&running.shared))
            .map(|running| running.shared.id)
            .collect();
        for id in &ids {
            self.stop(*id).await;
        }
        ids.len()
    }

    /// A terminal closes: the tunnels on its connection stop with it.
    pub async fn stop_session(&self, session: SessionId) -> usize {
        self.stop_where(|shared| shared.session == Some(session))
            .await
    }

    /// A host was deleted: its tunnels stop.
    pub async fn stop_host(&self, host: Uuid) -> usize {
        self.stop_where(|shared| shared.host_id == host).await
    }

    pub async fn stop_all(&self) -> usize {
        self.stop_where(|_| true).await
    }

    /// Every tunnel that runs, starts or failed.
    pub async fn statuses(&self) -> Vec<TunnelStatus> {
        self.tunnels
            .lock()
            .await
            .values()
            .map(|running| running.shared.status())
            .collect()
    }

    pub async fn status(&self, id: Uuid) -> Option<TunnelStatus> {
        self.tunnels
            .lock()
            .await
            .get(&id)
            .map(|running| running.shared.status())
    }
}

type Started = (JoinHandle<()>, u16, Option<(String, u32)>);

async fn start_local(
    link: &SshLink,
    spec: &TunnelSpec,
    shared: &Arc<Shared>,
    stopped: watch::Receiver<bool>,
) -> Result<Started, TunnelError> {
    if link.is_closed() {
        return Err(TunnelError::ConnectionLost);
    }
    let listener = TcpListener::bind((spec.bind_address.as_str(), spec.bind_port))
        .await
        .map_err(|error| bind_error(spec, &error))?;
    let bound = listener
        .local_addr()
        .map(|address| address.port())
        .unwrap_or(spec.bind_port);

    let link = link.clone();
    let shared = Arc::clone(shared);
    let target_host = spec.target_host.clone();
    let target_port = spec.target_port;
    let task = tokio::spawn(async move {
        let mut stop = stopped.clone();
        let mut closed = link.closed();
        let lost = loop {
            tokio::select! {
                _ = raised(&mut stop) => break false,
                _ = raised(&mut closed) => break true,
                accepted = listener.accept() => match accepted {
                    Ok((socket, peer)) => {
                        tokio::spawn(open_local(
                            link.clone(),
                            target_host.clone(),
                            target_port,
                            socket,
                            peer,
                            Arc::clone(&shared),
                            stopped.clone(),
                        ));
                    }
                    Err(error) => {
                        // Out of file handles and the like: the next accept
                        // may work again, and a busy loop is what a short
                        // pause prevents.
                        tracing::warn!(%error, "tunnel could not accept a connection");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
        };
        // The listener goes here: before `stop` returns, and before a lost
        // connection is reported.
        drop(listener);
        if lost {
            shared.fail(TunnelError::ConnectionLost);
        }
    });
    Ok((task, bound, None))
}

async fn open_local(
    link: SshLink,
    target_host: String,
    target_port: u16,
    socket: TcpStream,
    peer: SocketAddr,
    shared: Arc<Shared>,
    mut stop: watch::Receiver<bool>,
) {
    let _ = socket.set_nodelay(true);
    let opening = link.inner.handle.channel_open_direct_tcpip(
        target_host.clone(),
        u32::from(target_port),
        peer.ip().to_string(),
        u32::from(peer.port()),
    );
    let channel = tokio::select! {
        _ = raised(&mut stop) => return,
        opened = tokio::time::timeout(OPEN_TIMEOUT, opening) => opened,
    };
    match channel {
        Ok(Ok(channel)) => pipe(socket, channel.into_stream(), shared, stop).await,
        Ok(Err(error)) => {
            tracing::debug!(%error, host = %target_host, port = target_port, "the server did not open the tunnel's channel");
        }
        Err(_) => tracing::debug!("the server did not answer the tunnel's channel"),
    }
}

async fn start_remote(
    link: &SshLink,
    spec: &TunnelSpec,
    shared: &Arc<Shared>,
    stopped: watch::Receiver<bool>,
) -> Result<Started, TunnelError> {
    if link.is_closed() {
        return Err(TunnelError::ConnectionLost);
    }
    let forward = RemoteForward {
        target_host: spec.target_host.clone(),
        target_port: spec.target_port,
        shared: Arc::clone(shared),
        stop: stopped.clone(),
    };
    let requested = u32::from(spec.bind_port);
    // A fixed port is registered before asking, so a connection the server
    // forwards right away already finds its tunnel.
    if requested != 0 {
        let mut forwards = link.inner.forwards.lock();
        if forwards.contains_key(&requested) {
            return Err(TunnelError::PortInUse {
                address: spec.bind_address.clone(),
                port: spec.bind_port,
            });
        }
        forwards.insert(requested, forward.clone());
    }
    let refused = || TunnelError::ForwardRefused {
        address: spec.bind_address.clone(),
        port: spec.bind_port,
    };
    let asked = tokio::time::timeout(
        OPEN_TIMEOUT,
        link.inner
            .handle
            .tcpip_forward(spec.bind_address.clone(), requested),
    )
    .await;
    let port = match asked {
        Ok(Ok(given)) => {
            if requested == 0 {
                link.inner.forwards.lock().insert(given, forward);
                given
            } else {
                requested
            }
        }
        failed => {
            if requested != 0 {
                link.inner.forwards.lock().remove(&requested);
            }
            return Err(match failed {
                _ if link.is_closed() => TunnelError::ConnectionLost,
                Ok(Err(russh::Error::RequestDenied)) => refused(),
                Ok(Err(error)) => TunnelError::Protocol {
                    reason: error.to_string(),
                },
                _ => refused(),
            });
        }
    };

    let shared = Arc::clone(shared);
    let mut closed = link.closed();
    let mut stop = stopped;
    let task = tokio::spawn(async move {
        tokio::select! {
            _ = raised(&mut stop) => {}
            _ = raised(&mut closed) => shared.fail(TunnelError::ConnectionLost),
        }
    });
    let bound = u16::try_from(port).unwrap_or(spec.bind_port);
    Ok((task, bound, Some((spec.bind_address.clone(), port))))
}

/// Wait until a flag is raised — or its sender is gone, which means the same.
async fn raised(flag: &mut watch::Receiver<bool>) {
    let _ = flag.wait_for(|raised| *raised).await;
}

fn bind_error(spec: &TunnelSpec, error: &std::io::Error) -> TunnelError {
    match error.kind() {
        std::io::ErrorKind::AddrInUse => TunnelError::PortInUse {
            address: spec.bind_address.clone(),
            port: spec.bind_port,
        },
        _ => TunnelError::BindFailed {
            address: spec.bind_address.clone(),
            port: spec.bind_port,
            reason: error.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_parse_and_a_newer_one_is_not_guessed() {
        assert_eq!(TunnelKind::parse("local"), Some(TunnelKind::Local));
        assert_eq!(TunnelKind::parse("remote"), Some(TunnelKind::Remote));
        assert_eq!(TunnelKind::parse("dynamic"), None);
    }

    #[test]
    fn a_status_serialises_flat_for_the_ui() {
        let status = TunnelStatus {
            id: Uuid::nil(),
            host_id: Uuid::nil(),
            session: None,
            state: TunnelState::Failed {
                error: TunnelError::PortInUse {
                    address: "127.0.0.1".into(),
                    port: 8080,
                },
            },
            connections: 0,
            active: 0,
            bytes_out: 0,
            bytes_in: 0,
        };
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["state"], "failed");
        assert_eq!(json["error"]["kind"], "port-in-use");
        assert_eq!(json["error"]["port"], 8080);
        assert_eq!(json["hostId"], Uuid::nil().to_string());

        let running = serde_json::to_value(TunnelState::Running { bound_port: 9000 }).unwrap();
        assert_eq!(running["state"], "running");
        assert_eq!(running["boundPort"], 9000);
    }
}
