//! The `JSON.stringify` fast path: SerializeJSONProperty for plain data, without a
//! replacer.
//!
//! It walks the value with an explicit stack in exactly the order of the spec algorithm.
//! When it meets anything that could run user code or needs the full algorithm (an
//! enumerable accessor, a `toJSON` that is an object anywhere on a prototype chain, an
//! object class other than ordinary or array, BigInt, host refs), it returns
//! [`StringifyError::Slow`]. Up to that point it has only read the heap (and flattened
//! ropes, which is invisible), so the caller can rerun the full algorithm from the start.
//! Because everything before the bail-out point was plain data, a cycle or the depth limit
//! found here would be found at the same point by the full algorithm, so those errors are
//! final.
//!
//! The output goes into a growable one-byte buffer that switches to two-byte the first
//! time a code unit above 0xFF is written. Past [`SPILL_AT`] units the buffer is moved
//! into the result's heap string at the next check point and starts again (D2553): a
//! large result is built in the arena, where it ends up, no Rust copy of it is alive while
//! the arena grows to take it, and the string length limit and the memory cap apply as it
//! grows. (Closing brackets and their indentation are not check points: a run of them
//! stays in the buffer until the next value or the end, as before.)

use alloc::vec::Vec;

use super::DEFAULT_MAX_DEPTH;
use crate::bytecode::intrinsics::const_id;
use crate::heap::{
    Atom, Heap, HeapError, Lookup, ObjectClass, PropFlags, Property, StrView, atoms,
};
use crate::num;
use crate::value::{Kind, Obj, Str, Value};

/// Why [`stringify_fast`] did not produce a result.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StringifyError {
    /// The value needs the full algorithm; nothing observable has happened.
    Slow,
    /// `TypeError: circular reference`.
    Circular,
    /// `RangeError: Maximum call stack size exceeded` (nesting deeper than
    /// [`DEFAULT_MAX_DEPTH`]; D013).
    StackOverflow,
    /// Out of memory, or the result would be longer than the string limit
    /// (`RangeError: invalid string length`).
    Heap(HeapError),
    /// `TypeError: Do not know how to serialize a BigInt` (no `toJSON` on its
    /// prototype chain).
    BigInt,
}

impl StringifyError {
    /// The JS error constructor name and message, as QuickJS reports them. `None` for
    /// [`StringifyError::Slow`].
    pub fn js_error(self) -> Option<(&'static str, &'static str)> {
        match self {
            StringifyError::Slow => None,
            StringifyError::Circular => Some(("TypeError", "circular reference")),
            StringifyError::StackOverflow => {
                Some(("RangeError", "Maximum call stack size exceeded"))
            }
            StringifyError::Heap(HeapError::OutOfMemory) => {
                Some(("InternalError", "out of memory"))
            }
            StringifyError::Heap(_) => Some(("RangeError", "invalid string length")),
            StringifyError::BigInt => Some(("TypeError", "Do not know how to serialize a BigInt")),
        }
    }
}

impl From<HeapError> for StringifyError {
    fn from(e: HeapError) -> Self {
        StringifyError::Heap(e)
    }
}

/// The gap for a Number `space` argument: `min(10, ToIntegerOrInfinity(space))` spaces.
pub fn gap_from_number(space: f64) -> Vec<u16> {
    // `as` truncates toward zero and saturates; NaN becomes 0.
    let n = (space.clamp(0.0, 10.0) as usize).min(10);
    alloc::vec![b' ' as u16; n]
}

/// The gap for a String `space` argument: its first 10 code units.
pub fn gap_from_units(space: &[u16]) -> &[u16] {
    &space[..space.len().min(10)]
}

/// `JSON.stringify(value, undefined, space)` for plain data, with `gap` already computed
/// from `space` (see [`gap_from_number`] / [`gap_from_units`]). `Ok(None)` is the
/// `undefined` result (for `undefined`, symbols).
pub fn stringify_fast(
    heap: &mut Heap,
    value: Value,
    gap: &[u16],
) -> Result<Option<Str>, StringifyError> {
    stringify_fast_at(heap, value, gap, &[], true, None)
}

/// [`stringify_fast`] for a value inside the full algorithm (0090, D1901): `outer` are
/// the objects being serialized around it (the algorithm's stack; reaching one is a
/// cycle), which also set its indentation, and with `to_json` false the value's own
/// `toJSON` is not looked at (the algorithm called it already).
pub fn stringify_fast_at(
    heap: &mut Heap,
    value: Value,
    gap: &[u16],
    outer: &[Obj],
    to_json: bool,
    list: Option<&[Atom]>,
) -> Result<Option<Str>, StringifyError> {
    let mut s = Ser {
        heap,
        out: Out {
            // (Most results are a few hundred bytes: no growth through 8, 16, ... 128.)
            buf: Buf::Narrow(crate::buf::vec_with_capacity(256)),
            pieces: Vec::new(),
        },
        gap,
        outer,
        skip_to_json: !to_json,
        list,
        stack: Vec::new(),
        keys: Vec::new(),
        deep: Vec::new(),
        clean_protos: Vec::new(),
        // Filled on the first key: most calls on small values need none.
        key_cache: Vec::new(),
        key_bytes: Vec::new(),
    };
    if !s.run(value)? {
        return Ok(None);
    }
    if s.out.pieces.is_empty() {
        return Ok(Some(s.out.buf.alloc(s.heap)?));
    }
    s.spill()?;
    // Usually one piece; a rope of them when something else was allocated in between.
    let mut str = s.out.pieces[0];
    for &p in &s.out.pieces[1..] {
        str = s.heap.concat(str, p)?;
    }
    Ok(Some(str))
}

/// The buffered output, in code units, at which a check point moves it into the heap
/// (D2553).
/// The buffer then stays below the size that goes to page runs of its own instead of the
/// allocator's segments.
pub(super) const SPILL_AT: usize = 4096;

/// QuoteJSONString ( value ), 25.5.2.3, of a string, without a serializer's setup
/// (the full algorithm quotes every key and string through this).
pub fn quote(heap: &mut Heap, s: Str) -> Result<Str, HeapError> {
    let flat = heap.contiguous(s)?;
    let view = heap.str_view(flat).expect("flat");
    let mut out = Out {
        buf: Buf::Narrow(crate::buf::vec_with_capacity(view.len() + 2)),
        pieces: Vec::new(),
    };
    out.quote(view);
    out.buf.alloc(heap)
}

enum Buf {
    Narrow(Vec<u8>),
    Wide(Vec<u16>),
}

impl Buf {
    /// A heap string of the buffer's units.
    fn alloc(&self, heap: &mut Heap) -> Result<Str, HeapError> {
        match self {
            Buf::Narrow(b) => heap.alloc_latin1(b),
            Buf::Wide(w) => heap.alloc_utf16(w),
        }
    }
}

struct Out {
    buf: Buf,
    /// The output moved into the heap so far: flat strings, in order, each of the
    /// buffer's width when it was started (see [`Ser::spill`]).
    pieces: Vec<Str>,
}

impl Out {
    /// Appends Latin-1 (or ASCII) bytes.
    #[inline]
    fn latin1(&mut self, s: &[u8]) {
        match &mut self.buf {
            // Most pieces are short (numbers, punctuation, keys): an inline copy beats a
            // memcpy call; long ones (strings) still get the memcpy.
            Buf::Narrow(b) if s.len() <= 32 => b.extend(s.iter().copied()),
            Buf::Narrow(b) => b.extend_from_slice(s),
            Buf::Wide(w) => w.extend(s.iter().map(|&c| c as u16)),
        }
    }

    #[inline]
    fn byte(&mut self, c: u8) {
        match &mut self.buf {
            Buf::Narrow(b) => b.push(c),
            Buf::Wide(w) => w.push(c as u16),
        }
    }

    #[inline]
    fn unit(&mut self, u: u16) {
        if u <= 0xFF {
            return self.byte(u as u8);
        }
        self.widen();
        if let Buf::Wide(w) = &mut self.buf {
            w.push(u);
        }
    }

    /// Switches to the wide buffer (the result will have a unit above 0xFF).
    fn widen(&mut self) {
        if let Buf::Narrow(b) = &mut self.buf {
            let mut w = crate::buf::vec_with_capacity(b.len() * 2 + 16);
            w.extend(b.iter().map(|&c| c as u16));
            self.buf = Buf::Wide(w);
        }
    }

    /// Appends little-endian units; the buffer must be wide.
    fn wide_run(&mut self, units: &[[u8; 2]]) {
        if let Buf::Wide(w) = &mut self.buf {
            w.extend(units.iter().map(|p| u16::from_le_bytes(*p)));
        }
    }

    fn units(&mut self, us: &[u16]) {
        for &u in us {
            self.unit(u);
        }
    }

    /// `\uXXXX` with lowercase hex, as QuickJS writes it.
    fn escape_u(&mut self, u: u16) {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let n = u as usize;
        self.latin1(&[
            b'\\',
            b'u',
            HEX[n >> 12],
            HEX[(n >> 8) & 15],
            HEX[(n >> 4) & 15],
            HEX[n & 15],
        ]);
    }

    /// The escape for an ASCII unit that QuoteJSONString escapes.
    fn escape_ascii(&mut self, c: u8) {
        match c {
            b'"' => self.latin1(b"\\\""),
            b'\\' => self.latin1(b"\\\\"),
            0x08 => self.latin1(b"\\b"),
            0x09 => self.latin1(b"\\t"),
            0x0A => self.latin1(b"\\n"),
            0x0C => self.latin1(b"\\f"),
            0x0D => self.latin1(b"\\r"),
            _ => self.escape_u(c as u16),
        }
    }

    /// QuoteJSONString.
    fn quote(&mut self, v: StrView<'_>) {
        // Room for the quotes and a few escapes at once (0116): the runs and escapes
        // below then append without growing.
        if let Buf::Narrow(o) = &mut self.buf {
            o.reserve(v.len() + v.len() / 16 + 2);
        }
        self.byte(b'"');
        match v {
            StrView::Latin1(b) => {
                let mut run = 0;
                let mut i = 0;
                while i < b.len() {
                    // 8 bytes at a time: skip a word with no byte to escape, else go
                    // straight to its first such byte (0129: not byte by byte up to it).
                    if let Some(w) = b.get(i..i + 8) {
                        let m = super::escape_mask8(u64::from_le_bytes(w.try_into().unwrap()));
                        if m == 0 {
                            i += 8;
                            continue;
                        }
                        i += (m.trailing_zeros() / 8) as usize;
                    }
                    let c = b[i];
                    if c < 0x20 || c == b'"' || c == b'\\' {
                        self.latin1(&b[run..i]);
                        self.escape_ascii(c);
                        run = i + 1;
                    }
                    i += 1;
                }
                self.latin1(&b[run..]);
            }
            StrView::Utf16(bytes) => {
                // A two-byte string has a unit above 0xFF (the width invariant), so the
                // output becomes wide anyway: switch once, then copy unescaped runs in
                // bulk instead of unit by unit.
                self.widen();
                let units = bytes.as_chunks::<2>().0;
                let mut run = 0;
                let mut i = 0;
                while i < units.len() {
                    let u = u16::from_le_bytes(units[i]);
                    if !matches!(u, 0..=0x1F | 0x22 | 0x5C | 0xD800..=0xDFFF) {
                        i += 1;
                        continue;
                    }
                    self.wide_run(&units[run..i]);
                    i += 1;
                    match u {
                        0xD800..=0xDBFF
                            if i < units.len()
                                && (0xDC00..=0xDFFF).contains(&u16::from_le_bytes(units[i])) =>
                        {
                            // A valid pair is copied as is; the run continues after it.
                            run = i - 1;
                            i += 1;
                            continue;
                        }
                        0xD800..=0xDFFF => self.escape_u(u),
                        _ => self.escape_ascii(u as u8),
                    }
                    run = i;
                }
                self.wide_run(&units[run..]);
            }
        }
        self.byte(b'"');
    }
}

/// Whether any of the 8 bytes in `w` is below 0x20, `"` or `\\` (SWAR: a byte is zero
/// after the subtraction or xor exactly when it matched).
#[cfg(test)]
fn needs_escape8(w: u64) -> bool {
    super::escape_mask8(w) != 0
}

/// Writes `n` in decimal.
fn write_int(out: &mut Out, n: i64) {
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    let neg = n < 0;
    let mut m = n.unsigned_abs();
    loop {
        i -= 1;
        buf[i] = b'0' + (m % 10) as u8;
        m /= 10;
        if m == 0 {
            break;
        }
    }
    if neg {
        out.byte(b'-');
    }
    out.latin1(&buf[i..]);
}

#[derive(Clone, Copy)]
enum Frame {
    /// A plain object (see `Heap::plain_entry_slots`): serializing its entry slots
    /// `next..count` directly.
    Plain {
        o: Obj,
        next: u32,
        count: u32,
        any: bool,
    },
    /// Serializing the keys `next..end` of `keys`; the frame owns `start..end`.
    Obj {
        o: Obj,
        start: usize,
        next: usize,
        end: usize,
        any: bool,
    },
    Arr {
        o: Obj,
        next: u32,
        len: u32,
    },
}

impl Frame {
    fn obj(&self) -> Obj {
        match *self {
            Frame::Plain { o, .. } | Frame::Obj { o, .. } | Frame::Arr { o, .. } => o,
        }
    }
}

/// How a value serializes.
enum Class {
    /// `undefined`: skipped in objects, `null` in arrays.
    Skip,
    Primitive,
    Object(Obj, bool),
    /// A Date whose `toJSON` is the original (D1901).
    Date(Obj),
}

/// Date.prototype's methods that `toJSON` of a Date runs: with the originals in place,
/// it is `null` for an invalid date and the ISO string otherwise.
const DATE_JSON: [u16; 4] = [
    const_id("DatePrototypeToJSON"),
    const_id("DatePrototypeToISOString"),
    const_id("DatePrototypeValueOf"),
    const_id("DatePrototypeToPrimitive"),
];

/// Frames beyond this many are also kept in a set, so the cycle check stays cheap for deep
/// structures.
const LINEAR_CYCLE_CHECK: usize = 32;

struct Ser<'h, 'g> {
    heap: &'h mut Heap,
    out: Out,
    gap: &'g [u16],
    /// The enclosing objects of a value inside the full algorithm (`stringify_fast_at`).
    outer: &'g [Obj],
    /// The next object classified is that value: its `toJSON` was handled already.
    skip_to_json: bool,
    /// The keys of an array replacer (the PropertyList): objects serialize these.
    list: Option<&'g [Atom]>,
    stack: Vec<Frame>,
    keys: Vec<Atom>,
    /// Sorted offsets of the frames beyond the first [`LINEAR_CYCLE_CHECK`].
    deep: Vec<u32>,
    /// Prototypes whose chain is known to have no (object-valued) `toJSON`. Nothing can
    /// change during one call, so the answer stays valid.
    clean_protos: Vec<Obj>,
    /// Quoted Latin-1 keys by atom, direct-mapped: arrays of similar objects repeat keys.
    key_cache: Vec<(Atom, u32, u32)>,
    /// The quoted keys of `key_cache` (its entries are ranges here): one buffer, not a
    /// Vec per key (0116).
    key_bytes: Vec<u8>,
}

const KEY_CACHE: usize = 128;

#[cfg(test)]
extern crate std;

#[cfg(test)]
std::thread_local! {
    /// One-byte keys quoted (cache misses), for the key cache's test.
    static KEYS_QUOTED: core::cell::Cell<usize> = const { core::cell::Cell::new(0) };
}
const CLEAN_PROTOS: usize = 8;

/// Whether a `toJSON` lookup result means user code might run.
fn to_json_blocks(p: Option<Property>) -> bool {
    match p {
        None => false,
        Some(Property::Data { value, .. }) => value.as_object().is_some(),
        Some(Property::Accessor { .. }) => true,
    }
}

impl Ser<'_, '_> {
    /// Fails with `Slow` if `toJSON` on `o` (own or inherited) might be callable.
    fn check_to_json(&mut self, o: Obj) -> Result<(), StringifyError> {
        let own = self.heap.get_own_property(o, atoms::TO_JSON);
        if own.is_some() {
            return if to_json_blocks(own) {
                Err(StringifyError::Slow)
            } else {
                Ok(())
            };
        }
        let Some(p) = self.heap.get_prototype_of(o) else {
            return Ok(());
        };
        if self.clean_protos.contains(&p) {
            return Ok(());
        }
        // A proxy on the chain answers with its `get` trap (user code).
        let found = match self.heap.lookup_property(p, atoms::TO_JSON) {
            Lookup::Found(_, p) => Some(p),
            Lookup::Absent => None,
            Lookup::Proxy(_) => return Err(StringifyError::Slow),
        };
        if to_json_blocks(found) {
            return Err(StringifyError::Slow);
        }
        if self.clean_protos.len() < CLEAN_PROTOS {
            self.clean_protos.push(p);
        }
        Ok(())
    }

    fn classify(&mut self, v: Value) -> Result<Class, StringifyError> {
        Ok(match v.kind() {
            Kind::Undefined | Kind::Symbol => Class::Skip,
            Kind::Null | Kind::Bool | Kind::Number | Kind::String => Class::Primitive,
            Kind::Object => {
                let o = v.as_object().unwrap();
                let is_array = match self.heap.object_class(o) {
                    ObjectClass::Ordinary => false,
                    ObjectClass::Array => true,
                    ObjectClass::Date if !self.skip_to_json => {
                        return self.date(o).ok_or(StringifyError::Slow);
                    }
                    // Functions (skipped), wrappers (ToPrimitive), exotics (proxies
                    // included): full algorithm.
                    #[allow(unreachable_patterns)]
                    _ => return Err(StringifyError::Slow),
                };
                // A toJSON that might be callable means calling user code.
                if !core::mem::take(&mut self.skip_to_json) {
                    self.check_to_json(o)?;
                }
                Class::Object(o, is_array)
            }
            Kind::BigInt => {
                // A BigInt.prototype.toJSON (or one further up) is user code.
                if self.heap.realm().is_undefined() {
                    return Err(StringifyError::BigInt);
                }
                let proto = crate::interp::realm::primitive_proto(self.heap, v).unwrap();
                let found = self
                    .heap
                    .find_property(proto, atoms::TO_JSON)
                    .map(|(_, p)| p);
                return Err(if found.is_some() {
                    StringifyError::Slow
                } else {
                    StringifyError::BigInt
                });
            }
            Kind::HostRef | Kind::Hole | Kind::Uninitialized | Kind::Internal => {
                return Err(StringifyError::Slow);
            }
        })
    }

    /// A Date whose `toJSON`, `toISOString`, `valueOf` and @@toPrimitive are the
    /// originals, found as data properties without a Proxy: `toJSON` runs no user code
    /// then, and gives its time value as an ISO string (or `null`).
    fn date(&mut self, o: Obj) -> Option<Class> {
        let iso = self.heap.atom_str("toISOString").ok()?;
        let keys = [
            atoms::TO_JSON,
            iso,
            atoms::VALUE_OF,
            atoms::SYM_TO_PRIMITIVE,
        ];
        for (key, id) in keys.into_iter().zip(DATE_JSON) {
            let original = crate::interp::realm::installed_intrinsic(self.heap, id);
            match self.heap.lookup_property(o, key) {
                Lookup::Found(_, Property::Data { value, .. }) if value == original => {}
                _ => return None,
            }
        }
        Some(Class::Date(o))
    }

    /// A check point: moves a buffer past [`SPILL_AT`] into the heap, where the string
    /// length limit and the memory cap apply. The buffer itself stays small.
    #[inline]
    fn check(&mut self) -> Result<(), StringifyError> {
        let units = match &self.out.buf {
            Buf::Narrow(b) => b.len(),
            Buf::Wide(w) => w.len(),
        };
        if units >= SPILL_AT {
            self.spill()?;
        }
        Ok(())
    }

    /// Moves the buffer into the heap: appended to the last piece in place when that is
    /// still the arena's last cell and has the buffer's width, else as a new piece.
    #[inline(never)]
    fn spill(&mut self) -> Result<(), StringifyError> {
        let appended = match (&self.out.buf, self.out.pieces.last()) {
            (Buf::Narrow(b), Some(&p)) => self.heap.append_last_latin1(p, b)?,
            _ => false,
        };
        if !appended {
            let piece = self.out.buf.alloc(self.heap)?;
            self.out.pieces.push(piece);
        }
        match &mut self.out.buf {
            Buf::Narrow(b) => b.clear(),
            Buf::Wide(w) => w.clear(),
        }
        Ok(())
    }

    fn newline(&mut self, depth: usize) {
        if self.gap.is_empty() {
            return;
        }
        self.out.byte(b'\n');
        for _ in 0..depth + self.outer.len() {
            self.out.units(self.gap);
        }
    }

    /// Writes a primitive.
    fn primitive(&mut self, v: Value) -> Result<(), StringifyError> {
        if let Some(i) = v.as_int() {
            write_int(&mut self.out, i as i64);
        } else if let Some(n) = v.as_number() {
            if n.is_finite() {
                self.out.latin1(num::to_string(n).as_bytes());
            } else {
                self.out.latin1(b"null");
            }
        } else if let Some(b) = v.as_bool() {
            self.out.latin1(if b { b"true" } else { b"false" });
        } else if v.is_null() {
            self.out.latin1(b"null");
        } else if let Some(s) = v.as_string() {
            let flat = self.heap.contiguous(s)?;
            let view = self.heap.str_view(flat).expect("flat");
            self.out.quote(view);
        }
        self.check()
    }

    /// Writes the key of an object member.
    fn key(&mut self, k: Atom) {
        if let Some(i) = k.as_int() {
            self.out.byte(b'"');
            write_int(&mut self.out, i as i64);
            self.out.byte(b'"');
        } else {
            if self.key_cache.is_empty() {
                self.key_cache = alloc::vec![(Atom::NULL, 0, 0); KEY_CACHE];
            }
            let slot = k.id() as usize % KEY_CACHE;
            let (a, at, len) = self.key_cache[slot];
            if a == k {
                self.out
                    .latin1(&self.key_bytes[at as usize..(at + len) as usize]);
                return;
            }
            let s = self
                .heap
                .atom_string(k)
                .expect("string atoms do not allocate");
            let view = self.heap.str_view(s).expect("atom strings are flat");
            if !matches!(view, StrView::Latin1(_)) {
                self.out.quote(view);
                return;
            }
            // A one-byte key is quoted into the cache and copied from there, whatever the
            // output's width: once a two-byte string widened the output, quoting every
            // key again cost `JSON.stringify` of records 7% (0224).
            #[cfg(test)]
            KEYS_QUOTED.with(|n| n.set(n.get() + 1));
            let at = self.key_bytes.len();
            let mut q = Out {
                buf: Buf::Narrow(core::mem::take(&mut self.key_bytes)),
                pieces: Vec::new(),
            };
            q.quote(view);
            if let Buf::Narrow(b) = q.buf {
                self.key_bytes = b;
            }
            let len = self.key_bytes.len() - at;
            self.key_cache[slot] = (k, at as u32, len as u32);
            self.out.latin1(&self.key_bytes[at..]);
        }
    }

    /// Writes a value of class `c`: a primitive, or the opening of a container.
    fn emit(&mut self, v: Value, c: Class) -> Result<(), StringifyError> {
        let (o, is_array) = match c {
            Class::Skip => unreachable!(),
            Class::Primitive => return self.primitive(v),
            Class::Date(d) => {
                let t = self.heap.object_slot(d, 0).as_number().unwrap_or(f64::NAN);
                match crate::builtins::date::format(t, 2, 3, 0) {
                    Some(iso) => {
                        self.out.byte(b'"');
                        self.out.latin1(iso.as_bytes());
                        self.out.byte(b'"');
                    }
                    None => self.out.latin1(b"null"),
                }
                return self.check();
            }
            Class::Object(o, a) => (o, a),
        };
        let n = self.stack.len();
        if self.stack[..n.min(LINEAR_CYCLE_CHECK)]
            .iter()
            .any(|f| f.obj() == o)
            || self.deep.binary_search(&o.offset()).is_ok()
            || self.outer.contains(&o)
        {
            return Err(StringifyError::Circular);
        }
        if n + self.outer.len() >= DEFAULT_MAX_DEPTH as usize {
            return Err(StringifyError::StackOverflow);
        }
        if n >= LINEAR_CYCLE_CHECK
            && let Err(i) = self.deep.binary_search(&o.offset())
        {
            self.deep.insert(i, o.offset());
        }
        // Containers without primitives (`[[],[]]` with a long gap) must not grow the
        // output unchecked either.
        self.check()?;
        if is_array {
            self.out.byte(b'[');
            let len = self.heap.array_length(o);
            self.stack.push(Frame::Arr { o, next: 0, len });
        } else if self.list.is_none()
            && let Some(count) = self.heap.plain_entry_slots(o)
        {
            self.out.byte(b'{');
            self.stack.push(Frame::Plain {
                o,
                next: 0,
                count,
                any: false,
            });
        } else {
            self.out.byte(b'{');
            let start = self.keys.len();
            if let Some(list) = self.list {
                self.keys.extend_from_slice(list);
            } else {
                for k in self.heap.own_keys(o) {
                    if !k.is_symbol() {
                        self.keys.push(k);
                    }
                }
            }
            let end = self.keys.len();
            self.stack.push(Frame::Obj {
                o,
                start,
                next: start,
                end,
                any: false,
            });
        }
        Ok(())
    }

    fn pop(&mut self) {
        let f = self.stack.pop().unwrap();
        if self.stack.len() >= LINEAR_CYCLE_CHECK
            && let Ok(i) = self.deep.binary_search(&f.obj().offset())
        {
            self.deep.remove(i);
        }
        if let Frame::Obj { start, end, .. } = f {
            // Keys of nested frames were pushed after ours and are gone already.
            debug_assert_eq!(self.keys.len(), end);
            self.keys.truncate(start);
        }
    }

    /// Array element `i` ([[Get]]), or `Slow` if that would call a getter.
    fn element(&mut self, o: Obj, i: u32) -> Result<Value, StringifyError> {
        if let Some(v) = self.heap.get_element_fast(o, i) {
            return Ok(v);
        }
        let key = self.heap.atom_from_index(i)?;
        match self.heap.lookup_property(o, key) {
            Lookup::Absent => Ok(Value::UNDEFINED),
            Lookup::Found(_, Property::Data { value, .. }) => Ok(value),
            Lookup::Found(_, Property::Accessor { .. }) | Lookup::Proxy(_) => {
                Err(StringifyError::Slow)
            }
        }
    }

    /// The value of an own property as an object member: `None` if it is skipped
    /// (non-enumerable, or serializes to `undefined`), `Slow` for an enumerable accessor.
    fn member_value(&mut self, p: Property) -> Result<Option<(Value, Class)>, StringifyError> {
        let value = match p {
            Property::Data { value, flags } if flags.enumerable() => value,
            Property::Accessor { flags, .. } if flags.enumerable() => {
                return Err(StringifyError::Slow);
            }
            _ => return Ok(None),
        };
        Ok(match self.classify(value)? {
            Class::Skip => None,
            c => Some((value, c)),
        })
    }

    /// Writes `,`, the indentation, `"key":` and the value.
    fn member(
        &mut self,
        key: Atom,
        any: bool,
        depth: usize,
        value: Value,
        c: Class,
    ) -> Result<(), StringifyError> {
        if any {
            self.out.byte(b',');
        }
        self.newline(depth);
        self.key(key);
        self.out.byte(b':');
        if !self.gap.is_empty() {
            self.out.byte(b' ');
        }
        self.emit(value, c)
    }

    /// Serializes `v`; `false` if the result is `undefined`.
    fn run(&mut self, v: Value) -> Result<bool, StringifyError> {
        match self.classify(v)? {
            Class::Skip => return Ok(false),
            c => self.emit(v, c)?,
        }
        while let Some(&frame) = self.stack.last() {
            let top = self.stack.len() - 1;
            let depth = self.stack.len();
            match frame {
                Frame::Plain {
                    o,
                    next,
                    count,
                    any,
                } => {
                    if next == count {
                        if any {
                            self.newline(depth - 1);
                        }
                        self.out.byte(b'}');
                        self.pop();
                        continue;
                    }
                    self.stack[top] = Frame::Plain {
                        o,
                        next: next + 1,
                        count,
                        any,
                    };
                    let Some((key, p)) = self.heap.plain_entry(o, next) else {
                        continue;
                    };
                    if key.is_symbol() {
                        continue;
                    }
                    let Some((value, c)) = self.member_value(p)? else {
                        continue;
                    };
                    self.stack[top] = Frame::Plain {
                        o,
                        next: next + 1,
                        count,
                        any: true,
                    };
                    self.member(key, any, depth, value, c)?;
                }
                Frame::Obj {
                    o,
                    start,
                    next,
                    end,
                    any,
                } => {
                    if next == end {
                        if any {
                            self.newline(depth - 1);
                        }
                        self.out.byte(b'}');
                        self.pop();
                        continue;
                    }
                    let key = self.keys[next];
                    self.stack[top] = Frame::Obj {
                        o,
                        start,
                        next: next + 1,
                        end,
                        any,
                    };
                    let p = if self.list.is_some() {
                        // A property list reads each key with [[Get]]: inherited and
                        // non-enumerable properties too.
                        match self.heap.lookup_property(o, key) {
                            Lookup::Found(_, Property::Data { value, .. }) => {
                                Property::data(value, PropFlags::ALL)
                            }
                            Lookup::Absent => continue,
                            Lookup::Found(..) | Lookup::Proxy(_) => {
                                return Err(StringifyError::Slow);
                            }
                        }
                    } else {
                        match self.heap.get_own_property(o, key) {
                            Some(p) => p,
                            None => continue,
                        }
                    };
                    let Some((value, c)) = self.member_value(p)? else {
                        continue;
                    };
                    self.stack[top] = Frame::Obj {
                        o,
                        start,
                        next: next + 1,
                        end,
                        any: true,
                    };
                    self.member(key, any, depth, value, c)?;
                }
                Frame::Arr { o, next, len } => {
                    if next == len {
                        if len > 0 {
                            self.newline(depth - 1);
                        }
                        self.out.byte(b']');
                        self.pop();
                        continue;
                    }
                    self.stack[top] = Frame::Arr {
                        o,
                        next: next + 1,
                        len,
                    };
                    if next > 0 {
                        self.out.byte(b',');
                    }
                    self.newline(depth);
                    let value = self.element(o, next)?;
                    match self.classify(value)? {
                        Class::Skip => {
                            self.out.latin1(b"null");
                            self.check()?;
                        }
                        c => self.emit(value, c)?,
                    }
                }
            }
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::needs_escape8;
    use crate::json::escape_mask8;

    #[test]
    fn keys_come_from_the_cache_in_two_byte_output() {
        // Once a two-byte string widens the output, keys are still quoted once each
        // (0224: quoting them again per object cost 7% on records).
        use crate::heap::Heap;
        use crate::value::Value;
        let mut h = Heap::new().unwrap();
        let arr = h.new_array(None, 0).unwrap();
        let wide = h.alloc_str("日本").unwrap();
        h.set_element_fast(arr, 0, Value::string(wide)).unwrap();
        let (a, b) = (h.atom_str("alpha").unwrap(), h.atom_str("beta").unwrap());
        for i in 1..=100 {
            let o = h.new_object(None).unwrap();
            h.create_data_property(o, a, Value::int(i)).unwrap();
            h.create_data_property(o, b, Value::int(-i)).unwrap();
            h.set_element_fast(arr, i as u32, Value::object(o)).unwrap();
        }
        let before = super::KEYS_QUOTED.with(|n| n.get());
        let s = super::stringify_fast(&mut h, Value::object(arr), &[])
            .unwrap()
            .unwrap();
        assert_eq!(super::KEYS_QUOTED.with(|n| n.get()) - before, 2);
        let text = h.str_to_string(s);
        assert!(
            text.starts_with("[\"日本\",{\"alpha\":1,\"beta\":-1},"),
            "{text}"
        );
        assert!(text.ends_with("{\"alpha\":100,\"beta\":-100}]"), "{text}");
    }

    /// The lowest flag of the mask is the first byte that needs escaping, whatever the
    /// other bytes are (random words over bytes near the edges: 0x1F/0x20, `"`, `\\`).
    #[test]
    fn escape_mask_lowest_flag_is_exact() {
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let pool = [
            0x00, 0x1F, 0x20, 0x21, b'"', b'#', b'\\', b']', b'a', 0x80, 0xFF, 0x5B,
        ];
        for _ in 0..100_000 {
            let mut w = [0u8; 8];
            for b in &mut w {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                *b = pool[(seed % pool.len() as u64) as usize];
            }
            let first = w.iter().position(|&b| b < 0x20 || b == b'"' || b == b'\\');
            let m = escape_mask8(u64::from_le_bytes(w));
            assert_eq!(
                (m != 0).then(|| (m.trailing_zeros() / 8) as usize),
                first,
                "{w:?}"
            );
        }
    }

    #[test]
    fn swar_escape_scan_is_exact() {
        for b in 0..=255u8 {
            for pos in 0..8 {
                let mut w = [b'a'; 8];
                w[pos] = b;
                let want = b < 0x20 || b == b'"' || b == b'\\';
                assert_eq!(
                    needs_escape8(u64::from_le_bytes(w)),
                    want,
                    "{b:#x} at {pos}"
                );
                // With high Latin-1 bytes around it.
                let mut w = [0xE9; 8];
                w[pos] = b;
                assert_eq!(
                    needs_escape8(u64::from_le_bytes(w)),
                    want,
                    "{b:#x} at {pos}"
                );
            }
        }
    }
}
