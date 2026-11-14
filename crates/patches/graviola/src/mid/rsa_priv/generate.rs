// Written for Graviola by Joe Birr-Pixton, 2025.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

//! RSA key generation routines.
//!
//! All references below are to FIPS186-5:
//! <https://nvlpubs.nist.gov/nistpubs/FIPS/NIST.FIPS.186-5.pdf>

use crate::Error;
use crate::low::{PosInt, zeroise};
use crate::mid::rng::RandomSource;

/// Supported RSA key sizes for key generation.
///
/// This refers to the public modulus size.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum KeySize {
    /// Generate a key with a 2048-bit public modulus.
    Rsa2048,
    /// Generate a key with a 3072-bit public modulus.
    Rsa3072,
    /// Generate a key with a 4096-bit public modulus.
    Rsa4096,
    /// Generate a key with a 6144-bit public modulus.
    Rsa6144,
    /// Generate a key with a 8192-bit public modulus.
    Rsa8192,
}

impl KeySize {
    fn public_modulus_size_bits(&self) -> usize {
        match self {
            Self::Rsa2048 => 2048,
            Self::Rsa3072 => 3072,
            Self::Rsa4096 => 4096,
            Self::Rsa6144 => 6144,
            Self::Rsa8192 => 8192,
        }
    }

    fn private_prime_size_bits(&self) -> usize {
        self.public_modulus_size_bits() / 2
    }

    fn miller_rabin_rounds(&self) -> usize {
        // refer to Table B.1.
        match self {
            Self::Rsa2048 => 5,
            Self::Rsa3072 => 4,
            Self::Rsa4096 => 4,
            // nb. not included in FIPS186-5.
            Self::Rsa6144 | Self::Rsa8192 => 4,
        }
    }
}

impl TryFrom<usize> for KeySize {
    type Error = Error;

    /// Convert an integer number of bits to a `KeySize`.
    fn try_from(value: usize) -> Result<Self, Self::Error> {
        match value {
            2048 => Ok(Self::Rsa2048),
            3072 => Ok(Self::Rsa3072),
            4096 => Ok(Self::Rsa4096),
            6144 => Ok(Self::Rsa6144),
            8192 => Ok(Self::Rsa8192),
            _ => Err(Error::OutOfRange),
        }
    }
}

pub(super) fn generate_key(
    size: KeySize,
    candidate_random: &mut dyn RandomSource,
    checks_random: &mut dyn RandomSource,
) -> Result<super::RsaPrivateKey, Error> {
    loop {
        let p = random_prime(size, candidate_random, checks_random)?;
        let q = random_prime(size, candidate_random, checks_random)?;

        if p.equals(&q) {
            // for the set of `RsaSizes` supported here, choosing p == q
            // means `random` is hosed.
            return Err(Error::RngFailed);
        }

        // arrange that q < p
        let (p, q) = if q.less_than(&p) { (p, q) } else { (q, p) };

        let n: super::RsaPosIntModN = PosInt::mul(&p, &q);
        let e = PosInt::word(PUBLIC_EXPONENT.into());

        let one = PosInt::one();
        let p_1: super::RsaPosIntModP = p.sub(&one);
        let q_1: super::RsaPosIntModP = q.sub(&one);
        let phi = PosInt::mul(&p_1, &q_1);

        let d = match e.invert_vartime(&phi) {
            Some(d) => d,
            None => {
                // That means e is not coprime with phi; because e is prime, that means
                // phi has a factor of e -- p or q are 1 mod e.
                //
                // We throw away both primes and start again.
                continue;
            }
        };

        let iqmp = q.mod_inverse(&p);
        let dp = d.reduce_even(&p_1);
        let dq = d.reduce_even(&q_1);

        return super::RsaPrivateKey::new(p, q, d, dp, dq, iqmp, n, PUBLIC_EXPONENT);
    }
}

/// We only support F4 for the public exponent.
const PUBLIC_EXPONENT: u32 = 0x10001;

fn random_prime(
    size: KeySize,
    candidate_random: &mut dyn RandomSource,
    checks_random: &mut dyn RandomSource,
) -> Result<super::RsaPosIntModP, Error> {
    loop {
        match random_prime_one(size, candidate_random, checks_random) {
            Ok(Some(candidate)) => return Ok(candidate),
            Ok(None) => continue,
            Err(err) => return Err(err),
        }
    }
}

fn random_prime_one(
    size: KeySize,
    candidate_random: &mut dyn RandomSource,
    checks_random: &mut dyn RandomSource,
) -> Result<Option<super::RsaPosIntModP>, Error> {
    let bytes = size.private_prime_size_bits() / 8;
    let mut buffer = [0u8; super::MAX_PRIVATE_MODULUS_BYTES];
    candidate_random.fill(&mut buffer[..bytes])?;

    // set top two bits to ensure the product of two primes, n, is the right size.
    buffer[0] |= 0b1100_0000;

    // set bottom bit: an even number (of given `size`) is never prime
    buffer[bytes - 1] |= 0b0000_0001;

    let candidate = PosInt::from_bytes(&buffer[..bytes])?;

    if is_prime(&candidate, size, checks_random)? {
        // this is the one we'll use, so it becomes sensitive on return.
        zeroise(&mut buffer[..bytes]);

        Ok(Some(candidate))
    } else {
        Ok(None)
    }
}

fn is_prime(
    candidate: &MaxPosInt,
    size: KeySize,
    rng: &mut dyn RandomSource,
) -> Result<bool, Error> {
    let small_primes = PosInt::from_bytes(PRODUCT_OF_SMALL_PRIMES).unwrap();

    if !candidate.is_coprime(&small_primes) {
        return Ok(false);
    }

    miller_rabin(candidate, size, rng)
}

fn miller_rabin(w: &MaxPosInt, size: KeySize, rng: &mut dyn RandomSource) -> Result<bool, Error> {
    // See FIPS186-5 B.3.1:
    // Steps 1 & 2:
    let mr = MillerRabinParams::new(w);

    // 3. wlen = len(w).
    let w_len = w.len_bits();

    // 4. For i = 1 to iterations do
    let mut iter = 1..=size.miller_rabin_rounds();
    loop {
        // 4.1. Obtain a string b of wlen bits from a DRBG.
        //      Convert b to an integer using the algorithm in B.2.1.
        //
        // We obtain bytes, and then discard the excess high bits.
        let mut buffer = [0u8; super::MAX_PRIVATE_MODULUS_BYTES];
        let w_bytes = w_len.div_ceil(8);
        rng.fill(&mut buffer[..w_bytes])?;
        if w_len & 7 != 0 {
            buffer[0] &= (1u8 << (w_len & 7)).saturating_sub(1);
        }
        let b = PosInt::from_bytes(&buffer[..w_bytes])?;

        // Steps 4.2-4.6 inclusive.
        match mr.check_base(&b) {
            MillerRabinResult::UnsuitableBase => continue,
            MillerRabinResult::PossiblyPrime => {}
            MillerRabinResult::Composite => {
                return Ok(false);
            }
        };

        // 4.7. Continue
        if iter.next().is_none() {
            // 5. Return PROBABLY PRIME.
            return Ok(true);
        }
    }
}

struct MillerRabinParams<'a> {
    w: &'a MaxPosInt,
    w1: MaxPosInt,
    a: usize,
    m: MaxPosInt,
    w_0: u64,
    w_montifier: MaxPosInt,
    mont_w1: MaxPosInt,
    mont_one: MaxPosInt,
}

impl<'a> MillerRabinParams<'a> {
    fn new(w: &'a MaxPosInt) -> Self {
        // 1. Let a be the largest integer such that 2 ^ a divides w - 1.
        let w1 = w.sub(&PosInt::word(1));
        let a = w1.count_trailing_zeroes();

        // 2. m = (w - 1) / 2 ^ a.
        let m = w1.shift_right_vartime(a);

        // Main arithmetic below is done in montgomery form mod w.
        let w_montifier = w.montifier();
        let w_0 = w.mont_neg_inverse();
        let mont_w1 = w1.to_montgomery(&w_montifier, w);
        let mont_one = w.fixed_one().mont_mul(&w_montifier, w, w_0);

        Self {
            w,
            w1,
            a,
            m,
            w_0,
            w_montifier,
            mont_w1,
            mont_one,
        }
    }

    fn check_base(&self, b: &MaxPosInt) -> MillerRabinResult {
        // 4.2. If ((b <= 1) or (b >= w - 1)), then go to step 4.1.
        if b.len_bits() <= 1 || !b.less_than(&self.w1) {
            return MillerRabinResult::UnsuitableBase;
        }

        // 4.3. z = b ^ m mod w.
        let mut z = b.to_montgomery(&self.w_montifier, self.w).mont_exp(
            self.mont_one.clone(),
            &self.m,
            self.w,
            self.w_0,
        );

        // 4.4. If ((z = 1) or (z = w − 1)), then go to step 4.7.
        if z.equals(&self.mont_one) || z.equals(&self.mont_w1) {
            return MillerRabinResult::PossiblyPrime;
        }

        // 4.5. For j = 1 to a − 1 do.
        for _j in 1..self.a {
            // 4.5.1. z = z ^ 2 mod w.
            z = z.mont_sqr(self.w, self.w_0);

            // 4.5.2. If (z = w − 1), then go to step 4.7.
            if z.equals(&self.mont_w1) {
                return MillerRabinResult::PossiblyPrime;
            }

            // 4.5.3 If (z = 1), then go to step 4.6.
            if z.equals(&self.mont_one) {
                return MillerRabinResult::Composite;
            }
        }

        // 4.6. Return COMPOSITE.
        MillerRabinResult::Composite
    }
}

#[derive(Debug)]
enum MillerRabinResult {
    UnsuitableBase,
    PossiblyPrime,
    Composite,
}

/// Product of the primes from 3..743, such that the product fits in 1024-bits
/// -- our smallest allowed prime.
///
/// If a candidate is coprime to this it does not have any small factors.
///
/// It is traditional to do this check instead by trial division, but we'd
/// prefer to use the formally verified `bignum_coprime`.  That comes with
/// the expense of not being able to early-reject candidates that are (for
/// example) a multiple of 3 (which is a significant proportion).
const PRODUCT_OF_SMALL_PRIMES: &[u8] = &[
    0x02, 0xc8, 0x5f, 0xf8, 0x70, 0xf2, 0x4b, 0xe8, 0x0f, 0x62, 0xb1, 0xba, 0x6c, 0x20, 0xbd, 0x72,
    0xb8, 0x37, 0xef, 0xdf, 0x12, 0x12, 0x06, 0xd8, 0x7d, 0xb5, 0x6b, 0x7d, 0x69, 0xfa, 0x4c, 0x02,
    0x1c, 0x10, 0x7c, 0x3c, 0xa2, 0x06, 0xfe, 0x8f, 0xa7, 0x08, 0x0e, 0xf5, 0x76, 0xef, 0xfc, 0x82,
    0xf9, 0xb1, 0x0f, 0x57, 0x50, 0x65, 0x6b, 0x77, 0x94, 0xb1, 0x6a, 0xfd, 0x70, 0x99, 0x6e, 0x91,
    0xae, 0xf6, 0xe0, 0xad, 0x15, 0xe9, 0x1b, 0x07, 0x1a, 0xc9, 0xb2, 0x4d, 0x98, 0xb2, 0x33, 0xad,
    0x86, 0xee, 0x05, 0x55, 0x18, 0xe5, 0x8e, 0x56, 0x63, 0x8e, 0xf1, 0x8b, 0xac, 0x5c, 0x74, 0xcb,
    0x35, 0xbb, 0xb6, 0xe5, 0xda, 0xe2, 0x78, 0x3d, 0xd1, 0xc0, 0xce, 0x7d, 0xec, 0x4f, 0xc7, 0x0e,
    0x51, 0x86, 0xd4, 0x11, 0xdf, 0x36, 0x36, 0x8f, 0x06, 0x1a, 0xa3, 0x60, 0x11, 0xf3, 0x01, 0x79,
];

type MaxPosInt = PosInt<{ super::MAX_PRIVATE_MODULUS_WORDS }>;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use core::time::Duration;
    use std::time::Instant;

    use super::*;
    use crate::high::hash::Sha256;
    use crate::high::hmac_drbg::HmacDrbg;
    use crate::mid::rng::{ChainRandomSource, SliceRandomSource, SystemRandom};

    fn read_hex_from_file(data: &str) -> Vec<u8> {
        data.split_whitespace()
            .flat_map(|line| {
                (0..line.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&line[i..i + 2], 16).unwrap())
            })
            .collect()
    }

    fn read_hex_lines_from_file(data: &str) -> impl Iterator<Item = Vec<u8>> + '_ {
        data.lines()
            .inspect(|line| println!("Processing line: {line}"))
            .filter(|line| !line.starts_with("#"))
            .map(|line| {
                (0..line.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&line[i..i + 2], 16).unwrap())
                    .collect()
            })
    }

    /// `testdata/rsa.bench.2048.txt` is a transcript of candidate primes, with one
    /// candidate per line.  see:
    /// <https://github.com/C2SP/CCTV/tree/main/keygen>
    ///
    /// The results from this should be comparable with the results of:
    ///
    /// ```
    /// $ go test crypto/rsa -bench BenchmarkGenerateKey/2048
    /// goos: linux
    /// goarch: amd64
    /// pkg: crypto/rsa
    /// cpu: AMD Ryzen 9 7940HS w/ Radeon 780M Graphics
    /// BenchmarkGenerateKey/2048-16    10    111795391 ns/op
    ///
    /// $ cargo test --release test_rsa_bench -- --nocapture
    /// running 1 test
    /// Results: min=31038596ns, mean=31920351ns, max=35448808ns
    /// test mid::rsa_priv::generate::tests::test_rsa_bench ... ok
    ///
    /// test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 102 filtered out; finished in 1.02s
    /// ```
    #[test]
    fn test_rsa_bench() {
        let bytes = read_hex_from_file(include_str!("../../testdata/rsa.bench.2048.txt"));
        let mut results = vec![];

        // If running under Valgrind, which is slow, do fewer iterations.
        #[cfg(feature = "__ctgrind")]
        const NUM_ITERATIONS: usize = 1;

        #[cfg(not(feature = "__ctgrind"))]
        const NUM_ITERATIONS: usize = 32;

        for _ in 0..NUM_ITERATIONS {
            let mut candidate_source = SliceRandomSource(&bytes);
            let mut witness_source = SystemRandom;

            let start = Instant::now();
            generate_key(KeySize::Rsa2048, &mut candidate_source, &mut witness_source).unwrap();
            results.push(start.elapsed());
        }
        println!(
            "Results: min={}ns, mean={}ns, max={}ns",
            results.iter().min().unwrap().as_nanos(),
            results.iter().sum::<Duration>().as_nanos() / results.len() as u128,
            results.iter().max().unwrap().as_nanos(),
        )
    }

    #[test]
    fn test_hosed_rng() {
        // candidate rng
        assert_eq!(
            generate_key(
                KeySize::Rsa2048,
                &mut SliceRandomSource(&[]),
                &mut SystemRandom
            )
            .err(),
            Some(Error::RngFailed)
        );
        // MR base rng
        assert_eq!(
            generate_key(
                KeySize::Rsa2048,
                &mut SystemRandom,
                &mut SliceRandomSource(&[]),
            )
            .err(),
            Some(Error::RngFailed)
        );
    }

    #[test]
    fn test_p_q_equal() {
        // get one prime from this test vector and duplicate it.
        let mut bytes = read_hex_from_file(include_str!(
            "../../testdata/rsa.phi-not-coprime-e.2048.txt"
        ));

        assert_eq!(bytes.len(), 256);
        bytes.drain(128..);

        for i in 0..128 {
            bytes.push(bytes[i]);
        }

        let mut candidate_source = SliceRandomSource(&bytes);
        assert_eq!(
            generate_key(KeySize::Rsa2048, &mut candidate_source, &mut SystemRandom).err(),
            Some(Error::RngFailed),
        );
    }

    #[test]
    fn test_key_size_try_from() {
        assert_eq!(KeySize::try_from(2048), Ok(KeySize::Rsa2048));
        assert_eq!(KeySize::try_from(3072), Ok(KeySize::Rsa3072));
        assert_eq!(KeySize::try_from(4096), Ok(KeySize::Rsa4096));
        assert_eq!(KeySize::try_from(6144), Ok(KeySize::Rsa6144));
        assert_eq!(KeySize::try_from(8192), Ok(KeySize::Rsa8192));
        assert_eq!(KeySize::try_from(1024), Err(Error::OutOfRange));
    }

    #[test]
    fn test_phi_not_coprime_e() {
        let bytes = read_hex_from_file(include_str!(
            "../../testdata/rsa.phi-not-coprime-e.2048.txt"
        ));

        let mut fixed_source = SliceRandomSource(&bytes);
        let mut retry_source = SystemRandom;
        let mut candidate_source = ChainRandomSource::First(&mut fixed_source, &mut retry_source);
        let mut witness_source = SystemRandom;

        generate_key(KeySize::Rsa2048, &mut candidate_source, &mut witness_source).unwrap();
    }

    #[test]
    fn test_valid_primes() {
        for line in read_hex_lines_from_file(include_str!("../../testdata/valid-primes.txt")) {
            let candidate = PosInt::from_bytes(&line).unwrap();
            assert_eq!(
                is_prime(
                    &candidate,
                    KeySize::Rsa2048,
                    &mut HmacDrbg::<Sha256>::new(b"", b"", b"")
                ),
                Ok(true)
            );
        }
    }

    #[test]
    fn test_invalid_primes() {
        for line in read_hex_lines_from_file(include_str!("../../testdata/invalid-primes.txt")) {
            let candidate = PosInt::from_bytes(&line).unwrap();
            assert_eq!(
                is_prime(
                    &candidate,
                    KeySize::Rsa2048,
                    &mut HmacDrbg::<Sha256>::new(b"", b"", b"")
                ),
                Ok(false)
            );
        }
    }
}
