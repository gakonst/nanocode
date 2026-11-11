//! LZRC: the codec of the shipped base image (0122, D2252). LZ77 with an adaptive binary
//! range coder in the manner of LZMA: four repeat distances, literals coded against the
//! byte at the last distance after a match, distances as a slot plus extra bits. The base
//! image is a heap of 8-byte aligned records, which this model compresses about a fifth
//! better than the gzip that serves the module (D020 measures gzip).
//!
//! `decode` never panics on bad input: every read is checked and the output is checked
//! against the length and hash in the header. `encode` (for djs-prelude's build;
//! a module that only decodes links none of it) parses with a price-based search over the
//! whole input; it is deterministic, so a build gives the same bytes.
//!
//! Format: `LZR1`, the output length (u32 LE), the hash of the output (u64 LE),
//! then the range coder's bytes. The hash is `fnv64`'s (FNV-1a over 8-byte words).

#![no_std]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

mod encode;
pub use encode::encode;

const MAGIC: &[u8; 4] = b"LZR1";
const HEADER: usize = 16;
/// The longest match: 2 + 8 + 8 + 255.
pub(crate) const MAX_LEN: usize = 273;

/// What is wrong with a stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// No `LZR1` header, or an output length above the caller's limit.
    Header,
    /// The coded bytes end early.
    Truncated,
    /// A match reaches back before the start, or past the declared length.
    Corrupt,
    /// The output does not have the header's hash.
    Checksum,
}

/// FNV-1a with 64-bit words: over the little-endian 8-byte words of `b`, then its last
/// bytes one by one. A check against corruption, eight times faster than bytewise FNV in
/// wasm: a host's copy of the base is checked once per instance (D2252).
pub fn fnv64(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let (words, rest) = b.as_chunks::<8>();
    for w in words {
        h = (h ^ u64::from_le_bytes(*w)).wrapping_mul(0x100_0000_01b3);
    }
    for &x in rest {
        h = (h ^ x as u64).wrapping_mul(0x100_0000_01b3);
    }
    h
}

// ---- the model (shared by both sides) ------------------------------------------------

pub(crate) const PROB_BITS: u32 = 11;
pub(crate) const PROB_INIT: u16 = 1 << (PROB_BITS - 1);
pub(crate) const MOVE: u32 = 5;

/// States: after a literal, after a match, after a repeat.
pub(crate) const ST_LIT: usize = 0;
pub(crate) const ST_MATCH: usize = 1;
pub(crate) const ST_REP: usize = 2;

pub(crate) struct Len {
    pub choice: u16,
    pub choice2: u16,
    pub low: [[u16; 8]; 4],
    pub mid: [[u16; 8]; 4],
    pub high: [u16; 256],
}

impl Len {
    fn new() -> Len {
        Len {
            choice: PROB_INIT,
            choice2: PROB_INIT,
            low: [[PROB_INIT; 8]; 4],
            mid: [[PROB_INIT; 8]; 4],
            high: [PROB_INIT; 256],
        }
    }
}

pub(crate) struct Model {
    pub is_match: [[u16; 4]; 3],
    pub is_rep: [u16; 3],
    pub rep0: [u16; 3],
    pub rep1: u16,
    pub rep2: u16,
    pub match_len: Len,
    pub rep_len: Len,
    pub slot: [[u16; 64]; 4],
    pub extra: [[u16; 16]; 64],
    pub align: [u16; 16],
    /// Plain literals: by position mod 8 and the previous byte's top 3 bits.
    pub lit: Vec<[u16; 256]>,
    /// Literals after a match, while their bits agree with the byte at rep0: by that bit.
    pub matched: [[u16; 256]; 2],
}

impl Model {
    pub fn new() -> Model {
        Model {
            is_match: [[PROB_INIT; 4]; 3],
            is_rep: [PROB_INIT; 3],
            rep0: [PROB_INIT; 3],
            rep1: PROB_INIT,
            rep2: PROB_INIT,
            match_len: Len::new(),
            rep_len: Len::new(),
            slot: [[PROB_INIT; 64]; 4],
            extra: [[PROB_INIT; 16]; 64],
            align: [PROB_INIT; 16],
            lit: vec![[PROB_INIT; 256]; 64],
            matched: [[PROB_INIT; 256]; 2],
        }
    }
}

/// The literal context of position `pos` after byte `prev`.
pub(crate) fn lit_ctx(pos: usize, prev: u8) -> usize {
    (pos & 7) << 3 | (prev >> 5) as usize
}

/// A distance's slot, the number of extra bits and their value.
pub(crate) fn slot_of(d: u32) -> (usize, u32, u32) {
    if d < 4 {
        return (d as usize, 0, 0);
    }
    let n = 31 - d.leading_zeros();
    let s = 2 * n + ((d >> (n - 1)) & 1);
    (s as usize, n - 1, d & ((1 << (n - 1)) - 1))
}

// ---- decoding ---------------------------------------------------------------------------

struct Decoder<'a> {
    src: &'a [u8],
    at: usize,
    range: u32,
    code: u32,
}

impl Decoder<'_> {
    fn byte(&mut self) -> Result<u8, Error> {
        let b = *self.src.get(self.at).ok_or(Error::Truncated)?;
        self.at += 1;
        Ok(b)
    }

    #[inline]
    fn normalize(&mut self) -> Result<(), Error> {
        if self.range < 1 << 24 {
            self.range <<= 8;
            self.code = (self.code << 8) | self.byte()? as u32;
        }
        Ok(())
    }

    // Out of line: some 20 call sites, decoded once per module (0122: code size).
    #[inline(never)]
    fn bit(&mut self, p: &mut u16) -> Result<u32, Error> {
        let bound = (self.range >> PROB_BITS) * *p as u32;
        let bit = if self.code < bound {
            self.range = bound;
            *p += ((1 << PROB_BITS) - *p) >> MOVE;
            0
        } else {
            self.range -= bound;
            self.code -= bound;
            *p -= *p >> MOVE;
            1
        };
        self.normalize()?;
        Ok(bit)
    }

    fn tree(&mut self, probs: &mut [u16], bits: u32) -> Result<u32, Error> {
        let mut m = 1usize;
        for _ in 0..bits {
            m = (m << 1) | self.bit(&mut probs[m])? as usize;
        }
        Ok(m as u32 - (1 << bits))
    }

    fn direct(&mut self, bits: u32) -> Result<u32, Error> {
        let mut v = 0;
        for _ in 0..bits {
            self.range >>= 1;
            let b = if self.code >= self.range {
                self.code -= self.range;
                1
            } else {
                0
            };
            v = (v << 1) | b;
            self.normalize()?;
        }
        Ok(v)
    }

    fn len(&mut self, l: &mut Len, pc: usize) -> Result<usize, Error> {
        Ok(2 + if self.bit(&mut l.choice)? == 0 {
            self.tree(&mut l.low[pc], 3)? as usize
        } else if self.bit(&mut l.choice2)? == 0 {
            8 + self.tree(&mut l.mid[pc], 3)? as usize
        } else {
            16 + self.tree(&mut l.high, 8)? as usize
        })
    }
}

/// The output length a stream declares, if it has a header.
pub fn decoded_len(packed: &[u8]) -> Option<usize> {
    header(packed).map(|h| h.0)
}

/// The output length and FNV-1a 64 hash a stream declares, if it has a header.
pub fn header(packed: &[u8]) -> Option<(usize, u64)> {
    if packed.len() < HEADER || &packed[..4] != MAGIC {
        return None;
    }
    let n = u32::from_le_bytes(packed[4..8].try_into().unwrap()) as usize;
    Some((n, u64::from_le_bytes(packed[8..16].try_into().unwrap())))
}

/// Whether `bytes` are what `packed` decodes to, by its header (length and hash): for an
/// output obtained some other way (a host that decoded it once for many instances).
pub fn matches(packed: &[u8], bytes: &[u8]) -> bool {
    header(packed).is_some_and(|(n, h)| n == bytes.len() && fnv64(bytes) == h)
}

/// Decodes `packed`, refusing outputs longer than `limit`.
pub fn decode(packed: &[u8], limit: usize) -> Result<Vec<u8>, Error> {
    decode_with_capacity(packed, limit, 0)
}

/// [`decode`] into a buffer of at least `capacity` bytes, allocated after the decoder's
/// own tables: a buffer that is to grow then ends the allocations (a wasm host's page
/// allocator grows it in place).
pub fn decode_with_capacity(
    packed: &[u8],
    limit: usize,
    capacity: usize,
) -> Result<Vec<u8>, Error> {
    let (n, hash) = header(packed).ok_or(Error::Header)?;
    if n > limit {
        return Err(Error::Header);
    }
    let mut d = Decoder {
        src: &packed[HEADER..],
        at: 0,
        range: u32::MAX,
        code: 0,
    };
    // The encoder's first byte is always 0 (its cache), then the code.
    d.byte()?;
    for _ in 0..4 {
        d.code = (d.code << 8) | d.byte()? as u32;
    }
    let mut m = Model::new();
    let mut out = Vec::with_capacity(n.max(capacity));
    let mut reps = [1u32; 4];
    let mut st = ST_LIT;
    while out.len() < n {
        let pos = out.len();
        let pc = pos & 3;
        if d.bit(&mut m.is_match[st][pc])? == 0 {
            let prev = out.last().copied().unwrap_or(0);
            let ctx = lit_ctx(pos, prev);
            let r0 = reps[0] as usize;
            let v = if st != ST_LIT && r0 <= pos {
                let mb = out[pos - r0] as u32;
                let mut x = 1usize;
                let mut same = true;
                for k in (0..8).rev() {
                    let bit = if same {
                        let mbit = ((mb >> k) & 1) as usize;
                        let b = d.bit(&mut m.matched[mbit][x])?;
                        same = b as usize == mbit;
                        b
                    } else {
                        d.bit(&mut m.lit[ctx][x])?
                    };
                    x = (x << 1) | bit as usize;
                }
                (x - 256) as u8
            } else {
                d.tree(&mut m.lit[ctx], 8)? as u8
            };
            out.push(v);
            st = ST_LIT;
            continue;
        }
        let (len, dist) = if d.bit(&mut m.is_rep[st])? == 1 {
            let k = if d.bit(&mut m.rep0[st])? == 0 {
                0
            } else if d.bit(&mut m.rep1)? == 0 {
                1
            } else if d.bit(&mut m.rep2)? == 0 {
                2
            } else {
                3
            };
            let dist = reps[k];
            reps.copy_within(0..k, 1);
            reps[0] = dist;
            st = ST_REP;
            (d.len(&mut m.rep_len, pc)?, dist)
        } else {
            let len = d.len(&mut m.match_len, pc)?;
            let s = d.tree(&mut m.slot[(len - 2).min(3)], 6)? as usize;
            let dist = if s < 4 {
                s as u32
            } else {
                let n = (s >> 1) as u32;
                let nb = n - 1;
                let top = (2 | (s as u32 & 1)) << nb;
                let extra = if nb <= 4 {
                    d.tree(&mut m.extra[s], nb)?
                } else {
                    (d.direct(nb - 4)? << 4) | d.tree(&mut m.align, 4)?
                };
                top | extra
            };
            reps.copy_within(0..3, 1);
            reps[0] = dist;
            st = ST_MATCH;
            (len, dist)
        };
        let dist = dist as usize;
        if dist == 0 || dist > pos || pos + len > n {
            return Err(Error::Corrupt);
        }
        if dist >= len {
            out.extend_from_within(pos - dist..pos - dist + len);
        } else {
            for i in 0..len {
                let b = out[pos - dist + i];
                out.push(b);
            }
        }
    }
    if fnv64(&out) != hash {
        return Err(Error::Checksum);
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
