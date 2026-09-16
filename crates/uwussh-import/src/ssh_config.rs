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
        SshConfigImporter::from_text(SAMPLE).import().expect("import")
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
        let result = SshConfigImporter::from_text("Host nas\n  User root\n").import().unwrap();
        assert_eq!(result.hosts[0].address, "nas");
    }

    #[test]
    fn equals_syntax_parses_too() {
        let result =
            SshConfigImporter::from_text("Host x\n  HostName=1.2.3.4\n  Port=2200\n").import().unwrap();
        assert_eq!(result.hosts[0].address, "1.2.3.4");
        assert_eq!(result.hosts[0].port, 2200);
    }

    #[test]
    fn keys_are_case_insensitive() {
        let result =
            SshConfigImporter::from_text("host x\n  HOSTNAME 5.6.7.8\n  uSeR root\n").import().unwrap();
        assert_eq!(result.hosts[0].address, "5.6.7.8");
        assert_eq!(result.hosts[0].username.as_deref(), Some("root"));
    }

    #[test]
    fn an_empty_config_is_an_error_not_an_empty_success() {
        assert!(SshConfigImporter::from_text("# nothing here\n").import().is_err());
    }
}
