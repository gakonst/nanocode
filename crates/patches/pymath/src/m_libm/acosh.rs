use super::{log, log1p, sqrt};

const LN2: f64 = 0.693147180559945309417232121458176568;

pub fn acosh(x: f64) -> f64 {
    let u = x.to_bits();
    let e = ((u >> 52) as usize) & 0x7ff;

    if e < 0x3ff + 1 {
        return log1p(x - 1.0 + sqrt((x - 1.0) * (x - 1.0) + 2.0 * (x - 1.0)));
    }
    if e < 0x3ff + 26 {
        return log(2.0 * x - 1.0 / (x + sqrt(x * x - 1.0)));
    }

    return log(x) + LN2;
}
