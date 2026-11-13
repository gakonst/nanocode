use super::{EXPONENT_MASK, IMPLICIT_BIT, SIGN_MASK, SIGNIFICAND_BITS};

const STEPWISE_EXPONENT_MAX: u32 = 32;
const REDUCTION_SHIFT_BITS: u32 = u128::BITS - u64::BITS;

pub fn fmod(x: f64, y: f64) -> f64 {
    let sx = x.to_bits() & SIGN_MASK;
    let ux = x.to_bits() & !SIGN_MASK;
    let uy = y.to_bits() & !SIGN_MASK;
    let x_nan_or_inf = ux & EXPONENT_MASK == EXPONENT_MASK;
    let y_nan_or_zero = uy.wrapping_sub(1) & EXPONENT_MASK == EXPONENT_MASK;
    if x_nan_or_inf | y_nan_or_zero {
        return (x * y) / (x * y);
    }
    if ux < uy {
        return x;
    }
    let (num, ex) = into_significand_exponent(ux);
    let (div, ey) = into_significand_exponent(uy);
    let rem = reduction(num, ex - ey, div);
    if rem == 0 {
        return f64::from_bits(sx);
    }
    let shift = ey.min(SIGNIFICAND_BITS - rem.ilog2());
    let bits = (rem << shift) + (u64::from(ey - shift) << SIGNIFICAND_BITS);
    f64::from_bits(sx + bits)
}

fn into_significand_exponent(mut bits: u64) -> (u64, u32) {
    bits &= !SIGN_MASK;
    let saturated = bits.checked_sub(IMPLICIT_BIT).unwrap_or(0);
    (
        bits - (saturated & EXPONENT_MASK),
        (saturated >> SIGNIFICAND_BITS) as u32,
    )
}

fn reduction(mut x: u64, e: u32, y: u64) -> u64 {
    if x >= (y << 1) {
        x %= y;
    }
    if e < STEPWISE_EXPONENT_MAX {
        for _ in 0..e {
            x = x.checked_sub(y).unwrap_or(x);
            x <<= 1;
        }
        return x.checked_sub(y).unwrap_or(x);
    }
    let mut remaining = e;
    while remaining > 0 {
        let step = remaining.min(REDUCTION_SHIFT_BITS);
        x = ((u128::from(x) << step) % u128::from(y)) as u64;
        remaining -= step;
    }
    x
}
