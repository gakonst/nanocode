use super::SIGN_MASK;

pub fn fabs(x: f64) -> f64 {
    f64::from_bits(x.to_bits() & !SIGN_MASK)
}
