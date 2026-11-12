use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

mod default_types;
pub mod gitignore;
pub mod overrides;
pub mod types;
pub mod walk;

#[derive(Debug)]
pub enum Error {
    Partial(Vec<Error>),
    WithLineNumber { line: u64, err: Box<Error> },
    WithPath { path: PathBuf, err: Box<Error> },
    WithDepth { depth: usize, err: Box<Error> },
    Loop { ancestor: PathBuf, child: PathBuf },
    Io(io::Error),
    Glob { glob: String, err: String },
    UnrecognizedFileType(String),
    InvalidDefinition,
}

impl Clone for Error {
    fn clone(&self) -> Error {
        match self {
            Error::Partial(errors) => Error::Partial(errors.clone()),
            Error::WithLineNumber { line, err } => Error::WithLineNumber {
                line: *line,
                err: err.clone(),
            },
            Error::WithPath { path, err } => Error::WithPath {
                path: path.clone(),
                err: err.clone(),
            },
            Error::WithDepth { depth, err } => Error::WithDepth {
                depth: *depth,
                err: err.clone(),
            },
            Error::Loop { ancestor, child } => Error::Loop {
                ancestor: ancestor.clone(),
                child: child.clone(),
            },
            Error::Io(err) => match err.raw_os_error() {
                Some(code) => Error::Io(io::Error::from_raw_os_error(code)),
                None => Error::Io(io::Error::new(err.kind(), err.to_string())),
            },
            Error::Glob { glob, err } => Error::Glob {
                glob: glob.clone(),
                err: err.clone(),
            },
            Error::UnrecognizedFileType(name) => Error::UnrecognizedFileType(name.clone()),
            Error::InvalidDefinition => Error::InvalidDefinition,
        }
    }
}

impl Error {
    fn is_io(&self) -> bool {
        match self {
            Error::Partial(errors) => errors.len() == 1 && errors[0].is_io(),
            Error::WithLineNumber { err, .. }
            | Error::WithPath { err, .. }
            | Error::WithDepth { err, .. } => err.is_io(),
            Error::Io(_) => true,
            Error::Loop { .. }
            | Error::Glob { .. }
            | Error::UnrecognizedFileType(_)
            | Error::InvalidDefinition => false,
        }
    }

    fn with_path(self, path: &Path) -> Error {
        Error::WithPath {
            path: path.to_path_buf(),
            err: Box::new(self),
        }
    }

    fn tagged(self, path: &Path, line: u64) -> Error {
        let with_line = Error::WithLineNumber {
            line,
            err: Box::new(self),
        };
        if path.as_os_str().is_empty() {
            return with_line;
        }
        with_line.with_path(path)
    }
}

impl std::error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Partial(errors) => {
                let messages: Vec<String> = errors.iter().map(Error::to_string).collect();
                f.write_str(&messages.join("\n"))
            }
            Error::WithLineNumber { line, err } => write!(f, "line {line}: {err}"),
            Error::WithPath { path, err } => write!(f, "{}: {err}", path.display()),
            Error::WithDepth { err, .. } => err.fmt(f),
            Error::Loop { ancestor, child } => write!(
                f,
                "File system loop found: {} points to an ancestor {}",
                child.display(),
                ancestor.display()
            ),
            Error::Io(err) => err.fmt(f),
            Error::Glob { glob, err } => write!(f, "error parsing glob '{glob}': {err}"),
            Error::UnrecognizedFileType(name) => write!(f, "unrecognized file type: {name}"),
            Error::InvalidDefinition => {
                f.write_str("invalid definition (format is type:glob, e.g., html:*.html)")
            }
        }
    }
}

#[derive(Debug, Default)]
struct PartialErrors(Vec<Error>);

impl PartialErrors {
    fn push(&mut self, error: Error) {
        self.0.push(error);
    }

    fn maybe_push(&mut self, error: Option<Error>) {
        if let Some(error) = error {
            self.push(error);
        }
    }

    fn maybe_push_ignore_io(&mut self, error: Option<Error>) {
        if let Some(error) = error
            && !error.is_io()
        {
            self.push(error);
        }
    }

    fn into_error(mut self) -> Option<Error> {
        match self.0.len() {
            0 => None,
            1 => self.0.pop(),
            _ => Some(Error::Partial(self.0)),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Match<T> {
    None,
    Ignore(T),
    Whitelist(T),
}

impl<T> Match<T> {
    pub fn is_none(&self) -> bool {
        matches!(self, Match::None)
    }

    pub fn is_ignore(&self) -> bool {
        matches!(self, Match::Ignore(_))
    }

    pub fn is_whitelist(&self) -> bool {
        matches!(self, Match::Whitelist(_))
    }

    pub fn invert(self) -> Match<T> {
        match self {
            Match::None => Match::None,
            Match::Ignore(value) => Match::Whitelist(value),
            Match::Whitelist(value) => Match::Ignore(value),
        }
    }

    pub fn or(self, other: Match<T>) -> Match<T> {
        if self.is_none() { other } else { self }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WalkState {
    Continue,
    Skip,
    Quit,
}

fn strip_prefix<'a>(prefix: &Path, path: &'a Path) -> Option<&'a Path> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let rest = path
            .as_os_str()
            .as_bytes()
            .strip_prefix(prefix.as_os_str().as_bytes())?;
        Some(Path::new(std::ffi::OsStr::from_bytes(rest)))
    }
    #[cfg(not(unix))]
    {
        path.strip_prefix(prefix).ok()
    }
}

fn is_file_name(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        !path.as_os_str().as_bytes().contains(&b'/')
    }
    #[cfg(not(unix))]
    {
        path.parent()
            .is_some_and(|parent| parent.as_os_str().is_empty())
    }
}

fn file_name(path: &Path) -> Option<&std::ffi::OsStr> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let bytes = path.as_os_str().as_bytes();
        if bytes.is_empty() {
            return None;
        }
        let start = bytes
            .iter()
            .rposition(|&b| b == b'/')
            .map_or(0, |slash| slash + 1);
        let name = &bytes[start..];
        if name == b"." || name == b".." {
            return None;
        }
        Some(std::ffi::OsStr::from_bytes(name))
    }
    #[cfg(not(unix))]
    {
        path.file_name()
    }
}
