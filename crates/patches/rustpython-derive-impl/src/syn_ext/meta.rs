use crate::syn_ext::attribute::AttributeExt;
use crate::syn_ext::ident::GetIdent;
use crate::syn_ext::path::GetPath;
use nanocode_macro_support::Meta as Meta2;
use nanocode_macro_support::{
    Attribute, Delimiter, Error, Expr, ExprLit, Group, Ident, Lit, Parse, ParseStream, Path,
    PathSegment, Punctuated, Result, Span, ToTokens, Token, TokenTree, Tokens, parse_optional,
    parse_terminated, token,
};
use std::collections::HashMap as Map;

#[derive(Clone)]
pub struct MetaNameValue1 {
    pub path: Path,
    pub eq_token: token::Eq,
    pub value: Expr,
}

impl ToTokens for MetaNameValue1 {
    fn to_tokens(&self, tokens: &mut Tokens) {
        self.path.to_tokens(tokens);
        self.eq_token.to_tokens(tokens);
        self.value.to_tokens(tokens);
    }
}

#[derive(Clone)]
pub enum Meta1 {
    Path(Path),
    List(MetaList1),
    NameValue(MetaNameValue1),
}
impl Parse for Meta1 {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let path = parse_meta_path(input)?;
        if input.peek_group(Delimiter::Parenthesis) {
            Ok(Meta1::List(MetaList1::parse_meta_list_after_path(
                path, input,
            )?))
        } else if input.peek_punct('=') {
            Ok(Meta1::NameValue(MetaNameValue1 {
                path,
                eq_token: token::Eq::parse_token(input)?,
                value: degroup(Expr::parse(input)?),
            }))
        } else {
            Ok(Meta1::Path(path))
        }
    }
}
fn degroup(mut expr: Expr) -> Expr {
    while let Expr::Group(group) = expr {
        expr = *group.expr
    }
    expr
}
impl std::convert::TryFrom<Meta2> for Meta1 {
    type Error = Error;

    fn try_from(meta: Meta2) -> std::result::Result<Self, Self::Error> {
        Ok(match meta {
            Meta2::Path(path) => Meta1::Path(path),
            Meta2::List(list) => {
                if list.delimiter != Delimiter::Parenthesis {
                    return Err(Error::new(list.delimiter_span, "expected paren"));
                }
                Meta1::List(MetaList1 {
                    path: list.path.clone(),
                    paren_span: list.delimiter_span,
                    nested: list.parse_args_with(|input| {
                        parse_terminated::<NestedMeta, token::Comma>(input, NestedMeta::parse)
                    })?,
                })
            }
            Meta2::NameValue(nv) => Meta1::NameValue(MetaNameValue1 {
                path: nv.path,
                eq_token: nv.eq_token,
                value: Expr::from_meta_value(&nv.value),
            }),
        })
    }
}

impl ToTokens for Meta1 {
    fn to_tokens(&self, tokens: &mut Tokens) {
        match self {
            Meta1::Path(path) => path.to_tokens(tokens),
            Meta1::List(list) => list.to_tokens(tokens),
            Meta1::NameValue(nv) => nv.to_tokens(tokens),
        }
    }
}

impl Meta1 {
    pub fn path(&self) -> &Path {
        match self {
            Meta1::Path(path) => path,
            Meta1::List(list) => &list.path,
            Meta1::NameValue(nv) => &nv.path,
        }
    }
}

fn peek_any_ident_at(input: ParseStream<'_>, offset: usize) -> bool {
    match input.peek_tree(offset) {
        Some(TokenTree::Ident(_)) => true,
        Some(TokenTree::Group(group)) if group.delimiter() == Delimiter::None => {
            let mut trees = group.stream().into_iter();
            matches!(
                (trees.next(), trees.next()),
                (Some(TokenTree::Ident(_)), None)
            )
        }
        _ => false,
    }
}

fn peek_bool(input: ParseStream<'_>) -> bool {
    matches!(input.peek_tree(0), Some(TokenTree::Ident(ident)) if matches!(ident.to_string().as_str(), "true" | "false"))
}

fn parse_meta_path(input: ParseStream<'_>) -> Result<Path> {
    Ok(Path {
        leading_colon: parse_optional::<token::PathSep>(input)?,
        segments: {
            let mut segments = Punctuated::new();
            while peek_any_ident_at(input, 0) {
                let ident = input.parse_any_ident()?;
                segments.push_value(PathSegment::from(ident));
                if !input.peek_joint("::") {
                    break;
                }
                let punct = token::PathSep::parse_token(input)?;
                segments.push_punct(punct);
            }
            if segments.is_empty() {
                return Err(input.error("expected path"));
            } else if segments.trailing_punct() {
                return Err(input.error("expected path segment"));
            }
            segments
        },
    })
}

#[derive(Clone)]
pub struct MetaList1 {
    pub path: Path,
    pub paren_span: Span,
    pub nested: PunctuatedNestedMeta,
}
impl MetaList1 {
    fn parse_meta_list_after_path(path: Path, input: ParseStream<'_>) -> Result<Self> {
        let group = input.parse_group(Delimiter::Parenthesis)?;
        let content = nanocode_macro_support::ParseBuffer::from_group(&group);
        let nested = parse_terminated::<NestedMeta, token::Comma>(&content, NestedMeta::parse)?;
        Ok(MetaList1 {
            path,
            paren_span: group.span(),
            nested,
        })
    }
}
impl Parse for MetaList1 {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let path = parse_meta_path(input)?;
        Self::parse_meta_list_after_path(path, input)
    }
}

impl ToTokens for MetaList1 {
    fn to_tokens(&self, tokens: &mut Tokens) {
        self.path.to_tokens(tokens);
        let mut group = Group::new(
            Delimiter::Parenthesis,
            self.nested.to_token_stream().into_stream(),
        );
        group.set_span(self.paren_span);
        tokens.push(group);
    }
}

#[derive(Clone)]
pub enum NestedMeta {
    Meta(Meta1),
    Lit(Lit),
}
impl Parse for NestedMeta {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let literal = input.peek_literal() || peek_bool(input);
        if literal && !(peek_bool(input) && input.peek_punct_at(1, '=')) {
            Lit::parse(input).map(NestedMeta::Lit)
        } else if peek_any_ident_at(input, 0)
            || input.peek_joint("::") && peek_any_ident_at(input, 2)
        {
            Meta1::parse(input).map(NestedMeta::Meta)
        } else {
            Err(input.error("expected identifier or literal"))
        }
    }
}

impl ToTokens for NestedMeta {
    fn to_tokens(&self, tokens: &mut Tokens) {
        match self {
            NestedMeta::Meta(meta) => meta.to_tokens(tokens),
            NestedMeta::Lit(lit) => lit.to_tokens(tokens),
        }
    }
}

pub type PunctuatedNestedMeta = Punctuated<NestedMeta, token::Comma>;

pub trait MetaExt {
    fn is_path(&self) -> bool;
    fn is_list(&self) -> bool;
    fn is_name_value(&self) -> bool;
    fn is_doc(&self) -> bool;

    fn promote_to_list(&mut self, paren: Span) -> Result<&mut MetaList1>;

    fn list(&self) -> Result<&MetaList1>;
    fn list_mut(&mut self) -> Result<&mut MetaList1>;

    fn name_value(&self) -> Result<&MetaNameValue1>;
    fn name_value_mut(&mut self) -> Result<&mut MetaNameValue1>;

    fn doc(&self) -> Result<String>;
}

pub(crate) fn err_promote_to_list(meta: &Meta1) -> Error {
    Error::new_spanned(
        meta,
        "Only Path can be promoted and List is accepted as non-promoted",
    )
}

impl MetaExt for Meta1 {
    fn is_path(&self) -> bool {
        matches!(self, Meta1::Path(_))
    }
    fn is_list(&self) -> bool {
        matches!(self, Meta1::List(_))
    }
    fn is_name_value(&self) -> bool {
        matches!(self, Meta1::NameValue(_))
    }
    fn is_doc(&self) -> bool {
        self.name_value().is_ok_and(|v| {
            v.path.is_ident("doc")
                && matches!(
                    v.value,
                    Expr::Lit(ExprLit {
                        lit: Lit::Str(_),
                        ..
                    })
                )
        })
    }

    fn promote_to_list(&mut self, paren: Span) -> Result<&mut MetaList1> {
        let path = match self {
            Meta1::Path(path) => path.clone(),
            Meta1::List(metalist) => return Ok(metalist),
            other => return Err(err_promote_to_list(other)),
        };
        *self = Meta1::List(MetaList1 {
            path,
            paren_span: paren,
            nested: PunctuatedNestedMeta::new(),
        });
        self.list_mut()
    }

    fn list(&self) -> Result<&MetaList1> {
        match self {
            Meta1::List(list) => Ok(list),
            other => Err(Error::new_spanned(other, "Not a List meta")),
        }
    }
    fn list_mut(&mut self) -> Result<&mut MetaList1> {
        match self {
            Meta1::List(list) => Ok(list),
            other => Err(Error::new_spanned(other, "Not a List meta")),
        }
    }

    fn name_value(&self) -> Result<&MetaNameValue1> {
        match self {
            Meta1::NameValue(name) => Ok(name),
            other => Err(Error::new_spanned(other, "Not a NameValue meta")),
        }
    }
    fn name_value_mut(&mut self) -> Result<&mut MetaNameValue1> {
        match self {
            Meta1::NameValue(name) => Ok(name),
            other => Err(Error::new_spanned(other, "Not a NameValue meta")),
        }
    }

    fn doc(&self) -> Result<String> {
        let name_value = self.name_value()?;
        if !name_value.path.is_ident("doc") {
            return Err(Error::new_spanned(name_value, "Not a doc meta"));
        }
        match &name_value.value {
            Expr::Lit(ExprLit {
                lit: Lit::Str(lit), ..
            }) => Ok(lit.value().trim().to_owned()),
            other => Err(Error::new_spanned(other, "Doc meta expects string literal")),
        }
    }
}

type IndexMetaRef<M> = (usize, M);
type MultiMetaMap<'a, K, M> = Map<K, Vec<IndexMetaRef<M>>>;
type UniqueMetaMap<'a, K, M> = Map<K, IndexMetaRef<M>>;

pub trait MetaIteratorExt<'a, M>
where
    M: 'a + std::borrow::Borrow<Meta1>,
{
    fn to_multi_map<K, KF>(self, path_to_key: KF) -> Result<MultiMetaMap<'a, K, M>>
    where
        K: std::hash::Hash + Eq,
        KF: Fn(&Path) -> Result<Option<K>>;

    fn to_unique_map<K, KF>(self, path_to_key: KF) -> Result<UniqueMetaMap<'a, K, M>>
    where
        K: std::hash::Hash + Eq,
        KF: Fn(&Path) -> Result<Option<K>>;
}

impl<'a, I, M> MetaIteratorExt<'a, M> for I
where
    M: 'a + std::borrow::Borrow<Meta1>,
    I: std::iter::Iterator<Item = IndexMetaRef<M>>,
{
    fn to_multi_map<K, KF>(self, path_to_key: KF) -> Result<MultiMetaMap<'a, K, M>>
    where
        K: std::hash::Hash + Eq,
        KF: Fn(&Path) -> Result<Option<K>>,
    {
        let mut map: Map<K, Vec<_>> = Map::new();
        for (i, meta) in self {
            let path = meta.borrow().path();
            let key = if let Some(key) = path_to_key(path)? {
                key
            } else {
                continue;
            };
            map.entry(key).or_default().push((i, meta))
        }
        Ok(map)
    }

    fn to_unique_map<K, KF>(self, path_to_key: KF) -> Result<UniqueMetaMap<'a, K, M>>
    where
        K: std::hash::Hash + Eq,
        KF: Fn(&Path) -> Result<Option<K>>,
    {
        let mut map = Map::new();
        for (i, meta) in self {
            let path = meta.borrow().path();
            let key = if let Some(key) = path_to_key(path)? {
                key
            } else {
                continue;
            };
            if let Some((_, removed)) = map.insert(key, (i, meta)) {
                return Err(Error::new_spanned(
                    removed.borrow(),
                    "this attribute path must be unique in the attribute",
                ));
            }
        }
        Ok(map)
    }
}

#[allow(clippy::type_complexity)]
pub trait NestedMetaRefIteratorExt<'a, M>
where
    M: 'a + std::borrow::Borrow<Meta1>,
{
    fn to_multi_map_and_lits<K, KF>(
        self,
        path_to_key: KF,
    ) -> Result<(MultiMetaMap<'a, K, M>, Vec<(usize, &'a Lit)>)>
    where
        K: std::hash::Hash + Eq,
        KF: Fn(&Path) -> Result<Option<K>>;

    fn to_unique_map_and_lits<K, KF>(
        self,
        path_to_key: KF,
    ) -> Result<(UniqueMetaMap<'a, K, M>, Vec<(usize, &'a Lit)>)>
    where
        K: std::hash::Hash + Eq,
        KF: Fn(&Path) -> Result<Option<K>>;
}

#[allow(clippy::type_complexity)]
impl<'a, I> NestedMetaRefIteratorExt<'a, &'a Meta1> for I
where
    I: std::iter::IntoIterator<Item = &'a NestedMeta>,
{
    fn to_multi_map_and_lits<K, KF>(
        self,
        path_to_key: KF,
    ) -> Result<(MultiMetaMap<'a, K, &'a Meta1>, Vec<(usize, &'a Lit)>)>
    where
        K: std::hash::Hash + Eq,
        KF: Fn(&Path) -> Result<Option<K>>,
    {
        let mut metas = Vec::new();
        let mut lits = Vec::new();

        for (i, nmeta) in self.into_iter().enumerate() {
            match nmeta {
                NestedMeta::Meta(meta) => metas.push((i, meta)),
                NestedMeta::Lit(lit) => lits.push((i, lit)),
            }
        }

        let map = metas.into_iter().to_multi_map(path_to_key)?;
        Ok((map, lits))
    }

    fn to_unique_map_and_lits<K, KF>(
        self,
        path_to_key: KF,
    ) -> Result<(UniqueMetaMap<'a, K, &'a Meta1>, Vec<(usize, &'a Lit)>)>
    where
        K: std::hash::Hash + Eq,
        KF: Fn(&Path) -> Result<Option<K>>,
    {
        let mut metas = Vec::new();
        let mut lits = Vec::new();

        for (i, nmeta) in self.into_iter().enumerate() {
            match nmeta {
                NestedMeta::Meta(meta) => metas.push((i, meta)),
                NestedMeta::Lit(lit) => lits.push((i, lit)),
            }
        }
        let map = metas.into_iter().to_unique_map(path_to_key)?;
        Ok((map, lits))
    }
}

#[allow(clippy::type_complexity)]
pub trait NestedMetaIteratorExt<'a> {
    fn into_multi_map_and_lits<K, KF>(
        self,
        path_to_key: KF,
    ) -> Result<(MultiMetaMap<'a, K, Meta1>, Vec<(usize, Lit)>)>
    where
        K: std::hash::Hash + Eq,
        KF: Fn(&Path) -> Result<Option<K>>;

    fn into_unique_map_and_lits<K, KF>(
        self,
        path_to_key: KF,
    ) -> Result<(UniqueMetaMap<'a, K, Meta1>, Vec<(usize, Lit)>)>
    where
        K: std::hash::Hash + Eq,
        KF: Fn(&Path) -> Result<Option<K>>;
}

#[allow(clippy::type_complexity)]
impl<'a, I> NestedMetaIteratorExt<'a> for I
where
    I: std::iter::IntoIterator<Item = NestedMeta>,
{
    fn into_multi_map_and_lits<K, KF>(
        self,
        path_to_key: KF,
    ) -> Result<(MultiMetaMap<'a, K, Meta1>, Vec<(usize, Lit)>)>
    where
        K: std::hash::Hash + Eq,
        KF: Fn(&Path) -> Result<Option<K>>,
    {
        let mut metas = Vec::new();
        let mut lits = Vec::new();

        for (i, nmeta) in self.into_iter().enumerate() {
            match nmeta {
                NestedMeta::Meta(meta) => metas.push((i, meta)),
                NestedMeta::Lit(lit) => lits.push((i, lit)),
            }
        }

        let map = metas.into_iter().to_multi_map(path_to_key)?;
        Ok((map, lits))
    }

    fn into_unique_map_and_lits<K, KF>(
        self,
        path_to_key: KF,
    ) -> Result<(UniqueMetaMap<'a, K, Meta1>, Vec<(usize, Lit)>)>
    where
        K: std::hash::Hash + Eq,
        KF: Fn(&Path) -> Result<Option<K>>,
    {
        let mut metas = Vec::new();
        let mut lits = Vec::new();

        for (i, nmeta) in self.into_iter().enumerate() {
            match nmeta {
                NestedMeta::Meta(meta) => metas.push((i, meta)),
                NestedMeta::Lit(lit) => lits.push((i, lit)),
            }
        }
        let map = metas.into_iter().to_unique_map(path_to_key)?;
        Ok((map, lits))
    }
}

#[allow(dead_code)]
#[allow(clippy::type_complexity)]
pub trait MetaAttributeExt<'a> {
    fn to_multi_map_and_attrs<K, KF>(
        self,
        path_to_key: KF,
    ) -> Result<(MultiMetaMap<'a, K, Meta1>, Vec<(usize, &'a Attribute)>)>
    where
        K: std::hash::Hash + Eq,
        KF: Fn(&Path) -> Result<Option<K>>;

    fn to_unique_map_and_attrs<K, KF>(
        self,
        path_to_key: KF,
    ) -> Result<(UniqueMetaMap<'a, K, Meta1>, Vec<(usize, &'a Attribute)>)>
    where
        K: std::hash::Hash + Eq,
        KF: Fn(&Path) -> Result<Option<K>>;
}
#[allow(clippy::type_complexity)]
impl<'a, I> MetaAttributeExt<'a> for I
where
    I: std::iter::IntoIterator<Item = &'a Attribute>,
{
    fn to_multi_map_and_attrs<K, KF>(
        self,
        path_to_key: KF,
    ) -> Result<(MultiMetaMap<'a, K, Meta1>, Vec<(usize, &'a Attribute)>)>
    where
        K: std::hash::Hash + Eq,
        KF: Fn(&Path) -> Result<Option<K>>,
    {
        let mut metas = Vec::new();
        let mut attrs = Vec::new();

        for (i, attr) in self.into_iter().enumerate() {
            match attr.parse_meta() {
                Ok(meta) => metas.push((i, meta)),
                Err(_) => attrs.push((i, attr)),
            }
        }

        let map = metas.into_iter().to_multi_map(path_to_key)?;
        Ok((map, attrs))
    }

    fn to_unique_map_and_attrs<K, KF>(
        self,
        path_to_key: KF,
    ) -> Result<(UniqueMetaMap<'a, K, Meta1>, Vec<(usize, &'a Attribute)>)>
    where
        K: std::hash::Hash + Eq,
        KF: Fn(&Path) -> Result<Option<K>>,
    {
        let mut metas = Vec::new();
        let mut attrs = Vec::new();

        for (i, attr) in self.into_iter().enumerate() {
            match attr.parse_meta() {
                Ok(meta) => metas.push((i, meta)),
                Err(_) => attrs.push((i, attr)),
            }
        }
        let map = metas.into_iter().to_unique_map(path_to_key)?;
        Ok((map, attrs))
    }
}

impl GetPath for NestedMeta {
    fn get_path(&self) -> Option<&Path> {
        match self {
            NestedMeta::Meta(meta) => Some(meta.path()),
            NestedMeta::Lit(_) => None,
        }
    }
}

impl GetIdent for Meta1 {
    fn get_ident(&self) -> Option<&Ident> {
        self.path().get_ident()
    }
}
