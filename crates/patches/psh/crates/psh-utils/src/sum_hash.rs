//! The digests of the checksum utilities and git: MD5 (RFC 1321), SHA-1 and the SHA-2
//! family (FIPS 180-4: SHA-224, SHA-256, SHA-384, SHA-512, SHA-512/224, SHA-512/256),
//! from the RustCrypto crates, which use the CPU's SHA instructions where it has them.
//! Streaming: [`Digest::update`] any number of times, then [`Digest::finish`]. The tests
//! check the specifications' vectors.

/// A digest algorithm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Algo {
    Md5,
    Sha1,
    Sha224,
    Sha256,
    Sha384,
    Sha512,
    Sha512_224,
    Sha512_256,
}

impl Algo {
    /// The name in BSD-style lines (`SHA256 (file) = ...`), as GNU `--tag` writes it.
    pub fn tag(self) -> &'static str {
        match self {
            Algo::Md5 => "MD5",
            Algo::Sha1 => "SHA1",
            Algo::Sha224 => "SHA224",
            Algo::Sha256 => "SHA256",
            Algo::Sha384 => "SHA384",
            Algo::Sha512 => "SHA512",
            Algo::Sha512_224 => "SHA512/224",
            Algo::Sha512_256 => "SHA512/256",
        }
    }

    /// The digest's length in bytes.
    pub fn len(self) -> usize {
        match self {
            Algo::Md5 => 16,
            Algo::Sha1 => 20,
            Algo::Sha224 | Algo::Sha512_224 => 28,
            Algo::Sha256 | Algo::Sha512_256 => 32,
            Algo::Sha384 => 48,
            Algo::Sha512 => 64,
        }
    }
}

/// A running digest.
pub struct Digest {
    algo: Algo,
    state: State,
}

enum State {
    Md5(md5::Md5),
    Sha1(sha1::Sha1),
    Sha224(sha2::Sha224),
    Sha256(sha2::Sha256),
    Sha384(sha2::Sha384),
    Sha512(sha2::Sha512),
    Sha512_224(sha2::Sha512_224),
    Sha512_256(sha2::Sha512_256),
}

impl Digest {
    pub fn new(algo: Algo) -> Digest {
        use sha2::Digest as _;
        let state = match algo {
            Algo::Md5 => State::Md5(md5::Md5::new()),
            Algo::Sha1 => State::Sha1(sha1::Sha1::new()),
            Algo::Sha224 => State::Sha224(sha2::Sha224::new()),
            Algo::Sha256 => State::Sha256(sha2::Sha256::new()),
            Algo::Sha384 => State::Sha384(sha2::Sha384::new()),
            Algo::Sha512 => State::Sha512(sha2::Sha512::new()),
            Algo::Sha512_224 => State::Sha512_224(sha2::Sha512_224::new()),
            Algo::Sha512_256 => State::Sha512_256(sha2::Sha512_256::new()),
        };
        Digest { algo, state }
    }

    pub fn update(&mut self, data: &[u8]) {
        use sha2::Digest as _;
        match &mut self.state {
            State::Md5(d) => d.update(data),
            State::Sha1(d) => d.update(data),
            State::Sha224(d) => d.update(data),
            State::Sha256(d) => d.update(data),
            State::Sha384(d) => d.update(data),
            State::Sha512(d) => d.update(data),
            State::Sha512_224(d) => d.update(data),
            State::Sha512_256(d) => d.update(data),
        }
    }

    pub fn finish(self) -> Vec<u8> {
        use sha2::Digest as _;
        let out = match self.state {
            State::Md5(d) => d.finalize().to_vec(),
            State::Sha1(d) => d.finalize().to_vec(),
            State::Sha224(d) => d.finalize().to_vec(),
            State::Sha256(d) => d.finalize().to_vec(),
            State::Sha384(d) => d.finalize().to_vec(),
            State::Sha512(d) => d.finalize().to_vec(),
            State::Sha512_224(d) => d.finalize().to_vec(),
            State::Sha512_256(d) => d.finalize().to_vec(),
        };
        debug_assert_eq!(out.len(), self.algo.len());
        out
    }
}

/// The digest of `data`.
pub fn digest(algo: Algo, data: &[u8]) -> Vec<u8> {
    let mut d = Digest::new(algo);
    d.update(data);
    d.finish()
}

/// Lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(DIGITS[usize::from(b >> 4)] as char);
        s.push(DIGITS[usize::from(b & 15)] as char);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(algo: Algo, data: &[u8]) -> String {
        hex(&digest(algo, data))
    }

    /// RFC 1321 Appendix A.5.
    #[test]
    fn md5_vectors() {
        let cases: &[(&str, &str)] = &[
            ("", "d41d8cd98f00b204e9800998ecf8427e"),
            ("a", "0cc175b9c0f1b6a831c399e269772661"),
            ("abc", "900150983cd24fb0d6963f7d28e17f72"),
            ("message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
            (
                "abcdefghijklmnopqrstuvwxyz",
                "c3fcd3d76192e4007dfb496cca67e13b",
            ),
            (
                "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
                "d174ab98d277d9f5a5611c2c9f419d9f",
            ),
            (
                "12345678901234567890123456789012345678901234567890123456789012345678901234567890",
                "57edf4a22be3c955ac49da2e2107b67a",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(h(Algo::Md5, input.as_bytes()), *want, "{input:?}");
        }
    }

    const ABC: &[u8] = b"abc";
    const TWO_BLOCK: &[u8] = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
    const SHA512_TWO_BLOCK: &[u8] = b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu";

    /// FIPS 180-4's examples (NIST CSRC "Examples with intermediate values").
    #[test]
    fn sha_vectors() {
        let cases: &[(Algo, &[u8], &str)] = &[
            (Algo::Sha1, ABC, "a9993e364706816aba3e25717850c26c9cd0d89d"),
            (
                Algo::Sha1,
                TWO_BLOCK,
                "84983e441c3bd26ebaae4aa1f95129e5e54670f1",
            ),
            (Algo::Sha1, b"", "da39a3ee5e6b4b0d3255bfef95601890afd80709"),
            (
                Algo::Sha224,
                ABC,
                "23097d223405d8228642a477bda255b32aadbce4bda0b3f7e36c9da7",
            ),
            (
                Algo::Sha224,
                TWO_BLOCK,
                "75388b16512776cc5dba5da1fd890150b0c6455cb4f58b1952522525",
            ),
            (
                Algo::Sha256,
                ABC,
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                Algo::Sha256,
                TWO_BLOCK,
                "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
            ),
            (
                Algo::Sha256,
                b"",
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
            (
                Algo::Sha384,
                ABC,
                "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7",
            ),
            (
                Algo::Sha384,
                SHA512_TWO_BLOCK,
                "09330c33f71147e83d192fc782cd1b4753111b173b3b05d22fa08086e3b0f712fcc7c71a557e2db966c3e9fa91746039",
            ),
            (
                Algo::Sha512,
                ABC,
                "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
            ),
            (
                Algo::Sha512,
                SHA512_TWO_BLOCK,
                "8e959b75dae313da8cf4f72814fc143f8f7779c6eb9f7fa17299aeadb6889018501d289e4900f7e4331b99dec4b5433ac7d329eeb6dd26545e96e55b874be909",
            ),
            (
                Algo::Sha512_224,
                ABC,
                "4634270f707b6a54daae7530460842e20e37ed265ceee9a43e8924aa",
            ),
            (
                Algo::Sha512_256,
                ABC,
                "53048e2681941ef99b2e29b76b4c7dabe4c2d0c634fc6d46e0e2f13107e7af23",
            ),
        ];
        for (algo, input, want) in cases {
            assert_eq!(h(*algo, input), *want, "{algo:?} {input:?}");
        }
    }

    /// One million `a`s (FIPS 180-4 / RFC 3174's third test), fed in uneven pieces so
    /// that the block buffering is exercised.
    #[test]
    fn long_inputs() {
        let data = vec![b'a'; 1_000_000];
        let piecewise = |algo| {
            let mut d = Digest::new(algo);
            for chunk in data.chunks(997) {
                d.update(chunk);
            }
            hex(&d.finish())
        };
        assert_eq!(
            piecewise(Algo::Sha1),
            "34aa973cd4c4daa4f61eeb2bdbad27316534016f"
        );
        assert_eq!(
            piecewise(Algo::Sha256),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
        assert_eq!(
            piecewise(Algo::Sha512),
            "e718483d0ce769644e2e42c7bc15b4638e1f98b13b2044285632a803afa973ebde0ff244877ea60a4cb0432ce577c31beb009c5c2c49aa2e4eadb217ad8cc09b"
        );
        assert_eq!(piecewise(Algo::Md5), "7707d6ae4e027c70eea2a935c2296f21");
        // Lengths around the padding boundary.
        for n in 50..140 {
            let d: Vec<u8> = (0..n).map(|i| i as u8).collect();
            for algo in [Algo::Md5, Algo::Sha1, Algo::Sha256, Algo::Sha512] {
                let mut s = Digest::new(algo);
                let (x, y) = d.split_at(n / 3);
                s.update(x);
                s.update(y);
                assert_eq!(s.finish(), digest(algo, &d));
            }
        }
    }
}
