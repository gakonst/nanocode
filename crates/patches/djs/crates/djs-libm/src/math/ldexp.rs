pub fn ldexp(x: f64, n: i32) -> f64 {
    super::scalbn(x, n)
}
