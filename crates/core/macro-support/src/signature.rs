use proc_macro::{Delimiter, Span, TokenTree};

use crate::attribute::{AttrStyle, Attribute};
use crate::buffer::{Parse, ParseBuffer, ParseStream};
use crate::error::Result;
use crate::generics::{Generics, WhereClause};
use crate::ident::{Ident, Lifetime};
use crate::path::ReturnType;
use crate::pattern::Pat;
use crate::punctuated::{And, Colon, Comma, Punctuated, Token};
use crate::tokens::{ToTokens, Tokens, push_group};
use crate::ty::{Type, parse_type};

#[derive(Clone)]
pub struct Signature {
    pub constness: Option<Ident>,
    pub asyncness: Option<Ident>,
    pub unsafety: Option<Ident>,
    pub abi: Option<Tokens>,
    pub fn_token: Ident,
    pub ident: Ident,
    pub generics: Generics,
    pub paren_span: Span,
    pub inputs: Punctuated<FnArg, Comma>,
    pub variadic: Option<Tokens>,
    pub output: ReturnType,
}

#[derive(Clone)]
pub enum FnArg {
    Receiver(Receiver),
    Typed(PatType),
}

#[derive(Clone)]
pub struct Receiver {
    pub attrs: Vec<Attribute>,
    pub reference: Option<(And, Option<Lifetime>)>,
    pub mutability: Option<Ident>,
    pub self_token: Ident,
    pub colon_token: Option<Colon>,
    pub ty: Option<Box<Type>>,
}

#[derive(Clone)]
pub struct PatType {
    pub attrs: Vec<Attribute>,
    pub pat: Box<Pat>,
    pub colon_token: Colon,
    pub ty: Box<Type>,
}

#[derive(Clone)]
pub struct Block {
    pub brace_span: Span,
    pub stmts: Tokens,
}

pub fn peek_signature(input: ParseStream<'_>) -> bool {
    let mut offset = 0;
    for qualifier in ["const", "async"] {
        if input.peek_keyword_at(offset, qualifier) {
            offset += 1;
        }
    }
    if input.peek_keyword_at(offset, "unsafe") || input.peek_keyword_at(offset, "safe") {
        offset += 1;
    }
    if input.peek_keyword_at(offset, "extern") {
        offset += 1;
        if matches!(input.peek_tree(offset), Some(TokenTree::Literal(_))) {
            offset += 1;
        }
    }
    input.peek_keyword_at(offset, "fn")
}

pub fn print_outer_attributes(attrs: &[Attribute], tokens: &mut Tokens) {
    for attribute in attrs {
        if matches!(attribute.style, AttrStyle::Outer) {
            attribute.to_tokens(tokens);
        }
    }
}

pub fn print_inner_attributes(attrs: &[Attribute], tokens: &mut Tokens) {
    for attribute in attrs {
        if matches!(attribute.style, AttrStyle::Inner(_)) {
            attribute.to_tokens(tokens);
        }
    }
}

impl Signature {
    pub fn receiver(&self) -> Option<&Receiver> {
        match self.inputs.first()? {
            FnArg::Receiver(receiver) => Some(receiver),
            FnArg::Typed(_) => None,
        }
    }
}

impl Block {
    pub fn parse_with_inner(input: ParseStream<'_>, attrs: &mut Vec<Attribute>) -> Result<Self> {
        let group = input.parse_group(Delimiter::Brace)?;
        let content = ParseBuffer::from_group(&group);
        attrs.extend(Attribute::parse_inner(&content)?);
        Ok(Self { brace_span: group.span(), stmts: content.rest() })
    }

    pub fn print_with_inner(&self, attrs: &[Attribute], tokens: &mut Tokens) {
        let mut inner = Tokens::new();
        print_inner_attributes(attrs, &mut inner);
        inner.extend(self.stmts.clone());
        push_group(tokens, self.brace_span, Delimiter::Brace, inner);
    }
}

impl Parse for Block {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let group = input.parse_group(Delimiter::Brace)?;
        Ok(Self { brace_span: group.span(), stmts: Tokens::from_trees(group.stream().into_iter().collect()) })
    }
}

impl ToTokens for Block {
    fn to_tokens(&self, tokens: &mut Tokens) {
        push_group(tokens, self.brace_span, Delimiter::Brace, self.stmts.clone());
    }
}

impl Parse for Signature {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let constness = input.eat_keyword("const");
        let asyncness = input.eat_keyword("async");
        let unsafety = input.eat_keyword("unsafe");
        let abi = match input.eat_keyword("extern") {
            Some(extern_token) => {
                let mut abi = extern_token.to_token_stream();
                if input.peek_literal() {
                    abi.push(input.parse_literal()?);
                }
                Some(abi)
            }
            None => None,
        };
        let fn_token = input.parse_keyword("fn")?;
        let ident = input.parse_any_ident()?;
        let mut generics = Generics::parse(input)?;
        let group = input.parse_group(Delimiter::Parenthesis)?;
        let content = ParseBuffer::from_group(&group);
        let (inputs, variadic) = parse_inputs(&content)?;
        let output = ReturnType::parse_helper(input, true)?;
        generics.where_clause = WhereClause::parse_optional(input)?;
        Ok(Self {
            constness,
            asyncness,
            unsafety,
            abi,
            fn_token,
            ident,
            generics,
            paren_span: group.span(),
            inputs,
            variadic,
            output,
        })
    }
}

fn parse_inputs(content: ParseStream<'_>) -> Result<(Punctuated<FnArg, Comma>, Option<Tokens>)> {
    let mut inputs = Punctuated::new();
    while !content.is_empty() {
        let attrs = Attribute::parse_outer(content)?;
        if content.peek_joint("...") {
            let mut tokens = Tokens::new();
            for attribute in &attrs {
                attribute.to_tokens(&mut tokens);
            }
            tokens.extend(content.rest());
            return Ok((inputs, Some(tokens)));
        }
        inputs.push_value(FnArg::parse_with_attributes(content, attrs)?);
        if content.is_empty() {
            break;
        }
        inputs.push_punct(Comma::parse_token(content)?);
    }
    Ok((inputs, None))
}

impl FnArg {
    pub fn parse_with_attributes(input: ParseStream<'_>, attrs: Vec<Attribute>) -> Result<Self> {
        let start = input.position();
        let reference = if input.peek_punct('&') {
            let and_token = And::parse_token(input)?;
            let lifetime = if input.peek_lifetime() { Some(input.parse_lifetime()?) } else { None };
            Some((and_token, lifetime))
        } else {
            None
        };
        let mutability = input.eat_keyword("mut");
        if input.peek_keyword("self") && !input.peek_joint_at(1, "::") {
            let self_token = input.parse_any_ident()?;
            let (colon_token, ty) = if reference.is_none() && input.peek_punct(':') && !input.peek_joint("::") {
                let colon_token = Colon::parse_token(input)?;
                (Some(colon_token), Some(Box::new(parse_type(input, true)?)))
            } else {
                (None, None)
            };
            return Ok(Self::Receiver(Receiver { attrs, reference, mutability, self_token, colon_token, ty }));
        }
        input.reset(start);
        let mut pattern = Vec::new();
        while let Some(tree) = input.peek_tree(0) {
            if input.peek_joint("::") {
                pattern.push(tree.clone());
                input.next_tree();
                pattern.push(input.parse_tree()?);
                continue;
            }
            if input.peek_punct(':') {
                break;
            }
            pattern.push(tree.clone());
            input.next_tree();
        }
        let pat = Box::new(Pat::from_tokens(Tokens::from_trees(pattern)));
        let colon_token = Colon::parse_token(input)?;
        let ty = Box::new(parse_type(input, true)?);
        Ok(Self::Typed(PatType { attrs, pat, colon_token, ty }))
    }
}

impl Parse for FnArg {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let attrs = Attribute::parse_outer(input)?;
        Self::parse_with_attributes(input, attrs)
    }
}

impl ToTokens for Signature {
    fn to_tokens(&self, tokens: &mut Tokens) {
        self.constness.to_tokens(tokens);
        self.asyncness.to_tokens(tokens);
        self.unsafety.to_tokens(tokens);
        self.abi.to_tokens(tokens);
        self.fn_token.to_tokens(tokens);
        self.ident.to_tokens(tokens);
        self.generics.to_tokens(tokens);
        let mut arguments = self.inputs.to_token_stream();
        if let Some(variadic) = &self.variadic {
            if !self.inputs.empty_or_trailing() {
                Comma::default().to_tokens(&mut arguments);
            }
            variadic.to_tokens(&mut arguments);
        }
        push_group(tokens, self.paren_span, Delimiter::Parenthesis, arguments);
        self.output.to_tokens(tokens);
        self.generics.where_clause.to_tokens(tokens);
    }
}

impl ToTokens for FnArg {
    fn to_tokens(&self, tokens: &mut Tokens) {
        match self {
            Self::Receiver(receiver) => receiver.to_tokens(tokens),
            Self::Typed(typed) => typed.to_tokens(tokens),
        }
    }
}

impl ToTokens for Receiver {
    fn to_tokens(&self, tokens: &mut Tokens) {
        print_outer_attributes(&self.attrs, tokens);
        if let Some((and_token, lifetime)) = &self.reference {
            and_token.to_tokens(tokens);
            lifetime.to_tokens(tokens);
        }
        self.mutability.to_tokens(tokens);
        self.self_token.to_tokens(tokens);
        if let Some(colon_token) = &self.colon_token {
            colon_token.to_tokens(tokens);
            self.ty.to_tokens(tokens);
        }
    }
}

impl ToTokens for PatType {
    fn to_tokens(&self, tokens: &mut Tokens) {
        print_outer_attributes(&self.attrs, tokens);
        self.pat.to_tokens(tokens);
        self.colon_token.to_tokens(tokens);
        self.ty.to_tokens(tokens);
    }
}
