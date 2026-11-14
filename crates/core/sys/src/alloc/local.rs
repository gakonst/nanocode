use core::alloc::Layout;
use core::cell::Cell;
use core::ffi::c_void;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicU64, Ordering};

use super::class::{BLOCK_ALIGN_BYTES, CLASS_BYTES, CLASS_SLICES, CLASSES, class_for, small_bytes};
use super::os::{self, check};
use super::segment::{
    self, ABANDONED, Block, HEADER_SLICES, KIND_LARGE, OWNER_ABANDONED, Page, SLICE_BYTES, SLICES,
    SLICES_FREE_ALL, Segment, find_run, page_of, segment_of, span_mask,
};

const PAGE_SCAN_MAX: usize = 8;
const COLLECT_PAGES_MIN: usize = 32;
const COLLECT_LIVE_DIVISOR: usize = 4;
const DIRTY_SLICES_MAX: usize = 64;
const SHRINK_IN_PLACE_DIVISOR: usize = 2;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    pub static THREAD: State = const { State::new() };
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Watched,
    Unwatched,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Heads {
    Kept,
    Retired,
}

pub struct State {
    id: Cell<u64>,
    heads: [Cell<*mut Page>; CLASSES],
    tails: [Cell<*mut Page>; CLASSES],
    segments: Cell<*mut Segment>,
    segments_tail: Cell<*mut Segment>,
    pages_live: Cell<usize>,
    pages_since_collect: Cell<usize>,
    slices_dirty: Cell<usize>,
}

pub struct Census {
    pub segments: usize,
    pub pages: usize,
    pub blocks_used: usize,
}

unsafe extern "C" fn thread_exited(state: *mut c_void) {
    let state = state.cast_const().cast::<State>();
    unsafe { (*state).abandon() };
}

impl State {
    pub const fn new() -> Self {
        Self {
            id: Cell::new(0),
            heads: [const { Cell::new(null_mut()) }; CLASSES],
            tails: [const { Cell::new(null_mut()) }; CLASSES],
            segments: Cell::new(null_mut()),
            segments_tail: Cell::new(null_mut()),
            pages_live: Cell::new(0),
            pages_since_collect: Cell::new(0),
            slices_dirty: Cell::new(0),
        }
    }

    #[inline]
    pub unsafe fn allocate(&self, layout: Layout, exit: Exit) -> *mut u8 {
        let Some(bytes) = small_bytes(layout.size(), layout.align()) else {
            return unsafe { segment::large_allocate(layout.size(), layout.align()) };
        };
        let class = class_for(bytes);
        let page = self.heads[class].get();
        if !page.is_null() {
            let block = unsafe { (*page).take() };
            if !block.is_null() {
                return block;
            }
        }
        unsafe { self.allocate_slow(class, exit) }
    }

    #[inline]
    pub unsafe fn allocate_zeroed(&self, layout: Layout, exit: Exit) -> *mut u8 {
        if small_bytes(layout.size(), layout.align()).is_none() {
            let (pointer, contents) =
                unsafe { segment::large_allocate_with(layout.size(), layout.align()) };
            if !pointer.is_null() && contents == segment::Contents::Dirty {
                unsafe { pointer.write_bytes(0, layout.size()) };
            }
            return pointer;
        }
        let pointer = unsafe { self.allocate(layout, exit) };
        if !pointer.is_null() {
            unsafe { pointer.write_bytes(0, layout.size()) };
        }
        pointer
    }

    #[inline]
    pub unsafe fn deallocate(&self, pointer: *mut u8) {
        let segment = segment_of(pointer);
        unsafe {
            if (*segment).kind.get() == KIND_LARGE {
                segment::large_free(segment);
                return;
            }
            let page = page_of(segment, pointer);
            let block = pointer.cast::<Block>();
            if (*segment).owner.load(Ordering::Relaxed) != self.id.get() {
                (*page).free_remote(block);
                return;
            }
            (*page).free_local(block);
            if (*page).used.get() == 0 && self.heads[(*page).class.get()].get() != page {
                self.retire(page);
            }
        }
    }

    #[inline]
    pub unsafe fn reallocate(
        &self,
        pointer: *mut u8,
        layout: Layout,
        bytes_new: usize,
        exit: Exit,
    ) -> *mut u8 {
        if layout.align() <= BLOCK_ALIGN_BYTES && unsafe { Self::fits_in_place(pointer, bytes_new) }
        {
            return pointer;
        }
        let Ok(layout_new) = Layout::from_size_align(bytes_new, layout.align()) else {
            return null_mut();
        };
        let moved = unsafe { self.allocate(layout_new, exit) };
        if !moved.is_null() {
            unsafe {
                core::ptr::copy_nonoverlapping(pointer, moved, layout.size().min(bytes_new));
                self.deallocate(pointer);
            }
        }
        moved
    }

    #[inline]
    unsafe fn fits_in_place(pointer: *mut u8, bytes_new: usize) -> bool {
        let segment = segment_of(pointer);
        unsafe {
            if (*segment).kind.get() == KIND_LARGE {
                return segment::large_resize(segment, pointer, bytes_new);
            }
            let bytes = (*page_of(segment, pointer)).block_bytes.get();
            bytes_new <= bytes
                && CLASS_BYTES[class_for(bytes_new)] * SHRINK_IN_PLACE_DIVISOR > bytes
        }
    }

    #[cold]
    unsafe fn allocate_slow(&self, class: usize, exit: Exit) -> *mut u8 {
        if self.id.get() == 0 {
            self.start(exit);
        }
        if self.pages_since_collect.get()
            >= COLLECT_PAGES_MIN.max(self.pages_live.get() / COLLECT_LIVE_DIVISOR)
        {
            self.collect_all(Heads::Kept);
        }
        for _ in 0..PAGE_SCAN_MAX {
            let page = self.heads[class].get();
            if page.is_null() {
                break;
            }
            unsafe {
                (*page).collect();
                let block = (*page).take();
                if !block.is_null() {
                    return block;
                }
                if (*page).next.get().is_null() {
                    break;
                }
                self.unlink(page);
                self.push_back(page);
            }
        }
        let page = unsafe { self.page_new(class) };
        if page.is_null() {
            return null_mut();
        }
        unsafe {
            self.push_front(page);
            (*page).take()
        }
    }

    fn start(&self, exit: Exit) {
        self.id.set(NEXT_ID.fetch_add(1, Ordering::Relaxed));
        if exit == Exit::Watched {
            os::watch_exit(core::ptr::from_ref(self).cast(), thread_exited);
        }
    }

    unsafe fn page_new(&self, class: usize) -> *mut Page {
        self.pages_since_collect
            .set(self.pages_since_collect.get() + 1);
        let span = CLASS_SLICES[class];
        let mut segment = self.segments.get();
        while !segment.is_null() {
            let free = unsafe { (*segment).free_slices.get() };
            if free == 0 {
                break;
            }
            if let Some(start) = find_run(free, span) {
                return unsafe { self.make_page(segment, start, class) };
            }
            segment = unsafe { (*segment).next.get() };
        }
        let adopted = ABANDONED.take(|_| true);
        if adopted != 0 {
            let segment = adopted as *mut Segment;
            if unsafe { self.adopt(segment) }
                && let Some(start) = find_run(unsafe { (*segment).free_slices.get() }, span)
            {
                return unsafe { self.make_page(segment, start, class) };
            }
        }
        let segment = unsafe { segment::acquire(self.id.get()) };
        if segment.is_null() {
            return null_mut();
        }
        self.link_segment_front(segment);
        unsafe { self.make_page(segment, HEADER_SLICES, class) }
    }

    unsafe fn make_page(&self, segment: *mut Segment, start: usize, class: usize) -> *mut Page {
        let span = CLASS_SLICES[class];
        let mask = span_mask(start, span);
        unsafe {
            let seg = &*segment;
            check(
                seg.free_slices.get() & mask == mask,
                "a new page takes free slices",
            );
            let dirty = seg.dirty_slices.get();
            self.slices_dirty
                .set(self.slices_dirty.get() - (dirty & mask).count_ones() as usize);
            seg.dirty_slices.set(dirty & !mask);
            seg.free_slices.set(seg.free_slices.get() & !mask);
            for page in &seg.pages[start..start + span] {
                page.head.set(start);
            }
            let page = &seg.pages[start];
            check(
                page.thread_free.load(Ordering::Relaxed).is_null(),
                "a new page has no pending frees",
            );
            let bytes = CLASS_BYTES[class];
            let data = segment.addr() + start * SLICE_BYTES;
            page.free.set(null_mut());
            page.bump.set(data);
            page.end.set(data + (span * SLICE_BYTES / bytes) * bytes);
            page.block_bytes.set(bytes);
            page.used.set(0);
            page.span.set(span);
            page.class.set(class);
            page.next.set(null_mut());
            page.prev.set(null_mut());
            self.pages_live.set(self.pages_live.get() + 1);
            if seg.free_slices.get() == 0 {
                self.unlink_segment(segment);
                self.link_segment_back(segment);
            }
            core::ptr::from_ref(page).cast_mut()
        }
    }

    unsafe fn retire(&self, page: *mut Page) {
        unsafe {
            self.unlink(page);
            self.free_page(page);
        }
    }

    unsafe fn free_page(&self, page: *mut Page) {
        let segment = segment_of(page.cast());
        unsafe {
            self.free_slices(page);
            self.unlink_segment(segment);
            if !self.release_if_empty(segment) {
                self.link_segment_front(segment);
            }
        }
        if self.slices_dirty.get() > DIRTY_SLICES_MAX {
            self.purge_all();
        }
    }

    unsafe fn free_slices(&self, page: *mut Page) {
        unsafe {
            let seg = &*segment_of(page.cast());
            let mask = (*page).span_mask();
            check(
                seg.free_slices.get() & mask == 0,
                "a retired page held its slices",
            );
            let dirty = seg.dirty_slices.get();
            self.slices_dirty
                .set(self.slices_dirty.get() + (mask & !dirty).count_ones() as usize);
            seg.dirty_slices.set(dirty | mask);
            seg.free_slices.set(seg.free_slices.get() | mask);
            (*page).block_bytes.set(0);
        }
        self.pages_live.set(self.pages_live.get() - 1);
    }

    unsafe fn release_if_empty(&self, unlinked: *mut Segment) -> bool {
        unsafe {
            if (*unlinked).free_slices.get() != SLICES_FREE_ALL || self.segments.get().is_null() {
                return false;
            }
            let dirty = (*unlinked).dirty_slices.get().count_ones() as usize;
            self.slices_dirty.set(self.slices_dirty.get() - dirty);
            segment::release(unlinked);
        }
        true
    }

    fn purge_all(&self) {
        let mut segment = self.segments.get();
        while !segment.is_null() {
            unsafe {
                segment::purge_dirty(segment);
                segment = (*segment).next.get();
            }
        }
        self.slices_dirty.set(0);
    }

    fn collect_all(&self, heads: Heads) {
        self.pages_since_collect.set(0);
        for class in 0..CLASSES {
            let mut page = self.heads[class].get();
            while !page.is_null() {
                unsafe {
                    let next = (*page).next.get();
                    (*page).collect();
                    if (*page).used.get() == 0
                        && (heads == Heads::Retired || self.heads[class].get() != page)
                    {
                        self.retire(page);
                    }
                    page = next;
                }
            }
        }
    }

    pub fn abandon(&self) {
        if self.id.get() == 0 {
            return;
        }
        self.collect_all(Heads::Retired);
        self.purge_all();
        let mut segment = self.segments.get();
        while !segment.is_null() {
            unsafe {
                let next = (*segment).next.get();
                if (*segment).free_slices.get() == SLICES_FREE_ALL {
                    segment::release(segment);
                } else {
                    (*segment).owner.store(OWNER_ABANDONED, Ordering::Release);
                    ABANDONED.put(segment.addr());
                }
                segment = next;
            }
        }
        for class in 0..CLASSES {
            self.heads[class].set(null_mut());
            self.tails[class].set(null_mut());
        }
        self.segments.set(null_mut());
        self.segments_tail.set(null_mut());
        self.pages_live.set(0);
        self.pages_since_collect.set(0);
        self.slices_dirty.set(0);
        self.id.set(0);
    }

    pub fn adopt_abandoned(&self) -> usize {
        if self.id.get() == 0 {
            self.start(Exit::Unwatched);
        }
        let mut adopted = 0;
        loop {
            let taken = ABANDONED.take(|_| true);
            if taken == 0 {
                return adopted;
            }
            unsafe { self.adopt(taken as *mut Segment) };
            adopted += 1;
        }
    }

    unsafe fn adopt(&self, segment: *mut Segment) -> bool {
        unsafe {
            let seg = &*segment;
            seg.owner.store(self.id.get(), Ordering::Relaxed);
            let dirty_free = seg.dirty_slices.get() & seg.free_slices.get();
            self.slices_dirty
                .set(self.slices_dirty.get() + dirty_free.count_ones() as usize);
            let mut slice = HEADER_SLICES;
            while slice < SLICES {
                let page = &seg.pages[slice];
                if seg.free_slices.get() & (1 << slice) != 0
                    || page.block_bytes.get() == 0
                    || page.head.get() != slice
                {
                    slice += 1;
                    continue;
                }
                slice += page.span.get();
                let page = core::ptr::from_ref(page).cast_mut();
                (*page).collect();
                self.pages_live.set(self.pages_live.get() + 1);
                if (*page).used.get() == 0 {
                    self.free_slices(page);
                } else {
                    self.push_back(page);
                }
            }
            if self.release_if_empty(segment) {
                return false;
            }
            if seg.free_slices.get() == 0 {
                self.link_segment_back(segment);
            } else {
                self.link_segment_front(segment);
            }
        }
        true
    }

    pub fn census(&self) -> Census {
        let mut census = Census {
            segments: 0,
            pages: 0,
            blocks_used: 0,
        };
        let mut segment = self.segments.get();
        while !segment.is_null() {
            let seg = unsafe { &*segment };
            assert_eq!(
                seg.owner.load(Ordering::Relaxed),
                self.id.get(),
                "a heap's segments are its own"
            );
            census.segments += 1;
            let mut taken = 0u64;
            let mut slice = HEADER_SLICES;
            while slice < SLICES {
                let page = &seg.pages[slice];
                if seg.free_slices.get() & (1 << slice) != 0 {
                    slice += 1;
                    continue;
                }
                assert_eq!(
                    page.head.get(),
                    slice,
                    "a taken slice outside a page run is a page's head"
                );
                assert_ne!(page.block_bytes.get(), 0, "a taken slice heads a live page");
                page.collect();
                let span = page.span.get();
                for inner in &seg.pages[slice..slice + span] {
                    assert_eq!(
                        inner.head.get(),
                        slice,
                        "each slice of a page names its head"
                    );
                }
                taken |= span_mask(slice, span);
                census.pages += 1;
                census.blocks_used += Self::page_census(page);
                slice += span;
            }
            assert_eq!(
                taken | seg.free_slices.get(),
                SLICES_FREE_ALL,
                "every slice is free or in one page"
            );
            assert_eq!(
                taken & seg.free_slices.get(),
                0,
                "no slice is both free and in a page"
            );
            segment = seg.next.get();
        }
        assert_eq!(
            census.pages,
            self.pages_live.get(),
            "the heap counts its live pages"
        );
        let mut queued = 0;
        for class in 0..CLASSES {
            let mut page = self.heads[class].get();
            let mut previous = null_mut();
            while !page.is_null() {
                let current = unsafe { &*page };
                assert_eq!(
                    current.class.get(),
                    class,
                    "a page is queued under its class"
                );
                assert_eq!(current.prev.get(), previous, "a queue's links agree");
                queued += 1;
                previous = page;
                page = current.next.get();
            }
            assert_eq!(
                self.tails[class].get(),
                previous,
                "a queue's tail is its last page"
            );
        }
        assert_eq!(queued, census.pages, "every live page is queued once");
        census
    }

    fn page_census(page: &Page) -> usize {
        let capacity = page.capacity();
        let mut free = 0;
        let mut block = page.free.get();
        while !block.is_null() {
            free += 1;
            assert!(
                free <= capacity,
                "a page's free list is no longer than its blocks"
            );
            block = unsafe { (*block).next };
        }
        let unbumped = (page.end.get() - page.bump.get()) / page.block_bytes.get();
        assert_eq!(
            page.used.get() + free + unbumped,
            capacity,
            "a page's blocks are used, free or not yet carved"
        );
        page.used.get()
    }

    unsafe fn push_front(&self, page: *mut Page) {
        unsafe {
            let class = (*page).class.get();
            let head = self.heads[class].get();
            (*page).prev.set(null_mut());
            (*page).next.set(head);
            if head.is_null() {
                self.tails[class].set(page);
            } else {
                (*head).prev.set(page);
            }
            self.heads[class].set(page);
        }
    }

    unsafe fn push_back(&self, page: *mut Page) {
        unsafe {
            let class = (*page).class.get();
            let tail = self.tails[class].get();
            (*page).next.set(null_mut());
            (*page).prev.set(tail);
            if tail.is_null() {
                self.heads[class].set(page);
            } else {
                (*tail).next.set(page);
            }
            self.tails[class].set(page);
        }
    }

    unsafe fn unlink(&self, page: *mut Page) {
        unsafe {
            let class = (*page).class.get();
            let next = (*page).next.get();
            let prev = (*page).prev.get();
            if prev.is_null() {
                self.heads[class].set(next);
            } else {
                (*prev).next.set(next);
            }
            if next.is_null() {
                self.tails[class].set(prev);
            } else {
                (*next).prev.set(prev);
            }
            (*page).next.set(null_mut());
            (*page).prev.set(null_mut());
        }
    }

    fn link_segment_front(&self, segment: *mut Segment) {
        let head = self.segments.get();
        unsafe {
            (*segment).prev.set(null_mut());
            (*segment).next.set(head);
            if head.is_null() {
                self.segments_tail.set(segment);
            } else {
                (*head).prev.set(segment);
            }
        }
        self.segments.set(segment);
    }

    fn link_segment_back(&self, segment: *mut Segment) {
        let tail = self.segments_tail.get();
        unsafe {
            (*segment).next.set(null_mut());
            (*segment).prev.set(tail);
            if tail.is_null() {
                self.segments.set(segment);
            } else {
                (*tail).next.set(segment);
            }
        }
        self.segments_tail.set(segment);
    }

    fn unlink_segment(&self, segment: *mut Segment) {
        unsafe {
            let next = (*segment).next.get();
            let prev = (*segment).prev.get();
            if prev.is_null() {
                self.segments.set(next);
            } else {
                (*prev).next.set(next);
            }
            if next.is_null() {
                self.segments_tail.set(prev);
            } else {
                (*next).prev.set(prev);
            }
            (*segment).next.set(null_mut());
            (*segment).prev.set(null_mut());
        }
    }
}
