// Copied from ripgrep (https://github.com/BurntSushi/ripgrep, commit 91f5d22),
// crates/core/messages.rs, with the psh patches marked `psh:` (D1133):
// Copyright (c) 2015 Andrew Gallant. Licensed under the MIT License (see NOTICE).

/*!
This module defines some macros and some light shared mutable state.

This state is responsible for keeping track of whether we should emit certain
kinds of messages to the user (such as errors) that are distinct from the
standard "debug" or "trace" log messages. This state is specifically set at
startup time when CLI arguments are parsed and then never changed.

The other state tracked here is whether ripgrep experienced an error
condition. Aside from errors associated with invalid CLI arguments, ripgrep
generally does not abort when an error occurs (e.g., if reading a file failed).
But when an error does occur, it will alter ripgrep's exit status. Thus, when
an error message is emitted via `err_message`, then a global flag is toggled
indicating that at least one error occurred. When ripgrep exits, this flag is
consulted to determine what the exit status ought to be.
*/

// psh: the state is per invocation (D1133), not process-wide statics: a thread-local
// that `scoped` installs for the duration of one rg run on its thread (search threads
// never print messages; their errors come back to it), with messages written to the
// utility's stderr instead of the process's. A failed write is ignored rather than
// ending the process.

use std::cell::RefCell;
use std::io::Write;

use psh_engine::util::SyncWriter;

/// One invocation's message state.
pub(crate) struct State {
    /// When false, "messages" will not be printed.
    messages: bool,
    /// When false, "messages" related to ignore rules will not be printed.
    ignore_messages: bool,
    /// Flipped to true when an error message is printed.
    errored: bool,
    out: Out,
}

enum Out {
    /// Before the utility's stderr is at hand (parsing on the engine thread).
    Buffer(Vec<u8>),
    Writer(SyncWriter),
}

impl State {
    /// The state before parsing: messages off (as ripgrep's statics start), collected.
    pub(crate) fn buffered() -> State {
        State {
            messages: false,
            ignore_messages: false,
            errored: false,
            out: Out::Buffer(Vec::new()),
        }
    }

    /// What was collected so far, and from now on writes go to `w`.
    pub(crate) fn write_to(&mut self, w: SyncWriter) -> Vec<u8> {
        match std::mem::replace(&mut self.out, Out::Writer(w)) {
            Out::Buffer(b) => b,
            Out::Writer(_) => Vec::new(),
        }
    }

    /// Takes the messages collected so far.
    pub(crate) fn take_buffered(&mut self) -> Vec<u8> {
        match &mut self.out {
            Out::Buffer(b) => std::mem::take(b),
            Out::Writer(_) => Vec::new(),
        }
    }
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// Runs `f` with `state` as this thread's message state (taken back afterwards, also
/// when `f` panics).
pub(crate) fn scoped<R>(state: State, f: impl FnOnce() -> R) -> (R, State) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            STATE.with(|s| s.borrow_mut().take());
        }
    }
    STATE.with(|s| *s.borrow_mut() = Some(state));
    let reset = Reset;
    let r = f();
    let state = STATE.with(|s| s.borrow_mut().take());
    drop(reset);
    (r, state.unwrap_or_else(State::buffered))
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.borrow_mut().as_mut().map(f))
}

/// Writes `rg: MSG` and a newline to the invocation's stderr.
pub(crate) fn eprint(msg: std::fmt::Arguments<'_>) {
    let line = format!("rg: {msg}\n");
    with(|s| match &mut s.out {
        Out::Buffer(b) => b.extend_from_slice(line.as_bytes()),
        Out::Writer(w) => {
            let _ = w.write_all(line.as_bytes());
        }
    });
}

/// Like eprintln, but to the invocation's stderr, with ripgrep's `rg: ` prefix.
macro_rules! eprintln_locked {
    ($($tt:tt)*) => {
        $crate::rg::messages::eprint(format_args!($($tt)*))
    }
}

/// Emit a non-fatal error message, unless messages were disabled.
macro_rules! message {
    ($($tt:tt)*) => {
        if $crate::rg::messages::messages() {
            eprintln_locked!($($tt)*);
        }
    }
}

/// Like message, but sets ripgrep's "errored" flag, which controls the exit
/// status.
macro_rules! err_message {
    ($($tt:tt)*) => {
        $crate::rg::messages::set_errored();
        message!($($tt)*);
    }
}

/// Emit a non-fatal ignore-related error message (like a parse error), unless
/// ignore-messages were disabled.
macro_rules! ignore_message {
    ($($tt:tt)*) => {
        if $crate::rg::messages::messages() && $crate::rg::messages::ignore_messages() {
            eprintln_locked!($($tt)*);
        }
    }
}

/// ripgrep's `log::debug!`/`log::trace!`: there is no logger (`--debug` and `--trace`
/// run the real rg), so they only check their arguments.
macro_rules! debug {
    ($($tt:tt)*) => {
        if false {
            let _ = format!($($tt)*);
        }
    }
}

macro_rules! trace {
    ($($tt:tt)*) => {
        debug!($($tt)*)
    }
}

/// Returns true if and only if messages should be shown.
pub(crate) fn messages() -> bool {
    with(|s| s.messages).unwrap_or(false)
}

/// Set whether messages should be shown or not.
///
/// By default, they are not shown.
pub(crate) fn set_messages(yes: bool) {
    with(|s| s.messages = yes);
}

/// Returns true if and only if "ignore" related messages should be shown.
pub(crate) fn ignore_messages() -> bool {
    with(|s| s.ignore_messages).unwrap_or(false)
}

/// Set whether "ignore" related messages should be shown or not.
///
/// By default, they are not shown.
///
/// Note that this is overridden if `messages` is disabled. Namely, if
/// `messages` is disabled, then "ignore" messages are never shown, regardless
/// of this setting.
pub(crate) fn set_ignore_messages(yes: bool) {
    with(|s| s.ignore_messages = yes);
}

/// Returns true if and only if ripgrep came across a non-fatal error.
pub(crate) fn errored() -> bool {
    with(|s| s.errored).unwrap_or(false)
}

/// Indicate that ripgrep has come across a non-fatal error.
///
/// Callers should not use this directly. Instead, it is called automatically
/// via the `err_message` macro.
pub(crate) fn set_errored() {
    with(|s| s.errored = true);
}
