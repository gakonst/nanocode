#![recursion_limit = "128"]
#![doc(html_logo_url = "https://raw.githubusercontent.com/RustPython/RustPython/main/logo.png")]
#![doc(html_root_url = "https://docs.rs/rustpython-derive/")]

extern crate alloc;
extern crate proc_macro;

#[macro_use]
mod error;
#[macro_use]
mod util;
pub mod syn_ext;

mod class_docs;
mod compile_bytecode;
mod from_args;
mod pyclass;
mod pymodule;
mod pypayload;
mod pystructseq;
mod pytraverse;

use crate::syn_ext::types::PunctuatedNestedMeta;
use error::Diagnostic;
use nanocode_macro_support::ToTokens;
use nanocode_macro_support::Tokens;
use nanocode_macro_support::{DeriveInput, Item};

pub use pymodule::PyModuleArgs;

pub use compile_bytecode::Compiler;

fn result_to_tokens(result: Result<Tokens, impl Into<Diagnostic>>) -> Tokens {
    result
        .map_err(|e| e.into())
        .unwrap_or_else(ToTokens::into_token_stream)
}

#[must_use]
pub fn derive_from_args(input: DeriveInput) -> Tokens {
    result_to_tokens(from_args::impl_from_args(input))
}

#[must_use]
pub fn pyclass(attr: PunctuatedNestedMeta, item: Item) -> Tokens {
    if matches!(
        item,
        nanocode_macro_support::Item::Impl(_) | nanocode_macro_support::Item::Trait(_)
    ) {
        result_to_tokens(pyclass::impl_pyclass_impl(attr, item))
    } else {
        result_to_tokens(pyclass::impl_pyclass(attr, item))
    }
}

#[must_use]
pub fn pyexception(attr: PunctuatedNestedMeta, item: Item) -> Tokens {
    if matches!(item, nanocode_macro_support::Item::Impl(_)) {
        pyclass::impl_pyexception_impl(attr, item)
    } else {
        result_to_tokens(pyclass::impl_pyexception(attr, &item))
    }
}

#[must_use]
pub fn pymodule(attr: PyModuleArgs, item: Item) -> Tokens {
    result_to_tokens(pymodule::impl_pymodule(attr, item))
}

#[must_use]
pub fn pystruct_sequence(attr: PunctuatedNestedMeta, item: Item) -> Tokens {
    result_to_tokens(pystructseq::impl_pystruct_sequence(attr, item))
}

#[must_use]
pub fn pystruct_sequence_data(attr: PunctuatedNestedMeta, item: Item) -> Tokens {
    result_to_tokens(pystructseq::impl_pystruct_sequence_data(attr, item))
}

pub fn py_compile(input: Tokens, compiler: &dyn Compiler) -> Tokens {
    result_to_tokens(compile_bytecode::impl_py_compile(input, compiler))
}

pub fn py_freeze(input: Tokens, compiler: &dyn Compiler) -> Tokens {
    result_to_tokens(compile_bytecode::impl_py_freeze(input, compiler))
}

#[must_use]
pub fn pypayload(input: DeriveInput) -> Tokens {
    pypayload::impl_pypayload(input)
}

#[must_use]
pub fn pytraverse(item: DeriveInput) -> Tokens {
    result_to_tokens(pytraverse::impl_pytraverse(item))
}
