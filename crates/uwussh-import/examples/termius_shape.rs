//! What Termius' sealed fields contain — as shapes, never as values.
//!
//!   cargo run -p uwussh-import --example termius_shape
//!
//! Opens every sealed field of the local Termius install with its local key
//! and reports, per field: which header length opened it, and what kind of
//! plaintext came out — a JSON object and its property names, a PEM block and
//! its label, an OpenSSH public key and its algorithm, or just "text". No
//! plaintext, no lengths, nothing that identifies a host or a secret.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use std::collections::BTreeMap;
use uwussh_import::chromium::{idb, leveldb, v8::Value};
use uwussh_import::termius::{self, seal::LocalKey};

type Tally = BTreeMap<String, BTreeMap<String, usize>>;

fn main() {
    let dir = termius::default_data_dir().expect("no data directory");
    let key = match LocalKey::from_credential_store() {
        Ok(key) => key,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    println!("local key: found");

    let snapshot = leveldb::read_dir(&dir).expect("readable database");
    let mut tally = Tally::new();

    for database in idb::databases(&snapshot) {
        for store in &database.stores {
            for record in &store.records {
                if let Ok(value) = &record.value {
                    walk(&key, value, &store.name, &mut tally);
                }
            }
        }
    }

    for (path, kinds) in &tally {
        let kinds: Vec<String> = kinds.iter().map(|(k, n)| format!("{k}×{n}")).collect();
        println!("{path:<48} {}", kinds.join(" "));
    }
}

fn walk(key: &LocalKey, value: &Value, path: &str, tally: &mut Tally) {
    match value {
        Value::Object(properties) => {
            for (name, child) in properties {
                let name = if is_schema_name(name) {
                    name.as_str()
                } else {
                    "<key>"
                };
                walk(key, child, &format!("{path}.{name}"), tally);
            }
        }
        Value::String(s) if looks_sealed(s) => {
            let (header, kind) = match open_any_header(key, s) {
                Some((header, plaintext)) => {
                    (format!("h{header}"), describe(&plaintext, path, tally))
                }
                None => ("-".into(), "does-not-open".into()),
            };
            *tally
                .entry(path.to_owned())
                .or_default()
                .entry(format!("{header}:{kind}"))
                .or_default() += 1;
        }
        _ => {}
    }
}

fn open_any_header(key: &LocalKey, sealed: &str) -> Option<(usize, zeroize::Zeroizing<Vec<u8>>)> {
    let raw = zeroize::Zeroizing::new(BASE64.decode(sealed).ok()?);
    (1..=3).find_map(|header| {
        let body = raw.get(header..)?;
        let nonce: &[u8; 24] = body.get(..24)?.try_into().ok()?;
        let opened = key.open_box(nonce, body.get(24..)?).ok()?;
        // The production path, too, so a mismatch between the two shows here.
        if header == 2 {
            assert!(key
                .open(sealed)
                .is_ok_and(|p| p.as_slice() == opened.as_slice()));
        }
        Some((header, opened))
    })
}

/// A kind for the plaintext. JSON objects are also walked, into `path#`.
fn describe(plaintext: &[u8], path: &str, tally: &mut Tally) -> String {
    let Ok(text) = std::str::from_utf8(plaintext) else {
        return "binary".into();
    };
    if text.is_empty() {
        return "empty".into();
    }
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(text) {
        match &json {
            serde_json::Value::Object(_) => {
                json_shape(&json, &format!("{path}#"), tally);
                return "json-object".into();
            }
            serde_json::Value::Array(_) => {
                json_shape(&json, &format!("{path}#"), tally);
                return "json-array".into();
            }
            // Bare numbers and words also parse as JSON; they are just text.
            _ => {}
        }
    }
    let first_line = text.lines().next().unwrap_or_default();
    if let Some(label) = first_line
        .strip_prefix("-----BEGIN ")
        .and_then(|rest| rest.strip_suffix("-----"))
    {
        return format!("pem:{label}");
    }
    let first_word = first_line.split_whitespace().next().unwrap_or_default();
    if ["ssh-", "ecdsa-", "sk-"]
        .iter()
        .any(|p| first_word.starts_with(p))
        && first_line.contains(' ')
    {
        return format!("openssh-public:{first_word}");
    }
    if text.contains('\n') {
        "text-multiline".into()
    } else if text.starts_with("|1|") {
        "hashed-hostname".into()
    } else if text.contains(',') {
        "text-with-commas".into()
    } else {
        "text".into()
    }
}

fn json_shape(value: &serde_json::Value, path: &str, tally: &mut Tally) {
    use serde_json::Value as J;
    let kind = match value {
        J::Null => "null",
        J::Bool(_) => "bool",
        J::Number(_) => "number",
        J::String(s) if s.is_empty() => "empty-string",
        J::String(_) => "string",
        J::Array(_) => "array",
        J::Object(_) => "object",
    };
    *tally
        .entry(path.to_owned())
        .or_default()
        .entry(kind.into())
        .or_default() += 1;
    match value {
        J::Object(map) => {
            for (name, child) in map {
                let name = if is_schema_name(name) {
                    name.as_str()
                } else {
                    "<key>"
                };
                let sep = if path.ends_with('#') { "" } else { "." };
                json_shape(child, &format!("{path}{sep}{name}"), tally);
            }
        }
        J::Array(items) => {
            for item in items {
                json_shape(item, &format!("{path}[]"), tally);
            }
        }
        _ => {}
    }
}

fn is_schema_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    let starts_like_identifier = bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_' || b == b'$');
    let looks_like_a_hash = name.len() >= 16 && name.bytes().all(|b| b.is_ascii_hexdigit());
    starts_like_identifier
        && name.len() <= 40
        && !looks_like_a_hash
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'$')
}

fn looks_sealed(s: &str) -> bool {
    s.len() >= 40
        && s.len().is_multiple_of(4)
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
}
