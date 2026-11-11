//! Pattern parser: ES2025 pattern syntax over UTF-16 source, with the Annex B grammar in
//! non-unicode mode (D1110) and the `v` flag's class set syntax. Produces a small tree that
//! `compiler` turns into bytecode. Errors use QuickJS' messages.
//!
//! The structure and the error checks follow QuickJS' `libregexp.c` (MIT, Fabrice Bellard
//! and Charlie Gordon) closely, so that valid/invalid decisions and messages match qjs.
use crate::PushOut;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use super::charset::{self, CharSet, canonicalize};
use super::{Flags, SyntaxError};
use crate::unicode;

/// Nesting limit for groups and classes, so that recursion stays bounded: 211 nested
/// groups are accepted (test262's S15.10.2.8_A3_T15 needs 200). In the shipped wasm
/// module they compile and match in its 128 KiB stack with 10% to spare: 244 nested
/// capture groups and 236 nested lookaheads trap there (D2540, D2545).
const MAX_DEPTH: u32 = 212;
/// Capture group limit (qjs allows 254 groups; we allow more).
pub(crate) const MAX_CAPTURES: u32 = 65535;
/// Quantifier bounds saturate here, like qjs (`{99999999999}` means 2^31 - 1).
const QUANT_INF: u32 = u32::MAX;
const QUANT_SAT: u32 = i32::MAX as u32;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Node {
    Empty,
    /// A character; canonicalized when `icase`.
    Char {
        c: u32,
        icase: bool,
    },
    /// A character class. With `icase`, the input character is canonicalized before the
    /// membership test (the set holds canonical forms where it matters). `strings` are the
    /// `v` mode strings of length != 1 (possibly empty).
    Class {
        set: CharSet,
        strings: Vec<Vec<u32>>,
        icase: bool,
    },
    Dot {
        all: bool,
    },
    LineStart {
        multiline: bool,
    },
    LineEnd {
        multiline: bool,
    },
    WordBoundary {
        negate: bool,
        unicode_icase: bool,
    },
    Group {
        index: u32,
        body: Box<Node>,
    },
    Look {
        behind: bool,
        negate: bool,
        body: Box<Node>,
    },
    /// A backreference to the first of `groups` that is set (several for duplicate names).
    Backref {
        groups: Vec<u32>,
        name: Option<Vec<u16>>,
        icase: bool,
    },
    Repeat {
        min: u32,
        /// `u32::MAX` is infinity.
        max: u32,
        greedy: bool,
        body: Box<Node>,
        /// Capture groups inside the body: `first..last`.
        caps: (u32, u32),
    },
    Concat(Vec<Node>),
    Alt(Vec<Node>),
}

pub(crate) struct Parsed {
    pub(crate) node: Node,
    /// Number of capture groups including group 0.
    pub(crate) capture_count: u32,
    /// Group names by capture index (index 0 unused).
    pub(crate) names: Vec<Option<Vec<u16>>>,
}

struct GroupName {
    index: u32,
    name: Vec<u16>,
    /// The (disjunction, alternative) path from the root, for the duplicate name rule.
    path: Vec<(u32, u32)>,
}

/// A class atom: a single character or a set (`\d`, `\w`, `\p{..}`, `\q{..}`, ...).
enum ClassAtom {
    Char(u32),
    Set(ClassSet),
}

/// A class under construction: characters plus (in `v` mode) strings of length != 1.
#[derive(Clone, Default)]
struct ClassSet {
    chars: CharSet,
    strings: Vec<Vec<u32>>,
}

impl ClassSet {
    fn add_string(&mut self, s: Vec<u32>) {
        if s.len() == 1 {
            self.chars.add(s[0]);
        } else if !self.strings.contains(&s) {
            self.strings.push_out(s);
        }
    }

    fn union(&mut self, other: ClassSet) {
        self.chars = self.chars.union(&other.chars);
        for s in other.strings {
            self.add_string(s);
        }
    }

    fn intersect(&mut self, other: &ClassSet) {
        self.chars = self.chars.intersect(&other.chars);
        self.strings.retain(|s| other.strings.contains(s));
    }

    fn subtract(&mut self, other: &ClassSet) {
        self.chars = self.chars.subtract(&other.chars);
        self.strings.retain(|s| !other.strings.contains(s));
    }
}

fn err<T>(msg: &str) -> Result<T, SyntaxError> {
    Err(SyntaxError::new(String::from(msg)))
}

fn is_digit(c: u32) -> bool {
    (0x30..=0x39).contains(&c)
}

fn from_hex(c: u32) -> Option<u32> {
    char::from_u32(c)?.to_digit(16)
}

fn is_hi_surrogate(c: u32) -> bool {
    (0xD800..=0xDBFF).contains(&c)
}

fn is_lo_surrogate(c: u32) -> bool {
    (0xDC00..=0xDFFF).contains(&c)
}

fn from_surrogates(hi: u32, lo: u32) -> u32 {
    0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
}

struct Parser<'a> {
    src: &'a [u16],
    pos: usize,
    unicode: bool,
    sets: bool,
    icase: bool,
    multiline: bool,
    dotall: bool,
    /// Index of the next capture group.
    capture_count: u32,
    /// From a pre-scan of the source, computed on demand (like qjs).
    total_captures: Option<u32>,
    has_named: Option<bool>,
    names: Vec<GroupName>,
    path: Vec<(u32, u32)>,
    next_disjunction: u32,
    depth: u32,
}

pub(crate) fn parse(src: &[u16], flags: Flags) -> Result<Parsed, SyntaxError> {
    let mut p = Parser {
        src,
        pos: 0,
        unicode: flags.unicode_mode(),
        sets: flags.contains(Flags::UNICODE_SETS),
        icase: flags.contains(Flags::IGNORE_CASE),
        multiline: flags.contains(Flags::MULTILINE),
        dotall: flags.contains(Flags::DOT_ALL),
        capture_count: 1,
        total_captures: None,
        has_named: None,
        names: Vec::new(),
        path: Vec::new(),
        next_disjunction: 0,
        depth: 0,
    };
    let mut node = p.disjunction(false)?;
    if p.pos < src.len() {
        return err("extraneous characters at the end");
    }
    let mut names: Vec<Option<Vec<u16>>> = alloc::vec![None; p.capture_count as usize];
    for g in &p.names {
        names[g.index as usize] = Some(g.name.clone());
    }
    // Only named backreferences need it (their groups start empty).
    if !p.names.is_empty() {
        resolve_names(&mut node, &p.names);
    }
    Ok(Parsed {
        node,
        capture_count: p.capture_count,
        names,
    })
}

/// Fills in the group indices of named backreferences.
fn resolve_names(node: &mut Node, names: &[GroupName]) {
    match node {
        Node::Backref {
            groups,
            name: Some(name),
            ..
        } => {
            *groups = names
                .iter()
                .filter(|g| &g.name == name)
                .map(|g| g.index)
                .collect();
        }
        Node::Group { body, .. } | Node::Look { body, .. } | Node::Repeat { body, .. } => {
            resolve_names(body, names)
        }
        Node::Concat(v) | Node::Alt(v) => v.iter_mut().for_each(|n| resolve_names(n, names)),
        _ => {}
    }
}

impl Parser<'_> {
    /// The code unit at `i`, or 0 past the end (qjs' NUL terminator; callers that care
    /// about a literal NUL check `i < len`).
    fn at(&self, i: usize) -> u32 {
        self.src.get(i).map_or(0, |&c| c as u32)
    }

    fn cur(&self) -> u32 {
        self.at(self.pos)
    }

    fn at_end(&self) -> bool {
        self.pos >= self.src.len()
    }

    /// The source character at `i` and its length in code units: a surrogate pair is one
    /// character in unicode mode.
    fn source_char(&self, i: usize) -> (u32, usize) {
        let c = self.at(i);
        if self.unicode && is_hi_surrogate(c) && i + 1 < self.src.len() {
            let d = self.at(i + 1);
            if is_lo_surrogate(d) {
                return (from_surrogates(c, d), 2);
            }
        }
        (c, 1)
    }

    fn expect(&mut self, c: u8) -> Result<(), SyntaxError> {
        if self.at_end() || self.cur() != c as u32 {
            return Err(SyntaxError::new(crate::text::cat!(
                "expecting '",
                c as char,
                "'"
            )));
        }
        self.pos += 1;
        Ok(())
    }

    fn enter(&mut self) -> Result<(), SyntaxError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return err("stack overflow");
        }
        Ok(())
    }

    /// Decimal digits at `self.pos`; saturates at 2^31 - 1, or returns `None` on overflow
    /// when `saturate` is false.
    fn digits(&mut self, saturate: bool) -> Option<u32> {
        let mut v: u64 = 0;
        while is_digit(self.cur()) && !self.at_end() {
            v = v * 10 + (self.cur() - 0x30) as u64;
            if v >= QUANT_SAT as u64 {
                if saturate {
                    v = QUANT_SAT as u64;
                } else {
                    return None;
                }
            }
            self.pos += 1;
        }
        Some(v as u32)
    }

    fn disjunction(&mut self, backward: bool) -> Result<Node, SyntaxError> {
        self.enter()?;
        let id = self.next_disjunction;
        self.next_disjunction += 1;
        self.path.push_out((id, 0));
        let first = self.alternative(backward)?;
        if self.at_end() || self.cur() != '|' as u32 {
            self.path.pop();
            self.depth -= 1;
            return Ok(first);
        }
        let mut alts = crate::buf::vec_with_capacity(4);
        alts.push_out(first);
        while !self.at_end() && self.cur() == '|' as u32 {
            self.pos += 1;
            self.path.last_mut().unwrap().1 += 1;
            alts.push_out(self.alternative(backward)?);
        }
        self.path.pop();
        self.depth -= 1;
        Ok(if alts.len() == 1 {
            alts.pop().unwrap()
        } else {
            Node::Alt(alts)
        })
    }

    fn alternative(&mut self, backward: bool) -> Result<Node, SyntaxError> {
        // Room for the terms up to the next `|` or `)` if each were one character (a
        // literal run usually is): growing from empty reallocates repeatedly.
        let room = self.src[self.pos.min(self.src.len())..]
            .iter()
            .take(32)
            .take_while(|&&c| c != '|' as u16 && c != ')' as u16)
            .count();
        let mut terms = crate::buf::vec_with_capacity(room);
        while !self.at_end() && self.cur() != '|' as u32 && self.cur() != ')' as u32 {
            // A literal ASCII character that no quantifier follows: what `term` would
            // make of it, without its dispatch (literal runs dominate most patterns).
            let c = self.cur();
            if c < 0x80
                && !b"^$\\.*+?()[]{}|".contains(&(c as u8))
                && !b"*+?{".contains(&(self.at(self.pos + 1) as u8))
            {
                self.pos += 1;
                terms.push_out(self.char_node(c));
                continue;
            }
            if let Some(t) = self.term(backward)? {
                terms.push_out(t);
            }
        }
        Ok(match terms.len() {
            0 => Node::Empty,
            1 => terms.pop().unwrap(),
            _ => Node::Concat(terms),
        })
    }

    /// Parses one term (an assertion, or an atom with an optional quantifier).
    fn term(&mut self, backward: bool) -> Result<Option<Node>, SyntaxError> {
        let start_captures = self.capture_count;
        let c = self.cur();
        // `quantifiable` is qjs' `last_atom_start >= 0`.
        let (node, quantifiable) = match char::from_u32(c).unwrap_or('\0') {
            '^' => {
                self.pos += 1;
                (
                    Node::LineStart {
                        multiline: self.multiline,
                    },
                    false,
                )
            }
            '$' => {
                self.pos += 1;
                (
                    Node::LineEnd {
                        multiline: self.multiline,
                    },
                    false,
                )
            }
            '.' => {
                self.pos += 1;
                (Node::Dot { all: self.dotall }, true)
            }
            '{' if self.unicode => return err("syntax error"),
            '{' if !is_digit(self.at(self.pos + 1)) => (self.class_atom_term()?, true),
            '{' => {
                // Annex B: `{` is a literal unless it looks like a repetition count.
                let save = self.pos;
                self.pos += 1;
                self.digits(true);
                if self.cur() == ',' as u32 {
                    self.pos += 1;
                    if is_digit(self.cur()) {
                        self.digits(true);
                    }
                }
                let quantifier = self.cur() == '}' as u32;
                self.pos = save;
                if !quantifier {
                    (self.class_atom_term()?, true)
                } else {
                    return err("nothing to repeat");
                }
            }
            '*' | '+' | '?' => return err("nothing to repeat"),
            '(' => return self.group(backward, start_captures),
            '\\' => match char::from_u32(self.at(self.pos + 1)).unwrap_or('\0') {
                'b' | 'B' => {
                    let negate = self.at(self.pos + 1) == 'B' as u32;
                    self.pos += 2;
                    (
                        Node::WordBoundary {
                            negate,
                            unicode_icase: self.icase && self.unicode,
                        },
                        false,
                    )
                }
                'k' => match self.named_backref()? {
                    Some(n) => (n, true),
                    None => (self.class_atom_term()?, true),
                },
                '0' => {
                    self.pos += 2;
                    let mut c = 0;
                    if self.unicode {
                        if is_digit(self.cur()) {
                            return err("invalid decimal escape in regular expression");
                        }
                    } else if (0x30..=0x37).contains(&self.cur()) {
                        // Annex B.1.4: legacy octal.
                        c = self.cur() - 0x30;
                        self.pos += 1;
                        if (0x30..=0x37).contains(&self.cur()) {
                            c = (c << 3) + self.cur() - 0x30;
                            self.pos += 1;
                        }
                    }
                    (self.char_node(c), true)
                }
                '1'..='9' => (self.decimal_escape()?, true),
                _ => (self.class_atom_term()?, true),
            },
            '[' => {
                let set = self.class()?;
                (set, true)
            }
            ']' | '}' if self.unicode => return err("syntax error"),
            _ => (self.class_atom_term()?, true),
        };
        if !quantifiable {
            return Ok(Some(node));
        }
        self.quantifier(node, start_captures).map(Some)
    }

    fn char_node(&self, c: u32) -> Node {
        if self.icase {
            Node::Char {
                c: canonicalize(c, self.unicode),
                icase: true,
            }
        } else {
            Node::Char { c, icase: false }
        }
    }

    /// An atom that is a single class atom outside a class (a character or an escape).
    fn class_atom_term(&mut self) -> Result<Node, SyntaxError> {
        match self.class_atom(false, true)? {
            ClassAtom::Char(c) => Ok(self.char_node(c)),
            ClassAtom::Set(s) => Ok(Node::Class {
                set: s.chars,
                strings: s.strings,
                icase: self.icase,
            }),
        }
    }

    /// `\1`..`\9...`: a backreference, or (Annex B, non-unicode) a legacy octal escape or
    /// an identity escape.
    fn decimal_escape(&mut self) -> Result<Node, SyntaxError> {
        self.pos += 1;
        let q = self.pos;
        let n = self.digits(false);
        let valid = match n {
            Some(n) => n < self.capture_count || n < self.total_captures(),
            None => false,
        };
        if valid {
            return Ok(Node::Backref {
                groups: alloc::vec![n.unwrap()],
                name: None,
                icase: self.icase,
            });
        }
        if self.unicode {
            return err("back reference out of range in regular expression");
        }
        // Annex B.1.4: legacy octal (up to \377) or the digit itself.
        self.pos = q;
        let c;
        if self.cur() <= '7' as u32 {
            let mut v = 0;
            if self.cur() <= '3' as u32 {
                v = self.cur() - 0x30;
                self.pos += 1;
            }
            if (0x30..=0x37).contains(&self.cur()) {
                v = (v << 3) + self.cur() - 0x30;
                self.pos += 1;
                if (0x30..=0x37).contains(&self.cur()) {
                    v = (v << 3) + self.cur() - 0x30;
                    self.pos += 1;
                }
            }
            c = v;
        } else {
            c = self.cur();
            self.pos += 1;
        }
        Ok(self.char_node(c))
    }

    /// `\k<name>`. Returns `None` when Annex B makes it the literal `k`.
    fn named_backref(&mut self) -> Result<Option<Node>, SyntaxError> {
        let strict = self.unicode || self.has_named_captures();
        if self.at(self.pos + 2) != '<' as u32 {
            if strict {
                return err("expecting group name");
            }
            return Ok(None);
        }
        let mut p = self.pos + 3;
        let Some(name) = self.group_name(&mut p) else {
            if strict {
                return err("invalid group name");
            }
            return Ok(None);
        };
        let defined = self.names.iter().any(|g| g.name == name) || self.prescan_has_name(&name);
        if !defined {
            if strict {
                return err("group name not defined");
            }
            return Ok(None);
        }
        self.pos = p;
        Ok(Some(Node::Backref {
            groups: Vec::new(),
            name: Some(name),
            icase: self.icase,
        }))
    }

    fn group(&mut self, backward: bool, start_captures: u32) -> Result<Option<Node>, SyntaxError> {
        let p = self.pos;
        if self.at(p + 1) != '?' as u32 {
            self.pos += 1;
            return self.capture(backward, None, start_captures).map(Some);
        }
        let c2 = self.at(p + 2);
        match char::from_u32(c2).unwrap_or('\0') {
            ':' => {
                self.pos += 3;
                let body = self.disjunction(backward)?;
                self.expect(b')')?;
                self.quantifier(body, start_captures).map(Some)
            }
            'i' | 'm' | 's' | '-' => {
                self.pos += 2;
                let add = self.modifiers()?;
                let mut remove = 0;
                if self.cur() == '-' as u32 {
                    self.pos += 1;
                    remove = self.modifiers()?;
                }
                if (add == 0 && remove == 0) || add & remove != 0 {
                    return err("invalid modifiers");
                }
                self.expect(b':')?;
                let saved = (self.icase, self.multiline, self.dotall);
                let update = |v: bool, bit: u8| (v || add & bit != 0) && remove & bit == 0;
                self.icase = update(self.icase, 1);
                self.multiline = update(self.multiline, 2);
                self.dotall = update(self.dotall, 4);
                let body = self.disjunction(backward)?;
                self.expect(b')')?;
                (self.icase, self.multiline, self.dotall) = saved;
                self.quantifier(body, start_captures).map(Some)
            }
            '=' | '!' => {
                self.pos += 3;
                self.lookaround(false, c2 == '!' as u32, start_captures)
            }
            '<' if matches!(self.at(p + 3), 0x3D | 0x21) => {
                let negate = self.at(p + 3) == '!' as u32;
                self.pos += 4;
                self.lookaround(true, negate, start_captures)
            }
            '<' => {
                let mut q = p + 3;
                let Some(name) = self.group_name(&mut q) else {
                    return err("invalid group name");
                };
                if self
                    .names
                    .iter()
                    .any(|g| g.name == name && might_both_participate(&g.path, &self.path))
                {
                    return err("duplicate group name");
                }
                self.pos = q;
                self.has_named = Some(true);
                self.capture(backward, Some(name), start_captures).map(Some)
            }
            _ => err("invalid group"),
        }
    }

    fn lookaround(
        &mut self,
        behind: bool,
        negate: bool,
        start_captures: u32,
    ) -> Result<Option<Node>, SyntaxError> {
        let body = self.disjunction(behind)?;
        self.expect(b')')?;
        let node = Node::Look {
            behind,
            negate,
            body: Box::new(body),
        };
        // Annex B: a lookahead (not a lookbehind) can be quantified in non-unicode mode.
        if !self.unicode && !behind {
            return self.quantifier(node, start_captures).map(Some);
        }
        Ok(Some(node))
    }

    fn capture(
        &mut self,
        backward: bool,
        name: Option<Vec<u16>>,
        start_captures: u32,
    ) -> Result<Node, SyntaxError> {
        if self.capture_count >= MAX_CAPTURES {
            return err("too many captures");
        }
        let index = self.capture_count;
        self.capture_count += 1;
        if let Some(name) = name {
            self.names.push_out(GroupName {
                index,
                name,
                path: self.path.clone(),
            });
        }
        let body = self.disjunction(backward)?;
        self.expect(b')')?;
        let node = Node::Group {
            index,
            body: Box::new(body),
        };
        self.quantifier(node, start_captures)
    }

    fn modifiers(&mut self) -> Result<u8, SyntaxError> {
        let mut mask = 0u8;
        loop {
            let bit = match self.cur() {
                0x69 => 1, // i
                0x6D => 2, // m
                0x73 => 4, // s
                _ => return Ok(mask),
            };
            if mask & bit != 0 {
                return Err(SyntaxError::new(crate::text::cat!(
                    "duplicate modifier: '",
                    char::from_u32(self.cur()).unwrap(),
                    "'"
                )));
            }
            mask |= bit;
            self.pos += 1;
        }
    }

    /// Parses an optional quantifier after `atom`.
    fn quantifier(&mut self, atom: Node, start_captures: u32) -> Result<Node, SyntaxError> {
        if self.at_end() {
            return Ok(atom);
        }
        let (min, max) = match self.cur() {
            0x2A => {
                self.pos += 1;
                (0, QUANT_INF)
            }
            0x2B => {
                self.pos += 1;
                (1, QUANT_INF)
            }
            0x3F => {
                self.pos += 1;
                (0, 1)
            }
            0x7B => {
                let save = self.pos;
                if !is_digit(self.at(self.pos + 1)) {
                    if self.unicode {
                        return err("invalid repetition count");
                    }
                    return Ok(atom);
                }
                self.pos += 1;
                let min = self.digits(true).unwrap();
                let mut max = min;
                if self.cur() == ',' as u32 {
                    self.pos += 1;
                    if is_digit(self.cur()) {
                        max = self.digits(true).unwrap();
                        if max < min {
                            return err("invalid repetition count");
                        }
                    } else {
                        max = QUANT_INF;
                    }
                }
                if self.cur() != '}' as u32 && !self.unicode {
                    // Annex B: not a quantifier, a literal `{`.
                    self.pos = save;
                    return Ok(atom);
                }
                self.expect(b'}')?;
                (min, max)
            }
            _ => return Ok(atom),
        };
        let mut greedy = true;
        if self.cur() == '?' as u32 && !self.at_end() {
            self.pos += 1;
            greedy = false;
        }
        Ok(Node::Repeat {
            min,
            max,
            greedy,
            body: Box::new(atom),
            caps: (start_captures, self.capture_count),
        })
    }

    /// Parses a group name at `*p` (just after `<`), leaving `*p` after the `>`.
    fn group_name(&self, p: &mut usize) -> Option<Vec<u16>> {
        let mut name: Vec<u16> = Vec::new();
        loop {
            let c0 = self.at(*p);
            let c = if c0 == '\\' as u32 {
                *p += 1;
                if self.at(*p) != 'u' as u32 {
                    return None;
                }
                self.escape_u(p, true)?
            } else if c0 == '>' as u32 {
                break;
            } else if *p >= self.src.len() {
                return None;
            } else if is_hi_surrogate(c0) && is_lo_surrogate(self.at(*p + 1)) {
                // Surrogate pairs combine in group names in every mode (as in qjs).
                *p += 2;
                from_surrogates(c0, self.at(*p - 1))
            } else {
                *p += 1;
                c0
            };
            let ok = if name.is_empty() {
                unicode::is_js_id_start(c)
            } else {
                unicode::is_js_id_continue(c)
            };
            if !ok {
                return None;
            }
            let mut buf = [0u16; 2];
            name.extend_from_slice(char::from_u32(c)?.encode_utf16(&mut buf));
        }
        if name.is_empty() {
            return None;
        }
        *p += 1;
        Some(name)
    }

    /// The pre-scan of qjs' `re_parse_captures`: counts capture groups and notes named
    /// ones without parsing.
    fn prescan(&self, wanted: Option<&[u16]>) -> (u32, bool, bool) {
        let src = self.src;
        let mut index = 1u32;
        let mut has_named = false;
        let mut found = false;
        let mut p = 0;
        while p < src.len() {
            match src[p] {
                0x28 => {
                    if self.at(p + 1) == '?' as u32 {
                        if self.at(p + 2) == '<' as u32
                            && self.at(p + 3) != '=' as u32
                            && self.at(p + 3) != '!' as u32
                        {
                            has_named = true;
                            if let Some(w) = wanted {
                                // Like qjs: after a valid name the scan resumes one past the
                                // `>` (skipping a character), otherwise at the name.
                                let mut q = p + 3;
                                match self.group_name(&mut q) {
                                    Some(name) => {
                                        found |= name == w;
                                        p = q;
                                    }
                                    None => p += 3,
                                }
                            }
                            index += 1;
                        }
                    } else {
                        index += 1;
                    }
                }
                0x5C => p += 1,
                0x5B => {
                    p += 1;
                    while p < src.len() && src[p] != ']' as u16 {
                        if src[p] == '\\' as u16 {
                            p += 1;
                        }
                        p += 1;
                    }
                }
                _ => {}
            }
            p += 1;
        }
        (index, has_named, found)
    }

    fn total_captures(&mut self) -> u32 {
        if self.total_captures.is_none() {
            let (n, named, _) = self.prescan(None);
            self.total_captures = Some(n);
            self.has_named = Some(self.has_named.unwrap_or(false) || named);
        }
        self.total_captures.unwrap()
    }

    fn has_named_captures(&mut self) -> bool {
        if self.has_named.is_none() {
            self.total_captures();
        }
        self.has_named.unwrap()
    }

    fn prescan_has_name(&self, name: &[u16]) -> bool {
        self.prescan(Some(name)).2
    }

    /// `\u` escape at `*p` (pointing at the `u`). With `utf16` (unicode mode), accepts
    /// `\u{...}` and combines an escaped surrogate pair.
    fn escape_u(&self, p: &mut usize, utf16: bool) -> Option<u32> {
        let mut q = *p + 1;
        let c = if self.at(q) == '{' as u32 && utf16 {
            q += 1;
            let mut v = 0u32;
            loop {
                let h = from_hex(self.at(q))?;
                q += 1;
                v = (v << 4) | h;
                if v > 0x10FFFF {
                    return None;
                }
                if self.at(q) == '}' as u32 {
                    break;
                }
            }
            q += 1;
            v
        } else {
            let mut v = 0u32;
            for _ in 0..4 {
                let h = from_hex(self.at(q))?;
                q += 1;
                v = (v << 4) | h;
            }
            if is_hi_surrogate(v)
                && utf16
                && self.at(q) == '\\' as u32
                && self.at(q + 1) == 'u' as u32
            {
                let mut lo = 0u32;
                let mut ok = true;
                for i in 0..4 {
                    match from_hex(self.at(q + 2 + i)) {
                        Some(h) => lo = (lo << 4) | h,
                        None => {
                            ok = false;
                            break;
                        }
                    }
                }
                if ok && is_lo_surrogate(lo) {
                    q += 6;
                    v = from_surrogates(v, lo);
                }
            }
            v
        };
        *p = q;
        Some(c)
    }

    /// qjs' `lre_parse_escape` at `self.pos` (pointing after the backslash): `Ok(Some(c))`
    /// for a character escape, `Ok(None)` if this is not one, `Err(())` if malformed.
    fn char_escape(&mut self, utf16: bool) -> Result<Option<u32>, ()> {
        let mut p = self.pos;
        let c = self.at(p);
        p += 1;
        let v = match char::from_u32(c).unwrap_or('\0') {
            'b' => 0x08,
            'f' => 0x0C,
            'n' => 0x0A,
            'r' => 0x0D,
            't' => 0x09,
            'v' => 0x0B,
            'x' => {
                let h0 = from_hex(self.at(p)).ok_or(())?;
                let h1 = from_hex(self.at(p + 1)).ok_or(())?;
                p += 2;
                (h0 << 4) | h1
            }
            'u' => {
                let mut q = self.pos;
                let v = self.escape_u(&mut q, utf16).ok_or(())?;
                p = q;
                v
            }
            '0'..='7' => {
                let mut v = c - 0x30;
                if utf16 {
                    // Only \0 not followed by a digit.
                    if v != 0 || is_digit(self.at(p)) {
                        return Err(());
                    }
                } else {
                    // Legacy octal.
                    let d = self.at(p).wrapping_sub(0x30);
                    if d <= 7 {
                        v = (v << 3) | d;
                        p += 1;
                        if v < 32 {
                            let d = self.at(p).wrapping_sub(0x30);
                            if d <= 7 {
                                v = (v << 3) | d;
                                p += 1;
                            }
                        }
                    }
                }
                v
            }
            _ => return Ok(None),
        };
        self.pos = p;
        Ok(Some(v))
    }

    /// qjs' `get_class_atom`. `allow_sets` is false inside `\q{...}` (qjs' `cr == NULL`).
    fn class_atom(&mut self, in_class: bool, allow_sets: bool) -> Result<ClassAtom, SyntaxError> {
        let c = self.cur();
        if c == '\\' as u32 && !self.at_end() {
            self.pos += 1;
            if self.at_end() {
                return err("unexpected end");
            }
            let e = self.cur();
            self.pos += 1;
            let ch = char::from_u32(e).unwrap_or('\0');
            match ch {
                'd' | 'D' | 's' | 'S' | 'w' | 'W' if allow_sets => {
                    let mut set = match ch.to_ascii_lowercase() {
                        'd' => charset::digits(),
                        's' => charset::spaces(),
                        _ => charset::word(),
                    };
                    if ch.is_ascii_uppercase() {
                        set = set.invert();
                    }
                    return Ok(ClassAtom::Set(ClassSet {
                        chars: set,
                        strings: Vec::new(),
                    }));
                }
                'c' => {
                    let d = self.cur();
                    let letter = (0x41..=0x5A).contains(&d) || (0x61..=0x7A).contains(&d);
                    if letter || ((is_digit(d) || d == '_' as u32) && in_class && !self.unicode) {
                        // Annex B.1.4 allows digits and `_` in classes.
                        self.pos += 1;
                        return Ok(ClassAtom::Char(d & 0x1F));
                    }
                    if self.unicode {
                        return err("invalid escape sequence in regular expression");
                    }
                    // `\c` stays a backslash; the `c` is parsed next.
                    self.pos -= 1;
                    return Ok(ClassAtom::Char('\\' as u32));
                }
                '-' => {
                    if !in_class && self.unicode {
                        return err("invalid escape sequence in regular expression");
                    }
                    return Ok(ClassAtom::Char(e));
                }
                '&' | '!' | '#' | '%' | ',' | ':' | ';' | '<' | '=' | '>' | '@' | '`' | '~' => {
                    // ClassSetReservedPunctuator: only escapable in a `v` class.
                    if self.unicode && (!in_class || !self.sets) {
                        return err("invalid escape sequence in regular expression");
                    }
                    return Ok(ClassAtom::Char(e));
                }
                '^' | '$' | '\\' | '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}'
                | '|' | '/' => return Ok(ClassAtom::Char(e)),
                'p' | 'P' if self.unicode && allow_sets => {
                    return self.property_escape(ch == 'P').map(ClassAtom::Set);
                }
                'q' if self.sets && allow_sets && in_class => {
                    return self.class_strings().map(ClassAtom::Set);
                }
                _ => {}
            }
            self.pos -= 1;
            match self.char_escape(self.unicode) {
                Ok(Some(v)) => return Ok(ClassAtom::Char(v)),
                _ if self.unicode => {
                    return err("invalid escape sequence in regular expression");
                }
                // Annex B: an identity escape; the backslash is ignored.
                _ => {}
            }
        } else if self.at_end() {
            return err("unexpected end");
        } else if in_class && self.sets {
            match char::from_u32(c).unwrap_or('\0') {
                '&' | '!' | '#' | '$' | '%' | '*' | '+' | ',' | '.' | ':' | ';' | '<' | '='
                | '>' | '?' | '@' | '^' | '`' | '~'
                    if self.at(self.pos + 1) == c =>
                {
                    return err("invalid class set operation in regular expression");
                }
                '(' | ')' | '[' | ']' | '{' | '}' | '/' | '-' | '|' => {
                    return err("invalid character in class in regular expression");
                }
                _ => {}
            }
        }
        let (ch, len) = self.source_char(self.pos);
        self.pos += len;
        Ok(ClassAtom::Char(ch))
    }

    /// `\p{..}` / `\P{..}` (D1500): a property set, case-folded and inverted in qjs'
    /// order (which differs between `u` and `v`). Without the `regexp-unicode-props`
    /// feature the syntax is checked and then rejected (D1112).
    fn property_escape(&mut self, negate: bool) -> Result<ClassSet, SyntaxError> {
        if self.cur() != '{' as u32 {
            return err("expecting '{' after \\p");
        }
        self.pos += 1;
        let is_name_char =
            |c: u32| char::from_u32(c).is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
        let mut name = String::new();
        while is_name_char(self.cur()) {
            if name.len() >= 63 {
                return err("unknown unicode property name");
            }
            name.push_out(self.cur() as u8 as char);
            self.pos += 1;
        }
        let mut value = None;
        if self.cur() == '=' as u32 {
            self.pos += 1;
            let mut v = String::new();
            while is_name_char(self.cur()) {
                if v.len() >= 63 {
                    return err("unknown unicode property value");
                }
                v.push_out(self.cur() as u8 as char);
                self.pos += 1;
            }
            value = Some(v);
        }
        if self.cur() != '}' as u32 || self.at_end() {
            return err("expecting '}'");
        }
        self.pos += 1;
        self.property_set(&name, value.as_deref(), negate)
    }

    #[cfg(feature = "regexp-unicode-props")]
    fn property_set(
        &mut self,
        name: &str,
        value: Option<&str>,
        negate: bool,
    ) -> Result<ClassSet, SyntaxError> {
        use crate::unicode::property;
        let p = property::lookup(name, value.unwrap_or(""), self.sets && !negate)
            .map_err(|e| SyntaxError::new(String::from(e.message())))?;
        let mut set = ClassSet {
            chars: CharSet::from_sorted(p.ranges),
            strings: Vec::new(),
        };
        for s in p.strings {
            set.add_string(s);
        }
        if self.icase && self.sets {
            set = self.canonicalize_class(set);
        }
        if negate {
            set.chars = set.chars.invert();
        }
        if self.icase && !self.sets {
            set = self.canonicalize_class(set);
        }
        Ok(set)
    }

    #[cfg(not(feature = "regexp-unicode-props"))]
    fn property_set(
        &mut self,
        _name: &str,
        _value: Option<&str>,
        _negate: bool,
    ) -> Result<ClassSet, SyntaxError> {
        err("unicode property escapes are not supported")
    }

    /// `\q{a|bc|}` in a `v` class.
    fn class_strings(&mut self) -> Result<ClassSet, SyntaxError> {
        if self.cur() != '{' as u32 {
            return err("expecting '{' after \\q");
        }
        self.pos += 1;
        let mut set = ClassSet::default();
        loop {
            let mut s = Vec::new();
            while !self.at_end() && self.cur() != '}' as u32 && self.cur() != '|' as u32 {
                match self.class_atom(true, false)? {
                    ClassAtom::Char(c) => s.push_out(c),
                    ClassAtom::Set(_) => unreachable!("sets are not allowed in \\q"),
                }
            }
            if self.at_end() {
                return err("unexpected end");
            }
            set.add_string(s);
            if self.cur() == '}' as u32 {
                break;
            }
            self.pos += 1;
        }
        self.pos += 1;
        if self.icase {
            set = self.canonicalize_class(set);
        }
        Ok(set)
    }

    fn canonicalize_class(&self, set: ClassSet) -> ClassSet {
        let mut out = ClassSet {
            chars: set.chars.canonicalize(self.unicode),
            strings: Vec::new(),
        };
        for s in set.strings {
            out.add_string(s.iter().map(|&c| canonicalize(c, self.unicode)).collect());
        }
        out
    }

    fn class(&mut self) -> Result<Node, SyntaxError> {
        let set = self.nested_class()?;
        Ok(Node::Class {
            set: set.chars,
            strings: set.strings,
            icase: self.icase,
        })
    }

    /// One `[...]`, possibly nested in `v` mode (qjs' `re_parse_nested_class`).
    fn nested_class(&mut self) -> Result<ClassSet, SyntaxError> {
        self.enter()?;
        self.pos += 1; // '['
        let mut invert = false;
        if self.cur() == '^' as u32 && !self.at_end() {
            self.pos += 1;
            invert = true;
        }
        let mut cr = ClassSet::default();
        let mut is_first = true;
        loop {
            if self.cur() == ']' as u32 && !self.at_end() {
                break;
            }
            if self.cur() == '[' as u32 && self.sets && !self.at_end() {
                let inner = self.nested_class()?;
                cr.union(inner);
            } else {
                let c1 = self.class_atom(true, true)?;
                if self.cur() == '-' as u32
                    && !self.at_end()
                    && self.at(self.pos + 1) != ']' as u32
                    && !(self.at(self.pos + 1) == '-' as u32 && self.sets && is_first)
                {
                    self.class_range(&mut cr, c1)?;
                    // A range cannot be the left operand of `&&` or `--`.
                    is_first = false;
                } else {
                    self.add_atom(&mut cr, c1);
                }
            }
            if self.sets && is_first {
                if self.cur() == '&' as u32
                    && self.at(self.pos + 1) == '&' as u32
                    && self.at(self.pos + 2) != '&' as u32
                {
                    self.class_operations(&mut cr, b'&')?;
                } else if self.cur() == '-' as u32 && self.at(self.pos + 1) == '-' as u32 {
                    self.class_operations(&mut cr, b'-')?;
                }
            }
            is_first = false;
        }
        self.pos += 1; // ']'
        self.depth -= 1;
        if invert {
            if !cr.strings.is_empty() {
                // qjs' message, cut at its 64-byte error buffer.
                return err("negated character class with strings in regular expression debu");
            }
            cr.chars = cr.chars.invert();
        }
        Ok(cr)
    }

    /// `c1-c2` in a class, with `self.pos` at the `-`.
    fn class_range(&mut self, cr: &mut ClassSet, c1: ClassAtom) -> Result<(), SyntaxError> {
        let lo = match c1 {
            ClassAtom::Char(lo) => lo,
            ClassAtom::Set(_) if self.unicode => return err("invalid class range"),
            // Annex B: `\w-x` is `\w`, `-` and `x`.
            set => {
                self.add_atom(cr, set);
                return Ok(());
            }
        };
        let save = self.pos;
        self.pos += 1;
        match self.class_atom(true, true)? {
            ClassAtom::Set(_) if self.unicode => err("invalid class range"),
            ClassAtom::Set(_) => {
                // Annex B: the `-` and the set are parsed again as plain atoms.
                self.pos = save;
                self.add_atom(cr, ClassAtom::Char(lo));
                Ok(())
            }
            ClassAtom::Char(hi) => {
                if hi < lo {
                    return err("invalid class range");
                }
                let mut r = CharSet::new();
                r.add_range(lo, hi);
                if self.icase {
                    r = r.canonicalize(self.unicode);
                }
                cr.chars = cr.chars.union(&r);
                Ok(())
            }
        }
    }

    fn add_atom(&self, cr: &mut ClassSet, atom: ClassAtom) {
        match atom {
            ClassAtom::Set(s) => cr.union(s),
            ClassAtom::Char(c) => {
                let c = if self.icase {
                    canonicalize(c, self.unicode)
                } else {
                    c
                };
                cr.chars.add(c);
            }
        }
    }

    /// `&&` or `--` operations after the first operand of a `v` class.
    fn class_operations(&mut self, cr: &mut ClassSet, op: u8) -> Result<(), SyntaxError> {
        loop {
            if self.cur() == ']' as u32 && !self.at_end() {
                return Ok(());
            }
            let ok = if op == b'&' {
                self.cur() == '&' as u32
                    && self.at(self.pos + 1) == '&' as u32
                    && self.at(self.pos + 2) != '&' as u32
            } else {
                self.cur() == '-' as u32 && self.at(self.pos + 1) == '-' as u32
            };
            if !ok {
                return err("invalid operation in regular expression");
            }
            self.pos += 2;
            let operand = if self.cur() == '[' as u32 && !self.at_end() {
                self.nested_class()?
            } else {
                let atom = self.class_atom(true, true)?;
                let mut s = ClassSet::default();
                self.add_atom(&mut s, atom);
                s
            };
            if op == b'&' {
                cr.intersect(&operand);
            } else {
                cr.subtract(&operand);
            }
        }
    }
}

/// Two groups might both participate in a match unless some disjunction contains them in
/// different alternatives.
fn might_both_participate(a: &[(u32, u32)], b: &[(u32, u32)]) -> bool {
    for &(da, aa) in a {
        for &(db, ab) in b {
            if da == db && aa != ab {
                return false;
            }
        }
    }
    true
}
