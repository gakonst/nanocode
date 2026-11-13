use super::{SIGNIFICAND_BITS, SIGNIFICAND_MASK, exponent_unbiased};

pub fn floor(x: f64) -> f64 {
    let mut ix = x.to_bits();
    let e = exponent_unbiased(x);
    if e >= SIGNIFICAND_BITS as i32 {
        return x;
    }
    if e >= 0 {
        let m = SIGNIFICAND_MASK >> e as u32;
        if ix & m == 0 {
            return x;
        }
        if x.is_sign_negative() {
            ix += m;
        }
        ix &= !m;
        f64::from_bits(ix)
    } else if x.is_sign_positive() {
        0.0
    } else if ix << 1 != 0 {
        -1.0
    } else {
        x
    }
}
