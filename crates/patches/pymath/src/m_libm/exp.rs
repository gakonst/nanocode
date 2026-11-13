use super::scalbn;

const HALF: [f64; 2] = [0.5, -0.5];
const LN2HI: f64 = 6.93147180369123816490e-01;
const LN2LO: f64 = 1.90821492927058770002e-10;
const INVLN2: f64 = 1.44269504088896338700e+00;
const P1: f64 = 1.66666666666666019037e-01;
const P2: f64 = -2.77777777770155933842e-03;
const P3: f64 = 6.61375632143793436117e-05;
const P4: f64 = -1.65339022054652515390e-06;
const P5: f64 = 4.13813679705723846039e-08;

pub fn exp(mut x: f64) -> f64 {
    let x1p1023 = f64::from_bits(0x7fe0000000000000);

    let hi: f64;
    let lo: f64;
    let c: f64;
    let xx: f64;
    let y: f64;
    let k: i32;
    let sign: i32;
    let mut hx: u32;

    hx = (x.to_bits() >> 32) as u32;
    sign = (hx >> 31) as i32;
    hx &= 0x7fffffff;

    if hx >= 0x4086232b {
        if x.is_nan() {
            return x;
        }
        if x > 709.782712893383973096 {
            x *= x1p1023;
            return x;
        }
        if x < -708.39641853226410622 {
            if x < -745.13321910194110842 {
                return 0.;
            }
        }
    }

    if hx > 0x3fd62e42 {
        if hx >= 0x3ff0a2b2 {
            k = (INVLN2 * x + HALF[sign as usize]) as i32;
        } else {
            k = 1 - sign - sign;
        }
        hi = x - k as f64 * LN2HI;
        lo = k as f64 * LN2LO;
        x = hi - lo;
    } else if hx > 0x3e300000 {
        k = 0;
        hi = x;
        lo = 0.;
    } else {
        return 1. + x;
    }

    xx = x * x;
    c = x - xx * (P1 + xx * (P2 + xx * (P3 + xx * (P4 + xx * P5))));
    y = 1. + (x * c / (2. - c) - lo + hi);
    if k == 0 { y } else { scalbn(y, k) }
}
