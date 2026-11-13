use super::{exp, fabs, get_high_word, with_set_low_word};

const ERX: f64 = 8.45062911510467529297e-01;

const EFX8: f64 = 1.02703333676410069053e+00;
const PP0: f64 = 1.28379167095512558561e-01;
const PP1: f64 = -3.25042107247001499370e-01;
const PP2: f64 = -2.84817495755985104766e-02;
const PP3: f64 = -5.77027029648944159157e-03;
const PP4: f64 = -2.37630166566501626084e-05;
const QQ1: f64 = 3.97917223959155352819e-01;
const QQ2: f64 = 6.50222499887672944485e-02;
const QQ3: f64 = 5.08130628187576562776e-03;
const QQ4: f64 = 1.32494738004321644526e-04;
const QQ5: f64 = -3.96022827877536812320e-06;

const PA0: f64 = -2.36211856075265944077e-03;
const PA1: f64 = 4.14856118683748331666e-01;
const PA2: f64 = -3.72207876035701323847e-01;
const PA3: f64 = 3.18346619901161753674e-01;
const PA4: f64 = -1.10894694282396677476e-01;
const PA5: f64 = 3.54783043256182359371e-02;
const PA6: f64 = -2.16637559486879084300e-03;
const QA1: f64 = 1.06420880400844228286e-01;
const QA2: f64 = 5.40397917702171048937e-01;
const QA3: f64 = 7.18286544141962662868e-02;
const QA4: f64 = 1.26171219808761642112e-01;
const QA5: f64 = 1.36370839120290507362e-02;
const QA6: f64 = 1.19844998467991074170e-02;

const RA0: f64 = -9.86494403484714822705e-03;
const RA1: f64 = -6.93858572707181764372e-01;
const RA2: f64 = -1.05586262253232909814e+01;
const RA3: f64 = -6.23753324503260060396e+01;
const RA4: f64 = -1.62396669462573470355e+02;
const RA5: f64 = -1.84605092906711035994e+02;
const RA6: f64 = -8.12874355063065934246e+01;
const RA7: f64 = -9.81432934416914548592e+00;
const SA1: f64 = 1.96512716674392571292e+01;
const SA2: f64 = 1.37657754143519042600e+02;
const SA3: f64 = 4.34565877475229228821e+02;
const SA4: f64 = 6.45387271733267880336e+02;
const SA5: f64 = 4.29008140027567833386e+02;
const SA6: f64 = 1.08635005541779435134e+02;
const SA7: f64 = 6.57024977031928170135e+00;
const SA8: f64 = -6.04244152148580987438e-02;

const RB0: f64 = -9.86494292470009928597e-03;
const RB1: f64 = -7.99283237680523006574e-01;
const RB2: f64 = -1.77579549177547519889e+01;
const RB3: f64 = -1.60636384855821916062e+02;
const RB4: f64 = -6.37566443368389627722e+02;
const RB5: f64 = -1.02509513161107724954e+03;
const RB6: f64 = -4.83519191608651397019e+02;
const SB1: f64 = 3.03380607434824582924e+01;
const SB2: f64 = 3.25792512996573918826e+02;
const SB3: f64 = 1.53672958608443695994e+03;
const SB4: f64 = 3.19985821950859553908e+03;
const SB5: f64 = 2.55305040643316442583e+03;
const SB6: f64 = 4.74528541206955367215e+02;
const SB7: f64 = -2.24409524465858183362e+01;

fn erfc1(x: f64) -> f64 {
    let s: f64;
    let p: f64;
    let q: f64;

    s = fabs(x) - 1.0;
    p = PA0 + s * (PA1 + s * (PA2 + s * (PA3 + s * (PA4 + s * (PA5 + s * PA6)))));
    q = 1.0 + s * (QA1 + s * (QA2 + s * (QA3 + s * (QA4 + s * (QA5 + s * QA6)))));

    1.0 - ERX - p / q
}

fn erfc2(ix: u32, mut x: f64) -> f64 {
    let s: f64;
    let r: f64;
    let big_s: f64;
    let z: f64;

    if ix < 0x3ff40000 {
        return erfc1(x);
    }

    x = fabs(x);
    s = 1.0 / (x * x);
    if ix < 0x4006db6d {
        r = RA0 + s * (RA1 + s * (RA2 + s * (RA3 + s * (RA4 + s * (RA5 + s * (RA6 + s * RA7))))));
        big_s = 1.0
            + s * (SA1
                + s * (SA2 + s * (SA3 + s * (SA4 + s * (SA5 + s * (SA6 + s * (SA7 + s * SA8)))))));
    } else {
        r = RB0 + s * (RB1 + s * (RB2 + s * (RB3 + s * (RB4 + s * (RB5 + s * RB6)))));
        big_s =
            1.0 + s * (SB1 + s * (SB2 + s * (SB3 + s * (SB4 + s * (SB5 + s * (SB6 + s * SB7))))));
    }
    z = with_set_low_word(x, 0);

    exp(-z * z - 0.5625) * exp((z - x) * (z + x) + r / big_s) / x
}

pub fn erf(x: f64) -> f64 {
    let r: f64;
    let s: f64;
    let z: f64;
    let y: f64;
    let mut ix: u32;
    let sign: usize;

    ix = get_high_word(x);
    sign = (ix >> 31) as usize;
    ix &= 0x7fffffff;
    if ix >= 0x7ff00000 {
        return 1.0 - 2.0 * (sign as f64) + 1.0 / x;
    }
    if ix < 0x3feb0000 {
        if ix < 0x3e300000 {
            return 0.125 * (8.0 * x + EFX8 * x);
        }
        z = x * x;
        r = PP0 + z * (PP1 + z * (PP2 + z * (PP3 + z * PP4)));
        s = 1.0 + z * (QQ1 + z * (QQ2 + z * (QQ3 + z * (QQ4 + z * QQ5))));
        y = r / s;
        return x + x * y;
    }
    if ix < 0x40180000 {
        y = 1.0 - erfc2(ix, x);
    } else {
        let x1p_1022 = f64::from_bits(0x0010000000000000);
        y = 1.0 - x1p_1022;
    }

    if sign != 0 { -y } else { y }
}

pub fn erfc(x: f64) -> f64 {
    let r: f64;
    let s: f64;
    let z: f64;
    let y: f64;
    let mut ix: u32;
    let sign: usize;

    ix = get_high_word(x);
    sign = (ix >> 31) as usize;
    ix &= 0x7fffffff;
    if ix >= 0x7ff00000 {
        return 2.0 * (sign as f64) + 1.0 / x;
    }
    if ix < 0x3feb0000 {
        if ix < 0x3c700000 {
            return 1.0 - x;
        }
        z = x * x;
        r = PP0 + z * (PP1 + z * (PP2 + z * (PP3 + z * PP4)));
        s = 1.0 + z * (QQ1 + z * (QQ2 + z * (QQ3 + z * (QQ4 + z * QQ5))));
        y = r / s;
        if sign != 0 || ix < 0x3fd00000 {
            return 1.0 - (x + x * y);
        }
        return 0.5 - (x - 0.5 + x * y);
    }
    if ix < 0x403c0000 {
        if sign != 0 {
            return 2.0 - erfc2(ix, x);
        } else {
            return erfc2(ix, x);
        }
    }

    let x1p_1022 = f64::from_bits(0x0010000000000000);
    if sign != 0 {
        2.0 - x1p_1022
    } else {
        x1p_1022 * x1p_1022
    }
}
