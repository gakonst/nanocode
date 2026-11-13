use crate::{
    decode_out::DecodeOut,
    entropy::{bitstream::BitDStreamStatus, fse},
    error::{Error, Result},
    literals::DecodedLiterals,
};
use std::sync::OnceLock;
pub(crate) const LITERAL_LENGTH_BASELINES: [u32; 36] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 18, 20, 22, 24, 28, 32, 40, 48, 64,
    128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536,
];
pub(crate) const LITERAL_LENGTH_EXTRA_BITS: [u8; 36] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 3, 3, 4, 6, 7, 8, 9, 10, 11,
    12, 13, 14, 15, 16,
];
pub(crate) const MATCH_LENGTH_BASELINES: [u32; 53] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27,
    28, 29, 30, 31, 32, 33, 34, 35, 37, 39, 41, 43, 47, 51, 59, 67, 83, 99, 131, 259, 515, 1027,
    2051, 4099, 8195, 16387, 32771, 65539,
];
pub(crate) const MATCH_LENGTH_EXTRA_BITS: [u8; 53] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    1, 1, 1, 1, 2, 2, 3, 3, 4, 4, 5, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16,
];
pub(crate) const LITERAL_LENGTH_DEFAULT_DISTRIBUTION: [i16; 36] = [
    4, 3, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 3, 2, 1, 1, 1, 1, 1,
    -1, -1, -1, -1,
];
pub(crate) const OFFSET_DEFAULT_DISTRIBUTION: [i16; 29] = [
    1, 1, 1, 1, 1, 1, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, -1, -1, -1, -1, -1,
];
pub(crate) const MATCH_LENGTH_DEFAULT_DISTRIBUTION: [i16; 53] = [
    1, 4, 3, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, -1, -1, -1, -1, -1, -1, -1,
];
pub(crate) const LL_DEFAULT_LOG: u32 = 6;
pub(crate) const OF_DEFAULT_LOG: u32 = 5;
pub(crate) const ML_DEFAULT_LOG: u32 = 6;
static PREDEFINED_SEQUENCE_DTABLES: OnceLock<PredefinedSequenceDTables> = OnceLock::new();
static PREDEFINED_SEQ_DTABLES: OnceLock<PredefinedSeqDTables> = OnceLock::new();
struct PredefinedSequenceDTables {
    literal_lengths: fse::DTable,
    offsets: fse::DTable,
    match_lengths: fse::DTable,
}
struct PredefinedSeqDTables {
    literal_lengths: fse::SequenceDTable,
    offsets: fse::SequenceDTable,
    match_lengths: fse::SequenceDTable,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SequencePart {
    LiteralLength,
    Offset,
    MatchLength,
}
impl SequencePart {}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TableTarget {
    SequenceOnly,
    Both,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompressionMode {
    Predefined,
    Rle,
    FseCompressed,
    Repeat,
}
impl CompressionMode {
    fn from_bits(bits: u8) -> Self {
        match bits & 0x3 {
            0 => Self::Predefined,
            1 => Self::Rle,
            2 => Self::FseCompressed,
            3 => Self::Repeat,
            _ => unreachable!(),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SequenceCompressionModes {
    pub(crate) literal_lengths: CompressionMode,
    pub(crate) offsets: CompressionMode,
    pub(crate) match_lengths: CompressionMode,
}
#[derive(Clone)]
pub(crate) struct SequenceTablesState {
    literal_lengths: Option<fse::DTable>,
    offsets: Option<fse::DTable>,
    match_lengths: Option<fse::DTable>,
    seq_ll: fse::SequenceDTable,
    seq_of: fse::SequenceDTable,
    seq_ml: fse::SequenceDTable,
    ll_dirty: bool,
    of_dirty: bool,
    ml_dirty: bool,
    has_ll: bool,
    has_of: bool,
    has_ml: bool,
}
impl Default for SequenceTablesState {
    fn default() -> Self {
        Self {
            literal_lengths: None,
            offsets: None,
            match_lengths: None,
            seq_ll: fse::SequenceDTable::default(),
            seq_of: fse::SequenceDTable::default(),
            seq_ml: fse::SequenceDTable::default(),
            ll_dirty: true,
            of_dirty: true,
            ml_dirty: true,
            has_ll: false,
            has_of: false,
            has_ml: false,
        }
    }
}
impl SequenceTablesState {}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedSequenceSection<'a> {
    pub(crate) number_of_sequences: usize,
    pub(crate) header_size: usize,
    pub(crate) modes: Option<SequenceCompressionModes>,
    pub(crate) bitstream: &'a [u8],
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RepeatOffsets {
    values: [u32; 3],
}
impl Default for RepeatOffsets {
    fn default() -> Self {
        Self { values: [1, 4, 8] }
    }
}
pub(crate) fn parse_sequence_count(src: &[u8]) -> Result<(usize, usize)> {
    let byte0 = *src.first().ok_or(Error::UnexpectedEof)?;
    if byte0 < 128 {
        Ok((byte0 as usize, 1))
    } else if byte0 < 255 {
        let byte1 = *src.get(1).ok_or(Error::UnexpectedEof)?;
        Ok((((byte0 as usize - 128) << 8) + byte1 as usize, 2))
    } else {
        let byte1 = *src.get(1).ok_or(Error::UnexpectedEof)?;
        let byte2 = *src.get(2).ok_or(Error::UnexpectedEof)?;
        Ok((0x7F00 + byte1 as usize + ((byte2 as usize) << 8), 3))
    }
}
pub(crate) fn parse_sequence_section<'a>(
    src: &'a [u8],
    tables: &mut SequenceTablesState,
    target: TableTarget,
) -> Result<ParsedSequenceSection<'a>> {
    let (number_of_sequences, count_size) = parse_sequence_count(src)?;
    if number_of_sequences == 0 {
        return Ok(ParsedSequenceSection {
            number_of_sequences,
            header_size: count_size,
            modes: None,
            bitstream: &[],
        });
    }
    let modes_byte = *src.get(count_size).ok_or(Error::UnexpectedEof)?;
    if (modes_byte & 0x3) != 0 {
        return Err(Error::Corruption(
            "sequence compression modes reserved bits are set",
        ));
    }
    let modes = SequenceCompressionModes {
        literal_lengths: CompressionMode::from_bits(modes_byte >> 6),
        offsets: CompressionMode::from_bits((modes_byte >> 4) & 0x3),
        match_lengths: CompressionMode::from_bits((modes_byte >> 2) & 0x3),
    };
    let mut cursor = count_size + 1;
    decode_seq_table_both(
        &mut tables.literal_lengths,
        &mut tables.seq_ll,
        &mut tables.has_ll,
        src,
        &mut cursor,
        SequencePart::LiteralLength,
        modes.literal_lengths,
        target,
    )?;
    if modes.literal_lengths != CompressionMode::Repeat {
        tables.ll_dirty = false;
    }
    decode_seq_table_both(
        &mut tables.offsets,
        &mut tables.seq_of,
        &mut tables.has_of,
        src,
        &mut cursor,
        SequencePart::Offset,
        modes.offsets,
        target,
    )?;
    if modes.offsets != CompressionMode::Repeat {
        tables.of_dirty = false;
    }
    decode_seq_table_both(
        &mut tables.match_lengths,
        &mut tables.seq_ml,
        &mut tables.has_ml,
        src,
        &mut cursor,
        SequencePart::MatchLength,
        modes.match_lengths,
        target,
    )?;
    if modes.match_lengths != CompressionMode::Repeat {
        tables.ml_dirty = false;
    }
    let bitstream = src.get(cursor..).ok_or(Error::UnexpectedEof)?;
    if bitstream.is_empty() {
        return Err(Error::UnexpectedEof);
    }
    Ok(ParsedSequenceSection {
        number_of_sequences,
        header_size: cursor,
        modes: Some(modes),
        bitstream,
    })
}
pub(crate) fn decode_and_execute_sequences(
    section: &ParsedSequenceSection<'_>,
    tables: &mut SequenceTablesState,
    out: &mut DecodeOut<'_>,
    frame_start: usize,
    window_size: usize,
    block_size_max: usize,
    dictionary: Option<&[u8]>,
    literals: DecodedLiterals<'_>,
    repeat_offsets: &mut RepeatOffsets,
    limit: OutputLimit,
) -> Result<()> {
    if section.number_of_sequences == 0 {
        let mut remaining_block_bytes = block_size_max;
        let mut remaining_output_bytes = limit.remaining(out.len())?;
        return append_trailing_literals(
            out,
            literals.as_slice(),
            0,
            &mut remaining_block_bytes,
            &mut remaining_output_bytes,
            limit,
        );
    }
    decode_and_execute_sequences_unified(
        section,
        tables,
        out,
        frame_start,
        window_size,
        block_size_max,
        dictionary,
        literals,
        repeat_offsets,
        limit,
    )
}
#[inline(never)]
fn decode_setup(
    tables: &mut SequenceTablesState,
    out: &mut DecodeOut<'_>,
    block_size_max: usize,
    limit: OutputLimit,
) -> Result<(usize, usize)> {
    if tables.ll_dirty {
        let ll = tables
            .literal_lengths
            .as_ref()
            .ok_or(Error::Corruption("missing literal-length FSE table"))?;
        fse::build_sequence_dtable_from(
            ll,
            &LITERAL_LENGTH_BASELINES,
            &LITERAL_LENGTH_EXTRA_BITS,
            &mut tables.seq_ll,
        );
        tables.ll_dirty = false;
        tables.has_ll = true;
    } else if !tables.has_ll {
        return Err(Error::Corruption("missing literal-length FSE table"));
    }
    if tables.of_dirty {
        let of = tables
            .offsets
            .as_ref()
            .ok_or(Error::Corruption("missing offset FSE table"))?;
        fse::build_offset_sequence_dtable_from(of, &mut tables.seq_of);
        tables.of_dirty = false;
        tables.has_of = true;
    } else if !tables.has_of {
        return Err(Error::Corruption("missing offset FSE table"));
    }
    if tables.ml_dirty {
        let ml = tables
            .match_lengths
            .as_ref()
            .ok_or(Error::Corruption("missing match-length FSE table"))?;
        fse::build_sequence_dtable_from(
            ml,
            &MATCH_LENGTH_BASELINES,
            &MATCH_LENGTH_EXTRA_BITS,
            &mut tables.seq_ml,
        );
        tables.ml_dirty = false;
        tables.has_ml = true;
    } else if !tables.has_ml {
        return Err(Error::Corruption("missing match-length FSE table"));
    }
    reserve_block_output(out, block_size_max, limit)?;
    let remaining_output_bytes = limit.remaining(out.len())?;
    Ok((block_size_max, remaining_output_bytes))
}
fn decode_and_execute_sequences_unified(
    section: &ParsedSequenceSection<'_>,
    tables: &mut SequenceTablesState,
    out: &mut DecodeOut<'_>,
    frame_start: usize,
    window_size: usize,
    block_size_max: usize,
    dictionary: Option<&[u8]>,
    literals: DecodedLiterals<'_>,
    repeat_offsets: &mut RepeatOffsets,
    limit: OutputLimit,
) -> Result<()> {
    let (mut remaining_block_bytes, mut remaining_output_bytes) =
        decode_setup(tables, out, block_size_max, limit)?;
    let start = out.len();
    let room = remaining_output_bytes.min(block_size_max);
    let buffer = out.buffer_mut();
    buffer.resize(
        start + room.min(OUTPUT_GROWTH_BYTES_MIN) + WILD_COPY_SLACK_BYTES,
        0,
    );
    let mut position = start;
    let mut literal_cursor = 0usize;
    let result = execute_sequences(
        section,
        tables,
        buffer,
        start,
        start + room + WILD_COPY_SLACK_BYTES,
        &mut position,
        &mut literal_cursor,
        frame_start,
        window_size,
        dictionary,
        literals,
        repeat_offsets,
        limit,
        &mut remaining_block_bytes,
        &mut remaining_output_bytes,
    );
    assert!(
        position <= start + room,
        "sequences wrote {} bytes into a block of {room}",
        position - start
    );
    buffer.truncate(position);
    result?;
    append_trailing_literals(
        out,
        literals.as_slice(),
        literal_cursor,
        &mut remaining_block_bytes,
        &mut remaining_output_bytes,
        limit,
    )
}
#[inline(always)]
fn execute_sequences(
    section: &ParsedSequenceSection<'_>,
    tables: &SequenceTablesState,
    buffer: &mut Vec<u8>,
    block_start: usize,
    buffer_bytes_max: usize,
    position: &mut usize,
    literal_cursor: &mut usize,
    frame_start: usize,
    window_size: usize,
    dictionary: Option<&[u8]>,
    literals: DecodedLiterals<'_>,
    repeat_offsets: &mut RepeatOffsets,
    limit: OutputLimit,
    remaining_block_bytes: &mut usize,
    remaining_output_bytes: &mut usize,
) -> Result<()> {
    let mut reader = crate::entropy::bitstream::BitDStream::new(section.bitstream)?;
    let mut literal_state = fse::init_dstate_seq(&mut reader, &tables.seq_ll);
    let mut offset_state = fse::init_dstate_seq(&mut reader, &tables.seq_of);
    let mut match_state = fse::init_dstate_seq(&mut reader, &tables.seq_ml);
    let padded_literals = literals.padded();
    for index in 0..section.number_of_sequences {
        let lit_raw = tables.seq_ll.get_entry_raw(literal_state.state)?;
        let off_raw = tables.seq_of.get_entry_raw(offset_state.state)?;
        let ml_raw = tables.seq_ml.get_entry_raw(match_state.state)?;
        let offset_code = fse::raw_entry_nb_additional_bits(off_raw);
        let match_extra_bits = fse::raw_entry_nb_additional_bits(ml_raw);
        let literal_extra_bits = fse::raw_entry_nb_additional_bits(lit_raw);
        let offset_extra = if offset_code == 0 {
            0
        } else {
            reader.read_bits_fast(offset_code) as u32
        };
        let match_extra = if match_extra_bits == 0 {
            0
        } else {
            reader.read_bits_fast(match_extra_bits) as u32
        };
        if offset_code + match_extra_bits + literal_extra_bits >= 31 {
            let _ = reader.reload();
        }
        let literal_extra = if literal_extra_bits == 0 {
            0
        } else {
            reader.read_bits_fast(literal_extra_bits) as u32
        };
        let literal_length = (fse::raw_entry_baseline(lit_raw) + literal_extra) as usize;
        let match_length = (fse::raw_entry_baseline(ml_raw) + match_extra) as usize;
        let offset_value = (1u32 << offset_code) + offset_extra;
        let total = literal_length + match_length;
        let literal_end = (*literal_cursor)
            .checked_add(literal_length)
            .ok_or(Error::OutputSizeOverflow)?;
        if literal_end > literals.len() {
            return Err(Error::Corruption(
                "sequence literal length exceeds literals buffer",
            ));
        }
        ensure_decode_output_room(
            *position,
            total,
            remaining_block_bytes,
            remaining_output_bytes,
            limit,
        )?;
        let needed = *position + total + WILD_COPY_SLACK_BYTES;
        if needed > buffer.len() {
            grow_output(buffer, block_start, needed, buffer_bytes_max);
        }
        let [rep1, rep2, rep3] = repeat_offsets.values;
        let offset = if offset_value > 3 {
            let offset = offset_value - 3;
            repeat_offsets.values = [offset, rep1, rep2];
            offset as usize
        } else {
            let rep_index = offset_value + u32::from(literal_length == 0);
            match rep_index {
                1 => rep1 as usize,
                2 => {
                    repeat_offsets.values = [rep2, rep1, rep3];
                    rep2 as usize
                }
                3 => {
                    repeat_offsets.values = [rep3, rep1, rep2];
                    rep3 as usize
                }
                _ => {
                    let offset = rep1
                        .checked_sub(1)
                        .filter(|offset| *offset != 0)
                        .ok_or(Error::Corruption("repeat offset 1 minus 1 is zero"))?;
                    repeat_offsets.values = [offset, rep1, rep2];
                    offset as usize
                }
            }
        };
        copy_literals(
            buffer,
            *position,
            padded_literals,
            *literal_cursor,
            literal_length,
        );
        *position += literal_length;
        *literal_cursor = literal_end;
        let produced_in_frame = *position - frame_start;
        if offset != 0 && offset <= produced_in_frame && offset <= window_size {
            copy_match(buffer, *position, *position - offset, match_length);
            *position += match_length;
        } else if let Some(dictionary) = dictionary {
            let available_history = dictionary
                .len()
                .checked_add(produced_in_frame)
                .ok_or(Error::OutputSizeOverflow)?;
            if offset == 0 || offset > available_history || produced_in_frame > window_size {
                return Err(Error::Corruption(
                    "sequence offset exceeds the available history window",
                ));
            }
            copy_match_from_dictionary(
                buffer,
                *position,
                frame_start,
                dictionary,
                offset,
                match_length,
            );
            *position += match_length;
        } else {
            return Err(Error::Corruption(
                "sequence offset exceeds the available history window",
            ));
        }
        if index + 1 < section.number_of_sequences {
            let ll_bits = fse::raw_entry_nb_bits(lit_raw);
            let ml_bits = fse::raw_entry_nb_bits(ml_raw);
            let of_bits = fse::raw_entry_nb_bits(off_raw);
            let state_bits = ll_bits + ml_bits + of_bits;
            let state_val = if state_bits == 0 {
                0
            } else {
                reader.read_bits_fast(state_bits) as usize
            };
            literal_state.state = fse::raw_entry_new_state(lit_raw)
                + ((state_val >> (ml_bits + of_bits)) & ((1usize << ll_bits).wrapping_sub(1)));
            match_state.state = fse::raw_entry_new_state(ml_raw)
                + ((state_val >> of_bits) & ((1usize << ml_bits).wrapping_sub(1)));
            offset_state.state = fse::raw_entry_new_state(off_raw)
                + (state_val & ((1usize << of_bits).wrapping_sub(1)));
        }
        if reader.reload() == BitDStreamStatus::Overflow {
            return Err(Error::Corruption("sequence bitstream overflow"));
        }
    }
    if !reader.end_of_stream() {
        return Err(Error::Corruption(
            "sequence bitstream was not fully consumed",
        ));
    }
    Ok(())
}
const WILD_COPY_BYTES: usize = 16;
const NARROW_COPY_BYTES: usize = 8;
const WILD_COPY_SLACK_BYTES: usize = 2 * WILD_COPY_BYTES;
const OUTPUT_GROWTH_BYTES_MIN: usize = 4096;
#[cold]
#[inline(never)]
fn grow_output(buffer: &mut Vec<u8>, block_start: usize, needed: usize, buffer_bytes_max: usize) {
    assert!(
        needed <= buffer_bytes_max,
        "a block needs {needed} bytes of its {buffer_bytes_max}"
    );
    let doubled = block_start + 2 * (buffer.len() - block_start);
    buffer.resize(needed.max(doubled).min(buffer_bytes_max), 0);
}
#[inline(always)]
fn copy_literals(
    buffer: &mut [u8],
    position: usize,
    padded_literals: &[u8],
    cursor: usize,
    length: usize,
) {
    let mut copied = 0;
    loop {
        let chunk: [u8; WILD_COPY_BYTES] = padded_literals
            [cursor + copied..cursor + copied + WILD_COPY_BYTES]
            .try_into()
            .expect("a wild copy reads one chunk");
        buffer[position + copied..position + copied + WILD_COPY_BYTES].copy_from_slice(&chunk);
        copied += WILD_COPY_BYTES;
        if copied >= length {
            return;
        }
    }
}
#[inline(always)]
fn copy_chunks<const CHUNK_BYTES: usize>(
    buffer: &mut [u8],
    position: usize,
    distance: usize,
    start: usize,
    length: usize,
) {
    assert!(
        distance >= CHUNK_BYTES,
        "chunk copies read behind what they write"
    );
    let mut copied = start;
    while copied < length {
        let source = position + copied - distance;
        let chunk: [u8; CHUNK_BYTES] = buffer[source..source + CHUNK_BYTES]
            .try_into()
            .expect("a chunk copy reads one chunk");
        buffer[position + copied..position + copied + CHUNK_BYTES].copy_from_slice(&chunk);
        copied += CHUNK_BYTES;
    }
}
#[inline(always)]
fn copy_match(buffer: &mut [u8], position: usize, source: usize, length: usize) {
    let distance = position - source;
    assert!(distance != 0, "a match copies from behind its position");
    if distance >= WILD_COPY_BYTES {
        copy_chunks::<WILD_COPY_BYTES>(buffer, position, distance, 0, length);
        return;
    }
    if distance >= NARROW_COPY_BYTES {
        copy_chunks::<NARROW_COPY_BYTES>(buffer, position, distance, 0, length);
        return;
    }
    let period = distance * NARROW_COPY_BYTES.div_ceil(distance);
    let lead = length.min(period);
    for index in position..position + lead {
        buffer[index] = buffer[index - distance];
    }
    copy_chunks::<NARROW_COPY_BYTES>(buffer, position, period, lead, length);
}
#[inline(always)]
fn copy_match_from_dictionary(
    buffer: &mut [u8],
    position: usize,
    frame_start: usize,
    dictionary: &[u8],
    offset: usize,
    match_length: usize,
) {
    if match_length == 0 {
        return;
    }
    let history_len = dictionary.len() + (position - frame_start);
    let source_index = history_len - offset;
    if source_index < dictionary.len() {
        let dictionary_bytes = (dictionary.len() - source_index).min(match_length);
        buffer[position..position + dictionary_bytes]
            .copy_from_slice(&dictionary[source_index..source_index + dictionary_bytes]);
        let remaining = match_length - dictionary_bytes;
        if remaining != 0 {
            copy_match(buffer, position + dictionary_bytes, frame_start, remaining);
        }
        return;
    }
    let match_start = frame_start + (source_index - dictionary.len());
    copy_match(buffer, position, match_start, match_length);
}
#[derive(Debug, Clone, Copy)]
pub(crate) struct OutputLimit {
    produced_before: u64,
    max_output_size: Option<usize>,
}
impl OutputLimit {
    pub(crate) const fn after(produced_before: u64, max_output_size: Option<usize>) -> Self {
        Self {
            produced_before,
            max_output_size,
        }
    }
    #[inline(always)]
    const fn produced(&self, out_len: usize) -> u64 {
        self.produced_before.saturating_add(out_len as u64)
    }
    #[inline(always)]
    pub(crate) const fn check_total(&self, out_len: usize) -> Result<()> {
        let Some(max_output_size) = self.max_output_size else {
            return Ok(());
        };
        let produced = self.produced(out_len);
        if produced > max_output_size as u64 {
            return Err(Error::OutputSizeTooLarge {
                output_size: produced,
                max_output_size,
            });
        }
        Ok(())
    }
    #[inline(always)]
    fn remaining(&self, out_len: usize) -> Result<usize> {
        let Some(limit) = self.max_output_size else {
            return Ok(usize::MAX);
        };
        let produced = self.produced(out_len);
        let remaining = (limit as u64)
            .checked_sub(produced)
            .ok_or(Error::OutputSizeTooLarge {
                output_size: produced,
                max_output_size: limit,
            })?;
        Ok(remaining as usize)
    }
}
#[inline(always)]
fn reserve_block_output(
    out: &mut DecodeOut<'_>,
    block_size_max: usize,
    limit: OutputLimit,
) -> Result<()> {
    let reserve = limit.remaining(out.len())?.min(block_size_max);
    out.try_reserve(reserve)
}
#[inline(always)]
fn ensure_decode_output_room(
    out_len: usize,
    additional: usize,
    remaining_block_bytes: &mut usize,
    remaining_output_bytes: &mut usize,
    limit: OutputLimit,
) -> Result<()> {
    if additional > *remaining_block_bytes {
        return Err(corruption_error(
            "compressed block output exceeds the frame block size limit",
        ));
    }
    *remaining_block_bytes -= additional;
    if additional > *remaining_output_bytes {
        let max_output_size = limit
            .max_output_size
            .expect("output limit present when tracking remaining bytes");
        let output_size = limit
            .produced(out_len)
            .checked_add(additional as u64)
            .ok_or_else(output_size_overflow_error)?;
        return Err(output_size_too_large_error(output_size, max_output_size));
    }
    *remaining_output_bytes -= additional;
    Ok(())
}
#[inline(always)]
fn append_trailing_literals(
    out: &mut DecodeOut<'_>,
    literals: &[u8],
    literal_cursor: usize,
    remaining_block_bytes: &mut usize,
    remaining_output_bytes: &mut usize,
    limit: OutputLimit,
) -> Result<()> {
    let trailing = literals.get(literal_cursor..).ok_or(Error::Corruption(
        "sequence literal cursor exceeded literals buffer",
    ))?;
    ensure_decode_output_room(
        out.len(),
        trailing.len(),
        remaining_block_bytes,
        remaining_output_bytes,
        limit,
    )?;
    out.append(trailing)
}
fn predefined_sequence_dtable(part: SequencePart) -> &'static fse::DTable {
    let tables = PREDEFINED_SEQUENCE_DTABLES.get_or_init(|| {
        let mut literal_lengths = fse::DTable::default();
        let mut offsets = fse::DTable::default();
        let mut match_lengths = fse::DTable::default();
        fse::build_dtable(
            &mut literal_lengths,
            &LITERAL_LENGTH_DEFAULT_DISTRIBUTION,
            (LITERAL_LENGTH_DEFAULT_DISTRIBUTION.len() - 1) as u32,
            LL_DEFAULT_LOG,
        )
        .expect("literal-length default dtable must build");
        fse::build_dtable(
            &mut offsets,
            &OFFSET_DEFAULT_DISTRIBUTION,
            (OFFSET_DEFAULT_DISTRIBUTION.len() - 1) as u32,
            OF_DEFAULT_LOG,
        )
        .expect("offset default dtable must build");
        fse::build_dtable(
            &mut match_lengths,
            &MATCH_LENGTH_DEFAULT_DISTRIBUTION,
            (MATCH_LENGTH_DEFAULT_DISTRIBUTION.len() - 1) as u32,
            ML_DEFAULT_LOG,
        )
        .expect("match-length default dtable must build");
        PredefinedSequenceDTables {
            literal_lengths,
            offsets,
            match_lengths,
        }
    });
    match part {
        SequencePart::LiteralLength => &tables.literal_lengths,
        SequencePart::Offset => &tables.offsets,
        SequencePart::MatchLength => &tables.match_lengths,
    }
}
fn predefined_seq_dtable(part: SequencePart) -> &'static fse::SequenceDTable {
    let tables = PREDEFINED_SEQ_DTABLES.get_or_init(|| {
        let mut ll = fse::SequenceDTable::default();
        let mut of = fse::SequenceDTable::default();
        let mut ml = fse::SequenceDTable::default();
        fse::build_sequence_dtable_direct(
            &mut ll,
            &LITERAL_LENGTH_DEFAULT_DISTRIBUTION,
            (LITERAL_LENGTH_DEFAULT_DISTRIBUTION.len() - 1) as u32,
            LL_DEFAULT_LOG,
            &LITERAL_LENGTH_BASELINES,
            &LITERAL_LENGTH_EXTRA_BITS,
        )
        .expect("literal-length default seq dtable must build");
        fse::build_offset_sequence_dtable_direct(
            &mut of,
            &OFFSET_DEFAULT_DISTRIBUTION,
            (OFFSET_DEFAULT_DISTRIBUTION.len() - 1) as u32,
            OF_DEFAULT_LOG,
        )
        .expect("offset default seq dtable must build");
        fse::build_sequence_dtable_direct(
            &mut ml,
            &MATCH_LENGTH_DEFAULT_DISTRIBUTION,
            (MATCH_LENGTH_DEFAULT_DISTRIBUTION.len() - 1) as u32,
            ML_DEFAULT_LOG,
            &MATCH_LENGTH_BASELINES,
            &MATCH_LENGTH_EXTRA_BITS,
        )
        .expect("match-length default seq dtable must build");
        PredefinedSeqDTables {
            literal_lengths: ll,
            offsets: of,
            match_lengths: ml,
        }
    });
    match part {
        SequencePart::LiteralLength => &tables.literal_lengths,
        SequencePart::Offset => &tables.offsets,
        SequencePart::MatchLength => &tables.match_lengths,
    }
}
fn decode_seq_table_both(
    dt_slot: &mut Option<fse::DTable>,
    seq_dt: &mut fse::SequenceDTable,
    has_table: &mut bool,
    src: &[u8],
    cursor: &mut usize,
    part: SequencePart,
    mode: CompressionMode,
    target: TableTarget,
) -> Result<()> {
    let want_dtable = target == TableTarget::Both;
    match mode {
        CompressionMode::Predefined => {
            if want_dtable {
                let precomputed_dt = predefined_sequence_dtable(part);
                let dt = dt_slot.get_or_insert_with(fse::DTable::default);
                let ts = if precomputed_dt.table_log() == 0 {
                    1
                } else {
                    1usize << precomputed_dt.table_log()
                };
                dt.copy_active_from(precomputed_dt, ts);
            } else {
                *dt_slot = None;
            }
            let precomputed_seq = predefined_seq_dtable(part);
            seq_dt.table_log = precomputed_seq.table_log;
            let seq_ts = if precomputed_seq.table_log == 0 {
                1
            } else {
                1usize << precomputed_seq.table_log
            };
            seq_dt.entries[..seq_ts].copy_from_slice(&precomputed_seq.entries[..seq_ts]);
            *has_table = true;
        }
        CompressionMode::Rle => {
            let symbol = *src.get(*cursor).ok_or(Error::UnexpectedEof)?;
            if symbol > part.max_symbol_value() {
                return Err(Error::Corruption(
                    "sequence RLE symbol exceeds the supported code range",
                ));
            }
            if want_dtable {
                let dt = dt_slot.get_or_insert_with(fse::DTable::default);
                fse::build_rle_dtable(dt, symbol);
            } else {
                *dt_slot = None;
            }
            match part {
                SequencePart::Offset => {
                    fse::build_rle_offset_sequence_dtable(seq_dt, symbol);
                }
                SequencePart::LiteralLength => {
                    fse::build_rle_sequence_dtable(
                        seq_dt,
                        symbol,
                        &LITERAL_LENGTH_BASELINES,
                        &LITERAL_LENGTH_EXTRA_BITS,
                    );
                }
                SequencePart::MatchLength => {
                    fse::build_rle_sequence_dtable(
                        seq_dt,
                        symbol,
                        &MATCH_LENGTH_BASELINES,
                        &MATCH_LENGTH_EXTRA_BITS,
                    );
                }
            }
            *has_table = true;
            *cursor += 1;
        }
        CompressionMode::FseCompressed => {
            let mut normalized = [0i16; fse::SYMBOLVALUE_MAX + 1];
            let mut max_symbol_value = u32::from(part.max_symbol_value());
            let mut table_log = 0u32;
            let consumed = fse::read_ncount(
                &mut normalized,
                &mut max_symbol_value,
                &mut table_log,
                &src[*cursor..],
                part.max_accuracy_log(),
            )?;
            if want_dtable {
                let dt = dt_slot.get_or_insert_with(fse::DTable::default);
                fse::build_dtable(dt, &normalized, max_symbol_value, table_log)?;
            } else {
                *dt_slot = None;
            }
            match part {
                SequencePart::Offset => {
                    fse::build_offset_sequence_dtable_direct(
                        seq_dt,
                        &normalized,
                        max_symbol_value,
                        table_log,
                    )?;
                }
                SequencePart::LiteralLength => {
                    fse::build_sequence_dtable_direct(
                        seq_dt,
                        &normalized,
                        max_symbol_value,
                        table_log,
                        &LITERAL_LENGTH_BASELINES,
                        &LITERAL_LENGTH_EXTRA_BITS,
                    )?;
                }
                SequencePart::MatchLength => {
                    fse::build_sequence_dtable_direct(
                        seq_dt,
                        &normalized,
                        max_symbol_value,
                        table_log,
                        &MATCH_LENGTH_BASELINES,
                        &MATCH_LENGTH_EXTRA_BITS,
                    )?;
                }
            }
            *has_table = true;
            *cursor += consumed;
        }
        CompressionMode::Repeat => {
            if !*has_table {
                return Err(Error::Corruption(
                    "sequence repeat mode requires a previous FSE table",
                ));
            }
        }
    }
    Ok(())
}
impl SequencePart {
    const fn max_symbol_value(self) -> u8 {
        match self {
            Self::LiteralLength => 35,
            Self::Offset => 31,
            Self::MatchLength => 52,
        }
    }
    const fn max_accuracy_log(self) -> usize {
        match self {
            Self::LiteralLength | Self::MatchLength => 9,
            Self::Offset => 8,
        }
    }
}
#[cold]
#[inline(never)]
const fn corruption_error(msg: &'static str) -> Error {
    Error::Corruption(msg)
}
#[cold]
#[inline(never)]
const fn output_size_overflow_error() -> Error {
    Error::OutputSizeOverflow
}
#[cold]
#[inline(never)]
const fn output_size_too_large_error(output_size: u64, max_output_size: usize) -> Error {
    Error::OutputSizeTooLarge {
        output_size,
        max_output_size,
    }
}
