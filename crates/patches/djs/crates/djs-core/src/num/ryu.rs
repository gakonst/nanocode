//! Shortest round-trip decimal digits for an `f64` (Ryu, Ulf Adams, PLDI 2018).
//!
//! Ported from the `ryu` crate's `d2s.rs` (Apache-2.0 OR BSL-1.0, itself translated from
//! <https://github.com/ulfjack/ryu>). The algorithm is unchanged; the power tables come from
//! `pow5` instead of the full 10 KB table.
//!
//! The result is the shortest digit string that rounds back to the input, and among those the
//! one closest to the exact value (ties to even), which is what ECMAScript's Number::toString
//! requires.

use super::pow5::{ryu_inv_pow5, ryu_pow5};

const MANTISSA_BITS: u32 = 52;
const BIAS: i32 = 1023;
const POW5_INV_BITCOUNT: i32 = 125;
const POW5_BITCOUNT: i32 = 125;

/// `mantissa * 10^exponent`, with `mantissa` free of trailing zeros only where Ryu removed
/// them (callers strip any that remain).
pub(crate) struct Decimal {
    pub(crate) mantissa: u64,
    pub(crate) exponent: i32,
}

fn pow5bits(e: i32) -> i32 {
    (((e as u32) * 1217359) >> 19) as i32 + 1
}

fn log10_pow2(e: i32) -> u32 {
    ((e as u32) * 78913) >> 18
}

fn log10_pow5(e: i32) -> u32 {
    ((e as u32) * 732923) >> 20
}

fn pow5_factor(mut value: u64) -> u32 {
    const M_INV_5: u64 = 14757395258967641293; // 5 * M_INV_5 == 1 (mod 2^64)
    const N_DIV_5: u64 = 3689348814741910323; // 2^64 / 5
    let mut count = 0;
    loop {
        value = value.wrapping_mul(M_INV_5);
        if value > N_DIV_5 {
            return count;
        }
        count += 1;
    }
}

fn multiple_of_power_of_5(value: u64, p: u32) -> bool {
    pow5_factor(value) >= p
}

fn multiple_of_power_of_2(value: u64, p: u32) -> bool {
    value & ((1u64 << p) - 1) == 0
}

fn mul_shift_64(m: u64, mul: u128, j: u32) -> u64 {
    let b0 = m as u128 * (mul as u64) as u128;
    let b2 = m as u128 * (mul >> 64);
    (((b0 >> 64) + b2) >> (j - 64)) as u64
}

/// Shortest decimal for a finite, non-zero double given by its raw mantissa and biased
/// exponent fields.
pub(crate) fn d2d(ieee_mantissa: u64, ieee_exponent: u32) -> Decimal {
    let (e2, m2) = if ieee_exponent == 0 {
        // Subtract 2 so the bounds computation has 2 additional bits.
        (1 - BIAS - MANTISSA_BITS as i32 - 2, ieee_mantissa)
    } else {
        (
            ieee_exponent as i32 - BIAS - MANTISSA_BITS as i32 - 2,
            (1u64 << MANTISSA_BITS) | ieee_mantissa,
        )
    };
    let even = m2 & 1 == 0;
    let accept_bounds = even;

    // Step 2: the interval of valid decimal representations.
    let mv = 4 * m2;
    let mm_shift = (ieee_mantissa != 0 || ieee_exponent <= 1) as u32;
    // mp = 4 * m2 + 2, mm = mv - 1 - mm_shift.

    // Step 3: convert to a decimal power base using 128-bit arithmetic.
    let mut vr: u64;
    let mut vp: u64;
    let mut vm: u64;
    let e10: i32;
    let mut vm_is_trailing_zeros = false;
    let mut vr_is_trailing_zeros = false;
    if e2 >= 0 {
        let q = log10_pow2(e2) - (e2 > 3) as u32;
        e10 = q as i32;
        let k = POW5_INV_BITCOUNT + pow5bits(q as i32) - 1;
        let i = -e2 + q as i32 + k;
        let mul = ryu_inv_pow5(q);
        vr = mul_shift_64(4 * m2, mul, i as u32);
        vp = mul_shift_64(4 * m2 + 2, mul, i as u32);
        vm = mul_shift_64(4 * m2 - 1 - mm_shift as u64, mul, i as u32);
        if q <= 21 {
            // Only one of mp, mv and mm can be a multiple of 5, if any.
            if mv.is_multiple_of(5) {
                vr_is_trailing_zeros = multiple_of_power_of_5(mv, q);
            } else if accept_bounds {
                vm_is_trailing_zeros = multiple_of_power_of_5(mv - 1 - mm_shift as u64, q);
            } else {
                vp -= multiple_of_power_of_5(mv + 2, q) as u64;
            }
        }
    } else {
        let q = log10_pow5(-e2) - (-e2 > 1) as u32;
        e10 = q as i32 + e2;
        let i = -e2 - q as i32;
        let k = pow5bits(i) - POW5_BITCOUNT;
        let j = q as i32 - k;
        let mul = ryu_pow5(i as u32);
        vr = mul_shift_64(4 * m2, mul, j as u32);
        vp = mul_shift_64(4 * m2 + 2, mul, j as u32);
        vm = mul_shift_64(4 * m2 - 1 - mm_shift as u64, mul, j as u32);
        if q <= 1 {
            // {vr,vp,vm} is trailing zeros if {mv,mp,mm} has at least q trailing 0 bits.
            // mv = 4 * m2 always has at least two.
            vr_is_trailing_zeros = true;
            if accept_bounds {
                // mm = mv - 1 - mm_shift has 1 trailing 0 bit iff mm_shift == 1.
                vm_is_trailing_zeros = mm_shift == 1;
            } else {
                // mp = mv + 2 always has at least one trailing 0 bit.
                vp -= 1;
            }
        } else if q < 63 {
            // We want to know if the full product has at least q trailing zeros:
            // min(p2(mv), p5(mv) - e2) >= q <=> p2(mv) >= q (because -e2 >= q).
            vr_is_trailing_zeros = multiple_of_power_of_2(mv, q);
        }
    }

    // Step 4: the shortest decimal representation in the interval.
    let mut removed = 0i32;
    let mut last_removed_digit = 0u8;
    let output = if vm_is_trailing_zeros || vr_is_trailing_zeros {
        // General case, which happens rarely (~0.7%).
        while vp / 10 > vm / 10 {
            vm_is_trailing_zeros &= vm.is_multiple_of(10);
            vr_is_trailing_zeros &= last_removed_digit == 0;
            last_removed_digit = (vr % 10) as u8;
            vr /= 10;
            vp /= 10;
            vm /= 10;
            removed += 1;
        }
        if vm_is_trailing_zeros {
            while vm.is_multiple_of(10) {
                vr_is_trailing_zeros &= last_removed_digit == 0;
                last_removed_digit = (vr % 10) as u8;
                vr /= 10;
                vm /= 10;
                removed += 1;
            }
        }
        if vr_is_trailing_zeros && last_removed_digit == 5 && vr.is_multiple_of(2) {
            // Round even if the exact number is .....50..0.
            last_removed_digit = 4;
        }
        // Take vr + 1 if vr is outside the bounds or we need to round up.
        vr + ((vr == vm && (!accept_bounds || !vm_is_trailing_zeros)) || last_removed_digit >= 5)
            as u64
    } else {
        // Specialized for the common case (~99.3%).
        let mut round_up = false;
        // Remove two digits at a time (~86.2%).
        if vp / 100 > vm / 100 {
            round_up = vr % 100 >= 50;
            vr /= 100;
            vp /= 100;
            vm /= 100;
            removed += 2;
        }
        while vp / 10 > vm / 10 {
            round_up = vr % 10 >= 5;
            vr /= 10;
            vp /= 10;
            vm /= 10;
            removed += 1;
        }
        vr + (vr == vm || round_up) as u64
    };
    Decimal {
        mantissa: output,
        exponent: e10 + removed,
    }
}
