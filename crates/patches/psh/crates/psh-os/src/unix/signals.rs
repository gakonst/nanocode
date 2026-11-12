//! Signals for the standalone `psh` binary only; an embedded shell installs nothing
//! (research/execution-core.md §5, pitfall 4).
//!
//! The handlers are async-signal-safe: they set a bit in an atomic word and write a
//! byte into a self-pipe. A watcher thread reads the pipe and hands each signal to the
//! driver's callback (which cancels the run). Children never see these handlers: an
//! exec resets caught signals, and the spawner resets every disposition anyway.
//! A signal ignored when psh started stays ignored and gets no handler, as in bash.
//! The API takes and gives the engine's (Linux) signal numbers (D1283).

use std::io::{self, Read};
use std::os::fd::{AsRawFd, IntoRawFd};
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};

use psh_engine::host::Signal;

/// Signals caught and not yet handed to the callback, one bit each.
static PENDING: AtomicU64 = AtomicU64::new(0);
/// The engine's pending-signal mask the handler sets itself (`forward_to`), so a
/// signal acts at the engine's next safepoint even before the watcher thread runs.
static FORWARD: std::sync::atomic::AtomicPtr<std::sync::atomic::AtomicU64> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

/// The write end of the self-pipe; -1 until `catch` ran.
static WAKE_FD: AtomicI32 = AtomicI32::new(-1);

/// The watcher thread's work, from `catch` until [`watch`] starts the thread.
static WATCHER: std::sync::Mutex<Option<Box<dyn FnOnce() + Send>>> = std::sync::Mutex::new(None);
/// Whether [`watch`] has nothing left to start.
static WATCHING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
/// What [`catch`] calls for each delivery.
static CALLBACK: std::sync::OnceLock<Box<dyn Fn(Signal) + Send + Sync>> =
    std::sync::OnceLock::new();

/// The watcher thread's work: hands each signal the handler noted to the callback.
fn watcher(mut r: io::PipeReader) -> Box<dyn FnOnce() + Send> {
    Box::new(move || {
        let mut buf = [0u8; 64];
        loop {
            match r.read(&mut buf) {
                Ok(0) => return,
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => return,
            }
            let bits = PENDING.swap(0, Ordering::SeqCst);
            for sig in 1..64 {
                if bits & (1 << sig) != 0
                    && let Some(callback) = CALLBACK.get()
                {
                    callback(super::signum::from_os(sig));
                }
            }
        }
    })
}

/// In a forked copy of a process that catches signals: the self-pipe and the watcher
/// thread are the parent's, so the copy gets a pipe of its own and starts its thread
/// when it first parks ([`watch`]). Called with every signal blocked.
pub(crate) fn forked() {
    let old = WAKE_FD.load(Ordering::SeqCst);
    if old < 0 {
        return;
    }
    let Ok((r, w)) = super::pipe() else {
        return;
    };
    if super::ready::set_nonblocking(w.as_raw_fd()).is_err() {
        return;
    }
    WAKE_FD.store(w.into_raw_fd(), Ordering::SeqCst);
    // SAFETY: `old` is this copy's descriptor of the parent's pipe, used by nothing
    // else here (the handler reads `WAKE_FD`, and signals are blocked).
    unsafe { libc::close(old) };
    PENDING.store(0, Ordering::SeqCst);
    *WATCHER.lock().unwrap_or_else(|e| e.into_inner()) = Some(watcher(r));
    WATCHING.store(false, Ordering::SeqCst);
}

extern "C" fn on_signal(sig: libc::c_int) {
    let saved = io::Error::last_os_error().raw_os_error().unwrap_or(0);
    if (0..64).contains(&sig) {
        PENDING.fetch_or(1 << sig, Ordering::SeqCst);
        let word = FORWARD.load(Ordering::SeqCst);
        let linux = super::signum::from_os(sig);
        if !word.is_null() && (1..64).contains(&linux) {
            // SAFETY: `forward_to` stored a pointer to a `'static` atomic; the table
            // lookup and an atomic or are async-signal-safe.
            unsafe { (*word).fetch_or(1u64 << linux, Ordering::SeqCst) };
        }
    }
    let fd = WAKE_FD.load(Ordering::SeqCst);
    if fd >= 0 {
        let b = 1u8;
        // SAFETY: write(2) is async-signal-safe; the pipe end lives for the process.
        // A full pipe (EAGAIN) already guarantees a wakeup.
        unsafe { libc::write(fd, (&raw const b).cast(), 1) };
    }
    // errno is per thread and the interrupted code may look at it.
    set_errno(saved);
}

fn set_errno(e: i32) {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    // SAFETY: __error returns this thread's errno location.
    unsafe {
        *libc::__error() = e;
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    // SAFETY: __errno_location returns this thread's errno location.
    unsafe {
        *libc::__errno_location() = e;
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "linux",
        target_os = "android"
    )))]
    let _ = e;
}

/// Makes the handler set the caught signal's bit (Linux numbering) in `word`, the
/// engine's pending-signal mask (`psh_engine::ControlHandle::signal_word`): the signal
/// then acts at the engine's next safepoint without waiting for the watcher thread.
pub fn forward_to(word: &'static std::sync::atomic::AtomicU64) {
    FORWARD.store(std::ptr::from_ref(word).cast_mut(), Ordering::SeqCst);
}

/// Blocks or unblocks `sigs` in the calling thread.
fn mask(how: libc::c_int, sigs: &[Signal]) {
    // SAFETY: sigset_t is plain data, initialized by sigemptyset; pthread_sigmask only
    // reads the set.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&raw mut set);
        for s in sigs.iter().filter_map(|&s| super::signum::to_os(s)) {
            libc::sigaddset(&raw mut set, s);
        }
        libc::pthread_sigmask(how, &raw const set, std::ptr::null_mut());
    }
}

/// Blocks `sigs` in the calling thread (threads it starts inherit that).
pub fn block_in_thread(sigs: &[Signal]) {
    mask(libc::SIG_BLOCK, sigs);
}

/// Unblocks `sigs` in the calling thread.
pub fn unblock_in_thread(sigs: &[Signal]) {
    mask(libc::SIG_UNBLOCK, sigs);
}

/// Blocks every asynchronous signal in the calling thread and returns the old mask
/// (`restore_mask`): around the start of psh-os's helper threads, which inherit it.
pub(crate) fn block_all_in_thread() -> libc::sigset_t {
    // SAFETY: as in `mask`; the synchronous signals stay deliverable (blocking them
    // would make a fault in this thread undefined).
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        let mut old: libc::sigset_t = std::mem::zeroed();
        libc::sigfillset(&raw mut set);
        for s in [
            libc::SIGSEGV,
            libc::SIGBUS,
            libc::SIGFPE,
            libc::SIGILL,
            libc::SIGTRAP,
        ] {
            libc::sigdelset(&raw mut set, s);
        }
        libc::pthread_sigmask(libc::SIG_BLOCK, &raw const set, &raw mut old);
        old
    }
}

/// Restores a mask `block_all_in_thread` returned.
pub(crate) fn restore_mask(old: libc::sigset_t) {
    // SAFETY: `old` is a valid set from pthread_sigmask.
    unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &raw const old, std::ptr::null_mut()) };
}

/// Resets those of `sigs` the process ignores to their default disposition (Linux
/// numbers): for a test runner or harness whose children must not depend on how it
/// was started (a background job starts with SIGINT ignored, `nohup` ignores SIGHUP).
/// Never SIGPIPE in a Rust program: its writes rely on EPIPE.
pub fn reset_ignored(sigs: &[Signal]) {
    for &sig in sigs {
        if !is_ignored(sig) {
            continue;
        }
        let Some(os) = super::signum::to_os(sig) else {
            continue;
        };
        // SAFETY: sets a default disposition; no handler code is involved.
        unsafe { libc::signal(os, libc::SIG_DFL) };
    }
}

/// The asynchronous signals a test harness resets with [`reset_ignored`] (Linux
/// numbers; not PIPE, KILL, STOP or the faults).
pub const HARNESS_RESET: [Signal; 11] = [1, 2, 3, 10, 12, 14, 15, 24, 25, 26, 27];

/// Starts the watcher thread of [`catch`], once: for the driver to call before it
/// parks. The handler has set the signal's bit for the engine already, so only a
/// parked shell needs the thread (to be woken); a signal caught before it starts is in
/// the pipe and reaches the callback when it does. A script that never waits (`echo`,
/// a loop, a pipeline of builtins) ends without the thread having existed.
pub fn watch() {
    if WATCHING.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Some(run) = WATCHER.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = crate::spawn_helper("psh-signals", Some(64 << 10), run);
    }
}

/// Whether the process ignores `sig` now.
pub fn is_ignored(sig: Signal) -> bool {
    let Some(sig) = super::signum::to_os(sig) else {
        return false;
    };
    // SAFETY: sigaction is plain data, valid zeroed.
    let mut old: libc::sigaction = unsafe { std::mem::zeroed() };
    // SAFETY: a null new action only queries; `old` is writable.
    let rc = unsafe { libc::sigaction(sig, std::ptr::null(), &raw mut old) };
    rc == 0 && old.sa_sigaction == libc::SIG_IGN
}

/// Catches `sigs` (except those ignored now) and calls `callback` for each delivery,
/// on a watcher thread that [`watch`] starts. Once per process.
pub fn catch(
    sigs: &[Signal],
    callback: impl Fn(Signal) + Send + Sync + 'static,
) -> io::Result<()> {
    let (r, w) = super::pipe()?;
    super::ready::set_nonblocking(w.as_raw_fd())?;
    let wfd = w.into_raw_fd();
    if WAKE_FD
        .compare_exchange(-1, wfd, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        // SAFETY: `wfd` is ours and unused.
        unsafe { libc::close(wfd) };
        return Err(io::Error::other("signals are caught already"));
    }
    let _ = CALLBACK.set(Box::new(callback));
    *WATCHER.lock().unwrap_or_else(|e| e.into_inner()) = Some(watcher(r));
    WATCHING.store(false, Ordering::SeqCst);
    for &sig in sigs {
        if is_ignored(sig) {
            continue;
        }
        let Some(sig) = super::signum::to_os(sig) else {
            continue;
        };
        // SAFETY: sigaction is plain data; the handler only does async-signal-safe
        // work (atomics, write).
        unsafe {
            let mut act: libc::sigaction = std::mem::zeroed();
            act.sa_sigaction = on_signal as *const () as libc::sighandler_t;
            act.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&raw mut act.sa_mask);
            if libc::sigaction(sig, &raw const act, std::ptr::null_mut()) != 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }
    Ok(())
}

/// Ends the process by `sig` with its default action, so the parent sees the signal
/// (as bash does when a signal ends it); exits with `128 + sig` if that fails. When the
/// process leads its process group (a harness started it in a group of its own), the
/// signal goes to the whole group: what the shell's externals left behind there dies
/// with it, even when only the shell's pid was signalled (D1067).
pub fn die_by(sig: Signal) -> ! {
    let os = super::signum::to_os(sig).unwrap_or(libc::SIGTERM);
    // SAFETY: plain calls with valid arguments; the process ends here.
    unsafe {
        libc::signal(os, libc::SIG_DFL);
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&raw mut set);
        libc::sigaddset(&raw mut set, os);
        libc::pthread_sigmask(libc::SIG_UNBLOCK, &raw const set, std::ptr::null_mut());
        if libc::getpgrp() == libc::getpid() {
            libc::kill(0, os);
        }
        libc::raise(os);
    }
    std::process::exit(128 + sig)
}
