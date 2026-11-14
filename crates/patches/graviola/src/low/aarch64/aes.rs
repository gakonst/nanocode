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

