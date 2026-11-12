use super::{
    EXPONENT_BIAS, HIGH_SIGNIFICAND_BITS, HIGH_SIGNIFICAND_MASK, NON_FINITE_HIGH_WORD,
    ONE_HIGH_WORD, SMALLEST_NORMAL_HIGH_WORD, WORD_BITS, from_words, power_of_two,
    power_of_two_high_word, with_low_word,
};

const LN2_HIGH: f64 = 0.6931471803691238;
const LN2_LOW: f64 = 1.9082149292705877e-10;
const LOG: [f64; 7] = [
    0.6666666666666735,
    0.3999999999940942,
    0.2857142874366239,
    0.22222198432149784,
    0.1818357216161805,
    0.15313837699209373,
    0.14798198605116586,
];
const INVERSE_LN2_HIGH: f64 = 1.4426950407214463;
const INVERSE_LN2_LOW: f64 = 1.6751713164886512e-10;
const INVERSE_LN10_HIGH: f64 = 0.4342944818781689;
const INVERSE_LN10_LOW: f64 = 2.5082946711645275e-11;
const LOG10_2_HIGH: f64 = 0.30102999566361177;
const LOG10_2_LOW: f64 = 3.694239077158931e-13;
const SUBNORMAL_SCALE_EXPONENT: i32 = 54;
const SQRT_HALF_HIGH_WORD: u32 = 0x3fe6_a09e;
const LOG1P_SQRT_2_MINUS_1_HIGH_WORD: u32 = 0x3fda_827a;
const LOG1P_MINUS_ONE_HIGH_WORD: u32 = 0xbff0_0000;
const LOG1P_TINY_HIGH_WORD: u32 = power_of_two_high_word(-53);
const LOG1P_SQRT_HALF_MINUS_1_HIGH_WORD: u32 = 0xbfd2_bec4;
const LOG1P_CORRECTION_SCALE_LIMIT: i32 = 54;
const LOW_WORD_MASK: u64 = 0xffff_ffff;

struct Reduced {
    f: f64,
    half_f_squared: f64,
    s: f64,
    r: f64,
    k: i32,
}

fn polynomial(f: f64, k: i32) -> Reduced {
    let half_f_squared = 0.5 * f * f;
    let s = f / (2.0 + f);
    let z = s * s;
    let w = z * z;
    let t1 = w * (LOG[1] + w * (LOG[3] + w * LOG[5]));
    let t2 = z * (LOG[0] + w * (LOG[2] + w * (LOG[4] + w * LOG[6])));
    Reduced {
        f,
        half_f_squared,
        s,
        r: t2 + t1,
        k,
    }
}

fn fold_high_word(high: u32) -> (u32, i32) {
    let shifted = high + (ONE_HIGH_WORD - SQRT_HALF_HIGH_WORD);
    let k = (shifted >> HIGH_SIGNIFICAND_BITS) as i32 - EXPONENT_BIAS;
    ((shifted & HIGH_SIGNIFICAND_MASK) + SQRT_HALF_HIGH_WORD, k)
}

fn reduce(x: f64) -> Result<Reduced, f64> {
    let mut x = x;
    let mut bits = x.to_bits();
    let mut high = (bits >> WORD_BITS) as u32;
    let mut k = 0;
    if high < SMALLEST_NORMAL_HIGH_WORD || high >> 31 != 0 {
        if bits << 1 == 0 {
            return Err(-1.0 / (x * x));
        }
        if high >> 31 != 0 {
            return Err(super::difference(x, x) / 0.0);
        }
        k -= SUBNORMAL_SCALE_EXPONENT;
        x *= power_of_two(SUBNORMAL_SCALE_EXPONENT);
        bits = x.to_bits();
        high = (bits >> WORD_BITS) as u32;
    } else if high >= NON_FINITE_HIGH_WORD {
        return Err(x);
    } else if high == ONE_HIGH_WORD && bits << WORD_BITS == 0 {
        return Err(0.0);
    }
    let (high, exponent) = fold_high_word(high);
    let x = from_words(high, (bits & LOW_WORD_MASK) as u32);
    Ok(polynomial(x - 1.0, k + exponent))
}

pub fn log(x: f64) -> f64 {
    let reduced = match reduce(x) {
        Ok(reduced) => reduced,
        Err(special) => return special,
    };
    let Reduced {
        f,
        half_f_squared,
        s,
        r,
        k,
    } = reduced;
    let dk = f64::from(k);
    s * (half_f_squared + r) + dk * LN2_LOW - half_f_squared + f + dk * LN2_HIGH
}

fn split(reduced: &Reduced) -> (f64, f64) {
    let Reduced {
        f,
        half_f_squared,
        s,
        r,
        ..
    } = *reduced;
    let high = with_low_word(f - half_f_squared, 0);
    let low = f - high - half_f_squared + s * (half_f_squared + r);
    (high, low)
}

fn add_exact(y: f64, value_high: f64, value_low: f64) -> f64 {
    let w = y + value_high;
    let value_low = value_low + ((y - w) + value_high);
    value_low + w
}

pub fn log2(x: f64) -> f64 {
    let reduced = match reduce(x) {
        Ok(reduced) => reduced,
        Err(special) => return special,
    };
    let (high, low) = split(&reduced);
    let value_high = high * INVERSE_LN2_HIGH;
    let value_low = (low + high) * INVERSE_LN2_LOW + low * INVERSE_LN2_HIGH;
    add_exact(f64::from(reduced.k), value_high, value_low)
}

pub fn log10(x: f64) -> f64 {
    let reduced = match reduce(x) {
        Ok(reduced) => reduced,
        Err(special) => return special,
    };
    let (high, low) = split(&reduced);
    let value_high = high * INVERSE_LN10_HIGH;
    let dk = f64::from(reduced.k);
    let y = dk * LOG10_2_HIGH;
    let value_low = dk * LOG10_2_LOW + (low + high) * INVERSE_LN10_LOW + low * INVERSE_LN10_HIGH;
    add_exact(y, value_high, value_low)
}

fn log1p_reduce(x: f64) -> (f64, f64, i32) {
    let u = 1.0 + x;
    let bits = u.to_bits();
    let (high, k) = fold_high_word((bits >> WORD_BITS) as u32);
    let c = if k < LOG1P_CORRECTION_SCALE_LIMIT {
        let c = if k >= 2 { 1.0 - (u - x) } else { x - (u - 1.0) };
        c / u
    } else {
        0.0
    };
    (from_words(high, (bits & LOW_WORD_MASK) as u32) - 1.0, c, k)
}

pub fn log1p(x: f64) -> f64 {
    let high = (x.to_bits() >> WORD_BITS) as u32;
    let mut small = false;
    if high < LOG1P_SQRT_2_MINUS_1_HIGH_WORD || high >> 31 != 0 {
        if high >= LOG1P_MINUS_ONE_HIGH_WORD {
            if x == -1.0 {
                return x / 0.0;
            }
            return super::difference(x, x) / 0.0;
        }
        if high << 1 < LOG1P_TINY_HIGH_WORD << 1 {
            return x;
        }
        small = high <= LOG1P_SQRT_HALF_MINUS_1_HIGH_WORD;
    } else if high >= NON_FINITE_HIGH_WORD {
        return x;
    }
    let (f, c, k) = if small { (x, 0.0, 0) } else { log1p_reduce(x) };
    let Reduced {
        f,
        half_f_squared,
        s,
        r,
        k,
    } = polynomial(f, k);
    let dk = f64::from(k);
    s * (half_f_squared + r) + (dk * LN2_LOW + c) - half_f_squared + f + dk * LN2_HIGH
}
