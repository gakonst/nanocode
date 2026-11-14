use crate::libc;
use std::{
    io,
    os::fd::{BorrowedFd, FromRawFd, OwnedFd},
    sync::{
        OnceLock,
        atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering},
    },
};

use super::{checked, set_close_on_exec, set_nonblocking};

pub use libc::{
    SIGALRM, SIGCHLD, SIGHUP, SIGINT, SIGPIPE, SIGQUIT, SIGTERM, SIGUSR1, SIGUSR2, SIGWINCH,
};

#[cfg(target_os = "linux")]
const SIGNALS: usize = 65;
#[cfg(not(target_os = "linux"))]
const SIGNALS: usize = 33;

const FORBIDDEN: [libc::c_int; 5] = [
    libc::SIGILL,
    libc::SIGFPE,
    libc::SIGKILL,
    libc::SIGSEGV,
    libc::SIGSTOP,
];

static WRITER: AtomicI32 = AtomicI32::new(-1);
static PIPE: OnceLock<io::Result<(OwnedFd, OwnedFd)>> = OnceLock::new();
static PENDING: [AtomicBool; SIGNALS] = [const { AtomicBool::new(false) }; SIGNALS];
static INSTALLED: [AtomicBool; SIGNALS] = [const { AtomicBool::new(false) }; SIGNALS];
static PREVIOUS: [AtomicUsize; SIGNALS] = [const { AtomicUsize::new(0) }; SIGNALS];

#[cfg(target_os = "linux")]
unsafe fn errno_location() -> *mut libc::c_int {
    unsafe { libc::__errno_location() }
}

#[cfg(not(target_os = "linux"))]
unsafe fn errno_location() -> *mut libc::c_int {
    unsafe { libc::__error() }
}

extern "C" fn handle(signal: libc::c_int) {
    let saved = unsafe { *errno_location() };
    if let Ok(index) = usize::try_from(signal)
        && index < SIGNALS
    {
        PENDING[index].store(true, Ordering::SeqCst);
        let writer = WRITER.load(Ordering::SeqCst);
        if writer >= 0 {
            let byte = [0_u8];
            unsafe { libc::write(writer, byte.as_ptr().cast(), 1) };
        }
        let previous = PREVIOUS[index].load(Ordering::SeqCst);
        if previous != libc::SIG_DFL && previous != libc::SIG_IGN && previous != 0 {
            let chained: extern "C" fn(libc::c_int) = unsafe { std::mem::transmute(previous) };
            chained(signal);
        }
    }
    unsafe { *errno_location() = saved };
}

fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as libc::c_int; 2];
    checked(unsafe { libc::pipe(fds.as_mut_ptr()) })?;
    let reader = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    let writer = unsafe { OwnedFd::from_raw_fd(fds[1]) };
    for fd in [&reader, &writer] {
        set_close_on_exec(fd, true)?;
        set_nonblocking(fd, true)?;
    }
    Ok((reader, writer))
}

pub fn wakeups() -> io::Result<BorrowedFd<'static>> {
    match PIPE.get_or_init(|| {
        let (reader, writer) = pipe()?;
        WRITER.store(std::os::fd::AsRawFd::as_raw_fd(&writer), Ordering::SeqCst);
        Ok((reader, writer))
    }) {
        Ok((reader, _)) => Ok(std::os::fd::AsFd::as_fd(reader)),
        Err(error) => Err(io::Error::new(error.kind(), error.to_string())),
    }
}

pub fn listen(signal: libc::c_int) -> io::Result<()> {
    let index = usize::try_from(signal)
        .ok()
        .filter(|index| *index > 0 && *index < SIGNALS)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid signal number"))?;
    if FORBIDDEN.contains(&signal) {
        return Err(io::Error::other(format!(
            "Refusing to register signal {signal}"
        )));
    }
    wakeups()?;
    if INSTALLED[index].swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = handle as extern "C" fn(libc::c_int) as libc::sighandler_t;
    action.sa_flags = libc::SA_RESTART;
    checked(unsafe { libc::sigemptyset(&raw mut action.sa_mask) })?;
    let mut previous: libc::sigaction = unsafe { std::mem::zeroed() };
    if let Err(error) =
        checked(unsafe { libc::sigaction(signal, &raw const action, &raw mut previous) })
    {
        INSTALLED[index].store(false, Ordering::SeqCst);
        return Err(error);
    }
    if previous.sa_flags & libc::SA_SIGINFO == 0 {
        PREVIOUS[index].store(previous.sa_sigaction, Ordering::SeqCst);
    }
    Ok(())
}

#[must_use]
pub fn take_pending(signal: libc::c_int) -> bool {
    usize::try_from(signal)
        .ok()
        .filter(|index| *index < SIGNALS)
        .is_some_and(|index| PENDING[index].swap(false, Ordering::SeqCst))
}

#[must_use]
pub const fn signal_count() -> usize {
    SIGNALS
}

#[cfg(target_os = "linux")]
pub fn pidfd_open(pid: u32) -> io::Result<OwnedFd> {
    let pid =
        libc::pid_t::try_from(pid).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = libc::c_int::try_from(fd)
        .map_err(|_| io::Error::other("pidfd_open returned a large descriptor"))?;
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    set_close_on_exec(&fd, true)?;
    Ok(fd)
}
