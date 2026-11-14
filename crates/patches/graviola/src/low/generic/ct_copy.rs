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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cond_true_keeps_output() {
        // cond == true: `output` is left untouched, `if_false` is ignored.
        let mut output = [1u8, 2, 3, 4];
        let if_false = [10u8, 20, 30, 40];
        ct_copy(true, &mut output, &if_false);
        assert_eq!(output, [1, 2, 3, 4]);
    }

    #[test]
    fn cond_false_copies_if_false() {
        // cond == false: `output` is overwritten with `if_false`.
        let mut output = [1u8, 2, 3, 4];
        let if_false = [10u8, 20, 30, 40];
        ct_copy(false, &mut output, &if_false);
        assert_eq!(output, [10, 20, 30, 40]);
    }

    #[test]
    fn selects_every_byte_independently() {
        // every byte position must follow the condition, including 0x00/0xff
        // values that could expose a broken mask.
        let mut output = [0x00, 0xff, 0xaa, 0x55, 0x01, 0x80];
        let if_false = [0xff, 0x00, 0x55, 0xaa, 0x80, 0x01];

        let mut kept = output;
        ct_copy(true, &mut kept, &if_false);
        assert_eq!(kept, output);

        ct_copy(false, &mut output, &if_false);
        assert_eq!(output, if_false);
    }

    #[test]
    fn empty_arrays_are_a_no_op() {
        let mut output = [0u8; 0];
        let if_false = [0u8; 0];
        ct_copy(true, &mut output, &if_false);
        ct_copy(false, &mut output, &if_false);
        assert_eq!(output, [0u8; 0]);
    }

    #[test]
    fn single_byte() {
        let mut output = [0x42u8];
        ct_copy(true, &mut output, &[0x99]);
        assert_eq!(output, [0x42]);
        ct_copy(false, &mut output, &[0x99]);
        assert_eq!(output, [0x99]);
    }

    #[test]
    fn equal_inputs_unchanged_either_way() {
        // when both sides are equal, the result is that value regardless of cond.
        for cond in [true, false] {
            let mut output = [7u8; 16];
            ct_copy(cond, &mut output, &[7u8; 16]);
            assert_eq!(output, [7u8; 16]);
        }
    }

    #[test]
    fn larger_buffer() {
        let mut output = [0u8; 64];
        let mut if_false = [0u8; 64];
        for (i, (o, f)) in output.iter_mut().zip(if_false.iter_mut()).enumerate() {
            *o = i as u8;
            *f = (255 - i) as u8;
        }
        let original = output;

        ct_copy(true, &mut output, &if_false);
        assert_eq!(output, original);

        ct_copy(false, &mut output, &if_false);
        assert_eq!(output, if_false);
    }

    #[test]
    fn select_bit_set_returns_if_set() {
        for bit in 1..=0xff {
            assert_eq!(ct_select_i16(bit, 1234, -5678), 1234);
        }
    }

    #[test]
    fn select_bit_unset_returns_if_unset() {
        assert_eq!(ct_select_i16(0, 1234, -5678), -5678);
    }

    #[test]
    fn select_covers_full_i16_range() {
        // exercise the extremes and sign-bit-heavy values to catch a broken
        // mask in any bit position.
        let interesting = [
            0i16,
            1,
            -1,
            i16::MAX,
            i16::MIN,
            0x5555u16 as i16,
            0xaaaau16 as i16,
            0x00ff,
            -256,
        ];
        for &set in &interesting {
            for &unset in &interesting {
                assert_eq!(ct_select_i16(1, set, unset), set);
                assert_eq!(ct_select_i16(0, set, unset), unset);
            }
        }
    }

    #[test]
    fn select_equal_inputs() {
        for bit in [0u8, 1] {
            assert_eq!(ct_select_i16(bit, -42, -42), -42);
        }
    }

    #[test]
    fn select_does_not_mix_arms() {
        // a faulty mask could blend bits of the two arms; check we get exactly
        // one or the other, never a combination.
        let set = 0x0f0fu16 as i16;
        let unset = 0xf0f0u16 as i16;
        assert_eq!(ct_select_i16(1, set, unset), set);
        assert_eq!(ct_select_i16(0, set, unset), unset);
    }
}
