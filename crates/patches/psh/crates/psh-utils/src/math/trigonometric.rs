use super::reduction::{PI_OVER_4_HIGH_WORD, reduce};
use super::{HIGH_MAGNITUDE_MASK, NON_FINITE_HIGH_WORD, high_word, power_of_two_high_word};

const SINE: [f64; 6] = [
    -0.16666666666666632,
    0.00833333333332249,
    -0.0001984126982985795,
    2.7557313707070068e-06,
    -2.5050760253406863e-08,
    1.58969099521155e-10,
];
const COSINE: [f64; 6] = [
    0.0416666666666666,
    -0.001388888888887411,
    2.480158728947673e-05,
    -2.7557314351390663e-07,
    2.087572321298175e-09,
    -1.1359647557788195e-11,
];
const TANGENT: [f64; 13] = [
    0.3333333333333341,
    0.13333333333320124,
    0.05396825397622605,
    0.021869488294859542,
    0.0088632398235993,
    0.0035920791075913124,
    0.0014562094543252903,
    0.0005880412408202641,
    0.0002464631348184699,
    7.817944429395571e-05,
    7.140724913826082e-05,
    -1.8558637485527546e-05,
    2.590730518636337e-05,
];
const PI_OVER_4_LOW: f64 = 3.061616997868383e-17;
const TANGENT_FOLD_HIGH_WORD: u32 = 0x3fe5_9428;
const SINE_TINY_HIGH_WORD: u32 = power_of_two_high_word(-26);
const COSINE_TINY_HIGH_WORD: u32 = 0x3e46_a09e;
const TANGENT_TINY_HIGH_WORD: u32 = power_of_two_high_word(-27);
const LOW_WORD_MASK: u64 = 0xffff_ffff_0000_0000;

pub(super) fn sine_kernel(x: f64, tail: f64, has_tail: bool) -> f64 {
    let z = x * x;
    let w = z * z;
    let r = SINE[1] + z * (SINE[2] + z * SINE[3]) + z * w * (SINE[4] + z * SINE[5]);
    let v = z * x;
    if has_tail {
        x - ((z * (0.5 * tail - v * r) - tail) - v * SINE[0])
    } else {
        x + v * (SINE[0] + z * r)
    }
}

pub(super) fn cosine_kernel(x: f64, tail: f64) -> f64 {
    let z = x * x;
    let w = z * z;
    let r = z * (COSINE[0] + z * (COSINE[1] + z * COSINE[2]))
        + w * w * (COSINE[3] + z * (COSINE[4] + z * COSINE[5]));
    let half_z = 0.5 * z;
    let w = 1.0 - half_z;
    w + (((1.0 - w) - half_z) + (z * r - x * tail))
}

fn without_low_word(x: f64) -> f64 {
    f64::from_bits(x.to_bits() & LOW_WORD_MASK)
}

fn tangent_kernel(x: f64, tail: f64, odd: bool) -> f64 {
    let high = high_word(x);
    let negative = high >> 31 != 0;
    let big = high & HIGH_MAGNITUDE_MASK >= TANGENT_FOLD_HIGH_WORD;
    let (x, tail) = if big {
        let (x, tail) = if negative { (-x, -tail) } else { (x, tail) };
        (
            (std::f64::consts::FRAC_PI_4 - x) + (PI_OVER_4_LOW - tail),
            0.0,
        )
    } else {
        (x, tail)
    };
    let z = x * x;
    let w = z * z;
    let t = &TANGENT;
    let r = t[1] + w * (t[3] + w * (t[5] + w * (t[7] + w * (t[9] + w * t[11]))));
    let v = z * (t[2] + w * (t[4] + w * (t[6] + w * (t[8] + w * (t[10] + w * t[12])))));
    let s = z * x;
    let r = tail + z * (s * (r + v) + tail) + s * t[0];
    let w = x + r;
    if big {
        let s = 1.0 - 2.0 * f64::from(u8::from(odd));
        let v = s - 2.0 * (x + (r - w * w / (w + s)));
        return if negative { -v } else { v };
    }
    if !odd {
        return w;
    }
    let w_high = without_low_word(w);
    let v = r - (w_high - x);
    let a = -1.0 / w;
    let a_high = without_low_word(a);
    a_high + a * (1.0 + a_high * w_high + a_high * v)
}

pub fn sin(x: f64) -> f64 {
    let magnitude = high_word(x) & HIGH_MAGNITUDE_MASK;
    if magnitude <= PI_OVER_4_HIGH_WORD {
        if magnitude < SINE_TINY_HIGH_WORD {
            return x;
        }
        return sine_kernel(x, 0.0, false);
    }
    if magnitude >= NON_FINITE_HIGH_WORD {
        return super::difference(x, x);
    }
    let reduced = reduce(x);
    match reduced.quadrant & 3 {
        0 => sine_kernel(reduced.high, reduced.low, true),
        1 => cosine_kernel(reduced.high, reduced.low),
        2 => -sine_kernel(reduced.high, reduced.low, true),
        _ => -cosine_kernel(reduced.high, reduced.low),
    }
}

pub fn cos(x: f64) -> f64 {
    let magnitude = high_word(x) & HIGH_MAGNITUDE_MASK;
    if magnitude <= PI_OVER_4_HIGH_WORD {
        if magnitude < COSINE_TINY_HIGH_WORD && x as i32 == 0 {
            return 1.0;
        }
        return cosine_kernel(x, 0.0);
    }
    if magnitude >= NON_FINITE_HIGH_WORD {
        return super::difference(x, x);
    }
    let reduced = reduce(x);
    match reduced.quadrant & 3 {
        0 => cosine_kernel(reduced.high, reduced.low),
        1 => -sine_kernel(reduced.high, reduced.low, true),
        2 => -cosine_kernel(reduced.high, reduced.low),
        _ => sine_kernel(reduced.high, reduced.low, true),
    }
}

pub fn tan(x: f64) -> f64 {
    let magnitude = high_word(x) & HIGH_MAGNITUDE_MASK;
    if magnitude <= PI_OVER_4_HIGH_WORD {
        if magnitude < TANGENT_TINY_HIGH_WORD {
            return x;
        }
        return tangent_kernel(x, 0.0, false);
    }
    if magnitude >= NON_FINITE_HIGH_WORD {
        return super::difference(x, x);
    }
    let reduced = reduce(x);
    tangent_kernel(reduced.high, reduced.low, reduced.quadrant & 1 != 0)
}

pub(super) fn sin_pi(x: f64) -> f64 {
    let half = x * 0.5;
    let x = 2.0 * (half - half.floor());
    let n = (x * 4.0) as i32;
    let n = (n + 1) / 2;
    let x = (x - f64::from(n) * 0.5) * std::f64::consts::PI;
    match n {
        1 => cosine_kernel(x, 0.0),
        2 => sine_kernel(-x, 0.0, false),
        3 => -cosine_kernel(x, 0.0),
        _ => sine_kernel(x, 0.0, false),
    }
}
