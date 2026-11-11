//! The digests of the checksum utilities, written for psh from their specifications:
//! MD5 (RFC 1321), SHA-1 and the SHA-2 family (FIPS 180-4: SHA-224, SHA-256, SHA-384,
//! SHA-512, SHA-512/224, SHA-512/256). Streaming: [`Digest::update`] any number of
//! times, then [`Digest::finish`]. The tests check the specifications' vectors.

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
    /// The bytes not yet compressed (less than one block).
    buf: Vec<u8>,
    /// The message length so far, in bytes.
    total: u128,
}

enum State {
    Md5([u32; 4]),
    Sha1([u32; 5]),
    Sha256([u32; 8]),
    Sha512([u64; 8]),
}

impl Digest {
    pub fn new(algo: Algo) -> Digest {
        let state = match algo {
            Algo::Md5 => State::Md5([0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476]),
            Algo::Sha1 => State::Sha1([0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476, 0xc3d2e1f0]),
            Algo::Sha224 => State::Sha256([
                0xc1059ed8, 0x367cd507, 0x3070dd17, 0xf70e5939, 0xffc00b31, 0x68581511, 0x64f98fa7,
                0xbefa4fa4,
            ]),
            Algo::Sha256 => State::Sha256([
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ]),
            Algo::Sha384 => State::Sha512([
                0xcbbb9d5dc1059ed8,
                0x629a292a367cd507,
                0x9159015a3070dd17,
                0x152fecd8f70e5939,
                0x67332667ffc00b31,
                0x8eb44a8768581511,
                0xdb0c2e0d64f98fa7,
                0x47b5481dbefa4fa4,
            ]),
            Algo::Sha512 => State::Sha512(SHA512_IV),
            Algo::Sha512_224 => State::Sha512([
                0x8c3d37c819544da2,
                0x73e1996689dcd4d6,
                0x1dfab7ae32ff9c82,
                0x679dd514582f9fcf,
                0x0f6d2b697bd44da8,
                0x77e36f7304c48942,
                0x3f9d85a86a1d36c8,
                0x1112e6ad91d692a1,
            ]),
            Algo::Sha512_256 => State::Sha512([
                0x22312194fc2bf72c,
                0x9f555fa3c84c64c2,
                0x2393b86b6f53b151,
                0x963877195940eabd,
                0x96283ee2a88effe3,
                0xbe5e1e2553863992,
                0x2b0199fc2c85b8aa,
                0x0eb72ddc81c52ca2,
            ]),
        };
        Digest {
            algo,
            state,
            buf: Vec::with_capacity(128),
            total: 0,
        }
    }

    fn block_len(&self) -> usize {
        match self.state {
            State::Sha512(_) => 128,
            _ => 64,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.total += data.len() as u128;
        let bl = self.block_len();
        if !self.buf.is_empty() {
            let take = (bl - self.buf.len()).min(data.len());
            self.buf.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.buf.len() < bl {
                return;
            }
            let block = std::mem::take(&mut self.buf);
            self.compress(&block);
            self.buf = block;
            self.buf.clear();
        }
        while data.len() >= bl {
            self.compress(&data[..bl]);
            data = &data[bl..];
        }
        self.buf.extend_from_slice(data);
    }

    fn compress(&mut self, block: &[u8]) {
        match &mut self.state {
            State::Md5(s) => md5_block(s, block),
            State::Sha1(s) => sha1_block(s, block),
            State::Sha256(s) => sha256_block(s, block),
            State::Sha512(s) => sha512_block(s, block),
        }
    }

    pub fn finish(mut self) -> Vec<u8> {
        let bl = self.block_len();
        // The length field: 64 bits (128 for SHA-384/512), little-endian for MD5.
        let len_bytes = if bl == 128 { 16 } else { 8 };
        let bits = self.total.wrapping_mul(8);
        let mut tail = std::mem::take(&mut self.buf);
        tail.push(0x80);
        while !(tail.len() + len_bytes).is_multiple_of(bl) {
            tail.push(0);
        }
        match self.state {
            State::Md5(_) => tail.extend_from_slice(&(bits as u64).to_le_bytes()),
            State::Sha512(_) => tail.extend_from_slice(&bits.to_be_bytes()),
            _ => tail.extend_from_slice(&(bits as u64).to_be_bytes()),
        }
        for block in tail.chunks(bl) {
            self.compress(block);
        }
        let mut out = Vec::with_capacity(64);
        match &self.state {
            State::Md5(s) => s
                .iter()
                .for_each(|w| out.extend_from_slice(&w.to_le_bytes())),
            State::Sha1(s) => s
                .iter()
                .for_each(|w| out.extend_from_slice(&w.to_be_bytes())),
            State::Sha256(s) => s
                .iter()
                .for_each(|w| out.extend_from_slice(&w.to_be_bytes())),
            State::Sha512(s) => s
                .iter()
                .for_each(|w| out.extend_from_slice(&w.to_be_bytes())),
        }
        out.truncate(self.algo.len());
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

// MD5: RFC 1321 §3.4.

const MD5_S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

/// `floor(abs(sin(i + 1)) * 2^32)`, the RFC's table T.
const MD5_K: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];

fn md5_block(s: &mut [u32; 4], block: &[u8]) {
    let mut x = [0u32; 16];
    for (i, w) in x.iter_mut().enumerate() {
        *w = u32::from_le_bytes(block[i * 4..i * 4 + 4].try_into().unwrap());
    }
    let [mut a, mut b, mut c, mut d] = *s;
    for i in 0..64 {
        let (f, g) = match i / 16 {
            0 => ((b & c) | (!b & d), i),
            1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
            2 => (b ^ c ^ d, (3 * i + 5) % 16),
            _ => (c ^ (b | !d), (7 * i) % 16),
        };
        let t = a
            .wrapping_add(f)
            .wrapping_add(MD5_K[i])
            .wrapping_add(x[g])
            .rotate_left(MD5_S[i]);
        a = d;
        d = c;
        c = b;
        b = b.wrapping_add(t);
    }
    s[0] = s[0].wrapping_add(a);
    s[1] = s[1].wrapping_add(b);
    s[2] = s[2].wrapping_add(c);
    s[3] = s[3].wrapping_add(d);
}

// SHA-1: FIPS 180-4 §6.1.

fn sha1_block(s: &mut [u32; 5], block: &[u8]) {
    // The sixteen words of the schedule that are live, and each run of twenty rounds
    // as a loop of its own (nanocode: git status hashes every file whose stat the
    // index cannot vouch for, and the round's function chosen by a division per round
    // held this to 200 MB/s).
    let mut w = [0u32; 16];
    for (i, word) in w.iter_mut().enumerate() {
        *word = u32::from_be_bytes([
            block[i * 4],
            block[i * 4 + 1],
            block[i * 4 + 2],
            block[i * 4 + 3],
        ]);
    }
    let [mut a, mut b, mut c, mut d, mut e] = *s;
    macro_rules! rounds {
        ($from:expr, $to:expr, $k:expr, $f:expr) => {
            for i in $from..$to {
                let wi = if i < 16 {
                    w[i]
                } else {
                    let next = (w[(i + 13) & 15] ^ w[(i + 8) & 15] ^ w[(i + 2) & 15] ^ w[i & 15])
                        .rotate_left(1);
                    w[i & 15] = next;
                    next
                };
                let f: u32 = $f;
                let t = a
                    .rotate_left(5)
                    .wrapping_add(f)
                    .wrapping_add(e)
                    .wrapping_add($k)
                    .wrapping_add(wi);
                e = d;
                d = c;
                c = b.rotate_left(30);
                b = a;
                a = t;
            }
        };
    }
    rounds!(0usize, 20usize, 0x5a827999u32, (b & c) | (!b & d));
    rounds!(20usize, 40usize, 0x6ed9eba1u32, b ^ c ^ d);
    rounds!(40usize, 60usize, 0x8f1bbcdcu32, (b & c) | (b & d) | (c & d));
    rounds!(60usize, 80usize, 0xca62c1d6u32, b ^ c ^ d);
    for (x, v) in s.iter_mut().zip([a, b, c, d, e]) {
        *x = x.wrapping_add(v);
    }
}

// SHA-256: FIPS 180-4 §6.2 (SHA-224 differs in its IV and truncation).

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

fn sha256_block(s: &mut [u32; 8], block: &[u8]) {
    let mut w = [0u32; 64];
    for i in 0..16 {
        w[i] = u32::from_be_bytes(block[i * 4..i * 4 + 4].try_into().unwrap());
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *s;
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ (!e & g);
        let t1 = h
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(SHA256_K[i])
            .wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (x, v) in s.iter_mut().zip([a, b, c, d, e, f, g, h]) {
        *x = x.wrapping_add(v);
    }
}

// SHA-512: FIPS 180-4 §6.4 (SHA-384 and SHA-512/t differ in their IVs and truncation).

const SHA512_IV: [u64; 8] = [
    0x6a09e667f3bcc908,
    0xbb67ae8584caa73b,
    0x3c6ef372fe94f82b,
    0xa54ff53a5f1d36f1,
    0x510e527fade682d1,
    0x9b05688c2b3e6c1f,
    0x1f83d9abfb41bd6b,
    0x5be0cd19137e2179,
];

const SHA512_K: [u64; 80] = [
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

fn sha512_block(s: &mut [u64; 8], block: &[u8]) {
    let mut w = [0u64; 80];
    for i in 0..16 {
        w[i] = u64::from_be_bytes(block[i * 8..i * 8 + 8].try_into().unwrap());
    }
    for i in 16..80 {
        let s0 = w[i - 15].rotate_right(1) ^ w[i - 15].rotate_right(8) ^ (w[i - 15] >> 7);
        let s1 = w[i - 2].rotate_right(19) ^ w[i - 2].rotate_right(61) ^ (w[i - 2] >> 6);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *s;
    for i in 0..80 {
        let s1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
        let ch = (e & f) ^ (!e & g);
        let t1 = h
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(SHA512_K[i])
            .wrapping_add(w[i]);
        let s0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (x, v) in s.iter_mut().zip([a, b, c, d, e, f, g, h]) {
        *x = x.wrapping_add(v);
    }
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
