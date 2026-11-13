use super::expm1;

pub fn tanh(mut x: f64) -> f64 {
    let mut uf: f64 = x;
    let mut ui: u64 = f64::to_bits(uf);

    let w: u32;
    let sign: bool;
    let mut t: f64;

    sign = ui >> 63 != 0;
    ui &= !1 / 2;
    uf = f64::from_bits(ui);
    x = uf;
    w = (ui >> 32) as u32;

    if w > 0x3fe193ea {
        if w > 0x40340000 {
            t = 1.0 - 0.0 / x;
        } else {
            t = expm1(2.0 * x);
            t = 1.0 - 2.0 / (t + 2.0);
        }
    } else if w > 0x3fd058ae {
        t = expm1(2.0 * x);
        t = t / (t + 2.0);
    } else if w >= 0x00100000 {
        t = expm1(-2.0 * x);
        t = -t / (t + 2.0);
    } else {
        t = x;
    }

    if sign { -t } else { t }
}
