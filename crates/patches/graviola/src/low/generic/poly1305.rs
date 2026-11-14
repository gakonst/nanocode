// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0
// Originally from cifra, but later adopting the 32x32
// multiplication layout from poly1305-donna.

use super::blockwise::Blockwise;

pub(crate) struct Poly1305 {
    /// Current accumulator
    h: [u32; 5],

    /// Block multiplier
    r: [u32; 5],

    /// r[1..5] times 5
    r5: [u32; 4],

    /// Final XOR offset
    s: [u32; 4],

    /// Unprocessed input
    bw: Blockwise<16>,
}

impl Poly1305 {
    pub(crate) fn new(key: &[u8; 32]) -> Self {
        let h = [0; 5];
        let r = [
            read32(&key[0..4]) & 0x3ffffff,
            (read32(&key[3..7]) >> 2) & 0x3ffff03,
            (read32(&key[6..10]) >> 4) & 0x3ffc0ff,
            (read32(&key[9..13]) >> 6) & 0x3f03fff,
            (read32(&key[12..16]) >> 8) & 0x00fffff,
        ];
        let r5 = [r[1] * 5, r[2] * 5, r[3] * 5, r[4] * 5];
        let s = [
            read32(&key[16..20]),
            read32(&key[20..24]),
            read32(&key[24..28]),
            read32(&key[28..32]),
        ];
        Self {
            h,
            r,
            r5,
            s,
            bw: Blockwise::new(),
        }
    }

    pub(crate) fn add_bytes(&mut self, bytes: &[u8]) {
        let bytes = self.bw.add_leading(bytes);

        if let Some(block) = self.bw.take() {
            self.process_whole_block(&block);
        }

        let mut full_blocks = bytes.chunks_exact(16);
        for block in full_blocks.by_ref() {
            self.process_whole_block(block.try_into().unwrap());
        }

        self.bw.add_trailing(full_blocks.remainder());
    }

    pub(crate) fn finish(mut self) -> [u8; 16] {
        if let Some(block) = self.bw.clone().peek_remaining() {
            self.process_last_block(block);
        }

        full_reduce(&mut self.h);

        // redistribute into 4 words
        self.h[0] |= self.h[1] << 26;
        self.h[1] = (self.h[1] >> 6) | (self.h[2] << 20);
        self.h[2] = (self.h[2] >> 12) | (self.h[3] << 14);
        self.h[3] = (self.h[3] >> 18) | (self.h[4] << 8);

        // add s with carry
        fn add32(a: u32, b: u32) -> u64 {
            (a as u64) + (b as u64)
        }
        let f = add32(self.h[0], self.s[0]);
        self.h[0] = f as u32;
        let f = add32(self.h[1], self.s[1]) + (f >> 32);
        self.h[1] = f as u32;
        let f = add32(self.h[2], self.s[2]) + (f >> 32);
        self.h[2] = f as u32;
        let f = add32(self.h[3], self.s[3]) + (f >> 32);
        self.h[3] = f as u32;

        let mut r = [0u8; 16];
        r[0..4].copy_from_slice(&self.h[0].to_le_bytes());
        r[4..8].copy_from_slice(&self.h[1].to_le_bytes());
        r[8..12].copy_from_slice(&self.h[2].to_le_bytes());
        r[12..16].copy_from_slice(&self.h[3].to_le_bytes());

        r
    }

    fn process_whole_block(&mut self, inp: &[u8; 16]) {
        let block = [
            read32(&inp[0..4]) & 0x3ff_ffff,
            (read32(&inp[3..7]) >> 2) & 0x3ff_ffff,
            (read32(&inp[6..10]) >> 4) & 0x3ff_ffff,
            (read32(&inp[9..13]) >> 6) & 0x3ff_ffff,
            (read32(&inp[12..16]) >> 8) | (1 << 24),
        ];
        self.process_block(&block);
    }

    fn process_last_block(&mut self, inp: &[u8]) {
        let mut bytes = [0u8; 16];
        bytes[..inp.len()].copy_from_slice(inp);
        bytes[inp.len()] = 0x01;

        let block = [
            read32(&bytes[0..4]) & 0x3ff_ffff,
            (read32(&bytes[3..7]) >> 2) & 0x3ff_ffff,
            (read32(&bytes[6..10]) >> 4) & 0x3ff_ffff,
            (read32(&bytes[9..13]) >> 6) & 0x3ff_ffff,
            (read32(&bytes[12..16]) >> 8),
        ];
        self.process_block(&block);
    }

    fn process_block(&mut self, block: &[u32; 5]) {
        add(&mut self.h, block);
        mul(&mut self.h, &self.r, &self.r5);
    }
}

fn read32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes.try_into().unwrap())
}

fn add(h: &mut [u32; 5], x: &[u32; 5]) {
    h[0] = h[0].wrapping_add(x[0]);
    h[1] = h[1].wrapping_add(x[1]);
    h[2] = h[2].wrapping_add(x[2]);
    h[3] = h[3].wrapping_add(x[3]);
    h[4] = h[4].wrapping_add(x[4]);
}

fn mul(h: &mut [u32; 5], r: &[u32; 5], s: &[u32; 4]) {
    fn mul32(a: u32, b: u32) -> u64 {
        u64::from(a) * u64::from(b)
    }

    let d0 = mul32(h[0], r[0])
        + mul32(h[1], s[3])
        + mul32(h[2], s[2])
        + mul32(h[3], s[1])
        + mul32(h[4], s[0]);
    let d1 = mul32(h[0], r[1])
        + mul32(h[1], r[0])
        + mul32(h[2], s[3])
        + mul32(h[3], s[2])
        + mul32(h[4], s[1]);
    let d2 = mul32(h[0], r[2])
        + mul32(h[1], r[1])
        + mul32(h[2], r[0])
        + mul32(h[3], s[3])
        + mul32(h[4], s[2]);
    let d3 = mul32(h[0], r[3])
        + mul32(h[1], r[2])
        + mul32(h[2], r[1])
        + mul32(h[3], r[0])
        + mul32(h[4], s[3]);
    let d4 = mul32(h[0], r[4])
        + mul32(h[1], r[3])
        + mul32(h[2], r[2])
        + mul32(h[3], r[1])
        + mul32(h[4], r[0]);

    // partial reduction
    let carry = d0 >> 26;
    h[0] = (d0 & 0x3ff_ffff) as u32;
    let d1 = d1 + carry;
    let carry = d1 >> 26;
    h[1] = (d1 & 0x3ff_ffff) as u32;
    let d2 = d2 + carry;
    let carry = d2 >> 26;
    h[2] = (d2 & 0x3ff_ffff) as u32;
    let d3 = d3 + carry;
    let carry = d3 >> 26;
    h[3] = (d3 & 0x3ff_ffff) as u32;
    let d4 = d4 + carry;
    let carry = (d4 >> 26) as u32;
    h[4] = (d4 & 0x3ff_ffff) as u32;
    h[0] += carry * 5;
    let carry = h[0] >> 26;
    h[0] &= 0x3ff_ffff;
    h[1] += carry;
}

fn full_reduce(h: &mut [u32; 5]) {
    min_reduce(h);
    maybe_sub_130_5(h);
}

fn min_reduce(h: &mut [u32; 5]) {
    let carry = h[1] >> 26;
    h[1] &= 0x3ffffff;
    h[2] = h[2].wrapping_add(carry);

    let carry = h[2] >> 26;
    h[2] &= 0x3ffffff;
    h[3] = h[3].wrapping_add(carry);

    let carry = h[3] >> 26;
    h[3] &= 0x3ffffff;
    h[4] = h[4].wrapping_add(carry);

    let carry = h[4] >> 26;
    h[4] &= 0x3ffffff;
    h[0] = h[0].wrapping_add(carry.wrapping_mul(5));

    let carry = h[0] >> 26;
    h[0] &= 0x3ffffff;
    h[1] = h[1].wrapping_add(carry);
}

fn maybe_sub_130_5(h: &mut [u32; 5]) {
    let g0 = h[0].wrapping_add(5);
    let carry = g0 >> 26;
    let g0 = g0 & 0x3ffffff;

    let g1 = h[1].wrapping_add(carry);
    let carry = g1 >> 26;
    let g1 = g1 & 0x3ffffff;

    let g2 = h[2].wrapping_add(carry);
    let carry = g2 >> 26;
    let g2 = g2 & 0x3ffffff;

    let g3 = h[3].wrapping_add(carry);
    let carry = g3 >> 26;
    let g3 = g3 & 0x3ffffff;

    let g4 = h[4].wrapping_add(carry).wrapping_sub(1 << 26);

    let negative_mask = equal_mask(g4 & 0x80000000, 0x80000000);
    let positive_mask = !negative_mask;

    h[0] = (h[0] & negative_mask) | (g0 & positive_mask);
    h[1] = (h[1] & negative_mask) | (g1 & positive_mask);
    h[2] = (h[2] & negative_mask) | (g2 & positive_mask);
    h[3] = (h[3] & negative_mask) | (g3 & positive_mask);
    h[4] = (h[4] & negative_mask) | (g4 & positive_mask);
}

/// Produce 0xffffffff if x == y, zero
fn equal_mask(x: u32, y: u32) -> u32 {
    let diff = x ^ y;
    let diff_is_zero = !diff & diff.wrapping_sub(1);
    0u32.wrapping_sub(diff_is_zero >> 31)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_vectors() {
        // From draft-agl-tls-chacha20poly1305-04 section 7
        let key = &[
            0x74, 0x68, 0x69, 0x73, 0x20, 0x69, 0x73, 0x20, 0x33, 0x32, 0x2d, 0x62, 0x79, 0x74,
            0x65, 0x20, 0x6b, 0x65, 0x79, 0x20, 0x66, 0x6f, 0x72, 0x20, 0x50, 0x6f, 0x6c, 0x79,
            0x31, 0x33, 0x30, 0x35,
        ];

        let mut p = Poly1305::new(key);
        p.add_bytes(&[0u8; 32]);
        assert_eq!(
            p.finish(),
            [
                0x49, 0xec, 0x78, 0x09, 0x0e, 0x48, 0x1e, 0xc6, 0xc2, 0x6b, 0x33, 0xb9, 0x1c, 0xcc,
                0x03, 0x07
            ]
        );

        let mut p = Poly1305::new(key);
        p.add_bytes(&[
            0x48, 0x65, 0x6c, 0x6c, 0x6f, 0x20, 0x77, 0x6f, 0x72, 0x6c, 0x64, 0x21,
        ]);
        assert_eq!(
            p.finish(),
            [
                0xa6, 0xf7, 0x45, 0x00, 0x8f, 0x81, 0xc9, 0x16, 0xa2, 0x0d, 0xcc, 0x74, 0xee, 0xf2,
                0xb2, 0xf0
            ]
        );
    }
}
