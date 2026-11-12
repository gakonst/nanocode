use core::hash::{BuildHasher, Hasher};
use std::collections::{HashMap, HashSet};

pub type FxHashMap<K, V> = HashMap<K, V, FxBuildHasher>;

pub type FxHashSet<V> = HashSet<V, FxBuildHasher>;

#[cfg(target_pointer_width = "64")]
const MULTIPLIER: usize = 0xf135_7aea_2e62_a9c5;
#[cfg(target_pointer_width = "32")]
const MULTIPLIER: usize = 0x93d7_65dd;

#[cfg(target_pointer_width = "64")]
const FINISH_ROTATION_BITS: u32 = 26;
#[cfg(target_pointer_width = "32")]
const FINISH_ROTATION_BITS: u32 = 15;

const BYTES_SEED_FIRST: u64 = 0x243f_6a88_85a3_08d3;
const BYTES_SEED_SECOND: u64 = 0x1319_8a2e_0370_7344;
const BYTES_ZERO_COLLAPSE_GUARD: u64 = 0xa409_3822_299f_31d0;
const BYTES_SHORT_MAX: usize = 16;
const BYTES_CHUNK: usize = 16;
const WORD_BYTES: usize = 8;
const HALF_WORD_BYTES: usize = 4;

#[derive(Clone, Default)]
pub struct FxHasher {
    hash: usize,
}

impl FxHasher {
    #[inline]
    const fn add_to_hash(&mut self, value: usize) {
        self.hash = self.hash.wrapping_add(value).wrapping_mul(MULTIPLIER);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        self.write_u64(hash_bytes(bytes));
    }

    #[inline]
    fn write_u8(&mut self, value: u8) {
        self.add_to_hash(value as usize);
    }

    #[inline]
    fn write_u16(&mut self, value: u16) {
        self.add_to_hash(value as usize);
    }

    #[inline]
    fn write_u32(&mut self, value: u32) {
        self.add_to_hash(value as usize);
    }

    #[inline]
    #[allow(clippy::cast_possible_truncation)]
    fn write_u64(&mut self, value: u64) {
        self.add_to_hash(value as usize);
        #[cfg(target_pointer_width = "32")]
        self.add_to_hash((value >> 32) as usize);
    }

    #[inline]
    #[allow(clippy::cast_possible_truncation)]
    fn write_u128(&mut self, value: u128) {
        self.add_to_hash(value as usize);
        #[cfg(target_pointer_width = "32")]
        self.add_to_hash((value >> 32) as usize);
        self.add_to_hash((value >> 64) as usize);
        #[cfg(target_pointer_width = "32")]
        self.add_to_hash((value >> 96) as usize);
    }

    #[inline]
    fn write_usize(&mut self, value: usize) {
        self.add_to_hash(value);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.hash.rotate_left(FINISH_ROTATION_BITS) as u64
    }
}

#[inline]
#[allow(clippy::cast_possible_truncation)]
const fn multiply_mix(left: u64, right: u64) -> u64 {
    if cfg!(any(
        all(
            target_pointer_width = "64",
            not(any(target_arch = "sparc64", target_arch = "wasm64")),
        ),
        target_arch = "aarch64",
        target_arch = "x86_64",
        all(target_family = "wasm", target_feature = "wide-arithmetic"),
    )) {
        let full = (left as u128).wrapping_mul(right as u128);
        (full as u64) ^ ((full >> 64) as u64)
    } else {
        let left_low = left as u32;
        let right_low = right as u32;
        let left_high = (left >> 32) as u32;
        let right_high = (right >> 32) as u32;
        let first = (left_low as u64).wrapping_mul(right_high as u64);
        let second = (left_high as u64).wrapping_mul(right_low as u64);
        first ^ second.rotate_right(32)
    }
}

#[inline]
fn word(bytes: &[u8], at: usize) -> u64 {
    let mut word = [0; WORD_BYTES];
    word.copy_from_slice(&bytes[at..at + WORD_BYTES]);
    u64::from_le_bytes(word)
}

#[inline]
fn half_word(bytes: &[u8], at: usize) -> u64 {
    let mut half = [0; HALF_WORD_BYTES];
    half.copy_from_slice(&bytes[at..at + HALF_WORD_BYTES]);
    u64::from(u32::from_le_bytes(half))
}

#[inline]
fn hash_bytes(bytes: &[u8]) -> u64 {
    let length = bytes.len();
    let mut first = BYTES_SEED_FIRST;
    let mut second = BYTES_SEED_SECOND;
    if length <= BYTES_SHORT_MAX {
        if length >= WORD_BYTES {
            first ^= word(bytes, 0);
            second ^= word(bytes, length - WORD_BYTES);
        } else if length >= HALF_WORD_BYTES {
            first ^= half_word(bytes, 0);
            second ^= half_word(bytes, length - HALF_WORD_BYTES);
        } else if length > 0 {
            first ^= u64::from(bytes[0]);
            second ^= (u64::from(bytes[length - 1]) << 8) | u64::from(bytes[length / 2]);
        }
    } else {
        let mut bulk = &bytes[..length - 1];
        while let Some((chunk, rest)) = bulk.split_first_chunk::<BYTES_CHUNK>() {
            let mixed = multiply_mix(
                first ^ word(chunk, 0),
                BYTES_ZERO_COLLAPSE_GUARD ^ word(chunk, WORD_BYTES),
            );
            first = second;
            second = mixed;
            bulk = rest;
        }
        first ^= word(bytes, length - BYTES_CHUNK);
        second ^= word(bytes, length - WORD_BYTES);
    }
    multiply_mix(first, second) ^ (length as u64)
}

#[derive(Copy, Clone, Default)]
pub struct FxBuildHasher;

impl BuildHasher for FxBuildHasher {
    type Hasher = FxHasher;

    fn build_hasher(&self) -> FxHasher {
        FxHasher::default()
    }
}
