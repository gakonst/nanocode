use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};

use super::{
    HIGH_MAGNITUDE_MASK, NON_FINITE_HIGH_WORD, ONE_HIGH_WORD, high_word, low_word,
    power_of_two_high_word, with_low_word,
};

const PI_OVER_2_LOW: f64 = 6.123233995736766e-17;
const PI_LOW: f64 = 1.2246467991473532e-16;
const ARCSINE_NUMERATOR: [f64; 6] = [
    0.16666666666666666,
    -0.3255658186224009,
    0.20121253213486293,
    -0.04005553450067941,
    0.0007915349942898145,
    3.479331075960212e-05,
];
const ARCSINE_DENOMINATOR: [f64; 4] = [
    -2.403394911734414,
    2.0209457602335057,
    -0.6882839716054533,
    0.07703815055590194,
];
const ARCTANGENT_HIGH: [f64; 4] = [0.4636476090008061, FRAC_PI_4, 0.982793723247329, FRAC_PI_2];
const ARCTANGENT_LOW: [f64; 4] = [
    2.2698777452961687e-17,
    3.061616997868383e-17,
    1.3903311031230998e-17,
    6.123233995736766e-17,
];
const ARCTANGENT: [f64; 11] = [
    0.3333333333333293,
    -0.19999999999876483,
    0.14285714272503466,
    -0.11111110405462356,
    0.09090887133436507,
    -0.0769187620504483,
    0.06661073137387531,
    -0.058335701337905735,
    0.049768779946159324,
    -0.036531572744216916,
    0.016285820115365782,
];
const HALF_HIGH_WORD: u32 = power_of_two_high_word(-1);
const ARCSINE_TINY_HIGH_WORD: u32 = power_of_two_high_word(-26);
const ARCCOSINE_TINY_HIGH_WORD: u32 = power_of_two_high_word(-57);
const ARCSINE_NEAR_ONE_HIGH_WORD: u32 = 0x3fef_3333;
const SMALLEST_NORMAL_HIGH_WORD: u32 = super::SMALLEST_NORMAL_HIGH_WORD;
const ARCTANGENT_HUGE_HIGH_WORD: u32 = power_of_two_high_word(66);
const ARCTANGENT_TINY_HIGH_WORD: u32 = power_of_two_high_word(-27);
const SEVEN_SIXTEENTHS_HIGH_WORD: u32 = 0x3fdc_0000;
const ELEVEN_SIXTEENTHS_HIGH_WORD: u32 = 0x3fe6_0000;
const NINETEEN_SIXTEENTHS_HIGH_WORD: u32 = 0x3ff3_0000;
const THIRTY_NINE_SIXTEENTHS_HIGH_WORD: u32 = 0x4003_8000;
const RATIO_EXPONENT_LIMIT_HIGH_WORD: u32 = 64 << super::HIGH_SIGNIFICAND_BITS;

fn arcsine_ratio(z: f64) -> f64 {
    let p = &ARCSINE_NUMERATOR;
    let q = &ARCSINE_DENOMINATOR;
    let numerator = z * (p[0] + z * (p[1] + z * (p[2] + z * (p[3] + z * (p[4] + z * p[5])))));
    let denominator = 1.0 + z * (q[0] + z * (q[1] + z * (q[2] + z * q[3])));
    numerator / denominator
}

pub fn acos(x: f64) -> f64 {
    let high = high_word(x);
    let magnitude = high & HIGH_MAGNITUDE_MASK;
    let negative = high >> 31 != 0;
    if magnitude >= ONE_HIGH_WORD {
        if (magnitude - ONE_HIGH_WORD) | low_word(x) == 0 {
            return if negative { 2.0 * FRAC_PI_2 } else { 0.0 };
        }
        return 0.0 / super::difference(x, x);
    }
    if magnitude < HALF_HIGH_WORD {
        if magnitude <= ARCCOSINE_TINY_HIGH_WORD {
            return FRAC_PI_2;
        }
        return FRAC_PI_2 - (x - (PI_OVER_2_LOW - x * arcsine_ratio(x * x)));
    }
    if negative {
        let z = (1.0 + x) * 0.5;
        let s = z.sqrt();
        let w = arcsine_ratio(z) * s - PI_OVER_2_LOW;
        return 2.0 * (FRAC_PI_2 - (s + w));
    }
    let z = (1.0 - x) * 0.5;
    let s = z.sqrt();
    let s_high = with_low_word(s, 0);
    let correction = (z - s_high * s_high) / (s + s_high);
    let w = arcsine_ratio(z) * s + correction;
    2.0 * (s_high + w)
}

pub fn asin(x: f64) -> f64 {
    let high = high_word(x);
    let magnitude = high & HIGH_MAGNITUDE_MASK;
    if magnitude >= ONE_HIGH_WORD {
        if (magnitude - ONE_HIGH_WORD) | low_word(x) == 0 {
            return x * FRAC_PI_2;
        }
        return 0.0 / super::difference(x, x);
    }
    if magnitude < HALF_HIGH_WORD {
        if (SMALLEST_NORMAL_HIGH_WORD..ARCSINE_TINY_HIGH_WORD).contains(&magnitude) {
            return x;
        }
        return x + x * arcsine_ratio(x * x);
    }
    let z = (1.0 - x.abs()) * 0.5;
    let s = z.sqrt();
    let r = arcsine_ratio(z);
    let result = if magnitude >= ARCSINE_NEAR_ONE_HIGH_WORD {
        FRAC_PI_2 - (2.0 * (s + s * r) - PI_OVER_2_LOW)
    } else {
        let s_high = with_low_word(s, 0);
        let correction = (z - s_high * s_high) / (s + s_high);
        0.5 * FRAC_PI_2
            - (2.0 * s * r - (PI_OVER_2_LOW - 2.0 * correction) - (0.5 * FRAC_PI_2 - 2.0 * s_high))
    };
    if high >> 31 != 0 { -result } else { result }
}

fn arctangent_reduce(x: f64, magnitude: u32) -> (f64, usize) {
    let x = x.abs();
    if magnitude < NINETEEN_SIXTEENTHS_HIGH_WORD {
        if magnitude < ELEVEN_SIXTEENTHS_HIGH_WORD {
            return ((2.0 * x - 1.0) / (2.0 + x), 0);
        }
        return ((x - 1.0) / (x + 1.0), 1);
    }
    if magnitude < THIRTY_NINE_SIXTEENTHS_HIGH_WORD {
        return ((x - 1.5) / (1.0 + 1.5 * x), 2);
    }
    (-1.0 / x, 3)
}

pub fn atan(x: f64) -> f64 {
    let high = high_word(x);
    let negative = high >> 31 != 0;
    let magnitude = high & HIGH_MAGNITUDE_MASK;
    if magnitude >= ARCTANGENT_HUGE_HIGH_WORD {
        if x.is_nan() {
            return x;
        }
        let z = ARCTANGENT_HIGH[3];
        return if negative { -z } else { z };
    }
    let (x, interval) = if magnitude < SEVEN_SIXTEENTHS_HIGH_WORD {
        if magnitude < ARCTANGENT_TINY_HIGH_WORD {
            return x;
        }
        (x, None)
    } else {
        let (reduced, interval) = arctangent_reduce(x, magnitude);
        (reduced, Some(interval))
    };
    let z = x * x;
    let w = z * z;
    let a = &ARCTANGENT;
    let odd = z * (a[0] + w * (a[2] + w * (a[4] + w * (a[6] + w * (a[8] + w * a[10])))));
    let even = w * (a[1] + w * (a[3] + w * (a[5] + w * (a[7] + w * a[9]))));
    let Some(interval) = interval else {
        return x - x * (odd + even);
    };
    let z = ARCTANGENT_HIGH[interval] - (x * (odd + even) - ARCTANGENT_LOW[interval] - x);
    if negative { -z } else { z }
}

pub fn atan2(y: f64, x: f64) -> f64 {
    if x.is_nan() || y.is_nan() {
        return x + y;
    }
    let x_high = high_word(x);
    let x_low = low_word(x);
    let y_high = high_word(y);
    let y_low = low_word(y);
    if x_high.wrapping_sub(ONE_HIGH_WORD) | x_low == 0 {
        return atan(y);
    }
    let quadrant = (y_high >> 31) & 1 | (x_high >> 30) & 2;
    let x_magnitude = x_high & HIGH_MAGNITUDE_MASK;
    let y_magnitude = y_high & HIGH_MAGNITUDE_MASK;
    if y_magnitude | y_low == 0 {
        return match quadrant {
            0 | 1 => y,
            2 => PI,
            _ => -PI,
        };
    }
    if x_magnitude | x_low == 0 {
        return if quadrant & 1 != 0 {
            -PI / 2.0
        } else {
            PI / 2.0
        };
    }
    if x_magnitude == NON_FINITE_HIGH_WORD {
        let infinite_y = y_magnitude == NON_FINITE_HIGH_WORD;
        return match (quadrant, infinite_y) {
            (0, true) => PI / 4.0,
            (1, true) => -PI / 4.0,
            (2, true) => 3.0 * PI / 4.0,
            (_, true) => -3.0 * PI / 4.0,
            (0, false) => 0.0,
            (1, false) => -0.0,
            (2, false) => PI,
            (_, false) => -PI,
        };
    }
    if x_magnitude.wrapping_add(RATIO_EXPONENT_LIMIT_HIGH_WORD) < y_magnitude
        || y_magnitude == NON_FINITE_HIGH_WORD
    {
        return if quadrant & 1 != 0 {
            -PI / 2.0
        } else {
            PI / 2.0
        };
    }
    let z = if quadrant & 2 != 0
        && y_magnitude.wrapping_add(RATIO_EXPONENT_LIMIT_HIGH_WORD) < x_magnitude
    {
        0.0
    } else {
        atan((y / x).abs())
    };
    match quadrant {
        0 => z,
        1 => -z,
        2 => PI - (z - PI_LOW),
        _ => (z - PI_LOW) - PI,
    }
}
