use alloc::boxed::Box;
use alloc::vec;

use super::tables::{
    BYTE_BITS, COMPARE_WORDS, DICTIONARY_BYTES, DICTIONARY_MASK, DICTIONARY_PADDED_BYTES,
    HASH_ENTRIES, HASH_MASK, HASH_SHIFT_BITS, LONG_MATCH_LENGTH, MATCH_LENGTH_MAX,
    MATCH_LENGTH_MIN, MIRROR_BYTES, POSITION_MASK, WORD_BYTES,
};

pub(super) struct Dictionary {
    pub(super) bytes: Box<[u8; DICTIONARY_PADDED_BYTES]>,
    pub(super) next: Box<[u16; DICTIONARY_BYTES]>,
    pub(super) hash: Box<[u16; HASH_ENTRIES]>,
    pub(super) size: usize,
    pub(super) lookahead_size: usize,
    pub(super) lookahead_position: usize,
    pub(super) block_start: usize,
}

impl Dictionary {
    pub(super) fn new() -> Self {
        Self {
            bytes: zeroed(),
            next: zeroed(),
            hash: zeroed(),
            size: 0,
            lookahead_size: 0,
            lookahead_position: 0,
            block_start: 0,
        }
    }

    #[inline]
    fn read_u16(&self, index: usize) -> u16 {
        u16::from_le_bytes([self.bytes[index], self.bytes[index + 1]])
    }

    #[inline]
    pub(super) fn read_u32(&self, position: usize) -> u32 {
        let index = position & DICTIONARY_MASK;
        u32::from_le_bytes(self.bytes[index..index + 4].try_into().expect("four bytes"))
    }

    #[inline]
    fn read_u64(&self, position: usize) -> u64 {
        let index = position & DICTIONARY_MASK;
        u64::from_le_bytes(
            self.bytes[index..index + WORD_BYTES]
                .try_into()
                .expect("eight bytes"),
        )
    }

    #[inline]
    fn store(&mut self, destination: usize, byte: u8) {
        self.bytes[destination] = byte;
        if destination < MIRROR_BYTES {
            self.bytes[DICTIONARY_BYTES + destination] = byte;
        }
    }

    pub(super) fn load_plain(&mut self, chunk: &[u8]) {
        let mut destination = (self.lookahead_position + self.lookahead_size) & DICTIONARY_MASK;
        self.lookahead_size += chunk.len();
        let mut rest = chunk;
        while !rest.is_empty() {
            let bytes = (DICTIONARY_BYTES - destination).min(rest.len());
            self.bytes[destination..destination + bytes].copy_from_slice(&rest[..bytes]);
            if destination < MIRROR_BYTES {
                let mirrored = bytes.min(MIRROR_BYTES - destination);
                self.bytes
                    [DICTIONARY_BYTES + destination..DICTIONARY_BYTES + destination + mirrored]
                    .copy_from_slice(&rest[..mirrored]);
            }
            rest = &rest[bytes..];
            destination = (destination + bytes) & DICTIONARY_MASK;
        }
    }

    pub(super) fn load_hashed(&mut self, chunk: &[u8]) {
        if self.lookahead_size + self.size >= MATCH_LENGTH_MIN - 1 && !chunk.is_empty() {
            let mut destination = (self.lookahead_position + self.lookahead_size) & DICTIONARY_MASK;
            let first_insert = self.lookahead_position + self.lookahead_size - 2;
            let mut hash = roll(
                u32::from(self.bytes[first_insert & DICTIONARY_MASK]),
                self.bytes[(first_insert + 1) & DICTIONARY_MASK],
            );
            self.lookahead_size += chunk.len();
            for (insert, &byte) in (first_insert..).zip(chunk) {
                self.store(destination, byte);
                hash = roll(hash, byte);
                self.next[insert & DICTIONARY_MASK] = self.hash[hash as usize];
                self.hash[hash as usize] = insert as u16;
                destination = (destination + 1) & DICTIONARY_MASK;
            }
            return;
        }
        for &byte in chunk {
            let destination = (self.lookahead_position + self.lookahead_size) & DICTIONARY_MASK;
            self.store(destination, byte);
            self.lookahead_size += 1;
            if self.lookahead_size + self.size >= MATCH_LENGTH_MIN {
                let insert = self.lookahead_position + self.lookahead_size - MATCH_LENGTH_MIN;
                let first = u32::from(self.bytes[insert & DICTIONARY_MASK]);
                let second = u32::from(self.bytes[(insert + 1) & DICTIONARY_MASK]);
                let hash = ((first << (2 * HASH_SHIFT_BITS))
                    ^ (second << HASH_SHIFT_BITS)
                    ^ u32::from(byte))
                    & HASH_MASK;
                self.next[insert & DICTIONARY_MASK] = self.hash[hash as usize];
                self.hash[hash as usize] = insert as u16;
            }
        }
    }

    pub(super) fn find_match(&self, probes: [u32; 2], longest: u32, length: u32) -> (u32, u32) {
        let longest = longest.min(MATCH_LENGTH_MAX as u32);
        let mut best_length = length.max(1);
        let mut best_distance = 0;
        if longest <= best_length {
            return (best_distance, best_length);
        }
        let position = self.lookahead_position & DICTIONARY_MASK;
        let mut probe = position;
        let mut probes_left = if best_length < LONG_MATCH_LENGTH {
            probes[0]
        } else {
            probes[1]
        };
        let mut tail = self.read_u16(position + best_length as usize - 1);
        let head = self.read_u16(position);
        loop {
            let Some(distance) =
                self.next_candidate(&mut probe, &mut probes_left, best_length, tail)
            else {
                return (best_distance, best_length);
            };
            if distance == 0 {
                return (best_distance, best_length);
            }
            if self.read_u16(probe) != head {
                continue;
            }
            let Some(matched) = self.compare(position + 2, probe + 2, position) else {
                return (distance, longest);
            };
            if matched > best_length as usize {
                best_distance = distance;
                best_length = longest.min(matched as u32);
                if best_length >= longest {
                    return (best_distance, best_length);
                }
                tail = self.read_u16((position + best_length as usize).saturating_sub(1));
            }
        }
    }

    #[inline]
    fn next_candidate(
        &self,
        probe: &mut usize,
        probes_left: &mut u32,
        length: u32,
        tail: u16,
    ) -> Option<u32> {
        loop {
            *probes_left -= 1;
            if *probes_left == 0 {
                return None;
            }
            for _ in 0..3 {
                let candidate = usize::from(self.next[*probe]);
                let distance = self.lookahead_position.wrapping_sub(candidate) & POSITION_MASK;
                if candidate == 0
                    || distance > self.size
                    || length == 0
                    || length as usize > MATCH_LENGTH_MAX
                {
                    return None;
                }
                *probe = candidate & DICTIONARY_MASK;
                if self.read_u16(*probe + length as usize - 1) == tail {
                    return Some(distance as u32);
                }
            }
        }
    }

    #[inline]
    pub(super) fn compare(
        &self,
        mut current: usize,
        mut candidate: usize,
        start: usize,
    ) -> Option<usize> {
        for _ in 0..COMPARE_WORDS {
            let difference = self.read_u64(current) ^ self.read_u64(candidate);
            if difference != 0 {
                return Some(current - start + (difference.trailing_zeros() / BYTE_BITS) as usize);
            }
            current += WORD_BYTES;
            candidate += WORD_BYTES;
        }
        None
    }
}

fn zeroed<T: Copy + Default, const N: usize>() -> Box<[T; N]> {
    vec![T::default(); N]
        .into_boxed_slice()
        .try_into()
        .unwrap_or_else(|_| unreachable!("a vector of N elements converts to an array of N"))
}

fn roll(hash: u32, byte: u8) -> u32 {
    ((hash << HASH_SHIFT_BITS) ^ u32::from(byte)) & HASH_MASK
}
