//! A Rust encoder of the wire format (docs/wire.md), for Rust hosts: what js/src/wire.ts is
//! for JS hosts. The writer only emits; it does not check the structure (that a container
//! gets as many elements as it announced), which the decoder does.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use super::{MAGIC, VERSION, tag};

/// Writes one wire buffer: the header, then exactly one value. A container is announced
/// with its element count ([`Writer::array`], [`Writer::object`]) and followed by that many
/// values (an object's each after its [`Writer::key`]).
///
/// ```ignore
/// let mut w = Writer::new();
/// w.object(2);
/// w.key("method");
/// w.str("GET");
/// w.key("headers");
/// w.array(0);
/// let bytes = w.finish();
/// ```
#[derive(Clone, Debug)]
pub struct Writer {
    out: Vec<u8>,
    /// The key table: each key's index (`k - 1` on the wire)
    keys: BTreeMap<Vec<u8>, u32>,
}

impl Default for Writer {
    fn default() -> Self {
        Writer::new()
    }
}

impl Writer {
    pub fn new() -> Writer {
        Writer::with_capacity(64)
    }

    /// A writer whose buffer starts with room for `bytes`.
    pub fn with_capacity(bytes: usize) -> Writer {
        let mut out = Vec::with_capacity(bytes.max(4));
        out.extend_from_slice(&MAGIC);
        out.push(VERSION);
        Writer {
            out,
            keys: BTreeMap::new(),
        }
    }

    /// The buffer.
    pub fn finish(self) -> Vec<u8> {
        self.out
    }

    fn varint(&mut self, mut v: u32) {
        while v >= 0x80 {
            self.out.push((v as u8 & 0x7F) | 0x80);
            v >>= 7;
        }
        self.out.push(v as u8);
    }

    /// The bytes written so far (the header included).
    pub fn written(&self) -> usize {
        self.out.len()
    }

    /// A length as a varint; lengths beyond `u32` cannot be decoded anyway.
    fn count(&mut self, n: usize) {
        self.varint(u32::try_from(n).unwrap_or(u32::MAX));
    }

    pub fn undefined(&mut self) {
        self.out.push(tag::UNDEFINED);
    }

    pub fn null(&mut self) {
        self.out.push(tag::NULL);
    }

    pub fn bool(&mut self, b: bool) {
        self.out.push(if b { tag::TRUE } else { tag::FALSE });
    }

    /// A number, in the smallest form that keeps it exactly (`-0` stays a float64).
    pub fn number(&mut self, n: f64) {
        let i = n as i32;
        if i as f64 == n && !(i == 0 && n.is_sign_negative()) {
            if let Ok(b) = i8::try_from(i) {
                self.out.push(tag::INT8);
                self.out.push(b as u8);
            } else {
                self.out.push(tag::INT32);
                self.out.extend_from_slice(&i.to_le_bytes());
            }
        } else {
            self.out.push(tag::FLOAT64);
            self.out.extend_from_slice(&n.to_le_bytes());
        }
    }

    /// A hole: only as an array element.
    pub fn hole(&mut self) {
        self.out.push(tag::HOLE);
    }

    /// A string, as UTF-8 without a unit count (the decoder counts).
    pub fn str(&mut self, s: &str) {
        self.out.push(tag::STR_UTF8_UNCOUNTED);
        self.count(s.len());
        self.out.extend_from_slice(s.as_bytes());
    }

    /// A string of UTF-16 code units: for strings that are not well-formed (lone
    /// surrogates), which UTF-8 cannot carry.
    pub fn str_utf16(&mut self, units: &[u16]) {
        self.out.push(tag::STR_UTF16);
        self.count(units.len());
        self.out.reserve(units.len() * 2);
        for u in units {
            self.out.extend_from_slice(&u.to_le_bytes());
        }
    }

    /// A string of Latin-1 code units (one byte each, U+0000..U+00FF).
    pub fn str_latin1(&mut self, units: &[u8]) {
        let wide = units.iter().filter(|&&b| b >= 0x80).count();
        self.out.push(tag::STR_UTF8);
        self.count(units.len());
        self.count(units.len() + wide);
        if wide == 0 {
            self.out.extend_from_slice(units);
            return;
        }
        self.out.reserve(units.len() + wide);
        for &b in units {
            if b < 0x80 {
                self.out.push(b);
            } else {
                self.out.push(0xC0 | (b >> 6));
                self.out.push(0x80 | (b & 0x3F));
            }
        }
    }

    /// Bytes: a `Uint8Array` in the VM.
    pub fn bytes(&mut self, b: &[u8]) {
        self.out.push(tag::BYTES);
        self.count(b.len());
        self.out.extend_from_slice(b);
    }

    /// An array of `count` elements: `count` values (or holes) follow.
    pub fn array(&mut self, count: usize) {
        self.out.push(tag::ARRAY);
        self.count(count);
    }

    /// An object of `count` properties: `count` times a [`Writer::key`] and a value
    /// follow.
    pub fn object(&mut self, count: usize) {
        self.out.push(tag::OBJECT);
        self.count(count);
    }

    /// A property key: an index into the key table, or the new key's string.
    pub fn key(&mut self, k: &str) {
        self.key_bytes(k.as_bytes(), |w| w.str(k));
    }

    /// [`Writer::key`] of UTF-16 code units (keys with lone surrogates).
    pub fn key_utf16(&mut self, units: &[u16]) {
        let mut id = Vec::with_capacity(units.len() * 2 + 1);
        // Never equal to a UTF-8 key's bytes: those never start with 0xFF.
        id.push(0xFF);
        for u in units {
            id.extend_from_slice(&u.to_le_bytes());
        }
        self.key_bytes(&id, |w| w.str_utf16(units));
    }

    fn key_bytes(&mut self, id: &[u8], write: impl FnOnce(&mut Writer)) {
        if let Some(&k) = self.keys.get(id) {
            self.varint(k + 1);
            return;
        }
        let k = u32::try_from(self.keys.len()).unwrap_or(u32::MAX);
        self.keys.insert(id.to_vec(), k);
        self.varint(0);
        write(self);
    }
}
