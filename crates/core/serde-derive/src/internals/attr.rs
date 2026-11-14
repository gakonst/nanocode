use crate::internals::name::{MultiName, Name};
use crate::internals::symbol::*;
use crate::internals::{Ctxt, ungroup};
use nanocode_macro_support as syntax;
use nanocode_macro_support::ToTokens;
use nanocode_macro_support::{Spacing, Span, TokenTree, Tokens};
use std::collections::BTreeSet;
use std::iter::FromIterator;
use syntax::NestedMeta;
use syntax::ParseStream;
use syntax::Punctuated;
use syntax::Spanned;
use syntax::{Ident, Lifetime};

pub use crate::internals::case::RenameRule;

pub(crate) struct Attr<'c, T> {
    cx: &'c Ctxt,
    name: Symbol,
    tokens: Tokens,
    value: Option<T>,
}

impl<'c, T> Attr<'c, T> {
    const fn none(cx: &'c Ctxt, name: Symbol) -> Self {
        Attr {
            cx,
            name,
            tokens: Tokens::new(),
            value: None,
        }
    }

    fn set<A: ToTokens>(&mut self, obj: A, value: T) {
        let tokens = obj.into_token_stream();

        if self.value.is_some() {
            let msg = format!("duplicate serde attribute `{}`", self.name);
            self.cx.error_spanned_by(tokens, msg);
        } else {
            self.tokens = tokens;
            self.value = Some(value);
        }
    }

    fn set_opt<A: ToTokens>(&mut self, obj: A, value: Option<T>) {
        if let Some(value) = value {
            self.set(obj, value);
        }
    }

    fn set_if_none(&mut self, value: T) {
        if self.value.is_none() {
            self.value = Some(value);
        }
    }

    pub(crate) fn get(self) -> Option<T> {
        self.value
    }

    fn get_with_tokens(self) -> Option<(Tokens, T)> {
        match self.value {
            Some(v) => Some((self.tokens, v)),
            None => None,
        }
    }
}

struct BoolAttr<'c>(Attr<'c, ()>);

impl<'c> BoolAttr<'c> {
    const fn none(cx: &'c Ctxt, name: Symbol) -> Self {
        BoolAttr(Attr::none(cx, name))
    }

    fn set_true<A: ToTokens>(&mut self, obj: A) {
        self.0.set(obj, ());
    }

    const fn get(&self) -> bool {
        self.0.value.is_some()
    }
}

pub(crate) struct VecAttr<'c, T> {
    cx: &'c Ctxt,
    name: Symbol,
    first_dup_tokens: Tokens,
    values: Vec<T>,
}

impl<'c, T> VecAttr<'c, T> {
    const fn none(cx: &'c Ctxt, name: Symbol) -> Self {
        VecAttr {
            cx,
            name,
            first_dup_tokens: Tokens::new(),
            values: Vec::new(),
        }
    }

    fn insert<A: ToTokens>(&mut self, obj: A, value: T) {
        if self.values.len() == 1 {
            self.first_dup_tokens = obj.into_token_stream();
        }
        self.values.push(value);
    }

    fn at_most_one(mut self) -> Option<T> {
        if self.values.len() > 1 {
            let dup_token = self.first_dup_tokens;
            let msg = format!("duplicate serde attribute `{}`", self.name);
            self.cx.error_spanned_by(dup_token, msg);
            None
        } else {
            self.values.pop()
        }
    }

    pub(crate) fn get(self) -> Vec<T> {
        self.values
    }
}

fn unraw(ident: &Ident) -> Ident {
    Ident::new(ident.to_string().trim_start_matches("r#"), ident.span())
}

#[derive(Copy, Clone)]
pub struct RenameAllRules {
    pub serialize: RenameRule,
    pub deserialize: RenameRule,
}

impl RenameAllRules {
    pub const fn or(self, other_rules: Self) -> Self {
        Self {
            serialize: self.serialize.or(other_rules.serialize),
            deserialize: self.deserialize.or(other_rules.deserialize),
        }
    }
}

pub struct Container {
    name: MultiName,
    transparent: bool,
    deny_unknown_fields: bool,
    default: Default,
    rename_all_rules: RenameAllRules,
    rename_all_fields_rules: RenameAllRules,
    ser_bound: Option<Vec<syntax::WherePredicate>>,
    de_bound: Option<Vec<syntax::WherePredicate>>,
    tag: TagType,
    type_from: Option<syntax::Type>,
    type_try_from: Option<syntax::Type>,
    type_into: Option<syntax::Type>,
    remote: Option<syntax::Path>,
    identifier: Identifier,
    serde_path: Option<syntax::Path>,
    is_packed: bool,

    expecting: Option<String>,
    non_exhaustive: bool,
}

pub enum TagType {
    External,

    Internal { tag: String },

    Adjacent { tag: String, content: String },

    None,
}

#[derive(Copy, Clone)]
pub enum Identifier {
    No,

    Field,

    Variant,
}

impl Identifier {}

impl Container {
    pub fn from_ast(cx: &Ctxt, item: &syntax::DeriveInput) -> Self {
        let mut ser_name = Attr::none(cx, RENAME);
        let mut de_name = Attr::none(cx, RENAME);
        let mut transparent = BoolAttr::none(cx, TRANSPARENT);
        let mut deny_unknown_fields = BoolAttr::none(cx, DENY_UNKNOWN_FIELDS);
        let mut default = Attr::none(cx, DEFAULT);
        let mut rename_all_ser_rule = Attr::none(cx, RENAME_ALL);
        let mut rename_all_de_rule = Attr::none(cx, RENAME_ALL);
        let mut rename_all_fields_ser_rule = Attr::none(cx, RENAME_ALL_FIELDS);
        let mut rename_all_fields_de_rule = Attr::none(cx, RENAME_ALL_FIELDS);
        let mut ser_bound = Attr::none(cx, BOUND);
        let mut de_bound = Attr::none(cx, BOUND);
        let mut untagged = BoolAttr::none(cx, UNTAGGED);
        let mut internal_tag = Attr::none(cx, TAG);
        let mut content = Attr::none(cx, CONTENT);
        let mut type_from = Attr::none(cx, FROM);
        let mut type_try_from = Attr::none(cx, TRY_FROM);
        let mut type_into = Attr::none(cx, INTO);
        let mut remote = Attr::none(cx, REMOTE);
        let mut field_identifier = BoolAttr::none(cx, FIELD_IDENTIFIER);
        let mut variant_identifier = BoolAttr::none(cx, VARIANT_IDENTIFIER);
        let mut serde_path = Attr::none(cx, CRATE);
        let mut expecting = Attr::none(cx, EXPECTING);
        let mut non_exhaustive = false;

        for attr in &item.attrs {
            if attr.path() != SERDE {
                non_exhaustive |=
                    matches!(&attr.meta, syntax::Meta::Path(path) if path == NON_EXHAUSTIVE);
                continue;
            }

            if let syntax::Meta::List(meta) = &attr.meta
                && meta.tokens.is_empty()
            {
                continue;
            }

            if let Err(err) = attr.parse_nested_meta(|meta| {
                if meta.path == RENAME {
                    let (ser, de) = get_renames(cx, RENAME, &meta)?;
                    ser_name.set_opt(&meta.path, ser.as_ref().map(Name::from));
                    de_name.set_opt(&meta.path, de.as_ref().map(Name::from));
                } else if meta.path == RENAME_ALL {
                    let one_name = meta.input.peek_punct('=');
                    let (ser, de) = get_renames(cx, RENAME_ALL, &meta)?;
                    if let Some(ser) = ser {
                        match RenameRule::from_str(&ser.value()) {
                            Ok(rename_rule) => rename_all_ser_rule.set(&meta.path, rename_rule),
                            Err(err) => cx.error_spanned_by(ser, err),
                        }
                    }
                    if let Some(de) = de {
                        match RenameRule::from_str(&de.value()) {
                            Ok(rename_rule) => rename_all_de_rule.set(&meta.path, rename_rule),
                            Err(err) => {
                                if !one_name {
                                    cx.error_spanned_by(de, err);
                                }
                            }
                        }
                    }
                } else if meta.path == RENAME_ALL_FIELDS {
                    let one_name = meta.input.peek_punct('=');
                    let (ser, de) = get_renames(cx, RENAME_ALL_FIELDS, &meta)?;

                    match item.data {
                        syntax::Data::Enum(_) => {
                            if let Some(ser) = ser {
                                match RenameRule::from_str(&ser.value()) {
                                    Ok(rename_rule) => {
                                        rename_all_fields_ser_rule.set(&meta.path, rename_rule);
                                    }
                                    Err(err) => cx.error_spanned_by(ser, err),
                                }
                            }
                            if let Some(de) = de {
                                match RenameRule::from_str(&de.value()) {
                                    Ok(rename_rule) => {
                                        rename_all_fields_de_rule.set(&meta.path, rename_rule);
                                    }
                                    Err(err) => {
                                        if !one_name {
                                            cx.error_spanned_by(de, err);
                                        }
                                    }
                                }
                            }
                        }
                        syntax::Data::Struct(_) => {
                            let msg = "#[serde(rename_all_fields)] can only be used on enums";
                            cx.syn_error(meta.error(msg));
                        }
                        syntax::Data::Union(_) => {
                            let msg = "#[serde(rename_all_fields)] can only be used on enums";
                            cx.syn_error(meta.error(msg));
                        }
                    }
                } else if meta.path == TRANSPARENT {
                    transparent.set_true(meta.path);
                } else if meta.path == DENY_UNKNOWN_FIELDS {
                    deny_unknown_fields.set_true(meta.path);
                } else if meta.path == DEFAULT {
                    if meta.input.peek_punct('=') {
                        if let Some(path) = parse_lit_into_expr_path(cx, DEFAULT, &meta)? {
                            match &item.data {
                                syntax::Data::Struct(syntax::DataStruct { fields, .. }) => match fields {
                                    syntax::Fields::Named(_) | syntax::Fields::Unnamed(_) => {
                                        default.set(&meta.path, Default::Path(path));
                                    }
                                    syntax::Fields::Unit => {
                                        let msg = "#[serde(default = \"...\")] can only be used on structs that have fields";
                                        cx.syn_error(meta.error(msg));
                                    }
                                },
                                syntax::Data::Enum(_) => {
                                    let msg = "#[serde(default = \"...\")] can only be used on structs";
                                    cx.syn_error(meta.error(msg));
                                }
                                syntax::Data::Union(_) => {
                                    let msg = "#[serde(default = \"...\")] can only be used on structs";
                                    cx.syn_error(meta.error(msg));
                                }
                            }
                        }
                    } else {
                        match &item.data {
                            syntax::Data::Struct(syntax::DataStruct { fields, .. }) => match fields {
                                syntax::Fields::Named(_) | syntax::Fields::Unnamed(_) => {
                                    default.set(meta.path, Default::Default);
                                }
                                syntax::Fields::Unit => {
                                    let msg = "#[serde(default)] can only be used on structs that have fields";
                                    cx.error_spanned_by(fields, msg);
                                }
                            },
                            syntax::Data::Enum(_) => {
                                let msg = "#[serde(default)] can only be used on structs";
                                cx.syn_error(meta.error(msg));
                            }
                            syntax::Data::Union(_) => {
                                let msg = "#[serde(default)] can only be used on structs";
                                cx.syn_error(meta.error(msg));
                            }
                        }
                    }
                } else if meta.path == BOUND {
                    let (ser, de) = get_where_predicates(cx, &meta)?;
                    ser_bound.set_opt(&meta.path, ser);
                    de_bound.set_opt(&meta.path, de);
                } else if meta.path == UNTAGGED {
                    match item.data {
                        syntax::Data::Enum(_) => {
                            untagged.set_true(&meta.path);
                        }
                        syntax::Data::Struct(_) => {
                            let msg = "#[serde(untagged)] can only be used on enums";
                            cx.syn_error(meta.error(msg));
                        }
                        syntax::Data::Union(_) => {
                            let msg = "#[serde(untagged)] can only be used on enums";
                            cx.syn_error(meta.error(msg));
                        }
                    }
                } else if meta.path == TAG {
                    if let Some(s) = get_lit_str(cx, TAG, &meta)? {
                        match &item.data {
                            syntax::Data::Enum(_) => {
                                internal_tag.set(&meta.path, s.value());
                            }
                            syntax::Data::Struct(syntax::DataStruct { fields, .. }) => match fields {
                                syntax::Fields::Named(_) => {
                                    internal_tag.set(&meta.path, s.value());
                                }
                                syntax::Fields::Unnamed(_) | syntax::Fields::Unit => {
                                    let msg = "#[serde(tag = \"...\")] can only be used on enums and structs with named fields";
                                    cx.syn_error(meta.error(msg));
                                }
                            },
                            syntax::Data::Union(_) => {
                                let msg = "#[serde(tag = \"...\")] can only be used on enums and structs with named fields";
                                cx.syn_error(meta.error(msg));
                            }
                        }
                    }
                } else if meta.path == CONTENT {
                    if let Some(s) = get_lit_str(cx, CONTENT, &meta)? {
                        match &item.data {
                            syntax::Data::Enum(_) => {
                                content.set(&meta.path, s.value());
                            }
                            syntax::Data::Struct(_) => {
                                let msg = "#[serde(content = \"...\")] can only be used on enums";
                                cx.syn_error(meta.error(msg));
                            }
                            syntax::Data::Union(_) => {
                                let msg = "#[serde(content = \"...\")] can only be used on enums";
                                cx.syn_error(meta.error(msg));
                            }
                        }
                    }
                } else if meta.path == FROM {
                    if let Some(from_ty) = parse_lit_into_ty(cx, FROM, &meta)? {
                        type_from.set_opt(&meta.path, Some(from_ty));
                    }
                } else if meta.path == TRY_FROM {
                    if let Some(try_from_ty) = parse_lit_into_ty(cx, TRY_FROM, &meta)? {
                        type_try_from.set_opt(&meta.path, Some(try_from_ty));
                    }
                } else if meta.path == INTO {
                    if let Some(into_ty) = parse_lit_into_ty(cx, INTO, &meta)? {
                        type_into.set_opt(&meta.path, Some(into_ty));
                    }
                } else if meta.path == REMOTE {
                    if let Some(path) = parse_lit_into_path(cx, REMOTE, &meta)? {
                        if is_primitive_path(&path, "Self") {
                            remote.set(&meta.path, item.ident.clone().into());
                        } else {
                            remote.set(&meta.path, path);
                        }
                    }
                } else if meta.path == FIELD_IDENTIFIER {
                    field_identifier.set_true(&meta.path);
                } else if meta.path == VARIANT_IDENTIFIER {
                    variant_identifier.set_true(&meta.path);
                } else if meta.path == CRATE {
                    if let Some(path) = parse_lit_into_path(cx, CRATE, &meta)? {
                        serde_path.set(&meta.path, path);
                    }
                } else if meta.path == EXPECTING {
                    if let Some(s) = get_lit_str(cx, EXPECTING, &meta)? {
                        expecting.set(&meta.path, s.value());
                    }
                } else {
                    let path = meta.path.to_token_stream().to_string().replace(' ', "");
                    return Err(
                        meta.error(format_args!("unknown serde container attribute `{}`", path))
                    );
                }
                Ok(())
            }) {
                cx.syn_error(err);
            }
        }

        let mut is_packed = false;
        for attr in &item.attrs {
            if attr.path() == REPR {
                let _ = attr.parse_args_with(|input: ParseStream| {
                    while let Some(token) = input.next_tree() {
                        if let TokenTree::Ident(ident) = token {
                            is_packed |= ident.to_string() == "packed";
                        }
                    }
                    Ok(())
                });
            }
        }

        Self {
            name: MultiName::from_attrs(Name::from(&unraw(&item.ident)), ser_name, de_name, None),
            transparent: transparent.get(),
            deny_unknown_fields: deny_unknown_fields.get(),
            default: default.get().unwrap_or(Default::None),
            rename_all_rules: RenameAllRules {
                serialize: rename_all_ser_rule.get().unwrap_or(RenameRule::None),
                deserialize: rename_all_de_rule.get().unwrap_or(RenameRule::None),
            },
            rename_all_fields_rules: RenameAllRules {
                serialize: rename_all_fields_ser_rule.get().unwrap_or(RenameRule::None),
                deserialize: rename_all_fields_de_rule.get().unwrap_or(RenameRule::None),
            },
            ser_bound: ser_bound.get(),
            de_bound: de_bound.get(),
            tag: decide_tag(cx, item, untagged, internal_tag, content),
            type_from: type_from.get(),
            type_try_from: type_try_from.get(),
            type_into: type_into.get(),
            remote: remote.get(),
            identifier: decide_identifier(cx, item, field_identifier, variant_identifier),
            serde_path: serde_path.get(),
            is_packed,
            expecting: expecting.get(),
            non_exhaustive,
        }
    }

    pub const fn name(&self) -> &MultiName {
        &self.name
    }

    pub const fn rename_all_rules(&self) -> RenameAllRules {
        self.rename_all_rules
    }

    pub const fn rename_all_fields_rules(&self) -> RenameAllRules {
        self.rename_all_fields_rules
    }

    pub const fn transparent(&self) -> bool {
        self.transparent
    }

    pub const fn deny_unknown_fields(&self) -> bool {
        self.deny_unknown_fields
    }

    pub const fn default(&self) -> &Default {
        &self.default
    }

    pub fn ser_bound(&self) -> Option<&[syntax::WherePredicate]> {
        self.ser_bound.as_ref().map(|vec| &vec[..])
    }

    pub fn de_bound(&self) -> Option<&[syntax::WherePredicate]> {
        self.de_bound.as_ref().map(|vec| &vec[..])
    }

    pub const fn tag(&self) -> &TagType {
        &self.tag
    }

    pub const fn type_from(&self) -> Option<&syntax::Type> {
        self.type_from.as_ref()
    }

    pub const fn type_try_from(&self) -> Option<&syntax::Type> {
        self.type_try_from.as_ref()
    }

    pub const fn type_into(&self) -> Option<&syntax::Type> {
        self.type_into.as_ref()
    }

    pub const fn remote(&self) -> Option<&syntax::Path> {
        self.remote.as_ref()
    }

    pub const fn is_packed(&self) -> bool {
        self.is_packed
    }

    pub const fn identifier(&self) -> Identifier {
        self.identifier
    }

    pub const fn custom_serde_path(&self) -> Option<&syntax::Path> {
        self.serde_path.as_ref()
    }

    pub fn expecting(&self) -> Option<&str> {
        self.expecting.as_ref().map(String::as_ref)
    }

    pub const fn non_exhaustive(&self) -> bool {
        self.non_exhaustive
    }
}

fn decide_tag(
    cx: &Ctxt,
    item: &syntax::DeriveInput,
    untagged: BoolAttr,
    internal_tag: Attr<String>,
    content: Attr<String>,
) -> TagType {
    match (
        untagged.0.get_with_tokens(),
        internal_tag.get_with_tokens(),
        content.get_with_tokens(),
    ) {
        (None, None, None) => TagType::External,
        (Some(_), None, None) => TagType::None,
        (None, Some((_, tag)), None) => {
            if let syntax::Data::Enum(data) = &item.data {
                for variant in &data.variants {
                    match &variant.fields {
                        syntax::Fields::Named(_) | syntax::Fields::Unit => {}
                        syntax::Fields::Unnamed(fields) => {
                            if fields.unnamed.len() != 1 {
                                let msg =
                                    "#[serde(tag = \"...\")] cannot be used with tuple variants";
                                cx.error_spanned_by(variant, msg);
                                break;
                            }
                        }
                    }
                }
            }
            TagType::Internal { tag }
        }
        (Some((untagged_tokens, ())), Some((tag_tokens, _)), None) => {
            let msg = "enum cannot be both untagged and internally tagged";
            cx.error_spanned_by(untagged_tokens, msg);
            cx.error_spanned_by(tag_tokens, msg);
            TagType::External
        }
        (None, None, Some((content_tokens, _))) => {
            let msg = "#[serde(tag = \"...\", content = \"...\")] must be used together";
            cx.error_spanned_by(content_tokens, msg);
            TagType::External
        }
        (Some((untagged_tokens, ())), None, Some((content_tokens, _))) => {
            let msg = "untagged enum cannot have #[serde(content = \"...\")]";
            cx.error_spanned_by(untagged_tokens, msg);
            cx.error_spanned_by(content_tokens, msg);
            TagType::External
        }
        (None, Some((_, tag)), Some((_, content))) => TagType::Adjacent { tag, content },
        (Some((untagged_tokens, ())), Some((tag_tokens, _)), Some((content_tokens, _))) => {
            let msg = "untagged enum cannot have #[serde(tag = \"...\", content = \"...\")]";
            cx.error_spanned_by(untagged_tokens, msg);
            cx.error_spanned_by(tag_tokens, msg);
            cx.error_spanned_by(content_tokens, msg);
            TagType::External
        }
    }
}

fn decide_identifier(
    cx: &Ctxt,
    item: &syntax::DeriveInput,
    field_identifier: BoolAttr,
    variant_identifier: BoolAttr,
) -> Identifier {
    match (
        &item.data,
        field_identifier.0.get_with_tokens(),
        variant_identifier.0.get_with_tokens(),
    ) {
        (_, None, None) => Identifier::No,
        (_, Some((field_identifier_tokens, ())), Some((variant_identifier_tokens, ()))) => {
            let msg =
                "#[serde(field_identifier)] and #[serde(variant_identifier)] cannot both be set";
            cx.error_spanned_by(field_identifier_tokens, msg);
            cx.error_spanned_by(variant_identifier_tokens, msg);
            Identifier::No
        }
        (syntax::Data::Enum(_), Some(_), None) => Identifier::Field,
        (syntax::Data::Enum(_), None, Some(_)) => Identifier::Variant,
        (syntax::Data::Struct(syntax::DataStruct { struct_token, .. }), Some(_), None) => {
            let msg = "#[serde(field_identifier)] can only be used on an enum";
            cx.error_spanned_by(struct_token, msg);
            Identifier::No
        }
        (syntax::Data::Union(syntax::DataUnion { union_token, .. }), Some(_), None) => {
            let msg = "#[serde(field_identifier)] can only be used on an enum";
            cx.error_spanned_by(union_token, msg);
            Identifier::No
        }
        (syntax::Data::Struct(syntax::DataStruct { struct_token, .. }), None, Some(_)) => {
            let msg = "#[serde(variant_identifier)] can only be used on an enum";
            cx.error_spanned_by(struct_token, msg);
            Identifier::No
        }
        (syntax::Data::Union(syntax::DataUnion { union_token, .. }), None, Some(_)) => {
            let msg = "#[serde(variant_identifier)] can only be used on an enum";
            cx.error_spanned_by(union_token, msg);
            Identifier::No
        }
    }
}

pub struct Variant {
    name: MultiName,
    rename_all_rules: RenameAllRules,
    ser_bound: Option<Vec<syntax::WherePredicate>>,
    de_bound: Option<Vec<syntax::WherePredicate>>,
    skip_deserializing: bool,
    skip_serializing: bool,
    other: bool,
    serialize_with: Option<syntax::ExprPath>,
    deserialize_with: Option<syntax::ExprPath>,
    borrow: Option<BorrowAttribute>,
    untagged: bool,
}

struct BorrowAttribute {
    path: syntax::Path,
    lifetimes: Option<BTreeSet<syntax::Lifetime>>,
}

impl Variant {
    pub fn from_ast(cx: &Ctxt, variant: &syntax::Variant) -> Self {
        let mut ser_name = Attr::none(cx, RENAME);
        let mut de_name = Attr::none(cx, RENAME);
        let mut de_aliases = VecAttr::none(cx, RENAME);
        let mut skip_deserializing = BoolAttr::none(cx, SKIP_DESERIALIZING);
        let mut skip_serializing = BoolAttr::none(cx, SKIP_SERIALIZING);
        let mut rename_all_ser_rule = Attr::none(cx, RENAME_ALL);
        let mut rename_all_de_rule = Attr::none(cx, RENAME_ALL);
        let mut ser_bound = Attr::none(cx, BOUND);
        let mut de_bound = Attr::none(cx, BOUND);
        let mut other = BoolAttr::none(cx, OTHER);
        let mut serialize_with = Attr::none(cx, SERIALIZE_WITH);
        let mut deserialize_with = Attr::none(cx, DESERIALIZE_WITH);
        let mut borrow = Attr::none(cx, BORROW);
        let mut untagged = BoolAttr::none(cx, UNTAGGED);

        for attr in &variant.attrs {
            if attr.path() != SERDE {
                continue;
            }

            if let syntax::Meta::List(meta) = &attr.meta
                && meta.tokens.is_empty()
            {
                continue;
            }

            if let Err(err) = attr.parse_nested_meta(|meta| {
                if meta.path == RENAME {
                    let (ser, de) = get_multiple_renames(cx, &meta)?;
                    ser_name.set_opt(&meta.path, ser.as_ref().map(Name::from));
                    for de_value in de {
                        de_name.set_if_none(Name::from(&de_value));
                        de_aliases.insert(&meta.path, Name::from(&de_value));
                    }
                } else if meta.path == ALIAS {
                    if let Some(s) = get_lit_str(cx, ALIAS, &meta)? {
                        de_aliases.insert(&meta.path, Name::from(&s));
                    }
                } else if meta.path == RENAME_ALL {
                    let one_name = meta.input.peek_punct('=');
                    let (ser, de) = get_renames(cx, RENAME_ALL, &meta)?;
                    if let Some(ser) = ser {
                        match RenameRule::from_str(&ser.value()) {
                            Ok(rename_rule) => rename_all_ser_rule.set(&meta.path, rename_rule),
                            Err(err) => cx.error_spanned_by(ser, err),
                        }
                    }
                    if let Some(de) = de {
                        match RenameRule::from_str(&de.value()) {
                            Ok(rename_rule) => rename_all_de_rule.set(&meta.path, rename_rule),
                            Err(err) => {
                                if !one_name {
                                    cx.error_spanned_by(de, err);
                                }
                            }
                        }
                    }
                } else if meta.path == SKIP {
                    skip_serializing.set_true(&meta.path);
                    skip_deserializing.set_true(&meta.path);
                } else if meta.path == SKIP_DESERIALIZING {
                    skip_deserializing.set_true(&meta.path);
                } else if meta.path == SKIP_SERIALIZING {
                    skip_serializing.set_true(&meta.path);
                } else if meta.path == OTHER {
                    other.set_true(&meta.path);
                } else if meta.path == BOUND {
                    let (ser, de) = get_where_predicates(cx, &meta)?;
                    ser_bound.set_opt(&meta.path, ser);
                    de_bound.set_opt(&meta.path, de);
                } else if meta.path == WITH {
                    if let Some(path) = parse_lit_into_expr_path(cx, WITH, &meta)? {
                        let mut ser_path = path.clone();
                        ser_path
                            .path
                            .segments
                            .push(Ident::new("serialize", ser_path.span()).into());
                        serialize_with.set(&meta.path, ser_path);
                        let mut de_path = path;
                        de_path
                            .path
                            .segments
                            .push(Ident::new("deserialize", de_path.span()).into());
                        deserialize_with.set(&meta.path, de_path);
                    }
                } else if meta.path == SERIALIZE_WITH {
                    if let Some(path) = parse_lit_into_expr_path(cx, SERIALIZE_WITH, &meta)? {
                        serialize_with.set(&meta.path, path);
                    }
                } else if meta.path == DESERIALIZE_WITH {
                    if let Some(path) = parse_lit_into_expr_path(cx, DESERIALIZE_WITH, &meta)? {
                        deserialize_with.set(&meta.path, path);
                    }
                } else if meta.path == BORROW {
                    let borrow_attribute = if meta.input.peek_punct('=') {
                        let lifetimes = parse_lit_into_lifetimes(cx, &meta)?;
                        BorrowAttribute {
                            path: meta.path.clone(),
                            lifetimes: Some(lifetimes),
                        }
                    } else {
                        BorrowAttribute {
                            path: meta.path.clone(),
                            lifetimes: None,
                        }
                    };
                    match &variant.fields {
                        syntax::Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                            borrow.set(&meta.path, borrow_attribute);
                        }
                        _ => {
                            let msg = "#[serde(borrow)] may only be used on newtype variants";
                            cx.error_spanned_by(variant, msg);
                        }
                    }
                } else if meta.path == UNTAGGED {
                    untagged.set_true(&meta.path);
                } else {
                    let path = meta.path.to_token_stream().to_string().replace(' ', "");
                    return Err(
                        meta.error(format_args!("unknown serde variant attribute `{}`", path))
                    );
                }
                Ok(())
            }) {
                cx.syn_error(err);
            }
        }

        Self {
            name: MultiName::from_attrs(
                Name::from(&unraw(&variant.ident)),
                ser_name,
                de_name,
                Some(de_aliases),
            ),
            rename_all_rules: RenameAllRules {
                serialize: rename_all_ser_rule.get().unwrap_or(RenameRule::None),
                deserialize: rename_all_de_rule.get().unwrap_or(RenameRule::None),
            },
            ser_bound: ser_bound.get(),
            de_bound: de_bound.get(),
            skip_deserializing: skip_deserializing.get(),
            skip_serializing: skip_serializing.get(),
            other: other.get(),
            serialize_with: serialize_with.get(),
            deserialize_with: deserialize_with.get(),
            borrow: borrow.get(),
            untagged: untagged.get(),
        }
    }

    pub const fn name(&self) -> &MultiName {
        &self.name
    }

    pub const fn aliases(&self) -> &BTreeSet<Name> {
        self.name.deserialize_aliases()
    }

    pub fn rename_by_rules(&mut self, rules: RenameAllRules) {
        if !self.name.serialize_renamed {
            self.name.serialize.value =
                rules.serialize.apply_to_variant(&self.name.serialize.value);
        }
        if !self.name.deserialize_renamed {
            self.name.deserialize.value = rules
                .deserialize
                .apply_to_variant(&self.name.deserialize.value);
        }
        self.name
            .deserialize_aliases
            .insert(self.name.deserialize.clone());
    }

    pub const fn rename_all_rules(&self) -> RenameAllRules {
        self.rename_all_rules
    }

    pub fn ser_bound(&self) -> Option<&[syntax::WherePredicate]> {
        self.ser_bound.as_ref().map(|vec| &vec[..])
    }

    pub fn de_bound(&self) -> Option<&[syntax::WherePredicate]> {
        self.de_bound.as_ref().map(|vec| &vec[..])
    }

    pub const fn skip_deserializing(&self) -> bool {
        self.skip_deserializing
    }

    pub const fn skip_serializing(&self) -> bool {
        self.skip_serializing
    }

    pub const fn other(&self) -> bool {
        self.other
    }

    pub const fn serialize_with(&self) -> Option<&syntax::ExprPath> {
        self.serialize_with.as_ref()
    }

    pub const fn deserialize_with(&self) -> Option<&syntax::ExprPath> {
        self.deserialize_with.as_ref()
    }

    pub const fn untagged(&self) -> bool {
        self.untagged
    }
}

pub struct Field {
    name: MultiName,
    skip_serializing: bool,
    skip_deserializing: bool,
    skip_serializing_if: Option<syntax::ExprPath>,
    default: Default,
    serialize_with: Option<syntax::ExprPath>,
    deserialize_with: Option<syntax::ExprPath>,
    ser_bound: Option<Vec<syntax::WherePredicate>>,
    de_bound: Option<Vec<syntax::WherePredicate>>,
    borrowed_lifetimes: BTreeSet<syntax::Lifetime>,
    getter: Option<syntax::ExprPath>,
    flatten: bool,
    transparent: bool,
}

#[allow(clippy::enum_variant_names)]
pub enum Default {
    None,

    Default,

    Path(syntax::ExprPath),
}

impl Default {
    pub const fn is_none(&self) -> bool {
        match self {
            Self::None => true,
            Self::Default | Self::Path(_) => false,
        }
    }
}

impl Field {
    pub fn from_ast(
        cx: &Ctxt,
        index: usize,
        field: &syntax::Field,
        attrs: Option<&Variant>,
        container_default: &Default,
        private: &Ident,
    ) -> Self {
        let mut ser_name = Attr::none(cx, RENAME);
        let mut de_name = Attr::none(cx, RENAME);
        let mut de_aliases = VecAttr::none(cx, RENAME);
        let mut skip_serializing = BoolAttr::none(cx, SKIP_SERIALIZING);
        let mut skip_deserializing = BoolAttr::none(cx, SKIP_DESERIALIZING);
        let mut skip_serializing_if = Attr::none(cx, SKIP_SERIALIZING_IF);
        let mut default = Attr::none(cx, DEFAULT);
        let mut serialize_with = Attr::none(cx, SERIALIZE_WITH);
        let mut deserialize_with = Attr::none(cx, DESERIALIZE_WITH);
        let mut ser_bound = Attr::none(cx, BOUND);
        let mut de_bound = Attr::none(cx, BOUND);
        let mut borrowed_lifetimes = Attr::none(cx, BORROW);
        let mut getter = Attr::none(cx, GETTER);
        let mut flatten = BoolAttr::none(cx, FLATTEN);

        let ident = match &field.ident {
            Some(ident) => Name::from(&unraw(ident)),
            None => Name {
                value: index.to_string(),
                span: Span::call_site(),
            },
        };

        if let Some(borrow_attribute) = attrs.and_then(|variant| variant.borrow.as_ref())
            && let Ok(borrowable) = borrowable_lifetimes(cx, &ident, field)
        {
            if let Some(lifetimes) = &borrow_attribute.lifetimes {
                for lifetime in lifetimes {
                    if !borrowable.contains(lifetime) {
                        let msg = format!("field `{}` does not have lifetime {}", ident, lifetime);
                        cx.error_spanned_by(field, msg);
                    }
                }
                borrowed_lifetimes.set(&borrow_attribute.path, lifetimes.clone());
            } else {
                borrowed_lifetimes.set(&borrow_attribute.path, borrowable);
            }
        }

        for attr in &field.attrs {
            if attr.path() != SERDE {
                continue;
            }

            if let syntax::Meta::List(meta) = &attr.meta
                && meta.tokens.is_empty()
            {
                continue;
            }

            if let Err(err) = attr.parse_nested_meta(|meta| {
                if meta.path == RENAME {
                    let (ser, de) = get_multiple_renames(cx, &meta)?;
                    ser_name.set_opt(&meta.path, ser.as_ref().map(Name::from));
                    for de_value in de {
                        de_name.set_if_none(Name::from(&de_value));
                        de_aliases.insert(&meta.path, Name::from(&de_value));
                    }
                } else if meta.path == ALIAS {
                    if let Some(s) = get_lit_str(cx, ALIAS, &meta)? {
                        de_aliases.insert(&meta.path, Name::from(&s));
                    }
                } else if meta.path == DEFAULT {
                    if meta.input.peek_punct('=') {
                        if let Some(path) = parse_lit_into_expr_path(cx, DEFAULT, &meta)? {
                            default.set(&meta.path, Default::Path(path));
                        }
                    } else {
                        default.set(&meta.path, Default::Default);
                    }
                } else if meta.path == SKIP_SERIALIZING {
                    skip_serializing.set_true(&meta.path);
                } else if meta.path == SKIP_DESERIALIZING {
                    skip_deserializing.set_true(&meta.path);
                } else if meta.path == SKIP {
                    skip_serializing.set_true(&meta.path);
                    skip_deserializing.set_true(&meta.path);
                } else if meta.path == SKIP_SERIALIZING_IF {
                    if let Some(path) = parse_lit_into_expr_path(cx, SKIP_SERIALIZING_IF, &meta)? {
                        skip_serializing_if.set(&meta.path, path);
                    }
                } else if meta.path == SERIALIZE_WITH {
                    if let Some(path) = parse_lit_into_expr_path(cx, SERIALIZE_WITH, &meta)? {
                        serialize_with.set(&meta.path, path);
                    }
                } else if meta.path == DESERIALIZE_WITH {
                    if let Some(path) = parse_lit_into_expr_path(cx, DESERIALIZE_WITH, &meta)? {
                        deserialize_with.set(&meta.path, path);
                    }
                } else if meta.path == WITH {
                    if let Some(path) = parse_lit_into_expr_path(cx, WITH, &meta)? {
                        let mut ser_path = path.clone();
                        ser_path
                            .path
                            .segments
                            .push(Ident::new("serialize", ser_path.span()).into());
                        serialize_with.set(&meta.path, ser_path);
                        let mut de_path = path;
                        de_path
                            .path
                            .segments
                            .push(Ident::new("deserialize", de_path.span()).into());
                        deserialize_with.set(&meta.path, de_path);
                    }
                } else if meta.path == BOUND {
                    let (ser, de) = get_where_predicates(cx, &meta)?;
                    ser_bound.set_opt(&meta.path, ser);
                    de_bound.set_opt(&meta.path, de);
                } else if meta.path == BORROW {
                    if meta.input.peek_punct('=') {
                        let lifetimes = parse_lit_into_lifetimes(cx, &meta)?;
                        if let Ok(borrowable) = borrowable_lifetimes(cx, &ident, field) {
                            for lifetime in &lifetimes {
                                if !borrowable.contains(lifetime) {
                                    let msg = format!(
                                        "field `{}` does not have lifetime {}",
                                        ident, lifetime,
                                    );
                                    cx.error_spanned_by(field, msg);
                                }
                            }
                            borrowed_lifetimes.set(&meta.path, lifetimes);
                        }
                    } else {
                        if let Ok(borrowable) = borrowable_lifetimes(cx, &ident, field) {
                            borrowed_lifetimes.set(&meta.path, borrowable);
                        }
                    }
                } else if meta.path == GETTER {
                    if let Some(path) = parse_lit_into_expr_path(cx, GETTER, &meta)? {
                        getter.set(&meta.path, path);
                    }
                } else if meta.path == FLATTEN {
                    flatten.set_true(&meta.path);
                } else {
                    let path = meta.path.to_token_stream().to_string().replace(' ', "");
                    return Err(
                        meta.error(format_args!("unknown serde field attribute `{}`", path))
                    );
                }
                Ok(())
            }) {
                cx.syn_error(err);
            }
        }

        if let Default::None = *container_default
            && skip_deserializing.0.value.is_some()
        {
            default.set_if_none(Default::Default);
        }

        let mut borrowed_lifetimes = borrowed_lifetimes.get().unwrap_or_default();
        if !borrowed_lifetimes.is_empty() {
            if is_cow(&field.ty, is_str) {
                let mut path = syntax::Path {
                    leading_colon: None,
                    segments: Punctuated::new(),
                };
                let span = Span::call_site();
                path.segments.push(Ident::new("_serde", span).into());
                path.segments.push(private.clone().into());
                path.segments.push(Ident::new("de", span).into());
                path.segments
                    .push(Ident::new("borrow_cow_str", span).into());
                let expr = syntax::ExprPath {
                    attrs: Vec::new(),
                    qself: None,
                    path,
                };
                deserialize_with.set_if_none(expr);
            } else if is_cow(&field.ty, is_slice_u8) {
                let mut path = syntax::Path {
                    leading_colon: None,
                    segments: Punctuated::new(),
                };
                let span = Span::call_site();
                path.segments.push(Ident::new("_serde", span).into());
                path.segments.push(private.clone().into());
                path.segments.push(Ident::new("de", span).into());
                path.segments
                    .push(Ident::new("borrow_cow_bytes", span).into());
                let expr = syntax::ExprPath {
                    attrs: Vec::new(),
                    qself: None,
                    path,
                };
                deserialize_with.set_if_none(expr);
            }
        } else if is_implicitly_borrowed(&field.ty) {
            collect_lifetimes(&field.ty, &mut borrowed_lifetimes);
        }

        Self {
            name: MultiName::from_attrs(ident, ser_name, de_name, Some(de_aliases)),
            skip_serializing: skip_serializing.get(),
            skip_deserializing: skip_deserializing.get(),
            skip_serializing_if: skip_serializing_if.get(),
            default: default.get().unwrap_or(Default::None),
            serialize_with: serialize_with.get(),
            deserialize_with: deserialize_with.get(),
            ser_bound: ser_bound.get(),
            de_bound: de_bound.get(),
            borrowed_lifetimes,
            getter: getter.get(),
            flatten: flatten.get(),
            transparent: false,
        }
    }

    pub const fn name(&self) -> &MultiName {
        &self.name
    }

    pub const fn aliases(&self) -> &BTreeSet<Name> {
        self.name.deserialize_aliases()
    }

    pub fn rename_by_rules(&mut self, rules: RenameAllRules) {
        if !self.name.serialize_renamed {
            self.name.serialize.value = rules.serialize.apply_to_field(&self.name.serialize.value);
        }
        if !self.name.deserialize_renamed {
            self.name.deserialize.value = rules
                .deserialize
                .apply_to_field(&self.name.deserialize.value);
        }
        self.name
            .deserialize_aliases
            .insert(self.name.deserialize.clone());
    }

    pub const fn skip_serializing(&self) -> bool {
        self.skip_serializing
    }

    pub const fn skip_deserializing(&self) -> bool {
        self.skip_deserializing
    }

    pub const fn skip_serializing_if(&self) -> Option<&syntax::ExprPath> {
        self.skip_serializing_if.as_ref()
    }

    pub const fn default(&self) -> &Default {
        &self.default
    }

    pub const fn serialize_with(&self) -> Option<&syntax::ExprPath> {
        self.serialize_with.as_ref()
    }

    pub const fn deserialize_with(&self) -> Option<&syntax::ExprPath> {
        self.deserialize_with.as_ref()
    }

    pub fn ser_bound(&self) -> Option<&[syntax::WherePredicate]> {
        self.ser_bound.as_ref().map(|vec| &vec[..])
    }

    pub fn de_bound(&self) -> Option<&[syntax::WherePredicate]> {
        self.de_bound.as_ref().map(|vec| &vec[..])
    }

    pub const fn borrowed_lifetimes(&self) -> &BTreeSet<syntax::Lifetime> {
        &self.borrowed_lifetimes
    }

    pub const fn getter(&self) -> Option<&syntax::ExprPath> {
        self.getter.as_ref()
    }

    pub const fn flatten(&self) -> bool {
        self.flatten
    }

    pub const fn transparent(&self) -> bool {
        self.transparent
    }

    pub const fn mark_transparent(&mut self) {
        self.transparent = true;
    }
}

type SerAndDe<T> = (Option<T>, Option<T>);

fn get_ser_and_de<'c, T, F, R>(
    cx: &'c Ctxt,
    attr_name: Symbol,
    meta: &NestedMeta<'_>,
    f: F,
) -> syntax::Result<(VecAttr<'c, T>, VecAttr<'c, T>)>
where
    T: Clone,
    F: Fn(&Ctxt, Symbol, Symbol, &NestedMeta<'_>) -> syntax::Result<R>,
    R: Into<Option<T>>,
{
    let mut ser_meta = VecAttr::none(cx, attr_name);
    let mut de_meta = VecAttr::none(cx, attr_name);

    let lookahead = meta.input.lookahead1();
    if lookahead.peek_punct('=') {
        if let Some(both) = f(cx, attr_name, attr_name, meta)?.into() {
            ser_meta.insert(&meta.path, both.clone());
            de_meta.insert(&meta.path, both);
        }
    } else if lookahead.peek_group(syntax::Delimiter::Parenthesis) {
        meta.parse_nested_meta(|meta| {
            if meta.path == SERIALIZE {
                if let Some(v) = f(cx, attr_name, SERIALIZE, &meta)?.into() {
                    ser_meta.insert(&meta.path, v);
                }
            } else if meta.path == DESERIALIZE {
                if let Some(v) = f(cx, attr_name, DESERIALIZE, &meta)?.into() {
                    de_meta.insert(&meta.path, v);
                }
            } else {
                return Err(meta.error(format_args!(
                    "malformed {0} attribute, expected `{0}(serialize = ..., deserialize = ...)`",
                    attr_name,
                )));
            }
            Ok(())
        })?;
    } else {
        return Err(lookahead.error());
    }

    Ok((ser_meta, de_meta))
}

fn get_renames(
    cx: &Ctxt,
    attr_name: Symbol,
    meta: &NestedMeta<'_>,
) -> syntax::Result<SerAndDe<syntax::LitStr>> {
    let (ser, de) = get_ser_and_de(cx, attr_name, meta, get_lit_str2)?;
    Ok((ser.at_most_one(), de.at_most_one()))
}

fn get_multiple_renames(
    cx: &Ctxt,
    meta: &NestedMeta<'_>,
) -> syntax::Result<(Option<syntax::LitStr>, Vec<syntax::LitStr>)> {
    let (ser, de) = get_ser_and_de(cx, RENAME, meta, get_lit_str2)?;
    Ok((ser.at_most_one(), de.get()))
}

fn get_where_predicates(
    cx: &Ctxt,
    meta: &NestedMeta<'_>,
) -> syntax::Result<SerAndDe<Vec<syntax::WherePredicate>>> {
    let (ser, de) = get_ser_and_de(cx, BOUND, meta, parse_lit_into_where)?;
    Ok((ser.at_most_one(), de.at_most_one()))
}

fn get_lit_str(
    cx: &Ctxt,
    attr_name: Symbol,
    meta: &NestedMeta<'_>,
) -> syntax::Result<Option<syntax::LitStr>> {
    get_lit_str2(cx, attr_name, attr_name, meta)
}

fn get_lit_str2(
    cx: &Ctxt,
    attr_name: Symbol,
    meta_item_name: Symbol,
    meta: &NestedMeta<'_>,
) -> syntax::Result<Option<syntax::LitStr>> {
    let expr = meta.value()?.take_until_comma();
    let buffer = syntax::ParseBuffer::new(expr.clone());
    let literal = match buffer.parse::<syntax::Lit>() {
        Ok(syntax::Lit::Str(lit)) if buffer.is_empty() => Some(lit),
        _ => None,
    };
    if let Some(lit) = &literal {
        let suffix = lit.suffix();
        if !suffix.is_empty() {
            cx.error_spanned_by(
                lit,
                format!("unexpected suffix `{}` on string literal", suffix),
            );
        }
        Ok(Some(lit.clone()))
    } else {
        cx.error_spanned_by(
            expr,
            format!(
                "expected serde {} attribute to be a string: `{} = \"...\"`",
                attr_name, meta_item_name
            ),
        );
        Ok(None)
    }
}

fn parse_lit_into_path(
    cx: &Ctxt,
    attr_name: Symbol,
    meta: &NestedMeta<'_>,
) -> syntax::Result<Option<syntax::Path>> {
    let string = match get_lit_str(cx, attr_name, meta)? {
        Some(string) => string,
        None => return Ok(None),
    };

    Ok(match string.parse() {
        Ok(path) => Some(path),
        Err(_) => {
            cx.error_spanned_by(
                &string,
                format!("failed to parse path: {:?}", string.value()),
            );
            None
        }
    })
}

fn parse_lit_into_expr_path(
    cx: &Ctxt,
    attr_name: Symbol,
    meta: &NestedMeta<'_>,
) -> syntax::Result<Option<syntax::ExprPath>> {
    let string = match get_lit_str(cx, attr_name, meta)? {
        Some(string) => string,
        None => return Ok(None),
    };

    Ok(match string.parse() {
        Ok(expr) => Some(expr),
        Err(_) => {
            cx.error_spanned_by(
                &string,
                format!("failed to parse path: {:?}", string.value()),
            );
            None
        }
    })
}

fn parse_lit_into_where(
    cx: &Ctxt,
    attr_name: Symbol,
    meta_item_name: Symbol,
    meta: &NestedMeta<'_>,
) -> syntax::Result<Vec<syntax::WherePredicate>> {
    let string = match get_lit_str2(cx, attr_name, meta_item_name, meta)? {
        Some(string) => string,
        None => return Ok(Vec::new()),
    };

    Ok(
        match string.parse_with(
            Punctuated::<syntax::WherePredicate, syntax::token::Comma>::parse_terminated,
        ) {
            Ok(predicates) => Vec::from_iter(predicates),
            Err(err) => {
                cx.error_spanned_by(string, err);
                Vec::new()
            }
        },
    )
}

fn parse_lit_into_ty(
    cx: &Ctxt,
    attr_name: Symbol,
    meta: &NestedMeta<'_>,
) -> syntax::Result<Option<syntax::Type>> {
    let string = match get_lit_str(cx, attr_name, meta)? {
        Some(string) => string,
        None => return Ok(None),
    };

    Ok(match string.parse() {
        Ok(ty) => Some(ty),
        Err(_) => {
            cx.error_spanned_by(
                &string,
                format!("failed to parse type: {} = {:?}", attr_name, string.value()),
            );
            None
        }
    })
}

fn parse_lit_into_lifetimes(
    cx: &Ctxt,
    meta: &NestedMeta<'_>,
) -> syntax::Result<BTreeSet<syntax::Lifetime>> {
    let string = match get_lit_str(cx, BORROW, meta)? {
        Some(string) => string,
        None => return Ok(BTreeSet::new()),
    };

    if let Ok(lifetimes) = string.parse_with(|input: ParseStream| {
        let mut set = BTreeSet::new();
        while !input.is_empty() {
            let lifetime: Lifetime = input.parse()?;
            if !set.insert(lifetime.clone()) {
                cx.error_spanned_by(
                    &string,
                    format!("duplicate borrowed lifetime `{}`", lifetime),
                );
            }
            if input.is_empty() {
                break;
            }
            input.parse::<syntax::token::Plus>()?;
        }
        Ok(set)
    }) {
        if lifetimes.is_empty() {
            cx.error_spanned_by(string, "at least one lifetime must be borrowed");
        }
        return Ok(lifetimes);
    }

    cx.error_spanned_by(
        &string,
        format!("failed to parse borrowed lifetimes: {:?}", string.value()),
    );
    Ok(BTreeSet::new())
}

fn is_implicitly_borrowed(ty: &syntax::Type) -> bool {
    is_implicitly_borrowed_reference(ty) || is_option(ty, is_implicitly_borrowed_reference)
}

fn is_implicitly_borrowed_reference(ty: &syntax::Type) -> bool {
    is_reference(ty, is_str) || is_reference(ty, is_slice_u8)
}

fn is_cow(ty: &syntax::Type, elem: fn(&syntax::Type) -> bool) -> bool {
    let path = match ungroup(ty) {
        syntax::Type::Path(ty) => &ty.path,
        _ => {
            return false;
        }
    };
    let seg = match path.segments.last() {
        Some(seg) => seg,
        None => {
            return false;
        }
    };
    let args = match &seg.arguments {
        syntax::PathArguments::AngleBracketed(bracketed) => &bracketed.args,
        _ => {
            return false;
        }
    };
    seg.ident == "Cow"
        && args.len() == 2
        && match (&args[0], &args[1]) {
            (syntax::GenericArgument::Lifetime(_), syntax::GenericArgument::Type(arg)) => elem(arg),
            _ => false,
        }
}

fn is_option(ty: &syntax::Type, elem: fn(&syntax::Type) -> bool) -> bool {
    let path = match ungroup(ty) {
        syntax::Type::Path(ty) => &ty.path,
        _ => {
            return false;
        }
    };
    let seg = match path.segments.last() {
        Some(seg) => seg,
        None => {
            return false;
        }
    };
    let args = match &seg.arguments {
        syntax::PathArguments::AngleBracketed(bracketed) => &bracketed.args,
        _ => {
            return false;
        }
    };
    seg.ident == "Option"
        && args.len() == 1
        && match &args[0] {
            syntax::GenericArgument::Type(arg) => elem(arg),
            _ => false,
        }
}

fn is_reference(ty: &syntax::Type, elem: fn(&syntax::Type) -> bool) -> bool {
    match ungroup(ty) {
        syntax::Type::Reference(ty) => ty.mutability.is_none() && elem(&ty.elem),
        _ => false,
    }
}

fn is_str(ty: &syntax::Type) -> bool {
    is_primitive_type(ty, "str")
}

fn is_slice_u8(ty: &syntax::Type) -> bool {
    match ungroup(ty) {
        syntax::Type::Slice(ty) => is_primitive_type(&ty.elem, "u8"),
        _ => false,
    }
}

fn is_primitive_type(ty: &syntax::Type, primitive: &str) -> bool {
    match ungroup(ty) {
        syntax::Type::Path(ty) => ty.qself.is_none() && is_primitive_path(&ty.path, primitive),
        _ => false,
    }
}

fn is_primitive_path(path: &syntax::Path, primitive: &str) -> bool {
    path.leading_colon.is_none()
        && path.segments.len() == 1
        && path.segments[0].ident == primitive
        && path.segments[0].arguments.is_empty()
}

fn borrowable_lifetimes(
    cx: &Ctxt,
    name: &Name,
    field: &syntax::Field,
) -> Result<BTreeSet<syntax::Lifetime>, ()> {
    let mut lifetimes = BTreeSet::new();
    collect_lifetimes(&field.ty, &mut lifetimes);
    if lifetimes.is_empty() {
        let msg = format!("field `{}` has no lifetimes to borrow", name);
        cx.error_spanned_by(field, msg);
        Err(())
    } else {
        Ok(lifetimes)
    }
}

fn collect_lifetimes(ty: &syntax::Type, out: &mut BTreeSet<syntax::Lifetime>) {
    match ty {
        syntax::Type::Slice(ty) => {
            collect_lifetimes(&ty.elem, out);
        }
        syntax::Type::Array(ty) => {
            collect_lifetimes(&ty.elem, out);
        }
        syntax::Type::Ptr(ty) => {
            collect_lifetimes(&ty.elem, out);
        }
        syntax::Type::Reference(ty) => {
            out.extend(ty.lifetime.iter().cloned());
            collect_lifetimes(&ty.elem, out);
        }
        syntax::Type::Tuple(ty) => {
            for elem in &ty.elems {
                collect_lifetimes(elem, out);
            }
        }
        syntax::Type::Path(ty) => {
            if let Some(qself) = &ty.qself {
                collect_lifetimes(&qself.ty, out);
            }
            for seg in &ty.path.segments {
                if let syntax::PathArguments::AngleBracketed(bracketed) = &seg.arguments {
                    for arg in &bracketed.args {
                        match arg {
                            syntax::GenericArgument::Lifetime(lifetime) => {
                                out.insert(lifetime.clone());
                            }
                            syntax::GenericArgument::Type(ty) => {
                                collect_lifetimes(ty, out);
                            }
                            syntax::GenericArgument::AssocType(binding) => {
                                collect_lifetimes(&binding.ty, out);
                            }
                            syntax::GenericArgument::Const(_)
                            | syntax::GenericArgument::AssocConst(_)
                            | syntax::GenericArgument::Constraint(_) => {}
                        }
                    }
                }
            }
        }
        syntax::Type::Paren(ty) => {
            collect_lifetimes(&ty.elem, out);
        }
        syntax::Type::Group(ty) => {
            collect_lifetimes(&ty.elem, out);
        }
        syntax::Type::Macro(ty) => {
            collect_lifetimes_from_tokens(ty.tokens.clone(), out);
        }
        syntax::Type::BareFn(_)
        | syntax::Type::Never(_)
        | syntax::Type::TraitObject(_)
        | syntax::Type::ImplTrait(_)
        | syntax::Type::Infer(_)
        | syntax::Type::Verbatim(_) => {}
    }
}

fn collect_lifetimes_from_tokens(tokens: Tokens, out: &mut BTreeSet<syntax::Lifetime>) {
    let mut iter = tokens.into_iter();
    while let Some(tt) = iter.next() {
        match &tt {
            TokenTree::Punct(op) if op.as_char() == '\'' && op.spacing() == Spacing::Joint => {
                if let Some(TokenTree::Ident(ident)) = iter.next() {
                    out.insert(syntax::Lifetime {
                        apostrophe: op.span(),
                        ident: syntax::Ident::from_raw(ident),
                    });
                }
            }
            TokenTree::Group(group) => {
                let tokens = Tokens::from_stream(group.stream());
                collect_lifetimes_from_tokens(tokens, out);
            }
            _ => {}
        }
    }
}
