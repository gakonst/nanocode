use proc_macro::{Delimiter, Span};

use crate::buffer::{Parse, ParseBuffer, ParseStream};
use crate::error::Result;
use crate::ident::{Ident, Lifetime};
use crate::punctuated::{Comma, Eq, Gt, Lt, PathSep, Plus, Punctuated, RArrow, Token};
use crate::tokens::{ToTokens, Tokens, push_group};
use crate::ty::{Type, TypeParamBound, parse_type, parse_type_bound};

#[derive(Clone)]
pub struct Path {
    pub leading_colon: Option<PathSep>,
    pub segments: Punctuated<PathSegment, PathSep>,
}

#[derive(Clone)]
pub struct PathSegment {
    pub ident: Ident,
    pub arguments: PathArguments,
}

#[derive(Clone, Default)]
pub enum PathArguments {
    #[default]
    None,
    AngleBracketed(AngleBracketedGenericArguments),
    Parenthesized(ParenthesizedGenericArguments),
}

#[derive(Clone)]
pub struct AngleBracketedGenericArguments {
    pub colon2_token: Option<PathSep>,
    pub lt_token: Lt,
    pub args: Punctuated<GenericArgument, Comma>,
    pub gt_token: Gt,
}

#[derive(Clone)]
pub enum GenericArgument {
    Lifetime(Lifetime),
    Type(Type),
    Const(Tokens),
    AssocType(AssocType),
    AssocConst(AssocConst),
    Constraint(Constraint),
}

#[derive(Clone)]
pub struct AssocType {
    pub ident: Ident,
    pub generics: Option<AngleBracketedGenericArguments>,
    pub eq_token: Eq,
    pub ty: Type,
}

#[derive(Clone)]
pub struct AssocConst {
    pub ident: Ident,
    pub generics: Option<AngleBracketedGenericArguments>,
    pub eq_token: Eq,
    pub value: Tokens,
}

#[derive(Clone)]
pub struct Constraint {
    pub ident: Ident,
    pub generics: Option<AngleBracketedGenericArguments>,
    pub colon_token: crate::punctuated::Colon,
    pub bounds: Punctuated<TypeParamBound, Plus>,
}

#[derive(Clone)]
pub struct ParenthesizedGenericArguments {
    pub paren_span: Span,
    pub inputs: Punctuated<Type, Comma>,
    pub output: ReturnType,
}

#[derive(Clone)]
pub enum ReturnType {
    Default,
    Type(RArrow, Box<Type>),
}

#[derive(Clone)]
pub struct QSelf {
    pub lt_token: Lt,
    pub ty: Box<Type>,
    pub position: usize,
    pub as_token: Option<Ident>,
    pub gt_token: Gt,
}

impl Path {
    pub fn is_ident(&self, name: &str) -> bool {
        self.get_ident().is_some_and(|ident| *ident == *name)
    }

    pub fn get_ident(&self) -> Option<&Ident> {
        if self.leading_colon.is_some() || self.segments.len() != 1 {
            return None;
        }
        let segment = self.segments.first()?;
        if !segment.arguments.is_none() {
            return None;
        }
        Some(&segment.ident)
    }

    pub fn require_ident(&self) -> Result<&Ident> {
        self.get_ident().ok_or_else(|| {
            crate::error::Error::new_spanned(self, "expected this path to be an identifier")
        })
    }

    pub fn parse_mod_style(input: ParseStream<'_>) -> Result<Self> {
        let leading_colon = parse_optional::<PathSep>(input)?;
        let mut segments = Punctuated::new();
        loop {
            segments.push_value(PathSegment::from(input.parse_any_ident()?));
            if !input.peek_path_separator() {
                break;
            }
            segments.push_punct(PathSep::parse_token(input)?);
        }
        Ok(Self {
            leading_colon,
            segments,
        })
    }

    pub fn parse_helper(input: ParseStream<'_>, expression_style: bool) -> Result<Self> {
        let leading_colon = parse_optional::<PathSep>(input)?;
        let mut segments = Punctuated::new();
        segments.push_value(PathSegment::parse_helper(input, expression_style)?);
        let mut path = Self {
            leading_colon,
            segments,
        };
        Self::parse_rest(input, &mut path, expression_style)?;
        Ok(path)
    }

    pub fn parse_rest(
        input: ParseStream<'_>,
        path: &mut Self,
        expression_style: bool,
    ) -> Result<()> {
        while input.peek_path_separator() && !peek_group_at(input, 2, Delimiter::Parenthesis) {
            path.segments.push_punct(PathSep::parse_token(input)?);
            path.segments
                .push_value(PathSegment::parse_helper(input, expression_style)?);
        }
        Ok(())
    }
}

pub fn parse_optional<T: Token>(input: ParseStream<'_>) -> Result<Option<T>> {
    if T::peek(input) {
        return T::parse_token(input).map(Some);
    }
    Ok(None)
}

pub fn peek_group_at(input: ParseStream<'_>, offset: usize, delimiter: Delimiter) -> bool {
    matches!(input.peek_tree(offset), Some(proc_macro::TokenTree::Group(group)) if group.delimiter() == delimiter)
}

impl From<Ident> for Path {
    fn from(ident: Ident) -> Self {
        let mut segments = Punctuated::new();
        segments.push_value(PathSegment::from(ident));
        Self {
            leading_colon: None,
            segments,
        }
    }
}

impl From<PathSegment> for Path {
    fn from(segment: PathSegment) -> Self {
        let mut segments = Punctuated::new();
        segments.push_value(segment);
        Self {
            leading_colon: None,
            segments,
        }
    }
}

impl Parse for Path {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        Self::parse_helper(input, false)
    }
}

impl ToTokens for Path {
    fn to_tokens(&self, tokens: &mut Tokens) {
        self.leading_colon.to_tokens(tokens);
        self.segments.to_tokens(tokens);
    }
}

impl From<Ident> for PathSegment {
    fn from(ident: Ident) -> Self {
        Self {
            ident,
            arguments: PathArguments::None,
        }
    }
}

impl PathSegment {
    pub fn parse_helper(input: ParseStream<'_>, expression_style: bool) -> Result<Self> {
        let ident = input.parse_any_ident()?;
        let angle = (!expression_style && input.peek_punct('<') && !input.peek_joint("<="))
            || (input.peek_path_separator() && input.peek_punct_at(2, '<'));
        if angle {
            return Ok(Self {
                ident,
                arguments: PathArguments::AngleBracketed(input.parse()?),
            });
        }
        Ok(Self::from(ident))
    }
}

impl ToTokens for PathSegment {
    fn to_tokens(&self, tokens: &mut Tokens) {
        self.ident.to_tokens(tokens);
        self.arguments.to_tokens(tokens);
    }
}

impl PathArguments {
    pub const fn is_empty(&self) -> bool {
        match self {
            Self::None => true,
            Self::AngleBracketed(arguments) => arguments.args.is_empty(),
            Self::Parenthesized(_) => false,
        }
    }

    pub const fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }
}

impl ToTokens for PathArguments {
    fn to_tokens(&self, tokens: &mut Tokens) {
        match self {
            Self::None => {}
            Self::AngleBracketed(arguments) => arguments.to_tokens(tokens),
            Self::Parenthesized(arguments) => arguments.to_tokens(tokens),
        }
    }
}

impl Parse for AngleBracketedGenericArguments {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let colon2_token = parse_optional::<PathSep>(input)?;
        let lt_token = Lt::parse_token(input)?;
        let mut args = Punctuated::new();
        loop {
            if input.peek_punct('>') {
                break;
            }
            args.push_value(input.parse()?);
            if input.peek_punct('>') {
                break;
            }
            args.push_punct(Comma::parse_token(input)?);
        }
        let gt_token = Gt::parse_token(input)?;
        Ok(Self {
            colon2_token,
            lt_token,
            args,
            gt_token,
        })
    }
}

impl ToTokens for AngleBracketedGenericArguments {
    fn to_tokens(&self, tokens: &mut Tokens) {
        self.colon2_token.to_tokens(tokens);
        self.lt_token.to_tokens(tokens);
        let mut trailing_or_empty = true;
        for pair in self.args.pairs() {
            if let GenericArgument::Lifetime(_) = pair.value() {
                pair.to_tokens(tokens);
                trailing_or_empty = pair.punct().is_some();
            }
        }
        for pair in self.args.pairs() {
            match pair.value() {
                GenericArgument::Type(_)
                | GenericArgument::Const(_)
                | GenericArgument::AssocType(_)
                | GenericArgument::AssocConst(_)
                | GenericArgument::Constraint(_) => {
                    if !trailing_or_empty {
                        Comma::default().to_tokens(tokens);
                    }
                    pair.to_tokens(tokens);
                    trailing_or_empty = pair.punct().is_some();
                }
                GenericArgument::Lifetime(_) => {}
            }
        }
        self.gt_token.to_tokens(tokens);
    }
}

fn starts_const_argument(input: ParseStream<'_>) -> bool {
    input.peek_literal()
        || input.peek_group(Delimiter::Brace)
        || input.peek_keyword("true")
        || input.peek_keyword("false")
}

fn parse_const_argument(input: ParseStream<'_>) -> Result<Tokens> {
    let mut tokens = Tokens::new();
    tokens.push(input.parse_tree()?);
    Ok(tokens)
}

impl Parse for GenericArgument {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        if input.peek_lifetime() && !input.peek_punct_at(2, '+') {
            return Ok(Self::Lifetime(input.parse_lifetime()?));
        }
        if starts_const_argument(input) {
            return parse_const_argument(input).map(Self::Const);
        }
        let argument = parse_type(input, true)?;
        let Type::Path(mut path_type) = argument else {
            return Ok(Self::Type(argument));
        };
        let simple = path_type.qself.is_none()
            && path_type.path.leading_colon.is_none()
            && path_type.path.segments.len() == 1
            && !matches!(
                path_type.path.segments[0].arguments,
                PathArguments::Parenthesized(_)
            );
        if simple && input.peek_punct('=') && !input.peek_joint("==") {
            let eq_token = Eq::parse_token(input)?;
            let segment = path_type
                .path
                .segments
                .pop()
                .expect("one segment")
                .into_value();
            let generics = match segment.arguments {
                PathArguments::AngleBracketed(arguments) => Some(arguments),
                PathArguments::None | PathArguments::Parenthesized(_) => None,
            };
            if starts_const_argument(input) {
                let value = parse_const_argument(input)?;
                return Ok(Self::AssocConst(AssocConst {
                    ident: segment.ident,
                    generics,
                    eq_token,
                    value,
                }));
            }
            let ty = parse_type(input, true)?;
            return Ok(Self::AssocType(AssocType {
                ident: segment.ident,
                generics,
                eq_token,
                ty,
            }));
        }
        if simple && input.peek_punct(':') && !input.peek_path_separator() {
            let colon_token = crate::punctuated::Colon::parse_token(input)?;
            let segment = path_type
                .path
                .segments
                .pop()
                .expect("one segment")
                .into_value();
            let generics = match segment.arguments {
                PathArguments::AngleBracketed(arguments) => Some(arguments),
                PathArguments::None | PathArguments::Parenthesized(_) => None,
            };
            let mut bounds = Punctuated::new();
            loop {
                if input.peek_punct(',') || input.peek_punct('>') {
                    break;
                }
                bounds.push_value(parse_type_bound(input)?);
                if !input.peek_punct('+') {
                    break;
                }
                bounds.push_punct(Plus::parse_token(input)?);
            }
            return Ok(Self::Constraint(Constraint {
                ident: segment.ident,
                generics,
                colon_token,
                bounds,
            }));
        }
        Ok(Self::Type(Type::Path(path_type)))
    }
}

impl ToTokens for GenericArgument {
    fn to_tokens(&self, tokens: &mut Tokens) {
        match self {
            Self::Lifetime(lifetime) => lifetime.to_tokens(tokens),
            Self::Type(ty) => ty.to_tokens(tokens),
            Self::Const(value) => value.to_tokens(tokens),
            Self::AssocType(assoc) => {
                assoc.ident.to_tokens(tokens);
                assoc.generics.to_tokens(tokens);
                assoc.eq_token.to_tokens(tokens);
                assoc.ty.to_tokens(tokens);
            }
            Self::AssocConst(assoc) => {
                assoc.ident.to_tokens(tokens);
                assoc.generics.to_tokens(tokens);
                assoc.eq_token.to_tokens(tokens);
                assoc.value.to_tokens(tokens);
            }
            Self::Constraint(constraint) => {
                constraint.ident.to_tokens(tokens);
                constraint.generics.to_tokens(tokens);
                constraint.colon_token.to_tokens(tokens);
                constraint.bounds.to_tokens(tokens);
            }
        }
    }
}

impl Parse for ParenthesizedGenericArguments {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let group = input.parse_group(Delimiter::Parenthesis)?;
        let content = ParseBuffer::from_group(&group);
        let mut inputs = Punctuated::new();
        while !content.is_empty() {
            inputs.push_value(parse_type(&content, true)?);
            if content.is_empty() {
                break;
            }
            inputs.push_punct(Comma::parse_token(&content)?);
        }
        let output = ReturnType::parse_helper(input, false)?;
        Ok(Self {
            paren_span: group.span(),
            inputs,
            output,
        })
    }
}

impl ToTokens for ParenthesizedGenericArguments {
    fn to_tokens(&self, tokens: &mut Tokens) {
        push_group(
            tokens,
            self.paren_span,
            Delimiter::Parenthesis,
            self.inputs.to_token_stream(),
        );
        self.output.to_tokens(tokens);
    }
}

impl ReturnType {
    pub fn parse_helper(input: ParseStream<'_>, allow_plus: bool) -> Result<Self> {
        if input.peek_joint("->") {
            let arrow = RArrow::parse_token(input)?;
            let ty = parse_type(input, allow_plus)?;
            return Ok(Self::Type(arrow, Box::new(ty)));
        }
        Ok(Self::Default)
    }
}

impl Parse for ReturnType {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        Self::parse_helper(input, true)
    }
}

impl ToTokens for ReturnType {
    fn to_tokens(&self, tokens: &mut Tokens) {
        if let Self::Type(arrow, ty) = self {
            arrow.to_tokens(tokens);
            ty.to_tokens(tokens);
        }
    }
}

pub fn parse_qualified_path(
    input: ParseStream<'_>,
    expression_style: bool,
) -> Result<(Option<QSelf>, Path)> {
    if !input.peek_punct('<') {
        return Ok((None, Path::parse_helper(input, expression_style)?));
    }
    let lt_token = Lt::parse_token(input)?;
    let this = parse_type(input, true)?;
    let trait_path = match input.eat_keyword("as") {
        Some(as_token) => Some((as_token, Path::parse_helper(input, false)?)),
        None => None,
    };
    let gt_token = Gt::parse_token(input)?;
    let colon2_token = PathSep::parse_token(input)?;
    let mut rest = Punctuated::<PathSegment, PathSep>::new();
    loop {
        rest.push_value(PathSegment::parse_helper(input, expression_style)?);
        if !input.peek_path_separator() {
            break;
        }
        rest.push_punct(PathSep::parse_token(input)?);
    }
    let (position, as_token, path) = match trait_path {
        Some((as_token, mut path)) => {
            let position = path.segments.len();
            path.segments.push_punct(colon2_token);
            for pair in rest.into_pairs() {
                let (value, punct) = pair.into_tuple();
                path.segments.push_value(value);
                if let Some(punct) = punct {
                    path.segments.push_punct(punct);
                }
            }
            (position, Some(as_token), path)
        }
        None => (
            0,
            None,
            Path {
                leading_colon: Some(colon2_token),
                segments: rest,
            },
        ),
    };
    let qself = QSelf {
        lt_token,
        ty: Box::new(this),
        position,
        as_token,
        gt_token,
    };
    Ok((Some(qself), path))
}

pub fn print_path(tokens: &mut Tokens, qself: Option<&QSelf>, path: &Path) {
    let Some(qself) = qself else {
        path.to_tokens(tokens);
        return;
    };
    qself.lt_token.to_tokens(tokens);
    qself.ty.to_tokens(tokens);
    let position = qself.position.min(path.segments.len());
    let mut segments = path.segments.pairs();
    if position > 0 {
        match &qself.as_token {
            Some(as_token) => as_token.to_tokens(tokens),
            None => Ident::new("as", Span::call_site()).to_tokens(tokens),
        }
        path.leading_colon.to_tokens(tokens);
        for (index, segment) in segments.by_ref().take(position).enumerate() {
            if index + 1 == position {
                segment.value().to_tokens(tokens);
                qself.gt_token.to_tokens(tokens);
                segment.punct().to_tokens(tokens);
            } else {
                segment.to_tokens(tokens);
            }
        }
    } else {
        qself.gt_token.to_tokens(tokens);
        path.leading_colon.to_tokens(tokens);
    }
    for segment in segments {
        segment.to_tokens(tokens);
    }
}
