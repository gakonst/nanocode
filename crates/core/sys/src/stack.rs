#[cfg(unix)]
use crate::libc;
use std::cell::Cell;

thread_local! {
    static STACK_BOUNDS: Cell<Option<Option<(usize, usize)>>> = const { Cell::new(None) };
}

pub fn current_thread_stack_remaining_bytes() -> Option<usize> {
    let (low, high) = STACK_BOUNDS.with(|cached| match cached.get() {
        Some(bounds) => bounds,
        None => {
            let bounds = stack_bounds();
            cached.set(Some(bounds));
            bounds
        }
    })?;
    let marker = 0u8;
    let position = std::ptr::from_ref(std::hint::black_box(&marker)).addr();
    if position < low || position >= high {
        return None;
    }
    Some(position - low)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn stack_bounds() -> Option<(usize, usize)> {
    let mut attributes = std::mem::MaybeUninit::<libc::pthread_attr_t>::uninit();
    if unsafe { libc::pthread_getattr_np(libc::pthread_self(), attributes.as_mut_ptr()) } != 0 {
        return None;
    }
    let mut address = std::ptr::null_mut();
    let mut size = 0;
    let read =
        unsafe { libc::pthread_attr_getstack(attributes.as_ptr(), &raw mut address, &raw mut size) };
    let destroyed = unsafe { libc::pthread_attr_destroy(attributes.as_mut_ptr()) };
    assert_eq!(destroyed, 0, "pthread_attr_destroy accepts attributes pthread_getattr_np filled");
    if read != 0 {
        return None;
    }
    let low = address.addr();
    Some((low, low.checked_add(size)?))
}

#[cfg(target_vendor = "apple")]
fn stack_bounds() -> Option<(usize, usize)> {
    let thread = unsafe { libc::pthread_self() };
    let high = unsafe { libc::pthread_get_stackaddr_np(thread) }.addr();
    let size = unsafe { libc::pthread_get_stacksize_np(thread) };
    Some((high.checked_sub(size)?, high))
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
const fn stack_bounds() -> Option<(usize, usize)> {
    None
}
