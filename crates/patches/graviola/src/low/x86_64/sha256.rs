// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

use core::arch::x86_64::*;

pub(in crate::low) fn sha256_compress_blocks_shaext(
    state: &mut [u32; 8],
    blocks: &[u8],
    _token: super::cpu::HaveSha256,
) {
    debug_assert!(blocks.len().is_multiple_of(64));
    // SAFETY: `_token` proves the caller checked the `sha` feature;
    // this crate requires the `sse4.1` and `ssse3` features
    unsafe { sha256(state, blocks) }
}

macro_rules! k {
    ($k:literal) => {
        // SAFETY: `$k` is a compile-time constant and results in a valid index
        // into the const table `K`.  `K` has proper alignment for this aligned
        // load.
        unsafe { _mm_load_si128(K.0.as_ptr().add($k * 4).cast()) }
    };
}

macro_rules! round {
    ($msg_cur:ident, $state0:ident, $state1:ident, $k:ident) => {
        let msg = _mm_add_epi32($msg_cur, $k);
        $state1 = _mm_sha256rnds2_epu32($state1, $state0, msg);
        let msg = _mm_shuffle_epi32(msg, 0b00_00_11_10);
        $state0 = _mm_sha256rnds2_epu32($state0, $state1, msg);
    };

    ($msg_cur:ident, $msg_prev:ident, $state0:ident, $state1:ident, $k:ident) => {
        round!($msg_cur, $state0, $state1, $k);
        let $msg_prev = _mm_sha256msg1_epu32($msg_prev, $msg_cur);
    };

    ($msg_cur:ident, $msg_prev:ident, $msg_next:ident, $state0:ident, $state1:ident, $k:ident) => {
        let msg = _mm_add_epi32($msg_cur, $k);
        $state1 = _mm_sha256rnds2_epu32($state1, $state0, msg);
        let tmp = _mm_alignr_epi8($msg_cur, $msg_prev, 4);
        let $msg_next = _mm_add_epi32($msg_next, tmp);
        let $msg_next = _mm_sha256msg2_epu32($msg_next, $msg_cur);
        let msg = _mm_shuffle_epi32(msg, 0b00_00_11_10);
        $state0 = _mm_sha256rnds2_epu32($state0, $state1, msg);
        let $msg_prev = _mm_sha256msg1_epu32($msg_prev, $msg_cur);
    };
}

#[target_feature(enable = "sha,sse4.1,ssse3")]
fn sha256(state: &mut [u32; 8], blocks: &[u8]) {
    let little_endian_shuffle = _mm_set_epi64x(0x0c0d0e0f08090a0b, 0x0405060700010203);

    // SAFETY: `state` is 8 32-bit words and readable
    let (state0, state1) = unsafe {
        (
            _mm_loadu_si128(state[0..4].as_ptr().cast()),
            _mm_loadu_si128(state[4..8].as_ptr().cast()),
        )
    };

    let tmp = _mm_shuffle_epi32(state0, 0b10_11_00_01);
    let state1 = _mm_shuffle_epi32(state1, 0b00_01_10_11);
    let mut state0 = _mm_alignr_epi8(tmp, state1, 8);
    let mut state1 = _mm_blend_epi16(state1, tmp, 0xf0);

    let k0 = k!(0);
    let k1 = k!(1);
    let k2 = k!(2);
    let k3 = k!(3);
    let k4 = k!(4);
    let k5 = k!(5);
    let k6 = k!(6);
    let k7 = k!(7);
    let k8 = k!(8);
    let k9 = k!(9);
    let k10 = k!(10);
    let k11 = k!(11);
    let k12 = k!(12);
    let k13 = k!(13);
    let k14 = k!(14);
    let k15 = k!(15);

    for block in blocks.chunks_exact(64) {
        let state0_prev = state0;
        let state1_prev = state1;

        // SAFETY: `block` is 64 bytes and readable
        let (msg0, msg1, msg2, msg3) = unsafe {
            (
                _mm_loadu_si128(block[0..].as_ptr().cast()),
                _mm_loadu_si128(block[16..].as_ptr().cast()),
                _mm_loadu_si128(block[32..].as_ptr().cast()),
                _mm_loadu_si128(block[48..].as_ptr().cast()),
            )
        };

        let msg0 = _mm_shuffle_epi8(msg0, little_endian_shuffle);
        let msg1 = _mm_shuffle_epi8(msg1, little_endian_shuffle);
        let msg2 = _mm_shuffle_epi8(msg2, little_endian_shuffle);
        let msg3 = _mm_shuffle_epi8(msg3, little_endian_shuffle);

        round!(msg0, state0, state1, k0);
        round!(msg1, msg0, state0, state1, k1);
        round!(msg2, msg1, state0, state1, k2);
        round!(msg3, msg2, msg0, state0, state1, k3);
        round!(msg0, msg3, msg1, state0, state1, k4);
        round!(msg1, msg0, msg2, state0, state1, k5);
        round!(msg2, msg1, msg3, state0, state1, k6);
        round!(msg3, msg2, msg0, state0, state1, k7);
        round!(msg0, msg3, msg1, state0, state1, k8);
        round!(msg1, msg0, msg2, state0, state1, k9);
        round!(msg2, msg1, msg3, state0, state1, k10);
        round!(msg3, msg2, msg0, state0, state1, k11);
        round!(msg0, msg3, msg1, state0, state1, k12);
        round!(msg1, msg0, msg2, state0, state1, k13);
        round!(msg2, msg1, msg3, state0, state1, k14);
        round!(msg3, state0, state1, k15);
        let _ = msg1;
        let _ = msg0;

        state0 = _mm_add_epi32(state0, state0_prev);
        state1 = _mm_add_epi32(state1, state1_prev);
    }

    let tmp = _mm_shuffle_epi32(state0, 0b00_01_10_11);
    let state1 = _mm_shuffle_epi32(state1, 0b10_11_00_01);
    let state0 = _mm_blend_epi16(tmp, state1, 0xf0);
    let state1 = _mm_alignr_epi8(state1, tmp, 8);

    // SAFETY: `state` is 8 32-bit words and writable
    unsafe {
        _mm_storeu_si128(state[0..4].as_mut_ptr().cast(), state0);
        _mm_storeu_si128(state[4..8].as_mut_ptr().cast(), state1);
    }
}

#[repr(align(16))]
struct Aligned([u32; 64]);

static K: Aligned = Aligned([
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
]);
