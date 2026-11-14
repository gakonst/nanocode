// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

use core::ops::{Deref, DerefMut};

use crate::low::ct_equal;
use crate::mid::sha2;

/// Output from a hash function.
///
/// This has one variant per supported hash function.
#[derive(Clone, Debug)]
pub enum HashOutput {
    /// Output from SHA256
    Sha256([u8; sha2::Sha256Context::OUTPUT_SZ]),
    /// Output from SHA384
    Sha384([u8; sha2::Sha384Context::OUTPUT_SZ]),
    /// Output from SHA512
    Sha512([u8; sha2::Sha512Context::OUTPUT_SZ]),
}

impl HashOutput {
    /// Constant-time equality, `other` may not be truncated.
    pub fn ct_equal(&self, other: &[u8]) -> bool {
        ct_equal(self.as_ref(), other)
    }

    /// Constant-time equality after truncation.
    ///
    /// `self` is truncated to `L` bytes (a compile-time constant)
    /// before comparison with `other`.
    ///
    /// `L` being compile-time prevents the misuse that the
    /// truncation length is attacker-controlled.  `L` must be non-zero,
    /// and less than or equal to the size of the stored hash.
    pub fn truncated_ct_equal<const L: usize>(&self, other: &[u8]) -> bool {
        assert_ne!(L, 0);
        assert!(L <= self.as_ref().len());
        ct_equal(&self.as_ref()[..L], other)
    }
}

impl PartialEq for HashOutput {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Sha256(s), Self::Sha256(o)) => ct_equal(s, o),
            (Self::Sha384(s), Self::Sha384(o)) => ct_equal(s, o),
            (Self::Sha512(s), Self::Sha512(o)) => ct_equal(s, o),
            _ => false,
        }
    }
}

impl AsRef<[u8]> for HashOutput {
    fn as_ref(&self) -> &[u8] {
        match self {
            Self::Sha256(v) => v,
            Self::Sha384(v) => v,
            Self::Sha512(v) => v,
        }
    }
}

impl AsMut<[u8]> for HashOutput {
    fn as_mut(&mut self) -> &mut [u8] {
        match self {
            Self::Sha256(v) => v,
            Self::Sha384(v) => v,
            Self::Sha512(v) => v,
        }
    }
}

/// One block of hash function input.
#[derive(Copy, Clone)]
pub struct HashBlock {
    buf: [u8; 128],
    len: usize,
}

impl HashBlock {
    /// Creates a new `HashBlock`, containing `len` zeroed bytes.
    fn new(len: usize) -> Self {
        Self {
            buf: [0u8; 128],
            len,
        }
    }
}

impl Deref for HashBlock {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        &self.buf[..self.len]
    }
}

impl DerefMut for HashBlock {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.buf[..self.len]
    }
}

/// A generic trait over supported hash functions.
///
/// This exists so (eg.) HMAC may be generic.
pub trait Hash {
    /// The associated context type.
    type Context: HashContext;

    /// Create a new hash context.
    fn new() -> Self::Context;

    /// Hash the given bytes (one-shot style) and return the output.
    fn hash(bytes: &[u8]) -> HashOutput;

    /// Return a zeroed `HashBlock` of the correct size.
    fn zeroed_block() -> HashBlock;

    /// Return a zeroed [`HashOutput`] of the correct size.
    fn zeroed_output() -> HashOutput;
}

/// A generic trait over supported hash function contexts.
///
/// These may be cloned: the semantics of that forks the
/// computation so two different messages with the same
/// prefix can be computed, without reprocessing the prefix.
/// This property is essential to efficiently compute HMAC
/// when used for PBKDF2.
pub trait HashContext: Clone {
    /// Hash the given `bytes`.
    fn update(&mut self, bytes: &[u8]);

    /// Complete the computation.
    fn finish(self) -> HashOutput;
}

/// This is SHA256.
///
/// SHA256 is standardized in [FIPS180](https://nvlpubs.nist.gov/nistpubs/FIPS/NIST.FIPS.180-4.pdf).
#[derive(Clone)]
pub struct Sha256;

impl Hash for Sha256 {
    type Context = Sha256Context;

    fn new() -> Self::Context {
        Sha256Context(sha2::Sha256Context::new())
    }

    fn hash(bytes: &[u8]) -> HashOutput {
        let mut ctx = Self::new();
        ctx.update(bytes);
        ctx.finish()
    }

    fn zeroed_block() -> HashBlock {
        HashBlock::new(sha2::Sha256Context::BLOCK_SZ)
    }

    fn zeroed_output() -> HashOutput {
        HashOutput::Sha256([0u8; sha2::Sha256Context::OUTPUT_SZ])
    }
}

#[derive(Clone)]
pub struct Sha256Context(sha2::Sha256Context);

impl HashContext for Sha256Context {
    fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes)
    }

    fn finish(self) -> HashOutput {
        HashOutput::Sha256(self.0.finish())
    }
}

/// This is SHA384.
///
/// SHA384 is standardized in [FIPS180](https://nvlpubs.nist.gov/nistpubs/FIPS/NIST.FIPS.180-4.pdf).
#[derive(Clone)]
pub struct Sha384;

impl Hash for Sha384 {
    type Context = Sha384Context;

    fn new() -> Self::Context {
        Sha384Context(sha2::Sha384Context::new())
    }

    fn hash(bytes: &[u8]) -> HashOutput {
        let mut ctx = Self::new();
        ctx.update(bytes);
        ctx.finish()
    }

    fn zeroed_block() -> HashBlock {
        HashBlock::new(sha2::Sha512Context::BLOCK_SZ)
    }

    fn zeroed_output() -> HashOutput {
        HashOutput::Sha384([0u8; sha2::Sha384Context::OUTPUT_SZ])
    }
}

#[derive(Clone)]
pub struct Sha384Context(sha2::Sha384Context);

impl HashContext for Sha384Context {
    fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes)
    }

    fn finish(self) -> HashOutput {
        HashOutput::Sha384(self.0.finish())
    }
}

/// This is SHA512.
///
/// SHA512 is standardized in [FIPS180](https://nvlpubs.nist.gov/nistpubs/FIPS/NIST.FIPS.180-4.pdf).
#[derive(Clone)]
pub struct Sha512;

impl Hash for Sha512 {
    type Context = Sha512Context;

    fn new() -> Self::Context {
        Sha512Context(sha2::Sha512Context::new())
    }

    fn hash(bytes: &[u8]) -> HashOutput {
        let mut ctx = Self::new();
        ctx.update(bytes);
        ctx.finish()
    }

    fn zeroed_block() -> HashBlock {
        HashBlock::new(sha2::Sha512Context::BLOCK_SZ)
    }

    fn zeroed_output() -> HashOutput {
        HashOutput::Sha512([0u8; sha2::Sha512Context::OUTPUT_SZ])
    }
}

#[derive(Clone)]
pub struct Sha512Context(sha2::Sha512Context);

impl HashContext for Sha512Context {
    fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes)
    }

    fn finish(self) -> HashOutput {
        HashOutput::Sha512(self.0.finish())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::test::*;

    #[test]
    fn cavp() {
        #[derive(Debug)]
        enum Kind {
            None,
            Sha256,
            Sha384,
            Sha512,
        }

        #[derive(Debug)]
        struct Cavp {
            kind: Kind,
            message: Vec<u8>,
            message_bytes: usize,
        }

        impl Default for Cavp {
            fn default() -> Self {
                Self {
                    kind: Kind::None,
                    message: Vec::new(),
                    message_bytes: 0,
                }
            }
        }

        impl CavpSink for Cavp {
            fn on_meta(&mut self, meta: &str) {
                self.kind = match meta {
                    "L = 32" => Kind::Sha256,
                    "L = 48" => Kind::Sha384,
                    "L = 64" => Kind::Sha512,
                    _ => panic!("unhandled {meta:?}"),
                };
            }

            fn on_value(&mut self, name: &str, value: Value<'_>) {
                match name {
                    "Len" => self.message_bytes = (value.int() / 8) as usize,
                    "Msg" => {
                        self.message = value.bytes();
                        self.message.truncate(self.message_bytes);
                    }
                    "MD" => match self.kind {
                        Kind::None => {}
                        Kind::Sha256 => {
                            let result = Sha256::hash(&self.message);
                            let wanted = value.bytes();
                            assert!(result.ct_equal(&wanted));
                        }
                        Kind::Sha384 => {
                            let result = Sha384::hash(&self.message);
                            let wanted = value.bytes();
                            assert!(result.ct_equal(&wanted));
                        }
                        Kind::Sha512 => {
                            let result = Sha512::hash(&self.message);
                            let wanted = value.bytes();
                            assert!(result.ct_equal(&wanted));
                        }
                    },
                    _ => {
                        todo!("{self:?} value {name} = {value:?}");
                    }
                }
            }
        }

        process_cavp(
            "../thirdparty/cavp/sha2/SHA256ShortMsg.rsp",
            &mut Cavp::default(),
        );
        process_cavp(
            "../thirdparty/cavp/sha2/SHA256LongMsg.rsp",
            &mut Cavp::default(),
        );
        process_cavp(
            "../thirdparty/cavp/sha2/SHA384ShortMsg.rsp",
            &mut Cavp::default(),
        );
        process_cavp(
            "../thirdparty/cavp/sha2/SHA384LongMsg.rsp",
            &mut Cavp::default(),
        );
        process_cavp(
            "../thirdparty/cavp/sha2/SHA512ShortMsg.rsp",
            &mut Cavp::default(),
        );
        process_cavp(
            "../thirdparty/cavp/sha2/SHA512LongMsg.rsp",
            &mut Cavp::default(),
        );
    }

    #[test]
    fn cavp_monte_carlo() {
        #[derive(Debug)]
        enum Kind {
            None,
            Sha256,
            Sha384,
            Sha512,
        }

        #[derive(Debug)]
        struct Cavp {
            kind: Kind,
            seed: Vec<u8>,
        }

        impl Default for Cavp {
            fn default() -> Self {
                Self {
                    kind: Kind::None,
                    seed: Vec::new(),
                }
            }
        }

        impl CavpSink for Cavp {
            fn on_meta(&mut self, meta: &str) {
                self.kind = match meta {
                    "L = 32" => Kind::Sha256,
                    "L = 48" => Kind::Sha384,
                    "L = 64" => Kind::Sha512,
                    _ => panic!("unhandled {meta:?}"),
                };
            }

            fn on_value(&mut self, name: &str, value: Value<'_>) {
                match name {
                    "Seed" => self.seed = value.bytes(),
                    "COUNT" => {}
                    "MD" => match self.kind {
                        Kind::None => {}
                        Kind::Sha256 => {
                            self.seed = monte_carlo_1000::<Sha256>(&self.seed);
                            assert_eq!(self.seed, value.bytes());
                        }
                        Kind::Sha384 => {
                            self.seed = monte_carlo_1000::<Sha384>(&self.seed);
                            assert_eq!(self.seed, value.bytes());
                        }
                        Kind::Sha512 => {
                            self.seed = monte_carlo_1000::<Sha512>(&self.seed);
                            assert_eq!(self.seed, value.bytes());
                        }
                    },
                    _ => {
                        todo!("{self:?} value {name} = {value:?}");
                    }
                }
            }
        }

        fn monte_carlo_1000<H: Hash>(seed: &[u8]) -> Vec<u8> {
            let mut window = [seed.to_vec(), seed.to_vec(), seed.to_vec()];
            for _ in 0..1000 {
                let mut ctx = H::new();
                ctx.update(&window[0]);
                ctx.update(&window[1]);
                ctx.update(&window[2]);
                let r = ctx.finish();
                window.rotate_left(1);
                window[2].copy_from_slice(r.as_ref());
            }
            window[2].clone()
        }

        if std::env::var_os("SLOW_TESTS").is_some() {
            process_cavp(
                "../thirdparty/cavp/sha2/SHA256Monte.rsp",
                &mut Cavp::default(),
            );
            process_cavp(
                "../thirdparty/cavp/sha2/SHA384Monte.rsp",
                &mut Cavp::default(),
            );
            process_cavp(
                "../thirdparty/cavp/sha2/SHA512Monte.rsp",
                &mut Cavp::default(),
            );
        }
    }
}
