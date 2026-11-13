use super::SIGN_MASK;

pub fn copysign(x: f64, y: f64) -> f64 {
    let mut ux = x.to_bits();
    let uy = y.to_bits();
    ux &= !SIGN_MASK;
    ux |= uy & SIGN_MASK;
    f64::from_bits(ux)
}
