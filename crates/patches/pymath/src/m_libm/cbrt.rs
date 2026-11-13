use super::{EXPONENT_SATURATED, SIGN_MASK, SIGNIFICAND_BITS, SIGNIFICAND_MASK};

pub fn cbrt(x: f64) -> f64 {
    const ESCALE: [f64; 3] = [
        1.0,
        f64::from_bits(0x3ff428a2f98d728b),
        f64::from_bits(0x3ff965fea53d6e3d),
    ];

    const C: [f64; 4] = [
        f64::from_bits(0x3fe1b0babccfef9c),
        f64::from_bits(0x3fe2c9a3e94d1da5),
        f64::from_bits(0xbfc4dc30b1a1ddba),
        f64::from_bits(0x3f97a8d3e4ec9b07),
    ];

    let u0: f64 = f64::from_bits(0x3fd5555555555555);
    let u1: f64 = f64::from_bits(0x3fcc71c71c71c71c);

    let rsc = [1.0, -1.0, 0.5, -0.5, 0.25, -0.25];

    let off = [f64::from_bits(0x3ca0000000000000), 0.0, 0.0, 0.0];

    let hx: u64 = x.to_bits();
    let mut mant: u64 = hx & SIGNIFICAND_MASK;
    let sign: u64 = hx >> 63;

    let mut e: u32 = (hx >> SIGNIFICAND_BITS) as u32 & EXPONENT_SATURATED;

    if ((e + 1) & EXPONENT_SATURATED) < 2 {
        let ix: u64 = hx & !SIGN_MASK;

        if e == EXPONENT_SATURATED || ix == 0 {
            return x + x;
        }

        let nz = ix.leading_zeros() - 11;
        mant <<= nz;
        mant &= SIGNIFICAND_MASK;
        e = e.wrapping_sub(nz - 1);
    }

    e = e.wrapping_add(3072);
    let cvt1: u64 = mant | (0x3ffu64 << 52);
    let mut cvt5: u64 = cvt1;

    let et: u32 = e / 3;
    let it: u32 = e % 3;

    cvt5 += u64::from(it) << SIGNIFICAND_BITS;
    cvt5 |= sign << 63;
    let zz: f64 = f64::from_bits(cvt5);

    let mut isc: u64 = ESCALE[it as usize].to_bits();
    isc |= sign << 63;
    let cvt2: u64 = isc;
    let z: f64 = f64::from_bits(cvt1);

    let r: f64 = 1.0 / z;
    let rr: f64 = r * rsc[((it as usize) << 1) | sign as usize];
    let z2: f64 = z * z;
    let c0: f64 = C[0] + z * C[1];
    let c2: f64 = C[2] + z * C[3];
    let mut y: f64 = c0 + z2 * c2;
    let mut y2: f64 = y * y;

    let mut h: f64 = y2 * (y * r) - 1.0;

    y -= (h * y) * (u0 - u1 * h);

    y *= f64::from_bits(cvt2);

    y2 = y * y;
    let mut y2l: f64 = y.mul_add(y, -y2);

    let mut y3: f64 = y2 * y;
    let mut y3l: f64 = y.mul_add(y2, -y3) + y * y2l;

    h = ((y3 - zz) + y3l) * rr;
    let mut dy: f64 = h * (y * u0);

    let mut y1: f64 = y - dy;
    dy = (y - y1) - dy;

    let mut ady: f64 = dy.abs();

    let mut ady0: f64 = (ady - off[0]).abs();
    let mut ady1: f64 = (ady - (f64::from_bits(0x3cb0000000000000) + off[0])).abs();

    if ady0 < f64::from_bits(0x3b40000000000000) || ady1 < f64::from_bits(0x3b40000000000000) {
        y2 = y1 * y1;
        y2l = y1.mul_add(y1, -y2);
        y3 = y2 * y1;
        y3l = y1.mul_add(y2, -y3) + y1 * y2l;
        h = ((y3 - zz) + y3l) * rr;
        dy = h * (y1 * u0);
        y = y1 - dy;
        dy = (y1 - y) - dy;
        y1 = y;
        ady = dy.abs();
        ady0 = (ady - off[0]).abs();
        ady1 = (ady - (f64::from_bits(0x3cb0000000000000) + off[0])).abs();

        if ady0 < f64::from_bits(0x39d0000000000000) || ady1 < f64::from_bits(0x39d0000000000000) {
            let azz: f64 = zz.abs();

            if azz == f64::from_bits(0x4009b78223aa307c) {
                y1 = f64::from_bits(0x3ff79d15d0e8d59c).copysign(zz);
            }

            if azz == f64::from_bits(0x401a202bfc89ddff) {
                y1 = f64::from_bits(0x3ffde87aa837820f).copysign(zz);
            }
        }
    }

    let mut cvt3: u64 = y1.to_bits();
    cvt3 = cvt3.wrapping_add(((et.wrapping_sub(342).wrapping_sub(1023)) as u64) << 52);
    let m0: u64 = cvt3 << 30;
    let m1 = m0 >> 63;

    if (m0 ^ m1) <= (1u64 << 30) {
        let mut cvt4: u64 = y1.to_bits();
        cvt4 = (cvt4 + (164 << 15)) & 0xffffffffffff0000u64;

        if ((f64::from_bits(cvt4) - y1) - dy).abs() < f64::from_bits(0x3c30000000000000)
            || (zz).abs() == 1.0
        {
            cvt3 = (cvt3 + (1u64 << 15)) & 0xffffffffffff0000u64;
        }
    }

    f64::from_bits(cvt3)
}
