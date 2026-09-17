//! `~/.ssh/config`.
//!
//! Only the parts that describe a host worth importing. Two rules matter and
//! are easy to get wrong:
//!
//! * Keys are **case-insensitive** (`HostName`, `hostname` and `HOSTNAME` are
//!   the same option), values are not.
//! * A `Host` line with wildcards (`Host *`, `Host *.lan`) is a rule, not a
//!   machine. Importing those produces hosts nobody can connect to, so they
//!   are skipped and reported rather than silently turned into junk entries.

use crate::{ImportError, ImportResult, ImportedHost, Importer, Result, Source};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub struct SshConfigImporter {
    text: String,
}

impl SshConfigImporter {
    pub fn from_text(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    pub fn from_path(path: &std::path::Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| ImportError::Read(e.to_string()))?;
        Ok(Self::from_text(text))
    }
}

/// `~/.ssh/config`, on Windows too — or wherever `UWUSSH_SSH_CONFIG` points,
/// which the end-to-end run uses to import from a fixture instead of the real
/// file.
pub fn default_config_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("UWUSSH_SSH_CONFIG") {
        return Some(PathBuf::from(path));
    }
    home_dir().map(|home| home.join(".ssh").join("config"))
}

/// Whether there is a config worth importing.
pub fn has_config() -> bool {
    default_config_path().is_some_and(|path| path.is_file())
}

/// Read the user's `~/.ssh/config`, following `Include` directives, and import
/// every real host. A missing file is not an error — there is simply nothing to
/// import.
pub fn read_default() -> Result<ImportResult> {
    match default_config_path() {
        Some(path) if path.is_file() => read_file(&path),
        _ => Ok(ImportResult::default()),
    }
}

/// Read one config file and everything it includes, then import.
pub fn read_file(path: &Path) -> Result<ImportResult> {
    // Relative includes resolve against the directory of the top config, which
    // is how OpenSSH treats the user config's `~/.ssh`.
    let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut lines = Vec::new();
    let mut skipped = Vec::new();
    let mut visited = HashSet::new();
    expand(path, &base, 0, &mut lines, &mut skipped, &mut visited);

    let mut result = match SshConfigImporter::from_text(lines.join("\n")).import() {
        Ok(result) => result,
        // No hosts in the file is "nothing to import", not a failure, once we
        // are reading a real file rather than a caller-supplied string.
        Err(ImportError::Empty) => ImportResult::default(),
        Err(other) => return Err(other),
    };
    result.skipped.extend(skipped);
    Ok(result)
}

/// How deep `Include` nesting is followed; also the guard against a cycle.
const MAX_INCLUDE_DEPTH: usize = 16;

/// Read `path` into `lines`, splicing each `Include`d file in at the point it
/// appears — the order OpenSSH applies them in.
fn expand(
    path: &Path,
    base: &Path,
    depth: usize,
    lines: &mut Vec<String>,
    skipped: &mut Vec<(String, String)>,
    visited: &mut HashSet<PathBuf>,
) {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if depth > MAX_INCLUDE_DEPTH || !visited.insert(canonical) {
        return;
    }
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(_) => {
            skipped.push((path.display().to_string(), "could not be read".into()));
            return;
        }
    };

    for raw in text.lines() {
        let line = raw.trim();
        if let Some(rest) = include_directive(line) {
            for pattern in rest.split_whitespace() {
                match resolve_include(pattern, base) {
                    Ok(files) if files.is_empty() => {}
                    Ok(files) => {
                        for file in files {
                            expand(&file, base, depth + 1, lines, skipped, visited);
                        }
                    }
                    Err(reason) => skipped.push((format!("Include {pattern}"), reason.into())),
                }
            }
        } else {
            lines.push(raw.to_string());
        }
    }
}

/// The tail of an `Include` line, or `None` if this is not one.
fn include_directive(line: &str) -> Option<&str> {
    let (key, value) = split_option(line)?;
    key.eq_ignore_ascii_case("include").then_some(value)
}

/// Resolve one `Include` pattern to the files it matches, sorted. Supports a
/// `*`/`?` wildcard in the final path component, which is the common
/// `config.d/*` case.
fn resolve_include(pattern: &str, base: &Path) -> std::result::Result<Vec<PathBuf>, &'static str> {
    // Reading `\\server\share\…` makes Windows log in to that server with the
    // user's password hash; a config file must not be able to trigger that.
    let bytes = pattern.as_bytes();
    if bytes.len() >= 2 && matches!(bytes[0], b'\\' | b'/') && matches!(bytes[1], b'\\' | b'/') {
        return Err("network paths are not followed");
    }
    let expanded = expand_home(pattern);
    let full = if expanded.is_absolute() {
        expanded
    } else {
        base.join(expanded)
    };

    let name = full
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("has no file name")?;

    if !name.contains(['*', '?']) {
        return Ok(if full.is_file() {
            vec![full]
        } else {
            Vec::new()
        });
    }
    if full
        .parent()
        .is_some_and(|p| p.to_string_lossy().contains(['*', '?']))
    {
        return Err("wildcards in a directory name are not supported");
    }

    let dir = full.parent().unwrap_or(Path::new("."));
    let mut matches: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|_| "directory could not be read")?
        .flatten()
        .filter(|entry| entry.path().is_file())
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|file| wildcard_match(name, file))
        })
        .map(|entry| entry.path())
        .collect();
    matches.sort();
    Ok(matches)
}

/// A minimal `*`/`?` glob match for one file name.
fn wildcard_match(pattern: &str, name: &str) -> bool {
    fn go(p: &[u8], n: &[u8]) -> bool {
        match p.first() {
            None => n.is_empty(),
            Some(b'*') => go(&p[1..], n) || (!n.is_empty() && go(p, &n[1..])),
            Some(b'?') => !n.is_empty() && go(&p[1..], &n[1..]),
            Some(&c) => n.first() == Some(&c) && go(&p[1..], &n[1..]),
        }
    }
    go(pattern.as_bytes(), name.as_bytes())
}

fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        Some(rest) => match home_dir() {
            Some(home) => home.join(rest),
            None => PathBuf::from(path),
        },
        None => PathBuf::from(path),
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

impl Importer for SshConfigImporter {
    fn source(&self) -> Source {
        Source::SshConfig
    }

    fn import(&self) -> Result<ImportResult> {
        let mut result = ImportResult::default();
        let mut current: Option<ImportedHost> = None;

        for raw in self.text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let Some((key, value)) = split_option(line) else {
                continue;
            };

            if key.eq_ignore_ascii_case("host") {
                if let Some(host) = current.take() {
                    push(&mut result, host);
                }
                // `Host a b c` declares several aliases; the first is the name.
                let Some(name) = value.split_whitespace().next() else {
                    continue;
                };

                if name.contains('*') || name.contains('?') {
                    result
                        .skipped
                        .push((name.to_string(), "pattern, not a host".into()));
                    continue;
                }

                current = Some(ImportedHost {
                    name: name.to_string(),
                    // Without a HostName the alias *is* the address.
                    address: name.to_string(),
                    port: 22,
                    ..Default::default()
                });
                continue;
            }

            let Some(host) = current.as_mut() else {
                continue;
            };

            match key.to_ascii_lowercase().as_str() {
                "hostname" => host.address = value.to_string(),
                "port" => {
                    if let Ok(port) = value.parse() {
                        host.port = port;
                    }
                }
                "user" => host.username = Some(value.to_string()),
                "identityfile" => host.key_path = Some(strip_quotes(value)),
                "proxyjump" => host.jump_host = Some(value.to_string()),
                "remotecommand" => host.remote_command = Some(value.to_string()),
                other => host.extras.push((other.to_string(), value.to_string())),
            }
        }

        if let Some(host) = current.take() {
            push(&mut result, host);
        }

        if result.hosts.is_empty() {
            return Err(ImportError::Empty);
        }
        Ok(result)
    }
}

fn push(result: &mut ImportResult, host: ImportedHost) {
    if host.address.trim().is_empty() {
        result.skipped.push((host.name, "no address".into()));
    } else {
        result.hosts.push(host);
    }
}

/// `Key value` and `Key=value` are both legal.
fn split_option(line: &str) -> Option<(&str, &str)> {
    if let Some((key, value)) = line.split_once('=') {
        return Some((key.trim(), value.trim()));
    }
    let mut parts = line.splitn(2, char::is_whitespace);
    let key = parts.next()?.trim();
    let value = parts.next()?.trim();
    Some((key, value))
}

fn strip_quotes(value: &str) -> String {
    value.trim_matches('"').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
# homelab
Host prox-1
    HostName 10.0.0.12
    User root
    Port 22
    IdentityFile ~/.ssh/id_ed25519

Host db-01
    HostName db-01.internal
    User lorin
    ProxyJump edge-bastion
    Port 2222

Host *.lan
    User admin

Host edge-bastion
    HostName bastion.example.com
"#;

    fn imported() -> ImportResult {
        SshConfigImporter::from_text(SAMPLE)
            .import()
            .expect("import")
    }

    #[test]
    fn reads_every_real_host() {
        let names: Vec<_> = imported().hosts.iter().map(|h| h.name.clone()).collect();
        assert_eq!(names, vec!["prox-1", "db-01", "edge-bastion"]);
    }

    #[test]
    fn wildcards_are_skipped_and_reported() {
        let result = imported();
        assert!(result.hosts.iter().all(|h| !h.name.contains('*')));
        assert!(result.skipped.iter().any(|(name, _)| name == "*.lan"));
    }

    #[test]
    fn proxy_jump_is_kept() {
        let result = imported();
        let db = result.hosts.iter().find(|h| h.name == "db-01").unwrap();
        assert_eq!(db.jump_host.as_deref(), Some("edge-bastion"));
        assert_eq!(db.port, 2222);
    }

    #[test]
    fn an_alias_without_hostname_uses_itself_as_address() {
        let result = SshConfigImporter::from_text("Host nas\n  User root\n")
            .import()
            .unwrap();
        assert_eq!(result.hosts[0].address, "nas");
    }

    #[test]
    fn equals_syntax_parses_too() {
        let result = SshConfigImporter::from_text("Host x\n  HostName=1.2.3.4\n  Port=2200\n")
            .import()
            .unwrap();
        assert_eq!(result.hosts[0].address, "1.2.3.4");
        assert_eq!(result.hosts[0].port, 2200);
    }

    #[test]
    fn keys_are_case_insensitive() {
        let result = SshConfigImporter::from_text("host x\n  HOSTNAME 5.6.7.8\n  uSeR root\n")
            .import()
            .unwrap();
        assert_eq!(result.hosts[0].address, "5.6.7.8");
        assert_eq!(result.hosts[0].username.as_deref(), Some("root"));
    }

    #[test]
    fn an_empty_config_is_an_error_not_an_empty_success() {
        assert!(SshConfigImporter::from_text("# nothing here\n")
            .import()
            .is_err());
    }

    #[test]
    fn wildcard_matches_stars_and_question_marks() {
        assert!(wildcard_match("*", "anything"));
        assert!(wildcard_match("*.conf", "prod.conf"));
        assert!(wildcard_match("host?", "host9"));
        assert!(wildcard_match("a*b*c", "axxbyyc"));
        assert!(!wildcard_match("*.conf", "conf.bak"));
        assert!(!wildcard_match("host?", "host10"));
    }

    #[test]
    fn include_pulls_in_other_files_at_that_point() {
        let dir = std::env::temp_dir().join(format!("uwussh-sshcfg-{}", std::process::id()));
        let confd = dir.join("config.d");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&confd).unwrap();

        std::fs::write(
            dir.join("config"),
            "Host top\n  HostName 10.0.0.1\n\nInclude config.d/*.conf\n",
        )
        .unwrap();
        std::fs::write(
            confd.join("10-work.conf"),
            "Host work\n  HostName work.example.com\n  User lorin\n",
        )
        .unwrap();
        std::fs::write(
            confd.join("20-home.conf"),
            "Host nas\n  HostName 10.0.0.9\n",
        )
        .unwrap();
        // Not matched by *.conf, so it must not be imported.
        std::fs::write(confd.join("notes.txt"), "Host ghost\n  HostName ghost\n").unwrap();

        let result = read_file(&dir.join("config")).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();

        let names: Vec<_> = result.hosts.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(
            names,
            ["top", "work", "nas"],
            "top first, then the sorted include"
        );
        assert!(result.hosts.iter().all(|h| h.name != "ghost"));
    }

    #[test]
    fn a_missing_config_is_empty_not_an_error() {
        let result = read_file(Path::new("/uwussh/definitely/no/such/config")).unwrap();
        assert!(result.hosts.is_empty());
    }

    #[test]
    fn an_unreadable_include_is_reported() {
        let dir = std::env::temp_dir().join(format!("uwussh-sshcfg-miss-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config"),
            "Include gone.d/*\nHost real\n  HostName 1.2.3.4\n",
        )
        .unwrap();

        let result = read_file(&dir.join("config")).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        // The one real host still imports; the missing include is just absent.
        assert_eq!(result.hosts.len(), 1);
        assert_eq!(result.hosts[0].name, "real");
    }
}
