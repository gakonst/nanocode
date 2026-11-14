#[cfg(unix)]
mod class;
#[cfg(unix)]
mod local;
#[cfg(unix)]
mod os;
#[cfg(unix)]
mod segment;

use core::alloc::{GlobalAlloc, Layout};
#[cfg(unix)]
use core::ptr::NonNull;

#[cfg(unix)]
pub use local::Census;

pub struct Heap;

#[cfg(unix)]
unsafe impl GlobalAlloc for Heap {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        local::THREAD.with(|state| unsafe { state.allocate(layout, local::Exit::Watched) })
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        local::THREAD.with(|state| unsafe { state.allocate_zeroed(layout, local::Exit::Watched) })
    }

    #[inline]
    unsafe fn dealloc(&self, pointer: *mut u8, _layout: Layout) {
        local::THREAD.with(|state| unsafe { state.deallocate(pointer) });
    }

    #[inline]
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, bytes_new: usize) -> *mut u8 {
        local::THREAD.with(|state| unsafe {
            state.reallocate(pointer, layout, bytes_new, local::Exit::Watched)
        })
    }
}

#[cfg(not(unix))]
unsafe impl GlobalAlloc for Heap {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { std::alloc::System.alloc(layout) }
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        unsafe { std::alloc::System.alloc_zeroed(layout) }
    }

    #[inline]
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { std::alloc::System.dealloc(pointer, layout) }
    }

    #[inline]
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, bytes_new: usize) -> *mut u8 {
        unsafe { std::alloc::System.realloc(pointer, layout, bytes_new) }
    }
}

#[cfg(unix)]
pub struct Local {
    state: local::State,
}

#[cfg(unix)]
impl Local {
    pub const fn new() -> Self {
        Self {
            state: local::State::new(),
        }
    }

    pub fn allocate(&self, layout: Layout) -> Option<Allocation> {
        let pointer = unsafe { self.state.allocate(layout, local::Exit::Unwatched) };
        NonNull::new(pointer).map(|pointer| Allocation { pointer, layout })
    }

    pub fn deallocate(&self, allocation: Allocation) {
        unsafe { self.state.deallocate(allocation.pointer.as_ptr()) }
    }

    pub fn reallocate(
        &self,
        allocation: Allocation,
        bytes_new: usize,
    ) -> Result<Allocation, Allocation> {
        let Ok(layout) = Layout::from_size_align(bytes_new, allocation.layout.align()) else {
            return Err(allocation);
        };
        let pointer = unsafe {
            self.state.reallocate(
                allocation.pointer.as_ptr(),
                allocation.layout,
                bytes_new,
                local::Exit::Unwatched,
            )
        };
        NonNull::new(pointer)
            .map(|pointer| Allocation { pointer, layout })
            .ok_or(allocation)
    }

    pub fn abandon(&self) {
        self.state.abandon();
    }

    pub fn adopt_abandoned(&self) -> usize {
        self.state.adopt_abandoned()
    }

    pub fn census(&self) -> Census {
        self.state.census()
    }
}

#[cfg(unix)]
pub struct Allocation {
    pointer: NonNull<u8>,
    layout: Layout,
}

#[cfg(unix)]
impl Allocation {
    pub fn address(&self) -> usize {
        self.pointer.as_ptr().addr()
    }

    pub const fn layout(&self) -> Layout {
        self.layout
    }

    pub const fn bytes(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.pointer.as_ptr(), self.layout.size()) }
    }

    pub const fn bytes_mut(&mut self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.pointer.as_ptr(), self.layout.size()) }
    }
}

#[cfg(unix)]
impl Default for Local {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(unix)]
impl Drop for Local {
    fn drop(&mut self) {
        self.state.abandon();
    }
}

#[cfg(unix)]
pub const fn is_small(layout: Layout) -> bool {
    class::small_bytes(layout.size(), layout.align()).is_some()
}
