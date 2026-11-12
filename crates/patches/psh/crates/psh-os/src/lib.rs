//! The OS hosts of psh: the real processes, pipes and files behind the engine's `Host`
//! trait (ARCH.md "psh-os", research/execution-core.md §5).
//!
//! - [`unix`]: macOS, Linux and Android.
//! - [`windows`]: Windows (x64 and arm64).
//!
//! The platform-neutral parts ([`lifecycle`], the timer thread, [`Leftovers`]) live
//! outside the platform modules and are shared by both hosts.

pub mod lifecycle;
mod terminal;
mod timer;
#[cfg(unix)]
pub mod unix;
#[cfg(windows)]
pub mod windows;

pub use terminal::stdout_columns;
pub use timer::sleep;

/// The fate of a run's leftovers when its host is dropped (D1067).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leftovers {
    /// TERM to every group of the run, KILL after the grace period. Only detached
    /// (`nohup`, `setsid`) and disowned externals survive. The default.
    Kill,
    /// Leave them running, as bash does.
    Keep,
}

/// Starts a helper thread with every asynchronous signal blocked from its first
/// instruction (Unix): signals for the process then reach the embedder's threads or,
/// standalone, the engine thread, never ours (a `kill $$` acts before `kill` returns).
///
/// On Unix it is a thread of the system's own, not one of std's: std keeps a table of
/// its threads behind a lock that a starting and an ending thread take, and a fork
/// (`UnixConfig::fork_jobs`) while another thread holds it would leave the copy unable
/// to start a thread. `stack`: its size, or the system's default.
#[cfg(unix)]
pub(crate) fn spawn_helper<F: FnOnce() + Send + 'static>(
    name: &str,
    stack: Option<usize>,
    f: F,
) -> std::io::Result<()> {
    type Body = (std::ffi::CString, Box<dyn FnOnce() + Send>);
    extern "C" fn run(arg: *mut libc::c_void) -> *mut libc::c_void {
        // SAFETY: `arg` is the box `spawn_helper` made for this thread alone.
        let (name, body) = *unsafe { Box::from_raw(arg.cast::<Body>()) };
        // SAFETY: a NUL-terminated name for the calling thread; the kernel cuts it to
        // its limit.
        #[cfg(any(target_os = "linux", target_os = "android"))]
        unsafe {
            libc::prctl(libc::PR_SET_NAME, name.as_ptr());
        }
        // SAFETY: as above.
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        unsafe {
            libc::pthread_setname_np(name.as_ptr());
        }
        let _ = name;
        // A panic must not unwind into the C caller.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
        std::ptr::null_mut()
    }
    let name = std::ffi::CString::new(name).unwrap_or_default();
    let arg = Box::into_raw(Box::new((name, Box::new(f) as Box<dyn FnOnce() + Send>)));
    let old = unix::signals::block_all_in_thread();
    // SAFETY: the attribute object lives for these calls; `run` takes the box, which
    // nothing else frees once the thread exists.
    let rc = unsafe {
        let mut attr: libc::pthread_attr_t = std::mem::zeroed();
        libc::pthread_attr_init(&raw mut attr);
        libc::pthread_attr_setdetachstate(&raw mut attr, libc::PTHREAD_CREATE_DETACHED);
        if let Some(stack) = stack {
            // At least what the system needs for a thread (its thread-local data).
            let least = usize::try_from(libc::sysconf(libc::_SC_THREAD_STACK_MIN)).unwrap_or(0);
            libc::pthread_attr_setstacksize(&raw mut attr, stack.max(least));
        }
        let mut thread: libc::pthread_t = std::mem::zeroed();
        let rc = libc::pthread_create(&raw mut thread, &raw const attr, run, arg.cast());
        libc::pthread_attr_destroy(&raw mut attr);
        rc
    };
    unix::signals::restore_mask(old);
    if rc != 0 {
        // SAFETY: no thread took the box.
        drop(unsafe { Box::from_raw(arg) });
        return Err(std::io::Error::from_raw_os_error(rc));
    }
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn spawn_helper<F: FnOnce() + Send + 'static>(
    name: &str,
    stack: Option<usize>,
    f: F,
) -> std::io::Result<()> {
    let mut thread = std::thread::Builder::new().name(name.into());
    if let Some(stack) = stack {
        thread = thread.stack_size(stack);
    }
    thread.spawn(f).map(drop)
}
