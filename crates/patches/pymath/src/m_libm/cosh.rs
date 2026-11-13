use super::{exp, expm1, k_expo2};

pub fn cosh(mut x: f64) -> f64 {
    let mut ix = x.to_bits();
    ix &= 0x7fffffffffffffff;
    x = f64::from_bits(ix);
    let w = ix >> 32;

    if w < 0x3fe62e42 {
        if w < 0x3ff00000 - (26 << 20) {
            return 1.;
        }
        let t = expm1(x);
        return 1. + t * t / (2. * (1. + t));
    }

    if w < 0x40862e42 {
        let t = exp(x);

        return 0.5 * (t + 1. / t);
    }

    k_expo2(x)
}
