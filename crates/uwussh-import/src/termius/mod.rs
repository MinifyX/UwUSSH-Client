//! Termius, which has no export.
//!
//! What it has is an Electron IndexedDB on the user's disk — Termius 10 keeps
//! one database per entity: `hosts`, `groups`, `ssh_configs`,
//! `ssh_identities`, `keys`, `known_hosts`, `snippets` and more — with every
//! sensitive field sealed under a local key from the credential store. The same
//! user on the same machine can read both, which is all an import needs: no
//! Termius account, no password, and no export file with every secret in
//! plain text left behind in the downloads folder.
//!
//! How the entities hang together, as found in Termius 10.0:
//!
//! ```text
//! host ──ssh_config──► ssh_config ──identity──► ssh_identity ──ssh_key──► key
//!   └──group──► group ──parent_group──► group
//!                 └──ssh_config──► ssh_config   (what hosts inherit)
//! tag_host ──host──► host, ──tag──► tag
//! snippet ──package──► snippets_package
//! ```
//!
//! **Tied to Termius' internals.** Chromium's formats underneath are stable;
//! which databases Termius keeps and what their fields mean is not.

pub mod records;
pub mod seal;

use crate::chromium::{idb, leveldb};
use crate::{
    ImportBundle, ImportedHost, ImportedIdentity, ImportedKey, ImportedKnownHost, ImportedSnippet,
    KeyFormat, Secret,
};
use records::{read_store, Record};
use seal::{KeyError, LocalKey};
use std::path::{Path, PathBuf};

/// How deep group nesting is followed; also what stops a cycle.
const MAX_GROUP_DEPTH: usize = 16;

#[derive(Debug, thiserror::Error)]
pub enum TermiusError {
    #[error("no Termius data found for this user")]
    NotInstalled,
    #[error(transparent)]
    Key(#[from] KeyError),
    #[error(transparent)]
    Read(#[from] leveldb::LevelDbError),
    #[error("Termius' data is there, but it holds no hosts — a newer Termius may have moved it")]
    NoHosts,
}

/// Where Termius keeps its IndexedDB for the current user.
pub fn default_data_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        PathBuf::from(std::env::var_os("APPDATA")?)
    } else if cfg!(target_os = "macos") {
        PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".config")))?
    };
    Some(base.join("Termius/IndexedDB/file__0.indexeddb.leveldb"))
}

/// Whether this user has a Termius install to import from.
pub fn is_installed() -> bool {
    default_data_dir().is_some_and(|dir| dir.is_dir())
}

/// Import the local Termius install of the user running this process.
pub fn import_local() -> Result<ImportBundle, TermiusError> {
    let dir = default_data_dir()
        .filter(|dir| dir.is_dir())
        .ok_or(TermiusError::NotInstalled)?;
    import(&dir, &LocalKey::from_credential_store()?)
}

/// Import from a Termius IndexedDB directory with the given key.
pub fn import(dir: &Path, key: &LocalKey) -> Result<ImportBundle, TermiusError> {
    let snapshot = leveldb::read_dir(dir)?;
    let databases = idb::databases(&snapshot);
    let store = |name| read_store(&databases, name, key).unwrap_or_default();

    let data = Termius {
        hosts: store("hosts"),
        groups: store("groups"),
        configs: store("ssh_configs"),
        identities: store("ssh_identities"),
        keys: store("keys"),
        known_hosts: store("known_hosts"),
        snippets: store("snippets"),
        packages: store("snippets_packages"),
        tags: store("tags"),
        tag_hosts: store("tag_hosts"),
    };
    if data.hosts.is_empty() {
        return Err(TermiusError::NoHosts);
    }

    let mut bundle = ImportBundle::default();
    for (file, problem) in snapshot.unreadable {
        bundle
            .skipped
            .push((format!("Termius file {file}"), problem.to_owned()));
    }
    data.map_into(&mut bundle);
    Ok(bundle)
}

struct Termius {
    hosts: Vec<Record>,
    groups: Vec<Record>,
    configs: Vec<Record>,
    identities: Vec<Record>,
    keys: Vec<Record>,
    known_hosts: Vec<Record>,
    snippets: Vec<Record>,
    packages: Vec<Record>,
    tags: Vec<Record>,
    tag_hosts: Vec<Record>,
}

/// Records waiting to be deleted on the next sync are already gone for the
/// user.
fn is_live(record: &Record) -> bool {
    !record
        .status
        .as_deref()
        .is_some_and(|status| status.eq_ignore_ascii_case("deleted"))
}

/// Records whose sealed fields did not open were sealed with a key this device
/// does not have: a team vault's. Importing them half-read would be worse than
/// saying so.
fn is_readable(record: &Record, what: &str, skipped: &mut Vec<(String, String)>) -> bool {
    if record.unopened.is_empty() {
        return true;
    }
    let name = record.non_empty("label").unwrap_or("unnamed");
    skipped.push((
        format!("{what} {name:?}"),
        "sealed with a key this device does not have, probably a team vault's".into(),
    ));
    false
}

impl Termius {
    fn map_into(self, bundle: &mut ImportBundle) {
        // Keys first, then identities pointing at keys, then hosts pointing at
        // identities; each step remembers where its records landed.
        let mut key_index = Vec::new();
        for record in self.keys.iter().filter(|r| is_live(r)) {
            if !is_readable(record, "key", &mut bundle.skipped) {
                continue;
            }
            let label = record
                .non_empty("label")
                .unwrap_or("Termius key")
                .to_owned();
            let Some(private_key) = record.non_empty("private_key") else {
                bundle
                    .skipped
                    .push((format!("key {label:?}"), "has no private key".into()));
                continue;
            };
            let Some(format) = KeyFormat::detect(private_key) else {
                bundle.skipped.push((
                    format!("key {label:?}"),
                    "is in a format UwUSSH cannot read".into(),
                ));
                continue;
            };
            key_index.push((record, bundle.keys.len()));
            bundle.keys.push(ImportedKey {
                label,
                format,
                private_key: secret(private_key),
                passphrase: record.non_empty("passphrase").map(secret),
                public_key: record.non_empty("public_key").map(str::to_owned),
            });
        }

        let mut identity_index = Vec::new();
        for record in self.identities.iter().filter(|r| is_live(r)) {
            if !is_readable(record, "identity", &mut bundle.skipped) {
                continue;
            }
            let key = key_index
                .iter()
                .find(|(key, _)| record.points_at("ssh_key", key))
                .map(|&(_, index)| index);
            identity_index.push((record, bundle.identities.len()));
            bundle.identities.push(ImportedIdentity {
                label: record.non_empty("label").map(str::to_owned),
                username: record.non_empty("username").map(str::to_owned),
                password: record.non_empty("password").map(secret),
                key,
                shared: record.boolean("is_visible").unwrap_or(false),
            });
        }

        for host in self.hosts.iter().filter(|r| is_live(r)) {
            let label = host.non_empty("label");
            if !is_readable(host, "host", &mut bundle.skipped) {
                continue;
            }
            let Some(address) = host.non_empty("address") else {
                bundle.skipped.push((
                    format!("host {:?}", label.unwrap_or("unnamed")),
                    "has no address".into(),
                ));
                continue;
            };
            let name = label.unwrap_or(address).trim().to_owned();

            let groups = self.group_chain(host);
            let configs: Vec<&Record> = std::iter::once(host)
                .chain(groups.iter().copied())
                .filter_map(|holder| {
                    self.configs
                        .iter()
                        .find(|c| holder.points_at("ssh_config", c))
                })
                .collect();

            let port = match configs.iter().find_map(|c| c.number("port")) {
                None => 22,
                Some(port) if (1.0..=65535.0).contains(&port) && port.fract() == 0.0 => port as u16,
                Some(_) => {
                    bundle
                        .skipped
                        .push((format!("host {name:?}"), "has an invalid port".into()));
                    continue;
                }
            };

            let identity = configs
                .iter()
                .find_map(|config| {
                    identity_index
                        .iter()
                        .find(|(identity, _)| config.points_at("identity", identity))
                })
                .map(|&(_, index)| index);

            let group_path = (!groups.is_empty()).then(|| {
                groups
                    .iter()
                    .rev()
                    .map(|g| g.non_empty("label").unwrap_or("Unnamed group").trim())
                    .collect::<Vec<_>>()
                    .join("/")
            });

            let tags = self
                .tag_hosts
                .iter()
                .filter(|link| is_live(link) && link.points_at("host", host))
                .filter_map(|link| self.tags.iter().find(|tag| link.points_at("tag", tag)))
                .filter_map(|tag| tag.non_empty("label"))
                .map(str::to_owned)
                .collect();

            let mut extras = Vec::new();
            if let Some(os) = host.non_empty("os_name") {
                extras.push(("termius.os_name".into(), os.to_owned()));
            }
            if let Some(backspace) = host.non_empty("backspace") {
                extras.push(("termius.backspace".into(), backspace.to_owned()));
            }
            if configs.iter().any(|c| {
                c.non_empty("env_variables")
                    .is_some_and(|env| env.trim() != "{}")
            }) {
                // May hold tokens; not carried over until hosts have somewhere
                // safe to keep them.
                bundle.skipped.push((
                    format!("host {name:?}"),
                    "has environment variables, which are not imported yet".into(),
                ));
            }

            bundle.hosts.push(ImportedHost {
                name,
                address: address.trim().to_owned(),
                port,
                username: identity.and_then(|i| bundle.identities[i].username.clone()),
                key_path: None,
                jump_host: None,
                group_path,
                remote_command: None,
                charset: configs
                    .iter()
                    .find_map(|c| c.non_empty("charset"))
                    .map(str::to_owned),
                extras,
                identity,
                tags,
            });
        }

        for record in self.known_hosts.iter().filter(|r| is_live(r)) {
            if !is_readable(record, "known host", &mut bundle.skipped) {
                continue;
            }
            match known_host(record) {
                Ok(entries) => bundle.known_hosts.extend(entries),
                Err(problem) => bundle
                    .skipped
                    .push(("a known host key".into(), problem.into())),
            }
        }

        for record in self.snippets.iter().filter(|r| is_live(r)) {
            if !is_readable(record, "snippet", &mut bundle.skipped) {
                continue;
            }
            let Some(script) = record.text("script").filter(|s| !s.trim().is_empty()) else {
                continue;
            };
            let group = self
                .packages
                .iter()
                .find(|p| record.points_at("package", p))
                .and_then(|p| p.non_empty("label"))
                .map(str::to_owned);
            bundle.snippets.push(ImportedSnippet {
                label: record.non_empty("label").unwrap_or("Snippet").to_owned(),
                script: script.to_owned(),
                group,
            });
        }
    }

    /// A host's group, that group's parent, and so on, innermost first.
    fn group_chain(&self, host: &Record) -> Vec<&Record> {
        let mut chain: Vec<&Record> = Vec::new();
        let mut field = "group";
        let mut current = host;
        while chain.len() < MAX_GROUP_DEPTH {
            let Some(group) = self.groups.iter().find(|g| current.points_at(field, g)) else {
                break;
            };
            if chain.iter().any(|seen| std::ptr::eq(*seen, group)) {
                break;
            }
            chain.push(group);
            current = group;
            field = "parent_group";
        }
        chain
    }
}

fn secret(text: &str) -> Secret {
    Secret::new(text.to_owned())
}

/// One `known_hosts` record: host patterns as in OpenSSH (`host`,
/// `[host]:port`, several separated by commas) and a public key line.
fn known_host(record: &Record) -> Result<Vec<ImportedKnownHost>, &'static str> {
    let hostnames = record.non_empty("hostnames").ok_or("has no host name")?;
    let mut key = record
        .non_empty("key")
        .ok_or("has no key")?
        .split_whitespace();
    let (Some(algorithm), Some(blob)) = (key.next(), key.next()) else {
        return Err("has a key in an unknown format");
    };

    let mut entries = Vec::new();
    for pattern in hostnames
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        if pattern.starts_with('|') {
            // Hashed names can be checked against a name, not listed.
            continue;
        }
        let (host, port) = match pattern
            .strip_prefix('[')
            .and_then(|rest| rest.split_once("]:"))
        {
            Some((host, port)) => match port.parse::<u16>() {
                Ok(port) if port > 0 => (host, port),
                _ => return Err("has a host name with an invalid port"),
            },
            None => (pattern, 22),
        };
        entries.push(ImportedKnownHost {
            host: host.to_owned(),
            port,
            algorithm: algorithm.to_owned(),
            key: blob.to_owned(),
        });
    }
    if entries.is_empty() {
        return Err("has only hashed host names");
    }
    Ok(entries)
}

#[cfg(test)]
mod tests;
