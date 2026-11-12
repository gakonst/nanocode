use std::collections::HashMap;
use std::ops::Range;

use super::Error;
use super::set::{CodePointSet, Interval};
use super::unicode::{self, PropertyName};

pub(super) const GROUP_COUNT_MAX: usize = u16::MAX as usize;
pub(super) const LOOP_COUNT_MAX: usize = u16::MAX as usize;
pub(super) const NESTING_DEPTH_MAX: u32 = 256;

const ZERO_WIDTH_NON_JOINER: char = '\u{200C}';
const ZERO_WIDTH_JOINER: char = '\u{200D}';
const BACKSPACE: u32 = 0x08;
const CONTROL_LETTER_MODULUS: u32 = 32;
const SURROGATE_HIGH: std::ops::RangeInclusive<u16> = 0xD800..=0xDBFF;

#[derive(Clone, Copy, Debug)]
pub(super) struct Quantifier {
    pub(super) min: usize,
    pub(super) max: Option<usize>,
    pub(super) greedy: bool,
}

#[derive(Clone, Debug)]
pub(super) enum Node {
    Empty,
    Char(u32),
    CharSet(Vec<u32>),
    Bracket {
        invert: bool,
        set: CodePointSet,
    },
    MatchAny,
    MatchAnyExceptLineTerminator,
    StartOfLine {
        multiline: bool,
    },
    EndOfLine {
        multiline: bool,
    },
    WordBoundary {
        invert: bool,
        ignore_case: bool,
    },
    Cat(Vec<Self>),
    Alt(Vec<Self>),
    Group {
        id: u16,
        contents: Box<Self>,
    },
    BackReference {
        group: u32,
        ignore_case: bool,
    },
    Lookaround {
        negate: bool,
        backwards: bool,
        groups: Range<u16>,
        contents: Box<Self>,
    },
    Loop {
        body: Box<Self>,
        quantifier: Quantifier,
        groups: Range<u16>,
    },
}

#[derive(Clone, Copy)]
struct Flags {
    ignore_case: bool,
    multiline: bool,
    dot_all: bool,
}

#[derive(Clone, Copy)]
enum ClassType {
    Digits,
    Spaces,
    Words,
}

enum ClassAtom {
    CodePoint(u32),
    Class { class: ClassType, positive: bool },
    Set { set: CodePointSet, negate: bool },
}

#[derive(Clone, PartialEq, Eq)]
struct AlternativePath {
    segments: Vec<(usize, usize)>,
}

impl AlternativePath {
    fn conflicts_with(&self, other: &Self) -> bool {
        let shared = self.segments.len().min(other.segments.len());
        self.segments[..shared] == other.segments[..shared]
    }
}

pub(super) struct Parsed {
    pub(super) node: Node,
    pub(super) group_count: u16,
}

pub(super) fn parse(pattern: &str) -> Result<Parsed, Error> {
    let input: Vec<char> = pattern.chars().collect();
    let mut parser = Parser {
        input: &input,
        position: 0,
        flags: Flags {
            ignore_case: false,
            multiline: false,
            dot_all: false,
        },
        loop_count: 0,
        group_count: 0,
        group_count_max: 0,
        named_groups: HashMap::new(),
        has_lookbehind: false,
        depth: 0,
    };
    let mut node = parser.parse_pattern()?;
    if parser.has_lookbehind {
        reverse_lookbehind_cats(&mut node, false);
    }
    Ok(Parsed {
        node,
        group_count: parser.group_count,
    })
}

fn error<T>(text: impl Into<String>) -> Result<T, Error> {
    Err(Error { text: text.into() })
}

fn make_cat(mut nodes: Vec<Node>) -> Node {
    match nodes.len() {
        0 => Node::Empty,
        1 => nodes.pop().expect("one node"),
        _ => Node::Cat(nodes),
    }
}

fn make_alt(mut nodes: Vec<Node>) -> Node {
    match nodes.len() {
        0 => Node::Empty,
        1 => nodes.pop().expect("one node"),
        _ => Node::Alt(nodes),
    }
}

fn reverse_lookbehind_cats(node: &mut Node, in_lookbehind: bool) {
    match node {
        Node::Cat(nodes) => {
            if in_lookbehind {
                nodes.reverse();
            }
            for child in nodes {
                reverse_lookbehind_cats(child, in_lookbehind);
            }
        }
        Node::Alt(nodes) => {
            for child in nodes {
                reverse_lookbehind_cats(child, in_lookbehind);
            }
        }
        Node::Group { contents, .. } => reverse_lookbehind_cats(contents, in_lookbehind),
        Node::Loop { body, .. } => reverse_lookbehind_cats(body, in_lookbehind),
        Node::Lookaround {
            backwards,
            contents,
            ..
        } => reverse_lookbehind_cats(contents, *backwards),
        _ => {}
    }
}

fn class_set(class: ClassType) -> CodePointSet {
    match class {
        ClassType::Digits => CodePointSet::from_intervals(&unicode::DIGITS),
        ClassType::Words => CodePointSet::from_intervals(&unicode::WORD_CHARACTERS),
        ClassType::Spaces => {
            let mut set = CodePointSet::from_intervals(&unicode::WHITE_SPACE);
            set.add_set(&CodePointSet::from_intervals(&unicode::LINE_TERMINATORS));
            set
        }
    }
}

fn class_atom_set(class: ClassType, positive: bool) -> CodePointSet {
    let set = class_set(class);
    if positive { set } else { set.inverted() }
}

const fn is_syntax_character(c: char) -> bool {
    matches!(
        c,
        '^' | '$' | '\\' | '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '/'
    )
}

struct Parser<'a> {
    input: &'a [char],
    position: usize,
    flags: Flags,
    loop_count: usize,
    group_count: u16,
    group_count_max: u32,
    named_groups: HashMap<String, Vec<u32>>,
    has_lookbehind: bool,
    depth: u32,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.input.get(self.position).copied()
    }

    fn next(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.position += 1;
        Some(c)
    }

    fn consume(&mut self, expected: char) {
        let c = self.next();
        assert_eq!(c, Some(expected));
    }

    fn try_consume(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.position += 1;
            return true;
        }
        false
    }

    fn try_consume_str(&mut self, expected: &str) -> bool {
        let mut position = self.position;
        for c in expected.chars() {
            if self.input.get(position) != Some(&c) {
                return false;
            }
            position += 1;
        }
        self.position = position;
        true
    }

    fn char_node(&self, c: u32) -> Node {
        if !self.flags.ignore_case {
            return Node::Char(c);
        }
        let mut variants = unicode::unfold(c);
        if variants.len() == 1 {
            return Node::Char(variants.pop().expect("one variant"));
        }
        Node::CharSet(variants)
    }

    fn parse_pattern(&mut self) -> Result<Node, Error> {
        self.scan_capture_groups()?;
        let body = self.parse_disjunction()?;
        match self.peek() {
            Some(')') => error("Unbalanced parenthesis"),
            Some(c) => error(format!("Unexpected char: {c}")),
            None => Ok(body),
        }
    }

    fn parse_disjunction(&mut self) -> Result<Node, Error> {
        self.depth += 1;
        if self.depth > NESTING_DEPTH_MAX {
            return error("Regular expression is too deeply nested");
        }
        let mut terms = vec![self.parse_term()?];
        while self.try_consume('|') {
            terms.push(self.parse_term()?);
        }
        self.depth -= 1;
        Ok(make_alt(terms))
    }

    fn parse_term(&mut self) -> Result<Node, Error> {
        let mut result: Vec<Node> = Vec::new();
        while let Some(c) = self.peek() {
            if c == ')' || c == '|' {
                break;
            }
            let start_group = self.group_count;
            let quantifier_allowed = self.parse_atom(c, &mut result)?;
            let Some(quantifier) = self.try_consume_quantifier()? else {
                continue;
            };
            if !quantifier_allowed {
                return error("Quantifier not allowed here");
            }
            if quantifier.max.is_some_and(|max| quantifier.min > max) {
                return error("Invalid quantifier");
            }
            let body = result.pop().expect("an atom precedes its quantifier");
            if self.loop_count >= LOOP_COUNT_MAX {
                return error("Loop count limit exceeded");
            }
            self.loop_count += 1;
            result.push(Node::Loop {
                body: Box::new(body),
                quantifier,
                groups: start_group..self.group_count,
            });
        }
        Ok(make_cat(result))
    }

    fn parse_atom(&mut self, c: char, result: &mut Vec<Node>) -> Result<bool, Error> {
        match c {
            '^' => {
                self.consume('^');
                result.push(Node::StartOfLine {
                    multiline: self.flags.multiline,
                });
                Ok(false)
            }
            '$' => {
                self.consume('$');
                result.push(Node::EndOfLine {
                    multiline: self.flags.multiline,
                });
                Ok(false)
            }
            '\\' => {
                self.consume('\\');
                let Some(escaped) = self.peek() else {
                    return error("Incomplete escape");
                };
                if escaped == 'b' || escaped == 'B' {
                    self.consume(escaped);
                    result.push(Node::WordBoundary {
                        invert: escaped == 'B',
                        ignore_case: self.flags.ignore_case,
                    });
                } else {
                    result.push(self.parse_atom_escape()?);
                }
                Ok(true)
            }
            '.' => {
                self.consume('.');
                result.push(if self.flags.dot_all {
                    Node::MatchAny
                } else {
                    Node::MatchAnyExceptLineTerminator
                });
                Ok(true)
            }
            '(' => self.parse_group(result),
            '[' => {
                result.push(self.parse_bracket()?);
                Ok(true)
            }
            '*' | '+' | '?' | ']' | '{' | '}' => error("Invalid atom character"),
            _ => {
                self.consume(c);
                result.push(self.char_node(u32::from(c)));
                Ok(true)
            }
        }
    }

    fn parse_group(&mut self, result: &mut Vec<Node>) -> Result<bool, Error> {
        let mut quantifier_allowed = true;
        let lookarounds = [
            ("(?=", false, false),
            ("(?!", true, false),
            ("(?<=", false, true),
            ("(?<!", true, true),
        ];
        if let Some(&(_, negate, backwards)) = lookarounds
            .iter()
            .find(|(prefix, _, _)| self.try_consume_str(prefix))
        {
            quantifier_allowed = false;
            if backwards {
                self.has_lookbehind = true;
            }
            let start_group = self.group_count;
            let contents = self.parse_disjunction()?;
            result.push(Node::Lookaround {
                negate,
                backwards,
                groups: start_group..self.group_count,
                contents: Box::new(contents),
            });
        } else if self.try_consume_str("(?:") {
            result.push(self.parse_disjunction()?);
        } else if let Some(group) = self.try_parse_modifier_group()? {
            result.push(group);
        } else {
            self.consume('(');
            let id = self.group_count;
            if usize::from(self.group_count) >= GROUP_COUNT_MAX {
                return error("Capture group count limit exceeded");
            }
            self.group_count += 1;
            if self.try_consume('?') && self.try_parse_group_name().is_none() {
                return error("Invalid token at named capture group identifier");
            }
            let contents = self.parse_disjunction()?;
            result.push(Node::Group {
                id,
                contents: Box::new(contents),
            });
        }
        if !self.try_consume(')') {
            return error("Unbalanced parenthesis");
        }
        Ok(quantifier_allowed)
    }

    fn try_parse_modifier_group(&mut self) -> Result<Option<Node>, Error> {
        let mut cursor = self.position;
        if self.input.get(cursor) != Some(&'(') || self.input.get(cursor + 1) != Some(&'?') {
            return Ok(None);
        }
        cursor += 2;
        let Some(&first) = self.input.get(cursor) else {
            return Ok(None);
        };
        if first == '<' {
            return Ok(None);
        }
        let mut seen_hyphen = false;
        let mut saw_flag = false;
        let mut overrides: [Option<bool>; 3] = [None; 3];
        let mut current = first;
        loop {
            match current {
                'i' | 'm' | 's' => {
                    let slot = match current {
                        'i' => 0,
                        'm' => 1,
                        _ => 2,
                    };
                    if overrides[slot].is_some() {
                        return error("Invalid group modifier");
                    }
                    overrides[slot] = Some(!seen_hyphen);
                    saw_flag = true;
                }
                '-' => {
                    if seen_hyphen {
                        return error("Invalid group modifier");
                    }
                    seen_hyphen = true;
                }
                ':' => {
                    if !saw_flag {
                        return error("Invalid group modifier");
                    }
                    self.position = cursor + 1;
                    return self.parse_modified_disjunction(overrides).map(Some);
                }
                _ => return error("Invalid group modifier"),
            }
            cursor += 1;
            let Some(&next) = self.input.get(cursor) else {
                return error("Invalid group modifier");
            };
            current = next;
        }
    }

    fn parse_modified_disjunction(&mut self, overrides: [Option<bool>; 3]) -> Result<Node, Error> {
        let saved = self.flags;
        if let Some(value) = overrides[0] {
            self.flags.ignore_case = value;
        }
        if let Some(value) = overrides[1] {
            self.flags.multiline = value;
        }
        if let Some(value) = overrides[2] {
            self.flags.dot_all = value;
        }
        let contents = self.parse_disjunction();
        self.flags = saved;
        contents
    }

    fn parse_bracket(&mut self) -> Result<Node, Error> {
        self.consume('[');
        let invert = self.try_consume('^');
        let mut set = CodePointSet::new();
        loop {
            match self.peek() {
                None => return error("Unbalanced bracket"),
                Some(']') => {
                    self.consume(']');
                    if self.flags.ignore_case {
                        set = unicode::add_case_variants(&set);
                    }
                    return Ok(Node::Bracket { invert, set });
                }
                Some(_) => {}
            }
            let Some(first) = self.try_parse_class_atom()? else {
                continue;
            };
            if !self.try_consume('-') {
                add_class_atom(&mut set, first);
                continue;
            }
            let Some(second) = self.try_parse_class_atom()? else {
                add_class_atom(&mut set, first);
                add_class_atom(&mut set, ClassAtom::CodePoint(u32::from('-')));
                continue;
            };
            let (ClassAtom::CodePoint(low), ClassAtom::CodePoint(high)) = (&first, &second) else {
                return error("Invalid character range");
            };
            if low > high {
                return error(
                    "Range values reversed, start char code is greater than end char code.",
                );
            }
            set.add(Interval::new(*low, *high));
        }
    }

    fn try_parse_class_atom(&mut self) -> Result<Option<ClassAtom>, Error> {
        let Some(c) = self.peek() else {
            return Ok(None);
        };
        match c {
            ']' => Ok(None),
            '\\' => {
                self.consume('\\');
                let Some(escaped) = self.peek() else {
                    return error("Unterminated escape");
                };
                self.parse_class_escape(escaped).map(Some)
            }
            _ => {
                self.consume(c);
                Ok(Some(ClassAtom::CodePoint(u32::from(c))))
            }
        }
    }

    fn parse_class_escape(&mut self, escaped: char) -> Result<ClassAtom, Error> {
        let class = match escaped {
            'b' => {
                self.consume('b');
                return Ok(ClassAtom::CodePoint(BACKSPACE));
            }
            '-' => {
                self.consume('-');
                return Ok(ClassAtom::CodePoint(u32::from('-')));
            }
            'p' | 'P' => {
                self.consume(escaped);
                let set = self.parse_property_escape()?;
                return Ok(ClassAtom::Set {
                    set,
                    negate: escaped == 'P',
                });
            }
            'd' | 'D' => ClassType::Digits,
            's' | 'S' => ClassType::Spaces,
            'w' | 'W' => ClassType::Words,
            _ => return Ok(ClassAtom::CodePoint(self.parse_character_escape()?)),
        };
        self.consume(escaped);
        Ok(ClassAtom::Class {
            class,
            positive: escaped.is_ascii_lowercase(),
        })
    }

    fn try_consume_quantifier(&mut self) -> Result<Option<Quantifier>, Error> {
        let (min, max) = match self.peek() {
            Some(symbol @ ('+' | '*' | '?')) => {
                self.consume(symbol);
                match symbol {
                    '+' => (1, None),
                    '*' => (0, None),
                    _ => (0, Some(1)),
                }
            }
            Some('{') => match self.try_consume_braced_quantifier() {
                Some(bounds) => bounds,
                None => return error("Invalid quantifier"),
            },
            _ => return Ok(None),
        };
        let greedy = !self.try_consume('?');
        Ok(Some(Quantifier { min, max, greedy }))
    }

    fn try_consume_braced_quantifier(&mut self) -> Option<(usize, Option<usize>)> {
        let start = self.position;
        self.consume('{');
        let Some(min) = self.try_consume_decimal() else {
            self.position = start;
            return None;
        };
        let mut max = Some(min);
        if self.try_consume(',') {
            max = self.try_consume_decimal();
        }
        if !self.try_consume('}') {
            self.position = start;
            return None;
        }
        Some((min, max))
    }

    fn try_consume_decimal(&mut self) -> Option<usize> {
        let mut value: usize = 0;
        let mut digits = 0;
        while let Some(digit) = self.peek().and_then(|c| c.to_digit(10)) {
            self.position += 1;
            digits += 1;
            value = value.saturating_mul(10).saturating_add(digit as usize);
        }
        if digits > 0 { Some(value) } else { None }
    }
}

fn add_class_atom(set: &mut CodePointSet, atom: ClassAtom) {
    match atom {
        ClassAtom::CodePoint(c) => set.add_one(c),
        ClassAtom::Class { class, positive } => set.add_set(&class_atom_set(class, positive)),
        ClassAtom::Set {
            set: atom_set,
            negate,
        } => {
            if negate {
                set.add_set(&atom_set.inverted());
            } else {
                set.add_set(&atom_set);
            }
        }
    }
}

impl Parser<'_> {
    fn parse_atom_escape(&mut self) -> Result<Node, Error> {
        let Some(c) = self.peek() else {
            return error("Incomplete escape");
        };
        let class = match c {
            'd' | 'D' => ClassType::Digits,
            's' | 'S' => ClassType::Spaces,
            'w' | 'W' => ClassType::Words,
            'p' | 'P' => {
                self.consume(c);
                let set = self.parse_property_escape()?;
                return Ok(self.property_node(set, c == 'P'));
            }
            '1'..='9' => {
                let group = self.try_consume_decimal().expect("a digit follows");
                if group > self.group_count_max as usize {
                    return error("Invalid character escape");
                }
                return Ok(Node::BackReference {
                    group: u32::try_from(group).expect("bounded by the group count"),
                    ignore_case: self.flags.ignore_case,
                });
            }
            'k' => {
                self.consume('k');
                return self.parse_named_back_reference();
            }
            _ => {
                let c = self.parse_character_escape()?;
                return Ok(self.char_node(c));
            }
        };
        self.consume(c);
        let positive = c.is_ascii_lowercase();
        let mut set = class_set(class);
        if self.flags.ignore_case {
            set = unicode::add_case_variants(&set);
        }
        if !positive {
            set = set.inverted();
        }
        Ok(Node::Bracket { invert: false, set })
    }

    fn property_node(&self, set: CodePointSet, negate: bool) -> Node {
        if !self.flags.ignore_case {
            return Node::Bracket {
                invert: negate,
                set,
            };
        }
        let set = if negate {
            unicode::add_case_variants(&set.inverted())
        } else {
            unicode::add_case_variants(&set)
        };
        Node::Bracket { invert: false, set }
    }

    fn parse_named_back_reference(&mut self) -> Result<Node, Error> {
        let Some(name) = self.try_parse_group_name() else {
            return error("Invalid named backreference syntax");
        };
        let Some(indices) = self.named_groups.get(&name) else {
            return error(format!(
                "Backreference to invalid named capture group: {name}"
            ));
        };
        let ignore_case = self.flags.ignore_case;
        let references: Vec<Node> = indices
            .iter()
            .map(|index| Node::BackReference {
                group: index + 1,
                ignore_case,
            })
            .collect();
        assert!(!references.is_empty());
        Ok(make_alt(references))
    }

    fn parse_property_escape(&mut self) -> Result<CodePointSet, Error> {
        if !self.try_consume('{') {
            return error("Invalid character at property escape start");
        }
        let mut buffer = String::new();
        let mut name: Option<PropertyName> = None;
        while let Some(c) = self.peek() {
            match c {
                '}' => {
                    self.consume('}');
                    let Some(set) = unicode::property(&buffer, name) else {
                        break;
                    };
                    return Ok(set);
                }
                '=' if name.is_none() => {
                    self.consume('=');
                    let Some(property) = unicode::property_name(&buffer) else {
                        break;
                    };
                    name = Some(property);
                    buffer.clear();
                }
                c if c.is_ascii_alphanumeric() || c == '_' => {
                    self.consume(c);
                    buffer.push(c);
                }
                _ => break,
            }
        }
        error("Invalid property name")
    }

    fn parse_character_escape(&mut self) -> Result<u32, Error> {
        let c = self.next().expect("an escaped character follows");
        match c {
            'f' => Ok(0x0C),
            'n' => Ok(0x0A),
            'r' => Ok(0x0D),
            't' => Ok(0x09),
            'v' => Ok(0x0B),
            'c' => match self.next() {
                Some(letter) if letter.is_ascii_alphabetic() => {
                    Ok(u32::from(letter) % CONTROL_LETTER_MODULUS)
                }
                _ => error("Invalid character escape"),
            },
            '0' if !self.peek().is_some_and(|next| next.is_ascii_digit()) => Ok(0),
            'x' => {
                let high = self.next().and_then(|digit| digit.to_digit(16));
                let low = self.next().and_then(|digit| digit.to_digit(16));
                match (high, low) {
                    (Some(high), Some(low)) => Ok(high * 16 + low),
                    _ => error("Invalid character escape"),
                }
            }
            'u' => match self.try_parse_unicode_escape() {
                Some(code_point) => Ok(code_point),
                None => error("Invalid unicode escape"),
            },
            c if is_syntax_character(c) => Ok(u32::from(c)),
            _ => error("Invalid character escape"),
        }
    }

    fn try_parse_unicode_escape(&mut self) -> Option<u32> {
        let start = self.position;
        if self.try_consume('{') {
            let mut digits = String::new();
            loop {
                match self.next() {
                    Some('}') => break,
                    Some(c) => digits.push(c),
                    None => {
                        self.position = start;
                        return None;
                    }
                }
            }
            return match u32::from_str_radix(&digits, 16) {
                Ok(code_point) if code_point <= super::set::CODE_POINT_MAX => Some(code_point),
                _ => {
                    self.position = start;
                    None
                }
            };
        }
        let Some(unit) = self.try_parse_hex4() else {
            self.position = start;
            return None;
        };
        if !SURROGATE_HIGH.contains(&unit) || !self.try_consume_str("\\u") {
            return Some(u32::from(unit));
        }
        let after_escape = self.position;
        let pair = self
            .try_parse_hex4()
            .and_then(|low| char::decode_utf16([unit, low]).next()?.ok());
        match pair {
            Some(c) => Some(u32::from(c)),
            None => {
                self.position = after_escape;
                Some(u32::from(unit))
            }
        }
    }

    fn try_parse_hex4(&mut self) -> Option<u16> {
        let mut digits = String::new();
        for _ in 0..4 {
            digits.push(self.next()?);
        }
        u16::from_str_radix(&digits, 16).ok()
    }

    fn try_parse_group_name(&mut self) -> Option<String> {
        if !self.try_consume('<') {
            return None;
        }
        let start = self.position;
        let name = self.parse_group_name_body();
        if name.is_none() {
            self.position = start;
        }
        name
    }

    fn next_group_name_char(&mut self) -> Option<char> {
        let c = self.next()?;
        if c == '\\' && self.try_consume('u') {
            return self.try_parse_unicode_escape().and_then(char::from_u32);
        }
        Some(c)
    }

    fn parse_group_name_body(&mut self) -> Option<String> {
        let first = self.next_group_name_char()?;
        if !(unicode::is_identifier_start(u32::from(first)) || first == '$' || first == '_') {
            return None;
        }
        let mut name = String::from(first);
        loop {
            let c = self.next_group_name_char()?;
            if c == '>' {
                return Some(name);
            }
            let continues = unicode::is_identifier_continue(u32::from(c))
                || matches!(c, '$' | '_' | ZERO_WIDTH_NON_JOINER | ZERO_WIDTH_JOINER);
            if !continues {
                return None;
            }
            name.push(c);
        }
    }

    fn scan_capture_groups(&mut self) -> Result<(), Error> {
        let start = self.position;
        let mut depth: usize = 0;
        let mut alternatives: HashMap<usize, usize> = HashMap::from([(0, 0)]);
        let mut locations: HashMap<String, Vec<AlternativePath>> = HashMap::new();
        while let Some(c) = self.next() {
            match c {
                '\\' => {
                    self.next();
                }
                '[' => self.skip_bracket(),
                '(' => {
                    let (capturing, name) = if self.try_consume('?') {
                        let name = self.try_parse_group_name();
                        (name.is_some(), name)
                    } else {
                        (true, None)
                    };
                    if let Some(name) = name {
                        let segments = (0..=depth)
                            .map(|level| (level, alternatives.get(&level).copied().unwrap_or(0)))
                            .collect();
                        locations
                            .entry(name.clone())
                            .or_default()
                            .push(AlternativePath { segments });
                        self.named_groups
                            .entry(name)
                            .or_default()
                            .push(self.group_count_max);
                    }
                    if capturing && (self.group_count_max as usize) < GROUP_COUNT_MAX {
                        self.group_count_max += 1;
                    }
                    depth += 1;
                    alternatives.insert(depth, 0);
                }
                ')' => {
                    if depth > 0 {
                        alternatives.remove(&depth);
                        depth -= 1;
                    }
                }
                '|' => *alternatives.entry(depth).or_insert(0) += 1,
                _ => {}
            }
        }
        self.position = start;
        let conflict = locations.values().any(|paths| {
            paths.iter().enumerate().any(|(index, path)| {
                paths[index + 1..]
                    .iter()
                    .any(|other| path.conflicts_with(other))
            })
        });
        if conflict {
            return error("Duplicate capture group name");
        }
        Ok(())
    }

    fn skip_bracket(&mut self) {
        while let Some(c) = self.next() {
            match c {
                '\\' => {
                    self.next();
                }
                ']' => return,
                _ => {}
            }
        }
    }
}
