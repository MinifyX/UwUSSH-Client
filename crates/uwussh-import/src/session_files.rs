//! PuTTY and KiTTY sessions that live in files rather than in the registry.
//!
//! Two shapes, both common enough to be worth a folder picker:
//!
//! - **KiTTY portable** (`savemode=dir` in `kitty.ini`) keeps one file per
//!   session in a `Sessions` folder next to `kitty.exe`. The file name is the
//!   session name, percent-encoded like the registry key, and every line is
//!   `Name\Value\` — the value may hold backslashes of its own
//!   (`PublicKeyFile\C:\keys\nas.ppk\`), so only the first and the last one
//!   are separators.
//! - **A registry export** (`reg export HKCU\Software\SimonTatham\PuTTY\Sessions`,
//!   or the same for KiTTY), which is how people carry PuTTY sessions from one
//!   machine to the next. UTF-16 with a BOM from `regedit`, ANSI from
//!   `REGEDIT4`.
//!
//! The person picks a folder: the `Sessions` folder itself, the KiTTY folder
//! around it, or a folder with `.reg` files in it. Everything found goes
//! through the same mapping as the registry import, so a session reads the
//! same wherever it was kept.
//!
//! Nothing here trusts the files: sizes and counts are capped, names are
//! decoded on bytes, and a file that is neither shape is skipped with a reason.

use crate::putty::{decode_session_name, from_session_values};
use crate::{ImportResult, Source};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// More sessions than anyone keeps; a folder with more files is not one.
const MAX_FILES: usize = 5_000;
/// A session file is a few kilobytes.
const MAX_SESSION_BYTES: u64 = 1 << 20;
/// A registry export of every session PuTTY ever saw is still well below this.
const MAX_REG_BYTES: u64 = 32 << 20;
/// PuTTY's own template, which has settings but never a host.
const DEFAULT_SESSION: &str = "Default Settings";

/// Whether a folder holds anything this reader would understand.
pub fn looks_importable(dir: &Path) -> bool {
    sessions_dir(dir).is_some() || reg_files(dir).next().is_some()
}

/// Read every session in `dir`: a `Sessions` folder (or the folder around
/// one) and any `.reg` export in it.
pub fn read_folder(dir: &Path) -> crate::Result<ImportResult> {
    if !dir.is_dir() {
        return Err(crate::ImportError::Read(format!(
            "{} is not a folder",
            dir.display()
        )));
    }
    let mut result = ImportResult::default();
    let mut seen = 0usize;

    let sessions_folder = sessions_dir(dir);
    if let Some(sessions) = &sessions_folder {
        let entries = std::fs::read_dir(sessions)
            .map_err(|e| crate::ImportError::Read(format!("{}: {e}", sessions.display())))?;
        for entry in entries.flatten() {
            seen += 1;
            if seen > MAX_FILES {
                result
                    .skipped
                    .push((sessions.display().to_string(), "too many files".into()));
                break;
            }
            let path = entry.path();
            // Symlinks are not followed: a session is a file KiTTY wrote.
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let file_name = entry.file_name().to_string_lossy().into_owned();
            if is_reg(&path) {
                read_reg_file(&path, meta.len(), &mut result);
                continue;
            }
            let name = decode_session_name(&file_name);
            if name == DEFAULT_SESSION {
                continue;
            }
            if meta.len() > MAX_SESSION_BYTES {
                result
                    .skipped
                    .push((name, "is too large for a session".into()));
                continue;
            }
            match std::fs::read(&path) {
                Ok(bytes) => match parse_session_file(&text_of(&bytes)) {
                    Some(values) => push(&mut result, &file_name, &values, Source::Kitty),
                    None => result
                        .skipped
                        .push((name, "is not a PuTTY or KiTTY session".into())),
                },
                Err(e) => result
                    .skipped
                    .push((name, format!("could not be read: {e}"))),
            }
        }
    }

    // `.reg` exports next to (or instead of) a Sessions folder. A picked
    // Sessions folder had its own read above.
    let beside = sessions_folder.as_deref() != Some(dir);
    for path in reg_files(dir).filter(|_| beside).take(MAX_FILES) {
        let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        read_reg_file(&path, len, &mut result);
    }

    Ok(result)
}

fn push(
    result: &mut ImportResult,
    encoded: &str,
    values: &HashMap<String, String>,
    source: Source,
) {
    let host = from_session_values(encoded, values, source);
    if host.address.trim().is_empty() {
        result.skipped.push((host.name, "has no host name".into()));
    } else {
        result.hosts.push(host);
    }
}

/// The `Sessions` folder: `dir` itself when it is named so or holds session
/// files directly, else a `Sessions` folder inside it (any case).
fn sessions_dir(dir: &Path) -> Option<PathBuf> {
    let named = |path: &Path| {
        path.file_name()
            .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("sessions"))
    };
    if named(dir) {
        return Some(dir.to_path_buf());
    }
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| named(path) && path.is_dir())
}

fn is_reg(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.to_string_lossy().eq_ignore_ascii_case("reg"))
}

fn reg_files(dir: &Path) -> impl Iterator<Item = PathBuf> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| is_reg(path) && path.is_file())
}

fn read_reg_file(path: &Path, len: u64, result: &mut ImportResult) {
    let label = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if len > MAX_REG_BYTES {
        result.skipped.push((label, "is too large".into()));
        return;
    }
    match std::fs::read(path) {
        Ok(bytes) => {
            let sessions = parse_reg(&text_of(&bytes));
            if sessions.is_empty() {
                result
                    .skipped
                    .push((label, "holds no PuTTY or KiTTY sessions".into()));
            }
            for (encoded, values, source) in sessions {
                if decode_session_name(&encoded) == DEFAULT_SESSION {
                    continue;
                }
                push(result, &encoded, &values, source);
            }
        }
        Err(e) => result
            .skipped
            .push((label, format!("could not be read: {e}"))),
    }
}

/// Text from a file that may be UTF-16 (with BOM), UTF-8, or Windows' ANSI
/// code page — which for the characters in a host name is Latin-1 closely
/// enough.
fn text_of(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

/// `Name\Value\` lines. `None` when not a single line has that shape, which
/// means the file is something else that happens to sit in the folder.
fn parse_session_file(text: &str) -> Option<HashMap<String, String>> {
    let mut values = HashMap::new();
    for line in text.lines() {
        let line = line.trim_end_matches(['\r', '\n']);
        let Some((name, rest)) = line.split_once('\\') else {
            continue;
        };
        let Some(value) = rest.strip_suffix('\\') else {
            continue;
        };
        if name.is_empty() || name.len() > 64 || !name.bytes().all(|b| b.is_ascii_graphic()) {
            continue;
        }
        values.insert(name.to_string(), value.to_string());
    }
    (!values.is_empty()).then_some(values)
}

/// Sessions in a registry export: each `[…\Sessions\<name>]` section, with its
/// string and DWORD values. Binary values and anything outside a Sessions key
/// are ignored.
fn parse_reg(text: &str) -> Vec<(String, HashMap<String, String>, Source)> {
    let mut sessions: Vec<(String, HashMap<String, String>, Source)> = Vec::new();
    let mut current: Option<usize> = None;
    let mut continued = false;

    for raw in text.lines() {
        let line = raw.trim();
        // A hex value that runs over several lines ends each with a backslash.
        if continued {
            continued = line.ends_with('\\');
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            let key = &line[1..line.len() - 1];
            current = session_of(key).map(|(name, source)| {
                sessions.push((name, HashMap::new(), source));
                sessions.len() - 1
            });
            continue;
        }
        let Some(index) = current else { continue };
        let Some((name, value)) = split_value(line) else {
            continued = line.ends_with('\\');
            continue;
        };
        let parsed = if let Some(text) = value.strip_prefix('"') {
            unquote(text)
        } else if let Some(hex) = value.strip_prefix("dword:") {
            u32::from_str_radix(hex.trim(), 16)
                .ok()
                .map(|n| n.to_string())
        } else {
            continued = value.ends_with('\\');
            None
        };
        if let Some(parsed) = parsed {
            sessions[index].1.insert(name, parsed);
        }
    }
    sessions
}

/// The session a registry key names, if it is one directly under a Sessions key.
fn session_of(key: &str) -> Option<(String, Source)> {
    let lower = key.to_ascii_lowercase();
    let at = lower.rfind("\\sessions\\")?;
    let name = &key[at + "\\sessions\\".len()..];
    if name.is_empty() || name.contains('\\') {
        return None;
    }
    let source = if lower.contains("\\kitty\\") {
        Source::Kitty
    } else {
        Source::Putty
    };
    Some((name.to_string(), source))
}

/// `"Name"=value` → (`Name`, `value`).
fn split_value(line: &str) -> Option<(String, &str)> {
    let rest = line.strip_prefix('"')?;
    let mut name = String::new();
    let mut chars = rest.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => {
                if let Some((_, next)) = chars.next() {
                    name.push(next);
                }
            }
            '"' => {
                let after = rest[i + 1..].strip_prefix('=')?;
                return Some((name, after.trim()));
            }
            other => name.push(other),
        }
    }
    None
}

/// The rest of a quoted `.reg` string after its opening quote, unescaped.
fn unquote(text: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push(chars.next()?),
            '"' => return Some(out),
            other => out.push(other),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_kitty_session_file() {
        let values = parse_session_file(
            "HostName\\10.0.0.12\\\r\nPortNumber\\2222\\\r\nUserName\\root\\\r\nPublicKeyFile\\C:\\keys\\nas.ppk\\\r\n",
        )
        .unwrap();
        assert_eq!(values["HostName"], "10.0.0.12");
        assert_eq!(values["PortNumber"], "2222");
        assert_eq!(
            values["PublicKeyFile"], r"C:\keys\nas.ppk",
            "inner backslashes stay"
        );
    }

    #[test]
    fn a_file_that_is_no_session_is_not_one() {
        assert!(parse_session_file("just some notes\nnothing here").is_none());
        assert!(parse_session_file("").is_none());
    }

    #[test]
    fn reads_a_registry_export() {
        let reg = r#"Windows Registry Editor Version 5.00

[HKEY_CURRENT_USER\Software\SimonTatham\PuTTY\Sessions]

[HKEY_CURRENT_USER\Software\SimonTatham\PuTTY\Sessions\Default%20Settings]
"PortNumber"=dword:00000016

[HKEY_CURRENT_USER\Software\SimonTatham\PuTTY\Sessions\homelab%2Fprox-1]
"HostName"="10.0.0.12"
"PortNumber"=dword:000008ae
"UserName"="root"
"PublicKeyFile"="C:\\keys\\home \"lab\".ppk"
"Colour0"=hex:01,02,\
  03,04

[HKEY_CURRENT_USER\Software\9bis.com\KiTTY\Sessions\web]
"HostName"="web.lan"
"#;
        let sessions = parse_reg(reg);
        assert_eq!(sessions.len(), 3);
        let (name, values, source) = &sessions[1];
        assert_eq!(name, "homelab%2Fprox-1");
        assert_eq!(*source, Source::Putty);
        assert_eq!(values["PortNumber"], "2222");
        assert_eq!(values["PublicKeyFile"], r#"C:\keys\home "lab".ppk"#);
        assert!(!values.contains_key("Colour0"));
        assert_eq!(sessions[2].2, Source::Kitty);
    }

    #[test]
    fn reads_utf16_with_a_bom() {
        let text = "Windows Registry Editor Version 5.00\r\n[HKEY_CURRENT_USER\\Software\\SimonTatham\\PuTTY\\Sessions\\bür%C3%B6]\r\n\"HostName\"=\"b.lan\"\r\n";
        let mut bytes = vec![0xFF, 0xFE];
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        let sessions = parse_reg(&text_of(&bytes));
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].1["HostName"], "b.lan");
    }

    #[test]
    fn reads_a_kitty_folder_with_a_reg_file_beside_it() {
        let dir = std::env::temp_dir().join(format!("uwussh-kitty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let sessions = dir.join("Sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            sessions.join("My%20NAS"),
            "HostName\\nas.lan\\\nUserName\\lorin\\\n",
        )
        .unwrap();
        std::fs::write(sessions.join("Default%20Settings"), "PortNumber\\22\\\n").unwrap();
        std::fs::write(sessions.join("broken"), "UserName\\nobody\\\n").unwrap();
        std::fs::write(sessions.join("readme.txt"), "hello").unwrap();
        std::fs::write(
            dir.join("putty.reg"),
            "REGEDIT4\n[HKEY_CURRENT_USER\\Software\\SimonTatham\\PuTTY\\Sessions\\pi]\n\"HostName\"=\"pi.lan\"\n",
        )
        .unwrap();

        assert!(looks_importable(&dir));
        let result = read_folder(&dir).unwrap();
        // The Sessions folder picked directly reads the same sessions.
        let direct = read_folder(&sessions).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();

        let mut names: Vec<_> = result.hosts.iter().map(|h| h.name.as_str()).collect();
        names.sort();
        assert_eq!(names, ["My NAS", "pi"]);
        assert!(result
            .skipped
            .iter()
            .any(|(name, why)| name == "broken" && why.contains("no host name")));
        assert!(result.skipped.iter().any(|(name, _)| name == "readme.txt"));
        assert!(!result
            .skipped
            .iter()
            .any(|(name, _)| name == DEFAULT_SESSION));
        assert_eq!(direct.hosts.len(), 1);
    }

    #[test]
    fn a_folder_with_nothing_in_it_says_so() {
        let dir = std::env::temp_dir().join(format!("uwussh-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!looks_importable(&dir));
        let result = read_folder(&dir).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(result.hosts.is_empty());
        assert!(read_folder(&dir).is_err(), "gone now");
    }
}
