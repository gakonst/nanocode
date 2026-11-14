use proc_macro::{Delimiter, Span, TokenTree};

use crate::attribute::Attribute;
use crate::buffer::{Parse, ParseBuffer, ParseStream};
use crate::data::{Data, DeriveInput, Fields, Variant, Visibility};
use crate::error::{Error, Result};
use crate::expression_tree::Expr;
use crate::generics::{Generics, WhereClause};
use crate::ident::Ident;
use crate::path::{Path, parse_optional};
use crate::punctuated::{Colon, Comma, Eq, Not, PathSep, Plus, Punctuated, Semi, Star, Token};
use crate::signature::{
    Block, Signature, peek_signature, print_inner_attributes, print_outer_attributes,
};
use crate::tokens::{ToTokens, Tokens, push_group};
use crate::ty::{Type, TypeParamBound, parse_type, parse_type_bounds};

#[derive(Clone)]
pub enum Item {
    Const(ItemConst),
    Enum(ItemEnum),
    ExternCrate(ItemExternCrate),
    Fn(ItemFn),
    ForeignMod(ItemForeignMod),
    Impl(ItemImpl),
    Macro(ItemMacro),
    Mod(ItemMod),
    Static(ItemStatic),
    Struct(ItemStruct),
    Trait(ItemTrait),
    TraitAlias(ItemTraitAlias),
    Type(ItemType),
    Union(ItemUnion),
    Use(ItemUse),
    Verbatim(Tokens),
}

#[derive(Clone)]
pub struct ItemConst {
    pub attrs: Vec<Attribute>,
    pub vis: Visibility,
    pub const_token: Ident,
    pub ident: Ident,
    pub colon_token: Colon,
    pub ty: Box<Type>,
    pub eq_token: Eq,
    pub expr: Box<Expr>,
    pub semi_token: Semi,
}

#[derive(Clone)]
pub struct ItemEnum {
    pub attrs: Vec<Attribute>,
    pub vis: Visibility,
    pub enum_token: Ident,
    pub ident: Ident,
    pub generics: Generics,
    pub brace_span: Span,
    pub variants: Punctuated<Variant, Comma>,
}

#[derive(Clone)]
pub struct ItemExternCrate {
    pub attrs: Vec<Attribute>,
    pub vis: Visibility,
    pub ident: Ident,
    pub tokens: Tokens,
}

#[derive(Clone)]
pub struct ItemFn {
    pub attrs: Vec<Attribute>,
    pub vis: Visibility,
    pub sig: Signature,
    pub block: Box<Block>,
}

#[derive(Clone)]
pub struct ItemForeignMod {
    pub attrs: Vec<Attribute>,
    pub tokens: Tokens,
}

#[derive(Clone)]
pub struct ItemImpl {
    pub attrs: Vec<Attribute>,
    pub defaultness: Option<Ident>,
    pub unsafety: Option<Ident>,
    pub impl_token: Ident,
    pub generics: Generics,
    pub trait_: Option<(Option<Not>, Path, Ident)>,
    pub self_ty: Box<Type>,
    pub brace_span: Span,
    pub items: Vec<ImplItem>,
}

#[derive(Clone)]
pub struct ItemMacro {
    pub attrs: Vec<Attribute>,
    pub ident: Option<Ident>,
    pub mac: Macro,
    pub semi_token: Option<Semi>,
}

#[derive(Clone)]
pub struct ItemMod {
    pub attrs: Vec<Attribute>,
    pub vis: Visibility,
    pub unsafety: Option<Ident>,
    pub mod_token: Ident,
    pub ident: Ident,
    pub content: Option<(Span, Vec<Item>)>,
    pub semi: Option<Semi>,
}

#[derive(Clone)]
pub struct ItemStatic {
    pub attrs: Vec<Attribute>,
    pub vis: Visibility,
    pub ident: Ident,
    pub tokens: Tokens,
}

#[derive(Clone)]
pub struct ItemStruct {
    pub attrs: Vec<Attribute>,
    pub vis: Visibility,
    pub struct_token: Ident,
    pub ident: Ident,
    pub generics: Generics,
    pub fields: Fields,
    pub semi_token: Option<Semi>,
}

#[derive(Clone)]
pub struct ItemTrait {
    pub attrs: Vec<Attribute>,
    pub vis: Visibility,
    pub unsafety: Option<Ident>,
    pub auto_token: Option<Ident>,
    pub trait_token: Ident,
    pub ident: Ident,
    pub generics: Generics,
    pub colon_token: Option<Colon>,
    pub supertraits: Punctuated<TypeParamBound, Plus>,
    pub brace_span: Span,
    pub items: Vec<TraitItem>,
}

#[derive(Clone)]
pub struct ItemTraitAlias {
    pub attrs: Vec<Attribute>,
    pub vis: Visibility,
    pub ident: Ident,
    pub tokens: Tokens,
}

#[derive(Clone)]
pub struct ItemType {
    pub attrs: Vec<Attribute>,
    pub vis: Visibility,
    pub ident: Ident,
    pub tokens: Tokens,
}

#[derive(Clone)]
pub struct ItemUnion {
    pub attrs: Vec<Attribute>,
    pub vis: Visibility,
    pub ident: Ident,
    pub tokens: Tokens,
}

#[derive(Clone)]
pub struct ItemUse {
    pub attrs: Vec<Attribute>,
    pub vis: Visibility,
    pub use_token: Ident,
    pub leading_colon: Option<PathSep>,
    pub tree: UseTree,
    pub semi_token: Semi,
}

#[derive(Clone)]
pub enum UseTree {
    Path(UsePath),
    Name(UseName),
    Rename(UseRename),
    Glob(UseGlob),
    Group(UseGroup),
}

#[derive(Clone)]
pub struct UsePath {
    pub ident: Ident,
    pub colon2_token: PathSep,
    pub tree: Box<UseTree>,
}

#[derive(Clone)]
pub struct UseName {
    pub ident: Ident,
}

#[derive(Clone)]
pub struct UseRename {
    pub ident: Ident,
    pub as_token: Ident,
    pub rename: Ident,
}

#[derive(Clone)]
pub struct UseGlob {
    pub star_token: Star,
}

#[derive(Clone)]
pub struct UseGroup {
    pub brace_span: Span,
    pub items: Punctuated<UseTree, Comma>,
}

#[derive(Clone)]
pub struct Macro {
    pub path: Path,
    pub bang_token: Not,
    pub delimiter: Delimiter,
    pub delimiter_span: Span,
    pub tokens: Tokens,
}

#[derive(Clone)]
pub enum ImplItem {
    Const(ImplItemConst),
    Fn(ImplItemFn),
    Type(ImplItemType),
    Macro(ImplItemMacro),
    Verbatim(Tokens),
}

#[derive(Clone)]
pub struct ImplItemConst {
    pub attrs: Vec<Attribute>,
    pub vis: Visibility,
    pub defaultness: Option<Ident>,
    pub const_token: Ident,
    pub ident: Ident,
    pub colon_token: Colon,
    pub ty: Type,
    pub eq_token: Eq,
    pub expr: Expr,
    pub semi_token: Semi,
}

#[derive(Clone)]
pub struct ImplItemFn {
    pub attrs: Vec<Attribute>,
    pub vis: Visibility,
    pub defaultness: Option<Ident>,
    pub sig: Signature,
    pub block: Block,
}

#[derive(Clone)]
pub struct ImplItemType {
    pub attrs: Vec<Attribute>,
    pub ident: Ident,
    pub tokens: Tokens,
}

#[derive(Clone)]
pub struct ImplItemMacro {
    pub attrs: Vec<Attribute>,
    pub mac: Macro,
    pub semi_token: Option<Semi>,
}

#[derive(Clone)]
pub enum TraitItem {
    Const(TraitItemConst),
    Fn(TraitItemFn),
    Type(TraitItemType),
    Macro(TraitItemMacro),
    Verbatim(Tokens),
}

#[derive(Clone)]
pub struct TraitItemConst {
    pub attrs: Vec<Attribute>,
    pub const_token: Ident,
    pub ident: Ident,
    pub colon_token: Colon,
    pub ty: Type,
    pub default: Option<(Eq, Expr)>,
    pub semi_token: Semi,
}

#[derive(Clone)]
pub struct TraitItemFn {
    pub attrs: Vec<Attribute>,
    pub sig: Signature,
    pub default: Option<Block>,
    pub semi_token: Option<Semi>,
}

#[derive(Clone)]
pub struct TraitItemType {
    pub attrs: Vec<Attribute>,
    pub ident: Ident,
    pub tokens: Tokens,
}

#[derive(Clone)]
pub struct TraitItemMacro {
    pub attrs: Vec<Attribute>,
    pub mac: Macro,
    pub semi_token: Option<Semi>,
}

fn take_through_semicolon(input: ParseStream<'_>) -> Result<Tokens> {
    let mut tokens = Tokens::new();
    loop {
        let tree = input.parse_tree()?;
        let ends = matches!(&tree, TokenTree::Punct(punct) if punct.as_char() == ';');
        tokens.push(tree);
        if ends {
            return Ok(tokens);
        }
    }
}

fn take_through_brace(input: ParseStream<'_>) -> Result<Tokens> {
    let mut tokens = Tokens::new();
    loop {
        let tree = input.parse_tree()?;
        let ends =
            matches!(&tree, TokenTree::Group(group) if group.delimiter() == Delimiter::Brace);
        tokens.push(tree);
        if ends {
            return Ok(tokens);
        }
    }
}

fn ident_after(input: ParseStream<'_>, keywords: &[&str]) -> Result<Ident> {
    let start = input.position();
    for keyword in keywords {
        input.eat_keyword(keyword);
    }
    let ident = input.parse_any_ident();
    input.reset(start);
    ident
}

enum ItemKind {
    Const,
    Enum,
    ExternCrate,
    Fn,
    ForeignMod,
    Impl,
    Macro,
    Mod,
    Static,
    Struct,
    Trait,
    Type,
    Union,
    Use,
}

fn foreign_module_at(input: ParseStream<'_>, offset: usize) -> bool {
    if !input.peek_keyword_at(offset, "extern") {
        return false;
    }
    let after = if matches!(input.peek_tree(offset + 1), Some(TokenTree::Literal(_))) {
        offset + 2
    } else {
        offset + 1
    };
    matches!(input.peek_tree(after), Some(TokenTree::Group(group)) if group.delimiter() == Delimiter::Brace)
}

fn item_kind(input: ParseStream<'_>) -> Option<ItemKind> {
    if foreign_module_at(input, 0) || (input.peek_keyword("unsafe") && foreign_module_at(input, 1))
    {
        return Some(ItemKind::ForeignMod);
    }
    if peek_signature(input) {
        return Some(ItemKind::Fn);
    }
    let first = match input.peek_tree(0) {
        Some(TokenTree::Ident(ident)) => ident.to_string(),
        Some(TokenTree::Punct(punct)) if punct.as_char() == ':' => return Some(ItemKind::Macro),
        _ => return None,
    };
    let second_is = |word: &str| input.peek_keyword_at(1, word);
    let kind = match first.as_str() {
        "use" => ItemKind::Use,
        "static" => ItemKind::Static,
        "const" => ItemKind::Const,
        "type" => ItemKind::Type,
        "struct" => ItemKind::Struct,
        "enum" => ItemKind::Enum,
        "mod" => ItemKind::Mod,
        "impl" => ItemKind::Impl,
        "trait" | "auto" => ItemKind::Trait,
        "extern" if second_is("crate") => ItemKind::ExternCrate,
        "union" if matches!(input.peek_tree(1), Some(TokenTree::Ident(_))) => ItemKind::Union,
        "unsafe" if second_is("impl") => ItemKind::Impl,
        "unsafe" if second_is("trait") || second_is("auto") => ItemKind::Trait,
        "unsafe" if second_is("mod") => ItemKind::Mod,
        "default" if second_is("impl") || second_is("unsafe") => ItemKind::Impl,
        _ if input.peek_punct_at(1, '!') || input.peek_joint_at(1, "::") => ItemKind::Macro,
        _ => return None,
    };
    Some(kind)
}

impl Parse for Item {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let start = input.position();
        let attrs = Attribute::parse_outer(input)?;
        let vis = Visibility::parse(input)?;
        let Some(kind) = item_kind(input) else {
            return Err(input.error("expected item"));
        };
        let item = match kind {
            ItemKind::Struct | ItemKind::Enum => {
                input.reset(start);
                return parse_derive_input_item(input);
            }
            ItemKind::Const => Self::Const(parse_item_const(input, attrs, vis)?),
            ItemKind::ExternCrate => {
                let ident = ident_after(input, &["extern", "crate"])?;
                Self::ExternCrate(ItemExternCrate {
                    attrs,
                    vis,
                    ident,
                    tokens: take_through_semicolon(input)?,
                })
            }
            ItemKind::Fn => {
                let mut attrs = attrs;
                let sig = Signature::parse(input)?;
                let block = Box::new(Block::parse_with_inner(input, &mut attrs)?);
                Self::Fn(ItemFn {
                    attrs,
                    vis,
                    sig,
                    block,
                })
            }
            ItemKind::ForeignMod => {
                let mut tokens = vis.to_token_stream();
                tokens.extend(take_through_brace(input)?);
                Self::ForeignMod(ItemForeignMod { attrs, tokens })
            }
            ItemKind::Impl => Self::Impl(parse_item_impl(input, attrs)?),
            ItemKind::Macro => Self::Macro(parse_item_macro(input, attrs)?),
            ItemKind::Mod => Self::Mod(parse_item_mod(input, attrs, vis)?),
            ItemKind::Static => {
                let ident = ident_after(input, &["static", "mut"])?;
                Self::Static(ItemStatic {
                    attrs,
                    vis,
                    ident,
                    tokens: take_through_semicolon(input)?,
                })
            }
            ItemKind::Trait => parse_item_trait(input, attrs, vis)?,
            ItemKind::Type => {
                let ident = ident_after(input, &["type"])?;
                Self::Type(ItemType {
                    attrs,
                    vis,
                    ident,
                    tokens: take_through_semicolon(input)?,
                })
            }
            ItemKind::Union => {
                let ident = ident_after(input, &["union"])?;
                Self::Union(ItemUnion {
                    attrs,
                    vis,
                    ident,
                    tokens: take_through_brace(input)?,
                })
            }
            ItemKind::Use => Self::Use(parse_item_use(input, attrs, vis)?),
        };
        Ok(item)
    }
}

impl Parse for ItemFn {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let mut attrs = Attribute::parse_outer(input)?;
        let vis = Visibility::parse(input)?;
        let sig = Signature::parse(input)?;
        let block = Box::new(Block::parse_with_inner(input, &mut attrs)?);
        Ok(Self {
            attrs,
            vis,
            sig,
            block,
        })
    }
}

fn parse_derive_input_item(input: ParseStream<'_>) -> Result<Item> {
    let DeriveInput {
        attrs,
        vis,
        ident,
        generics,
        data,
    } = DeriveInput::parse(input)?;
    match data {
        Data::Struct(data) => Ok(Item::Struct(ItemStruct {
            attrs,
            vis,
            struct_token: data.struct_token,
            ident,
            generics,
            fields: data.fields,
            semi_token: data.semi_token,
        })),
        Data::Enum(data) => Ok(Item::Enum(ItemEnum {
            attrs,
            vis,
            enum_token: data.enum_token,
            ident,
            generics,
            brace_span: data.brace_span,
            variants: data.variants,
        })),
        Data::Union(data) => Err(Error::new(
            data.union_token.span(),
            "expected struct or enum",
        )),
    }
}

fn parse_item_const(
    input: ParseStream<'_>,
    attrs: Vec<Attribute>,
    vis: Visibility,
) -> Result<ItemConst> {
    let const_token = input.parse_keyword("const")?;
    let ident = input.parse_any_ident()?;
    let colon_token = Colon::parse_token(input)?;
    let ty = Box::new(parse_type(input, true)?);
    let eq_token = Eq::parse_token(input)?;
    let expr = Box::new(Expr::parse_until(input, ';'));
    let semi_token = Semi::parse_token(input)?;
    Ok(ItemConst {
        attrs,
        vis,
        const_token,
        ident,
        colon_token,
        ty,
        eq_token,
        expr,
        semi_token,
    })
}

fn parse_item_impl(input: ParseStream<'_>, mut attrs: Vec<Attribute>) -> Result<ItemImpl> {
    let defaultness = input.eat_keyword("default");
    let unsafety = input.eat_keyword("unsafe");
    let impl_token = input.parse_keyword("impl")?;
    let mut generics = if input.peek_punct('<') {
        Generics::parse(input)?
    } else {
        Generics::default()
    };
    let negative = input.peek_punct('!')
        && !matches!(input.peek_tree(1), Some(TokenTree::Group(group)) if group.delimiter() == Delimiter::Brace);
    let polarity = if negative {
        Some(Not::parse_token(input)?)
    } else {
        None
    };
    let first_ty = Type::without_plus(input)?;
    let (trait_, self_ty) = match input.eat_keyword("for") {
        Some(for_token) => {
            let mut trait_ty = first_ty;
            while let Type::Group(group) = trait_ty {
                trait_ty = *group.elem;
            }
            let Type::Path(trait_path) = trait_ty else {
                return Err(Error::new(
                    for_token.span(),
                    "expected a trait path before `for`",
                ));
            };
            if trait_path.qself.is_some() {
                return Err(Error::new(
                    for_token.span(),
                    "expected a trait path before `for`",
                ));
            }
            (
                Some((polarity, trait_path.path, for_token)),
                parse_type(input, true)?,
            )
        }
        None => {
            if let Some(polarity) = polarity {
                return Err(Error::new(
                    polarity.span,
                    "expected `for` after a negative impl",
                ));
            }
            (None, first_ty)
        }
    };
    generics.where_clause = WhereClause::parse_optional(input)?;
    let group = input.parse_group(Delimiter::Brace)?;
    let content = ParseBuffer::from_group(&group);
    attrs.extend(Attribute::parse_inner(&content)?);
    let mut items = Vec::new();
    while !content.is_empty() {
        items.push(ImplItem::parse(&content)?);
    }
    Ok(ItemImpl {
        attrs,
        defaultness,
        unsafety,
        impl_token,
        generics,
        trait_,
        self_ty: Box::new(self_ty),
        brace_span: group.span(),
        items,
    })
}

impl Parse for Macro {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let path = Path::parse_mod_style(input)?;
        let bang_token = Not::parse_token(input)?;
        parse_macro_body(input, path, bang_token)
    }
}

fn parse_macro_body(input: ParseStream<'_>, path: Path, bang_token: Not) -> Result<Macro> {
    let group = match input.parse_tree()? {
        TokenTree::Group(group) if group.delimiter() != Delimiter::None => group,
        _ => return Err(Error::new(bang_token.span, "expected macro delimiter")),
    };
    Ok(Macro {
        path,
        bang_token,
        delimiter: group.delimiter(),
        delimiter_span: group.span(),
        tokens: Tokens::from_stream(group.stream()),
    })
}

fn macro_semicolon(input: ParseStream<'_>, mac: &Macro) -> Result<Option<Semi>> {
    if mac.delimiter == Delimiter::Brace {
        return Ok(None);
    }
    Semi::parse_token(input).map(Some)
}

fn parse_item_macro(input: ParseStream<'_>, attrs: Vec<Attribute>) -> Result<ItemMacro> {
    let path = Path::parse_mod_style(input)?;
    let bang_token = Not::parse_token(input)?;
    let ident = if input.peek_ident() {
        Some(input.parse_any_ident()?)
    } else {
        None
    };
    let mac = parse_macro_body(input, path, bang_token)?;
    let semi_token = macro_semicolon(input, &mac)?;
    Ok(ItemMacro {
        attrs,
        ident,
        mac,
        semi_token,
    })
}

fn parse_item_mod(
    input: ParseStream<'_>,
    mut attrs: Vec<Attribute>,
    vis: Visibility,
) -> Result<ItemMod> {
    let unsafety = input.eat_keyword("unsafe");
    let mod_token = input.parse_keyword("mod")?;
    let ident = input.parse_any_ident()?;
    if input.peek_punct(';') {
        let semi = Some(Semi::parse_token(input)?);
        return Ok(ItemMod {
            attrs,
            vis,
            unsafety,
            mod_token,
            ident,
            content: None,
            semi,
        });
    }
    let group = input.parse_group(Delimiter::Brace)?;
    let content = ParseBuffer::from_group(&group);
    attrs.extend(Attribute::parse_inner(&content)?);
    let mut items = Vec::new();
    while !content.is_empty() {
        items.push(Item::parse(&content)?);
    }
    Ok(ItemMod {
        attrs,
        vis,
        unsafety,
        mod_token,
        ident,
        content: Some((group.span(), items)),
        semi: None,
    })
}

fn parse_item_trait(
    input: ParseStream<'_>,
    mut attrs: Vec<Attribute>,
    vis: Visibility,
) -> Result<Item> {
    let start = input.position();
    let unsafety = input.eat_keyword("unsafe");
    let auto_token = input.eat_keyword("auto");
    let trait_token = input.parse_keyword("trait")?;
    let ident = input.parse_any_ident()?;
    let mut generics = Generics::parse(input)?;
    if input.peek_punct('=') {
        input.reset(start);
        return Ok(Item::TraitAlias(ItemTraitAlias {
            attrs,
            vis,
            ident,
            tokens: take_through_semicolon(input)?,
        }));
    }
    let colon_token = if input.peek_punct(':') && !input.peek_joint("::") {
        Some(Colon::parse_token(input)?)
    } else {
        None
    };
    let supertraits = if colon_token.is_some()
        && !input.peek_keyword("where")
        && !input.peek_group(Delimiter::Brace)
    {
        parse_type_bounds(input, true)?
    } else {
        Punctuated::new()
    };
    generics.where_clause = WhereClause::parse_optional(input)?;
    let group = input.parse_group(Delimiter::Brace)?;
    let content = ParseBuffer::from_group(&group);
    attrs.extend(Attribute::parse_inner(&content)?);
    let mut items = Vec::new();
    while !content.is_empty() {
        items.push(TraitItem::parse(&content)?);
    }
    Ok(Item::Trait(ItemTrait {
        attrs,
        vis,
        unsafety,
        auto_token,
        trait_token,
        ident,
        generics,
        colon_token,
        supertraits,
        brace_span: group.span(),
        items,
    }))
}

fn parse_item_use(
    input: ParseStream<'_>,
    attrs: Vec<Attribute>,
    vis: Visibility,
) -> Result<ItemUse> {
    let use_token = input.parse_keyword("use")?;
    let leading_colon = parse_optional::<PathSep>(input)?;
    let tree = UseTree::parse(input)?;
    let semi_token = Semi::parse_token(input)?;
    Ok(ItemUse {
        attrs,
        vis,
        use_token,
        leading_colon,
        tree,
        semi_token,
    })
}

impl Parse for UseTree {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        if input.peek_punct('*') {
            return Ok(Self::Glob(UseGlob {
                star_token: Star::parse_token(input)?,
            }));
        }
        if input.peek_group(Delimiter::Brace) {
            let group = input.parse_group(Delimiter::Brace)?;
            let content = ParseBuffer::from_group(&group);
            let mut items = Punctuated::new();
            while !content.is_empty() {
                items.push_value(Self::parse(&content)?);
                if content.is_empty() {
                    break;
                }
                items.push_punct(Comma::parse_token(&content)?);
            }
            return Ok(Self::Group(UseGroup {
                brace_span: group.span(),
                items,
            }));
        }
        let ident = input.parse_any_ident()?;
        if input.peek_joint("::") {
            let colon2_token = PathSep::parse_token(input)?;
            let tree = Box::new(Self::parse(input)?);
            return Ok(Self::Path(UsePath {
                ident,
                colon2_token,
                tree,
            }));
        }
        if let Some(as_token) = input.eat_keyword("as") {
            let rename = input.parse_any_ident()?;
            return Ok(Self::Rename(UseRename {
                ident,
                as_token,
                rename,
            }));
        }
        Ok(Self::Name(UseName { ident }))
    }
}

fn parse_impl_const(
    input: ParseStream<'_>,
    attrs: Vec<Attribute>,
    vis: Visibility,
    defaultness: Option<Ident>,
) -> Result<ImplItemConst> {
    let const_token = input.parse_keyword("const")?;
    let ident = input.parse_any_ident()?;
    let colon_token = Colon::parse_token(input)?;
    let ty = parse_type(input, true)?;
    let eq_token = Eq::parse_token(input)?;
    let expr = Expr::parse_until(input, ';');
    let semi_token = Semi::parse_token(input)?;
    Ok(ImplItemConst {
        attrs,
        vis,
        defaultness,
        const_token,
        ident,
        colon_token,
        ty,
        eq_token,
        expr,
        semi_token,
    })
}

fn tokens_since(input: ParseStream<'_>, start: usize) -> Tokens {
    let end = input.position();
    input.reset(start);
    let mut tokens = Tokens::new();
    while input.position() < end {
        if let Some(tree) = input.next_tree() {
            tokens.push(tree);
        }
    }
    tokens
}

impl Parse for ImplItem {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let begin = input.position();
        let mut attrs = Attribute::parse_outer(input)?;
        let after_attributes = input.position();
        let vis = Visibility::parse(input)?;
        let defaultness = if input.peek_keyword("default") && !input.peek_punct_at(1, '!') {
            input.eat_keyword("default")
        } else {
            None
        };
        if peek_signature(input) {
            let sig = Signature::parse(input)?;
            if input.peek_punct(';') {
                Semi::parse_token(input)?;
                return Ok(Self::Verbatim(tokens_since(input, begin)));
            }
            let block = Block::parse_with_inner(input, &mut attrs)?;
            return Ok(Self::Fn(ImplItemFn {
                attrs,
                vis,
                defaultness,
                sig,
                block,
            }));
        }
        if input.peek_keyword("const") {
            return parse_impl_const(input, attrs, vis, defaultness).map(Self::Const);
        }
        if input.peek_keyword("type") {
            let ident = ident_after(input, &["type"])?;
            input.reset(after_attributes);
            return Ok(Self::Type(ImplItemType {
                attrs,
                ident,
                tokens: take_through_semicolon(input)?,
            }));
        }
        if matches!(vis, Visibility::Inherited)
            && defaultness.is_none()
            && (input.peek_ident() || input.peek_joint("::"))
        {
            let mac = Macro::parse(input)?;
            let semi_token = macro_semicolon(input, &mac)?;
            return Ok(Self::Macro(ImplItemMacro {
                attrs,
                mac,
                semi_token,
            }));
        }
        Err(input.error("expected an impl item"))
    }
}

impl Parse for TraitItem {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let mut attrs = Attribute::parse_outer(input)?;
        let after_attributes = input.position();
        if peek_signature(input) {
            let sig = Signature::parse(input)?;
            if input.peek_punct(';') {
                let semi_token = Some(Semi::parse_token(input)?);
                return Ok(Self::Fn(TraitItemFn {
                    attrs,
                    sig,
                    default: None,
                    semi_token,
                }));
            }
            let block = Block::parse_with_inner(input, &mut attrs)?;
            return Ok(Self::Fn(TraitItemFn {
                attrs,
                sig,
                default: Some(block),
                semi_token: None,
            }));
        }
        if input.peek_keyword("const") {
            let const_token = input.parse_keyword("const")?;
            let ident = input.parse_any_ident()?;
            let colon_token = Colon::parse_token(input)?;
            let ty = parse_type(input, true)?;
            let default = if input.peek_punct('=') {
                let eq_token = Eq::parse_token(input)?;
                Some((eq_token, Expr::parse_until(input, ';')))
            } else {
                None
            };
            let semi_token = Semi::parse_token(input)?;
            return Ok(Self::Const(TraitItemConst {
                attrs,
                const_token,
                ident,
                colon_token,
                ty,
                default,
                semi_token,
            }));
        }
        if input.peek_keyword("type") {
            let ident = ident_after(input, &["type"])?;
            input.reset(after_attributes);
            return Ok(Self::Type(TraitItemType {
                attrs,
                ident,
                tokens: take_through_semicolon(input)?,
            }));
        }
        if input.peek_ident() || input.peek_joint("::") {
            let mac = Macro::parse(input)?;
            let semi_token = macro_semicolon(input, &mac)?;
            return Ok(Self::Macro(TraitItemMacro {
                attrs,
                mac,
                semi_token,
            }));
        }
        Err(input.error("expected a trait item"))
    }
}

impl ToTokens for Item {
    fn to_tokens(&self, tokens: &mut Tokens) {
        match self {
            Self::Const(item) => item.to_tokens(tokens),
            Self::Enum(item) => item.to_tokens(tokens),
            Self::ExternCrate(item) => item.to_tokens(tokens),
            Self::Fn(item) => item.to_tokens(tokens),
            Self::ForeignMod(item) => item.to_tokens(tokens),
            Self::Impl(item) => item.to_tokens(tokens),
            Self::Macro(item) => item.to_tokens(tokens),
            Self::Mod(item) => item.to_tokens(tokens),
            Self::Static(item) => item.to_tokens(tokens),
            Self::Struct(item) => item.to_tokens(tokens),
            Self::Trait(item) => item.to_tokens(tokens),
            Self::TraitAlias(item) => item.to_tokens(tokens),
            Self::Type(item) => item.to_tokens(tokens),
            Self::Union(item) => item.to_tokens(tokens),
            Self::Use(item) => item.to_tokens(tokens),
            Self::Verbatim(verbatim) => verbatim.to_tokens(tokens),
        }
    }
}

impl ToTokens for ItemConst {
    fn to_tokens(&self, tokens: &mut Tokens) {
        print_outer_attributes(&self.attrs, tokens);
        self.vis.to_tokens(tokens);
        self.const_token.to_tokens(tokens);
        self.ident.to_tokens(tokens);
        self.colon_token.to_tokens(tokens);
        self.ty.to_tokens(tokens);
        self.eq_token.to_tokens(tokens);
        self.expr.to_tokens(tokens);
        self.semi_token.to_tokens(tokens);
    }
}

impl ToTokens for ItemEnum {
    fn to_tokens(&self, tokens: &mut Tokens) {
        print_outer_attributes(&self.attrs, tokens);
        self.vis.to_tokens(tokens);
        self.enum_token.to_tokens(tokens);
        self.ident.to_tokens(tokens);
        self.generics.to_tokens(tokens);
        self.generics.where_clause.to_tokens(tokens);
        push_group(
            tokens,
            self.brace_span,
            Delimiter::Brace,
            self.variants.to_token_stream(),
        );
    }
}

macro_rules! opaque_with_visibility {
    ($($name:ident),*) => {
        $(
            impl ToTokens for $name {
                fn to_tokens(&self, tokens: &mut Tokens) {
                    print_outer_attributes(&self.attrs, tokens);
                    self.vis.to_tokens(tokens);
                    self.tokens.to_tokens(tokens);
                }
            }
        )*
    };
}

opaque_with_visibility!(
    ItemExternCrate,
    ItemStatic,
    ItemTraitAlias,
    ItemType,
    ItemUnion
);

macro_rules! opaque_without_visibility {
    ($($name:ident),*) => {
        $(
            impl ToTokens for $name {
                fn to_tokens(&self, tokens: &mut Tokens) {
                    print_outer_attributes(&self.attrs, tokens);
                    self.tokens.to_tokens(tokens);
                }
            }
        )*
    };
}

opaque_without_visibility!(ItemForeignMod, ImplItemType, TraitItemType);

impl ToTokens for ItemFn {
    fn to_tokens(&self, tokens: &mut Tokens) {
        print_outer_attributes(&self.attrs, tokens);
        self.vis.to_tokens(tokens);
        self.sig.to_tokens(tokens);
        self.block.print_with_inner(&self.attrs, tokens);
    }
}

impl ToTokens for ItemImpl {
    fn to_tokens(&self, tokens: &mut Tokens) {
        print_outer_attributes(&self.attrs, tokens);
        self.defaultness.to_tokens(tokens);
        self.unsafety.to_tokens(tokens);
        self.impl_token.to_tokens(tokens);
        self.generics.to_tokens(tokens);
        if let Some((polarity, path, for_token)) = &self.trait_ {
            polarity.to_tokens(tokens);
            path.to_tokens(tokens);
            for_token.to_tokens(tokens);
        }
        self.self_ty.to_tokens(tokens);
        self.generics.where_clause.to_tokens(tokens);
        let mut inner = Tokens::new();
        print_inner_attributes(&self.attrs, &mut inner);
        inner.append_all(&self.items);
        push_group(tokens, self.brace_span, Delimiter::Brace, inner);
    }
}

impl ToTokens for Macro {
    fn to_tokens(&self, tokens: &mut Tokens) {
        self.path.to_tokens(tokens);
        self.bang_token.to_tokens(tokens);
        push_group(
            tokens,
            self.delimiter_span,
            self.delimiter,
            self.tokens.clone(),
        );
    }
}

impl ToTokens for ItemMacro {
    fn to_tokens(&self, tokens: &mut Tokens) {
        print_outer_attributes(&self.attrs, tokens);
        self.mac.path.to_tokens(tokens);
        self.mac.bang_token.to_tokens(tokens);
        self.ident.to_tokens(tokens);
        push_group(
            tokens,
            self.mac.delimiter_span,
            self.mac.delimiter,
            self.mac.tokens.clone(),
        );
        self.semi_token.to_tokens(tokens);
    }
}

impl ToTokens for ItemMod {
    fn to_tokens(&self, tokens: &mut Tokens) {
        print_outer_attributes(&self.attrs, tokens);
        self.vis.to_tokens(tokens);
        self.unsafety.to_tokens(tokens);
        self.mod_token.to_tokens(tokens);
        self.ident.to_tokens(tokens);
        match &self.content {
            Some((brace_span, items)) => {
                let mut inner = Tokens::new();
                print_inner_attributes(&self.attrs, &mut inner);
                inner.append_all(items);
                push_group(tokens, *brace_span, Delimiter::Brace, inner);
            }
            None => self.semi.unwrap_or_default().to_tokens(tokens),
        }
    }
}

impl ToTokens for ItemStruct {
    fn to_tokens(&self, tokens: &mut Tokens) {
        print_outer_attributes(&self.attrs, tokens);
        self.vis.to_tokens(tokens);
        self.struct_token.to_tokens(tokens);
        self.ident.to_tokens(tokens);
        self.generics.to_tokens(tokens);
        match &self.fields {
            Fields::Named(_) => {
                self.generics.where_clause.to_tokens(tokens);
                self.fields.to_tokens(tokens);
            }
            Fields::Unnamed(_) => {
                self.fields.to_tokens(tokens);
                self.generics.where_clause.to_tokens(tokens);
                self.semi_token.unwrap_or_default().to_tokens(tokens);
            }
            Fields::Unit => {
                self.generics.where_clause.to_tokens(tokens);
                self.semi_token.unwrap_or_default().to_tokens(tokens);
            }
        }
    }
}

impl ToTokens for ItemTrait {
    fn to_tokens(&self, tokens: &mut Tokens) {
        print_outer_attributes(&self.attrs, tokens);
        self.vis.to_tokens(tokens);
        self.unsafety.to_tokens(tokens);
        self.auto_token.to_tokens(tokens);
        self.trait_token.to_tokens(tokens);
        self.ident.to_tokens(tokens);
        self.generics.to_tokens(tokens);
        if !self.supertraits.is_empty() {
            self.colon_token.unwrap_or_default().to_tokens(tokens);
            self.supertraits.to_tokens(tokens);
        }
        self.generics.where_clause.to_tokens(tokens);
        let mut inner = Tokens::new();
        print_inner_attributes(&self.attrs, &mut inner);
        inner.append_all(&self.items);
        push_group(tokens, self.brace_span, Delimiter::Brace, inner);
    }
}

impl ToTokens for ItemUse {
    fn to_tokens(&self, tokens: &mut Tokens) {
        print_outer_attributes(&self.attrs, tokens);
        self.vis.to_tokens(tokens);
        self.use_token.to_tokens(tokens);
        self.leading_colon.to_tokens(tokens);
        self.tree.to_tokens(tokens);
        self.semi_token.to_tokens(tokens);
    }
}

impl ToTokens for UseTree {
    fn to_tokens(&self, tokens: &mut Tokens) {
        match self {
            Self::Path(path) => {
                path.ident.to_tokens(tokens);
                path.colon2_token.to_tokens(tokens);
                path.tree.to_tokens(tokens);
            }
            Self::Name(name) => name.ident.to_tokens(tokens),
            Self::Rename(rename) => {
                rename.ident.to_tokens(tokens);
                rename.as_token.to_tokens(tokens);
                rename.rename.to_tokens(tokens);
            }
            Self::Glob(glob) => glob.to_tokens(tokens),
            Self::Group(group) => push_group(
                tokens,
                group.brace_span,
                Delimiter::Brace,
                group.items.to_token_stream(),
            ),
        }
    }
}

impl ToTokens for UseGlob {
    fn to_tokens(&self, tokens: &mut Tokens) {
        self.star_token.to_tokens(tokens);
    }
}

impl ToTokens for ImplItem {
    fn to_tokens(&self, tokens: &mut Tokens) {
        match self {
            Self::Const(item) => item.to_tokens(tokens),
            Self::Fn(item) => item.to_tokens(tokens),
            Self::Type(item) => item.to_tokens(tokens),
            Self::Macro(item) => {
                print_outer_attributes(&item.attrs, tokens);
                item.mac.to_tokens(tokens);
                item.semi_token.to_tokens(tokens);
            }
            Self::Verbatim(verbatim) => verbatim.to_tokens(tokens),
        }
    }
}

impl ToTokens for ImplItemConst {
    fn to_tokens(&self, tokens: &mut Tokens) {
        print_outer_attributes(&self.attrs, tokens);
        self.vis.to_tokens(tokens);
        self.defaultness.to_tokens(tokens);
        self.const_token.to_tokens(tokens);
        self.ident.to_tokens(tokens);
        self.colon_token.to_tokens(tokens);
        self.ty.to_tokens(tokens);
        self.eq_token.to_tokens(tokens);
        self.expr.to_tokens(tokens);
        self.semi_token.to_tokens(tokens);
    }
}

impl ToTokens for ImplItemFn {
    fn to_tokens(&self, tokens: &mut Tokens) {
        print_outer_attributes(&self.attrs, tokens);
        self.vis.to_tokens(tokens);
        self.defaultness.to_tokens(tokens);
        self.sig.to_tokens(tokens);
        self.block.print_with_inner(&self.attrs, tokens);
    }
}

impl ToTokens for TraitItem {
    fn to_tokens(&self, tokens: &mut Tokens) {
        match self {
            Self::Const(item) => item.to_tokens(tokens),
            Self::Fn(item) => item.to_tokens(tokens),
            Self::Type(item) => item.to_tokens(tokens),
            Self::Macro(item) => {
                print_outer_attributes(&item.attrs, tokens);
                item.mac.to_tokens(tokens);
                item.semi_token.to_tokens(tokens);
            }
            Self::Verbatim(verbatim) => verbatim.to_tokens(tokens),
        }
    }
}

impl ToTokens for TraitItemConst {
    fn to_tokens(&self, tokens: &mut Tokens) {
        print_outer_attributes(&self.attrs, tokens);
        self.const_token.to_tokens(tokens);
        self.ident.to_tokens(tokens);
        self.colon_token.to_tokens(tokens);
        self.ty.to_tokens(tokens);
        if let Some((eq_token, default)) = &self.default {
            eq_token.to_tokens(tokens);
            default.to_tokens(tokens);
        }
        self.semi_token.to_tokens(tokens);
    }
}

impl ToTokens for TraitItemFn {
    fn to_tokens(&self, tokens: &mut Tokens) {
        print_outer_attributes(&self.attrs, tokens);
        self.sig.to_tokens(tokens);
        match &self.default {
            Some(block) => block.print_with_inner(&self.attrs, tokens),
            None => self.semi_token.unwrap_or_default().to_tokens(tokens),
        }
    }
}
