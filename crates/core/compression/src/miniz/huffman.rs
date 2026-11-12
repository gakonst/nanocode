use alloc::vec::Vec;

use super::bits::BitWriter;
use super::tables::{
    CODE_BITS_MAX, CODE_LENGTH_SYMBOLS, DEPTHS_MAX, NO_PREVIOUS_LENGTH, REPEAT_COUNT_MIN,
    REPEAT_PREVIOUS, REPEAT_PREVIOUS_COUNT_MAX, REPEAT_ZEROS_LONG, REPEAT_ZEROS_LONG_COUNT_MAX,
    REPEAT_ZEROS_LONG_COUNT_MIN, REPEAT_ZEROS_SHORT, REPEAT_ZEROS_SHORT_COUNT_MAX,
};

pub(super) struct Huffman<const N: usize> {
    pub(super) sizes: [u8; N],
    pub(super) codes: [u16; N],
}

impl<const N: usize> Huffman<N> {
    pub(super) const fn new() -> Self {
        Self {
            sizes: [0; N],
            codes: [0; N],
        }
    }

    pub(super) fn build_from_counts(&mut self, counts: &[u16; N], limit: usize) {
        let mut order = [0u16; N];
        let mut used = 0;
        for (symbol, &count) in counts.iter().enumerate() {
            if count != 0 {
                order[used] = symbol as u16;
                used += 1;
            }
        }
        let order = &mut order[..used];
        order.sort_by_key(|&symbol| counts[usize::from(symbol)]);
        let mut keys = [0u16; N];
        for (key, &symbol) in keys.iter_mut().zip(order.iter()) {
            *key = counts[usize::from(symbol)];
        }
        let keys = &mut keys[..used];
        minimum_redundancy(keys);
        let mut lengths = [0i32; DEPTHS_MAX + 1];
        for &depth in keys.iter() {
            lengths[usize::from(depth)] += 1;
        }
        limit_lengths(&mut lengths, used, limit);
        self.sizes = [0; N];
        let mut last = used;
        for (length, &count) in lengths.iter().enumerate().take(limit + 1).skip(1) {
            let first = last - count as usize;
            for &symbol in &order[first..last] {
                self.sizes[usize::from(symbol)] = length as u8;
            }
            last = first;
        }
        self.assign_codes(limit);
    }

    pub(super) fn assign_codes(&mut self, limit: usize) {
        let mut lengths = [0u32; CODE_BITS_MAX + 1];
        for &size in &self.sizes {
            lengths[usize::from(size)] += 1;
        }
        let mut next = [0u32; CODE_BITS_MAX + 1];
        let mut code = 0;
        for length in 2..=limit {
            code = (code + lengths[length - 1]) << 1;
            next[length] = code;
        }
        for (size, slot) in self.sizes.iter().zip(self.codes.iter_mut()) {
            if *size == 0 {
                *slot = 0;
                continue;
            }
            let code = next[usize::from(*size)];
            next[usize::from(*size)] += 1;
            *slot = (code as u16).reverse_bits() >> (u16::BITS - u32::from(*size));
        }
    }

    #[inline]
    pub(super) fn put(&self, writer: &mut BitWriter, symbol: usize) {
        writer.put(u32::from(self.codes[symbol]), u32::from(self.sizes[symbol]));
    }
}

fn minimum_redundancy(keys: &mut [u16]) {
    let count = keys.len();
    if count == 0 {
        return;
    }
    if count == 1 {
        keys[0] = 1;
        return;
    }
    keys[0] = keys[0].wrapping_add(keys[1]);
    let (mut root, mut leaf) = (0, 2);
    for next in 1..count - 1 {
        if leaf >= count || keys[root] < keys[leaf] {
            keys[next] = keys[root];
            keys[root] = next as u16;
            root += 1;
        } else {
            keys[next] = keys[leaf];
            leaf += 1;
        }
        if leaf >= count || (root < next && keys[root] < keys[leaf]) {
            keys[next] = keys[next].wrapping_add(keys[root]);
            keys[root] = next as u16;
            root += 1;
        } else {
            keys[next] = keys[next].wrapping_add(keys[leaf]);
            leaf += 1;
        }
    }
    keys[count - 2] = 0;
    for next in (0..count - 2).rev() {
        keys[next] = keys[usize::from(keys[next])].wrapping_add(1);
    }
    assign_depths(keys);
}

fn assign_depths(keys: &mut [u16]) {
    let count = keys.len();
    let (mut available, mut used, mut depth) = (1usize, 0usize, 0u16);
    let mut root = count as isize - 2;
    let mut next = count as isize - 1;
    while available > 0 {
        assert!(usize::from(depth) <= count);
        while root >= 0 && keys[root as usize] == depth {
            used += 1;
            root -= 1;
        }
        while available > used {
            keys[next as usize] = depth;
            next -= 1;
            available -= 1;
        }
        available = 2 * used;
        depth += 1;
        used = 0;
    }
}

fn limit_lengths(lengths: &mut [i32; DEPTHS_MAX + 1], used: usize, limit: usize) {
    if used <= 1 {
        return;
    }
    let overflow: i32 = lengths[limit + 1..].iter().sum();
    lengths[limit] += overflow;
    let total: u32 = (1..=limit)
        .map(|length| (lengths[length] as u32) << (limit - length))
        .sum();
    for _ in (1u32 << limit)..total {
        lengths[limit] -= 1;
        for length in (1..limit).rev() {
            if lengths[length] != 0 {
                lengths[length] -= 1;
                lengths[length + 1] += 2;
                break;
            }
        }
    }
}

pub(super) struct Packer {
    zeros: usize,
    repeats: usize,
    previous: u8,
    pub(super) packed: Vec<(u8, u8)>,
    pub(super) counts: [u16; CODE_LENGTH_SYMBOLS],
}

impl Packer {
    pub(super) fn pack(lengths: &[u8]) -> Self {
        let mut packer = Self {
            zeros: 0,
            repeats: 0,
            previous: NO_PREVIOUS_LENGTH,
            packed: Vec::new(),
            counts: [0; CODE_LENGTH_SYMBOLS],
        };
        for &length in lengths {
            if length == 0 {
                packer.flush_repeats();
                packer.zeros += 1;
                if packer.zeros == REPEAT_ZEROS_LONG_COUNT_MAX {
                    packer.flush_zeros();
                }
            } else {
                packer.flush_zeros();
                if length != packer.previous {
                    packer.flush_repeats();
                    packer.emit(length, 0, 1);
                } else {
                    packer.repeats += 1;
                    if packer.repeats == REPEAT_PREVIOUS_COUNT_MAX {
                        packer.flush_repeats();
                    }
                }
            }
            packer.previous = length;
        }
        if packer.repeats != 0 {
            packer.flush_repeats();
        } else {
            packer.flush_zeros();
        }
        packer
    }

    fn emit(&mut self, symbol: u8, extra: u8, times: usize) {
        let slot = &mut self.counts[usize::from(symbol)];
        *slot = slot.wrapping_add(times as u16);
        for _ in 0..times {
            self.packed.push((symbol, extra));
        }
    }

    fn flush_repeats(&mut self) {
        match self.repeats {
            0 => return,
            repeats if repeats < REPEAT_COUNT_MIN => self.emit(self.previous, 0, repeats),
            repeats => self.emit(REPEAT_PREVIOUS, (repeats - REPEAT_COUNT_MIN) as u8, 1),
        }
        self.repeats = 0;
    }

    fn flush_zeros(&mut self) {
        match self.zeros {
            0 => return,
            zeros if zeros < REPEAT_COUNT_MIN => self.emit(0, 0, zeros),
            zeros if zeros <= REPEAT_ZEROS_SHORT_COUNT_MAX => {
                self.emit(REPEAT_ZEROS_SHORT, (zeros - REPEAT_COUNT_MIN) as u8, 1)
            }
            zeros => self.emit(
                REPEAT_ZEROS_LONG,
                (zeros - REPEAT_ZEROS_LONG_COUNT_MIN) as u8,
                1,
            ),
        }
        self.zeros = 0;
    }
}
