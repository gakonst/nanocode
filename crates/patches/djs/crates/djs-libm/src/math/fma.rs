/* SPDX-License-Identifier: MIT */
/* origin: musl src/math/fma.c, fmaf.c Ported to generic Rust algorithm in 2025, TG. */

use super::generic;
use crate::support::Round;

/// Fused multiply add (f64)
///
/// Computes `(x*y)+z`, rounded as one ternary operation (i.e. calculated with infinite precision).
pub fn fma(x: f64, y: f64, z: f64) -> f64 {
    select_implementation! {
        name: fma,
        use_arch: any(
            all(target_arch = "aarch64", target_feature = "neon"),
            target_feature = "sse2",
        ),
        args: x, y, z,
    }

    generic::fma_round(x, y, z, Round::Nearest).val
}
