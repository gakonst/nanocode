use core::ffi::c_void;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicUsize, Ordering};

static PAGE_BYTES: AtomicUsize = AtomicUsize::new(0);
static EXIT_KEY: AtomicUsize = AtomicUsize::new(0);

#[cfg(target_vendor = "apple")]
const PURGE_ADVICE: libc::c_int = libc::MADV_FREE_REUSABLE;
#[cfg(not(target_vendor = "apple"))]
const PURGE_ADVICE: libc::c_int = libc::MADV_DONTNEED;

#[cold]
pub fn broken(what: &str) -> ! {
    let prefix = b"nanocode allocator invariant broken: ";
    unsafe {
        libc::write(libc::STDERR_FILENO, prefix.as_ptr().cast(), prefix.len());
        libc::write(libc::STDERR_FILENO, what.as_ptr().cast(), what.len());
        libc::write(libc::STDERR_FILENO, b"\n".as_ptr().cast(), 1);
        libc::abort()
    }
}

#[inline]
pub fn check(holds: bool, what: &str) {
    if !holds {
        broken(what);
    }
}

pub fn page_bytes() -> usize {
    let cached = PAGE_BYTES.load(Ordering::Relaxed);
    if cached != 0 {
        return cached;
    }
    let read = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    let Some(bytes) = usize::try_from(read)
        .ok()
        .filter(|bytes| bytes.is_power_of_two())
    else {
        broken("the system page size is a power of two");
    };
    PAGE_BYTES.store(bytes, Ordering::Relaxed);
    bytes
}

pub unsafe fn map_aligned(bytes: usize, align: usize) -> usize {
    let Some(total) = bytes.checked_add(align) else {
        return 0;
    };
    let raw = unsafe {
        libc::mmap(
            null_mut(),
            total,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    if raw == libc::MAP_FAILED {
        return 0;
    }
    let start = raw.addr();
    let aligned = (start + align - 1) & !(align - 1);
    let tail = aligned + bytes;
    let end = start + total;
    unsafe {
        if aligned > start {
            unmap(start, aligned - start);
        }
        if end > tail {
            unmap(tail, end - tail);
        }
    }
    aligned
}

pub unsafe fn unmap(address: usize, bytes: usize) {
    let result = unsafe { libc::munmap(address as *mut c_void, bytes) };
    check(result == 0, "munmap releases a range this allocator mapped");
}

pub unsafe fn purge(address: usize, bytes: usize) {
    unsafe { libc::madvise(address as *mut c_void, bytes, PURGE_ADVICE) };
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub unsafe fn grow_in_place(address: usize, bytes: usize, bytes_new: usize) -> bool {
    let moved = unsafe { libc::mremap(address as *mut c_void, bytes, bytes_new, 0) };
    moved != libc::MAP_FAILED
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub const unsafe fn grow_in_place(_address: usize, _bytes: usize, _bytes_new: usize) -> bool {
    false
}

pub fn watch_exit(state: *const c_void, on_exit: unsafe extern "C" fn(*mut c_void)) -> bool {
    let key = match EXIT_KEY.load(Ordering::Acquire) {
        0 => {
            let mut created: libc::pthread_key_t = 0;
            if unsafe { libc::pthread_key_create(&raw mut created, Some(on_exit)) } != 0 {
                return false;
            }
            let encoded = created as usize + 1;
            match EXIT_KEY.compare_exchange(0, encoded, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => encoded,
                Err(winner) => {
                    unsafe { libc::pthread_key_delete(created) };
                    winner
                }
            }
        }
        encoded => encoded,
    };
    unsafe { libc::pthread_setspecific((key - 1) as libc::pthread_key_t, state) == 0 }
}
