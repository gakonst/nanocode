// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

//! Basic implementation using vmull_p64.
//!
//! Based on the implementation in low/x86_64/ghash.rs

use core::arch::aarch64::*;
use core::mem;

use crate::low;

pub(crate) struct GhashTable {
    powers: [uint64x2_t; 8],
    powers_xor: [uint64x2_t; 8],
}

impl GhashTable {
    pub(crate) fn new(h: u128) -> Self {
        // SAFETY: this crate requires the `neon` cpu feature
        let h = unsafe { gf128_big_endian(from_u128(h)) };
        let h2 = mul(h, h);
        let h3 = mul(h2, h);
        let h4 = mul(h3, h);
        let h5 = mul(h4, h);
        let h6 = mul(h5, h);
        let h7 = mul(h6, h);
        let h8 = mul(h7, h);

        let powers = [h, h2, h3, h4, h5, h6, h7, h8];
        // SAFETY: this crate requires the `neon` cpu feature
        let powers_xor = unsafe {
            [
                xor_halves(h),
                xor_halves(h2),
                xor_halves(h3),
                xor_halves(h4),
                xor_halves(h5),
                xor_halves(h6),
                xor_halves(h7),
                xor_halves(h8),
            ]
        };

        Self { powers, powers_xor }
    }
}

impl Drop for GhashTable {
    fn drop(&mut self) {
        low::zeroise(&mut self.powers);
        low::zeroise(&mut self.powers_xor);
    }
}

pub(crate) struct Ghash<'a> {
    pub(crate) table: &'a GhashTable,
    pub(crate) current: uint64x2_t,
}

impl<'a> Ghash<'a> {
    pub(crate) fn new(table: &'a GhashTable) -> Self {
        Self {
            table,
            current: zero(),
        }
    }

    /// Input `bytes` to the computation.
    ///
    /// `bytes` is zero-padded, if required.
    pub(crate) fn add(&mut self, bytes: &[u8]) {
        let mut by8_blocks = bytes.chunks_exact(128);

        for chunk8 in by8_blocks.by_ref() {
            self.eight_blocks(
                u128::from_be_bytes(chunk8[0..16].try_into().unwrap()),
                u128::from_be_bytes(chunk8[16..32].try_into().unwrap()),
                u128::from_be_bytes(chunk8[32..48].try_into().unwrap()),
                u128::from_be_bytes(chunk8[48..64].try_into().unwrap()),
                u128::from_be_bytes(chunk8[64..80].try_into().unwrap()),
                u128::from_be_bytes(chunk8[80..96].try_into().unwrap()),
                u128::from_be_bytes(chunk8[96..112].try_into().unwrap()),
                u128::from_be_bytes(chunk8[112..128].try_into().unwrap()),
            );
        }

        let mut whole_blocks = by8_blocks.remainder().chunks_exact(16);

        for chunk in whole_blocks.by_ref() {
            let u = u128::from_be_bytes(chunk.try_into().unwrap());
            self.one_block(u);
        }

        let bytes = whole_blocks.remainder();
        if !bytes.is_empty() {
            let mut block = [0u8; 16];
            block[..bytes.len()].copy_from_slice(bytes);

            let u = u128::from_be_bytes(block);
            self.one_block(u);
        }
    }

    pub(crate) fn into_bytes(self) -> [u8; 16] {
        to_u128(self.current).to_be_bytes()
    }

    fn one_block(&mut self, block: u128) {
        // SAFETY: this crate requires the `neon` cpu feature
        self.current = unsafe { veorq_u64(self.current, from_u128(block)) };
        self.current = mul(self.current, self.table.powers[0]);
    }

    #[inline]
    pub(crate) fn add_block(&mut self, block: uint8x16_t) {
        // SAFETY: This crate requires the `neon` CPU feature.
        self.current = unsafe { veorq_u64(self.current, to_uint64x2_be(block)) };
        self.current = mul(self.current, self.table.powers[0]);
    }

    #[inline]
    pub(crate) fn eight_blocks(
        &mut self,
        b1: u128,
        b2: u128,
        b3: u128,
        b4: u128,
        b5: u128,
        b6: u128,
        b7: u128,
        b8: u128,
    ) {
        // SAFETY: this crate requires the `neon` cpu feature
        let b1 = unsafe { veorq_u64(self.current, from_u128(b1)) };
        self.current = mul8(
            self.table,
            b1,
            from_u128(b2),
            from_u128(b3),
            from_u128(b4),
            from_u128(b5),
            from_u128(b6),
            from_u128(b7),
            from_u128(b8),
        );
    }

    #[inline]
    pub(crate) fn add_eight_blocks(
        &mut self,
        b1: uint8x16_t,
        b2: uint8x16_t,
        b3: uint8x16_t,
        b4: uint8x16_t,
        b5: uint8x16_t,
        b6: uint8x16_t,
        b7: uint8x16_t,
        b8: uint8x16_t,
    ) {
        // SAFETY: this crate requires the `neon` cpu feature
        unsafe {
            let b1 = veorq_u64(self.current, to_uint64x2_be(b1));
            self.current = mul8(
                self.table,
                b1,
                to_uint64x2_be(b2),
                to_uint64x2_be(b3),
                to_uint64x2_be(b4),
                to_uint64x2_be(b5),
                to_uint64x2_be(b6),
                to_uint64x2_be(b7),
                to_uint64x2_be(b8),
            );
        }
    }
}

#[inline]
fn mul(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    // SAFETY: this crate requires the `neon` and `aes` cpu features
    unsafe { _mul(a, b) }
}

#[inline]
fn mul8(
    table: &GhashTable,
    a: uint64x2_t,
    b: uint64x2_t,
    c: uint64x2_t,
    d: uint64x2_t,
    e: uint64x2_t,
    f: uint64x2_t,
    g: uint64x2_t,
    h: uint64x2_t,
) -> uint64x2_t {
    // SAFETY: this crate requires the `neon` and `aes` cpu features
    unsafe { _mul8(table, a, b, c, d, e, f, g, h) }
}

macro_rules! mul {
    ($lo:ident, $mi:ident, $hi:ident, $x:ident, $h:expr, $hx:expr) => {
        let tlo = vmull_p64_fix($x, $h);
        $lo = veorq_u64(tlo, $lo);

        let xx = vextq_u64($x, $x, 1);
        let xx = veorq_u64(xx, $x);

        let thi = vmull_high_p64_fix($x, $h);
        $hi = veorq_u64(thi, $hi);

        let tmi = vmull_p64_fix(xx, $hx);
        $mi = veorq_u64(tmi, $mi);
    };
}

macro_rules! reduce {
    ($lo:ident, $mi:ident, $hi:ident) => {{
        let $mi = veorq_u64($mi, $lo);
        let $mi = veorq_u64($mi, $hi);

        let ls = vextq_u64($lo, $lo, 1);
        let $lo = vmull_p64_fix(GF128_POLY_HI, $lo);
        let $mi = veorq_u64($mi, ls);
        let $mi = veorq_u64($mi, $lo);

        let ms = vextq_u64($mi, $mi, 1);
        let $mi = vmull_p64_fix(GF128_POLY_HI, $mi);
        let $hi = veorq_u64($hi, ms);
        let $hi = veorq_u64($hi, $mi);
        $hi
    }};
}

#[target_feature(enable = "neon,aes")]
fn _mul(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    let (mut lo, mut mi, mut hi) = (zero(), zero(), zero());
    let bx = xor_halves(b);
    mul!(lo, mi, hi, a, b, bx);
    reduce!(lo, mi, hi)
}

#[target_feature(enable = "neon,aes")]
#[inline]
fn _mul8(
    table: &GhashTable,
    a: uint64x2_t,
    b: uint64x2_t,
    c: uint64x2_t,
    d: uint64x2_t,
    e: uint64x2_t,
    f: uint64x2_t,
    g: uint64x2_t,
    h: uint64x2_t,
) -> uint64x2_t {
    let (mut lo, mut mi, mut hi) = (zero(), zero(), zero());
    mul!(lo, mi, hi, a, table.powers[7], table.powers_xor[7]);
    mul!(lo, mi, hi, b, table.powers[6], table.powers_xor[6]);
    mul!(lo, mi, hi, c, table.powers[5], table.powers_xor[5]);
    mul!(lo, mi, hi, d, table.powers[4], table.powers_xor[4]);
    mul!(lo, mi, hi, e, table.powers[3], table.powers_xor[3]);
    mul!(lo, mi, hi, f, table.powers[2], table.powers_xor[2]);
    mul!(lo, mi, hi, g, table.powers[1], table.powers_xor[1]);
    mul!(lo, mi, hi, h, table.powers[0], table.powers_xor[0]);
    reduce!(lo, mi, hi)
}

#[target_feature(enable = "neon")]
fn xor_halves(h: uint64x2_t) -> uint64x2_t {
    let hx = vextq_u64(h, h, 1);
    veorq_u64(hx, h)
}

#[target_feature(enable = "neon")]
fn gf128_big_endian(h: uint64x2_t) -> uint64x2_t {
    // takes a raw hash subkey, and arranges that it can
    // be used in big endian ordering.
    let t = vreinterpretq_s32_u64(h);
    let (a, c, d) = (
        vgetq_lane_s32::<3>(t),
        vgetq_lane_s32::<1>(t),
        vgetq_lane_s32::<0>(t),
    );
    let t = vsetq_lane_s32(a, t, 3);
    let t = vsetq_lane_s32(c, t, 2);
    let t = vsetq_lane_s32(d, t, 1);
    let t = vsetq_lane_s32(a, t, 0);

    let t = vreinterpretq_u64_s32(vshrq_n_s32(t, 31));
    let h = vaddq_u64(h, h);
    let t = vandq_u64(GF128_POLY_CARRY_MASK, t);
    veorq_u64(h, t)
}

// the intrinsics exist, but have the wrong types :(

#[inline]
#[target_feature(enable = "neon,aes")]
fn vmull_p64_fix(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    let a = vgetq_lane_u64::<0>(a);
    let b = vgetq_lane_u64::<0>(b);
    let p = vmull_p64(a, b);
    // SAFETY: uint64x2_t and u128 have the same representation
    unsafe { mem::transmute(p) }
}

#[inline]
#[target_feature(enable = "neon,aes")]
fn vmull_high_p64_fix(a: uint64x2_t, b: uint64x2_t) -> uint64x2_t {
    let a = vgetq_lane_u64::<1>(a);
    let b = vgetq_lane_u64::<1>(b);
    let p = vmull_p64(a, b);
    // SAFETY: uint64x2_t and u128 have the same representation
    unsafe { mem::transmute(p) }
}

#[inline]
fn zero() -> uint64x2_t {
    // SAFETY: u128 and uint64x2_t have the same size and meaning of bits
    unsafe { mem::transmute(0u128) }
}

#[inline]
fn from_u128(u: u128) -> uint64x2_t {
    // SAFETY: u128 and uint64x2_t have the same size and meaning of bits
    unsafe { mem::transmute(u) }
}

#[inline]
fn to_u128(u: uint64x2_t) -> u128 {
    // SAFETY: u128 and uint64x2_t have the same size and meaning of bits
    unsafe { mem::transmute(u) }
}

#[inline]
#[target_feature(enable = "neon")]
// Make a copy of `u` with the bytes reversed, and cast to `uint64x2_t`.
pub(crate) fn to_uint64x2_be(u: uint8x16_t) -> uint64x2_t {
    // Reverse the order of the bytes in each of the two 64-bit lanes in `u`.
    let u = vrev64q_u8(u);
    let u = vreinterpretq_u64_u8(u);

    // Swap the locations of the two 64-bit lanes to finish reversing the bytes.
    vextq_u64(u, u, 1)
}

// SAFETY: u128 and uint64x2_t have the same size and meaning of bits
const GF128_POLY_HI: uint64x2_t =
    unsafe { mem::transmute(0xc2000000_00000000_c2000000_00000000u128) };

// SAFETY: u128 and uint64x2_t have the same size and meaning of bits
const GF128_POLY_CARRY_MASK: uint64x2_t =
    unsafe { mem::transmute(0xc2000000_00000001_00000000_00000001u128) };

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::low::generic::ghash as model;

    #[test]
    fn pairwise() {
        check(0, b"");
        check(0, b"hello");
        check(1, b"");
        check(1, b"hello");
        let k = 0x00112233_44556677_8899aabb_ccddeeffu128;
        check(k, b"hello");
        check(k, b"hello world!");
        check(k, &[b'a'; 32]);
        check(k, &[b'b'; 64]);
        check(k, &[b'c'; 512 + 64 + 32 + 16]);

        let mut pattern = [0; 512 + 64 + 32 + 16];
        for (i, p) in pattern.iter_mut().enumerate() {
            *p = i as u8;
        }
        check(k, &pattern);

        let k = 0xffeeeedd_ffeeeedd_ff0000dd_ff0000ddu128;
        for (i, p) in pattern.iter_mut().enumerate() {
            *p = 0xf0 | (i & 0xf) as u8;
        }
        check(k, &pattern);
    }

    fn check(key: u128, input: &[u8]) {
        println!("check: input={input:02x?}");
        let ta = GhashTable::new(key);
        let tb = model::GhashTable::new(key);
        let mut a = Ghash::new(&ta);
        let mut b = model::Ghash::new(&tb);
        a.add(input);
        b.add(input);

        let fa = a.into_bytes();
        let fb = b.into_bytes();

        if fa != fb {
            panic!(
                "for input: {:02x?}:\n\n impl  {:02x?}\n    !=\nmodel  {:02x?}",
                input, fa, fb
            );
        }
    }
}
