// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0
//
// refs:
// - https://documentation-service.arm.com/static/5f202270bb903e39c84d7ede
// - https://documentation-service.arm.com/static/65fdad3c1bc22b03bca90781
//   (sections C7.2.8, C7.2.10)
//
// cf. the x86_64 version, on which this one is based.

use core::arch::aarch64::*;

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
}

impl Drop for AesKey {
    fn drop(&mut self) {
        low::zeroise_value(self);
    }
}

impl low::generic::zeroise::Zeroable for AesKey {}

pub(crate) struct AesKey128 {
    round_keys: [uint8x16_t; 10 + 1],
}

impl AesKey128 {
    pub(crate) fn new(key: &[u8; 16]) -> Self {
        // we do the key expansion in 32 bit words, then convert
        // uint8x16_t as the final step.
        let mut rk32 = [0; (10 + 1) * 4];
        rk32[0] = u32::from_be_bytes(key[0..4].try_into().unwrap());
        rk32[1] = u32::from_be_bytes(key[4..8].try_into().unwrap());
        rk32[2] = u32::from_be_bytes(key[8..12].try_into().unwrap());
        rk32[3] = u32::from_be_bytes(key[12..16].try_into().unwrap());

        for r in 1..11 {
            let (prev, current) = &mut rk32[((r - 1) * 4)..((r + 1) * 4)].split_at_mut(4);
            current[0] = sub_word(prev[3].rotate_left(8)) ^ (RCON[r - 1] << 24) ^ prev[0];
            current[1] = current[0] ^ prev[1];
            current[2] = current[1] ^ prev[2];
            current[3] = current[2] ^ prev[3];
        }

        let mut round_keys = [zero(); 10 + 1];
        for (i, rk) in rk32.chunks(4).enumerate() {
            // SAFETY: `rk` is 128-bit in size; `vld1q_u8` has no alignment req.
            round_keys[i] = unsafe { vrev32q_u8(vld1q_u8(rk.as_ptr().cast())) };
        }

        Self { round_keys }
    }

    pub(crate) fn encrypt_block(&self, inout: &mut [u8; 16]) {
        // SAFETY: this crate requires the `aes` cpu feature
        unsafe { aes128_block(&self.round_keys, inout) }
    }

    pub(crate) fn round_keys(&self) -> &[uint8x16_t; 11] {
        &self.round_keys
    }
}

pub(crate) struct AesKey256 {
    round_keys: [uint8x16_t; 14 + 1],
}

impl AesKey256 {
    pub(crate) fn new(key: &[u8; 32]) -> Self {
        let mut rk32 = [0; (14 + 1) * 4];
        rk32[0] = u32::from_be_bytes(key[0..4].try_into().unwrap());
        rk32[1] = u32::from_be_bytes(key[4..8].try_into().unwrap());
        rk32[2] = u32::from_be_bytes(key[8..12].try_into().unwrap());
        rk32[3] = u32::from_be_bytes(key[12..16].try_into().unwrap());
        rk32[4] = u32::from_be_bytes(key[16..20].try_into().unwrap());
        rk32[5] = u32::from_be_bytes(key[20..24].try_into().unwrap());
        rk32[6] = u32::from_be_bytes(key[24..28].try_into().unwrap());
        rk32[7] = u32::from_be_bytes(key[28..32].try_into().unwrap());

        for r in 2..15 {
            let (prev, current) = &mut rk32[((r - 2) * 4)..((r + 1) * 4)].split_at_mut(8);
            if r & 1 == 1 {
                // odd round
                current[0] = sub_word(prev[7]) ^ prev[0];
            } else {
                // even round
                current[0] = sub_word(prev[7].rotate_left(8)) ^ (RCON[(r - 1) / 2] << 24) ^ prev[0];
            }
            current[1] = current[0] ^ prev[1];
            current[2] = current[1] ^ prev[2];
            current[3] = current[2] ^ prev[3];
        }

        let mut round_keys = [zero(); 14 + 1];
        for (i, rk) in rk32.chunks(4).enumerate() {
            // SAFETY: `rk` is 128-bit in size; `vld1q_u8` has no alignment req.
            round_keys[i] = unsafe { vrev32q_u8(vld1q_u8(rk.as_ptr().cast())) };
        }

        Self { round_keys }
    }

    pub(crate) fn encrypt_block(&self, inout: &mut [u8; 16]) {
        // SAFETY: this crate requires the `aes` cpu feature
        unsafe { aes256_block(&self.round_keys, inout) }
    }

    pub(crate) fn round_keys(&self) -> &[uint8x16_t; 15] {
        &self.round_keys
    }
}

fn zero() -> uint8x16_t {
    // SAFETY: this crate requires the `neon` cpu feature
    unsafe { vdupq_n_u8(0) }
}

fn sub_word(w: u32) -> u32 {
    // SAFETY: this crate requires the `aes` cpu feature
    unsafe { _sub_word(w) }
}

#[target_feature(enable = "aes")]
fn _sub_word(w: u32) -> u32 {
    // we have the `aese` instruction, which is
    // `sub_word(shift_rows(w), S)`. however, fortunately
    // `shift_rows` is the identity for the first 32-bit word

    let t2 = vdupq_n_u32(w);
    let t2 = vreinterpretq_u8_u32(t2);
    let z = vdupq_n_u8(0);
    let t2 = vaeseq_u8(t2, z);
    let mut out: u32 = 0;
    let t2 = vreinterpretq_u32_u8(t2);
    // SAFETY: out is a writable u32
    unsafe { vst1q_lane_u32(&mut out, t2, 0) };
    out
}

const RCON: [u32; 10] = [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1b, 0x36];

#[target_feature(enable = "aes")]
fn aes128_block(round_keys: &[uint8x16_t; 11], block_inout: &mut [u8]) {
    // SAFETY: `block_inout` is 16 bytes and readable
    let block = unsafe { vld1q_u8(block_inout.as_ptr().cast()) };
    let block = _aes128_block(round_keys, block);
    // SAFETY: `block_inout` is 16 bytes and writable
    unsafe { vst1q_u8(block_inout.as_mut_ptr().cast(), block) };
}

#[target_feature(enable = "aes")]
#[inline]
pub(crate) fn _aes128_block(round_keys: &[uint8x16_t; 11], block: uint8x16_t) -> uint8x16_t {
    let block = vaeseq_u8(block, round_keys[0]);
    let block = vaesmcq_u8(block);
    let block = vaeseq_u8(block, round_keys[1]);
    let block = vaesmcq_u8(block);
    let block = vaeseq_u8(block, round_keys[2]);
    let block = vaesmcq_u8(block);
    let block = vaeseq_u8(block, round_keys[3]);
    let block = vaesmcq_u8(block);
    let block = vaeseq_u8(block, round_keys[4]);
    let block = vaesmcq_u8(block);
    let block = vaeseq_u8(block, round_keys[5]);
    let block = vaesmcq_u8(block);
    let block = vaeseq_u8(block, round_keys[6]);
    let block = vaesmcq_u8(block);
    let block = vaeseq_u8(block, round_keys[7]);
    let block = vaesmcq_u8(block);
    let block = vaeseq_u8(block, round_keys[8]);
    let block = vaesmcq_u8(block);
    let block = vaeseq_u8(block, round_keys[9]);
    veorq_u8(block, round_keys[10])
}

macro_rules! round_8 {
    ($b0:ident, $b1:ident, $b2:ident, $b3:ident, $b4:ident, $b5:ident, $b6:ident, $b7:ident, $rk:expr) => {
        let rk = $rk;
        $b0 = vaeseq_u8($b0, rk);
        $b0 = vaesmcq_u8($b0);
        $b1 = vaeseq_u8($b1, rk);
        $b1 = vaesmcq_u8($b1);
        $b2 = vaeseq_u8($b2, rk);
        $b2 = vaesmcq_u8($b2);
        $b3 = vaeseq_u8($b3, rk);
        $b3 = vaesmcq_u8($b3);
        $b4 = vaeseq_u8($b4, rk);
        $b4 = vaesmcq_u8($b4);
        $b5 = vaeseq_u8($b5, rk);
        $b5 = vaesmcq_u8($b5);
        $b6 = vaeseq_u8($b6, rk);
        $b6 = vaesmcq_u8($b6);
        $b7 = vaeseq_u8($b7, rk);
        $b7 = vaesmcq_u8($b7);
    };
}

#[target_feature(enable = "aes")]
#[inline]
pub(crate) fn _aes128_8_blocks(
    round_keys: &[uint8x16_t; 11],
    mut b0: uint8x16_t,
    mut b1: uint8x16_t,
    mut b2: uint8x16_t,
    mut b3: uint8x16_t,
    mut b4: uint8x16_t,
    mut b5: uint8x16_t,
    mut b6: uint8x16_t,
    mut b7: uint8x16_t,
) -> (
    uint8x16_t,
    uint8x16_t,
    uint8x16_t,
    uint8x16_t,
    uint8x16_t,
    uint8x16_t,
    uint8x16_t,
    uint8x16_t,
) {
    round_8!(b0, b1, b2, b3, b4, b5, b6, b7, round_keys[0]);
    round_8!(b0, b1, b2, b3, b4, b5, b6, b7, round_keys[1]);
    round_8!(b0, b1, b2, b3, b4, b5, b6, b7, round_keys[2]);
    round_8!(b0, b1, b2, b3, b4, b5, b6, b7, round_keys[3]);
    round_8!(b0, b1, b2, b3, b4, b5, b6, b7, round_keys[4]);
    round_8!(b0, b1, b2, b3, b4, b5, b6, b7, round_keys[5]);
    round_8!(b0, b1, b2, b3, b4, b5, b6, b7, round_keys[6]);
    round_8!(b0, b1, b2, b3, b4, b5, b6, b7, round_keys[7]);
    round_8!(b0, b1, b2, b3, b4, b5, b6, b7, round_keys[8]);

    let b0 = vaeseq_u8(b0, round_keys[9]);
    let b1 = vaeseq_u8(b1, round_keys[9]);
    let b2 = vaeseq_u8(b2, round_keys[9]);
    let b3 = vaeseq_u8(b3, round_keys[9]);
    let b4 = vaeseq_u8(b4, round_keys[9]);
    let b5 = vaeseq_u8(b5, round_keys[9]);
    let b6 = vaeseq_u8(b6, round_keys[9]);
    let b7 = vaeseq_u8(b7, round_keys[9]);
    (
        veorq_u8(b0, round_keys[10]),
        veorq_u8(b1, round_keys[10]),
        veorq_u8(b2, round_keys[10]),
        veorq_u8(b3, round_keys[10]),
        veorq_u8(b4, round_keys[10]),
        veorq_u8(b5, round_keys[10]),
        veorq_u8(b6, round_keys[10]),
        veorq_u8(b7, round_keys[10]),
    )
}

#[target_feature(enable = "aes")]
fn aes256_block(round_keys: &[uint8x16_t; 15], block_inout: &mut [u8; 16]) {
    // SAFETY: `block_inout` is 16 bytes and readable
    let block = unsafe { vld1q_u8(block_inout.as_ptr().cast()) };
    let block = _aes256_block(round_keys, block);
    // SAFETY: `block_inout` is 16 bytes and writable
    unsafe { vst1q_u8(block_inout.as_mut_ptr().cast(), block) };
}

#[target_feature(enable = "aes")]
#[inline]
pub(crate) fn _aes256_block(round_keys: &[uint8x16_t; 15], block: uint8x16_t) -> uint8x16_t {
    let block = vaeseq_u8(block, round_keys[0]);
    let block = vaesmcq_u8(block);
    let block = vaeseq_u8(block, round_keys[1]);
    let block = vaesmcq_u8(block);
    let block = vaeseq_u8(block, round_keys[2]);
    let block = vaesmcq_u8(block);
    let block = vaeseq_u8(block, round_keys[3]);
    let block = vaesmcq_u8(block);
    let block = vaeseq_u8(block, round_keys[4]);
    let block = vaesmcq_u8(block);
    let block = vaeseq_u8(block, round_keys[5]);
    let block = vaesmcq_u8(block);
    let block = vaeseq_u8(block, round_keys[6]);
    let block = vaesmcq_u8(block);
    let block = vaeseq_u8(block, round_keys[7]);
    let block = vaesmcq_u8(block);
    let block = vaeseq_u8(block, round_keys[8]);
    let block = vaesmcq_u8(block);
    let block = vaeseq_u8(block, round_keys[9]);
    let block = vaesmcq_u8(block);
    let block = vaeseq_u8(block, round_keys[10]);
    let block = vaesmcq_u8(block);
    let block = vaeseq_u8(block, round_keys[11]);
    let block = vaesmcq_u8(block);
    let block = vaeseq_u8(block, round_keys[12]);
    let block = vaesmcq_u8(block);
    let block = vaeseq_u8(block, round_keys[13]);
    veorq_u8(block, round_keys[14])
}

#[target_feature(enable = "aes")]
#[inline]
pub(crate) fn _aes256_8_blocks(
    round_keys: &[uint8x16_t; 15],
    mut b0: uint8x16_t,
    mut b1: uint8x16_t,
    mut b2: uint8x16_t,
    mut b3: uint8x16_t,
    mut b4: uint8x16_t,
    mut b5: uint8x16_t,
    mut b6: uint8x16_t,
    mut b7: uint8x16_t,
) -> (
    uint8x16_t,
    uint8x16_t,
    uint8x16_t,
    uint8x16_t,
    uint8x16_t,
    uint8x16_t,
    uint8x16_t,
    uint8x16_t,
) {
    round_8!(b0, b1, b2, b3, b4, b5, b6, b7, round_keys[0]);
    round_8!(b0, b1, b2, b3, b4, b5, b6, b7, round_keys[1]);
    round_8!(b0, b1, b2, b3, b4, b5, b6, b7, round_keys[2]);
    round_8!(b0, b1, b2, b3, b4, b5, b6, b7, round_keys[3]);
    round_8!(b0, b1, b2, b3, b4, b5, b6, b7, round_keys[4]);
    round_8!(b0, b1, b2, b3, b4, b5, b6, b7, round_keys[5]);
    round_8!(b0, b1, b2, b3, b4, b5, b6, b7, round_keys[6]);
    round_8!(b0, b1, b2, b3, b4, b5, b6, b7, round_keys[7]);
    round_8!(b0, b1, b2, b3, b4, b5, b6, b7, round_keys[8]);
    round_8!(b0, b1, b2, b3, b4, b5, b6, b7, round_keys[9]);
    round_8!(b0, b1, b2, b3, b4, b5, b6, b7, round_keys[10]);
    round_8!(b0, b1, b2, b3, b4, b5, b6, b7, round_keys[11]);
    round_8!(b0, b1, b2, b3, b4, b5, b6, b7, round_keys[12]);

    let b0 = vaeseq_u8(b0, round_keys[13]);
    let b1 = vaeseq_u8(b1, round_keys[13]);
    let b2 = vaeseq_u8(b2, round_keys[13]);
    let b3 = vaeseq_u8(b3, round_keys[13]);
    let b4 = vaeseq_u8(b4, round_keys[13]);
    let b5 = vaeseq_u8(b5, round_keys[13]);
    let b6 = vaeseq_u8(b6, round_keys[13]);
    let b7 = vaeseq_u8(b7, round_keys[13]);
    (
        veorq_u8(b0, round_keys[14]),
        veorq_u8(b1, round_keys[14]),
        veorq_u8(b2, round_keys[14]),
        veorq_u8(b3, round_keys[14]),
        veorq_u8(b4, round_keys[14]),
        veorq_u8(b5, round_keys[14]),
        veorq_u8(b6, round_keys[14]),
        veorq_u8(b7, round_keys[14]),
    )
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #[cfg(panic = "unwind")]
    use std::panic;

    use super::*;

    fn to_u128(v: uint8x16_t) -> u128 {
        let mut buf = [0u8; 16];
        // SAFETY: This crate requires the `neon` cpu feature.
        unsafe {
            vst1q_u8(buf.as_mut_ptr(), v);
        }
        u128::from_be_bytes(buf)
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
            assert_eq!(to_u128(context.round_keys[i]), expect);
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
            assert_eq!(to_u128(context.round_keys[i]), expect);
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
