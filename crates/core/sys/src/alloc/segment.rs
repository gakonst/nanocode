use core::cell::Cell;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicIsize, AtomicPtr, AtomicU64, AtomicUsize, Ordering};

use super::class::SMALL_BYTES_MAX;
use super::os::{self, check};

pub const SEGMENT_BYTES: usize = 4 << 20;
pub const SLICE_BYTES: usize = 64 << 10;
pub const SLICES: usize = SEGMENT_BYTES / SLICE_BYTES;
pub const HEADER_SLICES: usize = 1;
pub const SLICES_FREE_ALL: u64 = u64::MAX << HEADER_SLICES;
pub const OWNER_ABANDONED: u64 = u64::MAX;
pub const KIND_SMALL: usize = 1;
pub const KIND_LARGE: usize = 2;
const LARGE_HEADER_BYTES: usize = 4096;
const ABANDONED_SLOTS: usize = 1024;
const SEGMENT_CACHE_SLOTS: usize = 16;
const LARGE_CACHE_SLOTS: usize = 16;
const LARGE_CACHE_BYTES_MAX: usize = 8 << 20;
const LARGE_CACHE_FIT: usize = 2;
const LARGE_UNITS_LIMIT: usize = SEGMENT_BYTES;

pub static ABANDONED: Slots<ABANDONED_SLOTS> = Slots::new();
static SEGMENT_CACHE: Slots<SEGMENT_CACHE_SLOTS> = Slots::new();
static LARGE_CACHE: Slots<LARGE_CACHE_SLOTS> = Slots::new();
static LARGE_CACHED_BYTES: AtomicUsize = AtomicUsize::new(0);

pub struct Block {
    pub next: *mut Self,
}

pub struct Page {
    pub free: Cell<*mut Block>,
    pub thread_free: AtomicPtr<Block>,
    pub bump: Cell<usize>,
    pub end: Cell<usize>,
    pub block_bytes: Cell<usize>,
    pub used: Cell<usize>,
    pub head: Cell<usize>,
    pub span: Cell<usize>,
    pub class: Cell<usize>,
    pub next: Cell<*mut Self>,
    pub prev: Cell<*mut Self>,
}

#[repr(C)]
pub struct Segment {
    pub kind: Cell<usize>,
    pub mapping_bytes: Cell<usize>,
    pub owner: AtomicU64,
    pub free_slices: Cell<u64>,
    pub dirty_slices: Cell<u64>,
    pub next: Cell<*mut Self>,
    pub prev: Cell<*mut Self>,
    pub pages: [Page; SLICES],
}

const _: () = {
    assert!(size_of::<Segment>() <= HEADER_SLICES * SLICE_BYTES);
    assert!(SLICES == u64::BITS as usize);
    assert!(LARGE_HEADER_BYTES >= 2 * size_of::<usize>());
};

pub struct Slots<const N: usize> {
    slots: [AtomicUsize; N],
    filled: AtomicIsize,
}

impl<const N: usize> Slots<N> {
    const fn new() -> Self {
        Self {
            slots: [const { AtomicUsize::new(0) }; N],
            filled: AtomicIsize::new(0),
        }
    }

    pub fn put(&self, value: usize) -> bool {
        check(value != 0, "a slot holds a nonzero value");
        for slot in &self.slots {
            if slot.load(Ordering::Relaxed) == 0
                && slot
                    .compare_exchange(0, value, Ordering::Release, Ordering::Relaxed)
                    .is_ok()
            {
                self.filled.fetch_add(1, Ordering::Release);
                return true;
            }
        }
        false
    }

    pub fn take(&self, fits: impl Fn(usize) -> bool) -> usize {
        if self.filled.load(Ordering::Acquire) <= 0 {
            return 0;
        }
        for slot in &self.slots {
            let value = slot.load(Ordering::Relaxed);
            if value != 0
                && fits(value)
                && slot
                    .compare_exchange(value, 0, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
            {
                self.filled.fetch_sub(1, Ordering::Relaxed);
                return value;
            }
        }
        0
    }
}

impl Page {
    #[inline]
    pub fn take(&self) -> *mut u8 {
        let block = self.free.get();
        if !block.is_null() {
            self.free.set(unsafe { (*block).next });
            self.used.set(self.used.get() + 1);
            return block.cast();
        }
        let bump = self.bump.get();
        let next = bump + self.block_bytes.get();
        if next <= self.end.get() {
            self.bump.set(next);
            self.used.set(self.used.get() + 1);
            return bump as *mut u8;
        }
        null_mut()
    }

    #[inline]
    pub unsafe fn free_local(&self, block: *mut Block) {
        let used = self.used.get();
        check(used > 0, "a block freed by its page's owner was handed out");
        unsafe { (*block).next = self.free.get() };
        self.free.set(block);
        self.used.set(used - 1);
    }

    #[inline]
    pub unsafe fn free_remote(&self, block: *mut Block) {
        let mut head = self.thread_free.load(Ordering::Relaxed);
        loop {
            unsafe { (*block).next = head };
            match self.thread_free.compare_exchange_weak(
                head,
                block,
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                Ok(_) => return,
                Err(seen) => head = seen,
            }
        }
    }

    pub const fn capacity(&self) -> usize {
        self.span.get() * SLICE_BYTES / self.block_bytes.get()
    }

    pub fn collect(&self) {
        if self.thread_free.load(Ordering::Relaxed).is_null() {
            return;
        }
        let list = self.thread_free.swap(null_mut(), Ordering::Acquire);
        if list.is_null() {
            return;
        }
        let capacity = self.capacity();
        let mut tail = list;
        let mut count = 1;
        unsafe {
            while !(*tail).next.is_null() {
                tail = (*tail).next;
                count += 1;
                check(
                    count <= capacity,
                    "a page's freed list is no longer than its blocks",
                );
            }
            (*tail).next = self.free.get();
        }
        self.free.set(list);
        let used = self.used.get();
        check(
            count <= used,
            "every block freed by another thread was handed out",
        );
        self.used.set(used - count);
    }

    pub const fn span_mask(&self) -> u64 {
        span_mask(self.head.get(), self.span.get())
    }
}

pub const fn span_mask(start: usize, span: usize) -> u64 {
    ((1u64 << span) - 1) << start
}

pub const fn find_run(free: u64, span: usize) -> Option<usize> {
    let mut runs = free;
    let mut shift = 1;
    while shift < span {
        runs &= free >> shift;
        shift += 1;
    }
    if runs == 0 {
        None
    } else {
        Some(runs.trailing_zeros() as usize)
    }
}

#[inline]
pub fn segment_of(pointer: *mut u8) -> *mut Segment {
    (pointer.addr() & !(SEGMENT_BYTES - 1)) as *mut Segment
}

#[inline]
pub unsafe fn page_of(segment: *mut Segment, pointer: *mut u8) -> *mut Page {
    let slice = (pointer.addr() - segment.addr()) / SLICE_BYTES;
    unsafe {
        let head = (*segment).pages[slice].head.get();
        (&raw mut (*segment).pages[head]).cast()
    }
}

pub unsafe fn acquire(owner: u64) -> *mut Segment {
    let mut base = SEGMENT_CACHE.take(|_| true);
    if base == 0 {
        base = unsafe { os::map_aligned(SEGMENT_BYTES, SEGMENT_BYTES) };
        if base == 0 {
            return null_mut();
        }
    }
    let segment = base as *mut Segment;
    unsafe {
        (*segment).kind.set(KIND_SMALL);
        (*segment).mapping_bytes.set(SEGMENT_BYTES);
        (*segment).owner.store(owner, Ordering::Relaxed);
        (*segment).free_slices.set(SLICES_FREE_ALL);
        (*segment).dirty_slices.set(0);
        (*segment).next.set(null_mut());
        (*segment).prev.set(null_mut());
        for page in &(*segment).pages {
            check(
                page.thread_free.load(Ordering::Relaxed).is_null(),
                "a reused segment has no pending frees",
            );
            page.block_bytes.set(0);
            page.used.set(0);
            page.free.set(null_mut());
        }
    }
    segment
}

pub unsafe fn purge_dirty(segment: *mut Segment) -> u64 {
    unsafe {
        let free = (*segment).free_slices.get();
        let dirty = (*segment).dirty_slices.get() & free;
        let mut rest = dirty;
        while rest != 0 {
            let start = rest.trailing_zeros() as usize;
            let run = (!(rest >> start)).trailing_zeros() as usize;
            os::purge(segment.addr() + start * SLICE_BYTES, run * SLICE_BYTES);
            rest &= !span_mask(start, run);
        }
        (*segment)
            .dirty_slices
            .set((*segment).dirty_slices.get() & !dirty);
        dirty
    }
}

pub unsafe fn release(segment: *mut Segment) {
    unsafe {
        check(
            (*segment).free_slices.get() == SLICES_FREE_ALL,
            "a released segment holds no pages",
        );
        purge_dirty(segment);
        if !SEGMENT_CACHE.put(segment.addr()) {
            os::unmap(segment.addr(), SEGMENT_BYTES);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Contents {
    Zero,
    Dirty,
}

pub unsafe fn large_allocate(bytes: usize, align: usize) -> *mut u8 {
    unsafe { large_allocate_with(bytes, align).0 }
}

pub unsafe fn large_allocate_with(bytes: usize, align: usize) -> (*mut u8, Contents) {
    let offset = align.max(LARGE_HEADER_BYTES);
    if offset >= SEGMENT_BYTES {
        return (null_mut(), Contents::Dirty);
    }
    let page = os::page_bytes();
    let Some(need) = offset
        .checked_add(bytes)
        .and_then(|end| end.checked_next_multiple_of(page))
    else {
        return (null_mut(), Contents::Dirty);
    };
    let units = need / page;
    let cached = if units < LARGE_UNITS_LIMIT {
        LARGE_CACHE.take(|value| {
            let have = value & (SEGMENT_BYTES - 1);
            have >= units && have <= units * LARGE_CACHE_FIT
        })
    } else {
        0
    };
    let (base, mapping_bytes, contents) = if cached == 0 {
        (
            unsafe { os::map_aligned(need, SEGMENT_BYTES) },
            need,
            Contents::Zero,
        )
    } else {
        let have = (cached & (SEGMENT_BYTES - 1)) * page;
        LARGE_CACHED_BYTES.fetch_sub(have, Ordering::Relaxed);
        (cached & !(SEGMENT_BYTES - 1), have, Contents::Dirty)
    };
    if base == 0 {
        return (null_mut(), Contents::Dirty);
    }
    let segment = base as *mut Segment;
    unsafe {
        (*segment).kind.set(KIND_LARGE);
        (*segment).mapping_bytes.set(mapping_bytes);
    }
    ((base + offset) as *mut u8, contents)
}

pub unsafe fn large_free(segment: *mut Segment) {
    let bytes = unsafe { (*segment).mapping_bytes.get() };
    let units = bytes / os::page_bytes();
    if units < LARGE_UNITS_LIMIT {
        let before = LARGE_CACHED_BYTES.fetch_add(bytes, Ordering::Relaxed);
        if before + bytes <= LARGE_CACHE_BYTES_MAX && LARGE_CACHE.put(segment.addr() | units) {
            return;
        }
        LARGE_CACHED_BYTES.fetch_sub(bytes, Ordering::Relaxed);
    }
    unsafe { os::unmap(segment.addr(), bytes) };
}

pub unsafe fn large_resize(segment: *mut Segment, pointer: *mut u8, bytes_new: usize) -> bool {
    let offset = pointer.addr() - segment.addr();
    let mapping_bytes = unsafe { (*segment).mapping_bytes.get() };
    let capacity = mapping_bytes - offset;
    if bytes_new <= capacity {
        return bytes_new > capacity / LARGE_CACHE_FIT && bytes_new > SMALL_BYTES_MAX;
    }
    let Some(need) = offset
        .checked_add(bytes_new)
        .and_then(|end| end.checked_next_multiple_of(os::page_bytes()))
    else {
        return false;
    };
    if unsafe { os::grow_in_place(segment.addr(), mapping_bytes, need) } {
        unsafe { (*segment).mapping_bytes.set(need) };
        return true;
    }
    false
}
