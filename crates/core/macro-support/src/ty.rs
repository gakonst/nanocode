use proc_macro::{Delimiter, Span, TokenTree};

use crate::attribute::Attribute;
use crate::buffer::{Parse, ParseBuffer, ParseStream};
use crate::error::{Error, Result};
use crate::generics::BoundLifetimes;
use crate::ident::{Ident, Lifetime};
use crate::path::{
    ParenthesizedGenericArguments, Path, PathArguments, QSelf, ReturnType, parse_qualified_path,
    print_path,
};
use crate::punctuated::{And, Colon, Comma, Not, Plus, Punctuated, Semi, Star, Token};
use crate::tokens::{ToTokens, Tokens, push_group};

#[derive(Clone)]
pub enum Type {
    Array(TypeArray),
    BareFn(TypeBareFn),
    Group(TypeGroup),
    ImplTrait(TypeImplTrait),
    Infer(Span),
    Macro(TypeMacro),
    Never(Span),
    Paren(TypeParen),
    Path(TypePath),
    Ptr(TypePtr),
    Reference(TypeReference),
    Slice(TypeSlice),
    TraitObject(TypeTraitObject),
    Tuple(TypeTuple),
    Verbatim(Tokens),
}

#[derive(Clone)]
pub struct TypeArray {
    pub bracket_span: Span,
    pub elem: Box<Type>,
    pub semi_token: Semi,
    pub len: Tokens,
}

#[derive(Clone)]
pub struct TypeBareFn {
    pub lifetimes: Option<BoundLifetimes>,
    pub unsafety: Option<Ident>,
    pub abi: Option<Tokens>,
    pub fn_token: Ident,
    pub paren_span: Span,
    pub inputs: Punctuated<BareFnArg, Comma>,
    pub variadic: Option<Tokens>,
    pub output: ReturnType,
}

#[derive(Clone)]
pub struct BareFnArg {
    pub attrs: Vec<Attribute>,
    pub name: Option<(Ident, Colon)>,
    pub ty: Type,
}

#[derive(Clone)]
pub struct TypeGroup {
    pub group_span: Span,
    pub elem: Box<Type>,
}

#[derive(Clone)]
pub struct TypeImplTrait {
    pub impl_token: Ident,
    pub bounds: Punctuated<TypeParamBound, Plus>,
}

#[derive(Clone)]
pub struct TypeMacro {
    pub path: Path,
    pub bang_token: Not,
    pub delimiter: Delimiter,
    pub delimiter_span: Span,
    pub tokens: Tokens,
}

#[derive(Clone)]
pub struct TypeParen {
    pub paren_span: Span,
    pub elem: Box<Type>,
}

#[derive(Clone)]
pub struct TypePath {
    pub qself: Option<QSelf>,
    pub path: Path,
}

#[derive(Clone)]
pub struct TypePtr {
    pub star_token: Star,
    pub const_token: Option<Ident>,
    pub mutability: Option<Ident>,
    pub elem: Box<Type>,
}

#[derive(Clone)]
pub struct TypeReference {
    pub and_token: And,
    pub lifetime: Option<Lifetime>,
    pub mutability: Option<Ident>,
    pub elem: Box<Type>,
}

#[derive(Clone)]
pub struct TypeSlice {
    pub bracket_span: Span,
    pub elem: Box<Type>,
}

#[derive(Clone)]
pub struct TypeTraitObject {
    pub dyn_token: Option<Ident>,
    pub bounds: Punctuated<TypeParamBound, Plus>,
}

#[derive(Clone)]
pub struct TypeTuple {
    pub paren_span: Span,
    pub elems: Punctuated<Type, Comma>,
}

#[derive(Clone)]
pub enum TypeParamBound {
    Trait(TraitBound),
    Lifetime(Lifetime),
    PreciseCapture(Tokens),
    Verbatim(Tokens),
}

#[derive(Clone)]
pub struct TraitBound {
    pub paren_span: Option<Span>,
    pub modifier: TraitBoundModifier,
    pub lifetimes: Option<BoundLifetimes>,
    pub path: Path,
}

#[derive(Clone, Copy)]
pub enum TraitBoundModifier {
    None,
    Maybe(Span),
}

impl From<TypePath> for Type {
    fn from(path: TypePath) -> Self {
        Self::Path(path)
    }
}

impl Parse for Type {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        parse_type(input, true)
    }
}

impl Parse for TypePath {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        match parse_path_type(input, false)? {
            Type::Path(path) => Ok(path),
            _ => Err(input.error("expected type path")),
        }
    }
}

impl Type {
    pub fn without_plus(input: ParseStream<'_>) -> Result<Self> {
        parse_type(input, false)
    }
}

fn parse_group_type(input: ParseStream<'_>, delimiter: Delimiter) -> Result<(Span, ParseBuffer)> {
    let group = input.parse_group(delimiter)?;
    Ok((group.span(), ParseBuffer::from_group(&group)))
}

fn parse_parenthesized_type(input: ParseStream<'_>) -> Result<Type> {
    let (paren_span, content) = parse_group_type(input, Delimiter::Parenthesis)?;
    let mut elems = Punctuated::new();
    if content.is_empty() {
        return Ok(Type::Tuple(TypeTuple { paren_span, elems }));
    }
    let first = parse_type(&content, true)?;
    if content.is_empty() {
        return Ok(Type::Paren(TypeParen {
            paren_span,
            elem: Box::new(first),
        }));
    }
    elems.push_value(first);
    elems.push_punct(Comma::parse_token(&content)?);
    while !content.is_empty() {
        elems.push_value(parse_type(&content, true)?);
        if content.is_empty() {
            break;
        }
        elems.push_punct(Comma::parse_token(&content)?);
    }
    Ok(Type::Tuple(TypeTuple { paren_span, elems }))
}

fn parse_bracketed_type(input: ParseStream<'_>) -> Result<Type> {
    let (bracket_span, content) = parse_group_type(input, Delimiter::Bracket)?;
    let elem = Box::new(parse_type(&content, true)?);
    if content.peek_punct(';') {
        let semi_token = Semi::parse_token(&content)?;
        let len = content.rest();
        return Ok(Type::Array(TypeArray {
            bracket_span,
            elem,
            semi_token,
            len,
        }));
    }
    content.expect_end()?;
    Ok(Type::Slice(TypeSlice { bracket_span, elem }))
}

fn parse_path_type(input: ParseStream<'_>, allow_plus: bool) -> Result<Type> {
    let (qself, mut path) = parse_qualified_path(input, false)?;
    if qself.is_none() && input.peek_punct('!') && !input.peek_joint("!=") {
        let bang_token = Not::parse_token(input)?;
        let group = match input.parse_tree()? {
            TokenTree::Group(group) if group.delimiter() != Delimiter::None => group,
            _ => return Err(Error::new(bang_token.span, "expected macro delimiter")),
        };
        return Ok(Type::Macro(TypeMacro {
            path,
            bang_token,
            delimiter: group.delimiter(),
            delimiter_span: group.span(),
            tokens: Tokens::from_stream(group.stream()),
        }));
    }
    let last_is_empty = path
        .segments
        .last()
        .is_some_and(|segment| segment.arguments.is_none());
    if last_is_empty && input.peek_group(Delimiter::Parenthesis) {
        let arguments = ParenthesizedGenericArguments::parse(input)?;
        path.segments
            .last_mut()
            .expect("a path has a segment")
            .arguments = PathArguments::Parenthesized(arguments);
    }
    if allow_plus && input.peek_punct('+') {
        return Err(input.error("trait objects without `dyn` are not supported"));
    }
    Ok(Type::Path(TypePath { qself, path }))
}

fn parse_bare_fn(input: ParseStream<'_>, lifetimes: Option<BoundLifetimes>) -> Result<Type> {
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
    let (paren_span, content) = parse_group_type(input, Delimiter::Parenthesis)?;
    let mut inputs = Punctuated::new();
    let mut variadic = None;
    while !content.is_empty() {
        let attrs = Attribute::parse_outer(&content)?;
        if content.peek_joint("...") {
            let mut tokens = Tokens::new();
            for attribute in &attrs {
                attribute.to_tokens(&mut tokens);
            }
            tokens.extend(content.rest());
            variadic = Some(tokens);
            break;
        }
        let named = matches!(content.peek_tree(0), Some(TokenTree::Ident(_)))
            && content.peek_punct_at(1, ':')
            && !content.peek_joint_at(1, "::");
        let name = if named {
            let ident = content.parse_any_ident()?;
            let colon = Colon::parse_token(&content)?;
            Some((ident, colon))
        } else {
            None
        };
        let ty = parse_type(&content, true)?;
        inputs.push_value(BareFnArg { attrs, name, ty });
        if content.is_empty() {
            break;
        }
        inputs.push_punct(Comma::parse_token(&content)?);
    }
    let output = ReturnType::parse_helper(input, false)?;
    Ok(Type::BareFn(TypeBareFn {
        lifetimes,
        unsafety,
        abi,
        fn_token,
        paren_span,
        inputs,
        variadic,
        output,
    }))
}

pub fn parse_type(input: ParseStream<'_>, allow_plus: bool) -> Result<Type> {
    match input.peek_tree(0) {
        Some(TokenTree::Group(group)) => match group.delimiter() {
            Delimiter::None => {
                let group = group.clone();
                input.next_tree();
                let content = ParseBuffer::from_group(&group);
                let elem = parse_type(&content, true)?;
                content.expect_end()?;
                Ok(Type::Group(TypeGroup {
                    group_span: group.span(),
                    elem: Box::new(elem),
                }))
            }
            Delimiter::Parenthesis => parse_parenthesized_type(input),
            Delimiter::Bracket => parse_bracketed_type(input),
            Delimiter::Brace => Err(input.error("expected type")),
        },
        Some(TokenTree::Punct(punct)) => match punct.as_char() {
            '&' => {
                let and_token = And::parse_token(input)?;
                let lifetime = if input.peek_lifetime() {
                    Some(input.parse_lifetime()?)
                } else {
                    None
                };
                let mutability = input.eat_keyword("mut");
                let elem = Box::new(parse_type(input, false)?);
                Ok(Type::Reference(TypeReference {
                    and_token,
                    lifetime,
                    mutability,
                    elem,
                }))
            }
            '*' => {
                let star_token = Star::parse_token(input)?;
                let const_token = input.eat_keyword("const");
                let mutability = if const_token.is_none() {
                    Some(input.parse_keyword("mut")?)
                } else {
                    None
                };
                let elem = Box::new(parse_type(input, false)?);
                Ok(Type::Ptr(TypePtr {
                    star_token,
                    const_token,
                    mutability,
                    elem,
                }))
            }
            '!' => {
                let span = punct.span();
                input.next_tree();
                Ok(Type::Never(span))
            }
            '<' | ':' => parse_path_type(input, allow_plus),
            _ => Err(input.error("expected type")),
        },
        Some(TokenTree::Ident(ident)) => match ident.to_string().as_str() {
            "_" => {
                let span = ident.span();
                input.next_tree();
                Ok(Type::Infer(span))
            }
            "fn" | "unsafe" | "extern" => parse_bare_fn(input, None),
            "for" => {
                let lifetimes = BoundLifetimes::parse(input)?;
                parse_bare_fn(input, Some(lifetimes))
            }
            "impl" => {
                let impl_token = input.parse_keyword("impl")?;
                let bounds = parse_type_bounds(input, allow_plus)?;
                Ok(Type::ImplTrait(TypeImplTrait { impl_token, bounds }))
            }
            "dyn" => {
                let dyn_token = Some(input.parse_keyword("dyn")?);
                let bounds = parse_type_bounds(input, allow_plus)?;
                Ok(Type::TraitObject(TypeTraitObject { dyn_token, bounds }))
            }
            _ => parse_path_type(input, allow_plus),
        },
        Some(TokenTree::Literal(_)) => Err(input.error("expected type, found literal")),
        None => Err(input.error("expected type")),
    }
}

fn starts_bound(input: ParseStream<'_>) -> bool {
    input.peek_ident()
        || input.peek_path_separator()
        || input.peek_punct('?')
        || input.peek_lifetime()
        || input.peek_group(Delimiter::Parenthesis)
        || input.peek_punct('~')
}

pub fn parse_type_bounds(
    input: ParseStream<'_>,
    allow_plus: bool,
) -> Result<Punctuated<TypeParamBound, Plus>> {
    let mut bounds = Punctuated::new();
    loop {
        bounds.push_value(parse_type_bound(input)?);
        if !(allow_plus && input.peek_punct('+')) {
            break;
        }
        bounds.push_punct(Plus::parse_token(input)?);
        if !starts_bound(input) {
            break;
        }
    }
    Ok(bounds)
}

pub fn parse_type_bound(input: ParseStream<'_>) -> Result<TypeParamBound> {
    if input.peek_lifetime() {
        return input.parse_lifetime().map(TypeParamBound::Lifetime);
    }
    if input.peek_keyword("use") && input.peek_punct_at(1, '<') {
        let mut tokens = Tokens::new();
        tokens.push(input.parse_tree()?);
        let mut depth = 0usize;
        loop {
            let tree = input.parse_tree()?;
            let closes = matches!(&tree, TokenTree::Punct(punct) if punct.as_char() == '>');
            let opens = matches!(&tree, TokenTree::Punct(punct) if punct.as_char() == '<');
            tokens.push(tree);
            if opens {
                depth += 1;
            }
            if closes {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
        }
        return Ok(TypeParamBound::PreciseCapture(tokens));
    }
    if input.peek_punct('~') || input.peek_keyword("const") || input.peek_keyword("async") {
        let mut tokens = Tokens::new();
        if input.peek_punct('~') {
            tokens.push(input.parse_tree()?);
        }
        tokens.push(input.parse_tree()?);
        let bound = TraitBound::parse(input)?;
        bound.to_tokens(&mut tokens);
        return Ok(TypeParamBound::Verbatim(tokens));
    }
    if input.peek_group(Delimiter::Parenthesis) {
        let (paren_span, content) = parse_group_type(input, Delimiter::Parenthesis)?;
        let mut bound = TraitBound::parse(&content)?;
        content.expect_end()?;
        bound.paren_span = Some(paren_span);
        return Ok(TypeParamBound::Trait(bound));
    }
    TraitBound::parse(input).map(TypeParamBound::Trait)
}

impl Parse for TypeParamBound {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        parse_type_bound(input)
    }
}

impl Parse for TraitBound {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let lifetimes_first = if input.peek_keyword("for") {
            Some(BoundLifetimes::parse(input)?)
        } else {
            None
        };
        let modifier = match input.eat_punct('?') {
            Some(span) => TraitBoundModifier::Maybe(span),
            None => TraitBoundModifier::None,
        };
        let lifetimes = match lifetimes_first {
            Some(lifetimes) => Some(lifetimes),
            None if input.peek_keyword("for") => Some(BoundLifetimes::parse(input)?),
            None => None,
        };
        let mut path = Path::parse_helper(input, false)?;
        let last_is_empty = path
            .segments
            .last()
            .is_some_and(|segment| segment.arguments.is_empty());
        let parenthesized = input.peek_group(Delimiter::Parenthesis)
            || (input.peek_path_separator()
                && crate::path::peek_group_at(input, 2, Delimiter::Parenthesis));
        if last_is_empty && parenthesized {
            input.eat_joint("::");
            let arguments = ParenthesizedGenericArguments::parse(input)?;
            path.segments
                .last_mut()
                .expect("a path has a segment")
                .arguments = PathArguments::Parenthesized(arguments);
        }
        Ok(Self {
            paren_span: None,
            modifier,
            lifetimes,
            path,
        })
    }
}

impl ToTokens for TraitBound {
    fn to_tokens(&self, tokens: &mut Tokens) {
        let mut inner = Tokens::new();
        if let TraitBoundModifier::Maybe(span) = self.modifier {
            crate::tokens::push_punct(&mut inner, span, "?");
        }
        self.lifetimes.to_tokens(&mut inner);
        self.path.to_tokens(&mut inner);
        match self.paren_span {
            Some(span) => push_group(tokens, span, Delimiter::Parenthesis, inner),
            None => tokens.extend(inner),
        }
    }
}

impl ToTokens for TypeParamBound {
    fn to_tokens(&self, tokens: &mut Tokens) {
        match self {
            Self::Trait(bound) => bound.to_tokens(tokens),
            Self::Lifetime(lifetime) => lifetime.to_tokens(tokens),
            Self::PreciseCapture(capture) => capture.to_tokens(tokens),
            Self::Verbatim(verbatim) => verbatim.to_tokens(tokens),
        }
    }
}

impl ToTokens for Type {
    fn to_tokens(&self, tokens: &mut Tokens) {
        match self {
            Self::Array(array) => {
                let mut inner = array.elem.to_token_stream();
                array.semi_token.to_tokens(&mut inner);
                array.len.to_tokens(&mut inner);
                push_group(tokens, array.bracket_span, Delimiter::Bracket, inner);
            }
            Self::BareFn(bare) => bare.to_tokens(tokens),
            Self::Group(group) => push_group(
                tokens,
                group.group_span,
                Delimiter::None,
                group.elem.to_token_stream(),
            ),
            Self::ImplTrait(implementation) => {
                implementation.impl_token.to_tokens(tokens);
                implementation.bounds.to_tokens(tokens);
            }
            Self::Infer(span) => crate::tokens::push_ident(tokens, *span, "_"),
            Self::Macro(mac) => {
                mac.path.to_tokens(tokens);
                mac.bang_token.to_tokens(tokens);
                push_group(
                    tokens,
                    mac.delimiter_span,
                    mac.delimiter,
                    mac.tokens.clone(),
                );
            }
            Self::Never(span) => crate::tokens::push_punct(tokens, *span, "!"),
            Self::Paren(paren) => push_group(
                tokens,
                paren.paren_span,
                Delimiter::Parenthesis,
                paren.elem.to_token_stream(),
            ),
            Self::Path(path) => path.to_tokens(tokens),
            Self::Ptr(pointer) => {
                pointer.star_token.to_tokens(tokens);
                match &pointer.mutability {
                    Some(mutability) => mutability.to_tokens(tokens),
                    None => match &pointer.const_token {
                        Some(const_token) => const_token.to_tokens(tokens),
                        None => Ident::new("const", Span::call_site()).to_tokens(tokens),
                    },
                }
                pointer.elem.to_tokens(tokens);
            }
            Self::Reference(reference) => {
                reference.and_token.to_tokens(tokens);
                reference.lifetime.to_tokens(tokens);
                reference.mutability.to_tokens(tokens);
                reference.elem.to_tokens(tokens);
            }
            Self::Slice(slice) => push_group(
                tokens,
                slice.bracket_span,
                Delimiter::Bracket,
                slice.elem.to_token_stream(),
            ),
            Self::TraitObject(object) => {
                object.dyn_token.to_tokens(tokens);
                object.bounds.to_tokens(tokens);
            }
            Self::Tuple(tuple) => {
                let mut inner = tuple.elems.to_token_stream();
                if tuple.elems.len() == 1 && !tuple.elems.trailing_punct() {
                    Comma::default().to_tokens(&mut inner);
                }
                push_group(tokens, tuple.paren_span, Delimiter::Parenthesis, inner);
            }
            Self::Verbatim(verbatim) => verbatim.to_tokens(tokens),
        }
    }
}

impl ToTokens for TypePath {
    fn to_tokens(&self, tokens: &mut Tokens) {
        print_path(tokens, self.qself.as_ref(), &self.path);
    }
}

impl ToTokens for TypeBareFn {
    fn to_tokens(&self, tokens: &mut Tokens) {
        self.lifetimes.to_tokens(tokens);
        self.unsafety.to_tokens(tokens);
        self.abi.to_tokens(tokens);
        self.fn_token.to_tokens(tokens);
        let mut inner = self.inputs.to_token_stream();
        if let Some(variadic) = &self.variadic {
            if !self.inputs.empty_or_trailing() {
                Comma::default().to_tokens(&mut inner);
            }
            variadic.to_tokens(&mut inner);
        }
        push_group(tokens, self.paren_span, Delimiter::Parenthesis, inner);
        self.output.to_tokens(tokens);
    }
}

impl ToTokens for BareFnArg {
    fn to_tokens(&self, tokens: &mut Tokens) {
        for attribute in &self.attrs {
            attribute.to_tokens(tokens);
        }
        if let Some((name, colon)) = &self.name {
            name.to_tokens(tokens);
            colon.to_tokens(tokens);
        }
        self.ty.to_tokens(tokens);
    }
}
