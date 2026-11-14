use std::fmt;

use proc_macro::{Delimiter, Span, TokenTree};

use crate::buffer::{Parse, ParseBuffer, ParseStream};
use crate::error::{Error, Result};
use crate::literal::Lit;
use crate::path::{Path, PathSegment, parse_optional};
use crate::punctuated::{Comma, Eq, PathSep, Punctuated, Token};
use crate::tokens::{ToTokens, Tokens, push_group, push_punct};

#[derive(Clone, Copy)]
pub enum AttrStyle {
    Outer,
    Inner(Span),
}

#[derive(Clone)]
pub struct Attribute {
    pub pound_span: Span,
    pub style: AttrStyle,
    pub bracket_span: Span,
    pub meta: Meta,
}

#[derive(Clone)]
pub enum Meta {
    Path(Path),
    List(MetaList),
    NameValue(MetaNameValue),
}

#[derive(Clone)]
pub struct MetaList {
    pub path: Path,
    pub delimiter: Delimiter,
    pub delimiter_span: Span,
    pub tokens: Tokens,
}

#[derive(Clone)]
pub struct MetaNameValue {
    pub path: Path,
    pub eq_token: Eq,
    pub value: Tokens,
}

impl Attribute {
    pub fn parse_outer(input: ParseStream<'_>) -> Result<Vec<Self>> {
        let mut attributes = Vec::new();
        while input.peek_punct('#') && !input.peek_punct_at(1, '!') {
            let pound_span = input.parse_punct('#')?;
            attributes.push(Self::parse_body(input, pound_span, AttrStyle::Outer)?);
        }
        Ok(attributes)
    }

    pub fn parse_inner(input: ParseStream<'_>) -> Result<Vec<Self>> {
        let mut attributes = Vec::new();
        while input.peek_punct('#') && input.peek_punct_at(1, '!') {
            let pound_span = input.parse_punct('#')?;
            let bang_span = input.parse_punct('!')?;
            attributes.push(Self::parse_body(
                input,
                pound_span,
                AttrStyle::Inner(bang_span),
            )?);
        }
        Ok(attributes)
    }

    fn parse_body(input: ParseStream<'_>, pound_span: Span, style: AttrStyle) -> Result<Self> {
        let group = input.parse_group(Delimiter::Bracket)?;
        let content = ParseBuffer::from_group(&group);
        let meta = Meta::parse(&content)?;
        content.expect_end()?;
        Ok(Self {
            pound_span,
            style,
            bracket_span: group.span(),
            meta,
        })
    }

    pub const fn path(&self) -> &Path {
        self.meta.path()
    }

    pub fn parse_args<T: Parse>(&self) -> Result<T> {
        let list = self.meta.require_list()?;
        list.parse_args()
    }

    pub fn parse_args_with<T>(
        &self,
        parser: impl FnOnce(ParseStream<'_>) -> Result<T>,
    ) -> Result<T> {
        let list = self.meta.require_list()?;
        list.parse_args_with(parser)
    }

    pub fn parse_nested_meta(
        &self,
        callback: impl FnMut(NestedMeta<'_>) -> Result<()>,
    ) -> Result<()> {
        let list = self.meta.require_list()?;
        if list.delimiter != Delimiter::Parenthesis {
            return Err(Error::new(list.delimiter_span, "expected parentheses"));
        }
        list.parse_nested_meta(callback)
    }
}

impl ToTokens for Attribute {
    fn to_tokens(&self, tokens: &mut Tokens) {
        push_punct(tokens, self.pound_span, "#");
        if let AttrStyle::Inner(span) = self.style {
            push_punct(tokens, span, "!");
        }
        push_group(
            tokens,
            self.bracket_span,
            Delimiter::Bracket,
            self.meta.to_token_stream(),
        );
    }
}

impl Meta {
    pub const fn path(&self) -> &Path {
        match self {
            Self::Path(path) => path,
            Self::List(list) => &list.path,
            Self::NameValue(name_value) => &name_value.path,
        }
    }

    pub fn require_list(&self) -> Result<&MetaList> {
        match self {
            Self::List(list) => Ok(list),
            Self::Path(path) => Err(Error::new_spanned(
                path,
                format!(
                    "expected attribute arguments in parentheses: `{}(...)`",
                    path_text(path)
                ),
            )),
            Self::NameValue(name_value) => {
                Err(Error::new(name_value.eq_token.span, "expected `(`"))
            }
        }
    }

    pub fn require_name_value(&self) -> Result<&MetaNameValue> {
        match self {
            Self::NameValue(name_value) => Ok(name_value),
            Self::Path(path) => Err(Error::new_spanned(
                path,
                format!(
                    "expected a value for this attribute: `{} = ...`",
                    path_text(path)
                ),
            )),
            Self::List(list) => Err(Error::new(list.delimiter_span, "expected `=`")),
        }
    }

    pub fn require_path_only(&self) -> Result<&Path> {
        match self {
            Self::Path(path) => Ok(path),
            Self::List(list) => Err(Error::new(
                list.delimiter_span,
                "unexpected token in attribute",
            )),
            Self::NameValue(name_value) => Err(Error::new(
                name_value.eq_token.span,
                "unexpected token in attribute",
            )),
        }
    }
}

fn path_text(path: &Path) -> String {
    path.to_token_stream().to_string().replace(' ', "")
}

impl Parse for Meta {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let path = parse_meta_path(input)?;
        Self::parse_after_path(path, input)
    }
}

impl Meta {
    pub fn parse_after_path(path: Path, input: ParseStream<'_>) -> Result<Self> {
        if let Some(TokenTree::Group(group)) = input.peek_tree(0)
            && group.delimiter() != Delimiter::None
        {
            let group = group.clone();
            input.next_tree();
            return Ok(Self::List(MetaList {
                path,
                delimiter: group.delimiter(),
                delimiter_span: group.span(),
                tokens: Tokens::from_stream(group.stream()),
            }));
        }
        if input.peek_punct('=') {
            let eq_token = Eq::parse_token(input)?;
            let value = input.take_until_comma();
            if value.is_empty() {
                return Err(input.error("expected an expression"));
            }
            return Ok(Self::NameValue(MetaNameValue {
                path,
                eq_token,
                value,
            }));
        }
        Ok(Self::Path(path))
    }
}

impl ToTokens for Meta {
    fn to_tokens(&self, tokens: &mut Tokens) {
        match self {
            Self::Path(path) => path.to_tokens(tokens),
            Self::List(list) => list.to_tokens(tokens),
            Self::NameValue(name_value) => name_value.to_tokens(tokens),
        }
    }
}

impl MetaList {
    pub fn parse_args<T: Parse>(&self) -> Result<T> {
        let buffer = ParseBuffer::with_end(self.tokens.clone(), self.delimiter_span);
        let value = T::parse(&buffer)?;
        buffer.expect_end()?;
        Ok(value)
    }

    pub fn parse_args_with<T>(
        &self,
        parser: impl FnOnce(ParseStream<'_>) -> Result<T>,
    ) -> Result<T> {
        let buffer = ParseBuffer::with_end(self.tokens.clone(), self.delimiter_span);
        let value = parser(&buffer)?;
        buffer.expect_end()?;
        Ok(value)
    }

    pub fn parse_nested_meta(
        &self,
        callback: impl FnMut(NestedMeta<'_>) -> Result<()>,
    ) -> Result<()> {
        let buffer = ParseBuffer::with_end(self.tokens.clone(), self.delimiter_span);
        parse_nested_meta(&buffer, callback)
    }
}

impl ToTokens for MetaList {
    fn to_tokens(&self, tokens: &mut Tokens) {
        self.path.to_tokens(tokens);
        push_group(
            tokens,
            self.delimiter_span,
            self.delimiter,
            self.tokens.clone(),
        );
    }
}

impl MetaNameValue {
    pub fn value_lit(&self) -> Option<Lit> {
        let buffer = ParseBuffer::new(self.value.clone());
        let literal = Lit::parse(&buffer).ok()?;
        buffer.is_empty().then_some(literal)
    }
}

impl ToTokens for MetaNameValue {
    fn to_tokens(&self, tokens: &mut Tokens) {
        self.path.to_tokens(tokens);
        self.eq_token.to_tokens(tokens);
        self.value.to_tokens(tokens);
    }
}

pub fn parse_meta_path(input: ParseStream<'_>) -> Result<Path> {
    let leading_colon = parse_optional::<PathSep>(input)?;
    let mut segments = Punctuated::new();
    if input.peek_literal() {
        return Err(input.error("unexpected literal in nested attribute, expected ident"));
    }
    segments.push_value(PathSegment::from(input.parse_any_ident()?));
    while input.peek_path_separator() {
        segments.push_punct(PathSep::parse_token(input)?);
        segments.push_value(PathSegment::from(input.parse_any_ident()?));
    }
    Ok(Path {
        leading_colon,
        segments,
    })
}

pub struct NestedMeta<'a> {
    pub path: Path,
    pub input: ParseStream<'a>,
}

impl<'a> NestedMeta<'a> {
    pub fn value(&self) -> Result<ParseStream<'a>> {
        if self.input.is_empty() || self.input.peek_punct(',') {
            return Err(self.input.error("expected `=`"));
        }
        self.input.parse_punct('=')?;
        Ok(self.input)
    }

    pub fn parse_nested_meta(
        &self,
        callback: impl FnMut(NestedMeta<'_>) -> Result<()>,
    ) -> Result<()> {
        let group = self.input.parse_group(Delimiter::Parenthesis)?;
        parse_nested_meta(&ParseBuffer::from_group(&group), callback)
    }

    pub fn error(&self, message: impl fmt::Display) -> Error {
        Error::new_spanned(&self.path, message)
    }
}

pub fn parse_nested_meta(
    input: ParseStream<'_>,
    mut callback: impl FnMut(NestedMeta<'_>) -> Result<()>,
) -> Result<()> {
    loop {
        let path = parse_meta_path(input)?;
        callback(NestedMeta { path, input })?;
        if input.is_empty() {
            return Ok(());
        }
        Comma::parse_token(input)?;
        if input.is_empty() {
            return Ok(());
        }
    }
}
