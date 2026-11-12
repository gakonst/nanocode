// `parse_human_readable_size` and its error, and `patterns_from_reader` with
// `InvalidPatternError`, are copied from grep-cli 0.1.12 (`src/human.rs`,
// `src/pattern.rs`, in ripgrep, https://github.com/BurntSushi/ripgrep):
// Copyright (c) 2015 Andrew Gallant. Licensed under the MIT License (see NOTICE).

//! What ripgrep takes from `grep-cli`, which psh does not depend on (D020, D1133): size
//! parsing and pattern files as there, and stdout as an ANSI or plain stream over the
//! utility's fd 1 ([`SyncWriter`]) instead of the process's.

use std::io::{self, BufWriter, LineWriter, Write};

use crate::ansi::{ColorSpec, WriteColor};
use bstr::ByteSlice;
use bstr::io::BufReadExt;
use psh_engine::util::SyncWriter;

/// An error that occurs when parsing a human readable size description.
///
/// This error provides an end user friendly message describing why the
/// description couldn't be parsed and what the expected format is.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseSizeError {
    original: String,
    kind: ParseSizeErrorKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ParseSizeErrorKind {
    InvalidFormat,
    InvalidInt(std::num::ParseIntError),
    Overflow,
}

impl ParseSizeError {
    fn format(original: &str) -> ParseSizeError {
        ParseSizeError {
            original: original.to_string(),
            kind: ParseSizeErrorKind::InvalidFormat,
        }
    }

    fn int(original: &str, err: std::num::ParseIntError) -> ParseSizeError {
        ParseSizeError {
            original: original.to_string(),
            kind: ParseSizeErrorKind::InvalidInt(err),
        }
    }

    fn overflow(original: &str) -> ParseSizeError {
        ParseSizeError {
            original: original.to_string(),
            kind: ParseSizeErrorKind::Overflow,
        }
    }
}

impl std::error::Error for ParseSizeError {}

impl std::fmt::Display for ParseSizeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use self::ParseSizeErrorKind::*;

        match self.kind {
            InvalidFormat => write!(
                f,
                "invalid format for size '{}', which should be a non-empty \
                 sequence of digits followed by an optional 'K', 'M' or 'G' \
                 suffix",
                self.original
            ),
            InvalidInt(ref err) => write!(
                f,
                "invalid integer found in size '{}': {}",
                self.original, err
            ),
            Overflow => write!(f, "size too big in '{}'", self.original),
        }
    }
}

/// Parse a human readable size like `2M` into a corresponding number of bytes.
///
/// Supported size suffixes are `K` (for kilobyte), `M` (for megabyte) and `G`
/// (for gigabyte). If a size suffix is missing, then the size is interpreted
/// as bytes. If the size is too big to fit into a `u64`, then this returns an
/// error.
///
/// Additional suffixes may be added over time.
pub fn parse_human_readable_size(size: &str) -> Result<u64, ParseSizeError> {
    let digits_end =
        size.as_bytes().iter().take_while(|&b| b.is_ascii_digit()).count();
    let digits = &size[..digits_end];
    if digits.is_empty() {
        return Err(ParseSizeError::format(size));
    }
    let value =
        digits.parse::<u64>().map_err(|e| ParseSizeError::int(size, e))?;

    let suffix = &size[digits_end..];
    if suffix.is_empty() {
        return Ok(value);
    }
    let bytes = match suffix {
        "K" => value.checked_mul(1 << 10),
        "M" => value.checked_mul(1 << 20),
        "G" => value.checked_mul(1 << 30),
        _ => return Err(ParseSizeError::format(size)),
    };
    bytes.ok_or_else(|| ParseSizeError::overflow(size))
}

/// An error that occurs when a pattern could not be converted to valid UTF-8.
///
/// The purpose of this error is to give a more targeted failure mode for
/// patterns written by end users that are not valid UTF-8.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvalidPatternError {
    original: String,
    valid_up_to: usize,
}

impl std::error::Error for InvalidPatternError {}

impl std::fmt::Display for InvalidPatternError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "found invalid UTF-8 in pattern at byte offset {}: {} \
             (disable Unicode mode and use hex escape sequences to match \
             arbitrary bytes in a pattern, e.g., '(?-u)\\xFF')",
            self.valid_up_to, self.original,
        )
    }
}

/// Convert arbitrary bytes into a regular expression pattern.
///
/// This conversion fails if the given pattern is not valid UTF-8, in which
/// case, a targeted error with more information about where the invalid UTF-8
/// occurs is given. The error also suggests the use of hex escape sequences,
/// which are supported by many regex engines.
pub fn pattern_from_bytes(
    pattern: &[u8],
) -> Result<&str, InvalidPatternError> {
    std::str::from_utf8(pattern).map_err(|err| InvalidPatternError {
        original: pattern.escape_bytes().to_string(),
        valid_up_to: err.valid_up_to(),
    })
}

/// Read patterns from any reader, one per line.
///
/// If there was a problem reading or if any of the patterns contain invalid
/// UTF-8, then an error is returned. If there was a problem with a specific
/// pattern, then the error message will include the line number.
pub fn patterns_from_reader<R: io::Read>(rdr: R) -> io::Result<Vec<String>> {
    let mut patterns = vec![];
    let mut line_number = 0;
    io::BufReader::new(rdr).for_byte_line(|line| {
        line_number += 1;
        match pattern_from_bytes(line) {
            Ok(pattern) => {
                patterns.push(pattern.to_string());
                Ok(true)
            }
            Err(err) => {
                Err(io::Error::other(format!("{}: {}", line_number, err)))
            }
        }
    })?;
    Ok(patterns)
}

/// grep-cli's `StandardStream`: stdout, line or block buffered, with ANSI colors or
/// none. Never the Windows console API (ARCH "Windows").
pub enum StandardStream {
    Ansi(crate::ansi::Ansi<Buffered>),
    NoColor(crate::ansi::NoColor<Buffered>),
}

pub enum Buffered {
    Line(LineWriter<SyncWriter>),
    Block(BufWriter<SyncWriter>),
}

impl Write for Buffered {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Buffered::Line(w) => w.write(buf),
            Buffered::Block(w) => w.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Buffered::Line(w) => w.flush(),
            Buffered::Block(w) => w.flush(),
        }
    }
}

/// A stdout stream; `color`: ANSI escapes (the choice is made, `Auto` included, by
/// the caller from the utility's TTY state and environment).
pub fn stdout(out: SyncWriter, line: bool, color: bool) -> StandardStream {
    let b = if line {
        Buffered::Line(LineWriter::new(out))
    } else {
        Buffered::Block(BufWriter::new(out))
    };
    if color {
        StandardStream::Ansi(crate::ansi::Ansi::new(b))
    } else {
        StandardStream::NoColor(crate::ansi::NoColor::new(b))
    }
}

impl Write for StandardStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            StandardStream::Ansi(w) => w.write(buf),
            StandardStream::NoColor(w) => w.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            StandardStream::Ansi(w) => w.flush(),
            StandardStream::NoColor(w) => w.flush(),
        }
    }
}

impl WriteColor for StandardStream {
    fn supports_color(&self) -> bool {
        matches!(self, StandardStream::Ansi(_))
    }

    fn supports_hyperlinks(&self) -> bool {
        matches!(self, StandardStream::Ansi(_))
    }

    fn set_color(&mut self, spec: &ColorSpec) -> io::Result<()> {
        match self {
            StandardStream::Ansi(w) => w.set_color(spec),
            StandardStream::NoColor(w) => w.set_color(spec),
        }
    }

    fn set_hyperlink(
        &mut self,
        link: &crate::ansi::HyperlinkSpec,
    ) -> io::Result<()> {
        match self {
            StandardStream::Ansi(w) => w.set_hyperlink(link),
            StandardStream::NoColor(w) => w.set_hyperlink(link),
        }
    }

    fn reset(&mut self) -> io::Result<()> {
        match self {
            StandardStream::Ansi(w) => w.reset(),
            StandardStream::NoColor(w) => w.reset(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(parse_human_readable_size("123").unwrap(), 123);
        assert_eq!(parse_human_readable_size("123K").unwrap(), 123 << 10);
        assert_eq!(parse_human_readable_size("123M").unwrap(), 123 << 20);
        assert_eq!(parse_human_readable_size("123G").unwrap(), 123 << 30);
        assert!(parse_human_readable_size("").is_err());
        assert!(parse_human_readable_size("a").is_err());
        assert!(parse_human_readable_size("123k").is_err());
        assert!(parse_human_readable_size("99999999999999999999G").is_err());
    }

    #[test]
    fn patterns() {
        let p =
            patterns_from_reader(&b"foo\nbar\\s+foo\n[a-z]{3}\n"[..]).unwrap();
        assert_eq!(p, vec!["foo", r"bar\s+foo", "[a-z]{3}"]);
        let e = patterns_from_reader(&b"ok\nabc\xFFxyz\n"[..]).unwrap_err();
        assert!(e.to_string().starts_with("2: found invalid UTF-8"), "{e}");
    }
}
