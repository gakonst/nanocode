#[macro_use]
pub mod macros;
mod env;
// Runtime feature detection requires atomics.
#[cfg(target_has_atomic = "ptr")]
pub(crate) mod feature_detect;
mod float_traits;
pub mod hex_float;
mod int_traits;
mod modular;

// Clippy seems to have a false positive
#[allow(unused_imports, clippy::single_component_path_imports)]
pub(crate) use cfg_if;
pub use env::{FpResult, Round, Status};
pub(crate) use float_traits::f64_from_bits;
#[allow(unused_imports)]
pub use float_traits::{Float, IntTy};
#[allow(unused_imports)]
pub use hex_float::hf64;
pub use int_traits::{CastFrom, CastInto, DInt, HInt, Int, MinInt, NarrowingDiv};
pub use modular::linear_mul_reduction;

/// Hint to the compiler that the current path is cold.
pub fn cold_path() {
    #[cfg(intrinsics_enabled)]
    core::intrinsics::cold_path();
}
