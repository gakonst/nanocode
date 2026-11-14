use std::hash::{Hash, Hasher};

use proc_macro::{Delimiter, Literal, Span, TokenTree};

use crate::attribute::Attribute;
use crate::buffer::{Parse, ParseBuffer, ParseStream};
use crate::error::{Error, Result};
use crate::generics::{Generics, WhereClause};
use crate::ident::Ident;
use crate::punctuated::{Colon, Comma, Eq, Punctuated, Semi, Token};
use crate::tokens::{ToTokens, Tokens, push_group};
use crate::ty::{Type, parse_type};

#[derive(Clone)]
pub struct DeriveInput {
    pub attrs: Vec<Attribute>,
    pub vis: Visibility,
    pub ident: Ident,
    pub generics: Generics,
    pub data: Data,
}

#[derive(Clone)]
pub enum Data {
    Struct(DataStruct),
    Enum(DataEnum),
    Union(DataUnion),
}

#[derive(Clone)]
pub struct DataStruct {
    pub struct_token: Ident,
    pub fields: Fields,
    pub semi_token: Option<Semi>,
}

#[derive(Clone)]
pub struct DataEnum {
    pub enum_token: Ident,
    pub brace_span: Span,
    pub variants: Punctuated<Variant, Comma>,
}

#[derive(Clone)]
pub struct DataUnion {
    pub union_token: Ident,
    pub fields: Fields,
}

#[derive(Clone)]
pub enum Fields {
    Named(FieldsNamed),
    Unnamed(FieldsUnnamed),
    Unit,
}

#[derive(Clone)]
pub struct FieldsNamed {
    pub brace_span: Span,
    pub named: Punctuated<Field, Comma>,
}

#[derive(Clone)]
pub struct FieldsUnnamed {
    pub paren_span: Span,
    pub unnamed: Punctuated<Field, Comma>,
}

#[derive(Clone)]
pub struct Field {
    pub attrs: Vec<Attribute>,
    pub vis: Visibility,
    pub ident: Option<Ident>,
    pub colon_token: Option<Colon>,
    pub ty: Type,
}

#[derive(Clone)]
pub struct Variant {
    pub attrs: Vec<Attribute>,
    pub ident: Ident,
    pub fields: Fields,
    pub discriminant: Option<(Eq, Tokens)>,
}

#[derive(Clone)]
pub enum Visibility {
    Public(Ident),
    Restricted(Ident, Span, Tokens),
    Inherited,
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub enum Member {
    Named(Ident),
    Unnamed(Index),
}

#[derive(Clone)]
pub struct Index {
    pub index: u32,
    pub span: Span,
}

impl From<usize> for Index {
    fn from(index: usize) -> Self {
        let index = u32::try_from(index).expect("a field index fits in u32");
        Self {
            index,
            span: Span::call_site(),
        }
    }
}

impl PartialEq for Index {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index
    }
}

impl std::cmp::Eq for Index {}

impl Hash for Index {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.index.hash(state);
    }
}

impl ToTokens for Index {
    fn to_tokens(&self, tokens: &mut Tokens) {
        let mut literal = Literal::i64_unsuffixed(i64::from(self.index));
        literal.set_span(self.span);
        tokens.push(literal);
    }
}

impl ToTokens for Member {
    fn to_tokens(&self, tokens: &mut Tokens) {
        match self {
            Self::Named(ident) => ident.to_tokens(tokens),
            Self::Unnamed(index) => index.to_tokens(tokens),
        }
    }
}

impl Fields {
    pub fn iter(&self) -> Box<dyn Iterator<Item = &Field> + '_> {
        match self {
            Self::Named(fields) => Box::new(fields.named.iter()),
            Self::Unnamed(fields) => Box::new(fields.unnamed.iter()),
            Self::Unit => Box::new(std::iter::empty()),
        }
    }

    pub fn iter_mut(&mut self) -> Box<dyn Iterator<Item = &mut Field> + '_> {
        match self {
            Self::Named(fields) => Box::new(fields.named.iter_mut()),
            Self::Unnamed(fields) => Box::new(fields.unnamed.iter_mut()),
            Self::Unit => Box::new(std::iter::empty()),
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Self::Named(fields) => fields.named.len(),
            Self::Unnamed(fields) => fields.unnamed.len(),
            Self::Unit => 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<'a> IntoIterator for &'a Fields {
    type Item = &'a Field;
    type IntoIter = Box<dyn Iterator<Item = &'a Field> + 'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a> IntoIterator for &'a mut Fields {
    type Item = &'a mut Field;
    type IntoIter = Box<dyn Iterator<Item = &'a mut Field> + 'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

impl Parse for Visibility {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let Some(pub_token) = input.eat_keyword("pub") else {
            return Ok(Self::Inherited);
        };
        if let Some(TokenTree::Group(group)) = input.peek_tree(0)
            && group.delimiter() == Delimiter::Parenthesis
        {
            let content = ParseBuffer::from_group(group);
            let restricted = if content.peek_keyword("crate")
                || content.peek_keyword("self")
                || content.peek_keyword("super")
            {
                content.next_tree();
                content.is_empty()
            } else {
                content.peek_keyword("in")
            };
            if restricted {
                let span = group.span();
                let tokens = Tokens::from_stream(group.stream());
                input.next_tree();
                return Ok(Self::Restricted(pub_token, span, tokens));
            }
        }
        Ok(Self::Public(pub_token))
    }
}

impl ToTokens for Visibility {
    fn to_tokens(&self, tokens: &mut Tokens) {
        match self {
            Self::Public(pub_token) => pub_token.to_tokens(tokens),
            Self::Restricted(pub_token, span, inner) => {
                pub_token.to_tokens(tokens);
                push_group(tokens, *span, Delimiter::Parenthesis, inner.clone());
            }
            Self::Inherited => {}
        }
    }
}

fn parse_named_fields(input: ParseStream<'_>) -> Result<FieldsNamed> {
    let group = input.parse_group(Delimiter::Brace)?;
    let content = ParseBuffer::from_group(&group);
    let mut named = Punctuated::new();
    while !content.is_empty() {
        let attrs = Attribute::parse_outer(&content)?;
        let vis = Visibility::parse(&content)?;
        let ident = content.parse_any_ident()?;
        let colon_token = Some(Colon::parse_token(&content)?);
        let ty = parse_type(&content, true)?;
        named.push_value(Field {
            attrs,
            vis,
            ident: Some(ident),
            colon_token,
            ty,
        });
        if content.is_empty() {
            break;
        }
        named.push_punct(Comma::parse_token(&content)?);
    }
    Ok(FieldsNamed {
        brace_span: group.span(),
        named,
    })
}

fn parse_unnamed_fields(input: ParseStream<'_>) -> Result<FieldsUnnamed> {
    let group = input.parse_group(Delimiter::Parenthesis)?;
    let content = ParseBuffer::from_group(&group);
    let mut unnamed = Punctuated::new();
    while !content.is_empty() {
        let attrs = Attribute::parse_outer(&content)?;
        let vis = Visibility::parse(&content)?;
        let ty = parse_type(&content, true)?;
        unnamed.push_value(Field {
            attrs,
            vis,
            ident: None,
            colon_token: None,
            ty,
        });
        if content.is_empty() {
            break;
        }
        unnamed.push_punct(Comma::parse_token(&content)?);
    }
    Ok(FieldsUnnamed {
        paren_span: group.span(),
        unnamed,
    })
}

fn parse_variant(input: ParseStream<'_>) -> Result<Variant> {
    let attrs = Attribute::parse_outer(input)?;
    Visibility::parse(input)?;
    let ident = input.parse_any_ident()?;
    let fields = if input.peek_group(Delimiter::Brace) {
        Fields::Named(parse_named_fields(input)?)
    } else if input.peek_group(Delimiter::Parenthesis) {
        Fields::Unnamed(parse_unnamed_fields(input)?)
    } else {
        Fields::Unit
    };
    let discriminant = if input.peek_punct('=') {
        let eq_token = Eq::parse_token(input)?;
        Some((eq_token, input.take_until_comma()))
    } else {
        None
    };
    Ok(Variant {
        attrs,
        ident,
        fields,
        discriminant,
    })
}

impl Parse for DeriveInput {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let attrs = Attribute::parse_outer(input)?;
        let vis = Visibility::parse(input)?;
        let keyword = input.parse_any_ident()?;
        let ident = input.parse_any_ident()?;
        let mut generics = Generics::parse(input)?;
        let data = match keyword.text() {
            "struct" => {
                let mut where_clause = WhereClause::parse_optional(input)?;
                let (fields, semi_token) =
                    if where_clause.is_none() && input.peek_group(Delimiter::Parenthesis) {
                        let fields = Fields::Unnamed(parse_unnamed_fields(input)?);
                        where_clause = WhereClause::parse_optional(input)?;
                        (fields, Some(Semi::parse_token(input)?))
                    } else if input.peek_group(Delimiter::Brace) {
                        (Fields::Named(parse_named_fields(input)?), None)
                    } else {
                        (Fields::Unit, Some(Semi::parse_token(input)?))
                    };
                generics.where_clause = where_clause;
                Data::Struct(DataStruct {
                    struct_token: keyword,
                    fields,
                    semi_token,
                })
            }
            "enum" => {
                generics.where_clause = WhereClause::parse_optional(input)?;
                let group = input.parse_group(Delimiter::Brace)?;
                let content = ParseBuffer::from_group(&group);
                let mut variants = Punctuated::new();
                while !content.is_empty() {
                    variants.push_value(parse_variant(&content)?);
                    if content.is_empty() {
                        break;
                    }
                    variants.push_punct(Comma::parse_token(&content)?);
                }
                Data::Enum(DataEnum {
                    enum_token: keyword,
                    brace_span: group.span(),
                    variants,
                })
            }
            "union" => {
                generics.where_clause = WhereClause::parse_optional(input)?;
                let fields = Fields::Named(parse_named_fields(input)?);
                Data::Union(DataUnion {
                    union_token: keyword,
                    fields,
                })
            }
            _ => {
                return Err(Error::new(
                    keyword.span(),
                    "expected `struct`, `enum` or `union`",
                ));
            }
        };
        Ok(Self {
            attrs,
            vis,
            ident,
            generics,
            data,
        })
    }
}

impl ToTokens for Fields {
    fn to_tokens(&self, tokens: &mut Tokens) {
        match self {
            Self::Named(fields) => push_group(
                tokens,
                fields.brace_span,
                Delimiter::Brace,
                fields.named.to_token_stream(),
            ),
            Self::Unnamed(fields) => {
                push_group(
                    tokens,
                    fields.paren_span,
                    Delimiter::Parenthesis,
                    fields.unnamed.to_token_stream(),
                );
            }
            Self::Unit => {}
        }
    }
}

impl ToTokens for Field {
    fn to_tokens(&self, tokens: &mut Tokens) {
        for attribute in &self.attrs {
            attribute.to_tokens(tokens);
        }
        self.vis.to_tokens(tokens);
        if let Some(ident) = &self.ident {
            ident.to_tokens(tokens);
            self.colon_token.unwrap_or_default().to_tokens(tokens);
        }
        self.ty.to_tokens(tokens);
    }
}

impl ToTokens for Variant {
    fn to_tokens(&self, tokens: &mut Tokens) {
        for attribute in &self.attrs {
            attribute.to_tokens(tokens);
        }
        self.ident.to_tokens(tokens);
        self.fields.to_tokens(tokens);
        if let Some((eq_token, value)) = &self.discriminant {
            eq_token.to_tokens(tokens);
            value.to_tokens(tokens);
        }
    }
}

impl ToTokens for DeriveInput {
    fn to_tokens(&self, tokens: &mut Tokens) {
        for attribute in &self.attrs {
            attribute.to_tokens(tokens);
        }
        self.vis.to_tokens(tokens);
        match &self.data {
            Data::Struct(data) => {
                data.struct_token.to_tokens(tokens);
                self.ident.to_tokens(tokens);
                self.generics.to_tokens(tokens);
                match &data.fields {
                    Fields::Named(_) => {
                        self.generics.where_clause.to_tokens(tokens);
                        data.fields.to_tokens(tokens);
                    }
                    Fields::Unnamed(_) => {
                        data.fields.to_tokens(tokens);
                        self.generics.where_clause.to_tokens(tokens);
                        data.semi_token.unwrap_or_default().to_tokens(tokens);
                    }
                    Fields::Unit => {
                        self.generics.where_clause.to_tokens(tokens);
                        data.semi_token.unwrap_or_default().to_tokens(tokens);
                    }
                }
            }
            Data::Enum(data) => {
                data.enum_token.to_tokens(tokens);
                self.ident.to_tokens(tokens);
                self.generics.to_tokens(tokens);
                self.generics.where_clause.to_tokens(tokens);
                push_group(
                    tokens,
                    data.brace_span,
                    Delimiter::Brace,
                    data.variants.to_token_stream(),
                );
            }
            Data::Union(data) => {
                data.union_token.to_tokens(tokens);
                self.ident.to_tokens(tokens);
                self.generics.to_tokens(tokens);
                self.generics.where_clause.to_tokens(tokens);
                data.fields.to_tokens(tokens);
            }
        }
    }
}
