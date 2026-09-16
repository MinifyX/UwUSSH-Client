//! SSH sessions against a real SSH server, running in this process.
//!
//! No test host, no Docker, no network: russh's server half stands in for
//! sshd on 127.0.0.1. That makes the parts that matter testable on every
//! commit — above all the order of operations, where a mistake would mean
//! sending a password to a server whose key nobody checked.

use parking_lot::Mutex;
use russh::keys::ssh_key::LineEnding;
use russh::keys::{Algorithm, HashAlg, PrivateKey, PublicKey};
use russh::server::{self, Auth, Msg, Server as _, Session};
use russh::{Channel, ChannelId};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use uwussh_core::{FrameSink, SessionId, SessionManager, SinkError, SshAuth, SshError, SshTarget};
use zeroize::Zeroizing;

const USER: &str = "lorin";
const PASSWORD: &str = "correct horse battery staple";
const FLOOD_MIB: usize = 8;
const COLS: u16 = 100;
const ROWS: u16 = 30;

// ── The server ──────────────────────────────────────────────────────────────

#[derive(Default)]
struct ServerLog {
    auth_attempts: usize,
    pty: Option<(u32, u32)>,
    resizes: Vec<(u32, u32)>,
    received: Vec<u8>,
}

#[derive(Clone)]
struct TestSshd {
    accepted_key: PublicKey,
    log: Arc<Mutex<ServerLog>>,
}

impl server::Server for TestSshd {
    type Handler = Self;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
        self.clone()
    }
}

fn reject() -> Auth {
    Auth::Reject {
        proceed_with_methods: None,
        partial_success: false,
    }
}

impl server::Handler for TestSshd {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        self.log.lock().auth_attempts += 1;
        Ok(if user == USER && password == PASSWORD {
            Auth::Accept
        } else {
            reject()
        })
    }

    async fn auth_publickey(&mut self, user: &str, key: &PublicKey) -> Result<Auth, Self::Error> {
        self.log.lock().auth_attempts += 1;
        Ok(
            if user == USER && key.key_data() == self.accepted_key.key_data() {
                Auth::Accept
            } else {
                reject()
            },
        )
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn pty_request(
        &mut self,
        _channel: ChannelId,
        _term: &str,
        cols: u32,
        rows: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(russh::Pty, u32)],
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.log.lock().pty = Some((cols, rows));
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.data(channel, b"welcome\r\n".to_vec())?;
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.log.lock().received.extend_from_slice(data);

        match data {
            b"flood\r" => {
                // From a task, so this handler returns and the server keeps
                // processing — and so `Handle::data` can wait on backpressure.
                let handle = session.handle();
                tokio::spawn(async move {
                    let chunk = vec![b'x'; 64 * 1024];
                    for _ in 0..FLOOD_MIB * 16 {
                        if handle.data(channel, chunk.clone()).await.is_err() {
                            return;
                        }
                    }
                    let _ = handle.data(channel, b"<end>".to_vec()).await;
                });
            }
            b"exit\r" => {
                session.exit_status_request(channel, 0)?;
                session.eof(channel)?;
                session.close(channel)?;
            }
            echo => session.data(channel, echo.to_vec())?,
        }
        Ok(())
    }

    async fn window_change_request(
        &mut self,
        _channel: ChannelId,
        cols: u32,
        rows: u32,
        _pix_width: u32,
        _pix_height: u32,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.log.lock().resizes.push((cols, rows));
        Ok(())
    }
}

struct Sshd {
    port: u16,
    fingerprint: String,
    client_key: PrivateKey,
    log: Arc<Mutex<ServerLog>>,
}

async fn start_sshd() -> Sshd {
    let host_key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
    let client_key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
    let log = Arc::new(Mutex::new(ServerLog::default()));

    let config = Arc::new(server::Config {
        auth_rejection_time: Duration::from_millis(10),
        auth_rejection_time_initial: Some(Duration::ZERO),
        keys: vec![host_key.clone()],
        ..Default::default()
    });

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut sshd = TestSshd {
        accepted_key: client_key.public_key().clone(),
        log: Arc::clone(&log),
    };
    tokio::spawn(async move {
        let _ = sshd.run_on_socket(config, &listener).await;
    });

    Sshd {
        port,
        fingerprint: host_key
            .public_key()
            .fingerprint(HashAlg::Sha256)
            .to_string(),
        client_key,
        log,
    }
}

// ── The client side ─────────────────────────────────────────────────────────

/// What a terminal would have shown.
#[derive(Clone, Default)]
struct Screen {
    bytes: Arc<Mutex<Vec<u8>>>,
    ended: Arc<AtomicBool>,
}

impl FrameSink for Screen {
    fn send(&self, frame: &[u8]) -> Result<(), SinkError> {
        self.bytes.lock().extend_from_slice(frame);
        Ok(())
    }

    fn finish(&self) {
        self.ended.store(true, Ordering::SeqCst);
    }
}

impl Screen {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes.lock()).into_owned()
    }

    fn len(&self) -> usize {
        self.bytes.lock().len()
    }
}

fn target(sshd: &Sshd, auth: SshAuth, trusted: Option<&str>) -> SshTarget {
    SshTarget {
        address: "127.0.0.1".into(),
        port: sshd.port,
        username: USER.into(),
        auth,
        trusted_fingerprint: trusted.map(str::to_string),
    }
}

fn password(value: &str) -> SshAuth {
    SshAuth::Password(Some(Zeroizing::new(value.to_string())))
}

/// Play renderer: acknowledge what arrived, at most `per_tick` bytes every
/// 5 ms, until `done` holds or the deadline passes.
async fn render_until(
    manager: &SessionManager,
    id: SessionId,
    screen: &Screen,
    per_tick: usize,
    timeout: Duration,
    done: impl Fn(&Screen) -> bool,
) -> bool {
    let started = Instant::now();
    let mut acked = 0usize;
    while started.elapsed() < timeout {
        let received = screen.len();
        let step = (received - acked).min(per_tick);
        if step > 0 {
            let _ = manager.ack(id, step as u64);
            acked += step;
        }
        if done(screen) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    done(screen)
}

struct TempKey(PathBuf);

impl TempKey {
    fn write(key: &PrivateKey) -> Self {
        let path = std::env::temp_dir().join(format!("uwussh-test-key-{}", uuid::Uuid::now_v7()));
        std::fs::write(&path, key.to_openssh(LineEnding::LF).unwrap().as_bytes()).unwrap();
        Self(path)
    }

    fn path(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
}

impl Drop for TempKey {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

async fn open_with_password(sshd: &Sshd) -> (SessionManager, SessionId, Screen) {
    let manager = SessionManager::new();
    let screen = Screen::default();
    let id = manager
        .spawn_ssh(
            target(sshd, password(PASSWORD), Some(&sshd.fingerprint)),
            COLS,
            ROWS,
            screen.clone(),
        )
        .await
        .expect("password login");
    (manager, id, screen)
}

// ── Security order ──────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_untrusted_server_shows_its_key_and_gets_no_login_attempt() {
    let sshd = start_sshd().await;
    let err = SessionManager::new()
        .spawn_ssh(
            target(&sshd, password(PASSWORD), None),
            COLS,
            ROWS,
            Screen::default(),
        )
        .await
        .unwrap_err();

    let SshError::UnknownHostKey { observed } = err else {
        panic!("expected UnknownHostKey, got {err:?}");
    };
    assert_eq!(observed.fingerprint, sshd.fingerprint);
    assert_eq!(observed.algorithm, "ssh-ed25519");
    assert!(
        observed.randomart.contains("[SHA256]"),
        "{}",
        observed.randomart
    );
    assert!(observed.public_key.starts_with("ssh-ed25519 "));
    assert_eq!(
        sshd.log.lock().auth_attempts,
        0,
        "credentials went to an untrusted server"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_changed_key_is_refused_before_the_password_leaves() {
    let sshd = start_sshd().await;
    let stale = "SHA256:0000000000000000000000000000000000000000000";
    let err = SessionManager::new()
        .spawn_ssh(
            target(&sshd, password(PASSWORD), Some(stale)),
            COLS,
            ROWS,
            Screen::default(),
        )
        .await
        .unwrap_err();

    let SshError::HostKeyChanged {
        trusted_fingerprint,
        observed,
    } = err
    else {
        panic!("expected HostKeyChanged, got {err:?}");
    };
    assert_eq!(trusted_fingerprint, stale);
    assert_eq!(observed.fingerprint, sshd.fingerprint);
    assert_eq!(
        sshd.log.lock().auth_attempts,
        0,
        "the password was sent to a changed key"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_missing_password_is_asked_for_before_anything_is_sent() {
    let sshd = start_sshd().await;
    let err = SessionManager::new()
        .spawn_ssh(
            target(&sshd, SshAuth::Password(None), Some(&sshd.fingerprint)),
            COLS,
            ROWS,
            Screen::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, SshError::PasswordRequired), "{err:?}");
    assert_eq!(sshd.log.lock().auth_attempts, 0);
}

// ── Logging in ──────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_password_login_opens_a_shell_with_the_right_size() {
    let sshd = start_sshd().await;
    let (manager, id, screen) = open_with_password(&sshd).await;

    assert!(
        render_until(
            &manager,
            id,
            &screen,
            usize::MAX,
            Duration::from_secs(5),
            |s| { s.text().contains("welcome") }
        )
        .await,
        "no greeting, screen: {:?}",
        screen.text()
    );
    assert_eq!(
        sshd.log.lock().pty,
        Some((u32::from(COLS), u32::from(ROWS)))
    );
    manager.close(id).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wrong_password_is_rejected() {
    let sshd = start_sshd().await;
    let err = SessionManager::new()
        .spawn_ssh(
            target(&sshd, password("hunter2"), Some(&sshd.fingerprint)),
            COLS,
            ROWS,
            Screen::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, SshError::AuthRejected { .. }), "{err:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_key_file_logs_in() {
    let sshd = start_sshd().await;
    let key = TempKey::write(&sshd.client_key);
    let manager = SessionManager::new();
    let screen = Screen::default();

    let id = manager
        .spawn_ssh(
            target(
                &sshd,
                SshAuth::Key {
                    path: key.path(),
                    passphrase: None,
                },
                Some(&sshd.fingerprint),
            ),
            COLS,
            ROWS,
            screen.clone(),
        )
        .await
        .expect("key login");

    assert!(
        render_until(
            &manager,
            id,
            &screen,
            usize::MAX,
            Duration::from_secs(5),
            |s| { s.text().contains("welcome") }
        )
        .await
    );
    manager.close(id).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_encrypted_key_asks_for_its_passphrase_and_rejects_a_wrong_one() {
    let sshd = start_sshd().await;
    let encrypted = sshd
        .client_key
        .encrypt(&mut rand::rng(), "hunter2")
        .unwrap();
    let key = TempKey::write(&encrypted);
    let manager = SessionManager::new();

    let attempt = |passphrase: Option<&str>| {
        target(
            &sshd,
            SshAuth::Key {
                path: key.path(),
                passphrase: passphrase.map(|p| Zeroizing::new(p.to_string())),
            },
            Some(&sshd.fingerprint),
        )
    };

    let err = manager
        .spawn_ssh(attempt(None), COLS, ROWS, Screen::default())
        .await
        .unwrap_err();
    assert!(
        matches!(err, SshError::PassphraseRequired { .. }),
        "{err:?}"
    );

    let err = manager
        .spawn_ssh(attempt(Some("wrong")), COLS, ROWS, Screen::default())
        .await
        .unwrap_err();
    assert!(
        matches!(err, SshError::PassphraseRejected { .. }),
        "{err:?}"
    );

    assert_eq!(
        sshd.log.lock().auth_attempts,
        0,
        "a locked key never reaches the server"
    );

    let id = manager
        .spawn_ssh(attempt(Some("hunter2")), COLS, ROWS, Screen::default())
        .await
        .expect("login with the right passphrase");
    manager.close(id).unwrap();
}

// ── Using the session ───────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn keystrokes_arrive_in_the_order_they_were_typed() {
    let sshd = start_sshd().await;
    let (manager, id, screen) = open_with_password(&sshd).await;

    let typed: Vec<u8> = (0..300)
        .map(|i| b"abcdefghijklmnopqrstuvwxyz"[i % 26])
        .collect();
    for byte in &typed {
        manager.write(id, &[*byte]).unwrap();
    }

    let arrived = render_until(
        &manager,
        id,
        &screen,
        usize::MAX,
        Duration::from_secs(5),
        |_| sshd.log.lock().received.len() >= typed.len(),
    )
    .await;
    assert!(
        arrived,
        "only {} of {} keystrokes arrived",
        sshd.log.lock().received.len(),
        typed.len()
    );
    assert_eq!(sshd.log.lock().received, typed);
    manager.close(id).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resizing_reaches_the_server() {
    let sshd = start_sshd().await;
    let (manager, id, screen) = open_with_password(&sshd).await;

    manager.resize(id, 160, 48).unwrap();
    let resized = render_until(
        &manager,
        id,
        &screen,
        usize::MAX,
        Duration::from_secs(5),
        |_| sshd.log.lock().resizes.contains(&(160, 48)),
    )
    .await;
    assert!(resized, "resizes seen: {:?}", sshd.log.lock().resizes);
    manager.close(id).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_flood_arrives_complete_while_a_slow_renderer_holds_it_back() {
    let sshd = start_sshd().await;
    let (manager, id, screen) = open_with_password(&sshd).await;
    manager.write(id, b"flood\r").unwrap();

    // 128 KiB per 5 ms is ~25 MiB/s — slower than the server sends, so flow
    // control has to pause the stream for everything to arrive intact.
    let complete = render_until(
        &manager,
        id,
        &screen,
        128 * 1024,
        Duration::from_secs(30),
        |s| s.text().ends_with("<end>"),
    )
    .await;
    assert!(
        complete,
        "flood incomplete: {} bytes on screen",
        screen.len()
    );

    let text = screen.text();
    let flood = text.bytes().filter(|&b| b == b'x').count();
    assert_eq!(
        flood,
        FLOOD_MIB * 1024 * 1024,
        "bytes went missing on the way"
    );

    let metrics = manager.metrics(id).unwrap();
    assert!(
        metrics.flow_pauses > 0,
        "the slow renderer never held the stream back"
    );
    assert!(
        metrics.peak_unacked <= 512 * 1024 + 256 * 1024,
        "outstanding bytes peaked at {}",
        metrics.peak_unacked
    );
    manager.close(id).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_session_ends_when_the_remote_shell_exits() {
    let sshd = start_sshd().await;
    let (manager, id, screen) = open_with_password(&sshd).await;

    manager.write(id, b"exit\r").unwrap();
    let ended = render_until(
        &manager,
        id,
        &screen,
        usize::MAX,
        Duration::from_secs(5),
        |s| s.ended.load(Ordering::SeqCst),
    )
    .await;
    assert!(ended, "the end of the session never reached the renderer");

    let metrics = manager.metrics(id).unwrap();
    assert!(metrics.finished);
    assert!(metrics.child_exited, "the exit status was not seen");
    manager.close(id).unwrap();
}
