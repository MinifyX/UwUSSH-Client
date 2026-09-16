//! Getting an existing setup into UwUSSH.
//!
//! Nobody retypes eighty hosts, so this is a milestone-1 crate, not a
//! someday one.
//!
//! Every source is an [`Importer`] that produces [`ImportedHost`] values.
//! Everything after that — preview, duplicate detection, group assignment,
//! writing — is shared, which is why adding MobaXterm later costs an adapter
//! and not a second pipeline.

pub mod putty;
pub mod ssh_config;

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("could not read the source: {0}")]
    Read(String),
    #[error("nothing importable found")]
    Empty,
}

pub type Result<T> = std::result::Result<T, ImportError>;

/// Where a host came from, so the preview can say "47 from PuTTY, 3 from
/// ssh_config" instead of one anonymous pile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Putty,
    Kitty,
    SshConfig,
    Termius,
    WinScp,
    MRemoteNg,
    MobaXterm,
}

/// The neutral shape every importer targets.
///
/// Deliberately not [`uwussh_proto::Host`]: imported data is messy, half of it
/// needs a decision from the user, and none of it has ids yet. Converting
/// happens once, after the preview, in one place.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedHost {
    pub name: String,
    pub address: String,
    pub port: u16,
    pub username: Option<String>,
    /// Path to a key file as the source recorded it. `.ppk` files still need
    /// converting; that happens after the user picks what to import.
    pub key_path: Option<String>,
    pub jump_host: Option<String>,
    /// Folder or group path the source had, `/`-separated.
    pub group_path: Option<String>,
    pub remote_command: Option<String>,
    pub charset: Option<String>,
    /// Anything recognised but not yet mapped, kept so an import never silently
    /// throws away settings someone cared enough to configure.
    #[serde(default)]
    pub extras: Vec<(String, String)>,
}

impl ImportedHost {
    /// How duplicates are detected across sources.
    pub fn dedupe_key(&self) -> String {
        format!("{}:{}", self.address.to_ascii_lowercase(), self.port)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImportResult {
    pub hosts: Vec<ImportedHost>,
    /// Entries that were found but could not be understood, with a reason. The
    /// preview shows these: a silent partial import is worse than a visible
    /// incomplete one.
    pub skipped: Vec<(String, String)>,
}

pub trait Importer {
    fn source(&self) -> Source;
    fn import(&self) -> Result<ImportResult>;
}

/// Merge several imports, keeping the first occurrence of each host.
pub fn deduplicate(results: Vec<ImportResult>) -> ImportResult {
    let mut seen = std::collections::HashSet::new();
    let mut merged = ImportResult::default();

    for result in results {
        for host in result.hosts {
            if seen.insert(host.dedupe_key()) {
                merged.hosts.push(host);
            }
        }
        merged.skipped.extend(result.skipped);
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(name: &str, address: &str, port: u16) -> ImportedHost {
        ImportedHost {
            name: name.into(),
            address: address.into(),
            port,
            ..Default::default()
        }
    }

    #[test]
    fn the_same_host_from_two_sources_lands_once() {
        let a = ImportResult {
            hosts: vec![host("prox", "10.0.0.12", 22)],
            skipped: vec![],
        };
        let b = ImportResult {
            hosts: vec![host("proxmox", "10.0.0.12", 22)],
            skipped: vec![],
        };

        let merged = deduplicate(vec![a, b]);
        assert_eq!(merged.hosts.len(), 1);
        assert_eq!(merged.hosts[0].name, "prox", "first source wins");
    }

    #[test]
    fn a_different_port_is_a_different_host() {
        let a = ImportResult {
            hosts: vec![host("web", "10.0.0.5", 22)],
            skipped: vec![],
        };
        let b = ImportResult {
            hosts: vec![host("web-alt", "10.0.0.5", 2222)],
            skipped: vec![],
        };
        assert_eq!(deduplicate(vec![a, b]).hosts.len(), 2);
    }

    #[test]
    fn dedupe_ignores_hostname_case() {
        assert_eq!(
            host("a", "Prox-1.lan", 22).dedupe_key(),
            host("b", "prox-1.lan", 22).dedupe_key()
        );
    }
}
