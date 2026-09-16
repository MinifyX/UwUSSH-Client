//! Termius records with their sealed fields opened.
//!
//! A record is one IndexedDB object. Its plain fields — numbers, flags,
//! references — are used as they are; its sealed strings are opened with the
//! local key; and its `content` field, a sealed JSON object carrying the same
//! sensitive fields again plus a format `version`, is opened and laid over the
//! top. Which of the two copies Termius reads is not visible from outside, so
//! a mismatch between them is reported rather than silently resolved.

use super::seal::LocalKey;
use crate::chromium::{idb, v8::Value};
use std::collections::BTreeMap;
use zeroize::Zeroizing;

/// A pointer to another record. Termius fills in `local_id` for everything it
/// has on this device and `id` once the server knows the record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ref {
    pub id: Option<i64>,
    pub local_id: Option<i64>,
}

#[derive(Debug)]
enum Field {
    Text(Zeroizing<String>),
    Number(f64),
    Bool(bool),
    Ref(Ref),
}

#[derive(Debug)]
pub struct Record {
    pub id: Option<i64>,
    pub local_id: Option<i64>,
    pub status: Option<String>,
    fields: BTreeMap<String, Field>,
    /// Sealed fields that exist in both copies and disagree.
    pub mismatched: Vec<String>,
    /// Sealed fields that did not open — sealed with another key, such as a
    /// team vault's.
    pub unopened: Vec<String>,
}

impl Record {
    pub fn text(&self, name: &str) -> Option<&str> {
        match self.fields.get(name)? {
            Field::Text(text) => Some(text.as_str()),
            _ => None,
        }
    }

    /// Text that is actually there: empty strings count as unset.
    pub fn non_empty(&self, name: &str) -> Option<&str> {
        self.text(name).filter(|text| !text.trim().is_empty())
    }

    pub fn number(&self, name: &str) -> Option<f64> {
        match self.fields.get(name)? {
            Field::Number(n) => Some(*n),
            Field::Text(text) => text.trim().parse().ok(),
            _ => None,
        }
    }

    pub fn boolean(&self, name: &str) -> Option<bool> {
        match self.fields.get(name)? {
            Field::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn reference(&self, name: &str) -> Option<Ref> {
        match self.fields.get(name)? {
            Field::Ref(r) => Some(*r),
            _ => None,
        }
    }

    /// Whether `target` is what this record's field `name` points at.
    pub fn points_at(&self, name: &str, target: &Record) -> bool {
        self.reference(name).is_some_and(|r| target.is(r))
    }

    /// Whether `r` points at this record: by local id where both have one,
    /// by server id otherwise.
    pub fn is(&self, r: Ref) -> bool {
        match (r.local_id, self.local_id) {
            (Some(a), Some(b)) => a == b,
            _ => r.id.is_some() && r.id == self.id,
        }
    }
}

/// Every record of one Termius database, `None` if Termius has no such
/// database at all.
pub fn read_store(databases: &[idb::Database], name: &str, key: &LocalKey) -> Option<Vec<Record>> {
    let database = databases.iter().find(|d| d.name == name)?;
    let store = database.stores.iter().find(|s| s.name == name)?;
    Some(
        store
            .records
            .iter()
            .filter_map(|record| record.value.as_ref().ok())
            .filter_map(|value| read_record(value, key))
            .collect(),
    )
}

fn read_record(value: &Value, key: &LocalKey) -> Option<Record> {
    let Value::Object(properties) = value else {
        return None;
    };
    let mut record = Record {
        id: value.get("id").and_then(Value::as_f64).map(|n| n as i64),
        local_id: value
            .get("local_id")
            .and_then(Value::as_f64)
            .map(|n| n as i64),
        status: value
            .get("status")
            .and_then(Value::as_str)
            .map(str::to_owned),
        fields: BTreeMap::new(),
        mismatched: Vec::new(),
        unopened: Vec::new(),
    };
    let mut content = None;

    for (name, value) in properties {
        let field = match value {
            Value::String(s) if looks_sealed(s) => match key.open(s) {
                Ok(opened) => {
                    let text = Zeroizing::new(String::from_utf8_lossy(&opened).into_owned());
                    if name == "content" {
                        content = Some(text);
                        continue;
                    }
                    Field::Text(text)
                }
                // A plain value that merely looks like base64 does not open
                // either; only fields Termius seals are worth a warning.
                Err(_) if is_sealed_by_termius(s) => {
                    record.unopened.push(name.clone());
                    continue;
                }
                Err(_) => Field::Text(Zeroizing::new(s.clone())),
            },
            Value::String(s) => Field::Text(Zeroizing::new(s.clone())),
            Value::Number(n) => Field::Number(*n),
            Value::Bool(b) => Field::Bool(*b),
            object @ Value::Object(_) => {
                let id = object.get("id").and_then(Value::as_f64).map(|n| n as i64);
                let local_id = object
                    .get("local_id")
                    .and_then(Value::as_f64)
                    .map(|n| n as i64);
                if id.is_none() && local_id.is_none() {
                    continue;
                }
                Field::Ref(Ref { id, local_id })
            }
            _ => continue,
        };
        record.fields.insert(name.clone(), field);
    }

    if let Some(content) = content {
        overlay_content(&mut record, &content);
    }
    Some(record)
}

fn overlay_content(record: &mut Record, content: &str) {
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(content)
    else {
        record.unopened.push("content".into());
        return;
    };
    for (name, value) in map {
        let field = match value {
            serde_json::Value::String(s) => Field::Text(Zeroizing::new(s)),
            serde_json::Value::Number(n) => match n.as_f64() {
                Some(n) => Field::Number(n),
                None => continue,
            },
            serde_json::Value::Bool(b) => Field::Bool(b),
            _ => continue,
        };
        if let (Some(Field::Text(old)), Field::Text(new)) = (record.fields.get(&name), &field) {
            if old.as_str() != new.as_str() {
                record.mismatched.push(name.clone());
            }
        }
        record.fields.insert(name, field);
    }
}

fn looks_sealed(s: &str) -> bool {
    s.len() >= 40
        && s.len().is_multiple_of(4)
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
}

/// Termius' format byte, 4, makes every sealed field start with `BA`.
fn is_sealed_by_termius(s: &str) -> bool {
    s.starts_with("BA")
}
