//! POSIX patterns in the syntax of Rust's `regex` crate (D1131): the search utilities
//! (`grep`, `sed`, `find -regex`) parse with this crate's parser, so syntax, errors and
//! bracket rules are GNU's, and match with the ripgrep crates where that gives the same
//! answer. The caller falls back to [`Regex`](crate::Regex) when it does not:
//! back-references, and spans (`grep -o`, `sed s///`) of a pattern whose
//! leftmost-first match can differ from the leftmost-longest one.

use alloc::string::String;
use core::fmt::Write;

use crate::class::{Class, INVALID, Set, Unit};
use crate::parse::{Assert, Node, parse};
use crate::{Error, Options};

/// A pattern translated by [`to_rust_syntax`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Translation {
    /// The pattern for `regex-syntax`, or `None` when it uses back-references (which
    /// Rust's `regex` lacks). Its meaning does not depend on the parser's flags except
    /// case folding: anchors, `.` and the newline rules are spelled out from
    /// [`Options::newline`], and a C-locale (`!utf8`) pattern is byte-oriented.
    pub pattern: Option<String>,
    /// The leftmost-first match (Rust's) can end elsewhere than the leftmost-longest one
    /// (POSIX's): the pattern has an alternation or more than one variable-length part.
    /// Which lines match is the same either way; only spans can differ.
    pub ambiguous: bool,
    /// The number of parenthesized subexpressions; the Rust pattern numbers its groups
    /// the same way.
    pub groups: usize,
}

/// Translates a POSIX pattern (ERE or BRE with GNU's extensions, as
/// [`Regex::new`](crate::Regex::new) reads it) to Rust's regex syntax. The errors are
/// [`Regex::new`](crate::Regex::new)'s, except those its compiler finds (sizes).
pub fn to_rust_syntax(pattern: &[u8], opts: Options) -> Result<Translation, Error> {
    let parsed = parse(pattern, opts)?;
    let ambiguous = ambiguous(&parsed.node);
    let pattern = if parsed.backrefs {
        None
    } else {
        let mut e = Emit {
            out: String::new(),
            opts,
        };
        if !opts.utf8 {
            e.out.push_str("(?-u)");
        }
        e.node(&parsed.node);
        Some(e.out)
    };
    Ok(Translation {
        pattern,
        ambiguous,
        groups: parsed.groups,
    })
}

/// Whether leftmost-first and leftmost-longest can disagree on where a match ends. Both
/// start at the leftmost position where any match starts. With no alternation and at
/// most one variable-length part, the rest has a fixed length and greedy repetition
/// tries the longest choice first, so the first match found is the longest.
fn ambiguous(node: &Node) -> bool {
    fn variable(node: &Node, alt: &mut bool) -> usize {
        match node {
            Node::Empty | Node::Lit(_) | Node::Any | Node::Set(_) | Node::Assert(_) => 0,
            // Its length is that of the group it names: count it as variable.
            Node::Backref(_) => 1,
            Node::Group(inner, _) => variable(inner, alt),
            Node::Cat(items) => items.iter().map(|n| variable(n, alt)).sum(),
            Node::Alt(items) => {
                *alt = true;
                items.iter().map(|n| variable(n, alt)).sum()
            }
            Node::Repeat(inner, min, max) => {
                let inside = variable(inner, alt);
                let fixed = *max == Some(*min);
                match (fixed, inside) {
                    (true, 0) => 0,
                    (true, n) if *min <= 1 => n,
                    (true, n) => n * 2,
                    (false, n) => n + 1,
                }
            }
        }
    }
    let mut alt = false;
    let n = variable(node, &mut alt);
    alt || n > 1
}

struct Emit {
    out: String,
    opts: Options,
}

impl Emit {
    fn node(&mut self, node: &Node) {
        match node {
            Node::Empty => self.out.push_str("(?:)"),
            Node::Lit(u) => self.unit(*u),
            Node::Any => {
                if self.opts.newline {
                    self.out.push_str("[^\\n]");
                } else {
                    self.out.push_str("(?s:.)");
                }
            }
            Node::Set(set) => self.set(set),
            Node::Assert(a) => self.out.push_str(match (a, self.opts.newline) {
                (Assert::Bol, true) => "(?m:^)",
                (Assert::Bol, false) => "(?-m:^)",
                (Assert::Eol, true) => "(?m:$)",
                (Assert::Eol, false) => "(?-m:$)",
                (Assert::WordBoundary, _) => "\\b",
                (Assert::NotWordBoundary, _) => "\\B",
                (Assert::WordStart, _) => "\\b{start}",
                (Assert::WordEnd, _) => "\\b{end}",
                (Assert::BufStart, _) => "\\A",
                (Assert::BufEnd, _) => "\\z",
            }),
            Node::Group(inner, _) => {
                self.out.push('(');
                self.node(inner);
                self.out.push(')');
            }
            Node::Cat(items) => {
                for item in items {
                    self.node(item);
                }
            }
            Node::Alt(items) => {
                self.out.push_str("(?:");
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        self.out.push('|');
                    }
                    self.node(item);
                }
                self.out.push(')');
            }
            Node::Repeat(inner, min, max) => {
                let atom = matches!(
                    **inner,
                    Node::Lit(_) | Node::Any | Node::Set(_) | Node::Group(..)
                );
                if atom {
                    self.node(inner);
                } else {
                    self.out.push_str("(?:");
                    self.node(inner);
                    self.out.push(')');
                }
                let _ = match (*min, *max) {
                    (0, None) => write!(self.out, "*"),
                    (1, None) => write!(self.out, "+"),
                    (0, Some(1)) => write!(self.out, "?"),
                    (m, None) => write!(self.out, "{{{m},}}"),
                    (m, Some(n)) if m == n => write!(self.out, "{{{m}}}"),
                    (m, Some(n)) => write!(self.out, "{{{m},{n}}}"),
                };
            }
            // Patterns with back-references are not emitted.
            Node::Backref(_) => {}
        }
    }

    /// One character, outside or inside a class.
    fn unit(&mut self, u: Unit) {
        if u >= INVALID {
            // A byte that is not UTF-8: only itself.
            let _ = write!(self.out, "(?-u:\\x{:02X})", u - INVALID);
        } else if !self.opts.utf8 {
            // Under `(?-u)`, `\xHH` is the byte.
            if (u as u8).is_ascii_alphanumeric() {
                self.out.push(u as u8 as char);
            } else {
                let _ = write!(self.out, "\\x{u:02X}");
            }
        } else {
            match char::from_u32(u) {
                Some(c) if c.is_ascii_alphanumeric() || (!c.is_ascii() && !c.is_control()) => {
                    self.out.push(c)
                }
                _ => {
                    let _ = write!(self.out, "\\x{{{u:X}}}");
                }
            }
        }
    }

    fn class_unit(&mut self, u: Unit) {
        if !self.opts.utf8 {
            let _ = write!(self.out, "\\x{u:02X}");
        } else {
            let _ = write!(self.out, "\\x{{{u:X}}}");
        }
    }

    fn set(&mut self, set: &Set) {
        let mut body = Emit {
            out: String::new(),
            opts: self.opts,
        };
        // Undecodable bytes match no set: ranges stop at the last code point.
        let top = if self.opts.utf8 { 0x10_FFFF } else { 0xFF };
        for &(lo, hi) in &set.ranges {
            if lo > top {
                continue;
            }
            let hi = hi.min(top);
            body.class_unit(lo);
            if hi != lo {
                body.out.push('-');
                body.class_unit(hi);
            }
        }
        for &c in &set.classes {
            body.out.push('[');
            body.class(c);
            body.out.push(']');
        }
        for &c in &set.not_classes {
            body.out.push_str("[^");
            body.class(c);
            body.out.push(']');
        }
        // A negated set never matches a newline in newline mode.
        if set.negated && self.opts.newline {
            body.out.push_str("\\n");
        }
        match (body.out.is_empty(), set.negated) {
            // Only undecodable bytes: nothing, or everything.
            (true, false) => self.out.push_str("[a&&b]"),
            (true, true) => self.out.push_str("(?s:.)"),
            (false, false) => {
                self.out.push('[');
                self.out.push_str(&body.out);
                self.out.push(']');
            }
            (false, true) => {
                self.out.push_str("[^");
                self.out.push_str(&body.out);
                self.out.push(']');
            }
        }
    }

    /// The members of a character class, for inside `[...]`. In UTF-8 they follow
    /// [`Class::has`] (the ASCII members, then Unicode properties for the rest); in the
    /// C locale only ASCII characters are classified.
    fn class(&mut self, c: Class) {
        // Not control or a line/paragraph separator (`cntrl`), not `space`, for
        // characters outside ASCII.
        const CNTRL: &str = "\\p{Cc}\\x{2028}\\x{2029}";
        const SPACE: &str = "[\\s--[\\x{85}\\x{A0}\\x{2007}\\x{202F}]]";
        if !self.opts.utf8 {
            self.out.push_str(match c {
                Class::Alpha => "[:alpha:]",
                Class::Digit => "[:digit:]",
                Class::Alnum => "[:alnum:]",
                Class::Upper => "[:upper:]",
                Class::Lower => "[:lower:]",
                Class::Space => "[:space:]",
                Class::Blank => "[:blank:]",
                Class::Punct => "[:punct:]",
                Class::Print => "[:print:]",
                Class::Graph => "[:graph:]",
                Class::Cntrl => "[:cntrl:]",
                Class::Xdigit => "[:xdigit:]",
                Class::Word => "[:word:]",
            });
            return;
        }
        let _ = match c {
            Class::Alpha => write!(self.out, "\\p{{Alphabetic}}"),
            Class::Digit => write!(self.out, "0-9"),
            Class::Alnum => write!(self.out, "0-9\\p{{Alphabetic}}"),
            Class::Word => write!(self.out, "0-9_\\p{{Alphabetic}}"),
            Class::Upper => write!(self.out, "\\p{{Uppercase}}"),
            Class::Lower => write!(self.out, "\\p{{Lowercase}}"),
            Class::Xdigit => write!(self.out, "0-9A-Fa-f"),
            Class::Space => write!(self.out, "\\t\\n\\x0B\\x0C\\r {SPACE}"),
            Class::Blank => write!(
                self.out,
                "\\t [{SPACE}--[\\x00-\\x7F\\x{{2028}}\\x{{2029}}]]"
            ),
            Class::Cntrl => write!(self.out, "\\x00-\\x1F\\x7F{CNTRL}"),
            Class::Print => write!(self.out, " -~[^\\x00-\\x7F{CNTRL}]"),
            Class::Graph => write!(self.out, "!-~[^\\x00-\\x7F{CNTRL}{SPACE}]"),
            Class::Punct => write!(
                self.out,
                "!-/:-@\\[-`{{-~[^\\x00-\\x7F{CNTRL}{SPACE}\\p{{Alphabetic}}]"
            ),
        };
    }
}
