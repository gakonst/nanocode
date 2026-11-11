// Copied from fd (https://github.com/sharkdp/fd, commit 193d0df), src/fmt/mod.rs,
// with the psh patches marked `psh:` (D1138):
// Copyright (c) 2017-present The fd developers. Licensed under the MIT License (see NOTICE).

// psh: paths are the shell's bytes (`/`, and `\` on Windows, separate); a plain
// leftmost-first search stands in for aho-corasick.

mod input;

use std::borrow::Cow;
use std::fmt::{self, Display, Formatter};

pub use self::input::strip_current_dir;
use self::input::{basename, dirname, remove_extension};
use super::output::is_separator;

/// Designates what should be written to a buffer
///
/// Each `Token` contains either text, or a placeholder variant, which will be used to generate
/// commands after all tokens for a given command template have been collected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Token {
    Placeholder,
    Basename,
    Parent,
    NoExt,
    BasenameNoExt,
    Text(String),
}

impl Display for Token {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match *self {
            Self::Placeholder => f.write_str("{}")?,
            Self::Basename => f.write_str("{/}")?,
            Self::Parent => f.write_str("{//}")?,
            Self::NoExt => f.write_str("{.}")?,
            Self::BasenameNoExt => f.write_str("{/.}")?,
            Self::Text(ref string) => f.write_str(string)?,
        }
        Ok(())
    }
}

/// A parsed format string
///
/// This is either a collection of `Token`s including at least one placeholder variant,
/// or a fixed text.
#[derive(Clone, Debug, PartialEq)]
pub enum FormatTemplate {
    Tokens(Vec<Token>),
    Text(String),
}

/// The patterns, in aho-corasick's order (leftmost first, then the first listed).
const PLACEHOLDERS: [&str; 7] = ["{{", "}}", "{}", "{/}", "{//}", "{.}", "{/.}"];

/// psh: the first placeholder match: its pattern id, start and end.
fn find_placeholder(s: &str) -> Option<(u32, usize, usize)> {
    let b = s.as_bytes();
    (0..b.len()).find_map(|i| {
        PLACEHOLDERS
            .iter()
            .position(|p| b[i..].starts_with(p.as_bytes()))
            .map(|id| (id as u32, i, i + PLACEHOLDERS[id].len()))
    })
}

impl FormatTemplate {
    pub fn has_tokens(&self) -> bool {
        matches!(self, Self::Tokens(_))
    }

    pub fn parse(fmt: &str) -> Self {
        // NOTE: we assume that { and } have the same length
        const BRACE_LEN: usize = '{'.len_utf8();
        let mut tokens = Vec::new();
        let mut remaining = fmt;
        let mut buf = String::new();
        while let Some((id, start, end)) = find_placeholder(remaining) {
            match id {
                0 | 1 => {
                    // we found an escaped {{ or }}, so add
                    // everything up to the first char to the buffer
                    // then skip the second one.
                    buf += &remaining[..start + BRACE_LEN];
                    remaining = &remaining[end..];
                }
                id if !remaining[end..].starts_with('}') => {
                    buf += &remaining[..start];
                    if !buf.is_empty() {
                        tokens.push(Token::Text(std::mem::take(&mut buf)));
                    }
                    tokens.push(token_from_pattern_id(id));
                    remaining = &remaining[end..];
                }
                _ => {
                    // We got a normal pattern, but the final "}"
                    // is escaped, so add up to that to the buffer, then
                    // skip the final }
                    buf += &remaining[..end];
                    remaining = &remaining[end + BRACE_LEN..];
                }
            }
        }
        // Add the rest of the string to the buffer, and add the final buffer to the tokens
        if !remaining.is_empty() {
            buf += remaining;
        }
        if tokens.is_empty() {
            // No placeholders were found, so just return the text
            return Self::Text(buf);
        }
        // Add final text segment
        if !buf.is_empty() {
            tokens.push(Token::Text(buf));
        }
        debug_assert!(!tokens.is_empty());
        Self::Tokens(tokens)
    }

    /// Generate a result string from this template. If path_separator is Some, then it will replace
    /// the path separator in all placeholder tokens. Fixed text and tokens are not affected by
    /// path separator substitution.
    pub fn generate(&self, path: &[u8], path_separator: Option<&str>) -> Vec<u8> {
        use Token::*;

        match *self {
            Self::Tokens(ref tokens) => {
                let mut s = Vec::new();
                for token in tokens {
                    match token {
                        Basename => s.extend_from_slice(&Self::replace_separator(
                            basename(path),
                            path_separator,
                        )),
                        BasenameNoExt => s.extend_from_slice(&Self::replace_separator(
                            &remove_extension(basename(path)),
                            path_separator,
                        )),
                        NoExt => s.extend_from_slice(&Self::replace_separator(
                            &remove_extension(path),
                            path_separator,
                        )),
                        Parent => s.extend_from_slice(&Self::replace_separator(
                            &dirname(path),
                            path_separator,
                        )),
                        Placeholder => {
                            s.extend_from_slice(&Self::replace_separator(path, path_separator))
                        }
                        Text(string) => s.extend_from_slice(string.as_bytes()),
                    }
                }
                s
            }
            Self::Text(ref text) => text.as_bytes().to_vec(),
        }
    }

    /// Replace the path separator in the input with the custom separator string. If path_separator
    /// is None, simply return a borrowed Cow<OsStr> of the input. Otherwise, the input is
    /// interpreted as a Path and its components are iterated through and re-joined into a new
    /// OsString.
    // psh: `Path::components` on bytes: a root, then the names (no empty ones, no `.`
    // but a leading one); a drive prefix (`C:`) as is. UNC paths are not walked.
    fn replace_separator<'a>(path: &'a [u8], path_separator: Option<&str>) -> Cow<'a, [u8]> {
        // fast-path - no replacement necessary
        let Some(path_separator) = path_separator else {
            return Cow::Borrowed(path);
        };
        let sep = path_separator.as_bytes();
        let mut out = Vec::with_capacity(path.len());
        let mut rest = path;
        if cfg!(windows) && rest.len() >= 2 && rest[0].is_ascii_alphabetic() && rest[1] == b':' {
            out.extend_from_slice(&rest[..2]);
            rest = &rest[2..];
        }
        if rest.first().copied().is_some_and(is_separator) {
            // Root directory is always replaced with the custom separator.
            out.extend_from_slice(sep);
        }
        let mut components = rest
            .split(|&b| is_separator(b))
            .enumerate()
            .filter(|(i, c)| !c.is_empty() && (*c != b"." || *i == 0))
            .map(|(_, c)| c)
            .peekable();
        // Everything else is joined normally, with a trailing separator if we're not last
        while let Some(comp) = components.next() {
            out.extend_from_slice(comp);
            if components.peek().is_some() {
                out.extend_from_slice(sep);
            }
        }
        Cow::Owned(out)
    }
}

// Convert the id from an aho-corasick match to the
// appropriate token
fn token_from_pattern_id(id: u32) -> Token {
    use Token::*;
    match id {
        2 => Placeholder,
        3 => Basename,
        4 => Parent,
        5 => NoExt,
        6 => BasenameNoExt,
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod fmt_tests {
    use super::*;

    #[test]
    fn parse_no_placeholders() {
        let templ = FormatTemplate::parse("This string has no placeholders");
        assert_eq!(
            templ,
            FormatTemplate::Text("This string has no placeholders".into())
        );
    }

    #[test]
    fn parse_only_brace_escapes() {
        let templ = FormatTemplate::parse("This string only has escapes like {{ and }}");
        assert_eq!(
            templ,
            FormatTemplate::Text("This string only has escapes like { and }".into())
        );
    }

    #[test]
    fn all_placeholders() {
        use Token::*;

        let templ = FormatTemplate::parse(
            "{{path={} \
            basename={/} \
            parent={//} \
            noExt={.} \
            basenameNoExt={/.} \
            }}",
        );
        assert_eq!(
            templ,
            FormatTemplate::Tokens(vec![
                Text("{path=".into()),
                Placeholder,
                Text(" basename=".into()),
                Basename,
                Text(" parent=".into()),
                Parent,
                Text(" noExt=".into()),
                NoExt,
                Text(" basenameNoExt=".into()),
                BasenameNoExt,
                Text(" }".into()),
            ])
        );

        // psh: the path as bytes.
        let path = b"a/folder/file.txt";

        let expanded = String::from_utf8(templ.generate(path, Some("/"))).unwrap();

        assert_eq!(
            expanded,
            "{path=a/folder/file.txt \
            basename=file.txt \
            parent=a/folder \
            noExt=a/folder/file \
            basenameNoExt=file }"
        );
    }

    // psh: the separator tests of upstream's exec/mod.rs.
    #[test]
    fn generate_custom_path_separator() {
        let arg = FormatTemplate::Tokens(vec![Token::Placeholder]);
        let check = |input: &str, expected: &str| {
            assert_eq!(
                arg.generate(input.as_bytes(), Some("#")),
                expected.as_bytes()
            );
        };
        check("foo", "foo");
        check("foo/bar", "foo#bar");
        check("/foo/bar/baz", "#foo#bar#baz");
        check("./foo//bar/", ".#foo#bar");
    }
}
