use std::f64::consts::PI;

use super::exponential::exp;
use super::logarithm::log;
use super::power::pow;
use super::trigonometric::sin_pi;
use super::{HIGH_MAGNITUDE_MASK, NON_FINITE_HIGH_WORD, power_of_two, power_of_two_high_word};

const NEAR_ZERO: [f64; 12] = [
    0.07721566490153287,
    0.3224670334241136,
    0.06735230105312927,
    0.020580808432516733,
    0.007385550860814029,
    0.0028905138367341563,
    0.0011927076318336207,
    0.0005100697921535113,
    0.00022086279071390839,
    0.00010801156724758394,
    2.5214456545125733e-05,
    4.4864094961891516e-05,
];
const MINIMUM_POSITION: f64 = 1.4616321449683622;
const MINIMUM_VALUE: f64 = -0.12148629053584961;
const MINIMUM_CORRECTION: f64 = -3.638676997039505e-18;
const NEAR_MINIMUM: [f64; 15] = [
    0.48383612272381005,
    -0.1475877229945939,
    0.06462494023913339,
    -0.032788541075985965,
    0.01797067508118204,
    -0.010314224129834144,
    0.006100538702462913,
    -0.0036845201678113826,
    0.0022596478090061247,
    -0.0014034646998923284,
    0.000881081882437654,
    -0.0005385953053567405,
    0.00031563207090362595,
    -0.00031275416837512086,
    0.0003355291926355191,
];
const NEAR_ONE_NUMERATOR: [f64; 6] = [
    -0.07721566490153287,
    0.6328270640250934,
    1.4549225013723477,
    0.9777175279633727,
    0.22896372806469245,
    0.013381091853678766,
];
const NEAR_ONE_DENOMINATOR: [f64; 5] = [
    2.4559779371304113,
    2.128489763798934,
    0.7692851504566728,
    0.10422264559336913,
    0.003217092422824239,
];
const SMALL_NUMERATOR: [f64; 7] = [
    -0.07721566490153287,
    0.21498241596060885,
    0.325778796408931,
    0.14635047265246445,
    0.02664227030336386,
    0.0018402845140733772,
    3.194753265841009e-05,
];
const SMALL_DENOMINATOR: [f64; 6] = [
    1.3920053346762105,
    0.7219355475671381,
    0.17193386563280308,
    0.01864591917156529,
    0.0007779424963818936,
    7.326684307446256e-06,
];
const STIRLING: [f64; 7] = [
    0.4189385332046727,
    0.08333333333333297,
    -0.0027777777772877554,
    0.0007936505586430196,
    -0.00059518755745034,
    0.0008363399189962821,
    -0.0016309293409657527,
];
const LGAMMA_TINY_HIGH_WORD: u32 = power_of_two_high_word(-70);
const ONE_HIGH_WORD: u32 = super::ONE_HIGH_WORD;
const TWO_HIGH_WORD: u32 = power_of_two_high_word(1);
const EIGHT_HIGH_WORD: u32 = power_of_two_high_word(3);
const STIRLING_LIMIT_HIGH_WORD: u32 = power_of_two_high_word(58);
const NEAR_ONE_LIMIT_HIGH_WORD: u32 = 0x3fec_cccc;
const BELOW_ONE_NEAR_HIGH_WORD: u32 = 0x3fe7_6944;
const BELOW_ONE_MINIMUM_HIGH_WORD: u32 = 0x3fcd_a661;
const BELOW_TWO_NEAR_HIGH_WORD: u32 = 0x3ffb_b4c3;
const ABOVE_ONE_MINIMUM_HIGH_WORD: u32 = 0x3ff3_b4c4;
const LOW_WORD_MASK: u64 = 0xffff_ffff;

#[derive(Clone, Copy)]
enum Near {
    Zero,
    Minimum,
    One,
}

fn below_two(x: f64, magnitude: u32) -> f64 {
    let (r, y, interval) = if magnitude <= NEAR_ONE_LIMIT_HIGH_WORD {
        let r = -log(x);
        if magnitude >= BELOW_ONE_NEAR_HIGH_WORD {
            (r, 1.0 - x, Near::Zero)
        } else if magnitude >= BELOW_ONE_MINIMUM_HIGH_WORD {
            (r, x - (MINIMUM_POSITION - 1.0), Near::Minimum)
        } else {
            (r, x, Near::One)
        }
    } else if magnitude >= BELOW_TWO_NEAR_HIGH_WORD {
        (0.0, 2.0 - x, Near::Zero)
    } else if magnitude >= ABOVE_ONE_MINIMUM_HIGH_WORD {
        (0.0, x - MINIMUM_POSITION, Near::Minimum)
    } else {
        (0.0, x - 1.0, Near::One)
    };
    match interval {
        Near::Zero => {
            let a = &NEAR_ZERO;
            let z = y * y;
            let p1 = a[0] + z * (a[2] + z * (a[4] + z * (a[6] + z * (a[8] + z * a[10]))));
            let p2 = z * (a[1] + z * (a[3] + z * (a[5] + z * (a[7] + z * (a[9] + z * a[11])))));
            let p = y * p1 + p2;
            r + (p - 0.5 * y)
        }
        Near::Minimum => {
            let t = &NEAR_MINIMUM;
            let z = y * y;
            let w = z * y;
            let p1 = t[0] + w * (t[3] + w * (t[6] + w * (t[9] + w * t[12])));
            let p2 = t[1] + w * (t[4] + w * (t[7] + w * (t[10] + w * t[13])));
            let p3 = t[2] + w * (t[5] + w * (t[8] + w * (t[11] + w * t[14])));
            let p = z * p1 - (MINIMUM_CORRECTION - w * (p2 + y * p3));
            r + (MINIMUM_VALUE + p)
        }
        Near::One => {
            let u = &NEAR_ONE_NUMERATOR;
            let v = &NEAR_ONE_DENOMINATOR;
            let p1 = y * (u[0] + y * (u[1] + y * (u[2] + y * (u[3] + y * (u[4] + y * u[5])))));
            let p2 = 1.0 + y * (v[0] + y * (v[1] + y * (v[2] + y * (v[3] + y * v[4]))));
            r + (-0.5 * y + p1 / p2)
        }
    }
}

fn below_eight(x: f64) -> f64 {
    let i = x as i32;
    let y = x - f64::from(i);
    let s = &SMALL_NUMERATOR;
    let q = &SMALL_DENOMINATOR;
    let p = y * (s[0] + y * (s[1] + y * (s[2] + y * (s[3] + y * (s[4] + y * (s[5] + y * s[6]))))));
    let d = 1.0 + y * (q[0] + y * (q[1] + y * (q[2] + y * (q[3] + y * (q[4] + y * q[5])))));
    let mut r = 0.5 * y + p / d;
    let mut z = 1.0;
    for k in (3..=i.min(7)).rev() {
        z *= y + f64::from(k - 1);
    }
    if i >= 3 {
        r += log(z);
    }
    r
}

pub fn lgamma_r(x: f64) -> (f64, i32) {
    let bits = x.to_bits();
    let negative = bits >> 63 != 0;
    let magnitude = (bits >> 32) as u32 & HIGH_MAGNITUDE_MASK;
    if magnitude >= NON_FINITE_HIGH_WORD {
        return (x * x, 1);
    }
    if magnitude < LGAMMA_TINY_HIGH_WORD {
        if negative {
            return (-log(-x), -1);
        }
        return (-log(x), 1);
    }
    let mut sign = 1;
    let mut x = x;
    let mut adjustment = 0.0;
    if negative {
        x = -x;
        let mut t = sin_pi(x);
        if t == 0.0 {
            return (1.0 / super::difference(x, x), sign);
        }
        if t > 0.0 {
            sign = -1;
        } else {
            t = -t;
        }
        adjustment = log(PI / (t * x));
    }
    let r = if (magnitude == ONE_HIGH_WORD || magnitude == TWO_HIGH_WORD)
        && bits & LOW_WORD_MASK == 0
    {
        0.0
    } else if magnitude < TWO_HIGH_WORD {
        below_two(x, magnitude)
    } else if magnitude < EIGHT_HIGH_WORD {
        below_eight(x)
    } else if magnitude < STIRLING_LIMIT_HIGH_WORD {
        let t = log(x);
        let z = 1.0 / x;
        let y = z * z;
        let w = &STIRLING;
        let w = w[0] + z * (w[1] + y * (w[2] + y * (w[3] + y * (w[4] + y * (w[5] + y * w[6])))));
        (x - 0.5) * (t - 1.0) + w
    } else {
        x * (log(x) - 1.0)
    };
    if negative {
        return (adjustment - r, sign);
    }
    (r, sign)
}

pub fn lgamma(x: f64) -> f64 {
    lgamma_r(x).0
}

const LANCZOS_TERMS: usize = 13;
const LANCZOS_SHIFT: f64 = 5.52468004077673;
const LANCZOS_NUMERATOR: [f64; LANCZOS_TERMS] = [
    23531376880.41076,
    42919803642.6491,
    35711959237.35567,
    17921034426.03721,
    6039542586.352028,
    1439720407.3117216,
    248874557.86205417,
    31426415.585400194,
    2876370.6289353725,
    186056.26539522348,
    8071.672002365816,
    210.82427775157936,
    2.5066282746310002,
];
const LANCZOS_DENOMINATOR: [f64; LANCZOS_TERMS] = [
    0.0,
    39916800.0,
    120543840.0,
    150917976.0,
    105258076.0,
    45995730.0,
    13339535.0,
    2637558.0,
    357423.0,
    32670.0,
    1925.0,
    66.0,
    1.0,
];
const FACTORIALS: [f64; 23] = [
    1.0,
    1.0,
    2.0,
    6.0,
    24.0,
    120.0,
    720.0,
    5040.0,
    40320.0,
    362880.0,
    3628800.0,
    39916800.0,
    479001600.0,
    6227020800.0,
    87178291200.0,
    1307674368000.0,
    20922789888000.0,
    355687428096000.0,
    6402373705728000.0,
    1.21645100408832e17,
    2.43290200817664e18,
    5.109094217170944e19,
    1.1240007277776077e21,
];
const LANCZOS_FORWARD_LIMIT: f64 = 8.0;
const TGAMMA_TINY_HIGH_WORD: u32 = power_of_two_high_word(-54);
const TGAMMA_SATURATED_HIGH_WORD: u32 = 0x4067_0000;
const OVERFLOW_SCALE: f64 = power_of_two(1023);

fn lanczos_sum(x: f64) -> f64 {
    let mut numerator = 0.0;
    let mut denominator = 0.0;
    let terms = LANCZOS_NUMERATOR.iter().zip(&LANCZOS_DENOMINATOR);
    if x < LANCZOS_FORWARD_LIMIT {
        for (n, d) in terms.rev() {
            numerator = numerator * x + n;
            denominator = denominator * x + d;
        }
    } else {
        for (n, d) in terms {
            numerator = numerator / x + n;
            denominator = denominator / x + d;
        }
    }
    numerator / denominator
}

pub fn tgamma(x: f64) -> f64 {
    let bits = x.to_bits();
    let magnitude = (bits >> 32) as u32 & HIGH_MAGNITUDE_MASK;
    let negative = bits >> 63 != 0;
    if magnitude >= NON_FINITE_HIGH_WORD {
        return x + f64::INFINITY;
    }
    if magnitude < TGAMMA_TINY_HIGH_WORD {
        return 1.0 / x;
    }
    if x == x.floor() {
        if negative {
            return super::quotient(0.0, 0.0);
        }
        if x <= FACTORIALS.len() as f64 {
            return FACTORIALS[x as usize - 1];
        }
    }
    if magnitude >= TGAMMA_SATURATED_HIGH_WORD {
        if negative {
            if x.floor() * 0.5 == (x * 0.5).floor() {
                return 0.0;
            }
            return -0.0;
        }
        return x * OVERFLOW_SCALE;
    }
    let absolute = if negative { -x } else { x };
    let y = absolute + LANCZOS_SHIFT;
    let dy = if absolute > LANCZOS_SHIFT {
        (y - absolute) - LANCZOS_SHIFT
    } else {
        (y - LANCZOS_SHIFT) - absolute
    };
    let z = absolute - 0.5;
    let mut r = lanczos_sum(absolute) * exp(-y);
    let (dy, z) = if x < 0.0 {
        r = -PI / (sin_pi(absolute) * absolute * r);
        (-dy, -z)
    } else {
        (dy, z)
    };
    r += dy * (LANCZOS_SHIFT + 0.5) * r / y;
    let z = pow(y, 0.5 * z);
    r * z * z
}
