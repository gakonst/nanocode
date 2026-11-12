use super::{
    bits::BitWriter,
    fse::{self, CState, CTable},
};

pub(crate) const SYMBOLS: usize = 256;
const WEIGHT_MAX: usize = 12;
const WEIGHT_TABLE_LOG_MAX: u32 = 6;
const DIRECT_WEIGHTS_MAX: usize = 128;
const COMPRESSED_HEADER_MAX: usize = 127;

pub(crate) struct Huffman {
    lengths: [u8; SYMBOLS],
    codes: [u16; SYMBOLS],
    max_bits: u32,
    max_symbol: usize,
    weights: [u8; SYMBOLS],
    weight_table: CTable,
    spread: Vec<u8>,
    scratch: Vec<u8>,
    nodes: Vec<(u64, usize)>,
}

impl Default for Huffman {
    fn default() -> Self {
        Self {
            lengths: [0; SYMBOLS],
            codes: [0; SYMBOLS],
            max_bits: 0,
            max_symbol: 0,
            weights: [0; SYMBOLS],
            weight_table: CTable::default(),
            spread: Vec::new(),
            scratch: Vec::new(),
            nodes: Vec::with_capacity(2 * SYMBOLS),
        }
    }
}

impl Huffman {
    pub(crate) fn build(&mut self, counts: &[u32; SYMBOLS], max_symbol: usize, limit: u32) {
        self.lengths = [0; SYMBOLS];
        self.max_symbol = max_symbol;
        self.optimal_lengths(counts, max_symbol);
        self.limit_lengths(counts, limit);
        self.max_bits = u32::from(*self.lengths.iter().max().unwrap_or(&0));
        assert!(self.max_bits >= 1 && self.max_bits <= limit);
        let kraft: u64 = self
            .lengths
            .iter()
            .filter(|&&length| length > 0)
            .map(|&length| 1_u64 << (self.max_bits - u32::from(length)))
            .sum();
        assert_eq!(kraft, 1 << self.max_bits, "a Huffman code is complete");
        self.assign_codes();
    }

    fn optimal_lengths(&mut self, counts: &[u32; SYMBOLS], max_symbol: usize) {
        self.nodes.clear();
        for (symbol, &count) in counts[..=max_symbol].iter().enumerate() {
            if count > 0 {
                self.nodes.push((u64::from(count), symbol));
            }
        }
        let leaves = self.nodes.len();
        assert!(leaves >= 2, "a Huffman code needs two symbols");
        self.nodes.sort_unstable();
        let mut parents = [0_usize; 2 * SYMBOLS];
        let mut weights = [0_u64; 2 * SYMBOLS];
        for (index, &(count, _)) in self.nodes.iter().enumerate() {
            weights[index] = count;
        }
        let (mut leaf, mut inner) = (0, leaves);
        for node in leaves..2 * leaves - 1 {
            let mut pick = || {
                if leaf < leaves && (inner >= node || weights[leaf] <= weights[inner]) {
                    leaf += 1;
                    leaf - 1
                } else {
                    inner += 1;
                    inner - 1
                }
            };
            let first = pick();
            let second = pick();
            weights[node] = weights[first] + weights[second];
            parents[first] = node;
            parents[second] = node;
        }
        let root = 2 * leaves - 2;
        let mut depths = [0_u8; 2 * SYMBOLS];
        for node in (0..root).rev() {
            depths[node] = depths[parents[node]].saturating_add(1);
        }
        for (index, &(_, symbol)) in self.nodes.iter().enumerate() {
            self.lengths[symbol] = depths[index];
        }
    }

    fn limit_lengths(&mut self, counts: &[u32; SYMBOLS], limit: u32) {
        let limit_length = limit as u8;
        if self.lengths.iter().all(|&length| length <= limit_length) {
            return;
        }
        let full = 1_u64 << limit;
        let mut kraft: u64 = 0;
        for length in &mut self.lengths {
            if *length > limit_length {
                *length = limit_length;
            }
            if *length > 0 {
                kraft += 1 << (limit - u32::from(*length));
            }
        }
        while kraft > full {
            let symbol = (0..SYMBOLS)
                .filter(|&symbol| self.lengths[symbol] > 0 && self.lengths[symbol] < limit_length)
                .max_by_key(|&symbol| (self.lengths[symbol], std::cmp::Reverse(counts[symbol])))
                .expect("an over-full code has a symbol shorter than the limit");
            self.lengths[symbol] += 1;
            kraft -= 1 << (limit - u32::from(self.lengths[symbol]));
        }
        while kraft < full {
            let room = full - kraft;
            let symbol = (0..SYMBOLS)
                .filter(|&symbol| {
                    self.lengths[symbol] > 1
                        && (1_u64 << (limit - u32::from(self.lengths[symbol]))) <= room
                })
                .max_by_key(|&symbol| (counts[symbol], std::cmp::Reverse(symbol)))
                .expect("an incomplete code has a symbol to shorten");
            kraft += 1 << (limit - u32::from(self.lengths[symbol]));
            self.lengths[symbol] -= 1;
        }
    }

    fn assign_codes(&mut self) {
        let mut per_length = [0_u16; WEIGHT_MAX + 1];
        for &length in &self.lengths[..=self.max_symbol] {
            per_length[usize::from(length)] += 1;
        }
        let mut next = [0_u16; WEIGHT_MAX + 1];
        let mut start = 0_u16;
        for length in (1..=self.max_bits as usize).rev() {
            next[length] = start;
            start += per_length[length];
            start >>= 1;
        }
        for symbol in 0..=self.max_symbol {
            let length = usize::from(self.lengths[symbol]);
            if length > 0 {
                self.codes[symbol] = next[length];
                next[length] += 1;
            }
        }
    }

    pub(crate) fn write_description(&mut self, out: &mut Vec<u8>) -> bool {
        let count = self.max_symbol;
        for symbol in 0..count {
            let length = u32::from(self.lengths[symbol]);
            self.weights[symbol] = if length == 0 {
                0
            } else {
                (self.max_bits + 1 - length) as u8
            };
        }
        let compressed = self.compress_weights(count);
        let direct = count.div_ceil(2);
        match compressed {
            Some(size) if count > DIRECT_WEIGHTS_MAX || size < direct => {
                out.push(size as u8);
                out.extend_from_slice(&self.scratch);
                true
            }
            _ if count <= DIRECT_WEIGHTS_MAX => {
                out.push((COMPRESSED_HEADER_MAX + count) as u8);
                for pair in self.weights[..count].chunks(2) {
                    let low = pair.get(1).copied().unwrap_or(0);
                    out.push((pair[0] << 4) | low);
                }
                true
            }
            _ => false,
        }
    }

    fn compress_weights(&mut self, count: usize) -> Option<usize> {
        if count <= 2 {
            return None;
        }
        let weights = &self.weights[..count];
        let mut histogram = [0_u32; WEIGHT_MAX + 1];
        for &weight in weights {
            histogram[usize::from(weight)] += 1;
        }
        let max_symbol = histogram.iter().rposition(|&n| n > 0)?;
        let most = *histogram.iter().max()?;
        if most as usize == count || most == 1 {
            return None;
        }
        let table_log = fse::optimal_table_log(WEIGHT_TABLE_LOG_MAX, count, max_symbol);
        let mut norm = [0_i16; WEIGHT_MAX + 1];
        fse::normalize(&mut norm, table_log, &histogram, count, max_symbol, false);
        self.scratch.clear();
        fse::write_description(&mut self.scratch, &norm, max_symbol, table_log);
        self.weight_table
            .build(&norm, max_symbol, table_log, &mut self.spread);
        let table = &self.weight_table;
        let mut bits = BitWriter::new(&mut self.scratch);
        let last = count - 1;
        let (mut even, mut odd) = if last.is_multiple_of(2) {
            let even = CState::new(table, weights[last]);
            let odd = CState::new(table, weights[last - 1]);
            (even, odd)
        } else {
            let odd = CState::new(table, weights[last]);
            let even = CState::new(table, weights[last - 1]);
            (even, odd)
        };
        for index in (0..last - 1).rev() {
            if index.is_multiple_of(2) {
                even.encode(&mut bits, table, weights[index]);
            } else {
                odd.encode(&mut bits, table, weights[index]);
            }
        }
        odd.flush(&mut bits, table);
        even.flush(&mut bits, table);
        bits.close();
        let size = self.scratch.len();
        (size <= COMPRESSED_HEADER_MAX).then_some(size)
    }

    pub(crate) fn encode(&self, out: &mut Vec<u8>, symbols: &[u8]) {
        let mut bits = BitWriter::new(out);
        for &symbol in symbols.iter().rev() {
            let symbol = usize::from(symbol);
            bits.add(
                u64::from(self.codes[symbol]),
                u32::from(self.lengths[symbol]),
            );
        }
        bits.close();
    }
}
