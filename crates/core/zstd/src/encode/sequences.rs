use super::{
    bits::BitWriter,
    fse::{self, CState, CTable},
};
use crate::sequence::{
    LITERAL_LENGTH_BASELINES, LITERAL_LENGTH_DEFAULT_DISTRIBUTION, LITERAL_LENGTH_EXTRA_BITS,
    LL_DEFAULT_LOG, MATCH_LENGTH_BASELINES, MATCH_LENGTH_DEFAULT_DISTRIBUTION,
    MATCH_LENGTH_EXTRA_BITS, ML_DEFAULT_LOG, OF_DEFAULT_LOG, OFFSET_DEFAULT_DISTRIBUTION,
};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Sequence {
    pub(crate) literal_length: u32,
    pub(crate) match_length: u32,
    pub(crate) offset_base: u32,
}

const OFFSET_CODES: usize = 32;
const LONG_COUNT_FROM: usize = 0x80;
const LONGEST_COUNT_FROM: usize = 0x7F00;
const LOW_PROBABILITY_FROM: usize = 2048;
const LITERAL_CODE_TABLE: usize = 64;
const MATCH_CODE_TABLE: usize = 128;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Literal,
    Offset,
    Match,
}

impl Kind {
    const ALL: [Self; 3] = [Self::Literal, Self::Offset, Self::Match];

    const fn table_log_max(self) -> u32 {
        match self {
            Self::Literal | Self::Match => 9,
            Self::Offset => 8,
        }
    }

    const fn symbols(self) -> usize {
        match self {
            Self::Literal => LITERAL_LENGTH_BASELINES.len(),
            Self::Offset => OFFSET_CODES,
            Self::Match => MATCH_LENGTH_BASELINES.len(),
        }
    }

    const fn defaults(self) -> (&'static [i16], u32) {
        match self {
            Self::Literal => (&LITERAL_LENGTH_DEFAULT_DISTRIBUTION, LL_DEFAULT_LOG),
            Self::Offset => (&OFFSET_DEFAULT_DISTRIBUTION, OF_DEFAULT_LOG),
            Self::Match => (&MATCH_LENGTH_DEFAULT_DISTRIBUTION, ML_DEFAULT_LOG),
        }
    }
}

#[derive(Clone, Copy)]
enum Table {
    Predefined,
    Rle,
    Compressed {
        norm: [i16; MATCH_LENGTH_BASELINES.len()],
        max_symbol: usize,
        table_log: u32,
    },
}

#[derive(Clone, Copy)]
enum Mode {
    Predefined = 0,
    Rle = 1,
    Compressed = 2,
    Repeat = 3,
}

const fn code_table<const N: usize>(baselines: &[u32], shift: u32) -> [u8; N] {
    let mut table = [0_u8; N];
    let mut value = 0;
    while value < N {
        let mut code = 0;
        while code + 1 < baselines.len() && baselines[code + 1] <= value as u32 + shift {
            code += 1;
        }
        table[value] = code as u8;
        value += 1;
    }
    table
}

static LITERAL_CODES: [u8; LITERAL_CODE_TABLE] = code_table(&LITERAL_LENGTH_BASELINES, 0);
static MATCH_CODES: [u8; MATCH_CODE_TABLE] = code_table(&MATCH_LENGTH_BASELINES, 3);
const LITERAL_CODE_DELTA: u32 = 19;
const MATCH_CODE_DELTA: u32 = 36;

#[inline]
fn literal_code(length: u32) -> u8 {
    match LITERAL_CODES.get(length as usize) {
        Some(&code) => code,
        None => (fse::highbit(length) + LITERAL_CODE_DELTA) as u8,
    }
}

#[inline]
fn match_code(length: u32) -> u8 {
    let base = length - 3;
    match MATCH_CODES.get(base as usize) {
        Some(&code) => code,
        None => (fse::highbit(base) + MATCH_CODE_DELTA) as u8,
    }
}

#[derive(Default)]
pub(crate) struct SequenceCoder {
    committed: [Option<Table>; 3],
    pending: [Option<Table>; 3],
    codes: [Vec<u8>; 3],
    tables: [CTable; 3],
    spread: Vec<u8>,
    description: Vec<u8>,
}

impl SequenceCoder {
    pub(crate) const fn reset(&mut self) {
        self.committed = [None; 3];
        self.pending = [None; 3];
    }

    pub(crate) const fn commit(&mut self) {
        self.committed = self.pending;
    }

    pub(crate) fn write(&mut self, out: &mut Vec<u8>, sequences: &[Sequence]) {
        self.pending = self.committed;
        let count = sequences.len();
        if count < LONG_COUNT_FROM {
            out.push(count as u8);
        } else if count < LONGEST_COUNT_FROM {
            out.extend_from_slice(&[((count >> 8) + LONG_COUNT_FROM) as u8, count as u8]);
        } else {
            let rest = u16::try_from(count - LONGEST_COUNT_FROM)
                .expect("a block's sequences fit the count field");
            out.push(0xFF);
            out.extend_from_slice(&rest.to_le_bytes());
        }
        if count == 0 {
            return;
        }
        let mut counts = [[0_u32; MATCH_LENGTH_BASELINES.len()]; 3];
        for codes in &mut self.codes {
            codes.clear();
        }
        let [literal_codes, offset_codes, match_codes] = &mut self.codes;
        for sequence in sequences {
            assert!(
                sequence.match_length >= 3,
                "a match is at least three bytes"
            );
            assert!(sequence.offset_base >= 1, "an offset value is at least one");
            let literal = literal_code(sequence.literal_length);
            let offset = fse::highbit(sequence.offset_base) as u8;
            let matched = match_code(sequence.match_length);
            counts[0][usize::from(literal)] += 1;
            counts[1][usize::from(offset)] += 1;
            counts[2][usize::from(matched)] += 1;
            literal_codes.push(literal);
            offset_codes.push(offset);
            match_codes.push(matched);
        }
        let modes_at = out.len();
        out.push(0);
        let mut modes = 0_u8;
        for (index, kind) in Kind::ALL.into_iter().enumerate() {
            let mode = self.choose(index, kind, &counts[index], out);
            modes |= (mode as u8) << (6 - 2 * index);
        }
        out[modes_at] = modes;
        self.encode(out, sequences);
    }

    fn choose(
        &mut self,
        index: usize,
        kind: Kind,
        counts: &[u32; MATCH_LENGTH_BASELINES.len()],
        out: &mut Vec<u8>,
    ) -> Mode {
        let total = self.codes[index].len();
        let max_symbol = counts.iter().rposition(|&n| n > 0).unwrap_or(0);
        assert!(max_symbol < kind.symbols(), "a code is in its alphabet");
        if counts[max_symbol] as usize == total {
            out.push(max_symbol as u8);
            self.tables[index].rle(max_symbol as u8);
            self.pending[index] = Some(Table::Rle);
            return Mode::Rle;
        }
        let (defaults, default_log) = kind.defaults();
        let predefined = fse::cost_bits(counts, max_symbol, defaults, default_log);
        let repeat = match self.committed[index] {
            Some(Table::Predefined) => predefined,
            Some(Table::Compressed {
                norm,
                max_symbol: covered,
                table_log,
            }) if max_symbol <= covered => {
                fse::cost_bits(counts, max_symbol, &norm[..=covered], table_log)
            }
            _ => None,
        };
        let table_log = fse::optimal_table_log(kind.table_log_max(), total, max_symbol);
        let mut norm = [0_i16; MATCH_LENGTH_BASELINES.len()];
        fse::normalize(
            &mut norm,
            table_log,
            counts,
            total,
            max_symbol,
            total >= LOW_PROBABILITY_FROM,
        );
        self.description.clear();
        fse::write_description(&mut self.description, &norm, max_symbol, table_log);
        let compressed = fse::cost_bits(counts, max_symbol, &norm, table_log)
            .expect("a distribution covers its own symbols")
            + 8.0 * self.description.len() as f64;
        let best_reused = match (repeat, predefined) {
            (Some(repeat), Some(predefined)) if predefined < repeat => {
                Some((Mode::Predefined, predefined))
            }
            (Some(repeat), _) => Some((Mode::Repeat, repeat)),
            (None, Some(predefined)) => Some((Mode::Predefined, predefined)),
            (None, None) => None,
        };
        match best_reused {
            Some((Mode::Repeat, cost)) if cost <= compressed => {
                let Some(Table::Compressed {
                    norm,
                    max_symbol,
                    table_log,
                }) = self.committed[index]
                else {
                    let (defaults, default_log) = kind.defaults();
                    self.tables[index].build(
                        defaults,
                        defaults.len() - 1,
                        default_log,
                        &mut self.spread,
                    );
                    return Mode::Repeat;
                };
                self.tables[index].build(&norm, max_symbol, table_log, &mut self.spread);
                Mode::Repeat
            }
            Some((Mode::Predefined, cost)) if cost <= compressed => {
                self.tables[index].build(
                    defaults,
                    defaults.len() - 1,
                    default_log,
                    &mut self.spread,
                );
                self.pending[index] = Some(Table::Predefined);
                Mode::Predefined
            }
            _ => {
                out.extend_from_slice(&self.description);
                self.tables[index].build(&norm, max_symbol, table_log, &mut self.spread);
                self.pending[index] = Some(Table::Compressed {
                    norm,
                    max_symbol,
                    table_log,
                });
                Mode::Compressed
            }
        }
    }

    fn encode(&self, out: &mut Vec<u8>, sequences: &[Sequence]) {
        let [literal_codes, offset_codes, match_codes] = &self.codes;
        let [literal_table, offset_table, match_table] = &self.tables;
        let last = sequences.len() - 1;
        let mut bits = BitWriter::new(out);
        let mut match_state = CState::new(match_table, match_codes[last]);
        let mut offset_state = CState::new(offset_table, offset_codes[last]);
        let mut literal_state = CState::new(literal_table, literal_codes[last]);
        add_extra(
            &mut bits,
            &sequences[last],
            literal_codes[last],
            offset_codes[last],
            match_codes[last],
        );
        for index in (0..last).rev() {
            let (literal_code, offset_code, match_code) = (
                literal_codes[index],
                offset_codes[index],
                match_codes[index],
            );
            offset_state.encode(&mut bits, offset_table, offset_code);
            match_state.encode(&mut bits, match_table, match_code);
            literal_state.encode(&mut bits, literal_table, literal_code);
            add_extra(
                &mut bits,
                &sequences[index],
                literal_code,
                offset_code,
                match_code,
            );
        }
        match_state.flush(&mut bits, match_table);
        offset_state.flush(&mut bits, offset_table);
        literal_state.flush(&mut bits, literal_table);
        bits.close();
    }
}

#[inline]
fn add_extra(
    bits: &mut BitWriter<'_>,
    sequence: &Sequence,
    literal_code: u8,
    offset_code: u8,
    match_code: u8,
) {
    let literal_code = usize::from(literal_code);
    let match_code = usize::from(match_code);
    let literal_bits = u32::from(LITERAL_LENGTH_EXTRA_BITS[literal_code]);
    let match_bits = u32::from(MATCH_LENGTH_EXTRA_BITS[match_code]);
    let literal_extra = u64::from(sequence.literal_length - LITERAL_LENGTH_BASELINES[literal_code]);
    let match_extra = u64::from(sequence.match_length - MATCH_LENGTH_BASELINES[match_code]);
    bits.add(
        literal_extra | (match_extra << literal_bits),
        literal_bits + match_bits,
    );
    bits.add(
        u64::from(sequence.offset_base - (1 << offset_code)),
        u32::from(offset_code),
    );
}
