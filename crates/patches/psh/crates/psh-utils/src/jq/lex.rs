use super::parse::NESTING_DEPTH_MAX;

#[derive(Debug)]
pub enum StrPart<'s, T> {
    Str(&'s str),
    Term(T),
    Char(char),
}

#[derive(Debug)]
pub struct Token<'s> {
    pub text: &'s str,
    pub kind: Kind<'s>,
}

#[derive(Debug)]
pub enum Kind<'s> {
    Word,
    Var,
    Fmt,
    Num,
    Str(Vec<StrPart<'s, Token<'s>>>),
    Sym,
    Block(Vec<Token<'s>>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expect {
    Character,
    Delim,
    Depth,
}

pub type Error<'s> = (Expect, &'s str);

pub struct Lexer<'s> {
    rest: &'s str,
    errors: Vec<Error<'s>>,
    depth: usize,
}

const OPERATOR_HEAD: &str = "|=!<>+-*/%";

fn operator_head(c: char) -> bool {
    OPERATOR_HEAD.contains(c)
}

fn operator_tail(c: char) -> bool {
    operator_head(c) && c != '-'
}

fn ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

impl<'s> Lexer<'s> {
    pub fn new(code: &'s str) -> Self {
        Lexer {
            rest: code,
            errors: Vec::new(),
            depth: 0,
        }
    }

    pub fn lex(mut self) -> Result<Vec<Token<'s>>, Vec<Error<'s>>> {
        let tokens = self.tokens();
        self.space();
        if !self.rest.is_empty() {
            self.errors.push((Expect::Character, self.rest));
        }
        if let Some(deep) = self.errors.iter().find(|(e, _)| *e == Expect::Depth) {
            return Err(Vec::from([*deep]));
        }
        if self.errors.is_empty() {
            Ok(tokens)
        } else {
            Err(self.errors)
        }
    }

    fn take(&mut self, bytes: usize) -> &'s str {
        let (head, tail) = self.rest.split_at(bytes);
        self.rest = tail;
        head
    }

    fn since(&self, start: &'s str) -> &'s str {
        &start[..start.len() - self.rest.len()]
    }

    fn space(&mut self) {
        loop {
            self.rest = self.rest.trim_start();
            match self.rest.strip_prefix('#') {
                Some(comment) => self.rest = comment,
                None => return,
            }
            loop {
                let (line, after) = match self.rest.split_once('\n') {
                    Some(split) => split,
                    None => (self.rest, &self.rest[self.rest.len()..]),
                };
                let line = line.strip_suffix('\r').unwrap_or(line);
                self.rest = after;
                let backslashes = line.chars().rev().take_while(|c| *c == '\\').count();
                if backslashes % 2 == 0 {
                    break;
                }
            }
        }
    }

    fn skip_ident_chars(&mut self) {
        self.rest = self.rest.trim_start_matches(ident_char);
    }

    fn ident(&mut self) {
        match self.rest.strip_prefix(ident_start) {
            Some(after) => {
                self.rest = after;
                self.skip_ident_chars();
            }
            None => self.errors.push((Expect::Character, self.rest)),
        }
    }

    fn word(&mut self) {
        self.skip_ident_chars();
        if let Some(after) = self.rest.strip_prefix("::") {
            self.rest = after.strip_prefix(['@', '$']).unwrap_or(after);
            self.ident();
        }
    }

    fn digits(&mut self) {
        match self.rest.strip_prefix(|c: char| c.is_ascii_digit()) {
            Some(after) => self.rest = after.trim_start_matches(|c: char| c.is_ascii_digit()),
            None => self.errors.push((Expect::Character, self.rest)),
        }
    }

    fn number(&mut self) {
        self.rest = self.rest.trim_start_matches(|c: char| c.is_ascii_digit());
        if let Some(after) = self.rest.strip_prefix('.') {
            self.rest = after;
            self.digits();
        }
        if let Some(after) = self.rest.strip_prefix(['e', 'E']) {
            self.rest = after.strip_prefix(['+', '-']).unwrap_or(after);
            self.digits();
        }
    }

    fn unicode(&mut self, after_u: &'s str) -> Option<char> {
        let mut chars = after_u.chars();
        let mut code = 0;
        for _ in 0..4 {
            let at = chars.as_str();
            match chars.next().and_then(|c| c.to_digit(16)) {
                Some(digit) => code = (code << 4) + digit,
                None => {
                    self.rest = at;
                    self.errors.push((Expect::Character, at));
                    return None;
                }
            }
        }
        match char::from_u32(code) {
            Some(c) => {
                self.rest = chars.as_str();
                Some(c)
            }
            None => {
                self.rest = after_u;
                self.errors.push((Expect::Character, after_u));
                None
            }
        }
    }

    fn escape(&mut self) -> Option<StrPart<'s, Token<'s>>> {
        let mut chars = self.rest.chars();
        let c = match chars.next() {
            Some(c @ ('\\' | '/' | '"')) => c,
            Some('b') => '\u{8}',
            Some('f') => '\u{c}',
            Some('n') => '\n',
            Some('r') => '\r',
            Some('t') => '\t',
            Some('u') => return self.unicode(chars.as_str()).map(StrPart::Char),
            Some('(') => {
                let start = self.rest;
                let kind = self.block();
                let text = self.since(start);
                return Some(StrPart::Term(Token { text, kind }));
            }
            Some(_) | None => {
                self.errors.push((Expect::Character, self.rest));
                return None;
            }
        };
        self.rest = chars.as_str();
        Some(StrPart::Char(c))
    }

    fn string(&mut self) -> Kind<'s> {
        let quote = self.take(1);
        assert_eq!(quote, "\"");
        let mut parts = Vec::new();
        loop {
            let start = self.rest;
            self.rest = self.rest.trim_start_matches(|c| c != '\\' && c != '"');
            let text = self.since(start);
            if !text.is_empty() {
                parts.push(StrPart::Str(text));
            }
            let mut chars = self.rest.chars();
            match chars.next() {
                Some('"') => {
                    self.rest = chars.as_str();
                    break;
                }
                Some('\\') => {
                    self.rest = chars.as_str();
                    if let Some(part) = self.escape() {
                        parts.push(part);
                    }
                }
                Some(other) => {
                    unreachable!("the scan stops only at a quote or a backslash, not {other:?}")
                }
                None => {
                    self.errors.push((Expect::Delim, self.rest));
                    break;
                }
            }
        }
        Kind::Str(parts)
    }

    fn token(&mut self) -> Option<Token<'s>> {
        self.space();
        let start = self.rest;
        let mut chars = self.rest.chars();
        let kind = match chars.next()? {
            c if ident_start(c) => {
                self.take(1);
                self.word();
                Kind::Word
            }
            '$' => {
                self.take(1);
                self.ident();
                Kind::Var
            }
            '@' => {
                self.take(1);
                self.ident();
                Kind::Fmt
            }
            '0'..='9' => {
                self.take(1);
                self.number();
                Kind::Num
            }
            c if operator_head(c) => {
                self.take(1);
                self.rest = self.rest.trim_start_matches(operator_tail);
                Kind::Sym
            }
            '.' => {
                match chars.next() {
                    Some('.') => {
                        self.take(2);
                    }
                    Some(c) if ident_start(c) => {
                        self.take(2);
                        self.skip_ident_chars();
                    }
                    _ => {
                        self.take(1);
                    }
                }
                Kind::Sym
            }
            ':' | ';' | ',' | '?' => {
                self.take(1);
                Kind::Sym
            }
            '"' => self.string(),
            '(' | '[' | '{' => self.block(),
            _ => return None,
        };
        Some(Token {
            text: self.since(start),
            kind,
        })
    }

    fn tokens(&mut self) -> Vec<Token<'s>> {
        let mut tokens = Vec::new();
        while let Some(token) = self.token() {
            tokens.push(token);
        }
        tokens
    }

    fn block(&mut self) -> Kind<'s> {
        let open = self.take(1);
        let close = match open {
            "(" => ')',
            "[" => ']',
            "{" => '}',
            other => unreachable!("a block opens with a bracket, not {other:?}"),
        };
        if self.depth == NESTING_DEPTH_MAX {
            self.errors.push((Expect::Depth, open));
            self.rest = &self.rest[self.rest.len()..];
            return Kind::Block(Vec::new());
        }
        self.depth += 1;
        let mut tokens = self.tokens();
        self.depth -= 1;
        self.space();
        if let Some(after) = self.rest.strip_prefix(close) {
            tokens.push(Token {
                text: &self.rest[..1],
                kind: Kind::Sym,
            });
            self.rest = after;
        } else {
            self.errors.push((Expect::Delim, self.rest));
        }
        Kind::Block(tokens)
    }
}
