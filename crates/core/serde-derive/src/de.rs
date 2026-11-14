use crate::deprecated::allow_deprecated;
use crate::fragment::{Expr, Fragment, Stmts};
use crate::internals::ast::{Container, Data, Field, Style, Variant};
use crate::internals::name::Name;
use crate::internals::{Ctxt, Derive, attr, replace_receiver, ungroup};
use crate::{bound, dummy, pretend, private, this};
use nanocode_macro_support::{Span, Tokens};
use nanocode_macro_support::{ToTokens, quote, quote_spanned};
use std::collections::BTreeSet;
use std::ptr;
use syntax::Punctuated;
use syntax::Spanned;
use syntax::{Ident, Index, Member, parse_quote};

mod enum_;
mod enum_adjacently;
mod enum_externally;
mod enum_internally;
mod enum_untagged;
mod identifier;
mod struct_;
mod tuple;
mod unit;
use nanocode_macro_support as syntax;

pub fn expand_derive_deserialize(input: &mut syntax::DeriveInput) -> syntax::Result<Tokens> {
    replace_receiver(input);

    let ctxt = Ctxt::new();
    let cont = match Container::from_ast(&ctxt, input, Derive::Deserialize, &private.ident()) {
        Some(cont) => cont,
        None => return Err(ctxt.check().unwrap_err()),
    };
    precondition(&ctxt, &cont);
    ctxt.check()?;

    let ident = &cont.ident;
    let params = Parameters::new(&cont);
    let (de_impl_generics, _, ty_generics, where_clause) = params.generics_with_de_lifetime();
    let body = Stmts(deserialize_body(&cont, &params));
    let delife = params.borrowed.de_lifetime();
    let allow_deprecated = allow_deprecated(input);

    let impl_block = if let Some(remote) = cont.attrs.remote() {
        let vis = &input.vis;
        let used = pretend::pretend_used(&cont, params.is_packed);
        quote! {
            #[automatically_derived]
            #allow_deprecated
            impl #de_impl_generics #ident #ty_generics #where_clause {
                #vis fn deserialize<__D>(__deserializer: __D) -> _serde::#private::Result<#remote #ty_generics, __D::Error>
                where
                    __D: _serde::Deserializer<#delife>,
                {
                    #used
                    #body
                }
            }
        }
    } else {
        quote! {
            #[automatically_derived]
            #allow_deprecated
            impl #de_impl_generics _serde::Deserialize<#delife> for #ident #ty_generics #where_clause {
                fn deserialize<__D>(__deserializer: __D) -> _serde::#private::Result<Self, __D::Error>
                where
                    __D: _serde::Deserializer<#delife>,
                {
                    #body
                }
            }
        }
    };

    Ok(dummy::wrap_in_const(
        cont.attrs.custom_serde_path(),
        impl_block,
    ))
}

fn precondition(cx: &Ctxt, cont: &Container) {
    precondition_sized(cx, cont);
    precondition_no_de_lifetime(cx, cont);
}

fn precondition_sized(cx: &Ctxt, cont: &Container) {
    if let Data::Struct(_, fields) = &cont.data
        && let Some(last) = fields.last()
        && let syntax::Type::Slice(_) = ungroup(last.ty)
    {
        cx.error_spanned_by(
            cont.original,
            "cannot deserialize a dynamically sized struct",
        );
    }
}

fn precondition_no_de_lifetime(cx: &Ctxt, cont: &Container) {
    if let BorrowedLifetimes::Borrowed(_) = borrowed_lifetimes(cont) {
        for param in cont.generics.lifetimes() {
            if param.lifetime.to_string() == "'de" {
                cx.error_spanned_by(
                    &param.lifetime,
                    "cannot deserialize when there is a lifetime parameter called 'de",
                );
                return;
            }
        }
    }
}

struct Parameters {
    local: syntax::Ident,

    this_type: syntax::Path,

    this_value: syntax::Path,

    generics: syntax::Generics,

    borrowed: BorrowedLifetimes,

    has_getter: bool,

    is_packed: bool,
}

impl Parameters {
    fn new(cont: &Container) -> Self {
        let local = cont.ident.clone();
        let this_type = this::this_type(cont);
        let this_value = this::this_value(cont);
        let borrowed = borrowed_lifetimes(cont);
        let generics = build_generics(cont, &borrowed);
        let has_getter = cont.data.has_getter();
        let is_packed = cont.attrs.is_packed();

        Self {
            local,
            this_type,
            this_value,
            generics,
            borrowed,
            has_getter,
            is_packed,
        }
    }

    fn type_name(&self) -> String {
        self.this_type.segments.last().unwrap().ident.to_string()
    }

    const fn generics_with_de_lifetime(
        &self,
    ) -> (
        DeImplGenerics<'_>,
        DeTypeGenerics<'_>,
        syntax::TypeGenerics<'_>,
        Option<&syntax::WhereClause>,
    ) {
        let de_impl_generics = DeImplGenerics(self);
        let de_ty_generics = DeTypeGenerics(self);
        let (_, ty_generics, where_clause) = self.generics.split_for_impl();
        (de_impl_generics, de_ty_generics, ty_generics, where_clause)
    }
}

fn build_generics(cont: &Container, borrowed: &BorrowedLifetimes) -> syntax::Generics {
    let generics = bound::without_defaults(cont.generics);

    let generics = bound::with_where_predicates_from_fields(cont, &generics, attr::Field::de_bound);

    let generics =
        bound::with_where_predicates_from_variants(cont, &generics, attr::Variant::de_bound);

    match cont.attrs.de_bound() {
        Some(predicates) => bound::with_where_predicates(&generics, predicates),
        None => {
            let generics = match *cont.attrs.default() {
                attr::Default::Default => bound::with_self_bound(
                    cont,
                    &generics,
                    &parse_quote!(_serde::#private::Default),
                ),
                attr::Default::None | attr::Default::Path(_) => generics,
            };

            let delife = borrowed.de_lifetime();
            let generics = bound::with_bound(
                cont,
                &generics,
                needs_deserialize_bound,
                &parse_quote!(_serde::Deserialize<#delife>),
            );

            bound::with_bound(
                cont,
                &generics,
                requires_default,
                &parse_quote!(_serde::#private::Default),
            )
        }
    }
}

fn needs_deserialize_bound(field: &attr::Field, variant: Option<&attr::Variant>) -> bool {
    !field.skip_deserializing()
        && field.deserialize_with().is_none()
        && field.de_bound().is_none()
        && variant.is_none_or(|variant| {
            !variant.skip_deserializing()
                && variant.deserialize_with().is_none()
                && variant.de_bound().is_none()
        })
}

const fn requires_default(field: &attr::Field, _variant: Option<&attr::Variant>) -> bool {
    matches!(*field.default(), attr::Default::Default)
}

enum BorrowedLifetimes {
    Borrowed(BTreeSet<syntax::Lifetime>),
    Static,
}

impl BorrowedLifetimes {
    fn de_lifetime(&self) -> syntax::Lifetime {
        match *self {
            Self::Borrowed(_) => syntax::Lifetime::new("'de", Span::call_site()),
            Self::Static => syntax::Lifetime::new("'static", Span::call_site()),
        }
    }

    fn de_lifetime_param(&self) -> Option<syntax::LifetimeParam> {
        match self {
            Self::Borrowed(bounds) => Some(syntax::LifetimeParam {
                attrs: Vec::new(),
                lifetime: syntax::Lifetime::new("'de", Span::call_site()),
                colon_token: None,
                bounds: bounds.iter().cloned().collect(),
            }),
            Self::Static => None,
        }
    }
}

fn borrowed_lifetimes(cont: &Container) -> BorrowedLifetimes {
    let mut lifetimes = BTreeSet::new();
    for field in cont.data.all_fields() {
        if !field.attrs.skip_deserializing() {
            lifetimes.extend(field.attrs.borrowed_lifetimes().iter().cloned());
        }
    }
    if lifetimes.iter().any(|b| b.to_string() == "'static") {
        BorrowedLifetimes::Static
    } else {
        BorrowedLifetimes::Borrowed(lifetimes)
    }
}

fn deserialize_body(cont: &Container, params: &Parameters) -> Fragment {
    if cont.attrs.transparent() {
        deserialize_transparent(cont, params)
    } else if let Some(type_from) = cont.attrs.type_from() {
        deserialize_from(type_from)
    } else if let Some(type_try_from) = cont.attrs.type_try_from() {
        deserialize_try_from(type_try_from)
    } else if let attr::Identifier::No = cont.attrs.identifier() {
        match &cont.data {
            Data::Enum(variants) => enum_::deserialize(params, variants, &cont.attrs),
            Data::Struct(Style::Struct, fields) => {
                struct_::deserialize(params, fields, &cont.attrs, StructForm::Struct)
            }
            Data::Struct(Style::Tuple, fields) | Data::Struct(Style::Newtype, fields) => {
                tuple::deserialize(params, fields, &cont.attrs, TupleForm::Tuple)
            }
            Data::Struct(Style::Unit, _) => unit::deserialize(params, &cont.attrs),
        }
    } else {
        match &cont.data {
            Data::Enum(variants) => identifier::deserialize_custom(params, variants, &cont.attrs),
            Data::Struct(_, _) => unreachable!("checked in serde_derive_internals"),
        }
    }
}

fn deserialize_transparent(cont: &Container, params: &Parameters) -> Fragment {
    let fields = match &cont.data {
        Data::Struct(_, fields) => fields,
        Data::Enum(_) => unreachable!(),
    };

    let this_value = &params.this_value;
    let transparent_field = fields.iter().find(|f| f.attrs.transparent()).unwrap();

    let path = match transparent_field.attrs.deserialize_with() {
        Some(path) => quote!(#path),
        None => {
            let span = transparent_field.original.span();
            quote_spanned!(span=> _serde::Deserialize::deserialize)
        }
    };

    let assign = fields.iter().map(|field| {
        let member = &field.member;
        if ptr::eq(field, transparent_field) {
            quote!(#member: __transparent)
        } else {
            let value = match field.attrs.default() {
                attr::Default::Default => quote!(_serde::#private::Default::default()),

                attr::Default::Path(path) => quote_spanned!(path.span()=> #path()),
                attr::Default::None => quote!(_serde::#private::PhantomData),
            };
            quote!(#member: #value)
        }
    });

    quote_block! {
        _serde::#private::Result::map(
            #path(__deserializer),
            |__transparent| #this_value { #(#assign),* })
    }
}

fn deserialize_from(type_from: &syntax::Type) -> Fragment {
    quote_block! {
        _serde::#private::Result::map(
            <#type_from as _serde::Deserialize>::deserialize(__deserializer),
            _serde::#private::From::from)
    }
}

fn deserialize_try_from(type_try_from: &syntax::Type) -> Fragment {
    quote_block! {
        _serde::#private::Result::and_then(
            <#type_try_from as _serde::Deserialize>::deserialize(__deserializer),
            |v| _serde::#private::TryFrom::try_from(v).map_err(_serde::de::Error::custom))
    }
}

enum TupleForm<'a> {
    Tuple,

    ExternallyTagged(&'a syntax::Ident),

    Untagged(&'a syntax::Ident),
}

fn deserialize_seq(
    type_path: &Tokens,
    params: &Parameters,
    fields: &[Field],
    is_struct: bool,
    cattrs: &attr::Container,
    expecting: &str,
) -> Fragment {
    let vars = (0..fields.len()).map(field_i as fn(_) -> _);

    let deserialized_count = fields
        .iter()
        .filter(|field| !field.attrs.skip_deserializing())
        .count();
    let expecting = if deserialized_count == 1 {
        format!("{} with 1 element", expecting)
    } else {
        format!("{} with {} elements", expecting, deserialized_count)
    };
    let expecting = cattrs.expecting().unwrap_or(&expecting);

    let mut index_in_seq = 0_usize;
    let let_values = vars.clone().zip(fields).map(|(var, field)| {
        if field.attrs.skip_deserializing() {
            let default = Expr(expr_is_missing(field, cattrs));
            quote! {
                let #var = #default;
            }
        } else {
            let visit = match field.attrs.deserialize_with() {
                None => {
                    let field_ty = field.ty;
                    let span = field.original.span();
                    let func =
                        quote_spanned!(span=> _serde::de::SeqAccess::next_element::<#field_ty>);
                    quote!(#func(&mut __seq)?)
                }
                Some(path) => {
                    let (wrapper, wrapper_ty) = wrap_deserialize_field_with(params, field.ty, path);
                    quote!({
                        #wrapper
                        _serde::#private::Option::map(
                            _serde::de::SeqAccess::next_element::<#wrapper_ty>(&mut __seq)?,
                            |__wrap| __wrap.value)
                    })
                }
            };
            let value_if_none = expr_is_missing_seq(None, index_in_seq, field, cattrs, expecting);
            let assign = quote! {
                let #var = match #visit {
                    _serde::#private::Some(__value) => __value,
                    _serde::#private::None => #value_if_none,
                };
            };
            index_in_seq += 1;
            assign
        }
    });

    let mut result = if is_struct {
        let names = fields.iter().map(|f| &f.member);
        quote! {
            #type_path { #( #names: #vars ),* }
        }
    } else {
        quote! {
            #type_path ( #(#vars),* )
        }
    };

    if params.has_getter {
        let this_type = &params.this_type;
        let (_, ty_generics, _) = params.generics.split_for_impl();
        result = quote! {
            _serde::#private::Into::<#this_type #ty_generics>::into(#result)
        };
    }

    let let_default = match cattrs.default() {
        attr::Default::Default => Some(quote!(
            let __default: Self::Value = _serde::#private::Default::default();
        )),

        attr::Default::Path(path) => Some(quote_spanned!(path.span()=>
            let __default: Self::Value = #path();
        )),
        attr::Default::None => None,
    };

    quote_block! {
        #let_default
        #(#let_values)*
        _serde::#private::Ok(#result)
    }
}

enum StructForm<'a> {
    Struct,

    ExternallyTagged(&'a syntax::Ident),

    InternallyTagged(&'a syntax::Ident),

    Untagged(&'a syntax::Ident),
}

struct FieldWithAliases<'a> {
    ident: Ident,
    aliases: &'a BTreeSet<Name>,
}

fn field_i(i: usize) -> Ident {
    Ident::new(&format!("__field{}", i), Span::call_site())
}

fn wrap_deserialize_with(
    params: &Parameters,
    value_ty: &Tokens,
    deserialize_with: &syntax::ExprPath,
) -> (Tokens, Tokens) {
    let this_type = &params.this_type;
    let (de_impl_generics, de_ty_generics, ty_generics, where_clause) =
        params.generics_with_de_lifetime();
    let delife = params.borrowed.de_lifetime();
    let deserializer_var = quote!(__deserializer);

    let value = quote_spanned! {deserialize_with.span()=>
        #deserialize_with(#deserializer_var)?
    };
    let wrapper = quote! {
        #[doc(hidden)]
        struct __DeserializeWith #de_impl_generics #where_clause {
            value: #value_ty,
            phantom: _serde::#private::PhantomData<#this_type #ty_generics>,
            lifetime: _serde::#private::PhantomData<&#delife ()>,
        }

        #[automatically_derived]
        impl #de_impl_generics _serde::Deserialize<#delife> for __DeserializeWith #de_ty_generics #where_clause {
            fn deserialize<__D>(#deserializer_var: __D) -> _serde::#private::Result<Self, __D::Error>
            where
                __D: _serde::Deserializer<#delife>,
            {
                _serde::#private::Ok(__DeserializeWith {
                    value: #value,
                    phantom: _serde::#private::PhantomData,
                    lifetime: _serde::#private::PhantomData,
                })
            }
        }
    };

    let wrapper_ty = quote!(__DeserializeWith #de_ty_generics);

    (wrapper, wrapper_ty)
}

fn wrap_deserialize_field_with(
    params: &Parameters,
    field_ty: &syntax::Type,
    deserialize_with: &syntax::ExprPath,
) -> (Tokens, Tokens) {
    wrap_deserialize_with(params, &quote!(#field_ty), deserialize_with)
}

fn unwrap_to_variant_closure(params: &Parameters, variant: &Variant, with_wrapper: bool) -> Tokens {
    let this_value = &params.this_value;
    let variant_ident = &variant.ident;

    let (arg, wrapper) = if with_wrapper {
        (quote! { __wrap }, quote! { __wrap.value })
    } else {
        let field_tys = variant.fields.iter().map(|field| field.ty);
        (quote! { __wrap: (#(#field_tys),*) }, quote! { __wrap })
    };

    let field_access = (0..variant.fields.len()).map(|n| {
        Member::Unnamed(Index {
            index: n as u32,
            span: Span::call_site(),
        })
    });

    match variant.style {
        Style::Struct if variant.fields.len() == 1 => {
            let member = &variant.fields[0].member;
            quote! {
                |#arg| #this_value::#variant_ident { #member: #wrapper }
            }
        }
        Style::Struct => {
            let members = variant.fields.iter().map(|field| &field.member);
            quote! {
                |#arg| #this_value::#variant_ident { #(#members: #wrapper.#field_access),* }
            }
        }
        Style::Tuple => quote! {
            |#arg| #this_value::#variant_ident(#(#wrapper.#field_access),*)
        },
        Style::Newtype => quote! {
            |#arg| #this_value::#variant_ident(#wrapper)
        },
        Style::Unit => quote! {
            |#arg| #this_value::#variant_ident
        },
    }
}

fn expr_is_missing(field: &Field, cattrs: &attr::Container) -> Fragment {
    match field.attrs.default() {
        attr::Default::Default => {
            let span = field.original.span();
            let func = quote_spanned!(span=> _serde::#private::Default::default);
            return quote_expr!(#func());
        }
        attr::Default::Path(path) => {
            return Fragment::Expr(quote_spanned!(path.span()=> #path()));
        }
        attr::Default::None => {}
    }

    match *cattrs.default() {
        attr::Default::Default | attr::Default::Path(_) => {
            let member = &field.member;
            return quote_expr!(__default.#member);
        }
        attr::Default::None => {}
    }

    let name = field.attrs.name().deserialize_name();
    match field.attrs.deserialize_with() {
        None => {
            let span = field.original.span();
            let func = quote_spanned!(span=> _serde::#private::de::missing_field);
            quote_expr! {
                #func(#name)?
            }
        }
        Some(_) => {
            quote_expr! {
                return _serde::#private::Err(<__A::Error as _serde::de::Error>::missing_field(#name))
            }
        }
    }
}

fn expr_is_missing_seq(
    assign_to: Option<Tokens>,
    index: usize,
    field: &Field,
    cattrs: &attr::Container,
    expecting: &str,
) -> Tokens {
    match field.attrs.default() {
        attr::Default::Default => {
            let span = field.original.span();
            return quote_spanned!(span=> #assign_to _serde::#private::Default::default());
        }
        attr::Default::Path(path) => {
            return quote_spanned!(path.span()=> #assign_to #path());
        }
        attr::Default::None => {}
    }

    match *cattrs.default() {
        attr::Default::Default | attr::Default::Path(_) => {
            let member = &field.member;
            quote!(#assign_to __default.#member)
        }
        attr::Default::None => quote!(
            return _serde::#private::Err(_serde::de::Error::invalid_length(#index, &#expecting))
        ),
    }
}

fn effective_style(variant: &Variant) -> Style {
    match variant.style {
        Style::Newtype if variant.fields[0].attrs.skip_deserializing() => Style::Unit,
        other => other,
    }
}

fn has_flatten(fields: &[Field]) -> bool {
    fields
        .iter()
        .any(|field| field.attrs.flatten() && !field.attrs.skip_deserializing())
}

struct DeImplGenerics<'a>(&'a Parameters);

impl<'a> ToTokens for DeImplGenerics<'a> {
    fn to_tokens(&self, tokens: &mut Tokens) {
        let mut generics = self.0.generics.clone();
        if let Some(de_lifetime) = self.0.borrowed.de_lifetime_param() {
            generics.params = Some(syntax::GenericParam::Lifetime(de_lifetime))
                .into_iter()
                .chain(generics.params)
                .collect();
        }
        let (impl_generics, _, _) = generics.split_for_impl();
        impl_generics.to_tokens(tokens);
    }
}

struct DeTypeGenerics<'a>(&'a Parameters);

fn de_type_generics_to_tokens(
    mut generics: syntax::Generics,
    borrowed: &BorrowedLifetimes,
    tokens: &mut Tokens,
) {
    if borrowed.de_lifetime_param().is_some() {
        let def = syntax::LifetimeParam {
            attrs: Vec::new(),
            lifetime: syntax::Lifetime::new("'de", Span::call_site()),
            colon_token: None,
            bounds: Punctuated::new(),
        };

        generics.params = Some(syntax::GenericParam::Lifetime(def))
            .into_iter()
            .chain(generics.params)
            .collect();
    }
    let (_, ty_generics, _) = generics.split_for_impl();
    ty_generics.to_tokens(tokens);
}

impl<'a> ToTokens for DeTypeGenerics<'a> {
    fn to_tokens(&self, tokens: &mut Tokens) {
        de_type_generics_to_tokens(self.0.generics.clone(), &self.0.borrowed, tokens);
    }
}
