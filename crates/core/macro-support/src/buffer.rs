use std::cell::{Cell, RefCell};
use std::fmt;
use std::str::FromStr;

use proc_macro::{Delimiter, Group, Literal, Spacing, Span, TokenStream, TokenTree};

use crate::error::{Error, Result};
use crate::ident::{Ident, Lifetime};
use crate::literal::LitStr;
use crate::punctuated::{Punctuated, Token};
use crate::tokens::Tokens;

pub struct ParseBuffer {
    trees: Vec<TokenTree>,
    index: Cell<usize>,
    end: Span,
}

pub type ParseStream<'a> = &'a ParseBuffer;

pub trait Parse: Sized {
    fn parse(input: ParseStream<'_>) -> Result<Self>;
}

impl ParseBuffer {
    pub fn new(tokens: Tokens) -> Self {
        Self::with_end(tokens, Span::call_site())
    }

    pub fn with_end(tokens: Tokens, end: Span) -> Self {
        Self {
            trees: tokens.into_trees(),
            index: Cell::new(0),
            end,
        }
    }

    pub fn from_stream(stream: TokenStream) -> Self {
        Self::new(Tokens::from_stream(stream))
    }

    pub fn from_group(group: &Group) -> Self {
        Self::with_end(Tokens::from_stream(group.stream()), group.span_close())
    }

    pub fn parse<T: Parse>(&self) -> Result<T> {
        T::parse(self)
    }

    pub fn call<T>(&self, function: fn(ParseStream<'_>) -> Result<T>) -> Result<T> {
        function(self)
    }

    pub const fn is_empty(&self) -> bool {
        self.index.get() >= self.trees.len()
    }

    pub const fn position(&self) -> usize {
        self.index.get()
    }

    pub fn reset(&self, position: usize) {
        assert!(position <= self.trees.len());
        self.index.set(position);
    }

    pub fn peek_tree(&self, offset: usize) -> Option<&TokenTree> {
        self.trees.get(self.index.get() + offset)
    }

    pub fn next_tree(&self) -> Option<TokenTree> {
        let tree = self.trees.get(self.index.get()).cloned();
        if tree.is_some() {
            self.index.set(self.index.get() + 1);
        }
        tree
    }

    pub fn parse_tree(&self) -> Result<TokenTree> {
        self.next_tree()
            .ok_or_else(|| self.error("unexpected end of input"))
    }

    pub fn span(&self) -> Span {
        self.peek_tree(0).map_or(self.end, TokenTree::span)
    }

    pub fn error(&self, message: impl fmt::Display) -> Error {
        if self.is_empty() {
            return Error::new(self.end, format!("unexpected end of input, {message}"));
        }
        Error::new(self.span(), message)
    }

    pub fn peek_punct(&self, character: char) -> bool {
        self.peek_punct_at(0, character)
    }

    pub fn peek_punct_at(&self, offset: usize, character: char) -> bool {
        matches!(self.peek_tree(offset), Some(TokenTree::Punct(punct)) if punct.as_char() == character)
    }

    pub fn peek_joint(&self, text: &str) -> bool {
        self.peek_joint_at(0, text)
    }

    pub fn peek_joint_at(&self, offset: usize, text: &str) -> bool {
        let count = text.chars().count();
        for (index, character) in text.chars().enumerate() {
            let Some(TokenTree::Punct(punct)) = self.peek_tree(offset + index) else {
                return false;
            };
            if punct.as_char() != character {
                return false;
            }
            if index + 1 < count && punct.spacing() != Spacing::Joint {
                return false;
            }
        }
        true
    }

    pub fn eat_punct(&self, character: char) -> Option<Span> {
        if !self.peek_punct(character) {
            return None;
        }
        self.next_tree().map(|tree| tree.span())
    }

    pub fn parse_punct(&self, character: char) -> Result<Span> {
        self.eat_punct(character)
            .ok_or_else(|| self.error(format!("expected `{character}`")))
    }

    pub fn eat_joint(&self, text: &str) -> Option<Span> {
        if !self.peek_joint(text) {
            return None;
        }
        let span = self.span();
        self.index.set(self.index.get() + text.chars().count());
        Some(span)
    }

    pub fn parse_joint(&self, text: &str) -> Result<Span> {
        self.eat_joint(text)
            .ok_or_else(|| self.error(format!("expected `{text}`")))
    }

    pub fn peek_path_separator(&self) -> bool {
        self.peek_joint("::")
    }

    pub fn peek_ident(&self) -> bool {
        matches!(self.peek_tree(0), Some(TokenTree::Ident(_)))
    }

    pub fn peek_keyword(&self, word: &str) -> bool {
        self.peek_keyword_at(0, word)
    }

    pub fn peek_keyword_at(&self, offset: usize, word: &str) -> bool {
        matches!(self.peek_tree(offset), Some(TokenTree::Ident(ident)) if ident.to_string() == word)
    }

    pub fn eat_keyword(&self, word: &str) -> Option<Ident> {
        if !self.peek_keyword(word) {
            return None;
        }
        match self.next_tree() {
            Some(TokenTree::Ident(ident)) => Some(Ident::from_raw(ident)),
            _ => unreachable!("peeked an identifier"),
        }
    }

    pub fn parse_keyword(&self, word: &str) -> Result<Ident> {
        self.eat_keyword(word)
            .ok_or_else(|| self.error(format!("expected `{word}`")))
    }

    pub fn parse_any_ident(&self) -> Result<Ident> {
        match self.peek_tree(0) {
            Some(TokenTree::Ident(ident)) => {
                let ident = Ident::from_raw(ident.clone());
                self.index.set(self.index.get() + 1);
                Ok(ident)
            }
            Some(TokenTree::Group(group)) if group.delimiter() == Delimiter::None => {
                let inner = Self::from_group(group);
                let ident = inner.parse_any_ident()?;
                inner.expect_end()?;
                self.index.set(self.index.get() + 1);
                Ok(ident)
            }
            _ => Err(self.error("expected identifier")),
        }
    }

    pub fn peek_lifetime(&self) -> bool {
        matches!(self.peek_tree(0), Some(TokenTree::Punct(punct)) if punct.as_char() == '\'' && punct.spacing() == Spacing::Joint)
            && matches!(self.peek_tree(1), Some(TokenTree::Ident(_)))
    }

    pub fn parse_lifetime(&self) -> Result<Lifetime> {
        if !self.peek_lifetime() {
            return Err(self.error("expected lifetime"));
        }
        let apostrophe = self.span();
        self.index.set(self.index.get() + 1);
        let ident = self.parse_any_ident()?;
        Ok(Lifetime { apostrophe, ident })
    }

    pub fn peek_group(&self, delimiter: Delimiter) -> bool {
        matches!(self.peek_tree(0), Some(TokenTree::Group(group)) if group.delimiter() == delimiter)
    }

    pub fn parse_group(&self, delimiter: Delimiter) -> Result<Group> {
        match self.peek_tree(0) {
            Some(TokenTree::Group(group)) if group.delimiter() == delimiter => {
                let group = group.clone();
                self.index.set(self.index.get() + 1);
                Ok(group)
            }
            _ => Err(self.error(match delimiter {
                Delimiter::Parenthesis => "expected parentheses",
                Delimiter::Bracket => "expected square brackets",
                Delimiter::Brace => "expected curly braces",
                Delimiter::None => "expected invisible group",
            })),
        }
    }

    pub fn peek_literal(&self) -> bool {
        match self.peek_tree(0) {
            Some(TokenTree::Literal(_)) => true,
            Some(TokenTree::Group(group)) if group.delimiter() == Delimiter::None => {
                Self::from_group(group).peek_literal()
            }
            _ => false,
        }
    }

    pub fn parse_literal(&self) -> Result<Literal> {
        match self.peek_tree(0) {
            Some(TokenTree::Literal(literal)) => {
                let literal = literal.clone();
                self.index.set(self.index.get() + 1);
                Ok(literal)
            }
            Some(TokenTree::Group(group)) if group.delimiter() == Delimiter::None => {
                let inner = Self::from_group(group);
                let literal = inner.parse_literal()?;
                inner.expect_end()?;
                self.index.set(self.index.get() + 1);
                Ok(literal)
            }
            _ => Err(self.error("expected literal")),
        }
    }

    pub fn rest(&self) -> Tokens {
        let trees = self.trees[self.index.get()..].to_vec();
        self.index.set(self.trees.len());
        Tokens::from_trees(trees)
    }

    pub fn take_until_comma(&self) -> Tokens {
        let mut trees = Vec::new();
        while let Some(tree) = self.peek_tree(0) {
            if matches!(tree, TokenTree::Punct(punct) if punct.as_char() == ',') {
                break;
            }
            trees.push(tree.clone());
            self.index.set(self.index.get() + 1);
        }
        Tokens::from_trees(trees)
    }

    pub fn expect_end(&self) -> Result<()> {
        if self.is_empty() {
            return Ok(());
        }
        Err(Error::new(self.span(), "unexpected token"))
    }

    pub const fn lookahead1(&self) -> Lookahead<'_> {
        Lookahead {
            buffer: self,
            expected: RefCell::new(Vec::new()),
        }
    }
}

pub struct Lookahead<'a> {
    buffer: &'a ParseBuffer,
    expected: RefCell<Vec<String>>,
}

impl Lookahead<'_> {
    pub fn peek_punct(&self, character: char) -> bool {
        let found = self.buffer.peek_punct(character);
        if !found {
            self.expected.borrow_mut().push(format!("`{character}`"));
        }
        found
    }

    pub fn peek_group(&self, delimiter: Delimiter) -> bool {
        let found = self.buffer.peek_group(delimiter);
        if !found {
            let name = match delimiter {
                Delimiter::Parenthesis => "parentheses",
                Delimiter::Bracket => "square brackets",
                Delimiter::Brace => "curly braces",
                Delimiter::None => "invisible group",
            };
            self.expected.borrow_mut().push(name.to_string());
        }
        found
    }

    pub fn peek_keyword(&self, word: &str) -> bool {
        let found = self.buffer.peek_keyword(word);
        if !found {
            self.expected.borrow_mut().push(format!("`{word}`"));
        }
        found
    }

    pub fn peek_lifetime(&self) -> bool {
        let found = self.buffer.peek_lifetime();
        if !found {
            self.expected.borrow_mut().push("lifetime".to_string());
        }
        found
    }

    pub fn peek_literal(&self) -> bool {
        let found = self.buffer.peek_literal();
        if !found {
            self.expected.borrow_mut().push("literal".to_string());
        }
        found
    }

    pub fn error(self) -> Error {
        let expected = self.expected.into_inner();
        match expected.as_slice() {
            [] => self.buffer.error("unexpected token"),
            [only] => self.buffer.error(format!("expected {only}")),
            [first, second] => self.buffer.error(format!("expected {first} or {second}")),
            several => self
                .buffer
                .error(format!("expected one of: {}", several.join(", "))),
        }
    }
}

pub fn parse_tokens<T: Parse>(tokens: Tokens) -> Result<T> {
    let buffer = ParseBuffer::new(tokens);
    let value = T::parse(&buffer)?;
    buffer.expect_end()?;
    Ok(value)
}

pub fn parse_quoted<T: Parse>(tokens: Tokens) -> T {
    match parse_tokens(tokens) {
        Ok(value) => value,
        Err(error) => panic!("parse_quote! produced tokens that do not parse: {error}"),
    }
}

pub fn parse_stream<T: Parse>(stream: TokenStream) -> Result<T> {
    parse_tokens(Tokens::from_stream(stream))
}

pub fn parse_terminated<T, P: Token>(
    input: ParseStream<'_>,
    parser: fn(ParseStream<'_>) -> Result<T>,
) -> Result<Punctuated<T, P>> {
    let mut punctuated = Punctuated::new();
    loop {
        if input.is_empty() {
            break;
        }
        punctuated.push_value(parser(input)?);
        if input.is_empty() {
            break;
        }
        punctuated.push_punct(P::parse_token(input)?);
    }
    Ok(punctuated)
}

pub fn respan(tokens: Tokens, span: Span) -> Tokens {
    tokens
        .into_iter()
        .map(|tree| respan_tree(tree, span))
        .collect()
}

fn respan_tree(tree: TokenTree, span: Span) -> TokenTree {
    match tree {
        TokenTree::Group(group) => {
            let stream = respan(Tokens::from_stream(group.stream()), span).into_stream();
            let mut respanned = Group::new(group.delimiter(), stream);
            respanned.set_span(span);
            TokenTree::Group(respanned)
        }
        mut other => {
            other.set_span(span);
            other
        }
    }
}

pub fn lex_string(literal: &LitStr) -> Result<Tokens> {
    let stream = TokenStream::from_str(&literal.value())
        .map_err(|error| Error::new(literal.span(), format!("lex error: {error}")))?;
    Ok(respan(Tokens::from_stream(stream), literal.span()))
}

pub fn parse_string<T: Parse>(literal: &LitStr) -> Result<T> {
    parse_tokens(lex_string(literal)?)
}
