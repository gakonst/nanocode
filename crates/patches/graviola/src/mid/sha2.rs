// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

//! SHA2-family hash functions.
//!
//! This is SHA256, SHA384, and SHA512.
//! These are all described in [FIPS180](https://nvlpubs.nist.gov/nistpubs/FIPS/NIST.FIPS.180-4.pdf).

use crate::low::Blockwise;

/// A context for incremental computation of SHA256.
#[derive(Clone)]
pub struct Sha256Context {
    h: [u32; 8],
    blockwise: Blockwise<{ Self::BLOCK_SZ }>,
    nblocks: usize,
}

impl Sha256Context {
    /// Start a new SHA256 hash computation.
    pub const fn new() -> Self {
        Self {
            h: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            blockwise: Blockwise::new(),
            nblocks: 0,
        }
    }

    /// Add `bytes` to the ongoing hash computation.
    pub fn update(&mut self, bytes: &[u8]) {
        if self.blockwise.used() == 0 && bytes.len().is_multiple_of(Self::BLOCK_SZ) {
            self.update_blocks(bytes);
            return;
        }

        let bytes = self.blockwise.add_leading(bytes);

        if let Some(block) = self.blockwise.take() {
            self.update_blocks(&block);
        }

        let (whole_blocks, remainder) = {
            let whole_len = bytes.len() - (bytes.len() & (Self::BLOCK_SZ - 1));
            (&bytes[..whole_len], &bytes[whole_len..])
        };

        self.update_blocks(whole_blocks);

        self.blockwise.add_trailing(remainder);
    }

    /// Complete the SHA256 computation, returning the hash output.
    pub fn finish(mut self) -> [u8; Self::OUTPUT_SZ] {
        let bytes = self
            .nblocks
            .checked_mul(Self::BLOCK_SZ)
            .and_then(|bytes| bytes.checked_add(self.blockwise.used()))
            .unwrap();

        let bits = bytes
            .checked_mul(8)
            .expect("excess data processed by hash function");

        let last_blocks = self
            .blockwise
            .md_pad_with_length(&(bits as u64).to_be_bytes());
        self.update_blocks(last_blocks.as_ref());

        let mut r = [0u8; Self::OUTPUT_SZ];
        for (out, state) in r.chunks_exact_mut(4).zip(self.h.iter()) {
            out.copy_from_slice(&state.to_be_bytes());
        }
        r
    }

    fn update_blocks(&mut self, blocks: &[u8]) {
        debug_assert!(blocks.len().is_multiple_of(Self::BLOCK_SZ));
        if !blocks.is_empty() {
            crate::low::sha256_compress_blocks(&mut self.h, blocks);
            self.nblocks = self.nblocks.saturating_add(blocks.len() / Self::BLOCK_SZ);
        }
    }

    /// The internal block size of SHA256.
    pub const BLOCK_SZ: usize = 64;

    /// The output size of SHA256.
    pub const OUTPUT_SZ: usize = 32;
}

/// A context for incremental computation of SHA384.
#[derive(Clone)]
pub struct Sha384Context {
    inner: Sha512Context,
}

impl Sha384Context {
    /// Start a new SHA384 hash computation.
    pub const fn new() -> Self {
        Self {
            inner: Sha512Context {
                h: [
                    0xcbbb9d5dc1059ed8,
                    0x629a292a367cd507,
                    0x9159015a3070dd17,
                    0x152fecd8f70e5939,
                    0x67332667ffc00b31,
                    0x8eb44a8768581511,
                    0xdb0c2e0d64f98fa7,
                    0x47b5481dbefa4fa4,
                ],
                blockwise: Blockwise::new(),
                nblocks: 0,
            },
        }
    }

    /// Add `bytes` to the ongoing hash computation.
    pub fn update(&mut self, bytes: &[u8]) {
        self.inner.update(bytes)
    }

    /// Complete the SHA384 computation, returning the hash output.
    pub fn finish(self) -> [u8; Self::OUTPUT_SZ] {
        let inner = self.inner.finish();
        // SAFETY: 48 is less than 64.
        inner[..Self::OUTPUT_SZ].try_into().unwrap()
    }

    /// The output size of SHA384.
    pub const OUTPUT_SZ: usize = 48;
}

/// A context for incremental computation of SHA512.
#[derive(Clone)]
pub struct Sha512Context {
    h: [u64; 8],
    blockwise: Blockwise<{ Self::BLOCK_SZ }>,
    nblocks: usize,
}

impl Sha512Context {
    /// Start a new SHA512 hash computation.
    pub const fn new() -> Self {
        Self {
            h: [
                0x6a09e667f3bcc908,
                0xbb67ae8584caa73b,
                0x3c6ef372fe94f82b,
                0xa54ff53a5f1d36f1,
                0x510e527fade682d1,
                0x9b05688c2b3e6c1f,
                0x1f83d9abfb41bd6b,
                0x5be0cd19137e2179,
            ],
            blockwise: Blockwise::new(),
            nblocks: 0,
        }
    }

    /// Add `bytes` to the ongoing hash computation.
    pub fn update(&mut self, bytes: &[u8]) {
        if self.blockwise.used() == 0 && bytes.len().is_multiple_of(Self::BLOCK_SZ) {
            self.update_blocks(bytes);
            return;
        }

        let bytes = self.blockwise.add_leading(bytes);

        if let Some(block) = self.blockwise.take() {
            self.update_blocks(&block);
        }

        let (whole_blocks, remainder) = {
            let whole_len = bytes.len() - (bytes.len() & (Self::BLOCK_SZ - 1));
            (&bytes[..whole_len], &bytes[whole_len..])
        };

        self.update_blocks(whole_blocks);

        self.blockwise.add_trailing(remainder);
    }

    /// Complete the SHA512 computation, returning the hash output.
    pub fn finish(mut self) -> [u8; Self::OUTPUT_SZ] {
        let bytes = self
            .nblocks
            .checked_mul(Self::BLOCK_SZ)
            .and_then(|bytes| bytes.checked_add(self.blockwise.used()))
            .unwrap();

        let bits = (bytes as u128)
            .checked_mul(8)
            .expect("excess data processed by hash function");

        let last_blocks = self.blockwise.md_pad_with_length(&bits.to_be_bytes());
        self.update_blocks(last_blocks.as_ref());

        let mut r = [0u8; Self::OUTPUT_SZ];
        for (out, state) in r.chunks_exact_mut(8).zip(self.h.iter()) {
            out.copy_from_slice(&state.to_be_bytes());
        }
        r
    }

    fn update_blocks(&mut self, blocks: &[u8]) {
        debug_assert!(blocks.len().is_multiple_of(Self::BLOCK_SZ));
        if !blocks.is_empty() {
            crate::low::sha512_compress_blocks(&mut self.h, blocks);
            self.nblocks = self.nblocks.saturating_add(blocks.len() / Self::BLOCK_SZ);
        }
    }

    /// The internal block size of SHA512.
    pub const BLOCK_SZ: usize = 128;

    /// The output size of SHA512.
    pub const OUTPUT_SZ: usize = 64;
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn hello() {
        let mut ctx = Sha256Context::new();
        ctx.update(b"hello");
        assert_eq!(&ctx.finish(),
                   b"\x2c\xf2\x4d\xba\x5f\xb0\xa3\x0e\x26\xe8\x3b\x2a\xc5\xb9\xe2\x9e\x1b\x16\x1e\x5c\x1f\xa7\x42\x5e\x73\x04\x33\x62\x93\x8b\x98\x24");

        let mut ctx = Sha512Context::new();
        ctx.update(b"hello");
        assert_eq!(&ctx.finish(),
                   b"\x9b\x71\xd2\x24\xbd\x62\xf3\x78\x5d\x96\xd4\x6a\xd3\xea\x3d\x73\x31\x9b\xfb\xc2\x89\x0c\xaa\xda\xe2\xdf\xf7\x25\x19\x67\x3c\xa7\x23\x23\xc3\xd9\x9b\xa5\xc1\x1d\x7c\x7a\xcc\x6e\x14\xb8\xc5\xda\x0c\x46\x63\x47\x5c\x2e\x5c\x3a\xde\xf4\x6f\x73\xbc\xde\xc0\x43");
    }

    #[test]
    fn sha512_long() {
        let mut data = Vec::with_capacity(1024);
        for i in 0..1024 {
            data.push(i as u8);
        }
        let mut ctx = Sha512Context::new();
        ctx.update(&data);
        assert_eq!(
            &ctx.finish(),
            &[
                55, 246, 82, 190, 134, 127, 40, 237, 3, 50, 105, 203, 186, 32, 26, 242, 17, 44, 43,
                63, 211, 52, 168, 159, 210, 247, 87, 147, 141, 222, 232, 21, 120, 124, 198, 29,
                110, 36, 168, 163, 51, 64, 208, 247, 232, 111, 252, 5, 136, 22, 184, 133, 48, 118,
                107, 166, 226, 49, 98, 10, 19, 11, 86, 108
            ]
        );
    }

    #[test]
    fn sha256_all_lengths() {
        // see cifra `vector_length` and associated
        let mut outer = Sha256Context::new();

        for len in 0..1024 {
            let mut inner = Sha256Context::new();

            for _ in 0..len {
                inner.update(&[len as u8]);
            }

            outer.update(&inner.finish());
        }

        assert_eq!(&outer.finish(),
                   b"\x55\x7b\xfd\xd5\xef\xda\xfd\x63\x06\x5e\xb7\x98\x87\xde\x86\xdb\x54\xc3\xfe\xdf\x7b\xcc\xcb\x97\x08\xfa\x87\xf0\x11\x87\x61\xdc");
    }
    #[test]
    fn sha512_all_lengths() {
        let mut outer = Sha512Context::new();

        for len in 0..1024 {
            let mut inner = Sha512Context::new();

            for _ in 0..len {
                inner.update(&[len as u8]);
            }

            outer.update(&inner.finish());
        }

        assert_eq!(&outer.finish(),
                   b"\x61\x20\x81\x2e\xd5\x0c\xc3\x11\x67\x04\x3f\x1f\x06\x9d\xcd\x4a\xd8\x83\x23\xd9\x96\x53\xd9\x67\x38\x2c\xc3\x44\x25\x69\x53\x1c\xd0\x3d\xe4\x79\x0a\x71\xde\x88\x45\x44\x66\x80\xb8\xc5\x90\xb3\x07\xc8\xae\x52\x57\x67\xf9\x28\xf8\xda\x9e\x9e\x80\xc9\x35\x5e");
    }
}
