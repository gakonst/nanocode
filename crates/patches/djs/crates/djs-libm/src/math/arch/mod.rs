//! Architecture-specific routines and operations.
//!
//! LLVM will already optimize calls to some of these in cases that there are hardware
//! instructions. Providing an implementation here just ensures that the faster implementation
//! is used when calling the function directly. This helps anyone who uses `libm` directly, as
//! well as improving things when these routines are called as part of other implementations.

// Most implementations should be defined here, to ensure they are not made available when
// soft floats are required.
// djs-libm: libm's default `arch` feature is always on (no `force-soft-floats`), so this is
// not gated on `arch_enabled`. Of the routines libm selects by default only `sqrt` and
// `fma` (cbrt) are reachable from the functions kept here. The wasm32 routines need libm's
// `unstable-intrinsics` (off by default, so wasm uses the generic code) and the i586 ones
// (x86 without SSE2) a target djs does not build for: both are left out.
cfg_if! {
    if #[cfg(target_feature = "sse2")] {
        mod x86;
        pub use x86::{sqrt, fma};
    } else if #[cfg(all(
        any(target_arch = "aarch64", target_arch = "arm64ec"),
        target_feature = "neon"
    ))] {
        mod aarch64;

        pub use aarch64::{fma, sqrt};
    }
}
