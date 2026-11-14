// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0
//
//! Ref. <https://www.intel.com/content/dam/www/public/us/en/documents/white-papers/communications-ia-cryptographic-paper.pdf>

use core::arch::x86_64::*;
use core::mem;

use super::aes::AesKey;
use super::cpu::HaveAvx512ForAesGcm;
use super::ghash::{self, Ghash};
use crate::low::ghash::{GhashTable, GhashTableAvx512};

pub(crate) fn encrypt(
    key: &AesKey,
    ghash: &mut Ghash<'_>,
    initial_counter: &[u8; 16],
    aad: &[u8],
    cipher_inout: &mut [u8],
) {
    if let (true, GhashTable::Avx512(GhashTableAvx512 { token, .. })) =
        (cipher_inout.len() >= AVX512_MINIMUM_CIPHER_LEN, ghash.table)
    {
        // SAFETY: this crate requires the `aes`, `sse3`, `ssse3`, `pclmulqdq`, `avx` and `avx2` cpu features;
        // `token` proves the remaining features were dynamically checked
        unsafe { _cipher_avx512::<true>(key, ghash, initial_counter, aad, cipher_inout, *token) };
        return;
    }

    // SAFETY: this crate requires the `aes`, `ssse3`, `pclmulqdq` and `avx` cpu features
    unsafe { _cipher::<true>(key, ghash, initial_counter, aad, cipher_inout) }
}

pub(crate) fn decrypt(
    key: &AesKey,
    ghash: &mut Ghash<'_>,
    initial_counter: &[u8; 16],
    aad: &[u8],
    cipher_inout: &mut [u8],
) {
    if let (true, GhashTable::Avx512(GhashTableAvx512 { token, .. })) =
        (cipher_inout.len() >= AVX512_MINIMUM_CIPHER_LEN, ghash.table)
    {
        // SAFETY: this crate requires the `aes`, `sse3`, `ssse3`, `pclmulqdq`, `avx` and `avx2` cpu features;
        // `token` proves the remaining features were dynamically checked
        unsafe { _cipher_avx512::<false>(key, ghash, initial_counter, aad, cipher_inout, *token) };
        return;
    }
    // SAFETY: this crate requires the `aes`, `ssse3`, `pclmulqdq` and `avx` cpu features
    unsafe { _cipher::<false>(key, ghash, initial_counter, aad, cipher_inout) }
}

#[target_feature(enable = "aes,ssse3,pclmulqdq,avx")]
fn _cipher<const ENC: bool>(
    key: &AesKey,
    ghash: &mut Ghash<'_>,
    initial_counter: &[u8; 16],
    aad: &[u8],
    cipher_inout: &mut [u8],
) {
    ghash.add(aad);

    let (rk_first, rks, rk_last) = key.round_keys();

    let mut counter = Counter::new(initial_counter);
    let mut by8_iter = cipher_inout.chunks_exact_mut(128);
    let avx_ghash_table = ghash.table.avx();

    for blocks in by8_iter.by_ref() {
        // prefetch to avoid any stall later
        super::cpu::prefetch(blocks, 64);

        let c1 = counter.next();
        let c2 = counter.next();
        let c3 = counter.next();
        let c4 = counter.next();
        let c5 = counter.next();
        let c6 = counter.next();
        let c7 = counter.next();
        let c8 = counter.next();

        let mut c1 = _mm_xor_si128(c1, rk_first);
        let mut c2 = _mm_xor_si128(c2, rk_first);
        let mut c3 = _mm_xor_si128(c3, rk_first);
        let mut c4 = _mm_xor_si128(c4, rk_first);
        let mut c5 = _mm_xor_si128(c5, rk_first);
        let mut c6 = _mm_xor_si128(c6, rk_first);
        let mut c7 = _mm_xor_si128(c7, rk_first);
        let mut c8 = _mm_xor_si128(c8, rk_first);

        for rk in rks {
            c1 = _mm_aesenc_si128(c1, *rk);
            c2 = _mm_aesenc_si128(c2, *rk);
            c3 = _mm_aesenc_si128(c3, *rk);
            c4 = _mm_aesenc_si128(c4, *rk);
            c5 = _mm_aesenc_si128(c5, *rk);
            c6 = _mm_aesenc_si128(c6, *rk);
            c7 = _mm_aesenc_si128(c7, *rk);
            c8 = _mm_aesenc_si128(c8, *rk);
        }

        let c1 = _mm_aesenclast_si128(c1, rk_last);
        let c2 = _mm_aesenclast_si128(c2, rk_last);
        let c3 = _mm_aesenclast_si128(c3, rk_last);
        let c4 = _mm_aesenclast_si128(c4, rk_last);
        let c5 = _mm_aesenclast_si128(c5, rk_last);
        let c6 = _mm_aesenclast_si128(c6, rk_last);
        let c7 = _mm_aesenclast_si128(c7, rk_last);
        let c8 = _mm_aesenclast_si128(c8, rk_last);

        // SAFETY: `blocks` is 128 bytes and readable
        let (p1, p2, p3, p4, p5, p6, p7, p8) = unsafe {
            (
                _mm_loadu_si128(blocks.as_ptr().add(0).cast()),
                _mm_loadu_si128(blocks.as_ptr().add(16).cast()),
                _mm_loadu_si128(blocks.as_ptr().add(32).cast()),
                _mm_loadu_si128(blocks.as_ptr().add(48).cast()),
                _mm_loadu_si128(blocks.as_ptr().add(64).cast()),
                _mm_loadu_si128(blocks.as_ptr().add(80).cast()),
                _mm_loadu_si128(blocks.as_ptr().add(96).cast()),
                _mm_loadu_si128(blocks.as_ptr().add(112).cast()),
            )
        };

        let c1 = _mm_xor_si128(c1, p1);
        let c2 = _mm_xor_si128(c2, p2);
        let c3 = _mm_xor_si128(c3, p3);
        let c4 = _mm_xor_si128(c4, p4);
        let c5 = _mm_xor_si128(c5, p5);
        let c6 = _mm_xor_si128(c6, p6);
        let c7 = _mm_xor_si128(c7, p7);
        let c8 = _mm_xor_si128(c8, p8);

        // SAFETY: `blocks` is 128 bytes and writable, due to `chunks_exact_mut`
        unsafe {
            _mm_storeu_si128(blocks.as_mut_ptr().add(0).cast(), c1);
            _mm_storeu_si128(blocks.as_mut_ptr().add(16).cast(), c2);
            _mm_storeu_si128(blocks.as_mut_ptr().add(32).cast(), c3);
            _mm_storeu_si128(blocks.as_mut_ptr().add(48).cast(), c4);
            _mm_storeu_si128(blocks.as_mut_ptr().add(64).cast(), c5);
            _mm_storeu_si128(blocks.as_mut_ptr().add(80).cast(), c6);
            _mm_storeu_si128(blocks.as_mut_ptr().add(96).cast(), c7);
            _mm_storeu_si128(blocks.as_mut_ptr().add(112).cast(), c8);
        }

        let (a1, a2, a3, a4, a5, a6, a7, a8) = if ENC {
            (c1, c2, c3, c4, c5, c6, c7, c8)
        } else {
            (p1, p2, p3, p4, p5, p6, p7, p8)
        };

        let a1 = _mm_shuffle_epi8(a1, BYTESWAP);
        let a2 = _mm_shuffle_epi8(a2, BYTESWAP);
        let a3 = _mm_shuffle_epi8(a3, BYTESWAP);
        let a4 = _mm_shuffle_epi8(a4, BYTESWAP);
        let a5 = _mm_shuffle_epi8(a5, BYTESWAP);
        let a6 = _mm_shuffle_epi8(a6, BYTESWAP);
        let a7 = _mm_shuffle_epi8(a7, BYTESWAP);
        let a8 = _mm_shuffle_epi8(a8, BYTESWAP);

        let a1 = _mm_xor_si128(ghash.current, a1);
        ghash.current = ghash::_mul8(avx_ghash_table, a1, a2, a3, a4, a5, a6, a7, a8);
    }

    let cipher_inout = by8_iter.into_remainder();

    if !ENC {
        ghash.add(cipher_inout);
    }

    {
        let mut blocks_iter = cipher_inout.chunks_exact_mut(16);
        for block in blocks_iter.by_ref() {
            let c1 = counter.next();

            let mut c1 = _mm_xor_si128(c1, rk_first);

            for rk in rks {
                c1 = _mm_aesenc_si128(c1, *rk);
            }

            let c1 = _mm_aesenclast_si128(c1, rk_last);

            // SAFETY: `block` is 16 bytes due to `chunks_exact_mut`
            let p1 = unsafe { _mm_loadu_si128(block.as_ptr().cast()) };
            let c1 = _mm_xor_si128(c1, p1);

            // SAFETY: `block` is 16 bytes and writable, due to `chunks_exact_mut`
            unsafe { _mm_storeu_si128(block.as_mut_ptr().cast(), c1) };
        }

        let cipher_inout = blocks_iter.into_remainder();
        if !cipher_inout.is_empty() {
            let mut block = [0u8; 16];
            let len = cipher_inout.len();
            debug_assert!(len < 16);
            block[..len].copy_from_slice(cipher_inout);

            let c1 = counter.next();
            let mut c1 = _mm_xor_si128(c1, rk_first);

            for rk in rks {
                c1 = _mm_aesenc_si128(c1, *rk);
            }

            let c1 = _mm_aesenclast_si128(c1, rk_last);

            // SAFETY: `block` is 16 bytes and writable.
            let p1 = unsafe { _mm_loadu_si128(block.as_ptr().cast()) };
            let c1 = _mm_xor_si128(c1, p1);

            // SAFETY: `block` is 16 bytes and writable.
            unsafe {
                _mm_storeu_si128(block.as_mut_ptr().cast(), c1);
            }

            cipher_inout.copy_from_slice(&block[..len]);
        }
    }

    if ENC {
        ghash.add(cipher_inout);
    }
}

#[target_feature(
    enable = "aes,avx,avx2,avx512bw,avx512f,avx512vl,sse3,ssse3,pclmulqdq,vaes,vpclmulqdq"
)]
fn _cipher_avx512<const ENC: bool>(
    key: &AesKey,
    ghash: &mut Ghash<'_>,
    initial_counter: &[u8; 16],
    aad: &[u8],
    cipher_inout: &mut [u8],
    _feature_token: HaveAvx512ForAesGcm,
) {
    ghash.add(aad);

    let round_keys = key.round_keys_512();
    let (rk_first, rks, rk_last) = round_keys.split();

    let mut counter = Counter512::new(initial_counter);
    let mut by16_iter = cipher_inout.chunks_exact_mut(AVX512_MINIMUM_CIPHER_LEN);

    let ghash_avx512 = match ghash.table {
        GhashTable::Avx512(ghash_avx512) => ghash_avx512,
        _ => panic!("unexpected ghash table variant"),
    };

    for blocks in by16_iter.by_ref() {
        // prefetch to avoid any stall later
        super::cpu::prefetch(blocks, 256);

        let c0123 = counter.next4();
        let c4567 = counter.next4();
        let c89ab = counter.next4();
        let ccdef = counter.next4();

        let mut c0123 = _mm512_xor_epi32(c0123, rk_first);
        let mut c4567 = _mm512_xor_epi32(c4567, rk_first);
        let mut c89ab = _mm512_xor_epi32(c89ab, rk_first);
        let mut ccdef = _mm512_xor_epi32(ccdef, rk_first);

        for rk in rks {
            c0123 = _mm512_aesenc_epi128(c0123, *rk);
            c4567 = _mm512_aesenc_epi128(c4567, *rk);
            c89ab = _mm512_aesenc_epi128(c89ab, *rk);
            ccdef = _mm512_aesenc_epi128(ccdef, *rk);
        }

        let c0123 = _mm512_aesenclast_epi128(c0123, rk_last);
        let c4567 = _mm512_aesenclast_epi128(c4567, rk_last);
        let c89ab = _mm512_aesenclast_epi128(c89ab, rk_last);
        let ccdef = _mm512_aesenclast_epi128(ccdef, rk_last);

        // SAFETY: `blocks` is 256 bytes due to `chunks_exact_mut`
        let (p0123, p4567, p89ab, pcdef) = unsafe {
            (
                _mm512_loadu_si512(blocks.as_ptr().add(0).cast()),
                _mm512_loadu_si512(blocks.as_ptr().add(64).cast()),
                _mm512_loadu_si512(blocks.as_ptr().add(128).cast()),
                _mm512_loadu_si512(blocks.as_ptr().add(192).cast()),
            )
        };

        let c0123 = _mm512_xor_epi32(c0123, p0123);
        let c4567 = _mm512_xor_epi32(c4567, p4567);
        let c89ab = _mm512_xor_epi32(c89ab, p89ab);
        let ccdef = _mm512_xor_epi32(ccdef, pcdef);

        // SAFETY: `blocks` is 256 bytes and writable due to `chunks_exact_mut`
        unsafe {
            _mm512_storeu_si512(blocks.as_mut_ptr().add(0).cast(), c0123);
            _mm512_storeu_si512(blocks.as_mut_ptr().add(64).cast(), c4567);
            _mm512_storeu_si512(blocks.as_mut_ptr().add(128).cast(), c89ab);
            _mm512_storeu_si512(blocks.as_mut_ptr().add(192).cast(), ccdef);
        }

        let (a0123, a4567, a89ab, acdef) = if ENC {
            (c0123, c4567, c89ab, ccdef)
        } else {
            (p0123, p4567, p89ab, pcdef)
        };

        let a0123 = _mm512_shuffle_epi8(a0123, BYTESWAP_512);
        let a4567 = _mm512_shuffle_epi8(a4567, BYTESWAP_512);
        let a89ab = _mm512_shuffle_epi8(a89ab, BYTESWAP_512);
        let acdef = _mm512_shuffle_epi8(acdef, BYTESWAP_512);

        let c0___ = _mm512_inserti32x4::<0>(_mm512_setzero_si512(), ghash.current);
        let a0123 = _mm512_xor_epi64(a0123, c0___);

        ghash.current = ghash::_mul16(ghash_avx512, a0123, a4567, a89ab, acdef);
    }

    let cipher_inout = by16_iter.into_remainder();
    let mut counter = counter.into_128();

    if !ENC {
        ghash.add(cipher_inout);
    }

    {
        let mut blocks_iter = cipher_inout.chunks_exact_mut(16);
        let (rk_first, rks, rk_last) = key.round_keys();
        for block in blocks_iter.by_ref() {
            let c1 = counter.next();

            let mut c1 = _mm_xor_si128(c1, rk_first);

            for rk in rks {
                c1 = _mm_aesenc_si128(c1, *rk);
            }

            let c1 = _mm_aesenclast_si128(c1, rk_last);

            // SAFETY: `block` is 16 bytes and writable.
            let p1 = unsafe { _mm_loadu_si128(block.as_ptr().cast()) };
            let c1 = _mm_xor_si128(c1, p1);

            // SAFETY: `block` is 16 bytes and writable.
            unsafe {
                _mm_storeu_si128(block.as_mut_ptr().cast(), c1);
            }
        }

        let cipher_inout = blocks_iter.into_remainder();
        if !cipher_inout.is_empty() {
            let mut block = [0u8; 16];
            let len = cipher_inout.len();
            debug_assert!(len < 16);
            block[..len].copy_from_slice(cipher_inout);

            let c1 = counter.next();
            let mut c1 = _mm_xor_si128(c1, rk_first);

            for rk in rks {
                c1 = _mm_aesenc_si128(c1, *rk);
            }

            let c1 = _mm_aesenclast_si128(c1, rk_last);

            // SAFETY: `block` is 16 bytes and writable.
            let p1 = unsafe { _mm_loadu_si128(block.as_ptr().cast()) };
            let c1 = _mm_xor_si128(c1, p1);

            // SAFETY: `block` is 16 bytes and writable.
            unsafe {
                _mm_storeu_si128(block.as_mut_ptr().cast(), c1);
            }

            cipher_inout.copy_from_slice(&block[..len]);
        }
    }

    if ENC {
        ghash.add(cipher_inout);
    }
}

/// How many message bytes needed to benefit from the AVX512 by-16 impl
///
/// `cipher_avx512` _works_ for shorter lengths, but is not worth it.
const AVX512_MINIMUM_CIPHER_LEN: usize = 16 * 16;

/// This stores the current four counter values, in big endian.
#[derive(Debug)]
struct Counter512(__m512i);

impl Counter512 {
    #[inline]
    #[target_feature(enable = "sse3,ssse3,avx,avx2,avx512f")]
    fn new(bytes: &[u8; 16]) -> Self {
        // SAFETY: `bytes` is a 128-bits and can be loaded from
        Self(unsafe {
            let mut cnt = Counter::new(bytes);
            let a = cnt.next();
            let b = cnt.next();
            let c = cnt.next();
            let d = cnt.next();

            let r: __m512i = mem::transmute(_mm512_undefined());
            let r = _mm512_inserti32x4::<0>(r, _mm_shuffle_epi8(a, BYTESWAP_EPI64));
            let r = _mm512_inserti32x4::<1>(r, _mm_shuffle_epi8(b, BYTESWAP_EPI64));
            let r = _mm512_inserti32x4::<2>(r, _mm_shuffle_epi8(c, BYTESWAP_EPI64));
            _mm512_inserti32x4::<3>(r, _mm_shuffle_epi8(d, BYTESWAP_EPI64))
        })
    }

    #[target_feature(enable = "avx512f")]
    #[must_use]
    #[inline]
    fn into_128(self) -> Counter {
        Counter(_mm512_extracti32x4_epi32::<0>(self.0))
    }

    #[target_feature(enable = "sse3,ssse3,avx,avx2,avx512f,avx512bw")]
    #[must_use]
    #[inline]
    fn next4(&mut self) -> __m512i {
        let r = _mm512_shuffle_epi8(self.0, BYTESWAP_512_EPI64);
        self.0 = _mm512_add_epi64(self.0, COUNTER_512_4);
        r
    }
}

/// This stores the current counter value, in big endian.
#[derive(Clone, Copy, Debug)]
struct Counter(__m128i);

impl Counter {
    #[inline]
    #[target_feature(enable = "sse2,sse3,ssse3")]
    fn new(bytes: &[u8; 16]) -> Self {
        // SAFETY: `bytes` is a 128-bit value and can be loaded from
        let c = unsafe { _mm_lddqu_si128(bytes.as_ptr().cast()) };
        let c = _mm_shuffle_epi8(c, BYTESWAP_EPI64);
        let c = _mm_add_epi32(c, COUNTER_1); // skip first counter (it was already used as y0)

        Self(c)
    }

    #[target_feature(enable = "sse2,ssse3")]
    #[must_use]
    #[inline]
    fn next(&mut self) -> __m128i {
        let r = _mm_shuffle_epi8(self.0, BYTESWAP_EPI64);
        self.0 = _mm_add_epi32(self.0, COUNTER_1);
        r
    }
}

// SAFETY: both u128 and __m128i have the same size and all bits mean the same thing
const COUNTER_1: __m128i = unsafe { mem::transmute(1u128 << 64) };

// SAFETY: [u8; 64] and __m512i have the same size
const COUNTER_512_4: __m512i = unsafe {
    mem::transmute([
        0u8, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, //
        0, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, //
        0, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, //
        0, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, //
    ])
};

// SAFETY: both u128 and __m128i have the same size and all bits mean the same thing
const BYTESWAP: __m128i = unsafe { mem::transmute(0x00010203_04050607_08090a0b_0c0d0e0fu128) };

// SAFETY: both u128 and __m128i have the same size and all bits mean the same thing
const BYTESWAP_EPI64: __m128i =
    unsafe { mem::transmute(0x08090a0b_0c0d0e0f_00010203_04050607u128) };

// SAFETY: [u8; 64] and __m512i have the same size
const BYTESWAP_512: __m512i = unsafe {
    mem::transmute([
        15u8, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0, //
        31, 30, 29, 28, 27, 26, 25, 24, 23, 22, 21, 20, 19, 18, 17, 16, //
        47, 46, 45, 44, 43, 42, 41, 40, 39, 38, 37, 36, 35, 34, 33, 32, //
        63, 62, 61, 60, 59, 58, 57, 56, 55, 54, 53, 52, 51, 50, 49, 48, //
    ])
};

// SAFETY: [u8; 64] and __m512i have the same size
const BYTESWAP_512_EPI64: __m512i = unsafe {
    mem::transmute([
        7, 6, 5, 4, 3, 2, 1, 0, 15u8, 14, 13, 12, 11, 10, 9, 8, //
        23, 22, 21, 20, 19, 18, 17, 16, 31, 30, 29, 28, 27, 26, 25, 24, //
        39, 38, 37, 36, 35, 34, 33, 32, 47, 46, 45, 44, 43, 42, 41, 40, //
        55, 54, 53, 52, 51, 50, 49, 48, 63, 62, 61, 60, 59, 58, 57, 56,
    ])
};

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::undocumented_unsafe_blocks)]

    #[test]
    fn check_counter512() {
        use super::*;

        if HaveAvx512ForAesGcm::check().is_none() {
            println!("no AVX512 support");
            return;
        }

        let mut c = unsafe { Counter::new(&[0u8; 16]) };
        println!("??? {c:x?}");
        println!("1-- {:x?}", unsafe { c.next() });
        println!("2-- {:x?}", unsafe { c.next() });
        println!("3-- {:x?}", unsafe { c.next() });
        println!("4-- {:x?}", unsafe { c.next() });
        println!("??? {c:x?}");

        let mut c4 = unsafe { Counter512::new(&[0u8; 16]) };
        println!("{c4:x?}");
        println!("1-4 {:x?}", unsafe { c4.next4() });
        println!("{c4:x?}");
        let c4c = unsafe { c4.into_128() };
        println!("{c4c:x?}");

        unsafe {
            assert_eq!(
                mem::transmute::<__m128i, u128>(c4c.0),
                mem::transmute::<__m128i, u128>(c.0)
            );
        }
    }
}
