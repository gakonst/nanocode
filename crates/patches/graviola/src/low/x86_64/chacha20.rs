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

pub(crate) struct XChaCha20(ChaCha20);

impl XChaCha20 {
    pub(crate) fn new(key: &[u8; 32], nonce: &[u8; 24]) -> Self {
        // SAFETY: this crate requires the `avx2` and `ssse3` cpu features
        unsafe { Self(hchacha(key, nonce)) }
    }

    pub(crate) fn cipher(&mut self, buffer: &mut [u8]) {
        self.0.cipher(buffer);
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

macro_rules! rotate_left_128 {
    ($reg:expr, 8) => {
        // this is a byte shuffle leftwards, except little-endian
        _mm_shuffle_epi8(
            $reg,
            _mm_set_epi8(14, 13, 12, 15, 10, 9, 8, 11, 6, 5, 4, 7, 2, 1, 0, 3),
        )
    };
    ($reg:expr, 16) => {
        // this is a two-byte shuffle leftwards, except little-endian
        _mm_shuffle_epi8(
            $reg,
            _mm_set_epi8(13, 12, 15, 14, 9, 8, 11, 10, 5, 4, 7, 6, 1, 0, 3, 2),
        )
    };
    ($reg:expr, $rot:literal) => {
        _mm_or_si128(_mm_slli_epi32($reg, $rot), _mm_srli_epi32($reg, 32 - $rot))
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

#[target_feature(enable = "ssse3,avx2")]
fn hchacha(key: &[u8; 32], nonce: &[u8; 24]) -> ChaCha20 {
    // SAFETY: `SIGMA`, `key` and `nonce` are all readable
    let (mut z03, mut z47, mut z8b, mut zcf) = unsafe {
        (
            _mm_lddqu_si128(SIGMA.as_ptr().cast()),
            _mm_lddqu_si128(key[0..16].as_ptr().cast()),
            _mm_lddqu_si128(key[16..32].as_ptr().cast()),
            _mm_lddqu_si128(nonce[0..16].as_ptr().cast()),
        )
    };

    for _ in 0..10 {
        z03 = _mm_add_epi32(z03, z47);
        zcf = _mm_xor_si128(zcf, z03);
        zcf = rotate_left_128!(zcf, 16);

        z8b = _mm_add_epi32(z8b, zcf);
        z47 = _mm_xor_si128(z47, z8b);
        z47 = rotate_left_128!(z47, 12);

        z03 = _mm_add_epi32(z03, z47);
        zcf = _mm_xor_si128(zcf, z03);
        zcf = rotate_left_128!(zcf, 8);

        z8b = _mm_add_epi32(z8b, zcf);
        z47 = _mm_xor_si128(z47, z8b);
        z47 = rotate_left_128!(z47, 7);

        z47 = _mm_shuffle_epi32(z47, 0b00_11_10_01);
        z8b = _mm_shuffle_epi32(z8b, 0b01_00_11_10);
        zcf = _mm_shuffle_epi32(zcf, 0b10_01_00_11);

        z03 = _mm_add_epi32(z03, z47);
        zcf = _mm_xor_si128(zcf, z03);
        zcf = rotate_left_128!(zcf, 16);

        z8b = _mm_add_epi32(z8b, zcf);
        z47 = _mm_xor_si128(z47, z8b);
        z47 = rotate_left_128!(z47, 12);

        z03 = _mm_add_epi32(z03, z47);
        zcf = _mm_xor_si128(zcf, z03);
        zcf = rotate_left_128!(zcf, 8);

        z8b = _mm_add_epi32(z8b, zcf);
        z47 = _mm_xor_si128(z47, z8b);
        z47 = rotate_left_128!(z47, 7);

        z47 = _mm_shuffle_epi32(z47, 0b10_01_00_11);
        z8b = _mm_shuffle_epi32(z8b, 0b01_00_11_10);
        zcf = _mm_shuffle_epi32(zcf, 0b00_11_10_01);
    }

    // SAFETY: `SIGMA` is readable.
    let z07 = _mm256_set_m128i(unsafe { _mm_lddqu_si128(SIGMA.as_ptr().cast()) }, z03);

    let mut chacha_nonce = [0u8; 16];
    chacha_nonce[8..16].copy_from_slice(&nonce[16..24]);
    // SAFETY: `chacha_nonce` is certainly 16 bytes and readable.
    let z8f = _mm256_set_m128i(zcf, unsafe {
        _mm_lddqu_si128(chacha_nonce.as_ptr().cast())
    });

    ChaCha20 { z07, z8f }
}

const SIGMA: [u8; 16] = *b"expand 32-byte k";

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_vectors() {
        // From draft-agl-tls-chacha20poly1305-04 section 7
        let mut c = ChaCha20::new(&[0u8; 32], &[0u8; 16]);
        let mut block = [0u8; 64];
        c.cipher(&mut block);
        assert_eq!(
            block,
            [
                0x76, 0xb8, 0xe0, 0xad, 0xa0, 0xf1, 0x3d, 0x90, 0x40, 0x5d, 0x6a, 0xe5, 0x53, 0x86,
                0xbd, 0x28, 0xbd, 0xd2, 0x19, 0xb8, 0xa0, 0x8d, 0xed, 0x1a, 0xa8, 0x36, 0xef, 0xcc,
                0x8b, 0x77, 0x0d, 0xc7, 0xda, 0x41, 0x59, 0x7c, 0x51, 0x57, 0x48, 0x8d, 0x77, 0x24,
                0xe0, 0x3f, 0xb8, 0xd8, 0x4a, 0x37, 0x6a, 0x43, 0xb8, 0xf4, 0x15, 0x18, 0xa1, 0x1c,
                0xc3, 0x87, 0xb6, 0x69, 0xb2, 0xee, 0x65, 0x86
            ]
        );

        let mut key = [0u8; 32];
        key[31] = 0x01;
        let mut c = ChaCha20::new(&key, &[0u8; 16]);
        let mut block = [0u8; 64];
        c.cipher(&mut block);
        assert_eq!(
            block,
            [
                0x45, 0x40, 0xf0, 0x5a, 0x9f, 0x1f, 0xb2, 0x96, 0xd7, 0x73, 0x6e, 0x7b, 0x20, 0x8e,
                0x3c, 0x96, 0xeb, 0x4f, 0xe1, 0x83, 0x46, 0x88, 0xd2, 0x60, 0x4f, 0x45, 0x09, 0x52,
                0xed, 0x43, 0x2d, 0x41, 0xbb, 0xe2, 0xa0, 0xb6, 0xea, 0x75, 0x66, 0xd2, 0xa5, 0xd1,
                0xe7, 0xe2, 0x0d, 0x42, 0xaf, 0x2c, 0x53, 0xd7, 0x92, 0xb1, 0xc4, 0x3f, 0xea, 0x81,
                0x7e, 0x9a, 0xd2, 0x75, 0xae, 0x54, 0x69, 0x63
            ]
        );

        let mut nonce = [0u8; 16];
        nonce[15] = 0x01;
        let mut c = ChaCha20::new(&[0u8; 32], &nonce);
        let mut block = [0u8; 64];
        c.cipher(&mut block);
        assert_eq!(
            block[..60],
            [
                0xde, 0x9c, 0xba, 0x7b, 0xf3, 0xd6, 0x9e, 0xf5, 0xe7, 0x86, 0xdc, 0x63, 0x97, 0x3f,
                0x65, 0x3a, 0x0b, 0x49, 0xe0, 0x15, 0xad, 0xbf, 0xf7, 0x13, 0x4f, 0xcb, 0x7d, 0xf1,
                0x37, 0x82, 0x10, 0x31, 0xe8, 0x5a, 0x05, 0x02, 0x78, 0xa7, 0x08, 0x45, 0x27, 0x21,
                0x4f, 0x73, 0xef, 0xc7, 0xfa, 0x5b, 0x52, 0x77, 0x06, 0x2e, 0xb7, 0xa0, 0x43, 0x3e,
                0x44, 0x5f, 0x41, 0xe3
            ]
        );

        let mut nonce = [0u8; 16];
        nonce[8] = 0x01;
        let mut c = ChaCha20::new(&[0u8; 32], &nonce);
        let mut block = [0u8; 64];
        c.cipher(&mut block);
        assert_eq!(
            block,
            [
                0xef, 0x3f, 0xdf, 0xd6, 0xc6, 0x15, 0x78, 0xfb, 0xf5, 0xcf, 0x35, 0xbd, 0x3d, 0xd3,
                0x3b, 0x80, 0x09, 0x63, 0x16, 0x34, 0xd2, 0x1e, 0x42, 0xac, 0x33, 0x96, 0x0b, 0xd1,
                0x38, 0xe5, 0x0d, 0x32, 0x11, 0x1e, 0x4c, 0xaf, 0x23, 0x7e, 0xe5, 0x3c, 0xa8, 0xad,
                0x64, 0x26, 0x19, 0x4a, 0x88, 0x54, 0x5d, 0xdc, 0x49, 0x7a, 0x0b, 0x46, 0x6e, 0x7d,
                0x6b, 0xbd, 0xb0, 0x04, 0x1b, 0x2f, 0x58, 0x6b
            ]
        );

        let mut c = ChaCha20::new(
            &[
                0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
                0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b,
                0x1c, 0x1d, 0x1e, 0x1f,
            ],
            &[
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05,
                0x06, 0x07,
            ],
        );

        let mut block = [0u8; 256];
        c.cipher(&mut block);

        assert_eq!(
            block,
            [
                0xf7, 0x98, 0xa1, 0x89, 0xf1, 0x95, 0xe6, 0x69, 0x82, 0x10, 0x5f, 0xfb, 0x64, 0x0b,
                0xb7, 0x75, 0x7f, 0x57, 0x9d, 0xa3, 0x16, 0x02, 0xfc, 0x93, 0xec, 0x01, 0xac, 0x56,
                0xf8, 0x5a, 0xc3, 0xc1, 0x34, 0xa4, 0x54, 0x7b, 0x73, 0x3b, 0x46, 0x41, 0x30, 0x42,
                0xc9, 0x44, 0x00, 0x49, 0x17, 0x69, 0x05, 0xd3, 0xbe, 0x59, 0xea, 0x1c, 0x53, 0xf1,
                0x59, 0x16, 0x15, 0x5c, 0x2b, 0xe8, 0x24, 0x1a, 0x38, 0x00, 0x8b, 0x9a, 0x26, 0xbc,
                0x35, 0x94, 0x1e, 0x24, 0x44, 0x17, 0x7c, 0x8a, 0xde, 0x66, 0x89, 0xde, 0x95, 0x26,
                0x49, 0x86, 0xd9, 0x58, 0x89, 0xfb, 0x60, 0xe8, 0x46, 0x29, 0xc9, 0xbd, 0x9a, 0x5a,
                0xcb, 0x1c, 0xc1, 0x18, 0xbe, 0x56, 0x3e, 0xb9, 0xb3, 0xa4, 0xa4, 0x72, 0xf8, 0x2e,
                0x09, 0xa7, 0xe7, 0x78, 0x49, 0x2b, 0x56, 0x2e, 0xf7, 0x13, 0x0e, 0x88, 0xdf, 0xe0,
                0x31, 0xc7, 0x9d, 0xb9, 0xd4, 0xf7, 0xc7, 0xa8, 0x99, 0x15, 0x1b, 0x9a, 0x47, 0x50,
                0x32, 0xb6, 0x3f, 0xc3, 0x85, 0x24, 0x5f, 0xe0, 0x54, 0xe3, 0xdd, 0x5a, 0x97, 0xa5,
                0xf5, 0x76, 0xfe, 0x06, 0x40, 0x25, 0xd3, 0xce, 0x04, 0x2c, 0x56, 0x6a, 0xb2, 0xc5,
                0x07, 0xb1, 0x38, 0xdb, 0x85, 0x3e, 0x3d, 0x69, 0x59, 0x66, 0x09, 0x96, 0x54, 0x6c,
                0xc9, 0xc4, 0xa6, 0xea, 0xfd, 0xc7, 0x77, 0xc0, 0x40, 0xd7, 0x0e, 0xaf, 0x46, 0xf7,
                0x6d, 0xad, 0x39, 0x79, 0xe5, 0xc5, 0x36, 0x0c, 0x33, 0x17, 0x16, 0x6a, 0x1c, 0x89,
                0x4c, 0x94, 0xa3, 0x71, 0x87, 0x6a, 0x94, 0xdf, 0x76, 0x28, 0xfe, 0x4e, 0xaa, 0xf2,
                0xcc, 0xb2, 0x7d, 0x5a, 0xaa, 0xe0, 0xad, 0x7a, 0xd0, 0xf9, 0xd4, 0xb6, 0xad, 0x3b,
                0x54, 0x09, 0x87, 0x46, 0xd4, 0x52, 0x4d, 0x38, 0x40, 0x7a, 0x6d, 0xeb, 0x3a, 0xb7,
                0x8f, 0xab, 0x78, 0xc9
            ]
        );
    }

    #[test]
    fn xchacha_test_vectors() {
        // From draft-irtf-cfrg-xchacha-03, A.2
        // https://datatracker.ietf.org/doc/html/draft-irtf-cfrg-xchacha-03#appendix-A.2

        let key = [
            0x80, 0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x8b, 0x8c, 0x8d,
            0x8e, 0x8f, 0x90, 0x91, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0x9b,
            0x9c, 0x9d, 0x9e, 0x9f,
        ];
        let nonce = *b"@ABCDEFGHIJKLMNOPQRSTUVX";
        let mut c = XChaCha20::new(&key, &nonce);

        let mut buffer = [
            0x54, 0x68, 0x65, 0x20, 0x64, 0x68, 0x6f, 0x6c, 0x65, 0x20, 0x28, 0x70, 0x72, 0x6f,
            0x6e, 0x6f, 0x75, 0x6e, 0x63, 0x65, 0x64, 0x20, 0x22, 0x64, 0x6f, 0x6c, 0x65, 0x22,
            0x29, 0x20, 0x69, 0x73, 0x20, 0x61, 0x6c, 0x73, 0x6f, 0x20, 0x6b, 0x6e, 0x6f, 0x77,
            0x6e, 0x20, 0x61, 0x73, 0x20, 0x74, 0x68, 0x65, 0x20, 0x41, 0x73, 0x69, 0x61, 0x74,
            0x69, 0x63, 0x20, 0x77, 0x69, 0x6c, 0x64, 0x20, 0x64, 0x6f, 0x67, 0x2c, 0x20, 0x72,
            0x65, 0x64, 0x20, 0x64, 0x6f, 0x67, 0x2c, 0x20, 0x61, 0x6e, 0x64, 0x20, 0x77, 0x68,
            0x69, 0x73, 0x74, 0x6c, 0x69, 0x6e, 0x67, 0x20, 0x64, 0x6f, 0x67, 0x2e, 0x20, 0x49,
            0x74, 0x20, 0x69, 0x73, 0x20, 0x61, 0x62, 0x6f, 0x75, 0x74, 0x20, 0x74, 0x68, 0x65,
            0x20, 0x73, 0x69, 0x7a, 0x65, 0x20, 0x6f, 0x66, 0x20, 0x61, 0x20, 0x47, 0x65, 0x72,
            0x6d, 0x61, 0x6e, 0x20, 0x73, 0x68, 0x65, 0x70, 0x68, 0x65, 0x72, 0x64, 0x20, 0x62,
            0x75, 0x74, 0x20, 0x6c, 0x6f, 0x6f, 0x6b, 0x73, 0x20, 0x6d, 0x6f, 0x72, 0x65, 0x20,
            0x6c, 0x69, 0x6b, 0x65, 0x20, 0x61, 0x20, 0x6c, 0x6f, 0x6e, 0x67, 0x2d, 0x6c, 0x65,
            0x67, 0x67, 0x65, 0x64, 0x20, 0x66, 0x6f, 0x78, 0x2e, 0x20, 0x54, 0x68, 0x69, 0x73,
            0x20, 0x68, 0x69, 0x67, 0x68, 0x6c, 0x79, 0x20, 0x65, 0x6c, 0x75, 0x73, 0x69, 0x76,
            0x65, 0x20, 0x61, 0x6e, 0x64, 0x20, 0x73, 0x6b, 0x69, 0x6c, 0x6c, 0x65, 0x64, 0x20,
            0x6a, 0x75, 0x6d, 0x70, 0x65, 0x72, 0x20, 0x69, 0x73, 0x20, 0x63, 0x6c, 0x61, 0x73,
            0x73, 0x69, 0x66, 0x69, 0x65, 0x64, 0x20, 0x77, 0x69, 0x74, 0x68, 0x20, 0x77, 0x6f,
            0x6c, 0x76, 0x65, 0x73, 0x2c, 0x20, 0x63, 0x6f, 0x79, 0x6f, 0x74, 0x65, 0x73, 0x2c,
            0x20, 0x6a, 0x61, 0x63, 0x6b, 0x61, 0x6c, 0x73, 0x2c, 0x20, 0x61, 0x6e, 0x64, 0x20,
            0x66, 0x6f, 0x78, 0x65, 0x73, 0x20, 0x69, 0x6e, 0x20, 0x74, 0x68, 0x65, 0x20, 0x74,
            0x61, 0x78, 0x6f, 0x6e, 0x6f, 0x6d, 0x69, 0x63, 0x20, 0x66, 0x61, 0x6d, 0x69, 0x6c,
            0x79, 0x20, 0x43, 0x61, 0x6e, 0x69, 0x64, 0x61, 0x65, 0x2e,
        ];
        c.cipher(&mut buffer);

        let expected = [
            0x45, 0x59, 0xab, 0xba, 0x4e, 0x48, 0xc1, 0x61, 0x02, 0xe8, 0xbb, 0x2c, 0x05, 0xe6,
            0x94, 0x7f, 0x50, 0xa7, 0x86, 0xde, 0x16, 0x2f, 0x9b, 0x0b, 0x7e, 0x59, 0x2a, 0x9b,
            0x53, 0xd0, 0xd4, 0xe9, 0x8d, 0x8d, 0x64, 0x10, 0xd5, 0x40, 0xa1, 0xa6, 0x37, 0x5b,
            0x26, 0xd8, 0x0d, 0xac, 0xe4, 0xfa, 0xb5, 0x23, 0x84, 0xc7, 0x31, 0xac, 0xbf, 0x16,
            0xa5, 0x92, 0x3c, 0x0c, 0x48, 0xd3, 0x57, 0x5d, 0x4d, 0x0d, 0x2c, 0x67, 0x3b, 0x66,
            0x6f, 0xaa, 0x73, 0x10, 0x61, 0x27, 0x77, 0x01, 0x09, 0x3a, 0x6b, 0xf7, 0xa1, 0x58,
            0xa8, 0x86, 0x42, 0x92, 0xa4, 0x1c, 0x48, 0xe3, 0xa9, 0xb4, 0xc0, 0xda, 0xec, 0xe0,
            0xf8, 0xd9, 0x8d, 0x0d, 0x7e, 0x05, 0xb3, 0x7a, 0x30, 0x7b, 0xbb, 0x66, 0x33, 0x31,
            0x64, 0xec, 0x9e, 0x1b, 0x24, 0xea, 0x0d, 0x6c, 0x3f, 0xfd, 0xdc, 0xec, 0x4f, 0x68,
            0xe7, 0x44, 0x30, 0x56, 0x19, 0x3a, 0x03, 0xc8, 0x10, 0xe1, 0x13, 0x44, 0xca, 0x06,
            0xd8, 0xed, 0x8a, 0x2b, 0xfb, 0x1e, 0x8d, 0x48, 0xcf, 0xa6, 0xbc, 0x0e, 0xb4, 0xe2,
            0x46, 0x4b, 0x74, 0x81, 0x42, 0x40, 0x7c, 0x9f, 0x43, 0x1a, 0xee, 0x76, 0x99, 0x60,
            0xe1, 0x5b, 0xa8, 0xb9, 0x68, 0x90, 0x46, 0x6e, 0xf2, 0x45, 0x75, 0x99, 0x85, 0x23,
            0x85, 0xc6, 0x61, 0xf7, 0x52, 0xce, 0x20, 0xf9, 0xda, 0x0c, 0x09, 0xab, 0x6b, 0x19,
            0xdf, 0x74, 0xe7, 0x6a, 0x95, 0x96, 0x74, 0x46, 0xf8, 0xd0, 0xfd, 0x41, 0x5e, 0x7b,
            0xee, 0x2a, 0x12, 0xa1, 0x14, 0xc2, 0x0e, 0xb5, 0x29, 0x2a, 0xe7, 0xa3, 0x49, 0xae,
            0x57, 0x78, 0x20, 0xd5, 0x52, 0x0a, 0x1f, 0x3f, 0xb6, 0x2a, 0x17, 0xce, 0x6a, 0x7e,
            0x68, 0xfa, 0x7c, 0x79, 0x11, 0x1d, 0x88, 0x60, 0x92, 0x0b, 0xc0, 0x48, 0xef, 0x43,
            0xfe, 0x84, 0x48, 0x6c, 0xcb, 0x87, 0xc2, 0x5f, 0x0a, 0xe0, 0x45, 0xf0, 0xcc, 0xe1,
            0xe7, 0x98, 0x9a, 0x9a, 0xa2, 0x20, 0xa2, 0x8b, 0xdd, 0x48, 0x27, 0xe7, 0x51, 0xa2,
            0x4a, 0x6d, 0x5c, 0x62, 0xd7, 0x90, 0xa6, 0x63, 0x93, 0xb9, 0x31, 0x11, 0xc1, 0xa5,
            0x5d, 0xd7, 0x42, 0x1a, 0x10, 0x18, 0x49, 0x74, 0xc7, 0xc5,
        ];

        assert_eq!(buffer, expected);
    }
}
