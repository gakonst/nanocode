//! The collection entry points and the copying collector (Cheney).
//!
//! The VM's own collections are in place (`compactor.rs`): [`Heap::collect`] (full),
//! [`Heap::collect_minor`] (the old region stays, D2041), [`Heap::collect_auto`] (the
//! schedule between the two) and [`Heap::seal`].
//!
//! [`Heap::collect_canonical`] copies everything reachable from the [`Roots`] cell into a
//! fresh arena and drops the old one:
//!
//! 1. Reserve the to-space: as many bytes as the from-space holds, which bounds the live
//!    data. If the host cannot provide them, the collection fails with
//!    [`HeapError::OutOfMemory`] before anything was touched. With the space reserved,
//!    nothing during the copy can fail.
//! 2. Copy the roots cell (offset 0 → offset 0).
//! 3. Scan the to-space cell by cell. For every traced slot (from the layout table, see
//!    [`walk::for_each_slot`]) that references a from-space cell, *forward* it: if the cell
//!    has already been copied, take its new offset, else copy it to the end of the to-space.
//!    Write the new offset back into the slot. The scan ends when it catches up with the
//!    allocation point.
//!
//! Forwarding sets [`gc_bits::FORWARDED`] in the old header and stores the new offset in
//! the old cell's first payload word (every cell has one; `cells.rs` asserts it). The
//! from-space is never scanned, so overwriting it is fine.
//!
//! References to a flattened rope are short-circuited to its flat string, so flattened
//! ropes do not survive a collection. Unflattened ropes are copied as they are; flattening
//! them here would allocate and could not be bounded by the reservation.
//!
//! The resulting layout is breadth-first from the roots, in slot order. That makes it a
//! function of the object graph alone: collecting an already collected heap reproduces it
//! byte for byte, which keeps snapshots deterministic.
//!
//! Almost nothing in the heap is keyed by arena offsets: the atom index is keyed by string
//! content hashes, property indexes by atom ids, string hashes are content based. The
//! exception is the index of Map/Set/WeakMap/WeakSet (object and symbol keys hash by
//! offset): `collect` rehashes every collection it copied, in place (D1381).
//!
//! Ephemerons (D1382): an [`EphemeronArray`] (WeakMap/WeakSet entries) is not traced by
//! the scan. After the scan catches up, every pair whose key has been copied gets its
//! key and value forwarded, and the scan resumes; this repeats until nothing changes.
//! Pairs whose keys were never copied are then cleared (HOLE key, undefined value). The
//! order (tables in copy order, pairs in index order) depends only on the object graph,
//! so the layout stays deterministic.
//!
//! [`Roots`]: super::cells::Roots
use crate::PushOut;

use super::arena::{self, Arena};
use alloc::vec::Vec;

use super::cells::{EphemeronArray, Header, Object, Roots, Rope, Slice, Str8, flags, gc_bits};
use super::layout::Ref;
use super::object::ObjectClass;
use super::walk::{self, Slot};
use super::{CellKind, Heap, HeapError};
use crate::value::Value;

/// The smallest growth between collections.
pub const MIN_GROWTH: u32 = 256 * 1024;

pub const LIVE_GROWTH: u32 = 2;

/// Every this many collections, [`Heap::collect_auto`] makes a full one (D2041): it
/// bounds how long dead old cells are retained.
pub const FULL_EVERY: u32 = 16;

/// Where the forwarding offset goes in a copied cell: the first payload word.
const FORWARD: u32 = 8;

/// What a collection did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GcStats {
    /// Arena size before.
    pub before: u32,
    /// Arena size after: the live data.
    pub after: u32,
}

/// What a builtin prefix counts for in [`next_threshold`]'s growth, whatever its size
/// (D2442): the prefix is immutable and never collected, so its size should not set how
/// often the session's data is collected. The value is the prefix of the base image
/// when the rule came in (`Roots::BASE_END` 288,160 B), so collections came as often as
/// before; retune it on its own.
pub const BASE_ALLOWANCE: u32 = 288_160;

/// The arena size at which the next collection is requested, for `live` bytes after a
/// collection, `base` of them the builtin prefix (`Roots::BASE_END`, 0 without one):
/// growth of [`LIVE_GROWTH`] times the live data (at least [`MIN_GROWTH`]), the prefix
/// counted as [`BASE_ALLOWANCE`], but never more than half of the remaining headroom
/// below `limit`. Close to the cap, collections get more frequent instead of running
/// into `OutOfMemory`.
pub fn next_threshold(live: u32, base: u32, limit: u32) -> u32 {
    next_threshold_cheap(live, base, limit, 0)
}

/// Leaf cells (strings, byte buffers) at least this large count as cheap for
/// [`next_threshold_cheap`] (D3144).
pub const CHEAP_LEAF: u32 = 16 << 10;

/// The share of the cheap bytes [`next_threshold_cheap`] does not count, in percent.
pub const CHEAP_DISCOUNT: u32 = 60;

/// [`next_threshold`] after a collection that found `cheap` live bytes in large leaf
/// cells above the old region (D3144): those cost a collection next to nothing (a mark
/// bit and their bitmap words, no fields to trace), so most of them do not count toward
/// the growth that pays for the next collection. A run that holds a few MB of tool output
/// as strings would otherwise let its heap grow by that much again before collecting.
pub fn next_threshold_cheap(live: u32, base: u32, limit: u32, cheap: u32) -> u32 {
    let headroom = limit.saturating_sub(live);
    let discount = (u64::from(cheap) * u64::from(CHEAP_DISCOUNT) / 100) as u32;
    let counted = if base > 0 {
        live.saturating_sub(base).saturating_sub(discount) + BASE_ALLOWANCE
    } else {
        live.saturating_sub(discount)
    };
    let growth = counted.saturating_mul(LIVE_GROWTH).max(MIN_GROWTH);
    if growth > headroom / 2 {
        return live + headroom / 2;
    }
    // Whole 64 KiB wasm pages when that stays within the headroom rule (D1870), counted
    // from the prefix's end (D2544): rounded from offset 0, a prefix a few KB smaller or
    // larger moved every collection point.
    let base = u64::from(base.min(live));
    let t = base + (u64::from(live) - base + u64::from(growth)).next_multiple_of(PAGE);
    if t <= live as u64 + headroom as u64 / 2 {
        t as u32
    } else {
        live + growth
    }
}

/// A wasm page.
const PAGE: u64 = 64 * 1024;

/// Forwards one traced slot of a to-space cell, rewriting it with the new offset.
fn trace(from: &mut Arena, to: &mut Arena, base: u32, slot: Slot) {
    match slot {
        Slot::Value(s) => {
            let v: Value = to.read(s);
            if let Some(t) = v.ref_offset() {
                let n = forward(from, to, t, base);
                let bits = (v.to_bits() & !0xFFFF_FFFF) | n as u64;
                to.write(s, Value::from_raw(bits));
            }
        }
        Slot::Ref(s) => {
            let r: Ref = to.read(s);
            if let Some(t) = r.get() {
                let n = forward(from, to, t, base);
                to.write(s, Ref(n));
            }
        }
    }
}

/// Empties the inline cache slots of a (copied) template that hold arena offsets
/// (prototype property slots, D1621): the offsets are stale now. Everything else in a
/// slot is offset-free. Clearing (rather than bumping an epoch) keeps a collection of a
/// collected heap byte-identical (D1090).
pub(super) fn clear_offset_caches(to: &mut Arena, template: u32) {
    use super::cells::{FuncTemplate, IC_OFFSET_SLOT, IC_WORDS, U32Array};
    let Some(ic) = to.get::<Ref>(template, FuncTemplate::IC).get() else {
        return;
    };
    let n = to.get(template, FuncTemplate::IC_COUNT) as u32;
    for i in 0..n {
        let at = U32Array::DATA.at(ic, i * IC_WORDS);
        if to.read::<u32>(at) & IC_OFFSET_SLOT != 0 {
            for k in 0..IC_WORDS {
                to.write::<u32>(at + 4 * k, 0);
            }
        }
    }
}

/// Whether the from-space cell at `off` has been copied (is live).
fn is_forwarded(from: &Arena, off: u32) -> bool {
    from.get(off, Header::GC) & gc_bits::FORWARDED != 0
}

/// Returns the new offset of the from-space cell at `off`, copying it if needed.
fn forward(from: &mut Arena, to: &mut Arena, mut off: u32, base: u32) -> u32 {
    // The builtin prefix stays where it is (copied before the scan).
    if off < base {
        return off;
    }
    if from.get(off, Header::KIND) == CellKind::Rope as u8
        && from.get(off, Header::FLAGS) & flags::ROPE_FLATTENED != 0
    {
        off = from.get(off, Rope::LEFT).0;
        if off < base {
            return off;
        }
    }
    if from.get(off, Header::GC) & gc_bits::FORWARDED != 0 {
        return from.read::<u32>(off + FORWARD);
    }
    // A slice comes out as the flat string it reads (D2542): the image, and so the
    // canonical hash, does not depend on which strings are slices.
    let slice = from.get(off, Header::KIND) == CellKind::Slice as u8;
    let (size, at, src, n) = if slice {
        let len = from.get(off, Slice::LEN);
        let parent = from.get(off, Slice::PARENT).0;
        let size = Str8::DATA
            .cell_size(len)
            .expect("a slice fits a flat string");
        let data = parent + Str8::DATA.offset + from.get(off, Slice::OFF);
        (size, Str8::DATA.offset, data, len)
    } else {
        let size = from.get(off, Header::SIZE);
        (size, 0, off, size)
    };
    let new = to
        .alloc_init(size, at, from.slice(src, n))
        .expect("the to-space has room for the whole from-space");
    if slice {
        to.set(new, Header::KIND, CellKind::Str8 as u8);
        to.set(new, Header::SIZE, size);
        to.set(new, Str8::LEN, n);
        to.set(new, Str8::HASH, from.get(off, Slice::HASH));
    }
    from.set(off, Header::GC, gc_bits::FORWARDED);
    from.write::<u32>(off + FORWARD, new);
    new
}

impl Heap {
    /// Whether allocation has crossed the GC threshold. The owner should call
    /// [`Heap::collect`] at its next safepoint.
    #[inline]
    pub fn gc_pending(&self) -> bool {
        self.gc_pending
    }

    /// The arena size at which `gc_pending` will be set.
    pub fn gc_threshold(&self) -> u32 {
        self.gc_threshold
    }

    /// Collects garbage in place (mark-compact, `compactor.rs`, 0101): a full collection
    /// of the whole heap. Must only be called at a safepoint: every `Obj`, `Str`, `Sym`
    /// and `Value` held outside the heap is invalid afterwards; keep [`Handle`]s instead.
    ///
    /// The surviving cells keep their address order; the old region (`Roots::OLD_END`,
    /// D2041) shrinks to its live cells. The builtin prefix below `Roots::BASE_END`
    /// (D022, D2064) is never moved or freed: its cells are walked like an old region's,
    /// and everything above it is collected. On `Err(HostOutOfMemory)` (the host could
    /// not provide the side tables) the heap is unchanged.
    ///
    /// [`Handle`]: super::Handle
    pub fn collect(&mut self) -> Result<GcStats, HeapError> {
        #[cfg(test)]
        if core::mem::take(&mut self.fail_next_gc_reserve) {
            return Err(HeapError::HostOutOfMemory);
        }
        let base = self.base_end();
        let c = self.without_barrier(|h| h.mark_compact(base, true))?;
        let (before, mut after) = (c.before, c.after);
        self.set_old_region(c.old_end, 0);
        self.after_collect(after, c.cheap);
        let detach = c.detach;
        if detach.is_empty() {
            return Ok(GcStats { before, after });
        }
        // Slices whose parent only they kept get their own units now (D2542), and a second
        // pass drops the parents: a snapshot right after a collection should not carry
        // them. Without room a slice stays as it is.
        self.detach_slices(detach);
        if let Ok(c) = self.without_barrier(|h| h.mark_compact(base, true)) {
            after = c.after;
            self.set_old_region(c.old_end, 0);
            self.after_collect(after, c.cheap);
        }
        Ok(GcStats { before, after })
    }

    /// A full collection that makes everything live old (D2041): later minor
    /// collections neither trace nor move it. For the end of a setup phase whose
    /// state lives on, such as pi's prelude: everything after it is young.
    pub fn seal(&mut self) -> Result<GcStats, HeapError> {
        let stats = self.collect()?;
        self.set_old_region(stats.after, 0);
        Ok(stats)
    }

    /// The collection the heap's schedule asks for (the interpreter's safepoints): minor,
    /// and full every [`FULL_EVERY`] collections or after a minor one that left less
    /// headroom than live data. The schedule is in the image (`Roots::MINORS`), so
    /// whether a collection is minor or full is a function of the image alone (D1601).
    /// Without an old region every collection is full.
    pub fn collect_auto(&mut self) -> Result<GcStats, HeapError> {
        let old = self.old_end();
        if old == 0 || self.arena.get(0, Roots::MINORS) + 1 >= FULL_EVERY {
            return self.collect();
        }
        let stats = self.collect_minor()?;
        if stats.after > self.arena.limit() / 2 {
            // Close to the cap, dead old cells matter: the next collection is full.
            self.arena.set(0, Roots::MINORS, FULL_EVERY - 1);
        }
        Ok(stats)
    }

    /// A minor collection: the cells in the old region `[0, OLD_END)` are live and stay
    /// where they are, the young ones are marked from every reference in the old region
    /// and compacted down to its end. Dead old cells, and young cells only they reach,
    /// survive until the next full collection.
    pub fn collect_minor(&mut self) -> Result<GcStats, HeapError> {
        #[cfg(test)]
        if core::mem::take(&mut self.fail_next_gc_reserve) {
            return Err(HeapError::HostOutOfMemory);
        }
        let old = self.old_end();
        let c = self.without_barrier(|h| h.mark_compact(old, false))?;
        let minors = self.arena.get(0, Roots::MINORS);
        self.set_old_region(old, minors + 1);
        self.after_collect(c.after, c.cheap);
        Ok(GcStats {
            before: c.before,
            after: c.after,
        })
    }

    /// Gives each flagged slice (`SLICE_DETACH`) its own units (D2542): it becomes a
    /// flattened rope, and its parent dies. Without room a slice stays as it is.
    fn detach_slices(&mut self, detach: Vec<u32>) {
        for s in detach {
            let s = crate::value::Str(s);
            if self.flatten(s).is_err() {
                let f = self.cell_flags(s.0);
                self.set_cell_flags(s.0, f & !flags::SLICE_DETACH);
            }
        }
    }

    /// The end of the builtin prefix (`Roots::BASE_END`, D022): 0 without one.
    pub fn base_end(&self) -> u32 {
        // (0 too while the arena has no roots cell yet: a heap being set up.)
        if self.arena.len() < Roots::BASE_END.offset + 4 {
            return 0;
        }
        self.arena.get(0, Roots::BASE_END)
    }

    /// The end of the old region (`Roots::OLD_END`).
    pub fn old_end(&self) -> u32 {
        self.arena.get(0, Roots::OLD_END)
    }

    fn set_old_region(&mut self, end: u32, minors: u32) {
        self.arena.set(0, Roots::OLD_END, end);
        self.arena.set(0, Roots::MINORS, minors);
    }

    /// The threshold, the pending flag, the reserve and the capacity after a collection.
    fn after_collect(&mut self, after: u32, cheap: u32) {
        self.mega_ic.fill(0);
        self.iterator_ic = Default::default();
        self.return_ic = Default::default();
        self.regexp_shape = 0;
        self.rest_shape = 0;
        self.atom_memo.fill([0; 2]);
        self.refresh_frozen();
        let t = next_threshold_cheap(after, self.base_end(), self.arena.limit(), cheap);
        self.set_gc_threshold(t);
        self.gc_pending = false;
        self.maybe_close_reserve();
        // A to-space sized for a mostly-garbage heap gives the excess back (in wasm to the
        // allocator, for the Rust side); close to the threshold it is kept, so the arena
        // does not regrow (and copy) right away.
        let t = self.gc_threshold as usize;
        if self.arena.capacity() > 2 * t {
            self.arena.shrink_capacity(t);
        }
    }

    /// Collects garbage by copying (Cheney): the heap comes out breadth-first from the
    /// roots, a function of the object graph alone (D1090). For canonical images: the
    /// canonical hash (D1730, on a copy) and the base image. Needs a to-space as big as
    /// the arena; on `Err(HostOutOfMemory)` the heap is unchanged.
    pub fn collect_canonical(&mut self) -> Result<GcStats, HeapError> {
        let before = self.arena.len();
        #[cfg(test)]
        if core::mem::take(&mut self.fail_next_gc_reserve) {
            return Err(HeapError::HostOutOfMemory);
        }
        // Uncapped: slices come out as flat strings, so the copy can outgrow the heap.
        let limit = self.arena.limit();
        let mut to = Arena::with_capacity(before as usize, arena::MAX_ARENA as usize)?;
        let base = self.base_end();
        let from = &mut self.arena;
        // The builtin prefix (D2064) is copied as it is, at the same offsets: only the
        // cells above it are laid out breadth-first. Without one, the roots cell starts
        // the copy.
        if base > 0 {
            to.alloc_init(base, 0, from.slice(0, base))
                .expect("to-space is reserved for the whole from-space");
        } else {
            let roots = forward(from, &mut to, 0, 0);
            debug_assert_eq!(roots, 0);
        }
        // Ephemeron tables (WeakMap/WeakSet entries) in to-space, with a flag per pair:
        // done once its key turned out live and the pair was forwarded.
        let mut ephemerons: Vec<(u32, Vec<bool>)> = Vec::new();
        // Collections to rehash afterwards (their object keys hash by offset).
        let mut collections: Vec<u32> = Vec::new();
        let mut scan = 0;
        loop {
            while scan < to.len() {
                let kb = to.get(scan, Header::KIND);
                let kind =
                    CellKind::from_u8(kb).unwrap_or_else(|| super::fail::bad_cell_kind(kb, scan));
                let size = to.get(scan, Header::SIZE);
                if kind == CellKind::EphemeronArray {
                    // Not traced: a pair lives only through its key (below).
                    let pairs = to.get(scan, EphemeronArray::LEN) / 2;
                    ephemerons.push_out((scan, alloc::vec![false; pairs as usize]));
                    scan += size;
                    continue;
                }
                if kind == CellKind::Object
                    && ObjectClass::from_u32(to.get(scan, Object::CLASS)).is_some_and(|c| {
                        matches!(
                            c,
                            ObjectClass::Map
                                | ObjectClass::Set
                                | ObjectClass::WeakMap
                                | ObjectClass::WeakSet
                        )
                    })
                {
                    collections.push_out(scan);
                }
                walk::for_each_slot(kind, scan, size, |slot| trace(from, &mut to, base, slot));
                if kind == CellKind::FuncTemplate {
                    clear_offset_caches(&mut to, scan);
                }
                scan += size;
            }
            // Ephemerons to a fixpoint: a pair whose key was copied keeps its value; that
            // may copy more cells (and make more keys live), so scan again.
            let mut progress = false;
            for (table, done) in &mut ephemerons {
                for (i, d) in done.iter_mut().enumerate() {
                    if *d {
                        continue;
                    }
                    let ks = EphemeronArray::VALUES.at(*table, 2 * i as u32);
                    let k: Value = to.read(ks);
                    let Some(t) = k.ref_offset() else {
                        // A cleared pair (HOLE): nothing to keep.
                        *d = true;
                        continue;
                    };
                    if t >= base && !is_forwarded(from, t) {
                        continue;
                    }
                    trace(from, &mut to, base, Slot::Value(ks));
                    trace(from, &mut to, base, Slot::Value(ks + 8));
                    *d = true;
                    progress = true;
                }
            }
            if !progress && scan == to.len() {
                break;
            }
        }
        // Pairs whose key died are cleared.
        for (table, done) in &ephemerons {
            for (i, d) in done.iter().enumerate() {
                if !d {
                    let ks = EphemeronArray::VALUES.at(*table, 2 * i as u32);
                    to.write(ks, Value::HOLE);
                    to.write(ks + 8, Value::UNDEFINED);
                }
            }
        }
        let after = to.len();
        // The old from-space goes back to the allocator. Keeping it as the next to-space
        // was measured and rejected (D1870): in wasm a freed from-space also serves the
        // Rust allocations between collections, and pinning it raised peak memory.
        // The barrier stays armed: the prefix was copied as it is (checked below).
        to.set_trap_end(self.arena.trap_end());
        to.set_limit(limit);
        self.arena = to;
        // Canonical images are sealed (the base image's old region is all of it), and
        // their bytes do not depend on where the old region ended (D1730). The first
        // collection after one is full (D2045): a VM made from the base kills some of its
        // cells right away (the atom table grows with the first compiled script), and
        // until a full collection drops them they shift every later threshold.
        self.set_old_region(after, FULL_EVERY - 1);
        // The prefix is where it was (a heap without one gets one from
        // `prepare_base_image`).
        self.arena.set(0, Roots::BASE_END, base);
        self.after_collect(after, 0);
        for o in collections {
            self.collection_rehash(crate::value::Obj(o));
        }
        self.check_prefix();
        Ok(GcStats { before, after })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heap::arena;

    #[test]
    fn thresholds() {
        let big = arena::MAX_ARENA;
        assert_eq!(next_threshold(0, 0, big), MIN_GROWTH);
        assert_eq!(next_threshold(1 << 20, 0, big), 2 << 20);
        // Rounded up to whole 64 KiB pages (D1870).
        assert_eq!(next_threshold(300_000, 0, big), 655_360);
        // Near the cap: half the headroom.
        assert_eq!(next_threshold(900, 0, 1000), 950);
        assert_eq!(next_threshold(1000, 0, 1000), 1000);
        assert_eq!(next_threshold(2000, 0, 1000), 2000);
        // A prefix counts as BASE_ALLOWANCE (D2442): at that size the growth is as before
        // (up to where the rounding starts); a smaller one leaves the growth as it was.
        let a = BASE_ALLOWANCE;
        assert!(
            next_threshold(a + 100_000, a, big).abs_diff(next_threshold(a + 100_000, 0, big))
                < PAGE as u32
        );
        // The rounding counts from the prefix's end (D2544): a prefix of another size moves
        // the threshold by as much, so collections come after the same allocations.
        for young in [0, 1, 70_000, 300_000, 5 << 20] {
            let t = |b: u32| next_threshold(b + young, b, big) - b;
            assert_eq!(t(a), t(a - 46_000));
            assert_eq!(t(a), t(a + 12_345));
            assert_eq!(t(a) % PAGE as u32, 0);
        }
        let small = next_threshold(100_000 + 100_000, 100_000, big);
        assert!(small >= 200_000 + a + 100_000, "{small}");
    }

    #[test]
    fn cheap_leaves_count_in_part() {
        // D3144: 4 MB live above the prefix, 3 MB of them large strings.
        let (big, base, live) = (arena::MAX_ARENA, 200_000, 200_000 + (4 << 20));
        let full = next_threshold(live, base, big);
        let cheap = next_threshold_cheap(live, base, big, 3 << 20);
        // Growth shrinks by the discounted share of the cheap bytes, up to the page rounding.
        let less = (3 << 20) * CHEAP_DISCOUNT / 100;
        assert!(
            (full - cheap).abs_diff(less) < PAGE as u32,
            "{full} {cheap} {less}"
        );
        // Never below the minimum growth, and no effect without cheap bytes.
        assert!(next_threshold_cheap(live, base, big, live) >= live + MIN_GROWTH);
        assert_eq!(next_threshold_cheap(live, base, big, 0), full);
    }
}
