use super::bits::BitWriter;

pub(crate) const TABLE_LOG_MIN: u32 = 5;
pub(crate) const TABLE_LOG_MAX: u32 = 12;
const ROUNDING_THRESHOLDS: [u64; 8] = [
    0, 473_195, 504_333, 520_860, 550_000, 700_000, 750_000, 830_000,
];
const SYMBOLS_MAX: usize = 256;

pub(crate) const fn highbit(value: u32) -> u32 {
    31 - value.leading_zeros()
}

const fn table_log_min(total: usize, max_symbol: usize) -> u32 {
    let from_total = highbit(total as u32) + 1;
    let from_symbols = highbit(max_symbol as u32) + 2;
    if from_total < from_symbols {
        from_total
    } else {
        from_symbols
    }
}

pub(crate) fn optimal_table_log(table_log_max: u32, total: usize, max_symbol: usize) -> u32 {
    assert!(total > 1, "a distribution of {total} symbols has no table");
    let from_total = i64::from(highbit((total - 1) as u32)) - 2;
    let mut table_log = i64::from(table_log_max).min(from_total);
    table_log = table_log.max(i64::from(table_log_min(total, max_symbol)));
    table_log.clamp(i64::from(TABLE_LOG_MIN), i64::from(TABLE_LOG_MAX)) as u32
}

pub(crate) fn normalize(
    norm: &mut [i16],
    table_log: u32,
    counts: &[u32],
    total: usize,
    max_symbol: usize,
    low_probability: bool,
) {
    assert!((TABLE_LOG_MIN..=TABLE_LOG_MAX).contains(&table_log));
    assert!(table_log >= table_log_min(total, max_symbol));
    let low_count: i16 = if low_probability { -1 } else { 1 };
    let scale = 62 - u64::from(table_log);
    let step = (1_u64 << 62) / total as u64;
    let v_step = 1_u64 << (scale - 20);
    let mut to_distribute: i32 = 1 << table_log;
    let mut largest = 0;
    let mut largest_probability: i16 = 0;
    let low_threshold = (total >> table_log) as u64;
    for symbol in 0..=max_symbol {
        let count = u64::from(counts[symbol]);
        assert!(count < total as u64, "a single-symbol distribution is RLE");
        if count == 0 {
            norm[symbol] = 0;
            continue;
        }
        if count <= low_threshold {
            norm[symbol] = low_count;
            to_distribute -= 1;
            continue;
        }
        let mut probability = ((count * step) >> scale) as i16;
        if probability < 8 {
            let rest_to_beat = v_step * ROUNDING_THRESHOLDS[probability as usize];
            let rest = count * step - ((probability as u64) << scale);
            probability += i16::from(rest > rest_to_beat);
        }
        if probability > largest_probability {
            largest_probability = probability;
            largest = symbol;
        }
        norm[symbol] = probability;
        to_distribute -= i32::from(probability);
    }
    if -to_distribute >= i32::from(norm[largest] >> 1) {
        normalize_slow(norm, table_log, counts, total, max_symbol, low_count);
    } else {
        norm[largest] += to_distribute as i16;
    }
    let sum: i32 = norm[..=max_symbol]
        .iter()
        .map(|&n| i32::from(n.abs()))
        .sum();
    assert_eq!(
        sum,
        1 << table_log,
        "a normalized distribution sums to its table size"
    );
}

fn normalize_slow(
    norm: &mut [i16],
    table_log: u32,
    counts: &[u32],
    total: usize,
    max_symbol: usize,
    low_count: i16,
) {
    const UNASSIGNED: i16 = -2;
    let mut total = total as u64;
    let mut distributed: u32 = 0;
    let low_threshold = total >> table_log;
    let mut low_one = (total * 3) >> (table_log + 1);
    for symbol in 0..=max_symbol {
        let count = u64::from(counts[symbol]);
        if count == 0 {
            norm[symbol] = 0;
        } else if count <= low_threshold {
            norm[symbol] = low_count;
            distributed += 1;
            total -= count;
        } else if count <= low_one {
            norm[symbol] = 1;
            distributed += 1;
            total -= count;
        } else {
            norm[symbol] = UNASSIGNED;
        }
    }
    let mut to_distribute = (1_u32 << table_log) - distributed;
    if to_distribute == 0 {
        return;
    }
    if total / u64::from(to_distribute) > low_one {
        low_one = (total * 3) / (u64::from(to_distribute) * 2);
        for symbol in 0..=max_symbol {
            let count = u64::from(counts[symbol]);
            if norm[symbol] == UNASSIGNED && count <= low_one {
                norm[symbol] = 1;
                distributed += 1;
                total -= count;
            }
        }
        to_distribute = (1_u32 << table_log) - distributed;
    }
    if distributed as usize == max_symbol + 1 {
        let most = (0..=max_symbol)
            .max_by_key(|&symbol| (counts[symbol], std::cmp::Reverse(symbol)))
            .unwrap_or(0);
        norm[most] += to_distribute as i16;
        return;
    }
    if total == 0 {
        let mut symbol = 0;
        while to_distribute > 0 {
            if norm[symbol] > 0 {
                to_distribute -= 1;
                norm[symbol] += 1;
            }
            symbol = (symbol + 1) % (max_symbol + 1);
        }
        return;
    }
    let step_log = 62 - u64::from(table_log);
    let middle = (1_u64 << (step_log - 1)) - 1;
    let step = ((1_u64 << step_log) * u64::from(to_distribute) + middle) / total;
    let mut running = middle;
    for symbol in 0..=max_symbol {
        if norm[symbol] == UNASSIGNED {
            let end = running + u64::from(counts[symbol]) * step;
            let weight = (end >> step_log) - (running >> step_log);
            assert!(weight >= 1, "a symbol was normalized to nothing");
            norm[symbol] = weight as i16;
            running = end;
        }
    }
}

pub(crate) fn write_description(
    out: &mut Vec<u8>,
    norm: &[i16],
    max_symbol: usize,
    table_log: u32,
) {
    let table_size: i32 = 1 << table_log;
    let mut stream: u32 = table_log - TABLE_LOG_MIN;
    let mut count_bits: i32 = 4;
    let mut remaining = table_size + 1;
    let mut threshold = table_size;
    let mut bits = table_log as i32 + 1;
    let mut symbol = 0;
    let mut previous_zero = false;
    let alphabet = max_symbol + 1;
    let flush = |out: &mut Vec<u8>, stream: &mut u32, count_bits: &mut i32| {
        if *count_bits > 16 {
            out.extend_from_slice(&(*stream as u16).to_le_bytes());
            *stream >>= 16;
            *count_bits -= 16;
        }
    };
    while symbol < alphabet && remaining > 1 {
        if previous_zero {
            let mut start = symbol;
            while symbol < alphabet && norm[symbol] == 0 {
                symbol += 1;
            }
            assert!(
                symbol < alphabet,
                "probability remains past the last symbol"
            );
            while symbol >= start + 24 {
                start += 24;
                stream += 0xFFFF << count_bits;
                out.extend_from_slice(&(stream as u16).to_le_bytes());
                stream >>= 16;
            }
            while symbol >= start + 3 {
                start += 3;
                stream += 3 << count_bits;
                count_bits += 2;
            }
            stream += ((symbol - start) as u32) << count_bits;
            count_bits += 2;
            flush(out, &mut stream, &mut count_bits);
        }
        let mut count = i32::from(norm[symbol]);
        symbol += 1;
        let max = (2 * threshold - 1) - remaining;
        remaining -= count.abs();
        count += 1;
        if count >= threshold {
            count += max;
        }
        stream += (count as u32) << count_bits;
        count_bits += bits;
        count_bits -= i32::from(count < max);
        previous_zero = count == 1;
        assert!(remaining >= 1, "a distribution overran its table");
        while remaining < threshold {
            bits -= 1;
            threshold >>= 1;
        }
        flush(out, &mut stream, &mut count_bits);
    }
    assert_eq!(remaining, 1, "a distribution fills its table exactly");
    let tail = ((count_bits + 7) / 8) as usize;
    out.extend_from_slice(&stream.to_le_bytes()[..tail]);
}

#[derive(Clone, Copy, Default)]
struct Transform {
    delta_bits: u32,
    delta_state: i32,
}

pub(crate) struct CTable {
    table_log: u32,
    states: Vec<u16>,
    transforms: [Transform; SYMBOLS_MAX],
}

impl Default for CTable {
    fn default() -> Self {
        Self {
            table_log: 0,
            states: Vec::with_capacity(1 << TABLE_LOG_MAX),
            transforms: [Transform::default(); SYMBOLS_MAX],
        }
    }
}

impl CTable {
    pub(crate) fn rle(&mut self, symbol: u8) {
        self.table_log = 0;
        self.states.clear();
        self.states.push(0);
        self.transforms[usize::from(symbol)] = Transform::default();
    }

    pub(crate) fn build(
        &mut self,
        norm: &[i16],
        max_symbol: usize,
        table_log: u32,
        spread: &mut Vec<u8>,
    ) {
        assert!(table_log <= TABLE_LOG_MAX);
        let table_size = 1_usize << table_log;
        let mask = table_size - 1;
        let step = (table_size >> 1) + (table_size >> 3) + 3;
        let mut cumulative = [0_u32; SYMBOLS_MAX + 1];
        spread.clear();
        spread.resize(table_size, 0);
        let mut high = table_size - 1;
        for symbol in 0..=max_symbol {
            if norm[symbol] == -1 {
                cumulative[symbol + 1] = cumulative[symbol] + 1;
                spread[high] = symbol as u8;
                high = high.wrapping_sub(1);
            } else {
                cumulative[symbol + 1] = cumulative[symbol] + norm[symbol].max(0) as u32;
            }
        }
        assert_eq!(cumulative[max_symbol + 1] as usize, table_size);
        let mut position = 0;
        for symbol in 0..=max_symbol {
            for _ in 0..norm[symbol].max(0) {
                spread[position] = symbol as u8;
                position = (position + step) & mask;
                while position > high {
                    position = (position + step) & mask;
                }
            }
        }
        assert_eq!(position, 0, "the spread visits every cell once");
        self.table_log = table_log;
        self.states.clear();
        self.states.resize(table_size, 0);
        for (cell, &symbol) in spread.iter().enumerate() {
            let next = &mut cumulative[usize::from(symbol)];
            self.states[*next as usize] = (table_size + cell) as u16;
            *next += 1;
        }
        let mut total: i32 = 0;
        for symbol in 0..=max_symbol {
            let transform = &mut self.transforms[symbol];
            match norm[symbol] {
                0 => {
                    transform.delta_bits = ((table_log + 1) << 16) - table_size as u32;
                }
                -1 | 1 => {
                    transform.delta_bits = (table_log << 16) - table_size as u32;
                    transform.delta_state = total - 1;
                    total += 1;
                }
                n => {
                    let n = n as u32;
                    let bits_out = table_log - highbit(n - 1);
                    let state_min = n << bits_out;
                    transform.delta_bits = (bits_out << 16) - state_min;
                    transform.delta_state = total - n as i32;
                    total += n as i32;
                }
            }
        }
    }
}

pub(crate) struct CState(u32);

impl CState {
    #[inline]
    pub(crate) fn new(table: &CTable, symbol: u8) -> Self {
        let transform = table.transforms[usize::from(symbol)];
        let bits_out = (transform.delta_bits + (1 << 15)) >> 16;
        let value = (bits_out << 16).wrapping_sub(transform.delta_bits);
        let index = ((value >> bits_out) as i32 + transform.delta_state) as usize;
        Self(u32::from(table.states[index]))
    }

    #[inline]
    pub(crate) fn encode(&mut self, bits: &mut BitWriter<'_>, table: &CTable, symbol: u8) {
        let transform = table.transforms[usize::from(symbol)];
        let bits_out = (self.0 + transform.delta_bits) >> 16;
        bits.add(u64::from(self.0), bits_out);
        let index = ((self.0 >> bits_out) as i32 + transform.delta_state) as usize;
        self.0 = u32::from(table.states[index]);
    }

    pub(crate) fn flush(&self, bits: &mut BitWriter<'_>, table: &CTable) {
        bits.add(u64::from(self.0), table.table_log);
    }
}

pub(crate) fn cost_bits(
    counts: &[u32],
    max_symbol: usize,
    norm: &[i16],
    table_log: u32,
) -> Option<f64> {
    let mut cost = 0.0;
    for symbol in 0..=max_symbol {
        let count = counts[symbol];
        if count == 0 {
            continue;
        }
        let probability = match norm.get(symbol).copied() {
            None | Some(0) => return None,
            Some(n) => f64::from(n.max(1)),
        };
        cost += f64::from(count) * (f64::from(table_log) - probability.log2());
    }
    Some(cost)
}
