mod machine;
mod parse;
mod program;
mod properties;
mod set;
mod unicode;

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Error {
    text: String,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.text)
    }
}

impl std::error::Error for Error {}

#[derive(Debug)]
pub(crate) struct Regex {
    program: program::Program,
}

impl Regex {
    pub(crate) fn new(pattern: &str) -> Result<Self, Error> {
        let parsed = parse::parse(pattern)?;
        Ok(Self {
            program: program::compile(parsed),
        })
    }

    pub(crate) fn is_match(&self, text: &str) -> bool {
        machine::is_match(&self.program, text)
    }
}
