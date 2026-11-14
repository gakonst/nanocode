// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

/// An extremely slow, by-the-book implementation.
///
/// Useful as a test model for faster implementations.
pub(crate) struct GhashTable {
    h: u128,
}

impl GhashTable {
    pub(crate) fn new(h: u128) -> Self {
        Self { h }
    }
}

pub(crate) struct Ghash<'a> {
    table: &'a GhashTable,
    current: u128,
}
impl<'a> Ghash<'a> {
    pub(crate) fn new(table: &'a GhashTable) -> Self {
        Self { table, current: 0 }
    }

    /// Input `bytes` to the computation.
    ///
    /// `bytes` is zero-padded, if required.
    pub(crate) fn add(&mut self, bytes: &[u8]) {
        let mut whole_blocks = bytes.chunks_exact(16);

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
        self.current.to_be_bytes()
    }

    fn one_block(&mut self, block: u128) {
        self.current ^= block;
        self.current = mul(self.current, self.table.h);
    }
}

pub(crate) fn mul(x: u128, y: u128) -> u128 {
    let mut z = 0;
    let mut v = x;

    for i in 0..128 {
        let bit = 127 - i;
        let mask = ((y >> bit) & 1).wrapping_neg();
        let sum = z ^ v;
        z = (z & !mask) | (sum & mask);
        v = double(v);
    }

    z
}

fn double(a: u128) -> u128 {
    let mask = (a & 1).wrapping_neg();
    let b = a >> 1;
    b ^ (mask & R)
}

const R: u128 = 0xe1000000_00000000_00000000_00000000;

