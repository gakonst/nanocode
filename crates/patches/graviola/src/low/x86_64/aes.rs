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

