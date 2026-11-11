//! [`Data`], the owned value tree of plain data, and its wire form.

use djs_core::wire::{self, Writer, tag};

use crate::Error;

/// Plain data as an owned tree: what crosses the boundary without JSON.
///
/// In the VM, `Object` is an ordinary object (prototype `Object.prototype`) with these
/// properties in this order (index keys sort first, as in JS), `Array` an array, `Bytes` a
/// `Uint8Array`. Read from the VM, strings that are not well-formed UTF-16 (lone surrogates)
/// get U+FFFD in their place, as `TextEncoder` does; numbers are `f64`.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Data {
    #[default]
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    /// A `Uint8Array`
    Bytes(Vec<u8>),
    /// Elements, holes as [`Data::Hole`]
    Array(Vec<Data>),
    /// Own enumerable properties, in property order
    Object(Vec<(String, Data)>),
    /// A missing array element (`[1, , 3]`); only meaningful inside an `Array`
    Hole,
}

impl Data {
    /// An object from `(key, value)` pairs.
    pub fn object<K: Into<String>>(entries: impl IntoIterator<Item = (K, Data)>) -> Data {
        Data::Object(entries.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    /// The property `key` of an object; `None` for other values or a missing key.
    pub fn get(&self, key: &str) -> Option<&Data> {
        match self {
            Data::Object(entries) => entries.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Data::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Data::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Data::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Data::Bytes(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Data]> {
        match self {
            Data::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&[(String, Data)]> {
        match self {
            Data::Object(entries) => Some(entries),
            _ => None,
        }
    }

    pub fn is_undefined(&self) -> bool {
        matches!(self, Data::Undefined)
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Data::Null)
    }

    /// The wire form (docs/wire.md). A [`Data::Hole`] outside an array makes a buffer
    /// that does not decode.
    pub fn to_wire(&self) -> Vec<u8> {
        let mut w = Writer::new();
        write(&mut w, self);
        w.finish()
    }

    /// Reads a wire buffer, as written by [`Data::to_wire`], `Instance` results
    /// ([`crate::ValueRef::to_wire`]) or a JS host's `encode`. UTF-16 strings with lone
    /// surrogates get U+FFFD. Corrupt input is [`Error::Data`].
    pub fn from_wire(bytes: &[u8]) -> Result<Data, Error> {
        Reader::new(bytes).read()
    }
}

impl From<bool> for Data {
    fn from(b: bool) -> Data {
        Data::Bool(b)
    }
}

impl From<f64> for Data {
    fn from(n: f64) -> Data {
        Data::Number(n)
    }
}

impl From<i32> for Data {
    fn from(n: i32) -> Data {
        Data::Number(n.into())
    }
}

impl From<u32> for Data {
    fn from(n: u32) -> Data {
        Data::Number(n.into())
    }
}

impl From<&str> for Data {
    fn from(s: &str) -> Data {
        Data::String(s.into())
    }
}

impl From<String> for Data {
    fn from(s: String) -> Data {
        Data::String(s)
    }
}

impl From<Vec<u8>> for Data {
    fn from(b: Vec<u8>) -> Data {
        Data::Bytes(b)
    }
}

impl From<Vec<Data>> for Data {
    fn from(items: Vec<Data>) -> Data {
        Data::Array(items)
    }
}

fn write(w: &mut Writer, v: &Data) {
    match v {
        Data::Undefined => w.undefined(),
        Data::Null => w.null(),
        Data::Bool(b) => w.bool(*b),
        Data::Number(n) => w.number(*n),
        Data::String(s) => w.str(s),
        Data::Bytes(b) => w.bytes(b),
        Data::Hole => w.hole(),
        Data::Array(items) => {
            w.array(items.len());
            for it in items {
                write(w, it);
            }
        }
        Data::Object(entries) => {
            w.object(entries.len());
            for (k, v) in entries {
                w.key(k);
                write(w, v);
            }
        }
    }
}

/// Nesting the reader accepts: the VM's decoder and encoder limit (`JSON.parse`'s).
const MAX_DEPTH: usize = 5000;

/// A wire reader with an explicit stack (deep nesting cannot overflow the host's).
struct Reader<'a> {
    input: &'a [u8],
    pos: usize,
    keys: Vec<String>,
}

/// An open container: what it holds so far and how many members are still to come.
enum Open {
    Array(Vec<Data>, u32),
    Object(Vec<(String, Data)>, u32, Option<String>),
}

impl<'a> Reader<'a> {
    fn new(input: &'a [u8]) -> Reader<'a> {
        Reader {
            input,
            pos: 0,
            keys: Vec::new(),
        }
    }

    fn fail<T>(&self, reason: &str) -> Result<T, Error> {
        Err(Error::Data(format!(
            "invalid wire data at offset {}: {reason}",
            self.pos
        )))
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        if n > self.input.len() - self.pos {
            return self.fail("unexpected end");
        }
        let s = &self.input[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn byte(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }

    fn varint(&mut self) -> Result<u32, Error> {
        let mut v: u64 = 0;
        for i in 0..5 {
            let b = self.byte()?;
            v |= u64::from(b & 0x7F) << (7 * i);
            if b & 0x80 == 0 {
                return u32::try_from(v).or_else(|_| self.fail("varint overflow"));
            }
        }
        self.fail("varint too long")
    }

    fn utf8(&mut self, n: usize) -> Result<String, Error> {
        let bytes = self.take(n)?;
        match std::str::from_utf8(bytes) {
            Ok(s) => Ok(s.to_owned()),
            Err(_) => self.fail("invalid UTF-8"),
        }
    }

    /// A string payload after its tag.
    fn string(&mut self, t: u8) -> Result<String, Error> {
        match t {
            tag::STR_UTF8 => {
                let _units = self.varint()?;
                let n = self.varint()? as usize;
                self.utf8(n)
            }
            tag::STR_UTF8_UNCOUNTED => {
                let n = self.varint()? as usize;
                self.utf8(n)
            }
            _ => {
                let units = self.varint()? as usize;
                let data = self.take(units.saturating_mul(2))?;
                let units = data
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|p| u16::from_le_bytes(*p));
                Ok(char::decode_utf16(units)
                    .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
                    .collect())
            }
        }
    }

    fn key(&mut self) -> Result<String, Error> {
        let k = self.varint()?;
        if k > 0 {
            return match self.keys.get(k as usize - 1) {
                Some(s) => Ok(s.clone()),
                None => self.fail("key index out of range"),
            };
        }
        let t = self.byte()?;
        if !matches!(t, tag::STR_UTF8 | tag::STR_UTF16 | tag::STR_UTF8_UNCOUNTED) {
            return self.fail("expected a key string");
        }
        let s = self.string(t)?;
        self.keys.push(s.clone());
        Ok(s)
    }

    fn read(mut self) -> Result<Data, Error> {
        let header = self.take(4)?;
        if header[..3] != wire::MAGIC || header[3] != wire::VERSION {
            self.pos = 0;
            return self.fail("bad header");
        }
        let mut stack: Vec<Open> = Vec::new();
        loop {
            if matches!(stack.last(), Some(Open::Object(..))) {
                let k = self.key()?;
                if let Some(Open::Object(_, _, key)) = stack.last_mut() {
                    *key = Some(k);
                }
            }
            let t = self.byte()?;
            let mut v = match t {
                tag::UNDEFINED => Data::Undefined,
                tag::NULL => Data::Null,
                tag::FALSE => Data::Bool(false),
                tag::TRUE => Data::Bool(true),
                tag::INT8 => Data::Number(f64::from(self.byte()? as i8)),
                tag::INT32 => {
                    let b = self.take(4)?;
                    Data::Number(f64::from(i32::from_le_bytes([b[0], b[1], b[2], b[3]])))
                }
                tag::FLOAT64 => {
                    let b = self.take(8)?;
                    Data::Number(f64::from_le_bytes(b.try_into().expect("8 bytes")))
                }
                tag::HOLE if matches!(stack.last(), Some(Open::Array(..))) => Data::Hole,
                tag::STR_UTF8 | tag::STR_UTF16 | tag::STR_UTF8_UNCOUNTED => {
                    Data::String(self.string(t)?)
                }
                tag::BYTES => {
                    let n = self.varint()? as usize;
                    Data::Bytes(self.take(n)?.to_vec())
                }
                tag::ARRAY | tag::OBJECT => {
                    let n = self.varint()?;
                    // Each member takes at least one byte (a property two).
                    let min = n as usize * if t == tag::OBJECT { 2 } else { 1 };
                    if min > self.input.len() - self.pos {
                        return self.fail("count exceeds the input");
                    }
                    if n > 0 {
                        if stack.len() >= MAX_DEPTH {
                            return self.fail("nesting too deep");
                        }
                        stack.push(if t == tag::OBJECT {
                            Open::Object(Vec::with_capacity(n as usize), n, None)
                        } else {
                            Open::Array(Vec::with_capacity(n as usize), n)
                        });
                        continue;
                    }
                    if t == tag::OBJECT {
                        Data::Object(Vec::new())
                    } else {
                        Data::Array(Vec::new())
                    }
                }
                _ => {
                    self.pos -= 1;
                    return self.fail("unknown tag");
                }
            };
            // Close every container that is now complete.
            loop {
                let Some(top) = stack.last_mut() else {
                    if self.pos != self.input.len() {
                        return self.fail("trailing bytes");
                    }
                    return Ok(v);
                };
                let left = match top {
                    Open::Array(items, left) => {
                        items.push(v);
                        left
                    }
                    Open::Object(entries, left, key) => {
                        entries.push((key.take().unwrap_or_default(), v));
                        left
                    }
                };
                *left -= 1;
                if *left > 0 {
                    break;
                }
                v = match stack.pop().expect("an open container") {
                    Open::Array(items, _) => Data::Array(items),
                    Open::Object(entries, _, _) => Data::Object(entries),
                };
            }
        }
    }
}

/// Builds a [`Data`] tree from the outbound encoder's calls ([`wire::encode_to`]): what
/// [`Data::from_wire`] of the encoded buffer gives, without the buffer.
#[derive(Default)]
pub(crate) struct Builder {
    stack: Vec<Open>,
    done: Option<Data>,
    /// The tree's host memory so far in `bytes` (nodes and payloads), for the encoder's
    /// cap: a tree takes more memory than the wire form, so the cap counts what it costs
    size: usize,
}

impl Builder {
    pub(crate) fn finish(self) -> Data {
        self.done.unwrap_or(Data::Undefined)
    }

    /// Stores a value in the open container, closing every container that is now complete.
    fn put(&mut self, mut v: Data) {
        self.size += size_of::<Data>();
        loop {
            let Some(top) = self.stack.last_mut() else {
                self.done = Some(v);
                return;
            };
            let left = match top {
                Open::Array(items, left) => {
                    items.push(v);
                    left
                }
                Open::Object(entries, left, key) => {
                    entries.push((key.take().unwrap_or_default(), v));
                    left
                }
            };
            *left -= 1;
            if *left > 0 {
                return;
            }
            v = match self.stack.pop().expect("an open container") {
                Open::Array(items, _) => Data::Array(items),
                Open::Object(entries, _, _) => Data::Object(entries),
            };
        }
    }

    fn string(&mut self, s: String) {
        self.size += s.len();
        self.put(Data::String(s));
    }
}

fn latin1(units: &[u8]) -> String {
    if units.is_ascii() {
        return String::from_utf8(units.to_vec()).expect("ASCII");
    }
    units.iter().map(|&b| char::from(b)).collect()
}

fn utf16_lossy(units: &[u16]) -> String {
    char::decode_utf16(units.iter().copied())
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

impl wire::Sink for Builder {
    fn written(&self) -> usize {
        self.size
    }
    fn undefined(&mut self) {
        self.put(Data::Undefined)
    }
    fn null(&mut self) {
        self.put(Data::Null)
    }
    fn bool(&mut self, b: bool) {
        self.put(Data::Bool(b))
    }
    fn number(&mut self, n: f64) {
        self.put(Data::Number(n))
    }
    fn hole(&mut self) {
        self.put(Data::Hole)
    }
    fn str_latin1(&mut self, units: &[u8]) {
        self.string(latin1(units))
    }
    fn str(&mut self, s: &str) {
        self.string(s.to_owned())
    }
    fn str_utf16(&mut self, units: &[u16]) {
        self.string(utf16_lossy(units))
    }
    fn bytes(&mut self, b: &[u8]) {
        self.size += b.len();
        self.put(Data::Bytes(b.to_vec()))
    }
    fn array(&mut self, count: usize) {
        match u32::try_from(count) {
            Ok(n) if n > 0 => self.stack.push(Open::Array(Vec::with_capacity(count), n)),
            _ => self.put(Data::Array(Vec::new())),
        }
    }
    fn object(&mut self, count: usize) {
        match u32::try_from(count) {
            Ok(n) if n > 0 => self
                .stack
                .push(Open::Object(Vec::with_capacity(count), n, None)),
            _ => self.put(Data::Object(Vec::new())),
        }
    }
    fn key(&mut self, k: &str) {
        self.size += size_of::<String>() + k.len();
        if let Some(Open::Object(_, _, key)) = self.stack.last_mut() {
            *key = Some(k.to_owned());
        }
    }
    fn key_utf16(&mut self, units: &[u16]) {
        self.size += size_of::<String>() + units.len() * 3;
        if let Some(Open::Object(_, _, key)) = self.stack.last_mut() {
            *key = Some(utf16_lossy(units));
        }
    }
}
