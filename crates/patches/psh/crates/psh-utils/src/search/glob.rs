use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, is_separator};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error {
    glob: String,
    kind: ErrorKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    UnclosedClass,
    InvalidRange(char, char),
    UnopenedAlternates,
    UnclosedAlternates,
    DanglingEscape,
}

impl Error {
    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }
}

impl std::error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "error parsing glob '{}': {}", self.glob, self.kind)
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            ErrorKind::UnclosedClass => f.write_str("unclosed character class; missing ']'"),
            ErrorKind::InvalidRange(start, end) => {
                write!(f, "invalid range; '{start}' > '{end}'")
            }
            ErrorKind::UnopenedAlternates => {
                f.write_str("unopened alternate group; missing '{' (maybe escape '}' with '[}]'?)")
            }
            ErrorKind::UnclosedAlternates => {
                f.write_str("unclosed alternate group; missing '}' (maybe escape '{' with '[{]'?)")
            }
            ErrorKind::DanglingEscape => f.write_str("dangling '\\'"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Token {
    Literal(char),
    Any,
    ZeroOrMore,
    RecursivePrefix,
    RecursiveSuffix,
    RecursiveZeroOrMore,
    Class {
        negated: bool,
        ranges: Vec<(char, char)>,
    },
    Alternates(usize),
}

#[derive(Clone, Copy, Debug)]
struct MatchOptions {
    case_insensitive: bool,
    literal_separator: bool,
}

#[derive(Clone, Copy, Debug)]
struct ParseOptions {
    backslash_escape: bool,
    allow_unclosed_class: bool,
}

#[derive(Clone, Debug)]
pub struct Glob {
    tokens: Vec<Token>,
    groups: Vec<Vec<Vec<Token>>>,
    options: MatchOptions,
}

#[derive(Clone, Debug)]
pub struct GlobBuilder<'a> {
    glob: &'a str,
    match_options: MatchOptions,
    parse_options: ParseOptions,
}

impl<'a> GlobBuilder<'a> {
    pub fn new(glob: &'a str) -> GlobBuilder<'a> {
        GlobBuilder {
            glob,
            match_options: MatchOptions {
                case_insensitive: false,
                literal_separator: false,
            },
            parse_options: ParseOptions {
                backslash_escape: !is_separator('\\'),
                allow_unclosed_class: false,
            },
        }
    }

    pub fn case_insensitive(&mut self, yes: bool) -> &mut GlobBuilder<'a> {
        self.match_options.case_insensitive = yes;
        self
    }

    pub fn literal_separator(&mut self, yes: bool) -> &mut GlobBuilder<'a> {
        self.match_options.literal_separator = yes;
        self
    }

    pub fn backslash_escape(&mut self, yes: bool) -> &mut GlobBuilder<'a> {
        self.parse_options.backslash_escape = yes;
        self
    }

    pub fn allow_unclosed_class(&mut self, yes: bool) -> &mut GlobBuilder<'a> {
        self.parse_options.allow_unclosed_class = yes;
        self
    }

    pub fn build(&self) -> Result<Glob, Error> {
        let mut parser = Parser {
            chars: self.glob.chars().collect(),
            at: 0,
            previous: None,
            current: None,
            unclosed_class_seen: false,
            alternation_starts: Vec::new(),
            branches: vec![Vec::new()],
            groups: Vec::new(),
            options: self.parse_options,
        };
        let mut parsed = parser.parse();
        if parsed.is_ok() && parser.branches.len() > 1 {
            parsed = Err(ErrorKind::UnclosedAlternates);
        }
        if let Err(kind) = parsed {
            return Err(Error {
                glob: self.glob.to_string(),
                kind,
            });
        }
        let tokens = parser.branches.pop().expect("the outermost branch remains");
        Ok(Glob {
            tokens,
            groups: parser.groups,
            options: self.match_options,
        })
    }
}

struct Parser {
    chars: Vec<char>,
    at: usize,
    previous: Option<char>,
    current: Option<char>,
    unclosed_class_seen: bool,
    alternation_starts: Vec<usize>,
    branches: Vec<Vec<Token>>,
    groups: Vec<Vec<Vec<Token>>>,
    options: ParseOptions,
}

impl Parser {
    fn bump(&mut self) -> Option<char> {
        self.previous = self.current;
        self.current = self.chars.get(self.at).copied();
        if self.current.is_some() {
            self.at += 1;
        }
        self.current
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn branch(&mut self) -> &mut Vec<Token> {
        self.branches.last_mut().expect("a branch is always open")
    }

    fn push(&mut self, token: Token) {
        self.branch().push(token);
    }

    fn parse(&mut self) -> Result<(), ErrorKind> {
        while let Some(c) = self.bump() {
            match c {
                '?' => self.push(Token::Any),
                '*' => self.parse_star(),
                '[' if !self.unclosed_class_seen => self.parse_class()?,
                '{' => {
                    self.alternation_starts.push(self.branches.len());
                    self.branches.push(Vec::new());
                }
                '}' => {
                    let start = self
                        .alternation_starts
                        .pop()
                        .ok_or(ErrorKind::UnopenedAlternates)?;
                    assert!(start >= 1);
                    let group: Vec<Vec<Token>> = self.branches.drain(start..).collect();
                    self.groups.push(group);
                    self.push(Token::Alternates(self.groups.len() - 1));
                }
                ',' if self.alternation_starts.is_empty() => self.push(Token::Literal(',')),
                ',' => self.branches.push(Vec::new()),
                '\\' if self.options.backslash_escape => {
                    let escaped = self.bump().ok_or(ErrorKind::DanglingEscape)?;
                    self.push(Token::Literal(escaped));
                }
                '\\' if is_separator('\\') => self.push(Token::Literal('/')),
                c => self.push(Token::Literal(c)),
            }
        }
        Ok(())
    }

    fn parse_star(&mut self) {
        let previous = self.previous;
        if self.peek() != Some('*') {
            self.push(Token::ZeroOrMore);
            return;
        }
        self.bump();
        if self.branch().is_empty() {
            match self.peek() {
                Some(c) if !is_separator(c) => self.push_two_stars(),
                _ => {
                    self.push(Token::RecursivePrefix);
                    self.bump();
                }
            }
            return;
        }
        let after_separator = previous.is_some_and(is_separator);
        let after_alternation_start = previous == Some(',') || previous == Some('{');
        if !after_separator && (self.branches.len() <= 1 || !after_alternation_start) {
            self.push_two_stars();
            return;
        }
        let is_suffix = match self.peek() {
            None => {
                self.bump();
                true
            }
            Some(',' | '}') if self.branches.len() >= 2 => true,
            Some(c) if is_separator(c) => {
                self.bump();
                false
            }
            _ => {
                self.push_two_stars();
                return;
            }
        };
        let last = self.branch().pop().expect("the branch has tokens");
        let token = match last {
            Token::RecursivePrefix => Token::RecursivePrefix,
            Token::RecursiveSuffix => Token::RecursiveSuffix,
            _ if is_suffix => Token::RecursiveSuffix,
            _ => Token::RecursiveZeroOrMore,
        };
        self.push(token);
    }

    fn push_two_stars(&mut self) {
        self.push(Token::ZeroOrMore);
        self.push(Token::ZeroOrMore);
    }

    fn parse_class(&mut self) -> Result<(), ErrorKind> {
        let saved = (self.at, self.previous, self.current);
        let negated = matches!(self.peek(), Some('!' | '^'));
        if negated {
            self.bump();
        }
        let mut ranges: Vec<(char, char)> = Vec::new();
        let mut first = true;
        let mut in_range = false;
        loop {
            let Some(c) = self.bump() else {
                if !self.options.allow_unclosed_class {
                    return Err(ErrorKind::UnclosedClass);
                }
                (self.at, self.previous, self.current) = saved;
                self.unclosed_class_seen = true;
                self.push(Token::Literal('['));
                return Ok(());
            };
            match c {
                ']' if first => ranges.push((']', ']')),
                ']' => break,
                '-' if first => ranges.push(('-', '-')),
                '-' if in_range => {
                    extend_range(&mut ranges, '-')?;
                    in_range = false;
                }
                '-' => {
                    assert!(!ranges.is_empty());
                    in_range = true;
                }
                c if in_range => {
                    extend_range(&mut ranges, c)?;
                    in_range = false;
                }
                c => ranges.push((c, c)),
            }
            first = false;
        }
        if in_range {
            ranges.push(('-', '-'));
        }
        self.push(Token::Class { negated, ranges });
        Ok(())
    }
}

fn extend_range(ranges: &mut [(char, char)], end: char) -> Result<(), ErrorKind> {
    let range = ranges.last_mut().expect("a range is open");
    range.1 = end;
    if range.1 < range.0 {
        return Err(ErrorKind::InvalidRange(range.0, range.1));
    }
    Ok(())
}

pub fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '?' | '*' | '[' | ']' | '{' | '}' => {
                escaped.push('[');
                escaped.push(c);
                escaped.push(']');
            }
            c => escaped.push(c),
        }
    }
    escaped
}

impl Glob {
    pub fn compile_matcher(&self) -> GlobMatcher {
        GlobMatcher {
            program: Program::new(self, &self.tokens),
        }
    }

    fn literal(&self) -> Option<String> {
        if self.options.case_insensitive {
            return None;
        }
        let literal = literal_text(&self.tokens)?;
        (!literal.is_empty()).then_some(literal)
    }

    fn extension(&self) -> Option<String> {
        if self.options.case_insensitive {
            return None;
        }
        let start = usize::from(self.tokens.first()? == &Token::RecursivePrefix);
        if self.tokens.get(start)? != &Token::ZeroOrMore {
            return None;
        }
        if start == 0 && self.options.literal_separator {
            return None;
        }
        if self.tokens.get(start + 1)? != &Token::Literal('.') {
            return None;
        }
        let mut extension = ".".to_string();
        for token in &self.tokens[start + 2..] {
            match *token {
                Token::Literal('.' | '/') => return None,
                Token::Literal(c) => extension.push(c),
                _ => return None,
            }
        }
        Some(extension)
    }

    fn required_extension(&self) -> Option<String> {
        if self.options.case_insensitive {
            return None;
        }
        let mut reversed: Vec<char> = Vec::new();
        for token in self.tokens.iter().rev() {
            match *token {
                Token::Literal('/') => return None,
                Token::Literal(c) => {
                    reversed.push(c);
                    if c == '.' {
                        break;
                    }
                }
                _ => return None,
            }
        }
        if reversed.last() != Some(&'.') {
            return None;
        }
        Some(reversed.into_iter().rev().collect())
    }

    fn prefix(&self) -> Option<String> {
        if self.options.case_insensitive {
            return None;
        }
        let (end, needs_separator) = match *self.tokens.last()? {
            Token::ZeroOrMore if self.options.literal_separator => return None,
            Token::ZeroOrMore => (self.tokens.len() - 1, false),
            Token::RecursiveSuffix => (self.tokens.len() - 1, true),
            _ => (self.tokens.len(), false),
        };
        let mut prefix = literal_text(&self.tokens[..end])?;
        if needs_separator {
            prefix.push('/');
        }
        (!prefix.is_empty()).then_some(prefix)
    }

    fn suffix(&self) -> Option<(String, bool)> {
        if self.options.case_insensitive {
            return None;
        }
        let mut suffix = String::new();
        let (mut start, component) = match *self.tokens.first()? {
            Token::RecursivePrefix if matches!(self.tokens.get(1), Some(Token::Literal(_))) => {
                suffix.push('/');
                (1, true)
            }
            Token::RecursivePrefix => (1, false),
            _ => (0, false),
        };
        if *self.tokens.get(start)? == Token::ZeroOrMore {
            if self.options.literal_separator {
                return None;
            }
            start += 1;
        }
        suffix.push_str(&literal_text(&self.tokens[start..])?);
        if suffix.is_empty() || suffix == "/" {
            return None;
        }
        Some((suffix, component))
    }

    fn basename_tokens(&self) -> Option<&[Token]> {
        if *self.tokens.first()? != Token::RecursivePrefix {
            return None;
        }
        let rest = &self.tokens[1..];
        if rest.is_empty() {
            return None;
        }
        for token in rest {
            match *token {
                Token::Literal('/') => return None,
                Token::Literal(_) => {}
                Token::Any | Token::ZeroOrMore if !self.options.literal_separator => return None,
                Token::Any | Token::ZeroOrMore => {}
                Token::RecursivePrefix
                | Token::RecursiveSuffix
                | Token::RecursiveZeroOrMore
                | Token::Alternates(_) => return None,
                Token::Class {
                    negated,
                    ref ranges,
                } => {
                    let has_slash = ranges
                        .iter()
                        .any(|&(start, end)| start <= '/' && '/' <= end);
                    if has_slash != negated {
                        return None;
                    }
                }
            }
        }
        Some(rest)
    }

    fn strategy(&self) -> Strategy {
        if !self.options.case_insensitive
            && let Some(tokens) = self.basename_tokens()
            && let Some(literal) = literal_text(tokens)
        {
            return Strategy::BasenameLiteral(literal);
        }
        if let Some(literal) = self.literal() {
            return Strategy::Literal(literal);
        }
        if let Some(extension) = self.extension() {
            return Strategy::Extension(extension);
        }
        if let Some(prefix) = self.prefix() {
            return Strategy::Prefix(prefix);
        }
        if let Some((suffix, component)) = self.suffix() {
            return Strategy::Suffix { suffix, component };
        }
        if let Some(extension) = self.required_extension() {
            return Strategy::RequiredExtension(extension);
        }
        if let Some(tokens) = self.basename_tokens() {
            return Strategy::BasenameProgram(Program::new(self, tokens));
        }
        Strategy::Program
    }
}

fn literal_text(tokens: &[Token]) -> Option<String> {
    let mut text = String::new();
    for token in tokens {
        let Token::Literal(c) = *token else {
            return None;
        };
        text.push(c);
    }
    Some(text)
}

enum Strategy {
    Literal(String),
    BasenameLiteral(String),
    Extension(String),
    Prefix(String),
    Suffix { suffix: String, component: bool },
    RequiredExtension(String),
    BasenameProgram(Program),
    Program,
}

const BYTE_VALUES: usize = 256;
const BITS_PER_WORD: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ByteSet([u64; BYTE_VALUES / BITS_PER_WORD]);

impl ByteSet {
    fn empty() -> ByteSet {
        ByteSet([0; BYTE_VALUES / BITS_PER_WORD])
    }

    fn all() -> ByteSet {
        ByteSet([u64::MAX; BYTE_VALUES / BITS_PER_WORD])
    }

    fn insert(&mut self, byte: u8) {
        let at = usize::from(byte);
        self.0[at / BITS_PER_WORD] |= 1 << (at % BITS_PER_WORD);
    }

    fn remove(&mut self, byte: u8) {
        let at = usize::from(byte);
        self.0[at / BITS_PER_WORD] &= !(1 << (at % BITS_PER_WORD));
    }

    fn insert_range(&mut self, low: u8, high: u8) {
        assert!(low <= high);
        for byte in low..=high {
            self.insert(byte);
        }
    }

    fn contains(&self, byte: u8) -> bool {
        let at = usize::from(byte);
        self.0[at / BITS_PER_WORD] & (1 << (at % BITS_PER_WORD)) != 0
    }

    fn fold_ascii_case(&mut self) {
        for byte in b'A'..=b'Z' {
            let lower = byte.to_ascii_lowercase();
            if self.contains(byte) || self.contains(lower) {
                self.insert(byte);
                self.insert(lower);
            }
        }
    }

    fn complement(&mut self) {
        for word in &mut self.0 {
            *word = !*word;
        }
    }
}

enum Node {
    Consume { set: ByteSet, next: usize },
    Split(Vec<usize>),
    Accept,
}

struct NodeBuilder {
    nodes: Vec<Node>,
    options: MatchOptions,
}

impl NodeBuilder {
    fn add(&mut self, node: Node) -> usize {
        self.nodes.push(node);
        self.nodes.len() - 1
    }

    fn consume(&mut self, set: ByteSet, next: usize) -> usize {
        self.add(Node::Consume { set, next })
    }

    fn star(&mut self, set: ByteSet, next: usize) -> usize {
        let split = self.add(Node::Split(Vec::new()));
        let consume = self.consume(set, split);
        self.nodes[split] = Node::Split(vec![consume, next]);
        split
    }

    fn byte(&self, byte: u8) -> ByteSet {
        let mut set = ByteSet::empty();
        set.insert(byte);
        if self.options.case_insensitive {
            set.fold_ascii_case();
        }
        set
    }

    fn wildcard(&self) -> ByteSet {
        let mut set = ByteSet::all();
        if self.options.literal_separator {
            set.remove(b'/');
        }
        set
    }

    fn class(&self, negated: bool, ranges: &[(char, char)]) -> ByteSet {
        let mut set = ByteSet::empty();
        let mut start_buffer = [0; 4];
        let mut end_buffer = [0; 4];
        for &(start, end) in ranges {
            let start_bytes = start.encode_utf8(&mut start_buffer).as_bytes();
            if start == end {
                for &byte in start_bytes {
                    set.insert(byte);
                }
                continue;
            }
            let end_bytes = end.encode_utf8(&mut end_buffer).as_bytes();
            let (start_last, start_lead) = start_bytes.split_last().expect("a char has bytes");
            let (end_first, end_rest) = end_bytes.split_first().expect("a char has bytes");
            for &byte in start_lead {
                set.insert(byte);
            }
            set.insert_range(*start_last, *end_first);
            for &byte in end_rest {
                set.insert(byte);
            }
        }
        if self.options.case_insensitive {
            set.fold_ascii_case();
        }
        if negated {
            set.complement();
        }
        set
    }

    fn token(&mut self, token: &Token, next: usize) -> usize {
        match *token {
            Token::Literal(c) => {
                let mut buffer = [0; 4];
                let mut start = next;
                for &byte in c.encode_utf8(&mut buffer).as_bytes().iter().rev() {
                    let set = self.byte(byte);
                    start = self.consume(set, start);
                }
                start
            }
            Token::Any => {
                let set = self.wildcard();
                self.consume(set, next)
            }
            Token::ZeroOrMore => {
                let set = self.wildcard();
                self.star(set, next)
            }
            Token::RecursivePrefix => {
                let slash = self.consume(self.byte(b'/'), next);
                let anything = self.star(ByteSet::all(), slash);
                self.add(Node::Split(vec![next, slash, anything]))
            }
            Token::RecursiveSuffix => {
                let anything = self.star(ByteSet::all(), next);
                self.consume(self.byte(b'/'), anything)
            }
            Token::RecursiveZeroOrMore => {
                let slash = self.consume(self.byte(b'/'), next);
                let anything = self.star(ByteSet::all(), slash);
                let choice = self.add(Node::Split(vec![next, anything]));
                self.consume(self.byte(b'/'), choice)
            }
            Token::Class {
                negated,
                ref ranges,
            } => {
                let set = self.class(negated, ranges);
                self.consume(set, next)
            }
            Token::Alternates(_) => unreachable!("alternates compile through the frame stack"),
        }
    }
}

struct Frame<'g> {
    tokens: &'g [Token],
    next: usize,
    alternation: Option<Alternation<'g>>,
}

struct Alternation<'g> {
    branches: &'g [Vec<Token>],
    branch: usize,
    starts: Vec<usize>,
}

fn empty_groups(groups: &[Vec<Vec<Token>>]) -> Vec<bool> {
    let mut empty: Vec<bool> = Vec::with_capacity(groups.len());
    for group in groups {
        let all_empty = group.iter().all(|branch| branch_is_empty(branch, &empty));
        empty.push(all_empty);
    }
    empty
}

fn branch_is_empty(branch: &[Token], empty_groups: &[bool]) -> bool {
    branch.iter().all(|token| match *token {
        Token::Alternates(group) => empty_groups[group],
        _ => false,
    })
}

fn compile(glob: &Glob, tokens: &[Token]) -> (Vec<Node>, usize) {
    let mut builder = NodeBuilder {
        nodes: vec![Node::Accept],
        options: glob.options,
    };
    let accept = 0;
    if tokens == [Token::RecursivePrefix] {
        let start = builder.star(ByteSet::all(), accept);
        return (builder.nodes, start);
    }
    let empty = empty_groups(&glob.groups);
    let mut stack = vec![Frame {
        tokens,
        next: accept,
        alternation: None,
    }];
    let mut returned: Option<usize> = None;
    let mut start = accept;
    while let Some(frame) = stack.last_mut() {
        if let Some(alternation) = &mut frame.alternation {
            if let Some(branch_start) = returned.take() {
                alternation.starts.push(branch_start);
            }
            while alternation.branch < alternation.branches.len()
                && branch_is_empty(&alternation.branches[alternation.branch], &empty)
            {
                alternation.branch += 1;
            }
            if let Some(branch) = alternation.branches.get(alternation.branch) {
                alternation.branch += 1;
                let next = frame.next;
                stack.push(Frame {
                    tokens: branch,
                    next,
                    alternation: None,
                });
                continue;
            }
            let starts = std::mem::take(&mut alternation.starts);
            assert!(!starts.is_empty());
            frame.alternation = None;
            frame.next = builder.add(Node::Split(starts));
            frame.tokens = &frame.tokens[..frame.tokens.len() - 1];
            continue;
        }
        let Some((last, rest)) = frame.tokens.split_last() else {
            let finished = frame.next;
            stack.pop();
            if stack.is_empty() {
                start = finished;
            } else {
                returned = Some(finished);
            }
            continue;
        };
        if let Token::Alternates(group) = *last {
            if empty[group] {
                frame.tokens = rest;
            } else {
                frame.alternation = Some(Alternation {
                    branches: &glob.groups[group],
                    branch: 0,
                    starts: Vec::new(),
                });
            }
            continue;
        }
        frame.next = builder.token(last, frame.next);
        frame.tokens = rest;
    }
    (builder.nodes, start)
}

#[derive(Clone, Debug)]
struct Program {
    words: usize,
    accept_bit: usize,
    start: Vec<u64>,
    follow: Vec<u64>,
    accepts: Vec<u64>,
}

impl Program {
    fn new(glob: &Glob, tokens: &[Token]) -> Program {
        let (nodes, entry) = compile(glob, tokens);
        let mut position_of: Vec<Option<usize>> = vec![None; nodes.len()];
        let mut positions = 0;
        for (index, node) in nodes.iter().enumerate() {
            if matches!(node, Node::Consume { .. }) {
                position_of[index] = Some(positions);
                positions += 1;
            }
        }
        let accept_bit = positions;
        let words = (positions + 1).div_ceil(BITS_PER_WORD);
        let mut closures = Closures {
            nodes: &nodes,
            position_of: &position_of,
            accept_bit,
            words,
            memo: vec![None; nodes.len()],
            visited: vec![false; nodes.len()],
        };
        let start = closures.of(entry);
        let mut follow = vec![0; positions * words];
        let mut accepts = vec![0; BYTE_VALUES * words];
        for (index, node) in nodes.iter().enumerate() {
            let (Node::Consume { set, next }, Some(position)) = (node, position_of[index]) else {
                continue;
            };
            let closure = closures.of(*next);
            follow[position * words..(position + 1) * words].copy_from_slice(&closure);
            for byte in 0..=u8::MAX {
                if set.contains(byte) {
                    accepts[usize::from(byte) * words + position / BITS_PER_WORD] |=
                        1 << (position % BITS_PER_WORD);
                }
            }
        }
        Program {
            words,
            accept_bit,
            start,
            follow,
            accepts,
        }
    }

    fn is_match(&self, haystack: &[u8]) -> bool {
        if self.words == 1 {
            return self.is_match_one_word(haystack);
        }
        let words = self.words;
        let mut current = self.start.clone();
        let mut next = vec![0u64; words];
        for &byte in haystack {
            next.fill(0);
            let accepts = &self.accepts[usize::from(byte) * words..(usize::from(byte) + 1) * words];
            let mut any = false;
            for (word_index, (&active, &accepted)) in current.iter().zip(accepts).enumerate() {
                let mut bits = active & accepted;
                while bits != 0 {
                    let position = word_index * BITS_PER_WORD + bits.trailing_zeros() as usize;
                    let follow = &self.follow[position * words..(position + 1) * words];
                    for (target, &source) in next.iter_mut().zip(follow) {
                        *target |= source;
                    }
                    any = true;
                    bits &= bits - 1;
                }
            }
            if !any {
                return false;
            }
            std::mem::swap(&mut current, &mut next);
        }
        current[self.accept_bit / BITS_PER_WORD] & (1 << (self.accept_bit % BITS_PER_WORD)) != 0
    }

    fn is_match_one_word(&self, haystack: &[u8]) -> bool {
        let mut current = self.start[0];
        for &byte in haystack {
            let mut bits = current & self.accepts[usize::from(byte)];
            if bits == 0 {
                return false;
            }
            let mut next = 0;
            while bits != 0 {
                next |= self.follow[bits.trailing_zeros() as usize];
                bits &= bits - 1;
            }
            current = next;
        }
        current & (1 << self.accept_bit) != 0
    }
}

struct Closures<'n> {
    nodes: &'n [Node],
    position_of: &'n [Option<usize>],
    accept_bit: usize,
    words: usize,
    memo: Vec<Option<Vec<u64>>>,
    visited: Vec<bool>,
}

impl Closures<'_> {
    fn of(&mut self, node: usize) -> Vec<u64> {
        if let Some(found) = &self.memo[node] {
            return found.clone();
        }
        let mut set = vec![0u64; self.words];
        self.visited.fill(false);
        let mut pending = vec![node];
        while let Some(at) = pending.pop() {
            if self.visited[at] {
                continue;
            }
            self.visited[at] = true;
            match &self.nodes[at] {
                Node::Consume { .. } => {
                    let position = self.position_of[at].expect("a consuming node has a position");
                    set[position / BITS_PER_WORD] |= 1 << (position % BITS_PER_WORD);
                }
                Node::Split(targets) => pending.extend(targets.iter().copied()),
                Node::Accept => {
                    set[self.accept_bit / BITS_PER_WORD] |= 1 << (self.accept_bit % BITS_PER_WORD);
                }
            }
        }
        self.memo[node] = Some(set.clone());
        set
    }
}

#[derive(Clone, Debug)]
pub struct GlobMatcher {
    program: Program,
}

impl GlobMatcher {
    pub fn is_match<P: AsRef<Path>>(&self, path: P) -> bool {
        self.program.is_match(&path_bytes(path.as_ref()))
    }
}

pub(crate) struct Candidate<'a> {
    path: Cow<'a, [u8]>,
    basename_end: usize,
    basename_start: usize,
    extension_start: usize,
}

impl<'a> Candidate<'a> {
    pub(crate) fn new(path: &'a Path) -> Candidate<'a> {
        let path = path_bytes(path);
        let (basename_start, basename_end) = match path.iter().rposition(|&b| b == b'/') {
            _ if path.is_empty() => (0, 0),
            Some(slash) if &path[slash + 1..] == b".." => (0, 0),
            None if &path[..] == b".." => (0, 0),
            Some(slash) => (slash + 1, path.len()),
            None => (0, path.len()),
        };
        let extension_start = path[basename_start..basename_end]
            .iter()
            .rposition(|&b| b == b'.')
            .map_or(basename_end, |dot| basename_start + dot);
        Candidate {
            path,
            basename_end,
            basename_start,
            extension_start,
        }
    }

    fn basename(&self) -> &[u8] {
        &self.path[self.basename_start..self.basename_end]
    }

    fn extension(&self) -> &[u8] {
        &self.path[self.extension_start..self.basename_end]
    }

    fn basename_for_program(&self) -> &[u8] {
        if self.basename().is_empty() && self.path.ends_with(b"..") {
            return b"..";
        }
        self.basename()
    }
}

fn path_bytes(path: &Path) -> Cow<'_, [u8]> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Cow::Borrowed(path.as_os_str().as_bytes())
    }
    #[cfg(not(unix))]
    {
        let mut bytes = path.to_string_lossy().into_owned().into_bytes();
        for byte in &mut bytes {
            if *byte != b'/' && is_separator(char::from(*byte)) {
                *byte = b'/';
            }
        }
        Cow::Owned(bytes)
    }
}

#[derive(Clone, Debug, Default)]
pub struct GlobSetBuilder {
    globs: Vec<Glob>,
}

impl GlobSetBuilder {
    pub fn new() -> GlobSetBuilder {
        GlobSetBuilder::default()
    }

    pub fn add(&mut self, glob: Glob) -> &mut GlobSetBuilder {
        self.globs.push(glob);
        self
    }

    pub fn build(&self) -> GlobSet {
        let mut set = GlobSet::empty();
        for (index, glob) in self.globs.iter().enumerate() {
            set.len += 1;
            match glob.strategy() {
                Strategy::Literal(literal) => add_literal(&mut set.literals, literal, index),
                Strategy::BasenameLiteral(literal) => {
                    add_literal(&mut set.basename_literals, literal, index)
                }
                Strategy::Extension(extension) => {
                    add_literal(&mut set.extensions, extension, index)
                }
                Strategy::Prefix(prefix) => set.prefixes.push((prefix.into_bytes(), index)),
                Strategy::Suffix { suffix, component } => {
                    if component {
                        add_literal(&mut set.literals, suffix[1..].to_string(), index);
                    }
                    set.suffixes.push((suffix.into_bytes(), index));
                }
                Strategy::RequiredExtension(extension) => set
                    .required_extensions
                    .entry(extension.into_bytes())
                    .or_default()
                    .push((index, glob.compile_matcher().program)),
                Strategy::BasenameProgram(program) => set.basename_programs.push((index, program)),
                Strategy::Program => set.programs.push((index, glob.compile_matcher().program)),
            }
        }
        set
    }
}

fn add_literal(map: &mut HashMap<Vec<u8>, Vec<usize>>, literal: String, index: usize) {
    map.entry(literal.into_bytes()).or_default().push(index);
}

#[derive(Clone, Debug, Default)]
pub struct GlobSet {
    len: usize,
    extensions: HashMap<Vec<u8>, Vec<usize>>,
    basename_literals: HashMap<Vec<u8>, Vec<usize>>,
    literals: HashMap<Vec<u8>, Vec<usize>>,
    suffixes: Vec<(Vec<u8>, usize)>,
    prefixes: Vec<(Vec<u8>, usize)>,
    required_extensions: HashMap<Vec<u8>, Vec<(usize, Program)>>,
    basename_programs: Vec<(usize, Program)>,
    programs: Vec<(usize, Program)>,
}

impl GlobSet {
    pub fn empty() -> GlobSet {
        GlobSet::default()
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn is_match<P: AsRef<Path>>(&self, path: P) -> bool {
        let candidate = Candidate::new(path.as_ref());
        let mut found = false;
        self.visit_matches(&candidate, &mut |_| found = true);
        found
    }

    pub fn matches_all<P: AsRef<Path>>(&self, path: P) -> bool {
        let candidate = Candidate::new(path.as_ref());
        let path = &candidate.path[..];
        let basename = candidate.basename();
        let extension = candidate.extension();
        let single_hit = |map: &HashMap<Vec<u8>, Vec<usize>>, key: &[u8]| {
            map.is_empty() || (map.len() == 1 && map.contains_key(key))
        };
        if !single_hit(&self.extensions, extension)
            || (!self.extensions.is_empty() && extension.is_empty())
        {
            return false;
        }
        if !self.basename_literals.is_empty() && basename.is_empty() {
            return false;
        }
        if !single_hit(&self.basename_literals, basename) || !single_hit(&self.literals, path) {
            return false;
        }
        if !self
            .suffixes
            .iter()
            .all(|(suffix, _)| path.ends_with(suffix))
        {
            return false;
        }
        if !self
            .prefixes
            .iter()
            .all(|(prefix, _)| path.starts_with(prefix))
        {
            return false;
        }
        if !self.required_extensions.is_empty() {
            if extension.is_empty() {
                return false;
            }
            let Some(programs) = self.required_extensions.get(extension) else {
                return false;
            };
            if !programs.iter().all(|(_, program)| program.is_match(path)) {
                return false;
            }
        }
        let basename = candidate.basename_for_program();
        self.basename_programs
            .iter()
            .all(|(_, program)| program.is_match(basename))
            && self
                .programs
                .iter()
                .all(|(_, program)| program.is_match(path))
    }

    pub(crate) fn visit_matches(&self, candidate: &Candidate<'_>, visit: &mut dyn FnMut(usize)) {
        if self.is_empty() {
            return;
        }
        let path = &candidate.path[..];
        let basename = candidate.basename();
        let extension = candidate.extension();
        let mut hits = |map: &HashMap<Vec<u8>, Vec<usize>>, key: &[u8]| {
            if let Some(indexes) = map.get(key) {
                indexes.iter().for_each(|&index| visit(index));
            }
        };
        if !extension.is_empty() {
            hits(&self.extensions, extension);
        }
        if !basename.is_empty() {
            hits(&self.basename_literals, basename);
        }
        hits(&self.literals, path);
        for (suffix, index) in &self.suffixes {
            if path.ends_with(suffix) {
                visit(*index);
            }
        }
        for (prefix, index) in &self.prefixes {
            if path.starts_with(prefix) {
                visit(*index);
            }
        }
        if !extension.is_empty()
            && let Some(programs) = self.required_extensions.get(extension)
        {
            for (index, program) in programs {
                if program.is_match(path) {
                    visit(*index);
                }
            }
        }
        let basename = candidate.basename_for_program();
        for (index, program) in &self.basename_programs {
            if program.is_match(basename) {
                visit(*index);
            }
        }
        for (index, program) in &self.programs {
            if program.is_match(path) {
                visit(*index);
            }
        }
    }
}
