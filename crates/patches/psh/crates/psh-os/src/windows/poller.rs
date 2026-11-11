//! Readiness of shared pipes without blocking the engine thread or taking their data:
//! the Windows counterpart of the Unix readiness thread.
//!
//! Anonymous pipes have no readiness event, so one process-wide thread checks the
//! watched handles with `PeekNamedPipe` and wakes the waker of each that has data or
//! hung up, then forgets it. It checks often while something is watched (every
//! millisecond at first, backing off to 16 ms while nothing changes) and sleeps on a
//! condition variable otherwise.
//!
//! A handle stays valid while it is watched: [`WinFile`](super::WinFile) calls
//! [`forget`] before it closes one.

use std::sync::{Condvar, LazyLock, Mutex};
use std::task::Waker;
use std::time::Duration;

use windows_sys::Win32::Foundation::HANDLE;

struct Watched {
    /// The handle as an integer (raw handles are not `Send`).
    handle: usize,
    wakers: Vec<Waker>,
}

struct Poller {
    list: Mutex<Vec<Watched>>,
    changed: Condvar,
}

static POLLER: LazyLock<Poller> = LazyLock::new(|| {
    let started = std::thread::Builder::new()
        .name("psh-poll".into())
        .stack_size(64 << 10)
        .spawn(run);
    if started.is_err() {
        // Without the thread nothing would wake a watcher: wake them at once instead,
        // so they poll again (busy, but correct).
        FALLBACK.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    Poller {
        list: Mutex::new(Vec::new()),
        changed: Condvar::new(),
    }
});

static FALLBACK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Wakes `waker` once `h` has data or hung up.
pub(crate) fn watch(h: HANDLE, waker: &Waker) {
    let p = &*POLLER;
    if FALLBACK.load(std::sync::atomic::Ordering::Relaxed) {
        waker.wake_by_ref();
        return;
    }
    let mut list = lock(&p.list);
    let handle = h as usize;
    match list.iter_mut().find(|w| w.handle == handle) {
        Some(w) => {
            if !w.wakers.iter().any(|x| x.will_wake(waker)) {
                w.wakers.push(waker.clone());
            }
        }
        None => list.push(Watched {
            handle,
            wakers: vec![waker.clone()],
        }),
    }
    p.changed.notify_one();
}

/// Stops watching `h` (it is about to be closed).
pub(crate) fn forget(h: HANDLE) {
    let p = &*POLLER;
    lock(&p.list).retain(|w| w.handle != h as usize);
}

fn run() {
    let p = &*POLLER;
    let mut idle_rounds = 0u32;
    loop {
        let mut list = lock(&p.list);
        while list.is_empty() {
            idle_rounds = 0;
            list = p.changed.wait(list).unwrap_or_else(|e| e.into_inner());
        }
        let mut ready = Vec::new();
        // Checked under the lock: `forget` cannot close a handle in the middle.
        list.retain_mut(|w| {
            if super::file::readable_now(w.handle as HANDLE) {
                ready.append(&mut w.wakers);
                false
            } else {
                true
            }
        });
        drop(list);
        if ready.is_empty() {
            idle_rounds = idle_rounds.saturating_add(1);
        } else {
            idle_rounds = 0;
            for w in ready {
                w.wake();
            }
        }
        let ms = 1u64 << idle_rounds.min(4);
        std::thread::sleep(Duration::from_millis(ms));
    }
}
