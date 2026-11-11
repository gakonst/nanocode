// Ported from GoAWK's lexer (lexer/lexer.go, lexer/token.go),
// Copyright (c) 2022 Ben Hoyt, MIT License (see NOTICE).

//! The awk lexer: source bytes to tokens. The parser asks for a regex after `/` or `/=`
//! (`scan_regex`), as only it knows whether a slash divides or starts a regex.

/// A builtin function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builtin {
    Atan2,
    Close,
    Cos,
    Exp,
    Fflush,
    Gsub,
    Index,
    Int,
    Length,
    Log,
    Match,
    Rand,
    Sin,
    Split,
    Sprintf,
    Sqrt,
    Srand,
    Sub,
    Substr,
    System,
    Tolower,
    Toupper,
    // gawk's.
    Gensub,
    Systime,
    Strftime,
    Mktime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tok {
    Illegal,
    Eof,
    Newline,

    Add,
    AddAssign,
    And,
    Append,
    Assign,
    Colon,
    Comma,
    Decr,
    Div,
    DivAssign,
    Dollar,
    Equals,
    Gte,
    Greater,
    Incr,
    LBrace,
    LBracket,
    Less,
    LParen,
    Lte,
    Match,
    Mod,
    ModAssign,
    Mul,
    MulAssign,
    NotMatch,
    Not,
    NotEquals,
    Or,
    Pipe,
    Pow,
    PowAssign,
    Question,
    RBrace,
    RBracket,
    RParen,
    Semicolon,
    Sub,
    SubAssign,

    Begin,
    Break,
    Continue,
    Delete,
    Do,
    Else,
    End,
    Exit,
    For,
    Function,
    Getline,
    If,
    In,
    Next,
    Nextfile,
    Print,
    Printf,
    Return,
    While,

    Func(Builtin),

    Name,
    Number,
    Str,
    Regex,
}

fn keyword(name: &[u8]) -> Option<Tok> {
    use Builtin as B;
    Some(match name {
        b"BEGIN" => Tok::Begin,
        b"break" => Tok::Break,
        b"continue" => Tok::Continue,
        b"delete" => Tok::Delete,
        b"do" => Tok::Do,
        b"else" => Tok::Else,
        b"END" => Tok::End,
        b"exit" => Tok::Exit,
        b"for" => Tok::For,
        // `func` is an alias in onetrueawk and gawk.
        b"function" | b"func" => Tok::Function,
        b"getline" => Tok::Getline,
        b"if" => Tok::If,
        b"in" => Tok::In,
        b"next" => Tok::Next,
        b"nextfile" => Tok::Nextfile,
        b"print" => Tok::Print,
        b"printf" => Tok::Printf,
        b"return" => Tok::Return,
        b"while" => Tok::While,
        b"atan2" => Tok::Func(B::Atan2),
        b"close" => Tok::Func(B::Close),
        b"cos" => Tok::Func(B::Cos),
        b"exp" => Tok::Func(B::Exp),
        b"fflush" => Tok::Func(B::Fflush),
        b"gsub" => Tok::Func(B::Gsub),
        b"index" => Tok::Func(B::Index),
        b"int" => Tok::Func(B::Int),
        b"length" => Tok::Func(B::Length),
        b"log" => Tok::Func(B::Log),
        b"match" => Tok::Func(B::Match),
        b"rand" => Tok::Func(B::Rand),
        b"sin" => Tok::Func(B::Sin),
        b"split" => Tok::Func(B::Split),
        b"sprintf" => Tok::Func(B::Sprintf),
        b"sqrt" => Tok::Func(B::Sqrt),
        b"srand" => Tok::Func(B::Srand),
        b"sub" => Tok::Func(B::Sub),
        b"substr" => Tok::Func(B::Substr),
        b"system" => Tok::Func(B::System),
        b"tolower" => Tok::Func(B::Tolower),
        b"toupper" => Tok::Func(B::Toupper),
        b"gensub" => Tok::Func(B::Gensub),
        b"systime" => Tok::Func(B::Systime),
        b"strftime" => Tok::Func(B::Strftime),
        b"mktime" => Tok::Func(B::Mktime),
        _ => return None,
    })
}

/// Where a token starts: line and byte column, from 1.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pos {
    pub line: u32,
    pub col: u32,
}

pub struct Lexer<'a> {
    src: &'a [u8],
    offset: usize,
    ch: u8,
    pos: Pos,
    next_pos: Pos,
    had_space: bool,
    last_tok: Tok,
}

/// A token: its position, kind and, for names, numbers, strings and regexes, its value
/// (for `Illegal`, the message).
pub type Token = (Pos, Tok, Vec<u8>);

impl<'a> Lexer<'a> {
    pub fn new(src: &'a [u8]) -> Lexer<'a> {
        let mut l = Lexer {
            src,
            offset: 0,
            ch: 0,
            pos: Pos::default(),
            next_pos: Pos { line: 1, col: 1 },
            had_space: false,
            last_tok: Tok::Illegal,
        };
        l.next();
        l
    }

    /// Whether whitespace came before the last token: `f (x)` is no call of a user
    /// function `f`.
    pub fn had_space(&self) -> bool {
        self.had_space
    }

    /// The next unscanned byte (0 at the end).
    pub fn peek_byte(&self) -> u8 {
        self.ch
    }

    pub fn scan(&mut self) -> Token {
        let t = self.scan_inner();
        self.last_tok = t.1;
        t
    }

    fn illegal(&self, msg: &str) -> Token {
        (self.pos, Tok::Illegal, msg.as_bytes().to_vec())
    }

    fn scan_inner(&mut self) -> Token {
        // Whitespace, except newline (a token), and line continuations.
        self.had_space = false;
        while matches!(self.ch, b' ' | b'\t' | b'\r' | b'\\') {
            self.had_space = true;
            if self.ch == b'\\' {
                self.next();
                if self.ch == b'\r' {
                    self.next();
                }
                if self.ch != b'\n' {
                    return self.illegal("backslash not last character on line");
                }
            }
            self.next();
        }
        if self.ch == b'#' {
            while self.ch != b'\n' && self.ch != 0 {
                self.next();
            }
        }
        if self.ch == 0 && self.offset > self.src.len() {
            return (self.pos, Tok::Eof, Vec::new());
        }
        let pos = self.pos;
        let ch = self.ch;
        self.next();

        if is_name_start(ch) {
            let start = self.offset - 2;
            while is_name_start(self.ch) || self.ch.is_ascii_digit() {
                self.next();
            }
            let name = &self.src[start..self.offset - 1];
            return match keyword(name) {
                Some(t) => (pos, t, Vec::new()),
                None => (pos, Tok::Name, name.to_vec()),
            };
        }

        let tok = match ch {
            b'$' => Tok::Dollar,
            b'0'..=b'9' | b'.' => {
                let start = self.offset - 2;
                let mut got_digit = false;
                if ch != b'.' {
                    got_digit = true;
                    // gawk's hex constants in source (`0x1F`).
                    if ch == b'0' && matches!(self.ch, b'x' | b'X') {
                        let save = (self.offset, self.ch, self.pos, self.next_pos);
                        self.next();
                        if self.ch.is_ascii_hexdigit() {
                            while self.ch.is_ascii_hexdigit() {
                                self.next();
                            }
                            return (pos, Tok::Number, self.src[start..self.offset - 1].to_vec());
                        }
                        (self.offset, self.ch, self.pos, self.next_pos) = save;
                    }
                    while self.ch.is_ascii_digit() {
                        self.next();
                    }
                    if self.ch == b'.' {
                        self.next();
                    }
                }
                while self.ch.is_ascii_digit() {
                    got_digit = true;
                    self.next();
                }
                if !got_digit {
                    return self.illegal("syntax error");
                }
                if self.ch == b'e' || self.ch == b'E' {
                    // "1e" is "1 e" (e a variable), "1e+" is "1e + ...", as in awk and gawk.
                    let save = (self.offset, self.ch, self.pos, self.next_pos);
                    self.next();
                    if self.ch == b'+' || self.ch == b'-' {
                        self.next();
                    }
                    if self.ch.is_ascii_digit() {
                        while self.ch.is_ascii_digit() {
                            self.next();
                        }
                    } else {
                        (self.offset, self.ch, self.pos, self.next_pos) = save;
                    }
                }
                return (pos, Tok::Number, self.src[start..self.offset - 1].to_vec());
            }
            b'{' => Tok::LBrace,
            b'}' => Tok::RBrace,
            b'=' => self.choice(b'=', Tok::Assign, Tok::Equals),
            b'<' => self.choice(b'=', Tok::Less, Tok::Lte),
            b'>' => match self.ch {
                b'=' => {
                    self.next();
                    Tok::Gte
                }
                b'>' => {
                    self.next();
                    Tok::Append
                }
                _ => Tok::Greater,
            },
            b'"' => {
                let mut s = Vec::new();
                loop {
                    match self.ch {
                        b'"' => break,
                        0 if self.offset > self.src.len() => {
                            return self.illegal("unterminated string");
                        }
                        b'\n' => return self.illegal("unterminated string"),
                        b'\\' => {
                            self.next();
                            if self.ch == b'\n' {
                                // A continued line inside a string.
                                self.next();
                                continue;
                            }
                            if self.ch == 0 && self.offset > self.src.len() {
                                return self.illegal("unterminated string");
                            }
                            self.escape(&mut s);
                        }
                        c => {
                            s.push(c);
                            self.next();
                        }
                    }
                }
                self.next();
                return (pos, Tok::Str, s);
            }
            b'(' => Tok::LParen,
            b')' => Tok::RParen,
            b',' => Tok::Comma,
            b';' => Tok::Semicolon,
            b'+' => match self.ch {
                b'+' => {
                    self.next();
                    Tok::Incr
                }
                b'=' => {
                    self.next();
                    Tok::AddAssign
                }
                _ => Tok::Add,
            },
            b'-' => match self.ch {
                b'-' => {
                    self.next();
                    Tok::Decr
                }
                b'=' => {
                    self.next();
                    Tok::SubAssign
                }
                _ => Tok::Sub,
            },
            b'*' => match self.ch {
                b'*' => {
                    self.next();
                    self.choice(b'=', Tok::Pow, Tok::PowAssign)
                }
                b'=' => {
                    self.next();
                    Tok::MulAssign
                }
                _ => Tok::Mul,
            },
            b'/' => self.choice(b'=', Tok::Div, Tok::DivAssign),
            b'%' => self.choice(b'=', Tok::Mod, Tok::ModAssign),
            b'[' => Tok::LBracket,
            b']' => Tok::RBracket,
            b'\n' => Tok::Newline,
            b'^' => self.choice(b'=', Tok::Pow, Tok::PowAssign),
            b'!' => match self.ch {
                b'=' => {
                    self.next();
                    Tok::NotEquals
                }
                b'~' => {
                    self.next();
                    Tok::NotMatch
                }
                _ => Tok::Not,
            },
            b'~' => Tok::Match,
            b'?' => Tok::Question,
            b':' => Tok::Colon,
            b'&' => {
                if self.ch == b'&' {
                    self.next();
                    Tok::And
                } else {
                    return (pos, Tok::Illegal, b"unexpected character '&'".to_vec());
                }
            }
            b'|' => self.choice(b'|', Tok::Pipe, Tok::Or),
            c => {
                return (
                    pos,
                    Tok::Illegal,
                    format!("unexpected character '{}'", c as char).into_bytes(),
                );
            }
        };
        (pos, tok, Vec::new())
    }

    /// One escape of a string literal, after its backslash.
    fn escape(&mut self, s: &mut Vec<u8>) {
        let c = match self.ch {
            b'n' => b'\n',
            b't' => b'\t',
            b'r' => b'\r',
            b'a' => 0x07,
            b'b' => 0x08,
            b'f' => 0x0c,
            b'v' => 0x0b,
            b'\\' => b'\\',
            b'"' => b'"',
            b'/' => b'/',
            b'0'..=b'7' => {
                let mut v = 0u32;
                for _ in 0..3 {
                    if !(b'0'..=b'7').contains(&self.ch) {
                        break;
                    }
                    v = v * 8 + (self.ch - b'0') as u32;
                    self.next();
                }
                s.push(v as u8);
                return;
            }
            b'x' => {
                // gawk: up to two hex digits.
                self.next();
                let mut v = 0u32;
                let mut n = 0;
                while n < 2 && self.ch.is_ascii_hexdigit() {
                    v = v * 16 + (self.ch as char).to_digit(16).unwrap();
                    self.next();
                    n += 1;
                }
                if n == 0 {
                    s.extend_from_slice(b"\\x");
                } else {
                    s.push(v as u8);
                }
                return;
            }
            // Any other escaped character stands for itself ("\." is "."), as in gawk
            // and onetrueawk.
            c => c,
        };
        s.push(c);
        self.next();
    }

    /// The regex after a `/` (or `/=`) token, up to the closing slash.
    pub fn scan_regex(&mut self) -> Token {
        let t = self.scan_regex_inner();
        self.last_tok = t.1;
        t
    }

    fn scan_regex_inner(&mut self) -> Token {
        let mut pos = self.pos;
        let mut chars = Vec::new();
        match self.last_tok {
            Tok::Div => pos.col -= 1,
            Tok::DivAssign => {
                pos.col -= 2;
                chars.push(b'=');
            }
            _ => unreachable!("scan_regex after / or /= only"),
        }
        // A slash inside a bracket expression does not end the regex (gawk, onetrueawk).
        let mut in_bracket = false;
        let mut bracket_start = 0;
        loop {
            let c = self.ch;
            if c == 0 && self.offset > self.src.len() || c == b'\n' {
                return self.illegal("unterminated regexp");
            }
            if c == b'/' && !in_bracket {
                break;
            }
            if c == b'\\' {
                self.next();
                if self.ch == b'/' {
                    chars.push(b'/');
                } else if self.ch == b'\n' {
                    // A continued line.
                } else {
                    chars.push(b'\\');
                    chars.push(self.ch);
                }
                self.next();
                continue;
            }
            if in_bracket {
                let len = chars.len() - bracket_start;
                if c == b']' && len > 0 && !(len == 1 && chars[bracket_start] == b'^') {
                    in_bracket = false;
                } else if c == b'[' && matches!(self.peek_next(), b':' | b'.' | b'=') {
                    // A class `[:alpha:]` inside: copy it whole.
                    let close = self.peek_next();
                    chars.push(c);
                    self.next();
                    chars.push(self.ch);
                    self.next();
                    while !(self.ch == close && self.peek_next() == b']') {
                        if self.ch == 0 && self.offset > self.src.len() || self.ch == b'\n' {
                            return self.illegal("unterminated regexp");
                        }
                        chars.push(self.ch);
                        self.next();
                    }
                    chars.push(self.ch);
                    self.next();
                    chars.push(self.ch);
                    self.next();
                    continue;
                }
            } else if c == b'[' {
                in_bracket = true;
                chars.push(c);
                self.next();
                bracket_start = chars.len();
                continue;
            }
            chars.push(c);
            self.next();
        }
        self.next();
        (pos, Tok::Regex, chars)
    }

    fn peek_next(&self) -> u8 {
        self.src.get(self.offset).copied().unwrap_or(0)
    }

    /// Loads the next byte into `ch` (0 past the end, where `offset` passes the length).
    fn next(&mut self) {
        self.pos = self.next_pos;
        if self.offset >= self.src.len() {
            if self.offset == self.src.len() {
                self.ch = 0;
                self.offset += 1;
                self.next_pos.col += 1;
            }
            return;
        }
        let ch = self.src[self.offset];
        if ch == b'\n' {
            self.next_pos.line += 1;
            self.next_pos.col = 1;
        } else {
            self.next_pos.col += 1;
        }
        self.ch = ch;
        self.offset += 1;
    }

    fn choice(&mut self, ch: u8, one: Tok, two: Tok) -> Tok {
        if self.ch == ch {
            self.next();
            two
        } else {
            one
        }
    }
}

fn is_name_start(ch: u8) -> bool {
    ch == b'_' || ch.is_ascii_alphabetic()
}

/// The escapes of a `var=value` operand or `-v` assignment, as in a string literal.
pub fn unescape(s: &[u8]) -> Vec<u8> {
    let mut src = Vec::with_capacity(s.len() + 2);
    src.push(b'"');
    // A NUL inside would end the lexer's input early: handle the plain case directly.
    if !s.contains(&b'\\') {
        return s.to_vec();
    }
    for &b in s {
        // Newlines and quotes are fine in a value: escape them for the lexer.
        match b {
            b'\n' => src.extend_from_slice(b"\\n"),
            b'"' => src.extend_from_slice(b"\\\""),
            _ => src.push(b),
        }
    }
    // A trailing lone backslash stays a backslash.
    let trailing = s.iter().rev().take_while(|&&b| b == b'\\').count();
    if trailing % 2 == 1 {
        src.push(b'\\');
    }
    src.push(b'"');
    let mut l = Lexer::new(&src);
    match l.scan() {
        (_, Tok::Str, v) => v,
        _ => s.to_vec(),
    }
}
