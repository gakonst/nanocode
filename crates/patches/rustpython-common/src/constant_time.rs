#[must_use]
pub fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let difference = left
        .iter()
        .zip(right)
        .fold(0u8, |difference, (left, right)| {
            difference | core::hint::black_box(left ^ right)
        });
    core::hint::black_box(difference) == 0
}
