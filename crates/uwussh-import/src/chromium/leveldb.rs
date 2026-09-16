//! Just enough LevelDB to read a database another process owns.
//!
//! Not a LevelDB implementation: no iterators, no comparator, no compaction.
//! It reads every table and log file in the directory and keeps, per key, the
//! entry with the highest sequence number — which is the database's current
//! state, whatever order the files come in. That sidesteps what makes opening
//! Chromium's databases with a real LevelDB awkward: IndexedDB registers its
//! own comparator, `idb_cmp1`, and a LevelDB without it refuses the database.
//!
//! The owning app may be writing while this reads. A torn record at the end of
//! a log fails its checksum and is dropped — that write had not happened yet —
//! and a table still being written by a compaction has no footer yet and is
//! skipped, its contents still being in the files it is compacting.

use super::varint;
use std::collections::HashMap;
use std::path::Path;

const TABLE_MAGIC: u64 = 0xdb47_7524_8b80_fb57;
const FOOTER_LEN: usize = 48;
const BLOCK_TRAILER_LEN: usize = 5;
const LOG_BLOCK: usize = 32 * 1024;
const LOG_HEADER_LEN: usize = 7;

#[derive(Debug, thiserror::Error)]
#[error("could not read {path}: {source}")]
pub struct LevelDbError {
    pub path: String,
    #[source]
    pub source: std::io::Error,
}

/// The live contents of a database at the moment it was read.
#[derive(Debug, Default)]
pub struct Snapshot {
    pub entries: HashMap<Vec<u8>, Vec<u8>>,
    /// Files that could not be parsed, with the reason. Usually empty; when not,
    /// the caller should say that the read may be incomplete.
    pub unreadable: Vec<(String, &'static str)>,
}

/// Read every `.ldb`, `.sst` and `.log` file in `dir`.
pub fn read_dir(dir: &Path) -> Result<Snapshot, LevelDbError> {
    let io = |source| LevelDbError {
        path: dir.display().to_string(),
        source,
    };
    let mut builder = Builder::default();
    let mut unreadable = Vec::new();

    for entry in std::fs::read_dir(dir).map_err(io)? {
        let path = entry.map_err(io)?.path();
        let Some(extension) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let parsed = match extension {
            "ldb" | "sst" => read_file(&path)?.and_then(|data| read_table(&data, &mut builder)),
            "log" => read_file(&path)?.map(|data| read_log(&data, &mut builder)),
            _ => continue,
        };
        if let Err(problem) = parsed {
            unreadable.push((name, problem));
        }
    }

    Ok(builder.finish(unreadable))
}

/// A file that vanished between listing and reading was deleted by a
/// compaction; its contents live on in the compaction's output.
fn read_file(path: &Path) -> Result<Result<Vec<u8>, &'static str>, LevelDbError> {
    match std::fs::read(path) {
        Ok(data) => Ok(Ok(data)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Err("deleted while reading")),
        Err(source) => Err(LevelDbError {
            path: path.display().to_string(),
            source,
        }),
    }
}

#[derive(Default)]
struct Builder {
    latest: HashMap<Vec<u8>, (u64, Option<Vec<u8>>)>,
}

impl Builder {
    fn put(&mut self, key: &[u8], sequence: u64, value: Option<&[u8]>) {
        match self.latest.get(key) {
            Some((seen, _)) if *seen >= sequence => {}
            _ => {
                self.latest
                    .insert(key.to_vec(), (sequence, value.map(<[u8]>::to_vec)));
            }
        }
    }

    fn finish(self, unreadable: Vec<(String, &'static str)>) -> Snapshot {
        let entries = self
            .latest
            .into_iter()
            .filter_map(|(key, (_, value))| Some((key, value?)))
            .collect();
        Snapshot {
            entries,
            unreadable,
        }
    }
}

// ── Tables ─────────────────────────────────────────────────────────────────

fn read_table(data: &[u8], builder: &mut Builder) -> Result<(), &'static str> {
    if data.len() < FOOTER_LEN {
        return Err("table shorter than its footer");
    }
    let footer = &data[data.len() - FOOTER_LEN..];
    if u64::from_le_bytes(footer[40..48].try_into().unwrap()) != TABLE_MAGIC {
        return Err("table without footer, probably still being written");
    }
    let mut pos = 0;
    let _metaindex = block_handle(footer, &mut pos)?;
    let index = block_handle(footer, &mut pos)?;

    let index = read_block(data, index)?;
    for (_, handle) in block_entries(&index)? {
        let block = read_block(data, block_handle(handle, &mut 0)?)?;
        for (internal_key, value) in block_entries(&block)? {
            let (user_key, sequence, is_value) = split_internal_key(&internal_key)?;
            builder.put(user_key, sequence, is_value.then_some(value));
        }
    }
    Ok(())
}

fn block_handle(data: &[u8], pos: &mut usize) -> Result<(usize, usize), &'static str> {
    let offset = varint(data, pos).ok_or("bad block handle")?;
    let size = varint(data, pos).ok_or("bad block handle")?;
    Ok((offset as usize, size as usize))
}

fn read_block(data: &[u8], (offset, size): (usize, usize)) -> Result<Vec<u8>, &'static str> {
    let end = offset
        .checked_add(size)
        .filter(|end| end + BLOCK_TRAILER_LEN <= data.len())
        .ok_or("block outside the file")?;
    let contents = &data[offset..end];
    let kind = data[end];
    let stored = u32::from_le_bytes(data[end + 1..end + 5].try_into().unwrap());
    if unmask(stored) != crc32c_extend(crc32c(contents), &[kind]) {
        return Err("block checksum mismatch");
    }
    match kind {
        0 => Ok(contents.to_vec()),
        1 => snap::raw::Decoder::new()
            .decompress_vec(contents)
            .map_err(|_| "bad snappy block"),
        _ => Err("unknown block compression"),
    }
}

/// A key, owned because it is rebuilt from shared prefixes, and its value.
type BlockEntry<'a> = (Vec<u8>, &'a [u8]);

/// Keys in a block share prefixes with the key before them.
fn block_entries(block: &[u8]) -> Result<Vec<BlockEntry<'_>>, &'static str> {
    const BAD: &str = "malformed block";
    let restarts = block
        .len()
        .checked_sub(4)
        .map(|at| u32::from_le_bytes(block[at..].try_into().unwrap()) as usize)
        .ok_or(BAD)?;
    let entries_end = restarts
        .checked_mul(4)
        .and_then(|len| block.len().checked_sub(4 + len))
        .ok_or(BAD)?;

    let mut entries = Vec::new();
    let mut key: Vec<u8> = Vec::new();
    let mut pos = 0;
    while pos < entries_end {
        let shared = varint(block, &mut pos).ok_or(BAD)? as usize;
        let unshared = varint(block, &mut pos).ok_or(BAD)? as usize;
        let value_len = varint(block, &mut pos).ok_or(BAD)? as usize;
        let key_end = pos.checked_add(unshared).ok_or(BAD)?;
        let value_end = key_end.checked_add(value_len).ok_or(BAD)?;
        if shared > key.len() || value_end > entries_end {
            return Err(BAD);
        }
        key.truncate(shared);
        key.extend_from_slice(&block[pos..key_end]);
        entries.push((key.clone(), &block[key_end..value_end]));
        pos = value_end;
    }
    Ok(entries)
}

/// An internal key is the user's key followed by `sequence << 8 | kind`.
fn split_internal_key(key: &[u8]) -> Result<(&[u8], u64, bool), &'static str> {
    let at = key.len().checked_sub(8).ok_or("internal key too short")?;
    let tag = u64::from_le_bytes(key[at..].try_into().unwrap());
    Ok((&key[..at], tag >> 8, tag & 0xff == 1))
}

// ── Logs ───────────────────────────────────────────────────────────────────

fn read_log(data: &[u8], builder: &mut Builder) {
    let mut fragments: Option<Vec<u8>> = None;

    for block in data.chunks(LOG_BLOCK) {
        let mut pos = 0;
        while pos + LOG_HEADER_LEN <= block.len() {
            let stored = u32::from_le_bytes(block[pos..pos + 4].try_into().unwrap());
            let len = u16::from_le_bytes([block[pos + 4], block[pos + 5]]) as usize;
            let kind = block[pos + 6];
            let start = pos + LOG_HEADER_LEN;
            // Zeroes are preallocated space nobody has written to yet; a record
            // running past its block was torn mid-write. Either way, nothing
            // more in this block.
            if (kind == 0 && len == 0) || start + len > block.len() {
                break;
            }
            let payload = &block[start..start + len];
            if unmask(stored) != crc32c_extend(crc32c(&[kind]), payload) {
                fragments = None;
                break;
            }
            match kind {
                1 => apply_batch(payload, builder),
                2 => fragments = Some(payload.to_vec()),
                3 => {
                    if let Some(record) = fragments.as_mut() {
                        record.extend_from_slice(payload);
                    }
                }
                4 => {
                    if let Some(mut record) = fragments.take() {
                        record.extend_from_slice(payload);
                        apply_batch(&record, builder);
                    }
                }
                _ => break,
            }
            pos = start + len;
        }
    }
}

/// A write batch: a starting sequence number, a count, then puts and deletes
/// that each take the next sequence number.
fn apply_batch(batch: &[u8], builder: &mut Builder) {
    if batch.len() < 12 {
        return;
    }
    let first = u64::from_le_bytes(batch[..8].try_into().unwrap());
    let count = u32::from_le_bytes(batch[8..12].try_into().unwrap());
    let mut pos = 12;

    for sequence in first..first.saturating_add(u64::from(count)) {
        let Some(&kind) = batch.get(pos) else { return };
        pos += 1;
        let Some(key) = length_prefixed(batch, &mut pos) else {
            return;
        };
        let value = match kind {
            1 => match length_prefixed(batch, &mut pos) {
                Some(value) => Some(value),
                None => return,
            },
            0 => None,
            _ => return,
        };
        builder.put(key, sequence, value);
    }
}

fn length_prefixed<'a>(data: &'a [u8], pos: &mut usize) -> Option<&'a [u8]> {
    let len = varint(data, pos)? as usize;
    let slice = data.get(*pos..pos.checked_add(len)?)?;
    *pos += len;
    Some(slice)
}

// ── Checksums ──────────────────────────────────────────────────────────────

const CRC32C_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0x82f6_3b78
            } else {
                crc >> 1
            };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
};

fn crc32c(data: &[u8]) -> u32 {
    crc32c_extend(0, data)
}

fn crc32c_extend(crc: u32, data: &[u8]) -> u32 {
    let mut crc = !crc;
    for &byte in data {
        crc = CRC32C_TABLE[((crc ^ u32::from(byte)) & 0xff) as usize] ^ (crc >> 8);
    }
    !crc
}

/// LevelDB stores checksums masked, so a checksum of data that itself contains
/// checksums does not degenerate.
fn unmask(masked: u32) -> u32 {
    let rot = masked.wrapping_sub(0xa282_ead8);
    rot.rotate_left(15)
}

/// Writers for the tests, here and in the layers above. They produce files a
/// real LevelDB would accept, which is what makes the reader's tests mean
/// something.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    fn mask(crc: u32) -> u32 {
        crc.rotate_right(15).wrapping_add(0xa282_ead8)
    }

    fn put_varint(out: &mut Vec<u8>, mut value: u64) {
        while value >= 0x80 {
            out.push((value as u8) | 0x80);
            value >>= 7;
        }
        out.push(value as u8);
    }

    pub enum Op<'a> {
        Put(&'a [u8], &'a [u8]),
        Delete(&'a [u8]),
    }

    pub fn batch(sequence: u64, ops: &[Op]) -> Vec<u8> {
        let mut out = sequence.to_le_bytes().to_vec();
        out.extend_from_slice(&(ops.len() as u32).to_le_bytes());
        for op in ops {
            match op {
                Op::Put(key, value) => {
                    out.push(1);
                    put_varint(&mut out, key.len() as u64);
                    out.extend_from_slice(key);
                    put_varint(&mut out, value.len() as u64);
                    out.extend_from_slice(value);
                }
                Op::Delete(key) => {
                    out.push(0);
                    put_varint(&mut out, key.len() as u64);
                    out.extend_from_slice(key);
                }
            }
        }
        out
    }

    /// Lay batches out as log records, fragmenting across blocks like LevelDB.
    pub fn log(batches: &[Vec<u8>]) -> Vec<u8> {
        let mut out = Vec::new();
        for batch in batches {
            let mut rest = &batch[..];
            let mut first = true;
            loop {
                let left_in_block = LOG_BLOCK - out.len() % LOG_BLOCK;
                if left_in_block < LOG_HEADER_LEN {
                    out.resize(out.len() + left_in_block, 0);
                    continue;
                }
                let take = rest.len().min(left_in_block - LOG_HEADER_LEN);
                let last = take == rest.len();
                let kind = match (first, last) {
                    (true, true) => 1,
                    (true, false) => 2,
                    (false, false) => 3,
                    (false, true) => 4,
                };
                let payload = &rest[..take];
                let crc = mask(crc32c_extend(crc32c(&[kind]), payload));
                out.extend_from_slice(&crc.to_le_bytes());
                out.extend_from_slice(&(take as u16).to_le_bytes());
                out.push(kind);
                out.extend_from_slice(payload);
                rest = &rest[take..];
                first = false;
                if last {
                    break;
                }
            }
        }
        out
    }

    fn block(entries: &[(Vec<u8>, Vec<u8>)], compress: bool) -> Vec<u8> {
        let mut contents = Vec::new();
        let mut restarts = Vec::new();
        let mut previous: &[u8] = &[];
        for (i, (key, value)) in entries.iter().enumerate() {
            // A restart every other entry, so both shared prefixes and restart
            // points get exercised.
            let shared = if i % 2 == 0 {
                restarts.push(contents.len() as u32);
                0
            } else {
                previous.iter().zip(key).take_while(|(a, b)| a == b).count()
            };
            put_varint(&mut contents, shared as u64);
            put_varint(&mut contents, (key.len() - shared) as u64);
            put_varint(&mut contents, value.len() as u64);
            contents.extend_from_slice(&key[shared..]);
            contents.extend_from_slice(value);
            previous = key.as_slice();
        }
        for restart in &restarts {
            contents.extend_from_slice(&restart.to_le_bytes());
        }
        contents.extend_from_slice(&(restarts.len() as u32).to_le_bytes());

        let (contents, kind) = if compress {
            (
                snap::raw::Encoder::new().compress_vec(&contents).unwrap(),
                1,
            )
        } else {
            (contents, 0)
        };
        let crc = mask(crc32c_extend(crc32c(&contents), &[kind]));
        let mut out = contents;
        out.push(kind);
        out.extend_from_slice(&crc.to_le_bytes());
        out
    }

    /// A table with one data block per chunk of `per_block` entries.
    pub fn table(entries: &[(&str, u64, Option<&str>)], per_block: usize) -> Vec<u8> {
        let mut out = Vec::new();
        let mut index = Vec::new();
        for (n, chunk) in entries.chunks(per_block).enumerate() {
            let internal: Vec<(Vec<u8>, Vec<u8>)> = chunk
                .iter()
                .map(|(key, sequence, value)| {
                    let mut internal = key.as_bytes().to_vec();
                    let tag = (sequence << 8) | u64::from(value.is_some());
                    internal.extend_from_slice(&tag.to_le_bytes());
                    (internal, value.unwrap_or_default().as_bytes().to_vec())
                })
                .collect();
            let data = block(&internal, n % 2 == 1);
            let mut handle = Vec::new();
            put_varint(&mut handle, out.len() as u64);
            put_varint(&mut handle, (data.len() - BLOCK_TRAILER_LEN) as u64);
            index.push((internal.last().unwrap().0.clone(), handle));
            out.extend_from_slice(&data);
        }

        let metaindex = block(&[], false);
        let metaindex_offset = out.len();
        out.extend_from_slice(&metaindex);
        let index_block = block(&index, false);
        let index_offset = out.len();
        out.extend_from_slice(&index_block);

        let mut footer = Vec::new();
        put_varint(&mut footer, metaindex_offset as u64);
        put_varint(&mut footer, (metaindex.len() - BLOCK_TRAILER_LEN) as u64);
        put_varint(&mut footer, index_offset as u64);
        put_varint(&mut footer, (index_block.len() - BLOCK_TRAILER_LEN) as u64);
        footer.resize(40, 0);
        footer.extend_from_slice(&TABLE_MAGIC.to_le_bytes());
        out.extend_from_slice(&footer);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{batch, log, table, Op};
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("uwussh-leveldb-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn value<'a>(snapshot: &'a Snapshot, key: &str) -> Option<&'a str> {
        snapshot
            .entries
            .get(key.as_bytes())
            .map(|v| std::str::from_utf8(v).unwrap())
    }

    #[test]
    fn crc32c_matches_the_published_check_value() {
        assert_eq!(crc32c(b"123456789"), 0xe306_9283);
    }

    #[test]
    fn the_newest_write_wins_across_tables_and_logs() {
        let dir = scratch("newest");
        std::fs::write(
            dir.join("000005.ldb"),
            table(
                &[
                    ("alpha", 1, Some("old")),
                    ("beta", 2, Some("kept")),
                    ("gamma", 3, Some("doomed")),
                ],
                2,
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join("000007.log"),
            log(&[batch(
                10,
                &[Op::Put(b"alpha", b"new"), Op::Delete(b"gamma")],
            )]),
        )
        .unwrap();

        let snapshot = read_dir(&dir).unwrap();
        assert_eq!(value(&snapshot, "alpha"), Some("new"));
        assert_eq!(value(&snapshot, "beta"), Some("kept"));
        assert_eq!(value(&snapshot, "gamma"), None, "deleted in the log");
        assert!(snapshot.unreadable.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn file_order_does_not_decide_which_write_wins() {
        let dir = scratch("order");
        // The log sorts first by name but holds the older write.
        std::fs::write(
            dir.join("000001.log"),
            log(&[batch(1, &[Op::Put(b"key", b"older")])]),
        )
        .unwrap();
        std::fs::write(
            dir.join("000009.ldb"),
            table(&[("key", 40, Some("newer"))], 1),
        )
        .unwrap();

        assert_eq!(value(&read_dir(&dir).unwrap(), "key"), Some("newer"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn records_larger_than_a_block_are_reassembled() {
        let big = vec![b'x'; LOG_BLOCK * 2 + 123];
        let mut builder = Builder::default();
        read_log(
            &log(&[
                batch(1, &[Op::Put(b"small", b"1")]),
                batch(2, &[Op::Put(b"big", &big)]),
                batch(3, &[Op::Put(b"after", b"2")]),
            ]),
            &mut builder,
        );
        let snapshot = builder.finish(Vec::new());
        assert_eq!(snapshot.entries[&b"big"[..]].len(), big.len());
        assert_eq!(value(&snapshot, "small"), Some("1"));
        assert_eq!(value(&snapshot, "after"), Some("2"));
    }

    #[test]
    fn a_write_torn_off_mid_record_is_dropped_and_earlier_ones_stay() {
        let mut data = log(&[
            batch(1, &[Op::Put(b"done", b"yes")]),
            batch(2, &[Op::Put(b"torn", b"half of this never made it")]),
        ]);
        data.truncate(data.len() - 5);

        let mut builder = Builder::default();
        read_log(&data, &mut builder);
        let snapshot = builder.finish(Vec::new());
        assert_eq!(value(&snapshot, "done"), Some("yes"));
        assert_eq!(value(&snapshot, "torn"), None);
    }

    #[test]
    fn a_corrupted_record_is_not_believed() {
        let mut data = log(&[batch(1, &[Op::Put(b"key", b"value")])]);
        let last = data.len() - 1;
        data[last] ^= 0xff;

        let mut builder = Builder::default();
        read_log(&data, &mut builder);
        assert!(builder.finish(Vec::new()).entries.is_empty());
    }

    #[test]
    fn a_table_still_being_written_is_reported_not_fatal() {
        let dir = scratch("partial");
        let mut partial = table(&[("key", 1, Some("value"))], 1);
        partial.truncate(partial.len() - 10);
        std::fs::write(dir.join("000003.ldb"), partial).unwrap();
        std::fs::write(
            dir.join("000004.log"),
            log(&[batch(2, &[Op::Put(b"other", b"fine")])]),
        )
        .unwrap();

        let snapshot = read_dir(&dir).unwrap();
        assert_eq!(value(&snapshot, "other"), Some("fine"));
        assert_eq!(snapshot.unreadable.len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
