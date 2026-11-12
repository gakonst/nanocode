//! Readline interface for REPLs
//!
//! This module provides a common interface for reading lines from the console, with support for history and completion.

use std::{io, path::Path};

type OtherError = Box<dyn core::error::Error>;
type OtherResult<T> = Result<T, OtherError>;

pub enum ReadlineResult {
    Line(String),
    Eof,
    Interrupt,
    Io(io::Error),
    #[cfg(unix)]
    OsError(String),
    Other(OtherError),
}

fn stdio_readline(prompt: &str) -> ReadlineResult {
    use std::io::prelude::*;
    eprint!("{prompt}");
    if let Err(e) = io::stderr().flush() {
        return ReadlineResult::Io(e);
    }

    let line = io::stdin().lock().lines().next();
    match line {
        Some(Ok(line)) => ReadlineResult::Line(line),
        None => ReadlineResult::Eof,
        Some(Err(e)) if e.kind() == io::ErrorKind::Interrupted => ReadlineResult::Interrupt,
        Some(Err(e)) => ReadlineResult::Io(e),
    }
}

#[allow(unused)]
pub mod basic_readline {
    use super::*;

    pub trait Helper {}

    impl<T> Helper for T {}

    pub struct Readline<H: Helper> {
        helper: H,
    }

    impl<H: Helper> Readline<H> {
        pub const fn new(helper: H) -> Self {
            Self { helper }
        }

        pub fn load_history(&mut self, _path: &Path) -> OtherResult<()> {
            Ok(())
        }

        pub fn save_history(&mut self, _path: &Path) -> OtherResult<()> {
            Ok(())
        }

        pub fn add_history_entry(&mut self, _entry: &str) -> OtherResult<()> {
            Ok(())
        }

        pub fn readline(&mut self, prompt: &str) -> ReadlineResult {
            stdio_readline(prompt)
        }
    }
}

use basic_readline as readline_inner;

pub use readline_inner::Helper;

pub struct Readline<H: Helper>(readline_inner::Readline<H>);

impl<H: Helper> Readline<H> {
    pub fn new(helper: H) -> Self {
        Self(readline_inner::Readline::new(helper))
    }

    pub fn load_history(&mut self, path: &Path) -> OtherResult<()> {
        self.0.load_history(path)
    }

    pub fn save_history(&mut self, path: &Path) -> OtherResult<()> {
        self.0.save_history(path)
    }

    pub fn add_history_entry(&mut self, entry: &str) -> OtherResult<()> {
        self.0.add_history_entry(entry)
    }

    pub fn readline(&mut self, prompt: &str) -> ReadlineResult {
        self.0.readline(prompt)
    }
}
