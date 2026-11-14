//! Signal helpers for `kill`, `jobs` and `wait` (D1900), over the engine's one signal
//! table in `builtins::trap` (D1283): Linux numbering on every platform, as the goldens
//! and `kill -l` show it; hosts map the numbers to the platform's own. Descriptions are
//! glibc's `strsignal` texts, as bash prints them on Linux.

use crate::builtins::trap::{signal_name, signal_number};
use crate::host::Signal;

/// The highest signal number (`SIGRTMAX`).
pub(crate) fn max() -> Signal {
    64
}

/// `SIGTERM` for 15, `SIGRTMIN+2` for 36; `None` when there is no such signal.
pub(crate) fn name(n: Signal) -> Option<String> {
    signal_name(n)
}

/// The number of a signal name: `TERM`, `SIGTERM`, `term` (any case), `RTMIN+2`.
/// `EXIT` is 0.
pub(crate) fn number(s: &str) -> Option<Signal> {
    if s.eq_ignore_ascii_case("EXIT") || s.eq_ignore_ascii_case("SIGEXIT") {
        return Some(0);
    }
    if s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let up = s.to_ascii_uppercase();
    match up.strip_prefix("SIG").unwrap_or(&up) {
        "IOT" => Some(6),
        "POLL" => Some(29),
        _ => signal_number(s),
    }
}

/// A signal given by name or number (`kill -s`, `kill -SIG`): `None` when invalid.
pub(crate) fn parse(s: &str) -> Option<Signal> {
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        let n: Signal = s.parse().ok()?;
        return (n == 0 || name(n).is_some()).then_some(n);
    }
    number(s)
}

/// What `jobs` and job reports say for a process a signal ended (glibc's texts).
pub(crate) fn describe(n: Signal) -> String {
    let base = name(n);
    let text = match base.as_deref().map(|s| &s[3..]) {
        Some("HUP") => "Hangup",
        Some("INT") => "Interrupt",
        Some("QUIT") => "Quit",
        Some("ILL") => "Illegal instruction",
        Some("TRAP") => "Trace/breakpoint trap",
        Some("ABRT") => "Aborted",
        Some("BUS") => "Bus error",
        Some("FPE") => "Floating point exception",
        Some("KILL") => "Killed",
        Some("USR1") => "User defined signal 1",
        Some("SEGV") => "Segmentation fault",
        Some("USR2") => "User defined signal 2",
        Some("PIPE") => "Broken pipe",
        Some("ALRM") => "Alarm clock",
        Some("TERM") => "Terminated",
        Some("STKFLT") => "Stack fault",
        Some("CHLD") => "Child exited",
        Some("CONT") => "Continued",
        Some("STOP") => "Stopped (signal)",
        Some("TSTP") => "Stopped",
        Some("TTIN") => "Stopped (tty input)",
        Some("TTOU") => "Stopped (tty output)",
        Some("URG") => "Urgent I/O condition",
        Some("XCPU") => "CPU time limit exceeded",
        Some("XFSZ") => "File size limit exceeded",
        Some("VTALRM") => "Virtual timer expired",
        Some("PROF") => "Profiling timer expired",
        Some("WINCH") => "Window changed",
        Some("IO") => "I/O possible",
        Some("PWR") => "Power failure",
        Some("SYS") => "Bad system call",
        _ => return format!("Signal {n}"),
    };
    text.to_string()
}

/// Signals whose default action does not end a process: a job sent one runs on.
pub(crate) fn harmless(n: Signal) -> bool {
    n == 0
        || matches!(
            name(n).as_deref(),
            Some("SIGCHLD" | "SIGCONT" | "SIGURG" | "SIGWINCH")
        )
}

/// Signals that stop rather than end a process.
pub(crate) fn stops(n: Signal) -> bool {
    matches!(
        name(n).as_deref(),
        Some("SIGSTOP" | "SIGTSTP" | "SIGTTIN" | "SIGTTOU")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_numbers_round_trip() {
        // Linux leaves 32 and 33 to the C library: no names.
        for n in 1..=max() {
            if let Some(s) = name(n) {
                assert_eq!(number(&s), Some(n), "{s}");
            }
        }
        assert_eq!(number("USR1"), Some(10));
        assert!(name(15).is_some() && name(max() + 1).is_none());
        assert_eq!(number("term"), Some(15));
        assert_eq!(number("SigTerm"), Some(15));
        assert_eq!(number("EXIT"), Some(0));
        assert_eq!(parse("9"), Some(9));
        assert_eq!(parse("9999"), None);
        assert_eq!(parse("NOPE"), None);
        assert_eq!(describe(15), "Terminated");
        assert_eq!(describe(9), "Killed");
    }
}

/// Which pending signals end a wait of the shell's own vproc (D1903).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Gives {
    /// A signal whose default action ends the shell (no trap, not ignored): a
    /// foreground command is abandoned, the shell ends at its next safepoint. A trapped
    /// signal waits for the command, as in bash.
    Fatal,
    /// Also a trapped signal: `wait` returns `128+n` and the trap runs.
    Trapped,
    /// Also a trapped INT: for one of psh's utilities in the foreground. A terminal
    /// sends INT to the whole foreground group, so the program in the utility's place
    /// would have got it and ended (`sleep 60` under a trap on INT ends at Ctrl-C with
    /// 130, and then the trap runs).
    Interrupt,
}

/// The pending signal that ends a wait of `vp` now, if any. Only the shell's own vproc
/// takes signals (D1284), never inside a trap.
pub(crate) fn breaking(vp: &crate::state::Vp, gives: Gives) -> Option<Signal> {
    if !vp.top || vp.in_trap.is_some() {
        return None;
    }
    breaking_in(&vp.core, &vp.sigs, &vp.traps, gives)
}

/// [`breaking`] for a wait that cannot borrow its vproc (an in-process utility holds
/// it): the caller checked that the vproc takes signals.
pub(crate) fn breaking_in(
    core: &crate::state::Core,
    sigs: &crate::state::Sigs,
    traps: &crate::builtins::trap::Traps,
    gives: Gives,
) -> Option<Signal> {
    use crate::builtins::trap::{TrapKey, terminates};
    let pending = sigs.peek(core);
    let gives = if sigs.group() { Gives::Trapped } else { gives };
    (1..64).filter(|s| pending & (1u64 << s) != 0).find(|&sig| {
        match traps.action(TrapKey::Signal(sig)) {
            Some(a) if a.is_empty() => false,
            Some(_) => {
                gives == Gives::Trapped || (gives == Gives::Interrupt && sig == crate::host::SIGINT)
            }
            None => terminates(sig) && (sig != crate::host::SIGQUIT || core.forked.get().is_some()),
        }
    })
}

/// Awaits `fut` unless a pending signal ends the wait first (`Err(signal)`).
pub(crate) async fn or_signal<F: std::future::Future>(
    vp: &crate::state::Vp,
    gives: Gives,
    fut: F,
) -> Result<F::Output, Signal> {
    let mut fut = std::pin::pin!(fut);
    std::future::poll_fn(|cx| {
        if let Some(sig) = breaking(vp, gives) {
            return std::task::Poll::Ready(Err(sig));
        }
        match fut.as_mut().poll(cx) {
            std::task::Poll::Ready(v) => std::task::Poll::Ready(Ok(v)),
            std::task::Poll::Pending => {
                if vp.top {
                    vp.sigs.on_signal(&vp.core, cx.waker());
                }
                std::task::Poll::Pending
            }
        }
    })
    .await
}
