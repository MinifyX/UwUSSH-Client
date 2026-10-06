//! Tunnels through the in-process SSH server: a local and a remote forward
//! to an echo server, stopping that frees the port, a port already taken, and
//! a terminal's connection ending underneath its tunnel.
//!
//! Every wait is for an event — a status the manager reports, bytes coming
//! back — never for time to pass.

use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use uuid::Uuid;
use uwussh_core::{
    SshLink, TunnelError, TunnelKind, TunnelManager, TunnelSpec, TunnelState, TunnelStatus,
};

const WAIT: Duration = Duration::from_secs(10);

/// An echo server on a free port: every connection gets back what it sent.
async fn echo_server() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (mut read, mut write) = socket.split();
                let _ = tokio::io::copy(&mut read, &mut write).await;
            });
        }
    });
    port
}

/// A manager whose status changes arrive on a channel.
fn manager() -> (TunnelManager, mpsc::UnboundedReceiver<TunnelStatus>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let manager = TunnelManager::new(Arc::new(move |status| {
        let _ = tx.send(status);
    }));
    (manager, rx)
}

/// The next reported status that satisfies `want`.
async fn reported(
    events: &mut mpsc::UnboundedReceiver<TunnelStatus>,
    want: impl Fn(&TunnelStatus) -> bool,
) -> TunnelStatus {
    tokio::time::timeout(WAIT, async {
        loop {
            let status = events.recv().await.expect("the manager is gone");
            if want(&status) {
                return status;
            }
        }
    })
    .await
    .expect("the status never came")
}

async fn link(sshd: &Sshd, sessions: &SessionManager) -> SshLink {
    sessions
        .open_link(
            "tunnel",
            target(sshd, password(PASSWORD), Some(&sshd.fingerprint)),
        )
        .await
        .expect("log in for the tunnel")
}

fn spec(kind: TunnelKind, bind_port: u16, target_port: u16) -> TunnelSpec {
    TunnelSpec {
        kind,
        bind_address: "127.0.0.1".into(),
        bind_port,
        target_host: "127.0.0.1".into(),
        target_port,
    }
}

fn bound(status: &TunnelStatus) -> u16 {
    match status.state {
        TunnelState::Running { bound_port } => bound_port,
        ref other => panic!("not running: {other:?}"),
    }
}

/// Send a line through `port` and read it back.
async fn round_trip(port: u16, line: &[u8]) {
    let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    socket.write_all(line).await.unwrap();
    let mut back = vec![0; line.len()];
    tokio::time::timeout(WAIT, socket.read_exact(&mut back))
        .await
        .expect("an answer through the tunnel")
        .unwrap();
    assert_eq!(back, line);
}

/// Whether nobody listens on the port any more.
async fn port_is_free(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).await.is_ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_local_tunnel_reaches_the_target_and_stopping_frees_its_port() {
    let sshd = start_sshd().await;
    let echo = echo_server().await;
    let sessions = SessionManager::new();
    let (tunnels, mut events) = manager();
    let (id, host) = (Uuid::now_v7(), Uuid::now_v7());

    let link = link(&sshd, &sessions).await;
    let status = tunnels
        .start(id, host, None, link, spec(TunnelKind::Local, 0, echo))
        .await
        .expect("the tunnel starts");
    let port = bound(&status);
    assert_ne!(port, 0, "port 0 picks a real one");

    round_trip(port, b"ping through -L\n").await;
    round_trip(port, b"and once more\n").await;
    let done = reported(&mut events, |s| s.connections == 2 && s.active == 0).await;
    assert!(done.bytes_out >= 30 && done.bytes_in >= 30);

    assert!(tunnels.stop(id).await);
    assert!(port_is_free(port).await, "stopping closed the listener");
    assert_eq!(tunnels.status(id).await, None);
    reported(&mut events, |s| s.state == TunnelState::Stopped).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_remote_tunnel_brings_the_servers_port_here_and_stopping_closes_it() {
    let sshd = start_sshd().await;
    let echo = echo_server().await;
    let sessions = SessionManager::new();
    let (tunnels, _events) = manager();
    let id = Uuid::now_v7();

    let link = link(&sshd, &sessions).await;
    let status = tunnels
        .start(
            id,
            Uuid::now_v7(),
            None,
            link,
            spec(TunnelKind::Remote, 0, echo),
        )
        .await
        .expect("the server listens");
    // The test server listens on this machine, so its port is reachable here.
    let port = bound(&status);
    assert_ne!(port, 0, "the server said which port it picked");

    round_trip(port, b"ping through -R\n").await;

    assert!(tunnels.stop(id).await);
    assert!(
        port_is_free(port).await,
        "the server stopped listening before stop returned"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_port_already_taken_is_said_so_and_kept_as_the_tunnels_state() {
    let sshd = start_sshd().await;
    let sessions = SessionManager::new();
    let (tunnels, _events) = manager();
    let id = Uuid::now_v7();
    let taken = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = taken.local_addr().unwrap().port();

    let link = link(&sshd, &sessions).await;
    let error = tunnels
        .start(
            id,
            Uuid::now_v7(),
            None,
            link.clone(),
            spec(TunnelKind::Local, port, 9),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&error, TunnelError::PortInUse { port: p, .. } if *p == port),
        "{error:?}"
    );
    let status = tunnels.status(id).await.expect("a failed tunnel is listed");
    assert_eq!(status.state, TunnelState::Failed { error });

    // Free the port and start it again: a failed tunnel can be retried.
    drop(taken);
    let status = tunnels
        .start(
            id,
            Uuid::now_v7(),
            None,
            link,
            spec(TunnelKind::Local, port, 9),
        )
        .await
        .expect("starts once the port is free");
    assert_eq!(bound(&status), port);
    tunnels.stop(id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tunnel_on_a_terminals_connection_fails_when_that_connection_ends() {
    let sshd = start_sshd().await;
    let echo = echo_server().await;
    let (sessions, session, _screen) = open_with_password(&sshd).await;
    let (tunnels, mut events) = manager();
    let id = Uuid::now_v7();

    let link = sessions.ssh_link(session).expect("an SSH terminal has one");
    let status = tunnels
        .start(
            id,
            Uuid::now_v7(),
            Some(session),
            link,
            spec(TunnelKind::Local, 0, echo),
        )
        .await
        .unwrap();
    let port = bound(&status);
    assert_eq!(status.session, Some(session));
    round_trip(port, b"along with the terminal\n").await;
    assert_eq!(sshd.log.lock().connections, 1, "no second connection");

    // The terminal's connection goes away underneath the tunnel.
    sessions.close(session).unwrap();
    let lost = reported(&mut events, |s| {
        matches!(s.state, TunnelState::Failed { .. })
    })
    .await;
    assert_eq!(
        lost.state,
        TunnelState::Failed {
            error: TunnelError::ConnectionLost
        }
    );
    assert!(port_is_free(port).await, "a lost tunnel stops listening");

    // Closing the terminal stops what ran on it.
    assert_eq!(tunnels.stop_session(session).await, 1);
    assert!(tunnels.statuses().await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tunnels_without_a_terminal_share_one_connection_per_host() {
    let sshd = start_sshd().await;
    let echo = echo_server().await;
    let sessions = SessionManager::new();
    let (tunnels, _events) = manager();
    let host = Uuid::now_v7();
    let (first, second) = (Uuid::now_v7(), Uuid::now_v7());

    assert!(tunnels.dedicated_link(host).is_none());
    let link = link(&sshd, &sessions).await;
    tunnels.keep_dedicated(host, &link);
    tunnels
        .start(first, host, None, link, spec(TunnelKind::Local, 0, echo))
        .await
        .unwrap();

    let shared = tunnels.dedicated_link(host).expect("still open");
    let status = tunnels
        .start(second, host, None, shared, spec(TunnelKind::Local, 0, echo))
        .await
        .unwrap();
    round_trip(bound(&status), b"second tunnel\n").await;
    assert_eq!(sshd.log.lock().connections, 1);

    assert_eq!(tunnels.stop_host(host).await, 2);
    assert!(tunnels.statuses().await.is_empty());
}
