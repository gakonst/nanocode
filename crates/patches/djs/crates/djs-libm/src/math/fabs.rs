/// Absolute value (magnitude) (f64)
///
/// Calculates the absolute value (magnitude) of the argument `x`,
/// by direct manipulation of the bit representation of `x`.
pub fn fabs(x: f64) -> f64 {
    select_implementation! {
        name: fabs,
        use_arch: all(target_arch = "wasm32", intrinsics_enabled),
        args: x,
    }

    super::generic::fabs(x)
}
