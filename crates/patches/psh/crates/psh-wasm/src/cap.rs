//! The module's memory budget: a global allocator that counts live bytes and refuses an
//! allocation that would pass the limit the JS side set (`psh_memory_limit`). A refused
//! allocation aborts the module, as any failed allocation does; the JS side then finds
//! `psh_memory_exceeded` set, reports the budget and loads a fresh instance. Without
//! this a script that doubles a string until memory runs out takes the whole JS host's
//! memory with it.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::export;

static USED: AtomicUsize = AtomicUsize::new(0);
static LIMIT: AtomicUsize = AtomicUsize::new(usize::MAX);
static EXCEEDED: AtomicBool = AtomicBool::new(false);

pub struct Capped;

/// Counts `size` more live bytes, unless that passes the limit.
fn take(size: usize) -> bool {
    let used = USED.fetch_add(size, Ordering::Relaxed);
    if used.saturating_add(size) > LIMIT.load(Ordering::Relaxed) {
        USED.fetch_sub(size, Ordering::Relaxed);
        EXCEEDED.store(true, Ordering::Relaxed);
        return false;
    }
    true
}

// SAFETY: every call is forwarded to `System` with the caller's layout and pointer
// unchanged; this type only counts sizes around it and answers null, which
// `GlobalAlloc` allows for a failed allocation, when the budget is spent.
#[allow(unsafe_code)]
unsafe impl GlobalAlloc for Capped {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !take(layout.size()) {
            return std::ptr::null_mut();
        }
        // SAFETY: the caller's contract for `alloc`, passed on unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if !take(layout.size()) {
            return std::ptr::null_mut();
        }
        // SAFETY: the caller's contract for `alloc_zeroed`, passed on unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        USED.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: the caller's contract for `dealloc`, passed on unchanged.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let old = layout.size();
        if new_size > old && !take(new_size - old) {
            return std::ptr::null_mut();
        }
        // SAFETY: the caller's contract for `realloc`, passed on unchanged.
        let moved = unsafe { System.realloc(ptr, layout, new_size) };
        if moved.is_null() {
            if new_size > old {
                USED.fetch_sub(new_size - old, Ordering::Relaxed);
            }
        } else if new_size < old {
            USED.fetch_sub(old - new_size, Ordering::Relaxed);
        }
        moved
    }
}

export! {
    /// Sets the most live bytes the module may hold (0: no limit) and clears the mark
    /// of an earlier refusal.
    fn psh_memory_limit(bytes: usize) {
        LIMIT.store(if bytes == 0 { usize::MAX } else { bytes }, Ordering::Relaxed);
        EXCEEDED.store(false, Ordering::Relaxed);
    }
}

export! {
    /// 1 when an allocation was refused for the limit since it was set.
    fn psh_memory_exceeded() -> u32 {
        u32::from(EXCEEDED.load(Ordering::Relaxed))
    }
}

export! {
    /// The live bytes the module holds now.
    fn psh_memory_used() -> usize {
        USED.load(Ordering::Relaxed)
    }
}
