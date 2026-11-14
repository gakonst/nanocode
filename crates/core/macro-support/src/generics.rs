use proc_macro::{Delimiter, Span};

use crate::attribute::Attribute;
use crate::buffer::{Parse, ParseStream};
use crate::error::Result;
use crate::ident::{Ident, Lifetime};
use crate::path::parse_optional;
use crate::punctuated::{Colon, Comma, Eq, Gt, Lt, PathSep, Plus, Punctuated, Token};
use crate::tokens::{ToTokens, Tokens};
use crate::ty::{Type, TypeParamBound, parse_type, parse_type_bound};

#[derive(Clone, Default)]
pub struct Generics {
    pub lt_token: Option<Lt>,
    pub params: Punctuated<GenericParam, Comma>,
    pub gt_token: Option<Gt>,
    pub where_clause: Option<WhereClause>,
}

#[derive(Clone)]
pub enum GenericParam {
    Lifetime(LifetimeParam),
    Type(TypeParam),
    Const(ConstParam),
}

#[derive(Clone)]
pub struct LifetimeParam {
    pub attrs: Vec<Attribute>,
    pub lifetime: Lifetime,
    pub colon_token: Option<Colon>,
    pub bounds: Punctuated<Lifetime, Plus>,
}

#[derive(Clone)]
pub struct TypeParam {
    pub attrs: Vec<Attribute>,
    pub ident: Ident,
    pub colon_token: Option<Colon>,
    pub bounds: Punctuated<TypeParamBound, Plus>,
    pub eq_token: Option<Eq>,
    pub default: Option<Type>,
}

#[derive(Clone)]
pub struct ConstParam {
    pub attrs: Vec<Attribute>,
    pub const_token: Ident,
    pub ident: Ident,
    pub colon_token: Colon,
    pub ty: Type,
    pub eq_token: Option<Eq>,
    pub default: Option<Tokens>,
}

#[derive(Clone)]
pub struct WhereClause {
    pub where_token: Ident,
    pub predicates: Punctuated<WherePredicate, Comma>,
}

#[derive(Clone)]
pub enum WherePredicate {
    Lifetime(PredicateLifetime),
    Type(PredicateType),
}

#[derive(Clone)]
pub struct PredicateLifetime {
    pub lifetime: Lifetime,
    pub colon_token: Colon,
    pub bounds: Punctuated<Lifetime, Plus>,
}

#[derive(Clone)]
pub struct PredicateType {
    pub lifetimes: Option<BoundLifetimes>,
    pub bounded_ty: Type,
    pub colon_token: Colon,
    pub bounds: Punctuated<TypeParamBound, Plus>,
}

#[derive(Clone)]
pub struct BoundLifetimes {
    pub for_token: Ident,
    pub lt_token: Lt,
    pub lifetimes: Punctuated<GenericParam, Comma>,
    pub gt_token: Gt,
}

impl LifetimeParam {
    pub const fn new(lifetime: Lifetime) -> Self {
        Self {
            attrs: Vec::new(),
            lifetime,
            colon_token: None,
            bounds: Punctuated::new(),
        }
    }
}

impl From<Ident> for TypeParam {
    fn from(ident: Ident) -> Self {
        Self {
            attrs: Vec::new(),
            ident,
            colon_token: None,
            bounds: Punctuated::new(),
            eq_token: None,
            default: None,
        }
    }
}

impl Generics {
    pub fn lifetimes(&self) -> impl Iterator<Item = &LifetimeParam> {
        self.params.iter().filter_map(|param| match param {
            GenericParam::Lifetime(lifetime) => Some(lifetime),
            GenericParam::Type(_) | GenericParam::Const(_) => None,
        })
    }

    pub fn lifetimes_mut(&mut self) -> impl Iterator<Item = &mut LifetimeParam> {
        self.params.iter_mut().filter_map(|param| match param {
            GenericParam::Lifetime(lifetime) => Some(lifetime),
            GenericParam::Type(_) | GenericParam::Const(_) => None,
        })
    }

    pub fn type_params(&self) -> impl Iterator<Item = &TypeParam> {
        self.params.iter().filter_map(|param| match param {
            GenericParam::Type(ty) => Some(ty),
            GenericParam::Lifetime(_) | GenericParam::Const(_) => None,
        })
    }

    pub fn type_params_mut(&mut self) -> impl Iterator<Item = &mut TypeParam> {
        self.params.iter_mut().filter_map(|param| match param {
            GenericParam::Type(ty) => Some(ty),
            GenericParam::Lifetime(_) | GenericParam::Const(_) => None,
        })
    }

    pub fn const_params(&self) -> impl Iterator<Item = &ConstParam> {
        self.params.iter().filter_map(|param| match param {
            GenericParam::Const(constant) => Some(constant),
            GenericParam::Lifetime(_) | GenericParam::Type(_) => None,
        })
    }

    pub fn make_where_clause(&mut self) -> &mut WhereClause {
        self.where_clause.get_or_insert_with(|| WhereClause {
            where_token: Ident::new("where", Span::call_site()),
            predicates: Punctuated::new(),
        })
    }

    pub const fn split_for_impl(
        &self,
    ) -> (ImplGenerics<'_>, TypeGenerics<'_>, Option<&WhereClause>) {
        (
            ImplGenerics(self),
            TypeGenerics(self),
            self.where_clause.as_ref(),
        )
    }
}

impl Parse for Generics {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        if !input.peek_punct('<') {
            return Ok(Self::default());
        }
        let lt_token = Lt::parse_token(input)?;
        let mut params = Punctuated::new();
        loop {
            if input.peek_punct('>') {
                break;
            }
            let attrs = Attribute::parse_outer(input)?;
            if input.peek_lifetime() {
                let mut param = LifetimeParam::parse(input)?;
                param.attrs = attrs;
                params.push_value(GenericParam::Lifetime(param));
            } else if input.peek_keyword("const") {
                let mut param = ConstParam::parse(input)?;
                param.attrs = attrs;
                params.push_value(GenericParam::Const(param));
            } else if input.peek_ident() {
                let mut param = TypeParam::parse(input)?;
                param.attrs = attrs;
                params.push_value(GenericParam::Type(param));
            } else {
                return Err(input.error("expected one of: lifetime, identifier, `const`"));
            }
            if input.peek_punct('>') {
                break;
            }
            params.push_punct(Comma::parse_token(input)?);
        }
        let gt_token = Gt::parse_token(input)?;
        Ok(Self {
            lt_token: Some(lt_token),
            params,
            gt_token: Some(gt_token),
            where_clause: None,
        })
    }
}

impl Parse for LifetimeParam {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let lifetime = input.parse_lifetime()?;
        let colon_token = parse_optional::<Colon>(input)?;
        let mut bounds = Punctuated::new();
        if colon_token.is_some() {
            loop {
                if input.is_empty() || input.peek_punct(',') || input.peek_punct('>') {
                    break;
                }
                bounds.push_value(input.parse_lifetime()?);
                if !input.peek_punct('+') {
                    break;
                }
                bounds.push_punct(Plus::parse_token(input)?);
            }
        }
        Ok(Self {
            attrs: Vec::new(),
            lifetime,
            colon_token,
            bounds,
        })
    }
}

impl Parse for TypeParam {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let ident = input.parse_any_ident()?;
        let colon_token = parse_optional::<Colon>(input)?;
        let mut bounds = Punctuated::new();
        if colon_token.is_some() {
            loop {
                if input.is_empty()
                    || input.peek_punct(',')
                    || input.peek_punct('>')
                    || input.peek_punct('=')
                {
                    break;
                }
                bounds.push_value(parse_type_bound(input)?);
                if !input.peek_punct('+') {
                    break;
                }
                bounds.push_punct(Plus::parse_token(input)?);
            }
        }
        let eq_token = parse_optional::<Eq>(input)?;
        let default = if eq_token.is_some() {
            Some(parse_type(input, true)?)
        } else {
            None
        };
        Ok(Self {
            attrs: Vec::new(),
            ident,
            colon_token,
            bounds,
            eq_token,
            default,
        })
    }
}

impl Parse for ConstParam {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let const_token = input.parse_keyword("const")?;
        let ident = input.parse_any_ident()?;
        let colon_token = Colon::parse_token(input)?;
        let ty = parse_type(input, true)?;
        let eq_token = parse_optional::<Eq>(input)?;
        let default = if eq_token.is_some() {
            let mut tokens = Tokens::new();
            if input.peek_punct('-') {
                tokens.push(input.parse_tree()?);
            }
            tokens.push(input.parse_tree()?);
            Some(tokens)
        } else {
            None
        };
        Ok(Self {
            attrs: Vec::new(),
            const_token,
            ident,
            colon_token,
            ty,
            eq_token,
            default,
        })
    }
}

impl Parse for BoundLifetimes {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let for_token = input.parse_keyword("for")?;
        let lt_token = Lt::parse_token(input)?;
        let mut lifetimes = Punctuated::new();
        while !input.peek_punct('>') {
            let attrs = Attribute::parse_outer(input)?;
            let mut param = LifetimeParam::parse(input)?;
            param.attrs = attrs;
            lifetimes.push_value(GenericParam::Lifetime(param));
            if input.peek_punct('>') {
                break;
            }
            lifetimes.push_punct(Comma::parse_token(input)?);
        }
        let gt_token = Gt::parse_token(input)?;
        Ok(Self {
            for_token,
            lt_token,
            lifetimes,
            gt_token,
        })
    }
}

fn ends_where_bounds(input: ParseStream<'_>) -> bool {
    input.is_empty()
        || input.peek_group(Delimiter::Brace)
        || input.peek_punct(',')
        || input.peek_punct(';')
        || (input.peek_punct(':') && !input.peek_path_separator())
        || input.peek_punct('=')
}

impl Parse for WhereClause {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let where_token = input.parse_keyword("where")?;
        let mut predicates = Punctuated::new();
        loop {
            if ends_where_bounds(input) {
                break;
            }
            predicates.push_value(input.parse()?);
            if !input.peek_punct(',') {
                break;
            }
            predicates.push_punct(Comma::parse_token(input)?);
        }
        Ok(Self {
            where_token,
            predicates,
        })
    }
}

impl WhereClause {
    pub fn parse_optional(input: ParseStream<'_>) -> Result<Option<Self>> {
        if input.peek_keyword("where") {
            return input.parse().map(Some);
        }
        Ok(None)
    }
}

impl Parse for WherePredicate {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        if input.peek_lifetime() && input.peek_punct_at(2, ':') && !input.peek_joint_at(2, "::") {
            let lifetime = input.parse_lifetime()?;
            let colon_token = Colon::parse_token(input)?;
            let mut bounds = Punctuated::new();
            loop {
                if ends_where_bounds(input) {
                    break;
                }
                bounds.push_value(input.parse_lifetime()?);
                if input.is_empty() || !input.peek_punct('+') {
                    break;
                }
                bounds.push_punct(Plus::parse_token(input)?);
            }
            return Ok(Self::Lifetime(PredicateLifetime {
                lifetime,
                colon_token,
                bounds,
            }));
        }
        let lifetimes = if input.peek_keyword("for") {
            Some(BoundLifetimes::parse(input)?)
        } else {
            None
        };
        let bounded_ty = parse_type(input, true)?;
        let colon_token = Colon::parse_token(input)?;
        let mut bounds = Punctuated::new();
        loop {
            if ends_where_bounds(input) {
                break;
            }
            bounds.push_value(parse_type_bound(input)?);
            if !input.peek_punct('+') {
                break;
            }
            bounds.push_punct(Plus::parse_token(input)?);
        }
        Ok(Self::Type(PredicateType {
            lifetimes,
            bounded_ty,
            colon_token,
            bounds,
        }))
    }
}

fn print_attributes(attrs: &[Attribute], tokens: &mut Tokens) {
    for attribute in attrs {
        attribute.to_tokens(tokens);
    }
}

impl ToTokens for LifetimeParam {
    fn to_tokens(&self, tokens: &mut Tokens) {
        print_attributes(&self.attrs, tokens);
        self.lifetime.to_tokens(tokens);
        if !self.bounds.is_empty() {
            self.colon_token.unwrap_or_default().to_tokens(tokens);
            self.bounds.to_tokens(tokens);
        }
    }
}

impl ToTokens for TypeParam {
    fn to_tokens(&self, tokens: &mut Tokens) {
        print_attributes(&self.attrs, tokens);
        self.ident.to_tokens(tokens);
        if !self.bounds.is_empty() {
            self.colon_token.unwrap_or_default().to_tokens(tokens);
            self.bounds.to_tokens(tokens);
        }
        if let Some(default) = &self.default {
            self.eq_token.unwrap_or_default().to_tokens(tokens);
            default.to_tokens(tokens);
        }
    }
}

impl ToTokens for ConstParam {
    fn to_tokens(&self, tokens: &mut Tokens) {
        print_attributes(&self.attrs, tokens);
        self.const_token.to_tokens(tokens);
        self.ident.to_tokens(tokens);
        self.colon_token.to_tokens(tokens);
        self.ty.to_tokens(tokens);
        if let Some(default) = &self.default {
            self.eq_token.unwrap_or_default().to_tokens(tokens);
            default.to_tokens(tokens);
        }
    }
}

impl ToTokens for GenericParam {
    fn to_tokens(&self, tokens: &mut Tokens) {
        match self {
            Self::Lifetime(param) => param.to_tokens(tokens),
            Self::Type(param) => param.to_tokens(tokens),
            Self::Const(param) => param.to_tokens(tokens),
        }
    }
}

impl ToTokens for Generics {
    fn to_tokens(&self, tokens: &mut Tokens) {
        if self.params.is_empty() {
            return;
        }
        self.lt_token.unwrap_or_default().to_tokens(tokens);
        let mut trailing_or_empty = true;
        for pair in self.params.pairs() {
            if let GenericParam::Lifetime(_) = pair.value() {
                pair.to_tokens(tokens);
                trailing_or_empty = pair.punct().is_some();
            }
        }
        for pair in self.params.pairs() {
            match pair.value() {
                GenericParam::Type(_) | GenericParam::Const(_) => {
                    if !trailing_or_empty {
                        Comma::default().to_tokens(tokens);
                        trailing_or_empty = true;
                    }
                    pair.to_tokens(tokens);
                }
                GenericParam::Lifetime(_) => {}
            }
        }
        self.gt_token.unwrap_or_default().to_tokens(tokens);
    }
}

pub struct ImplGenerics<'a>(pub &'a Generics);

pub struct TypeGenerics<'a>(pub &'a Generics);

pub struct Turbofish<'a>(pub &'a Generics);

impl<'a> TypeGenerics<'a> {
    pub const fn as_turbofish(&self) -> Turbofish<'a> {
        Turbofish(self.0)
    }
}

impl ToTokens for ImplGenerics<'_> {
    fn to_tokens(&self, tokens: &mut Tokens) {
        let generics = self.0;
        if generics.params.is_empty() {
            return;
        }
        generics.lt_token.unwrap_or_default().to_tokens(tokens);
        let mut trailing_or_empty = true;
        for pair in generics.params.pairs() {
            if let GenericParam::Lifetime(_) = pair.value() {
                pair.to_tokens(tokens);
                trailing_or_empty = pair.punct().is_some();
            }
        }
        for pair in generics.params.pairs() {
            if let GenericParam::Lifetime(_) = pair.value() {
                continue;
            }
            if !trailing_or_empty {
                Comma::default().to_tokens(tokens);
                trailing_or_empty = true;
            }
            match pair.value() {
                GenericParam::Lifetime(_) => unreachable!("lifetimes were skipped"),
                GenericParam::Type(param) => {
                    print_attributes(&param.attrs, tokens);
                    param.ident.to_tokens(tokens);
                    if !param.bounds.is_empty() {
                        param.colon_token.unwrap_or_default().to_tokens(tokens);
                        param.bounds.to_tokens(tokens);
                    }
                }
                GenericParam::Const(param) => {
                    print_attributes(&param.attrs, tokens);
                    param.const_token.to_tokens(tokens);
                    param.ident.to_tokens(tokens);
                    param.colon_token.to_tokens(tokens);
                    param.ty.to_tokens(tokens);
                }
            }
            pair.punct().to_tokens(tokens);
        }
        generics.gt_token.unwrap_or_default().to_tokens(tokens);
    }
}

impl ToTokens for TypeGenerics<'_> {
    fn to_tokens(&self, tokens: &mut Tokens) {
        let generics = self.0;
        if generics.params.is_empty() {
            return;
        }
        generics.lt_token.unwrap_or_default().to_tokens(tokens);
        let mut trailing_or_empty = true;
        for pair in generics.params.pairs() {
            if let GenericParam::Lifetime(param) = pair.value() {
                param.lifetime.to_tokens(tokens);
                pair.punct().to_tokens(tokens);
                trailing_or_empty = pair.punct().is_some();
            }
        }
        for pair in generics.params.pairs() {
            if let GenericParam::Lifetime(_) = pair.value() {
                continue;
            }
            if !trailing_or_empty {
                Comma::default().to_tokens(tokens);
                trailing_or_empty = true;
            }
            match pair.value() {
                GenericParam::Lifetime(_) => unreachable!("lifetimes were skipped"),
                GenericParam::Type(param) => param.ident.to_tokens(tokens),
                GenericParam::Const(param) => param.ident.to_tokens(tokens),
            }
            pair.punct().to_tokens(tokens);
        }
        generics.gt_token.unwrap_or_default().to_tokens(tokens);
    }
}

impl ToTokens for Turbofish<'_> {
    fn to_tokens(&self, tokens: &mut Tokens) {
        if !self.0.params.is_empty() {
            PathSep::default().to_tokens(tokens);
            TypeGenerics(self.0).to_tokens(tokens);
        }
    }
}

impl ToTokens for WhereClause {
    fn to_tokens(&self, tokens: &mut Tokens) {
        if !self.predicates.is_empty() {
            self.where_token.to_tokens(tokens);
            self.predicates.to_tokens(tokens);
        }
    }
}

impl ToTokens for WherePredicate {
    fn to_tokens(&self, tokens: &mut Tokens) {
        match self {
            Self::Lifetime(predicate) => {
                predicate.lifetime.to_tokens(tokens);
                predicate.colon_token.to_tokens(tokens);
                predicate.bounds.to_tokens(tokens);
            }
            Self::Type(predicate) => {
                predicate.lifetimes.to_tokens(tokens);
                predicate.bounded_ty.to_tokens(tokens);
                predicate.colon_token.to_tokens(tokens);
                predicate.bounds.to_tokens(tokens);
            }
        }
    }
}

impl ToTokens for BoundLifetimes {
    fn to_tokens(&self, tokens: &mut Tokens) {
        self.for_token.to_tokens(tokens);
        self.lt_token.to_tokens(tokens);
        self.lifetimes.to_tokens(tokens);
        self.gt_token.to_tokens(tokens);
    }
}
