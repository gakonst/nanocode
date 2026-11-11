//! Pattern syntax: POSIX ERE and BRE with GNU's extensions (`\w \W \s \S \b \B \< \>
//! \` \'`, back-references in ERE, `\+ \? \|` in BRE, `{,n}`).

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::class::{Class, Set, Unit, decode};
use crate::{Error, ErrorKind, Options};

/// Repetition counts above this are an error, as in GNU (`RE_DUP_MAX`).
pub(crate) const DUP_MAX: u32 = 0x7fff;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Assert {
    /// `^`
    Bol,
    /// `$`
    Eol,
    /// `\b`
    WordBoundary,
    /// `\B`
    NotWordBoundary,
    /// `\<`
    WordStart,
    /// `\>`
    WordEnd,
    /// `` \` ``
    BufStart,
    /// `\'`
    BufEnd,
}

#[derive(Clone, Debug)]
pub(crate) enum Node {
    Empty,
    Lit(Unit),
    /// `.`
    Any,
    Set(Box<Set>),
    Assert(Assert),
    Group(Box<Node>, usize),
    Cat(Vec<Node>),
    Alt(Vec<Node>),
    Repeat(Box<Node>, u32, Option<u32>),
    Backref(usize),
}

pub(crate) struct Parsed {
    pub node: Node,
    pub groups: usize,
    pub backrefs: bool,
}

pub(crate) fn parse(pat: &[u8], opts: Options) -> Result<Parsed, Error> {
    let mut p = P {
        s: pat,
        i: 0,
        opts,
        groups: 0,
        visible: Vec::new(),
        backrefs: false,
    };
    let node = p.alt(0)?;
    if p.i < p.s.len() {
        // Only an unmatched `)` (ERE) or `\)` (BRE) stops the top level.
        return Err(Error::new(ErrorKind::RParen));
    }
    Ok(Parsed {
        node,
        groups: p.groups,
        backrefs: p.backrefs,
    })
}

struct P<'a> {
    s: &'a [u8],
    i: usize,
    opts: Options,
    groups: usize,
    /// Closed groups a back-reference here may name: not those in another
    /// alternative of an enclosing `|` (GNU rejects `(a)|\1`).
    visible: Vec<usize>,
    backrefs: bool,
}

fn err<T>(kind: ErrorKind) -> Result<T, Error> {
    Err(Error::new(kind))
}

impl P<'_> {
    fn ere(&self) -> bool {
        self.opts.extended
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn peek2(&self) -> Option<u8> {
        self.s.get(self.i + 1).copied()
    }

    /// At `|` (ERE) or `\|` (BRE).
    fn at_bar(&self) -> bool {
        if self.ere() {
            self.peek() == Some(b'|')
        } else {
            self.peek() == Some(b'\\') && self.peek2() == Some(b'|')
        }
    }

    /// At `)` (ERE) or `\)` (BRE).
    fn at_close(&self) -> bool {
        if self.ere() {
            self.peek() == Some(b')')
        } else {
            self.peek() == Some(b'\\') && self.peek2() == Some(b')')
        }
    }

    fn alt(&mut self, depth: usize) -> Result<Node, Error> {
        let mut alts = Vec::new();
        let base = self.visible.len();
        let mut inside = Vec::new();
        loop {
            self.visible.truncate(base);
            alts.push(self.cat(depth)?);
            inside.extend_from_slice(&self.visible[base..]);
            if self.at_bar() {
                self.i += if self.ere() { 1 } else { 2 };
                continue;
            }
            break;
        }
        // After the alternation, the groups of all its alternatives can be named.
        self.visible.truncate(base);
        self.visible.extend(inside);
        Ok(if alts.len() == 1 {
            alts.pop().unwrap()
        } else {
            Node::Alt(alts)
        })
    }

    fn cat(&mut self, depth: usize) -> Result<Node, Error> {
        let mut items: Vec<Node> = Vec::new();
        let start = self.i;
        loop {
            if self.i >= self.s.len() || self.at_bar() {
                break;
            }
            if self.at_close() {
                if depth > 0 {
                    break;
                }
                // An unmatched `)` is an ordinary char in ERE; `\)` is an error in BRE.
                if self.ere() {
                    let atom = self.lit_here();
                    let atom = self.quantifiers(atom)?;
                    items.push(atom);
                    continue;
                }
                return err(ErrorKind::Paren);
            }
            let at_start = self.i == start;
            let atom = self.atom(at_start, depth)?;
            let atom = self.quantifiers(atom)?;
            items.push(atom);
        }
        Ok(match items.len() {
            0 => Node::Empty,
            1 => items.pop().unwrap(),
            _ => Node::Cat(items),
        })
    }

    fn quantifiers(&mut self, mut atom: Node) -> Result<Node, Error> {
        let assertion = matches!(atom, Node::Assert(_));
        // In BRE a `*` right after an anchor is literal.
        if !self.ere() && assertion && self.peek() == Some(b'*') {
            return Ok(atom);
        }
        // The previous repetition operator, for BRE's stacking rules.
        let mut prev: Option<u8> = None;
        loop {
            // Which operator is here: `*`, `+`, `?` or `{`.
            let op = match self.peek() {
                Some(b'*') => Some(b'*'),
                Some(c @ (b'+' | b'?' | b'{')) if self.ere() => Some(c),
                Some(b'\\') if !self.ere() => {
                    self.peek2().filter(|c| matches!(c, b'+' | b'?' | b'{'))
                }
                _ => None,
            };
            if let Some(op) = op {
                // An anchor cannot be repeated. In BRE (not ERE, where `a**` is fine),
                // `*` and `\{\}` cannot follow another operator (`a\?\?` and
                // `a\{1\}\+` are fine).
                let stacked = match prev {
                    None => false,
                    Some(_) if self.ere() => false,
                    Some(_) => op == b'*' || op == b'{',
                };
                if assertion || stacked {
                    return err(ErrorKind::BadRepeat);
                }
                prev = Some(op);
            }
            let (min, max) = match self.peek() {
                Some(b'*') => {
                    self.i += 1;
                    (0, None)
                }
                Some(b'+') if self.ere() => {
                    self.i += 1;
                    (1, None)
                }
                Some(b'?') if self.ere() => {
                    self.i += 1;
                    (0, Some(1))
                }
                Some(b'{') if self.ere() => {
                    self.i += 1;
                    self.interval()?
                }
                Some(b'\\') if !self.ere() => match self.peek2() {
                    Some(b'+') => {
                        self.i += 2;
                        (1, None)
                    }
                    Some(b'?') => {
                        self.i += 2;
                        (0, Some(1))
                    }
                    Some(b'{') => {
                        self.i += 2;
                        self.interval()?
                    }
                    _ => return Ok(atom),
                },
                _ => return Ok(atom),
            };
            atom = Node::Repeat(Box::new(atom), min, max);
        }
    }

    /// The rest of `{m}`, `{m,}`, `{m,n}` or `{,n}` (after the `{`), up to and
    /// including `}` (`\}` in BRE).
    fn interval(&mut self) -> Result<(u32, Option<u32>), Error> {
        let num = |p: &mut P<'_>| -> Option<u32> {
            let start = p.i;
            let mut v: u32 = 0;
            while let Some(c) = p.peek().filter(u8::is_ascii_digit) {
                v = v.saturating_mul(10).saturating_add((c - b'0') as u32);
                p.i += 1;
            }
            (p.i > start).then_some(v)
        };
        let min = num(self);
        // After the first count only `,` or the closing brace may follow.
        let closing = if self.ere() {
            self.peek() == Some(b'}')
        } else {
            self.peek() == Some(b'\\') && self.peek2() == Some(b'}')
        };
        if min.is_some() && self.peek() != Some(b',') && !closing {
            return err(ErrorKind::Brace);
        }
        let max = if self.peek() == Some(b',') {
            self.i += 1;
            num(self)
        } else {
            match min {
                Some(m) => Some(m),
                None => {
                    return err(if self.i >= self.s.len() {
                        ErrorKind::Brace
                    } else {
                        ErrorKind::BadBrace
                    });
                }
            }
        };
        let closed = if self.ere() {
            self.peek() == Some(b'}')
        } else {
            self.peek() == Some(b'\\') && self.peek2() == Some(b'}')
        };
        if !closed {
            return err(if self.i >= self.s.len() {
                ErrorKind::Brace
            } else {
                ErrorKind::BadBrace
            });
        }
        self.i += if self.ere() { 1 } else { 2 };
        let min = min.unwrap_or(0);
        if min > DUP_MAX || max.is_some_and(|m| m > DUP_MAX || m < min) {
            return err(ErrorKind::BadBrace);
        }
        Ok((min, max))
    }

    fn lit_here(&mut self) -> Node {
        let (u, n) = decode(self.s, self.i, self.opts.utf8);
        self.i += n;
        Node::Lit(u)
    }

    fn atom(&mut self, at_start: bool, depth: usize) -> Result<Node, Error> {
        let c = self.peek().unwrap();
        if self.ere() {
            match c {
                b'(' => {
                    self.i += 1;
                    return self.group(depth);
                }
                b'*' | b'+' | b'?' => {
                    if at_start {
                        return err(ErrorKind::BadRepeat);
                    }
                    // Repeating an anchor or another repetition is fine in GNU; reached
                    // only when the previous atom consumed no quantifier.
                    return err(ErrorKind::BadRepeat);
                }
                b'{' => {
                    if at_start {
                        return err(ErrorKind::BadRepeat);
                    }
                    return err(ErrorKind::BadRepeat);
                }
                b'^' => {
                    self.i += 1;
                    return Ok(Node::Assert(Assert::Bol));
                }
                b'$' => {
                    self.i += 1;
                    return Ok(Node::Assert(Assert::Eol));
                }
                _ => {}
            }
        } else {
            match c {
                b'*' if at_start => return Ok(self.lit_here()),
                b'^' if at_start => {
                    self.i += 1;
                    return Ok(Node::Assert(Assert::Bol));
                }
                b'$' => {
                    // An anchor only at the end of the pattern, a group or an
                    // alternative.
                    let next = self.i + 1;
                    let at_end = next >= self.s.len()
                        || (self.s[next] == b'\\'
                            && matches!(self.s.get(next + 1), Some(b')' | b'|')));
                    self.i += 1;
                    return Ok(if at_end {
                        Node::Assert(Assert::Eol)
                    } else {
                        Node::Lit(b'$' as Unit)
                    });
                }
                _ => {}
            }
        }
        match c {
            b'.' => {
                self.i += 1;
                Ok(Node::Any)
            }
            b'[' => {
                self.i += 1;
                self.bracket()
            }
            b'\\' => self.escape(depth),
            _ => Ok(self.lit_here()),
        }
    }

    fn group(&mut self, depth: usize) -> Result<Node, Error> {
        self.groups += 1;
        let idx = self.groups;
        let inner = self.alt(depth + 1)?;
        if !self.at_close() {
            return err(ErrorKind::Paren);
        }
        self.i += if self.ere() { 1 } else { 2 };
        self.visible.push(idx);
        Ok(Node::Group(Box::new(inner), idx))
    }

    fn escape(&mut self, depth: usize) -> Result<Node, Error> {
        let Some(c) = self.peek2() else {
            return err(ErrorKind::Escape);
        };
        if !self.ere() && c == b'(' {
            self.i += 2;
            return self.group(depth);
        }
        if !self.ere() && c == b'{' {
            return err(ErrorKind::BadRepeat);
        }
        let node = match c {
            b'1'..=b'9' => {
                let n = (c - b'0') as usize;
                if !self.visible.contains(&n) {
                    return err(ErrorKind::Subreg);
                }
                self.backrefs = true;
                Node::Backref(n)
            }
            b'w' | b'W' | b's' | b'S' => {
                let class = if c.eq_ignore_ascii_case(&b'w') {
                    Class::Word
                } else {
                    Class::Space
                };
                let mut set = Set::default();
                if c.is_ascii_lowercase() {
                    set.classes.push(class);
                } else {
                    set.not_classes.push(class);
                }
                Node::Set(Box::new(set))
            }
            b'b' => Node::Assert(Assert::WordBoundary),
            b'B' => Node::Assert(Assert::NotWordBoundary),
            b'<' => Node::Assert(Assert::WordStart),
            b'>' => Node::Assert(Assert::WordEnd),
            b'`' => Node::Assert(Assert::BufStart),
            b'\'' => Node::Assert(Assert::BufEnd),
            _ => {
                self.i += 1;
                return Ok(self.lit_here());
            }
        };
        self.i += 2;
        Ok(node)
    }

    /// A bracket expression, after the `[`.
    fn bracket(&mut self) -> Result<Node, Error> {
        let utf8 = self.opts.utf8;
        let mut set = Set::default();
        if self.peek() == Some(b'^') {
            set.negated = true;
            self.i += 1;
        }
        let mut first = true;
        loop {
            let Some(c) = self.peek() else {
                // GNU: `[` or `[^` at the very end is a bad pattern, not an open bracket.
                return err(if first {
                    ErrorKind::BadPattern
                } else {
                    ErrorKind::Brack
                });
            };
            if c == b']' && !first {
                self.i += 1;
                break;
            }
            first = false;
            // One element: a char, a collating symbol, an equivalence class, or a class.
            let lo = match self.element(&mut set)? {
                Some(u) => u,
                None => {
                    // A class cannot start a range.
                    if self.peek() == Some(b'-') && self.peek2().is_some_and(|c| c != b']') {
                        return err(ErrorKind::Range);
                    }
                    continue;
                }
            };
            // A range?
            if self.peek() == Some(b'-') && self.peek2().is_some_and(|c| c != b']') {
                self.i += 1;
                let hi = match self.element(&mut set)? {
                    Some(u) => u,
                    None => return err(ErrorKind::Range),
                };
                if hi < lo {
                    return err(ErrorKind::Range);
                }
                set.ranges.push((lo, hi));
            } else {
                set.ranges.push((lo, lo));
            }
            let _ = utf8;
        }
        Ok(Node::Set(Box::new(set)))
    }

    /// One element of a bracket expression: `Some(char)`, or `None` for a character
    /// class (added to `set`).
    fn element(&mut self, set: &mut Set) -> Result<Option<Unit>, Error> {
        if self.peek() == Some(b'[')
            && let Some(kind @ (b':' | b'=' | b'.')) = self.peek2()
        {
            let start = self.i + 2;
            let mut j = start;
            loop {
                if j + 1 >= self.s.len() {
                    return err(ErrorKind::Brack);
                }
                if self.s[j] == kind && self.s[j + 1] == b']' {
                    break;
                }
                j += 1;
            }
            let name = &self.s[start..j];
            self.i = j + 2;
            return match kind {
                b':' => match Class::by_name(name) {
                    Some(c) => {
                        set.classes.push(c);
                        Ok(None)
                    }
                    None => err(ErrorKind::Ctype),
                },
                _ => {
                    // Collating symbols and equivalence classes of one character (a
                    // UTF-8 or C locale has no others).
                    if name.is_empty() {
                        return err(ErrorKind::Collate);
                    }
                    let (u, n) = decode(name, 0, self.opts.utf8);
                    if n != name.len() {
                        return err(ErrorKind::Collate);
                    }
                    Ok(Some(u))
                }
            };
        }
        let (u, n) = decode(self.s, self.i, self.opts.utf8);
        self.i += n;
        Ok(Some(u))
    }
}
