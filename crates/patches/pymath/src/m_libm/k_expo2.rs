use super::exp;

const K: i32 = 2043;

pub(crate) fn k_expo2(x: f64) -> f64 {
    let k_ln2 = f64::from_bits(0x40962066151add8b);

    let scale = f64::from_bits(((((0x3ff + K / 2) as u32) << 20) as u64) << 32);

    exp(x - k_ln2) * scale * scale
}
