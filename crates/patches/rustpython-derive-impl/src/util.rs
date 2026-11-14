use nanocode_macro_support::{Span, Tokens};
use nanocode_macro_support::{ToTokens, quote};
use std::collections::{HashMap, HashSet};
use nanocode_macro_support::{
    Attribute, FnArg, Ident, Result, Signature, Type, UseTree,  Spanned,
};
use crate::syn_ext::{
    ext::{AttributeExt as SynAttributeExt, *},
    types::*,
};

pub(crate) const ALL_ALLOWED_NAMES: &[&str] = &[
    "pymethod",
    "pyclassmethod",
    "pystaticmethod",
    "pygetset",
    "pyfunction",
    "pyclass",
    "pyexception",
    "pystruct_sequence",
    "pyattr",
    "pyslot",
    "extend_class",
    "pymember",
];

pub(crate) fn attribute_texts(attributes: &[Attribute]) -> Vec<String> {
    attributes
        .iter()
        .map(|attribute| attribute.to_token_stream().to_string())
        .collect()
}

#[derive(Clone)]
pub(crate) struct AttributeList(pub Vec<Attribute>);

impl PartialEq for AttributeList {
    fn eq(&self, other: &Self) -> bool {
        attribute_texts(&self.0) == attribute_texts(&other.0)
    }
}

impl Eq for AttributeList {}

impl core::hash::Hash for AttributeList {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        attribute_texts(&self.0).hash(state);
    }
}

impl core::ops::Deref for AttributeList {
    type Target = Vec<Attribute>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Clone)]
struct NurseryItem {
    attr_name: Ident,
    py_names: Vec<String>,
    cfgs: Vec<Attribute>,
    tokens: Tokens,
    sort_order: usize,
}

#[derive(Default)]
pub(crate) struct ItemNursery(Vec<NurseryItem>);

pub(crate) struct ValidatedItemNursery(ItemNursery);

impl ItemNursery {
    pub(crate) fn add_item(
        &mut self,
        attr_name: Ident,
        py_names: Vec<String>,
        cfgs: Vec<Attribute>,
        tokens: Tokens,
        sort_order: usize,
    ) {
        self.0.push(NurseryItem {
            attr_name,
            py_names,
            cfgs,
            tokens,
            sort_order,
        });
    }

    pub(crate) fn validate(self) -> Result<ValidatedItemNursery> {
        let mut by_name: HashSet<(String, AttributeList)> = HashSet::new();
        for item in &self.0 {
            for py_name in &item.py_names {
                let inserted = by_name.insert((py_name.clone(), AttributeList(item.cfgs.clone())));
                if !inserted {
                    return Err(nanocode_macro_support::Error::new(
                        item.attr_name.span(),
                        format!("Duplicated #[py*] attribute found for {:?}", item.py_names),
                    ));
                }
            }
        }
        Ok(ValidatedItemNursery(self))
    }
}

impl ToTokens for ValidatedItemNursery {
    fn to_tokens(&self, tokens: &mut Tokens) {
        let mut sorted = self.0.0.clone();
        sorted.sort_by_key(|a| a.sort_order);
        tokens.append_all(sorted.iter().map(|item| {
            let cfgs = &item.cfgs;
            let tokens = &item.tokens;
            quote! {
                #(#cfgs)*
                {
                    #tokens
                }
            }
        }))
    }
}

#[derive(Clone)]
pub(crate) struct ContentItemInner<T> {
    pub index: usize,
    pub attr_name: T,
}

pub(crate) trait ContentItem {
    type AttrName: core::str::FromStr + core::fmt::Display;

    fn inner(&self) -> &ContentItemInner<Self::AttrName>;

    fn index(&self) -> usize {
        self.inner().index
    }

    fn attr_name(&self) -> &Self::AttrName {
        &self.inner().attr_name
    }

    fn new_syn_error(&self, span: Span, message: &str) -> nanocode_macro_support::Error {
        nanocode_macro_support::Error::new(span, format!("#[{}] {}", self.attr_name(), message))
    }
}

pub(crate) struct ItemMetaInner {
    pub item_ident: Ident,
    pub meta_ident: Ident,
    pub meta_map: HashMap<String, (usize, Meta)>,
}

impl ItemMetaInner {
    pub(crate) fn from_nested<I>(
        item_ident: Ident,
        meta_ident: Ident,
        nested: I,
        allowed_names: &[&'static str],
    ) -> Result<Self>
    where
        I: core::iter::Iterator<Item = NestedMeta>,
    {
        let (meta_map, lits) = nested.into_unique_map_and_lits(|path| {
            if let Some(ident) = path.get_ident() {
                let name = ident.to_string();
                if allowed_names.contains(&name.as_str()) {
                    Ok(Some(name))
                } else {
                    Err(err_span!(
                        ident,
                        "#[{meta_ident}({name})] is not one of allowed attributes [{}]",
                        allowed_names.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
                    ))
                }
            } else {
                Ok(None)
            }
        })?;

        if !lits.is_empty() {
            bail_span!(meta_ident, "#[{meta_ident}(..)] cannot contain literal")
        }

        Ok(Self {
            item_ident,
            meta_ident,
            meta_map,
        })
    }

    pub(crate) fn contains_key(&self, key: &str) -> bool {
        self.meta_map.contains_key(key)
    }

    pub(crate) fn item_name(&self) -> String {
        self.item_ident.to_string()
    }

    pub(crate) fn meta_name(&self) -> String {
        self.meta_ident.to_string()
    }

    pub(crate) fn _optional_str(&self, key: &str) -> Result<Option<String>> {
        let value = if let Some((_, meta)) = self.meta_map.get(key) {
            let Meta::NameValue(MetaNameValue {
                value:
                    nanocode_macro_support::Expr::Lit(nanocode_macro_support::ExprLit {
                        lit: nanocode_macro_support::Lit::Str(lit),
                        ..
                    }),
                ..
            }) = meta
            else {
                bail_span!(
                    meta,
                    "#[{}({} = ...)] must exist as a string",
                    self.meta_name(),
                    key
                )
            };
            Some(lit.value())
        } else {
            None
        };
        Ok(value)
    }

    pub(crate) fn _optional_path(&self, key: &str) -> Result<Option<nanocode_macro_support::Path>> {
        let value = if let Some((_, meta)) = self.meta_map.get(key) {
            let Meta::NameValue(MetaNameValue { value, .. }) = meta else {
                bail_span!(
                    meta,
                    "#[{}({} = ...)] must be a name-value pair",
                    self.meta_name(),
                    key
                )
            };

            // Try to parse as a Path (identifier or path like Foo or foo::Bar)
            match nanocode_macro_support::parse_tokens::<nanocode_macro_support::Path>(value.to_token_stream()) {
                Ok(path) => Some(path),
                Err(_) => {
                    bail_span!(
                        value,
                        "#[{}({} = ...)] must be a valid type path (e.g., PyBaseException)",
                        self.meta_name(),
                        key
                    )
                }
            }
        } else {
            None
        };
        Ok(value)
    }

    pub(crate) fn _bool(&self, key: &str) -> Result<bool> {
        let value = if let Some((_, meta)) = self.meta_map.get(key) {
            match meta {
                Meta::NameValue(MetaNameValue {
                    value:
                        nanocode_macro_support::Expr::Lit(nanocode_macro_support::ExprLit {
                            lit: nanocode_macro_support::Lit::Bool(lit),
                            ..
                        }),
                    ..
                }) => lit.value,
                Meta::Path(_) => true,
                _ => bail_span!(meta, "#[{}({})] is expected", self.meta_name(), key),
            }
        } else {
            false
        };
        Ok(value)
    }

    pub(crate) fn _optional_list(
        &self,
        key: &str,
    ) -> Result<Option<impl core::iter::Iterator<Item = &'_ NestedMeta>>> {
        let value = if let Some((_, meta)) = self.meta_map.get(key) {
            let Meta::List(MetaList {
                path: _, nested, ..
            }) = meta
            else {
                bail_span!(meta, "#[{}({}(...))] must be a list", self.meta_name(), key)
            };
            Some(nested.into_iter())
        } else {
            None
        };
        Ok(value)
    }
}

pub(crate) trait ItemMeta: Sized {
    const ALLOWED_NAMES: &'static [&'static str];

    fn from_attr(item_ident: Ident, attr: &Attribute) -> Result<Self> {
        let (meta_ident, nested) = attr.ident_and_promoted_nested()?;
        Self::from_nested(item_ident, meta_ident.clone(), nested.into_iter())
    }

    fn from_nested<I>(item_ident: Ident, meta_ident: Ident, nested: I) -> Result<Self>
    where
        I: core::iter::Iterator<Item = NestedMeta>,
    {
        Ok(Self::from_inner(ItemMetaInner::from_nested(
            item_ident,
            meta_ident,
            nested,
            Self::ALLOWED_NAMES,
        )?))
    }

    fn from_inner(inner: ItemMetaInner) -> Self;
    fn inner(&self) -> &ItemMetaInner;

    fn simple_name(&self) -> Result<String> {
        let inner = self.inner();
        Ok(inner
            ._optional_str("name")?
            .unwrap_or_else(|| inner.item_name()))
    }

    fn optional_name(&self) -> Option<String> {
        self.inner()._optional_str("name").ok().flatten()
    }

    fn new_meta_error(&self, msg: &str) -> nanocode_macro_support::Error {
        let inner = self.inner();
        err_span!(inner.meta_ident, "#[{}] {}", inner.meta_name(), msg)
    }
}
pub(crate) struct SimpleItemMeta(pub ItemMetaInner);

impl ItemMeta for SimpleItemMeta {
    const ALLOWED_NAMES: &'static [&'static str] = &["name"];

    fn from_inner(inner: ItemMetaInner) -> Self {
        Self(inner)
    }

    fn inner(&self) -> &ItemMetaInner {
        &self.0
    }
}

pub(crate) struct ModuleItemMeta(pub ItemMetaInner);

impl ItemMeta for ModuleItemMeta {
    const ALLOWED_NAMES: &'static [&'static str] = &["name", "sub"];

    fn from_inner(inner: ItemMetaInner) -> Self {
        Self(inner)
    }

    fn inner(&self) -> &ItemMetaInner {
        &self.0
    }
}

impl ModuleItemMeta {
    pub(crate) fn sub(&self) -> Result<bool> {
        self.inner()._bool("sub")
    }
}

pub(crate) struct AttrItemMeta(pub ItemMetaInner);

impl ItemMeta for AttrItemMeta {
    const ALLOWED_NAMES: &'static [&'static str] = &["name", "once"];

    fn from_inner(inner: ItemMetaInner) -> Self {
        Self(inner)
    }

    fn inner(&self) -> &ItemMetaInner {
        &self.0
    }
}

pub(crate) struct ClassItemMeta(ItemMetaInner);

impl ItemMeta for ClassItemMeta {
    const ALLOWED_NAMES: &'static [&'static str] = &[
        "module",
        "name",
        "base",
        "metaclass",
        "unhashable",
        "ctx",
        "impl",
        "traverse",
        "clear", // tp_clear
        "payload",
    ];

    fn from_inner(inner: ItemMetaInner) -> Self {
        Self(inner)
    }

    fn inner(&self) -> &ItemMetaInner {
        &self.0
    }
}

impl ClassItemMeta {
    pub(crate) fn class_name(&self) -> Result<String> {
        const KEY: &str = "name";
        let inner = self.inner();
        if let Some((_, meta)) = inner.meta_map.get(KEY) {
            match meta {
                Meta::NameValue(MetaNameValue {
                    value:
                        nanocode_macro_support::Expr::Lit(nanocode_macro_support::ExprLit {
                            lit: nanocode_macro_support::Lit::Str(lit),
                            ..
                        }),
                    ..
                }) => return Ok(lit.value()),
                Meta::Path(_) => return Ok(inner.item_name()),
                _ => {}
            }
        }
        bail_span!(
            inner.meta_ident,
            "#[{attr_name}(name = ...)] must exist as a string. Try \
             #[{attr_name}(name)] to use rust type name.",
            attr_name = inner.meta_name()
        )
    }

    pub(crate) fn ctx_name(&self) -> Result<Option<String>> {
        self.inner()._optional_str("ctx")
    }

    pub(crate) fn manual_payload(&self) -> Result<bool> {
        Ok(self.inner()._optional_str("payload")?.as_deref() == Some("manual"))
    }

    pub(crate) fn base(&self) -> Result<Option<nanocode_macro_support::Path>> {
        self.inner()._optional_path("base")
    }

    pub(crate) fn unhashable(&self) -> Result<bool> {
        self.inner()._bool("unhashable")
    }

    pub(crate) fn metaclass(&self) -> Result<Option<String>> {
        self.inner()._optional_str("metaclass")
    }

    pub(crate) fn module(&self) -> Result<Option<String>> {
        const KEY: &str = "module";
        let inner = self.inner();
        let value = if let Some((_, meta)) = inner.meta_map.get(KEY) {
            match meta {
                Meta::NameValue(MetaNameValue {
                    value: nanocode_macro_support::Expr::Lit(nanocode_macro_support::ExprLit{lit:nanocode_macro_support::Lit::Str(lit),..}),
                    ..
                }) => Ok(Some(lit.value())),
                Meta::NameValue(MetaNameValue {
                    value: nanocode_macro_support::Expr::Lit(nanocode_macro_support::ExprLit{lit:nanocode_macro_support::Lit::Bool(lit),..}),
                    ..
                }) => if lit.value {
                    Err(lit.span())
                } else {
                    Ok(None)
                }
                other => Err(other.span()),
            }
        } else {
            Err(inner.item_ident.span())
        }.map_err(|span| nanocode_macro_support::Error::new(
            span,
            format!(
                "#[{attr_name}(module = ...)] must exist as a string or false. Try #[{attr_name}(module = false)] for built-in types.",
                attr_name=inner.meta_name()
            ),
        ))?;
        Ok(value)
    }

    pub(crate) fn impl_attrs(&self) -> Result<Option<String>> {
        self.inner()._optional_str("impl")
    }

    // pub fn mandatory_module(&self) -> Result<String> {
    //     let inner = self.inner();
    //     let value = self.module().ok().flatten().
    //     ok_or_else(|| err_span!(
    //         inner.meta_ident,
    //         "#[{attr_name}(module = ...)] must exist as a string. Built-in module is not allowed here.",
    //         attr_name = inner.meta_name()
    //     ))?;
    //     Ok(value)
    // }
}

pub(crate) struct ExceptionItemMeta(ClassItemMeta);

impl ItemMeta for ExceptionItemMeta {
    const ALLOWED_NAMES: &'static [&'static str] = &[
        "module",
        "name",
        "base",
        "unhashable",
        "ctx",
        "impl",
        "traverse",
        "payload",
    ];

    fn from_inner(inner: ItemMetaInner) -> Self {
        Self(ClassItemMeta(inner))
    }

    fn inner(&self) -> &ItemMetaInner {
        &self.0.0
    }
}

impl ExceptionItemMeta {
    pub(crate) fn class_name(&self) -> Result<String> {
        const KEY: &str = "name";
        let inner = self.inner();
        if let Some((_, meta)) = inner.meta_map.get(KEY) {
            match meta {
                Meta::NameValue(MetaNameValue {
                    value:
                        nanocode_macro_support::Expr::Lit(nanocode_macro_support::ExprLit {
                            lit: nanocode_macro_support::Lit::Str(lit),
                            ..
                        }),
                    ..
                }) => return Ok(lit.value()),
                Meta::Path(_) => {
                    return Ok({
                        let type_name = inner.item_name();
                        let Some(py_name) = type_name.as_str().strip_prefix("Py") else {
                            bail_span!(
                                inner.item_ident,
                                "#[pyexception] expects its underlying type to be named `Py` prefixed"
                            )
                        };
                        py_name.to_string()
                    });
                }
                _ => {}
            }
        }
        bail_span!(
            inner.meta_ident,
            "#[{attr_name}(name = ...)] must exist as a string. Try \
             #[{attr_name}(name)] to use rust type name.",
            attr_name = inner.meta_name()
        )
    }

    pub(crate) fn has_impl(&self) -> Result<bool> {
        self.inner()._bool("impl")
    }

    /// `module = "..."` when present. Omitted means a builtin exception.
    pub(crate) fn optional_module(&self) -> Result<Option<String>> {
        if self.inner().meta_map.contains_key("module") {
            self.module()
        } else {
            Ok(None)
        }
    }
}

impl core::ops::Deref for ExceptionItemMeta {
    type Target = ClassItemMeta;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub(crate) trait AttributeExt: SynAttributeExt {
    fn promoted_nested(&self) -> Result<PunctuatedNestedMeta>;

    fn ident_and_promoted_nested(&self) -> Result<(&Ident, PunctuatedNestedMeta)>;

    fn try_remove_name(&mut self, name: &str) -> Result<Option<NestedMeta>>;

    fn fill_nested_meta<F>(&mut self, name: &str, new_item: F) -> Result<()>
    where
        F: Fn() -> NestedMeta;
}

impl AttributeExt for Attribute {
    fn promoted_nested(&self) -> Result<PunctuatedNestedMeta> {
        let list = self.promoted_list().map_err(|mut e| {
            let name = self.get_ident().unwrap().to_string();
            e.combine(err_span!(
                self,
                r##"#[{name} = "..."] cannot be a name/value, you probably meant \
                 #[{name}(name = "...")]"##,
            ));
            e
        })?;
        Ok(list.nested)
    }

    fn ident_and_promoted_nested(&self) -> Result<(&Ident, PunctuatedNestedMeta)> {
        Ok((self.get_ident().unwrap(), self.promoted_nested()?))
    }

    fn try_remove_name(&mut self, item_name: &str) -> Result<Option<NestedMeta>> {
        self.try_meta_mut(|meta| {
            let nested = match meta {
                Meta::List(MetaList { nested, .. }) => Ok(nested),
                other => Err(nanocode_macro_support::Error::new(
                    other.span(),
                    format!(
                        "#[{name}(...)] doesn't contain '{item}' to remove",
                        name = other.get_ident().unwrap(),
                        item = item_name
                    ),
                )),
            }?;

            let mut found = None;
            for (i, item) in nested.iter().enumerate() {
                if item.get_ident().is_none_or(|ident| ident != item_name) {
                    continue;
                }

                if found.is_some() {
                    return Err(nanocode_macro_support::Error::new(
                        item.span(),
                        format!("#[py..({item_name}...)] must be unique but found multiple times"),
                    ));
                }
                found = Some(i);
            }

            Ok(found.map(|idx| nested.remove(idx).into_value()))
        })
    }

    fn fill_nested_meta<F>(&mut self, name: &str, new_item: F) -> Result<()>
    where
        F: Fn() -> NestedMeta,
    {
        self.try_meta_mut(|meta| {
            let list = meta.promote_to_list(Span::call_site())?;
            let has_name = list
                .nested
                .iter()
                .any(|nested_meta| nested_meta.get_path().is_some_and(|p| p.is_ident(name)));
            if !has_name {
                list.nested.push(new_item())
            }
            Ok(())
        })
    }
}

pub(crate) fn pyclass_ident_and_attrs(item: &nanocode_macro_support::Item) -> Result<(&Ident, &[Attribute])> {
    Ok(match item {
        nanocode_macro_support::Item::Struct(nanocode_macro_support::ItemStruct { ident, attrs, .. }) => (ident, attrs),
        nanocode_macro_support::Item::Enum(nanocode_macro_support::ItemEnum { ident, attrs, .. }) => (ident, attrs),
        nanocode_macro_support::Item::Use(item_use) => (
            single(iter_use_idents(item_use, |ident, _is_unique| Ok(ident))?)
                .map_err(|()| {
                    err_span!(
                        item_use,
                        "#[pyclass] can only be on single name use statement",
                    )
                })?,
            &item_use.attrs,
        ),
        other => {
            bail_span!(
                other,
                "#[pyclass] can only be on a struct, enum or use declaration",
            )
        }
    })
}

pub(crate) fn pyexception_ident_and_attrs(item: &nanocode_macro_support::Item) -> Result<(&Ident, &[Attribute])> {
    Ok(match item {
        nanocode_macro_support::Item::Struct(nanocode_macro_support::ItemStruct { ident, attrs, .. }) => (ident, attrs),
        nanocode_macro_support::Item::Enum(nanocode_macro_support::ItemEnum { ident, attrs, .. }) => (ident, attrs),
        other => {
            bail_span!(other, "#[pyexception] can only be on a struct or enum",)
        }
    })
}

pub(crate) trait ErrorVec: Sized {
    fn into_error(self) -> Option<nanocode_macro_support::Error>;

    fn into_result(self) -> Result<()> {
        if let Some(error) = self.into_error() {
            Err(error)
        } else {
            Ok(())
        }
    }

    fn ok_or_push<T>(&mut self, r: Result<T>) -> Option<T>;
}

impl ErrorVec for Vec<nanocode_macro_support::Error> {
    fn into_error(self) -> Option<nanocode_macro_support::Error> {
        let mut iter = self.into_iter();
        if let Some(mut first) = iter.next() {
            for err in iter {
                first.combine(err);
            }
            Some(first)
        } else {
            None
        }
    }

    fn ok_or_push<T>(&mut self, r: Result<T>) -> Option<T> {
        match r {
            Ok(v) => Some(v),
            Err(e) => {
                self.push(e);
                None
            }
        }
    }
}

pub(crate) fn iter_use_idents<'a, F, R: 'a>(item_use: &'a nanocode_macro_support::ItemUse, mut f: F) -> Result<Vec<R>>
where
    F: FnMut(&'a nanocode_macro_support::Ident, bool) -> Result<R>,
{
    let mut result = Vec::new();
    match &item_use.tree {
        UseTree::Name(name) => result.push(f(&name.ident, true)?),
        UseTree::Rename(rename) => result.push(f(&rename.rename, true)?),
        UseTree::Path(path) => match &*path.tree {
            UseTree::Name(name) => result.push(f(&name.ident, true)?),
            UseTree::Rename(rename) => result.push(f(&rename.rename, true)?),
            other => iter_use_tree_idents(other, &mut result, &mut f)?,
        },
        other => iter_use_tree_idents(other, &mut result, &mut f)?,
    }
    Ok(result)
}

fn iter_use_tree_idents<'a, F, R: 'a>(
    tree: &'a nanocode_macro_support::UseTree,
    result: &mut Vec<R>,
    f: &mut F,
) -> Result<()>
where
    F: FnMut(&'a nanocode_macro_support::Ident, bool) -> Result<R>,
{
    match tree {
        UseTree::Name(name) => result.push(f(&name.ident, false)?),
        UseTree::Rename(rename) => result.push(f(&rename.rename, false)?),
        UseTree::Path(path) => iter_use_tree_idents(&path.tree, result, f)?,
        UseTree::Group(nanocode_macro_support::UseGroup { items, .. }) => {
            for subtree in items {
                iter_use_tree_idents(subtree, result, f)?;
            }
        }
        UseTree::Glob(glob) => {
            bail_span!(glob, "#[py*] doesn't allow '*'")
        }
    }
    Ok(())
}

enum SigPiece {
    Marker(String),
    Arg {
        name: String,
        ty: Type,
    },
    /// Parameters of the argument a method binds as its receiver.
    Implicit(Type),
}

fn is_vm_or_callee(ty: &Type) -> bool {
    let ty = quote!(#ty).to_string().replace(' ', "");
    (ty.starts_with('&') && ty.ends_with("VirtualMachine")) || ty.ends_with("Callee")
}

fn arg_name(pat: &nanocode_macro_support::Pat) -> String {
    match pat {
        nanocode_macro_support::Pat::Ident(pat) => {
            let ident = pat.ident.unraw().to_string();
            ident.strip_prefix('_').unwrap_or(&ident).to_owned()
        }
        // `Fildes(fd): Fildes` contributes `fd`. One binding only: a wider
        // pattern has no single parameter name. The name is unused when the
        // type supplies parameters.
        nanocode_macro_support::Pat::TupleStruct(pat) if pat.elems.len() == 1 => arg_name(&pat.elems[0]),
        nanocode_macro_support::Pat::Reference(pat) => arg_name(&pat.pat),
        nanocode_macro_support::Pat::Paren(pat) => arg_name(&pat.pat),
        _ => String::new(),
    }
}

fn trees_mention_self(trees: &[nanocode_macro_support::TokenTree]) -> bool {
    use nanocode_macro_support::TokenTree;
    let mut index = 0;
    while index < trees.len() {
        match &trees[index] {
            TokenTree::Ident(ident) if ident.to_string() == "Self" => return true,
            TokenTree::Punct(punct)
                if punct.as_char() == '!'
                    && matches!(trees.get(index + 1), Some(TokenTree::Group(_))) =>
            {
                index += 1;
            }
            TokenTree::Group(group) => {
                let inner = Tokens::from_stream(group.stream());
                if trees_mention_self(inner.trees()) {
                    return true;
                }
            }
            _ => {}
        }
        index += 1;
    }
    false
}

fn mentions_self(ty: &Type) -> bool {
    trees_mention_self(ty.to_token_stream().trees())
}

fn subst_self_in_path(path: &mut nanocode_macro_support::Path, self_ty: &Type) {
    use nanocode_macro_support::{GenericArgument, PathArguments, ReturnType};
    for segment in path.segments.iter_mut() {
        match &mut segment.arguments {
            PathArguments::None => {}
            PathArguments::AngleBracketed(arguments) => {
                for argument in arguments.args.iter_mut() {
                    match argument {
                        GenericArgument::Type(ty) => subst_self_in_type(ty, self_ty),
                        GenericArgument::AssocType(assoc) => subst_self_in_type(&mut assoc.ty, self_ty),
                        GenericArgument::Constraint(constraint) => {
                            subst_self_in_bounds(constraint.bounds.iter_mut(), self_ty)
                        }
                        GenericArgument::Lifetime(_)
                        | GenericArgument::Const(_)
                        | GenericArgument::AssocConst(_) => {}
                    }
                }
            }
            PathArguments::Parenthesized(arguments) => {
                for input in arguments.inputs.iter_mut() {
                    subst_self_in_type(input, self_ty);
                }
                if let ReturnType::Type(_, output) = &mut arguments.output {
                    subst_self_in_type(output, self_ty);
                }
            }
        }
    }
}

fn subst_self_in_bounds<'a>(
    bounds: impl Iterator<Item = &'a mut nanocode_macro_support::TypeParamBound>,
    self_ty: &Type,
) {
    for bound in bounds {
        if let nanocode_macro_support::TypeParamBound::Trait(bound) = bound {
            subst_self_in_path(&mut bound.path, self_ty);
        }
    }
}

fn subst_self_in_type(ty: &mut Type, self_ty: &Type) {
    if let Type::Path(path) = ty
        && path.qself.is_none()
        && path.path.is_ident("Self")
    {
        *ty = self_ty.clone();
        return;
    }
    match ty {
        Type::Array(array) => subst_self_in_type(&mut array.elem, self_ty),
        Type::BareFn(function) => {
            for input in function.inputs.iter_mut() {
                subst_self_in_type(&mut input.ty, self_ty);
            }
            if let nanocode_macro_support::ReturnType::Type(_, output) = &mut function.output {
                subst_self_in_type(output, self_ty);
            }
        }
        Type::Group(group) => subst_self_in_type(&mut group.elem, self_ty),
        Type::ImplTrait(implementation) => subst_self_in_bounds(implementation.bounds.iter_mut(), self_ty),
        Type::Paren(paren) => subst_self_in_type(&mut paren.elem, self_ty),
        Type::Path(path) => {
            if let Some(qself) = &mut path.qself {
                subst_self_in_type(&mut qself.ty, self_ty);
            }
            subst_self_in_path(&mut path.path, self_ty);
        }
        Type::Ptr(pointer) => subst_self_in_type(&mut pointer.elem, self_ty),
        Type::Reference(reference) => subst_self_in_type(&mut reference.elem, self_ty),
        Type::Slice(slice) => subst_self_in_type(&mut slice.elem, self_ty),
        Type::TraitObject(object) => subst_self_in_bounds(object.bounds.iter_mut(), self_ty),
        Type::Tuple(tuple) => {
            for element in tuple.elems.iter_mut() {
                subst_self_in_type(element, self_ty);
            }
        }
        Type::Infer(_) | Type::Macro(_) | Type::Never(_) | Type::Verbatim(_) => {}
    }
}

fn subst_self(ty: &Type, self_ty: Option<&Type>) -> Type {
    let Some(self_ty) = self_ty else {
        return ty.clone();
    };
    let mut ty = ty.clone();
    subst_self_in_type(&mut ty, self_ty);
    ty
}

fn sig_pieces(
    sig: &Signature,
    implicit_self: Option<&str>,
    self_ty: Option<&Type>,
    leading_marker: Option<&str>,
) -> Vec<SigPiece> {
    let mut pieces = Vec::new();
    if let Some(marker) = leading_marker {
        pieces.push(SigPiece::Marker(marker.to_owned()));
    }
    let mut implicit_self = implicit_self.map(str::to_owned);
    for arg in &sig.inputs {
        match arg {
            FnArg::Receiver(_) => pieces.push(SigPiece::Marker("$self".to_owned())),
            FnArg::Typed(typed) => {
                if is_vm_or_callee(&typed.ty) {
                    continue;
                }
                let name = arg_name(&typed.pat);
                let ty = subst_self(&typed.ty, self_ty);
                // References carry a lifetime, so `FromArgs` cannot be named
                // from a const item. They are leaf positional parameters.
                let leaf = mentions_self(&ty) || matches!(ty, Type::Reference(_));
                if let Some(marker) = implicit_self.take() {
                    // A nested const cannot name `Self`. The receiver is the
                    // marker; only a type that supplies parameters is kept.
                    pieces.push(SigPiece::Marker(marker));
                    if !leaf {
                        pieces.push(SigPiece::Implicit(ty));
                    }
                } else if leaf {
                    pieces.push(SigPiece::Marker(name));
                } else {
                    pieces.push(SigPiece::Arg { name, ty });
                }
            }
        }
    }
    pieces
}

fn sig_arg_tokens(piece: &SigPiece) -> Tokens {
    match piece {
        SigPiece::Marker(name) => quote!(::rustpython_vm::function::SigArg::marker(#name)),
        SigPiece::Arg { name, ty } => {
            quote!(::rustpython_vm::function::SigArg::from_arg::<#ty>(#name))
        }
        SigPiece::Implicit(ty) => quote!(::rustpython_vm::function::SigArg::implicit::<#ty>()),
    }
}

fn args_const(pieces: &[SigPiece]) -> Tokens {
    let args = pieces.iter().map(sig_arg_tokens);
    quote! {
        const ARGS: &[::rustpython_vm::function::SigArg] = &[#(#args),*];
    }
}

/// Expression of type `ItemDoc`.
/// `doc` is a const `ItemDoc` already chosen by the caller (a Rust doc comment
/// wins over the stored docstring). A database id keeps only the signature
/// prefix as static text. A Rust docstring is composed into one literal.
pub(crate) fn internal_doc_tokens(
    sig: &Signature,
    py_name: &str,
    implicit_self: Option<&str>,
    doc: Tokens,
    self_ty: Option<&Type>,
    leading_marker: Option<&str>,
) -> Tokens {
    let args_const = args_const(&sig_pieces(sig, implicit_self, self_ty, leading_marker));
    quote! {
        {
            #args_const
            const BASE: ::rustpython_vm::function::ItemDoc = #doc;
            if !::rustpython_vm::function::has_signature(ARGS) {
                if BASE.len == 0 {
                    match BASE.text {
                        Some(text) if !text.is_empty() => {
                            ::rustpython_vm::function::ItemDoc::static_text(text)
                        }
                        _ => ::rustpython_vm::function::ItemDoc::NONE,
                    }
                } else {
                    BASE
                }
            } else if BASE.len != 0 {
                const N: usize = ::rustpython_vm::function::signature_prefix_len(#py_name, ARGS);
                const B: [u8; N] =
                    ::rustpython_vm::function::signature_prefix_bytes::<N>(#py_name, ARGS);
                const PREFIX: &str = match ::core::str::from_utf8(&B) {
                    Ok(s) => s,
                    Err(_) => panic!(),
                };
                ::rustpython_vm::__cfg_doc!({
                    ::rustpython_vm::function::ItemDoc {
                        text: Some(PREFIX),
                        offset: BASE.offset,
                        len: BASE.len,
                    }
                } else {
                    ::rustpython_vm::function::ItemDoc::static_text(PREFIX)
                })
            } else {
                const BODY: &str = match BASE.text {
                    Some(text) => text,
                    None => "",
                };
                const N: usize = ::rustpython_vm::function::internal_doc_len(#py_name, ARGS, BODY);
                const B: [u8; N] =
                    ::rustpython_vm::function::internal_doc_bytes::<N>(#py_name, ARGS, BODY);
                const FULL: &str = match ::core::str::from_utf8(&B) {
                    Ok(s) => s,
                    Err(_) => panic!(),
                };
                ::rustpython_vm::function::ItemDoc::static_text(FULL)
            }
        }
    }
}

pub(crate) fn infer_native_call_flags(sig: &Signature, drop_first_typed: usize) -> Tokens {
    // Best-effort mapping of Rust function signatures to CPython-style
    // METH_* calling convention flags used by CALL specialization.
    let mut typed_args = Vec::new();
    for arg in &sig.inputs {
        let FnArg::Typed(typed) = arg else {
            continue;
        };
        let ty_tokens = &typed.ty;
        let ty = quote!(#ty_tokens).to_string().replace(' ', "");
        // The interpreter supplies `vm` and `callee`; a Python call never
        // passes them.
        if (ty.starts_with('&') && ty.ends_with("VirtualMachine")) || ty.ends_with("Callee") {
            continue;
        }
        typed_args.push(ty);
    }

    let mut user_args = typed_args.into_iter();
    for _ in 0..drop_first_typed {
        if user_args.next().is_none() {
            break;
        }
    }

    let mut has_keywords = false;
    let mut variable_arity = false;
    let mut fixed_positional = 0usize;

    for ty in user_args {
        let is_named = |name: &str| {
            ty == name
                || ty.starts_with(&format!("{name}<"))
                || ty.contains(&format!("::{name}<"))
                || ty.ends_with(&format!("::{name}"))
        };

        if is_named("FuncArgs") {
            has_keywords = true;
            variable_arity = true;
            continue;
        }
        if is_named("KwArgs") {
            has_keywords = true;
            variable_arity = true;
            continue;
        }
        if is_named("PosArgs") || is_named("OptionalArg") || is_named("OptionalOption") {
            variable_arity = true;
            continue;
        }
        fixed_positional += 1;
    }

    if has_keywords {
        quote! {
            rustpython_vm::function::PyMethodFlags::from_bits_retain(
                rustpython_vm::function::PyMethodFlags::FASTCALL.bits()
                    | rustpython_vm::function::PyMethodFlags::KEYWORDS.bits()
            )
        }
    } else if variable_arity {
        quote! { rustpython_vm::function::PyMethodFlags::FASTCALL }
    } else {
        match fixed_positional {
            0 => quote! { rustpython_vm::function::PyMethodFlags::NOARGS },
            1 => quote! { rustpython_vm::function::PyMethodFlags::O },
            _ => quote! { rustpython_vm::function::PyMethodFlags::FASTCALL },
        }
    }
}

fn single<T>(items: Vec<T>) -> core::result::Result<T, ()> {
    let mut items = items.into_iter();
    match (items.next(), items.next()) {
        (Some(item), None) => Ok(item),
        _ => Err(()),
    }
}
