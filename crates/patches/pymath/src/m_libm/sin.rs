use super::{k_cos, k_sin, rem_pio2};

pub fn sin(x: f64) -> f64 {
    let ix = (f64::to_bits(x) >> 32) as u32 & 0x7fffffff;

    if ix <= 0x3fe921fb {
        if ix < 0x3e500000 {
            return x;
        }
        return k_sin(x, 0.0, 0);
    }

    if ix >= 0x7ff00000 {
        return x - x;
    }

    let (n, y0, y1) = rem_pio2(x);
    match n & 3 {
        0 => k_sin(y0, y1, 1),
        1 => k_cos(y0, y1),
        2 => -k_sin(y0, y1, 1),
        _ => -k_cos(y0, y1),
    }
}
