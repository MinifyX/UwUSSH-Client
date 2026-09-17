//! A tiny SSH server for trying UwUSSH without a real host.
//!
//! ```text
//! cargo run -p uwussh-core --example dev_sshd
//! ```
//!
//! Listens on 127.0.0.1:2222, user `uwu`, password `nyu`. The shell is a toy —
//! `help`, `colors`, `flood <MiB>`, `sudo`, `exit` — but the SSH underneath is
//! real: key exchange, host key, password auth, a pty, window changes.
//!
//! It also accepts public-key auth for keys listed, one OpenSSH line each, in
//! the file named by `UWUSSH_DEV_SSHD_AUTHORIZED_KEYS` — which is how the
//! end-to-end run logs in with a key from the vault.
//!
//! Files: the `sftp` subsystem serves a folder (`UWUSSH_DEV_SSHD_FILES`, or
//! `uwussh-dev-sshd-files` in the temp directory) as the server's `/`, with a
//! home at `/home/uwu`. And any `exec` that asks `uname` answers like an
//! Ubuntu server, so the host list gets its icon.
//!
//! The host key is kept in the temp directory, so the fingerprint stays the
//! same between runs. Delete that file (the path is printed on start) to see
//! how UwUSSH reacts to a changed host key.

use russh::keys::ssh_key::LineEnding;
use russh::keys::{Algorithm, HashAlg, PrivateKey, PublicKey};
use russh::server::{self, Auth, Msg, Server as _, Session};
use russh::{Channel, ChannelId};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use uwussh_core::synthetic::LogLines;

#[path = "../tests/support/file_server.rs"]
mod file_server;
use file_server::FileServer;

const USER: &str = "uwu";
const PASSWORD: &str = "nyu";
const PROMPT: &str = "\x1b[38;5;211muwu\x1b[0m@\x1b[38;5;117mdev-sshd\x1b[0m:~$ ";
const SUDO_PROMPT: &str = "[sudo] password for uwu: ";

#[derive(Default)]
struct DevShell {
    line: String,
    /// `sudo` is waiting for a password, with this many tries left.
    sudo: Option<u8>,
    /// Public keys allowed to log in as `USER`. Shared by every client.
    authorized: Arc<Vec<PublicKey>>,
    files: Arc<PathBuf>,
    /// Channels opened but not yet claimed by a shell, an exec or SFTP.
    channels: HashMap<ChannelId, Channel<Msg>>,
    /// Channels running the toy shell. Only their data is typing: an SFTP
    /// channel's bytes go to the file server, and echoing them would break it.
    shells: std::collections::HashSet<ChannelId>,
}

impl server::Server for DevShell {
    type Handler = Self;
    fn new_client(&mut self, peer: Option<std::net::SocketAddr>) -> Self {
        println!("connection from {peer:?}");
        Self {
            authorized: Arc::clone(&self.authorized),
            files: Arc::clone(&self.files),
            ..Self::default()
        }
    }
}

impl server::Handler for DevShell {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        let accepted = user == USER && password == PASSWORD;
        println!(
            "password for {user}: {}",
            if accepted { "accepted" } else { "rejected" }
        );
        Ok(if accepted {
            Auth::Accept
        } else {
            Auth::Reject {
                proceed_with_methods: None,
                partial_success: false,
            }
        })
    }

    /// Called after russh has verified the client owns the key. Accept it if
    /// it is one of the authorized keys for `USER`.
    async fn auth_publickey(
        &mut self,
        user: &str,
        public_key: &PublicKey,
    ) -> Result<Auth, Self::Error> {
        let accepted = user == USER && self.authorized.iter().any(|key| key == public_key);
        println!(
            "publickey for {user}: {}",
            if accepted { "accepted" } else { "rejected" }
        );
        Ok(if accepted {
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
        term: &str,
        cols: u32,
        rows: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(russh::Pty, u32)],
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        println!("pty {term} {cols}x{rows}");
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
        println!("resize {cols}x{rows}");
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        // The toy shell answers through `data`, not through the channel.
        self.channels.remove(&channel);
        self.shells.insert(channel);
        let banner = format!(
            "\r\n  \x1b[1mdev-sshd\x1b[0m — a toy shell for trying UwUSSH\r\n  type \x1b[1mhelp\x1b[0m\r\n\r\n{PROMPT}"
        );
        session.data(channel, banner.into_bytes())?;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.remove(&channel);
        let command = String::from_utf8_lossy(data);
        println!("exec {command}");
        session.channel_success(channel)?;
        let (reply, status) = if command.contains("uname") {
            (
                "Linux\nPRETTY_NAME=\"Ubuntu 24.04 LTS\"\nNAME=\"Ubuntu\"\nID=ubuntu\nID_LIKE=debian\n\n",
                0,
            )
        } else {
            ("sh: command not found\n", 127)
        };
        session.data(channel, reply.as_bytes().to_vec())?;
        session.exit_status_request(channel, status)?;
        session.eof(channel)?;
        session.close(channel)?;
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        println!("subsystem {name}");
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

    async fn channel_eof(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.close(channel)?;
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if !self.shells.contains(&channel) {
            return Ok(());
        }
        let mut echo = Vec::new();
        for &byte in data {
            match byte {
                b'\r' if self.sudo.is_some() => {
                    let tries = self.sudo.take().unwrap_or(0);
                    let typed = std::mem::take(&mut self.line);
                    echo.extend_from_slice(b"\r\n");
                    if typed == PASSWORD {
                        println!("sudo: accepted");
                        echo.extend_from_slice(
                            format!("root access granted \u{2727}\r\n{PROMPT}").as_bytes(),
                        );
                    } else if tries > 1 {
                        println!("sudo: rejected");
                        echo.extend_from_slice(
                            format!("Sorry, try again.\r\n{SUDO_PROMPT}").as_bytes(),
                        );
                        self.sudo = Some(tries - 1);
                    } else {
                        echo.extend_from_slice(
                            format!("sudo: 3 incorrect password attempts\r\n{PROMPT}").as_bytes(),
                        );
                    }
                }
                b'\r' => {
                    echo.extend_from_slice(b"\r\n");
                    session.data(channel, std::mem::take(&mut echo))?;
                    let line = std::mem::take(&mut self.line);
                    if line.trim().starts_with("sudo") {
                        self.sudo = Some(3);
                        echo.extend_from_slice(SUDO_PROMPT.as_bytes());
                        continue;
                    }
                    if !run(line.trim(), channel, session)? {
                        return Ok(());
                    }
                }
                0x7f | 0x08 => {
                    if self.line.pop().is_some() && self.sudo.is_none() {
                        echo.extend_from_slice(b"\x08 \x08");
                    }
                }
                0x03 => {
                    self.line.clear();
                    self.sudo = None;
                    echo.extend_from_slice(format!("^C\r\n{PROMPT}").as_bytes());
                }
                byte if byte >= 0x20 => {
                    self.line.push(byte as char);
                    // A password prompt doesn't echo.
                    if self.sudo.is_none() {
                        echo.push(byte);
                    }
                }
                _ => {}
            }
        }
        if !echo.is_empty() {
            session.data(channel, echo)?;
        }
        Ok(())
    }
}

/// Run one command. Returns false when the session is over.
fn run(line: &str, channel: ChannelId, session: &mut Session) -> Result<bool, russh::Error> {
    let mut words = line.split_whitespace();
    let reply = match words.next() {
        None => String::new(),
        Some("help") => "  colors        a colour test\r\n  flood <MiB>   a lot of coloured log output (default 16)\r\n  sudo          asks for the password (nyu)\r\n  whoami        who you are\r\n  exit          end the session\r\n".into(),
        Some("whoami") => format!("{USER}\r\n"),
        Some("colors") => {
            let mut out = String::new();
            for code in 16..232u32 {
                out.push_str(&format!("\x1b[48;5;{code}m  "));
                if (code - 15) % 36 == 0 {
                    out.push_str("\x1b[0m\r\n");
                }
            }
            out.push_str("\x1b[0m");
            for step in 0..64u32 {
                let r = 255 - step * 4;
                let b = step * 4;
                out.push_str(&format!("\x1b[48;2;{r};80;{b}m "));
            }
            out.push_str("\x1b[0m\r\n");
            out
        }
        Some("flood") => {
            let mib: usize = words.next().and_then(|w| w.parse().ok()).unwrap_or(16);
            let handle = session.handle();
            tokio::spawn(async move {
                let mut lines = LogLines::new();
                let mut sent = 0;
                while sent < mib * 1024 * 1024 {
                    let mut chunk = Vec::with_capacity(64 * 1024 + 512);
                    while chunk.len() < 64 * 1024 {
                        lines.write_line(&mut chunk);
                    }
                    sent += chunk.len();
                    if handle.data(channel, chunk).await.is_err() {
                        return;
                    }
                }
                let _ = handle
                    .data(channel, format!("\r\n{mib} MiB sent\r\n{PROMPT}").into_bytes())
                    .await;
            });
            return Ok(true);
        }
        Some("exit") => {
            session.data(channel, b"bye\r\n".to_vec())?;
            session.exit_status_request(channel, 0)?;
            session.eof(channel)?;
            session.close(channel)?;
            return Ok(false);
        }
        Some(other) => format!("{other}: command not found (try help)\r\n"),
    };
    session.data(channel, format!("{reply}{PROMPT}").into_bytes())?;
    Ok(true)
}

fn host_key() -> PrivateKey {
    let path = std::env::temp_dir().join("uwussh-dev-sshd-host-ed25519");
    if let Ok(key) = russh::keys::load_secret_key(&path, None) {
        println!("host key: {}", path.display());
        return key;
    }
    let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).expect("key generation");
    std::fs::write(
        &path,
        key.to_openssh(LineEnding::LF).expect("encode").as_bytes(),
    )
    .expect("write host key");
    println!("host key: {} (new)", path.display());
    key
}

/// Public keys the server will accept, from the file named by
/// `UWUSSH_DEV_SSHD_AUTHORIZED_KEYS` (one OpenSSH line each). Absent means
/// password-only.
fn authorized_keys() -> Vec<PublicKey> {
    let Some(path) = std::env::var_os("UWUSSH_DEV_SSHD_AUTHORIZED_KEYS") else {
        return Vec::new();
    };
    let contents = std::fs::read_to_string(&path).unwrap_or_default();
    contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter_map(|line| PublicKey::from_openssh(line).ok())
        .collect()
}

/// The folder served over SFTP, with a home folder and a file to find.
fn files_root() -> PathBuf {
    let root = std::env::var_os("UWUSSH_DEV_SSHD_FILES")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("uwussh-dev-sshd-files"));
    let home = root.join("home").join("uwu");
    std::fs::create_dir_all(&home).expect("create the files folder");
    let welcome = home.join("welcome.txt");
    if !welcome.exists() {
        std::fs::write(&welcome, "hello from dev-sshd \u{2727}\n").expect("write welcome.txt");
    }
    std::fs::create_dir_all(root.join("etc")).expect("create etc");
    root
}

#[tokio::main]
async fn main() {
    let key = host_key();
    println!(
        "fingerprint: {}",
        key.public_key().fingerprint(HashAlg::Sha256)
    );

    let authorized = authorized_keys();
    println!("authorized keys: {}", authorized.len());
    let files = files_root();
    println!("files: {}", files.display());

    let config = Arc::new(server::Config {
        auth_rejection_time: Duration::from_millis(300),
        auth_rejection_time_initial: Some(Duration::ZERO),
        keys: vec![key],
        ..Default::default()
    });

    let listener = TcpListener::bind(("127.0.0.1", 2222))
        .await
        .expect("port 2222 is taken");
    println!("listening on 127.0.0.1:2222 — user {USER}, password {PASSWORD}");

    DevShell {
        authorized: Arc::new(authorized),
        files: Arc::new(files),
        ..DevShell::default()
    }
    .run_on_socket(config, &listener)
    .await
    .expect("server stopped");
}
