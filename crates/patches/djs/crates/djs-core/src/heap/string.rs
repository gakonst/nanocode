//! Strings: flat one-byte ([`Str8`]), flat two-byte ([`Str16`]) and [`Rope`] cells.
//!
//! JS strings are sequences of UTF-16 code units (lone surrogates allowed). Invariants:
//!
//! - A flat string whose code units all fit in a byte is always a `Str8`. Constructors
//!   narrow, so a `Str16` always holds a unit above 0xFF. Two equal strings therefore
//!   have the same width, and equal flat strings have equal bytes.
//! - A rope has length >= [`ROPE_MIN_LEN`]; shorter concatenations are copied. Its
//!   `ROPE_TWO_BYTE` flag is the OR of its children, which is the width its flat form has.
//! - Flattening writes a new flat cell and redirects the rope to it (`LEFT` = flat,
//!   `RIGHT` = null, `ROPE_FLATTENED`). Every operation resolves that redirect first; the GC
//!   can later short-circuit it.
//!
//! - An append rope (`ROPE_APPEND`, D1610) is the first `LEN` code units of the flat cell
//!   in `LEFT`, an append buffer (`STR_APPEND_BUF`: spare capacity past its `LEN`, the
//!   used length) or a frozen former one. `concat` writes into a buffer in place only when
//!   the rope ends where the buffer's used part ends, so every older, shorter rope over the
//!   same buffer keeps its value. Buffers are only ever referenced from append ropes'
//!   `LEFT`: [`Heap::flatten`] hands out a buffer only after freezing it (clearing the
//!   flag), and a frozen buffer is never written again.
//!
//! Nothing here recurses: rope walks use an explicit stack, so a million-deep rope built by
//! `s += x` in a loop is fine.
use crate::PushOut;

use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;

use super::cells::{Header, Rope, Slice, Str8, Str16, flags};
use super::layout::Ref;
use super::{CellKind, Heap, HeapError};
use crate::value::{Obj, Str};

/// Maximum string length in code units (the same as QuickJS).
pub const MAX_LEN: u32 = (1 << 30) - 1;

/// Concatenations shorter than this are copied into a flat string instead of making a rope.
pub const ROPE_MIN_LEN: u32 = 32;

/// Smallest append buffer, in code units.
const APPEND_MIN_CAP: u32 = 64;

/// A concatenation chain gets an append buffer once its left side is this long: shorter
/// strings (the pieces of an expression like `"id " + i + "\n"`) stay ropes or copies.
const APPEND_START_LEN: u32 = 256;

// The three string kinds share LEN and HASH offsets, so code that only needs those does not
// have to dispatch on the kind.
const _: () =
    assert!(Str8::LEN.offset == Str16::LEN.offset && Str8::LEN.offset == Rope::LEN.offset);
const _: () =
    assert!(Str8::HASH.offset == Str16::HASH.offset && Str8::HASH.offset == Rope::HASH.offset);
const _: () = assert!(Str8::DATA.offset == Str16::DATA.offset);
const _: () =
    assert!(Slice::LEN.offset == Rope::LEN.offset && Slice::HASH.offset == Rope::HASH.offset);
// Flattening turns a slice into a flattened rope in place: `PARENT` becomes `LEFT` (the
// copy) and `OFF` becomes the null `RIGHT`.
const _: () =
    assert!(Slice::PARENT.offset == Rope::LEFT.offset && Slice::OFF.offset == Rope::RIGHT.offset);
const _: () = assert!(Slice::LAYOUT.min_size == Rope::LAYOUT.min_size);

/// A substring of at least this many code units is a [`Slice`] of its string instead of a
/// copy (D2542): a 24-byte cell instead of 16 bytes plus the units.
pub const SLICE_MIN_LEN: u32 = 32;

/// Only strings of at least this many code units are sliced: substrings of short strings
/// are copied, which costs little and keeps no parent alive.
pub const SLICE_PARENT_MIN_LEN: u32 = 256;

/// A borrowed view of a flat string's code units.
#[derive(Clone, Copy, Debug)]
pub enum StrView<'a> {
    /// One byte per code unit.
    Latin1(&'a [u8]),
    /// Two little-endian bytes per code unit.
    Utf16(&'a [u8]),
}

impl<'a> StrView<'a> {
    /// Length in code units.
    #[inline]
    pub fn len(&self) -> usize {
        match self {
            StrView::Latin1(b) => b.len(),
            StrView::Utf16(b) => b.len() / 2,
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline]
    pub fn is_two_byte(&self) -> bool {
        matches!(self, StrView::Utf16(_))
    }

    /// The code unit at `i`. Panics when out of range.
    #[inline]
    pub fn get(&self, i: usize) -> u16 {
        match self {
            StrView::Latin1(b) => b[i] as u16,
            StrView::Utf16(b) => u16::from_le_bytes([b[2 * i], b[2 * i + 1]]),
        }
    }

    /// The raw bytes (Latin-1 or UTF-16LE).
    #[inline]
    pub fn bytes(&self) -> &'a [u8] {
        match self {
            StrView::Latin1(b) | StrView::Utf16(b) => b,
        }
    }

    pub fn units(self) -> Units<'a> {
        Units { view: self, pos: 0 }
    }
}

/// Iterator over the code units of a [`StrView`].
#[derive(Clone)]
pub struct Units<'a> {
    view: StrView<'a>,
    pos: usize,
}

impl Iterator for Units<'_> {
    type Item = u16;

    #[inline]
    fn next(&mut self) -> Option<u16> {
        if self.pos < self.view.len() {
            let u = self.view.get(self.pos);
            self.pos += 1;
            Some(u)
        } else {
            None
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.view.len() - self.pos;
        (n, Some(n))
    }
}

/// The string hash (D2433): SipHash-1-3 over code units, four to a 64-bit word. A VM's own
/// strings, atoms and Map/Set keys hash under the key in `Roots::HASH_KEY`: without it,
/// keys can't be built to collide, as they could for the fixed multiply-rotate hash used
/// before, and a collision flood (crafted JSON keys, Map keys) can't make lookups
/// quadratic. The base image's atom index hashes under [`BASE_HASH_KEY`], the same in
/// every VM of the build. Defined on code units, so it does not depend on the string's
/// representation (width, rope shape), and never 0, because 0 marks "not computed".
#[derive(Clone, Copy)]
pub struct KeyedHasher {
    v: [u64; 4],
    /// Code units not yet compressed, 16 bits each from the low end.
    word: u64,
    n: u32,
}

impl KeyedHasher {
    #[inline]
    pub fn new(key: u64) -> KeyedHasher {
        // A 128-bit SipHash key from the 64 bits of the image's.
        let (k0, k1) = (key, key.rotate_left(32) ^ 0x6A09_E667_F3BC_C908);
        KeyedHasher {
            v: [
                k0 ^ 0x736F_6D65_7073_6575,
                k1 ^ 0x646F_7261_6E64_6F6D,
                k0 ^ 0x6C79_6765_6E65_7261,
                k1 ^ 0x7465_6462_7974_6573,
            ],
            word: 0,
            n: 0,
        }
    }

    #[inline(always)]
    fn round(v: &mut [u64; 4]) {
        v[0] = v[0].wrapping_add(v[1]);
        v[1] = v[1].rotate_left(13) ^ v[0];
        v[0] = v[0].rotate_left(32);
        v[2] = v[2].wrapping_add(v[3]);
        v[3] = v[3].rotate_left(16) ^ v[2];
        v[0] = v[0].wrapping_add(v[3]);
        v[3] = v[3].rotate_left(21) ^ v[0];
        v[2] = v[2].wrapping_add(v[1]);
        v[1] = v[1].rotate_left(17) ^ v[2];
        v[2] = v[2].rotate_left(32);
    }

    // Out of line, as `finish64`: the hash is used in many places, and SipHash's rounds
    // inlined into each cost KBs of module.
    #[inline(never)]
    fn compress(&mut self, m: u64) {
        self.v[3] ^= m;
        Self::round(&mut self.v);
        self.v[0] ^= m;
    }

    #[inline]
    pub fn write(&mut self, unit: u16) {
        self.word |= (unit as u64) << (16 * (self.n & 3));
        self.n += 1;
        if self.n & 3 == 0 {
            let w = self.word;
            self.compress(w);
            self.word = 0;
        }
    }

    /// Code units as UTF-16LE bytes (a two-byte string's data).
    fn write_utf16le(&mut self, bytes: &[u8]) {
        for p in bytes.as_chunks::<2>().0 {
            self.write(u16::from_le_bytes(*p));
        }
    }

    pub fn write_units(&mut self, units: &[u16]) {
        for &u in units {
            self.write(u);
        }
    }

    #[inline(never)]
    pub fn write_view(&mut self, v: StrView) {
        match v {
            StrView::Latin1(b) => b.iter().for_each(|&u| self.write(u as u16)),
            StrView::Utf16(b) => self.write_utf16le(b),
        }
    }

    /// The 64-bit hash: the last units with the unit count in the top 16 bits (at most
    /// three units are left, 48 bits), then SipHash's finalization.
    #[inline(never)]
    fn finish64(mut self) -> u64 {
        let m = self.word | ((self.n as u64 & 0xFFFF) << 48);
        self.compress(m);
        self.v[2] ^= 0xFF;
        for _ in 0..3 {
            Self::round(&mut self.v);
        }
        self.v[0] ^ self.v[1] ^ self.v[2] ^ self.v[3]
    }

    #[inline]
    pub fn finish(self) -> u32 {
        let h = self.finish64();
        let h = (h ^ (h >> 32)) as u32;
        if h == 0 { 1 } else { h }
    }
}

/// The key of the base image's atom index and of the hashes the builtin prefix's strings
/// cache (D2433).
pub const BASE_HASH_KEY: u64 = 0;

/// The keyed hash of one 64-bit word (Map/Set keys that are not strings, D2433).
#[inline(never)]
pub fn keyed_u64(key: u64, x: u64) -> u32 {
    let mut h = KeyedHasher::new(key);
    h.compress(x);
    h.n = 4;
    h.finish()
}

/// The hash of Latin-1 code units under `key`.
#[inline(never)]
pub fn hash_latin1_with(key: u64, bytes: &[u8]) -> u32 {
    let mut h = KeyedHasher::new(key);
    h.write_view(StrView::Latin1(bytes));
    h.finish()
}

/// The hash of code units under `key`.
#[inline(never)]
pub fn hash_units_with(key: u64, units: &[u16]) -> u32 {
    let mut h = KeyedHasher::new(key);
    h.write_units(units);
    h.finish()
}

/// The hash key for a `Math.random` seed (D2433): one-way, so the key is not the generator's
/// state, which a script can read back from its numbers.
pub fn hash_key_for_seed(seed: u64) -> u64 {
    (keyed_u64(0x243F_6A88_85A3_08D3, seed) as u64) << 32
        | keyed_u64(0x1319_8A2E_0370_7344, seed) as u64
}

/// Whether every byte is ASCII. In wasm32, `<[u8]>::is_ascii` runs at 8–9 GB/s in node,
/// [`is_ascii_blocks`] at ~40 (0077, D1780); natively the standard one is faster (SIMD).
#[inline]
pub fn is_ascii(bytes: &[u8]) -> bool {
    #[cfg(target_arch = "wasm32")]
    return is_ascii_blocks(bytes);
    #[cfg(not(target_arch = "wasm32"))]
    return bytes.is_ascii();
}

/// The ASCII check for wasm32: 32-byte blocks OR-ed as u64 words and tested once per
/// block, so the loop has one branch per 32 bytes.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn is_ascii_blocks(bytes: &[u8]) -> bool {
    const HIGH: u64 = 0x8080_8080_8080_8080;
    let (blocks, rest) = bytes.as_chunks::<32>();
    for b in blocks {
        let w = b.as_chunks::<8>().0;
        let acc = u64::from_ne_bytes(w[0])
            | u64::from_ne_bytes(w[1])
            | u64::from_ne_bytes(w[2])
            | u64::from_ne_bytes(w[3]);
        if acc & HIGH != 0 {
            return false;
        }
    }
    let (words, tail) = rest.as_chunks::<8>();
    let mut acc = 0;
    for w in words {
        acc |= u64::from_ne_bytes(*w);
    }
    for &b in tail {
        acc |= b as u64;
    }
    acc & HIGH == 0
}

/// Iterator over the flat pieces of a string, in order, without flattening it.
pub struct Chunks<'a> {
    heap: &'a Heap,
    /// `(cell, length)`: the length clips an append rope's buffer.
    stack: Vec<(u32, u32)>,
}

impl<'a> Iterator for Chunks<'a> {
    type Item = StrView<'a>;

    fn next(&mut self) -> Option<StrView<'a>> {
        while let Some((off, len)) = self.stack.pop() {
            match self.heap.cell_kind(off) {
                CellKind::Str8 | CellKind::Str16 => {
                    let v = self.heap.prefix_view(off, len);
                    if !v.is_empty() {
                        return Some(v);
                    }
                }
                CellKind::Rope => self.heap.push_children(off, &mut self.stack),
                CellKind::Slice => {
                    let (parent, start) = self.heap.slice_parts(off);
                    return Some(self.heap.range_view(parent, start, len));
                }
                k => super::fail::not_a_string(k, off),
            }
        }
        None
    }
}

impl Heap {
    /// The parts of the rope `off` as `(cell, length)`, in order (one or two).
    fn children(&self, off: u32) -> ([(u32, u32); 2], usize) {
        let a = self.arena();
        let fl = self.cell_flags(off);
        let left = a.get(off, Rope::LEFT).0;
        if fl & flags::ROPE_APPEND != 0 {
            ([(left, a.get(off, Rope::LEN)), (0, 0)], 1)
        } else if fl & flags::ROPE_FLATTENED != 0 {
            ([(left, self.str_len(Str(left))), (0, 0)], 1)
        } else {
            let right = a.get(off, Rope::RIGHT).0;
            (
                [
                    (left, self.str_len(Str(left))),
                    (right, self.str_len(Str(right))),
                ],
                2,
            )
        }
    }

    /// Pushes the parts of the rope `off` as `(cell, length)` for a left-to-right walk
    /// with a stack (right first).
    fn push_children(&self, off: u32, stack: &mut Vec<(u32, u32)>) {
        let (kids, n) = self.children(off);
        stack.extend(kids[..n].iter().rev());
    }

    /// The parent and the first unit's index of the slice `off`.
    #[inline]
    fn slice_parts(&self, off: u32) -> (u32, u32) {
        let a = self.arena();
        (a.get(off, Slice::PARENT).0, a.get(off, Slice::OFF))
    }

    /// Units `[start, start + len)` of the flat one-byte cell `off` (a slice's parent).
    fn range_view(&self, off: u32, start: u32, len: u32) -> StrView<'_> {
        StrView::Latin1(self.arena().slice(off + Str8::DATA.offset + start, len))
    }

    /// Where the units of the leaf `cell` (a flat cell or a slice) start in the arena, and
    /// whether it is two-byte.
    fn leaf_data(&self, cell: u32) -> (u32, bool) {
        match self.cell_kind(cell) {
            CellKind::Str8 => (cell + Str8::DATA.offset, false),
            CellKind::Str16 => (cell + Str16::DATA.offset, true),
            CellKind::Slice => {
                let (parent, start) = self.slice_parts(cell);
                (parent + Str8::DATA.offset + start, false)
            }
            k => super::fail::not_a_string(k, cell),
        }
    }

    /// The first `len` code units of the flat cell `off`.
    fn prefix_view(&self, off: u32, len: u32) -> StrView<'_> {
        let data = off + Str8::DATA.offset;
        match self.cell_kind(off) {
            CellKind::Str8 => StrView::Latin1(self.arena().slice(data, len)),
            CellKind::Str16 => StrView::Utf16(self.arena().slice(data, len * 2)),
            k => super::fail::not_a_string(k, off),
        }
    }

    /// The buffer cell of an append rope, if `s` is one.
    fn append_rope_buffer(&self, s: Str) -> Option<u32> {
        (self.cell_kind(s.0) == CellKind::Rope && self.cell_flags(s.0) & flags::ROPE_APPEND != 0)
            .then(|| self.arena().get(s.0, Rope::LEFT).0)
    }

    /// The capacity of a flat string cell in code units.
    fn buffer_capacity(&self, buf: u32) -> u32 {
        let size = self.cell_size(buf);
        if self.cell_kind(buf) == CellKind::Str16 {
            Str16::LAYOUT.tail_capacity(size)
        } else {
            Str8::LAYOUT.tail_capacity(size)
        }
    }

    /// Allocates a flat string cell for `len` code units, with `LEN` set and the data
    /// zeroed.
    /// A new flat string with the code units of the flat string `s`.
    pub(crate) fn copy_flat(&mut self, s: Str) -> Result<Str, HeapError> {
        let (len, two) = (self.str_len(s), self.str_is_two_byte(s));
        let dst = self.alloc_flat(len, two)?;
        let n = if two { 2 * len } else { len };
        self.arena_mut()
            .copy_within(Str8::DATA.at(s.0, 0), n, Str8::DATA.at(dst, 0));
        Ok(Str(dst))
    }

    fn alloc_flat(&mut self, len: u32, two_byte: bool) -> Result<u32, HeapError> {
        if len > MAX_LEN {
            return Err(HeapError::StringTooLong);
        }
        let (kind, size) = if two_byte {
            (CellKind::Str16, Str16::DATA.cell_size(len))
        } else {
            (CellKind::Str8, Str8::DATA.cell_size(len))
        };
        let off = self.alloc_cell(kind, size.ok_or(HeapError::StringTooLong)?)?;
        self.arena_mut().set_untracked(off, Str8::LEN, len);
        Ok(off)
    }

    /// Allocates a flat string of `len` code units with zeroed data, for callers that write
    /// the data themselves (at [`Heap::str_data_offset`], one byte per unit for one-byte
    /// strings, little-endian pairs otherwise). A two-byte string must end up containing a
    /// unit above 0xFF (the width invariant), so only ask for one when that is known.
    pub fn alloc_str_uninit(&mut self, len: u32, two_byte: bool) -> Result<Str, HeapError> {
        self.alloc_flat(len, two_byte).map(Str)
    }

    /// Appends the one-byte units `data` to the one-byte flat string `s` in place, if `s`
    /// is the arena's last cell: its cell grows by what they need. Only for a string under
    /// construction that nothing references yet (JSON.stringify's large output, D2553): it
    /// builds the result where it ends up, without a copy of the whole outside the arena.
    /// The units are written once (past the arena's zeroed frontier they are appended, not
    /// zeroed and then copied). Returns false, changing nothing, when something was
    /// allocated after `s`.
    pub(crate) fn append_last_latin1(&mut self, s: Str, data: &[u8]) -> Result<bool, HeapError> {
        let size = self.cell_size(s.0);
        let end = s.0 + size;
        if end != self.arena.len() {
            return Ok(false);
        }
        debug_assert_eq!(self.cell_kind(s.0), CellKind::Str8);
        let len = self.str_len(s);
        let n = data.len() as u32;
        let new_size = len
            .checked_add(n)
            .filter(|&l| l <= MAX_LEN)
            .and_then(|l| Str8::DATA.cell_size(l))
            .ok_or(HeapError::StringTooLong)?;
        // The cell's padding takes the first bytes, a new block at the frontier the rest.
        let at = Self::str_data_offset(s) + len;
        let k = (end - at).min(n);
        if new_size > size {
            self.arena
                .alloc_init(new_size - size, 0, &data[k as usize..])?;
            if self.arena.len() >= self.gc_threshold {
                self.gc_pending = true;
            }
            self.arena.set(s.0, Header::SIZE, new_size);
        }
        self.arena
            .slice_mut(at, k)
            .copy_from_slice(&data[..k as usize]);
        self.arena.set(s.0, Str8::LEN, len + n);
        Ok(true)
    }

    /// The arena offset of the code units of a flat string cell.
    #[inline]
    pub fn str_data_offset(s: Str) -> u32 {
        s.0 + Str8::DATA.offset
    }

    /// A string from Latin-1 bytes (each byte is one code unit).
    pub fn alloc_latin1(&mut self, data: &[u8]) -> Result<Str, HeapError> {
        let len = u32::try_from(data.len()).map_err(|_| HeapError::StringTooLong)?;
        if len > MAX_LEN {
            return Err(HeapError::StringTooLong);
        }
        let size = Str8::DATA.cell_size(len).ok_or(HeapError::StringTooLong)?;
        let off = self.alloc_cell_init(CellKind::Str8, size, Str8::DATA.offset, data)?;
        self.arena_mut().set_untracked(off, Str8::LEN, len);
        Ok(Str(off))
    }

    /// A one-byte string of the pieces `s[a .. b)`, copied within the arena once (no
    /// bounce through a Rust buffer, 0110). Each piece must be flat, an append rope or a
    /// slice (`str_view_flat` first) and one-byte; `total` is the sum of the lengths.
    pub fn alloc_latin1_pieces(
        &mut self,
        pieces: &[(Str, u32, u32)],
        total: u32,
    ) -> Result<Str, HeapError> {
        let dst = self.alloc_flat(total, false)?;
        let mut at = dst + Str8::DATA.offset;
        for &(s, a, b) in pieces {
            // A flat one-byte cell from one kind read (most pieces); else the rope and
            // slice cases.
            let (cell, from) = if self.arena().get(s.0, Header::KIND) == CellKind::Str8 as u8 {
                (s.0, 0)
            } else {
                let s = self.resolve_str(s);
                let (cell, from) = match self.cell_kind(s.0) {
                    CellKind::Slice => self.slice_parts(s.0),
                    _ => (self.append_rope_buffer(s).unwrap_or(s.0), 0),
                };
                assert!(
                    self.cell_kind(cell) == CellKind::Str8,
                    "alloc_latin1_pieces: not a flat one-byte piece"
                );
                (cell, from)
            };
            assert!(
                a <= b && from + b <= self.str_len(Str(cell)),
                "alloc_latin1_pieces: piece out of range"
            );
            self.arena_mut()
                .copy_within(cell + Str8::DATA.offset + from + a, b - a, at);
            at += b - a;
        }
        Ok(Str(dst))
    }

    /// `Array.prototype.join` over the first `len` slots of an Array's dense store when
    /// each is a flat one-byte string or a slice, `undefined` or `null`, and `sep` is a flat one-byte
    /// string: the result written once, straight from the cells (0129: no list of
    /// pieces, one kind read per element). `None` (nothing allocated) for anything else,
    /// holes included.
    pub(crate) fn join_flat_latin1(
        &mut self,
        o: Obj,
        len: u32,
        sep: Str,
    ) -> Result<Option<Str>, HeapError> {
        let str8 = |h: &Self, s: Str| h.arena().get(s.0, Header::KIND) == CellKind::Str8 as u8;
        if !str8(self, sep) {
            return Ok(None);
        }
        let sep_len = self.str_len(sep);
        // (Checks the class before `array_length`.)
        let Some(values) = self.dense_array_values(o) else {
            return Ok(None);
        };
        if self.array_length(o) != len {
            return Ok(None);
        }
        // One-byte flat strings and slices (the lines of a split): read in place, from
        // where their units start (found once, in the first pass).
        let mut pieces: Vec<(u32, u32)> = crate::buf::vec_with_capacity(len as usize);
        let mut total = u64::from(sep_len) * u64::from(len.saturating_sub(1));
        for v in values {
            match v.as_string() {
                Some(s) => {
                    let k = self.arena().get(s.0, Header::KIND);
                    let data = if k == CellKind::Str8 as u8 {
                        s.0 + Str8::DATA.offset
                    } else if k == CellKind::Slice as u8 {
                        let (parent, start) = self.slice_parts(s.0);
                        parent + Str8::DATA.offset + start
                    } else {
                        return Ok(None);
                    };
                    let n = self.str_len(s);
                    total += u64::from(n);
                    pieces.push_out((data, n));
                }
                None if v.is_nullish() => pieces.push_out((0, 0)),
                None => return Ok(None),
            }
        }
        if total > u64::from(MAX_LEN) {
            return Err(HeapError::StringTooLong);
        }
        let dst = self.alloc_flat(total as u32, false)?;
        let sep_at = (sep.0 + Str8::DATA.offset) as usize;
        // Every piece and the separator are older cells than `dst`, so below it.
        let (src, out) = self.arena_mut().split_mut(dst);
        let sep = &src[sep_at..sep_at + sep_len as usize];
        let mut at = Str8::DATA.offset as usize;
        for (i, &(data, n)) in pieces.iter().enumerate() {
            if i > 0 {
                // (A one-unit separator, the usual one, without a copy call.)
                if let [b] = sep {
                    out[at] = *b;
                } else {
                    out[at..at + sep.len()].copy_from_slice(sep);
                }
                at += sep.len();
            }
            let (data, n) = (data as usize, n as usize);
            out[at..at + n].copy_from_slice(&src[data..data + n]);
            at += n;
        }
        debug_assert_eq!(at, Str8::DATA.offset as usize + total as usize);
        Ok(Some(Str(dst)))
    }

    /// A string from UTF-16 code units (lone surrogates allowed). Stored one-byte if every
    /// unit fits.
    pub fn alloc_utf16(&mut self, units: &[u16]) -> Result<Str, HeapError> {
        let len = u32::try_from(units.len()).map_err(|_| HeapError::StringTooLong)?;
        if len > MAX_LEN {
            return Err(HeapError::StringTooLong);
        }
        let two_byte = units.iter().any(|&u| u > 0xFF);
        // The data is converted straight into the new cell (written once, not zeroed
        // first); `flat_map` over arrays is TrustedLen, so the append is one tight loop.
        let off = if two_byte {
            let size = Str16::DATA.cell_size(len).ok_or(HeapError::StringTooLong)?;
            let data = units.iter().flat_map(|u| u.to_le_bytes());
            self.alloc_cell_from_iter(CellKind::Str16, size, Str16::DATA.offset, len * 2, data)?
        } else {
            let size = Str8::DATA.cell_size(len).ok_or(HeapError::StringTooLong)?;
            let data = units.iter().map(|&u| u as u8);
            self.alloc_cell_from_iter(CellKind::Str8, size, Str8::DATA.offset, len, data)?
        };
        self.arena_mut().set(off, Str8::LEN, len);
        Ok(Str(off))
    }

    /// A string from UTF-8, transcoded to Latin-1 or UTF-16.
    pub fn alloc_str(&mut self, s: &str) -> Result<Str, HeapError> {
        if is_ascii(s.as_bytes()) {
            return self.alloc_latin1(s.as_bytes());
        }
        let mut len: usize = 0;
        let mut two_byte = false;
        for c in s.chars() {
            let c = c as u32;
            two_byte |= c > 0xFF;
            len += if c > 0xFFFF { 2 } else { 1 };
        }
        let len = u32::try_from(len).map_err(|_| HeapError::StringTooLong)?;
        let off = self.alloc_flat(len, two_byte)?;
        let data = off + Str8::DATA.offset;
        if two_byte {
            let dst = self.arena_mut().slice_mut(data, len * 2);
            for (d, u) in dst.as_chunks_mut::<2>().0.iter_mut().zip(s.encode_utf16()) {
                d.copy_from_slice(&u.to_le_bytes());
            }
        } else {
            let dst = self.arena_mut().slice_mut(data, len);
            for (d, c) in dst.iter_mut().zip(s.chars()) {
                *d = c as u32 as u8;
            }
        }
        Ok(Str(off))
    }

    /// Follows a flattened rope to its flat string.
    #[inline]
    pub(crate) fn resolve_str(&self, s: Str) -> Str {
        if self.cell_kind(s.0) == CellKind::Rope
            && self.cell_flags(s.0) & flags::ROPE_FLATTENED != 0
        {
            Str(self.arena().get(s.0, Rope::LEFT).0)
        } else {
            s
        }
    }

    /// Length in UTF-16 code units.
    #[inline]
    pub fn str_len(&self, s: Str) -> u32 {
        self.arena().get(s.0, Str8::LEN)
    }

    /// Whether the string (flat or rope) has a code unit above 0xFF.
    pub fn str_is_two_byte(&self, s: Str) -> bool {
        match self.cell_kind(s.0) {
            CellKind::Str8 | CellKind::Slice => false,
            CellKind::Str16 => true,
            CellKind::Rope => self.cell_flags(s.0) & flags::ROPE_TWO_BYTE != 0,
            k => super::fail::not_a_string(k, s.0),
        }
    }

    /// The view of `s` if it is a flat cell (`Str8`/`Str16`), from one read of its kind
    /// byte; `None` for a rope. What `str_view` returns for a flat cell, without its
    /// repeated kind checks (0110: four or five per call, ~300 instructions of an
    /// `includes`). The view borrows the heap, so it cannot outlive an append (D1610).
    #[inline(always)]
    fn flat_fast(&self, s: Str) -> Option<StrView<'_>> {
        let a = self.arena();
        let k = a.get(s.0, Header::KIND);
        let one_byte = k == CellKind::Str8 as u8;
        if !one_byte && k != CellKind::Str16 as u8 {
            return None;
        }
        let len = a.get(s.0, Str8::LEN);
        let data = s.0 + Str8::DATA.offset;
        Some(if one_byte {
            StrView::Latin1(a.slice(data, len))
        } else {
            StrView::Utf16(a.slice(data, len * 2))
        })
    }

    /// The view of a flat string cell. `s` must be a `Str8` or `Str16`.
    fn flat_view(&self, s: Str) -> StrView<'_> {
        self.prefix_view(s.0, self.str_len(s))
    }

    /// The code units of `s` if they lie in one piece (a flat cell, a flattened rope, an
    /// append rope or a slice), else `None`.
    pub fn str_view(&self, s: Str) -> Option<StrView<'_>> {
        if let Some(v) = self.flat_fast(s) {
            return Some(v);
        }
        let s = self.resolve_str(s);
        if let Some(buf) = self.append_rope_buffer(s) {
            return Some(self.prefix_view(buf, self.str_len(s)));
        }
        match self.cell_kind(s.0) {
            CellKind::Rope => None,
            CellKind::Slice => {
                let (parent, start) = self.slice_parts(s.0);
                Some(self.range_view(parent, start, self.str_len(s)))
            }
            _ => Some(self.flat_view(s)),
        }
    }

    /// The code units of `s`, flattening it first if it is a concatenation rope. An append
    /// rope and a slice are read in place (no copy; the buffer stays appendable).
    pub fn str_view_flat(&mut self, s: Str) -> Result<StrView<'_>, HeapError> {
        // (Two lookups: the borrow checker does not let the first one's view escape.)
        if self.flat_fast(s).is_some() {
            return Ok(self.flat_fast(s).expect("flat"));
        }
        let s = self.resolve_str(s);
        if let Some(buf) = self.append_rope_buffer(s) {
            return Ok(self.prefix_view(buf, self.str_len(s)));
        }
        if self.cell_kind(s.0) == CellKind::Slice {
            let (parent, start) = self.slice_parts(s.0);
            return Ok(self.range_view(parent, start, self.str_len(s)));
        }
        let f = self.flatten(s)?;
        Ok(self.flat_view(f))
    }

    /// Iterates over the flat pieces of `s` without flattening it.
    pub fn str_chunks(&self, s: Str) -> Chunks<'_> {
        Chunks {
            heap: self,
            stack: alloc::vec![(s.0, self.str_len(s))],
        }
    }

    /// Iterates over the code units of `s` without flattening it.
    pub fn str_units(&self, s: Str) -> impl Iterator<Item = u16> + '_ {
        self.str_chunks(s).flat_map(StrView::units)
    }

    /// The code unit at `i`, or `None` when out of range. Does not flatten; on an
    /// unflattened rope this walks down the tree, so callers indexing repeatedly should
    /// flatten first.
    pub fn char_code_at(&self, s: Str, i: u32) -> Option<u16> {
        let mut s = self.resolve_str(s);
        if i >= self.str_len(s) {
            return None;
        }
        let mut i = i;
        loop {
            match self.cell_kind(s.0) {
                CellKind::Str8 | CellKind::Str16 => return Some(self.flat_view(s).get(i as usize)),
                CellKind::Slice => {
                    let (parent, start) = self.slice_parts(s.0);
                    return Some(
                        self.arena()
                            .read::<u8>(parent + Str8::DATA.offset + start + i)
                            as u16,
                    );
                }
                _ => {
                    let a = self.arena();
                    if self.cell_flags(s.0) & flags::ROPE_APPEND != 0 {
                        // i < LEN <= the buffer's used length.
                        let buf = a.get(s.0, Rope::LEFT).0;
                        return Some(self.prefix_view(buf, i + 1).get(i as usize));
                    }
                    if self.cell_flags(s.0) & flags::ROPE_FLATTENED != 0 {
                        s = Str(a.get(s.0, Rope::LEFT).0);
                        continue;
                    }
                    let left = Str(a.get(s.0, Rope::LEFT).0);
                    let ll = self.str_len(left);
                    if i < ll {
                        s = left;
                    } else {
                        i -= ll;
                        s = Str(a.get(s.0, Rope::RIGHT).0);
                    }
                }
            }
        }
    }

    /// Appends the flat leaves of `s` as `(cell, length)`, in order, to `out`.
    /// The leaves of `parts` (strings with fewer than [`ROPE_MIN_LEN`] units in total), in
    /// order, into `out`; returns how many. No Rust allocation (D1871): `'a' + i` used to
    /// spend more in malloc on three Vecs than on the concatenation. Every pending subtree
    /// and every leaf holds at least one unit, so neither array can overflow.
    fn short_leaves(
        &self,
        parts: [(u32, u32); 2],
        out: &mut [(u32, u32); ROPE_MIN_LEN as usize],
    ) -> usize {
        let mut stack = [(0, 0); ROPE_MIN_LEN as usize];
        stack[0] = parts[1];
        stack[1] = parts[0];
        let (mut sp, mut n) = (2, 0);
        while sp > 0 {
            sp -= 1;
            let (off, len) = stack[sp];
            if self.cell_kind(off) == CellKind::Rope {
                let (kids, k) = self.children(off);
                for &kid in kids[..k].iter().rev() {
                    stack[sp] = kid;
                    sp += 1;
                }
            } else {
                out[n] = (off, len);
                n += 1;
            }
        }
        n
    }

    fn collect_leaves(&self, s: Str, out: &mut Vec<(u32, u32)>) {
        let mut stack = alloc::vec![(s.0, self.str_len(s))];
        while let Some((off, len)) = stack.pop() {
            match self.cell_kind(off) {
                CellKind::Rope => self.push_children(off, &mut stack),
                _ => out.push_out((off, len)),
            }
        }
    }

    /// Copies the first `len` units of each flat cell in `leaves`, back to back, into the
    /// data of the flat cell `dst` from code unit `at` on. The leaves must lie below `dst`
    /// in the arena.
    fn write_leaves(&mut self, leaves: &[(u32, u32)], dst: u32, at: u32) {
        let two_byte = self.cell_kind(dst) == CellKind::Str16;
        let (src, out) = self.arena_mut().split_mut(dst);
        let unit = if two_byte { 2 } else { 1 };
        let mut pos = Str8::DATA.offset as usize + unit * at as usize;
        for &(leaf, len) in leaves {
            let leaf = leaf as usize;
            let len = len as usize;
            let leaf_two = src[leaf] == CellKind::Str16 as u8;
            let data = if src[leaf] == CellKind::Slice as u8 {
                // A slice's parent is older than the slice, so below `dst` too.
                let word = |at: usize| u32::from_le_bytes(src[at..at + 4].try_into().unwrap());
                let parent = word(leaf + Slice::PARENT.offset as usize) as usize;
                parent
                    + Str8::DATA.offset as usize
                    + word(leaf + Slice::OFF.offset as usize) as usize
            } else {
                leaf + Str8::DATA.offset as usize
            };
            match (leaf_two, two_byte) {
                (false, false) => {
                    out[pos..pos + len].copy_from_slice(&src[data..data + len]);
                    pos += len;
                }
                (false, true) => {
                    let d = &mut out[pos..pos + 2 * len];
                    for (d, &b) in d
                        .as_chunks_mut::<2>()
                        .0
                        .iter_mut()
                        .zip(&src[data..data + len])
                    {
                        d[0] = b;
                        d[1] = 0;
                    }
                    pos += 2 * len;
                }
                (true, true) => {
                    out[pos..pos + 2 * len].copy_from_slice(&src[data..data + 2 * len]);
                    pos += 2 * len;
                }
                (true, false) => unreachable!("two-byte leaf in a one-byte string"),
            }
        }
    }

    /// Appends `b` in place to the append buffer `buf`, whose used part ends at `at` (the
    /// caller checked the room, the width and that `at` is the end). The pieces may lie
    /// anywhere in the arena.
    fn append_into(&mut self, buf: u32, at: u32, b: Str) {
        if matches!(self.cell_kind(b.0), CellKind::Str8 | CellKind::Str16) {
            let lb = self.str_len(b);
            self.copy_piece(b.0, lb, buf, at);
            return;
        }
        // A small rope (a template literal's result): walk it with a stack on the Rust
        // stack, so the hot path does not allocate.
        let mut stack = [(0u32, 0u32); 16];
        let mut n = 0;
        stack[0] = (b.0, self.str_len(b));
        n += 1;
        let mut at = at;
        while n > 0 {
            n -= 1;
            let (off, len) = stack[n];
            if self.cell_kind(off) != CellKind::Rope {
                self.copy_piece(off, len, buf, at);
                at += len;
                continue;
            }
            let (kids, nk) = self.children(off);
            if n + nk > stack.len() {
                // Deep: the general walk.
                let mut leaves = Vec::new();
                for &(k, kl) in &kids[..nk] {
                    self.collect_leaves_of(k, kl, &mut leaves);
                }
                for (k, kl) in stack[..n].iter().rev().copied() {
                    self.collect_leaves_of(k, kl, &mut leaves);
                }
                for (leaf, len) in leaves {
                    self.copy_piece(leaf, len, buf, at);
                    at += len;
                }
                return;
            }
            for &k in kids[..nk].iter().rev() {
                stack[n] = k;
                n += 1;
            }
        }
    }

    /// The leaves of `(off, len)` (a string cell clipped to `len`), appended to `out`.
    fn collect_leaves_of(&self, off: u32, len: u32, out: &mut Vec<(u32, u32)>) {
        let mut stack = alloc::vec![(off, len)];
        while let Some((off, len)) = stack.pop() {
            match self.cell_kind(off) {
                CellKind::Rope => self.push_children(off, &mut stack),
                _ => out.push_out((off, len)),
            }
        }
    }

    /// Copies the first `len` units of the flat cell `src` into the buffer `buf` at unit
    /// `at` (widening when needed; anywhere in the arena).
    fn copy_piece(&mut self, src: u32, len: u32, buf: u32, at: u32) {
        let (from, src_two) = self.leaf_data(src);
        let wide = self.cell_kind(buf) == CellKind::Str16;
        let a = self.arena_mut();
        match (src_two, wide) {
            (false, false) => a.copy_within(from, len, buf + Str8::DATA.offset + at),
            (true, true) => a.copy_within(from, 2 * len, buf + Str16::DATA.offset + 2 * at),
            (false, true) => {
                for i in 0..len {
                    let c = a.read::<u8>(from + i);
                    a.write::<u16>(buf + Str16::DATA.offset + 2 * (at + i), c as u16);
                }
            }
            (true, false) => unreachable!("two-byte piece in a one-byte buffer"),
        }
    }

    /// A new append rope over `buf` with length `len`.
    fn new_append_rope(&mut self, buf: u32, len: u32) -> Result<Str, HeapError> {
        let two = self.cell_kind(buf) == CellKind::Str16;
        let r = self.alloc_cell(CellKind::Rope, Rope::LAYOUT.min_size)?;
        let ar = self.arena_mut();
        ar.set(r, Rope::LEN, len);
        ar.set(r, Rope::LEFT, Ref(buf));
        ar.set(r, Rope::RIGHT, Ref::NULL);
        let fl = flags::ROPE_APPEND | if two { flags::ROPE_TWO_BYTE } else { 0 };
        self.set_cell_flags(r, fl);
        Ok(Str(r))
    }

    /// A new append buffer holding `a` then `b`, with about twice their length as
    /// capacity, and an append rope over it.
    fn start_buffer(&mut self, a: Str, b: Str, len: u32, two_byte: bool) -> Result<Str, HeapError> {
        let cap = len.saturating_mul(2).clamp(APPEND_MIN_CAP, MAX_LEN);
        let buf = match self.alloc_flat(cap, two_byte) {
            Ok(buf) => buf,
            // No room for the slack: an exact buffer still works (it regrows later).
            Err(HeapError::OutOfMemory) => self.alloc_flat(len, two_byte)?,
            Err(e) => return Err(e),
        };
        let mut leaves = Vec::new();
        self.collect_leaves(a, &mut leaves);
        self.collect_leaves(b, &mut leaves);
        self.write_leaves(&leaves, buf, 0);
        self.arena_mut().set(buf, Str8::LEN, len);
        self.set_cell_flags(buf, flags::STR_APPEND_BUF);
        self.new_append_rope(buf, len)
    }

    /// Whether `a + b` looks like a step of `s += piece` in a loop: `a` is already an
    /// append rope, or a concatenation rope of at least `APPEND_START_LEN` units whose
    /// right part is short, and `b` is not longer than `a`. Such chains get a buffer (D1610); other concatenations stay O(1)
    /// ropes.
    fn appending(&self, a: Str, la: u32, lb: u32) -> bool {
        if lb > la || la < APPEND_START_LEN || self.cell_kind(a.0) != CellKind::Rope {
            return false;
        }
        let fl = self.cell_flags(a.0);
        if fl & flags::ROPE_APPEND != 0 {
            return true;
        }
        fl & flags::ROPE_FLATTENED == 0
            && self.str_len(Str(self.arena().get(a.0, Rope::RIGHT).0)) <= 2 * ROPE_MIN_LEN
    }

    /// Returns the flat form of `s`. A rope is copied into a new flat cell once and then
    /// redirected to it; later calls return that cell. An append rope that covers its
    /// whole buffer hands out the buffer itself, frozen first (from then on it may be
    /// referenced directly, so it is never written again); a shorter one is copied.
    pub fn flatten(&mut self, s: Str) -> Result<Str, HeapError> {
        let k = self.arena().get(s.0, Header::KIND);
        let slice = k == CellKind::Slice as u8;
        if k != CellKind::Rope as u8 && !slice {
            // Flat already (a flat cell's kind is checked where it is read).
            return Ok(s);
        }
        let s = self.resolve_str(s);
        if !matches!(self.cell_kind(s.0), CellKind::Rope | CellKind::Slice) {
            return Ok(s);
        }
        let len = self.str_len(s);
        let two_byte = self.str_is_two_byte(s);
        let dst = match self.append_rope_buffer(s) {
            Some(buf) if self.str_len(Str(buf)) == len => {
                let f = self.cell_flags(buf);
                self.set_cell_flags(buf, f & !flags::STR_APPEND_BUF);
                buf
            }
            _ => {
                let mut leaves = Vec::new();
                self.collect_leaves(s, &mut leaves);
                let dst = self.alloc_flat(len, two_byte)?;
                self.write_leaves(&leaves, dst, 0);
                dst
            }
        };
        // (A rope above the prefix caches the keyed hash, as its flat copy will.)
        let hash = if s.0 >= self.base_end() && dst >= self.base_end() {
            self.arena().get(s.0, Rope::HASH)
        } else {
            0
        };
        let a = self.arena_mut();
        if hash != 0 && a.get(dst, Str8::HASH) == 0 {
            a.set(dst, Str8::HASH, hash);
        }
        // A slice becomes a flattened rope in place (same size, `PARENT` is `LEFT`): later
        // reads find the copy, and the parent is no longer kept.
        a.set(s.0, Header::KIND, CellKind::Rope as u8);
        a.set(s.0, Rope::LEFT, Ref(dst));
        a.set(s.0, Rope::RIGHT, Ref::NULL);
        let f = if slice { 0 } else { self.cell_flags(s.0) };
        self.set_cell_flags(s.0, (f & !flags::ROPE_APPEND) | flags::ROPE_FLATTENED);
        Ok(Str(dst))
    }

    /// `s` itself if its units lie in one piece (`str_view` reads it in place: a flat
    /// cell, an append rope, a slice), else its flat form. For readers that need a view
    /// but no flat cell: unlike [`Heap::flatten`], a slice is not copied.
    #[inline]
    pub fn contiguous(&mut self, s: Str) -> Result<Str, HeapError> {
        // (One kind read for flat strings and slices, as `flatten` has for flat ones.)
        if self.arena().get(s.0, Header::KIND) != CellKind::Rope as u8
            || self.append_rope_buffer(s).is_some()
        {
            return Ok(s);
        }
        self.flatten(s)
    }

    /// A string of the units `s[start .. end)` that shares `s`'s units instead of
    /// copying them (D2542), when that pays: at least [`SLICE_MIN_LEN`] units of a
    /// one-byte string of at least [`SLICE_PARENT_MIN_LEN`]. `None` otherwise (the caller
    /// copies). A slice of a slice points at the same parent.
    pub fn substring_slice(
        &mut self,
        s: Str,
        start: u32,
        end: u32,
    ) -> Result<Option<Str>, HeapError> {
        if end - start < SLICE_MIN_LEN || self.str_len(s) < SLICE_PARENT_MIN_LEN {
            return Ok(None);
        }
        // (A rope is not sliced: callers that cut many pieces flatten it first.)
        match self.slice_source(s) {
            Some((parent, base)) => self
                .alloc_slice(parent, base + start, end - start)
                .map(Some),
            None => Ok(None),
        }
    }

    /// The parent and offset that slices of `s` read from: `s` itself if it is a flat
    /// one-byte string, its parent if it is a slice, else `None`.
    pub(crate) fn slice_source(&self, s: Str) -> Option<(u32, u32)> {
        let s = self.resolve_str(s);
        match self.cell_kind(s.0) {
            CellKind::Str8 => Some((s.0, 0)),
            CellKind::Slice => Some(self.slice_parts(s.0)),
            _ => None,
        }
    }

    /// A slice of `len` units of the flat one-byte string `parent` from unit `off` on.
    pub(crate) fn alloc_slice(
        &mut self,
        parent: u32,
        off: u32,
        len: u32,
    ) -> Result<Str, HeapError> {
        let c = self.alloc_cell(CellKind::Slice, Slice::LAYOUT.min_size)?;
        let a = self.arena_mut();
        a.set(c, Slice::LEN, len);
        a.set(c, Slice::PARENT, Ref(parent));
        a.set(c, Slice::OFF, off);
        Ok(Str(c))
    }

    /// Where the units of `s` start in the arena and whether they are two-byte; `s` must
    /// be [`Heap::contiguous`] (a flat cell, an append rope or a slice).
    pub(crate) fn contiguous_data(&self, s: Str) -> (u32, bool) {
        let s = self.resolve_str(s);
        let cell = self.append_rope_buffer(s).unwrap_or(s.0);
        self.leaf_data(cell)
    }

    /// Concatenates two strings. Results shorter than [`ROPE_MIN_LEN`] are flat copies;
    /// longer ones are ropes.
    pub fn concat(&mut self, a: Str, b: Str) -> Result<Str, HeapError> {
        let a = self.resolve_str(a);
        let b = self.resolve_str(b);
        let (la, lb) = (self.str_len(a), self.str_len(b));
        if la == 0 {
            return Ok(b);
        }
        if lb == 0 {
            return Ok(a);
        }
        let len = la + lb; // both <= MAX_LEN < 2^30, so no overflow
        if len > MAX_LEN {
            return Err(HeapError::StringTooLong);
        }
        let two_byte = self.str_is_two_byte(a) || self.str_is_two_byte(b);
        if len < ROPE_MIN_LEN {
            // Two flat pieces (the usual `a + b`): copied as they are, without the walk
            // and its two zeroed 32-entry arrays.
            if self.cell_kind(a.0) != CellKind::Rope && self.cell_kind(b.0) != CellKind::Rope {
                let dst = self.alloc_flat(len, two_byte)?;
                self.write_leaves(&[(a.0, la), (b.0, lb)], dst, 0);
                return Ok(Str(dst));
            }
            let mut leaves = [(0, 0); ROPE_MIN_LEN as usize];
            let n = self.short_leaves([(a.0, la), (b.0, lb)], &mut leaves);
            let dst = self.alloc_flat(len, two_byte)?;
            self.write_leaves(&leaves[..n], dst, 0);
            return Ok(Str(dst));
        }
        // In place (D1610): `a` ends exactly where its buffer's used part ends (no other
        // string sees the units past it), the buffer is not frozen, and `b` fits in room
        // and width.
        if let Some(buf) = self.append_rope_buffer(a)
            && self.cell_flags(buf) & flags::STR_APPEND_BUF != 0
            && self.str_len(Str(buf)) == la
            && len <= self.buffer_capacity(buf)
            && (self.cell_kind(buf) == CellKind::Str16 || !two_byte)
        {
            self.append_into(buf, la, b);
            self.arena_mut().set(buf, Str8::LEN, len);
            return self.new_append_rope(buf, len);
        }
        if self.appending(a, la, lb) {
            return self.start_buffer(a, b, len, two_byte);
        }
        let r = self.alloc_cell(CellKind::Rope, Rope::LAYOUT.min_size)?;
        let ar = self.arena_mut();
        ar.set_untracked(r, Rope::LEN, len);
        ar.set_untracked(r, Rope::LEFT, Ref(a.0));
        ar.set_untracked(r, Rope::RIGHT, Ref(b.0));
        if two_byte {
            self.set_cell_flags(r, flags::ROPE_TWO_BYTE);
        }
        Ok(Str(r))
    }

    /// The VM's keyed hash key (`Roots::HASH_KEY`, D2433).
    #[inline]
    pub(crate) fn hash_key(&self) -> u64 {
        self.arena().get(0, super::cells::Roots::HASH_KEY)
    }

    /// The keyed hash of a string ([`KeyedHasher`], D2433): what the VM's own atom index,
    /// Map/Set and string equality use. Cached in strings above the builtin prefix; the
    /// prefix, which no VM writes, caches the base hash instead ([`Heap::str_hash_base`]).
    pub fn str_hash(&mut self, s: Str) -> u32 {
        let s = self.resolve_str(s);
        if s.0 < self.base_end() {
            return self.keyed_content_hash(s);
        }
        let cached = self.arena().get(s.0, Str8::HASH);
        if cached != 0 {
            return cached;
        }
        let h = self.keyed_content_hash(s);
        self.arena_mut().set(s.0, Str8::HASH, h);
        h
    }

    /// The keyed hash without caching it.
    pub fn str_hash_uncached(&self, s: Str) -> u32 {
        let s = self.resolve_str(s);
        if s.0 >= self.base_end() {
            let cached = self.arena().get(s.0, Str8::HASH);
            if cached != 0 {
                return cached;
            }
        }
        self.keyed_content_hash(s)
    }

    /// The keyed hash of code units given as such.
    pub fn keyed_hash_units(&self, units: &[u16]) -> u32 {
        hash_units_with(self.hash_key(), units)
    }

    /// The keyed hash of Latin-1 code units.
    pub fn keyed_hash_latin1(&self, bytes: &[u8]) -> u32 {
        hash_latin1_with(self.hash_key(), bytes)
    }

    fn keyed_content_hash(&self, s: Str) -> u32 {
        self.content_hash_with(s, self.hash_key())
    }

    #[inline(never)]
    fn content_hash_with(&self, s: Str, key: u64) -> u32 {
        let mut h = KeyedHasher::new(key);
        match self.str_view(s) {
            Some(v) => h.write_view(v),
            None => self.str_chunks(s).for_each(|v| h.write_view(v)),
        }
        h.finish()
    }

    /// The base hash of a string (under [`BASE_HASH_KEY`]): the base image's atom index
    /// (D2433). Cached in the builtin prefix's strings (the base's atoms), computed for
    /// others.
    pub fn str_hash_base(&self, s: Str) -> u32 {
        let s = self.resolve_str(s);
        if s.0 < self.base_end() {
            let cached = self.arena().get(s.0, Str8::HASH);
            if cached != 0 {
                return cached;
            }
        }
        self.content_hash_with(s, BASE_HASH_KEY)
    }

    /// Makes the builtin prefix's cached string hashes the base ones (D2433), for the
    /// base image's build: the building VM cached keyed hashes before `BASE_END` existed.
    pub(crate) fn prefix_hashes_to_base(&mut self) {
        let base = self.base_end();
        let strings: Vec<u32> = self
            .cells()
            .take_while(|c| c.offset < base)
            .filter(|c| matches!(c.kind, CellKind::Str8 | CellKind::Str16 | CellKind::Rope))
            .map(|c| c.offset)
            .collect();
        for c in strings {
            if self.arena().get(c, Str8::HASH) != 0 {
                let h = self.content_hash_with(Str(c), BASE_HASH_KEY);
                self.arena_mut().set(c, Str8::HASH, h);
            }
        }
    }

    /// Replaces the keyed hash's key (D2433): the strings above the prefix forget their
    /// cached hashes, and the VM's own atom index and every Map/Set index are rebuilt
    /// under the new key (insertion order is the entries' order, which stays). No-op for
    /// the key in use. The prefix is not written: its strings cache the base hash.
    #[inline(never)]
    pub(crate) fn set_hash_key(&mut self, key: u64) {
        use super::ObjectClass;
        if key == self.hash_key() {
            return;
        }
        self.arena_mut().set(0, super::cells::Roots::HASH_KEY, key);
        let base = self.base_end();
        // Two walks over the cells (headers only, nothing moves): every cached hash goes
        // before any index is rebuilt with the new key.
        for pass in 0..2 {
            let mut c = 0;
            while c < self.arena().len() {
                let h: u64 = self.arena().read(c);
                let kind = h as u8;
                let string = kind == CellKind::Str8 as u8
                    || kind == CellKind::Str16 as u8
                    || kind == CellKind::Rope as u8
                    || kind == CellKind::Slice as u8;
                if pass == 0 && string && c >= base {
                    self.arena_mut().set(c, Str8::HASH, 0);
                } else if pass == 1
                    && kind == CellKind::Object as u8
                    && (ObjectClass::Map as u32..=ObjectClass::WeakSet as u32)
                        .contains(&self.arena().get(c, super::cells::Object::CLASS))
                {
                    self.collection_rehash(Obj(c));
                }
                c += (h >> 32) as u32;
            }
            if pass == 0 {
                self.rekey_atoms();
            }
        }
    }

    /// Code unit equality.
    pub fn str_eq(&self, a: Str, b: Str) -> bool {
        if a == b {
            return true;
        }
        let a = self.resolve_str(a);
        let b = self.resolve_str(b);
        if a == b {
            return true;
        }
        if self.str_len(a) != self.str_len(b) || self.str_is_two_byte(a) != self.str_is_two_byte(b)
        {
            return false;
        }
        let (ha, hb) = (
            self.arena().get(a.0, Str8::HASH),
            self.arena().get(b.0, Str8::HASH),
        );
        // The prefix caches the base hash, the rest the keyed one (D2433).
        let base = self.base_end();
        if ha != 0 && hb != 0 && ha != hb && (a.0 >= base) == (b.0 >= base) {
            return false;
        }
        match (self.str_view(a), self.str_view(b)) {
            // Same width (checked above), so equal content means equal bytes.
            (Some(x), Some(y)) => x.bytes() == y.bytes(),
            _ => self.str_units(a).eq(self.str_units(b)),
        }
    }

    /// Code unit order (what `<` on strings means in JS).
    pub fn str_cmp(&self, a: Str, b: Str) -> Ordering {
        if a == b {
            return Ordering::Equal;
        }
        match (self.str_view(a), self.str_view(b)) {
            (Some(StrView::Latin1(x)), Some(StrView::Latin1(y))) => x.cmp(y),
            _ => self.str_units(a).cmp(self.str_units(b)),
        }
    }

    /// Whether `s` has the same code units as the Rust string `t`.
    pub fn str_eq_str(&self, s: Str, t: &str) -> bool {
        if let Some(StrView::Latin1(b)) = self.str_view(s)
            && is_ascii(t.as_bytes())
        {
            return b == t.as_bytes();
        }
        self.str_units(s).eq(t.encode_utf16())
    }

    /// Converts to a Rust string, replacing lone surrogates with U+FFFD. For tests,
    /// debugging and host output.
    pub fn str_to_string(&self, s: Str) -> String {
        let mut out = String::with_capacity(self.str_len(s) as usize);
        self.push_str_to(s, &mut out);
        out
    }

    /// Appends `s` to `out` as [`Heap::str_to_string`] converts it.
    pub fn push_str_to(&self, s: Str, out: &mut String) {
        // A high surrogate waits for its low one, which may start the next piece.
        let mut high: Option<u16> = None;
        for chunk in self.str_chunks(s) {
            match chunk {
                StrView::Latin1(b) => {
                    if high.take().is_some() {
                        out.push(char::REPLACEMENT_CHARACTER);
                    }
                    // ASCII (most source text and JSON) is UTF-8 as it is.
                    match core::str::from_utf8(b) {
                        Ok(text) if b.is_ascii() => out.push_str(text),
                        _ => out.extend(b.iter().map(|&c| c as char)),
                    }
                }
                StrView::Utf16(_) => {
                    for u in chunk.units() {
                        if let Some(h) = high.take() {
                            if (0xDC00..=0xDFFF).contains(&u) {
                                let c = 0x1_0000
                                    + ((u32::from(h) - 0xD800) << 10)
                                    + (u32::from(u) - 0xDC00);
                                out.push(char::from_u32(c).unwrap_or(char::REPLACEMENT_CHARACTER));
                                continue;
                            }
                            out.push(char::REPLACEMENT_CHARACTER);
                        }
                        if (0xD800..=0xDBFF).contains(&u) {
                            high = Some(u);
                        } else {
                            out.push(
                                char::from_u32(u32::from(u)).unwrap_or(char::REPLACEMENT_CHARACTER),
                            );
                        }
                    }
                }
            }
        }
        if high.is_some() {
            out.push(char::REPLACEMENT_CHARACTER);
        }
    }

    /// The code units as a vector (tests and debugging).
    pub fn str_to_utf16(&self, s: Str) -> Vec<u16> {
        self.str_units(s).collect()
    }
}
