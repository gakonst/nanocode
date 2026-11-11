//! [`Error`]: every way a snapshot build or a call can fail.

use std::fmt;

/// Everything that is not a settled value. A call that returns an `Error` is over: the
/// instance is idle again (or, after [`Error::Panicked`], poisoned), its pending host calls
/// are dropped, and nothing of the call can still run.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Error {
    /// A source did not compile (a syntax error, or one of the early errors JS reports
    /// before running anything)
    Syntax { source: String, message: String },
    /// A setup module's graph did not load: the [`crate::Loader`] failed (its message). A
    /// syntax or link error in it is [`Error::Syntax`]
    Load { module: String, message: String },
    /// The script threw, or the entry's promise was rejected
    Exception(Exception),
    /// The call used up its fuel budget ([`crate::Limits::fuel`])
    OutOfFuel { budget: u64 },
    /// The call ran past its wall-clock deadline ([`crate::Limits::deadline`])
    Deadline,
    /// The heap reached its cap ([`crate::Limits::memory`]), or the host refused memory
    /// below it. Scripts cannot catch it.
    OutOfMemory,
    /// The host stopped the call ([`crate::Interrupt`])
    Interrupted,
    /// The entry's promise can never settle: nothing is left to run and no host call is
    /// pending
    NeverSettles,
    /// A value is not plain data (a function, an accessor, a class instance, ...: the
    /// message names its path), or a wire buffer is corrupt
    Data(String),
    /// The host used the API wrongly: no such function, an unknown or answered host call, a
    /// second call while one is in progress, a host call during setup
    Usage(String),
    /// djs panicked (a bug in djs). The instance is poisoned; the snapshot and every other
    /// instance are unaffected.
    Panicked(String),
    /// The instance panicked earlier, or its reset failed: drop it and instantiate a new one
    Poisoned,
}

/// A thrown value, as the host sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Exception {
    /// `Name: message` for an Error, else the value as `console.log` shows it
    pub message: String,
    /// The Error's `stack`, if it has one
    pub stack: Option<String>,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Syntax { source, message } => write!(f, "{source}: {message}"),
            Error::Load { module, message } => write!(f, "{module}: {message}"),
            Error::Exception(e) => write!(f, "uncaught {}", e.message),
            Error::OutOfFuel { budget } => write!(f, "out of fuel (budget {budget})"),
            Error::Deadline => f.write_str("deadline exceeded"),
            Error::OutOfMemory => f.write_str("out of memory"),
            Error::Interrupted => f.write_str("interrupted by the host"),
            Error::NeverSettles => f.write_str("the call's promise never settles"),
            Error::Data(m) => write!(f, "not plain data: {m}"),
            Error::Usage(m) => write!(f, "API misuse: {m}"),
            Error::Panicked(m) => write!(f, "djs panicked: {m}"),
            Error::Poisoned => f.write_str("the instance is poisoned by an earlier panic"),
        }
    }
}

impl std::error::Error for Error {}

/// The error a host function answers with: thrown at the call (sync) or rejecting its
/// promise (async), as a new Error object of `kind`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Throw {
    pub kind: ErrorKind,
    pub message: String,
}

impl Throw {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Throw {
        Throw {
            kind,
            message: message.into(),
        }
    }

    /// An `Error`.
    pub fn error(message: impl Into<String>) -> Throw {
        Throw::new(ErrorKind::Error, message)
    }

    /// A `TypeError`.
    pub fn type_error(message: impl Into<String>) -> Throw {
        Throw::new(ErrorKind::TypeError, message)
    }

    /// A `RangeError`.
    pub fn range_error(message: impl Into<String>) -> Throw {
        Throw::new(ErrorKind::RangeError, message)
    }
}

/// The constructor of a [`Throw`]'s Error object.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
    Error,
    TypeError,
    RangeError,
    ReferenceError,
    SyntaxError,
}

impl ErrorKind {
    pub(crate) fn core(self) -> djs_core::vm::ErrorKind {
        use djs_core::vm::ErrorKind as K;
        match self {
            ErrorKind::Error => K::Error,
            ErrorKind::TypeError => K::TypeError,
            ErrorKind::RangeError => K::RangeError,
            ErrorKind::ReferenceError => K::ReferenceError,
            ErrorKind::SyntaxError => K::SyntaxError,
        }
    }
}
