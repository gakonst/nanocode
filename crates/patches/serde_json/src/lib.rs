#![doc(html_root_url = "https://docs.rs/serde_json/1.0.150")]
#![allow(
    clippy::collapsible_else_if,
    clippy::comparison_chain,
    clippy::deprecated_cfg_attr,
    clippy::doc_markdown,
    clippy::elidable_lifetime_names,
    clippy::excessive_precision,
    clippy::explicit_auto_deref,
    clippy::float_cmp,
    clippy::manual_range_contains,
    clippy::match_like_matches_macro,
    clippy::match_single_binding,
    clippy::needless_doctest_main,
    clippy::needless_late_init,
    clippy::needless_lifetimes,
    clippy::return_self_not_must_use,
    clippy::transmute_ptr_to_ptr,
    clippy::unbuffered_bytes,
    clippy::unconditional_recursion, // https://github.com/rust-lang/rust-clippy/issues/12133
    clippy::uninlined_format_args,
    clippy::unnecessary_wraps
)]
#![allow(
    clippy::should_implement_trait,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::enum_glob_use,
    clippy::if_not_else,
    clippy::integer_division,
    clippy::let_underscore_untyped,
    clippy::map_err_ignore,
    clippy::match_same_arms,
    clippy::similar_names,
    clippy::unused_self,
    clippy::wildcard_imports,
    clippy::cast_lossless,
    clippy::items_after_statements,
    clippy::module_name_repetitions,
    clippy::redundant_else,
    clippy::shadow_unrelated,
    clippy::single_match_else,
    clippy::too_many_lines,
    clippy::unreadable_literal,
    clippy::unseparated_literal_suffix,
    clippy::use_self,
    clippy::zero_prefixed_literal,
    clippy::checked_conversions,
    clippy::mem_replace_with_default,
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
)]
#![deny(clippy::question_mark_used)]
#![allow(non_upper_case_globals)]
#![no_std]
#![allow(unknown_lints, mismatched_lifetime_syntaxes)]

extern crate alloc;

extern crate std;

extern crate serde_core as serde;

#[doc(hidden)]
pub mod __private {
    #[doc(hidden)]
    pub use alloc::vec;
}

#[doc(inline)]
pub use crate::de::from_reader;
#[doc(inline)]
pub use crate::de::{from_slice, from_str, Deserializer, StreamDeserializer};
#[doc(inline)]
pub use crate::error::{Error, Result};
#[doc(inline)]
pub use crate::ser::{to_string, to_string_pretty, to_vec, to_vec_pretty};
#[doc(inline)]
pub use crate::ser::{to_writer, to_writer_pretty, Serializer};
#[doc(inline)]
pub use crate::value::{from_value, to_value, Map, Number, Value};

macro_rules! tri {
    ($e:expr $(,)?) => {
        match $e {
            core::result::Result::Ok(val) => val,
            core::result::Result::Err(err) => return core::result::Result::Err(err),
        }
    };
}

#[macro_use]
mod macros;

pub mod de;
pub mod error;
pub mod map;
pub mod ser;
pub mod value;

mod io;
mod iter;
mod number;
mod float;
mod read;

mod raw;
