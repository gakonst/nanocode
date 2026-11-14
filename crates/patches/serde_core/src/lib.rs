#![doc(html_root_url = "https://docs.rs/serde_core/1.0.228")]
#![allow(unknown_lints, bare_trait_objects, deprecated)]
#![allow(
    clippy::unnested_or_patterns,
    clippy::semicolon_if_nothing_returned,
    clippy::empty_enum,
    clippy::type_repetition_in_bounds, // https://github.com/rust-lang/rust-clippy/issues/8772
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::module_name_repetitions,
    clippy::single_match_else,
    clippy::type_complexity,
    clippy::use_self,
    clippy::zero_prefixed_literal,
    clippy::derive_partial_eq_without_eq,
    clippy::enum_glob_use,
    clippy::explicit_auto_deref,
    clippy::incompatible_msrv,
    clippy::let_underscore_untyped,
    clippy::map_err_ignore,
    clippy::new_without_default,
    clippy::result_unit_err,
    clippy::wildcard_imports,
    clippy::needless_pass_by_value,
    clippy::similar_names,
    clippy::too_many_lines,
    clippy::doc_markdown,
    clippy::elidable_lifetime_names,
    clippy::needless_lifetimes,
    clippy::unseparated_literal_suffix,
    clippy::needless_doctest_main,
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
)]
#![deny(clippy::question_mark_used)]

#[macro_use]
mod crate_root;
#[macro_use]
mod macros;

crate_root!();

#[macro_export]
#[doc(hidden)]
macro_rules! __require_serde_not_serde_core {
    () => {
        ::core::compile_error!(
            "Serde derive requires a dependency on the serde crate, not serde_core"
        );
    };
}
