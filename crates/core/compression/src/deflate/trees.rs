use super::Strategy;
use super::bits::{BitWriter, BlockType, length_bits};
use super::state::{STD_MAX_MATCH, STD_MIN_MATCH, State};

pub(super) const LENGTH_CODES: usize = 29;
pub(super) const LITERALS: usize = 256;
pub(super) const END_BLOCK: usize = 256;
pub(super) const LITERAL_CODES: usize = LITERALS + 1 + LENGTH_CODES;
pub(super) const DISTANCE_CODES: usize = 30;
pub(super) const BIT_LENGTH_CODES: usize = 19;
pub(super) const HEAP_SIZE: usize = 2 * LITERAL_CODES + 1;
pub(super) const REPEAT_PREVIOUS: usize = 16;
pub(super) const REPEAT_ZERO_SHORT: usize = 17;
pub(super) const REPEAT_ZERO_LONG: usize = 18;
const MAX_BITS: usize = 15;
const MAX_BIT_LENGTH_BITS: u16 = 7;
const STATIC_LITERAL_CODES: usize = LITERAL_CODES + 2;
const STATIC_LITERAL_EIGHT_BITS_END: usize = 144;
const STATIC_LITERAL_NINE_BITS_END: usize = 256;
const STATIC_LITERAL_SEVEN_BITS_END: usize = 280;
const STATIC_DISTANCE_BITS: u16 = 5;
const STATIC_SEVEN_BITS: u16 = 7;
const STATIC_EIGHT_BITS: u16 = 8;
const STATIC_NINE_BITS: u16 = 9;
const DISTANCE_CODE_TABLE_LENGTH: usize = 512;
const DISTANCE_CODE_DIRECT_LIMIT: usize = 256;
const DISTANCE_CODE_SHIFT: u32 = 7;
const DISTANCE_CODES_DIRECT: usize = 16;
const LENGTH_CODE_TABLE_LENGTH: usize = STD_MAX_MATCH - STD_MIN_MATCH + 1;
const LITERAL_COUNT_BASE: usize = 257;
const DISTANCE_COUNT_BASE: usize = 1;
const BIT_LENGTH_COUNT_BASE: usize = 4;
const LITERAL_COUNT_BITS: u8 = 5;
const DISTANCE_COUNT_BITS: u8 = 5;
const BIT_LENGTH_COUNT_BITS: u8 = 4;
const BIT_LENGTH_CODE_BITS: u8 = 3;
const BLOCK_HEADER_BITS: usize = 3;
const BYTE_BITS: usize = 8;
const BYTE_ROUND_UP_BITS: usize = BYTE_BITS - 1;
const STORED_LENGTH_FIELDS_BYTES: usize = 4;
const STORED_BLOCK_OVERHEAD_BYTES: usize = 5;
const SCAN_GUARD_LENGTH: u16 = u16::MAX;
const HEAP_SMALLEST: usize = 1;
const FREQUENCY_SHIFT: u32 = 8;
const FORCED_CODES_MIN: usize = 2;

const EXTRA_LENGTH_BITS: [u8; LENGTH_CODES] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const EXTRA_DISTANCE_BITS: [u8; DISTANCE_CODES] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
const EXTRA_BIT_LENGTH_BITS: [u8; BIT_LENGTH_CODES] =
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 3, 7];
const BIT_LENGTH_ORDER: [u8; BIT_LENGTH_CODES] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Node {
    frequency_or_code: u16,
    parent_or_length: u16,
}

impl Node {
    const ZERO: Self = Self {
        frequency_or_code: 0,
        parent_or_length: 0,
    };

    pub(super) const fn frequency(self) -> u16 {
        self.frequency_or_code
    }

    pub(super) const fn code(self) -> u16 {
        self.frequency_or_code
    }

    const fn parent(self) -> u16 {
        self.parent_or_length
    }

    pub(super) const fn length(self) -> u16 {
        self.parent_or_length
    }
}

const fn canonical_tree<const N: usize>(lengths: [u16; N]) -> [Node; N] {
    let mut length_counts = [0_u16; MAX_BITS + 1];
    let mut n = 0;
    while n < N {
        length_counts[lengths[n] as usize] += 1;
        n += 1;
    }
    let mut next_code = [0_u16; MAX_BITS + 1];
    let mut code = 0_u16;
    let mut bits = 1;
    while bits <= MAX_BITS {
        code = (code + length_counts[bits - 1]) << 1;
        next_code[bits] = code;
        bits += 1;
    }
    let mut tree = [Node::ZERO; N];
    let mut n = 0;
    while n < N {
        let length = lengths[n];
        tree[n] = Node {
            frequency_or_code: next_code[length as usize].reverse_bits()
                >> (u16::BITS as u16 - length),
            parent_or_length: length,
        };
        next_code[length as usize] += 1;
        n += 1;
    }
    tree
}

const fn static_literal_lengths() -> [u16; STATIC_LITERAL_CODES] {
    let mut lengths = [0; STATIC_LITERAL_CODES];
    let mut n = 0;
    while n < STATIC_LITERAL_CODES {
        lengths[n] = if n < STATIC_LITERAL_EIGHT_BITS_END {
            STATIC_EIGHT_BITS
        } else if n < STATIC_LITERAL_NINE_BITS_END {
            STATIC_NINE_BITS
        } else if n < STATIC_LITERAL_SEVEN_BITS_END {
            STATIC_SEVEN_BITS
        } else {
            STATIC_EIGHT_BITS
        };
        n += 1;
    }
    lengths
}

pub(super) const STATIC_LITERAL_TREE: [Node; STATIC_LITERAL_CODES] =
    canonical_tree(static_literal_lengths());
pub(super) const STATIC_DISTANCE_TREE: [Node; DISTANCE_CODES] =
    canonical_tree([STATIC_DISTANCE_BITS; DISTANCE_CODES]);

struct CodeTables {
    length_code: [u8; LENGTH_CODE_TABLE_LENGTH],
    base_length: [u8; LENGTH_CODES],
    distance_code: [u8; DISTANCE_CODE_TABLE_LENGTH],
    base_distance: [u16; DISTANCE_CODES],
}

const fn code_tables() -> CodeTables {
    let mut tables = CodeTables {
        length_code: [0; LENGTH_CODE_TABLE_LENGTH],
        base_length: [0; LENGTH_CODES],
        distance_code: [0; DISTANCE_CODE_TABLE_LENGTH],
        base_distance: [0; DISTANCE_CODES],
    };
    let mut length = 0;
    let mut code = 0;
    while code < LENGTH_CODES - 1 {
        tables.base_length[code] = length as u8;
        let mut n = 0;
        while n < 1 << EXTRA_LENGTH_BITS[code] {
            tables.length_code[length] = code as u8;
            length += 1;
            n += 1;
        }
        code += 1;
    }
    tables.length_code[length - 1] = code as u8;
    let mut distance = 0;
    let mut code = 0;
    while code < DISTANCE_CODES_DIRECT {
        tables.base_distance[code] = distance as u16;
        let mut n = 0;
        while n < 1 << EXTRA_DISTANCE_BITS[code] {
            tables.distance_code[distance] = code as u8;
            distance += 1;
            n += 1;
        }
        code += 1;
    }
    distance >>= DISTANCE_CODE_SHIFT;
    while code < DISTANCE_CODES {
        tables.base_distance[code] = (distance << DISTANCE_CODE_SHIFT) as u16;
        let mut n = 0;
        while n < 1 << (EXTRA_DISTANCE_BITS[code] - DISTANCE_CODE_SHIFT as u8) {
            tables.distance_code[DISTANCE_CODE_DIRECT_LIMIT + distance] = code as u8;
            distance += 1;
            n += 1;
        }
        code += 1;
    }
    tables
}

const CODE_TABLES: CodeTables = code_tables();

pub(super) fn length_code(length: u8) -> usize {
    usize::from(CODE_TABLES.length_code[usize::from(length)])
}

pub(super) fn distance_code(distance: usize) -> usize {
    let index = if distance < DISTANCE_CODE_DIRECT_LIMIT {
        distance
    } else {
        DISTANCE_CODE_DIRECT_LIMIT + (distance >> DISTANCE_CODE_SHIFT)
    };
    usize::from(CODE_TABLES.distance_code[index])
}

pub(super) fn encode_length(literal_tree: &[Node], length: u8) -> (u64, u8) {
    let code = length_code(length);
    let node = literal_tree[code + LITERALS + 1];
    let mut bits = u64::from(node.code());
    let mut bits_length = length_bits(node.length());
    let extra = EXTRA_LENGTH_BITS[code];
    if extra != 0 {
        bits |= u64::from(length - CODE_TABLES.base_length[code]) << bits_length;
        bits_length += extra;
    }
    (bits, bits_length)
}

pub(super) fn encode_distance(distance_tree: &[Node], distance: u16) -> (u64, u8) {
    let distance = distance - 1;
    let code = distance_code(usize::from(distance));
    let node = distance_tree[code];
    let mut bits = u64::from(node.code());
    let mut bits_length = length_bits(node.length());
    let extra = EXTRA_DISTANCE_BITS[code];
    if extra != 0 {
        bits |= u64::from(distance - CODE_TABLES.base_distance[code]) << bits_length;
        bits_length += extra;
    }
    (bits, bits_length)
}

pub(super) struct RunLimits {
    pub(super) max: u16,
    pub(super) min: u16,
}

impl RunLimits {
    pub(super) const REPEAT_PREVIOUS_MIN: u16 = 3;
    pub(super) const REPEAT_PREVIOUS_MAX: u16 = 6;
    pub(super) const REPEAT_ZERO_SHORT_MIN: u16 = 3;
    pub(super) const REPEAT_ZERO_SHORT_MAX: u16 = 10;
    pub(super) const REPEAT_ZERO_LONG_MIN: u16 = 11;
    const REPEAT_ZERO_LONG_MAX: u16 = 138;
    const LITERAL_RUN_MAX: u16 = 7;
    const LITERAL_RUN_MIN: u16 = 4;

    pub(super) fn after(next_length: u16, current_length: Option<u16>) -> Self {
        if next_length == 0 {
            Self {
                max: Self::REPEAT_ZERO_LONG_MAX,
                min: Self::REPEAT_ZERO_SHORT_MIN,
            }
        } else if current_length == Some(next_length) {
            Self {
                max: Self::REPEAT_PREVIOUS_MAX,
                min: Self::REPEAT_PREVIOUS_MIN,
            }
        } else {
            Self {
                max: Self::LITERAL_RUN_MAX,
                min: Self::LITERAL_RUN_MIN,
            }
        }
    }
}

pub(super) struct StaticDescription {
    static_tree: &'static [Node],
    extra_bits: &'static [u8],
    extra_base: usize,
    elements: usize,
    max_length: u16,
}

impl StaticDescription {
    pub(super) const LITERAL: Self = Self {
        static_tree: &STATIC_LITERAL_TREE,
        extra_bits: &EXTRA_LENGTH_BITS,
        extra_base: LITERALS + 1,
        elements: LITERAL_CODES,
        max_length: MAX_BITS as u16,
    };
    pub(super) const DISTANCE: Self = Self {
        static_tree: &STATIC_DISTANCE_TREE,
        extra_bits: &EXTRA_DISTANCE_BITS,
        extra_base: 0,
        elements: DISTANCE_CODES,
        max_length: MAX_BITS as u16,
    };
    pub(super) const BIT_LENGTH: Self = Self {
        static_tree: &[],
        extra_bits: &EXTRA_BIT_LENGTH_BITS,
        extra_base: 0,
        elements: BIT_LENGTH_CODES,
        max_length: MAX_BIT_LENGTH_BITS,
    };
}

#[derive(Clone, Copy, Default)]
pub(super) struct BlockLengths {
    optimal_bits: usize,
    static_bits: usize,
}

#[derive(Clone)]
pub(super) struct TreeDescription<const N: usize> {
    pub(super) tree: [Node; N],
    max_code: usize,
    description: &'static StaticDescription,
}

impl<const N: usize> TreeDescription<N> {
    pub(super) const fn new(description: &'static StaticDescription) -> Self {
        Self {
            tree: [Node::ZERO; N],
            max_code: 0,
            description,
        }
    }

    pub(super) fn clear_frequencies(&mut self) {
        for node in &mut self.tree[..self.description.elements] {
            node.frequency_or_code = 0;
        }
    }

    pub(super) const fn mark_end_of_block(&mut self) {
        self.tree[END_BLOCK].frequency_or_code = 1;
    }

    pub(super) fn count_literal(&mut self, byte: u8) {
        self.tree[usize::from(byte)].frequency_or_code += 1;
    }

    pub(super) fn count_length(&mut self, length: u8) {
        self.tree[length_code(length) + LITERALS + 1].frequency_or_code += 1;
    }

    pub(super) fn count_distance(&mut self, distance: usize) {
        self.tree[distance_code(distance)].frequency_or_code += 1;
    }
}

struct Heap {
    heap: [u32; HEAP_SIZE],
    length: usize,
    max: usize,
    depth: [u8; HEAP_SIZE],
}

impl Heap {
    fn new<const N: usize>(tree: &mut [Node; N], elements: usize) -> (Self, Option<usize>) {
        let mut heap = Self {
            heap: [0; HEAP_SIZE],
            length: 0,
            max: HEAP_SIZE,
            depth: [0; HEAP_SIZE],
        };
        let mut max_code = None;
        for (n, node) in tree[..elements].iter_mut().enumerate() {
            if node.frequency() > 0 {
                heap.length += 1;
                heap.heap[heap.length] = u32::try_from(n).expect("a tree index fits in 32 bits");
                max_code = Some(n);
                heap.depth[n] = 0;
            } else {
                node.parent_or_length = 0;
            }
        }
        (heap, max_code)
    }

    fn key(&self, tree: &[Node], node: u32) -> u32 {
        let index = node as usize;
        (u32::from(tree[index].frequency()) << FREQUENCY_SHIFT) | u32::from(self.depth[index])
    }

    fn down(&mut self, tree: &[Node], mut k: usize) {
        let value = self.heap[k];
        let value_key = self.key(tree, value);
        let mut j = k << 1;
        while j <= self.length {
            let mut j_key = self.key(tree, self.heap[j]);
            if j < self.length {
                let sibling_key = self.key(tree, self.heap[j + 1]);
                if sibling_key <= j_key {
                    j += 1;
                    j_key = sibling_key;
                }
            }
            if value_key <= j_key {
                break;
            }
            self.heap[k] = self.heap[j];
            k = j;
            j <<= 1;
        }
        self.heap[k] = value;
    }

    fn remove(&mut self, tree: &[Node]) -> usize {
        let top = self.heap[HEAP_SMALLEST];
        self.heap[HEAP_SMALLEST] = self.heap[self.length];
        self.length -= 1;
        self.down(tree, HEAP_SMALLEST);
        top as usize
    }

    fn construct(&mut self, tree: &mut [Node], mut node: usize) {
        loop {
            let n = self.remove(tree);
            let m = self.heap[HEAP_SMALLEST] as usize;
            self.max -= 1;
            self.heap[self.max] = n as u32;
            self.max -= 1;
            self.heap[self.max] = m as u32;
            tree[node].frequency_or_code = tree[n].frequency() + tree[m].frequency();
            self.depth[node] = self.depth[n].max(self.depth[m]) + 1;
            let parent = u16::try_from(node).expect("a tree node index fits in 16 bits");
            tree[n].parent_or_length = parent;
            tree[m].parent_or_length = parent;
            self.heap[HEAP_SMALLEST] = node as u32;
            node += 1;
            self.down(tree, HEAP_SMALLEST);
            if self.length < 2 {
                break;
            }
        }
        self.max -= 1;
        self.heap[self.max] = self.heap[HEAP_SMALLEST];
    }
}

fn build_tree<const N: usize>(desc: &mut TreeDescription<N>, lengths: &mut BlockLengths) {
    let description = desc.description;
    let tree = &mut desc.tree;
    let (mut heap, max_code) = Heap::new(tree, description.elements);
    let mut max_code = max_code.map_or(-1, signed_code);
    while heap.length < FORCED_CODES_MIN {
        heap.length += 1;
        let node = if max_code < 2 {
            max_code += 1;
            usize::try_from(max_code).expect("a forced code is not negative")
        } else {
            0
        };
        heap.heap[heap.length] = node as u32;
        tree[node].frequency_or_code = 1;
        heap.depth[node] = 0;
        lengths.optimal_bits -= 1;
        if !description.static_tree.is_empty() {
            lengths.static_bits -= usize::from(description.static_tree[node].length());
        }
    }
    let max_code = usize::try_from(max_code).expect("a tree has codes after forcing two");
    desc.max_code = max_code;
    let mut n = heap.length / 2;
    while n >= 1 {
        heap.down(tree, n);
        n -= 1;
    }
    heap.construct(tree, description.elements);
    let length_counts = generate_lengths(tree, &mut heap, max_code, description, lengths);
    generate_codes(tree, max_code, &length_counts);
}

fn signed_code(code: usize) -> isize {
    isize::try_from(code).expect("a tree index fits in isize")
}

fn generate_lengths(
    tree: &mut [Node],
    heap: &mut Heap,
    max_code: usize,
    description: &StaticDescription,
    lengths: &mut BlockLengths,
) -> [u16; MAX_BITS + 1] {
    let mut length_counts = [0_u16; MAX_BITS + 1];
    tree[heap.heap[heap.max] as usize].parent_or_length = 0;
    let mut overflow: i32 = 0;
    for &n in &heap.heap[heap.max + 1..HEAP_SIZE] {
        let n = n as usize;
        let mut bits = tree[usize::from(tree[n].parent())].length() + 1;
        if bits > description.max_length {
            bits = description.max_length;
            overflow += 1;
        }
        tree[n].parent_or_length = bits;
        if n > max_code {
            continue;
        }
        length_counts[usize::from(bits)] += 1;
        let extra = if n >= description.extra_base {
            usize::from(description.extra_bits[n - description.extra_base])
        } else {
            0
        };
        let frequency = usize::from(tree[n].frequency());
        lengths.optimal_bits += frequency * (usize::from(bits) + extra);
        if !description.static_tree.is_empty() {
            lengths.static_bits +=
                frequency * (usize::from(description.static_tree[n].length()) + extra);
        }
    }
    if overflow == 0 {
        return length_counts;
    }
    rebalance_lengths(&mut length_counts, overflow, description.max_length);
    let mut h = HEAP_SIZE;
    for bits in (1..=description.max_length).rev() {
        let mut n = length_counts[usize::from(bits)];
        while n != 0 {
            h -= 1;
            let m = heap.heap[h] as usize;
            if m > max_code {
                continue;
            }
            if tree[m].length() != bits {
                let frequency = usize::from(tree[m].frequency());
                lengths.optimal_bits += usize::from(bits) * frequency;
                lengths.optimal_bits -= usize::from(tree[m].length()) * frequency;
                tree[m].parent_or_length = bits;
            }
            n -= 1;
        }
    }
    length_counts
}

fn rebalance_lengths(length_counts: &mut [u16; MAX_BITS + 1], mut overflow: i32, max_length: u16) {
    let max_length = usize::from(max_length);
    loop {
        let mut bits = max_length - 1;
        while length_counts[bits] == 0 {
            bits -= 1;
        }
        length_counts[bits] -= 1;
        length_counts[bits + 1] += 2;
        length_counts[max_length] -= 1;
        overflow -= 2;
        if overflow <= 0 {
            break;
        }
    }
}

fn generate_codes(tree: &mut [Node], max_code: usize, length_counts: &[u16; MAX_BITS + 1]) {
    let mut next_code = [0_u16; MAX_BITS + 1];
    let mut code = 0_u16;
    for bits in 1..=MAX_BITS {
        code = (code + length_counts[bits - 1]) << 1;
        next_code[bits] = code;
    }
    assert!(
        u32::from(code) + u32::from(length_counts[MAX_BITS]) == 1 << MAX_BITS,
        "the code lengths form a complete code"
    );
    for node in &mut tree[..=max_code] {
        let length = node.length();
        if length == 0 {
            continue;
        }
        assert!(usize::from(length) <= MAX_BITS, "a code is at most 15 bits");
        node.frequency_or_code =
            next_code[usize::from(length)].reverse_bits() >> (u16::BITS as u16 - length);
        next_code[usize::from(length)] += 1;
    }
}

fn scan_tree(bit_length_tree: &mut [Node], tree: &mut [Node], max_code: usize) {
    let mut previous_length = None;
    let mut next_length = tree[0].length();
    let mut count: u16 = 0;
    let mut limits = RunLimits::after(next_length, None);
    tree[max_code + 1].parent_or_length = SCAN_GUARD_LENGTH;
    for n in 0..=max_code {
        let current_length = next_length;
        next_length = tree[n + 1].length();
        count += 1;
        if count < limits.max && current_length == next_length {
            continue;
        } else if count < limits.min {
            bit_length_tree[usize::from(current_length)].frequency_or_code += count;
        } else if current_length != 0 {
            if Some(current_length) != previous_length {
                bit_length_tree[usize::from(current_length)].frequency_or_code += 1;
            }
            bit_length_tree[REPEAT_PREVIOUS].frequency_or_code += 1;
        } else if count <= RunLimits::REPEAT_ZERO_SHORT_MAX {
            bit_length_tree[REPEAT_ZERO_SHORT].frequency_or_code += 1;
        } else {
            bit_length_tree[REPEAT_ZERO_LONG].frequency_or_code += 1;
        }
        count = 0;
        previous_length = Some(current_length);
        limits = RunLimits::after(next_length, Some(current_length));
    }
}

pub(super) fn stored_block(bits: &mut BitWriter, data: &[u8], last: bool) {
    bits.emit_tree(BlockType::Stored, last);
    bits.emit_align();
    let length = u16::try_from(data.len()).expect("a stored block is at most 65535 bytes");
    bits.pending.extend(&length.to_le_bytes());
    bits.pending.extend(&(!length).to_le_bytes());
    if length > 0 {
        bits.pending.extend(data);
    }
}

impl State {
    fn build_bit_length_tree(&mut self) -> usize {
        scan_tree(
            &mut self.bit_length_tree.tree,
            &mut self.literal_tree.tree,
            self.literal_tree.max_code,
        );
        scan_tree(
            &mut self.bit_length_tree.tree,
            &mut self.distance_tree.tree,
            self.distance_tree.max_code,
        );
        build_tree(&mut self.bit_length_tree, &mut self.lengths);
        let mut max_index = BIT_LENGTH_CODES - 1;
        while max_index >= BIT_LENGTH_COUNT_BASE - 1 {
            let code = usize::from(BIT_LENGTH_ORDER[max_index]);
            if self.bit_length_tree.tree[code].length() != 0 {
                break;
            }
            max_index -= 1;
        }
        self.lengths.optimal_bits += usize::from(BIT_LENGTH_CODE_BITS) * (max_index + 1)
            + usize::from(LITERAL_COUNT_BITS)
            + usize::from(DISTANCE_COUNT_BITS)
            + usize::from(BIT_LENGTH_COUNT_BITS);
        max_index
    }

    fn send_all_trees(
        &mut self,
        literal_codes: usize,
        distance_codes: usize,
        bit_length_codes: usize,
    ) {
        let counts = [
            (literal_codes - LITERAL_COUNT_BASE, LITERAL_COUNT_BITS),
            (distance_codes - DISTANCE_COUNT_BASE, DISTANCE_COUNT_BITS),
            (
                bit_length_codes - BIT_LENGTH_COUNT_BASE,
                BIT_LENGTH_COUNT_BITS,
            ),
        ];
        for (count, bits) in counts {
            self.bits.send_bits(count as u64, bits);
        }
        for &code in &BIT_LENGTH_ORDER[..bit_length_codes] {
            let length = self.bit_length_tree.tree[usize::from(code)].length();
            self.bits.send_bits(u64::from(length), BIT_LENGTH_CODE_BITS);
        }
        self.bits.send_tree(
            &self.literal_tree.tree,
            &self.bit_length_tree.tree,
            literal_codes - 1,
        );
        self.bits.send_tree(
            &self.distance_tree.tree,
            &self.bit_length_tree.tree,
            distance_codes - 1,
        );
    }

    pub(super) fn flush_block(
        &mut self,
        window_offset: Option<usize>,
        stored_length: usize,
        last: bool,
    ) {
        let mut max_index = 0;
        let mut optimal_bytes;
        let static_bytes;
        if self.symbols.is_empty() {
            optimal_bytes = 0;
            static_bytes = 0;
        } else if self.level > 0 {
            build_tree(&mut self.literal_tree, &mut self.lengths);
            build_tree(&mut self.distance_tree, &mut self.lengths);
            max_index = self.build_bit_length_tree();
            optimal_bytes =
                (self.lengths.optimal_bits + BLOCK_HEADER_BITS + BYTE_ROUND_UP_BITS) / BYTE_BITS;
            static_bytes =
                (self.lengths.static_bits + BLOCK_HEADER_BITS + BYTE_ROUND_UP_BITS) / BYTE_BITS;
            if static_bytes <= optimal_bytes || self.strategy == Strategy::Fixed {
                optimal_bytes = static_bytes;
            }
        } else {
            assert!(
                window_offset.is_some(),
                "a stored-only block is still in the window"
            );
            optimal_bytes = stored_length + STORED_BLOCK_OVERHEAD_BYTES;
            static_bytes = stored_length + STORED_BLOCK_OVERHEAD_BYTES;
        }
        if stored_length + STORED_LENGTH_FIELDS_BYTES <= optimal_bytes
            && let Some(offset) = window_offset
        {
            stored_block(
                &mut self.bits,
                &self.window[offset..offset + stored_length],
                last,
            );
        } else if static_bytes == optimal_bytes {
            self.bits.emit_tree(BlockType::StaticTrees, last);
            self.bits
                .compress_block(&self.symbols, &STATIC_LITERAL_TREE, &STATIC_DISTANCE_TREE);
        } else {
            self.bits.emit_tree(BlockType::DynamicTrees, last);
            self.send_all_trees(
                self.literal_tree.max_code + 1,
                self.distance_tree.max_code + 1,
                max_index + 1,
            );
            self.bits.compress_block(
                &self.symbols,
                &self.literal_tree.tree,
                &self.distance_tree.tree,
            );
        }
        self.init_block();
        if last {
            self.bits.emit_align();
        }
    }
}
