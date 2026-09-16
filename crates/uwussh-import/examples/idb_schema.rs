//! The shape of a Chromium IndexedDB — databases, stores, field names, value
//! types and counts — without printing a single value.
//!
//!   cargo run -p uwussh-import --example idb_schema -- <leveldb directory>
//!
//! Written to learn how Termius lays out its data before writing the importer,
//! against a real install, without the output containing anyone's hosts. It
//! only reads: the app can keep running.

use std::collections::BTreeMap;
use std::path::PathBuf;
use uwussh_import::chromium::{idb, leveldb, v8::Value};

fn main() {
    let Some(dir) = std::env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("usage: idb_schema <leveldb directory>");
        std::process::exit(2);
    };

    let snapshot = match leveldb::read_dir(&dir) {
        Ok(snapshot) => snapshot,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    println!("{} live entries", snapshot.entries.len());
    for (file, problem) in &snapshot.unreadable {
        println!("unreadable: {file}: {problem}");
    }

    for database in idb::databases(&snapshot) {
        println!(
            "\ndatabase {:?} (origin {})",
            database.name, database.origin
        );
        for store in &database.stores {
            let mut fields: BTreeMap<String, BTreeMap<&'static str, usize>> = BTreeMap::new();
            let mut keys: BTreeMap<&'static str, usize> = BTreeMap::new();
            let mut errors: BTreeMap<String, usize> = BTreeMap::new();

            for record in &store.records {
                *keys.entry(key_kind(&record.key)).or_default() += 1;
                match &record.value {
                    Ok(value) => shape(value, "", 0, &mut fields),
                    Err(e) => *errors.entry(e.to_string()).or_default() += 1,
                }
            }

            println!(
                "  store {:?}: {} records, keys {}",
                store.name,
                store.records.len(),
                counts(&keys)
            );
            for (error, n) in &errors {
                println!("    ! {n}× {error}");
            }
            for (path, kinds) in &fields {
                println!("    {path:<40} {}", counts(kinds));
            }
        }
    }
}

fn shape(
    value: &Value,
    path: &str,
    depth: usize,
    fields: &mut BTreeMap<String, BTreeMap<&'static str, usize>>,
) {
    if !path.is_empty() {
        *fields
            .entry(path.to_owned())
            .or_default()
            .entry(kind(value))
            .or_default() += 1;
    }
    if depth >= 4 {
        return;
    }
    match value {
        Value::Object(properties) => {
            for (name, child) in properties {
                // Objects used as maps have data for property names — ids,
                // addresses. Only identifier-shaped names are schema.
                let name = if is_schema_name(name) { name } else { "<key>" };
                let child_path = if path.is_empty() {
                    name.to_owned()
                } else {
                    format!("{path}.{name}")
                };
                shape(child, &child_path, depth + 1, fields);
            }
        }
        Value::Array(items) => {
            for item in items {
                shape(item, &format!("{path}[]"), depth + 1, fields);
            }
        }
        _ => {}
    }
}

/// A type, never a value. Strings are only told apart by whether they look
/// like base64 long enough to be a sealed box — not by their length, which
/// would say how long a password is.
fn kind(value: &Value) -> &'static str {
    match value {
        Value::Undefined => "undefined",
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::BigInt { .. } => "bigint",
        Value::String(s) if s.is_empty() => "empty-string",
        Value::String(s) if looks_sealed(s) => "base64≥40",
        Value::String(_) => "string",
        Value::Date(_) => "date",
        Value::RegExp { .. } => "regexp",
        Value::Bytes(_) => "bytes",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
        Value::Map(_) => "map",
        Value::Set(_) => "set",
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

fn key_kind(key: &idb::Key) -> &'static str {
    match key {
        idb::Key::Null => "null",
        idb::Key::String(_) => "string",
        idb::Key::Date(_) => "date",
        idb::Key::Number(_) => "number",
        idb::Key::Array(_) => "array",
        idb::Key::Binary(_) => "binary",
        idb::Key::MinKey => "min",
        idb::Key::Unreadable => "unreadable",
    }
}

fn counts<K: std::fmt::Display>(map: &BTreeMap<K, usize>) -> String {
    map.iter()
        .map(|(k, n)| format!("{k}×{n}"))
        .collect::<Vec<_>>()
        .join(" ")
}
