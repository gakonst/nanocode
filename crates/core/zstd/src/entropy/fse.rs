use crate::{
    entropy::{
        bitstream::{BitDStream, BitDStreamStatus, CONTAINER_BITS},
        mem::highbit32,
    },
    error::{Error, Result},
};
pub(crate) const TABLELOG_MAX: usize = 9;
pub(crate) const SYMBOLVALUE_MAX: usize = 255;
const MIN_TABLELOG: usize = 5;
const TABLESIZE_MAX: usize = 1 << TABLELOG_MAX;
#[derive(Clone, Copy, Default)]
pub(crate) struct DecodeEntry {
    pub(crate) new_state: u16,
    pub(crate) symbol: u8,
    pub(crate) nb_bits: u8,
}
#[derive(Clone)]
pub(crate) struct DTable {
    table_log: u16,
    fast_mode: bool,
    entries: [DecodeEntry; TABLESIZE_MAX],
}
impl DTable {
    pub(crate) const fn table_log(&self) -> u16 {
        self.table_log
    }
    pub(crate) fn copy_active_from(&mut self, src: &Self, table_size: usize) {
        self.table_log = src.table_log;
        self.fast_mode = src.fast_mode;
        self.entries[..table_size].copy_from_slice(&src.entries[..table_size]);
    }
}
impl Default for DTable {
    fn default() -> Self {
        Self {
            table_log: 0,
            fast_mode: false,
            entries: [DecodeEntry::default(); TABLESIZE_MAX],
        }
    }
}
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub(crate) struct SequenceDecodeEntry {
    pub(crate) new_state: u16,
    pub(crate) nb_bits: u8,
    pub(crate) nb_additional_bits: u8,
    pub(crate) baseline: u32,
}
#[derive(Clone)]
#[repr(align(64))]
pub(crate) struct SequenceDTable {
    pub(crate) table_log: u16,
    pub(crate) entries: [SequenceDecodeEntry; TABLESIZE_MAX],
}
impl Default for SequenceDTable {
    fn default() -> Self {
        Self {
            table_log: 0,
            entries: [SequenceDecodeEntry::default(); TABLESIZE_MAX],
        }
    }
}
impl SequenceDTable {
    #[inline(always)]
    pub(crate) fn get_entry_raw(&self, state: usize) -> Result<u64> {
        let entry = self
            .entries
            .get(state)
            .ok_or(Error::Corruption("invalid FSE sequence state"))?;
        Ok(u64::from(entry.new_state)
            | (u64::from(entry.nb_bits) << 16)
            | (u64::from(entry.nb_additional_bits) << 24)
            | (u64::from(entry.baseline) << 32))
    }
}
#[inline(always)]
pub(crate) const fn raw_entry_nb_additional_bits(raw: u64) -> u32 {
    ((raw >> 24) & 0xFF) as u32
}
#[inline(always)]
pub(crate) const fn raw_entry_nb_bits(raw: u64) -> u32 {
    ((raw >> 16) & 0xFF) as u32
}
#[inline(always)]
pub(crate) const fn raw_entry_new_state(raw: u64) -> usize {
    (raw & 0xFFFF) as usize
}
#[inline(always)]
pub(crate) const fn raw_entry_baseline(raw: u64) -> u32 {
    (raw >> 32) as u32
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DState {
    pub(crate) state: usize,
}
#[inline(always)]
const fn table_step(table_size: usize) -> usize {
    (table_size >> 1) + (table_size >> 3) + 3
}
pub(crate) fn read_ncount(
    normalized_counter: &mut [i16],
    max_symbol_value: &mut u32,
    table_log: &mut u32,
    src: &[u8],
    max_table_log: usize,
) -> Result<usize> {
    let max_symbol_index =
        usize::try_from(*max_symbol_value).map_err(|_| Error::MaxSymbolValueTooLarge)?;
    if max_symbol_index > SYMBOLVALUE_MAX || max_symbol_index >= normalized_counter.len() {
        return Err(Error::MaxSymbolValueTooLarge);
    }
    if max_table_log > TABLELOG_MAX {
        return Err(Error::TableLogTooLarge);
    }
    if src.len() < 4 {
        let mut buffer = [0u8; 4];
        buffer[..src.len()].copy_from_slice(src);
        let count_size = read_ncount(
            normalized_counter,
            max_symbol_value,
            table_log,
            &buffer,
            max_table_log,
        )?;
        if count_size > src.len() {
            return Err(Error::Corruption("truncated FSE ncount"));
        }
        return Ok(count_size);
    }
    normalized_counter[..=max_symbol_index].fill(0);
    let mut bit_stream = u32::from_le_bytes(src[..4].try_into().unwrap());
    let mut nb_bits = ((bit_stream & 0xF) as i32) + MIN_TABLELOG as i32;
    if nb_bits > max_table_log as i32 {
        return Err(Error::TableLogTooLarge);
    }
    bit_stream >>= 4;
    let mut bit_count = 4i32;
    *table_log = nb_bits as u32;
    let mut remaining = (1i32 << nb_bits) + 1;
    let mut threshold = 1i32 << nb_bits;
    nb_bits += 1;
    let mut charnum = 0usize;
    let mut previous0 = false;
    let mut ip = 0usize;
    while remaining > 1 && charnum <= max_symbol_index {
        if previous0 {
            let mut n0 = charnum;
            while (bit_stream & 0xFFFF) == 0xFFFF {
                n0 += 24;
                if ip + 6 <= src.len() {
                    ip += 2;
                    bit_stream =
                        u32::from_le_bytes(src[ip..ip + 4].try_into().unwrap()) >> bit_count;
                } else {
                    bit_stream >>= 16;
                    bit_count += 16;
                }
            }
            while (bit_stream & 3) == 3 {
                n0 += 3;
                bit_stream >>= 2;
                bit_count += 2;
            }
            n0 += (bit_stream & 3) as usize;
            bit_count += 2;
            if n0 > max_symbol_index {
                return Err(Error::MaxSymbolValueTooSmall);
            }
            while charnum < n0 {
                normalized_counter[charnum] = 0;
                charnum += 1;
            }
            if can_reload_u32(src.len(), ip, bit_count) {
                ip += (bit_count >> 3) as usize;
                bit_count &= 7;
                bit_stream = u32::from_le_bytes(src[ip..ip + 4].try_into().unwrap()) >> bit_count;
            } else {
                bit_stream >>= 2;
            }
        }
        let max = (2 * threshold - 1) - remaining;
        let mut count;
        if (bit_stream & (threshold as u32 - 1)) < max as u32 {
            count = (bit_stream & (threshold as u32 - 1)) as i32;
            bit_count += nb_bits - 1;
        } else {
            count = (bit_stream & ((2 * threshold - 1) as u32)) as i32;
            if count >= threshold {
                count -= max;
            }
            bit_count += nb_bits;
        }
        count -= 1;
        remaining -= if count < 0 { -count } else { count };
        normalized_counter[charnum] = count as i16;
        charnum += 1;
        previous0 = count == 0;
        while remaining < threshold {
            nb_bits -= 1;
            threshold >>= 1;
        }
        if can_reload_u32(src.len(), ip, bit_count) {
            ip += (bit_count >> 3) as usize;
            bit_count &= 7;
        } else {
            bit_count -= (8 * (src.len().saturating_sub(4).saturating_sub(ip))) as i32;
            ip = src.len() - 4;
        }
        bit_stream = u32::from_le_bytes(src[ip..ip + 4].try_into().unwrap()) >> (bit_count & 31);
    }
    if remaining != 1 || bit_count > 32 {
        return Err(Error::Corruption("invalid FSE ncount"));
    }
    *max_symbol_value = charnum as u32 - 1;
    Ok(ip + ((bit_count + 7) >> 3) as usize)
}
#[inline(always)]
fn can_reload_u32(src_len: usize, ip: usize, bit_count: i32) -> bool {
    let advance = (bit_count >> 3) as usize;
    ip.checked_add(advance)
        .and_then(|next| next.checked_add(4))
        .is_some_and(|end| end <= src_len)
}
pub(crate) fn build_dtable(
    dt: &mut DTable,
    normalized_counter: &[i16],
    max_symbol_value: u32,
    table_log: u32,
) -> Result<()> {
    let max_symbol_index =
        usize::try_from(max_symbol_value).map_err(|_| Error::MaxSymbolValueTooLarge)?;
    if max_symbol_index > SYMBOLVALUE_MAX || max_symbol_index >= normalized_counter.len() {
        return Err(Error::MaxSymbolValueTooLarge);
    }
    if table_log == 0 {
        return Err(Error::Corruption("FSE table log must be non-zero"));
    }
    if table_log as usize > TABLELOG_MAX {
        return Err(Error::TableLogTooLarge);
    }
    let table_size = 1usize << table_log;
    let table_mask = table_size - 1;
    let mut symbol_next = [0u16; SYMBOLVALUE_MAX + 1];
    let mut high_threshold = table_size - 1;
    dt.table_log = table_log as u16;
    dt.fast_mode = true;
    let large_limit = 1i16 << (table_log - 1);
    for symbol in 0..=max_symbol_index {
        if normalized_counter[symbol] == -1 {
            dt.entries[high_threshold].symbol = symbol as u8;
            symbol_next[symbol] = 1;
            high_threshold = high_threshold.saturating_sub(1);
        } else {
            if normalized_counter[symbol] >= large_limit {
                dt.fast_mode = false;
            }
            symbol_next[symbol] = normalized_counter[symbol].max(0) as u16;
        }
    }
    let step = table_step(table_size);
    let mut position = 0usize;
    for symbol in 0..=max_symbol_index {
        for _ in 0..normalized_counter[symbol].max(0) {
            dt.entries[position].symbol = symbol as u8;
            position = (position + step) & table_mask;
            while position > high_threshold {
                position = (position + step) & table_mask;
            }
        }
    }
    if position != 0 {
        return Err(Error::Generic);
    }
    for index in 0..table_size {
        let symbol = dt.entries[index].symbol as usize;
        let next_state = symbol_next[symbol] as u32;
        symbol_next[symbol] = symbol_next[symbol].wrapping_add(1);
        dt.entries[index].nb_bits = (table_log - highbit32(next_state)) as u8;
        dt.entries[index].new_state =
            ((next_state << dt.entries[index].nb_bits) - table_size as u32) as u16;
    }
    Ok(())
}
pub(crate) const fn build_rle_dtable(dt: &mut DTable, symbol: u8) {
    dt.table_log = 0;
    dt.fast_mode = false;
    dt.entries[0] = DecodeEntry {
        new_state: 0,
        symbol,
        nb_bits: 0,
    };
}
pub(crate) const fn build_rle_sequence_dtable(
    out: &mut SequenceDTable,
    symbol: u8,
    baselines: &[u32],
    extra_bits: &[u8],
) {
    let sym = symbol as usize;
    out.table_log = 0;
    out.entries[0] = SequenceDecodeEntry {
        new_state: 0,
        nb_bits: 0,
        nb_additional_bits: if sym < extra_bits.len() {
            extra_bits[sym]
        } else {
            0
        },
        baseline: if sym < baselines.len() {
            baselines[sym]
        } else {
            0
        },
    };
}
pub(crate) const fn build_rle_offset_sequence_dtable(out: &mut SequenceDTable, symbol: u8) {
    out.table_log = 0;
    out.entries[0] = SequenceDecodeEntry {
        new_state: 0,
        nb_bits: 0,
        nb_additional_bits: symbol,
        baseline: 0,
    };
}
pub(crate) fn build_sequence_dtable_from(
    dt: &DTable,
    baselines: &[u32],
    extra_bits: &[u8],
    out: &mut SequenceDTable,
) {
    let table_size = if dt.table_log == 0 {
        1
    } else {
        1usize << dt.table_log
    };
    out.table_log = dt.table_log;
    for i in 0..table_size {
        let e = &dt.entries[i];
        let sym = e.symbol as usize;
        {
            out.entries[i] = SequenceDecodeEntry {
                new_state: e.new_state,
                nb_bits: e.nb_bits,
                nb_additional_bits: if sym < extra_bits.len() {
                    extra_bits[sym]
                } else {
                    0
                },
                baseline: if sym < baselines.len() {
                    baselines[sym]
                } else {
                    0
                },
            };
        }
    }
}
pub(crate) fn build_offset_sequence_dtable_from(dt: &DTable, out: &mut SequenceDTable) {
    let table_size = if dt.table_log == 0 {
        1
    } else {
        1usize << dt.table_log
    };
    out.table_log = dt.table_log;
    for i in 0..table_size {
        let e = &dt.entries[i];
        {
            out.entries[i] = SequenceDecodeEntry {
                new_state: e.new_state,
                nb_bits: e.nb_bits,
                nb_additional_bits: e.symbol,
                baseline: 0,
            };
        }
    }
}
pub(crate) fn build_sequence_dtable_direct(
    out: &mut SequenceDTable,
    normalized_counter: &[i16],
    max_symbol_value: u32,
    table_log: u32,
    baselines: &[u32],
    extra_bits: &[u8],
) -> Result<()> {
    let max_symbol_index =
        usize::try_from(max_symbol_value).map_err(|_| Error::MaxSymbolValueTooLarge)?;
    if max_symbol_index > SYMBOLVALUE_MAX || max_symbol_index >= normalized_counter.len() {
        return Err(Error::MaxSymbolValueTooLarge);
    }
    if table_log == 0 {
        return Err(Error::Corruption("FSE table log must be non-zero"));
    }
    if table_log as usize > TABLELOG_MAX {
        return Err(Error::TableLogTooLarge);
    }
    let table_size = 1usize << table_log;
    let table_mask = table_size - 1;
    let mut symbol_next = [0u16; SYMBOLVALUE_MAX + 1];
    let mut high_threshold = table_size - 1;
    let mut table_symbol = [0u8; TABLESIZE_MAX];
    for symbol in 0..=max_symbol_index {
        if normalized_counter[symbol] == -1 {
            table_symbol[high_threshold] = symbol as u8;
            symbol_next[symbol] = 1;
            high_threshold = high_threshold.saturating_sub(1);
        } else {
            symbol_next[symbol] = normalized_counter[symbol].max(0) as u16;
        }
    }
    let step = table_step(table_size);
    let mut position = 0usize;
    for symbol in 0..=max_symbol_index {
        for _ in 0..normalized_counter[symbol].max(0) {
            table_symbol[position] = symbol as u8;
            position = (position + step) & table_mask;
            while position > high_threshold {
                position = (position + step) & table_mask;
            }
        }
    }
    if position != 0 {
        return Err(Error::Generic);
    }
    out.table_log = table_log as u16;
    for index in 0..table_size {
        let sym = table_symbol[index] as usize;
        let next_state = symbol_next[sym] as u32;
        symbol_next[sym] = symbol_next[sym].wrapping_add(1);
        let nb_bits = (table_log - highbit32(next_state)) as u8;
        let new_state = ((next_state << nb_bits) - table_size as u32) as u16;
        {
            out.entries[index] = SequenceDecodeEntry {
                new_state,
                nb_bits,
                nb_additional_bits: if sym < extra_bits.len() {
                    extra_bits[sym]
                } else {
                    0
                },
                baseline: if sym < baselines.len() {
                    baselines[sym]
                } else {
                    0
                },
            };
        }
    }
    Ok(())
}
pub(crate) fn build_offset_sequence_dtable_direct(
    out: &mut SequenceDTable,
    normalized_counter: &[i16],
    max_symbol_value: u32,
    table_log: u32,
) -> Result<()> {
    let max_symbol_index =
        usize::try_from(max_symbol_value).map_err(|_| Error::MaxSymbolValueTooLarge)?;
    if max_symbol_index > SYMBOLVALUE_MAX || max_symbol_index >= normalized_counter.len() {
        return Err(Error::MaxSymbolValueTooLarge);
    }
    if table_log == 0 {
        return Err(Error::Corruption("FSE table log must be non-zero"));
    }
    if table_log as usize > TABLELOG_MAX {
        return Err(Error::TableLogTooLarge);
    }
    let table_size = 1usize << table_log;
    let table_mask = table_size - 1;
    let mut symbol_next = [0u16; SYMBOLVALUE_MAX + 1];
    let mut high_threshold = table_size - 1;
    let mut table_symbol = [0u8; TABLESIZE_MAX];
    for symbol in 0..=max_symbol_index {
        if normalized_counter[symbol] == -1 {
            table_symbol[high_threshold] = symbol as u8;
            symbol_next[symbol] = 1;
            high_threshold = high_threshold.saturating_sub(1);
        } else {
            symbol_next[symbol] = normalized_counter[symbol].max(0) as u16;
        }
    }
    let step = table_step(table_size);
    let mut position = 0usize;
    for symbol in 0..=max_symbol_index {
        for _ in 0..normalized_counter[symbol].max(0) {
            table_symbol[position] = symbol as u8;
            position = (position + step) & table_mask;
            while position > high_threshold {
                position = (position + step) & table_mask;
            }
        }
    }
    if position != 0 {
        return Err(Error::Generic);
    }
    out.table_log = table_log as u16;
    for index in 0..table_size {
        let sym = table_symbol[index];
        let next_state = symbol_next[sym as usize] as u32;
        symbol_next[sym as usize] = symbol_next[sym as usize].wrapping_add(1);
        let nb_bits = (table_log - highbit32(next_state)) as u8;
        let new_state = ((next_state << nb_bits) - table_size as u32) as u16;
        {
            out.entries[index] = SequenceDecodeEntry {
                new_state,
                nb_bits,
                nb_additional_bits: sym,
                baseline: 0,
            };
        }
    }
    Ok(())
}
#[inline(always)]
pub(crate) fn init_dstate_seq(bit_d: &mut BitDStream, dt: &SequenceDTable) -> DState {
    let state = bit_d.read_bits(dt.table_log as u32) as usize;
    let _ = bit_d.reload();
    DState { state }
}
#[inline(always)]
pub(crate) fn init_dstate(bit_d: &mut BitDStream, dt: &DTable) -> DState {
    let state = bit_d.read_bits(dt.table_log as u32) as usize;
    let _ = bit_d.reload();
    DState { state }
}
pub(crate) fn decode_symbol(state: &mut DState, bit_d: &mut BitDStream, dt: &DTable) -> u8 {
    let info = dt.entries[state.state];
    let low_bits = if bit_d.can_read_fast(info.nb_bits as u32) {
        bit_d.read_bits_fast(info.nb_bits as u32)
    } else {
        bit_d.read_bits(info.nb_bits as u32)
    } as usize;
    state.state = info.new_state as usize + low_bits;
    info.symbol
}
pub(crate) fn decode_symbol_fast(state: &mut DState, bit_d: &mut BitDStream, dt: &DTable) -> u8 {
    let info = dt.entries[state.state];
    let low_bits = bit_d.read_bits_fast(info.nb_bits as u32) as usize;
    state.state = info.new_state as usize + low_bits;
    info.symbol
}
fn decompress_using_dtable_generic(
    dst: &mut [u8],
    src: &[u8],
    dt: &DTable,
    fast: bool,
) -> Result<usize> {
    if dt.table_log == 0 {
        return Err(Error::Corruption(
            "FSE interleaved decode does not support RLE tables",
        ));
    }
    let mut bit_d = BitDStream::new(src)?;
    let mut state1 = init_dstate(&mut bit_d, dt);
    let mut state2 = init_dstate(&mut bit_d, dt);
    if bit_d.reload() == BitDStreamStatus::Overflow {
        return Err(Error::Corruption(
            "FSE bitstream is too short to hold both initial states",
        ));
    }
    let mut op = 0usize;
    let olimit = dst.len().saturating_sub(3);
    macro_rules! get_symbol {
        ($state:expr) => {{
            if fast {
                decode_symbol_fast($state, &mut bit_d, dt)
            } else {
                decode_symbol($state, &mut bit_d, dt)
            }
        }};
    }
    while bit_d.reload() == BitDStreamStatus::Unfinished && op < olimit {
        dst[op] = get_symbol!(&mut state1);
        op += 1;
        if (TABLELOG_MAX * 2 + 7) as u32 > CONTAINER_BITS {
            let _ = bit_d.reload();
        }
        dst[op] = get_symbol!(&mut state2);
        op += 1;
        if (TABLELOG_MAX * 4 + 7) as u32 > CONTAINER_BITS
            && bit_d.reload() != BitDStreamStatus::Unfinished
        {
            break;
        }
        dst[op] = get_symbol!(&mut state1);
        op += 1;
        if (TABLELOG_MAX * 2 + 7) as u32 > CONTAINER_BITS {
            let _ = bit_d.reload();
        }
        dst[op] = get_symbol!(&mut state2);
        op += 1;
    }
    loop {
        if op > dst.len().saturating_sub(2) {
            return Err(Error::DstSizeTooSmall);
        }
        dst[op] = get_symbol!(&mut state1);
        op += 1;
        if bit_d.reload() == BitDStreamStatus::Overflow {
            dst[op] = get_symbol!(&mut state2);
            op += 1;
            break;
        }
        if op > dst.len().saturating_sub(2) {
            return Err(Error::DstSizeTooSmall);
        }
        dst[op] = get_symbol!(&mut state2);
        op += 1;
        if bit_d.reload() == BitDStreamStatus::Overflow {
            dst[op] = get_symbol!(&mut state1);
            op += 1;
            break;
        }
    }
    Ok(op)
}
pub(crate) fn decompress_interleaved2(dst: &mut [u8], src: &[u8], dt: &DTable) -> Result<usize> {
    if dt.fast_mode {
        decompress_using_dtable_generic(dst, src, dt, true)
    } else {
        decompress_using_dtable_generic(dst, src, dt, false)
    }
}
pub(crate) fn decompress(
    dst: &mut [u8],
    src: &[u8],
    max_symbol_value: u32,
    max_table_log: usize,
) -> Result<usize> {
    let mut normalized = [0i16; SYMBOLVALUE_MAX + 1];
    let mut max_symbol_value = max_symbol_value;
    let mut table_log = 0u32;
    let ncount_len = read_ncount(
        &mut normalized,
        &mut max_symbol_value,
        &mut table_log,
        src,
        max_table_log,
    )?;
    let mut dtable = DTable::default();
    build_dtable(&mut dtable, &normalized, max_symbol_value, table_log)?;
    decompress_interleaved2(dst, &src[ncount_len..], &dtable)
}
