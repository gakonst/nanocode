// Written for Graviola by Joe Birr-Pixton, 2026.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

//! The SHA3 hash function and SHAKE construction.
//!
//! See <https://nvlpubs.nist.gov/nistpubs/fips/nist.fips.202.pdf>

use core::ops::Range;

use crate::low::{Blockwise, sha3_keccak_f1600, sha3_keccak2of4_f1600, sha3_keccak4_f1600};

/// A context for incremental computation of SHA3-256.
pub struct Sha3_256Context {
    sponge: Sponge<SHA3_256_R_BYTES, SHA_PAD_BYTE>,
}

impl Sha3_256Context {
    /// Start a new SHA3-256 hash computation.
    pub const fn new() -> Self {
        Self {
            sponge: Sponge::new(),
        }
    }

    /// Add `bytes` to the ongoing hash computation.
    pub fn update(&mut self, bytes: &[u8]) {
        self.sponge.absorb(bytes);
    }

    /// Complete the SHA3-256 computation, returning the hash output.
    pub fn finish(self) -> [u8; Self::OUTPUT_SZ] {
        let squeezing = self.sponge.absorb_final();
        let mut digest = [0u8; Self::OUTPUT_SZ];
        squeezing.into_single_squeeze(&mut digest);
        digest
    }

    /// The output size of SHA3-256.
    pub const OUTPUT_SZ: usize = 32;
}

/// A context for incremental computation of SHA3-512.
pub struct Sha3_512Context {
    sponge: Sponge<SHA3_512_R_BYTES, SHA_PAD_BYTE>,
}

impl Sha3_512Context {
    /// Start a new SHA3-512 hash computation.
    pub const fn new() -> Self {
        Self {
            sponge: Sponge::new(),
        }
    }

    /// Add `bytes` to the ongoing hash computation.
    pub fn update(&mut self, bytes: &[u8]) {
        self.sponge.absorb(bytes);
    }

    /// Complete the SHA3-512 computation, returning the hash output.
    pub fn finish(self) -> [u8; Self::OUTPUT_SZ] {
        let sponge = self.sponge.absorb_final();
        let mut digest = [0u8; Self::OUTPUT_SZ];
        sponge.into_single_squeeze(&mut digest);
        digest
    }

    /// The output size of SHA3-512.
    pub const OUTPUT_SZ: usize = 64;
}

/// This is SHAKE128.
///
/// This has one-shot input behaviour (all input must be provided to the
/// constructor [`Shake128::new`]) and incremental output behaviour.
pub struct Shake128 {
    sponge: SqueezingSponge<SHAKE_128_R_BYTES>,
    buffer: [u8; SHAKE_128_R_BYTES],
    buffer_offset: usize,
}

impl Shake128 {
    /// Create a new SHAKE128 instance using `message` as input.
    ///
    /// The items of `message` are processed in order, as if concatenated.
    pub fn new(message: &[&[u8]]) -> Self {
        Self {
            sponge: Sponge::<SHAKE_128_R_BYTES, SHAKE_PAD_BYTE>::new_for_message(message),
            buffer: [0u8; SHAKE_128_R_BYTES],
            buffer_offset: SHAKE_128_R_BYTES,
        }
    }

    /// Read data from this instance into `output`.
    ///
    /// This does not fail.  It always fills `output`.
    pub fn read(&mut self, output: &mut [u8]) {
        self.sponge
            .shake_read(output, &mut self.buffer, &mut self.buffer_offset);
    }
}

pub(crate) type Shake128Sponge = Sponge<SHAKE_128_R_BYTES, SHAKE_PAD_BYTE>;
pub(crate) type Shake128SqueezingSponge = SqueezingSponge<SHAKE_128_R_BYTES>;

/// This is SHAKE256.
///
/// This has one-shot input behaviour (all input must be provided to the
/// constructor [`Shake256::new`]) and incremental output behaviour.
pub struct Shake256 {
    sponge: SqueezingSponge<SHAKE_256_R_BYTES>,
    buffer: [u8; SHAKE_256_R_BYTES],
    buffer_offset: usize,
}

impl Shake256 {
    /// Create a new SHAKE256 instance using `message` as input.
    ///
    /// The items of `message` are processed in order, as if concatenated.
    pub fn new(message: &[&[u8]]) -> Self {
        Self {
            sponge: Sponge::<SHAKE_256_R_BYTES, SHAKE_PAD_BYTE>::new_for_message(message),
            buffer: [0u8; SHAKE_256_R_BYTES],
            buffer_offset: SHAKE_256_R_BYTES,
        }
    }

    /// Read data from this instance into `output`.
    ///
    /// This does not fail.  It always fills `output`.
    pub fn read(&mut self, output: &mut [u8]) {
        self.sponge
            .shake_read(output, &mut self.buffer, &mut self.buffer_offset);
    }

    /// Compute six SHAKE256 outputs in one go.
    ///
    /// Each item of `inputs` shall be 40 bytes in length, which accommodates ML-KEM's `SamplePolyCBD`
    /// requirements of 33 bytes.  The 34th byte shall be `SHAKE_PAD_BYTE` and the remainder shall be
    /// zeroes.
    pub(crate) fn one_shot_sextet(inputs: &[&[u8; 40]; 6], outputs: &mut [[u8; 128]; 6]) {
        debug_assert!(inputs.iter().all(|inp| inp[33] == SHAKE_PAD_BYTE));

        let mut s = [0; 25];
        s[SHAKE_256_R_BYTES / size_of::<u64>() - 1] = 0x8000_0000_0000_0000;

        // First, arrange a by-4 step.

        let mut states = [
            {
                for (i, inp) in inputs[0].chunks_exact(8).enumerate() {
                    s[i] = u64::from_le_bytes(inp.try_into().unwrap());
                }
                s
            },
            {
                for (i, inp) in inputs[1].chunks_exact(8).enumerate() {
                    s[i] = u64::from_le_bytes(inp.try_into().unwrap());
                }
                s
            },
            {
                for (i, inp) in inputs[2].chunks_exact(8).enumerate() {
                    s[i] = u64::from_le_bytes(inp.try_into().unwrap());
                }
                s
            },
            {
                for (i, inp) in inputs[3].chunks_exact(8).enumerate() {
                    s[i] = u64::from_le_bytes(inp.try_into().unwrap());
                }
                s
            },
        ];

        sha3_keccak4_f1600(&mut states, &RC);
        squeeze(&states[0], &mut outputs[0]);
        squeeze(&states[1], &mut outputs[1]);
        squeeze(&states[2], &mut outputs[2]);
        squeeze(&states[3], &mut outputs[3]);

        // Now, complete with a by-2.
        //
        // Note that states[2] and states[3] are unused.  This allows reuse of the
        // by-4 keccak permutation, which is quicker on x86-64 than two by-1 keccak
        // permutations!
        states[0] = {
            for (i, inp) in inputs[4].chunks_exact(8).enumerate() {
                s[i] = u64::from_le_bytes(inp.try_into().unwrap());
            }
            s
        };
        states[1] = {
            for (i, inp) in inputs[5].chunks_exact(8).enumerate() {
                s[i] = u64::from_le_bytes(inp.try_into().unwrap());
            }
            s
        };

        sha3_keccak2of4_f1600(&mut states, &RC);
        squeeze(&states[0], &mut outputs[4]);
        squeeze(&states[1], &mut outputs[5]);

        fn squeeze(states: &[u64; 25], output: &mut [u8; 128]) {
            debug_assert!(output.len() <= SHAKE_256_R_BYTES);
            for (i, ch) in output.chunks_mut(8).enumerate() {
                ch.copy_from_slice(&states[i].to_le_bytes());
            }
        }
    }
}

pub(crate) struct SqueezingSponge<const R: usize> {
    s: [u64; 25],
}

impl<const R: usize> SqueezingSponge<R> {
    /// Squeeze into `output`.
    pub(crate) fn squeeze(&mut self, output: &mut [u8]) {
        for chunk in output.chunks_mut(R) {
            self.squeeze_current(chunk);
            sha3_keccak_f1600(&mut self.s, &RC);
        }
    }

    /// Squeeze into `output`, which must be below `R` in length.
    fn into_single_squeeze(mut self, output: &mut [u8]) {
        debug_assert!(output.len() < R);
        self.squeeze_current(output);
    }

    fn squeeze_current(&mut self, output: &mut [u8]) {
        for (i, ch) in output.chunks_mut(8).enumerate() {
            ch.copy_from_slice(&self.s[i].to_le_bytes()[..ch.len()]);
        }
    }

    fn shake_read(&mut self, output: &mut [u8], buffer: &mut [u8; R], buffer_offset: &mut usize) {
        let output = match *buffer_offset == buffer.len() {
            true => output,
            false => {
                let chunk = Ord::min(R - *buffer_offset, output.len());
                let (output, rest) = output.split_at_mut(chunk);
                output.copy_from_slice(&buffer[*buffer_offset..*buffer_offset + chunk]);
                *buffer_offset += chunk;
                rest
            }
        };

        if output.is_empty() {
            return;
        }

        let whole_chunks = output.len() / R;
        let (whole, tail) = output.split_at_mut(whole_chunks * R);
        self.squeeze(whole);

        if !tail.is_empty() {
            self.squeeze(buffer);
            let chunk = Ord::min(R, tail.len());
            tail.copy_from_slice(&buffer[..chunk]);
            *buffer_offset = chunk;
        }
    }
}

pub(crate) struct SqueezingSponge4xShake128 {
    states: [[u64; 25]; 4],
}

impl SqueezingSponge4xShake128 {
    /// Makes four new `SqueezingSponge`s by processing four SHAKE128 inputs.
    ///
    /// Each item of `input` shall be 40 bytes in length, which matches ML-KEM's requirements
    /// of 34 bytes.  The 35th byte shall be `SHAKE_PAD_BYTE` and the remainder shall be zeroes.
    pub(crate) fn new(inputs: &[&[u8; 40]; 4]) -> Self {
        debug_assert!(inputs.iter().all(|inp| inp[34] == SHAKE_PAD_BYTE));

        // This is gnarly, for the benefit of avoiding a SHAKE_128_R_BYTES-byte buffer which is
        // mostly zeroes, and then decoding the buffer into 64-bit words.
        let mut s = [0; 25];

        s[SHAKE_128_R_BYTES / size_of::<u64>() - 1] = 0x8000_0000_0000_0000;

        let mut states = [
            {
                for (i, inp) in inputs[0].chunks_exact(8).enumerate() {
                    s[i] = u64::from_le_bytes(inp.try_into().unwrap());
                }
                s
            },
            {
                for (i, inp) in inputs[1].chunks_exact(8).enumerate() {
                    s[i] = u64::from_le_bytes(inp.try_into().unwrap());
                }
                s
            },
            {
                for (i, inp) in inputs[2].chunks_exact(8).enumerate() {
                    s[i] = u64::from_le_bytes(inp.try_into().unwrap());
                }
                s
            },
            {
                for (i, inp) in inputs[3].chunks_exact(8).enumerate() {
                    s[i] = u64::from_le_bytes(inp.try_into().unwrap());
                }
                s
            },
        ];

        sha3_keccak4_f1600(&mut states, &RC);

        Self { states }
    }

    pub(crate) fn squeeze(
        mut self,
        output: &mut [[u8; SHAKE_128_R_BYTES * 3]; 4],
    ) -> [SqueezingSpongeObligation<SHAKE_128_R_BYTES>; 4] {
        // nb. we're doing three blocks.  The states are post-absorb, so the sequence should be:
        //
        // - squeeze, keccak-f, squeeze, keccak-f, squeeze
        //
        // This leaves a trailing keccak-f owing; represented by `SqueezingSpongeObligation`.

        fn squeeze_rate(
            states: &[[u64; 25]; 4],
            output: &mut [[u8; SHAKE_128_R_BYTES * 3]; 4],
            span: Range<usize>,
        ) {
            for j in 0..4 {
                for (i, ch) in output[j][span.clone()].chunks_mut(8).enumerate() {
                    ch.copy_from_slice(&states[j][i].to_le_bytes());
                }
            }
        }

        squeeze_rate(&self.states, output, 0..SHAKE_128_R_BYTES);
        sha3_keccak4_f1600(&mut self.states, &RC);
        squeeze_rate(
            &self.states,
            output,
            SHAKE_128_R_BYTES..SHAKE_128_R_BYTES * 2,
        );
        sha3_keccak4_f1600(&mut self.states, &RC);
        squeeze_rate(
            &self.states,
            output,
            SHAKE_128_R_BYTES * 2..SHAKE_128_R_BYTES * 3,
        );

        let [s0, s1, s2, s3] = self.states;
        [
            SqueezingSpongeObligation(s0),
            SqueezingSpongeObligation(s1),
            SqueezingSpongeObligation(s2),
            SqueezingSpongeObligation(s3),
        ]
    }
}

/// A [`SqueezingSponge`] to which we owe a keccak-f application prior to further use.
pub(crate) struct SqueezingSpongeObligation<const R: usize>([u64; 25]);

impl<const R: usize> SqueezingSpongeObligation<R> {
    /// Pay back the debt by applying keccak-f and return the underlying [`SqueezingSponge`]
    pub(crate) fn restitute(mut self) -> SqueezingSponge<R> {
        sha3_keccak_f1600(&mut self.0, &RC);
        SqueezingSponge { s: self.0 }
    }
}

pub(crate) struct Sponge<const R: usize, const PAD: u8> {
    sponge: SqueezingSponge<R>,
    buffer: Blockwise<R>,
}

impl<const R: usize, const PAD: u8> Sponge<R, PAD> {
    const fn new() -> Self {
        Self {
            sponge: SqueezingSponge { s: [0; _] },
            buffer: Blockwise::new(),
        }
    }

    pub(crate) fn new_for_message(message: &[&[u8]]) -> SqueezingSponge<R> {
        let mut s = Self::new();
        for m in message {
            s.absorb(m);
        }
        s.absorb_final()
    }

    fn absorb(&mut self, bytes: &[u8]) {
        let bytes = self.buffer.add_leading(bytes);

        if let Some(block) = self.buffer.take() {
            self.absorb_block(&block);
        }

        let (blocks, remainder) = bytes.as_chunks();
        for block in blocks {
            self.absorb_block(block);
        }

        self.buffer.add_trailing(remainder);
    }

    #[must_use]
    fn absorb_final(mut self) -> SqueezingSponge<R> {
        match R - self.buffer.used() {
            1 => {
                self.buffer.add_leading(&[PAD | 0x80]);
            }
            2 => {
                self.buffer.add_leading(&[PAD, 0x80]);
            }
            n => {
                self.buffer.add(&[PAD]);
                self.buffer.add_leading(&R_ZEROES[..n - 2]);
                self.buffer.add_leading(&[0x80]);
            }
        }
        let padded = self.buffer.take().unwrap();
        self.absorb_block(&padded);
        self.sponge
    }

    fn absorb_block(&mut self, block: &[u8; R]) {
        for (i, block) in block.chunks_exact(8).enumerate() {
            self.sponge.s[i] ^= u64::from_le_bytes(block.try_into().unwrap());
        }
        sha3_keccak_f1600(&mut self.sponge.s, &RC);
    }
}

const RC: [u64; 24] = [
    0x00000000_00000001,
    0x00000000_00008082,
    0x80000000_0000808A,
    0x80000000_80008000,
    0x00000000_0000808B,
    0x00000000_80000001,
    0x80000000_80008081,
    0x80000000_00008009,
    0x00000000_0000008A,
    0x00000000_00000088,
    0x00000000_80008009,
    0x00000000_8000000A,
    0x00000000_8000808B,
    0x80000000_0000008B,
    0x80000000_00008089,
    0x80000000_00008003,
    0x80000000_00008002,
    0x80000000_00000080,
    0x00000000_0000800A,
    0x80000000_8000000A,
    0x80000000_80008081,
    0x80000000_00008080,
    0x00000000_80000001,
    0x80000000_80008008,
];

const R_ZEROES: [u8; SHAKE_128_R_BYTES] = [0; SHAKE_128_R_BYTES];

pub(crate) const SHAKE_128_R_BYTES: usize = (1600 - 256) / 8;
const SHAKE_256_R_BYTES: usize = (1600 - 512) / 8;

const SHA3_256_R_BYTES: usize = (1600 - 512) / 8;
const SHA3_512_R_BYTES: usize = (1600 - 1024) / 8;

/// This is 0b01_1 in keccak bit ordering.
///
/// `01` is the SHA3 domain separation constant, `1` is the multi-rate
/// padding bit.
const SHA_PAD_BYTE: u8 = 0b0000_0110;

/// This is 0b1111_1 in keccak bit ordering.
///
/// `1111` is the SHAKE domain separation constant, `1` is the multi-rate
/// padding bit.
pub(crate) const SHAKE_PAD_BYTE: u8 = 0b0001_1111;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::test::*;

    #[test]
    fn hello() {
        let mut ctx = Sha3_256Context::new();
        ctx.update(b"hello");
        assert_eq!(&ctx.finish(),
                   b"\x33\x38\xbe\x69\x4f\x50\xc5\xf3\x38\x81\x49\x86\xcd\xf0\x68\x64\x53\xa8\x88\xb8\x4f\x42\x4d\x79\x2a\xf4\xb9\x20\x23\x98\xf3\x92");

        let mut ctx = Sha3_512Context::new();
        ctx.update(b"hello");
        assert_eq!(&ctx.finish(),
            b"\x75\xd5\x27\xc3\x68\xf2\xef\xe8\x48\xec\xf6\xb0\x73\xa3\x67\x67\x80\x08\x05\xe9\xee\xf2\xb1\x85\x7d\x5f\x98\x4f\x03\x6e\xb6\xdf\x89\x1d\x75\xf7\x2d\x9b\x15\x45\x18\xc1\xcd\x58\x83\x52\x86\xd1\xda\x9a\x38\xde\xba\x3d\xe9\x8b\x5a\x53\xe5\xed\x78\xa8\x49\x76");
    }

    #[test]
    fn sha3_256_all_lengths() {
        // see cifra `vector_length` and associated
        let mut outer = Sha3_256Context::new();

        for len in 0..1024 {
            let mut inner = Sha3_256Context::new();

            for _ in 0..len {
                inner.update(&[len as u8]);
            }

            outer.update(&inner.finish());
        }

        assert_eq!(&outer.finish(),
                   b"\xf7\xed\xf7\x2b\x34\x8c\xb4\xab\x5e\xe7\x4f\x6c\xae\xaf\x11\xad\xe2\x2f\x04\x65\x84\x8e\x5c\xaa\x14\x38\x7f\xd4\xeb\xdb\x9d\x70");
    }

    #[test]
    fn sha3_512_all_lengths() {
        let mut outer = Sha3_512Context::new();

        for len in 0..1024 {
            let mut inner = Sha3_512Context::new();

            for _ in 0..len {
                inner.update(&[len as u8]);
            }

            outer.update(&inner.finish());
        }

        assert_eq!(&outer.finish(),
                   b"\x3a\x98\x11\x17\xbc\x2f\xa3\x3b\x00\x51\x71\xf8\x80\x86\x33\x7f\x4f\x6c\xe9\xd1\x5c\xb7\x38\xc0\x9b\xe2\x8a\xb6\xd5\x38\xba\xbf\
                     \x7b\xc5\x4e\xbf\x3d\xdb\x53\x4a\x9c\x3c\x10\x85\xe7\x18\x3d\x46\xa5\x8c\xbc\xb0\x15\xb0\xdf\x50\x7a\xad\x0e\xdf\xf3\x54\x8e\xfd");
    }

    #[test]
    fn shake_incremental_read() {
        // Read SHAKE output in a big chunk, and then incrementally with a variety
        // of chunk sizes.
        //
        // The outputs must agree.
        let mut long = [0u8; 4096];
        Shake128::new(&[b"hello"]).read(&mut long);

        let mut short = [0u8; 511];
        let mut s = Shake128::new(&[b"hello"]);
        let mut i = 0;
        let mut len = 1;
        while (i + len) < long.len() {
            s.read(&mut short[..len]);

            assert_eq!(&long[i..i + len], &short[..len]);
            i += len;
            len = ((len + 1) * 2) % short.len();
        }
    }

    #[test]
    fn one_shot_sextet_matches_single() {
        // build the six 40-byte inputs in the format `one_shot_sextet` requires:
        // SEED || index || SHAKE_PAD_BYTE || zeroes
        let mut inputs = [[0u8; 40]; 6];
        for (n, inp) in inputs.iter_mut().enumerate() {
            inp[..32].copy_from_slice(SEED);
            inp[32] = n as u8;
            inp[33] = SHAKE_PAD_BYTE;
        }

        let mut batched = [[0u8; 128]; 6];
        Shake256::one_shot_sextet(
            &[
                &inputs[0], &inputs[1], &inputs[2], &inputs[3], &inputs[4], &inputs[5],
            ],
            &mut batched,
        );

        for (n, inp) in inputs.iter().enumerate() {
            // the absorbed message is the 33 bytes preceding the pad byte
            let mut single = [0u8; 128];
            Shake256::new(&[&inp[..33]]).read(&mut single);
            assert_eq!(batched[n], single);
        }
    }

    #[test]
    fn squeezing_sponge_4x_shake128_matches_single() {
        // four 40-byte inputs: SEED || i || j || SHAKE_PAD_BYTE || zeroes, with each
        // lane given a distinct message so any lane mix-up would be caught.
        let mut inputs = [[0u8; 40]; 4];
        for (n, inp) in inputs.iter_mut().enumerate() {
            inp[..32].copy_from_slice(SEED);
            inp[32] = n as u8;
            inp[33] = (n as u8) ^ 0x5a;
            inp[34] = SHAKE_PAD_BYTE;
        }

        // batched: three rate-blocks up front, then continue via the obligation.
        let mut batched = [[0u8; SHAKE_128_R_BYTES * 3]; 4];
        let obligations =
            SqueezingSponge4xShake128::new(&[&inputs[0], &inputs[1], &inputs[2], &inputs[3]])
                .squeeze(&mut batched);

        const TAIL: usize = SHAKE_128_R_BYTES * 2;
        for (n, (input, obligation)) in inputs.iter().zip(obligations).enumerate() {
            // single reference: absorb the 34-byte message, read the same total length.
            let mut single = [0u8; SHAKE_128_R_BYTES * 3 + TAIL];
            Shake128::new(&[&input[..34]]).read(&mut single);

            // the three eagerly-squeezed blocks must match
            assert_eq!(&batched[n][..], &single[..SHAKE_128_R_BYTES * 3]);

            // and paying off the deferred keccak-f must continue the same stream
            let mut tail = [0u8; TAIL];
            obligation.restitute().squeeze(&mut tail);
            assert_eq!(&tail[..], &single[SHAKE_128_R_BYTES * 3..]);
        }
    }

    const SEED: &[u8; 32] = b"Damn right its better than yours";

    #[test]
    fn cavp_sha3() {
        #[derive(Debug)]
        enum Kind {
            None,
            Sha3_256,
            Sha3_512,
        }

        #[derive(Debug)]
        struct Cavp {
            kind: Kind,
            len: usize,
            message: Vec<u8>,
        }

        impl Default for Cavp {
            fn default() -> Self {
                Self {
                    kind: Kind::None,
                    len: 0,
                    message: Vec::new(),
                }
            }
        }

        impl CavpSink for Cavp {
            fn on_meta(&mut self, meta: &str) {
                self.kind = match meta {
                    "L = 256" => Kind::Sha3_256,
                    "L = 512" => Kind::Sha3_512,
                    _ => panic!("unhandled {meta:?}"),
                };
            }

            fn on_value(&mut self, name: &str, value: Value<'_>) {
                match name {
                    "Len" => self.len = (value.int() / 8) as usize,
                    "Msg" => self.message = value.bytes(),
                    "MD" => match self.kind {
                        Kind::None => {}
                        Kind::Sha3_256 => {
                            let mut h = Sha3_256Context::new();
                            h.update(&self.message[..self.len]);
                            assert_eq!(value.bytes(), h.finish());
                        }
                        Kind::Sha3_512 => {
                            let mut h = Sha3_512Context::new();
                            h.update(&self.message[..self.len]);
                            assert_eq!(value.bytes(), h.finish());
                        }
                    },
                    _ => {
                        todo!("{self:?} value {name} = {value:?}");
                    }
                }
            }
        }

        process_cavp(
            "../thirdparty/cavp/sha3/SHA3_256ShortMsg.rsp",
            &mut Cavp::default(),
        );
        process_cavp(
            "../thirdparty/cavp/sha3/SHA3_256LongMsg.rsp",
            &mut Cavp::default(),
        );
        process_cavp(
            "../thirdparty/cavp/sha3/SHA3_512ShortMsg.rsp",
            &mut Cavp::default(),
        );
        process_cavp(
            "../thirdparty/cavp/sha3/SHA3_512LongMsg.rsp",
            &mut Cavp::default(),
        );
    }

    #[test]
    fn cavp_shake() {
        #[derive(Debug)]
        enum Kind {
            None,
            Shake128,
            Shake256,
        }

        #[derive(Debug)]
        struct Cavp {
            kind: Kind,
            len: usize,
            message: Vec<u8>,
        }

        impl Default for Cavp {
            fn default() -> Self {
                Self {
                    kind: Kind::None,
                    len: 0,
                    message: Vec::new(),
                }
            }
        }

        impl CavpSink for Cavp {
            fn on_meta(&mut self, meta: &str) {
                match meta {
                    "Input Length = 128" => self.len = 16,
                    "Input Length = 256" => self.len = 32,
                    _ => {}
                }
            }

            fn on_value(&mut self, name: &str, value: Value<'_>) {
                match name {
                    "COUNT" | "Outputlen" => {}
                    "Msg" => self.message = value.bytes(),
                    "Len" => self.len = (value.int() / 8) as usize,
                    "Output" => match self.kind {
                        Kind::None => {}
                        Kind::Shake128 => {
                            let mut buffer = vec![0; value.bytes().len()];
                            Shake128::new(&[&self.message[..self.len]]).read(&mut buffer);
                            assert_eq!(value.bytes(), buffer);
                        }
                        Kind::Shake256 => {
                            let mut buffer = vec![0; value.bytes().len()];
                            Shake256::new(&[&self.message[..self.len]]).read(&mut buffer);
                            assert_eq!(value.bytes(), buffer);
                        }
                    },
                    _ => {
                        todo!("{self:?} value {name} = {value:?}");
                    }
                }
            }
        }

        process_cavp(
            "../thirdparty/cavp/sha3/SHAKE128ShortMsg.rsp",
            &mut Cavp {
                kind: Kind::Shake128,
                ..Cavp::default()
            },
        );
        process_cavp(
            "../thirdparty/cavp/sha3/SHAKE128LongMsg.rsp",
            &mut Cavp {
                kind: Kind::Shake128,
                ..Cavp::default()
            },
        );
        process_cavp(
            "../thirdparty/cavp/sha3/SHAKE128VariableOut.rsp",
            &mut Cavp {
                kind: Kind::Shake128,
                ..Cavp::default()
            },
        );

        process_cavp(
            "../thirdparty/cavp/sha3/SHAKE256ShortMsg.rsp",
            &mut Cavp {
                kind: Kind::Shake256,
                ..Cavp::default()
            },
        );
        process_cavp(
            "../thirdparty/cavp/sha3/SHAKE256LongMsg.rsp",
            &mut Cavp {
                kind: Kind::Shake256,
                ..Cavp::default()
            },
        );
        process_cavp(
            "../thirdparty/cavp/sha3/SHAKE256VariableOut.rsp",
            &mut Cavp {
                kind: Kind::Shake256,
                ..Cavp::default()
            },
        );
    }
}
