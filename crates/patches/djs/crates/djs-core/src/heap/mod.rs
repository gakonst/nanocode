//! The heap: all VM state in one position-independent arena (ARCH "The Core Idea").
//!
//! - [`arena`]: the byte buffer, bounds-checked access, the memory cap.
//! - [`layout`] and [`cells`]: the layout description. Every cell kind and its fields are
//!   declared once in `cells.rs`; accessors, the layout table and the trace description
//!   are generated from it.
//! - [`string`], [`atom`], [`object`]: strings (Latin-1, UTF-16, ropes), atoms and symbols,
//!   objects with properties and dense elements.
//! - [`walk`]: the cell walker, tracing and heap verification.
//!
//! # Conventions
//!
//! - References are `u32` arena offsets of a cell start; 0 is null. Cells are 8-aligned.
//! - The arena is little-endian, contains no host pointers, and can be copied byte for byte
//!   into a fresh [`Heap`] ([`Heap::from_bytes`]): that is the precursor of snapshots.
//! - Allocation never collects. It bumps, may fail with [`HeapError::OutOfMemory`], and
//!   sets [`Heap::gc_pending`] once the heap has grown past its threshold. Rust locals
//!   holding references stay valid until the next [`Heap::collect`], which the owner calls
//!   only at safepoints (ARCH "Generated state machines"). Across a collection, only
//!   [`Handle`]s and the arena's own roots survive.
//! - Allocated memory is zeroed, and zero is a valid non-reference state for every traced
//!   field, so a cell can be traced at any point after allocation.
//!
//! # Roots
//!
//! The [`Roots`] cell lives at offset 0 and holds everything the GC starts
//! from: the atom table, the realm, the [`handle`] table, and (once they exist) the value
//! stack, the job queue and the pending host call table. Rust code holds no roots: the
//! interpreter (0006) keeps live values on the arena value stack, and the embedder holds
//! values through the handle table. A copying GC copies the roots cell first and then
//! scans the to-space linearly from 0 (Cheney), so the roots cell is traced like any
//! other cell.

pub mod arena;
mod arrays;
pub mod atom;
pub mod bigint;
pub mod cells;
pub mod collection;
mod compact;
pub(crate) use compact::IMAGE_STACK;
mod compactor;
mod fail;
mod frozen;
pub mod gc;
pub mod handle;
pub mod layout;
pub mod object;
pub mod proxy;
pub mod shape;
pub mod string;
pub mod typed;
pub mod walk;
pub mod weakref;

use core::fmt;

pub use arena::Arena;
pub use atom::{Atom, atoms};
pub use cells::CellKind;
pub use gc::GcStats;
pub use handle::Handle;
pub use layout::Ref;
pub use object::{Lookup, ObjectClass, PropFlags, Property, PropertyDescriptor};
pub use string::StrView;

use cells::{Header, Roots};

use crate::value::Value;

/// Size of the cell header in bytes.
pub const HEADER_SIZE: u32 = 8;

/// Rounds up to a multiple of 8.
#[inline]
pub const fn align8(n: u32) -> u32 {
    (n + 7) & !7
}

/// Errors from heap operations that the caller turns into JS exceptions or a VM status.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HeapError {
    /// The arena hit its memory cap: a JS `InternalError: out of memory`, a function of
    /// the program and the cap (D1600).
    OutOfMemory,
    /// The host could not provide memory below the cap (arena growth, the collector's
    /// to-space): not the program's doing, so not catchable (D1690). The run ends with
    /// `Status::HostOutOfMemory`.
    HostOutOfMemory,
    /// A string would exceed [`string::MAX_LEN`] code units (a RangeError in JS).
    StringTooLong,
    /// An array length that is not a valid `u32` Number (a RangeError in JS).
    InvalidArrayLength,
}

impl HeapError {
    /// The Display form.
    pub fn as_str(self) -> &'static str {
        match self {
            HeapError::OutOfMemory => "out of memory",
            HeapError::HostOutOfMemory => "out of host memory",
            HeapError::StringTooLong => "invalid string length",
            HeapError::InvalidArrayLength => "invalid array length",
        }
    }
}

impl fmt::Display for HeapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a heap image was rejected by [`Heap::from_bytes`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LoadError {
    /// Bad magic, wrong layout version, or a malformed root cell.
    BadHeader,
    /// The image is larger than the memory cap.
    TooLarge,
    /// The cell structure is broken (see [`walk::VerifyError`]).
    Corrupt(walk::VerifyError),
    /// A VM image this build cannot run: a component of its dependency descriptor
    /// (`vm::ImageDeps`, D2265) differs from this build's.
    Incompatible {
        dep: crate::vm::ImageDep,
        found: u32,
        expected: u32,
    },
}

impl LoadError {
    /// The Display form, built without core::fmt (D2339).
    pub fn text(&self) -> alloc::string::String {
        match self {
            LoadError::BadHeader => "not a djs heap image of this layout".into(),
            LoadError::TooLarge => "the image is larger than the memory limit".into(),
            LoadError::Corrupt(e) => crate::detail::detail("corrupt heap image", e),
            LoadError::Incompatible {
                dep,
                found,
                expected,
            } => crate::text::cat!(
                "the image is from a djs with another ",
                dep.name(),
                " (image: ",
                dep.show(*found),
                "; this build: ",
                dep.show(*expected),
                ")"
            ),
        }
    }
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text())
    }
}

/// The out-of-memory reserve for a cap (D1600): 1/32 of it, at most 1 MiB.
fn reserve_for(limit: u32) -> u32 {
    (limit / 32).min(1 << 20) & !7
}

/// The arena capacity for a GC threshold `t` (D1990): the threshold plus a slack for
/// what is allocated between crossing it and the next safepoint, where the collection
/// runs. Without it that overshoot reallocated the whole arena, and in wasm the moved
/// block left its old place behind as a hole during the collection.
fn capacity_target(t: u32) -> u32 {
    t.saturating_add((t / 16).max(64 << 10))
}

/// The default memory cap: the whole `u32` offset space.
pub const DEFAULT_LIMIT: usize = arena::MAX_ARENA as usize;

/// The VM heap. All state is in the arena; this struct only adds methods.
pub struct Heap {
    arena: Arena,
    /// Allocation sets `gc_pending` once the arena reaches this size. A copy of
    /// `Roots::GC_THRESHOLD` (D1601), kept here for the allocation fast path.
    gc_threshold: u32,
    gc_pending: bool,
    /// The memory cap. The arena's own limit is `hard_limit - reserve` while the
    /// out-of-memory reserve is closed, and `hard_limit` while it is open (D1600).
    hard_limit: u32,
    reserve: u32,
    /// The frozen-builtins mode's boundary (D2304): `Roots::BASE_END` while the mode is
    /// on, else 0. A copy kept current by `refresh_frozen`, so that the check on every
    /// property write is one compare.
    frozen_end: u32,
    /// The primary composites of Unicode normalization, derived on first use and kept
    /// for the VM's lifetime (D2338). Not part of the image.
    #[cfg(feature = "unicode-normalization")]
    pub(crate) compositions: crate::unicode::normalize::Compositions,
    /// A spare regexp matcher state: the `@fast` RegExp entries run match after match in
    /// its memory instead of allocating one each (builtins/regexp.rs). Not part of the
    /// image.
    pub(crate) regexp_state: crate::regexp::ExecState,
    /// The megamorphic inline cache: cache slots shared by every get site, keyed by the
    /// key and the receiver, for sites whose own slot keeps missing (several receiver
    /// shapes or prototypes). Its slots hold arena offsets, so every collection empties
    /// it. Allocated on first fill; not part of the image.
    pub(crate) mega_ic: alloc::vec::Vec<u32>,
    pub(crate) iterator_ic: [u32; cells::IC_WORDS as usize],
    pub(crate) return_ic: [u32; 2],
    pub(crate) regexp_shape: u32,
    pub(crate) rest_shape: u32,
    pub(crate) iter_result_shape: u32,
    pub(crate) last_transition: [u32; 4],
    pub(crate) error_ctor_slot: u32,
    pub(crate) stack_limit_slot: u32,
    pub(crate) frame_scratch: alloc::vec::Vec<(crate::value::Obj, u32, bool)>,
    pub(crate) atom_memo: alloc::vec::Vec<[u32; 2]>,
    /// The compiler's buffers, kept between compiles so that each one starts with their
    /// capacity (`resolve::finish`). Not part of the image.
    pub(crate) compile_scratch: crate::compiler::CompileScratch,
    pub(crate) ident_atoms: crate::compiler::IdentAtoms,
    /// The prefix as it was when the barrier was armed (debug builds, D2065): what
    /// [`Heap::check_prefix`] compares against, for writes that bypass `Arena::write`.
    #[cfg(debug_assertions)]
    prefix_copy: Option<alloc::vec::Vec<u8>>,
    /// The barrier is armed without frozen builtins (`VmOptions::prefix_barrier`).
    barrier_forced: bool,
    /// While a compile is recorded for the compile cache (D3040): every interning, in
    /// order (`compile_cache::Intern`).
    #[cfg(feature = "compile-cache")]
    pub(crate) intern_log: Option<alloc::vec::Vec<crate::vm::compile_cache::Intern>>,
    /// Test hook: make the next collection fail to reserve its to-space.
    #[cfg(test)]
    pub(crate) fail_next_gc_reserve: bool,
    /// Test statistics (0083): conversions to dictionary mode by reason (`object::Dict`).
    #[cfg(test)]
    pub(crate) dict_reasons: [u32; 6],
}

impl Heap {
    /// A fresh heap with the default memory cap.
    pub fn new() -> Result<Heap, HeapError> {
        Heap::with_limit(DEFAULT_LIMIT)
    }

    /// A fresh heap: the roots cell, the atom table with the well-known atoms and the
    /// well-known symbols.
    pub fn with_limit(limit: usize) -> Result<Heap, HeapError> {
        let mut heap = Heap::over(Arena::new(limit));
        let roots = heap.alloc_cell(CellKind::Roots, Roots::LAYOUT.min_size)?;
        debug_assert_eq!(roots, 0);
        heap.arena.set(0, Roots::MAGIC, cells::MAGIC);
        heap.arena.set(0, Roots::VERSION, cells::LAYOUT_HASH);
        heap.arena.set(0, Roots::REALM, Value::UNDEFINED);
        let t = heap.gc_threshold;
        heap.set_gc_threshold(t);
        heap.init_atoms()?;
        Ok(heap)
    }

    /// Restores a heap from an image produced by [`Heap::bytes`] of the same build. The
    /// whole image is verified (cell structure and every reference).
    pub fn from_bytes(bytes: alloc::vec::Vec<u8>, limit: usize) -> Result<Heap, LoadError> {
        let heap = Heap::from_bytes_trusted(bytes, limit)?;
        heap.verify().map_err(LoadError::Corrupt)?;
        Ok(heap)
    }

    /// Like [`Heap::from_bytes`] but checks only the header, not every cell: for images
    /// that cannot be corrupt, such as the base snapshot embedded at build time (verified
    /// when it was built). Restoring the 36 KB base takes 8 µs with verification and
    /// 1 µs without (D1321). Still memory safe: a bad cell is a panic, not UB.
    pub fn from_bytes_trusted(bytes: alloc::vec::Vec<u8>, limit: usize) -> Result<Heap, LoadError> {
        let heap = Heap::over(Arena::from_bytes(bytes, limit));
        let a = &heap.arena;
        if a.len() > heap.hard_limit {
            return Err(LoadError::TooLarge);
        }
        if (a.len() as usize) < Roots::LAYOUT.min_size as usize
            || !a.len().is_multiple_of(8)
            || a.get(0, Header::KIND) != CellKind::Roots as u8
            || a.get(0, Roots::MAGIC) != cells::MAGIC
            || a.get(0, Roots::VERSION) != cells::LAYOUT_HASH
        {
            return Err(LoadError::BadHeader);
        }
        Ok(heap)
    }

    /// Makes the heap `image` again, which it was loaded from and tracks
    /// ([`Arena::track`]), by copying back the pages written since: what loading the
    /// image anew gives, without the copy of all of it. `false` (nothing done) when
    /// the heap is no longer that image with written pages (a collection ran).
    #[cfg(feature = "embed")]
    pub fn restore_image(&mut self, image: &[u8]) -> bool {
        if !self.arena.restore(image) {
            return false;
        }
        let mut arena = core::mem::replace(&mut self.arena, Arena::new(0));
        arena.set_limit(self.hard_limit);
        let mut fresh = Heap::over(arena);
        // What the heap derived for itself and keeps for the VM's lifetime.
        #[cfg(feature = "unicode-normalization")]
        {
            fresh.compositions = core::mem::take(&mut self.compositions);
        }
        fresh.compile_scratch = core::mem::take(&mut self.compile_scratch);
        fresh.ident_atoms = core::mem::take(&mut self.ident_atoms);
        let stable_below = fresh.atom_count();
        fresh.ident_atoms.restart(stable_below);
        *self = fresh;
        true
    }

    /// The arena capacity a heap restored from an image of `image_len` bytes reserves
    /// (D1942, D2541, D3141): the image, rounded up to whole wasm pages. A host can stage
    /// the image in a buffer of this capacity, which becomes the arena without growing
    /// again (`djs_image_staging`). The arena grows from there on demand; in wasm it stays
    /// at the end of memory and grows in place, so any room reserved up front only held
    /// pages an idle VM never touches.
    pub fn image_capacity(image_len: usize, limit: usize) -> usize {
        const PAGE: usize = 64 << 10;
        let hard = (limit.min(arena::MAX_ARENA as usize) & !7).max(image_len);
        image_len.next_multiple_of(PAGE).min(hard)
    }

    /// The arena capacity of a VM made from an image of `image_len` bytes for a host that
    /// expects `live` bytes (`Vm::from_base_reserved`; 0: no hint, [`Heap::image_capacity`]).
    pub fn start_capacity(image_len: usize, base_end: u32, live: usize, limit: usize) -> usize {
        if live == 0 {
            Heap::image_capacity(image_len, limit)
        } else {
            Heap::restore_capacity(image_len.max(live), base_end, limit)
        }
    }

    /// The arena capacity for a heap of `image_len` bytes that is expected to grow on: its
    /// first GC threshold plus the slack (D1990), `base_end` of the bytes its builtin
    /// prefix ([`Heap::image_base_end`]). The prefix must be the image's, or the first
    /// threshold (D2442) is past the capacity and the arena grows at once.
    pub fn restore_capacity(image_len: usize, base_end: u32, limit: usize) -> usize {
        let hard = (limit.min(arena::MAX_ARENA as usize) as u32) & !7;
        let len = u32::try_from(image_len).unwrap_or(u32::MAX);
        let t = gc::next_threshold(len, base_end.min(len), hard - reserve_for(hard));
        (capacity_target(t) as usize).max(image_len)
    }

    /// The builtin prefix (`Roots::BASE_END`) of a heap image: 0 without one.
    pub fn image_base_end(image: &[u8]) -> u32 {
        let at = Roots::BASE_END.offset as usize;
        image
            .get(at..at + 4)
            .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn over(arena: Arena) -> Heap {
        let hard_limit = arena.limit();
        let mut heap = Heap {
            arena,
            gc_threshold: 0,
            gc_pending: false,
            hard_limit,
            reserve: reserve_for(hard_limit),
            frozen_end: 0,
            #[cfg(feature = "unicode-normalization")]
            compositions: Default::default(),
            regexp_state: Default::default(),
            mega_ic: Default::default(),
            iterator_ic: Default::default(),
            return_ic: Default::default(),
            regexp_shape: 0,
            rest_shape: 0,
            iter_result_shape: 0,
            last_transition: [0; 4],
            error_ctor_slot: 0,
            stack_limit_slot: 0,
            frame_scratch: Default::default(),
            atom_memo: Default::default(),
            compile_scratch: Default::default(),
            ident_atoms: Default::default(),
            #[cfg(debug_assertions)]
            prefix_copy: None,
            barrier_forced: false,
            #[cfg(feature = "compile-cache")]
            intern_log: None,
            #[cfg(test)]
            fail_next_gc_reserve: false,
            #[cfg(test)]
            dict_reasons: [0; 6],
        };
        heap.apply_reserve();
        if heap.arena.len() >= Roots::LAYOUT.min_size {
            let len = heap.arena.len() as usize;
            heap.arena
                .reserve_total(Heap::image_capacity(len, hard_limit as usize) as u32);
        }
        heap.refresh_frozen();
        heap
    }

    /// Whether the out-of-memory reserve is open (`Roots::FLAGS`, so it survives
    /// snapshots).
    pub fn memory_reserve_open(&self) -> bool {
        self.arena.len() >= Roots::LAYOUT.min_size
            && self.arena.get(0, Roots::FLAGS) & cells::ROOT_RESERVE_OPEN != 0
    }

    /// Sets the arena's cap and the GC threshold from the reserve state.
    fn apply_reserve(&mut self) {
        let limit = if self.memory_reserve_open() {
            self.hard_limit
        } else {
            self.hard_limit - self.reserve
        };
        self.arena.set_limit(limit);
        // A restored image keeps the threshold it had (the same collection points as
        // without the snapshot), unless it is beyond what this cap allows.
        let fresh = gc::next_threshold(self.arena.len(), self.base_end(), self.arena.limit());
        let stored = self.stored_gc_threshold();
        let t = if stored == 0 {
            fresh
        } else {
            stored.min(fresh)
        };
        self.set_gc_threshold(t);
        self.gc_pending = self.arena.len() >= self.gc_threshold;
    }

    fn stored_gc_threshold(&self) -> u32 {
        if self.arena.len() >= Roots::LAYOUT.min_size {
            self.arena.get(0, Roots::GC_THRESHOLD)
        } else {
            0
        }
    }

    /// Sets the GC threshold from the live size as a collection would (`next_threshold`).
    pub fn reset_gc_threshold(&mut self) {
        let t = gc::next_threshold(self.arena.len(), self.base_end(), self.arena.limit());
        self.set_gc_threshold(t);
    }

    /// Sets the GC threshold, in the roots cell too (D1601).
    pub(crate) fn set_gc_threshold(&mut self, t: u32) {
        self.gc_threshold = t;
        // The threshold's pages count from the prefix's end (D2544); the arena's growth
        // stops at whole wasm pages counted from offset 0, where its memory is.
        self.arena.set_target(t.saturating_add(0xFFFF) & !0xFFFF);
        if self.arena.len() >= Roots::LAYOUT.min_size {
            self.arena.set(0, Roots::GC_THRESHOLD, t);
        }
    }

    /// Opens the out-of-memory reserve (D1600): the interpreter calls this when it turns
    /// an allocation failure into an exception, so that the handlers have room to run.
    /// It closes again in the first collection that leaves twice the reserve free.
    /// Whether weak references are strong (D1680, [`cells::ROOT_WEAK_STRONG`]).
    pub fn weak_refs_strong(&self) -> bool {
        self.arena.get(0, Roots::FLAGS) & cells::ROOT_WEAK_STRONG != 0
    }

    /// Makes the WeakRefs and FinalizationRegistry cells created from now on strong (or
    /// weak again). Part of the image.
    pub fn set_weak_refs_strong(&mut self, strong: bool) {
        let f = self.arena.get(0, Roots::FLAGS);
        let f = if strong {
            f | cells::ROOT_WEAK_STRONG
        } else {
            f & !cells::ROOT_WEAK_STRONG
        };
        self.arena.set(0, Roots::FLAGS, f);
    }

    pub fn open_memory_reserve(&mut self) {
        if !self.memory_reserve_open() {
            let f = self.arena.get(0, Roots::FLAGS);
            self.arena
                .set(0, Roots::FLAGS, f | cells::ROOT_RESERVE_OPEN);
            self.apply_reserve();
        }
    }

    /// After a collection: closes the reserve if the live data left room for it.
    fn maybe_close_reserve(&mut self) {
        if self.memory_reserve_open()
            && self.arena.len() <= self.hard_limit.saturating_sub(2 * self.reserve)
        {
            let f = self.arena.get(0, Roots::FLAGS);
            self.arena
                .set(0, Roots::FLAGS, f & !cells::ROOT_RESERVE_OPEN);
            self.arena.set_limit(self.hard_limit - self.reserve);
            let t = gc::next_threshold(self.arena.len(), self.base_end(), self.arena.limit());
            self.set_gc_threshold(t);
        }
    }

    /// Starts allocating a suspension record (D1710): until [`Heap::end_unbudgeted`],
    /// allocations ignore the memory cap and do not make a collection pending. A record
    /// exists only when a fuel slice ends inside an intrinsic, so if it counted, the GC
    /// and out-of-memory points would depend on the slice size. Returns what `end`
    /// restores.
    pub(crate) fn begin_unbudgeted(&mut self) -> (u32, bool) {
        let saved = (self.arena.limit(), self.gc_pending);
        self.arena.set_limit(arena::MAX_ARENA);
        saved
    }

    /// Ends [`Heap::begin_unbudgeted`].
    pub(crate) fn end_unbudgeted(&mut self, (limit, pending): (u32, bool)) {
        self.arena.set_limit(limit);
        self.gc_pending = pending;
    }

    /// Frees the cells from `start` to `end` if they are the last ones in the arena: a
    /// suspension record that its intrinsic has read back on resuming (D1710). Then the
    /// arena is exactly as if the slice had not ended there. If anything was allocated
    /// after them (the host, between two `run`s), they stay as garbage.
    pub(crate) fn release_top(&mut self, start: u32, end: u32) {
        if self.arena.len() == end && start <= end {
            self.arena.truncate(start);
            self.atom_memo.fill([0; 2]);
            self.last_transition = [0; 4];
            // A full collection between the two runs made the record old: the old region
            // must not end past the arena.
            for f in [Roots::OLD_END, Roots::BASE_END] {
                if self.arena.get(0, f) > start {
                    self.arena.set(0, f, start);
                }
            }
            self.refresh_frozen();
            self.gc_pending = self.arena.len() >= self.gc_threshold;
        }
    }

    /// Arms the prefix write barrier (D2065) for a VM without frozen builtins, whose
    /// scripts are known not to change builtins (test suites); with frozen builtins it is
    /// always armed. In debug builds an armed barrier makes any write into the builtin
    /// prefix (the cells below `Roots::BASE_END`, past the roots cell) panic: `Arena::write`
    /// and its siblings check each write, and [`Heap::check_prefix`] compares the whole
    /// prefix for the paths that write through raw slices (the interpreter's hot loop).
    /// Release builds have no barrier.
    pub fn force_prefix_barrier(&mut self, on: bool) {
        self.barrier_forced = on;
        self.refresh_barrier();
    }

    /// Arms or disarms the barrier as the mode asks: on with frozen builtins or when forced,
    /// if there is a prefix. Arming takes the copy `check_prefix` compares against.
    pub(crate) fn refresh_barrier(&mut self) {
        if self.arena.len() < Roots::LAYOUT.min_size {
            return;
        }
        let end = self.base_end();
        let want = if (self.frozen_builtins() || self.barrier_forced) && end > 0 {
            end
        } else {
            0
        };
        if want == self.arena.trap_end() {
            return;
        }
        self.arena.set_trap_end(want);
        #[cfg(debug_assertions)]
        {
            self.prefix_copy = (want > 0).then(|| self.arena.bytes()[..want as usize].to_vec());
        }
    }

    /// Whether the prefix write barrier is armed.
    pub fn prefix_barrier_armed(&self) -> bool {
        self.arena.trap_end() != 0
    }

    /// Debug builds with the barrier armed: panics if any byte of the prefix (past the
    /// roots cell) differs from what it was when the barrier was armed (D2065).
    pub fn check_prefix(&self) {
        #[cfg(debug_assertions)]
        if let Some(copy) = &self.prefix_copy {
            let from = cells::Roots::LAYOUT.min_size as usize;
            let now = &self.arena.bytes()[from..copy.len()];
            if let Some(i) = now.iter().zip(&copy[from..]).position(|(a, b)| a != b) {
                panic!("the builtin prefix changed at {} (D2065)", from + i);
            }
        }
    }

    /// Runs `f` (a collection) with the barrier suspended: collections set and clear mark
    /// bits in the prefix's headers. The prefix must be as it was afterwards.
    pub(crate) fn without_barrier<T>(&mut self, f: impl FnOnce(&mut Heap) -> T) -> T {
        // A collection moves cells: a tracked prefix is no image with written pages.
        self.arena.lose_tracked();
        let end = self.arena.trap_end();
        self.arena.set_trap_end(0);
        let r = f(self);
        self.arena.set_trap_end(end);
        self.check_prefix();
        r
    }

    /// The arena bytes: a complete, position-independent image of the heap.
    #[inline]
    pub fn bytes(&self) -> &[u8] {
        self.arena.bytes()
    }

    #[inline]
    pub fn arena(&self) -> &Arena {
        &self.arena
    }

    #[inline]
    pub fn arena_mut(&mut self) -> &mut Arena {
        &mut self.arena
    }

    /// Allocates a zeroed cell of at least `size` bytes and writes its header.
    pub fn alloc_cell(&mut self, kind: CellKind, size: u32) -> Result<u32, HeapError> {
        let size = align8(size.max(kind.min_size()));
        let off = self.arena.alloc(size)?;
        if self.arena.len() >= self.gc_threshold {
            self.gc_pending = true;
        }
        self.arena.set_untracked(off, Header::KIND, kind as u8);
        self.arena.set_untracked(off, Header::SIZE, size);
        Ok(off)
    }

    /// Like [`Heap::alloc_cell`], with `data` written at offset `at` of the cell instead
    /// of zeros (written once; see [`Arena::alloc_init`]).
    pub fn alloc_cell_init(
        &mut self,
        kind: CellKind,
        size: u32,
        at: u32,
        data: &[u8],
    ) -> Result<u32, HeapError> {
        let size = align8(size.max(kind.min_size()));
        let off = self.arena.alloc_init(size, at, data)?;
        if self.arena.len() >= self.gc_threshold {
            self.gc_pending = true;
        }
        self.arena.set_untracked(off, Header::KIND, kind as u8);
        self.arena.set_untracked(off, Header::SIZE, size);
        Ok(off)
    }

    /// Like [`Heap::alloc_cell_init`] with `n` bytes of data from an iterator (see
    /// [`Arena::alloc_from_iter`]).
    pub fn alloc_cell_from_iter(
        &mut self,
        kind: CellKind,
        size: u32,
        at: u32,
        n: u32,
        data: impl Iterator<Item = u8>,
    ) -> Result<u32, HeapError> {
        let size = align8(size.max(kind.min_size()));
        let off = self.arena.alloc_from_iter(size, at, n, data)?;
        if self.arena.len() >= self.gc_threshold {
            self.gc_pending = true;
        }
        self.arena.set_untracked(off, Header::KIND, kind as u8);
        self.arena.set_untracked(off, Header::SIZE, size);
        Ok(off)
    }

    /// The kind of the cell at `off`. Panics if the header is invalid.
    #[inline]
    pub fn cell_kind(&self, off: u32) -> CellKind {
        let k = self.arena.get(off, Header::KIND);
        CellKind::from_u8(k).unwrap_or_else(|| fail::bad_cell_kind(k, off))
    }

    #[inline]
    pub fn cell_size(&self, off: u32) -> u32 {
        self.arena.get(off, Header::SIZE)
    }

    #[inline]
    pub fn cell_flags(&self, off: u32) -> u16 {
        self.arena.get(off, Header::FLAGS)
    }

    #[inline]
    pub fn set_cell_flags(&mut self, off: u32, flags: u16) {
        self.arena.set(off, Header::FLAGS, flags)
    }

    /// The realm root value (`undefined` until a realm is created).
    pub fn realm(&self) -> Value {
        self.arena.get(0, Roots::REALM)
    }

    pub fn set_realm(&mut self, v: Value) {
        self.arena.set(0, Roots::REALM, v)
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod gc_tests;

#[cfg(test)]
mod append_tests;
#[cfg(test)]
mod hash_tests;
#[cfg(test)]
mod shape_tests;
#[cfg(test)]
mod slice_tests;
