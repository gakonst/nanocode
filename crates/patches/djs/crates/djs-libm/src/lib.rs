//! The math functions djs-core uses, copied from libm 0.2.16 (D3174): libm in pure Rust,
//! <https://github.com/rust-lang/compiler-builtins>, MIT (LICENSE.txt), itself a port of
//! musl's libm (and through it FreeBSD's msun), whose copyright notices stay in the files.
//!
//! The files under `math/` are libm's, unchanged except for what was cut: the functions
//! djs-core does not call (and their f32, f16 and f128 variants), libm's own tests, and the
//! items of cfgs that are off in libm's default build (`f16_enabled`, `f128_enabled`,
//! `assert_no_panic`, `unstable-public-internals`). The `arch` routines libm selects by
//! default stay (SSE2 and aarch64 `sqrt`), so the code and every result are libm 0.2.16's,
//! bit for bit; `tests/` compares the two. Updating means copying a newer libm's files the
//! same way, with a decision.
#![no_std]
// libm's own allow-list (its lib.rs), plus `explicit_counter_loop`, which newer clippy
// finds in rem_pio2_large: the copied code keeps its style.
#![allow(clippy::assign_op_pattern)]
#![allow(clippy::deprecated_cfg_attr)]
#![allow(clippy::eq_op)]
#![allow(clippy::excessive_precision)]
#![allow(clippy::explicit_counter_loop)]
#![allow(clippy::float_cmp)]
#![allow(clippy::int_plus_one)]
#![allow(clippy::just_underscores_and_digits)]
#![allow(clippy::many_single_char_names)]
#![allow(clippy::mixed_case_hex_literals)]
#![allow(clippy::needless_late_init)]
#![allow(clippy::needless_return)]
#![allow(clippy::unreadable_literal)]
#![allow(clippy::zero_divided_by_zero)]
#![forbid(unsafe_op_in_unsafe_fn)]
// libm's `Float` and `Int` traits have methods and constants of the names std may add
// (`BITS`, `SIGN_MASK`, `widen`, ...); within libm's crate the lint is the same.
#![allow(unstable_name_collisions)]

mod math;

pub use self::math::*;
