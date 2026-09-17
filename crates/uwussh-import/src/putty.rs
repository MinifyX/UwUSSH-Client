//! PuTTY and KiTTY sessions.
//!
//! Both store one registry key per session, with the same value names — KiTTY
//! is a PuTTY fork and kept the format, it just moved the path. So the mapping
//! below serves both, and the only thing that differs is where the key/value
//! pairs come from.
//!
//! Session names are URL-encoded in the registry (`%20` for a space), which is
//! easy to forget and produces a host list full of `My%20Server`.
//!
//! **Unverified:** the KiTTY registry path and its portable-mode layout are
//! from documentation, not from a KiTTY install. Check both against a real one
//! before the import is advertised as working.

use crate::{ImportedHost, Source};
use std::collections::HashMap;

/// Where PuTTY keeps sessions.
pub const PUTTY_REGISTRY_PATH: &str = r"Software\SimonTatham\PuTTY\Sessions";

/// Where KiTTY keeps sessions when it is *not* running portable. In portable
/// mode it writes a `Sessions\` directory next to the executable instead, one
/// file per session, same key=value lines.
pub const KITTY_REGISTRY_PATH: &str = r"Software\9bis.com\KiTTY\Sessions";

/// Turn one session's raw values into a host.
///
/// Takes a plain map so this is testable without a registry — which matters,
/// because the mapping is where the edge cases live, not the reading.
pub fn from_session_values(
    encoded_name: &str,
    values: &HashMap<String, String>,
    source: Source,
) -> ImportedHost {
    let name = decode_session_name(encoded_name);

    let mut host = ImportedHost {
        name: name.clone(),
        address: values.get("HostName").cloned().unwrap_or_default(),
        port: values
            .get("PortNumber")
            .and_then(|p| p.parse().ok())
            .unwrap_or(22),
        username: non_empty(values.get("UserName")),
        key_path: non_empty(values.get("PublicKeyFile")),
        jump_host: non_empty(values.get("ProxyHost")),
        remote_command: non_empty(values.get("RemoteCommand")),
        charset: non_empty(values.get("LineCodePage")),
        group_path: None,
        extras: Vec::new(),
        identity: None,
        tags: Vec::new(),
    };

    // PuTTY has no folders, but people fake them with "homelab/prox-1" or
    // "homelab | prox-1" in the session name. Honour that rather than dumping
    // eighty hosts into one flat list.
    if let Some((group, leaf)) = split_faux_folder(&name) {
        host.group_path = Some(group);
        host.name = leaf;
    }

    // Keep settings we recognise but do not map yet, so nothing a user
    // configured disappears without a trace.
    for key in [
        "Compression",
        "TerminalType",
        "BackspaceIsDelete",
        "PortForwardings",
    ] {
        if let Some(value) = non_empty(values.get(key)) {
            host.extras.push((key.to_string(), value));
        }
    }

    tracing::debug!(?source, name = %host.name, "mapped putty-style session");
    host
}

/// `My%20Server` → `My Server`.
///
/// Works on bytes: slicing the string next to a `%` could cut a character in
/// half (`%1ü`) and panic, and non-ASCII names must come back as they were.
fn decode_session_name(encoded: &str) -> String {
    let bytes = encoded.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(byte) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn split_faux_folder(name: &str) -> Option<(String, String)> {
    for sep in ['/', '\\', '|'] {
        if let Some((group, leaf)) = name.rsplit_once(sep) {
            let group = group.trim();
            let leaf = leaf.trim();
            if !group.is_empty() && !leaf.is_empty() {
                return Some((group.replace(['\\', '|'], "/"), leaf.to_string()));
            }
        }
    }
    None
}

fn non_empty(value: Option<&String>) -> Option<String> {
    value.filter(|v| !v.trim().is_empty()).cloned()
}

/// Read the sessions PuTTY or KiTTY keeps under `HKCU\<sessions_path>`, mapping
/// each into an [`ImportedHost`]. A missing key is not an error — the client is
/// simply not installed, so this returns an empty result.
///
/// On platforms without a registry it always returns empty, so callers stay
/// platform-agnostic.
pub fn read_sessions(
    sessions_path: &str,
    source: crate::Source,
) -> crate::Result<crate::ImportResult> {
    #[cfg(windows)]
    {
        registry::read_sessions(sessions_path, source)
    }
    #[cfg(not(windows))]
    {
        let _ = (sessions_path, source);
        Ok(crate::ImportResult::default())
    }
}

/// Whether there is at least one session to import at `HKCU\<sessions_path>`.
pub fn has_sessions(sessions_path: &str) -> bool {
    #[cfg(windows)]
    {
        registry::has_sessions(sessions_path)
    }
    #[cfg(not(windows))]
    {
        let _ = sessions_path;
        false
    }
}

#[cfg(windows)]
mod registry {
    use super::{decode_session_name, from_session_values};
    use crate::{ImportError, ImportResult, Result, Source};
    use std::collections::HashMap;
    use winreg::enums::{RegType, HKEY_CURRENT_USER};
    use winreg::types::FromRegValue;
    use winreg::RegKey;

    pub fn read_sessions(sessions_path: &str, source: Source) -> Result<ImportResult> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let root = match hkcu.open_subkey(sessions_path) {
            Ok(root) => root,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ImportResult::default())
            }
            Err(e) => return Err(ImportError::Read(format!("{sessions_path}: {e}"))),
        };

        let mut result = ImportResult::default();
        for name in root.enum_keys() {
            let Ok(name) = name else { continue };
            let Ok(session) = root.open_subkey(&name) else {
                result
                    .skipped
                    .push((decode_session_name(&name), "could not be read".into()));
                continue;
            };
            let host = from_session_values(&name, &values_of(&session), source);
            if host.address.trim().is_empty() {
                result.skipped.push((host.name, "has no host name".into()));
            } else {
                result.hosts.push(host);
            }
        }
        Ok(result)
    }

    pub fn has_sessions(sessions_path: &str) -> bool {
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(sessions_path)
            .map(|root| root.enum_keys().flatten().next().is_some())
            .unwrap_or(false)
    }

    /// Every string- and number-valued setting under a session key, as strings.
    /// PuTTY keeps `PortNumber` as a DWORD and the rest as strings.
    fn values_of(key: &RegKey) -> HashMap<String, String> {
        let mut map = HashMap::new();
        for entry in key.enum_values() {
            let Ok((name, value)) = entry else { continue };
            let text = match value.vtype {
                RegType::REG_SZ | RegType::REG_EXPAND_SZ => String::from_reg_value(&value).ok(),
                RegType::REG_DWORD => u32::from_reg_value(&value).ok().map(|n| n.to_string()),
                RegType::REG_QWORD => u64::from_reg_value(&value).ok().map(|n| n.to_string()),
                _ => None,
            };
            if let Some(text) = text {
                map.insert(name, text);
            }
        }
        map
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn maps_a_plain_session() {
        let host = from_session_values(
            "prox-1",
            &values(&[
                ("HostName", "10.0.0.12"),
                ("PortNumber", "22"),
                ("UserName", "root"),
            ]),
            Source::Putty,
        );

        assert_eq!(host.name, "prox-1");
        assert_eq!(host.address, "10.0.0.12");
        assert_eq!(host.port, 22);
        assert_eq!(host.username.as_deref(), Some("root"));
    }

    #[test]
    fn decodes_escaped_session_names() {
        let host = from_session_values("My%20NAS%20Box", &values(&[]), Source::Putty);
        assert_eq!(host.name, "My NAS Box");
    }

    #[test]
    fn a_missing_port_defaults_to_22() {
        let host = from_session_values("x", &values(&[("HostName", "a.b")]), Source::Putty);
        assert_eq!(host.port, 22);
    }

    #[test]
    fn a_nonsense_port_falls_back_rather_than_failing_the_import() {
        let host = from_session_values("x", &values(&[("PortNumber", "wat")]), Source::Putty);
        assert_eq!(host.port, 22);
    }

    #[test]
    fn faked_folders_in_the_name_become_groups() {
        let host = from_session_values("homelab%2Fprox-1", &values(&[]), Source::Kitty);
        assert_eq!(host.group_path.as_deref(), Some("homelab"));
        assert_eq!(host.name, "prox-1");
    }

    #[test]
    fn empty_values_do_not_become_empty_strings() {
        let host = from_session_values(
            "x",
            &values(&[("UserName", ""), ("PublicKeyFile", "   ")]),
            Source::Putty,
        );
        assert_eq!(host.username, None);
        assert_eq!(host.key_path, None);
    }

    #[test]
    fn unmapped_settings_survive_as_extras() {
        let host = from_session_values("x", &values(&[("Compression", "1")]), Source::Putty);
        assert!(host
            .extras
            .iter()
            .any(|(k, v)| k == "Compression" && v == "1"));
    }

    #[cfg(windows)]
    #[test]
    fn reads_sessions_out_of_a_real_registry_tree() {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;

        // A throwaway tree under HKCU, unique per process so parallel test
        // binaries never collide, cleaned up at the end.
        let base = format!(
            "Software\\UwUSSH-ImportTest-{}\\Sessions",
            std::process::id()
        );
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let _ = hkcu.delete_subkey_all(&base);

        let write = |name: &str, host: &str, port: u32, user: &str, key: Option<&str>| {
            let (session, _) = hkcu.create_subkey(format!("{base}\\{name}")).unwrap();
            session.set_value("HostName", &host).unwrap();
            session.set_value("PortNumber", &port).unwrap(); // DWORD, like PuTTY
            session.set_value("UserName", &user).unwrap();
            if let Some(key) = key {
                session.set_value("PublicKeyFile", &key).unwrap();
            }
        };
        write(
            "prox-1",
            "10.0.0.12",
            2222,
            "root",
            Some(r"C:\keys\homelab.ppk"),
        );
        write("homelab%2Fweb", "10.0.0.5", 22, "deploy", None);
        // A session with no host name is reported, not imported.
        let (empty, _) = hkcu.create_subkey(format!("{base}\\broken")).unwrap();
        empty.set_value("UserName", &"nobody").unwrap();

        let result = read_sessions(&base, Source::Putty).unwrap();
        hkcu.delete_subkey_all(&base).unwrap();

        let prox = result.hosts.iter().find(|h| h.name == "prox-1").unwrap();
        assert_eq!(prox.address, "10.0.0.12");
        assert_eq!(prox.port, 2222, "PortNumber is a DWORD");
        assert_eq!(prox.username.as_deref(), Some("root"));
        assert_eq!(prox.key_path.as_deref(), Some(r"C:\keys\homelab.ppk"));

        let web = result.hosts.iter().find(|h| h.name == "web").unwrap();
        assert_eq!(
            web.group_path.as_deref(),
            Some("homelab"),
            "folder from the name"
        );

        assert_eq!(result.hosts.len(), 2);
        assert!(result
            .skipped
            .iter()
            .any(|(_, why)| why.contains("no host name")));
    }

    #[cfg(windows)]
    #[test]
    fn a_missing_registry_key_is_empty_not_an_error() {
        let result =
            read_sessions("Software\\UwUSSH-DoesNotExist\\Sessions", Source::Kitty).unwrap();
        assert!(result.hosts.is_empty());
        assert!(!has_sessions("Software\\UwUSSH-DoesNotExist\\Sessions"));
    }

    #[test]
    fn a_ppk_path_is_carried_through_for_later_conversion() {
        let host = from_session_values(
            "x",
            &values(&[("PublicKeyFile", r"C:\keys\homelab.ppk")]),
            Source::Putty,
        );
        assert_eq!(host.key_path.as_deref(), Some(r"C:\keys\homelab.ppk"));
    }

    #[test]
    fn session_names_decode_on_bytes_without_panicking() {
        assert_eq!(decode_session_name("My%20Server"), "My Server");
        assert_eq!(decode_session_name("%1ü"), "%1ü");
        assert_eq!(decode_session_name("Büro%2Fnas"), "Büro/nas");
        assert_eq!(decode_session_name("trailing%4"), "trailing%4");
    }
}
