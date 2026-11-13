use super::log1p;

pub fn atanh(x: f64) -> f64 {
    let u = x.to_bits();
    let e = ((u >> 52) as usize) & 0x7ff;
    let sign = (u >> 63) != 0;

    let mut y = f64::from_bits(u & 0x7fff_ffff_ffff_ffff);

    if e < 0x3ff - 1 {
        if e < 0x3ff - 32 {
        } else {
            y = 0.5 * log1p(2.0 * y + 2.0 * y * y / (1.0 - y));
        }
    } else {
        y = 0.5 * log1p(2.0 * (y / (1.0 - y)));
    }

    if sign { -y } else { y }
}
