//! The outbound encoder (VM → host): a heap value as a wire buffer (docs/wire.md
//! "Outbound encoding"), for Rust hosts that take plain data out of a VM. It reads the heap
//! and never runs JS, so it refuses what only JS could turn into data: accessors, proxies,
//! class instances and every object class that is not plain data, with an error that names
//! the path (`$.items[3].when`), as js/src/wire.ts does on the host side.
//!
//! An explicit stack (no recursion), like the decoder: each open container's members wait
//! in one shared vector. It writes to a [`Sink`]: the wire [`Writer`], or a host's own
//! builder (djs-embed builds its data tree directly, without a buffer in between).

use alloc::string::String;
use alloc::vec::Vec;

use super::Writer;
use crate::heap::{Atom, Heap, ObjectClass, Property, StrView};
use crate::json::DEFAULT_MAX_DEPTH;
use crate::value::{Kind, Obj, Str, Value};

/// Why [`encode`] refused a value.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum EncodeError {
    /// Not plain data: `what` (`"a function"`, `"an accessor"`, ...) at `path` (`$.a[3]`).
    Unsupported { what: &'static str, path: String },
    /// An object that contains itself, at `path`.
    Cycle { path: String },
    /// Nesting deeper than the decoder's limit, at `path`.
    TooDeep { path: String },
    /// The buffer would exceed the caller's limit (shared values are copied each time
    /// they occur, so a small heap can mean a large buffer).
    TooLarge { limit: usize },
}

impl EncodeError {
    /// The Display form, built without core::fmt (D2339).
    pub fn text(&self) -> String {
        match self {
            EncodeError::Unsupported { what, path } => {
                crate::text::cat!("cannot encode ", *what, " at ", path.as_str())
            }
            EncodeError::Cycle { path } => crate::text::cat!("cyclic value at ", path.as_str()),
            EncodeError::TooDeep { path } => {
                crate::text::cat!("nesting too deep at ", path.as_str())
            }
            EncodeError::TooLarge { limit } => {
                crate::text::cat!("the encoded value exceeds ", *limit, " bytes")
            }
        }
    }
}

impl core::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.text())
    }
}

/// Where a member sits in its container, for error paths.
#[derive(Clone, Copy)]
enum Seg {
    Key(Atom),
    Index(u32),
}

/// An open object or array: its members are `pending[start..end]`.
struct Frame {
    o: Obj,
    start: usize,
    next: usize,
    end: usize,
}

/// Where [`encode_to`] writes: a value is a primitive call, or `array(n)` / `object(n)`
/// followed by its `n` members (an object's each after its `key`), as [`Writer`] has them.
pub trait Sink {
    /// The size so far in `bytes` of the value's wire form (or an estimate), for the cap
    fn written(&self) -> usize;
    fn undefined(&mut self);
    fn null(&mut self);
    fn bool(&mut self, b: bool);
    fn number(&mut self, n: f64);
    /// A hole: only as an array element
    fn hole(&mut self);
    /// A string of Latin-1 code units
    fn str_latin1(&mut self, units: &[u8]);
    /// A well-formed string
    fn str(&mut self, s: &str);
    /// A string with lone surrogates
    fn str_utf16(&mut self, units: &[u16]);
    /// A `Uint8Array`'s bytes
    fn bytes(&mut self, b: &[u8]);
    fn array(&mut self, count: usize);
    fn object(&mut self, count: usize);
    fn key(&mut self, k: &str);
    /// A key with lone surrogates
    fn key_utf16(&mut self, units: &[u16]);
}

impl Sink for Writer {
    fn written(&self) -> usize {
        Writer::written(self)
    }
    fn undefined(&mut self) {
        Writer::undefined(self)
    }
    fn null(&mut self) {
        Writer::null(self)
    }
    fn bool(&mut self, b: bool) {
        Writer::bool(self, b)
    }
    fn number(&mut self, n: f64) {
        Writer::number(self, n)
    }
    fn hole(&mut self) {
        Writer::hole(self)
    }
    fn str_latin1(&mut self, units: &[u8]) {
        Writer::str_latin1(self, units)
    }
    fn str(&mut self, s: &str) {
        Writer::str(self, s)
    }
    fn str_utf16(&mut self, units: &[u16]) {
        Writer::str_utf16(self, units)
    }
    fn bytes(&mut self, b: &[u8]) {
        Writer::bytes(self, b)
    }
    fn array(&mut self, count: usize) {
        Writer::array(self, count)
    }
    fn object(&mut self, count: usize) {
        Writer::object(self, count)
    }
    fn key(&mut self, k: &str) {
        Writer::key(self, k)
    }
    fn key_utf16(&mut self, units: &[u16]) {
        Writer::key_utf16(self, units)
    }
}

struct Enc<'h, 's, S: Sink> {
    heap: &'h Heap,
    w: &'s mut S,
    max_bytes: usize,
    object_proto: Option<Obj>,
    stack: Vec<Frame>,
    pending: Vec<(Seg, Value)>,
}

/// Encodes `v` as a wire buffer of at most `max_bytes` (docs/wire.md "Outbound
/// encoding"): primitives, strings, arrays (holes stay holes), `Uint8Array`s, and plain
/// objects (prototype `Object.prototype` or `null`) with their own enumerable
/// string-keyed data properties in property order. Never runs JS and never allocates in
/// the heap.
pub fn encode(heap: &Heap, v: Value, max_bytes: usize) -> Result<Vec<u8>, EncodeError> {
    let mut w = Writer::new();
    encode_to(heap, v, max_bytes, &mut w)?;
    Ok(w.finish())
}

/// [`encode`] to any [`Sink`]: the same values, refusals and cap.
pub fn encode_to<S: Sink>(
    heap: &Heap,
    v: Value,
    max_bytes: usize,
    sink: &mut S,
) -> Result<(), EncodeError> {
    let object_proto = if heap.realm().is_undefined() {
        None
    } else {
        Some(crate::interp::realm::intrinsic_obj(
            heap,
            crate::interp::Intrinsic::ObjectProto,
        ))
    };
    let mut e = Enc {
        heap,
        w: sink,
        max_bytes,
        object_proto,
        stack: Vec::new(),
        pending: Vec::new(),
    };
    e.value(v)?;
    while let Some(f) = e.stack.last_mut() {
        if f.next == f.end {
            let f = e.stack.pop().unwrap();
            e.pending.truncate(f.start);
            continue;
        }
        let (seg, v) = e.pending[f.next];
        f.next += 1;
        let is_object = e.heap.object_class(f.o) != ObjectClass::Array;
        if let (true, Seg::Key(k)) = (is_object, seg) {
            e.key(k);
        }
        e.value(v)?;
    }
    Ok(())
}

impl<S: Sink> Enc<'_, '_, S> {
    /// The path of the member being written: each open container's current member.
    fn path(&self) -> String {
        let mut p = String::from("$");
        for f in &self.stack {
            let Some(&(seg, _)) = f.next.checked_sub(1).and_then(|i| self.pending.get(i)) else {
                continue;
            };
            match seg {
                Seg::Index(i) => {
                    p.push('[');
                    p.push_str(&crate::text::cat!(i));
                    p.push(']');
                }
                Seg::Key(k) => {
                    let name = self.heap.atom_name(k);
                    let ident = !name.is_empty()
                        && !name.starts_with(|c: char| c.is_ascii_digit())
                        && name
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
                    if ident {
                        p.push('.');
                        p.push_str(&name);
                    } else {
                        p.push_str("[\"");
                        for c in name.chars() {
                            if c == '"' || c == '\\' {
                                p.push('\\');
                            }
                            p.push(c);
                        }
                        p.push_str("\"]");
                    }
                }
            }
        }
        p
    }

    fn unsupported<T>(&self, what: &'static str) -> Result<T, EncodeError> {
        Err(EncodeError::Unsupported {
            what,
            path: self.path(),
        })
    }

    fn key(&mut self, k: Atom) {
        if let Some(i) = k.as_int() {
            return self.w.key(&crate::text::cat!(i));
        }
        let Some(s) = self.heap.atom_str_value(k) else {
            return self.w.key("");
        };
        match self.heap.str_view(s) {
            Some(StrView::Latin1(b)) if crate::heap::string::is_ascii(b) => {
                self.w.key(core::str::from_utf8(b).unwrap_or_default());
            }
            _ => {
                let units: Vec<u16> = self.heap.str_units(s).collect();
                match String::from_utf16(&units) {
                    Ok(text) => self.w.key(&text),
                    Err(_) => self.w.key_utf16(&units),
                }
            }
        }
    }

    fn string(&mut self, s: Str) {
        match self.heap.str_view(s) {
            Some(StrView::Latin1(b)) => self.w.str_latin1(b),
            _ => {
                let units: Vec<u16> = self.heap.str_units(s).collect();
                if units.iter().all(|&u| u <= 0xFF) {
                    let b: Vec<u8> = units.iter().map(|&u| u as u8).collect();
                    return self.w.str_latin1(&b);
                }
                match String::from_utf16(&units) {
                    Ok(text) => self.w.str(&text),
                    Err(_) => self.w.str_utf16(&units),
                }
            }
        }
    }

    /// Writes a primitive or opens a container (its members are written by `encode`'s
    /// loop).
    fn value(&mut self, v: Value) -> Result<(), EncodeError> {
        if self.w.written() > self.max_bytes {
            return Err(EncodeError::TooLarge {
                limit: self.max_bytes,
            });
        }
        match v.kind() {
            Kind::Undefined => self.w.undefined(),
            Kind::Null => self.w.null(),
            Kind::Bool => self.w.bool(v.as_bool() == Some(true)),
            Kind::Number => self.w.number(v.as_number().unwrap_or(f64::NAN)),
            Kind::String => self.string(v.as_string().expect("a string")),
            Kind::Hole => self.w.hole(),
            Kind::Symbol => return self.unsupported("a symbol"),
            Kind::BigInt => return self.unsupported("a BigInt"),
            Kind::HostRef => return self.unsupported("a host ref"),
            Kind::Uninitialized | Kind::Internal => return self.unsupported("an internal value"),
            Kind::Object => self.object(v.as_object().expect("an object"))?,
        }
        Ok(())
    }

    fn object(&mut self, o: Obj) -> Result<(), EncodeError> {
        let class = self.heap.object_class(o);
        match class {
            ObjectClass::Array | ObjectClass::Ordinary => {}
            ObjectClass::TypedArray if self.heap.ta_kind(o) == crate::heap::typed::Kind::Uint8 => {
                let n = self.heap.ta_length(o).unwrap_or(0) as usize;
                let at = self.heap.ta_byte_offset(o) as usize;
                let data = self.heap.buffer_data(self.heap.ta_buffer(o));
                self.w.bytes(data.get(at..at + n).unwrap_or(&[]));
                return Ok(());
            }
            _ => return self.unsupported(describe(class)),
        }
        if self.stack.iter().any(|f| f.o == o) {
            return Err(EncodeError::Cycle { path: self.path() });
        }
        if self.stack.len() >= DEFAULT_MAX_DEPTH as usize {
            return Err(EncodeError::TooDeep { path: self.path() });
        }
        let start = self.pending.len();
        if class == ObjectClass::Array {
            let len = self.heap.array_length(o);
            for i in 0..len {
                let v = match self.heap.get_element_fast(o, i) {
                    Some(v) => v,
                    None => {
                        let Some(k) = Atom::int(i) else {
                            return self.unsupported("an array this long");
                        };
                        match self.heap.get_own_property(o, k) {
                            None => Value::HOLE,
                            Some(Property::Data { value, .. }) => value,
                            Some(Property::Accessor { .. }) => {
                                self.pending.push((Seg::Index(i), Value::HOLE));
                                self.stack.push(Frame {
                                    o,
                                    start,
                                    next: self.pending.len(),
                                    end: self.pending.len(),
                                });
                                return self.unsupported("an accessor");
                            }
                        }
                    }
                };
                self.pending.push((Seg::Index(i), v));
                // Holes cost a byte each: a sparse array is limited by the size cap.
                if self.pending.len() - start > self.max_bytes {
                    return Err(EncodeError::TooLarge {
                        limit: self.max_bytes,
                    });
                }
            }
            self.w.array(self.pending.len() - start);
        } else {
            let proto = self.heap.get_prototype_of(o);
            if proto.is_some() && proto != self.object_proto {
                return self.unsupported("an object with a custom prototype (a class instance)");
            }
            for k in self.heap.own_keys(o) {
                if k.is_symbol() {
                    continue;
                }
                match self.heap.get_own_property(o, k) {
                    Some(Property::Data { value, flags }) if flags.enumerable() => {
                        self.pending.push((Seg::Key(k), value));
                    }
                    Some(Property::Accessor { flags, .. }) if flags.enumerable() => {
                        self.pending.push((Seg::Key(k), Value::UNDEFINED));
                        self.stack.push(Frame {
                            o,
                            start,
                            next: self.pending.len(),
                            end: self.pending.len(),
                        });
                        return self.unsupported("an accessor");
                    }
                    _ => {}
                }
            }
            self.w.object(self.pending.len() - start);
        }
        self.stack.push(Frame {
            o,
            start,
            next: start,
            end: self.pending.len(),
        });
        Ok(())
    }
}

/// How an error names an object class that is not plain data.
fn describe(class: ObjectClass) -> &'static str {
    match class {
        ObjectClass::Function
        | ObjectClass::HostFunction
        | ObjectClass::BoundFunction
        | ObjectClass::BuiltinClosure => "a function",
        ObjectClass::Proxy => "a Proxy",
        ObjectClass::Date => "a Date",
        ObjectClass::Map => "a Map",
        ObjectClass::Set => "a Set",
        ObjectClass::Error => "an Error",
        ObjectClass::Promise => "a Promise",
        ObjectClass::RegExp => "a RegExp",
        ObjectClass::TypedArray => "a typed array other than Uint8Array",
        ObjectClass::ArrayBuffer => "an ArrayBuffer",
        ObjectClass::Boolean | ObjectClass::Number | ObjectClass::String => "a primitive wrapper",
        _ => "an object that is not plain data",
    }
}
