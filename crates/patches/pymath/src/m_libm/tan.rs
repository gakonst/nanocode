use super::{k_tan, rem_pio2};

pub fn tan(x: f64) -> f64 {
    let ix = (f64::to_bits(x) >> 32) as u32 & 0x7fffffff;

    if ix <= 0x3fe921fb {
        if ix < 0x3e400000 {
            return x;
        }
        return k_tan(x, 0.0, 0);
    }

    if ix >= 0x7ff00000 {
        return x - x;
    }

    let (n, y0, y1) = rem_pio2(x);
    k_tan(y0, y1, n & 1)
}
