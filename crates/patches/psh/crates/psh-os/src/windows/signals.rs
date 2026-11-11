//! Console control events for the standalone `psh` binary only; an embedded shell
//! installs nothing (research/execution-core.md §5, pitfall 4).
//!
//! Windows calls a console control handler on a thread of its own: it maps the event to
//! a signal (Ctrl-C: INT, Ctrl-Break: TERM, closing the console, logoff and shutdown:
//! HUP), hands it to the callback (which cancels the run) and reports the event as
//! handled, so the default action (ending psh at once) does not cut the cancel short.
//! Children sharing the console get the same event themselves.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, Ordering};

use psh_engine::host::{SIGHUP, SIGINT, SIGTERM, Signal};
use windows_sys::Win32::System::Console::{
    CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
    SetConsoleCtrlHandler,
};

type Callback = Box<dyn Fn(Signal) + Send + Sync>;

static CALLBACK: OnceLock<Callback> = OnceLock::new();
static FIRST: AtomicI32 = AtomicI32::new(0);

unsafe extern "system" fn on_event(event: u32) -> i32 {
    let sig = match event {
        CTRL_C_EVENT => SIGINT,
        CTRL_BREAK_EVENT => SIGTERM,
        CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT => SIGHUP,
        _ => return 0,
    };
    let _ = FIRST.compare_exchange(0, sig, Ordering::SeqCst, Ordering::SeqCst);
    if let Some(cb) = CALLBACK.get() {
        cb(sig);
    }
    1
}

/// The first event caught, as a signal.
pub fn first_caught() -> Option<Signal> {
    match FIRST.load(Ordering::SeqCst) {
        0 => None,
        s => Some(s),
    }
}

/// Catches console control events and calls `callback` with the signal of each. Once
/// per process.
pub fn catch(callback: impl Fn(Signal) + Send + Sync + 'static) -> std::io::Result<()> {
    if CALLBACK.set(Box::new(callback)).is_err() {
        return Err(std::io::Error::other("console events are caught already"));
    }
    // SAFETY: the handler is a plain function that lives for the process and only
    // touches statics.
    if unsafe { SetConsoleCtrlHandler(Some(on_event), 1) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Ends the process with the status a shell shows for `sig` (`128 + sig`): Windows has
/// no death by signal.
pub fn die_by(sig: Signal) -> ! {
    std::process::exit(128 + sig)
}
