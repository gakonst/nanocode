use super::{combine_words, exp};

pub(crate) fn expo2(x: f64) -> f64 {
    const K: i32 = 2043;
    let kln2 = f64::from_bits(0x40962066151add8b);

    let scale = combine_words(((0x3ff + K / 2) as u32) << 20, 0);

    exp(x - kln2) * scale * scale
}
