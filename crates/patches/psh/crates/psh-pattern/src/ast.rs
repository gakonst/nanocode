use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::error::{Error, ErrorKind};

pub const NEST_LIMIT_DEFAULT: u32 = 250;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Position {
    pub offset: usize,
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Span {
    pub start: Position,
    pub end: Position,
}

impl Span {
    pub fn new(start: Position, end: Position) -> Span {
        Span { start, end }
    }

    pub fn splat(position: Position) -> Span {
        Span::new(position, position)
    }

    pub fn is_one_line(&self) -> bool {
        self.start.line == self.end.line
    }

    pub(crate) fn key(&self) -> (usize, usize) {
        (self.start.offset, self.end.offset)
    }
}

pub type NodeId = usize;
pub type SetId = usize;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiteralKind {
    Verbatim,
    Meta,
    Superfluous,
    Octal,
    HexX,
    HexOther,
    Special,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Literal {
    pub span: Span,
    pub kind: LiteralKind,
    pub c: char,
}

impl Literal {
    pub fn byte(&self) -> Option<u8> {
        match self.kind {
            LiteralKind::HexX => u8::try_from(u32::from(self.c)).ok(),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssertionKind {
    StartLine,
    EndLine,
    StartText,
    EndText,
    WordBoundary,
    NotWordBoundary,
    WordBoundaryStart,
    WordBoundaryEnd,
    WordBoundaryStartAngle,
    WordBoundaryEndAngle,
    WordBoundaryStartHalf,
    WordBoundaryEndHalf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PerlKind {
    Digit,
    Space,
    Word,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClassPerl {
    pub span: Span,
    pub kind: PerlKind,
    pub negated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UnicodeKind {
    OneLetter(char),
    Named(String),
    NamedValue {
        name: String,
        value: String,
        not_equal: bool,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClassUnicode {
    pub span: Span,
    pub negated: bool,
    pub kind: UnicodeKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AsciiKind {
    Alnum,
    Alpha,
    Ascii,
    Blank,
    Cntrl,
    Digit,
    Graph,
    Lower,
    Print,
    Punct,
    Space,
    Upper,
    Word,
    Xdigit,
}

impl AsciiKind {
    fn from_name(name: &str) -> Option<AsciiKind> {
        Some(match name {
            "alnum" => AsciiKind::Alnum,
            "alpha" => AsciiKind::Alpha,
            "ascii" => AsciiKind::Ascii,
            "blank" => AsciiKind::Blank,
            "cntrl" => AsciiKind::Cntrl,
            "digit" => AsciiKind::Digit,
            "graph" => AsciiKind::Graph,
            "lower" => AsciiKind::Lower,
            "print" => AsciiKind::Print,
            "punct" => AsciiKind::Punct,
            "space" => AsciiKind::Space,
            "upper" => AsciiKind::Upper,
            "word" => AsciiKind::Word,
            "xdigit" => AsciiKind::Xdigit,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Flag {
    CaseInsensitive,
    MultiLine,
    DotMatchesNewLine,
    SwapGreed,
    Unicode,
    Crlf,
    IgnoreWhitespace,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FlagsItemKind {
    Negation,
    Flag(Flag),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FlagsItem {
    pub span: Span,
    pub kind: FlagsItemKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Flags {
    pub span: Span,
    pub items: Vec<FlagsItem>,
}

impl Flags {
    fn add_item(&mut self, item: FlagsItem) -> Option<usize> {
        if let Some(index) = self.items.iter().position(|x| x.kind == item.kind) {
            return Some(index);
        }
        self.items.push(item);
        None
    }

    pub fn flag_state(&self, flag: Flag) -> Option<bool> {
        let mut negated = false;
        for item in &self.items {
            match item.kind {
                FlagsItemKind::Negation => negated = true,
                FlagsItemKind::Flag(x) if x == flag => return Some(!negated),
                FlagsItemKind::Flag(_) => {}
            }
        }
        None
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepetitionKind {
    ZeroOrOne,
    ZeroOrMore,
    OneOrMore,
    Exactly(u32),
    AtLeast(u32),
    Bounded(u32, u32),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GroupKind {
    CaptureIndex(u32),
    CaptureName { name: String, index: u32 },
    NonCapturing(Flags),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Node {
    Empty(Span),
    Flags {
        span: Span,
        flags: Flags,
    },
    Literal(Literal),
    Dot(Span),
    Assertion {
        span: Span,
        kind: AssertionKind,
    },
    ClassUnicode(ClassUnicode),
    ClassPerl(ClassPerl),
    ClassBracketed {
        span: Span,
        negated: bool,
        set: SetId,
    },
    Repetition {
        span: Span,
        kind: RepetitionKind,
        greedy: bool,
        ast: NodeId,
    },
    Group {
        span: Span,
        kind: GroupKind,
        ast: NodeId,
    },
    Alternation {
        span: Span,
        asts: Vec<NodeId>,
    },
    Concat {
        span: Span,
        asts: Vec<NodeId>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryOpKind {
    Intersection,
    Difference,
    SymmetricDifference,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SetNode {
    Empty(Span),
    Literal(Literal),
    Range {
        span: Span,
        start: Literal,
        end: Literal,
    },
    Ascii {
        span: Span,
        kind: AsciiKind,
        negated: bool,
    },
    Unicode(ClassUnicode),
    Perl(ClassPerl),
    Bracketed {
        span: Span,
        negated: bool,
        set: SetId,
    },
    Union {
        span: Span,
        items: Vec<SetId>,
    },
    BinaryOp {
        span: Span,
        kind: BinaryOpKind,
        lhs: SetId,
        rhs: SetId,
    },
}

#[derive(Clone, Debug)]
pub struct Ast {
    pub nodes: Vec<Node>,
    pub sets: Vec<SetNode>,
    pub root: NodeId,
    pub capture_names: Vec<(String, u32)>,
}

impl Ast {
    pub fn node_span(&self, id: NodeId) -> Span {
        node_span(&self.nodes[id])
    }

    pub fn set_span(&self, id: SetId) -> Span {
        set_span(&self.sets[id])
    }

    pub fn literal_chars(&self) -> Vec<char> {
        let mut out = Vec::new();
        for node in &self.nodes {
            if let Node::Literal(literal) = node {
                out.push(literal.c);
            }
        }
        for set in &self.sets {
            match set {
                SetNode::Literal(literal) => out.push(literal.c),
                SetNode::Range { start, end, .. } => {
                    out.push(start.c);
                    out.push(end.c);
                }
                _ => {}
            }
        }
        out
    }
}

fn node_span(node: &Node) -> Span {
    match node {
        Node::Empty(span) | Node::Dot(span) => *span,
        Node::Flags { span, .. }
        | Node::Assertion { span, .. }
        | Node::ClassBracketed { span, .. }
        | Node::Repetition { span, .. }
        | Node::Group { span, .. }
        | Node::Alternation { span, .. }
        | Node::Concat { span, .. } => *span,
        Node::Literal(x) => x.span,
        Node::ClassUnicode(x) => x.span,
        Node::ClassPerl(x) => x.span,
    }
}

fn set_span(set: &SetNode) -> Span {
    match set {
        SetNode::Empty(span) => *span,
        SetNode::Literal(x) => x.span,
        SetNode::Unicode(x) => x.span,
        SetNode::Perl(x) => x.span,
        SetNode::Range { span, .. }
        | SetNode::Ascii { span, .. }
        | SetNode::Bracketed { span, .. }
        | SetNode::Union { span, .. }
        | SetNode::BinaryOp { span, .. } => *span,
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ParseOptions {
    pub ignore_whitespace: bool,
    pub octal: bool,
    pub nest_limit: u32,
}

impl Default for ParseOptions {
    fn default() -> ParseOptions {
        ParseOptions {
            ignore_whitespace: false,
            octal: false,
            nest_limit: NEST_LIMIT_DEFAULT,
        }
    }
}

enum Primitive {
    Literal(Literal),
    Assertion(Span, AssertionKind),
    Dot(Span),
    Perl(ClassPerl),
    Unicode(ClassUnicode),
}

impl Primitive {
    fn span(&self) -> Span {
        match self {
            Primitive::Literal(x) => x.span,
            Primitive::Assertion(span, _) | Primitive::Dot(span) => *span,
            Primitive::Perl(x) => x.span,
            Primitive::Unicode(x) => x.span,
        }
    }

    fn into_node(self) -> Node {
        match self {
            Primitive::Literal(x) => Node::Literal(x),
            Primitive::Assertion(span, kind) => Node::Assertion { span, kind },
            Primitive::Dot(span) => Node::Dot(span),
            Primitive::Perl(x) => Node::ClassPerl(x),
            Primitive::Unicode(x) => Node::ClassUnicode(x),
        }
    }
}

struct Concat {
    span: Span,
    asts: Vec<NodeId>,
}

struct Alternation {
    span: Span,
    asts: Vec<NodeId>,
}

struct GroupHeader {
    span: Span,
    kind: GroupKind,
}

enum GroupState {
    Group {
        concat: Concat,
        group: GroupHeader,
        ignore_whitespace: bool,
    },
    Alternation(Alternation),
}

struct Union {
    span: Span,
    items: Vec<SetId>,
}

struct Bracketed {
    span: Span,
    negated: bool,
}

enum ClassState {
    Open { union: Union, set: Bracketed },
    Op { kind: BinaryOpKind, lhs: SetId },
}

enum GroupOrFlags {
    Flags(Span, Flags),
    Group(GroupHeader),
}

pub fn parse(pattern: &str, options: ParseOptions) -> Result<Ast, Error> {
    let mut parser = Parser {
        pattern,
        position: Position {
            offset: 0,
            line: 1,
            column: 1,
        },
        capture_index: 0,
        options,
        ignore_whitespace: options.ignore_whitespace,
        stack_group: Vec::new(),
        stack_class: Vec::new(),
        capture_names: Vec::new(),
        nodes: Vec::new(),
        sets: Vec::new(),
    };
    let root = parser.parse()?;
    let capture_names = parser
        .capture_names
        .into_iter()
        .map(|(name, index, _)| (name, index))
        .collect();
    let ast = Ast {
        nodes: parser.nodes,
        sets: parser.sets,
        root,
        capture_names,
    };
    check_nest(&ast, pattern, options.nest_limit)?;
    Ok(ast)
}

fn is_hex(c: char) -> bool {
    c.is_ascii_hexdigit()
}

fn is_capture_char(c: char, first: bool) -> bool {
    if first {
        c == '_' || c.is_alphabetic()
    } else {
        c == '_' || c == '.' || c == '[' || c == ']' || c.is_alphanumeric()
    }
}

pub fn is_meta_character(c: char) -> bool {
    matches!(
        c,
        '\\' | '.'
            | '+'
            | '*'
            | '?'
            | '('
            | ')'
            | '|'
            | '['
            | ']'
            | '{'
            | '}'
            | '^'
            | '$'
            | '#'
            | '&'
            | '-'
            | '~'
    )
}

pub fn is_escapeable_character(c: char) -> bool {
    if is_meta_character(c) {
        return true;
    }
    if !c.is_ascii() {
        return false;
    }
    !matches!(c, '0'..='9' | 'A'..='Z' | 'a'..='z' | '<' | '>')
}

struct Parser<'p> {
    pattern: &'p str,
    position: Position,
    capture_index: u32,
    options: ParseOptions,
    ignore_whitespace: bool,
    stack_group: Vec<GroupState>,
    stack_class: Vec<ClassState>,
    capture_names: Vec<(String, u32, Span)>,
    nodes: Vec<Node>,
    sets: Vec<SetNode>,
}

impl<'p> Parser<'p> {
    fn error(&self, span: Span, kind: ErrorKind) -> Error {
        Error::new(kind, self.pattern, span)
    }

    fn push_node(&mut self, node: Node) -> NodeId {
        self.nodes.push(node);
        self.nodes.len() - 1
    }

    fn push_set(&mut self, set: SetNode) -> SetId {
        self.sets.push(set);
        self.sets.len() - 1
    }

    fn offset(&self) -> usize {
        self.position.offset
    }

    fn is_eof(&self) -> bool {
        self.offset() == self.pattern.len()
    }

    fn char(&self) -> char {
        self.char_at(self.offset())
    }

    fn char_at(&self, offset: usize) -> char {
        self.pattern[offset..]
            .chars()
            .next()
            .expect("a character at the parser's offset")
    }

    fn bump(&mut self) -> bool {
        if self.is_eof() {
            return false;
        }
        let c = self.char();
        let Position {
            mut offset,
            mut line,
            mut column,
        } = self.position;
        if c == '\n' {
            line = line.checked_add(1).expect("line count fits");
            column = 1;
        } else {
            column = column.checked_add(1).expect("column count fits");
        }
        offset += c.len_utf8();
        self.position = Position {
            offset,
            line,
            column,
        };
        !self.is_eof()
    }

    fn bump_if(&mut self, prefix: &str) -> bool {
        if self.pattern[self.offset()..].starts_with(prefix) {
            for _ in 0..prefix.chars().count() {
                self.bump();
            }
            true
        } else {
            false
        }
    }

    fn is_lookaround_prefix(&mut self) -> bool {
        self.bump_if("?=") || self.bump_if("?!") || self.bump_if("?<=") || self.bump_if("?<!")
    }

    fn bump_and_bump_space(&mut self) -> bool {
        if !self.bump() {
            return false;
        }
        self.bump_space();
        !self.is_eof()
    }

    fn bump_space(&mut self) {
        if !self.ignore_whitespace {
            return;
        }
        while !self.is_eof() {
            if self.char().is_whitespace() {
                self.bump();
            } else if self.char() == '#' {
                self.bump();
                while !self.is_eof() {
                    let c = self.char();
                    self.bump();
                    if c == '\n' {
                        break;
                    }
                }
            } else {
                break;
            }
        }
    }

    fn peek(&self) -> Option<char> {
        if self.is_eof() {
            return None;
        }
        self.pattern[self.offset() + self.char().len_utf8()..]
            .chars()
            .next()
    }

    fn peek_space(&self) -> Option<char> {
        if !self.ignore_whitespace {
            return self.peek();
        }
        if self.is_eof() {
            return None;
        }
        let mut start = self.offset() + self.char().len_utf8();
        let mut in_comment = false;
        for (index, c) in self.pattern[start..].char_indices() {
            if c.is_whitespace() {
                continue;
            } else if !in_comment && c == '#' {
                in_comment = true;
            } else if in_comment && c == '\n' {
                in_comment = false;
            } else {
                start += index;
                break;
            }
        }
        self.pattern[start..].chars().next()
    }

    fn span(&self) -> Span {
        Span::splat(self.position)
    }

    fn span_char(&self) -> Span {
        let c = self.char();
        let mut next = Position {
            offset: self
                .offset()
                .checked_add(c.len_utf8())
                .expect("offset fits"),
            line: self.position.line,
            column: self.position.column.checked_add(1).expect("column fits"),
        };
        if c == '\n' {
            next.line += 1;
            next.column = 1;
        }
        Span::new(self.position, next)
    }

    fn concat_into_node(&mut self, mut concat: Concat) -> NodeId {
        match concat.asts.len() {
            0 => self.push_node(Node::Empty(concat.span)),
            1 => concat.asts.pop().expect("one element"),
            _ => self.push_node(Node::Concat {
                span: concat.span,
                asts: concat.asts,
            }),
        }
    }

    fn alternation_into_node(&mut self, mut alternation: Alternation) -> NodeId {
        match alternation.asts.len() {
            0 => self.push_node(Node::Empty(alternation.span)),
            1 => alternation.asts.pop().expect("one element"),
            _ => self.push_node(Node::Alternation {
                span: alternation.span,
                asts: alternation.asts,
            }),
        }
    }

    fn union_push(&self, union: &mut Union, item: SetId) {
        let span = set_span(&self.sets[item]);
        if union.items.is_empty() {
            union.span.start = span.start;
        }
        union.span.end = span.end;
        union.items.push(item);
    }

    fn union_into_item(&mut self, mut union: Union) -> SetId {
        match union.items.len() {
            0 => self.push_set(SetNode::Empty(union.span)),
            1 => union.items.pop().expect("one element"),
            _ => self.push_set(SetNode::Union {
                span: union.span,
                items: union.items,
            }),
        }
    }

    fn push_alternate(&mut self, mut concat: Concat) -> Concat {
        assert_eq!(self.char(), '|');
        concat.span.end = self.position;
        self.push_or_add_alternation(concat);
        self.bump();
        Concat {
            span: self.span(),
            asts: Vec::new(),
        }
    }

    fn push_or_add_alternation(&mut self, concat: Concat) {
        let start = concat.span.start;
        let node = self.concat_into_node(concat);
        if let Some(GroupState::Alternation(alternation)) = self.stack_group.last_mut() {
            alternation.asts.push(node);
            return;
        }
        let span = Span::new(start, self.position);
        self.stack_group.push(GroupState::Alternation(Alternation {
            span,
            asts: vec![node],
        }));
    }

    fn push_group(&mut self, mut concat: Concat) -> Result<Concat, Error> {
        assert_eq!(self.char(), '(');
        match self.parse_group()? {
            GroupOrFlags::Flags(span, flags) => {
                if let Some(value) = flags.flag_state(Flag::IgnoreWhitespace) {
                    self.ignore_whitespace = value;
                }
                let node = self.push_node(Node::Flags { span, flags });
                concat.asts.push(node);
                Ok(concat)
            }
            GroupOrFlags::Group(group) => {
                let old = self.ignore_whitespace;
                let new = match &group.kind {
                    GroupKind::NonCapturing(flags) => {
                        flags.flag_state(Flag::IgnoreWhitespace).unwrap_or(old)
                    }
                    _ => old,
                };
                self.stack_group.push(GroupState::Group {
                    concat,
                    group,
                    ignore_whitespace: old,
                });
                self.ignore_whitespace = new;
                Ok(Concat {
                    span: self.span(),
                    asts: Vec::new(),
                })
            }
        }
    }

    fn pop_group(&mut self, mut group_concat: Concat) -> Result<Concat, Error> {
        assert_eq!(self.char(), ')');
        let (mut prior_concat, mut group, ignore_whitespace, alternation) =
            match self.stack_group.pop() {
                Some(GroupState::Group {
                    concat,
                    group,
                    ignore_whitespace,
                }) => (concat, group, ignore_whitespace, None),
                Some(GroupState::Alternation(alternation)) => match self.stack_group.pop() {
                    Some(GroupState::Group {
                        concat,
                        group,
                        ignore_whitespace,
                    }) => (concat, group, ignore_whitespace, Some(alternation)),
                    None | Some(GroupState::Alternation(_)) => {
                        return Err(self.error(self.span_char(), ErrorKind::GroupUnopened));
                    }
                },
                None => return Err(self.error(self.span_char(), ErrorKind::GroupUnopened)),
            };
        self.ignore_whitespace = ignore_whitespace;
        group_concat.span.end = self.position;
        self.bump();
        group.span.end = self.position;
        let concat_end = group_concat.span.end;
        let sub = match alternation {
            Some(mut alternation) => {
                alternation.span.end = concat_end;
                let node = self.concat_into_node(group_concat);
                alternation.asts.push(node);
                self.alternation_into_node(alternation)
            }
            None => self.concat_into_node(group_concat),
        };
        let node = self.push_node(Node::Group {
            span: group.span,
            kind: group.kind,
            ast: sub,
        });
        prior_concat.asts.push(node);
        Ok(prior_concat)
    }

    fn pop_group_end(&mut self, mut concat: Concat) -> Result<NodeId, Error> {
        concat.span.end = self.position;
        let node = match self.stack_group.pop() {
            None => self.concat_into_node(concat),
            Some(GroupState::Alternation(mut alternation)) => {
                alternation.span.end = self.position;
                let node = self.concat_into_node(concat);
                alternation.asts.push(node);
                self.push_node(Node::Alternation {
                    span: alternation.span,
                    asts: alternation.asts,
                })
            }
            Some(GroupState::Group { group, .. }) => {
                return Err(self.error(group.span, ErrorKind::GroupUnclosed));
            }
        };
        match self.stack_group.pop() {
            None => Ok(node),
            Some(GroupState::Alternation(_)) => {
                unreachable!("two alternations in a row on the group stack")
            }
            Some(GroupState::Group { group, .. }) => {
                Err(self.error(group.span, ErrorKind::GroupUnclosed))
            }
        }
    }

    fn push_class_open(&mut self, parent: Union) -> Result<Union, Error> {
        assert_eq!(self.char(), '[');
        let (set, union) = self.parse_set_class_open()?;
        self.stack_class
            .push(ClassState::Open { union: parent, set });
        Ok(union)
    }

    fn pop_class(&mut self, nested: Union) -> Result<Result<Union, SetId>, Error> {
        assert_eq!(self.char(), ']');
        let item = self.union_into_item(nested);
        let previous = self.pop_class_op(item);
        match self.stack_class.pop() {
            None => panic!("unexpected empty character class stack"),
            Some(ClassState::Op { .. }) => panic!("unexpected class operation on the stack"),
            Some(ClassState::Open { mut union, mut set }) => {
                self.bump();
                set.span.end = self.position;
                let bracketed = self.push_set(SetNode::Bracketed {
                    span: set.span,
                    negated: set.negated,
                    set: previous,
                });
                if self.stack_class.is_empty() {
                    Ok(Err(bracketed))
                } else {
                    self.union_push(&mut union, bracketed);
                    Ok(Ok(union))
                }
            }
        }
    }

    fn unclosed_class_error(&self) -> Error {
        for state in self.stack_class.iter().rev() {
            if let ClassState::Open { set, .. } = state {
                return self.error(set.span, ErrorKind::ClassUnclosed);
            }
        }
        panic!("no open character class found")
    }

    fn push_class_op(&mut self, kind: BinaryOpKind, union: Union) -> Union {
        let item = self.union_into_item(union);
        let lhs = self.pop_class_op(item);
        self.stack_class.push(ClassState::Op { kind, lhs });
        Union {
            span: self.span(),
            items: Vec::new(),
        }
    }

    fn pop_class_op(&mut self, rhs: SetId) -> SetId {
        let (kind, lhs) = match self.stack_class.pop() {
            Some(ClassState::Op { kind, lhs }) => (kind, lhs),
            Some(state @ ClassState::Open { .. }) => {
                self.stack_class.push(state);
                return rhs;
            }
            None => unreachable!("a class operation without an open class"),
        };
        let span = Span::new(
            set_span(&self.sets[lhs]).start,
            set_span(&self.sets[rhs]).end,
        );
        self.push_set(SetNode::BinaryOp {
            span,
            kind,
            lhs,
            rhs,
        })
    }

    fn parse(&mut self) -> Result<NodeId, Error> {
        let mut concat = Concat {
            span: self.span(),
            asts: Vec::new(),
        };
        loop {
            self.bump_space();
            if self.is_eof() {
                break;
            }
            match self.char() {
                '(' => concat = self.push_group(concat)?,
                ')' => concat = self.pop_group(concat)?,
                '|' => concat = self.push_alternate(concat),
                '[' => {
                    let class = self.parse_set_class()?;
                    concat.asts.push(class);
                }
                '?' => {
                    concat = self.parse_uncounted_repetition(concat, RepetitionKind::ZeroOrOne)?
                }
                '*' => {
                    concat = self.parse_uncounted_repetition(concat, RepetitionKind::ZeroOrMore)?
                }
                '+' => {
                    concat = self.parse_uncounted_repetition(concat, RepetitionKind::OneOrMore)?
                }
                '{' => concat = self.parse_counted_repetition(concat)?,
                _ => {
                    let primitive = self.parse_primitive()?;
                    let node = self.push_node(primitive.into_node());
                    concat.asts.push(node);
                }
            }
        }
        self.pop_group_end(concat)
    }

    fn pop_repeatable(&mut self, concat: &mut Concat) -> Result<NodeId, Error> {
        let node = match concat.asts.pop() {
            Some(node) => node,
            None => return Err(self.error(self.span(), ErrorKind::RepetitionMissing)),
        };
        if matches!(self.nodes[node], Node::Empty(_) | Node::Flags { .. }) {
            return Err(self.error(self.span(), ErrorKind::RepetitionMissing));
        }
        Ok(node)
    }

    fn parse_uncounted_repetition(
        &mut self,
        mut concat: Concat,
        kind: RepetitionKind,
    ) -> Result<Concat, Error> {
        let node = self.pop_repeatable(&mut concat)?;
        let mut greedy = true;
        if self.bump() && self.char() == '?' {
            greedy = false;
            self.bump();
        }
        let span = Span::new(node_span(&self.nodes[node]).start, self.position);
        let repetition = self.push_node(Node::Repetition {
            span,
            kind,
            greedy,
            ast: node,
        });
        concat.asts.push(repetition);
        Ok(concat)
    }

    fn parse_counted_repetition(&mut self, mut concat: Concat) -> Result<Concat, Error> {
        assert_eq!(self.char(), '{');
        let start = self.position;
        let node = self.pop_repeatable(&mut concat)?;
        if !self.bump_and_bump_space() {
            return Err(self.error(
                Span::new(start, self.position),
                ErrorKind::RepetitionCountUnclosed,
            ));
        }
        let count_start = specialize(self.parse_decimal());
        if self.is_eof() {
            return Err(self.error(
                Span::new(start, self.position),
                ErrorKind::RepetitionCountUnclosed,
            ));
        }
        let kind = if self.char() == ',' {
            if !self.bump_and_bump_space() {
                return Err(self.error(
                    Span::new(start, self.position),
                    ErrorKind::RepetitionCountUnclosed,
                ));
            }
            if self.char() != '}' {
                let count_start = count_start?;
                let count_end = specialize(self.parse_decimal())?;
                RepetitionKind::Bounded(count_start, count_end)
            } else {
                RepetitionKind::AtLeast(count_start?)
            }
        } else {
            RepetitionKind::Exactly(count_start?)
        };
        if self.is_eof() || self.char() != '}' {
            return Err(self.error(
                Span::new(start, self.position),
                ErrorKind::RepetitionCountUnclosed,
            ));
        }
        let mut greedy = true;
        if self.bump_and_bump_space() && self.char() == '?' {
            greedy = false;
            self.bump();
        }
        let op_span = Span::new(start, self.position);
        if let RepetitionKind::Bounded(low, high) = kind
            && low > high
        {
            return Err(self.error(op_span, ErrorKind::RepetitionCountInvalid));
        }
        let span = Span::new(node_span(&self.nodes[node]).start, self.position);
        let repetition = self.push_node(Node::Repetition {
            span,
            kind,
            greedy,
            ast: node,
        });
        concat.asts.push(repetition);
        Ok(concat)
    }

    fn next_capture_index(&mut self, span: Span) -> Result<u32, Error> {
        let index = self
            .capture_index
            .checked_add(1)
            .ok_or_else(|| self.error(span, ErrorKind::CaptureLimitExceeded))?;
        self.capture_index = index;
        Ok(index)
    }

    fn parse_group(&mut self) -> Result<GroupOrFlags, Error> {
        assert_eq!(self.char(), '(');
        let open_span = self.span_char();
        self.bump();
        self.bump_space();
        if self.is_lookaround_prefix() {
            return Err(self.error(
                Span::new(open_span.start, self.span().end),
                ErrorKind::UnsupportedLookAround,
            ));
        }
        let inner_span = self.span();
        if self.bump_if("?P<") || self.bump_if("?<") {
            let index = self.next_capture_index(open_span)?;
            let name = self.parse_capture_name(index)?;
            return Ok(GroupOrFlags::Group(GroupHeader {
                span: open_span,
                kind: GroupKind::CaptureName { name, index },
            }));
        }
        if self.bump_if("?") {
            if self.is_eof() {
                return Err(self.error(open_span, ErrorKind::GroupUnclosed));
            }
            let flags = self.parse_flags()?;
            let char_end = self.char();
            self.bump();
            if char_end == ')' {
                if flags.items.is_empty() {
                    return Err(self.error(inner_span, ErrorKind::RepetitionMissing));
                }
                return Ok(GroupOrFlags::Flags(
                    Span::new(open_span.start, self.position),
                    flags,
                ));
            }
            assert_eq!(char_end, ':');
            return Ok(GroupOrFlags::Group(GroupHeader {
                span: open_span,
                kind: GroupKind::NonCapturing(flags),
            }));
        }
        let index = self.next_capture_index(open_span)?;
        Ok(GroupOrFlags::Group(GroupHeader {
            span: open_span,
            kind: GroupKind::CaptureIndex(index),
        }))
    }

    fn parse_capture_name(&mut self, index: u32) -> Result<String, Error> {
        if self.is_eof() {
            return Err(self.error(self.span(), ErrorKind::GroupNameUnexpectedEof));
        }
        let start = self.position;
        loop {
            if self.char() == '>' {
                break;
            }
            if !is_capture_char(self.char(), self.position == start) {
                return Err(self.error(self.span_char(), ErrorKind::GroupNameInvalid));
            }
            if !self.bump() {
                break;
            }
        }
        let end = self.position;
        if self.is_eof() {
            return Err(self.error(self.span(), ErrorKind::GroupNameUnexpectedEof));
        }
        assert_eq!(self.char(), '>');
        self.bump();
        let name = &self.pattern[start.offset..end.offset];
        if name.is_empty() {
            return Err(self.error(Span::new(start, start), ErrorKind::GroupNameEmpty));
        }
        let span = Span::new(start, end);
        match self
            .capture_names
            .binary_search_by(|(x, _, _)| x.as_str().cmp(name))
        {
            Err(position) => self
                .capture_names
                .insert(position, (name.to_string(), index, span)),
            Ok(position) => {
                let original = self.capture_names[position].2;
                return Err(self.error(span, ErrorKind::GroupNameDuplicate { original }));
            }
        }
        Ok(name.to_string())
    }

    fn parse_flags(&mut self) -> Result<Flags, Error> {
        let mut flags = Flags {
            span: self.span(),
            items: Vec::new(),
        };
        let mut last_was_negation = None;
        while self.char() != ':' && self.char() != ')' {
            if self.char() == '-' {
                last_was_negation = Some(self.span_char());
                let item = FlagsItem {
                    span: self.span_char(),
                    kind: FlagsItemKind::Negation,
                };
                if let Some(index) = flags.add_item(item) {
                    let original = flags.items[index].span;
                    return Err(self.error(
                        self.span_char(),
                        ErrorKind::FlagRepeatedNegation { original },
                    ));
                }
            } else {
                last_was_negation = None;
                let item = FlagsItem {
                    span: self.span_char(),
                    kind: FlagsItemKind::Flag(self.parse_flag()?),
                };
                if let Some(index) = flags.add_item(item) {
                    let original = flags.items[index].span;
                    return Err(self.error(self.span_char(), ErrorKind::FlagDuplicate { original }));
                }
            }
            if !self.bump() {
                return Err(self.error(self.span(), ErrorKind::FlagUnexpectedEof));
            }
        }
        if let Some(span) = last_was_negation {
            return Err(self.error(span, ErrorKind::FlagDanglingNegation));
        }
        flags.span.end = self.position;
        Ok(flags)
    }

    fn parse_flag(&self) -> Result<Flag, Error> {
        match self.char() {
            'i' => Ok(Flag::CaseInsensitive),
            'm' => Ok(Flag::MultiLine),
            's' => Ok(Flag::DotMatchesNewLine),
            'U' => Ok(Flag::SwapGreed),
            'u' => Ok(Flag::Unicode),
            'R' => Ok(Flag::Crlf),
            'x' => Ok(Flag::IgnoreWhitespace),
            _ => Err(self.error(self.span_char(), ErrorKind::FlagUnrecognized)),
        }
    }

    fn parse_primitive(&mut self) -> Result<Primitive, Error> {
        match self.char() {
            '\\' => self.parse_escape(),
            '.' => {
                let primitive = Primitive::Dot(self.span_char());
                self.bump();
                Ok(primitive)
            }
            '^' => {
                let primitive = Primitive::Assertion(self.span_char(), AssertionKind::StartLine);
                self.bump();
                Ok(primitive)
            }
            '$' => {
                let primitive = Primitive::Assertion(self.span_char(), AssertionKind::EndLine);
                self.bump();
                Ok(primitive)
            }
            c => {
                let primitive = Primitive::Literal(Literal {
                    span: self.span_char(),
                    kind: LiteralKind::Verbatim,
                    c,
                });
                self.bump();
                Ok(primitive)
            }
        }
    }

    fn parse_escape(&mut self) -> Result<Primitive, Error> {
        assert_eq!(self.char(), '\\');
        let start = self.position;
        if !self.bump() {
            return Err(self.error(
                Span::new(start, self.position),
                ErrorKind::EscapeUnexpectedEof,
            ));
        }
        let c = self.char();
        match c {
            '0'..='7' => {
                if !self.options.octal {
                    return Err(self.error(
                        Span::new(start, self.span_char().end),
                        ErrorKind::UnsupportedBackreference,
                    ));
                }
                let mut literal = self.parse_octal();
                literal.span.start = start;
                return Ok(Primitive::Literal(literal));
            }
            '8'..='9' if !self.options.octal => {
                return Err(self.error(
                    Span::new(start, self.span_char().end),
                    ErrorKind::UnsupportedBackreference,
                ));
            }
            'x' | 'u' | 'U' => {
                let mut literal = self.parse_hex()?;
                literal.span.start = start;
                return Ok(Primitive::Literal(literal));
            }
            'p' | 'P' => {
                let mut class = self.parse_unicode_class()?;
                class.span.start = start;
                return Ok(Primitive::Unicode(class));
            }
            'd' | 's' | 'w' | 'D' | 'S' | 'W' => {
                let mut class = self.parse_perl_class();
                class.span.start = start;
                return Ok(Primitive::Perl(class));
            }
            _ => {}
        }
        self.bump();
        let span = Span::new(start, self.position);
        if is_meta_character(c) {
            return Ok(Primitive::Literal(Literal {
                span,
                kind: LiteralKind::Meta,
                c,
            }));
        }
        if is_escapeable_character(c) {
            return Ok(Primitive::Literal(Literal {
                span,
                kind: LiteralKind::Superfluous,
                c,
            }));
        }
        let special = |c| {
            Ok(Primitive::Literal(Literal {
                span,
                kind: LiteralKind::Special,
                c,
            }))
        };
        match c {
            'a' => special('\x07'),
            'f' => special('\x0C'),
            't' => special('\t'),
            'n' => special('\n'),
            'r' => special('\r'),
            'v' => special('\x0B'),
            'A' => Ok(Primitive::Assertion(span, AssertionKind::StartText)),
            'z' => Ok(Primitive::Assertion(span, AssertionKind::EndText)),
            'b' => {
                let mut span = span;
                let mut kind = AssertionKind::WordBoundary;
                if !self.is_eof()
                    && self.char() == '{'
                    && let Some(special) = self.maybe_parse_special_word_boundary(start)?
                {
                    kind = special;
                    span.end = self.position;
                }
                Ok(Primitive::Assertion(span, kind))
            }
            'B' => Ok(Primitive::Assertion(span, AssertionKind::NotWordBoundary)),
            '<' => Ok(Primitive::Assertion(
                span,
                AssertionKind::WordBoundaryStartAngle,
            )),
            '>' => Ok(Primitive::Assertion(
                span,
                AssertionKind::WordBoundaryEndAngle,
            )),
            _ => Err(self.error(span, ErrorKind::EscapeUnrecognized)),
        }
    }

    fn maybe_parse_special_word_boundary(
        &mut self,
        boundary_start: Position,
    ) -> Result<Option<AssertionKind>, Error> {
        assert_eq!(self.char(), '{');
        let is_valid_char = |c: char| c.is_ascii_alphabetic() || c == '-';
        let start = self.position;
        if !self.bump_and_bump_space() {
            return Err(self.error(
                Span::new(boundary_start, self.position),
                ErrorKind::SpecialWordOrRepetitionUnexpectedEof,
            ));
        }
        let start_contents = self.position;
        if !is_valid_char(self.char()) {
            self.position = start;
            return Ok(None);
        }
        let mut name = String::new();
        while !self.is_eof() && is_valid_char(self.char()) {
            name.push(self.char());
            self.bump_and_bump_space();
        }
        if self.is_eof() || self.char() != '}' {
            return Err(self.error(
                Span::new(start, self.position),
                ErrorKind::SpecialWordBoundaryUnclosed,
            ));
        }
        let end = self.position;
        self.bump();
        let kind = match name.as_str() {
            "start" => AssertionKind::WordBoundaryStart,
            "end" => AssertionKind::WordBoundaryEnd,
            "start-half" => AssertionKind::WordBoundaryStartHalf,
            "end-half" => AssertionKind::WordBoundaryEndHalf,
            _ => {
                return Err(self.error(
                    Span::new(start_contents, end),
                    ErrorKind::SpecialWordBoundaryUnrecognized,
                ));
            }
        };
        Ok(Some(kind))
    }

    fn parse_octal(&mut self) -> Literal {
        assert!(self.options.octal);
        let start = self.position;
        while self.bump()
            && ('0'..='7').contains(&self.char())
            && self.position.offset - start.offset <= 2
        {}
        let end = self.position;
        let digits = &self.pattern[start.offset..end.offset];
        let value = u32::from_str_radix(digits, 8).expect("valid octal number");
        let c = char::from_u32(value)
            .expect("an octal escape of at most three digits is a scalar value");
        Literal {
            span: Span::new(start, end),
            kind: LiteralKind::Octal,
            c,
        }
    }

    fn parse_hex(&mut self) -> Result<Literal, Error> {
        let digits = match self.char() {
            'x' => 2,
            'u' => 4,
            _ => 8,
        };
        let kind = if self.char() == 'x' {
            LiteralKind::HexX
        } else {
            LiteralKind::HexOther
        };
        if !self.bump_and_bump_space() {
            return Err(self.error(self.span(), ErrorKind::EscapeUnexpectedEof));
        }
        if self.char() == '{' {
            self.parse_hex_brace(LiteralKind::HexOther)
        } else {
            self.parse_hex_digits(kind, digits)
        }
    }

    fn parse_hex_digits(&mut self, kind: LiteralKind, digits: usize) -> Result<Literal, Error> {
        let mut hex = String::new();
        let start = self.position;
        for index in 0..digits {
            if index > 0 && !self.bump_and_bump_space() {
                return Err(self.error(self.span(), ErrorKind::EscapeUnexpectedEof));
            }
            if !is_hex(self.char()) {
                return Err(self.error(self.span_char(), ErrorKind::EscapeHexInvalidDigit));
            }
            hex.push(self.char());
        }
        self.bump_and_bump_space();
        let end = self.position;
        match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
            None => Err(self.error(Span::new(start, end), ErrorKind::EscapeHexInvalid)),
            Some(c) => Ok(Literal {
                span: Span::new(start, end),
                kind,
                c,
            }),
        }
    }

    fn parse_hex_brace(&mut self, kind: LiteralKind) -> Result<Literal, Error> {
        let mut hex = String::new();
        let brace = self.position;
        let start = self.span_char().end;
        while self.bump_and_bump_space() && self.char() != '}' {
            if !is_hex(self.char()) {
                return Err(self.error(self.span_char(), ErrorKind::EscapeHexInvalidDigit));
            }
            hex.push(self.char());
        }
        if self.is_eof() {
            return Err(self.error(
                Span::new(brace, self.position),
                ErrorKind::EscapeUnexpectedEof,
            ));
        }
        let end = self.position;
        assert_eq!(self.char(), '}');
        self.bump_and_bump_space();
        if hex.is_empty() {
            return Err(self.error(Span::new(brace, self.position), ErrorKind::EscapeHexEmpty));
        }
        match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
            None => Err(self.error(Span::new(start, end), ErrorKind::EscapeHexInvalid)),
            Some(c) => Ok(Literal {
                span: Span::new(start, self.position),
                kind,
                c,
            }),
        }
    }

    fn parse_decimal(&mut self) -> Result<u32, Error> {
        let mut digits = String::new();
        while !self.is_eof() && self.char().is_whitespace() {
            self.bump();
        }
        let start = self.position;
        while !self.is_eof() && self.char().is_ascii_digit() {
            digits.push(self.char());
            self.bump_and_bump_space();
        }
        let span = Span::new(start, self.position);
        while !self.is_eof() && self.char().is_whitespace() {
            self.bump_and_bump_space();
        }
        if digits.is_empty() {
            return Err(self.error(span, ErrorKind::DecimalEmpty));
        }
        digits
            .parse::<u32>()
            .map_err(|_| self.error(span, ErrorKind::DecimalInvalid))
    }

    fn parse_set_class(&mut self) -> Result<NodeId, Error> {
        assert_eq!(self.char(), '[');
        let mut union = Union {
            span: self.span(),
            items: Vec::new(),
        };
        loop {
            self.bump_space();
            if self.is_eof() {
                return Err(self.unclosed_class_error());
            }
            match self.char() {
                '[' => {
                    if !self.stack_class.is_empty()
                        && let Some(ascii) = self.maybe_parse_ascii_class()
                    {
                        let item = self.push_set(ascii);
                        self.union_push(&mut union, item);
                        continue;
                    }
                    union = self.push_class_open(union)?;
                }
                ']' => match self.pop_class(union)? {
                    Ok(nested) => union = nested,
                    Err(bracketed) => {
                        let (span, negated, set) = match &self.sets[bracketed] {
                            SetNode::Bracketed { span, negated, set } => (*span, *negated, *set),
                            _ => unreachable!("pop_class returns a bracketed class"),
                        };
                        return Ok(self.push_node(Node::ClassBracketed { span, negated, set }));
                    }
                },
                '&' if self.peek() == Some('&') => {
                    assert!(self.bump_if("&&"));
                    union = self.push_class_op(BinaryOpKind::Intersection, union);
                }
                '-' if self.peek() == Some('-') => {
                    assert!(self.bump_if("--"));
                    union = self.push_class_op(BinaryOpKind::Difference, union);
                }
                '~' if self.peek() == Some('~') => {
                    assert!(self.bump_if("~~"));
                    union = self.push_class_op(BinaryOpKind::SymmetricDifference, union);
                }
                _ => {
                    let item = self.parse_set_class_range()?;
                    self.union_push(&mut union, item);
                }
            }
        }
    }

    fn parse_set_class_range(&mut self) -> Result<SetId, Error> {
        let first = self.parse_set_class_item()?;
        self.bump_space();
        if self.is_eof() {
            return Err(self.unclosed_class_error());
        }
        if self.char() != '-' || self.peek_space() == Some(']') || self.peek_space() == Some('-') {
            let item = self.to_class_set_item(first)?;
            return Ok(self.push_set(item));
        }
        if !self.bump_and_bump_space() {
            return Err(self.unclosed_class_error());
        }
        let second = self.parse_set_class_item()?;
        let span = Span::new(first.span().start, second.span().end);
        let start = self.to_class_literal(first)?;
        let end = self.to_class_literal(second)?;
        if start.c > end.c {
            return Err(self.error(span, ErrorKind::ClassRangeInvalid));
        }
        Ok(self.push_set(SetNode::Range { span, start, end }))
    }

    fn to_class_set_item(&self, primitive: Primitive) -> Result<SetNode, Error> {
        match primitive {
            Primitive::Literal(x) => Ok(SetNode::Literal(x)),
            Primitive::Perl(x) => Ok(SetNode::Perl(x)),
            Primitive::Unicode(x) => Ok(SetNode::Unicode(x)),
            x => Err(self.error(x.span(), ErrorKind::ClassEscapeInvalid)),
        }
    }

    fn to_class_literal(&self, primitive: Primitive) -> Result<Literal, Error> {
        match primitive {
            Primitive::Literal(x) => Ok(x),
            x => Err(self.error(x.span(), ErrorKind::ClassRangeLiteral)),
        }
    }

    fn parse_set_class_item(&mut self) -> Result<Primitive, Error> {
        if self.char() == '\\' {
            return self.parse_escape();
        }
        let primitive = Primitive::Literal(Literal {
            span: self.span_char(),
            kind: LiteralKind::Verbatim,
            c: self.char(),
        });
        self.bump();
        Ok(primitive)
    }

    fn parse_set_class_open(&mut self) -> Result<(Bracketed, Union), Error> {
        assert_eq!(self.char(), '[');
        let start = self.position;
        if !self.bump_and_bump_space() {
            return Err(self.error(Span::new(start, self.position), ErrorKind::ClassUnclosed));
        }
        let negated = if self.char() != '^' {
            false
        } else {
            if !self.bump_and_bump_space() {
                return Err(self.error(Span::new(start, self.position), ErrorKind::ClassUnclosed));
            }
            true
        };
        let mut union = Union {
            span: self.span(),
            items: Vec::new(),
        };
        while self.char() == '-' {
            let item = self.push_set(SetNode::Literal(Literal {
                span: self.span_char(),
                kind: LiteralKind::Verbatim,
                c: '-',
            }));
            self.union_push(&mut union, item);
            if !self.bump_and_bump_space() {
                return Err(self.error(Span::new(start, start), ErrorKind::ClassUnclosed));
            }
        }
        if union.items.is_empty() && self.char() == ']' {
            let item = self.push_set(SetNode::Literal(Literal {
                span: self.span_char(),
                kind: LiteralKind::Verbatim,
                c: ']',
            }));
            self.union_push(&mut union, item);
            if !self.bump_and_bump_space() {
                return Err(self.error(Span::new(start, self.position), ErrorKind::ClassUnclosed));
            }
        }
        let set = Bracketed {
            span: Span::new(start, self.position),
            negated,
        };
        Ok((set, union))
    }

    fn maybe_parse_ascii_class(&mut self) -> Option<SetNode> {
        assert_eq!(self.char(), '[');
        let start = self.position;
        let mut negated = false;
        if !self.bump() || self.char() != ':' {
            self.position = start;
            return None;
        }
        if !self.bump() {
            self.position = start;
            return None;
        }
        if self.char() == '^' {
            negated = true;
            if !self.bump() {
                self.position = start;
                return None;
            }
        }
        let name_start = self.offset();
        while self.char() != ':' && self.bump() {}
        if self.is_eof() {
            self.position = start;
            return None;
        }
        let name_end = self.offset();
        if !self.bump_if(":]") {
            self.position = start;
            return None;
        }
        match AsciiKind::from_name(&self.pattern[name_start..name_end]) {
            Some(kind) => Some(SetNode::Ascii {
                span: Span::new(start, self.position),
                kind,
                negated,
            }),
            None => {
                self.position = start;
                None
            }
        }
    }

    fn parse_unicode_class(&mut self) -> Result<ClassUnicode, Error> {
        let negated = self.char() == 'P';
        if !self.bump_and_bump_space() {
            return Err(self.error(self.span(), ErrorKind::EscapeUnexpectedEof));
        }
        let (start, kind) = if self.char() == '{' {
            let start = self.span_char().end;
            let mut name = String::new();
            while self.bump_and_bump_space() && self.char() != '}' {
                name.push(self.char());
            }
            if self.is_eof() {
                return Err(self.error(self.span(), ErrorKind::EscapeUnexpectedEof));
            }
            assert_eq!(self.char(), '}');
            self.bump();
            let kind = if let Some(index) = name.find("!=") {
                UnicodeKind::NamedValue {
                    name: name[..index].to_string(),
                    value: name[index + 2..].to_string(),
                    not_equal: true,
                }
            } else if let Some(index) = name.find(':') {
                UnicodeKind::NamedValue {
                    name: name[..index].to_string(),
                    value: name[index + 1..].to_string(),
                    not_equal: false,
                }
            } else if let Some(index) = name.find('=') {
                UnicodeKind::NamedValue {
                    name: name[..index].to_string(),
                    value: name[index + 1..].to_string(),
                    not_equal: false,
                }
            } else {
                UnicodeKind::Named(name)
            };
            (start, kind)
        } else {
            let start = self.position;
            let c = self.char();
            if c == '\\' {
                return Err(self.error(self.span_char(), ErrorKind::UnicodeClassInvalid));
            }
            self.bump_and_bump_space();
            (start, UnicodeKind::OneLetter(c))
        };
        Ok(ClassUnicode {
            span: Span::new(start, self.position),
            negated,
            kind,
        })
    }

    fn parse_perl_class(&mut self) -> ClassPerl {
        let c = self.char();
        let span = self.span_char();
        self.bump();
        let (negated, kind) = match c {
            'd' => (false, PerlKind::Digit),
            'D' => (true, PerlKind::Digit),
            's' => (false, PerlKind::Space),
            'S' => (true, PerlKind::Space),
            'w' => (false, PerlKind::Word),
            'W' => (true, PerlKind::Word),
            c => panic!("expected a Perl class but got {c:?}"),
        };
        ClassPerl {
            span,
            kind,
            negated,
        }
    }
}

fn specialize(result: Result<u32, Error>) -> Result<u32, Error> {
    result.map_err(|error| {
        error.specialize(
            ErrorKind::DecimalEmpty,
            ErrorKind::RepetitionCountDecimalEmpty,
        )
    })
}

enum NestWork {
    Node(NodeId, u32),
    Set(SetId, u32),
}

fn check_nest(ast: &Ast, pattern: &str, limit: u32) -> Result<(), Error> {
    let mut stack = vec![NestWork::Node(ast.root, 0)];
    let exceeded = |span: Span| Error::new(ErrorKind::NestLimitExceeded(limit), pattern, span);
    while let Some(work) = stack.pop() {
        match work {
            NestWork::Node(id, depth) => {
                let node = &ast.nodes[id];
                let (span, children): (Span, &[NodeId]) = match node {
                    Node::ClassBracketed { span, set, .. } => {
                        let inner = depth + 1;
                        if inner > limit {
                            return Err(exceeded(*span));
                        }
                        stack.push(NestWork::Set(*set, inner));
                        continue;
                    }
                    Node::Repetition {
                        span, ast: child, ..
                    }
                    | Node::Group {
                        span, ast: child, ..
                    } => (*span, core::slice::from_ref(child)),
                    Node::Alternation { span, asts } | Node::Concat { span, asts } => {
                        (*span, asts.as_slice())
                    }
                    _ => continue,
                };
                let inner = depth + 1;
                if inner > limit {
                    return Err(exceeded(span));
                }
                for child in children.iter().rev() {
                    stack.push(NestWork::Node(*child, inner));
                }
            }
            NestWork::Set(id, depth) => {
                let set = &ast.sets[id];
                let span = set_span(set);
                match set {
                    SetNode::Bracketed { set: child, .. } => {
                        let inner = depth + 1;
                        if inner > limit {
                            return Err(exceeded(span));
                        }
                        stack.push(NestWork::Set(*child, inner));
                    }
                    SetNode::Union { items, .. } => {
                        let inner = depth + 1;
                        if inner > limit {
                            return Err(exceeded(span));
                        }
                        for item in items.iter().rev() {
                            stack.push(NestWork::Set(*item, inner));
                        }
                    }
                    SetNode::BinaryOp { lhs, rhs, .. } => {
                        let inner = depth + 1;
                        if inner > limit {
                            return Err(exceeded(span));
                        }
                        stack.push(NestWork::Set(*rhs, inner));
                        stack.push(NestWork::Set(*lhs, inner));
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

pub fn boxed_slice(bytes: &[u8]) -> Box<[u8]> {
    bytes.to_vec().into_boxed_slice()
}
