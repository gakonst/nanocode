use crate::internals::{Ctxt, Derive, attr, check};
use nanocode_macro_support as syntax;
use nanocode_macro_support::Ident;
use syntax::Punctuated;

pub struct Container<'a> {
    pub ident: syntax::Ident,

    pub attrs: attr::Container,

    pub data: Data<'a>,

    pub generics: &'a syntax::Generics,

    pub original: &'a syntax::DeriveInput,
}

pub enum Data<'a> {
    Enum(Vec<Variant<'a>>),
    Struct(Style, Vec<Field<'a>>),
}

pub struct Variant<'a> {
    pub ident: syntax::Ident,
    pub attrs: attr::Variant,
    pub style: Style,
    pub fields: Vec<Field<'a>>,
    pub original: &'a syntax::Variant,
}

pub struct Field<'a> {
    pub member: syntax::Member,
    pub attrs: attr::Field,
    pub ty: &'a syntax::Type,
    pub original: &'a syntax::Field,
}

#[derive(Copy, Clone)]
pub enum Style {
    Struct,

    Tuple,

    Newtype,

    Unit,
}

impl<'a> Container<'a> {
    pub fn from_ast(
        cx: &Ctxt,
        item: &'a syntax::DeriveInput,
        derive: Derive,
        private: &Ident,
    ) -> Option<Self> {
        let attrs = attr::Container::from_ast(cx, item);

        let mut data = match &item.data {
            syntax::Data::Enum(data) => {
                Data::Enum(enum_from_ast(cx, &data.variants, attrs.default(), private))
            }
            syntax::Data::Struct(data) => {
                let (style, fields) =
                    struct_from_ast(cx, &data.fields, None, attrs.default(), private);
                Data::Struct(style, fields)
            }
            syntax::Data::Union(_) => {
                cx.error_spanned_by(item, "Serde does not support derive for unions");
                return None;
            }
        };

        match &mut data {
            Data::Enum(variants) => {
                for variant in variants {
                    variant.attrs.rename_by_rules(attrs.rename_all_rules());
                    for field in &mut variant.fields {
                        field.attrs.rename_by_rules(
                            variant
                                .attrs
                                .rename_all_rules()
                                .or(attrs.rename_all_fields_rules()),
                        );
                    }
                }
            }
            Data::Struct(_, fields) => {
                for field in fields {
                    field.attrs.rename_by_rules(attrs.rename_all_rules());
                }
            }
        }

        let mut item = Container {
            ident: item.ident.clone(),
            attrs,
            data,
            generics: &item.generics,
            original: item,
        };
        check::check(cx, &mut item, derive);
        Some(item)
    }
}

impl<'a> Data<'a> {
    pub fn all_fields(&'a self) -> Box<dyn Iterator<Item = &'a Field<'a>> + 'a> {
        match self {
            Data::Enum(variants) => {
                Box::new(variants.iter().flat_map(|variant| variant.fields.iter()))
            }
            Data::Struct(_, fields) => Box::new(fields.iter()),
        }
    }

    pub fn has_getter(&self) -> bool {
        self.all_fields().any(|f| f.attrs.getter().is_some())
    }
}

fn enum_from_ast<'a>(
    cx: &Ctxt,
    variants: &'a Punctuated<syntax::Variant, syntax::token::Comma>,
    container_default: &attr::Default,
    private: &Ident,
) -> Vec<Variant<'a>> {
    let variants: Vec<Variant> = variants
        .iter()
        .map(|variant| {
            let attrs = attr::Variant::from_ast(cx, variant);
            let (style, fields) = struct_from_ast(
                cx,
                &variant.fields,
                Some(&attrs),
                container_default,
                private,
            );
            Variant {
                ident: variant.ident.clone(),
                attrs,
                style,
                fields,
                original: variant,
            }
        })
        .collect();

    let index_of_last_tagged_variant = variants
        .iter()
        .rposition(|variant| !variant.attrs.untagged());
    if let Some(index_of_last_tagged_variant) = index_of_last_tagged_variant {
        for variant in &variants[..index_of_last_tagged_variant] {
            if variant.attrs.untagged() {
                cx.error_spanned_by(&variant.ident, "all variants with the #[serde(untagged)] attribute must be placed at the end of the enum");
            }
        }
    }

    variants
}

fn struct_from_ast<'a>(
    cx: &Ctxt,
    fields: &'a syntax::Fields,
    attrs: Option<&attr::Variant>,
    container_default: &attr::Default,
    private: &Ident,
) -> (Style, Vec<Field<'a>>) {
    match fields {
        syntax::Fields::Named(fields) => (
            Style::Struct,
            fields_from_ast(cx, &fields.named, attrs, container_default, private),
        ),
        syntax::Fields::Unnamed(fields) if fields.unnamed.len() == 1 => (
            Style::Newtype,
            fields_from_ast(cx, &fields.unnamed, attrs, container_default, private),
        ),
        syntax::Fields::Unnamed(fields) => (
            Style::Tuple,
            fields_from_ast(cx, &fields.unnamed, attrs, container_default, private),
        ),
        syntax::Fields::Unit => (Style::Unit, Vec::new()),
    }
}

fn fields_from_ast<'a>(
    cx: &Ctxt,
    fields: &'a Punctuated<syntax::Field, syntax::token::Comma>,
    attrs: Option<&attr::Variant>,
    container_default: &attr::Default,
    private: &Ident,
) -> Vec<Field<'a>> {
    fields
        .iter()
        .enumerate()
        .map(|(i, field)| Field {
            member: match &field.ident {
                Some(ident) => syntax::Member::Named(ident.clone()),
                None => syntax::Member::Unnamed(i.into()),
            },
            attrs: attr::Field::from_ast(cx, i, field, attrs, container_default, private),
            ty: &field.ty,
            original: field,
        })
        .collect()
}
