//! V8's structured-clone wire format, the way IndexedDB values are stored
//! (`v8/src/objects/value-serializer.cc`).
//!
//! Only the reading half, and only the value types an app puts in its database
//! on purpose: plain objects and arrays, strings, numbers, booleans, dates,
//! maps, sets, byte buffers. Host objects such as `File` or `CryptoKey` are an
//! error rather than a guess.

use super::varint;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    BigInt {
        negative: bool,
        digits: Vec<u8>,
    },
    String(String),
    /// Milliseconds since the epoch, as JavaScript keeps them.
    Date(f64),
    RegExp {
        pattern: String,
        flags: u64,
    },
    Bytes(Vec<u8>),
    Array(Vec<Value>),
    /// Properties in the order they were serialized, which is the order the
    /// object had them in.
    Object(Vec<(String, Value)>),
    Map(Vec<(Value, Value)>),
    Set(Vec<Value>),
}

impl Value {
    /// A property of an object; `None` for anything that is not one.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(properties) => properties
                .iter()
                .find_map(|(name, value)| (name == key).then_some(value)),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }

    /// `null` and `undefined`, which apps use interchangeably for "not set".
    pub fn is_nullish(&self) -> bool {
        matches!(self, Value::Null | Value::Undefined)
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum V8Error {
    #[error("the value ends early")]
    Truncated,
    #[error("unsupported tag {0:#04x}")]
    UnsupportedTag(u8),
    #[error("nested too deeply")]
    TooDeep,
    #[error("malformed value: {0}")]
    Malformed(&'static str),
}

/// Deserialize one value, starting at V8's own `0xFF <version>` header.
pub fn deserialize(data: &[u8]) -> Result<Value, V8Error> {
    let mut reader = Reader {
        data,
        pos: 0,
        version: 0,
        objects: Vec::new(),
        depth: 0,
    };
    if reader.tag()? != tag::VERSION {
        return Err(V8Error::Malformed("no version header"));
    }
    reader.version = reader.varint()?;
    reader.value()
}

mod tag {
    pub const VERSION: u8 = 0xff;
    pub const PADDING: u8 = 0;
    pub const VERIFY_OBJECT_COUNT: u8 = b'?';
    pub const THE_HOLE: u8 = b'-';
    pub const UNDEFINED: u8 = b'_';
    pub const NULL: u8 = b'0';
    pub const TRUE: u8 = b'T';
    pub const FALSE: u8 = b'F';
    pub const INT32: u8 = b'I';
    pub const UINT32: u8 = b'U';
    pub const DOUBLE: u8 = b'N';
    pub const BIGINT: u8 = b'Z';
    pub const UTF8_STRING: u8 = b'S';
    pub const ONE_BYTE_STRING: u8 = b'"';
    pub const TWO_BYTE_STRING: u8 = b'c';
    pub const OBJECT_REFERENCE: u8 = b'^';
    pub const BEGIN_OBJECT: u8 = b'o';
    pub const END_OBJECT: u8 = b'{';
    pub const BEGIN_SPARSE_ARRAY: u8 = b'a';
    pub const END_SPARSE_ARRAY: u8 = b'@';
    pub const BEGIN_DENSE_ARRAY: u8 = b'A';
    pub const END_DENSE_ARRAY: u8 = b'$';
    pub const DATE: u8 = b'D';
    pub const TRUE_OBJECT: u8 = b'y';
    pub const FALSE_OBJECT: u8 = b'x';
    pub const NUMBER_OBJECT: u8 = b'n';
    pub const BIGINT_OBJECT: u8 = b'z';
    pub const STRING_OBJECT: u8 = b's';
    pub const REGEXP: u8 = b'R';
    pub const BEGIN_MAP: u8 = b';';
    pub const END_MAP: u8 = b':';
    pub const BEGIN_SET: u8 = b'\'';
    pub const END_SET: u8 = b',';
    pub const ARRAY_BUFFER: u8 = b'B';
    pub const RESIZABLE_ARRAY_BUFFER: u8 = b'~';
    pub const ARRAY_BUFFER_VIEW: u8 = b'V';
}

/// Deeper than any real record, shallow enough that a hostile file cannot
/// overflow the stack.
const MAX_DEPTH: usize = 128;

/// Sparse arrays may claim a length far beyond their bytes (`a[1000] = 1`),
/// so they get a fixed ceiling instead of the bytes-left check.
const MAX_SPARSE_LEN: usize = 1 << 16;

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    version: u64,
    /// Objects by the id V8 gave them, for back-references. `None` while an
    /// object is still being read, which is what a cycle would point at.
    objects: Vec<Option<Value>>,
    depth: usize,
}

impl Reader<'_> {
    fn byte(&mut self) -> Result<u8, V8Error> {
        let byte = *self.data.get(self.pos).ok_or(V8Error::Truncated)?;
        self.pos += 1;
        Ok(byte)
    }

    fn tag(&mut self) -> Result<u8, V8Error> {
        loop {
            match self.byte()? {
                tag::PADDING => continue,
                tag => return Ok(tag),
            }
        }
    }

    fn peek_tag(&mut self) -> Result<u8, V8Error> {
        let start = self.pos;
        let tag = self.tag();
        self.pos = start;
        tag
    }

    fn varint(&mut self) -> Result<u64, V8Error> {
        varint(self.data, &mut self.pos).ok_or(V8Error::Truncated)
    }

    fn length(&mut self) -> Result<usize, V8Error> {
        let len = self.varint()? as usize;
        // Every element takes at least one byte, so a length beyond what is
        // left is corrupt — and must not reach an allocation.
        if len > self.data.len() - self.pos {
            return Err(V8Error::Truncated);
        }
        Ok(len)
    }

    fn bytes(&mut self, len: usize) -> Result<&[u8], V8Error> {
        let end = self.pos.checked_add(len).ok_or(V8Error::Truncated)?;
        let bytes = self.data.get(self.pos..end).ok_or(V8Error::Truncated)?;
        self.pos = end;
        Ok(bytes)
    }

    fn double(&mut self) -> Result<f64, V8Error> {
        Ok(f64::from_le_bytes(self.bytes(8)?.try_into().unwrap()))
    }

    fn reserve_id(&mut self) -> usize {
        self.objects.push(None);
        self.objects.len() - 1
    }

    fn remember(&mut self, id: usize, value: Value) -> Value {
        self.objects[id] = Some(value.clone());
        value
    }

    fn value(&mut self) -> Result<Value, V8Error> {
        if self.depth >= MAX_DEPTH {
            return Err(V8Error::TooDeep);
        }
        self.depth += 1;
        let value = self.value_inner();
        self.depth -= 1;
        value
    }

    fn value_inner(&mut self) -> Result<Value, V8Error> {
        let tag = self.tag()?;
        Ok(match tag {
            tag::VERIFY_OBJECT_COUNT => {
                self.varint()?;
                return self.value_inner();
            }
            tag::UNDEFINED | tag::THE_HOLE => Value::Undefined,
            tag::NULL => Value::Null,
            tag::TRUE => Value::Bool(true),
            tag::FALSE => Value::Bool(false),
            tag::INT32 => {
                let raw = self.varint()? as u32;
                Value::Number(f64::from((raw >> 1) as i32 ^ -((raw & 1) as i32)))
            }
            tag::UINT32 => Value::Number(self.varint()? as u32 as f64),
            tag::DOUBLE => Value::Number(self.double()?),
            tag::BIGINT => self.bigint()?,
            tag::UTF8_STRING | tag::ONE_BYTE_STRING | tag::TWO_BYTE_STRING => {
                Value::String(self.string_body(tag)?)
            }
            tag::OBJECT_REFERENCE => {
                let id = self.varint()? as usize;
                match self.objects.get(id) {
                    Some(Some(value)) => value.clone(),
                    Some(None) => return Err(V8Error::Malformed("cyclic reference")),
                    None => return Err(V8Error::Malformed("reference to an unknown object")),
                }
            }
            tag::BEGIN_OBJECT => {
                let id = self.reserve_id();
                let mut properties = Vec::new();
                while self.peek_tag()? != tag::END_OBJECT {
                    let key = self.property_key()?;
                    properties.push((key, self.value()?));
                }
                self.tag()?;
                self.varint()?;
                self.remember(id, Value::Object(properties))
            }
            tag::BEGIN_DENSE_ARRAY => {
                let id = self.reserve_id();
                let len = self.length()?;
                let mut items = Vec::with_capacity(len);
                for _ in 0..len {
                    items.push(self.value()?);
                }
                // Named properties on an array are legal and never what a
                // record means; read past them.
                while self.peek_tag()? != tag::END_DENSE_ARRAY {
                    self.property_key()?;
                    self.value()?;
                }
                self.tag()?;
                self.varint()?;
                self.varint()?;
                self.remember(id, Value::Array(items))
            }
            tag::BEGIN_SPARSE_ARRAY => {
                let id = self.reserve_id();
                let len = self.varint()? as usize;
                if len > MAX_SPARSE_LEN {
                    return Err(V8Error::Malformed("sparse array too long"));
                }
                let mut items = vec![Value::Undefined; len];
                while self.peek_tag()? != tag::END_SPARSE_ARRAY {
                    let key = self.property_key()?;
                    let value = self.value()?;
                    if let Some(slot) = key.parse::<usize>().ok().and_then(|i| items.get_mut(i)) {
                        *slot = value;
                    }
                }
                self.tag()?;
                self.varint()?;
                self.varint()?;
                self.remember(id, Value::Array(items))
            }
            tag::DATE => {
                let id = self.reserve_id();
                let millis = self.double()?;
                self.remember(id, Value::Date(millis))
            }
            tag::TRUE_OBJECT | tag::FALSE_OBJECT => {
                let id = self.reserve_id();
                self.remember(id, Value::Bool(tag == tag::TRUE_OBJECT))
            }
            tag::NUMBER_OBJECT => {
                let id = self.reserve_id();
                let n = self.double()?;
                self.remember(id, Value::Number(n))
            }
            tag::BIGINT_OBJECT => {
                let id = self.reserve_id();
                let n = self.bigint()?;
                self.remember(id, n)
            }
            tag::STRING_OBJECT => {
                let id = self.reserve_id();
                let s = self.string()?;
                self.remember(id, Value::String(s))
            }
            tag::REGEXP => {
                let id = self.reserve_id();
                let pattern = self.string()?;
                let flags = self.varint()?;
                self.remember(id, Value::RegExp { pattern, flags })
            }
            tag::BEGIN_MAP => {
                let id = self.reserve_id();
                let mut entries = Vec::new();
                while self.peek_tag()? != tag::END_MAP {
                    let key = self.value()?;
                    entries.push((key, self.value()?));
                }
                self.tag()?;
                self.varint()?;
                self.remember(id, Value::Map(entries))
            }
            tag::BEGIN_SET => {
                let id = self.reserve_id();
                let mut items = Vec::new();
                while self.peek_tag()? != tag::END_SET {
                    items.push(self.value()?);
                }
                self.tag()?;
                self.varint()?;
                self.remember(id, Value::Set(items))
            }
            tag::ARRAY_BUFFER | tag::RESIZABLE_ARRAY_BUFFER => {
                let id = self.reserve_id();
                let len = self.length()?;
                if tag == tag::RESIZABLE_ARRAY_BUFFER {
                    self.varint()?;
                }
                let buffer = self.bytes(len)?.to_vec();
                self.remember(id, Value::Bytes(buffer.clone()));
                if self.peek_tag().ok() == Some(tag::ARRAY_BUFFER_VIEW) {
                    self.tag()?;
                    return self.view(&buffer);
                }
                Value::Bytes(buffer)
            }
            other => return Err(V8Error::UnsupportedTag(other)),
        })
    }

    /// A typed array or `DataView` over the buffer just read.
    fn view(&mut self, buffer: &[u8]) -> Result<Value, V8Error> {
        let id = self.reserve_id();
        let _subtag = self.byte()?;
        let offset = self.varint()? as usize;
        let len = self.varint()? as usize;
        if self.version >= 14 {
            self.varint()?;
        }
        let bytes = offset
            .checked_add(len)
            .and_then(|end| buffer.get(offset..end))
            .ok_or(V8Error::Malformed("view outside its buffer"))?;
        Ok(self.remember(id, Value::Bytes(bytes.to_vec())))
    }

    fn bigint(&mut self) -> Result<Value, V8Error> {
        let bitfield = self.varint()?;
        let len = usize::try_from(bitfield >> 1).map_err(|_| V8Error::Truncated)?;
        Ok(Value::BigInt {
            negative: bitfield & 1 == 1,
            digits: self.bytes(len)?.to_vec(),
        })
    }

    /// A string wherever the format requires one, such as a regexp's pattern.
    fn string(&mut self) -> Result<String, V8Error> {
        match self.tag()? {
            tag @ (tag::UTF8_STRING | tag::ONE_BYTE_STRING | tag::TWO_BYTE_STRING) => {
                self.string_body(tag)
            }
            _ => Err(V8Error::Malformed("expected a string")),
        }
    }

    fn string_body(&mut self, tag: u8) -> Result<String, V8Error> {
        let len = self.varint()? as usize;
        let bytes = self.bytes(len)?;
        Ok(match tag {
            // Latin-1: every byte is the code point of the same number.
            tag::ONE_BYTE_STRING => bytes.iter().map(|&b| char::from(b)).collect(),
            tag::TWO_BYTE_STRING => {
                let (pairs, odd) = bytes.as_chunks::<2>();
                if !odd.is_empty() {
                    return Err(V8Error::Malformed("odd two-byte string"));
                }
                let units: Vec<u16> = pairs.iter().map(|&pair| u16::from_le_bytes(pair)).collect();
                String::from_utf16_lossy(&units)
            }
            _ => String::from_utf8_lossy(bytes).into_owned(),
        })
    }

    /// Property names are strings, except integer-like ones, which V8 writes
    /// as numbers.
    fn property_key(&mut self) -> Result<String, V8Error> {
        match self.value()? {
            Value::String(s) => Ok(s),
            Value::Number(n) if n.fract() == 0.0 && n.abs() < 9.0e15 => Ok(format!("{}", n as i64)),
            Value::Number(n) => Ok(n.to_string()),
            _ => Err(V8Error::Malformed("property key is not a string or number")),
        }
    }
}

/// A writer for the tests in this module and the layers above: just enough of
/// the format to build real records by hand.
#[cfg(test)]
pub(crate) mod testing {
    use super::tag;

    pub struct Writer(pub Vec<u8>);

    impl Writer {
        pub fn new() -> Self {
            Writer(vec![tag::VERSION, 15])
        }

        fn varint(&mut self, mut value: u64) -> &mut Self {
            while value >= 0x80 {
                self.0.push((value as u8) | 0x80);
                value >>= 7;
            }
            self.0.push(value as u8);
            self
        }

        pub fn raw(&mut self, bytes: &[u8]) -> &mut Self {
            self.0.extend_from_slice(bytes);
            self
        }

        pub fn string(&mut self, s: &str) -> &mut Self {
            if s.is_ascii() {
                self.0.push(tag::ONE_BYTE_STRING);
                self.varint(s.len() as u64);
                self.0.extend_from_slice(s.as_bytes());
            } else {
                let units: Vec<u16> = s.encode_utf16().collect();
                let byte_len = units.len() as u64 * 2;
                // V8 pads so the two-byte payload starts on an even offset.
                let varint_len = (64 - byte_len.leading_zeros()).div_ceil(7).max(1) as usize;
                if (self.0.len() + 1 + varint_len) % 2 == 1 {
                    self.0.push(tag::PADDING);
                }
                self.0.push(tag::TWO_BYTE_STRING);
                self.varint(byte_len);
                for unit in units {
                    self.0.extend_from_slice(&unit.to_le_bytes());
                }
            }
            self
        }

        pub fn int(&mut self, n: i32) -> &mut Self {
            self.0.push(tag::INT32);
            self.varint(u64::from(((n << 1) ^ (n >> 31)) as u32))
        }

        pub fn double(&mut self, n: f64) -> &mut Self {
            self.0.push(tag::DOUBLE);
            self.raw(&n.to_le_bytes())
        }

        pub fn null(&mut self) -> &mut Self {
            self.raw(&[tag::NULL])
        }

        pub fn bool(&mut self, b: bool) -> &mut Self {
            self.raw(&[if b { tag::TRUE } else { tag::FALSE }])
        }

        pub fn begin_object(&mut self) -> &mut Self {
            self.raw(&[tag::BEGIN_OBJECT])
        }

        pub fn end_object(&mut self, properties: u64) -> &mut Self {
            self.0.push(tag::END_OBJECT);
            self.varint(properties)
        }

        pub fn begin_array(&mut self, len: u64) -> &mut Self {
            self.0.push(tag::BEGIN_DENSE_ARRAY);
            self.varint(len)
        }

        pub fn end_array(&mut self, len: u64) -> &mut Self {
            self.0.push(tag::END_DENSE_ARRAY);
            self.varint(0).varint(len)
        }

        pub fn reference(&mut self, id: u64) -> &mut Self {
            self.0.push(tag::OBJECT_REFERENCE);
            self.varint(id)
        }

        pub fn bytes(&self) -> Vec<u8> {
            self.0.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::Writer;
    use super::*;

    fn object(properties: &[(&str, Value)]) -> Value {
        Value::Object(
            properties
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        )
    }

    #[test]
    fn a_plain_object_reads_back_in_property_order() {
        // What V8 writes for {a: 1, b: "x"}, byte for byte.
        let bytes = [
            0xff, 0x0f, b'o', b'"', 1, b'a', b'I', 2, b'"', 1, b'b', b'"', 1, b'x', b'{', 2,
        ];
        assert_eq!(
            deserialize(&bytes).unwrap(),
            object(&[("a", Value::Number(1.0)), ("b", Value::String("x".into()))])
        );
    }

    #[test]
    fn nested_values_of_every_common_type() {
        let bytes = Writer::new()
            .begin_object()
            .string("label")
            .string("Grüße, Nyu ✿")
            .string("port")
            .int(-22)
            .string("ratio")
            .double(0.5)
            .string("unset")
            .null()
            .string("enabled")
            .bool(true)
            .string("tags")
            .begin_array(2)
            .string("homelab")
            .int(7)
            .end_array(2)
            .end_object(6)
            .bytes();

        let value = deserialize(&bytes).unwrap();
        assert_eq!(
            value.get("label").and_then(Value::as_str),
            Some("Grüße, Nyu ✿")
        );
        assert_eq!(value.get("port").and_then(Value::as_f64), Some(-22.0));
        assert_eq!(value.get("ratio").and_then(Value::as_f64), Some(0.5));
        assert!(value.get("unset").unwrap().is_nullish());
        assert_eq!(value.get("enabled").and_then(Value::as_bool), Some(true));
        assert_eq!(
            value.get("tags").and_then(Value::as_array).unwrap(),
            &[Value::String("homelab".into()), Value::Number(7.0)]
        );
    }

    #[test]
    fn a_repeated_object_is_read_through_its_reference() {
        // const inner = {}; ({first: inner, second: inner})
        let bytes = Writer::new()
            .begin_object()
            .string("first")
            .begin_object()
            .string("id")
            .int(3)
            .end_object(1)
            .string("second")
            .reference(1)
            .end_object(2)
            .bytes();

        let value = deserialize(&bytes).unwrap();
        assert_eq!(value.get("first"), value.get("second"));
        assert_eq!(
            value.get("second").unwrap().get("id"),
            Some(&Value::Number(3.0))
        );
    }

    #[test]
    fn a_reference_to_an_object_still_being_read_is_an_error() {
        let bytes = Writer::new()
            .begin_object()
            .string("me")
            .reference(0)
            .end_object(1)
            .bytes();
        assert_eq!(
            deserialize(&bytes),
            Err(V8Error::Malformed("cyclic reference"))
        );
    }

    #[test]
    fn integer_property_names_come_back_as_strings() {
        let bytes = Writer::new()
            .begin_object()
            .int(42)
            .string("answer")
            .end_object(1)
            .bytes();
        assert_eq!(
            deserialize(&bytes)
                .unwrap()
                .get("42")
                .and_then(Value::as_str),
            Some("answer")
        );
    }

    #[test]
    fn dates_maps_and_byte_buffers() {
        let mut writer = Writer::new();
        writer
            .begin_object()
            .string("updated")
            .raw(b"D")
            .raw(&1_700_000_000_000f64.to_le_bytes())
            .string("seen")
            .raw(b";")
            .string("k")
            .int(1)
            .raw(&[b':', 2])
            .string("blob")
            .raw(&[b'B', 4, 1, 2, 3, 4])
            .raw(&[b'V', b'B', 1, 2, 0])
            .end_object(3);

        let value = deserialize(&writer.bytes()).unwrap();
        assert_eq!(
            value.get("updated"),
            Some(&Value::Date(1_700_000_000_000.0))
        );
        assert_eq!(
            value.get("seen"),
            Some(&Value::Map(vec![(
                Value::String("k".into()),
                Value::Number(1.0)
            )]))
        );
        assert_eq!(value.get("blob"), Some(&Value::Bytes(vec![2, 3])));
    }

    #[test]
    fn broken_input_is_an_error_not_a_panic() {
        let full = Writer::new()
            .begin_object()
            .string("label")
            .string("web")
            .end_object(1)
            .bytes();
        for cut in 0..full.len() {
            assert!(deserialize(&full[..cut]).is_err(), "cut at {cut}");
        }

        assert_eq!(
            deserialize(&[0xff, 0x0f, b'\\']),
            Err(V8Error::UnsupportedTag(b'\\'))
        );
        // An array claiming more elements than there are bytes left.
        assert_eq!(
            deserialize(&[0xff, 0x0f, b'A', 0xff, 0xff, 0xff, 0xff, 0x0f]),
            Err(V8Error::Truncated)
        );
    }

    #[test]
    fn deeply_nested_input_cannot_overflow_the_stack() {
        let mut writer = Writer::new();
        for _ in 0..10_000 {
            writer.begin_array(1);
        }
        assert_eq!(deserialize(&writer.bytes()), Err(V8Error::TooDeep));
    }
}
