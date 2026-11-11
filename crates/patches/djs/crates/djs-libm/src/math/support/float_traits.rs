#![allow(unknown_lints)] // FIXME(msrv) we shouldn't need this

use core::{fmt, mem, ops};

use super::int_traits::{CastFrom, Int, MinInt};

/// Trait for some basic operations on floats
// #[allow(dead_code)]
#[allow(dead_code)] // Some constants are only used with tests
pub trait Float:
    Copy
    + fmt::Debug
    + PartialEq
    + PartialOrd
    + ops::AddAssign
    + ops::MulAssign
    + ops::Add<Output = Self>
    + ops::Sub<Output = Self>
    + ops::Mul<Output = Self>
    + ops::Div<Output = Self>
    + ops::Rem<Output = Self>
    + ops::Neg<Output = Self>
    + 'static
{
    /// A uint of the same width as the float
    type Int: Int<OtherSign = Self::SignedInt, Unsigned = Self::Int>;

    /// A int of the same width as the float
    type SignedInt: Int
        + MinInt<OtherSign = Self::Int, Unsigned = Self::Int>
        + ops::Neg<Output = Self::SignedInt>;

    const ZERO: Self;
    const NEG_ZERO: Self;
    const ONE: Self;
    const NEG_ONE: Self;
    const INFINITY: Self;
    const NEG_INFINITY: Self;
    const NAN: Self;
    const NEG_NAN: Self;
    const MAX: Self;
    const MIN: Self;
    const EPSILON: Self;
    const PI: Self;
    const NEG_PI: Self;
    const FRAC_PI_2: Self;

    const MIN_POSITIVE_NORMAL: Self;

    /// The bitwidth of the float type
    const BITS: u32;

    /// The bitwidth of the significand
    const SIG_BITS: u32;

    /// The bitwidth of the exponent
    const EXP_BITS: u32 = Self::BITS - Self::SIG_BITS - 1;

    /// The saturated (maximum bitpattern) value of the exponent, i.e. the infinite
    /// representation.
    ///
    /// This shifted fully right, use `EXP_MASK` for the shifted value.
    const EXP_SAT: u32 = (1 << Self::EXP_BITS) - 1;

    /// The exponent bias value
    const EXP_BIAS: u32 = Self::EXP_SAT >> 1;

    /// Maximum unbiased exponent value.
    const EXP_MAX: i32 = Self::EXP_BIAS as i32;

    /// Minimum *NORMAL* unbiased exponent value.
    const EXP_MIN: i32 = -(Self::EXP_MAX - 1);

    /// Minimum subnormal exponent value.
    const EXP_MIN_SUBNORM: i32 = Self::EXP_MIN - Self::SIG_BITS as i32;

    /// A mask for the sign bit
    const SIGN_MASK: Self::Int;

    /// A mask for the significand
    const SIG_MASK: Self::Int;

    /// A mask for the exponent
    const EXP_MASK: Self::Int;

    /// The implicit bit of the float format
    const IMPLICIT_BIT: Self::Int;

    /// Returns `self` transmuted to `Self::Int`
    fn to_bits(self) -> Self::Int;

    /// Returns `self` transmuted to `Self::SignedInt`
    #[allow(dead_code)]
    fn to_bits_signed(self) -> Self::SignedInt {
        self.to_bits().signed()
    }

    /// Check bitwise equality.
    #[allow(dead_code)]
    fn biteq(self, rhs: Self) -> bool {
        self.to_bits() == rhs.to_bits()
    }

    /// Checks if two floats have the same bit representation. *Except* for NaNs! NaN can be
    /// represented in multiple different ways.
    ///
    /// This method returns `true` if two NaNs are compared. Use [`biteq`](Self::biteq) instead
    /// if `NaN` should not be treated separately.
    #[allow(dead_code)]
    fn eq_repr(self, rhs: Self) -> bool {
        if self.is_nan() && rhs.is_nan() {
            true
        } else {
            self.biteq(rhs)
        }
    }

    /// Returns true if the value is NaN.
    fn is_nan(self) -> bool;

    /// Returns true if the value is +inf or -inf.
    fn is_infinite(self) -> bool;

    /// Returns true if the sign is negative. Extracts the sign bit regardless of zero or NaN.
    fn is_sign_negative(self) -> bool;

    /// Returns true if the sign is positive. Extracts the sign bit regardless of zero or NaN.
    fn is_sign_positive(self) -> bool {
        !self.is_sign_negative()
    }

    /// Returns if `self` is subnormal.
    #[allow(dead_code)]
    fn is_subnormal(self) -> bool {
        (self.to_bits() & Self::EXP_MASK) == Self::Int::ZERO
    }

    /// Returns the exponent, not adjusting for bias, not accounting for subnormals or zero.
    fn ex(self) -> u32 {
        u32::cast_from(self.to_bits() >> Self::SIG_BITS) & Self::EXP_SAT
    }

    /// Extract the exponent and adjust it for bias, not accounting for subnormals or zero.
    fn exp_unbiased(self) -> i32 {
        self.ex().signed() - (Self::EXP_BIAS as i32)
    }

    /// Returns the significand with no implicit bit (or the "fractional" part)
    #[allow(dead_code)]
    fn frac(self) -> Self::Int {
        self.to_bits() & Self::SIG_MASK
    }

    /// Returns a `Self::Int` transmuted back to `Self`
    fn from_bits(a: Self::Int) -> Self;

    /// Constructs a `Self` from its parts. Inputs are treated as bits and shifted into position.
    fn from_parts(negative: bool, exponent: u32, significand: Self::Int) -> Self {
        let sign = if negative {
            Self::Int::ONE
        } else {
            Self::Int::ZERO
        };
        Self::from_bits(
            (sign << (Self::BITS - 1))
                | (Self::Int::cast_from(exponent & Self::EXP_SAT) << Self::SIG_BITS)
                | (significand & Self::SIG_MASK),
        )
    }

    #[allow(dead_code)]
    fn abs(self) -> Self;

    /// Returns a number composed of the magnitude of self and the sign of sign.
    fn copysign(self, other: Self) -> Self;

    /// Fused multiply add, rounding once.
    fn fma(self, y: Self, z: Self) -> Self;

    /// Returns (normalized exponent, normalized significand)
    #[allow(dead_code)]
    fn normalize(significand: Self::Int) -> (i32, Self::Int);

    /// Returns a number that represents the sign of self.
    #[allow(dead_code)]
    fn signum(self) -> Self {
        if self.is_nan() {
            self
        } else {
            Self::ONE.copysign(self)
        }
    }

    /// Make a best-effort attempt to canonicalize the number. Note that this is allowed
    /// to be a nop and does not always quiet sNaNs.
    fn canonicalize(self) -> Self {
        // FIXME: LLVM often removes this. We should determine whether we can remove the operation,
        // or switch to something based on `llvm.canonicalize` (which has crashes,
        // <https://github.com/llvm/llvm-project/issues/32650>).
        self * Self::ONE
    }
}

/// Access the associated `Int` type from a float (helper to avoid ambiguous associated types).
pub type IntTy<F> = <F as Float>::Int;

macro_rules! float_impl {
    (
        $ty:ident,
        $ity:ident,
        $sity:ident,
        $bits:expr,
        $significand_bits:expr,
        $from_bits:path,
        $to_bits:path,
        $fma_fn:ident,
        $fma_intrinsic:ident
    ) => {
        impl Float for $ty {
            type Int = $ity;
            type SignedInt = $sity;

            const ZERO: Self = 0.0;
            const NEG_ZERO: Self = -0.0;
            const ONE: Self = 1.0;
            const NEG_ONE: Self = -1.0;
            const INFINITY: Self = Self::INFINITY;
            const NEG_INFINITY: Self = Self::NEG_INFINITY;
            const NAN: Self = Self::NAN;
            // NAN isn't guaranteed to be positive but it usually is. We only use this for
            // tests.
            const NEG_NAN: Self = $from_bits($to_bits(Self::NAN) | Self::SIGN_MASK);
            const MAX: Self = -Self::MIN;
            // Sign bit set, saturated mantissa, saturated exponent with last bit zeroed
            const MIN: Self = $from_bits(Self::Int::MAX & !(1 << Self::SIG_BITS));
            const EPSILON: Self = <$ty>::EPSILON;

            // Exponent is a 1 in the LSB
            const MIN_POSITIVE_NORMAL: Self = $from_bits(1 << Self::SIG_BITS);

            const PI: Self = core::$ty::consts::PI;
            const NEG_PI: Self = -Self::PI;
            const FRAC_PI_2: Self = core::$ty::consts::FRAC_PI_2;

            const BITS: u32 = $bits;
            const SIG_BITS: u32 = $significand_bits;

            const SIGN_MASK: Self::Int = 1 << (Self::BITS - 1);
            const SIG_MASK: Self::Int = (1 << Self::SIG_BITS) - 1;
            const EXP_MASK: Self::Int = !(Self::SIGN_MASK | Self::SIG_MASK);
            const IMPLICIT_BIT: Self::Int = 1 << Self::SIG_BITS;

            fn to_bits(self) -> Self::Int {
                self.to_bits()
            }
            fn is_nan(self) -> bool {
                self.is_nan()
            }
            fn is_infinite(self) -> bool {
                self.is_infinite()
            }
            fn is_sign_negative(self) -> bool {
                self.is_sign_negative()
            }
            fn from_bits(a: Self::Int) -> Self {
                Self::from_bits(a)
            }
            fn abs(self) -> Self {
                cfg_if! {
                    // FIXME(msrv): `abs` is available in `core` starting with 1.85.
                    if #[cfg(intrinsics_enabled)] {
                        self.abs()
                    } else {
                        super::super::generic::fabs(self)
                    }
                }
            }
            fn copysign(self, other: Self) -> Self {
                cfg_if! {
                    // FIXME(msrv): `copysign` is available in `core` starting with 1.85.
                    if #[cfg(intrinsics_enabled)] {
                        self.copysign(other)
                    } else {
                        super::super::generic::copysign(self, other)
                    }
                }
            }
            fn fma(self, y: Self, z: Self) -> Self {
                cfg_if! {
                    // fma is not yet available in `core`
                    if #[cfg(intrinsics_enabled)] {
                        // FIXME(msrv,bench): once our benchmark rustc version is above the
                        // 2022-09-23 nightly, this can be removed.
                        #[allow(unused_unsafe)]
                        unsafe { core::intrinsics::$fma_intrinsic(self, y, z) }
                    } else {
                        super::super::$fma_fn(self, y, z)
                    }
                }
            }
            fn normalize(significand: Self::Int) -> (i32, Self::Int) {
                let shift = significand.leading_zeros().wrapping_sub(Self::EXP_BITS);
                (
                    1i32.wrapping_sub(shift as i32),
                    significand << shift as Self::Int,
                )
            }
        }
    };
}

float_impl!(
    f64,
    u64,
    i64,
    64,
    52,
    f64_from_bits,
    f64_to_bits,
    fma,
    fmaf64
);

/* FIXME(msrv): vendor some things that are not const stable at our MSRV */

/// `f64::from_bits`
#[allow(unnecessary_transmutes)] // lint appears in newer versions of Rust
pub const fn f64_from_bits(bits: u64) -> f64 {
    // SAFETY: POD cast with no preconditions
    unsafe { mem::transmute::<u64, f64>(bits) }
}

/// `f64::to_bits`
#[allow(dead_code)] // workaround for false positive RUST-144060
#[allow(unnecessary_transmutes)] // lint appears in newer versions of Rust
pub const fn f64_to_bits(x: f64) -> u64 {
    // SAFETY: POD cast with no preconditions
    unsafe { mem::transmute::<f64, u64>(x) }
}
