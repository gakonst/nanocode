use super::{log, log1p, sqrt};

const LN2: f64 = 0.693147180559945309417232121458176568;

pub fn asinh(mut x: f64) -> f64 {
    let mut u = x.to_bits();
    let e = ((u >> 52) as usize) & 0x7ff;
    let sign = (u >> 63) != 0;

    u &= (!0) >> 1;
    x = f64::from_bits(u);

    if e >= 0x3ff + 26 {
        x = log(x) + LN2;
    } else if e >= 0x3ff + 1 {
        x = log(2.0 * x + 1.0 / (sqrt(x * x + 1.0) + x));
    } else if e >= 0x3ff - 26 {
        x = log1p(x + x * x / (sqrt(x * x + 1.0) + 1.0));
    }

    if sign { -x } else { x }
}
