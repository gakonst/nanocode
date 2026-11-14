//! The engine numbers signals as Linux does on every platform (D1283); this maps them
//! to the OS's numbers at the host's boundary, and back for exit statuses.

use nanocode_sys::libc;
use psh_engine::host::Signal;

/// Linux number → this OS's constant, for the signals that differ between platforms.
const TABLE: &[(Signal, libc::c_int)] = &[
    (1, libc::SIGHUP),
    (2, libc::SIGINT),
    (3, libc::SIGQUIT),
    (4, libc::SIGILL),
    (5, libc::SIGTRAP),
    (6, libc::SIGABRT),
    (7, libc::SIGBUS),
    (8, libc::SIGFPE),
    (9, libc::SIGKILL),
    (10, libc::SIGUSR1),
    (11, libc::SIGSEGV),
    (12, libc::SIGUSR2),
    (13, libc::SIGPIPE),
    (14, libc::SIGALRM),
    (15, libc::SIGTERM),
    #[cfg(any(target_os = "linux", target_os = "android"))]
    (16, libc::SIGSTKFLT),
    (17, libc::SIGCHLD),
    (18, libc::SIGCONT),
    (19, libc::SIGSTOP),
    (20, libc::SIGTSTP),
    (21, libc::SIGTTIN),
    (22, libc::SIGTTOU),
    (23, libc::SIGURG),
    (24, libc::SIGXCPU),
    (25, libc::SIGXFSZ),
    (26, libc::SIGVTALRM),
    (27, libc::SIGPROF),
    (28, libc::SIGWINCH),
    (29, libc::SIGIO),
    #[cfg(any(target_os = "linux", target_os = "android"))]
    (30, libc::SIGPWR),
    (31, libc::SIGSYS),
];

/// The OS's number for Linux signal `sig` (0 stays 0); `None` where the OS has no such
/// signal (`STKFLT`, `PWR` and the real-time ones outside Linux).
pub fn to_os(sig: Signal) -> Option<libc::c_int> {
    if sig == 0 {
        return Some(0);
    }
    if let Some(&(_, os)) = TABLE.iter().find(|(l, _)| *l == sig) {
        return Some(os);
    }
    // Real-time signals: Linux only, where the numbers are the same.
    (cfg!(any(target_os = "linux", target_os = "android")) && (34..=64).contains(&sig))
        .then_some(sig)
}

/// The Linux number for the OS's signal `os` (unknown ones pass through).
pub fn from_os(os: libc::c_int) -> Signal {
    TABLE.iter().find(|(_, o)| *o == os).map_or(os, |&(l, _)| l)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        for &(l, os) in TABLE {
            assert_eq!(to_os(l), Some(os));
            assert_eq!(from_os(os), l);
        }
        assert_eq!(to_os(10), Some(libc::SIGUSR1));
        assert_eq!(to_os(0), Some(0));
    }
}
