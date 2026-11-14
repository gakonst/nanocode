use std::fmt;

use proc_macro::{Ident as RawIdent, Literal, Span, TokenTree};

use crate::buffer::{Parse, ParseStream, parse_string};
use crate::error::{Error, Result};
use crate::tokens::{ToTokens, Tokens};

#[derive(Clone)]
pub struct LitStr {
    token: Literal,
    value: String,
    suffix: String,
}

impl LitStr {
    pub fn new(value: &str, span: Span) -> Self {
        let mut token = Literal::string(value);
        token.set_span(span);
        Self {
            token,
            value: value.to_string(),
            suffix: String::new(),
        }
    }

    pub fn from_literal(token: Literal) -> Option<Self> {
        let (value, suffix) = parse_string_literal(&token.to_string())?;
        Some(Self {
            token,
            value,
            suffix,
        })
    }

    pub fn value(&self) -> String {
        self.value.clone()
    }

    pub fn as_str(&self) -> &str {
        &self.value
    }

    pub fn suffix(&self) -> &str {
        &self.suffix
    }

    pub fn span(&self) -> Span {
        self.token.span()
    }

    pub fn set_span(&mut self, span: Span) {
        self.token.set_span(span);
    }

    pub const fn token(&self) -> &Literal {
        &self.token
    }

    pub fn parse<T: Parse>(&self) -> Result<T> {
        parse_string(self)
    }

    pub fn parse_with<T>(&self, parser: impl FnOnce(ParseStream<'_>) -> Result<T>) -> Result<T> {
        let buffer = crate::buffer::ParseBuffer::new(crate::buffer::lex_string(self)?);
        let value = parser(&buffer)?;
        buffer.expect_end()?;
        Ok(value)
    }
}

impl fmt::Debug for LitStr {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "LitStr({:?})", self.value)
    }
}

impl ToTokens for LitStr {
    fn to_tokens(&self, tokens: &mut Tokens) {
        tokens.push(self.token.clone());
    }
}

impl Parse for LitStr {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let span = input.span();
        let literal = input
            .parse_literal()
            .map_err(|_| Error::new(span, "expected string literal"))?;
        Self::from_literal(literal).ok_or_else(|| Error::new(span, "expected string literal"))
    }
}

#[derive(Clone)]
pub struct LitInt {
    token: Literal,
    digits: String,
    suffix: String,
}

impl LitInt {
    pub fn from_literal(token: Literal) -> Option<Self> {
        let (digits, suffix) = parse_integer_literal(&token.to_string())?;
        Some(Self {
            token,
            digits,
            suffix,
        })
    }

    pub fn base10_digits(&self) -> &str {
        &self.digits
    }

    pub fn base10_parse<N: std::str::FromStr>(&self) -> Result<N>
    where
        N::Err: fmt::Display,
    {
        self.digits
            .parse()
            .map_err(|error| Error::new(self.span(), error))
    }

    pub fn suffix(&self) -> &str {
        &self.suffix
    }

    pub fn span(&self) -> Span {
        self.token.span()
    }

    pub const fn token(&self) -> &Literal {
        &self.token
    }
}

impl ToTokens for LitInt {
    fn to_tokens(&self, tokens: &mut Tokens) {
        tokens.push(self.token.clone());
    }
}

impl Parse for LitInt {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let span = input.span();
        let literal = input
            .parse_literal()
            .map_err(|_| Error::new(span, "expected integer literal"))?;
        Self::from_literal(literal).ok_or_else(|| Error::new(span, "expected integer literal"))
    }
}

#[derive(Clone)]
pub struct LitBool {
    pub value: bool,
    pub span: Span,
}

impl LitBool {
    pub const fn value(&self) -> bool {
        self.value
    }
}

impl ToTokens for LitBool {
    fn to_tokens(&self, tokens: &mut Tokens) {
        let word = if self.value { "true" } else { "false" };
        tokens.push(RawIdent::new(word, self.span));
    }
}

#[derive(Clone)]
pub enum Lit {
    Str(LitStr),
    Int(LitInt),
    Bool(LitBool),
    Other(Literal),
}

impl Lit {
    pub fn from_literal(token: Literal) -> Self {
        let text = token.to_string();
        if (text.starts_with('"') || text.starts_with("r\"") || text.starts_with("r#"))
            && let Some(string) = LitStr::from_literal(token.clone())
        {
            return Self::Str(string);
        }
        if let Some(integer) = LitInt::from_literal(token.clone()) {
            return Self::Int(integer);
        }
        Self::Other(token)
    }

    pub fn span(&self) -> Span {
        match self {
            Self::Str(string) => string.span(),
            Self::Int(integer) => integer.span(),
            Self::Bool(boolean) => boolean.span,
            Self::Other(literal) => literal.span(),
        }
    }
}

impl ToTokens for Lit {
    fn to_tokens(&self, tokens: &mut Tokens) {
        match self {
            Self::Str(string) => string.to_tokens(tokens),
            Self::Int(integer) => integer.to_tokens(tokens),
            Self::Bool(boolean) => boolean.to_tokens(tokens),
            Self::Other(literal) => literal.to_tokens(tokens),
        }
    }
}

impl Parse for Lit {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        if let Some(TokenTree::Ident(ident)) = input.peek_tree(0) {
            let word = ident.to_string();
            if word == "true" || word == "false" {
                let span = ident.span();
                input.next_tree();
                return Ok(Self::Bool(LitBool {
                    value: word == "true",
                    span,
                }));
            }
        }
        let span = input.span();
        let literal = input
            .parse_literal()
            .map_err(|_| Error::new(span, "expected literal"))?;
        Ok(Self::from_literal(literal))
    }
}

fn split_suffix(rest: &str) -> Option<String> {
    let valid = rest
        .chars()
        .all(|character| character.is_alphanumeric() || character == '_');
    valid.then(|| rest.to_string())
}

fn skip_continuation_whitespace(characters: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while let Some(character) = characters.peek() {
        if !matches!(character, ' ' | '\t' | '\n' | '\r') {
            break;
        }
        characters.next();
    }
}

pub fn parse_string_literal(text: &str) -> Option<(String, String)> {
    if let Some(raw) = text.strip_prefix('r') {
        let hashes = raw
            .chars()
            .take_while(|character| *character == '#')
            .count();
        let body = raw[hashes..].strip_prefix('"')?;
        let closing = format!("\"{}", "#".repeat(hashes));
        let end = body.find(&closing)?;
        let suffix = split_suffix(&body[end + closing.len()..])?;
        return Some((body[..end].to_string(), suffix));
    }
    let body = text.strip_prefix('"')?;
    let mut value = String::with_capacity(body.len());
    let mut characters = body.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '"' => {
                let rest: String = characters.collect();
                let suffix = split_suffix(&rest)?;
                return Some((value, suffix));
            }
            '\\' => match characters.next()? {
                'n' => value.push('\n'),
                'r' => value.push('\r'),
                't' => value.push('\t'),
                '\\' => value.push('\\'),
                '0' => value.push('\0'),
                '\'' => value.push('\''),
                '"' => value.push('"'),
                'x' => {
                    let high = characters.next()?.to_digit(16)?;
                    let low = characters.next()?.to_digit(16)?;
                    let byte = high * 16 + low;
                    if byte > 0x7F {
                        return None;
                    }
                    value.push(char::from_u32(byte)?);
                }
                'u' => {
                    if characters.next()? != '{' {
                        return None;
                    }
                    let mut code = 0u32;
                    loop {
                        let digit = characters.next()?;
                        if digit == '}' {
                            break;
                        }
                        if digit == '_' {
                            continue;
                        }
                        code = code.checked_mul(16)?.checked_add(digit.to_digit(16)?)?;
                    }
                    value.push(char::from_u32(code)?);
                }
                '\n' => skip_continuation_whitespace(&mut characters),
                '\r' => {
                    if characters.next()? != '\n' {
                        return None;
                    }
                    skip_continuation_whitespace(&mut characters);
                }
                _ => return None,
            },
            '\r' => {
                if characters.next()? != '\n' {
                    return None;
                }
                value.push('\n');
            }
            other => value.push(other),
        }
    }
    None
}

const INTEGER_SUFFIXES: [&str; 12] = [
    "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize",
];

fn parse_integer_literal(text: &str) -> Option<(String, String)> {
    let (radix, body) = if let Some(rest) = text.strip_prefix("0x") {
        (16, rest)
    } else if let Some(rest) = text.strip_prefix("0o") {
        (8, rest)
    } else if let Some(rest) = text.strip_prefix("0b") {
        (2, rest)
    } else {
        (10, text)
    };
    let mut suffix = "";
    let mut digits_text = body;
    for candidate in INTEGER_SUFFIXES {
        if let Some(stripped) = body.strip_suffix(candidate) {
            suffix = candidate;
            digits_text = stripped;
            break;
        }
    }
    let mut value: u128 = 0;
    let mut any = false;
    for character in digits_text.chars() {
        if character == '_' {
            continue;
        }
        let digit = character.to_digit(radix)?;
        value = value
            .checked_mul(u128::from(radix))?
            .checked_add(u128::from(digit))?;
        any = true;
    }
    if !any {
        return None;
    }
    Some((value.to_string(), suffix.to_string()))
}
