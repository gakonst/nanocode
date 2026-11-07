use crate::{
    entropy::{
        bitstream::{BitDStream, BitDStreamStatus, CONTAINER_BITS, CONTAINER_BYTES},
        fse,
        mem::highbit32,
    },
    error::{Error, Result},
};
pub(crate) const TABLELOG_MAX: usize = 12;
const DECODER_FAST_TABLELOG: usize = 11;
const SYMBOLVALUE_MAX: usize = 255;
const MAX_FSE_TABLELOG_FOR_HUFF_HEADER: usize = 6;
#[derive(Clone, Copy, Default)]
struct DEltX1 {
    byte: u8,
    nb_bits: u8,
}
const DTABLE_X1_SIZE: usize = 1 << TABLELOG_MAX;
#[derive(Clone, Copy)]
pub(crate) struct DTableX1 {
    table_log: u8,
    entries: [DEltX1; DTABLE_X1_SIZE],
}
impl Default for DTableX1 {
    fn default() -> Self {
        Self {
            table_log: 0,
            entries: [DEltX1::default(); DTABLE_X1_SIZE],
        }
    }
}
#[inline(always)]
const fn read_segment_length(src: &[u8], offset: usize) -> usize {
    u16::from_le_bytes([src[offset], src[offset + 1]]) as usize
}
fn read_stats(
    huff_weight: &mut [u8; SYMBOLVALUE_MAX + 1],
    rank_stats: &mut [u32; TABLELOG_MAX + 1],
    nb_symbols: &mut u32,
    table_log: &mut u32,
    src: &[u8],
) -> Result<usize> {
    if src.is_empty() {
        return Err(Error::SrcSizeWrong);
    }
    let mut i_size = src[0] as usize;
    let o_size;
    if i_size >= 128 {
        o_size = i_size - 127;
        i_size = o_size.div_ceil(2);
        if i_size + 1 > src.len() || o_size >= huff_weight.len() {
            return Err(Error::SrcSizeWrong);
        }
        for index in (0..o_size).step_by(2) {
            huff_weight[index] = src[1 + (index / 2)] >> 4;
            huff_weight[index + 1] = src[1 + (index / 2)] & 15;
        }
    } else {
        if i_size + 1 > src.len() {
            return Err(Error::SrcSizeWrong);
        }
        let hw_len = huff_weight.len() - 1;
        o_size = fse::decompress(
            &mut huff_weight[..hw_len],
            &src[1..=i_size],
            TABLELOG_MAX as u32,
            MAX_FSE_TABLELOG_FOR_HUFF_HEADER,
        )?;
    }
    rank_stats.fill(0);
    let mut weight_total = 0u32;
    for &weight in &huff_weight[..o_size] {
        if weight as usize >= TABLELOG_MAX {
            return Err(Error::Corruption("invalid Huff0 weight"));
        }
        rank_stats[weight as usize] += 1;
        weight_total += (1u32 << weight) >> 1;
    }
    if weight_total == 0 {
        return Err(Error::Corruption("empty Huff0 weight table"));
    }
    let log = highbit32(weight_total) + 1;
    if log as usize > TABLELOG_MAX {
        return Err(Error::Corruption("Huff0 table log too large"));
    }
    *table_log = log;
    let total = 1u32 << log;
    let rest = total - weight_total;
    let verif = 1u32 << highbit32(rest);
    let last_weight = highbit32(rest) + 1;
    if verif != rest {
        return Err(Error::Corruption("non power-of-two Huff0 weight remainder"));
    }
    huff_weight[o_size] = last_weight as u8;
    rank_stats[last_weight as usize] += 1;
    if rank_stats[1] < 2 || (rank_stats[1] & 1) != 0 {
        return Err(Error::Corruption("invalid Huff0 rank-1 weight count"));
    }
    *nb_symbols = o_size as u32 + 1;
    Ok(i_size + 1)
}
pub(crate) fn read_dtable_x1(src: &[u8], dtable: &mut DTableX1) -> Result<usize> {
    let mut rank_val = [0u32; TABLELOG_MAX + 1];
    let mut huff_weight = [0u8; SYMBOLVALUE_MAX + 1];
    let mut table_log = 0u32;
    let mut nb_symbols = 0u32;
    let i_size = read_stats(
        &mut huff_weight,
        &mut rank_val,
        &mut nb_symbols,
        &mut table_log,
        src,
    )?;
    if (1usize << table_log) > dtable.entries.len() {
        return Err(Error::Corruption(
            "Huff0 table log exceeds decode table capacity",
        ));
    }
    dtable.table_log = table_log as u8;
    let mut next_rank_start = 0u32;
    for index in 1..=table_log as usize {
        let current = next_rank_start;
        next_rank_start += rank_val[index] << (index - 1);
        rank_val[index] = current;
    }
    for index in 0..nb_symbols as usize {
        let weight = huff_weight[index] as usize;
        let length = (1usize << weight) >> 1;
        let start = rank_val[weight] as usize;
        let end = start + length;
        let entry = DEltX1 {
            byte: index as u8,
            nb_bits: (table_log + 1 - weight as u32) as u8,
        };
        rank_val[weight] = end as u32;
        for slot in &mut dtable.entries[start..end] {
            *slot = entry;
        }
    }
    Ok(i_size)
}
#[inline(always)]
fn decode_symbol_x1(bit_d: &mut BitDStream, table: &[DEltX1], table_log: u32) -> u8 {
    let value = bit_d.look_bits_fast(table_log) as usize;
    let entry = { table[value] };
    bit_d.skip_bits(entry.nb_bits as u32);
    entry.byte
}
fn decode_stream_x1(
    dst: &mut [u8],
    mut pos: usize,
    bit_d: &mut BitDStream,
    end: usize,
    table: &[DEltX1],
    table_log: u32,
) -> usize {
    while bit_d.reload() == BitDStreamStatus::Unfinished && pos < end.saturating_sub(3) {
        dst[pos] = decode_symbol_x1(bit_d, table, table_log);
        pos += 1;
        dst[pos] = decode_symbol_x1(bit_d, table, table_log);
        pos += 1;
        dst[pos] = decode_symbol_x1(bit_d, table, table_log);
        pos += 1;
        dst[pos] = decode_symbol_x1(bit_d, table, table_log);
        pos += 1;
    }
    while pos < end {
        dst[pos] = decode_symbol_x1(bit_d, table, table_log);
        pos += 1;
    }
    pos
}
fn decompress_1x1_using_dtable(dst: &mut [u8], src: &[u8], dtable: &DTableX1) -> Result<usize> {
    let mut bit_d = BitDStream::new(src)?;
    let table_log = dtable.table_log as u32;
    let written = decode_stream_x1(dst, 0, &mut bit_d, dst.len(), &dtable.entries, table_log);
    if written != dst.len() || !bit_d.end_of_stream() {
        return Err(Error::Corruption("Huff0 stream did not terminate cleanly"));
    }
    Ok(written)
}
fn decompress_4x1_using_dtable(dst: &mut [u8], src: &[u8], dtable: &DTableX1) -> Result<usize> {
    if src.len() < 10 {
        return Err(Error::Corruption("truncated Huff0 4X stream"));
    }
    if dst.len() < 6 {
        return Err(Error::Corruption("Huff0 4X output too small to split"));
    }
    let length1 = read_segment_length(src, 0);
    let length2 = read_segment_length(src, 2);
    let length3 = read_segment_length(src, 4);
    let length4 = src.len().wrapping_sub(length1 + length2 + length3 + 6);
    if length4 > src.len() {
        return Err(Error::Corruption("invalid Huff0 4X segment lengths"));
    }
    let istart1 = 6;
    let istart2 = istart1 + length1;
    let istart3 = istart2 + length2;
    let istart4 = istart3 + length3;
    let segment_size = dst.len().div_ceil(4);
    let mut bit_d1 = BitDStream::new(&src[istart1..istart2])?;
    let mut bit_d2 = BitDStream::new(&src[istart2..istart3])?;
    let mut bit_d3 = BitDStream::new(&src[istart3..istart4])?;
    let mut bit_d4 = BitDStream::new(&src[istart4..])?;
    let op_start2 = segment_size;
    let op_start3 = op_start2 + segment_size;
    let op_start4 = op_start3 + segment_size;
    if op_start4 > dst.len() {
        return Err(Error::Corruption("Huff0 4X segment starts past the output"));
    }
    let olimit = dst.len().saturating_sub(3);
    let mut op1 = 0usize;
    let mut op2 = op_start2;
    let mut op3 = op_start3;
    let mut op4 = op_start4;
    let mut end_signal = true;
    let table_log = dtable.table_log as u32;
    while end_signal && op4 < olimit {
        dst[op1] = decode_symbol_x1(&mut bit_d1, &dtable.entries, table_log);
        op1 += 1;
        dst[op2] = decode_symbol_x1(&mut bit_d2, &dtable.entries, table_log);
        op2 += 1;
        dst[op3] = decode_symbol_x1(&mut bit_d3, &dtable.entries, table_log);
        op3 += 1;
        dst[op4] = decode_symbol_x1(&mut bit_d4, &dtable.entries, table_log);
        op4 += 1;
        dst[op1] = decode_symbol_x1(&mut bit_d1, &dtable.entries, table_log);
        op1 += 1;
        dst[op2] = decode_symbol_x1(&mut bit_d2, &dtable.entries, table_log);
        op2 += 1;
        dst[op3] = decode_symbol_x1(&mut bit_d3, &dtable.entries, table_log);
        op3 += 1;
        dst[op4] = decode_symbol_x1(&mut bit_d4, &dtable.entries, table_log);
        op4 += 1;
        dst[op1] = decode_symbol_x1(&mut bit_d1, &dtable.entries, table_log);
        op1 += 1;
        dst[op2] = decode_symbol_x1(&mut bit_d2, &dtable.entries, table_log);
        op2 += 1;
        dst[op3] = decode_symbol_x1(&mut bit_d3, &dtable.entries, table_log);
        op3 += 1;
        dst[op4] = decode_symbol_x1(&mut bit_d4, &dtable.entries, table_log);
        op4 += 1;
        dst[op1] = decode_symbol_x1(&mut bit_d1, &dtable.entries, table_log);
        op1 += 1;
        dst[op2] = decode_symbol_x1(&mut bit_d2, &dtable.entries, table_log);
        op2 += 1;
        dst[op3] = decode_symbol_x1(&mut bit_d3, &dtable.entries, table_log);
        op3 += 1;
        dst[op4] = decode_symbol_x1(&mut bit_d4, &dtable.entries, table_log);
        op4 += 1;
        end_signal &= bit_d1.reload_fast() == BitDStreamStatus::Unfinished;
        end_signal &= bit_d2.reload_fast() == BitDStreamStatus::Unfinished;
        end_signal &= bit_d3.reload_fast() == BitDStreamStatus::Unfinished;
        end_signal &= bit_d4.reload_fast() == BitDStreamStatus::Unfinished;
    }
    if op1 > op_start2 || op2 > op_start3 || op3 > op_start4 {
        return Err(Error::Corruption("Huff0 segment overrun"));
    }
    op1 = decode_stream_x1(dst, op1, &mut bit_d1, op_start2, &dtable.entries, table_log);
    op2 = decode_stream_x1(dst, op2, &mut bit_d2, op_start3, &dtable.entries, table_log);
    op3 = decode_stream_x1(dst, op3, &mut bit_d3, op_start4, &dtable.entries, table_log);
    op4 = decode_stream_x1(dst, op4, &mut bit_d4, dst.len(), &dtable.entries, table_log);
    debug_assert_eq!(op1, op_start2);
    debug_assert_eq!(op2, op_start3);
    debug_assert_eq!(op3, op_start4);
    debug_assert_eq!(op4, dst.len());
    if !bit_d1.end_of_stream()
        || !bit_d2.end_of_stream()
        || !bit_d3.end_of_stream()
        || !bit_d4.end_of_stream()
    {
        return Err(Error::Corruption("Huff0 stream did not consume all bits"));
    }
    Ok(dst.len())
}
#[derive(Clone, Copy, Default)]
struct DEltX2 {
    sequence: u16,
    nb_bits: u8,
    length: u8,
}
const DTABLE_X2_SIZE: usize = 1 << TABLELOG_MAX;
#[derive(Clone, Copy)]
pub(crate) struct DTableX2 {
    table_log: u8,
    entries: [DEltX2; DTABLE_X2_SIZE],
}
impl Default for DTableX2 {
    fn default() -> Self {
        Self {
            table_log: 0,
            entries: [DEltX2::default(); DTABLE_X2_SIZE],
        }
    }
}
#[inline]
fn build_delt_x2(symbol: u8, nb_bits: u32, base_symbol: u8, level: u8) -> DEltX2 {
    debug_assert!(level == 1 || level == 2);
    let sequence = if level == 1 {
        symbol as u16
    } else {
        base_symbol as u16 | ((symbol as u16) << 8)
    };
    DEltX2 {
        sequence,
        nb_bits: nb_bits as u8,
        length: level,
    }
}
fn fill_dtable_x2_for_weight(
    slots: &mut [DEltX2],
    symbols: &[u8],
    nb_bits: u32,
    table_log: u32,
    base_symbol: u8,
    level: u8,
) -> Result<()> {
    let shift = table_log.checked_sub(nb_bits).ok_or(Error::Corruption(
        "Huff0 X2 symbol is deeper than its table",
    ))?;
    let length = 1usize << shift;
    let mut at = 0usize;
    for &symbol in symbols {
        let entry = build_delt_x2(symbol, nb_bits, base_symbol, level);
        let end = at
            .checked_add(length)
            .ok_or(Error::Corruption("Huff0 X2 rank overflows the table"))?;
        slots
            .get_mut(at..end)
            .ok_or(Error::Corruption("Huff0 X2 rank runs past the table"))?
            .fill(entry);
        at = end;
    }
    Ok(())
}
#[allow(clippy::too_many_arguments)]
fn fill_dtable_x2_level2(
    sub_table: &mut [DEltX2],
    table_log: u32,
    consumed_bits: u32,
    rank_val: &[u32; TABLELOG_MAX + 1],
    min_weight: usize,
    weight_end: usize,
    sorted_symbols: &[u8],
    rank_start: &[u32; TABLELOG_MAX + 2],
    nb_bits_baseline: u32,
    base_symbol: u8,
) -> Result<()> {
    if min_weight > 1 {
        let skip = *rank_val
            .get(min_weight)
            .ok_or(Error::Corruption("Huff0 X2 weight exceeds the table log"))?
            as usize;
        let entry = build_delt_x2(base_symbol, consumed_bits, 0, 1);
        sub_table
            .get_mut(..skip)
            .ok_or(Error::Corruption("Huff0 X2 single-symbol run overflows"))?
            .fill(entry);
    }
    for weight in min_weight..weight_end {
        let begin = rank_start[weight] as usize;
        let end = rank_start[weight + 1] as usize;
        let nb_bits = nb_bits_baseline - weight as u32;
        let at = rank_val[weight] as usize;
        fill_dtable_x2_for_weight(
            sub_table
                .get_mut(at..)
                .ok_or(Error::Corruption("Huff0 X2 rank starts past the table"))?,
            sorted_symbols
                .get(begin..end)
                .ok_or(Error::Corruption("Huff0 X2 rank bounds are inconsistent"))?,
            nb_bits + consumed_bits,
            table_log,
            base_symbol,
            2,
        )?;
    }
    Ok(())
}
#[allow(clippy::too_many_arguments)]
fn fill_dtable_x2(
    entries: &mut [DEltX2],
    table_log: u32,
    sorted_symbols: &[u8],
    rank_start: &[u32; TABLELOG_MAX + 2],
    rank_val: &[[u32; TABLELOG_MAX + 1]; TABLELOG_MAX],
    max_weight: usize,
    nb_bits_baseline: u32,
) -> Result<()> {
    let scale_log = nb_bits_baseline as i32 - table_log as i32;
    let min_bits = nb_bits_baseline - max_weight as u32;
    for weight in 1..=max_weight {
        let begin = rank_start[weight] as usize;
        let end = rank_start[weight + 1] as usize;
        let nb_bits = nb_bits_baseline - weight as u32;
        let symbols = sorted_symbols
            .get(begin..end)
            .ok_or(Error::Corruption("Huff0 X2 rank bounds are inconsistent"))?;
        let at = rank_val[0][weight] as usize;
        if table_log - nb_bits >= min_bits {
            let length = 1usize << (table_log - nb_bits);
            let min_weight = (nb_bits as i32 + scale_log).max(1) as usize;
            let mut start = at;
            for &symbol in symbols {
                let sub_end = start
                    .checked_add(length)
                    .ok_or(Error::Corruption("Huff0 X2 rank overflows the table"))?;
                fill_dtable_x2_level2(
                    entries
                        .get_mut(start..sub_end)
                        .ok_or(Error::Corruption("Huff0 X2 rank runs past the table"))?,
                    table_log,
                    nb_bits,
                    &rank_val[nb_bits as usize],
                    min_weight,
                    max_weight + 1,
                    sorted_symbols,
                    rank_start,
                    nb_bits_baseline,
                    symbol,
                )?;
                start = sub_end;
            }
        } else {
            fill_dtable_x2_for_weight(
                entries
                    .get_mut(at..)
                    .ok_or(Error::Corruption("Huff0 X2 rank starts past the table"))?,
                symbols,
                nb_bits,
                table_log,
                0,
                1,
            )?;
        }
    }
    Ok(())
}
pub(crate) fn read_dtable_x2(src: &[u8], dtable: &mut DTableX2) -> Result<usize> {
    let mut rank_stats = [0u32; TABLELOG_MAX + 1];
    let mut huff_weight = [0u8; SYMBOLVALUE_MAX + 1];
    let mut table_log = 0u32;
    let mut nb_symbols = 0u32;
    let i_size = read_stats(
        &mut huff_weight,
        &mut rank_stats,
        &mut nb_symbols,
        &mut table_log,
        src,
    )?;
    if table_log as usize > TABLELOG_MAX {
        return Err(Error::Corruption(
            "Huff0 table log exceeds decode table capacity",
        ));
    }
    let target_log = if (table_log as usize) <= DECODER_FAST_TABLELOG {
        DECODER_FAST_TABLELOG as u32
    } else {
        TABLELOG_MAX as u32
    };
    dtable.table_log = target_log as u8;
    let mut max_weight = table_log as usize;
    while rank_stats[max_weight] == 0 {
        max_weight -= 1;
    }
    let mut rank_start = [0u32; TABLELOG_MAX + 2];
    let mut next_start = 0u32;
    for weight in 1..=max_weight {
        rank_start[weight] = next_start;
        next_start += rank_stats[weight];
    }
    rank_start[max_weight + 1] = next_start;
    let mut sorted_symbols = [0u8; SYMBOLVALUE_MAX + 1];
    let mut cursor = rank_start;
    let mut absent_cursor = next_start;
    for symbol in 0..nb_symbols as usize {
        let weight = huff_weight[symbol] as usize;
        let slot = if weight == 0 {
            &mut absent_cursor
        } else {
            &mut cursor[weight]
        };
        sorted_symbols[*slot as usize] = symbol as u8;
        *slot += 1;
    }
    let mut rank_val = [[0u32; TABLELOG_MAX + 1]; TABLELOG_MAX];
    let rescale = target_log as i32 - table_log as i32 - 1;
    let mut next_val = 0u32;
    for weight in 1..=max_weight {
        rank_val[0][weight] = next_val;
        next_val += rank_stats[weight] << (weight as i32 + rescale) as u32;
    }
    if next_val as usize != 1usize << target_log {
        return Err(Error::Corruption("Huff0 X2 ranks do not tile the table"));
    }
    let min_bits = table_log + 1 - max_weight as u32;
    debug_assert!(min_bits <= target_log);
    for consumed in min_bits..=(target_log - min_bits) {
        for weight in 1..=max_weight {
            rank_val[consumed as usize][weight] = rank_val[0][weight] >> consumed;
        }
    }
    fill_dtable_x2(
        &mut dtable.entries[..1usize << target_log],
        target_log,
        &sorted_symbols,
        &rank_start,
        &rank_val,
        max_weight,
        table_log + 1,
    )?;
    Ok(i_size)
}
#[inline(always)]
fn decode_symbol_x2(
    dst: &mut [u8],
    pos: usize,
    bit_d: &mut BitDStream,
    table: &[DEltX2],
    table_log: u32,
) -> usize {
    let value = bit_d.look_bits_fast(table_log) as usize;
    debug_assert!(value < table.len());
    let entry = { table[value] };
    {
        dst[pos..pos + 2].copy_from_slice(&entry.sequence.to_le_bytes());
    }
    bit_d.skip_bits(entry.nb_bits as u32);
    entry.length as usize
}
#[inline(always)]
fn decode_last_symbol_x2(
    dst: &mut [u8],
    pos: usize,
    bit_d: &mut BitDStream,
    table: &[DEltX2],
    table_log: u32,
) -> usize {
    let value = bit_d.look_bits_fast(table_log) as usize;
    debug_assert!(value < table.len());
    let entry = table[value & (table.len() - 1)];
    dst[pos] = entry.sequence as u8;
    if entry.length == 1 {
        bit_d.skip_bits(entry.nb_bits as u32);
    } else if bit_d.bits_consumed < CONTAINER_BITS {
        bit_d.skip_bits(entry.nb_bits as u32);
        if bit_d.bits_consumed > CONTAINER_BITS {
            bit_d.bits_consumed = CONTAINER_BITS;
        }
    }
    1
}
fn decode_stream_x2(
    dst: &mut [u8],
    mut pos: usize,
    bit_d: &mut BitDStream,
    end: usize,
    table: &[DEltX2],
    table_log: u32,
) -> usize {
    assert!(pos <= end && end <= dst.len());
    let width = CONTAINER_BYTES;
    if end - pos >= width {
        if table_log <= DECODER_FAST_TABLELOG as u32 {
            while bit_d.reload() == BitDStreamStatus::Unfinished && pos + 10 <= end {
                {
                    pos += decode_symbol_x2(dst, pos, bit_d, table, table_log);
                    pos += decode_symbol_x2(dst, pos, bit_d, table, table_log);
                    pos += decode_symbol_x2(dst, pos, bit_d, table, table_log);
                    pos += decode_symbol_x2(dst, pos, bit_d, table, table_log);
                    pos += decode_symbol_x2(dst, pos, bit_d, table, table_log);
                }
            }
        } else {
            while bit_d.reload() == BitDStreamStatus::Unfinished && pos + width <= end {
                {
                    pos += decode_symbol_x2(dst, pos, bit_d, table, table_log);
                    pos += decode_symbol_x2(dst, pos, bit_d, table, table_log);
                    pos += decode_symbol_x2(dst, pos, bit_d, table, table_log);
                    pos += decode_symbol_x2(dst, pos, bit_d, table, table_log);
                }
            }
        }
    } else {
        bit_d.reload();
    }
    if end - pos >= 2 {
        while bit_d.reload() == BitDStreamStatus::Unfinished && pos + 2 <= end {
            pos += decode_symbol_x2(dst, pos, bit_d, table, table_log);
        }
        while pos + 2 <= end {
            pos += decode_symbol_x2(dst, pos, bit_d, table, table_log);
        }
    }
    if pos < end {
        pos += decode_last_symbol_x2(dst, pos, bit_d, table, table_log);
    }
    pos
}
fn decompress_1x2_using_dtable(dst: &mut [u8], src: &[u8], dtable: &DTableX2) -> Result<usize> {
    let mut bit_d = BitDStream::new(src)?;
    let table_log = dtable.table_log as u32;
    let table = &dtable.entries[..1usize << table_log];
    let written = decode_stream_x2(dst, 0, &mut bit_d, dst.len(), table, table_log);
    if written != dst.len() || !bit_d.end_of_stream() {
        return Err(Error::Corruption("Huff0 stream did not terminate cleanly"));
    }
    Ok(written)
}
fn decompress_4x2_using_dtable(dst: &mut [u8], src: &[u8], dtable: &DTableX2) -> Result<usize> {
    if src.len() < 10 {
        return Err(Error::Corruption("truncated Huff0 4X stream"));
    }
    if dst.len() < 6 {
        return Err(Error::Corruption("Huff0 4X output too small to split"));
    }
    let length1 = read_segment_length(src, 0);
    let length2 = read_segment_length(src, 2);
    let length3 = read_segment_length(src, 4);
    let length4 = src.len().wrapping_sub(length1 + length2 + length3 + 6);
    if length4 > src.len() {
        return Err(Error::Corruption("invalid Huff0 4X segment lengths"));
    }
    let istart1 = 6;
    let istart2 = istart1 + length1;
    let istart3 = istart2 + length2;
    let istart4 = istart3 + length3;
    let segment_size = dst.len().div_ceil(4);
    let mut bit_d1 = BitDStream::new(&src[istart1..istart2])?;
    let mut bit_d2 = BitDStream::new(&src[istart2..istart3])?;
    let mut bit_d3 = BitDStream::new(&src[istart3..istart4])?;
    let mut bit_d4 = BitDStream::new(&src[istart4..])?;
    let op_start2 = segment_size;
    let op_start3 = op_start2 + segment_size;
    let op_start4 = op_start3 + segment_size;
    if op_start4 > dst.len() {
        return Err(Error::Corruption("Huff0 4X segment starts past the output"));
    }
    let mut op1 = 0usize;
    let mut op2 = op_start2;
    let mut op3 = op_start3;
    let mut op4 = op_start4;
    let mut end_signal = true;
    let table_log = dtable.table_log as u32;
    let table = &dtable.entries[..1usize << table_log];
    let width = CONTAINER_BYTES;
    while end_signal
        && op1 + width <= op_start2
        && op2 + width <= op_start3
        && op3 + width <= op_start4
        && op4 + width <= dst.len()
    {
        {
            op1 += decode_symbol_x2(dst, op1, &mut bit_d1, table, table_log);
            op2 += decode_symbol_x2(dst, op2, &mut bit_d2, table, table_log);
            op3 += decode_symbol_x2(dst, op3, &mut bit_d3, table, table_log);
            op4 += decode_symbol_x2(dst, op4, &mut bit_d4, table, table_log);
            op1 += decode_symbol_x2(dst, op1, &mut bit_d1, table, table_log);
            op2 += decode_symbol_x2(dst, op2, &mut bit_d2, table, table_log);
            op3 += decode_symbol_x2(dst, op3, &mut bit_d3, table, table_log);
            op4 += decode_symbol_x2(dst, op4, &mut bit_d4, table, table_log);
            op1 += decode_symbol_x2(dst, op1, &mut bit_d1, table, table_log);
            op2 += decode_symbol_x2(dst, op2, &mut bit_d2, table, table_log);
            op3 += decode_symbol_x2(dst, op3, &mut bit_d3, table, table_log);
            op4 += decode_symbol_x2(dst, op4, &mut bit_d4, table, table_log);
            op1 += decode_symbol_x2(dst, op1, &mut bit_d1, table, table_log);
            op2 += decode_symbol_x2(dst, op2, &mut bit_d2, table, table_log);
            op3 += decode_symbol_x2(dst, op3, &mut bit_d3, table, table_log);
            op4 += decode_symbol_x2(dst, op4, &mut bit_d4, table, table_log);
        }
        end_signal &= bit_d1.reload_fast() == BitDStreamStatus::Unfinished;
        end_signal &= bit_d2.reload_fast() == BitDStreamStatus::Unfinished;
        end_signal &= bit_d3.reload_fast() == BitDStreamStatus::Unfinished;
        end_signal &= bit_d4.reload_fast() == BitDStreamStatus::Unfinished;
    }
    if op1 > op_start2 || op2 > op_start3 || op3 > op_start4 {
        return Err(Error::Corruption("Huff0 segment overrun"));
    }
    op1 = decode_stream_x2(dst, op1, &mut bit_d1, op_start2, table, table_log);
    op2 = decode_stream_x2(dst, op2, &mut bit_d2, op_start3, table, table_log);
    op3 = decode_stream_x2(dst, op3, &mut bit_d3, op_start4, table, table_log);
    op4 = decode_stream_x2(dst, op4, &mut bit_d4, dst.len(), table, table_log);
    debug_assert_eq!(op1, op_start2);
    debug_assert_eq!(op2, op_start3);
    debug_assert_eq!(op3, op_start4);
    debug_assert_eq!(op4, dst.len());
    if !bit_d1.end_of_stream()
        || !bit_d2.end_of_stream()
        || !bit_d3.end_of_stream()
        || !bit_d4.end_of_stream()
    {
        return Err(Error::Corruption("Huff0 stream did not consume all bits"));
    }
    Ok(dst.len())
}
pub(crate) const fn prefers_double_symbol_decoder(
    regenerated_size: usize,
    compressed_size: usize,
) -> bool {
    const ALGO_TIME: [[(u64, u64); 2]; 16] = [
        [(0, 0), (1, 1)],
        [(0, 0), (1, 1)],
        [(150, 216), (381, 119)],
        [(170, 205), (514, 112)],
        [(177, 199), (539, 110)],
        [(197, 194), (644, 107)],
        [(221, 192), (735, 107)],
        [(256, 189), (881, 106)],
        [(359, 188), (1167, 109)],
        [(582, 187), (1570, 114)],
        [(688, 187), (1712, 122)],
        [(825, 186), (1965, 136)],
        [(976, 185), (2131, 150)],
        [(1180, 186), (2070, 175)],
        [(1377, 185), (1731, 202)],
        [(1412, 185), (1695, 202)],
    ];
    if regenerated_size == 0 {
        return false;
    }
    let quantized = if compressed_size >= regenerated_size {
        15
    } else {
        compressed_size * 16 / regenerated_size
    };
    let per_256 = (regenerated_size >> 8) as u64;
    let [(single_build, single_rate), (double_build, double_rate)] = ALGO_TIME[quantized];
    let single = single_build + single_rate * per_256;
    let double = double_build + double_rate * per_256;
    double + (double >> 5) < single
}
#[derive(Clone, Copy)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum HuffmanDTable {
    Single(DTableX1),
    Double(DTableX2),
}
impl HuffmanDTable {
    pub(crate) fn single_slot(slot: &mut Option<Self>) -> &mut DTableX1 {
        if !matches!(slot, Some(Self::Single(_))) {
            *slot = Some(Self::Single(DTableX1::default()));
        }
        match slot {
            Some(Self::Single(table)) => table,
            _ => unreachable!("just installed"),
        }
    }
    pub(crate) fn double_slot(slot: &mut Option<Self>) -> &mut DTableX2 {
        if !matches!(slot, Some(Self::Double(_))) {
            *slot = Some(Self::Double(DTableX2::default()));
        }
        match slot {
            Some(Self::Double(table)) => table,
            _ => unreachable!("just installed"),
        }
    }
}
pub(crate) fn decompress_1x_using_dtable(
    dst: &mut [u8],
    src: &[u8],
    dtable: &HuffmanDTable,
) -> Result<usize> {
    match dtable {
        HuffmanDTable::Single(table) => decompress_1x1_using_dtable(dst, src, table),
        HuffmanDTable::Double(table) => decompress_1x2_using_dtable(dst, src, table),
    }
}
pub(crate) fn decompress_4x_using_dtable(
    dst: &mut [u8],
    src: &[u8],
    dtable: &HuffmanDTable,
) -> Result<usize> {
    match dtable {
        HuffmanDTable::Single(table) => decompress_4x1_using_dtable(dst, src, table),
        HuffmanDTable::Double(table) => decompress_4x2_using_dtable(dst, src, table),
    }
}
