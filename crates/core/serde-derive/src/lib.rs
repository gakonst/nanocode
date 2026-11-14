extern crate proc_macro;

mod internals;

use nanocode_macro_support::{DeriveInput, Ident, Span, ToTokens, Tokens, parse_stream};
use proc_macro::TokenStream;

#[macro_use]
mod bound;
#[macro_use]
mod fragment;

mod de;
mod deprecated;
mod dummy;
mod pretend;
mod ser;
mod this;

#[allow(non_camel_case_types)]
struct private;

impl private {
    fn ident(&self) -> Ident {
        Ident::new(
            concat!("__private", env!("CARGO_PKG_VERSION_PATCH")),
            Span::call_site(),
        )
    }
}

impl ToTokens for private {
    fn to_tokens(&self, tokens: &mut Tokens) {
        self.ident().to_tokens(tokens);
    }
}

#[proc_macro_derive(Serialize, attributes(serde))]
pub fn derive_serialize(input: TokenStream) -> TokenStream {
    let mut input: DeriveInput = match parse_stream(input) {
        Ok(input) => input,
        Err(error) => return error.into_compile_error().into_stream(),
    };
    ser::expand_derive_serialize(&mut input)
        .unwrap_or_else(nanocode_macro_support::Error::into_compile_error)
        .into_stream()
}

#[proc_macro_derive(Deserialize, attributes(serde))]
pub fn derive_deserialize(input: TokenStream) -> TokenStream {
    let mut input: DeriveInput = match parse_stream(input) {
        Ok(input) => input,
        Err(error) => return error.into_compile_error().into_stream(),
    };
    de::expand_derive_deserialize(&mut input)
        .unwrap_or_else(nanocode_macro_support::Error::into_compile_error)
        .into_stream()
}
