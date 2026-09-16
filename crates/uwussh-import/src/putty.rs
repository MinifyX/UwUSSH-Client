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
        port: values.get("PortNumber").and_then(|p| p.parse().ok()).unwrap_or(22),
        username: non_empty(values.get("UserName")),
        key_path: non_empty(values.get("PublicKeyFile")),
        jump_host: non_empty(values.get("ProxyHost")),
        remote_command: non_empty(values.get("RemoteCommand")),
        charset: non_empty(values.get("LineCodePage")),
        group_path: None,
        extras: Vec::new(),
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
    for key in ["Compression", "TerminalType", "BackspaceIsDelete", "PortForwardings"] {
        if let Some(value) = non_empty(values.get(key)) {
            host.extras.push((key.to_string(), value));
        }
    }

    tracing::debug!(?source, name = %host.name, "mapped putty-style session");
    host
}

/// `My%20Server` → `My Server`.
fn decode_session_name(encoded: &str) -> String {
    let bytes = encoded.as_bytes();
    let mut out = String::with_capacity(encoded.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = &encoded[i + 1..i + 3];
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte as char);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    fn values(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn maps_a_plain_session() {
        let host = from_session_values(
            "prox-1",
            &values(&[("HostName", "10.0.0.12"), ("PortNumber", "22"), ("UserName", "root")]),
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
        let host =
            from_session_values("x", &values(&[("Compression", "1")]), Source::Putty);
        assert!(host.extras.iter().any(|(k, v)| k == "Compression" && v == "1"));
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
}
