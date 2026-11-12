//! Readiness of pipes, FIFOs, sockets and terminals without blocking the engine thread.
//!
//! The other end of a pipe may be a vproc of the same engine, so a `read(2)` or
//! `write(2)` that waits in the kernel would deadlock the scheduler (research/
//! execution-core.md §5, pitfall 1). A [`UnixFile`](super::UnixFile) therefore does only
//! I/O that cannot wait: on our private pipe ends, which are `O_NONBLOCK`, it tries and
//! gets `EAGAIN`; on shared or inherited descriptors, whose flags it never changes
//! (`O_NONBLOCK` lives on the open file description, which children and other processes
//! share; pitfall 5), it asks [`now`] first. When it would wait it registers the task's
//! waker with [`watch`] and returns `Pending`.
//!
//! One process-wide thread waits on every watched descriptor plus a wake pipe, wakes
//! the wakers of each descriptor that is ready (or failed, or hung up) and forgets the
//! registration. The wait is `poll(2)`, except on macOS, where neither `poll` nor
//! kqueue reports a FIFO whose last writer closed and `poll` answers `POLLNVAL` for
//! terminals: there it is `select(2)`, with descriptors at or above `FD_SETSIZE` checked
//! by `poll` every 20 ms (pitfall 11).
//!
//! The recipe follows oh-my-pi's `pi-shell/src/sys/ready.rs` (MIT, see NOTICE).

use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::sync::{LazyLock, Mutex};
use std::task::Waker;

/// Wait for input.
pub const READ: i16 = libc::POLLIN;
/// Wait for space to write.
pub const WRITE: i16 = libc::POLLOUT;

/// What a write to a shared pipe may carry after [`now`] said `WRITE` without risking
/// a wait: `PIPE_BUF` (a pipe that polls writable takes this much atomically).
pub const WRITE_CHUNK: usize = libc::PIPE_BUF;

#[cfg(target_os = "macos")]
pub const PIPE_ROOM: usize = 64 * 1024;

/// Whether `fd` is ready for `events` right now; also when it hung up or failed (the
/// I/O then returns end of file or the error at once).
pub fn now(fd: RawFd, events: i16) -> io::Result<bool> {
    #[cfg(target_os = "macos")]
    if selectable(fd) {
        return select_now(fd, events);
    }
    poll_now(fd, events)
}

fn poll_now(fd: RawFd, events: i16) -> io::Result<bool> {
    let mut p = libc::pollfd {
        fd,
        events,
        revents: 0,
    };
    loop {
        // SAFETY: one valid, writable pollfd; a zero timeout never waits.
        let n = unsafe { libc::poll(&raw mut p, 1, 0) };
        if n >= 0 {
            return Ok(n > 0 && p.revents != 0);
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

struct Entry {
    fd: RawFd,
    events: i16,
    wakers: Vec<Waker>,
}

struct Poller {
    entries: Mutex<Vec<Entry>>,
    /// The write end of the wake pipe; `None` when the thread could not start.
    wake: Option<std::io::PipeWriter>,
}

static POLLER: LazyLock<Poller> = LazyLock::new(Poller::start);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Poller {
    fn start() -> Poller {
        let started = super::pipe().and_then(|(r, w)| {
            set_nonblocking(w.as_raw_fd())?;
            set_nonblocking(r.as_raw_fd())?;
            crate::spawn_helper("psh-ready", 128 << 10, move || run(r))?;
            Ok(w)
        });
        Poller {
            entries: Mutex::new(Vec::new()),
            wake: started.ok(),
        }
    }
}

/// Sets `O_NONBLOCK` on a descriptor only we hold (never one a child or another
/// process shares).
pub(crate) fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    // SAFETY: fcntl on a descriptor the caller owns; no pointers.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    // SAFETY: as above.
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Wakes `waker` once `fd` is ready for `events` (once: the registration is forgotten
/// when it fires). Several tasks may wait on one descriptor.
pub fn watch(fd: RawFd, events: i16, waker: &Waker) -> io::Result<()> {
    let p = &*POLLER;
    let Some(wake) = &p.wake else {
        return Err(io::Error::other("the readiness thread is not running"));
    };
    {
        let mut entries = lock(&p.entries);
        match entries
            .iter_mut()
            .find(|e| e.fd == fd && e.events == events)
        {
            Some(e) => {
                if e.wakers.iter().any(|w| w.will_wake(waker)) {
                    return Ok(());
                }
                e.wakers.push(waker.clone());
            }
            None => entries.push(Entry {
                fd,
                events,
                wakers: vec![waker.clone()],
            }),
        }
    }
    // A full wake pipe already guarantees a rescan.
    let _ = (&*wake).write(&[1]);
    Ok(())
}

/// Forgets every registration of `fd`, before it is closed (its number may be reused).
/// The wakers are woken: a task still waiting on it sees the closed file.
pub fn forget(fd: RawFd) {
    let woken: Vec<Waker> = {
        let Ok(mut entries) = POLLER.entries.lock() else {
            return;
        };
        let mut woken = Vec::new();
        entries.retain_mut(|e| {
            if e.fd == fd {
                woken.append(&mut e.wakers);
                false
            } else {
                true
            }
        });
        woken
    };
    for w in woken {
        w.wake();
    }
}

#[derive(Clone, Copy)]
struct Watched {
    fd: RawFd,
    events: i16,
}

fn run(mut wake: std::io::PipeReader) {
    let mut watched: Vec<Watched> = Vec::new();
    let mut drain = [0u8; 64];
    loop {
        watched.clear();
        watched.extend(lock(&POLLER.entries).iter().map(|e| Watched {
            fd: e.fd,
            events: e.events,
        }));
        let (woke, ready) = wait(wake.as_raw_fd(), &watched);
        if woke {
            while matches!(wake.read(&mut drain), Ok(n) if n > 0) {}
        }
        if ready.is_empty() {
            continue;
        }
        let mut woken = Vec::new();
        lock(&POLLER.entries).retain_mut(|e| {
            if ready.iter().any(|w| w.fd == e.fd && w.events == e.events) {
                woken.append(&mut e.wakers);
                false
            } else {
                true
            }
        });
        for w in woken {
            w.wake();
        }
    }
}

/// Waits until the wake pipe or a watched descriptor is ready: whether the wake pipe
/// fired, and the ready ones.
#[cfg(not(target_os = "macos"))]
fn wait(wake: RawFd, watched: &[Watched]) -> (bool, Vec<Watched>) {
    let mut fds: Vec<libc::pollfd> = Vec::with_capacity(watched.len() + 1);
    fds.push(libc::pollfd {
        fd: wake,
        events: libc::POLLIN,
        revents: 0,
    });
    fds.extend(watched.iter().map(|w| libc::pollfd {
        fd: w.fd,
        events: w.events,
        revents: 0,
    }));
    // SAFETY: `fds` holds `fds.len()` valid pollfds; -1 waits until one is ready.
    let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) };
    if n < 0 {
        return (false, Vec::new());
    }
    let ready = fds[1..]
        .iter()
        .zip(watched)
        .filter(|(p, _)| p.revents != 0)
        .map(|(_, w)| *w)
        .collect();
    (fds[0].revents != 0, ready)
}

/// Whether `select(2)` can take `fd`.
#[cfg(target_os = "macos")]
fn selectable(fd: RawFd) -> bool {
    usize::try_from(fd).is_ok_and(|fd| fd < libc::FD_SETSIZE)
}

#[cfg(target_os = "macos")]
fn fd_set() -> libc::fd_set {
    // SAFETY: fd_set is plain data; FD_ZERO initializes it below.
    let mut set: libc::fd_set = unsafe { std::mem::zeroed() };
    // SAFETY: `set` is a valid, writable fd_set.
    unsafe { libc::FD_ZERO(&raw mut set) };
    set
}

/// `select(2)` with a zero timeout on one descriptor below `FD_SETSIZE`.
#[cfg(target_os = "macos")]
fn select_now(fd: RawFd, events: i16) -> io::Result<bool> {
    loop {
        let mut set = fd_set();
        // SAFETY: `fd` is below FD_SETSIZE (the caller checked); `set` is valid.
        unsafe { libc::FD_SET(fd, &raw mut set) };
        let (rd, wr): (*mut libc::fd_set, *mut libc::fd_set) = if events & WRITE != 0 {
            (std::ptr::null_mut(), &raw mut set)
        } else {
            (&raw mut set, std::ptr::null_mut())
        };
        let mut zero = libc::timeval {
            tv_sec: 0,
            tv_usec: 0,
        };
        // SAFETY: the set pointers are valid or null; a zero timeout never waits.
        let n = unsafe { libc::select(fd + 1, rd, wr, std::ptr::null_mut(), &raw mut zero) };
        if n >= 0 {
            return Ok(n > 0);
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

#[cfg(target_os = "macos")]
fn is_open(fd: RawFd) -> bool {
    // SAFETY: F_GETFD only inspects the descriptor table.
    unsafe { libc::fcntl(fd, libc::F_GETFD) >= 0 }
}

/// macOS: `select(2)` over the descriptors below `FD_SETSIZE`, the rest checked with
/// `poll` every 20 ms. A watched descriptor closed meanwhile (`EBADF`) counts as ready,
/// so its owner sees the error instead of the thread spinning.
#[cfg(target_os = "macos")]
fn wait(wake: RawFd, watched: &[Watched]) -> (bool, Vec<Watched>) {
    let (mut rd, mut wr) = (fd_set(), fd_set());
    let mut nfds = 0;
    let mut add = |fd: RawFd, set: &mut libc::fd_set| {
        // SAFETY: callers pass descriptors below FD_SETSIZE; `set` is valid.
        unsafe { libc::FD_SET(fd, set) };
        nfds = nfds.max(fd + 1);
    };
    let wake_sel = selectable(wake);
    if wake_sel {
        add(wake, &mut rd);
    }
    let mut slow = !wake_sel;
    for w in watched {
        if !selectable(w.fd) {
            slow = true;
        } else if w.events & WRITE != 0 {
            add(w.fd, &mut wr);
        } else {
            add(w.fd, &mut rd);
        }
    }
    let mut tick = libc::timeval {
        tv_sec: 0,
        tv_usec: 20_000,
    };
    let timeout = if slow {
        &raw mut tick
    } else {
        std::ptr::null_mut()
    };
    // SAFETY: both sets are valid; the timeout is valid or null (wait).
    let n = unsafe {
        libc::select(
            nfds,
            &raw mut rd,
            &raw mut wr,
            std::ptr::null_mut(),
            timeout,
        )
    };
    if n < 0 {
        if io::Error::last_os_error().raw_os_error() == Some(libc::EBADF) {
            let gone = watched.iter().filter(|w| !is_open(w.fd)).copied().collect();
            return (false, gone);
        }
        return (false, Vec::new());
    }
    let woke = if wake_sel {
        // SAFETY: `rd` is valid; `wake` is below FD_SETSIZE.
        unsafe { libc::FD_ISSET(wake, &raw const rd) }
    } else {
        poll_now(wake, READ).unwrap_or(false)
    };
    let ready = watched
        .iter()
        .filter(|w| {
            if !selectable(w.fd) {
                return poll_now(w.fd, w.events).unwrap_or(true);
            }
            let set = if w.events & WRITE != 0 {
                &raw const wr
            } else {
                &raw const rd
            };
            // SAFETY: `set` is valid; the descriptor is below FD_SETSIZE.
            unsafe { libc::FD_ISSET(w.fd, set) }
        })
        .copied()
        .collect();
    (woke, ready)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    struct Unpark(std::thread::Thread);
    impl std::task::Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }

    #[test]
    fn a_watch_fires_when_the_pipe_becomes_readable() {
        let (r, mut w) = io::pipe().unwrap();
        assert!(!now(r.as_raw_fd(), READ).unwrap());
        assert!(now(w.as_raw_fd(), WRITE).unwrap());
        let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
        watch(r.as_raw_fd(), READ, &waker).unwrap();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            w.write_all(b"x").unwrap();
            w
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        while !now(r.as_raw_fd(), READ).unwrap() {
            assert!(Instant::now() < deadline);
            std::thread::park_timeout(Duration::from_millis(100));
        }
        drop(writer.join().unwrap());
        assert!(now(r.as_raw_fd(), READ).unwrap(), "hang-up counts as ready");
        forget(r.as_raw_fd());
    }

    #[test]
    fn forget_drops_registrations() {
        let (r, _w) = io::pipe().unwrap();
        watch(r.as_raw_fd(), READ, Waker::noop()).unwrap();
        forget(r.as_raw_fd());
        assert!(!lock(&POLLER.entries).iter().any(|e| e.fd == r.as_raw_fd()));
    }
}
