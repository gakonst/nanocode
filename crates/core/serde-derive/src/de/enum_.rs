use crate::de::enum_adjacently;
use crate::de::enum_externally;
use crate::de::enum_internally;
use crate::de::enum_untagged;
use crate::de::identifier;
use crate::de::{FieldWithAliases, Parameters, field_i};
use crate::fragment::{Expr, Fragment, Stmts};
use crate::internals::ast::Variant;
use crate::internals::attr;
use crate::private;
use nanocode_macro_support::Tokens;
use nanocode_macro_support::quote;

pub(super) fn deserialize(
    params: &Parameters,
    variants: &[Variant],
    cattrs: &attr::Container,
) -> Fragment {
    match variants.iter().position(|var| var.attrs.untagged()) {
        Some(variant_idx) => {
            let (tagged, untagged) = variants.split_at(variant_idx);
            let tagged_frag = Expr(deserialize_homogeneous_enum(params, tagged, cattrs));

            let first_attempt = quote! {
                if let _serde::#private::Result::<_, __D::Error>::Ok(__ok) = (|| #tagged_frag)() {
                    return _serde::#private::Ok(__ok);
                }
            };
            enum_untagged::deserialize(params, untagged, cattrs, Some(first_attempt))
        }
        None => deserialize_homogeneous_enum(params, variants, cattrs),
    }
}

fn deserialize_homogeneous_enum(
    params: &Parameters,
    variants: &[Variant],
    cattrs: &attr::Container,
) -> Fragment {
    match cattrs.tag() {
        attr::TagType::External => enum_externally::deserialize(params, variants, cattrs),
        attr::TagType::Internal { tag } => {
            enum_internally::deserialize(params, variants, cattrs, tag)
        }
        attr::TagType::Adjacent { tag, content } => {
            enum_adjacently::deserialize(params, variants, cattrs, tag, content)
        }
        attr::TagType::None => enum_untagged::deserialize(params, variants, cattrs, None),
    }
}

pub fn prepare_enum_variant_enum(variants: &[Variant]) -> (Tokens, Stmts) {
    let deserialized_variants = variants
        .iter()
        .enumerate()
        .filter(|&(_i, variant)| !variant.attrs.skip_deserializing());

    let fallthrough = deserialized_variants
        .clone()
        .find(|(_i, variant)| variant.attrs.other())
        .map(|(i, _variant)| {
            let ignore_variant = field_i(i);
            quote!(_serde::#private::Ok(__Field::#ignore_variant))
        });

    let variants_stmt = {
        let variant_names = deserialized_variants
            .clone()
            .flat_map(|(_i, variant)| variant.attrs.aliases());
        quote! {
            #[doc(hidden)]
            const VARIANTS: &'static [&'static str] = &[ #(#variant_names),* ];
        }
    };

    let deserialized_variants: Vec<_> = deserialized_variants
        .map(|(i, variant)| FieldWithAliases {
            ident: field_i(i),
            aliases: variant.attrs.aliases(),
        })
        .collect();

    let variant_visitor = Stmts(identifier::deserialize_generated(
        &deserialized_variants,
        false,
        true,
        None,
        fallthrough,
    ));

    (variants_stmt, variant_visitor)
}
