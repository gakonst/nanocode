use std::f64::consts::FRAC_2_PI;

use super::logarithm::log;
use super::trigonometric::{cos, sin};
use super::{
    HIGH_MAGNITUDE_MASK, NON_FINITE_HIGH_WORD, high_word, low_word, power_of_two,
    power_of_two_high_word,
};

struct Asymptotic {
    p_numerator: [f64; 6],
    p_denominator: [f64; 5],
    q_numerator: [f64; 6],
    q_denominator: [f64; 6],
}

const ORDER_ZERO_ASYMPTOTIC: [Asymptotic; 4] = [
    Asymptotic {
        p_numerator: [
            0.0,
            -0.07031249999999004,
            -8.081670412753498,
            -257.06310567970485,
            -2485.216410094288,
            -5253.043804907295,
        ],
        p_denominator: [
            116.53436461966818,
            3833.7447536412183,
            40597.857264847255,
            116752.97256437592,
            47627.728414673096,
        ],
        q_numerator: [
            0.0,
            0.0732421874999935,
            11.76820646822527,
            557.6733802564019,
            8859.197207564686,
            37014.62677768878,
        ],
        q_denominator: [
            163.77602689568982,
            8098.344946564498,
            142538.29141912048,
            803309.2571195144,
            840501.5798190605,
            -343899.2935378666,
        ],
    },
    Asymptotic {
        p_numerator: [
            -1.141254646918945e-11,
            -0.07031249408735993,
            -4.159610644705878,
            -67.67476522651673,
            -331.23129964917297,
            -346.4333883656049,
        ],
        p_denominator: [
            60.753938269230034,
            1051.2523059570458,
            5978.970943338558,
            9625.445143577745,
            2406.058159229391,
        ],
        q_numerator: [
            1.8408596359451553e-11,
            0.07324217666126848,
            5.8356350896205695,
            135.11157728644983,
            1027.243765961641,
            1989.9778586460538,
        ],
        q_denominator: [
            82.77661022365378,
            2077.81416421393,
            18847.28877857181,
            56751.11228949473,
            35976.75384251145,
            -5354.342756019448,
        ],
    },
    Asymptotic {
        p_numerator: [
            -2.547046017719519e-09,
            -0.07031196163814817,
            -2.409032215495296,
            -21.96597747348831,
            -58.07917047017376,
            -31.44794705948885,
        ],
        p_denominator: [
            35.85603380552097,
            361.51398305030386,
            1193.6078379211153,
            1127.9967985690741,
            173.58093081333575,
        ],
        q_numerator: [
            4.377410140897386e-09,
            0.07324111800429114,
            3.344231375161707,
            42.621844074541265,
            170.8080913405656,
            166.73394869665117,
        ],
        q_denominator: [
            48.75887297245872,
            709.689221056606,
            3704.1482262011136,
            6460.425167525689,
            2516.3336892036896,
            -149.2474518361564,
        ],
    },
    Asymptotic {
        p_numerator: [
            -8.875343330325264e-08,
            -0.07030309954836247,
            -1.4507384678095299,
            -7.635696138235278,
            -11.193166886035675,
            -3.2336457935133534,
        ],
        p_denominator: [
            22.22029975320888,
            136.2067942182152,
            270.4702786580835,
            153.87539420832033,
            14.65761769482562,
        ],
        q_numerator: [
            1.5044444488698327e-07,
            0.07322342659630793,
            1.99819174093816,
            14.495602934788574,
            31.666231750478154,
            16.252707571092927,
        ],
        q_denominator: [
            30.36558483552192,
            269.34811860804984,
            844.7837575953201,
            882.9358451124886,
            212.66638851179883,
            -5.3109549388266695,
        ],
    },
];
const ORDER_ONE_ASYMPTOTIC: [Asymptotic; 4] = [
    Asymptotic {
        p_numerator: [
            0.0,
            0.11718749999998865,
            13.239480659307358,
            412.05185430737856,
            3874.7453891396053,
            7914.479540318917,
        ],
        p_denominator: [
            114.20737037567841,
            3650.9308342085346,
            36956.206026903346,
            97602.79359349508,
            30804.27206278888,
        ],
        q_numerator: [
            0.0,
            -0.10253906249999271,
            -16.271753454459,
            -759.6017225139501,
            -11849.806670242959,
            -48438.512428575035,
        ],
        q_denominator: [
            161.3953697007229,
            7825.385999233485,
            133875.33628724958,
            719657.7236832409,
            666601.2326177764,
            -294490.26430383464,
        ],
    },
    Asymptotic {
        p_numerator: [
            1.3199051955624352e-11,
            0.1171874931906141,
            6.802751278684329,
            108.30818299018911,
            517.6361395331998,
            528.7152013633375,
        ],
        p_denominator: [
            59.28059872211313,
            991.4014187336144,
            5353.26695291488,
            7844.690317495512,
            1504.0468881036106,
        ],
        q_numerator: [
            -2.089799311417641e-11,
            -0.10253905024137543,
            -8.05644828123936,
            -183.66960747488838,
            -1373.1937606550816,
            -2612.4444045321566,
        ],
        q_denominator: [
            81.27655013843358,
            1991.7987346048596,
            17468.48519249089,
            49851.42709103523,
            27948.075163891812,
            -4719.183547951285,
        ],
    },
    Asymptotic {
        p_numerator: [
            3.025039161373736e-09,
            0.11718686556725359,
            3.9329775003331564,
            35.11940355916369,
            91.05501107507813,
            48.55906851973649,
        ],
        p_denominator: [
            34.79130950012515,
            336.76245874782575,
            1046.8713997577513,
            890.8113463982564,
            103.78793243963928,
        ],
        q_numerator: [
            -5.078312264617666e-09,
            -0.10253782982083709,
            -4.610115811394734,
            -57.847221656278364,
            -228.2445407376317,
            -219.21012847890933,
        ],
        q_denominator: [
            47.66515503237295,
            673.8651126766997,
            3380.1528667952634,
            5547.729097207228,
            1903.119193388108,
            -135.20119144430734,
        ],
    },
    Asymptotic {
        p_numerator: [
            1.0771083010687374e-07,
            0.11717621946268335,
            2.368514966676088,
            12.242610914826123,
            17.693971127168773,
            5.073523125888185,
        ],
        p_denominator: [
            21.43648593638214,
            125.29022716840275,
            232.2764690571628,
            117.6793732871471,
            8.364638933716183,
        ],
        q_numerator: [
            -1.7838172751095887e-07,
            -0.10251704260798555,
            -2.7522056827818746,
            -19.663616264370372,
            -42.32531333728305,
            -21.371921170370406,
        ],
        q_denominator: [
            29.533362906052385,
            252.98154998219053,
            757.5028348686454,
            739.3932053204672,
            155.94900333666612,
            -4.959498988226282,
        ],
    },
];
const ORDER_ZERO_SMALL_NUMERATOR: [f64; 4] = [
    0.015624999999999995,
    -0.00018997929423885472,
    1.8295404953270067e-06,
    -4.618326885321032e-09,
];
const ORDER_ZERO_SMALL_DENOMINATOR: [f64; 4] = [
    0.015619102946489001,
    0.00011692678466333745,
    5.135465502073181e-07,
    1.1661400333379e-09,
];
const ORDER_ZERO_SECOND_NUMERATOR: [f64; 7] = [
    -0.07380429510868723,
    0.17666645250918112,
    -0.01381856719455969,
    0.00034745343209368365,
    -3.8140705372436416e-06,
    1.9559013703502292e-08,
    -3.982051941321034e-11,
];
const ORDER_ZERO_SECOND_DENOMINATOR: [f64; 4] = [
    0.01273048348341237,
    7.600686273503533e-05,
    2.591508518404578e-07,
    4.4111031133267547e-10,
];
const ORDER_ONE_SMALL_NUMERATOR: [f64; 4] = [
    -0.0625,
    0.001407056669551897,
    -1.599556310840356e-05,
    4.9672799960958445e-08,
];
const ORDER_ONE_SMALL_DENOMINATOR: [f64; 5] = [
    0.019153759953836346,
    0.00018594678558863092,
    1.1771846404262368e-06,
    5.0463625707621704e-09,
    1.2354227442613791e-11,
];
const ORDER_ONE_SECOND_NUMERATOR: [f64; 5] = [
    -0.19605709064623894,
    0.05044387166398113,
    -0.0019125689587576355,
    2.352526005616105e-05,
    -9.190991580398789e-08,
];
const ORDER_ONE_SECOND_DENOMINATOR: [f64; 5] = [
    0.01991673182366499,
    0.00020255258102513517,
    1.3560880109751623e-06,
    6.227414523646215e-09,
    1.6655924620799208e-11,
];

const INVERSE_SQRT_PI: f64 = 0.5641895835477563;
const ASYMPTOTIC_EIGHT_HIGH_WORD: u32 = power_of_two_high_word(3);
const ASYMPTOTIC_FIVE_HIGH_WORD: u32 = 0x4012_2e8b;
const ASYMPTOTIC_THREE_HIGH_WORD: u32 = 0x4006_db6d;
const TWO_HIGH_WORD: u32 = power_of_two_high_word(1);
const DOUBLE_OVERFLOW_HIGH_WORD: u32 = power_of_two_high_word(1023);
const ASYMPTOTIC_EXACT_HIGH_WORD: u32 = power_of_two_high_word(129);
const ORDER_ZERO_SERIES_HIGH_WORD: u32 = power_of_two_high_word(-13);
const BESSEL_TINY_HIGH_WORD: u32 = power_of_two_high_word(-127);
const ORDER_ZERO_SECOND_TINY_HIGH_WORD: u32 = power_of_two_high_word(-27);
const ORDER_ONE_SECOND_TINY_HIGH_WORD: u32 = power_of_two_high_word(-54);
const ORDER_ZERO_Q_OFFSET: f64 = -0.125;
const ORDER_ONE_Q_OFFSET: f64 = 0.375;
const RECURRENCE_HUGE_HIGH_WORD: u32 = power_of_two_high_word(302);
const TAYLOR_HIGH_WORD: u32 = power_of_two_high_word(-30);
const TAYLOR_ORDER_MAX: i32 = 32;
const CONTINUED_FRACTION_LIMIT: f64 = 1000000000.0;
const RECURRENCE_OVERFLOW_LOG: f64 = 709.782712893384;
const RECURRENCE_RESCALE: f64 = power_of_two(500);
const NEGATIVE_INFINITY_HIGH_WORD: u32 = 0xfff0_0000;

fn asymptotic_interval(table: &[Asymptotic; 4], x: f64) -> &Asymptotic {
    let magnitude = high_word(x) & HIGH_MAGNITUDE_MASK;
    if magnitude >= ASYMPTOTIC_EIGHT_HIGH_WORD {
        &table[0]
    } else if magnitude >= ASYMPTOTIC_FIVE_HIGH_WORD {
        &table[1]
    } else if magnitude >= ASYMPTOTIC_THREE_HIGH_WORD {
        &table[2]
    } else {
        &table[3]
    }
}

fn asymptotic_p(table: &[Asymptotic; 4], x: f64) -> f64 {
    let interval = asymptotic_interval(table, x);
    let p = &interval.p_numerator;
    let q = &interval.p_denominator;
    let z = 1.0 / (x * x);
    let r = p[0] + z * (p[1] + z * (p[2] + z * (p[3] + z * (p[4] + z * p[5]))));
    let s = 1.0 + z * (q[0] + z * (q[1] + z * (q[2] + z * (q[3] + z * q[4]))));
    1.0 + r / s
}

fn asymptotic_q(table: &[Asymptotic; 4], x: f64, offset: f64) -> f64 {
    let interval = asymptotic_interval(table, x);
    let p = &interval.q_numerator;
    let q = &interval.q_denominator;
    let z = 1.0 / (x * x);
    let r = p[0] + z * (p[1] + z * (p[2] + z * (p[3] + z * (p[4] + z * p[5]))));
    let s = 1.0 + z * (q[0] + z * (q[1] + z * (q[2] + z * (q[3] + z * (q[4] + z * q[5])))));
    (offset + r / s) / x
}

fn order_zero_large(magnitude: u32, x: f64, second_kind: bool) -> f64 {
    let s = sin(x);
    let mut c = cos(x);
    if second_kind {
        c = -c;
    }
    let mut cc = s + c;
    if magnitude < DOUBLE_OVERFLOW_HIGH_WORD {
        let mut ss = s - c;
        let z = -cos(2.0 * x);
        if s * c < 0.0 {
            cc = z / ss;
        } else {
            ss = z / cc;
        }
        if magnitude < ASYMPTOTIC_EXACT_HIGH_WORD {
            if second_kind {
                ss = -ss;
            }
            cc = asymptotic_p(&ORDER_ZERO_ASYMPTOTIC, x) * cc
                - asymptotic_q(&ORDER_ZERO_ASYMPTOTIC, x, ORDER_ZERO_Q_OFFSET) * ss;
        }
    }
    INVERSE_SQRT_PI * cc / x.sqrt()
}

fn order_one_large(magnitude: u32, x: f64, second_kind: bool, negative: bool) -> f64 {
    let mut s = sin(x);
    if second_kind {
        s = -s;
    }
    let c = cos(x);
    let mut cc = s - c;
    if magnitude < DOUBLE_OVERFLOW_HIGH_WORD {
        let mut ss = -s - c;
        let z = cos(2.0 * x);
        if s * c > 0.0 {
            cc = z / ss;
        } else {
            ss = z / cc;
        }
        if magnitude < ASYMPTOTIC_EXACT_HIGH_WORD {
            if second_kind {
                ss = -ss;
            }
            cc = asymptotic_p(&ORDER_ONE_ASYMPTOTIC, x) * cc
                - asymptotic_q(&ORDER_ONE_ASYMPTOTIC, x, ORDER_ONE_Q_OFFSET) * ss;
        }
    }
    if negative {
        cc = -cc;
    }
    INVERSE_SQRT_PI * cc / x.sqrt()
}

pub fn j0(x: f64) -> f64 {
    let magnitude = high_word(x) & HIGH_MAGNITUDE_MASK;
    if magnitude >= NON_FINITE_HIGH_WORD {
        return 1.0 / (x * x);
    }
    let x = x.abs();
    if magnitude >= TWO_HIGH_WORD {
        return order_zero_large(magnitude, x, false);
    }
    if magnitude >= ORDER_ZERO_SERIES_HIGH_WORD {
        let z = x * x;
        let p = &ORDER_ZERO_SMALL_NUMERATOR;
        let q = &ORDER_ZERO_SMALL_DENOMINATOR;
        let r = z * (p[0] + z * (p[1] + z * (p[2] + z * p[3])));
        let s = 1.0 + z * (q[0] + z * (q[1] + z * (q[2] + z * q[3])));
        return (1.0 + x / 2.0) * (1.0 - x / 2.0) + z * (r / s);
    }
    let x = if magnitude >= BESSEL_TINY_HIGH_WORD {
        0.25 * x * x
    } else {
        x
    };
    1.0 - x
}

pub fn y0(x: f64) -> f64 {
    let high = high_word(x);
    if (high << 1) | low_word(x) == 0 {
        return -1.0 / 0.0;
    }
    if high >> 31 != 0 {
        return super::quotient(0.0, 0.0);
    }
    if high >= NON_FINITE_HIGH_WORD {
        return 1.0 / x;
    }
    if high >= TWO_HIGH_WORD {
        return order_zero_large(high, x, true);
    }
    if high >= ORDER_ZERO_SECOND_TINY_HIGH_WORD {
        let z = x * x;
        let u = &ORDER_ZERO_SECOND_NUMERATOR;
        let v = &ORDER_ZERO_SECOND_DENOMINATOR;
        let numerator =
            u[0] + z * (u[1] + z * (u[2] + z * (u[3] + z * (u[4] + z * (u[5] + z * u[6])))));
        let denominator = 1.0 + z * (v[0] + z * (v[1] + z * (v[2] + z * v[3])));
        return numerator / denominator + FRAC_2_PI * (j0(x) * log(x));
    }
    ORDER_ZERO_SECOND_NUMERATOR[0] + FRAC_2_PI * log(x)
}

pub fn j1(x: f64) -> f64 {
    let high = high_word(x);
    let negative = high >> 31 != 0;
    let magnitude = high & HIGH_MAGNITUDE_MASK;
    if magnitude >= NON_FINITE_HIGH_WORD {
        return 1.0 / (x * x);
    }
    if magnitude >= TWO_HIGH_WORD {
        return order_one_large(magnitude, x.abs(), false, negative);
    }
    let z = if magnitude >= BESSEL_TINY_HIGH_WORD {
        let z = x * x;
        let p = &ORDER_ONE_SMALL_NUMERATOR;
        let q = &ORDER_ONE_SMALL_DENOMINATOR;
        let r = z * (p[0] + z * (p[1] + z * (p[2] + z * p[3])));
        let s = 1.0 + z * (q[0] + z * (q[1] + z * (q[2] + z * (q[3] + z * q[4]))));
        r / s
    } else {
        x
    };
    (0.5 + z) * x
}

pub fn y1(x: f64) -> f64 {
    let high = high_word(x);
    if (high << 1) | low_word(x) == 0 {
        return -1.0 / 0.0;
    }
    if high >> 31 != 0 {
        return super::quotient(0.0, 0.0);
    }
    if high >= NON_FINITE_HIGH_WORD {
        return 1.0 / x;
    }
    if high >= TWO_HIGH_WORD {
        return order_one_large(high, x, true, false);
    }
    if high < ORDER_ONE_SECOND_TINY_HIGH_WORD {
        return -FRAC_2_PI / x;
    }
    let z = x * x;
    let u = &ORDER_ONE_SECOND_NUMERATOR;
    let v = &ORDER_ONE_SECOND_DENOMINATOR;
    let numerator = u[0] + z * (u[1] + z * (u[2] + z * (u[3] + z * u[4])));
    let denominator = 1.0 + z * (v[0] + z * (v[1] + z * (v[2] + z * (v[3] + z * v[4]))));
    x * (numerator / denominator) + FRAC_2_PI * (j1(x) * log(x) - 1.0 / x)
}

fn is_nan_words(magnitude: u32, low: u32) -> bool {
    magnitude | ((low | low.wrapping_neg()) >> 31) > NON_FINITE_HIGH_WORD
}

fn first_kind_forward(order_below: i32, x: f64) -> f64 {
    let mut a = j0(x);
    let mut b = j1(x);
    let mut i = 0;
    while i < order_below {
        i += 1;
        let previous = b;
        b = b * (2.0 * f64::from(i) / x) - a;
        a = previous;
    }
    b
}

fn first_kind_taylor(order_below: i32, x: f64) -> f64 {
    if order_below > TAYLOR_ORDER_MAX {
        return 0.0;
    }
    let half = x * 0.5;
    let mut b = half;
    let mut a = 1.0;
    for i in 2..=order_below + 1 {
        a *= f64::from(i);
        b *= half;
    }
    b / a
}

fn first_kind_backward(order_below: i32, x: f64) -> f64 {
    let nf = f64::from(order_below) + 1.0;
    let w = 2.0 * nf / x;
    let h = 2.0 / x;
    let mut z = w + h;
    let mut q0 = w;
    let mut q1 = w * z - 1.0;
    let mut k = 1;
    while q1 < CONTINUED_FRACTION_LIMIT {
        k += 1;
        z += h;
        let next = z * q1 - q0;
        q0 = q1;
        q1 = next;
    }
    let mut t = 0.0;
    for i in (0..=k).rev() {
        t = 1.0 / (2.0 * (f64::from(i) + nf) / x - t);
    }
    let mut a = t;
    let mut b = 1.0;
    let rescale = nf * log(w.abs()) >= RECURRENCE_OVERFLOW_LOG;
    for i in (1..=order_below).rev() {
        let previous = b;
        b = b * (2.0 * f64::from(i)) / x - a;
        a = previous;
        if rescale && b > RECURRENCE_RESCALE {
            a /= b;
            t /= b;
            b = 1.0;
        }
    }
    let z = j0(x);
    let w = j1(x);
    if z.abs() >= w.abs() {
        t * z / b
    } else {
        t * w / a
    }
}

pub fn jn(n: i32, x: f64) -> f64 {
    let high = high_word(x);
    let low = low_word(x);
    let mut negative = high >> 31 != 0;
    let magnitude = high & HIGH_MAGNITUDE_MASK;
    if is_nan_words(magnitude, low) {
        return x;
    }
    if n == 0 {
        return j0(x);
    }
    let (order_below, x) = if n < 0 {
        negative = !negative;
        (-(n + 1), -x)
    } else {
        (n - 1, x)
    };
    if order_below == 0 {
        return j1(x);
    }
    negative &= n & 1 != 0;
    let x = x.abs();
    let b = if magnitude | low == 0 || magnitude == NON_FINITE_HIGH_WORD {
        0.0
    } else if f64::from(order_below) < x {
        if magnitude >= RECURRENCE_HUGE_HIGH_WORD {
            let temp = match order_below & 3 {
                0 => -cos(x) + sin(x),
                1 => -cos(x) - sin(x),
                2 => cos(x) - sin(x),
                _ => cos(x) + sin(x),
            };
            INVERSE_SQRT_PI * temp / x.sqrt()
        } else {
            first_kind_forward(order_below, x)
        }
    } else if magnitude < TAYLOR_HIGH_WORD {
        first_kind_taylor(order_below, x)
    } else {
        first_kind_backward(order_below, x)
    };
    if negative { -b } else { b }
}

pub fn yn(n: i32, x: f64) -> f64 {
    let high = high_word(x);
    let low = low_word(x);
    let magnitude = high & HIGH_MAGNITUDE_MASK;
    if is_nan_words(magnitude, low) {
        return x;
    }
    if high >> 31 != 0 && magnitude | low != 0 {
        return super::quotient(0.0, 0.0);
    }
    if magnitude == NON_FINITE_HIGH_WORD {
        return 0.0;
    }
    if n == 0 {
        return y0(x);
    }
    let (order_below, negative) = if n < 0 {
        (-(n + 1), n & 1 != 0)
    } else {
        (n - 1, false)
    };
    if order_below == 0 {
        return if negative { -y1(x) } else { y1(x) };
    }
    let b = if magnitude >= RECURRENCE_HUGE_HIGH_WORD {
        let temp = match order_below & 3 {
            0 => -sin(x) - cos(x),
            1 => -sin(x) + cos(x),
            2 => sin(x) + cos(x),
            _ => sin(x) - cos(x),
        };
        INVERSE_SQRT_PI * temp / x.sqrt()
    } else {
        let mut a = y0(x);
        let mut b = y1(x);
        let mut i = 0;
        while i < order_below && high_word(b) != NEGATIVE_INFINITY_HIGH_WORD {
            i += 1;
            let previous = b;
            b = (2.0 * f64::from(i) / x) * b - a;
            a = previous;
        }
        b
    };
    if negative { -b } else { b }
}
