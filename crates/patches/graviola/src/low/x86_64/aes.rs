// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0
//
// References:
// - https://iacr.org/archive/fse2009/56650054/56650054.pdf (or)
// - https://www.intel.com/content/dam/doc/white-paper/advanced-encryption-standard-new-instructions-set-paper.pdf

use core::arch::x86_64::*;

use crate::low;

pub(crate) enum AesKey {
    Aes128(AesKey128),
    Aes256(AesKey256),
}

impl AesKey {
    /// Creates an AesKey.
    ///
    /// `key` must be 16 or 32 bytes in length (AES-192 not supported).
    pub(crate) fn new(key: &[u8]) -> Self {
        match key.len() {
            16 => Self::Aes128(AesKey128::new(key.try_into().unwrap())),
            32 => Self::Aes256(AesKey256::new(key.try_into().unwrap())),
            24 => panic!("aes-192 not supported"),
            _ => panic!("invalid aes key size"),
        }
    }

    pub(crate) fn encrypt_block(&self, inout: &mut [u8; 16]) {
        match self {
            Self::Aes128(a128) => a128.encrypt_block(inout),
            Self::Aes256(a256) => a256.encrypt_block(inout),
        }
    }

    pub(crate) fn round_keys(&self) -> (__m128i, &[__m128i], __m128i) {
        match self {
            Self::Aes128(a128) => (
                a128.round_keys[0],
                &a128.round_keys[1..10],
                a128.round_keys[10],
            ),
            Self::Aes256(a256) => (
                a256.round_keys[0],
                &a256.round_keys[1..14],
                a256.round_keys[14],
            ),
        }
    }

    #[target_feature(enable = "avx512f")]
    pub(crate) fn round_keys_512(&self) -> RoundKeys512 {
        match self {
            Self::Aes128(AesKey128 {
                round_keys: [a, b, c, d, e, f, g, h, i, j, k],
            }) => RoundKeys512::Aes128([
                _mm512_broadcast_i32x4(*a),
                _mm512_broadcast_i32x4(*b),
                _mm512_broadcast_i32x4(*c),
                _mm512_broadcast_i32x4(*d),
                _mm512_broadcast_i32x4(*e),
                _mm512_broadcast_i32x4(*f),
                _mm512_broadcast_i32x4(*g),
                _mm512_broadcast_i32x4(*h),
                _mm512_broadcast_i32x4(*i),
                _mm512_broadcast_i32x4(*j),
                _mm512_broadcast_i32x4(*k),
            ]),
            Self::Aes256(AesKey256 {
                round_keys: [a, b, c, d, e, f, g, h, i, j, k, l, m, n, o],
            }) => RoundKeys512::Aes256([
                _mm512_broadcast_i32x4(*a),
                _mm512_broadcast_i32x4(*b),
                _mm512_broadcast_i32x4(*c),
                _mm512_broadcast_i32x4(*d),
                _mm512_broadcast_i32x4(*e),
                _mm512_broadcast_i32x4(*f),
                _mm512_broadcast_i32x4(*g),
                _mm512_broadcast_i32x4(*h),
                _mm512_broadcast_i32x4(*i),
                _mm512_broadcast_i32x4(*j),
                _mm512_broadcast_i32x4(*k),
                _mm512_broadcast_i32x4(*l),
                _mm512_broadcast_i32x4(*m),
                _mm512_broadcast_i32x4(*n),
                _mm512_broadcast_i32x4(*o),
            ]),
        }
    }
}

impl Drop for AesKey {
    fn drop(&mut self) {
        low::zeroise_value(self);
    }
}

impl low::generic::zeroise::Zeroable for AesKey {}

#[expect(clippy::large_enum_variant)] // 200 bytes is a bad heuristic for large enums
pub(crate) enum RoundKeys512 {
    Aes128([__m512i; 11]),
    Aes256([__m512i; 15]),
}

impl RoundKeys512 {
    pub(crate) fn split(&self) -> (__m512i, &[__m512i], __m512i) {
        match self {
            Self::Aes128(rks) => (rks[0], &rks[1..10], rks[10]),
            Self::Aes256(rks) => (rks[0], &rks[1..14], rks[14]),
        }
    }
}

pub(crate) struct AesKey128 {
    round_keys: [__m128i; 10 + 1],
}

impl AesKey128 {
    pub(crate) fn new(key: &[u8; 16]) -> Self {
        let mut round_keys = [zero(); (10 + 1)];

        // SAFETY: this crate requires the `aes` & `avx` cpu features
        unsafe {
            aes128_expand(key, &mut round_keys);
        }

        Self { round_keys }
    }

    pub(crate) fn encrypt_block(&self, inout: &mut [u8; 16]) {
        // SAFETY: this crate requires the `aes` & `avx` cpu features
        unsafe { aes128_block(&self.round_keys, inout) }
    }
}

fn zero() -> __m128i {
    // SAFETY: this crate requires the `avx` cpu feature
    unsafe { _mm_setzero_si128() }
}

pub(crate) struct AesKey256 {
    round_keys: [__m128i; 14 + 1],
}

impl AesKey256 {
    pub(crate) fn new(key: &[u8; 32]) -> Self {
        let mut round_keys = [zero(); 14 + 1];

        // SAFETY: this crate requires the `aes` & `avx` cpu features
        unsafe {
            aes256_expand(key, &mut round_keys);
        }

        Self { round_keys }
    }

    pub(crate) fn encrypt_block(&self, inout: &mut [u8; 16]) {
        // SAFETY: this crate requires the `aes` & `avx` cpu features
        unsafe { aes256_block(&self.round_keys, inout) }
    }
}

macro_rules! expand_128 {
    ($rcon:literal, $t1:ident, $out:expr) => {
        // with [X3, _, X1,  _] = t1
        // t2 := [RotWord (SubWord (X3)) XOR RCON, SubWord (X3),
        //        RotWord (SubWord (X1)) XOR RCON, SubWord (X1)]
        let t2 = _mm_aeskeygenassist_si128($t1, $rcon);

        // select just high dqword, distribute to other dqwords
        let t2 = _mm_shuffle_epi32(t2, 0b11_11_11_11);

        let t3 = _mm_slli_si128($t1, 0x4); // nb. 4 bytes shift
        $t1 = _mm_xor_si128($t1, t3);

        let t3 = _mm_slli_si128(t3, 0x4);
        $t1 = _mm_xor_si128($t1, t3);

        let t3 = _mm_slli_si128(t3, 0x4);
        $t1 = _mm_xor_si128($t1, t3);

        $t1 = _mm_xor_si128($t1, t2);

        $out = $t1;
    };
}

#[target_feature(enable = "aes,avx")]
fn aes128_expand(key: &[u8; 16], out: &mut [__m128i; 11]) {
    // SAFETY: `key` is 16 bytes and can be loaded from
    let mut t1 = unsafe { _mm_lddqu_si128(key.as_ptr().cast()) };
    out[0] = t1;

    expand_128!(0x01, t1, out[1]);
    expand_128!(0x02, t1, out[2]);
    expand_128!(0x04, t1, out[3]);
    expand_128!(0x08, t1, out[4]);
    expand_128!(0x10, t1, out[5]);
    expand_128!(0x20, t1, out[6]);
    expand_128!(0x40, t1, out[7]);
    expand_128!(0x80, t1, out[8]);
    expand_128!(0x1b, t1, out[9]);
    expand_128!(0x36, t1, out[10]);
}

macro_rules! expand_256 {
    (Odd, $rcon:literal, $t1:ident, $t3:ident, $out:expr) => {
        let t2 = _mm_aeskeygenassist_si128($t3, $rcon);
        let t2 = _mm_shuffle_epi32(t2, 0b11_11_11_11);

        let t3 = _mm_slli_si128($t1, 0x4); // nb. 4 bytes shift
        $t1 = _mm_xor_si128($t1, t3);

        let t3 = _mm_slli_si128(t3, 0x4);
        $t1 = _mm_xor_si128($t1, t3);

        let t3 = _mm_slli_si128(t3, 0x4);
        $t1 = _mm_xor_si128($t1, t3);

        $t1 = _mm_xor_si128($t1, t2);

        $out = $t1;
    };
    (Even, $t1:ident, $t3:ident, $out:expr) => {
        let t4 = _mm_aeskeygenassist_si128($t1, 0);
        let t2 = _mm_shuffle_epi32(t4, 0b10_10_10_10); // choose SubWord(X3) term

        let t4 = _mm_slli_si128($t3, 0x4);
        $t3 = _mm_xor_si128($t3, t4);

        let t4 = _mm_slli_si128(t4, 0x4);
        $t3 = _mm_xor_si128($t3, t4);

        let t4 = _mm_slli_si128(t4, 0x4);
        $t3 = _mm_xor_si128($t3, t4);
        $t3 = _mm_xor_si128($t3, t2);

        $out = $t3;
    };
}

#[target_feature(enable = "aes,avx")]
fn aes256_expand(key: &[u8; 32], out: &mut [__m128i; 15]) {
    // SAFETY: `key` is 32 bytes and readable
    let (mut t1, mut t3) = unsafe {
        (
            _mm_lddqu_si128(key.as_ptr().cast()),
            _mm_lddqu_si128(key[16..].as_ptr().cast()),
        )
    };
    out[0] = t1;
    out[1] = t3;

    // nb. 'odd' rounds in units of Nk have an rcon term (equivalent
    // to all rounds of 128-bit key expansion), 'even' rounds do not
    expand_256!(Odd, 0x01, t1, t3, out[2]);
    expand_256!(Even, t1, t3, out[3]);
    expand_256!(Odd, 0x02, t1, t3, out[4]);
    expand_256!(Even, t1, t3, out[5]);
    expand_256!(Odd, 0x04, t1, t3, out[6]);
    expand_256!(Even, t1, t3, out[7]);
    expand_256!(Odd, 0x08, t1, t3, out[8]);
    expand_256!(Even, t1, t3, out[9]);
    expand_256!(Odd, 0x10, t1, t3, out[10]);
    expand_256!(Even, t1, t3, out[11]);
    expand_256!(Odd, 0x20, t1, t3, out[12]);
    expand_256!(Even, t1, t3, out[13]);
    expand_256!(Odd, 0x40, t1, t3, out[14]);
}

#[target_feature(enable = "aes,avx")]
fn aes128_block(round_keys: &[__m128i; 11], block_inout: &mut [u8; 16]) {
    // SAFETY: `block_inout` is 16 bytes and readable
    let block = unsafe { _mm_lddqu_si128(block_inout.as_ptr().cast()) };
    let block = _mm_xor_si128(block, round_keys[0]);
    let block = _mm_aesenc_si128(block, round_keys[1]);
    let block = _mm_aesenc_si128(block, round_keys[2]);
    let block = _mm_aesenc_si128(block, round_keys[3]);
    let block = _mm_aesenc_si128(block, round_keys[4]);
    let block = _mm_aesenc_si128(block, round_keys[5]);
    let block = _mm_aesenc_si128(block, round_keys[6]);
    let block = _mm_aesenc_si128(block, round_keys[7]);
    let block = _mm_aesenc_si128(block, round_keys[8]);
    let block = _mm_aesenc_si128(block, round_keys[9]);
    let block = _mm_aesenclast_si128(block, round_keys[10]);
    // SAFETY: `block_inout` is 16 bytes and writeable
    unsafe { _mm_storeu_si128(block_inout.as_mut_ptr().cast(), block) };
}

#[target_feature(enable = "aes,avx")]
fn aes256_block(round_keys: &[__m128i; 15], block_inout: &mut [u8; 16]) {
    // SAFETY: `block_inout` is 16 bytes and readable
    let block = unsafe { _mm_lddqu_si128(block_inout.as_ptr().cast()) };
    let block = _mm_xor_si128(block, round_keys[0]);
    let block = _mm_aesenc_si128(block, round_keys[1]);
    let block = _mm_aesenc_si128(block, round_keys[2]);
    let block = _mm_aesenc_si128(block, round_keys[3]);
    let block = _mm_aesenc_si128(block, round_keys[4]);
    let block = _mm_aesenc_si128(block, round_keys[5]);
    let block = _mm_aesenc_si128(block, round_keys[6]);
    let block = _mm_aesenc_si128(block, round_keys[7]);
    let block = _mm_aesenc_si128(block, round_keys[8]);
    let block = _mm_aesenc_si128(block, round_keys[9]);
    let block = _mm_aesenc_si128(block, round_keys[10]);
    let block = _mm_aesenc_si128(block, round_keys[11]);
    let block = _mm_aesenc_si128(block, round_keys[12]);
    let block = _mm_aesenc_si128(block, round_keys[13]);
    let block = _mm_aesenclast_si128(block, round_keys[14]);
    // SAFETY: `block_inout` is 16 bytes and writeable
    unsafe { _mm_storeu_si128(block_inout.as_mut_ptr().cast(), block) };
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #[cfg(panic = "unwind")]
    use std::panic;

    use super::*;

    fn to_u128(v: __m128i) -> u128 {
        let mut u = 0;
        // SAFETY: __m128i has the same layout/meaning as u128
        unsafe {
            _mm_store_si128((&mut u as *mut u128).cast(), v);
        }
        u
    }

    // these test vectors from FIPS-197 appendices A.1 - A.3.

    #[test]
    #[cfg(panic = "unwind")]
    fn test_construct() {
        // The generic AesKey::new wrapper should create the proper subtype of key when called
        // with a 128- or 256-bit value and should fail with any other argument length.
        let input: [u8; 33] = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b,
            0x1c, 0x1d, 0x1e, 0x1f, 0xff,
        ];
        for len in 0..=input.len() {
            let num_bits = len * 8;
            match panic::catch_unwind(|| AesKey::new(&input[..len])) {
                Ok(key) => match key {
                    AesKey::Aes128(_) => assert_eq!(num_bits, 128),
                    AesKey::Aes256(_) => assert_eq!(num_bits, 256),
                },
                Err(_) => {
                    assert_ne!(num_bits, 128);
                    assert_ne!(num_bits, 256);
                }
            }
        }
    }

    #[test]
    fn test_key_expansion_128() {
        let context = AesKey128::new(&[
            0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6, 0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf,
            0x4f, 0x3c,
        ]);

        let expected = [
            0x2b7e1516_28aed2a6_abf71588_09cf4f3c,
            0xa0fafe17_88542cb1_23a33939_2a6c7605,
            0xf2c295f2_7a96b943_5935807a_7359f67f,
            0x3d80477d_4716fe3e_1e237e44_6d7a883b,
            0xef44a541_a8525b7f_b671253b_db0bad00,
            0xd4d1c6f8_7c839d87_caf2b8bc_11f915bc,
            0x6d88a37a_110b3efd_dbf98641_ca0093fd,
            0x4e54f70e_5f5fc9f3_84a64fb2_4ea6dc4f,
            0xead27321_b58dbad2_312bf560_7f8d292f,
            0xac7766f3_19fadc21_28d12941_575c006e,
            0xd014f9a8_c9ee2589_e13f0cc8_b6630ca6,
        ];

        for (i, expect) in expected.into_iter().enumerate() {
            assert_eq!(to_u128(context.round_keys[i]).swap_bytes(), expect);
        }
    }

    #[test]
    fn test_key_expansion_256() {
        let context = AesKey256::new(&[
            0x60, 0x3d, 0xeb, 0x10, 0x15, 0xca, 0x71, 0xbe, 0x2b, 0x73, 0xae, 0xf0, 0x85, 0x7d,
            0x77, 0x81, 0x1f, 0x35, 0x2c, 0x07, 0x3b, 0x61, 0x08, 0xd7, 0x2d, 0x98, 0x10, 0xa3,
            0x09, 0x14, 0xdf, 0xf4,
        ]);

        let expected = [
            0x603deb10_15ca71be_2b73aef0_857d7781,
            0x1f352c07_3b6108d7_2d9810a3_0914dff4,
            0x9ba35411_8e6925af_a51a8b5f_2067fcde,
            0xa8b09c1a_93d194cd_be49846e_b75d5b9a,
            0xd59aecb8_5bf3c917_fee94248_de8ebe96,
            0xb5a9328a_2678a647_98312229_2f6c79b3,
            0x812c81ad_dadf48ba_24360af2_fab8b464,
            0x98c5bfc9_bebd198e_268c3ba7_09e04214,
            0x68007bac_b2df3316_96e939e4_6c518d80,
            0xc814e204_76a9fb8a_5025c02d_59c58239,
            0xde136967_6ccc5a71_fa256395_9674ee15,
            0x5886ca5d_2e2f31d7_7e0af1fa_27cf73c3,
            0x749c47ab_18501dda_e2757e4f_7401905a,
            0xcafaaae3_e4d59b34_9adf6ace_bd10190d,
            0xfe4890d1_e6188d0b_046df344_706c631e,
        ];

        for (i, expect) in expected.into_iter().enumerate() {
            assert_eq!(to_u128(context.round_keys[i]).swap_bytes(), expect);
        }
    }

    #[test]
    fn test_block_128() {
        let context = AesKey128::new(&[
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f,
        ]);
        let mut block = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ];
        context.encrypt_block(&mut block);
        assert_eq!(
            block,
            [
                0x69, 0xc4, 0xe0, 0xd8, 0x6a, 0x7b, 0x04, 0x30, 0xd8, 0xcd, 0xb7, 0x80, 0x70, 0xb4,
                0xc5, 0x5a
            ]
        );
    }

    #[test]
    fn test_block_256() {
        let context = AesKey256::new(&[
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b,
            0x1c, 0x1d, 0x1e, 0x1f,
        ]);
        let mut block = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ];
        context.encrypt_block(&mut block);
        assert_eq!(
            block,
            [
                0x8e, 0xa2, 0xb7, 0xca, 0x51, 0x67, 0x45, 0xbf, 0xea, 0xfc, 0x49, 0x90, 0x4b, 0x49,
                0x60, 0x89
            ]
        );
    }
}
