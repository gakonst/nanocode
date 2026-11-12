//! The digests of the checksum utilities and git: MD5 (RFC 1321), SHA-1 and the SHA-2
//! family (FIPS 180-4: SHA-224, SHA-256, SHA-384, SHA-512, SHA-512/224, SHA-512/256),
//! written here from the specifications.
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

pub struct Digest {
    algo: Algo,
    state: State,
    block: [u8; BLOCK_BYTES_MAX],
    block_filled: usize,
    length_bytes: u128,
}

const BLOCK_BYTES_MAX: usize = 128;
const SMALL_BLOCK_BYTES: usize = 64;
const SMALL_LENGTH_BYTES: usize = 8;
const LARGE_LENGTH_BYTES: usize = 16;
const PADDING_MARKER: u8 = 0x80;

enum State {
    Md5([u32; 4]),
    Sha1([u32; 5]),
    Sha256([u32; 8]),
    Sha512([u64; 8]),
}

const MD5_INITIAL: [u32; 4] = [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];

const MD5_SHIFTS: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

const MD5_SINES: [u32; 64] = [
    0xd76a_a478,
    0xe8c7_b756,
    0x2420_70db,
    0xc1bd_ceee,
    0xf57c_0faf,
    0x4787_c62a,
    0xa830_4613,
    0xfd46_9501,
    0x6980_98d8,
    0x8b44_f7af,
    0xffff_5bb1,
    0x895c_d7be,
    0x6b90_1122,
    0xfd98_7193,
    0xa679_438e,
    0x49b4_0821,
    0xf61e_2562,
    0xc040_b340,
    0x265e_5a51,
    0xe9b6_c7aa,
    0xd62f_105d,
    0x0244_1453,
    0xd8a1_e681,
    0xe7d3_fbc8,
    0x21e1_cde6,
    0xc337_07d6,
    0xf4d5_0d87,
    0x455a_14ed,
    0xa9e3_e905,
    0xfcef_a3f8,
    0x676f_02d9,
    0x8d2a_4c8a,
    0xfffa_3942,
    0x8771_f681,
    0x6d9d_6122,
    0xfde5_380c,
    0xa4be_ea44,
    0x4bde_cfa9,
    0xf6bb_4b60,
    0xbebf_bc70,
    0x289b_7ec6,
    0xeaa1_27fa,
    0xd4ef_3085,
    0x0488_1d05,
    0xd9d4_d039,
    0xe6db_99e5,
    0x1fa2_7cf8,
    0xc4ac_5665,
    0xf429_2244,
    0x432a_ff97,
    0xab94_23a7,
    0xfc93_a039,
    0x655b_59c3,
    0x8f0c_cc92,
    0xffef_f47d,
    0x8584_5dd1,
    0x6fa8_7e4f,
    0xfe2c_e6e0,
    0xa301_4314,
    0x4e08_11a1,
    0xf753_7e82,
    0xbd3a_f235,
    0x2ad7_d2bb,
    0xeb86_d391,
];

const SHA1_INITIAL: [u32; 5] = [
    0x6745_2301,
    0xefcd_ab89,
    0x98ba_dcfe,
    0x1032_5476,
    0xc3d2_e1f0,
];

const SHA1_ROUND_CONSTANTS: [u32; 4] = [0x5a82_7999, 0x6ed9_eba1, 0x8f1b_bcdc, 0xca62_c1d6];

const SHA224_INITIAL: [u32; 8] = [
    0xc105_9ed8,
    0x367c_d507,
    0x3070_dd17,
    0xf70e_5939,
    0xffc0_0b31,
    0x6858_1511,
    0x64f9_8fa7,
    0xbefa_4fa4,
];

const SHA256_INITIAL: [u32; 8] = [
    0x6a09_e667,
    0xbb67_ae85,
    0x3c6e_f372,
    0xa54f_f53a,
    0x510e_527f,
    0x9b05_688c,
    0x1f83_d9ab,
    0x5be0_cd19,
];

const SHA256_ROUND_CONSTANTS: [u32; 64] = [
    0x428a_2f98,
    0x7137_4491,
    0xb5c0_fbcf,
    0xe9b5_dba5,
    0x3956_c25b,
    0x59f1_11f1,
    0x923f_82a4,
    0xab1c_5ed5,
    0xd807_aa98,
    0x1283_5b01,
    0x2431_85be,
    0x550c_7dc3,
    0x72be_5d74,
    0x80de_b1fe,
    0x9bdc_06a7,
    0xc19b_f174,
    0xe49b_69c1,
    0xefbe_4786,
    0x0fc1_9dc6,
    0x240c_a1cc,
    0x2de9_2c6f,
    0x4a74_84aa,
    0x5cb0_a9dc,
    0x76f9_88da,
    0x983e_5152,
    0xa831_c66d,
    0xb003_27c8,
    0xbf59_7fc7,
    0xc6e0_0bf3,
    0xd5a7_9147,
    0x06ca_6351,
    0x1429_2967,
    0x27b7_0a85,
    0x2e1b_2138,
    0x4d2c_6dfc,
    0x5338_0d13,
    0x650a_7354,
    0x766a_0abb,
    0x81c2_c92e,
    0x9272_2c85,
    0xa2bf_e8a1,
    0xa81a_664b,
    0xc24b_8b70,
    0xc76c_51a3,
    0xd192_e819,
    0xd699_0624,
    0xf40e_3585,
    0x106a_a070,
    0x19a4_c116,
    0x1e37_6c08,
    0x2748_774c,
    0x34b0_bcb5,
    0x391c_0cb3,
    0x4ed8_aa4a,
    0x5b9c_ca4f,
    0x682e_6ff3,
    0x748f_82ee,
    0x78a5_636f,
    0x84c8_7814,
    0x8cc7_0208,
    0x90be_fffa,
    0xa450_6ceb,
    0xbef9_a3f7,
    0xc671_78f2,
];

const SHA384_INITIAL: [u64; 8] = [
    0xcbbb_9d5d_c105_9ed8,
    0x629a_292a_367c_d507,
    0x9159_015a_3070_dd17,
    0x152f_ecd8_f70e_5939,
    0x6733_2667_ffc0_0b31,
    0x8eb4_4a87_6858_1511,
    0xdb0c_2e0d_64f9_8fa7,
    0x47b5_481d_befa_4fa4,
];

const SHA512_INITIAL: [u64; 8] = [
    0x6a09_e667_f3bc_c908,
    0xbb67_ae85_84ca_a73b,
    0x3c6e_f372_fe94_f82b,
    0xa54f_f53a_5f1d_36f1,
    0x510e_527f_ade6_82d1,
    0x9b05_688c_2b3e_6c1f,
    0x1f83_d9ab_fb41_bd6b,
    0x5be0_cd19_137e_2179,
];

const SHA512_224_INITIAL: [u64; 8] = [
    0x8c3d_37c8_1954_4da2,
    0x73e1_9966_89dc_d4d6,
    0x1dfa_b7ae_32ff_9c82,
    0x679d_d514_582f_9fcf,
    0x0f6d_2b69_7bd4_4da8,
    0x77e3_6f73_04c4_8942,
    0x3f9d_85a8_6a1d_36c8,
    0x1112_e6ad_91d6_92a1,
];

const SHA512_256_INITIAL: [u64; 8] = [
    0x2231_2194_fc2b_f72c,
    0x9f55_5fa3_c84c_64c2,
    0x2393_b86b_6f53_b151,
    0x9638_7719_5940_eabd,
    0x9628_3ee2_a88e_ffe3,
    0xbe5e_1e25_5386_3992,
    0x2b01_99fc_2c85_b8aa,
    0x0eb7_2ddc_81c5_2ca2,
];

const SHA512_ROUND_CONSTANTS: [u64; 80] = [
    0x428a_2f98_d728_ae22,
    0x7137_4491_23ef_65cd,
    0xb5c0_fbcf_ec4d_3b2f,
    0xe9b5_dba5_8189_dbbc,
    0x3956_c25b_f348_b538,
    0x59f1_11f1_b605_d019,
    0x923f_82a4_af19_4f9b,
    0xab1c_5ed5_da6d_8118,
    0xd807_aa98_a303_0242,
    0x1283_5b01_4570_6fbe,
    0x2431_85be_4ee4_b28c,
    0x550c_7dc3_d5ff_b4e2,
    0x72be_5d74_f27b_896f,
    0x80de_b1fe_3b16_96b1,
    0x9bdc_06a7_25c7_1235,
    0xc19b_f174_cf69_2694,
    0xe49b_69c1_9ef1_4ad2,
    0xefbe_4786_384f_25e3,
    0x0fc1_9dc6_8b8c_d5b5,
    0x240c_a1cc_77ac_9c65,
    0x2de9_2c6f_592b_0275,
    0x4a74_84aa_6ea6_e483,
    0x5cb0_a9dc_bd41_fbd4,
    0x76f9_88da_8311_53b5,
    0x983e_5152_ee66_dfab,
    0xa831_c66d_2db4_3210,
    0xb003_27c8_98fb_213f,
    0xbf59_7fc7_beef_0ee4,
    0xc6e0_0bf3_3da8_8fc2,
    0xd5a7_9147_930a_a725,
    0x06ca_6351_e003_826f,
    0x1429_2967_0a0e_6e70,
    0x27b7_0a85_46d2_2ffc,
    0x2e1b_2138_5c26_c926,
    0x4d2c_6dfc_5ac4_2aed,
    0x5338_0d13_9d95_b3df,
    0x650a_7354_8baf_63de,
    0x766a_0abb_3c77_b2a8,
    0x81c2_c92e_47ed_aee6,
    0x9272_2c85_1482_353b,
    0xa2bf_e8a1_4cf1_0364,
    0xa81a_664b_bc42_3001,
    0xc24b_8b70_d0f8_9791,
    0xc76c_51a3_0654_be30,
    0xd192_e819_d6ef_5218,
    0xd699_0624_5565_a910,
    0xf40e_3585_5771_202a,
    0x106a_a070_32bb_d1b8,
    0x19a4_c116_b8d2_d0c8,
    0x1e37_6c08_5141_ab53,
    0x2748_774c_df8e_eb99,
    0x34b0_bcb5_e19b_48a8,
    0x391c_0cb3_c5c9_5a63,
    0x4ed8_aa4a_e341_8acb,
    0x5b9c_ca4f_7763_e373,
    0x682e_6ff3_d6b2_b8a3,
    0x748f_82ee_5def_b2fc,
    0x78a5_636f_4317_2f60,
    0x84c8_7814_a1f0_ab72,
    0x8cc7_0208_1a64_39ec,
    0x90be_fffa_2363_1e28,
    0xa450_6ceb_de82_bde9,
    0xbef9_a3f7_b2c6_7915,
    0xc671_78f2_e372_532b,
    0xca27_3ece_ea26_619c,
    0xd186_b8c7_21c0_c207,
    0xeada_7dd6_cde0_eb1e,
    0xf57d_4f7f_ee6e_d178,
    0x06f0_67aa_7217_6fba,
    0x0a63_7dc5_a2c8_98a6,
    0x113f_9804_bef9_0dae,
    0x1b71_0b35_131c_471b,
    0x28db_77f5_2304_7d84,
    0x32ca_ab7b_40c7_2493,
    0x3c9e_be0a_15c9_bebc,
    0x431d_67c4_9c10_0d4c,
    0x4cc5_d4be_cb3e_42b6,
    0x597f_299c_fc65_7e2a,
    0x5fcb_6fab_3ad6_faec,
    0x6c44_198c_4a47_5817,
];

impl Digest {
    pub fn new(algo: Algo) -> Digest {
        let state = match algo {
            Algo::Md5 => State::Md5(MD5_INITIAL),
            Algo::Sha1 => State::Sha1(SHA1_INITIAL),
            Algo::Sha224 => State::Sha256(SHA224_INITIAL),
            Algo::Sha256 => State::Sha256(SHA256_INITIAL),
            Algo::Sha384 => State::Sha512(SHA384_INITIAL),
            Algo::Sha512 => State::Sha512(SHA512_INITIAL),
            Algo::Sha512_224 => State::Sha512(SHA512_224_INITIAL),
            Algo::Sha512_256 => State::Sha512(SHA512_256_INITIAL),
        };
        Digest {
            algo,
            state,
            block: [0; BLOCK_BYTES_MAX],
            block_filled: 0,
            length_bytes: 0,
        }
    }

    fn block_bytes(&self) -> usize {
        match self.state {
            State::Md5(_) | State::Sha1(_) | State::Sha256(_) => SMALL_BLOCK_BYTES,
            State::Sha512(_) => BLOCK_BYTES_MAX,
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.length_bytes += data.len() as u128;
        let block_bytes = self.block_bytes();
        let mut rest = data;
        if self.block_filled > 0 {
            let take = (block_bytes - self.block_filled).min(rest.len());
            self.block[self.block_filled..self.block_filled + take].copy_from_slice(&rest[..take]);
            self.block_filled += take;
            rest = &rest[take..];
            if self.block_filled < block_bytes {
                return;
            }
            let block = self.block;
            self.state.compress(&block[..block_bytes]);
            self.block_filled = 0;
        }
        let mut blocks = rest.chunks_exact(block_bytes);
        for block in &mut blocks {
            self.state.compress(block);
        }
        let tail = blocks.remainder();
        self.block[..tail.len()].copy_from_slice(tail);
        self.block_filled = tail.len();
    }

    pub fn finish(mut self) -> Vec<u8> {
        let block_bytes = self.block_bytes();
        let length_field_bytes = match self.state {
            State::Sha512(_) => LARGE_LENGTH_BYTES,
            State::Md5(_) | State::Sha1(_) | State::Sha256(_) => SMALL_LENGTH_BYTES,
        };
        let length_bits = self.length_bytes.wrapping_mul(8);
        let mut tail = [0u8; 2 * BLOCK_BYTES_MAX];
        let filled = self.block_filled;
        tail[..filled].copy_from_slice(&self.block[..filled]);
        tail[filled] = PADDING_MARKER;
        let total = if filled + 1 + length_field_bytes <= block_bytes {
            block_bytes
        } else {
            2 * block_bytes
        };
        let length_field = &mut tail[total - length_field_bytes..total];
        match self.state {
            State::Md5(_) => length_field.copy_from_slice(&(length_bits as u64).to_le_bytes()),
            State::Sha1(_) | State::Sha256(_) => {
                length_field.copy_from_slice(&(length_bits as u64).to_be_bytes())
            }
            State::Sha512(_) => length_field.copy_from_slice(&length_bits.to_be_bytes()),
        }
        for block in tail[..total].chunks_exact(block_bytes) {
            self.state.compress(block);
        }
        let mut out = Vec::with_capacity(self.algo.len());
        match &self.state {
            State::Md5(words) => words
                .iter()
                .for_each(|w| out.extend_from_slice(&w.to_le_bytes())),
            State::Sha1(words) => words
                .iter()
                .for_each(|w| out.extend_from_slice(&w.to_be_bytes())),
            State::Sha256(words) => words
                .iter()
                .for_each(|w| out.extend_from_slice(&w.to_be_bytes())),
            State::Sha512(words) => words
                .iter()
                .for_each(|w| out.extend_from_slice(&w.to_be_bytes())),
        }
        out.truncate(self.algo.len());
        assert_eq!(out.len(), self.algo.len());
        out
    }
}

impl State {
    fn compress(&mut self, block: &[u8]) {
        match self {
            State::Md5(words) => md5_compress(words, block),
            State::Sha1(words) => sha1_compress(words, block),
            State::Sha256(words) => sha256_compress(words, block),
            State::Sha512(words) => sha512_compress(words, block),
        }
    }
}

fn md5_compress(state: &mut [u32; 4], block: &[u8]) {
    assert_eq!(block.len(), SMALL_BLOCK_BYTES);
    let mut message = [0u32; 16];
    for (word, bytes) in message.iter_mut().zip(block.as_chunks::<4>().0) {
        *word = u32::from_le_bytes(*bytes);
    }
    let [mut a, mut b, mut c, mut d] = *state;
    for round in 0..64 {
        let (mixed, index) = match round / 16 {
            0 => ((b & c) | (!b & d), round),
            1 => ((d & b) | (!d & c), (5 * round + 1) % 16),
            2 => (b ^ c ^ d, (3 * round + 5) % 16),
            _ => (c ^ (b | !d), (7 * round) % 16),
        };
        let sum = a
            .wrapping_add(mixed)
            .wrapping_add(MD5_SINES[round])
            .wrapping_add(message[index]);
        a = d;
        d = c;
        c = b;
        b = b.wrapping_add(sum.rotate_left(MD5_SHIFTS[round]));
    }
    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
}

fn sha1_compress(state: &mut [u32; 5], block: &[u8]) {
    assert_eq!(block.len(), SMALL_BLOCK_BYTES);
    let mut schedule = [0u32; 80];
    for (word, bytes) in schedule.iter_mut().zip(block.as_chunks::<4>().0) {
        *word = u32::from_be_bytes(*bytes);
    }
    for index in 16..80 {
        schedule[index] = (schedule[index - 3]
            ^ schedule[index - 8]
            ^ schedule[index - 14]
            ^ schedule[index - 16])
            .rotate_left(1);
    }
    let [mut a, mut b, mut c, mut d, mut e] = *state;
    for (round, word) in schedule.iter().enumerate() {
        let mixed = match round / 20 {
            0 => (b & c) | (!b & d),
            1 | 3 => b ^ c ^ d,
            _ => (b & c) | (b & d) | (c & d),
        };
        let next = a
            .rotate_left(5)
            .wrapping_add(mixed)
            .wrapping_add(e)
            .wrapping_add(SHA1_ROUND_CONSTANTS[round / 20])
            .wrapping_add(*word);
        e = d;
        d = c;
        c = b.rotate_left(30);
        b = a;
        a = next;
    }
    for (word, add) in state.iter_mut().zip([a, b, c, d, e]) {
        *word = word.wrapping_add(add);
    }
}

fn sha256_compress(state: &mut [u32; 8], block: &[u8]) {
    assert_eq!(block.len(), SMALL_BLOCK_BYTES);
    let mut schedule = [0u32; 64];
    for (word, bytes) in schedule.iter_mut().zip(block.as_chunks::<4>().0) {
        *word = u32::from_be_bytes(*bytes);
    }
    for index in 16..64 {
        let low = schedule[index - 15];
        let high = schedule[index - 2];
        let sigma0 = low.rotate_right(7) ^ low.rotate_right(18) ^ (low >> 3);
        let sigma1 = high.rotate_right(17) ^ high.rotate_right(19) ^ (high >> 10);
        schedule[index] = schedule[index - 16]
            .wrapping_add(sigma0)
            .wrapping_add(schedule[index - 7])
            .wrapping_add(sigma1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for (word, constant) in schedule.iter().zip(SHA256_ROUND_CONSTANTS) {
        let sum1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let choice = (e & f) ^ (!e & g);
        let first = h
            .wrapping_add(sum1)
            .wrapping_add(choice)
            .wrapping_add(constant)
            .wrapping_add(*word);
        let sum0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let majority = (a & b) ^ (a & c) ^ (b & c);
        let second = sum0.wrapping_add(majority);
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(first);
        d = c;
        c = b;
        b = a;
        a = first.wrapping_add(second);
    }
    for (word, add) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
        *word = word.wrapping_add(add);
    }
}

fn sha512_compress(state: &mut [u64; 8], block: &[u8]) {
    assert_eq!(block.len(), BLOCK_BYTES_MAX);
    let mut schedule = [0u64; 80];
    for (word, bytes) in schedule.iter_mut().zip(block.as_chunks::<8>().0) {
        *word = u64::from_be_bytes(*bytes);
    }
    for index in 16..80 {
        let low = schedule[index - 15];
        let high = schedule[index - 2];
        let sigma0 = low.rotate_right(1) ^ low.rotate_right(8) ^ (low >> 7);
        let sigma1 = high.rotate_right(19) ^ high.rotate_right(61) ^ (high >> 6);
        schedule[index] = schedule[index - 16]
            .wrapping_add(sigma0)
            .wrapping_add(schedule[index - 7])
            .wrapping_add(sigma1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for (word, constant) in schedule.iter().zip(SHA512_ROUND_CONSTANTS) {
        let sum1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
        let choice = (e & f) ^ (!e & g);
        let first = h
            .wrapping_add(sum1)
            .wrapping_add(choice)
            .wrapping_add(constant)
            .wrapping_add(*word);
        let sum0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
        let majority = (a & b) ^ (a & c) ^ (b & c);
        let second = sum0.wrapping_add(majority);
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(first);
        d = c;
        c = b;
        b = a;
        a = first.wrapping_add(second);
    }
    for (word, add) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
        *word = word.wrapping_add(add);
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
