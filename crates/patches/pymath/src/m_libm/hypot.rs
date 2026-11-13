use super::sqrt;

const SPLIT: f64 = 134217728. + 1.;

fn sq(x: f64) -> (f64, f64) {
    let xh: f64;
    let xl: f64;
    let xc: f64;

    xc = x * SPLIT;
    xh = x - xc + xc;
    xl = x - xh;
    let hi = x * x;
    let lo = xh * xh - hi + 2. * xh * xl + xl * xl;
    (hi, lo)
}

pub fn hypot(mut x: f64, mut y: f64) -> f64 {
    let x1p700 = f64::from_bits(0x6bb0000000000000);
    let x1p_700 = f64::from_bits(0x1430000000000000);

    let mut uxi = x.to_bits();
    let mut uyi = y.to_bits();
    let uti;
    let ex: i64;
    let ey: i64;
    let mut z: f64;

    uxi &= -1i64 as u64 >> 1;
    uyi &= -1i64 as u64 >> 1;
    if uxi < uyi {
        uti = uxi;
        uxi = uyi;
        uyi = uti;
    }

    ex = (uxi >> 52) as i64;
    ey = (uyi >> 52) as i64;
    x = f64::from_bits(uxi);
    y = f64::from_bits(uyi);

    if ey == 0x7ff {
        return y;
    }
    if ex == 0x7ff || uyi == 0 {
        return x;
    }

    if ex - ey > 64 {
        return x + y;
    }

    z = 1.;
    if ex > 0x3ff + 510 {
        z = x1p700;
        x *= x1p_700;
        y *= x1p_700;
    } else if ey < 0x3ff - 450 {
        z = x1p_700;
        x *= x1p700;
        y *= x1p700;
    }
    let (hx, lx) = sq(x);
    let (hy, ly) = sq(y);
    z * sqrt(ly + lx + hy + hx)
}
