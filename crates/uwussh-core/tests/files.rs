//! File access and system detection against a real SSH server in this process.
//!
//! The server serves a temporary folder over SFTP, answers the system probe
//! like an Ubuntu machine, and plays `sudo` for root file access: it prompts
//! with UwUSSH's marker, checks the password, and only then starts SFTP on the
//! same channel — which is exactly the dance a real server does with
//! `sudo sftp-server` on a pseudo-terminal.

#[path = "support/file_server.rs"]
mod file_server;

use file_server::FileServer;
use parking_lot::Mutex;
use russh::keys::{Algorithm, HashAlg, PrivateKey};
use russh::server::{self, Auth, Msg, Server as _, Session};
use russh::{Channel, ChannelId};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use uwussh_core::{
    Elevation, EntryKind, FilesError, FrameSink, SessionManager, SftpError, SinkError, SshAuth,
    SshTarget,
};
use zeroize::Zeroizing;

const USER: &str = "uwu";
const PASSWORD: &str = "nyu";
const SUDO_PASSWORD: &str = "root-please";

#[derive(Default)]
struct Log {
    /// The password lines sudo received, in order.
    sudo_answers: Vec<String>,
    /// Whether a pty was asked for with echo switched off.
    raw_pty: bool,
}

struct TestServer {
    files: Arc<PathBuf>,
    log: Arc<Mutex<Log>>,
    channels: HashMap<ChannelId, Channel<Msg>>,
}

impl server::Server for TestServer {
    type Handler = Self;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
        Self {
            files: Arc::clone(&self.files),
            log: Arc::clone(&self.log),
            channels: HashMap::new(),
        }
    }
}

impl server::Handler for TestServer {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if user == USER && password == PASSWORD {
            Auth::Accept
        } else {
            Auth::Reject {
                proceed_with_methods: None,
                partial_success: false,
            }
        })
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn pty_request(
        &mut self,
        _channel: ChannelId,
        _term: &str,
        _cols: u32,
        _rows: u32,
        _pix_width: u32,
        _pix_height: u32,
        modes: &[(russh::Pty, u32)],
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        if modes.contains(&(russh::Pty::ECHO, 0)) && modes.contains(&(russh::Pty::OPOST, 0)) {
            self.log.lock().raw_pty = true;
        }
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.remove(&channel);
        session.data(channel, b"$ ".to_vec())?;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let command = String::from_utf8_lossy(data).into_owned();
        let open = self.channels.remove(&channel);
        session.channel_success(channel)?;

        if command.contains("uname") {
            session.data(channel, b"Linux\nID=ubuntu\nID_LIKE=debian\n".to_vec())?;
            session.exit_status_request(channel, 0)?;
            session.eof(channel)?;
            session.close(channel)?;
            return Ok(());
        }

        if command.starts_with("sudo -p 'UWUSSH-SUDO-PROMPT:'") {
            let Some(open) = open else { return Ok(()) };
            let files = PathBuf::clone(&self.files);
            let log = Arc::clone(&self.log);
            tokio::spawn(async move {
                let mut stream = open.into_stream();
                for _ in 0..3 {
                    let _ = stream.write_all(b"UWUSSH-SUDO-PROMPT:").await;
                    let mut line = Vec::new();
                    let mut byte = [0u8; 1];
                    loop {
                        if stream.read_exact(&mut byte).await.is_err() {
                            // The client went away without answering.
                            return;
                        }
                        if byte[0] == b'\n' {
                            break;
                        }
                        line.push(byte[0]);
                    }
                    let typed = String::from_utf8_lossy(&line).into_owned();
                    log.lock().sudo_answers.push(typed.clone());
                    if typed == SUDO_PASSWORD {
                        let _ = stream.write_all(b"\nUWUSSH-SFTP-READY\n").await;
                        russh_sftp::server::run(stream, FileServer::new(files)).await;
                        return;
                    }
                    let _ = stream.write_all(b"\nSorry, try again.\n").await;
                }
            });
        }
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        match (name, self.channels.remove(&channel)) {
            ("sftp", Some(open)) => {
                session.channel_success(channel)?;
                let files = FileServer::new(PathBuf::clone(&self.files));
                russh_sftp::server::run(open.into_stream(), files).await;
            }
            _ => session.channel_failure(channel)?,
        }
        Ok(())
    }
}

struct Running {
    port: u16,
    fingerprint: String,
    files: PathBuf,
    log: Arc<Mutex<Log>>,
}

async fn start() -> Running {
    let host_key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
    let files = std::env::temp_dir().join(format!("uwussh-files-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(files.join("home").join("uwu")).unwrap();
    std::fs::write(files.join("home/uwu/welcome.txt"), "hello \u{2727}\n").unwrap();
    std::fs::create_dir_all(files.join("etc")).unwrap();
    std::fs::write(files.join("etc/secret.conf"), "root only\n").unwrap();

    let log = Arc::new(Mutex::new(Log::default()));
    let config = Arc::new(server::Config {
        auth_rejection_time: Duration::from_millis(10),
        auth_rejection_time_initial: Some(Duration::ZERO),
        keys: vec![host_key.clone()],
        ..Default::default()
    });
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut server = TestServer {
        files: Arc::new(files.clone()),
        log: Arc::clone(&log),
        channels: HashMap::new(),
    };
    tokio::spawn(async move {
        let _ = server.run_on_socket(config, &listener).await;
    });
    Running {
        port,
        fingerprint: host_key
            .public_key()
            .fingerprint(HashAlg::Sha256)
            .to_string(),
        files,
        log,
    }
}

fn target(server: &Running) -> SshTarget {
    SshTarget {
        address: "127.0.0.1".into(),
        port: server.port,
        username: USER.into(),
        auth: SshAuth::Password(Some(Zeroizing::new(PASSWORD.into()))),
        trusted_fingerprint: Some(server.fingerprint.clone()),
    }
}

fn no_progress() -> impl Fn(u64) + Send + Sync {
    |_| {}
}

struct Discard;

impl FrameSink for Discard {
    fn send(&self, _frame: &[u8]) -> Result<(), SinkError> {
        Ok(())
    }
    fn finish(&self) {}
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_file_browser_lists_uploads_downloads_and_tidies_up() {
    let server = start().await;
    let manager = SessionManager::new();
    let (id, home) = manager
        .open_files("files", target(&server), Elevation::User)
        .await
        .unwrap();
    assert_eq!(home.as_deref(), Some("/home/uwu"));
    let files = manager.files(id).unwrap();
    let client = &files.client;

    let listed = client.list("/home/uwu").await.unwrap();
    assert!(listed
        .iter()
        .any(|e| e.name == "welcome.txt" && e.kind == EntryKind::File && e.size > 0));

    // Up: a folder with a file in it.
    let local = std::env::temp_dir().join(format!("uwussh-upload-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(local.join("inner")).unwrap();
    let payload: Vec<u8> = (0..600_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(local.join("inner").join("data.bin"), &payload).unwrap();
    let sent = Arc::new(Mutex::new(0u64));
    let counter = Arc::clone(&sent);
    let remote = client
        .upload(
            &local,
            "/home/uwu",
            false,
            &AtomicBool::new(false),
            &move |n| *counter.lock() += n,
        )
        .await
        .unwrap();
    assert_eq!(*sent.lock(), payload.len() as u64);
    let folder_name = local.file_name().unwrap().to_str().unwrap();
    assert_eq!(remote, format!("/home/uwu/{folder_name}"));
    assert_eq!(
        std::fs::read(
            server
                .files
                .join("home/uwu")
                .join(folder_name)
                .join("inner/data.bin")
        )
        .unwrap(),
        payload
    );
    assert_eq!(
        client
            .size_of(&remote, &AtomicBool::new(false))
            .await
            .unwrap(),
        payload.len() as u64
    );

    // Nothing is overwritten unless asked; asked, it is.
    assert!(matches!(
        client
            .upload(
                &local,
                "/home/uwu",
                false,
                &AtomicBool::new(false),
                &no_progress()
            )
            .await,
        Err(SftpError::AlreadyExists { .. })
    ));
    std::fs::write(local.join("inner").join("data.bin"), b"newer").unwrap();
    client
        .upload(
            &local,
            "/home/uwu",
            true,
            &AtomicBool::new(false),
            &no_progress(),
        )
        .await
        .unwrap();
    let uploaded = server
        .files
        .join("home/uwu")
        .join(folder_name)
        .join("inner/data.bin");
    assert_eq!(std::fs::read(&uploaded).unwrap(), b"newer");
    std::fs::write(local.join("inner").join("data.bin"), &payload).unwrap();
    client
        .upload(
            &local,
            "/home/uwu",
            true,
            &AtomicBool::new(false),
            &no_progress(),
        )
        .await
        .unwrap();

    // Down again, into another folder.
    let back = std::env::temp_dir().join(format!("uwussh-download-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&back).unwrap();
    let written = client
        .download(
            &remote,
            &back,
            false,
            &AtomicBool::new(false),
            &no_progress(),
        )
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(written.join("inner").join("data.bin")).unwrap(),
        payload
    );
    assert!(matches!(
        client
            .download(
                &remote,
                &back,
                false,
                &AtomicBool::new(false),
                &no_progress()
            )
            .await,
        Err(SftpError::AlreadyExists { .. })
    ));
    client
        .download(
            &remote,
            &back,
            true,
            &AtomicBool::new(false),
            &no_progress(),
        )
        .await
        .unwrap();

    // Rename, a new folder, and away with all of it.
    client.mkdir("/home/uwu/new").await.unwrap();
    assert!(matches!(
        client
            .rename("/home/uwu/new", "/home/uwu/welcome.txt")
            .await,
        Err(SftpError::AlreadyExists { .. })
    ));
    client
        .rename("/home/uwu/new", "/home/uwu/renamed")
        .await
        .unwrap();
    client.remove(&remote).await.unwrap();
    client.remove("/home/uwu/renamed").await.unwrap();
    let after: Vec<String> = client
        .list("/home/uwu")
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(after, vec!["welcome.txt"]);
    assert!(matches!(
        client.list("/home/uwu/missing").await,
        Err(SftpError::NotFound { .. })
    ));

    // A cancelled download leaves no half-written file behind.
    let cancelled = client
        .download(
            "/home/uwu/welcome.txt",
            &back,
            false,
            &AtomicBool::new(true),
            &no_progress(),
        )
        .await;
    assert!(matches!(cancelled, Err(SftpError::Cancelled)));
    assert!(!back.join("welcome.txt").exists());

    manager.close_files(id);
    for dir in [local, back, server.files] {
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn root_file_access_asks_sudo_on_a_raw_terminal_and_retries_on_the_same_login() {
    let server = start().await;
    let manager = SessionManager::new();

    let no_password = manager
        .open_files(
            "root",
            target(&server),
            Elevation::Root {
                sudo_password: None,
            },
        )
        .await;
    assert!(matches!(
        no_password,
        Err(FilesError::Sftp(SftpError::SudoPasswordRequired))
    ));

    let wrong = manager
        .open_files(
            "root",
            target(&server),
            Elevation::Root {
                sudo_password: Some(Zeroizing::new("guess".into())),
            },
        )
        .await;
    assert!(matches!(
        wrong,
        Err(FilesError::Sftp(SftpError::SudoPasswordRejected))
    ));

    let (id, home) = manager
        .open_files(
            "root",
            target(&server),
            Elevation::Root {
                sudo_password: Some(Zeroizing::new(SUDO_PASSWORD.into())),
            },
        )
        .await
        .unwrap();
    assert_eq!(home.as_deref(), Some("/"), "root starts at the top");
    let files = manager.files(id).unwrap();
    assert!(files.client.is_root());
    let etc = files.client.list("/etc").await.unwrap();
    assert!(etc.iter().any(|e| e.name == "secret.conf"));

    let log = server.log.lock();
    assert!(log.raw_pty, "the terminal was raw before sudo ran");
    assert_eq!(
        log.sudo_answers,
        vec!["guess".to_string(), SUDO_PASSWORD.to_string()],
        "sudo only ever got typed passwords, each at its own prompt"
    );
    drop(log);
    manager.close_files(id);
    let _ = std::fs::remove_dir_all(&server.files);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_terminal_session_finds_out_what_the_server_runs() {
    let server = start().await;
    let manager = SessionManager::new();
    let id = manager
        .spawn_ssh("tab", target(&server), 80, 24, Discard)
        .await
        .unwrap();
    let probe = manager.os_probe(id).expect("an ssh session can be probed");
    assert_eq!(probe.await, Some("ubuntu"));
    assert!(manager.is_ssh(id));
    manager.close(id).unwrap();
    let _ = std::fs::remove_dir_all(&server.files);
}
