static T: [f64; 13] = [
    3.33333333333334091986e-01,
    1.33333333333201242699e-01,
    5.39682539762260521377e-02,
    2.18694882948595424599e-02,
    8.86323982359930005737e-03,
    3.59207910759131235356e-03,
    1.45620945432529025516e-03,
    5.88041240820264096874e-04,
    2.46463134818469906812e-04,
    7.81794442939557092300e-05,
    7.14072491382608190305e-05,
    -1.85586374855275456654e-05,
    2.59073051863633712884e-05,
];
const PIO4: f64 = 7.85398163397448278999e-01;
const PIO4_LO: f64 = 3.06161699786838301793e-17;

pub(crate) fn k_tan(mut x: f64, mut y: f64, odd: i32) -> f64 {
    let hx = (f64::to_bits(x) >> 32) as u32;
    let big = (hx & 0x7fffffff) >= 0x3FE59428;
    if big {
        let sign = hx >> 31;
        if sign != 0 {
            x = -x;
            y = -y;
        }
        x = (PIO4 - x) + (PIO4_LO - y);
        y = 0.0;
    }
    let z = x * x;
    let w = z * z;

    let r = T[1] + w * (T[3] + w * (T[5] + w * (T[7] + w * (T[9] + w * T[11]))));
    let v = z * (T[2] + w * (T[4] + w * (T[6] + w * (T[8] + w * (T[10] + w * T[12])))));
    let s = z * x;
    let r = y + z * (s * (r + v) + y) + s * T[0];
    let w = x + r;
    if big {
        let sign = hx >> 31;
        let s = 1.0 - 2.0 * odd as f64;
        let v = s - 2.0 * (x + (r - w * w / (w + s)));
        return if sign != 0 { -v } else { v };
    }
    if odd == 0 {
        return w;
    }

    let w0 = zero_low_word(w);
    let v = r - (w0 - x);
    let a = -1.0 / w;
    let a0 = zero_low_word(a);
    a0 + a * (1.0 + a0 * w0 + a0 * v)
}

fn zero_low_word(x: f64) -> f64 {
    f64::from_bits(f64::to_bits(x) & 0xFFFF_FFFF_0000_0000)
}
