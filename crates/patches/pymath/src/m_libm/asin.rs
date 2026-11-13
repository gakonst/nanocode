use super::{fabs, get_high_word, get_low_word, sqrt, with_set_low_word};

const PIO2_HI: f64 = 1.57079632679489655800e+00;
const PIO2_LO: f64 = 6.12323399573676603587e-17;

const P_S0: f64 = 1.66666666666666657415e-01;
const P_S1: f64 = -3.25565818622400915405e-01;
const P_S2: f64 = 2.01212532134862925881e-01;
const P_S3: f64 = -4.00555345006794114027e-02;
const P_S4: f64 = 7.91534994289814532176e-04;
const P_S5: f64 = 3.47933107596021167570e-05;
const Q_S1: f64 = -2.40339491173441421878e+00;
const Q_S2: f64 = 2.02094576023350569471e+00;
const Q_S3: f64 = -6.88283971605453293030e-01;
const Q_S4: f64 = 7.70381505559019352791e-02;

fn comp_r(z: f64) -> f64 {
    let p = z * (P_S0 + z * (P_S1 + z * (P_S2 + z * (P_S3 + z * (P_S4 + z * P_S5)))));
    let q = 1.0 + z * (Q_S1 + z * (Q_S2 + z * (Q_S3 + z * Q_S4)));
    p / q
}

pub fn asin(mut x: f64) -> f64 {
    let z: f64;
    let r: f64;
    let s: f64;
    let hx: u32;
    let ix: u32;

    hx = get_high_word(x);
    ix = hx & 0x7fffffff;

    if ix >= 0x3ff00000 {
        let lx: u32;
        lx = get_low_word(x);
        if ((ix - 0x3ff00000) | lx) == 0 {
            return x * PIO2_HI + f64::from_bits(0x3870000000000000);
        } else {
            return 0.0 / (x - x);
        }
    }

    if ix < 0x3fe00000 {
        if (0x00100000..0x3e500000).contains(&ix) {
            return x;
        } else {
            return x + x * comp_r(x * x);
        }
    }

    z = (1.0 - fabs(x)) * 0.5;
    s = sqrt(z);
    r = comp_r(z);
    if ix >= 0x3fef3333 {
        x = PIO2_HI - (2. * (s + s * r) - PIO2_LO);
    } else {
        let f: f64;
        let c: f64;

        f = with_set_low_word(s, 0);
        c = (z - f * f) / (s + f);
        x = 0.5 * PIO2_HI - (2.0 * s * r - (PIO2_LO - 2.0 * c) - (0.5 * PIO2_HI - 2.0 * f));
    }
    if hx >> 31 != 0 { -x } else { x }
}
