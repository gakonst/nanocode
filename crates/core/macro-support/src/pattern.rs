use proc_macro::{Delimiter, Span, TokenTree};

use crate::attribute::Attribute;
use crate::buffer::{ParseBuffer, ParseStream};
use crate::error::Result;
use crate::ident::Ident;
use crate::path::{Path, QSelf, parse_qualified_path, print_path};
use crate::punctuated::{And, Comma, Punctuated, Token};
use crate::tokens::{ToTokens, Tokens, push_group, push_punct};

#[derive(Clone)]
pub enum Pat {
    Ident(PatIdent),
    Paren(PatParen),
    Reference(PatReference),
    TupleStruct(PatTupleStruct),
    Wild(PatWild),
    Verbatim(Tokens),
}

#[derive(Clone)]
pub struct PatIdent {
    pub attrs: Vec<Attribute>,
    pub by_ref: Option<Ident>,
    pub mutability: Option<Ident>,
    pub ident: Ident,
    pub subpat: Option<(Span, Box<Pat>)>,
}

#[derive(Clone)]
pub struct PatParen {
    pub attrs: Vec<Attribute>,
    pub paren_span: Span,
    pub pat: Box<Pat>,
}

#[derive(Clone)]
pub struct PatReference {
    pub attrs: Vec<Attribute>,
    pub and_token: And,
    pub mutability: Option<Ident>,
    pub pat: Box<Pat>,
}

#[derive(Clone)]
pub struct PatTupleStruct {
    pub attrs: Vec<Attribute>,
    pub qself: Option<QSelf>,
    pub path: Path,
    pub paren_span: Span,
    pub elems: Punctuated<Pat, Comma>,
}

#[derive(Clone)]
pub struct PatWild {
    pub attrs: Vec<Attribute>,
    pub underscore_span: Span,
}

impl Pat {
    pub fn from_tokens(tokens: Tokens) -> Self {
        let buffer = ParseBuffer::new(tokens.clone());
        match parse_pattern(&buffer) {
            Ok(pattern) if buffer.is_empty() => pattern,
            _ => Self::Verbatim(tokens),
        }
    }
}

fn binds_identifier(input: ParseStream<'_>) -> bool {
    if input.peek_keyword("ref") || input.peek_keyword("mut") {
        return true;
    }
    if !input.peek_ident() {
        return false;
    }
    let continues_path = input.peek_joint_at(1, "::");
    let continues_call = matches!(input.peek_tree(1), Some(TokenTree::Group(group)) if group.delimiter() != Delimiter::None);
    let continues_macro = input.peek_punct_at(1, '!');
    !continues_path && !continues_call && !continues_macro
}

fn parse_pattern(input: ParseStream<'_>) -> Result<Pat> {
    if input.peek_keyword("_") {
        let underscore_span = input.parse_any_ident()?.span();
        return Ok(Pat::Wild(PatWild {
            attrs: Vec::new(),
            underscore_span,
        }));
    }
    if input.peek_punct('&') {
        let and_token = And::parse_token(input)?;
        let mutability = input.eat_keyword("mut");
        let pat = Box::new(parse_pattern(input)?);
        return Ok(Pat::Reference(PatReference {
            attrs: Vec::new(),
            and_token,
            mutability,
            pat,
        }));
    }
    if input.peek_group(Delimiter::Parenthesis) {
        let group = input.parse_group(Delimiter::Parenthesis)?;
        let content = ParseBuffer::from_group(&group);
        let pat = parse_pattern(&content)?;
        content.expect_end()?;
        return Ok(Pat::Paren(PatParen {
            attrs: Vec::new(),
            paren_span: group.span(),
            pat: Box::new(pat),
        }));
    }
    if binds_identifier(input) {
        let by_ref = input.eat_keyword("ref");
        let mutability = input.eat_keyword("mut");
        let ident = input.parse_any_ident()?;
        let subpat = match input.eat_punct('@') {
            Some(span) => Some((span, Box::new(parse_pattern(input)?))),
            None => None,
        };
        return Ok(Pat::Ident(PatIdent {
            attrs: Vec::new(),
            by_ref,
            mutability,
            ident,
            subpat,
        }));
    }
    let (qself, path) = parse_qualified_path(input, true)?;
    let group = input.parse_group(Delimiter::Parenthesis)?;
    let content = ParseBuffer::from_group(&group);
    let mut elems = Punctuated::new();
    while !content.is_empty() {
        elems.push_value(Pat::from_tokens(content.take_until_comma()));
        if content.is_empty() {
            break;
        }
        elems.push_punct(Comma::parse_token(&content)?);
    }
    Ok(Pat::TupleStruct(PatTupleStruct {
        attrs: Vec::new(),
        qself,
        path,
        paren_span: group.span(),
        elems,
    }))
}

fn print_attributes(attrs: &[Attribute], tokens: &mut Tokens) {
    for attribute in attrs {
        attribute.to_tokens(tokens);
    }
}

impl ToTokens for Pat {
    fn to_tokens(&self, tokens: &mut Tokens) {
        match self {
            Self::Ident(pattern) => pattern.to_tokens(tokens),
            Self::Paren(pattern) => pattern.to_tokens(tokens),
            Self::Reference(pattern) => pattern.to_tokens(tokens),
            Self::TupleStruct(pattern) => pattern.to_tokens(tokens),
            Self::Wild(pattern) => pattern.to_tokens(tokens),
            Self::Verbatim(verbatim) => verbatim.to_tokens(tokens),
        }
    }
}

impl ToTokens for PatIdent {
    fn to_tokens(&self, tokens: &mut Tokens) {
        print_attributes(&self.attrs, tokens);
        self.by_ref.to_tokens(tokens);
        self.mutability.to_tokens(tokens);
        self.ident.to_tokens(tokens);
        if let Some((at_span, subpattern)) = &self.subpat {
            push_punct(tokens, *at_span, "@");
            subpattern.to_tokens(tokens);
        }
    }
}

impl ToTokens for PatParen {
    fn to_tokens(&self, tokens: &mut Tokens) {
        print_attributes(&self.attrs, tokens);
        push_group(
            tokens,
            self.paren_span,
            Delimiter::Parenthesis,
            self.pat.to_token_stream(),
        );
    }
}

impl ToTokens for PatReference {
    fn to_tokens(&self, tokens: &mut Tokens) {
        print_attributes(&self.attrs, tokens);
        self.and_token.to_tokens(tokens);
        self.mutability.to_tokens(tokens);
        self.pat.to_tokens(tokens);
    }
}

impl ToTokens for PatTupleStruct {
    fn to_tokens(&self, tokens: &mut Tokens) {
        print_attributes(&self.attrs, tokens);
        print_path(tokens, self.qself.as_ref(), &self.path);
        push_group(
            tokens,
            self.paren_span,
            Delimiter::Parenthesis,
            self.elems.to_token_stream(),
        );
    }
}

impl ToTokens for PatWild {
    fn to_tokens(&self, tokens: &mut Tokens) {
        print_attributes(&self.attrs, tokens);
        tokens.push(proc_macro::Ident::new("_", self.underscore_span));
    }
}
