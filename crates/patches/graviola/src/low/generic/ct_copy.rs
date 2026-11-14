// Written for Graviola by Joe Birr-Pixton, 2026.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

use core::hint::black_box;

/// Copy `if_false` into `output` if `cond` is false.
pub(crate) fn ct_copy<const N: usize>(cond: bool, output: &mut [u8; N], if_false: &[u8; N]) {
    let mask = ((-(black_box(cond as i16))) >> 8) as u8;
    for (out, ff) in output.iter_mut().zip(if_false.iter()) {
        *out = *ff ^ (mask & (*out ^ *ff));
    }
}

/// Return `bit ? if_set : if_unset`.
pub(crate) fn ct_select_i16(bit: u8, if_set: i16, if_unset: i16) -> i16 {
    let mask = ((-(black_box(bit as i32))) >> 16) as u16;
    let if_set = if_set as u16;
    let if_unset = if_unset as u16;
    (if_unset ^ (mask & (if_set ^ if_unset))) as i16
}

