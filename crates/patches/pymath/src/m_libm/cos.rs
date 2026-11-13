use super::{k_cos, k_sin, rem_pio2};

pub fn cos(x: f64) -> f64 {
    let ix = (f64::to_bits(x) >> 32) as u32 & 0x7fffffff;

    if ix <= 0x3fe921fb {
        if ix < 0x3e46a09e {
            if x as i32 == 0 {
                return 1.0;
            }
        }
        return k_cos(x, 0.0);
    }

    if ix >= 0x7ff00000 {
        return x - x;
    }

    let (n, y0, y1) = rem_pio2(x);
    match n & 3 {
        0 => k_cos(y0, y1),
        1 => -k_sin(y0, y1, 1),
        2 => -k_cos(y0, y1),
        _ => k_sin(y0, y1, 1),
    }
}
