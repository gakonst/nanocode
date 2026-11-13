use crate::error::JpegError;

const SCALE_BITS: i32 = 512 + 65536 + (128 << 17);
const FIRST_PASS_ROUNDING: i32 = 512;
const FIRST_PASS_SHIFT: u32 = 10;
const SECOND_PASS_SHIFT: u32 = 17;
const BLOCK_ROWS: usize = 8;

pub(crate) fn fits(output_length: usize, position: usize, stride: usize) -> bool {
    stride
        .checked_mul(BLOCK_ROWS - 1)
        .and_then(|span| span.checked_add(position))
        .and_then(|start| start.checked_add(BLOCK_ROWS))
        .is_some_and(|end| end <= output_length)
}

pub(crate) fn transform(
    coefficients: &mut [i32; 64],
    output: &mut [i16],
    position: usize,
    stride: usize,
) -> Result<(), JpegError> {
    if !fits(output.len(), position, stride) {
        return Err(JpegError::LayoutOutOfBounds);
    }
    if coefficients[1..]
        .iter()
        .fold(0, |combined, &coefficient| combined | coefficient)
        == 0
    {
        let value = (coefficients[0].wrapping_add(4 + 1024) >> 3).clamp(0, 255) as i16;
        for row in 0..BLOCK_ROWS {
            let start = position + row * stride;
            output[start..start + 8].fill(value);
        }
        return Ok(());
    }
    if nanocode_sys::jpeg::inverse_dct(coefficients, output, position, stride) {
        return Ok(());
    }
    let mut columns = [0_i32; 64];
    for column in 0..8 {
        let values = core::array::from_fn(|row| coefficients[row * 8 + column]);
        let results = butterfly(values, FIRST_PASS_ROUNDING, FIRST_PASS_SHIFT);
        for (row, result) in results.into_iter().enumerate() {
            columns[column * 8 + row] = result;
        }
    }
    let mut rows = [[0_i16; 8]; 8];
    for (row, target) in rows.iter_mut().enumerate() {
        let values = core::array::from_fn(|column| columns[column * 8 + row]);
        let results = butterfly(values, SCALE_BITS, SECOND_PASS_SHIFT);
        for (sample, result) in target.iter_mut().zip(results) {
            *sample = result.clamp(0, 255) as i16;
        }
    }
    for (row, samples) in rows.iter().enumerate() {
        let start = position + row * stride;
        output[start..start + 8].copy_from_slice(samples);
    }
    Ok(())
}

#[inline(always)]
const fn butterfly(input: [i32; 8], rounding: i32, shift: u32) -> [i32; 8] {
    let p2 = input[2];
    let p3 = input[6];
    let p1 = p2.wrapping_add(p3).wrapping_mul(2217);
    let t2 = p1.wrapping_add(p3.wrapping_mul(-7567));
    let t3 = p1.wrapping_add(p2.wrapping_mul(3135));
    let t0 = input[0].wrapping_add(input[4]).wrapping_shl(12);
    let t1 = input[0].wrapping_sub(input[4]).wrapping_shl(12);
    let x0 = t0.wrapping_add(t3).wrapping_add(rounding);
    let x3 = t0.wrapping_sub(t3).wrapping_add(rounding);
    let x1 = t1.wrapping_add(t2).wrapping_add(rounding);
    let x2 = t1.wrapping_sub(t2).wrapping_add(rounding);
    let mut t0 = input[7];
    let mut t1 = input[5];
    let mut t2 = input[3];
    let mut t3 = input[1];
    let p3 = t0.wrapping_add(t2);
    let p4 = t1.wrapping_add(t3);
    let p1 = t0.wrapping_add(t3);
    let p2 = t1.wrapping_add(t2);
    let p5 = p3.wrapping_add(p4).wrapping_mul(4816);
    t0 = t0.wrapping_mul(1223);
    t1 = t1.wrapping_mul(8410);
    t2 = t2.wrapping_mul(12586);
    t3 = t3.wrapping_mul(6149);
    let p1 = p5.wrapping_add(p1.wrapping_mul(-3685));
    let p2 = p5.wrapping_add(p2.wrapping_mul(-10497));
    let p3 = p3.wrapping_mul(-8034);
    let p4 = p4.wrapping_mul(-1597);
    t3 = t3.wrapping_add(p1.wrapping_add(p4));
    t2 = t2.wrapping_add(p2.wrapping_add(p3));
    t1 = t1.wrapping_add(p2.wrapping_add(p4));
    t0 = t0.wrapping_add(p1.wrapping_add(p3));
    [
        x0.wrapping_add(t3) >> shift,
        x1.wrapping_add(t2) >> shift,
        x2.wrapping_add(t1) >> shift,
        x3.wrapping_add(t0) >> shift,
        x3.wrapping_sub(t0) >> shift,
        x2.wrapping_sub(t1) >> shift,
        x1.wrapping_sub(t2) >> shift,
        x0.wrapping_sub(t3) >> shift,
    ]
}
