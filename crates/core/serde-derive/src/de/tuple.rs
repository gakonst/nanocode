use crate::de::{Parameters, TupleForm, deserialize_seq, has_flatten};
use crate::fragment::{Fragment, Stmts};
use crate::internals::ast::Field;
use crate::internals::attr;
use crate::private;
use nanocode_macro_support as syntax;
use nanocode_macro_support::Tokens;
use nanocode_macro_support::{quote, quote_spanned};
use syntax::Spanned;

pub(super) fn deserialize(
    params: &Parameters,
    fields: &[Field],
    cattrs: &attr::Container,
    form: TupleForm,
) -> Fragment {
    assert!(
        !has_flatten(fields),
        "tuples and tuple variants cannot have flatten fields"
    );

    let field_count = fields
        .iter()
        .filter(|field| !field.attrs.skip_deserializing())
        .count();

    let this_type = &params.this_type;
    let this_value = &params.this_value;
    let (de_impl_generics, de_ty_generics, ty_generics, where_clause) =
        params.generics_with_de_lifetime();
    let delife = params.borrowed.de_lifetime();

    let construct = if params.has_getter {
        let local = &params.local;
        quote!(#local)
    } else {
        quote!(#this_value)
    };

    let type_path = match form {
        TupleForm::Tuple => construct,
        TupleForm::ExternallyTagged(variant_ident) | TupleForm::Untagged(variant_ident) => {
            quote!(#construct::#variant_ident)
        }
    };
    let expecting = match form {
        TupleForm::Tuple => format!("tuple struct {}", params.type_name()),
        TupleForm::ExternallyTagged(variant_ident) | TupleForm::Untagged(variant_ident) => {
            format!("tuple variant {}::{}", params.type_name(), variant_ident)
        }
    };
    let expecting = cattrs.expecting().unwrap_or(&expecting);

    let nfields = fields.len();

    let visit_newtype_struct = match form {
        TupleForm::Tuple if nfields == 1 => {
            Some(deserialize_newtype_struct(&type_path, params, &fields[0]))
        }
        _ => None,
    };

    let visit_seq = Stmts(deserialize_seq(
        &type_path, params, fields, false, cattrs, expecting,
    ));

    let visitor_expr = quote! {
        __Visitor {
            marker: _serde::#private::PhantomData::<#this_type #ty_generics>,
            lifetime: _serde::#private::PhantomData,
        }
    };
    let dispatch = match form {
        TupleForm::Tuple if nfields == 1 => {
            let type_name = cattrs.name().deserialize_name();
            quote! {
                _serde::Deserializer::deserialize_newtype_struct(__deserializer, #type_name, #visitor_expr)
            }
        }
        TupleForm::Tuple => {
            let type_name = cattrs.name().deserialize_name();
            quote! {
                _serde::Deserializer::deserialize_tuple_struct(__deserializer, #type_name, #field_count, #visitor_expr)
            }
        }
        TupleForm::ExternallyTagged(_) => quote! {
            _serde::de::VariantAccess::tuple_variant(__variant, #field_count, #visitor_expr)
        },
        TupleForm::Untagged(_) => quote! {
            _serde::Deserializer::deserialize_tuple(__deserializer, #field_count, #visitor_expr)
        },
    };

    let visitor_var = if field_count == 0 {
        quote!(_)
    } else {
        quote!(mut __seq)
    };

    quote_block! {
        #[doc(hidden)]
        struct __Visitor #de_impl_generics #where_clause {
            marker: _serde::#private::PhantomData<#this_type #ty_generics>,
            lifetime: _serde::#private::PhantomData<&#delife ()>,
        }

        #[automatically_derived]
        impl #de_impl_generics _serde::de::Visitor<#delife> for __Visitor #de_ty_generics #where_clause {
            type Value = #this_type #ty_generics;

            fn expecting(&self, __formatter: &mut _serde::#private::Formatter) -> _serde::#private::fmt::Result {
                _serde::#private::Formatter::write_str(__formatter, #expecting)
            }

            #visit_newtype_struct

            #[inline]
            fn visit_seq<__A>(self, #visitor_var: __A) -> _serde::#private::Result<Self::Value, __A::Error>
            where
                __A: _serde::de::SeqAccess<#delife>,
            {
                #visit_seq
            }
        }

        #dispatch
    }
}

fn deserialize_newtype_struct(type_path: &Tokens, params: &Parameters, field: &Field) -> Tokens {
    let delife = params.borrowed.de_lifetime();
    let field_ty = field.ty;
    let deserializer_var = quote!(__e);

    let value = match field.attrs.deserialize_with() {
        None => {
            let span = field.original.span();
            let func = quote_spanned!(span=> <#field_ty as _serde::Deserialize>::deserialize);
            quote! {
                #func(#deserializer_var)?
            }
        }
        Some(path) => {
            quote_spanned! {path.span()=>
                #path(#deserializer_var)?
            }
        }
    };

    let mut result = quote!(#type_path(__field0));
    if params.has_getter {
        let this_type = &params.this_type;
        let (_, ty_generics, _) = params.generics.split_for_impl();
        result = quote! {
            _serde::#private::Into::<#this_type #ty_generics>::into(#result)
        };
    }

    quote! {
        #[inline]
        fn visit_newtype_struct<__E>(self, #deserializer_var: __E) -> _serde::#private::Result<Self::Value, __E::Error>
        where
            __E: _serde::Deserializer<#delife>,
        {
            let __field0: #field_ty = #value;
            _serde::#private::Ok(#result)
        }
    }
}
