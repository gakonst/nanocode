// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

// This is a port of the code in:
// "Parallelizing message schedules to accelerate the
// computations of hash functions", Shay Gueron and Vlad Krasnov,
// 2012, <https://eprint.iacr.org/2012/067>

use core::arch::x86_64::*;

#[inline]
fn ch(x: u64, y: u64, z: u64) -> u64 {
    (x & y) ^ (!x & z)
}

#[inline]
fn maj(x: u64, y: u64, z: u64) -> u64 {
    (x & y) ^ (x & z) ^ (y & z)
}

#[inline]
fn bsig0(x: u64) -> u64 {
    // equiv. x.rotate_right(28) ^ x.rotate_right(34) ^ x.rotate_right(39)
    let mut ret;

    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(
            "  rorx {t}, {x}, 28",
            "  rorx {r}, {x}, 34",
            "  xor  {r}, {t}",
            "  rorx {t}, {x}, 39",
            "  xor  {r}, {t}",
            x = in(reg) x,
            r = out(reg) ret,
            t = out(reg) _,
            options(nostack, nomem, pure),
        );
    }
    ret
}

#[inline]
fn bsig1(x: u64) -> u64 {
    // equiv. x.rotate_right(14) ^ x.rotate_right(18) ^ x.rotate_right(41)
    let mut ret;
    // SAFETY: inline assembly. see [crate::low::inline_assembly_safety] for safety info.
    unsafe {
        core::arch::asm!(
            "  rorx {t}, {x}, 14",
            "  rorx {r}, {x}, 18",
            "  xor  {r}, {t}",
            "  rorx {t}, {x}, 41",
            "  xor  {r}, {t}",
            x = in(reg) x,
            r = out(reg) ret,
            t = out(reg) _,
            options(nostack, nomem, pure),
        );
    }
    ret
}

#[inline]
#[target_feature(enable = "avx,avx2")]
fn sigma_0(w: __m256i) -> __m256i {
    _mm256_xor_si256(
        _mm256_xor_si256(
            _mm256_xor_si256(_mm256_srli_epi64(w, 7), _mm256_srli_epi64(w, 8)),
            _mm256_xor_si256(_mm256_srli_epi64(w, 1), _mm256_slli_epi64(w, 56)),
        ),
        _mm256_slli_epi64(w, 63),
    )
}

#[inline]
#[target_feature(enable = "avx,avx2")]
fn sigma_1(w: __m256i) -> __m256i {
    _mm256_xor_si256(
        _mm256_xor_si256(
            _mm256_xor_si256(_mm256_srli_epi64(w, 6), _mm256_srli_epi64(w, 61)),
            _mm256_xor_si256(_mm256_srli_epi64(w, 19), _mm256_slli_epi64(w, 3)),
        ),
        _mm256_slli_epi64(w, 45),
    )
}

macro_rules! k {
    ($i:expr) => {
        _mm256_broadcastq_epi64(_mm_set_epi64x(0, K[$i] as i64))
    };
}

// the message scheduling round
macro_rules! schedule_round {
    ($schedule:ident, $i:expr, $w1:ident, $w2:ident, $w3:ident, $w4:ident) => {
        let k = k!($i);
        let s0 = sigma_0($w1);
        let s1 = sigma_1($w2);
        $schedule[$i] = _mm256_add_epi64($w3, k);

        $w3 = _mm256_add_epi64(_mm256_add_epi64($w3, $w4), _mm256_add_epi64(s0, s1));
        $i += 1;
    };
}

#[target_feature(enable = "avx,avx2,bmi2")]
fn sha512_quad_message_schedule(schedule: &mut [__m256i; 80], message: *const u64) {
    // SAFETY: `message` is 16 x 64-bit words and readable
    unsafe {
        let gather_mask = _mm256_setr_epi64x(0, 16, 32, 48);
        let mut w0 = _mm256_i64gather_epi64(message.add(0).cast(), gather_mask, 8);
        w0 = _mm256_shuffle_epi8(w0, BSWAP_SHUFFLE);
        let mut w1 = _mm256_i64gather_epi64(message.add(1).cast(), gather_mask, 8);
        w1 = _mm256_shuffle_epi8(w1, BSWAP_SHUFFLE);
        let mut w2 = _mm256_i64gather_epi64(message.add(2).cast(), gather_mask, 8);
        w2 = _mm256_shuffle_epi8(w2, BSWAP_SHUFFLE);
        let mut w3 = _mm256_i64gather_epi64(message.add(3).cast(), gather_mask, 8);
        w3 = _mm256_shuffle_epi8(w3, BSWAP_SHUFFLE);
        let mut w4 = _mm256_i64gather_epi64(message.add(4).cast(), gather_mask, 8);
        w4 = _mm256_shuffle_epi8(w4, BSWAP_SHUFFLE);
        let mut w5 = _mm256_i64gather_epi64(message.add(5).cast(), gather_mask, 8);
        w5 = _mm256_shuffle_epi8(w5, BSWAP_SHUFFLE);
        let mut w6 = _mm256_i64gather_epi64(message.add(6).cast(), gather_mask, 8);
        w6 = _mm256_shuffle_epi8(w6, BSWAP_SHUFFLE);
        let mut w7 = _mm256_i64gather_epi64(message.add(7).cast(), gather_mask, 8);
        w7 = _mm256_shuffle_epi8(w7, BSWAP_SHUFFLE);
        let mut w8 = _mm256_i64gather_epi64(message.add(8).cast(), gather_mask, 8);
        w8 = _mm256_shuffle_epi8(w8, BSWAP_SHUFFLE);
        let mut w9 = _mm256_i64gather_epi64(message.add(9).cast(), gather_mask, 8);
        w9 = _mm256_shuffle_epi8(w9, BSWAP_SHUFFLE);
        let mut w10 = _mm256_i64gather_epi64(message.add(10).cast(), gather_mask, 8);
        w10 = _mm256_shuffle_epi8(w10, BSWAP_SHUFFLE);
        let mut w11 = _mm256_i64gather_epi64(message.add(11).cast(), gather_mask, 8);
        w11 = _mm256_shuffle_epi8(w11, BSWAP_SHUFFLE);
        let mut w12 = _mm256_i64gather_epi64(message.add(12).cast(), gather_mask, 8);
        w12 = _mm256_shuffle_epi8(w12, BSWAP_SHUFFLE);
        let mut w13 = _mm256_i64gather_epi64(message.add(13).cast(), gather_mask, 8);
        w13 = _mm256_shuffle_epi8(w13, BSWAP_SHUFFLE);
        let mut w14 = _mm256_i64gather_epi64(message.add(14).cast(), gather_mask, 8);
        w14 = _mm256_shuffle_epi8(w14, BSWAP_SHUFFLE);
        let mut w15 = _mm256_i64gather_epi64(message.add(15).cast(), gather_mask, 8);
        w15 = _mm256_shuffle_epi8(w15, BSWAP_SHUFFLE);

        let mut i = 0;
        while i < 64 {
            schedule_round!(schedule, i, w1, w14, w0, w9);
            schedule_round!(schedule, i, w2, w15, w1, w10);
            schedule_round!(schedule, i, w3, w0, w2, w11);
            schedule_round!(schedule, i, w4, w1, w3, w12);
            schedule_round!(schedule, i, w5, w2, w4, w13);
            schedule_round!(schedule, i, w6, w3, w5, w14);
            schedule_round!(schedule, i, w7, w4, w6, w15);
            schedule_round!(schedule, i, w8, w5, w7, w0);
            schedule_round!(schedule, i, w9, w6, w8, w1);
            schedule_round!(schedule, i, w10, w7, w9, w2);
            schedule_round!(schedule, i, w11, w8, w10, w3);
            schedule_round!(schedule, i, w12, w9, w11, w4);
            schedule_round!(schedule, i, w13, w10, w12, w5);
            schedule_round!(schedule, i, w14, w11, w13, w6);
            schedule_round!(schedule, i, w15, w12, w14, w7);
            schedule_round!(schedule, i, w0, w13, w15, w8);
        }
        schedule[64] = _mm256_add_epi64(w0, k!(64));
        schedule[65] = _mm256_add_epi64(w1, k!(65));
        schedule[66] = _mm256_add_epi64(w2, k!(66));
        schedule[67] = _mm256_add_epi64(w3, k!(67));
        schedule[68] = _mm256_add_epi64(w4, k!(68));
        schedule[69] = _mm256_add_epi64(w5, k!(69));
        schedule[70] = _mm256_add_epi64(w6, k!(70));
        schedule[71] = _mm256_add_epi64(w7, k!(71));
        schedule[72] = _mm256_add_epi64(w8, k!(72));
        schedule[73] = _mm256_add_epi64(w9, k!(73));
        schedule[74] = _mm256_add_epi64(w10, k!(74));
        schedule[75] = _mm256_add_epi64(w11, k!(75));
        schedule[76] = _mm256_add_epi64(w12, k!(76));
        schedule[77] = _mm256_add_epi64(w13, k!(77));
        schedule[78] = _mm256_add_epi64(w14, k!(78));
        schedule[79] = _mm256_add_epi64(w15, k!(79));
    }
}

macro_rules! round {
    ($a:ident, $b:ident, $c:ident, $d:ident, $e:ident, $f:ident, $g:ident, $h:ident, $w_t:expr) => {
        let t1 = $h
            .wrapping_add(bsig1($e))
            .wrapping_add(ch($e, $f, $g))
            .wrapping_add($w_t as u64);
        let t2 = bsig0($a).wrapping_add(maj($a, $b, $c));
        $d = $d.wrapping_add(t1);
        $h = t1.wrapping_add(t2);
    };
}

#[target_feature(enable = "avx,avx2,bmi2")]
fn sha512_compress_4_blocks(state: &mut [u64; 8], block4: *const u64) {
    let mut w = [_mm256_setzero_si256(); 80];
    sha512_quad_message_schedule(&mut w, block4);

    // keep intermediate state in ymm registers to reduce scalar register
    // pressure
    // SAFETY: `state` is 8 x 64-bit words and readable
    let (save_abcd, save_efgh) = unsafe {
        (
            _mm256_loadu_si256(state.as_ptr().add(0).cast()),
            _mm256_loadu_si256(state.as_ptr().add(4).cast()),
        )
    };

    // block 1
    let mut a = _mm256_extract_epi64(save_abcd, 0) as u64;
    let mut b = _mm256_extract_epi64(save_abcd, 1) as u64;
    let mut c = _mm256_extract_epi64(save_abcd, 2) as u64;
    let mut d = _mm256_extract_epi64(save_abcd, 3) as u64;
    let mut e = _mm256_extract_epi64(save_efgh, 0) as u64;
    let mut f = _mm256_extract_epi64(save_efgh, 1) as u64;
    let mut g = _mm256_extract_epi64(save_efgh, 2) as u64;
    let mut h = _mm256_extract_epi64(save_efgh, 3) as u64;
    for w_t in w.chunks_exact(8) {
        round!(a, b, c, d, e, f, g, h, _mm256_extract_epi64(w_t[0], 0));
        round!(h, a, b, c, d, e, f, g, _mm256_extract_epi64(w_t[1], 0));
        round!(g, h, a, b, c, d, e, f, _mm256_extract_epi64(w_t[2], 0));
        round!(f, g, h, a, b, c, d, e, _mm256_extract_epi64(w_t[3], 0));
        round!(e, f, g, h, a, b, c, d, _mm256_extract_epi64(w_t[4], 0));
        round!(d, e, f, g, h, a, b, c, _mm256_extract_epi64(w_t[5], 0));
        round!(c, d, e, f, g, h, a, b, _mm256_extract_epi64(w_t[6], 0));
        round!(b, c, d, e, f, g, h, a, _mm256_extract_epi64(w_t[7], 0));
    }

    let save_abcd = _mm256_add_epi64(
        save_abcd,
        _mm256_set_epi64x(d as i64, c as i64, b as i64, a as i64),
    );
    let save_efgh = _mm256_add_epi64(
        save_efgh,
        _mm256_set_epi64x(h as i64, g as i64, f as i64, e as i64),
    );

    // block 2
    let mut a = _mm256_extract_epi64(save_abcd, 0) as u64;
    let mut b = _mm256_extract_epi64(save_abcd, 1) as u64;
    let mut c = _mm256_extract_epi64(save_abcd, 2) as u64;
    let mut d = _mm256_extract_epi64(save_abcd, 3) as u64;
    let mut e = _mm256_extract_epi64(save_efgh, 0) as u64;
    let mut f = _mm256_extract_epi64(save_efgh, 1) as u64;
    let mut g = _mm256_extract_epi64(save_efgh, 2) as u64;
    let mut h = _mm256_extract_epi64(save_efgh, 3) as u64;

    for w_t in w.chunks_exact(8) {
        round!(a, b, c, d, e, f, g, h, _mm256_extract_epi64(w_t[0], 1));
        round!(h, a, b, c, d, e, f, g, _mm256_extract_epi64(w_t[1], 1));
        round!(g, h, a, b, c, d, e, f, _mm256_extract_epi64(w_t[2], 1));
        round!(f, g, h, a, b, c, d, e, _mm256_extract_epi64(w_t[3], 1));
        round!(e, f, g, h, a, b, c, d, _mm256_extract_epi64(w_t[4], 1));
        round!(d, e, f, g, h, a, b, c, _mm256_extract_epi64(w_t[5], 1));
        round!(c, d, e, f, g, h, a, b, _mm256_extract_epi64(w_t[6], 1));
        round!(b, c, d, e, f, g, h, a, _mm256_extract_epi64(w_t[7], 1));
    }

    let save_abcd = _mm256_add_epi64(
        save_abcd,
        _mm256_set_epi64x(d as i64, c as i64, b as i64, a as i64),
    );
    let save_efgh = _mm256_add_epi64(
        save_efgh,
        _mm256_set_epi64x(h as i64, g as i64, f as i64, e as i64),
    );

    // block 3
    let mut a = _mm256_extract_epi64(save_abcd, 0) as u64;
    let mut b = _mm256_extract_epi64(save_abcd, 1) as u64;
    let mut c = _mm256_extract_epi64(save_abcd, 2) as u64;
    let mut d = _mm256_extract_epi64(save_abcd, 3) as u64;
    let mut e = _mm256_extract_epi64(save_efgh, 0) as u64;
    let mut f = _mm256_extract_epi64(save_efgh, 1) as u64;
    let mut g = _mm256_extract_epi64(save_efgh, 2) as u64;
    let mut h = _mm256_extract_epi64(save_efgh, 3) as u64;

    for w_t in w.chunks_exact(8) {
        round!(a, b, c, d, e, f, g, h, _mm256_extract_epi64(w_t[0], 2));
        round!(h, a, b, c, d, e, f, g, _mm256_extract_epi64(w_t[1], 2));
        round!(g, h, a, b, c, d, e, f, _mm256_extract_epi64(w_t[2], 2));
        round!(f, g, h, a, b, c, d, e, _mm256_extract_epi64(w_t[3], 2));
        round!(e, f, g, h, a, b, c, d, _mm256_extract_epi64(w_t[4], 2));
        round!(d, e, f, g, h, a, b, c, _mm256_extract_epi64(w_t[5], 2));
        round!(c, d, e, f, g, h, a, b, _mm256_extract_epi64(w_t[6], 2));
        round!(b, c, d, e, f, g, h, a, _mm256_extract_epi64(w_t[7], 2));
    }

    let save_abcd = _mm256_add_epi64(
        save_abcd,
        _mm256_set_epi64x(d as i64, c as i64, b as i64, a as i64),
    );
    let save_efgh = _mm256_add_epi64(
        save_efgh,
        _mm256_set_epi64x(h as i64, g as i64, f as i64, e as i64),
    );

    // block 4
    let mut a = _mm256_extract_epi64(save_abcd, 0) as u64;
    let mut b = _mm256_extract_epi64(save_abcd, 1) as u64;
    let mut c = _mm256_extract_epi64(save_abcd, 2) as u64;
    let mut d = _mm256_extract_epi64(save_abcd, 3) as u64;
    let mut e = _mm256_extract_epi64(save_efgh, 0) as u64;
    let mut f = _mm256_extract_epi64(save_efgh, 1) as u64;
    let mut g = _mm256_extract_epi64(save_efgh, 2) as u64;
    let mut h = _mm256_extract_epi64(save_efgh, 3) as u64;

    for w_t in w.chunks_exact(8) {
        round!(a, b, c, d, e, f, g, h, _mm256_extract_epi64(w_t[0], 3));
        round!(h, a, b, c, d, e, f, g, _mm256_extract_epi64(w_t[1], 3));
        round!(g, h, a, b, c, d, e, f, _mm256_extract_epi64(w_t[2], 3));
        round!(f, g, h, a, b, c, d, e, _mm256_extract_epi64(w_t[3], 3));
        round!(e, f, g, h, a, b, c, d, _mm256_extract_epi64(w_t[4], 3));
        round!(d, e, f, g, h, a, b, c, _mm256_extract_epi64(w_t[5], 3));
        round!(c, d, e, f, g, h, a, b, _mm256_extract_epi64(w_t[6], 3));
        round!(b, c, d, e, f, g, h, a, _mm256_extract_epi64(w_t[7], 3));
    }

    let save_abcd = _mm256_add_epi64(
        save_abcd,
        _mm256_set_epi64x(d as i64, c as i64, b as i64, a as i64),
    );
    let save_efgh = _mm256_add_epi64(
        save_efgh,
        _mm256_set_epi64x(h as i64, g as i64, f as i64, e as i64),
    );

    // SAFETY: `state` is 8 x 64-bit words and writable
    unsafe {
        _mm256_storeu_si256(state.as_ptr().add(0) as *mut _, save_abcd);
        _mm256_storeu_si256(state.as_ptr().add(4) as *mut _, save_efgh);
    }
}

/// Reads 32 bytes of input from $block & byte swaps it
///
/// This is the first 16 terms of W.
///
/// Then adds the matching terms of K (dictated by $i, which should be 0..4).
///
/// Returns both W, and W+K
macro_rules! input {
    ($block:ident, $i:literal) => {{
        // SAFETY: `$block` is 128 bytes, and `$i` is 0..4.
        let w = unsafe { _mm256_loadu_si256($block.as_ptr().add($i * 32).cast()) };
        let w = _mm256_shuffle_epi8(w, BSWAP_SHUFFLE);
        // SAFETY: `$i` is 0..4 and `K` is a constant table.
        let k = unsafe { _mm256_loadu_si256(K.as_ptr().add($i * 4).cast()) };
        let wk = _mm256_add_epi64(w, k);
        (w, wk)
    }};
}

/// Computes the next four terms of W schedule, and W+K.
///
/// $wm16 contains W[-16:-14], $wm12 contains W[-12:-8], etc.
macro_rules! schedule {
    ($wm16:ident, $wm12:ident, $wm8:ident, $wm4:ident, $k:ident) => {{
        // reminder: W[t] = SSIG1(W[t - 2]) + W[t - 7] + SSIG0(W[t - 15]) + W[t - 16];
        //
        // since we're computing this for four consecutive values of t, 0..4, we need:
        //
        // - W[t - 16]: W[-16, -15, -14, -13]
        let wm16_13 = $wm16;

        // - W[t - 15]: W[-15, -14, -13, -12]
        let wm14_11 = _mm256_permute2x128_si256($wm16, $wm12, 0b0010_0001);
        let wm15_12 = _mm256_alignr_epi8(wm14_11, $wm16, 8);

        // - W[t - 7]: W[-7, -6, -5, -4]
        let wm6_3 = _mm256_permute2x128_si256($wm8, $wm4, 0b0010_0001);
        let wm7_4 = _mm256_alignr_epi8(wm6_3, $wm8, 8);

        // The W[t - 2] term is more complex: it is W[-2, -1] followed by the first
        // two terms of this function.  That means we must compute this in two phases.
        let s0_wm15_12 = sigma_0(wm15_12);
        let w01xx = _mm256_permute4x64_epi64($wm4, 0b11_10_11_10);
        let s1_w01xx = sigma_1(w01xx);
        let tmp = _mm256_add_epi64(s0_wm15_12, wm16_13);
        let tmp = _mm256_add_epi64(wm7_4, tmp);
        let w01xx = _mm256_add_epi64(s1_w01xx, tmp);

        // finally, compute w[2, 3] based on the new w[0, 1]
        let w0101 = _mm256_permute4x64_epi64(w01xx, 0b01_00_01_00);
        let s1_w0101 = sigma_1(w0101);
        let wxx23 = _mm256_add_epi64(s1_w0101, tmp);

        let w = _mm256_permute2x128_si256(w01xx, wxx23, 0b0011_0000);
        let wk = _mm256_add_epi64(w, $k);
        (w, wk)
    }};
}

/// AVX2 SHA512 block compression.
///
/// This implementation uses the ideas in:
/// <https://web.archive.org/web/20220121160101/https://www.intel.com/content/dam/www/public/us/en/documents/white-papers/fast-sha512-implementations-ia-processors-paper.pdf>
/// specifically: we use AVX2 operations for message schedule,
/// and scalar operations for round computation, interleaved.
///
/// That means we interleave computing four words of W, followed by
/// four rounds.
#[target_feature(enable = "avx,avx2,bmi2")]
fn sha512_compress_block(state: &mut [u64; 8], block: &[u8]) {
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    let (w0_3, wk0_3) = input!(block, 0);
    round!(a, b, c, d, e, f, g, h, _mm256_extract_epi64(wk0_3, 0));
    round!(h, a, b, c, d, e, f, g, _mm256_extract_epi64(wk0_3, 1));
    round!(g, h, a, b, c, d, e, f, _mm256_extract_epi64(wk0_3, 2));
    round!(f, g, h, a, b, c, d, e, _mm256_extract_epi64(wk0_3, 3));
    let (w4_7, wk4_7) = input!(block, 1);
    round!(e, f, g, h, a, b, c, d, _mm256_extract_epi64(wk4_7, 0));
    round!(d, e, f, g, h, a, b, c, _mm256_extract_epi64(wk4_7, 1));
    round!(c, d, e, f, g, h, a, b, _mm256_extract_epi64(wk4_7, 2));
    round!(b, c, d, e, f, g, h, a, _mm256_extract_epi64(wk4_7, 3));
    let (w8_11, wk8_11) = input!(block, 2);
    round!(a, b, c, d, e, f, g, h, _mm256_extract_epi64(wk8_11, 0));
    round!(h, a, b, c, d, e, f, g, _mm256_extract_epi64(wk8_11, 1));
    round!(g, h, a, b, c, d, e, f, _mm256_extract_epi64(wk8_11, 2));
    round!(f, g, h, a, b, c, d, e, _mm256_extract_epi64(wk8_11, 3));
    let (w12_15, wk12_15) = input!(block, 3);
    round!(e, f, g, h, a, b, c, d, _mm256_extract_epi64(wk12_15, 0));
    round!(d, e, f, g, h, a, b, c, _mm256_extract_epi64(wk12_15, 1));
    round!(c, d, e, f, g, h, a, b, _mm256_extract_epi64(wk12_15, 2));
    round!(b, c, d, e, f, g, h, a, _mm256_extract_epi64(wk12_15, 3));

    // window of 16 W values
    let (mut wm4, mut wm3, mut wm2, mut wm1) = (w0_3, w4_7, w8_11, w12_15);

    for t in (16..80).step_by(8) {
        // SAFETY: `K` is a constant table of 80 words.
        let k0 = unsafe { _mm256_loadu_si256(K.as_ptr().add(t).cast()) };
        let (w_t, wk_t) = schedule!(wm4, wm3, wm2, wm1, k0);
        round!(a, b, c, d, e, f, g, h, _mm256_extract_epi64(wk_t, 0));
        round!(h, a, b, c, d, e, f, g, _mm256_extract_epi64(wk_t, 1));
        round!(g, h, a, b, c, d, e, f, _mm256_extract_epi64(wk_t, 2));
        round!(f, g, h, a, b, c, d, e, _mm256_extract_epi64(wk_t, 3));

        // SAFETY: `K` is a constant table of 80 words.
        let k4 = unsafe { _mm256_loadu_si256(K.as_ptr().add(t + 4).cast()) };
        let (w_t1, wk_t1) = schedule!(wm3, wm2, wm1, w_t, k4);
        round!(e, f, g, h, a, b, c, d, _mm256_extract_epi64(wk_t1, 0));
        round!(d, e, f, g, h, a, b, c, _mm256_extract_epi64(wk_t1, 1));
        round!(c, d, e, f, g, h, a, b, _mm256_extract_epi64(wk_t1, 2));
        round!(b, c, d, e, f, g, h, a, _mm256_extract_epi64(wk_t1, 3));

        wm4 = wm2;
        wm3 = wm1;
        wm2 = w_t;
        wm1 = w_t1;
    }

    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
    state[5] = state[5].wrapping_add(f);
    state[6] = state[6].wrapping_add(g);
    state[7] = state[7].wrapping_add(h);
}

pub(in crate::low) fn sha512_compress_blocks(
    state: &mut [u64; 8],
    blocks: &[u8],
    _token: super::cpu::HaveBmi2,
) {
    let mut iter4 = blocks.chunks_exact(512);
    for block4 in iter4.by_ref() {
        // SAFETY: `_token` proves caller checked cpu features for `bmi2`;
        // `avx` and `avx2` required by crate.
        unsafe { sha512_compress_4_blocks(state, block4.as_ptr().cast()) };
    }
    let blocks = iter4.remainder();

    for block in blocks.chunks_exact(128) {
        // SAFETY: caller checks cpu features for `bmi2`; `avx` and `avx2` required by crate.
        unsafe { sha512_compress_block(state, block) }
    }
}

// SAFETY: transmute from [u8; 32] to __m256i has same meaning of all bytes
const BSWAP_SHUFFLE: __m256i = unsafe {
    core::mem::transmute([
        7u8, 6, 5, 4, 3, 2, 1, 0, 15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0, 15, 14, 13,
        12, 11, 10, 9, 8,
    ])
};

static K: [u64; 80] = [
    0x428a2f98d728ae22,
    0x7137449123ef65cd,
    0xb5c0fbcfec4d3b2f,
    0xe9b5dba58189dbbc,
    0x3956c25bf348b538,
    0x59f111f1b605d019,
    0x923f82a4af194f9b,
    0xab1c5ed5da6d8118,
    0xd807aa98a3030242,
    0x12835b0145706fbe,
    0x243185be4ee4b28c,
    0x550c7dc3d5ffb4e2,
    0x72be5d74f27b896f,
    0x80deb1fe3b1696b1,
    0x9bdc06a725c71235,
    0xc19bf174cf692694,
    0xe49b69c19ef14ad2,
    0xefbe4786384f25e3,
    0x0fc19dc68b8cd5b5,
    0x240ca1cc77ac9c65,
    0x2de92c6f592b0275,
    0x4a7484aa6ea6e483,
    0x5cb0a9dcbd41fbd4,
    0x76f988da831153b5,
    0x983e5152ee66dfab,
    0xa831c66d2db43210,
    0xb00327c898fb213f,
    0xbf597fc7beef0ee4,
    0xc6e00bf33da88fc2,
    0xd5a79147930aa725,
    0x06ca6351e003826f,
    0x142929670a0e6e70,
    0x27b70a8546d22ffc,
    0x2e1b21385c26c926,
    0x4d2c6dfc5ac42aed,
    0x53380d139d95b3df,
    0x650a73548baf63de,
    0x766a0abb3c77b2a8,
    0x81c2c92e47edaee6,
    0x92722c851482353b,
    0xa2bfe8a14cf10364,
    0xa81a664bbc423001,
    0xc24b8b70d0f89791,
    0xc76c51a30654be30,
    0xd192e819d6ef5218,
    0xd69906245565a910,
    0xf40e35855771202a,
    0x106aa07032bbd1b8,
    0x19a4c116b8d2d0c8,
    0x1e376c085141ab53,
    0x2748774cdf8eeb99,
    0x34b0bcb5e19b48a8,
    0x391c0cb3c5c95a63,
    0x4ed8aa4ae3418acb,
    0x5b9cca4f7763e373,
    0x682e6ff3d6b2b8a3,
    0x748f82ee5defb2fc,
    0x78a5636f43172f60,
    0x84c87814a1f0ab72,
    0x8cc702081a6439ec,
    0x90befffa23631e28,
    0xa4506cebde82bde9,
    0xbef9a3f7b2c67915,
    0xc67178f2e372532b,
    0xca273eceea26619c,
    0xd186b8c721c0c207,
    0xeada7dd6cde0eb1e,
    0xf57d4f7fee6ed178,
    0x06f067aa72176fba,
    0x0a637dc5a2c898a6,
    0x113f9804bef90dae,
    0x1b710b35131c471b,
    0x28db77f523047d84,
    0x32caab7b40c72493,
    0x3c9ebe0a15c9bebc,
    0x431d67c49c100d4c,
    0x4cc5d4becb3e42b6,
    0x597f299cfc657e2a,
    0x5fcb6fab3ad6faec,
    0x6c44198c4a475817,
];
