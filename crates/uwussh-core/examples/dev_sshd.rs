//! A tiny SSH server for trying UwUSSH without a real host.
//!
//! ```text
//! cargo run -p uwussh-core --example dev_sshd
//! ```
//!
//! Listens on 127.0.0.1:2222, user `uwu`, password `nyu`. The shell is a toy —
//! `help`, `colors`, `flood <MiB>`, `exit` — but the SSH underneath is real:
//! key exchange, host key, password auth, a pty, window changes.
//!
//! The host key is kept in the temp directory, so the fingerprint stays the
//! same between runs. Delete that file (the path is printed on start) to see
//! how UwUSSH reacts to a changed host key.

use russh::keys::ssh_key::LineEnding;
use russh::keys::{Algorithm, HashAlg, PrivateKey};
use russh::server::{self, Auth, Msg, Server as _, Session};
use russh::{Channel, ChannelId};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use uwussh_core::synthetic::LogLines;

const USER: &str = "uwu";
const PASSWORD: &str = "nyu";
const PROMPT: &str = "\x1b[38;5;211muwu\x1b[0m@\x1b[38;5;117mdev-sshd\x1b[0m:~$ ";

#[derive(Clone, Default)]
struct DevShell {
    line: String,
}

impl server::Server for DevShell {
    type Handler = Self;
    fn new_client(&mut self, peer: Option<std::net::SocketAddr>) -> Self {
        println!("connection from {peer:?}");
        Self::default()
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
        let banner = format!(
            "\r\n  \x1b[1mdev-sshd\x1b[0m — a toy shell for trying UwUSSH\r\n  type \x1b[1mhelp\x1b[0m\r\n\r\n{PROMPT}"
        );
        session.data(channel, banner.into_bytes())?;
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let mut echo = Vec::new();
        for &byte in data {
            match byte {
                b'\r' => {
                    echo.extend_from_slice(b"\r\n");
                    session.data(channel, std::mem::take(&mut echo))?;
                    let line = std::mem::take(&mut self.line);
                    if !run(line.trim(), channel, session)? {
                        return Ok(());
                    }
                }
                0x7f | 0x08 => {
                    if self.line.pop().is_some() {
                        echo.extend_from_slice(b"\x08 \x08");
                    }
                }
                0x03 => {
                    self.line.clear();
                    echo.extend_from_slice(format!("^C\r\n{PROMPT}").as_bytes());
                }
                byte if byte >= 0x20 => {
                    self.line.push(byte as char);
                    echo.push(byte);
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
        Some("help") => "  colors        a colour test\r\n  flood <MiB>   a lot of coloured log output (default 16)\r\n  whoami        who you are\r\n  exit          end the session\r\n".into(),
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

#[tokio::main]
async fn main() {
    let key = host_key();
    println!(
        "fingerprint: {}",
        key.public_key().fingerprint(HashAlg::Sha256)
    );

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

    DevShell::default()
        .run_on_socket(config, &listener)
        .await
        .expect("server stopped");
}
