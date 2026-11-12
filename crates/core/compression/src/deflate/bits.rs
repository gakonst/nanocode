use alloc::{vec, vec::Vec};

use super::state::SymbolBuffer;
use super::trees::{
    END_BLOCK, Node, REPEAT_PREVIOUS, REPEAT_ZERO_LONG, REPEAT_ZERO_SHORT, RunLimits,
    STATIC_LITERAL_TREE, encode_distance, encode_length,
};

const BIT_BUFFER_BITS: u8 = 64;
const BLOCK_HEADER_BITS: u8 = 3;
const REPEAT_PREVIOUS_EXTRA_BITS: u8 = 2;
const REPEAT_ZERO_SHORT_EXTRA_BITS: u8 = 3;
const REPEAT_ZERO_LONG_EXTRA_BITS: u8 = 7;

#[derive(Clone, Copy)]
pub(super) enum BlockType {
    Stored,
    StaticTrees,
    DynamicTrees,
}

impl BlockType {
    const fn code(self) -> u64 {
        match self {
            Self::Stored => 0,
            Self::StaticTrees => 1,
            Self::DynamicTrees => 2,
        }
    }
}

#[derive(Clone)]
pub(super) struct Pending {
    buffer: Vec<u8>,
    start: usize,
    length: usize,
}

impl Pending {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            buffer: vec![0; capacity],
            start: 0,
            length: 0,
        }
    }

    pub(super) fn bytes(&self) -> &[u8] {
        &self.buffer[self.start..self.start + self.length]
    }

    pub(super) const fn length(&self) -> usize {
        self.length
    }

    pub(super) const fn capacity(&self) -> usize {
        self.buffer.len()
    }

    const fn remaining(&self) -> usize {
        self.buffer.len() - (self.start + self.length)
    }

    pub(super) fn advance(&mut self, bytes: usize) {
        assert!(bytes <= self.length, "only pending bytes are advanced over");
        self.start += bytes;
        self.length -= bytes;
        if self.length == 0 {
            self.start = 0;
        }
    }

    pub(super) fn rewind(&mut self, bytes: usize) {
        assert!(bytes <= self.length, "only pending bytes are rewound");
        self.length -= bytes;
        if self.length == 0 {
            self.start = 0;
        }
    }

    pub(super) fn extend(&mut self, bytes: &[u8]) {
        assert!(
            bytes.len() <= self.remaining(),
            "pending bytes fit in the buffer"
        );
        let end = self.start + self.length;
        self.buffer[end..end + bytes.len()].copy_from_slice(bytes);
        self.length += bytes.len();
    }
}

#[derive(Clone)]
pub(super) struct BitWriter {
    pub(super) pending: Pending,
    buffer: u64,
    pub(super) bits_valid: u8,
}

impl BitWriter {
    pub(super) const fn new(pending: Pending) -> Self {
        Self {
            pending,
            buffer: 0,
            bits_valid: 0,
        }
    }

    pub(super) fn flush_bits(&mut self) {
        let whole_bytes = self.bits_valid / u8::BITS as u8;
        let removed = whole_bytes * u8::BITS as u8;
        self.pending
            .extend(&self.buffer.to_le_bytes()[..usize::from(whole_bytes)]);
        self.bits_valid -= removed;
        self.buffer = self.buffer.checked_shr(u32::from(removed)).unwrap_or(0);
    }

    pub(super) fn emit_align(&mut self) {
        let bytes = self.bits_valid.div_ceil(u8::BITS as u8);
        self.pending
            .extend(&self.buffer.to_le_bytes()[..usize::from(bytes)]);
        self.buffer = 0;
        self.bits_valid = 0;
    }

    pub(super) fn send_bits(&mut self, value: u64, length: u8) {
        assert!(
            self.bits_valid < BIT_BUFFER_BITS,
            "the bit buffer is never full"
        );
        let total = length + self.bits_valid;
        if total < BIT_BUFFER_BITS {
            self.buffer |= value << self.bits_valid;
            self.bits_valid = total;
            return;
        }
        self.buffer |= value << self.bits_valid;
        self.pending.extend(&self.buffer.to_le_bytes());
        self.buffer = value >> (BIT_BUFFER_BITS - self.bits_valid);
        self.bits_valid = total - BIT_BUFFER_BITS;
    }

    fn send_code(&mut self, code: usize, tree: &[Node]) {
        let node = tree[code];
        self.send_bits(u64::from(node.code()), length_bits(node.length()));
    }

    pub(super) fn align(&mut self) {
        self.emit_tree(BlockType::StaticTrees, false);
        self.emit_end_block(&STATIC_LITERAL_TREE);
        self.flush_bits();
    }

    pub(super) fn emit_tree(&mut self, block_type: BlockType, last: bool) {
        let header = (block_type.code() << 1) | u64::from(last);
        self.send_bits(header, BLOCK_HEADER_BITS);
    }

    pub(super) fn emit_end_block(&mut self, literal_tree: &[Node]) {
        self.send_code(END_BLOCK, literal_tree);
    }

    pub(super) fn emit_end_block_and_align(&mut self, literal_tree: &[Node], last: bool) {
        self.emit_end_block(literal_tree);
        if last {
            self.emit_align();
        }
    }

    pub(super) fn emit_literal(&mut self, literal_tree: &[Node], byte: u8) {
        self.send_code(usize::from(byte), literal_tree);
    }

    pub(super) fn emit_distance(
        &mut self,
        literal_tree: &[Node],
        distance_tree: &[Node],
        length_code: u8,
        distance: u16,
    ) {
        let (mut bits, mut length) = encode_length(literal_tree, length_code);
        let (distance_bits, distance_length) = encode_distance(distance_tree, distance);
        bits |= distance_bits << length;
        length += distance_length;
        self.send_bits(bits, length);
    }

    pub(super) fn compress_block(
        &mut self,
        symbols: &SymbolBuffer,
        literal_tree: &[Node],
        distance_tree: &[Node],
    ) {
        for (distance, length_or_literal) in symbols.iter() {
            if distance == 0 {
                self.emit_literal(literal_tree, length_or_literal);
            } else {
                self.emit_distance(literal_tree, distance_tree, length_or_literal, distance);
            }
        }
        self.emit_end_block(literal_tree);
    }

    pub(super) fn send_tree(&mut self, tree: &[Node], bit_length_tree: &[Node], max_code: usize) {
        let mut previous_length = None;
        let mut next_length = tree[0].length();
        let mut count: u16 = 0;
        let mut limits = RunLimits::after(next_length, None);
        for n in 0..=max_code {
            let current_length = next_length;
            next_length = tree[n + 1].length();
            count += 1;
            if count < limits.max && current_length == next_length {
                continue;
            } else if count < limits.min {
                for _ in 0..count {
                    self.send_code(usize::from(current_length), bit_length_tree);
                }
            } else if current_length != 0 {
                if Some(current_length) != previous_length {
                    self.send_code(usize::from(current_length), bit_length_tree);
                    count -= 1;
                }
                assert!(
                    (RunLimits::REPEAT_PREVIOUS_MIN..=RunLimits::REPEAT_PREVIOUS_MAX)
                        .contains(&count),
                    "a repeat of the previous length is 3 to 6 long"
                );
                self.send_code(REPEAT_PREVIOUS, bit_length_tree);
                self.send_bits(
                    u64::from(count - RunLimits::REPEAT_PREVIOUS_MIN),
                    REPEAT_PREVIOUS_EXTRA_BITS,
                );
            } else if count <= RunLimits::REPEAT_ZERO_SHORT_MAX {
                self.send_code(REPEAT_ZERO_SHORT, bit_length_tree);
                self.send_bits(
                    u64::from(count - RunLimits::REPEAT_ZERO_SHORT_MIN),
                    REPEAT_ZERO_SHORT_EXTRA_BITS,
                );
            } else {
                self.send_code(REPEAT_ZERO_LONG, bit_length_tree);
                self.send_bits(
                    u64::from(count - RunLimits::REPEAT_ZERO_LONG_MIN),
                    REPEAT_ZERO_LONG_EXTRA_BITS,
                );
            }
            count = 0;
            previous_length = Some(current_length);
            limits = RunLimits::after(next_length, Some(current_length));
        }
    }
}

pub(super) fn length_bits(length: u16) -> u8 {
    u8::try_from(length).expect("a code length fits in a byte")
}
