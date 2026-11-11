//! The in-place mark-compact collector (0101, docs/mark-compact.md): the VM's own
//! collections, without a to-space.
//!
//! Four passes over the arena:
//! 1. **Mark** from the roots cell. Marking sets `gc_bits::MARK` in the header and the
//!    cell's words in a live-word bitmap (one bit per 8 bytes). The mark stack is
//!    bounded; on overflow a cell is marked but not pushed, and a walk over the marked
//!    cells rescans them afterwards. References to a flattened rope mark its flat string
//!    instead (Cheney's short-circuit). Ephemeron tables are marked but not traced: their
//!    pairs are resolved to a fixpoint, then the pairs whose key died are cleared.
//! 2. **Block table:** the new address of each 512-byte block's first live word, a prefix
//!    sum over the bitmap. A live cell at `a` moves to `table[a / 512]` plus 8 times the
//!    live words before it in its block (one popcount). Nothing is written into cells.
//! 3. **Update** the old region's slots that reference young cells.
//! 4. **Update and slide**, in one walk over the marked cells in address order: rewrite
//!    every traced slot to its target's new address, clear the mark, and move each run
//!    of adjacent live cells down; then truncate the freed tail. The offset slots of
//!    inline caches (D1621) were emptied when their template was traced, as Cheney
//!    does. Map/Set indexes are rehashed afterwards (D1381).
//!
//! A **minor** collection (D2041) leaves the old region `[0, Roots::OLD_END)` alone: its
//! cells count as live and are neither marked nor moved. The mark pass starts from a
//! linear walk over them instead of the roots cell: every traced slot with a young target
//! is a root, and is remembered, so the update pass rewrites only those old slots. Old
//! ephemeron tables join the fixpoint, old collections with young keys are rehashed, and
//! the bitmap, the block table and the slide cover only the young cells.
//!
//! Only the *set* of marked cells reaches the result, so the mark order (stack,
//! overflow rescans, the ephemeron fixpoint) cannot make it nondeterministic, and a full
//! collection keeps the same cells at the same sizes as Cheney: live bytes, thresholds
//! and GC points do not depend on which collector runs. The side tables (len/64 + len/128
//! bytes, a fixed stack, and in a minor collection the remembered list) are allocated
//! before anything is touched: a refusal leaves the heap as it was.
use crate::PushOut;

use alloc::vec::Vec;

use super::arena::Arena;
use super::cells::{EphemeronArray, Header, Object, Rope, Slice, Str8, flags, gc_bits};
use super::layout::Ref;
use super::object::ObjectClass;
use super::walk::{self, Slot};
use super::{CellKind, Heap, HeapError};
use crate::value::{Obj, Value};

/// Mark stack entries allowed at least (64 KiB). The stack grows on demand up to
/// len/64 entries (len/16 bytes, for deep structures like long lists) or this; past that,
/// or when the host refuses, cells are marked without being pushed and found by a rescan.
const STACK_CAP: usize = 16 * 1024;

/// Mark stack entries reserved up front (16 KiB): small enough for the wasm module's
/// dlmalloc segment, where a bigger block becomes a page run at memory's end (D3143).
const STACK_RESERVE: usize = 4 * 1024;

#[cfg(test)]
extern crate std;

#[cfg(test)]
std::thread_local! {
    /// Test hook: a smaller mark stack, so the overflow path runs (0 = `STACK_CAP`).
    pub(crate) static TEST_STACK_CAP: core::cell::Cell<usize> = const { core::cell::Cell::new(0) };
}

/// The largest mark stack (entries) for an arena of `len` bytes.
fn stack_cap(len: u32) -> usize {
    #[cfg(test)]
    {
        let c = TEST_STACK_CAP.with(|c| c.get());
        if c != 0 {
            return c;
        }
    }
    (len as usize / 64).max(STACK_CAP)
}

/// Entries of the remembered list of a minor collection (16 KiB). Old regions point to
/// few young cells (the interpreter's stack and registers, the handles, globals); past
/// that, the update pass walks the old region instead.
const REMEMBERED_CAP: usize = 4096;

fn remembered_cap() -> usize {
    #[cfg(test)]
    {
        let c = TEST_STACK_CAP.with(|c| c.get());
        if c != 0 {
            return c;
        }
    }
    REMEMBERED_CAP
}

/// Bytes per block of the forwarding table: one `u64` of the bitmap.
const BLOCK: u32 = 512;

/// A full collection keeps a slice's parent that nothing but slices reaches only if the
/// slices cover at least a `1/SLICE_KEEP_SHARE` of it, or it is shorter than
/// [`SLICE_KEEP_LEN`] units; otherwise the slices get copies of their units and the
/// parent dies (D2542). A short slice of a big string then does not keep the string.
pub(crate) const SLICE_KEEP_SHARE: u64 = 4;

/// See [`SLICE_KEEP_SHARE`].
pub(crate) const SLICE_KEEP_LEN: u32 = 4096;

struct Marker<'a> {
    arena: &'a mut Arena,
    /// The end of the old region: 0 for a full collection. Cells below it are live and
    /// are neither marked nor moved.
    old: u32,
    /// One bit per 8-byte word of a marked cell, from `old`.
    live: Live,
    stack: Vec<u32>,
    overflow: bool,
    /// The lowest cell marked but not pushed since the last rescan.
    overflow_from: u32,
    /// The largest stack (entries) before overflowing.
    cap: usize,
    /// Marked ephemeron tables (their pairs need the fixpoint).
    ephemerons: u32,
    /// Ephemeron tables in the old region (found by the old walk).
    old_ephemerons: u32,
    /// A full collection: a slice's parent is not marked through the slice, but decided
    /// on after the mark (`resolve_slices`, D2542).
    weak_slices: bool,
    /// Slices marked with their parent left to `resolve_slices`.
    slices: u32,
    /// The old walk's finds for the update pass: slots holding a young reference (a
    /// `Value` slot, or a `Ref` slot with [`REMEMBER_REF`]) and old collections to
    /// rehash ([`REMEMBER_REHASH`]). Valid only while `remembered_ok`.
    remembered: Vec<u32>,
    remembered_ok: bool,
    /// Bytes of marked leaf cells of at least [`super::gc::CHEAP_LEAF`] (D3144).
    cheap: u32,
}

/// What [`Heap::mark_compact`] did.
pub(super) struct Compacted {
    /// The arena's size before and after.
    pub before: u32,
    pub after: u32,
    /// Where the old region ends now: the live bytes below its old end.
    pub old_end: u32,
    /// Slices flagged for a copy of their units (`resolve_slices`).
    pub detach: Vec<u32>,
    /// Live bytes in large leaf cells the collection marked (D3144).
    pub cheap: u32,
}

/// A remembered `Ref` slot (slots are 4-aligned, so the low bits are free).
const REMEMBER_REF: u32 = 1;
/// A remembered old collection object to rehash.
const REMEMBER_REHASH: u32 = 2;

impl Marker<'_> {
    /// Whether the cell at `off` is live: old, or marked.
    #[inline]
    fn is_marked(&self, off: u32) -> bool {
        off < self.old || self.arena.get(off, Header::GC) & gc_bits::MARK != 0
    }

    /// The cell a reference to `off` keeps alive: a flattened rope stands for its flat
    /// string (an old one stands for itself: it is not resolved, see `mark`).
    #[inline]
    fn resolve(&self, off: u32) -> u32 {
        let arena = &*self.arena;
        if off >= self.old
            && arena.get(off, Header::KIND) == CellKind::Rope as u8
            && arena.get(off, Header::FLAGS) & flags::ROPE_FLATTENED != 0
        {
            arena.get(off, Rope::LEFT).0
        } else {
            off
        }
    }

    /// Marks the cell a reference to `off` keeps alive and returns it: `off`, or the
    /// flat string of a flattened rope (the caller rewrites its slot, so the update pass
    /// needs no rope check).
    fn mark(&mut self, mut off: u32) -> u32 {
        if off < self.old {
            // Old: live, not marked, not moved (an old flattened rope stays, and keeps its
            // flat string through its own slot, which the old walk traces).
            return off;
        }
        // The whole header in one read: kind, gc bits, flags, size.
        let mut h: u64 = self.arena.read(off);
        if h as u8 == CellKind::Rope as u8 && (h >> 16) as u16 & flags::ROPE_FLATTENED != 0 {
            off = self.arena.get(off, Rope::LEFT).0;
            if off < self.old {
                return off;
            }
            h = self.arena.read(off);
        }
        let gc = (h >> 8) as u8;
        if gc & gc_bits::MARK != 0 {
            return off;
        }
        self.arena
            .set_untracked(off, Header::GC, gc | gc_bits::MARK);
        self.live.set(off, (h >> 32) as u32);
        let kind = h as u8;
        if kind == CellKind::EphemeronArray as u8 {
            // A pair lives only through its key (the fixpoint below).
            self.ephemerons += 1;
            return off;
        }
        if kind == CellKind::Slice as u8 && self.weak_slices {
            // Its parent waits for `resolve_slices`.
            self.slices += 1;
            return off;
        }
        if is_leaf(kind) {
            // No traced slots: nothing to push.
            let size = (h >> 32) as u32;
            if size >= super::gc::CHEAP_LEAF {
                self.cheap = self.cheap.saturating_add(size);
            }
            return off;
        }
        // The stack grows on demand (a refusal is only an overflow, handled by a rescan).
        if self.stack.len() == self.stack.capacity()
            && (self.stack.len() >= self.cap || self.stack.try_reserve(self.stack.len()).is_err())
        {
            self.overflow = true;
            self.overflow_from = self.overflow_from.min(off);
            return off;
        }
        // Room was checked above: a plain push, inline (`push_out` is a call per cell).
        self.stack.push(off);
        off
    }

    /// Marks the target of the traced slot `s`, rewriting a reference to a flattened
    /// rope to its flat string.
    #[inline]
    fn mark_slot(&mut self, slot: Slot) {
        match slot {
            Slot::Value(s) => {
                let v: Value = self.arena.read(s);
                if let Some(t) = v.ref_offset() {
                    let n = self.mark(t);
                    if n != t {
                        let bits = (v.to_bits() & !0xFFFF_FFFF) | n as u64;
                        self.arena.write_untracked(s, Value::from_raw(bits));
                    }
                }
            }
            Slot::Ref(s) => {
                if let Some(t) = self.arena.read::<Ref>(s).get() {
                    let n = self.mark(t);
                    if n != t {
                        self.arena.write_untracked(s, Ref(n));
                    }
                }
            }
        }
    }

    /// Marks the targets of `cell`'s traced slots.
    fn trace(&mut self, cell: u32) {
        let kb = self.arena.get(cell, Header::KIND);
        let kind = CellKind::from_u8(kb).unwrap_or_else(|| super::fail::bad_cell_kind(kb, cell));
        if kind == CellKind::EphemeronArray || (kind == CellKind::Slice && self.weak_slices) {
            return;
        }
        if kind == CellKind::FuncTemplate {
            // Offsets of shapes and prototypes may move (D1621). Emptied now, where the
            // inline cache cell still is: the update-and-slide walk reads no other cell.
            super::gc::clear_offset_caches(self.arena, cell);
        }
        let size = self.arena.get(cell, Header::SIZE);
        walk::for_each_slot(kind, cell, size, |slot| self.mark_slot(slot));
    }

    /// After the mark of a full collection: keeps each slice's parent that nothing else
    /// reached if its slices cover enough of it ([`SLICE_KEEP_SHARE`]); otherwise keeps it
    /// for this collection only and flags its slices (`SLICE_DETACH`) for a copy right
    /// after it. Returns the flagged slices.
    ///
    /// Two walks over the marked cells, no table. The first sums, in each such parent's
    /// `HASH` field, the units its slices cover (the cached hash is dropped; it is
    /// computed again when needed) and sets `gc_bits::COUNTED`. The second marks the
    /// parent at its first slice and leaves `COUNTED` set if its slices are to be copied
    /// (the update pass clears it with the mark).
    fn resolve_slices(&mut self) -> Vec<u32> {
        let mut detach: Vec<u32> = Vec::new();
        let room = detach.try_reserve_exact(self.slices as usize).is_ok();
        for pass in 0..2 {
            let mut at = self.live.next_live(self.old);
            while let Some(c) = at {
                let h: u64 = self.arena.read(c);
                at = self.live.next_live(c + (h >> 32) as u32);
                if h as u8 != CellKind::Slice as u8 {
                    continue;
                }
                let p = self.arena.get(c, Slice::PARENT).0;
                let gc = self.arena.get(p, Header::GC);
                let counted = gc & gc_bits::COUNTED != 0;
                let marked = self.is_marked(p);
                if pass == 0 {
                    if !marked {
                        let n = self.arena.get(c, Slice::LEN);
                        let sum = self.arena.get(p, Str8::HASH);
                        let sum = if counted { sum.saturating_add(n) } else { n };
                        self.arena.set(p, Header::GC, gc | gc_bits::COUNTED);
                        self.arena.set(p, Str8::HASH, sum);
                    }
                    continue;
                }
                if !counted {
                    // Reached otherwise, or kept for its slices.
                    continue;
                }
                if !marked {
                    // The parent's first slice: decide. This collection keeps the parent
                    // either way (its slices still read it).
                    let len = self.arena.get(p, Str8::LEN);
                    let covered = u64::from(self.arena.get(p, Str8::HASH));
                    self.arena.set(p, Str8::HASH, 0);
                    let copy = room
                        && len >= SLICE_KEEP_LEN
                        && covered * SLICE_KEEP_SHARE < u64::from(len);
                    if !copy {
                        self.arena.set(p, Header::GC, gc & !gc_bits::COUNTED);
                        self.mark(p);
                        continue;
                    }
                    self.mark(p);
                }
                let f = self.arena.get(c, Header::FLAGS);
                self.arena.set(c, Header::FLAGS, f | flags::SLICE_DETACH);
                detach.push_out(c);
            }
        }
        detach
    }

    fn drain(&mut self) {
        while let Some(c) = self.stack.pop() {
            self.trace(c);
        }
    }

    /// The old walk of a minor collection: every traced slot of every old cell is a root
    /// for the young cells it references (no write barrier: the walk reads them all). Old
    /// ephemeron tables are counted for the fixpoint instead, and old collections with
    /// young keys are flagged for a rehash.
    ///
    /// The slots it finds are remembered for the update pass, which then does not walk
    /// the old region again (unless there were more than the list holds).
    fn mark_old(&mut self) {
        let old = self.old;
        let mut c = 0;
        while c < old {
            let h: u64 = self.arena.read(c);
            let (kb, size) = (h as u8, (h >> 32) as u32);
            if kb == CellKind::EphemeronArray as u8 {
                self.old_ephemerons += 1;
            } else if !is_leaf(kb) {
                let kind =
                    CellKind::from_u8(kb).unwrap_or_else(|| super::fail::bad_cell_kind(kb, c));
                if kind == CellKind::FuncTemplate {
                    // Offsets of young shapes and prototypes may move (D1621). Emptied
                    // now: nothing reads them during the collection.
                    super::gc::clear_offset_caches(self.arena, c);
                } else if kind == CellKind::Object
                    && is_collection(self.arena, c)
                    && super::collection::has_young_keys(self.arena, c, old)
                {
                    let gc = (h >> 8) as u8;
                    self.arena.set(c, Header::GC, gc | gc_bits::REHASH);
                    self.remember(c | REMEMBER_REHASH);
                }
                self.scan_old(kb, c, size);
                self.drain();
            }
            c += size;
        }
    }

    /// Marks the young targets of the old cell `c` (kind byte `kb`, not a leaf), from its
    /// precomputed [`walk::SlotPlan`]: a slot costs a load and two compares unless it points
    /// at a young cell (rare), which `old_hit` handles out of line.
    #[inline]
    fn scan_old(&mut self, kb: u8, c: u32, size: u32) {
        let plan = &walk::PLANS[kb as usize];
        let old = self.old;
        for i in 0..plan.n_fixed as usize {
            let off = plan.fixed[i] as u32;
            if off >= size {
                // Optional fields a smaller cell does not have (they come last).
                break;
            }
            self.old_slot(c + off, plan.fixed_refs & (1 << i) != 0, old);
        }
        if plan.n_tail == 0 {
            return;
        }
        let (start, elem) = (c + plan.tail_offset as u32, plan.elem_size as u32);
        let end = c + size;
        if plan.n_tail == 1 && plan.tail[0] == 0 && plan.tail_refs == 0 && elem == 8 {
            // Arrays of values (ValueArray, PropValues, object slots): one word each.
            let mut s = start;
            while s < end {
                let v: u64 = self.arena.read(s);
                if v as u32 >= old && Value::from_raw(v).is_ref() {
                    self.old_hit(Slot::Value(s));
                }
                s += 8;
            }
            return;
        }
        let mut e = start;
        while e + elem <= end {
            for i in 0..plan.n_tail as usize {
                self.old_slot(e + plan.tail[i] as u32, plan.tail_refs & (1 << i) != 0, old);
            }
            e += elem;
        }
    }

    #[inline(always)]
    fn old_slot(&mut self, s: u32, is_ref: bool, old: u32) {
        if is_ref {
            let r: u32 = self.arena.read(s);
            if r >= old {
                self.old_hit(Slot::Ref(s));
            }
        } else {
            let v: u64 = self.arena.read(s);
            if v as u32 >= old && Value::from_raw(v).is_ref() {
                self.old_hit(Slot::Value(s));
            }
        }
    }

    /// An old slot with a young target: a root, remembered for the update pass.
    #[cold]
    #[inline(never)]
    fn old_hit(&mut self, slot: Slot) {
        self.mark_slot(slot);
        self.remember(match slot {
            Slot::Value(s) => s,
            Slot::Ref(s) => s | REMEMBER_REF,
        });
    }

    /// Adds an entry to the remembered list (a slot, or a collection to rehash); when it
    /// is full or cannot grow, the update pass walks the old region instead.
    #[inline]
    fn remember(&mut self, entry: u32) {
        if !self.remembered_ok {
            return;
        }
        if self.remembered.len() == self.remembered.capacity() {
            self.remembered_ok = false;
            return;
        }
        self.remembered.push_out(entry);
    }

    /// The first ephemeron table to resolve at or after `from`, in address order: old
    /// ones (by a walk over the old region, only if it has any), then marked young ones.
    fn next_ephemeron(&self, mut from: u32) -> Option<u32> {
        if from < self.old {
            if self.old_ephemerons > 0 {
                while from < self.old {
                    if self.arena.get(from, Header::KIND) == CellKind::EphemeronArray as u8 {
                        return Some(from);
                    }
                    from += self.arena.get(from, Header::SIZE);
                }
            }
            from = self.old;
        }
        let mut at = self.live.next_live(from);
        while let Some(c) = at {
            if self.arena.get(c, Header::KIND) == CellKind::EphemeronArray as u8 {
                return Some(c);
            }
            at = self.live.next_live(c + self.arena.get(c, Header::SIZE));
        }
        None
    }

    /// Marks everything reachable: the stack, overflow rescans and the ephemeron
    /// fixpoint, until nothing new is marked.
    fn mark_all(&mut self) {
        loop {
            self.drain();
            if self.overflow {
                self.overflow = false;
                // Cells below the first one left unpushed were all traced.
                let from = core::mem::replace(&mut self.overflow_from, u32::MAX);
                let mut at = self.live.next_live(from);
                while let Some(c) = at {
                    self.trace(c);
                    self.drain();
                    at = self.live.next_live(c + self.arena.get(c, Header::SIZE));
                }
                continue;
            }
            if self.ephemerons + self.old_ephemerons == 0 || !self.ephemeron_round() {
                break;
            }
        }
    }

    /// One pass over the marked ephemeron tables: marks the value of every pair whose
    /// key is marked. True if that marked anything.
    fn ephemeron_round(&mut self) -> bool {
        let mut progress = false;
        let mut at = self.next_ephemeron(0);
        while let Some(c) = at {
            let len = self.arena.get(c, EphemeronArray::LEN);
            for i in (0..len).step_by(2) {
                let k: Value = self.arena.read(EphemeronArray::VALUES.at(c, i));
                let Some(k) = k.ref_offset() else { continue };
                if !self.is_marked(k) {
                    continue;
                }
                let vs = EphemeronArray::VALUES.at(c, i + 1);
                let v: Value = self.arena.read(vs);
                if let Some(t) = v.ref_offset() {
                    let fresh = !self.is_marked(self.resolve(t));
                    self.mark_slot(Slot::Value(vs));
                    progress |= fresh;
                }
            }
            at = self.next_ephemeron(c + self.arena.get(c, Header::SIZE));
        }
        progress
    }

    /// Clears the pairs whose key died (HOLE key, undefined value), as Cheney does.
    fn clear_dead_pairs(&mut self) {
        let mut at = self.next_ephemeron(0);
        while let Some(c) = at {
            let len = self.arena.get(c, EphemeronArray::LEN);
            for i in (0..len).step_by(2) {
                let ks = EphemeronArray::VALUES.at(c, i);
                let k: Value = self.arena.read(ks);
                if k.ref_offset().is_some_and(|k| !self.is_marked(k)) {
                    self.arena.write(ks, Value::HOLE);
                    self.arena.write(ks + 8, Value::UNDEFINED);
                }
            }
            at = self.next_ephemeron(c + self.arena.get(c, Header::SIZE));
        }
    }
}

/// Kinds without traced slots.
#[inline]
fn is_leaf(kind: u8) -> bool {
    matches!(
        CellKind::from_u8(kind),
        Some(
            CellKind::Str8
                | CellKind::Str16
                | CellKind::Bytes
                | CellKind::U32Array
                | CellKind::BigInt
        )
    )
}

/// Whether the object cell `c` is a Map, Set, WeakMap or WeakSet (offset-hashed keys).
fn is_collection(arena: &Arena, c: u32) -> bool {
    ObjectClass::from_u32(arena.get(c, Object::CLASS)).is_some_and(|k| {
        matches!(
            k,
            ObjectClass::Map | ObjectClass::Set | ObjectClass::WeakMap | ObjectClass::WeakSet
        )
    })
}

/// The live-word bitmap of the cells from `base` (the old region's end) on: one bit per
/// 8-byte word.
struct Live {
    base: u32,
    bits: Vec<u64>,
}

impl Live {
    /// Marks the `size` bytes of the cell at `off`.
    fn set(&mut self, off: u32, size: u32) {
        set_bits(&mut self.bits, (off - self.base) / 8, size / 8);
    }

    /// The first live word at or after `from` (`>= base`): the next marked cell when
    /// `from` is a cell boundary.
    fn next_live(&self, from: u32) -> Option<u32> {
        next_live(&self.bits, from - self.base).map(|r| r + self.base)
    }

    /// The first dead word at or after `from` (`u32::MAX` if none).
    fn next_dead(&self, from: u32) -> u32 {
        match next_dead(&self.bits, from - self.base) {
            u32::MAX => u32::MAX,
            r => r + self.base,
        }
    }

    /// The new address of the live cell at `a`, from the block table.
    #[inline]
    fn forward(&self, table: &[u32], a: u32) -> u32 {
        let r = a - self.base;
        let b = (r / BLOCK) as usize;
        let below = self.bits[b] & ((1u64 << ((r / 8) % 64)) - 1);
        table[b] + 8 * below.count_ones()
    }
}

/// Sets `n` bits from bit `first`.
fn set_bits(bits: &mut [u64], first: u32, n: u32) {
    let (mut i, end) = (first as usize, (first + n) as usize);
    while i < end {
        let w = i / 64;
        let lo = i % 64;
        let hi = (end - w * 64).min(64);
        let mask = if hi - lo == 64 {
            u64::MAX
        } else {
            ((1u64 << (hi - lo)) - 1) << lo
        };
        bits[w] |= mask;
        i = w * 64 + hi;
    }
}

/// The offset of the first live word at or after `from`: the next marked cell when
/// `from` is a cell boundary.
fn next_live(bits: &[u64], from: u32) -> Option<u32> {
    let i = (from / 8) as usize;
    let mut w = i / 64;
    if w >= bits.len() {
        return None;
    }
    let mut word = bits[w] & (u64::MAX << (i % 64));
    loop {
        if word != 0 {
            return Some(((w * 64) as u32 + word.trailing_zeros()) * 8);
        }
        w += 1;
        if w >= bits.len() {
            return None;
        }
        word = bits[w];
    }
}

/// The offset of the first dead word at or after `from` (`u32::MAX` if none).
fn next_dead(bits: &[u64], from: u32) -> u32 {
    let i = (from / 8) as usize;
    let mut w = i / 64;
    if w >= bits.len() {
        return u32::MAX;
    }
    let mut word = !bits[w] & (u64::MAX << (i % 64));
    loop {
        if word != 0 {
            return ((w * 64) as u32 + word.trailing_zeros()) * 8;
        }
        w += 1;
        if w >= bits.len() {
            return u32::MAX;
        }
        word = !bits[w];
    }
}

impl Heap {
    /// The in-place collection (see the module docs and [`Heap::collect`]) of the cells
    /// from `old` on: 0 for a full collection, the old region's end for a minor one, where
    /// the cells below `old` count as live and stay where they are.
    ///
    /// Returns the arena sizes before and after, and where the old region ends now: the
    /// live bytes below its old end (in a minor collection, its end).
    pub(super) fn mark_compact(&mut self, old: u32, full: bool) -> Result<Compacted, HeapError> {
        let before = self.arena.len();
        let old_end = self.arena.get(0, super::cells::Roots::OLD_END);
        let base_end = self.arena.get(0, super::cells::Roots::BASE_END);
        debug_assert!(old <= before);
        let blocks = (before - old).div_ceil(BLOCK) as usize;
        let mut live: Vec<u64> = Vec::new();
        let mut table: Vec<u32> = Vec::new();
        let mut stack: Vec<u32> = Vec::new();
        let mut remembered: Vec<u32> = Vec::new();
        live.try_reserve_exact(blocks)
            .and_then(|()| table.try_reserve_exact(blocks))
            .and_then(|()| match old {
                0 => Ok(()),
                _ => remembered.try_reserve_exact(remembered_cap()),
            })
            .and_then(|()| stack.try_reserve_exact(STACK_RESERVE.min(stack_cap(before))))
            .map_err(|_| HeapError::HostOutOfMemory)?;
        live.resize(blocks, 0);
        // 1. Mark: from the roots cell, or (minor) from every old cell.
        let mut m = Marker {
            arena: &mut self.arena,
            old,
            live: Live {
                base: old,
                bits: live,
            },
            stack,
            overflow: false,
            overflow_from: u32::MAX,
            cap: stack_cap(before),
            ephemerons: 0,
            old_ephemerons: 0,
            weak_slices: full,
            slices: 0,
            remembered,
            remembered_ok: true,
            cheap: 0,
        };
        if old == 0 {
            m.mark(0);
        } else {
            m.mark_old();
        }
        m.mark_all();
        if m.ephemerons + m.old_ephemerons > 0 {
            m.clear_dead_pairs();
        }
        // (Strings are not ephemeron keys, so slices are resolved after the fixpoint.)
        let mut detach = if m.slices > 0 {
            m.resolve_slices()
        } else {
            Vec::new()
        };
        let (live, remembered, remembered_ok, old_ephemerons, cheap) = (
            m.live,
            m.remembered,
            m.remembered_ok,
            m.old_ephemerons,
            m.cheap,
        );
        // Reused to remember the Map/Set objects (new offsets) for the rehash.
        let mut collections = m.stack;
        let mut all_collections = true;
        // 2. Block table.
        let mut sum = old;
        for &w in &live.bits {
            table.push_out(sum);
            sum += 8 * w.count_ones();
        }
        let after = sum;
        for d in &mut detach {
            *d = live.forward(&table, *d);
        }
        // The live bytes below the old region's end (a cell boundary, maybe of a dead
        // cell): its end after the slide.
        let slid = |b: u32| {
            if b >= before {
                after
            } else {
                live.forward(&table, b.max(old))
            }
        };
        let old_end = slid(old_end);
        // The prefix's end moves with what is live below it, until 0147 makes the prefix
        // immutable; a minor collection (`old` ≥ it) leaves it alone.
        let base_end = if base_end <= old {
            base_end
        } else {
            slid(base_end)
        };
        // Nothing below the first dead word moves (usually the base image's cells).
        let dense = live.next_dead(old).min(before);
        // 3. Update the old region's references to young cells.
        let arena = &mut self.arena;
        let update_slot = |arena: &mut Arena, slot: Slot| match slot {
            Slot::Value(s) => {
                let v: Value = arena.read(s);
                if let Some(t) = v.ref_offset()
                    && t >= dense
                {
                    let n = live.forward(&table, t);
                    arena.write_untracked(
                        s,
                        Value::from_raw((v.to_bits() & !0xFFFF_FFFF) | n as u64),
                    );
                }
            }
            Slot::Ref(s) => {
                if let Some(t) = arena.read::<Ref>(s).get()
                    && t >= dense
                {
                    arena.write_untracked(s, Ref(live.forward(&table, t)));
                }
            }
        };
        let update = |arena: &mut Arena, kind: CellKind, c: u32, size: u32| {
            walk::for_each_slot(kind, c, size, |slot| update_slot(arena, slot));
        };
        // The old region's references to young cells: the remembered slots and the old
        // ephemeron tables (not traced, so not remembered), or a walk over all of it.
        let mut remember_collection = |arena: &mut Arena, c: u32| {
            let gc = arena.get(c, Header::GC);
            arena.set(c, Header::GC, gc & !gc_bits::REHASH);
            if collections.len() < collections.capacity() {
                collections.push_out(c);
            } else {
                all_collections = false;
            }
        };
        if remembered_ok {
            for &e in &remembered {
                let s = e & !(REMEMBER_REF | REMEMBER_REHASH);
                match e & (REMEMBER_REF | REMEMBER_REHASH) {
                    0 => update_slot(arena, Slot::Value(s)),
                    REMEMBER_REF => update_slot(arena, Slot::Ref(s)),
                    _ => remember_collection(arena, s),
                }
            }
            let mut c = 0;
            while old_ephemerons > 0 && c < old {
                let h: u64 = arena.read(c);
                if h as u8 == CellKind::EphemeronArray as u8 {
                    update(arena, CellKind::EphemeronArray, c, (h >> 32) as u32);
                }
                c += (h >> 32) as u32;
            }
        } else {
            let mut c = 0;
            while c < old {
                let h: u64 = arena.read(c);
                let (kb, size) = (h as u8, (h >> 32) as u32);
                if !is_leaf(kb) {
                    let kind =
                        CellKind::from_u8(kb).unwrap_or_else(|| super::fail::bad_cell_kind(kb, c));
                    if (h >> 8) as u8 & gc_bits::REHASH != 0 {
                        remember_collection(arena, c);
                    }
                    update(arena, kind, c, size);
                }
                c += size;
            }
        }
        // 4. Update the marked cells' slots and slide them down, in one walk: a run of
        // adjacent live cells at a time, which all move by the same distance (never onto
        // a cell not yet updated). Forwarding needs only the bitmap and the block table,
        // so each cell is touched once (0224: a quarter fewer cycles than an update pass
        // and then a slide). Nothing here reads another cell: the templates' inline
        // caches were emptied by the mark.
        let mut at = live.next_live(old);
        while let Some(run) = at {
            let end = live.next_dead(run).min(before);
            let mut c = run;
            while c < end {
                // The whole header in one read: kind, gc bits, flags, size.
                let h: u64 = arena.read(c);
                let (kb, size) = (h as u8, (h >> 32) as u32);
                // The mark is not needed past this point (the bitmap has it).
                let gc = (h >> 8) as u8;
                arena.set_untracked(c, Header::GC, gc & !(gc_bits::MARK | gc_bits::COUNTED));
                if !is_leaf(kb) {
                    let kind =
                        CellKind::from_u8(kb).unwrap_or_else(|| super::fail::bad_cell_kind(kb, c));
                    if kind == CellKind::Object && is_collection(arena, c) {
                        if collections.len() < collections.capacity() {
                            collections.push_out(live.forward(&table, c));
                        } else {
                            all_collections = false;
                        }
                    }
                    // Marking rewrote references to flattened ropes: every target is
                    // marked.
                    update(arena, kind, c, size);
                }
                c += size;
            }
            let to = live.forward(&table, run);
            if to != run {
                arena.copy_within(run, end - run, to);
            }
            at = live.next_live(end);
        }
        arena.truncate(after);
        // Map/Set indexes hash object keys by offset (D1381): the ones remembered, or (if
        // there were more than the stack holds) a walk over the whole heap.
        if all_collections {
            for &c in &collections {
                self.collection_rehash(Obj(c));
            }
        } else {
            let mut c = 0;
            while c < after {
                let size = self.arena.get(c, Header::SIZE);
                if self.arena.get(c, Header::KIND) == CellKind::Object as u8
                    && is_collection(&self.arena, c)
                {
                    self.collection_rehash(Obj(c));
                }
                c += size;
            }
        }
        self.arena.set(0, super::cells::Roots::BASE_END, base_end);
        Ok(Compacted {
            before,
            after,
            old_end,
            detach,
            cheap,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The slots a [`walk::SlotPlan`] visits, in the order `scan_old` visits them.
    fn plan_slots(kb: u8, c: u32, size: u32) -> Vec<Slot> {
        let p = &walk::PLANS[kb as usize];
        let mut out = Vec::new();
        let slot = |s: u32, r: bool| if r { Slot::Ref(s) } else { Slot::Value(s) };
        for i in 0..p.n_fixed as usize {
            if p.fixed[i] as u32 >= size {
                break;
            }
            out.push_out(slot(c + p.fixed[i] as u32, p.fixed_refs & (1 << i) != 0));
        }
        if p.n_tail > 0 {
            let mut e = c + p.tail_offset as u32;
            while e + p.elem_size as u32 <= c + size {
                for i in 0..p.n_tail as usize {
                    out.push_out(slot(e + p.tail[i] as u32, p.tail_refs & (1 << i) != 0));
                }
                e += p.elem_size as u32;
            }
        }
        out
    }

    #[test]
    fn slot_plans_match_the_layouts() {
        // Every kind, every cell size up to a few tail elements past the minimum: the
        // old-region walk visits exactly the slots the generic walker does.
        for l in super::super::cells::LAYOUTS {
            let kind = CellKind::from_u8(l.kind).unwrap();
            let step = l.tail.map_or(8, |t| t.elem.size.max(8));
            let mut size = l.min_size;
            for _ in 0..6 {
                let mut want = Vec::new();
                walk::for_each_slot_by_layout(kind, 64, size, |s| want.push_out(s));
                assert_eq!(plan_slots(l.kind, 64, size), want, "{} size {size}", l.name);
                let mut got = Vec::new();
                walk::for_each_slot(kind, 64, size, |s| got.push_out(s));
                assert_eq!(got, want, "for_each_slot: {} size {size}", l.name);
                size += step;
            }
        }
    }
}
