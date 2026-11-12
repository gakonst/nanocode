use super::exponential::exp;
use super::{
    HIGH_MAGNITUDE_MASK, NON_FINITE_HIGH_WORD, high_word, power_of_two, power_of_two_high_word,
    with_low_word,
};

const ERX: f64 = 0.8450629115104675;
const EFX8: f64 = 1.0270333367641007;
const SMALL_NUMERATOR: [f64; 5] = [
    0.12837916709551256,
    -0.3250421072470015,
    -0.02848174957559851,
    -0.005770270296489442,
    -2.3763016656650163e-05,
];
const SMALL_DENOMINATOR: [f64; 5] = [
    0.39791722395915535,
    0.0650222499887673,
    0.005081306281875766,
    0.00013249473800432164,
    -3.960228278775368e-06,
];
const NEAR_ONE_NUMERATOR: [f64; 7] = [
    -0.0023621185607526594,
    0.41485611868374833,
    -0.3722078760357013,
    0.31834661990116175,
    -0.11089469428239668,
    0.035478304325618236,
    -0.002166375594868791,
];
const NEAR_ONE_DENOMINATOR: [f64; 6] = [
    0.10642088040084423,
    0.540397917702171,
    0.07182865441419627,
    0.12617121980876164,
    0.01363708391202905,
    0.011984499846799107,
];
const MIDDLE_NUMERATOR: [f64; 8] = [
    -0.009864944034847148,
    -0.6938585727071818,
    -10.558626225323291,
    -62.375332450326006,
    -162.39666946257347,
    -184.60509290671104,
    -81.2874355063066,
    -9.814329344169145,
];
const MIDDLE_DENOMINATOR: [f64; 8] = [
    19.651271667439257,
    137.65775414351904,
    434.56587747522923,
    645.3872717332679,
    429.00814002756783,
    108.63500554177944,
    6.570249770319282,
    -0.0604244152148581,
];
const TAIL_NUMERATOR: [f64; 7] = [
    -0.0098649429247001,
    -0.799283237680523,
    -17.757954917754752,
    -160.63638485582192,
    -637.5664433683896,
    -1025.0951316110772,
    -483.5191916086514,
];
const TAIL_DENOMINATOR: [f64; 7] = [
    30.33806074348246,
    325.7925129965739,
    1536.729586084437,
    3199.8582195085955,
    2553.0504064331644,
    474.52854120695537,
    -22.44095244658582,
];
const SMALL_HIGH_WORD: u32 = 0x3feb_0000;
const NEAR_ONE_HIGH_WORD: u32 = 0x3ff4_0000;
const MIDDLE_HIGH_WORD: u32 = 0x4006_db6d;
const QUARTER_HIGH_WORD: u32 = power_of_two_high_word(-2);
const ERF_TINY_HIGH_WORD: u32 = power_of_two_high_word(-28);
const ERFC_TINY_HIGH_WORD: u32 = power_of_two_high_word(-56);
const ERF_SATURATED_HIGH_WORD: u32 = 0x4018_0000;
const ERFC_SATURATED_HIGH_WORD: u32 = 0x403c_0000;
const SMALLEST_NORMAL: f64 = power_of_two(-1022);
const ERFC_EXP_OFFSET: f64 = 0.5625;

fn small_ratio(x: f64) -> f64 {
    let z = x * x;
    let p = &SMALL_NUMERATOR;
    let q = &SMALL_DENOMINATOR;
    let r = p[0] + z * (p[1] + z * (p[2] + z * (p[3] + z * p[4])));
    let s = 1.0 + z * (q[0] + z * (q[1] + z * (q[2] + z * (q[3] + z * q[4]))));
    r / s
}

fn complement_near_one(x: f64) -> f64 {
    let s = x.abs() - 1.0;
    let p = &NEAR_ONE_NUMERATOR;
    let q = &NEAR_ONE_DENOMINATOR;
    let numerator =
        p[0] + s * (p[1] + s * (p[2] + s * (p[3] + s * (p[4] + s * (p[5] + s * p[6])))));
    let denominator =
        1.0 + s * (q[0] + s * (q[1] + s * (q[2] + s * (q[3] + s * (q[4] + s * q[5])))));
    1.0 - ERX - numerator / denominator
}

fn complement(magnitude: u32, x: f64) -> f64 {
    if magnitude < NEAR_ONE_HIGH_WORD {
        return complement_near_one(x);
    }
    let x = x.abs();
    let s = 1.0 / (x * x);
    let (r, big_s) = if magnitude < MIDDLE_HIGH_WORD {
        let a = &MIDDLE_NUMERATOR;
        let b = &MIDDLE_DENOMINATOR;
        let r = a[0]
            + s * (a[1]
                + s * (a[2] + s * (a[3] + s * (a[4] + s * (a[5] + s * (a[6] + s * a[7]))))));
        let big_s = 1.0
            + s * (b[0]
                + s * (b[1]
                    + s * (b[2] + s * (b[3] + s * (b[4] + s * (b[5] + s * (b[6] + s * b[7])))))));
        (r, big_s)
    } else {
        let a = &TAIL_NUMERATOR;
        let b = &TAIL_DENOMINATOR;
        let r = a[0] + s * (a[1] + s * (a[2] + s * (a[3] + s * (a[4] + s * (a[5] + s * a[6])))));
        let big_s = 1.0
            + s * (b[0]
                + s * (b[1] + s * (b[2] + s * (b[3] + s * (b[4] + s * (b[5] + s * b[6]))))));
        (r, big_s)
    };
    let z = with_low_word(x, 0);
    exp(-z * z - ERFC_EXP_OFFSET) * exp((z - x) * (z + x) + r / big_s) / x
}

pub fn erf(x: f64) -> f64 {
    let high = high_word(x);
    let negative = high >> 31 != 0;
    let magnitude = high & HIGH_MAGNITUDE_MASK;
    if magnitude >= NON_FINITE_HIGH_WORD {
        return 1.0 - 2.0 * f64::from(u8::from(negative)) + 1.0 / x;
    }
    if magnitude < SMALL_HIGH_WORD {
        if magnitude < ERF_TINY_HIGH_WORD {
            return 0.125 * (8.0 * x + EFX8 * x);
        }
        return x + x * small_ratio(x);
    }
    let y = if magnitude < ERF_SATURATED_HIGH_WORD {
        1.0 - complement(magnitude, x)
    } else {
        1.0 - SMALLEST_NORMAL
    };
    if negative { -y } else { y }
}

pub fn erfc(x: f64) -> f64 {
    let high = high_word(x);
    let negative = high >> 31 != 0;
    let magnitude = high & HIGH_MAGNITUDE_MASK;
    if magnitude >= NON_FINITE_HIGH_WORD {
        return 2.0 * f64::from(u8::from(negative)) + 1.0 / x;
    }
    if magnitude < SMALL_HIGH_WORD {
        if magnitude < ERFC_TINY_HIGH_WORD {
            return 1.0 - x;
        }
        let y = small_ratio(x);
        if negative || magnitude < QUARTER_HIGH_WORD {
            return 1.0 - (x + x * y);
        }
        return 0.5 - (x - 0.5 + x * y);
    }
    if magnitude < ERFC_SATURATED_HIGH_WORD {
        if negative {
            return 2.0 - complement(magnitude, x);
        }
        return complement(magnitude, x);
    }
    if negative {
        2.0 - SMALLEST_NORMAL
    } else {
        SMALLEST_NORMAL * SMALLEST_NORMAL
    }
}
