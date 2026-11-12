use super::{ALPHABET_SIZE_MAX, CODE_LENGTH_SLOTS};

const HEAP_SLOTS: usize = ALPHABET_SIZE_MAX + 2;
const NODE_SLOTS: usize = ALPHABET_SIZE_MAX * 2;
const WEIGHT_SHIFT: u32 = u8::BITS;
const DEPTH_MASK: i32 = 0xFF;
const WEIGHT_MASK: i32 = !DEPTH_MASK;
const ROOT_PARENT: i32 = -2;
const NO_PARENT: i32 = -1;

const fn add_weights(left: i32, right: i32) -> i32 {
    let depth_left = left & DEPTH_MASK;
    let depth_right = right & DEPTH_MASK;
    let depth = if depth_left > depth_right {
        depth_left
    } else {
        depth_right
    };
    (left & WEIGHT_MASK).wrapping_add(right & WEIGHT_MASK) | (1 + depth)
}

const fn heap_up(heap: &mut [usize; HEAP_SLOTS], weight: &[i32; NODE_SLOTS], mut position: usize) {
    let node = heap[position];
    while weight[node] < weight[heap[position >> 1]] {
        heap[position] = heap[position >> 1];
        position >>= 1;
    }
    heap[position] = node;
}

const fn heap_down(
    heap: &mut [usize; HEAP_SLOTS],
    weight: &[i32; NODE_SLOTS],
    heap_size: usize,
    mut position: usize,
) {
    let node = heap[position];
    loop {
        let mut child = position << 1;
        if child > heap_size {
            break;
        }
        if child < heap_size && weight[heap[child + 1]] < weight[heap[child]] {
            child += 1;
        }
        if weight[node] < weight[heap[child]] {
            break;
        }
        heap[position] = heap[child];
        position = child;
    }
    heap[position] = node;
}

pub(super) fn make_code_lengths(lengths: &mut [u8], frequencies: &[i32], length_max: usize) {
    let alphabet_size = lengths.len();
    assert!(alphabet_size <= ALPHABET_SIZE_MAX);
    assert!(frequencies.len() >= alphabet_size);
    let mut heap = [0_usize; HEAP_SLOTS];
    let mut weight = [0_i32; NODE_SLOTS];
    let mut parent = [0_i32; NODE_SLOTS];
    for (symbol, &frequency) in frequencies[..alphabet_size].iter().enumerate() {
        weight[symbol + 1] = frequency.max(1) << WEIGHT_SHIFT;
    }
    loop {
        build_tree(&mut heap, &mut weight, &mut parent, alphabet_size);
        let mut too_long = false;
        for (symbol, length) in lengths.iter_mut().enumerate() {
            let mut depth = 0_usize;
            let mut node = symbol + 1;
            while parent[node] >= 0 {
                node = usize::try_from(parent[node]).expect("a parent index is non-negative");
                depth += 1;
            }
            *length =
                u8::try_from(depth).expect("a Huffman tree over 258 symbols is under 256 deep");
            too_long |= depth > length_max;
        }
        if !too_long {
            return;
        }
        for node_weight in &mut weight[1..=alphabet_size] {
            *node_weight = (1 + (*node_weight >> WEIGHT_SHIFT) / 2) << WEIGHT_SHIFT;
        }
    }
}

fn build_tree(
    heap: &mut [usize; HEAP_SLOTS],
    weight: &mut [i32; NODE_SLOTS],
    parent: &mut [i32; NODE_SLOTS],
    alphabet_size: usize,
) {
    let mut node_count = alphabet_size;
    let mut heap_size = 0;
    heap[0] = 0;
    weight[0] = 0;
    parent[0] = ROOT_PARENT;
    parent[1..=alphabet_size].fill(NO_PARENT);
    for symbol in 1..=alphabet_size {
        heap_size += 1;
        heap[heap_size] = symbol;
        heap_up(heap, weight, heap_size);
    }
    assert!(heap_size < HEAP_SLOTS);
    while heap_size > 1 {
        let first = heap[1];
        heap[1] = heap[heap_size];
        heap_size -= 1;
        heap_down(heap, weight, heap_size, 1);
        let second = heap[1];
        heap[1] = heap[heap_size];
        heap_size -= 1;
        heap_down(heap, weight, heap_size, 1);
        node_count += 1;
        let node = i32::try_from(node_count).expect("a node index fits in i32");
        parent[first] = node;
        parent[second] = node;
        weight[node_count] = add_weights(weight[first], weight[second]);
        parent[node_count] = NO_PARENT;
        heap_size += 1;
        heap[heap_size] = node_count;
        heap_up(heap, weight, heap_size);
    }
    assert!(node_count < NODE_SLOTS);
}

pub(super) fn assign_codes(codes: &mut [u32], lengths: &[u8], length_min: u8, length_max: u8) {
    let mut code = 0_u32;
    for length in length_min..=length_max {
        for (symbol, &symbol_length) in lengths.iter().enumerate() {
            if symbol_length == length {
                codes[symbol] = code;
                code += 1;
            }
        }
        code <<= 1;
    }
}

pub(super) struct DecodeTable {
    pub(super) limit: [i32; CODE_LENGTH_SLOTS],
    pub(super) base: [i32; CODE_LENGTH_SLOTS],
    pub(super) permutation: [u16; ALPHABET_SIZE_MAX],
    pub(super) length_min: u8,
}

impl DecodeTable {
    pub(super) const fn new() -> Self {
        Self {
            limit: [0; CODE_LENGTH_SLOTS],
            base: [0; CODE_LENGTH_SLOTS],
            permutation: [0; ALPHABET_SIZE_MAX],
            length_min: 0,
        }
    }

    pub(super) fn build(&mut self, lengths: &[u8]) {
        assert!(lengths.len() <= ALPHABET_SIZE_MAX);
        let length_min = lengths
            .iter()
            .copied()
            .min()
            .expect("an alphabet has symbols");
        let length_max = lengths
            .iter()
            .copied()
            .max()
            .expect("an alphabet has symbols");
        self.length_min = length_min;
        let mut next = 0;
        for length in length_min..=length_max {
            for (symbol, &symbol_length) in lengths.iter().enumerate() {
                if symbol_length == length {
                    self.permutation[next] =
                        u16::try_from(symbol).expect("a symbol under 258 fits in u16");
                    next += 1;
                }
            }
        }
        self.base.fill(0);
        for &length in lengths {
            self.base[usize::from(length) + 1] += 1;
        }
        for index in 1..CODE_LENGTH_SLOTS {
            self.base[index] += self.base[index - 1];
        }
        self.limit.fill(0);
        let mut code = 0;
        let (length_min, length_max) = (usize::from(length_min), usize::from(length_max));
        for length in length_min..=length_max {
            code += self.base[length + 1] - self.base[length];
            self.limit[length] = code - 1;
            code <<= 1;
        }
        for length in length_min..length_max {
            self.base[length + 1] = 2 * (self.limit[length] + 1) - self.base[length + 1];
        }
    }
}
