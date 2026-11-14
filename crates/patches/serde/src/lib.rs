#![doc(html_root_url = "https://docs.rs/serde/1.0.228")]
#![allow(
    unknown_lints,
    bare_trait_objects,
    deprecated,
    mismatched_lifetime_syntaxes
)]
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

macro_rules! crate_root {
    () => {
        mod lib {
            mod core {
                pub use std::*;
            }

            pub use self::core::{f32, f64};
            pub use self::core::{ptr, str};

            pub use self::core::slice;

            pub use self::core::clone;
            pub use self::core::convert;
            pub use self::core::default;
            pub use self::core::fmt::{self, Debug, Display, Write as FmtWrite};
            pub use self::core::marker::{self, PhantomData};
            pub use self::core::option;
            pub use self::core::result;

            pub use std::borrow::{Cow, ToOwned};

            pub use std::string::{String, ToString};

            pub use std::vec::Vec;

            pub use std::boxed::Box;
        }

        macro_rules! tri {
            ($expr:expr) => {
                match $expr {
                    Ok(val) => val,
                    Err(err) => return Err(err),
                }
            };
        }

        pub use serde_core::{
            de, forward_to_deserialize_any, ser, Deserialize, Deserializer, Serialize, Serializer,
        };

        #[doc(hidden)]
        mod private;

        #[doc(hidden)]
        pub mod __private228 {
            #[doc(hidden)]
            pub use crate::private::*;
        }
        use serde_core::__private228 as serde_core_private;
    };
}

crate_root!();

mod integer128;

#[cfg(feature = "serde_derive")]
extern crate serde_derive;

#[cfg(feature = "serde_derive")]
pub use serde_derive::{Deserialize, Serialize};

#[macro_export]
#[doc(hidden)]
macro_rules! __require_serde_not_serde_core {
    () => {};
}
