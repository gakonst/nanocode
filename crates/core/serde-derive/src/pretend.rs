use crate::internals::ast::{Container, Data, Field, Style, Variant};
use crate::private;
use nanocode_macro_support::Tokens;
use nanocode_macro_support::{format_ident, quote};

pub fn pretend_used(cont: &Container, is_packed: bool) -> Tokens {
    let pretend_fields = pretend_fields_used(cont, is_packed);
    let pretend_variants = pretend_variants_used(cont);

    quote! {
        #pretend_fields
        #pretend_variants
    }
}

fn pretend_fields_used(cont: &Container, is_packed: bool) -> Tokens {
    match &cont.data {
        Data::Enum(variants) => pretend_fields_used_enum(cont, variants),
        Data::Struct(Style::Struct | Style::Tuple | Style::Newtype, fields) => {
            if is_packed {
                pretend_fields_used_struct_packed(cont, fields)
            } else {
                pretend_fields_used_struct(cont, fields)
            }
        }
        Data::Struct(Style::Unit, _) => quote!(),
    }
}

fn pretend_fields_used_struct(cont: &Container, fields: &[Field]) -> Tokens {
    let type_ident = &cont.ident;
    let (_, ty_generics, _) = cont.generics.split_for_impl();

    let members = fields.iter().map(|field| &field.member);
    let placeholders = (0usize..).map(|i| format_ident!("__v{}", i));

    quote! {
        match _serde::#private::None::<&#type_ident #ty_generics> {
            _serde::#private::Some(#type_ident { #(#members: #placeholders),* }) => {}
            _ => {}
        }
    }
}

fn pretend_fields_used_struct_packed(cont: &Container, fields: &[Field]) -> Tokens {
    let type_ident = &cont.ident;
    let (_, ty_generics, _) = cont.generics.split_for_impl();

    let members = fields.iter().map(|field| &field.member).collect::<Vec<_>>();

    let private2 = private;
    quote! {
        match _serde::#private::None::<&#type_ident #ty_generics> {
            _serde::#private::Some(__v @ #type_ident { #(#members: _),* }) => {
                #(
                    let _ = _serde::#private2::ptr::addr_of!(__v.#members);
                )*
            }
            _ => {}
        }
    }
}

fn pretend_fields_used_enum(cont: &Container, variants: &[Variant]) -> Tokens {
    let type_ident = &cont.ident;
    let (_, ty_generics, _) = cont.generics.split_for_impl();

    let patterns = variants
        .iter()
        .filter_map(|variant| match variant.style {
            Style::Struct | Style::Tuple | Style::Newtype => {
                let variant_ident = &variant.ident;
                let members = variant.fields.iter().map(|field| &field.member);
                let placeholders = (0usize..).map(|i| format_ident!("__v{}", i));
                Some(quote!(#type_ident::#variant_ident { #(#members: #placeholders),* }))
            }
            Style::Unit => None,
        })
        .collect::<Vec<_>>();

    let private2 = private;
    quote! {
        match _serde::#private::None::<&#type_ident #ty_generics> {
            #(
                _serde::#private2::Some(#patterns) => {}
            )*
            _ => {}
        }
    }
}

fn pretend_variants_used(cont: &Container) -> Tokens {
    let variants = match &cont.data {
        Data::Enum(variants) => variants,
        Data::Struct(_, _) => {
            return quote!();
        }
    };

    let type_ident = &cont.ident;
    let (_, ty_generics, _) = cont.generics.split_for_impl();
    let turbofish = ty_generics.as_turbofish();

    let cases = variants.iter().map(|variant| {
        let variant_ident = &variant.ident;
        let placeholders = &(0..variant.fields.len())
            .map(|i| format_ident!("__v{}", i))
            .collect::<Vec<_>>();

        let pat = match variant.style {
            Style::Struct => {
                let members = variant.fields.iter().map(|field| &field.member);
                quote!({ #(#members: #placeholders),* })
            }
            Style::Tuple | Style::Newtype => quote!(( #(#placeholders),* )),
            Style::Unit => quote!(),
        };

        quote! {
            match _serde::#private::None {
                _serde::#private::Some((#(#placeholders,)*)) => {
                    let _ = #type_ident::#variant_ident #turbofish #pat;
                }
                _ => {}
            }
        }
    });

    quote!(#(#cases)*)
}
