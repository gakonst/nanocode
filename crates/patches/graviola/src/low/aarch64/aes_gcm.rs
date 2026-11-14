// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

use core::arch::aarch64::*;

use crate::low::AesKey;
use crate::low::aarch64::cpu;
use crate::low::ghash::Ghash;

pub(crate) fn encrypt(
    key: &AesKey,
    ghash: &mut Ghash<'_>,
    initial_counter: &[u8; 16],
    aad: &[u8],
    cipher_inout: &mut [u8],
) {
    // SAFETY: this crate requires the `aes` & `neon` cpu features
    unsafe { _cipher::<true>(key, ghash, initial_counter, aad, cipher_inout) }
}

pub(crate) fn decrypt(
    key: &AesKey,
    ghash: &mut Ghash<'_>,
    initial_counter: &[u8; 16],
    aad: &[u8],
    cipher_inout: &mut [u8],
) {
    // SAFETY: this crate requires the `aes` & `neon` cpu features
    unsafe { _cipher::<false>(key, ghash, initial_counter, aad, cipher_inout) }
}

// AES-GCM encrypt (if `ENC` is `true`) or decrypt.
#[target_feature(enable = "aes,neon")]
fn _cipher<const ENC: bool>(
    key: &AesKey,
    ghash: &mut Ghash<'_>,
    initial_counter: &[u8; 16],
    aad: &[u8],
    cipher_inout: &mut [u8],
) {
    ghash.add(aad);

    // counter and inc are big endian, so must be vrev32q_u8'd before use
    // SAFETY: `initial_counter` is 16 bytes and readable
    let counter = unsafe { vld1q_u8(initial_counter.as_ptr().cast()) };
    let mut counter = vreinterpretq_u32_u8(vrev32q_u8(counter));

    let inc = vsetq_lane_u8(1, vdupq_n_u8(0), 15);
    let inc = vreinterpretq_u32_u8(vrev32q_u8(inc));

    let mut by8 = cipher_inout.chunks_exact_mut(128);

    for cipher8 in by8.by_ref() {
        cpu::prefetch_rw(cipher8.as_ptr());
        counter = vaddq_u32(counter, inc);
        let b0 = vrev32q_u8(vreinterpretq_u8_u32(counter));
        counter = vaddq_u32(counter, inc);
        let b1 = vrev32q_u8(vreinterpretq_u8_u32(counter));
        counter = vaddq_u32(counter, inc);
        let b2 = vrev32q_u8(vreinterpretq_u8_u32(counter));
        counter = vaddq_u32(counter, inc);
        let b3 = vrev32q_u8(vreinterpretq_u8_u32(counter));
        counter = vaddq_u32(counter, inc);
        let b4 = vrev32q_u8(vreinterpretq_u8_u32(counter));
        counter = vaddq_u32(counter, inc);
        let b5 = vrev32q_u8(vreinterpretq_u8_u32(counter));
        counter = vaddq_u32(counter, inc);
        let b6 = vrev32q_u8(vreinterpretq_u8_u32(counter));
        counter = vaddq_u32(counter, inc);
        let b7 = vrev32q_u8(vreinterpretq_u8_u32(counter));

        let (b0, b1, b2, b3, b4, b5, b6, b7) = match key {
            AesKey::Aes128(a128) => crate::low::aarch64::aes::_aes128_8_blocks(
                a128.round_keys(),
                b0,
                b1,
                b2,
                b3,
                b4,
                b5,
                b6,
                b7,
            ),
            AesKey::Aes256(a256) => crate::low::aarch64::aes::_aes256_8_blocks(
                a256.round_keys(),
                b0,
                b1,
                b2,
                b3,
                b4,
                b5,
                b6,
                b7,
            ),
        };

        // SAFETY: cipher8 is 128 bytes long, via `chunks_exact_mut`
        unsafe {
            let a0 = vld1q_u8(cipher8.as_ptr().add(0).cast());
            let a1 = vld1q_u8(cipher8.as_ptr().add(16).cast());
            let a2 = vld1q_u8(cipher8.as_ptr().add(32).cast());
            let a3 = vld1q_u8(cipher8.as_ptr().add(48).cast());
            let a4 = vld1q_u8(cipher8.as_ptr().add(64).cast());
            let a5 = vld1q_u8(cipher8.as_ptr().add(80).cast());
            let a6 = vld1q_u8(cipher8.as_ptr().add(96).cast());
            let a7 = vld1q_u8(cipher8.as_ptr().add(112).cast());

            if !ENC {
                ghash.add_eight_blocks(a0, a1, a2, a3, a4, a5, a6, a7);
            }

            let b0 = veorq_u8(a0, b0);
            let b1 = veorq_u8(a1, b1);
            let b2 = veorq_u8(a2, b2);
            let b3 = veorq_u8(a3, b3);
            let b4 = veorq_u8(a4, b4);
            let b5 = veorq_u8(a5, b5);
            let b6 = veorq_u8(a6, b6);
            let b7 = veorq_u8(a7, b7);

            vst1q_u8(cipher8.as_mut_ptr().add(0).cast(), b0);
            vst1q_u8(cipher8.as_mut_ptr().add(16).cast(), b1);
            vst1q_u8(cipher8.as_mut_ptr().add(32).cast(), b2);
            vst1q_u8(cipher8.as_mut_ptr().add(48).cast(), b3);
            vst1q_u8(cipher8.as_mut_ptr().add(64).cast(), b4);
            vst1q_u8(cipher8.as_mut_ptr().add(80).cast(), b5);
            vst1q_u8(cipher8.as_mut_ptr().add(96).cast(), b6);
            vst1q_u8(cipher8.as_mut_ptr().add(112).cast(), b7);

            if ENC {
                ghash.add_eight_blocks(b0, b1, b2, b3, b4, b5, b6, b7);
            }
        }
    }

    let mut singles = by8.into_remainder().chunks_exact_mut(16);

    for cipher in singles.by_ref() {
        // SAFETY: cipher is 16 bytes long, via `chunks_exact_mut`.
        let input_block = unsafe { vld1q_u8(cipher.as_ptr().add(0).cast()) };
        if !ENC {
            ghash.add_block(input_block);
        }
        counter = vaddq_u32(counter, inc);
        let block = vrev32q_u8(vreinterpretq_u8_u32(counter));

        let block = match key {
            AesKey::Aes128(a128) => {
                crate::low::aarch64::aes::_aes128_block(a128.round_keys(), block)
            }
            AesKey::Aes256(a256) => {
                crate::low::aarch64::aes::_aes256_block(a256.round_keys(), block)
            }
        };

        // SAFETY: `cipher` is 16 bytes and writable, via `chunks_exact_mut`
        unsafe {
            let block = veorq_u8(input_block, block);
            vst1q_u8(cipher.as_mut_ptr().cast(), block);
            if ENC {
                ghash.add_block(block);
            }
        }
    }

    {
        let cipher_inout = singles.into_remainder();
        if !cipher_inout.is_empty() {
            if !ENC {
                ghash.add(cipher_inout);
            }
            let mut cipher = [0u8; 16];
            let len = cipher_inout.len();
            debug_assert!(len < 16);
            cipher[..len].copy_from_slice(cipher_inout);

            counter = vaddq_u32(counter, inc);
            let block = vrev32q_u8(vreinterpretq_u8_u32(counter));

            let block = match key {
                AesKey::Aes128(a128) => {
                    crate::low::aarch64::aes::_aes128_block(a128.round_keys(), block)
                }
                AesKey::Aes256(a256) => {
                    crate::low::aarch64::aes::_aes256_block(a256.round_keys(), block)
                }
            };

            // SAFETY: `cipher` is 16 bytes and writable
            unsafe {
                let block = veorq_u8(vld1q_u8(cipher.as_ptr().cast()), block);
                vst1q_u8(cipher.as_mut_ptr().cast(), block)
            };

            cipher_inout.copy_from_slice(&cipher[..len]);
            if ENC {
                ghash.add(cipher_inout);
            }
        }
    }
}
