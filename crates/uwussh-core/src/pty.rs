//! Local PTY sessions.
//!
//! On Windows this is ConPTY, elsewhere a normal pty, both behind
//! `portable-pty`.
//!
//! One Windows quirk shapes this file: ConPTY keeps its output pipe open after
//! the child process has exited, so the reader never sees EOF on its own. The
//! end of a session is therefore detected by waiting on the child, not on the
//! pipe, and the pipe only closes once the session — and with it the pseudo
//! console — is dropped.

use crate::flow::FlowControl;
use crate::metrics::{Metrics, MetricsSnapshot};
use crate::stream::{self, FrameSink};
use crate::{CoreError, Result};
use parking_lot::Mutex;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Size of one read from the pty. Large enough that a flood does not turn into
/// a syscall storm, small enough that an interactive line arrives immediately.
const READ_BUFFER: usize = 64 * 1024;

pub struct PtySession {
    /// `Box<dyn MasterPty + Send>` is `Send` but not `Sync`, and the session
    /// map is shared across threads — so the handle lives behind a lock. It is
    /// only touched on resize, so the lock is never contended.
    master: Mutex<Box<dyn MasterPty + Send>>,
    /// Keystrokes go through one queue to one writer thread: they arrive in
    /// the order they were typed, and a program that stops reading its input
    /// blocks that thread instead of the UI.
    input: std::sync::mpsc::Sender<Vec<u8>>,
    /// The child itself lives on the watcher thread, blocked in `wait()`; this
    /// is the handle that can still end it from here.
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    metrics: Arc<Metrics>,
    flow: Arc<FlowControl>,
}

impl PtySession {
    /// Start the user's shell the way the system's own terminal does: on
    /// macOS as a login shell, everywhere with the environment a terminal is
    /// expected to set (see [`shell_env`]).
    pub fn spawn_shell<S: FrameSink>(cols: u16, rows: u16, sink: S) -> Result<Self> {
        let mut cmd = shell_command();
        let get = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
        for (name, value) in shell_env(&get, system_utf8_locale) {
            match value {
                Some(value) => cmd.env(name, value),
                None => cmd.env_remove(name),
            }
        }
        Self::spawn(cmd, cols, rows, true, sink)
    }

    /// Run one program in a pty — for M0, `type bigfile` as the realistic
    /// "someone cats a huge log" case.
    pub fn spawn_command<S: FrameSink>(
        program: &str,
        args: &[String],
        cols: u16,
        rows: u16,
        flow_control: bool,
        sink: S,
    ) -> Result<Self> {
        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);
        Self::spawn(cmd, cols, rows, flow_control, sink)
    }

    fn spawn<S: FrameSink>(
        cmd: CommandBuilder,
        cols: u16,
        rows: u16,
        flow_control: bool,
        sink: S,
    ) -> Result<Self> {
        let pair = native_pty_system()
            .openpty(size(cols, rows))
            .map_err(CoreError::Pty)?;

        let mut child = pair.slave.spawn_command(cmd).map_err(CoreError::Pty)?;
        let killer = child.clone_killer();

        // Drop the slave handle: holding it would keep our own end of the pty
        // open after the child is gone.
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().map_err(CoreError::Pty)?;
        let mut writer = pair.master.take_writer().map_err(CoreError::Pty)?;

        let (input, inputs) = std::sync::mpsc::channel::<Vec<u8>>();
        std::thread::Builder::new()
            .name("uwussh-pty-writer".into())
            .spawn(move || {
                for bytes in inputs {
                    if writer
                        .write_all(&bytes)
                        .and_then(|()| writer.flush())
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .map_err(CoreError::Io)?;

        let metrics = Arc::new(Metrics::new());
        let flow = Arc::new(FlowControl::new(flow_control));
        let (tx, rx) = mpsc::channel::<Vec<u8>>(stream::CHANNEL_CAPACITY);

        let watcher_metrics = Arc::clone(&metrics);
        std::thread::Builder::new()
            .name("uwussh-pty-child".into())
            .spawn(move || {
                let status = child.wait();
                tracing::debug!(?status, "pty child exited");
                watcher_metrics.mark_child_exited();
            })
            .map_err(CoreError::Io)?;

        // The reader has to be a real thread: pty reads are blocking, and
        // parking a tokio worker on one would starve everything else.
        let reader_metrics = Arc::clone(&metrics);
        std::thread::Builder::new()
            .name("uwussh-pty-reader".into())
            .spawn(move || {
                let mut buf = vec![0u8; READ_BUFFER];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            // A full queue means everything downstream is
                            // behind. Blocking here is the point: backpressure
                            // travels back to the program, as on a real tty.
                            if tx.capacity() == 0 {
                                reader_metrics.record_stall();
                            }
                            if tx.blocking_send(buf[..n].to_vec()).is_err() {
                                break;
                            }
                        }
                        Err(err) => {
                            tracing::debug!(?err, "pty reader finished");
                            break;
                        }
                    }
                }
            })
            .map_err(CoreError::Io)?;

        tokio::spawn(stream::run_batcher(
            rx,
            sink,
            Arc::clone(&metrics),
            Arc::clone(&flow),
        ));

        Ok(Self {
            master: Mutex::new(pair.master),
            input,
            killer: Mutex::new(killer),
            metrics,
            flow,
        })
    }

    pub fn write(&self, data: &[u8]) -> Result<()> {
        self.input
            .send(data.to_vec())
            .map_err(|_| CoreError::SessionClosed)
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.master
            .lock()
            .resize(size(cols, rows))
            .map_err(CoreError::Pty)
    }

    pub fn metrics(&self) -> MetricsSnapshot {
        self.metrics.snapshot(&self.flow)
    }

    pub fn ack(&self, bytes: u64) {
        self.flow.ack(bytes);
    }

    pub fn close(&self) {
        // A dead child is not an error here — the user may simply have typed
        // `exit` before hitting the close button.
        let _ = self.killer.lock().kill();
        self.flow.close();
    }
}

/// Never a zero-sized pty: zsh's line editor and full-screen programs divide
/// by the width, and a 0×0 terminal is what a not-yet-laid-out view reports.
fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows: rows.max(1),
        cols: cols.max(1),
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// PowerShell on Windows rather than whatever `COMSPEC` points at.
///
/// On macOS a login shell, like Terminal.app and iTerm2 start: only a login
/// zsh reads `/etc/zprofile` (`path_helper`) and `~/.zprofile` (Homebrew's
/// `shellenv`). An app started from the Finder gets launchd's bare
/// `/usr/bin:/bin:/usr/sbin:/sbin`, so without it `brew`, `git` from Homebrew
/// and everything in `/usr/local/bin` were missing. portable-pty's default
/// program is exactly that: `$SHELL` (or the passwd entry) with `-zsh` as
/// argv0, started in the home folder.
///
/// On Linux the shell starts like in GNOME Terminal or Konsole: not as a login
/// shell, from `$SHELL`.
fn shell_command() -> CommandBuilder {
    if cfg!(windows) {
        CommandBuilder::new("powershell.exe")
    } else if cfg!(target_os = "macos") {
        CommandBuilder::new_default_prog()
    } else {
        CommandBuilder::new(std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into()))
    }
}

/// What the local shell's environment gets on top of the app's own: `Some`
/// sets a variable, `None` removes it.
///
/// An app started from the Finder (or from a desktop menu on Linux) has no
/// terminal around it, so the environment it hands on has no `TERM` and, on
/// macOS, no `LANG` either. Without `TERM` zsh's line editor falls back to a
/// dumb terminal: arrow keys and Backspace print escape sequences, the prompt
/// redraws in the wrong place, `clear`, `less` and `vim` misbehave. Without a
/// UTF-8 locale zsh and bash treat an umlaut as two unknown bytes, so typing
/// `ä` garbles the line. Starting the app from a terminal hides both, because
/// it then inherits that terminal's variables — which is why this only showed
/// on a Mac started from the Dock.
///
/// `get` reads the app's environment (empty counts as unset); `locale` names
/// a UTF-8 locale for this system and is only asked when one is missing.
pub(crate) fn shell_env(
    get: &dyn Fn(&str) -> Option<String>,
    locale: impl FnOnce() -> String,
) -> Vec<(&'static str, Option<String>)> {
    let mut env = vec![
        // What xterm.js understands; whatever terminal started the app (tmux,
        // screen, a Linux console) must not leak through.
        ("TERM", Some("xterm-256color".to_string())),
        ("COLORTERM", Some("truecolor".to_string())),
        ("TERM_PROGRAM", Some("UwUSSH".to_string())),
        (
            "TERM_PROGRAM_VERSION",
            Some(env!("CARGO_PKG_VERSION").to_string()),
        ),
        // Started from inside tmux or screen, the shell would believe it
        // still runs there.
        ("TMUX", None),
        ("TMUX_PANE", None),
        ("STY", None),
    ];

    if cfg!(windows) {
        return env;
    }
    let is_utf8 = |value: &str| {
        let lower = value.to_ascii_lowercase();
        lower.contains("utf-8") || lower.contains("utf8")
    };
    // LC_ALL beats everything; someone who set it meant it.
    if get("LC_ALL").is_some() {
        return env;
    }
    let ctype = get("LC_CTYPE").or_else(|| get("LANG"));
    if ctype.as_deref().is_some_and(is_utf8) {
        return env;
    }
    let locale = locale();
    if get("LANG").is_none() {
        env.push(("LANG", Some(locale.clone())));
    }
    env.push(("LC_CTYPE", Some(locale)));
    env
}

/// A UTF-8 locale that exists on this system.
fn system_utf8_locale() -> String {
    if cfg!(target_os = "macos") {
        // The region and language from System Settings, as Terminal.app uses
        // it: `de_DE`, `en_DE`, `de_DE@rg=atzzzz`.
        let apple = std::process::Command::new("/usr/bin/defaults")
            .args(["read", "-g", "AppleLocale"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok());
        pick_utf8_locale(apple.as_deref(), |name| {
            std::path::Path::new("/usr/share/locale")
                .join(name)
                .is_dir()
        })
    } else {
        // glibc 2.35+ and musl always have it.
        "C.UTF-8".to_string()
    }
}

/// The best `xx_YY.UTF-8` for a macOS `AppleLocale`: the exact one, else the
/// language in its home region (`en_DE` → `en_US` is not guessable, but
/// `de_LU` → `de_DE` is), else `en_US.UTF-8`, which every Mac has.
fn pick_utf8_locale(apple_locale: Option<&str>, exists: impl Fn(&str) -> bool) -> String {
    const FALLBACK: &str = "en_US.UTF-8";
    let Some(apple) = apple_locale else {
        return FALLBACK.into();
    };
    let base = apple.trim().split('@').next().unwrap_or_default();
    let mut parts = base.split(['_', '-']);
    let language = parts.next().unwrap_or_default().to_ascii_lowercase();
    if language.len() < 2 || !language.chars().all(|c| c.is_ascii_alphabetic()) {
        return FALLBACK.into();
    }
    let region = parts
        .find(|part| part.len() == 2 && part.chars().all(|c| c.is_ascii_alphabetic()))
        .map(str::to_ascii_uppercase);

    let mut candidates = Vec::new();
    if let Some(region) = region {
        candidates.push(format!("{language}_{region}.UTF-8"));
    }
    candidates.push(format!(
        "{language}_{}.UTF-8",
        language.to_ascii_uppercase()
    ));
    if language == "en" {
        candidates.push(FALLBACK.into());
    }
    candidates
        .into_iter()
        .find(|candidate| exists(candidate))
        .unwrap_or_else(|| FALLBACK.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_of(vars: &[(&str, &str)]) -> HashMap<&'static str, Option<String>> {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        let get = move |name: &str| vars.get(name).cloned();
        shell_env(&get, || "de_DE.UTF-8".into())
            .into_iter()
            .collect()
    }

    #[test]
    fn a_shell_started_from_the_dock_still_gets_a_terminal_type() {
        let env = env_of(&[]);
        assert_eq!(env["TERM"].as_deref(), Some("xterm-256color"));
        assert_eq!(env["COLORTERM"].as_deref(), Some("truecolor"));
        assert_eq!(env["TERM_PROGRAM"].as_deref(), Some("UwUSSH"));
    }

    #[test]
    fn the_terminal_the_app_was_started_from_does_not_leak() {
        let env = env_of(&[
            ("TERM", "screen-256color"),
            ("TMUX", "/tmp/tmux-501/default,1,0"),
        ]);
        assert_eq!(env["TERM"].as_deref(), Some("xterm-256color"));
        assert_eq!(env["TMUX"], None);
        assert!(env.contains_key("TMUX"), "TMUX must be removed explicitly");
    }

    #[cfg(unix)]
    #[test]
    fn a_missing_locale_becomes_a_utf8_one() {
        let env = env_of(&[]);
        assert_eq!(env["LANG"].as_deref(), Some("de_DE.UTF-8"));
        assert_eq!(env["LC_CTYPE"].as_deref(), Some("de_DE.UTF-8"));
    }

    #[cfg(unix)]
    #[test]
    fn a_non_utf8_lang_keeps_its_language_but_gets_utf8_characters() {
        let env = env_of(&[("LANG", "C")]);
        assert!(!env.contains_key("LANG"));
        assert_eq!(env["LC_CTYPE"].as_deref(), Some("de_DE.UTF-8"));
    }

    #[test]
    fn a_utf8_locale_or_lc_all_is_left_alone() {
        for vars in [
            &[("LANG", "en_GB.UTF-8")][..],
            &[("LANG", "C"), ("LC_CTYPE", "de_DE.utf8")][..],
            &[("LC_ALL", "C")][..],
        ] {
            let env = env_of(vars);
            assert!(!env.contains_key("LANG"), "{vars:?}");
            assert!(!env.contains_key("LC_CTYPE"), "{vars:?}");
        }
    }

    #[test]
    fn the_mac_region_picks_an_existing_locale() {
        let mac = [
            "de_DE.UTF-8",
            "de_AT.UTF-8",
            "en_US.UTF-8",
            "en_GB.UTF-8",
            "fr_FR.UTF-8",
        ];
        let exists = |name: &str| mac.contains(&name);
        assert_eq!(pick_utf8_locale(Some("de_DE\n"), exists), "de_DE.UTF-8");
        assert_eq!(
            pick_utf8_locale(Some("de_AT@rg=atzzzz"), exists),
            "de_AT.UTF-8"
        );
        assert_eq!(pick_utf8_locale(Some("de_LU"), exists), "de_DE.UTF-8");
        assert_eq!(pick_utf8_locale(Some("fr-CA"), exists), "fr_FR.UTF-8");
        assert_eq!(pick_utf8_locale(Some("en_DE"), exists), "en_US.UTF-8");
        assert_eq!(pick_utf8_locale(Some("xx"), exists), "en_US.UTF-8");
        assert_eq!(pick_utf8_locale(Some(""), exists), "en_US.UTF-8");
        assert_eq!(pick_utf8_locale(None, exists), "en_US.UTF-8");
    }

    #[test]
    fn a_pty_is_never_zero_sized() {
        let size = size(0, 0);
        assert_eq!((size.cols, size.rows), (1, 1));
    }
}
