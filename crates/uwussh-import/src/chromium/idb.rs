//! IndexedDB's layout inside LevelDB, as Chromium writes it
//! (`content/browser/indexed_db/indexed_db_leveldb_coding.cc`), and Blink's
//! envelope around each value (`serialized_script_value.cc`,
//! `idb_value_wrapping.cc`).
//!
//! Every LevelDB key starts with a prefix naming a database, an object store
//! and an index. Database `0` is global metadata, which maps database names to
//! ids; store `0` of a database is its metadata, which names its stores; index
//! `1` of a store is the records themselves. Everything else — secondary
//! indexes, blob journals, free lists — is ignored.

use super::{leveldb::Snapshot, v8, varint};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug)]
pub struct Database {
    /// The origin that owns the database; `file__0` for an Electron app.
    pub origin: String,
    pub name: String,
    pub stores: Vec<ObjectStore>,
}

#[derive(Debug)]
pub struct ObjectStore {
    pub name: String,
    pub records: Vec<Record>,
}

#[derive(Debug)]
pub struct Record {
    pub key: Key,
    /// Per record, so one unreadable value costs that record and not the
    /// store.
    pub value: Result<v8::Value, ValueError>,
}

/// An IndexedDB key.
#[derive(Debug, Clone, PartialEq)]
pub enum Key {
    Null,
    String(String),
    Date(f64),
    Number(f64),
    Array(Vec<Key>),
    Binary(Vec<u8>),
    MinKey,
    /// A key this reader could not decode; the record is kept regardless.
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ValueError {
    #[error(transparent)]
    V8(#[from] v8::V8Error),
    #[error("the value is stored outside the database, as a blob")]
    InBlob,
    #[error("the compressed value does not decompress")]
    Compression,
    #[error("the value's envelope is malformed")]
    Envelope,
}

const GLOBAL_DATABASE_NAME: u8 = 201;
const OBJECT_STORE_METADATA: u8 = 50;
const OBJECT_STORE_NAME: u8 = 0;
const RECORDS_INDEX: u64 = 1;

/// `(database, object store, index)` from a key's prefix.
type Prefix = (u64, u64, u64);
/// A store's raw records, key bytes to value bytes.
type RawRecords<'a> = BTreeMap<&'a [u8], &'a [u8]>;

/// Every database in the snapshot, stores and records sorted by name and key
/// bytes so repeated reads come out in the same order.
pub fn databases(snapshot: &Snapshot) -> Vec<Database> {
    let mut database_names: HashMap<u64, (String, String)> = HashMap::new();
    let mut store_names: HashMap<(u64, u64), String> = HashMap::new();
    let mut records: HashMap<(u64, u64), RawRecords> = HashMap::new();

    for (key, value) in &snapshot.entries {
        let Some((prefix, rest)) = split_prefix(key) else {
            continue;
        };
        match (prefix, rest.first()) {
            ((0, 0, 0), Some(&GLOBAL_DATABASE_NAME)) => {
                let mut pos = 1;
                if let (Some(origin), Some(name)) = (
                    string_with_length(rest, &mut pos),
                    string_with_length(rest, &mut pos),
                ) {
                    database_names.insert(little_endian(value), (origin, name));
                }
            }
            ((database, 0, 0), Some(&OBJECT_STORE_METADATA)) if database != 0 => {
                let mut pos = 1;
                if let Some(store) = varint(rest, &mut pos) {
                    if rest.get(pos) == Some(&OBJECT_STORE_NAME) && pos + 1 == rest.len() {
                        store_names.insert((database, store), utf16_be(value));
                    }
                }
            }
            ((database, store, RECORDS_INDEX), _) if database != 0 && store != 0 => {
                records
                    .entry((database, store))
                    .or_default()
                    .insert(rest, value);
            }
            _ => {}
        }
    }

    let mut databases: Vec<Database> = database_names
        .into_iter()
        .map(|(id, (origin, name))| {
            let mut stores: Vec<ObjectStore> = store_names
                .iter()
                .filter(|((database, _), _)| *database == id)
                .map(|(&(_, store), name)| ObjectStore {
                    name: name.clone(),
                    records: records
                        .remove(&(id, store))
                        .unwrap_or_default()
                        .into_iter()
                        .map(|(key, value)| Record {
                            key: decode_key(key, &mut 0).unwrap_or(Key::Unreadable),
                            value: decode_value(value),
                        })
                        .collect(),
                })
                .collect();
            stores.sort_by(|a, b| a.name.cmp(&b.name));
            Database {
                origin,
                name,
                stores,
            }
        })
        .collect();
    databases.sort_by(|a, b| (&a.origin, &a.name).cmp(&(&b.origin, &b.name)));
    databases
}

/// `(database, object store, index)` and the rest of the key.
///
/// The first byte packs the byte widths of the three ids — 3, 3 and 2 bits,
/// each stored minus one — and the ids follow, little-endian.
fn split_prefix(key: &[u8]) -> Option<(Prefix, &[u8])> {
    let first = *key.first()?;
    let widths = [
        usize::from((first >> 5) & 0b111) + 1,
        usize::from((first >> 2) & 0b111) + 1,
        usize::from(first & 0b11) + 1,
    ];
    let mut pos = 1;
    let mut ids = [0u64; 3];
    for (id, width) in ids.iter_mut().zip(widths) {
        *id = little_endian(key.get(pos..pos + width)?);
        pos += width;
    }
    Some(((ids[0], ids[1], ids[2]), &key[pos..]))
}

fn little_endian(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .take(8)
        .rev()
        .fold(0, |acc, &b| (acc << 8) | u64::from(b))
}

/// A varint count of UTF-16 code units, then the units, big-endian.
fn string_with_length(data: &[u8], pos: &mut usize) -> Option<String> {
    let units = usize::try_from(varint(data, pos)?).ok()?;
    let bytes = data.get(*pos..pos.checked_add(units.checked_mul(2)?)?)?;
    *pos += bytes.len();
    Some(utf16_be(bytes))
}

fn utf16_be(bytes: &[u8]) -> String {
    let (pairs, _) = bytes.as_chunks::<2>();
    let units: Vec<u16> = pairs.iter().map(|&pair| u16::from_be_bytes(pair)).collect();
    String::from_utf16_lossy(&units)
}

/// Array keys nest; a real key is a handful of levels deep, and two bytes per
/// level must not be enough to overflow the stack.
const MAX_KEY_DEPTH: usize = 32;

/// Snappy says how large a value will be before it is inflated. Termius'
/// records are kilobytes; anything past this is not one of them.
pub(crate) const MAX_DECOMPRESSED: usize = 64 * 1024 * 1024;

fn decode_key(data: &[u8], pos: &mut usize) -> Option<Key> {
    decode_key_at(data, pos, 0)
}

fn decode_key_at(data: &[u8], pos: &mut usize, depth: usize) -> Option<Key> {
    if depth > MAX_KEY_DEPTH {
        return None;
    }
    let kind = *data.get(*pos)?;
    *pos += 1;
    Some(match kind {
        0 => Key::Null,
        1 => Key::String(string_with_length(data, pos)?),
        2 | 3 => {
            let bytes = data.get(*pos..pos.checked_add(8)?)?;
            *pos += 8;
            let n = f64::from_le_bytes(bytes.try_into().ok()?);
            if kind == 2 {
                Key::Date(n)
            } else {
                Key::Number(n)
            }
        }
        4 => {
            let len = usize::try_from(varint(data, pos)?).ok()?;
            let mut items = Vec::new();
            for _ in 0..len {
                items.push(decode_key_at(data, pos, depth + 1)?);
            }
            Key::Array(items)
        }
        5 => Key::MinKey,
        6 => {
            let len = usize::try_from(varint(data, pos)?).ok()?;
            let bytes = data.get(*pos..pos.checked_add(len)?)?;
            *pos += len;
            Key::Binary(bytes.to_vec())
        }
        _ => return None,
    })
}

/// A stored value: IndexedDB's record version, then what Blink serialized.
fn decode_value(stored: &[u8]) -> Result<v8::Value, ValueError> {
    let mut pos = 0;
    varint(stored, &mut pos).ok_or(ValueError::Envelope)?;
    let serialized = &stored[pos..];

    // Values Blink had to process before storing carry a pseudo-version:
    // replaced by a blob, or compressed.
    let decompressed;
    let serialized = match serialized {
        [0xff, 0x11, 0x01, ..] => return Err(ValueError::InBlob),
        [0xff, 0x11, 0x02, compressed @ ..] => {
            let size =
                snap::raw::decompress_len(compressed).map_err(|_| ValueError::Compression)?;
            if size > MAX_DECOMPRESSED {
                return Err(ValueError::Compression);
            }
            decompressed = snap::raw::Decoder::new()
                .decompress_vec(compressed)
                .map_err(|_| ValueError::Compression)?;
            &decompressed[..]
        }
        [0xff, 0x11, ..] => return Err(ValueError::Envelope),
        _ => serialized,
    };

    Ok(v8::deserialize(skip_blink_header(serialized))?)
}

/// Blink's `0xFF <version>` header and, since version 21, a trailer offset
/// (`0xFE`, eight bytes of offset, four of size) — then V8's own header. With
/// no second header the first one was already V8's.
fn skip_blink_header(serialized: &[u8]) -> &[u8] {
    if serialized.first() != Some(&0xff) {
        return serialized;
    }
    let mut pos = 1;
    if varint(serialized, &mut pos).is_none() {
        return serialized;
    }
    if serialized.get(pos) == Some(&0xfe) {
        pos += 13;
    }
    match serialized.get(pos) {
        Some(&0xff) => &serialized[pos..],
        _ => serialized,
    }
}

/// Builders for IndexedDB's keys and values, shared with the Termius tests.
#[cfg(test)]
pub(crate) mod testing {
    fn prefix(database: u8, store: u8, index: u8) -> Vec<u8> {
        vec![0, database, store, index]
    }

    fn string_with_length(out: &mut Vec<u8>, s: &str) {
        let units: Vec<u16> = s.encode_utf16().collect();
        assert!(units.len() < 128, "test strings stay short");
        out.push(units.len() as u8);
        for unit in units {
            out.extend_from_slice(&unit.to_be_bytes());
        }
    }

    pub fn database_name(origin: &str, name: &str, id: u8) -> (Vec<u8>, Vec<u8>) {
        let mut key = prefix(0, 0, 0);
        key.push(super::GLOBAL_DATABASE_NAME);
        string_with_length(&mut key, origin);
        string_with_length(&mut key, name);
        (key, vec![id])
    }

    pub fn store_name(database: u8, store: u8, name: &str) -> (Vec<u8>, Vec<u8>) {
        let mut key = prefix(database, 0, 0);
        key.extend_from_slice(&[
            super::OBJECT_STORE_METADATA,
            store,
            super::OBJECT_STORE_NAME,
        ]);
        let value = name.encode_utf16().flat_map(u16::to_be_bytes).collect();
        (key, value)
    }

    /// A record under a string key, its value wrapped the way current Blink
    /// writes it: record version, Blink header with trailer offset, then V8.
    pub fn record(database: u8, store: u8, key: &str, v8: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let mut full_key = prefix(database, store, 1);
        full_key.push(1);
        string_with_length(&mut full_key, key);
        let mut value = vec![1, 0xff, 21, 0xfe];
        value.extend_from_slice(&[0; 12]);
        value.extend_from_slice(v8);
        (full_key, value)
    }

    /// The same, compressed the way Blink compresses large values.
    pub fn compressed_record(database: u8, store: u8, key: &str, v8: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let (full_key, value) = record(database, store, key, v8);
        let mut wrapped = vec![value[0], 0xff, 0x11, 0x02];
        wrapped.extend(snap::raw::Encoder::new().compress_vec(&value[1..]).unwrap());
        (full_key, wrapped)
    }
}

#[cfg(test)]
mod tests {
    use super::super::v8::testing::Writer;
    use super::testing::*;
    use super::*;

    fn host(label: &str) -> Vec<u8> {
        Writer::new()
            .begin_object()
            .string("label")
            .string(label)
            .end_object(1)
            .bytes()
    }

    fn snapshot(entries: Vec<(Vec<u8>, Vec<u8>)>) -> Snapshot {
        Snapshot {
            entries: entries.into_iter().collect(),
            unreadable: Vec::new(),
        }
    }

    #[test]
    fn databases_stores_and_records_come_out_by_name() {
        let mut secondary_index_key = record(1, 1, "a", &[]).0;
        secondary_index_key[3] = 30;

        let databases = databases(&snapshot(vec![
            database_name("file__0", "termius", 1),
            store_name(1, 1, "hosts"),
            store_name(1, 2, "groups"),
            record(1, 1, "b", &host("web")),
            compressed_record(1, 1, "a", &host("prox")),
            record(1, 2, "g", &host("homelab")),
            // A secondary index entry: same store, not a record.
            (secondary_index_key, vec![1, 2, 3]),
        ]));

        assert_eq!(databases.len(), 1);
        let database = &databases[0];
        assert_eq!(
            (database.origin.as_str(), database.name.as_str()),
            ("file__0", "termius")
        );
        let names: Vec<_> = database.stores.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["groups", "hosts"]);

        let hosts = &database.stores[1];
        assert_eq!(hosts.records.len(), 2);
        let labels: Vec<_> = hosts
            .records
            .iter()
            .map(|r| {
                let value = r.value.as_ref().unwrap();
                (
                    r.key.clone(),
                    value.get("label").unwrap().as_str().unwrap().to_owned(),
                )
            })
            .collect();
        assert_eq!(
            labels,
            [
                (Key::String("a".into()), "prox".to_owned()),
                (Key::String("b".into()), "web".to_owned())
            ]
        );
    }

    #[test]
    fn values_without_a_blink_header_still_read() {
        let (key, mut value) = record(1, 1, "k", &host("bare"));
        // Record version stays; Blink's header and trailer offset go.
        value.drain(1..16);
        let databases = databases(&snapshot(vec![
            database_name("file__0", "db", 1),
            store_name(1, 1, "s"),
            (key, value),
        ]));
        let record = &databases[0].stores[0].records[0];
        assert_eq!(
            record
                .value
                .as_ref()
                .unwrap()
                .get("label")
                .unwrap()
                .as_str(),
            Some("bare")
        );
    }

    #[test]
    fn one_unreadable_value_costs_one_record() {
        let (key, mut value) = record(1, 1, "broken", &host("x"));
        value.truncate(value.len() - 3);
        let databases = databases(&snapshot(vec![
            database_name("file__0", "db", 1),
            store_name(1, 1, "s"),
            (key, value),
            record(1, 1, "fine", &host("y")),
        ]));
        let records = &databases[0].stores[0].records;
        assert_eq!(records.len(), 2);
        for record in records {
            match &record.key {
                Key::String(k) if k == "broken" => assert!(record.value.is_err()),
                _ => assert!(record.value.is_ok()),
            }
        }
    }

    #[test]
    fn blob_backed_values_are_named_not_guessed() {
        let (key, _) = record(1, 1, "big", &[]);
        let databases = databases(&snapshot(vec![
            database_name("file__0", "db", 1),
            store_name(1, 1, "s"),
            (key, vec![1, 0xff, 0x11, 0x01, 0x10, 0x00]),
        ]));
        assert_eq!(
            databases[0].stores[0].records[0].value,
            Err(ValueError::InBlob)
        );
    }

    #[test]
    fn wide_ids_in_the_prefix_decode() {
        // Database 1 (one byte), store 300 (two bytes), index 1 (one byte):
        // widths minus one are 0, 1 and 0, packed as 3, 3 and 2 bits.
        let key = [0b0000_0100, 1, 0x2c, 0x01, 1, 0xaa];
        let ((database, store, index), rest) = split_prefix(&key).unwrap();
        assert_eq!((database, store, index), (1, 300, 1));
        assert_eq!(rest, &[0xaa]);
    }

    #[test]
    fn keys_of_every_type_decode() {
        let mut data = vec![4, 3];
        data.extend_from_slice(&[3]);
        data.extend_from_slice(&7.0f64.to_le_bytes());
        data.extend_from_slice(&[1, 2, 0, b'h', 0, b'i']);
        data.extend_from_slice(&[6, 2, 0xde, 0xad]);
        assert_eq!(
            decode_key(&data, &mut 0),
            Some(Key::Array(vec![
                Key::Number(7.0),
                Key::String("hi".into()),
                Key::Binary(vec![0xde, 0xad]),
            ]))
        );
    }

    #[test]
    fn deeply_nested_array_keys_are_refused_not_a_stack_overflow() {
        let mut data = Vec::new();
        for _ in 0..100_000 {
            data.extend_from_slice(&[4, 1]);
        }
        data.push(0);
        assert_eq!(decode_key(&data, &mut 0), None);
    }
}
