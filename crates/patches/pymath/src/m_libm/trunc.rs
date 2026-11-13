use super::{SIGN_MASK, SIGNIFICAND_BITS, SIGNIFICAND_MASK, exponent_unbiased};

pub fn trunc(x: f64) -> f64 {
    let mut xi = x.to_bits();
    let e = exponent_unbiased(x);
    if e >= SIGNIFICAND_BITS as i32 {
        return x;
    }
    let mask = if e < 0 {
        SIGN_MASK
    } else {
        !(SIGNIFICAND_MASK >> e as u32)
    };
    if (xi & !mask) == 0 {
        return x;
    }
    xi &= mask;
    f64::from_bits(xi)
}
