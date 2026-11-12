use super::{
    Anchor, CharType, Class, Emptiness, Error, LookBehindLength, Node, Repeat, class::CodePointSet,
    fold, unicode,
};

const PARSE_DEPTH_MAX: u32 = 4_096;
const HEXADECIMAL_RADIX: u32 = 16;
const OCTAL_RADIX: u32 = 8;
const DECIMAL_RADIX: u32 = 10;
const SINGLE_DIGIT_BACKREF_MAX: u32 = 9;
const POSIX_BRACKET_CLOSE: &str = ":]";
const REPEAT_COUNT_MAX: u32 = 100_000;
const BACKREF_NUMBER_MAX: u32 = 1_000;
const HEX_DIGITS_BRACED_MAX: usize = 8;
const HEX_DIGITS_BYTE_MAX: usize = 2;
const HEX_DIGITS_UNICODE: usize = 4;
const OCTAL_DIGITS_MAX: usize = 3;
const OCTAL_DIGITS_AFTER_ZERO_MAX: usize = 2;
const ASCII_LIMIT: u32 = 0x80;
const CONTROL_MASK: u32 = 0x9f;
const META_BIT: u32 = 0x80;
const DELETE: u32 = 0x7f;

#[derive(Clone, Copy)]
struct Options {
    ignore_case: bool,
    extend: bool,
    multiline: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RepeatKind {
    Operator,
    Interval,
}

#[derive(Clone, Copy)]
struct RepeatToken {
    lower: u32,
    upper: Option<u32>,
    greedy: bool,
    possessive: bool,
    kind: RepeatKind,
}

#[derive(Clone, Copy)]
enum Token {
    End,
    Char(char),
    CodePoint(char),
    CrudeByte(u8),
    Repeat(RepeatToken),
    Alternation,
    GroupOpen,
    GroupClose,
    CharType { kind: CharType, negated: bool },
    Property { negated: bool },
    Anchor(Anchor),
    Backref(usize),
    ClassOpen,
    AnyChar,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ClassToken {
    End,
    Char(char, bool),
    CodePoint(char),
    CrudeByte(u8),
    Close,
    Range,
    CharType { kind: CharType, negated: bool },
    Property { negated: bool },
    PosixOpen,
    Open,
    And,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ClassState {
    Start,
    Value,
    Range,
    Complete,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ClassValue {
    Undefined,
    Single,
    Multi,
    Property,
}

pub(super) struct Parsed {
    pub(super) root: Node,
    pub(super) groups: usize,
}

struct Parser<'a> {
    pattern: &'a str,
    position: usize,
    options: Options,
    token: Token,
    groups: usize,
    names: Vec<(&'a str, Vec<usize>)>,
    depth: u32,
}

pub(super) fn parse(pattern: &str) -> Result<Parsed, Error> {
    let mut parser = Parser {
        pattern,
        position: 0,
        options: Options {
            ignore_case: false,
            extend: false,
            multiline: false,
        },
        token: Token::End,
        groups: 0,
        names: Vec::new(),
        depth: 0,
    };
    parser.fetch()?;
    let root = parser.alternatives(Terminator::End)?;
    Ok(Parsed {
        root,
        groups: parser.groups,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Terminator {
    End,
    GroupClose,
}

impl Terminator {
    const fn matches(self, token: Token) -> bool {
        match self {
            Self::End => matches!(token, Token::End),
            Self::GroupClose => matches!(token, Token::GroupClose),
        }
    }
}

enum Bag {
    Group(Node),
    OptionsOnly(Options),
    Node(Node),
}

impl<'a> Parser<'a> {
    const fn at_end(&self) -> bool {
        self.position >= self.pattern.len()
    }

    fn peek(&self) -> Option<char> {
        self.pattern[self.position..].chars().next()
    }

    fn peek_is(&self, expected: char) -> bool {
        self.peek() == Some(expected)
    }

    fn next_char(&mut self) -> Option<char> {
        let character = self.peek()?;
        self.position += character.len_utf8();
        Some(character)
    }

    const fn unfetch(&mut self, character: char) {
        self.position -= character.len_utf8();
    }

    const fn enter(&mut self) -> Result<(), Error> {
        self.depth += 1;
        if self.depth > PARSE_DEPTH_MAX {
            return Err(Error::ParseDepthLimit);
        }
        Ok(())
    }

    fn leave(&mut self) {
        assert!(self.depth > 0, "parse depth is entered before it is left");
        self.depth -= 1;
    }

    fn scan_number(&mut self) -> Result<Option<u32>, Error> {
        let start = self.position;
        let mut value: u32 = 0;
        while let Some(character) = self.peek() {
            let Some(digit) = character.to_digit(DECIMAL_RADIX) else {
                break;
            };
            self.position += 1;
            value = value
                .checked_mul(DECIMAL_RADIX)
                .and_then(|value| value.checked_add(digit))
                .filter(|value| i32::try_from(*value).is_ok())
                .ok_or(Error::NumberTooBig)?;
        }
        Ok((self.position > start).then_some(value))
    }

    fn scan_radix(
        &mut self,
        radix: u32,
        minimum: usize,
        maximum: usize,
    ) -> Result<(u32, usize), Error> {
        let mut value: u32 = 0;
        let mut digits = 0;
        while digits < maximum {
            let Some(character) = self.peek() else {
                break;
            };
            let Some(digit) = character.to_digit(radix).filter(|_| character.is_ascii()) else {
                break;
            };
            self.position += 1;
            digits += 1;
            value = value
                .checked_mul(radix)
                .and_then(|value| value.checked_add(digit))
                .ok_or(Error::NumberTooBig)?;
        }
        if digits < minimum {
            return Err(Error::InvalidCodePoint);
        }
        Ok((value, digits))
    }

    fn fetch(&mut self) -> Result<(), Error> {
        self.token = self.fetch_token()?;
        Ok(())
    }

    fn fetch_token(&mut self) -> Result<Token, Error> {
        loop {
            let Some(character) = self.next_char() else {
                return Ok(Token::End);
            };
            if character == '\\' {
                return self.fetch_escape();
            }
            let token = match character {
                '.' => Token::AnyChar,
                '*' => self.repeat(0, None, RepeatKind::Operator),
                '+' => self.repeat(1, None, RepeatKind::Operator),
                '?' => self.repeat(0, Some(1), RepeatKind::Operator),
                '{' => match self.fetch_interval()? {
                    Some(token) => token,
                    None => Token::Char('{'),
                },
                '|' => Token::Alternation,
                '(' => {
                    if self.peek_is('?') {
                        let saved = self.position;
                        self.position += 1;
                        if self.peek_is('#') {
                            self.position += 1;
                            loop {
                                let Some(character) = self.next_char() else {
                                    return Err(Error::EndPatternInGroup);
                                };
                                if character == '\\' {
                                    self.next_char();
                                } else if character == ')' {
                                    break;
                                }
                            }
                            continue;
                        }
                        self.position = saved;
                    }
                    Token::GroupOpen
                }
                ')' => Token::GroupClose,
                '^' => Token::Anchor(Anchor::BeginLine),
                '$' => Token::Anchor(Anchor::EndLine),
                '[' => Token::ClassOpen,
                '#' if self.options.extend => {
                    while let Some(character) = self.next_char() {
                        if character == '\n' {
                            break;
                        }
                    }
                    continue;
                }
                ' ' | '\t' | '\n' | '\r' | '\u{c}' if self.options.extend => continue,
                other => Token::Char(other),
            };
            return Ok(token);
        }
    }

    fn repeat(&mut self, lower: u32, upper: Option<u32>, kind: RepeatKind) -> Token {
        let mut token = RepeatToken {
            lower,
            upper,
            greedy: true,
            possessive: false,
            kind,
        };
        self.greedy_check(&mut token, true);
        Token::Repeat(token)
    }

    fn greedy_check(&mut self, token: &mut RepeatToken, lazy_allowed: bool) {
        if lazy_allowed && self.peek_is('?') && !token.possessive {
            self.position += 1;
            token.greedy = false;
            token.possessive = false;
            return;
        }
        token.greedy = true;
        if self.peek_is('+') && token.kind == RepeatKind::Operator && !token.possessive {
            self.position += 1;
            token.possessive = true;
        }
    }

    fn fetch_interval(&mut self) -> Result<Option<Token>, Error> {
        let start = self.position;
        if self.at_end() {
            return Ok(None);
        }
        let mut abbreviated_lower = false;
        let lower = match self.scan_number()? {
            Some(lower) if lower > REPEAT_COUNT_MAX => return Err(Error::RepeatRangeTooBig),
            Some(lower) => lower,
            None => {
                abbreviated_lower = true;
                0
            }
        };
        let invalid = |parser: &mut Self| {
            parser.position = start;
            Ok(None)
        };
        let Some(character) = self.next_char() else {
            return invalid(self);
        };
        let mut fixed = false;
        let upper = if character == ',' {
            match self.scan_number()? {
                Some(upper) if upper > REPEAT_COUNT_MAX => return Err(Error::RepeatRangeTooBig),
                Some(upper) => Some(upper),
                None if abbreviated_lower => return invalid(self),
                None => None,
            }
        } else {
            if abbreviated_lower {
                return invalid(self);
            }
            self.unfetch(character);
            fixed = true;
            Some(lower)
        };
        if self.next_char() != Some('}') {
            return invalid(self);
        }
        let mut token = RepeatToken {
            lower,
            upper,
            greedy: true,
            possessive: false,
            kind: RepeatKind::Interval,
        };
        if let Some(upper) = upper
            && lower > upper
        {
            token.possessive = true;
            token.lower = upper;
            token.upper = Some(lower);
        }
        self.greedy_check(&mut token, !fixed);
        Ok(Some(Token::Repeat(token)))
    }

    fn fetch_escape(&mut self) -> Result<Token, Error> {
        let Some(character) = self.next_char() else {
            return Err(Error::EndPatternAtEscape);
        };
        let token = match character {
            'w' => Token::CharType {
                kind: CharType::Word,
                negated: false,
            },
            'W' => Token::CharType {
                kind: CharType::Word,
                negated: true,
            },
            'b' => Token::Anchor(Anchor::WordBoundary),
            'B' => Token::Anchor(Anchor::NotWordBoundary),
            's' => Token::CharType {
                kind: CharType::Space,
                negated: false,
            },
            'S' => Token::CharType {
                kind: CharType::Space,
                negated: true,
            },
            'd' => Token::CharType {
                kind: CharType::Digit,
                negated: false,
            },
            'D' => Token::CharType {
                kind: CharType::Digit,
                negated: true,
            },
            'h' => Token::CharType {
                kind: CharType::HexDigit,
                negated: false,
            },
            'H' => Token::CharType {
                kind: CharType::HexDigit,
                negated: true,
            },
            'A' => Token::Anchor(Anchor::BeginBuffer),
            'Z' => Token::Anchor(Anchor::SemiEndBuffer),
            'z' => Token::Anchor(Anchor::EndBuffer),
            'G' => Token::Anchor(Anchor::BeginPosition),
            'y' | 'Y' | 'K' | 'R' | 'N' | 'O' | 'X' | 'g' => {
                return Err(Error::Unsupported("escape"));
            }
            'o' if self.peek_is('{') => return Err(Error::Unsupported("braced octal escape")),
            'x' if !self.at_end() => {
                if self.peek_is('{') {
                    self.position += 1;
                    return self
                        .braced_code_point(HEXADECIMAL_RADIX, HEX_DIGITS_BRACED_MAX)
                        .map(Token::CodePoint);
                }
                let (code, _) = self.scan_radix(HEXADECIMAL_RADIX, 0, HEX_DIGITS_BYTE_MAX)?;
                Token::CrudeByte(u8::try_from(code).expect("two hexadecimal digits fit a byte"))
            }
            'u' if !self.at_end() => {
                let (code, _) =
                    self.scan_radix(HEXADECIMAL_RADIX, HEX_DIGITS_UNICODE, HEX_DIGITS_UNICODE)?;
                Token::CodePoint(char::from_u32(code).ok_or(Error::InvalidCodePoint)?)
            }
            '1'..='9' => {
                self.unfetch(character);
                let before = self.position;
                match self.scan_number() {
                    Ok(Some(number))
                        if number <= BACKREF_NUMBER_MAX
                            && (number as usize <= self.groups
                                || number <= SINGLE_DIGIT_BACKREF_MAX) =>
                    {
                        Token::Backref(number as usize)
                    }
                    _ => {
                        self.position = before;
                        if matches!(character, '8' | '9') {
                            self.position += 1;
                            Token::Char(character)
                        } else {
                            self.octal(OCTAL_DIGITS_MAX)?
                        }
                    }
                }
            }
            '0' => self.octal(OCTAL_DIGITS_AFTER_ZERO_MAX)?,
            'k' if !self.at_end() => {
                let next = self.peek().expect("not at end");
                if next == '<' || next == '\'' {
                    self.position += 1;
                    return self.named_backref(next);
                }
                Token::Char('k')
            }
            'p' | 'P' if self.peek_is('{') => {
                self.position += 1;
                let mut negated = character == 'P';
                if self.peek_is('^') {
                    self.position += 1;
                    negated = !negated;
                }
                Token::Property { negated }
            }
            other => {
                self.unfetch(other);
                let value = self.escaped_value()?;
                if value == other as u32 {
                    Token::Char(other)
                } else {
                    Token::CodePoint(char::from_u32(value).ok_or(Error::InvalidCodePoint)?)
                }
            }
        };
        Ok(token)
    }

    fn octal(&mut self, maximum: usize) -> Result<Token, Error> {
        let (code, _) = self.scan_radix(OCTAL_RADIX, 0, maximum)?;
        let byte = u8::try_from(code).map_err(|_| Error::NumberTooBig)?;
        Ok(Token::CrudeByte(byte))
    }

    fn braced_code_point(&mut self, radix: u32, maximum: usize) -> Result<char, Error> {
        let (code, digits) = self.scan_radix(radix, 0, maximum)?;
        if let Some(next) = self.peek()
            && next.is_digit(radix)
            && next.is_ascii()
        {
            return Err(Error::TooLongWideCharValue);
        }
        if digits == 0 || !self.peek_is('}') {
            return Err(Error::Unsupported("code point sequence"));
        }
        self.position += 1;
        char::from_u32(code).ok_or(Error::InvalidCodePoint)
    }

    fn escaped_value(&mut self) -> Result<u32, Error> {
        let Some(character) = self.next_char() else {
            return Err(Error::EndPatternAtEscape);
        };
        let value = match character {
            'M' => {
                if self.next_char().ok_or(Error::EndPatternAtMeta)? != '-' {
                    return Err(Error::MetaCodeSyntax);
                }
                let mut code = self.next_char().ok_or(Error::EndPatternAtMeta)? as u32;
                if code == '\\' as u32 {
                    code = self.escaped_value()?;
                }
                (code & 0xff) | META_BIT
            }
            'C' => {
                if self.next_char().ok_or(Error::EndPatternAtControl)? != '-' {
                    return Err(Error::ControlCodeSyntax);
                }
                self.control()?
            }
            'c' => self.control()?,
            other => backslash_value(other) as u32,
        };
        Ok(value)
    }

    fn control(&mut self) -> Result<u32, Error> {
        let character = self.next_char().ok_or(Error::EndPatternAtControl)?;
        if character == '?' {
            return Ok(DELETE);
        }
        let mut code = character as u32;
        if character == '\\' {
            code = self.escaped_value()?;
        }
        Ok(code & CONTROL_MASK)
    }

    fn named_backref(&mut self, opening: char) -> Result<Token, Error> {
        let closing = if opening == '<' { '>' } else { '\'' };
        let start = self.position;
        loop {
            match self.next_char() {
                None => return Err(Error::InvalidGroupName),
                Some(character) if character == closing => break,
                Some(_) => {}
            }
        }
        let name = &self.pattern[start..self.position - closing.len_utf8()];
        if name.is_empty() {
            return Err(Error::InvalidGroupName);
        }
        if let Ok(number) = name.parse::<usize>() {
            if number == 0 {
                return Err(Error::InvalidBackref);
            }
            return Ok(Token::Backref(number));
        }
        let Some((_, groups)) = self.names.iter().find(|(known, _)| *known == name) else {
            return Err(Error::UndefinedNameReference);
        };
        if groups.len() != 1 {
            return Err(Error::Unsupported("backreference to a multiplex name"));
        }
        Ok(Token::Backref(groups[0]))
    }

    fn alternatives(&mut self, terminator: Terminator) -> Result<Node, Error> {
        self.enter()?;
        let saved = self.options;
        let first = self.branch(terminator)?;
        let node = if terminator.matches(self.token) {
            first
        } else if matches!(self.token, Token::Alternation) {
            let mut branches = vec![first];
            while matches!(self.token, Token::Alternation) {
                self.fetch()?;
                branches.push(self.branch(terminator)?);
            }
            if !terminator.matches(self.token) {
                return Err(unmatched(terminator));
            }
            Node::Alternation(branches)
        } else {
            return Err(unmatched(terminator));
        };
        self.options = saved;
        self.leave();
        Ok(node)
    }

    const fn branch_ends(&self, terminator: Terminator) -> bool {
        matches!(self.token, Token::End | Token::Alternation) || terminator.matches(self.token)
    }

    fn branch(&mut self, terminator: Terminator) -> Result<Node, Error> {
        self.enter()?;
        let first = self.expression(terminator)?;
        if self.branch_ends(terminator) {
            self.leave();
            return Ok(first);
        }
        let mut nodes = vec![first];
        while !self.branch_ends(terminator) {
            match self.expression(terminator)? {
                Node::Concat(inner) => nodes.extend(inner),
                other => nodes.push(other),
            }
        }
        self.leave();
        Ok(Node::Concat(nodes))
    }

    fn expression(&mut self, terminator: Terminator) -> Result<Node, Error> {
        if terminator.matches(self.token) || matches!(self.token, Token::Alternation | Token::End) {
            return Ok(Node::Empty);
        }
        let mut group = false;
        let mut node = match self.token {
            Token::End | Token::Alternation => unreachable!("handled above"),
            Token::GroupOpen => match self.bag()? {
                Bag::Group(node) => {
                    group = true;
                    node
                }
                Bag::OptionsOnly(options) => {
                    let saved = self.options;
                    self.options = options;
                    self.fetch()?;
                    let body = self.alternatives(terminator)?;
                    self.options = saved;
                    return Ok(Node::Options(Box::new(body)));
                }
                Bag::Node(node) => node,
            },
            Token::GroupClose => return Err(Error::UnmatchedCloseParenthesis),
            Token::Char(character) => {
                let mut text = String::new();
                text.push(character);
                loop {
                    self.fetch()?;
                    let Token::Char(character) = self.token else {
                        break;
                    };
                    text.push(character);
                }
                let node = self.string(text);
                return self.quantifiers(node, false);
            }
            Token::CrudeByte(byte) => {
                if byte >= ASCII_LIMIT as u8 {
                    return Err(Error::Unsupported("byte escape above ASCII"));
                }
                self.string(char::from(byte).to_string())
            }
            Token::CodePoint(character) => self.string(character.to_string()),
            Token::CharType { kind, negated } => match kind {
                CharType::Word => Node::Word { negated },
                other => Node::Class(Class {
                    set: char_type_set(other),
                    negated,
                }),
            },
            Token::Property { negated } => {
                let set = self.property()?;
                match set {
                    PropertySet::Word => Node::Word { negated },
                    PropertySet::Ranges(set) => Node::Class(Class { set, negated }),
                }
            }
            Token::ClassOpen => {
                let class = self.class()?;
                if self.options.ignore_case {
                    fold::fold_class(class)
                } else {
                    Node::Class(class)
                }
            }
            Token::AnyChar => Node::Any {
                multiline: self.options.multiline,
            },
            Token::Backref(group) => Node::Backref {
                group,
                ignore_case: self.options.ignore_case,
            },
            Token::Anchor(anchor) => Node::Anchor(anchor),
            Token::Repeat(_) => return Err(Error::RepeatTargetMissing),
        };
        self.fetch()?;
        if group {
            return self.quantifiers(node, true);
        }
        node = self.quantifiers(node, false)?;
        Ok(node)
    }

    const fn string(&self, text: String) -> Node {
        Node::String {
            text,
            ignore_case: self.options.ignore_case,
            expanded: false,
        }
    }

    fn quantifiers(&mut self, mut node: Node, mut group: bool) -> Result<Node, Error> {
        let mut prefix: Vec<Node> = Vec::new();
        while let Token::Repeat(token) = self.token {
            self.enter()?;
            if invalid_quantifier_target(&node) {
                return Err(Error::RepeatTargetInvalid);
            }
            let quantified = assign_quantifier(token, node, group, &mut prefix)?;
            node = if token.possessive {
                Node::Atomic(Box::new(quantified))
            } else {
                quantified
            };
            group = false;
            self.leave();
            self.fetch()?;
        }
        if prefix.is_empty() {
            Ok(node)
        } else {
            prefix.push(node);
            Ok(Node::Concat(prefix))
        }
    }

    fn property(&mut self) -> Result<PropertySet, Error> {
        let start = self.position;
        loop {
            match self.next_char() {
                None => return Err(Error::EndPatternWithUnmatchedParenthesis),
                Some('}') => break,
                Some('(' | ')' | '{' | '|') => {
                    return Err(Error::EndPatternWithUnmatchedParenthesis);
                }
                Some(_) => {}
            }
        }
        let name = &self.pattern[start..self.position - 1];
        property_set(name)
    }

    fn bag(&mut self) -> Result<Bag, Error> {
        if self.at_end() {
            return Err(Error::EndPatternWithUnmatchedParenthesis);
        }
        if self.peek_is('?') {
            self.position += 1;
            let character = self.next_char().ok_or(Error::EndPatternInGroup)?;
            let node = match character {
                ':' => {
                    self.fetch()?;
                    let body = self.alternatives(Terminator::GroupClose)?;
                    return Ok(Bag::Group(body));
                }
                '=' => self.look(false, false)?,
                '!' => self.look(false, true)?,
                '>' => {
                    self.fetch()?;
                    Node::Atomic(Box::new(self.alternatives(Terminator::GroupClose)?))
                }
                '\'' => self.named_group('\'')?,
                '<' => {
                    let next = self
                        .next_char()
                        .ok_or(Error::EndPatternWithUnmatchedParenthesis)?;
                    match next {
                        '=' => self.look(true, false)?,
                        '!' => self.look(true, true)?,
                        other => {
                            self.unfetch(other);
                            self.named_group('<')?
                        }
                    }
                }
                '-' | 'i' | 'm' | 'x' => return self.option_group(character),
                _ => return Err(Error::Unsupported("group kind")),
            };
            return Ok(Bag::Node(node));
        }
        if self.peek_is('*') {
            return Err(Error::Unsupported("callout"));
        }
        self.groups += 1;
        let group = self.groups;
        self.fetch()?;
        let body = self.alternatives(Terminator::GroupClose)?;
        Ok(Bag::Node(Node::Capture {
            group,
            body: Box::new(body),
        }))
    }

    fn look(&mut self, behind: bool, negated: bool) -> Result<Node, Error> {
        self.fetch()?;
        let body = Box::new(self.alternatives(Terminator::GroupClose)?);
        Ok(if behind {
            Node::LookBehind {
                negated,
                length: LookBehindLength::Unknown,
                body,
            }
        } else {
            Node::LookAhead { negated, body }
        })
    }

    fn named_group(&mut self, opening: char) -> Result<Node, Error> {
        let closing = if opening == '<' { '>' } else { '\'' };
        let start = self.position;
        let first = self.next_char().ok_or(Error::EmptyGroupName)?;
        if first == closing {
            return Err(Error::EmptyGroupName);
        }
        if first.is_ascii_digit() || !is_name_character(first) {
            return Err(Error::InvalidGroupName);
        }
        loop {
            match self.next_char() {
                None => return Err(Error::InvalidGroupName),
                Some(character) if character == closing => break,
                Some(character) if is_name_character(character) => {}
                Some(_) => return Err(Error::InvalidGroupName),
            }
        }
        let name = &self.pattern[start..self.position - closing.len_utf8()];
        self.groups += 1;
        let group = self.groups;
        match self.names.iter_mut().find(|(known, _)| *known == name) {
            Some((_, groups)) => groups.push(group),
            None => self.names.push((name, vec![group])),
        }
        self.fetch()?;
        let body = self.alternatives(Terminator::GroupClose)?;
        Ok(Node::Capture {
            group,
            body: Box::new(body),
        })
    }

    fn option_group(&mut self, first: char) -> Result<Bag, Error> {
        let mut options = self.options;
        let mut negated = false;
        let mut character = first;
        loop {
            match character {
                ')' => return Ok(Bag::OptionsOnly(options)),
                ':' => {
                    let saved = self.options;
                    self.options = options;
                    self.fetch()?;
                    let body = self.alternatives(Terminator::GroupClose);
                    self.options = saved;
                    return Ok(Bag::Node(Node::Options(Box::new(body?))));
                }
                '-' => negated = true,
                'x' => options.extend = !negated,
                'i' => options.ignore_case = !negated,
                'm' => options.multiline = !negated,
                _ => return Err(Error::Unsupported("group option")),
            }
            character = self.next_char().ok_or(Error::EndPatternInGroup)?;
        }
    }

    fn class(&mut self) -> Result<Class, Error> {
        self.enter()?;
        let mut token = self.fetch_class_token()?;
        let negated = if token == ClassToken::Char('^', false) {
            token = self.fetch_class_token()?;
            true
        } else {
            false
        };
        if token == ClassToken::Close {
            if !close_exists(&self.pattern[self.position..]) {
                return Err(Error::EmptyCharClass);
            }
            token = ClassToken::Char(']', false);
        }
        let mut set = CodePointSet::new();
        let mut previous: Option<CodePointSet> = None;
        let mut state = ClassState::Start;
        let mut current_code: u32 = 0;
        let mut current_type = ClassValue::Undefined;
        loop {
            if token == ClassToken::Close {
                break;
            }
            let mut fetched = false;
            match token {
                ClassToken::Char(character, _) => {
                    let value = character as u32;
                    let kind = value_kind(value);
                    char_next(
                        &mut set,
                        &mut current_code,
                        value,
                        kind,
                        &mut current_type,
                        &mut state,
                    )?;
                }
                ClassToken::CrudeByte(byte) => {
                    if u32::from(byte) >= ASCII_LIMIT {
                        return Err(Error::Unsupported("byte escape above ASCII"));
                    }
                    char_next(
                        &mut set,
                        &mut current_code,
                        u32::from(byte),
                        ClassValue::Single,
                        &mut current_type,
                        &mut state,
                    )?;
                }
                ClassToken::CodePoint(character) => {
                    let value = character as u32;
                    char_next(
                        &mut set,
                        &mut current_code,
                        value,
                        value_kind(value),
                        &mut current_type,
                        &mut state,
                    )?;
                }
                ClassToken::PosixOpen => {
                    let posix = self.posix_bracket()?;
                    set.union_with(&posix);
                    property_next(&mut set, &mut current_code, &mut current_type, &mut state)?;
                }
                ClassToken::CharType { kind, negated } => {
                    let mut added = match kind {
                        CharType::Word => CodePointSet::from_ranges(unicode::WORD),
                        other => char_type_set(other),
                    };
                    if negated {
                        added = added.complement();
                    }
                    set.union_with(&added);
                    property_next(&mut set, &mut current_code, &mut current_type, &mut state)?;
                }
                ClassToken::Property { negated } => {
                    let mut added = match self.property()? {
                        PropertySet::Word => CodePointSet::from_ranges(unicode::WORD),
                        PropertySet::Ranges(ranges) => ranges,
                    };
                    if negated {
                        added = added.complement();
                    }
                    set.union_with(&added);
                    property_next(&mut set, &mut current_code, &mut current_type, &mut state)?;
                }
                ClassToken::Range => match state {
                    ClassState::Value => {
                        let next = self.fetch_class_token()?;
                        fetched = true;
                        token = next;
                        if next == ClassToken::Close || next == ClassToken::And {
                            char_next(
                                &mut set,
                                &mut current_code,
                                '-' as u32,
                                ClassValue::Single,
                                &mut current_type,
                                &mut state,
                            )?;
                        } else if current_type == ClassValue::Property {
                            return Err(Error::UnmatchedRangeSpecifierInCharClass);
                        } else {
                            state = ClassState::Range;
                        }
                    }
                    ClassState::Start => {
                        let next = self.fetch_class_token()?;
                        fetched = true;
                        token = next;
                        char_next(
                            &mut set,
                            &mut current_code,
                            '-' as u32,
                            ClassValue::Single,
                            &mut current_type,
                            &mut state,
                        )?;
                    }
                    ClassState::Range => {
                        char_next(
                            &mut set,
                            &mut current_code,
                            '-' as u32,
                            ClassValue::Single,
                            &mut current_type,
                            &mut state,
                        )?;
                    }
                    ClassState::Complete => {
                        let next = self.fetch_class_token()?;
                        fetched = true;
                        token = next;
                        char_next(
                            &mut set,
                            &mut current_code,
                            '-' as u32,
                            ClassValue::Single,
                            &mut current_type,
                            &mut state,
                        )?;
                    }
                },
                ClassToken::Open => {
                    if state == ClassState::Value {
                        flush_value(&mut set, current_code, current_type);
                    }
                    state = ClassState::Complete;
                    let nested = self.class()?;
                    let nested_set = if nested.negated {
                        nested.set.complement()
                    } else {
                        nested.set
                    };
                    set.union_with(&nested_set);
                }
                ClassToken::And => {
                    if state == ClassState::Value {
                        flush_value(&mut set, current_code, current_type);
                    }
                    state = ClassState::Start;
                    current_type = ClassValue::Undefined;
                    let finished = std::mem::replace(&mut set, CodePointSet::new());
                    previous = Some(match previous {
                        Some(previous) => previous.intersection(&finished),
                        None => finished,
                    });
                }
                ClassToken::End => return Err(Error::PrematureEndOfCharClass),
                ClassToken::Close => unreachable!("loop exits on close"),
            }
            if !fetched {
                token = self.fetch_class_token()?;
            }
        }
        if state == ClassState::Value {
            flush_value(&mut set, current_code, current_type);
        }
        if let Some(previous) = previous {
            set = previous.intersection(&set);
        }
        self.leave();
        Ok(Class { set, negated })
    }

    fn posix_bracket(&mut self) -> Result<CodePointSet, Error> {
        let negated = if self.peek_is('^') {
            self.position += 1;
            true
        } else {
            false
        };
        const NAMES: [&str; 14] = [
            "alnum", "alpha", "blank", "cntrl", "digit", "graph", "lower", "print", "punct",
            "space", "upper", "xdigit", "ascii", "word",
        ];
        let rest = &self.pattern[self.position..];
        for name in NAMES {
            if let Some(after) = rest.strip_prefix(name) {
                if !after.starts_with(POSIX_BRACKET_CLOSE) {
                    break;
                }
                let set = posix_set(name)?;
                self.position += name.len() + POSIX_BRACKET_CLOSE.len();
                return Ok(if negated { set.complement() } else { set });
            }
        }
        Err(Error::InvalidPosixBracketType)
    }

    fn fetch_class_token(&mut self) -> Result<ClassToken, Error> {
        let Some(character) = self.next_char() else {
            return Ok(ClassToken::End);
        };
        let token = match character {
            ']' => ClassToken::Close,
            '-' => ClassToken::Range,
            '\\' => {
                let Some(escaped) = self.next_char() else {
                    return Err(Error::EndPatternAtEscape);
                };
                match escaped {
                    'w' | 'W' | 'd' | 'D' | 's' | 'S' | 'h' | 'H' => ClassToken::CharType {
                        kind: match escaped.to_ascii_lowercase() {
                            'w' => CharType::Word,
                            'd' => CharType::Digit,
                            's' => CharType::Space,
                            _ => CharType::HexDigit,
                        },
                        negated: escaped.is_ascii_uppercase(),
                    },
                    'p' | 'P' if self.peek_is('{') => {
                        self.position += 1;
                        let mut negated = escaped == 'P';
                        if self.peek_is('^') {
                            self.position += 1;
                            negated = !negated;
                        }
                        ClassToken::Property { negated }
                    }
                    'o' if self.peek_is('{') => {
                        return Err(Error::Unsupported("braced octal escape"));
                    }
                    'x' if !self.at_end() => {
                        if self.peek_is('{') {
                            self.position += 1;
                            return self
                                .braced_code_point(HEXADECIMAL_RADIX, HEX_DIGITS_BRACED_MAX)
                                .map(ClassToken::CodePoint);
                        }
                        let (code, _) =
                            self.scan_radix(HEXADECIMAL_RADIX, 0, HEX_DIGITS_BYTE_MAX)?;
                        ClassToken::CrudeByte(
                            u8::try_from(code).expect("two hexadecimal digits fit a byte"),
                        )
                    }
                    'u' if !self.at_end() => {
                        let (code, _) = self.scan_radix(
                            HEXADECIMAL_RADIX,
                            HEX_DIGITS_UNICODE,
                            HEX_DIGITS_UNICODE,
                        )?;
                        ClassToken::CodePoint(char::from_u32(code).ok_or(Error::InvalidCodePoint)?)
                    }
                    '0'..='7' => {
                        self.unfetch(escaped);
                        let (code, _) = self.scan_radix(OCTAL_RADIX, 0, OCTAL_DIGITS_MAX)?;
                        let byte = u8::try_from(code).map_err(|_| Error::NumberTooBig)?;
                        ClassToken::CrudeByte(byte)
                    }
                    other => {
                        self.unfetch(other);
                        let value = self.escaped_value()?;
                        if value == other as u32 {
                            ClassToken::Char(other, true)
                        } else {
                            ClassToken::CodePoint(
                                char::from_u32(value).ok_or(Error::InvalidCodePoint)?,
                            )
                        }
                    }
                }
            }
            '[' => {
                if self.peek_is(':') {
                    let saved = self.position;
                    self.position += 1;
                    if posix_bracket_start(&self.pattern[self.position..]) {
                        return Ok(ClassToken::PosixOpen);
                    }
                    self.position = saved;
                }
                ClassToken::Open
            }
            '&' if self.peek_is('&') => {
                self.position += 1;
                ClassToken::And
            }
            other => ClassToken::Char(other, false),
        };
        Ok(token)
    }
}

enum PropertySet {
    Word,
    Ranges(CodePointSet),
}

fn property_set(name: &str) -> Result<PropertySet, Error> {
    if !name.is_ascii() {
        return Err(Error::InvalidCharPropertyName);
    }
    let normalized: String = name
        .chars()
        .filter(|character| !matches!(character, ' ' | '-' | '_'))
        .map(|character| character.to_ascii_lowercase())
        .collect();
    let ranges = match normalized.as_str() {
        "word" => return Ok(PropertySet::Word),
        "l" | "letter" => unicode::LETTER,
        "lu" | "uppercaseletter" => unicode::UPPERCASE_LETTER,
        "ll" | "lowercaseletter" => unicode::LOWERCASE_LETTER,
        "lt" | "titlecaseletter" => unicode::TITLECASE_LETTER,
        "lo" | "otherletter" => unicode::OTHER_LETTER,
        "m" | "mark" | "combiningmark" => unicode::MARK,
        "n" | "number" => unicode::NUMBER,
        "nd" | "decimalnumber" | "digit" => unicode::DECIMAL_NUMBER,
        "nl" | "letternumber" => unicode::LETTER_NUMBER,
        "s" | "symbol" => unicode::SYMBOL,
        "sm" | "mathsymbol" => unicode::MATH_SYMBOL,
        "so" | "othersymbol" => unicode::OTHER_SYMBOL,
        "space" | "whitespace" => unicode::WHITE_SPACE,
        _ => return Err(Error::Unsupported("character property")),
    };
    Ok(PropertySet::Ranges(CodePointSet::from_ranges(ranges)))
}

fn posix_set(name: &str) -> Result<CodePointSet, Error> {
    Ok(match name {
        "ascii" => CodePointSet::from_ranges(&[(0, ASCII_LIMIT - 1)]),
        "xdigit" => char_type_set(CharType::HexDigit),
        "word" => CodePointSet::from_ranges(unicode::WORD),
        "space" => CodePointSet::from_ranges(unicode::WHITE_SPACE),
        "digit" => CodePointSet::from_ranges(unicode::DECIMAL_NUMBER),
        _ => return Err(Error::Unsupported("POSIX bracket")),
    })
}

pub(super) fn char_type_set(kind: CharType) -> CodePointSet {
    match kind {
        CharType::Word => CodePointSet::from_ranges(unicode::WORD),
        CharType::Space => CodePointSet::from_ranges(unicode::WHITE_SPACE),
        CharType::Digit => CodePointSet::from_ranges(unicode::DECIMAL_NUMBER),
        CharType::HexDigit => CodePointSet::from_ranges(&[
            ('0' as u32, '9' as u32),
            ('A' as u32, 'F' as u32),
            ('a' as u32, 'f' as u32),
        ]),
    }
}

fn posix_bracket_start(rest: &str) -> bool {
    let mut count = 0;
    let mut characters = rest.chars().peekable();
    while let Some(character) = characters.next() {
        if character == ':' {
            return characters.peek() == Some(&']') && count > 0;
        }
        if character == '^' && count == 0 {
        } else if !character.is_alphabetic() {
            break;
        }
        count += 1;
    }
    false
}

fn close_exists(rest: &str) -> bool {
    let mut escaped = false;
    for character in rest.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == ']' {
            return true;
        }
        if character == '\\' {
            escaped = true;
        }
    }
    false
}

const fn value_kind(value: u32) -> ClassValue {
    if value < ASCII_LIMIT {
        ClassValue::Single
    } else {
        ClassValue::Multi
    }
}

fn flush_value(set: &mut CodePointSet, code: u32, kind: ClassValue) {
    if matches!(kind, ClassValue::Single | ClassValue::Multi) {
        set.add_range(code, code);
    }
}

fn char_next(
    set: &mut CodePointSet,
    from: &mut u32,
    to: u32,
    kind: ClassValue,
    current_type: &mut ClassValue,
    state: &mut ClassState,
) -> Result<(), Error> {
    match *state {
        ClassState::Value => flush_value(set, *from, *current_type),
        ClassState::Range => {
            if *from > to {
                return Err(Error::EmptyRangeInCharClass);
            }
            set.add_range(*from, to);
            *state = ClassState::Complete;
        }
        ClassState::Complete | ClassState::Start => *state = ClassState::Value,
    }
    *from = to;
    *current_type = kind;
    Ok(())
}

fn property_next(
    set: &mut CodePointSet,
    from: &mut u32,
    current_type: &mut ClassValue,
    state: &mut ClassState,
) -> Result<(), Error> {
    if *state == ClassState::Range {
        return Err(Error::CharClassValueAtEndOfRange);
    }
    if *state == ClassState::Value {
        flush_value(set, *from, *current_type);
    }
    *state = ClassState::Value;
    *current_type = ClassValue::Property;
    Ok(())
}

const fn backslash_value(character: char) -> char {
    match character {
        'n' => '\n',
        't' => '\t',
        'r' => '\r',
        'f' => '\u{c}',
        'a' => '\u{7}',
        'b' => '\u{8}',
        'e' => '\u{1b}',
        'v' => '\u{b}',
        other => other,
    }
}

fn is_name_character(character: char) -> bool {
    character == '_' || character.is_alphanumeric()
}

const fn unmatched(terminator: Terminator) -> Error {
    match terminator {
        Terminator::GroupClose => Error::EndPatternWithUnmatchedParenthesis,
        Terminator::End => Error::UnmatchedCloseParenthesis,
    }
}

fn invalid_quantifier_target(node: &Node) -> bool {
    match node {
        Node::Anchor(_) | Node::LookAhead { .. } | Node::LookBehind { .. } => true,
        Node::Alternation(branches) => branches.iter().any(invalid_quantifier_target),
        _ => false,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Popular {
    Optional,
    Star,
    Plus,
    LazyOptional,
    LazyStar,
    LazyPlus,
}

const fn popular(lower: u32, upper: Option<u32>, greedy: bool) -> Option<Popular> {
    match (greedy, lower, upper) {
        (true, 0, Some(1)) => Some(Popular::Optional),
        (true, 0, None) => Some(Popular::Star),
        (true, 1, None) => Some(Popular::Plus),
        (false, 0, Some(1)) => Some(Popular::LazyOptional),
        (false, 0, None) => Some(Popular::LazyStar),
        (false, 1, None) => Some(Popular::LazyPlus),
        _ => None,
    }
}

#[derive(Clone, Copy)]
enum Reduction {
    AsIs,
    Delete,
    Star,
    Plus,
    LazyStar,
    LazyOptional,
    PlusLazyOptional,
}

const fn reduction(child: Popular, parent: Popular) -> Reduction {
    use Popular::*;
    use Reduction as R;
    let row = match child {
        Optional => [
            R::Delete,
            R::Star,
            R::Star,
            R::LazyOptional,
            R::LazyStar,
            R::AsIs,
        ],
        Star => [
            R::Delete,
            R::Delete,
            R::Delete,
            R::PlusLazyOptional,
            R::PlusLazyOptional,
            R::Delete,
        ],
        Plus => [
            R::Star,
            R::Star,
            R::Delete,
            R::AsIs,
            R::PlusLazyOptional,
            R::Delete,
        ],
        LazyOptional => [
            R::Delete,
            R::LazyStar,
            R::LazyStar,
            R::Delete,
            R::LazyStar,
            R::LazyStar,
        ],
        LazyStar => [R::Delete; 6],
        LazyPlus => [
            R::AsIs,
            R::Star,
            R::Plus,
            R::LazyStar,
            R::LazyStar,
            R::Delete,
        ],
    };
    row[parent as usize]
}

fn assign_quantifier(
    token: RepeatToken,
    target: Node,
    group: bool,
    prefix: &mut Vec<Node>,
) -> Result<Node, Error> {
    if token.lower == 1 && token.upper == Some(1) {
        return Ok(target);
    }
    let mut repeat = Repeat {
        lower: token.lower,
        upper: token.upper,
        greedy: token.greedy,
        emptiness: Emptiness::NotEmpty,
        head_exact: None,
        next_head_exact: None,
        body: Box::new(Node::Empty),
    };
    match target {
        Node::String {
            text,
            ignore_case,
            expanded,
        } if !group && text.chars().nth(1).is_some() => {
            let last = text.chars().next_back().expect("string has two characters");
            let head = text[..text.len() - last.len_utf8()].to_owned();
            prefix.push(Node::String {
                text: head,
                ignore_case,
                expanded,
            });
            repeat.body = Box::new(Node::String {
                text: last.to_string(),
                ignore_case,
                expanded,
            });
            Ok(Node::Repeat(repeat))
        }
        Node::Repeat(mut child) => {
            let parent_kind = popular(repeat.lower, repeat.upper, repeat.greedy);
            let child_kind = popular(child.lower, child.upper, child.greedy);
            if let (Some(_), None) = (child_kind, parent_kind) {
                if matches!(child_kind, Some(Popular::Star | Popular::Plus))
                    && repeat.upper.is_some_and(|upper| upper > 1)
                    && repeat.greedy
                {
                    repeat.upper = Some(if repeat.lower == 0 { 1 } else { repeat.lower });
                }
                repeat.body = Box::new(Node::Repeat(child));
                return Ok(Node::Repeat(repeat));
            }
            let (Some(parent_kind), Some(child_kind)) = (parent_kind, child_kind) else {
                if repeat.upper == Some(repeat.lower) && child.upper == Some(child.lower) {
                    let count = repeat
                        .lower
                        .checked_mul(child.lower)
                        .filter(|count| i32::try_from(*count).is_ok())
                        .ok_or(Error::RepeatRangeTooBig)?;
                    repeat.lower = count;
                    repeat.upper = Some(count);
                    repeat.body = child.body;
                    return Ok(Node::Repeat(repeat));
                }
                repeat.body = Box::new(Node::Repeat(child));
                return Ok(Node::Repeat(repeat));
            };
            match reduction(child_kind, parent_kind) {
                Reduction::AsIs => {
                    repeat.body = Box::new(Node::Repeat(child));
                    Ok(Node::Repeat(repeat))
                }
                Reduction::Delete => Ok(Node::Repeat(child)),
                Reduction::Star => Ok(reduced(child, 0, None, true)),
                Reduction::Plus => Ok(reduced(child, 1, None, true)),
                Reduction::LazyStar => Ok(reduced(child, 0, None, false)),
                Reduction::LazyOptional => Ok(reduced(child, 0, Some(1), false)),
                Reduction::PlusLazyOptional => {
                    child.lower = 1;
                    child.upper = None;
                    child.greedy = true;
                    repeat.lower = 0;
                    repeat.upper = Some(1);
                    repeat.greedy = false;
                    repeat.body = Box::new(Node::Repeat(child));
                    Ok(Node::Repeat(repeat))
                }
            }
        }
        other => {
            repeat.body = Box::new(other);
            Ok(Node::Repeat(repeat))
        }
    }
}

fn reduced(child: Repeat, lower: u32, upper: Option<u32>, greedy: bool) -> Node {
    Node::Repeat(Repeat {
        lower,
        upper,
        greedy,
        emptiness: Emptiness::NotEmpty,
        head_exact: None,
        next_head_exact: None,
        body: child.body,
    })
}
