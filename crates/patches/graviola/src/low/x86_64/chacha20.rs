// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0
// AVX2 by-8 implementation inspired by YuriMyakotin/ChaCha20-SIMD

use core::arch::x86_64::*;

pub(crate) struct ChaCha20 {
    z07: __m256i,
    z8f: __m256i,
}

impl ChaCha20 {
    pub(crate) fn new(key: &[u8; 32], nonce: &[u8; 16]) -> Self {
        // SAFETY: this crate requires the `avx2` and `ssse3` cpu features
        unsafe { format_key(key, nonce) }
    }

    pub(crate) fn cipher(&mut self, buffer: &mut [u8]) {
        let mut by8 = buffer.chunks_exact_mut(512);

        for block in by8.by_ref() {
            // SAFETY: this crate requires the `avx2` cpu feature
            unsafe {
                core_8x(self.z07, &mut self.z8f, block);
            }
        }

        for block in by8.into_remainder().chunks_mut(128) {
            // SAFETY: this crate requires the `avx2` cpu feature
            unsafe {
                core_2x(self.z07, &mut self.z8f, block);
            }
        }
    }
}

macro_rules! rotate_left {
    ($reg:expr, 8) => {
        // this is a byte shuffle leftwards, except little-endian
        _mm256_shuffle_epi8(
            $reg,
            _mm256_set_epi8(
                14, 13, 12, 15, 10, 9, 8, 11, 6, 5, 4, 7, 2, 1, 0, 3, 14, 13, 12, 15, 10, 9, 8, 11,
                6, 5, 4, 7, 2, 1, 0, 3,
            ),
        )
    };
    ($reg:expr, 16) => {
        // this is a two-byte shuffle leftwards, except little-endian
        _mm256_shuffle_epi8(
            $reg,
            _mm256_set_epi8(
                13, 12, 15, 14, 9, 8, 11, 10, 5, 4, 7, 6, 1, 0, 3, 2, 13, 12, 15, 14, 9, 8, 11, 10,
                5, 4, 7, 6, 1, 0, 3, 2,
            ),
        )
    };
    ($reg:expr, $rot:literal) => {
        _mm256_or_si256(
            _mm256_slli_epi32($reg, $rot),
            _mm256_srli_epi32($reg, 32 - $rot),
        )
    };
}

#[target_feature(enable = "ssse3,avx2")]
fn format_key(key: &[u8; 32], nonce: &[u8; 16]) -> ChaCha20 {
    // SAFETY: `SIGMA` `key`, and `nonce` are all readable
    unsafe {
        let z07 = _mm256_set_m128i(
            _mm_lddqu_si128(SIGMA.as_ptr().cast()),
            _mm_lddqu_si128(key[0..16].as_ptr().cast()),
        );
        let z8f = _mm256_set_m128i(
            _mm_lddqu_si128(key[16..32].as_ptr().cast()),
            _mm_lddqu_si128(nonce.as_ptr().cast()),
        );

        ChaCha20 { z07, z8f }
    }
}

/// Computes 8 blocks.  Does _NOT_ handle ragged output.
#[target_feature(enable = "avx2")]
fn core_8x(t07: __m256i, z8f: &mut __m256i, xor_out_512: &mut [u8]) {
    let t8f = *z8f;
    *z8f = _mm256_add_epi32(*z8f, _mm256_set_epi32(0, 0, 0, 0, 0, 0, 0, 8));

    let z03_z03 = _mm256_broadcastsi128_si256(_mm256_extracti128_si256(t07, 1));
    let z47_z47 = _mm256_broadcastsi128_si256(_mm256_extracti128_si256(t07, 0));
    let z8b_z8b = _mm256_broadcastsi128_si256(_mm256_extracti128_si256(t8f, 1));
    let zcf_zcf = _mm256_broadcastsi128_si256(_mm256_extracti128_si256(t8f, 0));

    let save_z03 = z03_z03;
    let save_z47 = z47_z47;
    let save_z8b = z8b_z8b;
    let save_zcf = zcf_zcf;

    let mut z03_z03 = [z03_z03; 4];
    let mut z47_z47 = [z47_z47; 4];
    let mut z8b_z8b = [z8b_z8b; 4];
    let mut zcf_zcf = [zcf_zcf; 4];

    zcf_zcf[0] = _mm256_add_epi32(zcf_zcf[0], _mm256_set_epi32(0, 0, 0, 0, 0, 0, 0, 4));
    zcf_zcf[1] = _mm256_add_epi32(zcf_zcf[1], _mm256_set_epi32(0, 0, 0, 1, 0, 0, 0, 5));
    zcf_zcf[2] = _mm256_add_epi32(zcf_zcf[2], _mm256_set_epi32(0, 0, 0, 2, 0, 0, 0, 6));
    zcf_zcf[3] = _mm256_add_epi32(zcf_zcf[3], _mm256_set_epi32(0, 0, 0, 3, 0, 0, 0, 7));

    for _ in 0..10 {
        for i in 0..4 {
            z03_z03[i] = _mm256_add_epi32(z03_z03[i], z47_z47[i]);
        }
        for i in 0..4 {
            zcf_zcf[i] = _mm256_xor_si256(zcf_zcf[i], z03_z03[i]);
        }
        for z in &mut zcf_zcf {
            *z = rotate_left!(*z, 16);
        }

        for i in 0..4 {
            z8b_z8b[i] = _mm256_add_epi32(z8b_z8b[i], zcf_zcf[i]);
        }
        for i in 0..4 {
            z47_z47[i] = _mm256_xor_si256(z47_z47[i], z8b_z8b[i]);
        }
        for z in &mut z47_z47 {
            *z = rotate_left!(*z, 12);
        }

        for i in 0..4 {
            z03_z03[i] = _mm256_add_epi32(z03_z03[i], z47_z47[i]);
        }
        for i in 0..4 {
            zcf_zcf[i] = _mm256_xor_si256(zcf_zcf[i], z03_z03[i]);
        }
        for z in &mut zcf_zcf {
            *z = rotate_left!(*z, 8);
        }

        for i in 0..4 {
            z8b_z8b[i] = _mm256_add_epi32(z8b_z8b[i], zcf_zcf[i]);
        }
        for i in 0..4 {
            z47_z47[i] = _mm256_xor_si256(z47_z47[i], z8b_z8b[i]);
        }
        for z in &mut z47_z47 {
            *z = rotate_left!(*z, 7);
        }

        for z in &mut z47_z47 {
            *z = _mm256_shuffle_epi32(*z, 0b00_11_10_01);
        }
        for z in &mut z8b_z8b {
            *z = _mm256_shuffle_epi32(*z, 0b01_00_11_10);
        }
        for z in &mut zcf_zcf {
            *z = _mm256_shuffle_epi32(*z, 0b10_01_00_11);
        }

        for i in 0..4 {
            z03_z03[i] = _mm256_add_epi32(z03_z03[i], z47_z47[i]);
        }
        for i in 0..4 {
            zcf_zcf[i] = _mm256_xor_si256(zcf_zcf[i], z03_z03[i]);
        }
        for z in &mut zcf_zcf {
            *z = rotate_left!(*z, 16);
        }

        for i in 0..4 {
            z8b_z8b[i] = _mm256_add_epi32(z8b_z8b[i], zcf_zcf[i]);
        }
        for i in 0..4 {
            z47_z47[i] = _mm256_xor_si256(z47_z47[i], z8b_z8b[i]);
        }
        for z in &mut z47_z47 {
            *z = rotate_left!(*z, 12);
        }

        for i in 0..4 {
            z03_z03[i] = _mm256_add_epi32(z03_z03[i], z47_z47[i]);
        }
        for i in 0..4 {
            zcf_zcf[i] = _mm256_xor_si256(zcf_zcf[i], z03_z03[i]);
        }
        for z in &mut zcf_zcf {
            *z = rotate_left!(*z, 8);
        }

        for i in 0..4 {
            z8b_z8b[i] = _mm256_add_epi32(z8b_z8b[i], zcf_zcf[i]);
        }
        for i in 0..4 {
            z47_z47[i] = _mm256_xor_si256(z47_z47[i], z8b_z8b[i]);
        }
        for z in &mut z47_z47 {
            *z = rotate_left!(*z, 7);
        }

        for z in &mut z47_z47 {
            *z = _mm256_shuffle_epi32(*z, 0b10_01_00_11);
        }
        for z in &mut z8b_z8b {
            *z = _mm256_shuffle_epi32(*z, 0b01_00_11_10);
        }
        for z in &mut zcf_zcf {
            *z = _mm256_shuffle_epi32(*z, 0b00_11_10_01);
        }
    }

    for i in 0..4 {
        z03_z03[i] = _mm256_add_epi32(z03_z03[i], save_z03);
        z47_z47[i] = _mm256_add_epi32(z47_z47[i], save_z47);
        z8b_z8b[i] = _mm256_add_epi32(z8b_z8b[i], save_z8b);
        zcf_zcf[i] = _mm256_add_epi32(zcf_zcf[i], save_zcf);
    }

    // reapply counter adjustments as save_zcf did not include them
    zcf_zcf[0] = _mm256_add_epi32(zcf_zcf[0], _mm256_set_epi32(0, 0, 0, 0, 0, 0, 0, 4));
    zcf_zcf[1] = _mm256_add_epi32(zcf_zcf[1], _mm256_set_epi32(0, 0, 0, 1, 0, 0, 0, 5));
    zcf_zcf[2] = _mm256_add_epi32(zcf_zcf[2], _mm256_set_epi32(0, 0, 0, 2, 0, 0, 0, 6));
    zcf_zcf[3] = _mm256_add_epi32(zcf_zcf[3], _mm256_set_epi32(0, 0, 0, 3, 0, 0, 0, 7));

    // our eight output keystream blocks
    let a0 = _mm256_permute2x128_si256(z03_z03[0], z47_z47[0], 0b0011_0001);
    let a1 = _mm256_permute2x128_si256(z8b_z8b[0], zcf_zcf[0], 0b0011_0001);
    let b0 = _mm256_permute2x128_si256(z03_z03[1], z47_z47[1], 0b0011_0001);
    let b1 = _mm256_permute2x128_si256(z8b_z8b[1], zcf_zcf[1], 0b0011_0001);

    let c0 = _mm256_permute2x128_si256(z03_z03[2], z47_z47[2], 0b0011_0001);
    let c1 = _mm256_permute2x128_si256(z8b_z8b[2], zcf_zcf[2], 0b0011_0001);
    let d0 = _mm256_permute2x128_si256(z03_z03[3], z47_z47[3], 0b0011_0001);
    let d1 = _mm256_permute2x128_si256(z8b_z8b[3], zcf_zcf[3], 0b0011_0001);

    let e0 = _mm256_permute2x128_si256(z03_z03[0], z47_z47[0], 0b0010_0000);
    let e1 = _mm256_permute2x128_si256(z8b_z8b[0], zcf_zcf[0], 0b0010_0000);
    let f0 = _mm256_permute2x128_si256(z03_z03[1], z47_z47[1], 0b0010_0000);
    let f1 = _mm256_permute2x128_si256(z8b_z8b[1], zcf_zcf[1], 0b0010_0000);

    let g0 = _mm256_permute2x128_si256(z03_z03[2], z47_z47[2], 0b0010_0000);
    let g1 = _mm256_permute2x128_si256(z8b_z8b[2], zcf_zcf[2], 0b0010_0000);
    let h0 = _mm256_permute2x128_si256(z03_z03[3], z47_z47[3], 0b0010_0000);
    let h1 = _mm256_permute2x128_si256(z8b_z8b[3], zcf_zcf[3], 0b0010_0000);

    let ptr: *mut __m256i = xor_out_512.as_mut_ptr().cast();

    // SAFETY: `xor_out_512` is 512 bytes due to chunking in caller. Observe that
    // `ptr` is unaligned which is allowed as we only use it here with unaligned
    // stores/loads.
    unsafe {
        _mm256_storeu_si256(
            ptr.add(0),
            _mm256_xor_si256(_mm256_loadu_si256(ptr.add(0)), a0),
        );
        _mm256_storeu_si256(
            ptr.add(1),
            _mm256_xor_si256(_mm256_loadu_si256(ptr.add(1)), a1),
        );

        _mm256_storeu_si256(
            ptr.add(2),
            _mm256_xor_si256(_mm256_loadu_si256(ptr.add(2)), b0),
        );
        _mm256_storeu_si256(
            ptr.add(3),
            _mm256_xor_si256(_mm256_loadu_si256(ptr.add(3)), b1),
        );

        _mm256_storeu_si256(
            ptr.add(4),
            _mm256_xor_si256(_mm256_loadu_si256(ptr.add(4)), c0),
        );
        _mm256_storeu_si256(
            ptr.add(5),
            _mm256_xor_si256(_mm256_loadu_si256(ptr.add(5)), c1),
        );

        _mm256_storeu_si256(
            ptr.add(6),
            _mm256_xor_si256(_mm256_loadu_si256(ptr.add(6)), d0),
        );
        _mm256_storeu_si256(
            ptr.add(7),
            _mm256_xor_si256(_mm256_loadu_si256(ptr.add(7)), d1),
        );

        _mm256_storeu_si256(
            ptr.add(8),
            _mm256_xor_si256(_mm256_loadu_si256(ptr.add(8)), e0),
        );
        _mm256_storeu_si256(
            ptr.add(9),
            _mm256_xor_si256(_mm256_loadu_si256(ptr.add(9)), e1),
        );

        _mm256_storeu_si256(
            ptr.add(10),
            _mm256_xor_si256(_mm256_loadu_si256(ptr.add(10)), f0),
        );
        _mm256_storeu_si256(
            ptr.add(11),
            _mm256_xor_si256(_mm256_loadu_si256(ptr.add(11)), f1),
        );

        _mm256_storeu_si256(
            ptr.add(12),
            _mm256_xor_si256(_mm256_loadu_si256(ptr.add(12)), g0),
        );
        _mm256_storeu_si256(
            ptr.add(13),
            _mm256_xor_si256(_mm256_loadu_si256(ptr.add(13)), g1),
        );

        _mm256_storeu_si256(
            ptr.add(14),
            _mm256_xor_si256(_mm256_loadu_si256(ptr.add(14)), h0),
        );
        _mm256_storeu_si256(
            ptr.add(15),
            _mm256_xor_si256(_mm256_loadu_si256(ptr.add(15)), h1),
        );
    }
}

/// Computes 2 blocks, but also handles ragged output (ie, xor_out may
/// be 0..64 bytes).
#[target_feature(enable = "avx2")]
fn core_2x(t07: __m256i, z8f: &mut __m256i, xor_out: &mut [u8]) {
    let t8f = *z8f;
    let blocks_used = if xor_out.len() > 32 { 2 } else { 1 };
    *z8f = _mm256_add_epi32(*z8f, _mm256_set_epi32(0, 0, 0, 0, 0, 0, 0, blocks_used));

    let mut z03_z03 = _mm256_broadcastsi128_si256(_mm256_extracti128_si256(t07, 1));
    let mut z47_z47 = _mm256_broadcastsi128_si256(_mm256_extracti128_si256(t07, 0));
    let mut z8b_z8b = _mm256_broadcastsi128_si256(_mm256_extracti128_si256(t8f, 1));
    let mut zcf_zcf = _mm256_broadcastsi128_si256(_mm256_extracti128_si256(t8f, 0));

    // we will calculate two blocks, so increment the counter of the second block
    zcf_zcf = _mm256_add_epi32(zcf_zcf, _mm256_set_epi32(0, 0, 0, 0, 0, 0, 0, 1));
    let save_z03 = z03_z03;
    let save_z47 = z47_z47;
    let save_z8b = z8b_z8b;
    let save_zcf = zcf_zcf;

    for _ in 0..10 {
        z03_z03 = _mm256_add_epi32(z03_z03, z47_z47);
        zcf_zcf = _mm256_xor_si256(zcf_zcf, z03_z03);
        zcf_zcf = rotate_left!(zcf_zcf, 16);

        z8b_z8b = _mm256_add_epi32(z8b_z8b, zcf_zcf);
        z47_z47 = _mm256_xor_si256(z47_z47, z8b_z8b);
        z47_z47 = rotate_left!(z47_z47, 12);

        z03_z03 = _mm256_add_epi32(z03_z03, z47_z47);
        zcf_zcf = _mm256_xor_si256(zcf_zcf, z03_z03);
        zcf_zcf = rotate_left!(zcf_zcf, 8);

        z8b_z8b = _mm256_add_epi32(z8b_z8b, zcf_zcf);
        z47_z47 = _mm256_xor_si256(z47_z47, z8b_z8b);
        z47_z47 = rotate_left!(z47_z47, 7);

        z47_z47 = _mm256_shuffle_epi32(z47_z47, 0b00_11_10_01);
        z8b_z8b = _mm256_shuffle_epi32(z8b_z8b, 0b01_00_11_10);
        zcf_zcf = _mm256_shuffle_epi32(zcf_zcf, 0b10_01_00_11);

        z03_z03 = _mm256_add_epi32(z03_z03, z47_z47);
        zcf_zcf = _mm256_xor_si256(zcf_zcf, z03_z03);
        zcf_zcf = rotate_left!(zcf_zcf, 16);

        z8b_z8b = _mm256_add_epi32(z8b_z8b, zcf_zcf);
        z47_z47 = _mm256_xor_si256(z47_z47, z8b_z8b);
        z47_z47 = rotate_left!(z47_z47, 12);

        z03_z03 = _mm256_add_epi32(z03_z03, z47_z47);
        zcf_zcf = _mm256_xor_si256(zcf_zcf, z03_z03);
        zcf_zcf = rotate_left!(zcf_zcf, 8);

        z8b_z8b = _mm256_add_epi32(z8b_z8b, zcf_zcf);
        z47_z47 = _mm256_xor_si256(z47_z47, z8b_z8b);
        z47_z47 = rotate_left!(z47_z47, 7);

        z47_z47 = _mm256_shuffle_epi32(z47_z47, 0b10_01_00_11);
        z8b_z8b = _mm256_shuffle_epi32(z8b_z8b, 0b01_00_11_10);
        zcf_zcf = _mm256_shuffle_epi32(zcf_zcf, 0b00_11_10_01);
    }

    let z03_z03 = _mm256_add_epi32(z03_z03, save_z03);
    let z47_z47 = _mm256_add_epi32(z47_z47, save_z47);
    let z8b_z8b = _mm256_add_epi32(z8b_z8b, save_z8b);
    let zcf_zcf = _mm256_add_epi32(zcf_zcf, save_zcf);

    let a0 = _mm256_permute2x128_si256(z03_z03, z47_z47, 0b0011_0001);
    let a1 = _mm256_permute2x128_si256(z8b_z8b, zcf_zcf, 0b0011_0001);
    let b0 = _mm256_permute2x128_si256(z03_z03, z47_z47, 0b0010_0000);
    let b1 = _mm256_permute2x128_si256(z8b_z8b, zcf_zcf, 0b0010_0000);

    let ptr: *mut __m256i = xor_out.as_mut_ptr().cast();

    if xor_out.len() == 128 {
        // SAFETY: `ptr` is 128 bytes, ie 4 x 256-bit ymm registers
        unsafe {
            let ia0 = _mm256_loadu_si256(ptr.add(0));
            let ia1 = _mm256_loadu_si256(ptr.add(1));
            let ib0 = _mm256_loadu_si256(ptr.add(2));
            let ib1 = _mm256_loadu_si256(ptr.add(3));

            _mm256_storeu_si256(ptr.add(0), _mm256_xor_si256(a0, ia0));
            _mm256_storeu_si256(ptr.add(1), _mm256_xor_si256(a1, ia1));
            _mm256_storeu_si256(ptr.add(2), _mm256_xor_si256(b0, ib0));
            _mm256_storeu_si256(ptr.add(3), _mm256_xor_si256(b1, ib1));
        }
    } else if xor_out.len() == 32 {
        // SAFETY: `ptr` is 32 bytes, ie one 256-bit ymm register
        unsafe {
            let ia0 = _mm256_loadu_si256(ptr.add(0));
            _mm256_storeu_si256(ptr.add(0), _mm256_xor_si256(a0, ia0));
        }
    } else {
        // slow path
        let mut ks = [0u8; 128];

        // SAFETY: `ks` is 128 bytes and writable.
        unsafe {
            _mm256_storeu_si256(ks[0..32].as_mut_ptr().cast(), a0);
            _mm256_storeu_si256(ks[32..64].as_mut_ptr().cast(), a1);
            _mm256_storeu_si256(ks[64..96].as_mut_ptr().cast(), b0);
            _mm256_storeu_si256(ks[96..128].as_mut_ptr().cast(), b1);
        }

        for (o, k) in xor_out.iter_mut().zip(ks.iter()) {
            *o ^= *k;
        }
    }
}

const SIGMA: [u8; 16] = *b"expand 32-byte k";

