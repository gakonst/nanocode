//! The arena: one growable little-endian byte buffer addressed by `u32` offsets.
//!
//! All access is bounds checked through slice indexing, so a bad offset panics (a wasm
//! trap) but never causes undefined behavior. Bulk operations take one slice up front so
//! the per-byte checks can be hoisted by the compiler.

use alloc::vec::Vec;

use super::HeapError;
use super::layout::{Field, FieldType};

/// The largest arena size: offsets must fit in a `u32`, and cell sizes are multiples of 8.
pub const MAX_ARENA: u32 = !7;

/// A growable byte buffer with a memory cap. Allocation only bumps; it never collects.
///
/// `bytes.len()` is the *zeroed frontier*: everything in `bytes[len..]` is zero, so an
/// allocation below the frontier is a bounds check and a bump (D1261). Zeroing happens
/// ahead in chunks of [`ZERO_AHEAD`]. An allocation with data at the frontier (strings,
/// GC copies) appends the data instead, so those bytes are written once.
pub struct Arena {
    bytes: Vec<u8>,
    len: u32,
    limit: u32,
    /// While the arena is below this size, growing never reserves past it (0: no target).
    /// The heap sets it to the GC threshold, so the Vec's growth does not overshoot the
    /// size the next collection starts at (D1870).
    target: u32,
    /// The prefix write barrier (D2065): in debug builds, a write below this offset (and
    /// past the roots cell) panics. 0: off. Not part of the image.
    #[cfg(debug_assertions)]
    trap_end: u32,
    /// The written pages of a tracked prefix ([`Arena::track`]). Not part of the image.
    #[cfg(feature = "embed")]
    tracked: Tracked,
    /// The embedder's stop request ([`Arena::set_interrupt`]). Not part of the image.
    #[cfg(feature = "embed")]
    interrupt: Option<alloc::sync::Arc<core::sync::atomic::AtomicBool>>,
}

/// A page of [`Tracked`]: 1 KiB (a bit of `dirty` each).
#[cfg(feature = "embed")]
const DIRTY_SHIFT: u32 = 10;

/// Which pages of the arena's first `image` bytes were written since they were a copy of
/// an image ([`Arena::track`]): what [`Arena::restore`] copies back, instead of the whole
/// image, to make the arena that image again. Every write goes through the arena's
/// write methods, which note the page of one below `end`. From `end` to `image` are
/// the interpreter's own cells (its registers and queues, and the value stack, which
/// the hot loop writes through [`Arena::hot_bytes`]): written all the time, they count
/// as written, and a write there costs what one above the image does.
#[cfg(feature = "embed")]
#[derive(Default)]
struct Tracked {
    /// The length of the prefix that was the image. 0: not tracking, or no longer: cells
    /// moved (a collection) or the arena shrank below it
    image: u32,
    /// Writes below it are noted (at most `image`)
    end: u32,
    /// A bit per page below `end`
    dirty: Vec<u64>,
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
std::thread_local! {
    /// Test hook: the next growth of any arena's backing store fails as if the host
    /// allocator refused (D1690).
    pub(crate) static FAIL_NEXT_RESERVE: core::cell::Cell<bool> = const { core::cell::Cell::new(false) };
}

/// How far ahead of an allocation the frontier is zeroed: big enough that small cells
/// rarely touch the Vec, small enough not to matter for memory or the GC.
const ZERO_AHEAD: u32 = 16 << 10;

/// A wasm page: the arena's growth step below the GC threshold.
const PAGE: usize = 64 << 10;

impl Arena {
    /// An empty arena that may grow to `limit` bytes (clamped to [`MAX_ARENA`]).
    pub fn new(limit: usize) -> Arena {
        Arena {
            bytes: Vec::new(),
            len: 0,
            limit: clamp_limit(limit),
            target: 0,
            #[cfg(debug_assertions)]
            trap_end: 0,
            #[cfg(feature = "embed")]
            tracked: Tracked::default(),
            #[cfg(feature = "embed")]
            interrupt: None,
        }
    }

    /// An empty arena with `capacity` bytes reserved up front, so allocations up to that
    /// total never touch the host allocator (the GC's to-space).
    pub fn with_capacity(capacity: usize, limit: usize) -> Result<Arena, HeapError> {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(capacity)
            .map_err(|_| HeapError::HostOutOfMemory)?;
        Ok(Arena {
            bytes,
            len: 0,
            limit: clamp_limit(limit),
            target: 0,
            #[cfg(debug_assertions)]
            trap_end: 0,
            #[cfg(feature = "embed")]
            tracked: Tracked::default(),
            #[cfg(feature = "embed")]
            interrupt: None,
        })
    }

    /// See the `target` field.
    pub(crate) fn set_target(&mut self, target: u32) {
        self.target = target;
    }

    /// Reserves capacity for `total` bytes now (best effort: a refusal leaves the arena as
    /// it is, and growth asks again later). A restored arena's buffer is exactly the image,
    /// and its first allocation would reallocate it anyway; right after the restore it is
    /// usually the allocator's last block, so it grows in place instead of being copied and
    /// leaving its old block behind as a hole (D1942).
    pub(crate) fn reserve_total(&mut self, total: u32) {
        let total = total.min(self.limit) as usize;
        if total > self.bytes.capacity() {
            let _ = self.bytes.try_reserve_exact(total - self.bytes.len());
        }
    }

    /// Bytes reserved from the host.
    pub fn capacity(&self) -> usize {
        self.bytes.capacity()
    }

    /// Returns reserved memory beyond `keep` bytes (at least the length) to the host.
    pub fn shrink_capacity(&mut self, keep: usize) {
        let keep = keep.max(self.len as usize);
        self.bytes.truncate(keep);
        self.bytes.shrink_to(keep);
    }

    /// An arena over existing bytes (for restoring a heap image). The caller validates the
    /// contents.
    pub fn from_bytes(bytes: Vec<u8>, limit: usize) -> Arena {
        Arena {
            len: bytes.len() as u32,
            bytes,
            limit: clamp_limit(limit),
            target: 0,
            #[cfg(debug_assertions)]
            trap_end: 0,
            #[cfg(feature = "embed")]
            tracked: Tracked::default(),
            #[cfg(feature = "embed")]
            interrupt: None,
        }
    }

    /// Sets the flag another thread raises to stop a run that is inside a long native
    /// operation, where no fuel check comes for seconds (a split or a join of hundreds of
    /// megabytes): once it is raised, the arena refuses to grow, as if the host had no
    /// memory left (D1690), and the run ends with `Status::HostOutOfMemory` at once.
    /// Checked where the zeroed frontier moves (every 16 KiB allocated) and for each
    /// string appended past it.
    #[cfg(feature = "embed")]
    pub fn set_interrupt(
        &mut self,
        flag: Option<alloc::sync::Arc<core::sync::atomic::AtomicBool>>,
    ) {
        self.interrupt = flag;
    }

    /// Whether the embedder asked the run to stop ([`Arena::set_interrupt`]).
    #[inline]
    pub fn interrupted(&self) -> bool {
        #[cfg(feature = "embed")]
        {
            self.interrupt
                .as_ref()
                .is_some_and(|flag| flag.load(core::sync::atomic::Ordering::Relaxed))
        }
        #[cfg(not(feature = "embed"))]
        false
    }

    /// Starts tracking writes to the first `image` bytes, which are a copy of an image
    /// now, or a copy with the `written` ranges (offset, length) changed: see
    /// [`Tracked`]. From `end` on the prefix counts as written.
    #[cfg(feature = "embed")]
    pub fn track(&mut self, image: u32, end: u32, written: impl Iterator<Item = (u32, u32)>) {
        // (A heap shorter than the image is not a copy of it.)
        let image = if image > self.len { 0 } else { image };
        let end = end.min(image);
        let pages = end.div_ceil(1 << DIRTY_SHIFT) as usize;
        self.tracked = Tracked {
            image,
            end,
            dirty: alloc::vec![0; pages.div_ceil(64)],
        };
        for (off, len) in written {
            if off < end {
                self.note(off, len);
            }
        }
    }

    /// Writes below it are noted in a tracked prefix (0: not tracking).
    #[inline(always)]
    pub(crate) fn tracked_end(&self) -> u32 {
        #[cfg(feature = "embed")]
        {
            self.tracked.end
        }
        #[cfg(not(feature = "embed"))]
        0
    }

    /// The tracked prefix is not its image any more (a collection moved cells).
    #[inline]
    pub(crate) fn lose_tracked(&mut self) {
        #[cfg(feature = "embed")]
        {
            self.tracked.image = 0;
            self.tracked.end = 0;
        }
    }

    /// The written parts of the tracked prefix as byte ranges, ascending: `None` when
    /// the arena does not track its prefix as a copy of an image of `image_len` bytes.
    #[cfg(feature = "embed")]
    pub fn written(&self, image_len: usize) -> Option<impl Iterator<Item = (usize, usize)> + '_> {
        let t = &self.tracked;
        if t.image == 0 || t.image as usize != image_len {
            return None;
        }
        let end = t.end as usize;
        let pages = t.dirty.iter().enumerate().flat_map(move |(w, &bits)| {
            (0..64).filter(move |b| bits >> b & 1 != 0).map(move |b| {
                let start = (w * 64 + b) << DIRTY_SHIFT;
                (start, (start + (1 << DIRTY_SHIFT)).min(end))
            })
        });
        Some(pages.chain(core::iter::once((end, image_len))))
    }

    /// Makes the arena `image` again by copying back what was written since
    /// [`Arena::track`] and dropping everything above it; the capacity stays. `false`
    /// (nothing done) when the arena does not track its prefix as a copy of `image`.
    #[cfg(feature = "embed")]
    pub(crate) fn restore(&mut self, image: &[u8]) -> bool {
        let t = &mut self.tracked;
        if t.image == 0 || t.image as usize != image.len() {
            return false;
        }
        let end = t.end as usize;
        for (w, bits) in t.dirty.iter_mut().enumerate() {
            let mut rest = core::mem::take(bits);
            while rest != 0 {
                let b = rest.trailing_zeros() as usize;
                rest &= rest - 1;
                let start = (w * 64 + b) << DIRTY_SHIFT;
                let stop = (start + (1 << DIRTY_SHIFT)).min(end);
                self.bytes[start..stop].copy_from_slice(&image[start..stop]);
            }
        }
        self.bytes[end..image.len()].copy_from_slice(&image[end..]);
        self.bytes.truncate(image.len());
        self.len = image.len() as u32;
        debug_assert!(
            self.bytes == image,
            "a write below the image was not tracked"
        );
        true
    }

    /// Takes the buffer out, leaving the arena empty: for a reload into the same
    /// allocation (`Vm::reset_to`).
    #[cfg(feature = "embed")]
    pub(crate) fn take_bytes(&mut self) -> Vec<u8> {
        self.len = 0;
        core::mem::take(&mut self.bytes)
    }

    /// Bytes in use. This is also the offset of the next allocation.
    #[inline(always)]
    pub fn len(&self) -> u32 {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The memory cap in bytes.
    pub fn limit(&self) -> u32 {
        self.limit
    }

    /// Moves the cap (the heap's out-of-memory reserve, D1600); clamped like `new`.
    pub(crate) fn set_limit(&mut self, limit: u32) {
        self.limit = clamp_limit(limit as usize);
    }

    /// All bytes in use.
    #[inline]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    /// Allocates `size` zeroed bytes (rounded up to 8) at the end and returns their offset.
    /// Fails with [`HeapError::OutOfMemory`] when the cap would be exceeded or the host
    /// allocator fails. Never panics on exhaustion.
    #[inline]
    pub fn alloc(&mut self, size: u32) -> Result<u32, HeapError> {
        let (start, end) = self.bounds(size)?;
        if end as usize > self.bytes.len() {
            self.zero_ahead(end)?;
        }
        self.len = end;
        Ok(start)
    }

    /// Like [`Arena::alloc`], but the `data.len()` bytes at offset `at` of the new block
    /// are `data` instead of zeros. At the frontier they are appended, so they are written
    /// once rather than zeroed and then copied (the bulk of strings and GC copies). Panics
    /// if `data` does not fit in `size` at `at`.
    pub fn alloc_init(&mut self, size: u32, at: u32, data: &[u8]) -> Result<u32, HeapError> {
        let fits = (size as usize).checked_sub(at as usize + data.len());
        assert!(fits.is_some(), "alloc_init: data does not fit");
        let (start, end) = self.bounds(size)?;
        let at_us = (start + at) as usize;
        if end as usize > self.bytes.len() {
            // Past the frontier: append instead of zeroing and then copying. The bytes
            // from `at_us` to the frontier are zeros, so dropping them keeps the invariant.
            self.reserve(end)?;
            self.bytes.truncate(at_us.max(self.len as usize));
            zero_to(&mut self.bytes, at_us);
            self.bytes.extend_from_slice(data);
            zero_to(&mut self.bytes, end as usize);
        } else {
            self.bytes[at_us..at_us + data.len()].copy_from_slice(data);
        }
        self.len = end;
        Ok(start)
    }

    /// Like [`Arena::alloc_init`] with the data from an iterator of exactly `n` bytes,
    /// for data that needs converting on the way in (UTF-16 units to little-endian pairs).
    /// Panics if the iterator yields a different number of bytes or `n` does not fit.
    #[inline]
    pub fn alloc_from_iter(
        &mut self,
        size: u32,
        at: u32,
        n: u32,
        data: impl Iterator<Item = u8>,
    ) -> Result<u32, HeapError> {
        let fits = (size as usize).checked_sub(at as usize + n as usize);
        assert!(fits.is_some(), "alloc_from_iter: data does not fit");
        let (start, end) = self.bounds(size)?;
        let at_us = (start + at) as usize;
        if end as usize > self.bytes.len() {
            self.reserve(end)?;
            self.bytes.truncate(at_us.max(self.len as usize));
            zero_to(&mut self.bytes, at_us);
            self.bytes.extend(data);
            assert_eq!(
                self.bytes.len(),
                at_us + n as usize,
                "alloc_from_iter: wrong length"
            );
            zero_to(&mut self.bytes, end as usize);
        } else {
            let dst = &mut self.bytes[at_us..at_us + n as usize];
            let mut written = 0;
            for (d, b) in dst.iter_mut().zip(data) {
                *d = b;
                written += 1;
            }
            assert_eq!(written, n as usize, "alloc_from_iter: wrong length");
        }
        self.len = end;
        Ok(start)
    }

    /// The start and end of a block of `size` bytes (rounded up to 8) at the end, if it is
    /// under the cap.
    #[inline(always)]
    fn bounds(&self, size: u32) -> Result<(u32, u32), HeapError> {
        let size = size.checked_add(7).ok_or(HeapError::OutOfMemory)? & !7;
        let end = self
            .len
            .checked_add(size)
            .filter(|&e| e <= self.limit)
            .ok_or(HeapError::OutOfMemory)?;
        Ok((self.len, end))
    }

    /// Moves the zeroed frontier to at least `end` (up to [`ZERO_AHEAD`] beyond it, within
    /// the cap and the reserved capacity).
    #[cold]
    fn zero_ahead(&mut self, end: u32) -> Result<(), HeapError> {
        self.reserve(end)?;
        let ahead = end
            .saturating_add(ZERO_AHEAD)
            .min(self.limit)
            .min(self.bytes.capacity() as u32)
            .max(end);
        zero_to(&mut self.bytes, ahead as usize);
        Ok(())
    }

    /// Makes the Vec's capacity at least `end`: geometric growth (by an eighth, in pages),
    /// never past the cap.
    fn reserve(&mut self, end: u32) -> Result<(), HeapError> {
        if self.interrupted() {
            return Err(HeapError::HostOutOfMemory);
        }
        let end = end as usize;
        if end > self.bytes.capacity() {
            let cap = self.bytes.capacity();
            let mut want = if self.target == 0 || end <= self.target as usize {
                // An eighth more, in whole wasm pages, but not past the target (D1870,
                // D3140): wasm grows the arena in place at memory's end, so doubling only
                // reserved pages the run might never touch, and memory never shrinks.
                #[cfg(target_arch = "wasm32")]
                let w = (cap + (cap / 8).max(PAGE))
                    .next_multiple_of(PAGE)
                    .max(end)
                    .max(4096);
                // A native allocator moves a block that grows, a copy of the whole arena,
                // and pages it reserved are not memory until they are written: double.
                #[cfg(not(target_arch = "wasm32"))]
                let w = (2 * cap).next_multiple_of(PAGE).max(end).max(4096);
                if self.target == 0 {
                    w
                } else if cfg!(target_arch = "wasm32") {
                    w.min(self.target as usize)
                } else {
                    // To the target at once: the arena stops there until a collection.
                    self.target as usize
                }
            } else {
                // Past the target a collection is pending: grow by a quarter, not double,
                // so the from-space (the next spare) stays close to the threshold.
                (cap + cap / 4).max(end)
            };
            want = want.max(end);
            let want = want.min(self.limit as usize);
            #[cfg(test)]
            if FAIL_NEXT_RESERVE.with(|f| f.replace(false)) {
                return Err(HeapError::HostOutOfMemory);
            }
            #[cfg(target_arch = "wasm32")]
            self.make_room_after(want - cap);
            // Below the cap, so the host's refusal is not the program's (D1690).
            self.bytes
                .try_reserve_exact(want - self.bytes.len())
                .map_err(|_| HeapError::HostOutOfMemory)?;
        }
        Ok(())
    }

    #[cfg(target_arch = "wasm32")]
    fn make_room_after(&self, more: usize) {
        let cap = self.bytes.capacity();
        if cap > 0 {
            hold_until_past(more, self.bytes.as_ptr() as usize + cap);
        }
    }

    // Field accesses are checked against the Vec (the zeroed frontier), not `len`: one
    // compare less on the hottest path, and still memory safe. Debug builds also check
    // `len`, so a stray access past the end is caught in tests (D1261).
    #[inline(always)]
    pub fn read<T: FieldType>(&self, off: u32) -> T {
        debug_assert!(
            off as usize + T::TY.size() as usize <= self.len as usize,
            "read past the arena end"
        );
        T::read(&self.bytes, off as usize)
    }

    /// Arms the prefix write barrier up to `end` (0 disarms; debug builds only, D2065).
    pub(crate) fn set_trap_end(&mut self, end: u32) {
        #[cfg(debug_assertions)]
        {
            self.trap_end = end;
        }
        let _ = end;
    }

    /// The armed barrier's end (0: off; always 0 in release builds).
    pub(crate) fn trap_end(&self) -> u32 {
        #[cfg(debug_assertions)]
        {
            self.trap_end
        }
        #[cfg(not(debug_assertions))]
        0
    }

    /// The barrier: `len` bytes written at `off`. Debug builds check the prefix write
    /// barrier; a tracked prefix notes its written pages.
    #[inline(always)]
    fn barrier(&mut self, off: u32, len: u32) {
        #[cfg(debug_assertions)]
        if off < self.trap_end && off + len > super::cells::Roots::LAYOUT.min_size {
            prefix_write(off);
        }
        #[cfg(feature = "embed")]
        if off < self.tracked.end {
            self.note(off, len);
        }
        let _ = (off, len);
    }

    /// Notes the pages of a write of `len` bytes below the tracked prefix's `end`. Out
    /// of line: the write methods are inlined everywhere, and nearly all writes are
    /// above it.
    #[cfg(feature = "embed")]
    #[cold]
    #[inline(never)]
    fn note(&mut self, off: u32, len: u32) {
        let t = &mut self.tracked;
        if len == 0 {
            return;
        }
        let last = off.saturating_add(len - 1).min(t.end - 1) >> DIRTY_SHIFT;
        let mut page = off >> DIRTY_SHIFT;
        while page <= last {
            t.dirty[(page >> 6) as usize] |= 1 << (page & 63);
            page += 1;
        }
    }

    /// Notes a write of `len` bytes at `off` made through [`Arena::hot_bytes`].
    #[inline(always)]
    pub(crate) fn wrote(&mut self, off: u32, len: u32) {
        #[cfg(feature = "embed")]
        if off < self.tracked.end {
            self.note(off, len);
        }
        let _ = (off, len);
    }

    /// [`Arena::write`] without the tracked prefix's check, for a write that cannot be
    /// below its `end` ([`Tracked`]; debug builds check): to a cell just allocated, to
    /// the value stack (the interpreter's general path, every push), or by a collection,
    /// which ends the tracking first. That check on each of them was a twentieth of the
    /// instructions of a run that builds and collects objects.
    #[inline(always)]
    pub(crate) fn write_untracked<T: FieldType>(&mut self, off: u32, v: T) {
        #[cfg(debug_assertions)]
        if off < self.trap_end && off + T::TY.size() > super::cells::Roots::LAYOUT.min_size {
            prefix_write(off);
        }
        #[cfg(feature = "embed")]
        debug_assert!(
            off >= self.tracked.end,
            "a write below the tracked prefix's end was not noted"
        );
        debug_assert!(
            off as usize + T::TY.size() as usize <= self.len as usize,
            "write past the arena end"
        );
        T::write(&mut self.bytes, off as usize, v)
    }

    /// [`Arena::set`] as [`Arena::write_untracked`].
    #[inline(always)]
    pub(crate) fn set_untracked<T: FieldType>(&mut self, base: u32, f: Field<T>, v: T) {
        self.write_untracked(base + f.offset, v)
    }

    #[inline(always)]
    pub fn write<T: FieldType>(&mut self, off: u32, v: T) {
        self.barrier(off, T::TY.size());
        debug_assert!(
            off as usize + T::TY.size() as usize <= self.len as usize,
            "write past the arena end"
        );
        T::write(&mut self.bytes, off as usize, v)
    }

    /// The backing store up to the zeroed frontier, the range `read`/`write` check against,
    /// for a loop that keeps the arena in a local while it cannot allocate
    /// (interp/run/hot.rs, D1930).
    #[inline(always)]
    pub(crate) fn hot_bytes(&mut self) -> &mut [u8] {
        &mut self.bytes
    }

    #[inline(always)]
    pub(crate) fn read_values<const N: usize>(&self, off: u32) -> [crate::value::Value; N] {
        let s = &self.bytes[off as usize..off as usize + N * 8];
        core::array::from_fn(|i| FieldType::read(s, i * 8))
    }

    #[inline(always)]
    pub(crate) fn write_values<const N: usize>(&mut self, off: u32, v: [crate::value::Value; N]) {
        self.barrier(off, (N * 8) as u32);
        let s = &mut self.bytes[off as usize..off as usize + N * 8];
        for (i, x) in v.into_iter().enumerate() {
            FieldType::write(s, i * 8, x);
        }
    }

    /// Reads field `f` of the cell (or record) at `base`.
    #[inline(always)]
    pub fn get<T: FieldType>(&self, base: u32, f: Field<T>) -> T {
        self.read(base + f.offset)
    }

    /// Writes field `f` of the cell (or record) at `base`.
    #[inline(always)]
    pub fn set<T: FieldType>(&mut self, base: u32, f: Field<T>, v: T) {
        self.write(base + f.offset, v)
    }

    #[inline]
    pub fn slice(&self, off: u32, len: u32) -> &[u8] {
        &self.bytes()[off as usize..off as usize + len as usize]
    }

    #[inline]
    pub fn slice_mut(&mut self, off: u32, len: u32) -> &mut [u8] {
        self.barrier(off, len);
        &mut self.bytes[..self.len as usize][off as usize..off as usize + len as usize]
    }

    /// Splits the arena at `at`: everything before it read-only, everything from it on
    /// writable. Used to copy from older cells into a freshly allocated one.
    #[inline]
    pub fn split_mut(&mut self, at: u32) -> (&[u8], &mut [u8]) {
        self.barrier(at, 1);
        let (a, b) = self.bytes[..self.len as usize].split_at_mut(at as usize);
        (a, b)
    }

    /// Copies `len` bytes from `src` to `dst` (the ranges may overlap).
    #[inline]
    pub fn copy_within(&mut self, src: u32, len: u32, dst: u32) {
        self.barrier(dst, len);
        let s = src as usize;
        self.bytes[..self.len as usize].copy_within(s..s + len as usize, dst as usize);
    }

    /// Zeroes `len` bytes at `off`.
    #[inline]
    pub fn zero(&mut self, off: u32, len: u32) {
        self.slice_mut(off, len).fill(0);
    }

    /// Frees everything from `to` on (`to <= len`). The zeroed frontier moves back to
    /// `to` too (the capacity stays): zeroing the freed bytes instead cost the collector
    /// a memset of all the garbage, and left a frontier far ahead, so every string
    /// allocated afterwards took `alloc_from_iter`'s byte-by-byte path below the frontier
    /// instead of the append (0224: `JSON.stringify` of 10k objects 1.5x slower).
    pub(crate) fn truncate(&mut self, to: u32) {
        assert!(to <= self.len, "truncate past the end");
        #[cfg(feature = "embed")]
        if to < self.tracked.image {
            self.lose_tracked();
        }
        self.bytes.truncate(to as usize);
        self.len = to;
    }
}

/// Appends zero bytes up to length `len` (nothing if already that long). `resize` is a
/// memset natively and at opt-level "s" and up (`memory.fill` in wasm). At "z" it is a
/// byte loop; no safe way to extend a Vec is better there (static-block or doubling
/// copies cost 15% natively), and `alloc_init` avoids zeroing where it matters most.
#[inline]
fn zero_to(bytes: &mut Vec<u8>, len: usize) {
    if len > bytes.len() {
        bytes.resize(len, 0);
    }
}

#[cfg(target_arch = "wasm32")]
fn hold_until_past(more: usize, end: usize) {
    let mut probe = Vec::<u8>::new();
    if probe.try_reserve_exact(more).is_ok()
        && (core::hint::black_box(probe.as_ptr()) as usize) < end
    {
        hold_until_past(more, end);
    }
}

fn clamp_limit(limit: usize) -> u32 {
    (limit.min(MAX_ARENA as usize) as u32) & !7
}

/// A write below the armed prefix barrier's end: an engine bug (D2065).
#[cfg(debug_assertions)]
#[cold]
#[inline(never)]
fn prefix_write(off: u32) -> ! {
    panic!("write into the builtin prefix at {off} (D2065)")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Value;

    #[test]
    fn alloc_is_aligned_and_zeroed() {
        let mut a = Arena::new(1 << 20);
        assert_eq!(a.alloc(1).unwrap(), 0);
        assert_eq!(a.alloc(9).unwrap(), 8);
        assert_eq!(a.alloc(0).unwrap(), 24);
        assert_eq!(a.alloc(8).unwrap(), 24);
        assert_eq!(a.len(), 32);
        assert!(a.bytes().iter().all(|&b| b == 0));
    }

    #[test]
    fn read_write_little_endian() {
        let mut a = Arena::new(1 << 20);
        let o = a.alloc(32).unwrap();
        a.write::<u32>(o, 0x0403_0201);
        assert_eq!(a.slice(o, 4), &[1, 2, 3, 4]);
        a.write::<u16>(o + 4, 0xBBAA);
        assert_eq!(a.slice(o + 4, 2), &[0xAA, 0xBB]);
        a.write(o + 8, Value::UNDEFINED);
        assert_eq!(a.read::<Value>(o + 8), Value::UNDEFINED);
        assert_eq!(a.read::<u64>(o + 8), Value::UNDEFINED.to_bits());
        a.write::<f64>(o + 16, -2.5);
        assert_eq!(a.read::<f64>(o + 16), -2.5);
        a.write::<i32>(o + 24, -3);
        assert_eq!(a.read::<i32>(o + 24), -3);
        a.copy_within(o, 4, o + 28);
        assert_eq!(a.read::<u32>(o + 28), 0x0403_0201);
    }

    #[test]
    fn alloc_init_writes_data_and_zeroes_the_rest() {
        let mut a = Arena::new(1 << 20);
        a.alloc(8).unwrap();
        let o = a.alloc_init(13, 3, b"abcd").unwrap();
        assert_eq!(o, 8);
        assert_eq!(a.len(), 24);
        assert_eq!(a.slice(8, 16), b"\0\0\0abcd\0\0\0\0\0\0\0\0\0");
        // Large blocks (more than one zero chunk) on both sides.
        let o = a.alloc_init(2000, 700, &[7; 600]).unwrap();
        let s = a.slice(o, 2000);
        assert!(s[..700].iter().all(|&b| b == 0));
        assert!(s[700..1300].iter().all(|&b| b == 7));
        assert!(s[1300..].iter().all(|&b| b == 0));
        let mut small = Arena::new(32);
        assert_eq!(small.alloc_init(40, 0, b"x"), Err(HeapError::OutOfMemory));
        assert_eq!(small.len(), 0);
    }

    /// Random allocations against a plain Vec model: whatever the frontier does, new
    /// blocks read as zeros (or their data), and `bytes()` is exactly what was allocated.
    #[test]
    fn frontier_model() {
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut next = |n: u32| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as u32
        };
        for round in 0..20 {
            let mut a = if round % 2 == 0 {
                Arena::new(1 << 22)
            } else {
                Arena::with_capacity(1 << 16, 1 << 22).unwrap()
            };
            let mut model: Vec<u8> = Vec::new();
            for _ in 0..300 {
                let size = match next(4) {
                    0 => next(64),
                    1 => next(600),
                    2 => next(40_000),
                    _ => 8,
                };
                let padded = (size + 7) & !7;
                match next(4) {
                    0 | 1 => {
                        let off = a.alloc(size).unwrap();
                        assert_eq!(off as usize, model.len());
                        model.resize(model.len() + padded as usize, 0);
                        // Dirty the block like a caller would.
                        if size >= 8 {
                            a.write::<u64>(off, 0xAAAA_AAAA_AAAA_AAAA);
                            model[off as usize..off as usize + 8].fill(0xAA);
                        }
                    }
                    2 => {
                        let at = next(size + 1);
                        let n = next(size - at + 1);
                        let data: Vec<u8> = (0..n).map(|i| i as u8 | 1).collect();
                        let off = a.alloc_init(size, at, &data).unwrap();
                        assert_eq!(off as usize, model.len());
                        model.resize(model.len() + padded as usize, 0);
                        let d = off as usize + at as usize;
                        model[d..d + n as usize].copy_from_slice(&data);
                    }
                    _ => a.shrink_capacity(next(1 << 20) as usize),
                }
                assert_eq!(a.bytes(), &model[..]);
            }
            // A restored image continues the same way.
            let mut b = Arena::from_bytes(a.bytes().to_vec(), 1 << 22);
            let off = b.alloc(100).unwrap();
            assert!(b.slice(off, 100).iter().all(|&x| x == 0));
        }
    }

    #[test]
    fn truncate_moves_the_frontier_back() {
        let mut a = Arena::new(1 << 24);
        a.alloc(1 << 20).unwrap();
        let cap = a.capacity();
        a.truncate(64);
        assert_eq!(a.len(), 64);
        // No zeroed tail kept: big allocations after a collection append their data
        // (the guard for 0224's stringify regression).
        assert_eq!(a.bytes.len(), 64);
        assert_eq!(a.capacity(), cap);
        let o = a.alloc(16).unwrap();
        assert_eq!(o, 64);
        assert!(a.bytes().iter().all(|&b| b == 0));
    }

    #[test]
    fn alloc_from_iter_both_paths() {
        let mut a = Arena::new(1 << 20);
        // At the frontier (fresh arena): appended.
        let units = [0x1234u16, 0xABCD, 0x0041];
        let it = units.iter().flat_map(|u| u.to_le_bytes());
        let o = a.alloc_from_iter(16, 4, 6, it).unwrap();
        assert_eq!(
            a.slice(o, 16),
            &[
                0, 0, 0, 0, 0x34, 0x12, 0xCD, 0xAB, 0x41, 0, 0, 0, 0, 0, 0, 0
            ]
        );
        // Below the frontier (zeroed ahead by a small alloc): written in place.
        a.alloc(8).unwrap();
        let o = a.alloc_from_iter(8, 1, 3, b"xyz".iter().copied()).unwrap();
        assert_eq!(a.slice(o, 8), b"\0xyz\0\0\0\0");
    }

    #[test]
    #[should_panic(expected = "wrong length")]
    fn alloc_from_iter_checks_the_length() {
        Arena::new(1 << 20)
            .alloc_from_iter(8, 0, 4, b"ab".iter().copied())
            .unwrap();
    }

    #[test]
    #[should_panic(expected = "does not fit")]
    fn alloc_init_checks_the_fit() {
        Arena::new(1 << 20).alloc_init(8, 4, b"12345").unwrap();
    }

    // With `unsafe-heap`, only debug builds check (D2520).
    #[test]
    #[cfg(any(debug_assertions, not(feature = "unsafe-heap")))]
    #[should_panic]
    fn out_of_bounds_panics() {
        let mut a = Arena::new(1 << 20);
        a.alloc(8).unwrap();
        // Past the zeroed frontier: a panic in every build.
        a.read::<u32>(1 << 20);
    }

    /// Just past the end but below the frontier: release builds read zeros (memory safe;
    /// D1261), debug builds catch it.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "past the arena end")]
    fn past_the_end_panics_in_debug_builds() {
        let mut a = Arena::new(1 << 20);
        a.alloc(8).unwrap();
        a.read::<u32>(6);
    }

    #[test]
    fn memory_cap() {
        let mut a = Arena::new(64);
        assert!(a.alloc(48).is_ok());
        assert!(a.alloc(16).is_ok());
        assert_eq!(a.alloc(1), Err(HeapError::OutOfMemory));
        assert_eq!(a.len(), 64);
        let mut b = Arena::new(usize::MAX);
        assert_eq!(b.limit(), MAX_ARENA);
        assert_eq!(b.alloc(u32::MAX), Err(HeapError::OutOfMemory));
        assert_eq!(b.alloc(u32::MAX - 3), Err(HeapError::OutOfMemory));
        assert_eq!(b.len(), 0);
    }
}
