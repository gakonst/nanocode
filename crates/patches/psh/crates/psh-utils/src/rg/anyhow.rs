//! The part of `anyhow` ripgrep's flag code uses (D1133): an error that is a chain of
//! messages (`{:#}` joins them with `: `, as anyhow does), `Context`, and the `anyhow!`,
//! `bail!` and `ensure!` macros. It remembers the kind of an I/O error it was made
//! from, so a broken pipe can still be told apart.

use std::fmt;
use std::io;

pub type Result<T> = std::result::Result<T, Error>;

pub struct Error {
    /// Outermost first.
    chain: Vec<String>,
    io_kind: Option<io::ErrorKind>,
}

impl Error {
    pub fn msg(msg: impl fmt::Display) -> Error {
        Error { chain: vec![msg.to_string()], io_kind: None }
    }

    fn context(mut self, c: impl fmt::Display) -> Error {
        self.chain.insert(0, c.to_string());
        self
    }

    /// The kind of the I/O error this came from, if it did.
    pub fn io_kind(&self) -> Option<io::ErrorKind> {
        self.io_kind
    }
}

impl<E: std::error::Error + Send + Sync + 'static> From<E> for Error {
    fn from(e: E) -> Error {
        let io_kind = (&e as &dyn std::any::Any)
            .downcast_ref::<io::Error>()
            .map(io::Error::kind);
        let mut chain = vec![e.to_string()];
        let mut source = e.source();
        while let Some(s) = source {
            chain.push(s.to_string());
            source = s.source();
        }
        Error { chain, io_kind }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if f.alternate() {
            f.write_str(&self.chain.join(": "))
        } else {
            f.write_str(self.chain.first().map_or("", String::as_str))
        }
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:#}")
    }
}

/// `.context(..)` and `.with_context(|| ..)` on results and options.
pub trait Context<T> {
    fn context<C: fmt::Display>(self, c: C) -> Result<T>;
    fn with_context<C: fmt::Display, F: FnOnce() -> C>(
        self,
        f: F,
    ) -> Result<T>;
}

impl<T, E: Into<Error>> Context<T> for std::result::Result<T, E> {
    fn context<C: fmt::Display>(self, c: C) -> Result<T> {
        self.map_err(|e| e.into().context(c))
    }

    fn with_context<C: fmt::Display, F: FnOnce() -> C>(
        self,
        f: F,
    ) -> Result<T> {
        self.map_err(|e| e.into().context(f()))
    }
}

impl<T> Context<T> for Option<T> {
    fn context<C: fmt::Display>(self, c: C) -> Result<T> {
        self.ok_or_else(|| Error::msg(c))
    }

    fn with_context<C: fmt::Display, F: FnOnce() -> C>(
        self,
        f: F,
    ) -> Result<T> {
        self.ok_or_else(|| Error::msg(f()))
    }
}

macro_rules! anyhow {
    ($fmt:literal $($t:tt)*) => {
        $crate::rg::anyhow::Error::msg(format!($fmt $($t)*))
    };
    ($e:expr $(,)?) => {
        $crate::rg::anyhow::Error::msg($e)
    };
}

macro_rules! bail {
    ($($t:tt)*) => {
        return Err($crate::rg::anyhow::anyhow!($($t)*))
    };
}

macro_rules! ensure {
    ($cond:expr, $($t:tt)*) => {
        if !$cond {
            $crate::rg::anyhow::bail!($($t)*);
        }
    };
}

pub(crate) use {anyhow, bail, ensure};
