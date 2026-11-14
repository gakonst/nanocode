use nanocode_macro_support as syntax;
use nanocode_macro_support::Tokens;
use nanocode_macro_support::quote;

pub fn allow_deprecated(input: &syntax::DeriveInput) -> Option<Tokens> {
    if should_allow_deprecated(input) {
        Some(quote! { #[allow(deprecated)] })
    } else {
        None
    }
}

fn should_allow_deprecated(input: &syntax::DeriveInput) -> bool {
    if contains_deprecated(&input.attrs) {
        return true;
    }
    if let syntax::Data::Enum(data_enum) = &input.data {
        for variant in &data_enum.variants {
            if contains_deprecated(&variant.attrs) {
                return true;
            }
        }
    }
    false
}

fn contains_deprecated(attrs: &[syntax::Attribute]) -> bool {
    for attr in attrs {
        if attr.path().is_ident("deprecated") {
            return true;
        }
        if let syntax::Meta::List(meta_list) = &attr.meta
            && meta_list.path.is_ident("allow")
        {
            let mut allow_deprecated = false;
            let _ = meta_list.parse_nested_meta(|meta| {
                if meta.path.is_ident("deprecated") {
                    allow_deprecated = true;
                }
                Ok(())
            });
            if allow_deprecated {
                return true;
            }
        }
    }
    false
}
