/* SPDX-License-Identifier: MIT OR Apache-2.0 */
use crate::support::{CastInto, DInt, HInt, MinInt};

/// Trait for unsigned division of a double-wide integer
/// when the quotient doesn't overflow.
///
/// This is the inverse of widening multiplication:
///  - for any `x` and nonzero `y`: `x.widen_mul(y).checked_narrowing_div_rem(y) == Some((x, 0))`,
///  - and for any `r in 0..y`: `x.carrying_mul(y, r).checked_narrowing_div_rem(y) == Some((x, r))`,
pub trait NarrowingDiv: DInt + MinInt<Unsigned = Self> {
    /// Computes `(self / n, self % n))`
    ///
    /// # Safety
    /// The caller must ensure that `self.hi() < n`, or equivalently,
    /// that the quotient does not overflow.
    unsafe fn unchecked_narrowing_div_rem(self, n: Self::H) -> (Self::H, Self::H);

    /// Returns `Some((self / n, self % n))` when `self.hi() < n`.
    fn checked_narrowing_div_rem(self, n: Self::H) -> Option<(Self::H, Self::H)> {
        if self.hi() < n {
            Some(unsafe { self.unchecked_narrowing_div_rem(n) })
        } else {
            None
        }
    }
}

// For primitive types we can just use the standard
// division operators in the double-wide type.
macro_rules! impl_narrowing_div_primitive {
    ($D:ident) => {
        impl NarrowingDiv for $D {
            unsafe fn unchecked_narrowing_div_rem(self, n: Self::H) -> (Self::H, Self::H) {
                if self.hi() >= n {
                    unsafe { core::hint::unreachable_unchecked() }
                }
                ((self / n.widen()).cast(), (self % n.widen()).cast())
            }
        }
    };
}

impl_narrowing_div_primitive!(u16);
impl_narrowing_div_primitive!(u32);
impl_narrowing_div_primitive!(u64);
impl_narrowing_div_primitive!(u128);
