//! Leaf intrinsics of the builtins (ARCH.md "The leaf rule"), one module per area.
//!
//! The builtins themselves are in `builtins/*.ts` (the builtin language): they do the
//! conversions and checks that may run user code, then call these leaf intrinsics,
//! which work on the heap and never run JS. `interp/run/intrinsics.rs` dispatches
//! `call_intrinsic` to them.

pub(crate) mod array;
pub(crate) mod collection;
pub(crate) mod date;
pub(crate) mod dense;
pub(crate) mod dom;
pub(crate) mod encoding;
pub(crate) mod eval;
pub(crate) mod fast;
pub(crate) mod function;
pub(crate) mod r#gen;
pub(crate) mod json;
pub(crate) mod math;
pub(crate) mod number;
pub(crate) mod object;
pub(crate) mod regexp;
pub(crate) mod string;
pub(crate) mod typed;
pub(crate) mod uri;
