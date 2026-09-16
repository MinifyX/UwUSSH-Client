//! Reading what a Chromium-based app — Electron included — keeps in IndexedDB.
//!
//! Termius has no export. What it has is an Electron IndexedDB on disk, and
//! that format is Chromium's, not Termius': LevelDB files, IndexedDB's key
//! encoding on top, V8's structured clone for the values. All three are pinned
//! down in Chromium's source and change far less often than any one app does,
//! so this is a reader for a file format rather than a scraper for one app's
//! internals. What is specific to Termius lives in `crate::termius`.
//!
//! Read-only by construction: files are read into memory and parsed, nothing
//! is ever opened for writing, and the owning app can keep running meanwhile.

pub mod idb;
pub mod leveldb;
pub mod v8;

/// LevelDB's and IndexedDB's variable-length integer: 7 bits per byte, low
/// bits first.
pub(crate) fn varint(data: &[u8], pos: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *data.get(*pos)?;
        *pos += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::varint;

    #[test]
    fn varints_decode_across_byte_boundaries() {
        let mut pos = 0;
        assert_eq!(varint(&[0x96, 0x01, 0x05], &mut pos), Some(150));
        assert_eq!(pos, 2);
        assert_eq!(varint(&[0x96, 0x01, 0x05], &mut pos), Some(5));
    }

    #[test]
    fn a_truncated_varint_is_none() {
        assert_eq!(varint(&[0x96], &mut 0), None);
    }
}
