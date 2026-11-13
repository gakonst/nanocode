use super::{EXPONENT_BIAS, EXPONENT_MAX, EXPONENT_MIN, SIGNIFICAND_BITS, from_exponent};

pub fn scalbn(mut x: f64, mut n: i32) -> f64 {
    let significand_total_bits = SIGNIFICAND_BITS + 1;
    let f_exponent_max = from_exponent(EXPONENT_BIAS << 1);
    let f_exponent_min = from_exponent(1);
    let f_power_subnormal = from_exponent(significand_total_bits + EXPONENT_BIAS);
    if n > EXPONENT_MAX {
        x *= f_exponent_max;
        n -= EXPONENT_MAX;
        if n > EXPONENT_MAX {
            x *= f_exponent_max;
            n -= EXPONENT_MAX;
            if n > EXPONENT_MAX {
                n = EXPONENT_MAX;
            }
        }
    } else if n < EXPONENT_MIN {
        let multiplier = f_exponent_min * f_power_subnormal;
        let add = -EXPONENT_MIN - significand_total_bits as i32;
        x *= multiplier;
        n += add;
        if n < EXPONENT_MIN {
            x *= multiplier;
            n += add;
            if n < EXPONENT_MIN {
                n = EXPONENT_MIN;
            }
        }
    }
    let scale = from_exponent((EXPONENT_BIAS as i32 + n) as u32);
    x * scale
}
