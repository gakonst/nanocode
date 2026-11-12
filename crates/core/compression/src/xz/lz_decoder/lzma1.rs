mod fast;
mod slow;

use alloc::boxed::Box;
use alloc::vec;

use super::super::lzma_common::{
    ALIGN_SIZE, DIST_MODEL_END, DIST_SLOTS, DIST_STATES, FULL_DISTANCES, LEN_HIGH_SYMBOLS,
    LEN_LOW_SYMBOLS, LEN_MID_SYMBOLS, LITERAL_CODER_SIZE, LITERAL_CODERS_MAX, LP_MAX,
    MATCH_LEN_MIN, POSITION_STATES_MAX, PROBABILITY_INITIAL, STATES, State, literal_mask,
    literal_subcoder_offset,
};
use super::super::stream::Error;
use super::dictionary::Dictionary;
use super::range_decoder::{Initialization, RangeCursor, RangeDecoder};
use super::{LzDecode, LzStatus};

const LITERAL: usize = 0;
const IS_MATCH: usize = LITERAL + LITERAL_CODER_SIZE * LITERAL_CODERS_MAX;
const IS_REP: usize = IS_MATCH + STATES * POSITION_STATES_MAX;
const IS_REP0: usize = IS_REP + STATES;
const IS_REP1: usize = IS_REP0 + STATES;
const IS_REP2: usize = IS_REP1 + STATES;
const IS_REP0_LONG: usize = IS_REP2 + STATES;
const DIST_SLOT: usize = IS_REP0_LONG + STATES * POSITION_STATES_MAX;
const DIST_SPECIAL: usize = DIST_SLOT + DIST_STATES * DIST_SLOTS;
const DIST_ALIGN: usize = DIST_SPECIAL + FULL_DISTANCES - DIST_MODEL_END as usize;
const MATCH_LEN: usize = DIST_ALIGN + ALIGN_SIZE;
const REP_LEN: usize = MATCH_LEN + LEN_PROBABILITIES;
const PROBABILITIES: usize = REP_LEN + LEN_PROBABILITIES;
const LEN_CHOICE: usize = 0;
const LEN_CHOICE2: usize = 1;
const LEN_LOW: usize = 2;
const LEN_MID: usize = LEN_LOW + POSITION_STATES_MAX * LEN_LOW_SYMBOLS;
const LEN_HIGH: usize = LEN_MID + POSITION_STATES_MAX * LEN_MID_SYMBOLS;
const LEN_PROBABILITIES: usize = LEN_HIGH + LEN_HIGH_SYMBOLS;
const LITERAL_BITS: u32 = 8;
const LITERAL_SYMBOLS: u32 = 1 << LITERAL_BITS;
const LITERAL_MATCH_OFFSET: u32 = LITERAL_SYMBOLS;
const LITERAL_POSITION_MASK: usize = (1 << LP_MAX) - 1;
const POSITION_STATE_MASK: usize = POSITION_STATES_MAX - 1;
const LEN_MID_MIN: u32 = MATCH_LEN_MIN as u32 + LEN_LOW_SYMBOLS as u32;
const LEN_HIGH_MIN: u32 = LEN_MID_MIN + LEN_MID_SYMBOLS as u32;
const END_MARKER_DISTANCE: u32 = u32::MAX;
const VLI_UNKNOWN: u64 = u64::MAX;
const FAST_INPUT_REQUIRED_BYTES: usize = 20;

type Probabilities = [u16; PROBABILITIES];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LengthKind {
    Match,
    Rep,
}

impl LengthKind {
    const fn base(self) -> usize {
        match self {
            Self::Match => MATCH_LEN,
            Self::Rep => REP_LEN,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sequence {
    IsMatch,
    Literal {
        probabilities: usize,
        symbol: u32,
    },
    LiteralMatched {
        probabilities: usize,
        symbol: u32,
        match_byte: u32,
        offset: u32,
    },
    LiteralWrite {
        symbol: u32,
    },
    IsRep,
    LenChoice(LengthKind),
    LenChoice2(LengthKind),
    LenBittree {
        kind: LengthKind,
        probabilities: usize,
        limit: u32,
        symbol: u32,
        len: u32,
    },
    DistSlot {
        probabilities: usize,
        symbol: u32,
        len: u32,
    },
    DistModel {
        probabilities: usize,
        symbol: u32,
        offset: u32,
        limit: u32,
        len: u32,
    },
    Direct {
        limit: u32,
        len: u32,
    },
    Align {
        symbol: u32,
        offset: u32,
        len: u32,
    },
    EndMarker,
    IsRep0,
    IsRep0Long,
    ShortRep,
    IsRep1,
    IsRep2,
    Copy {
        len: u32,
    },
}

impl Sequence {
    const fn writes_output(self) -> bool {
        matches!(
            self,
            Self::LiteralWrite { .. } | Self::ShortRep | Self::Copy { .. }
        )
    }
}

#[derive(Clone, Copy)]
struct Registers {
    state: State,
    rep0: u32,
    rep1: u32,
    rep2: u32,
    rep3: u32,
}

#[derive(Clone, Copy)]
struct Parameters {
    position_mask: usize,
    literal_context_bits: u32,
    literal_mask: u32,
}

impl Parameters {
    #[inline(always)]
    const fn position_state(self, dictionary: &Dictionary) -> usize {
        dictionary.position & self.position_mask & POSITION_STATE_MASK
    }

    #[inline(always)]
    fn literal_probabilities(self, dictionary: &Dictionary) -> usize {
        let position = u32::try_from(dictionary.position & LITERAL_POSITION_MASK)
            .expect("a masked position fits in 32 bits");
        LITERAL
            + literal_subcoder_offset(
                self.literal_context_bits,
                self.literal_mask,
                position,
                dictionary.get_previous(),
            )
    }
}

impl Registers {
    #[inline(always)]
    const fn shift_reps(&mut self) {
        self.rep3 = self.rep2;
        self.rep2 = self.rep1;
        self.rep1 = self.rep0;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Exit {
    Suspend(Sequence),
    End,
    Corrupt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FastExit {
    Slow,
    Copy(u32),
    EndMarker,
    Corrupt,
}

pub(super) struct Lzma1Decoder {
    probabilities: Box<Probabilities>,
    range_decoder: RangeDecoder,
    registers: Registers,
    parameters: Parameters,
    uncompressed_size: Option<u64>,
    allow_end_marker: bool,
    sequence: Sequence,
}

impl Lzma1Decoder {
    pub(super) fn new() -> Self {
        Self {
            probabilities: vec![0; PROBABILITIES]
                .into_boxed_slice()
                .try_into()
                .expect("the probability table has its fixed size"),
            range_decoder: RangeDecoder::new(),
            registers: Registers {
                state: State::LiteralLiteral,
                rep0: 0,
                rep1: 0,
                rep2: 0,
                rep3: 0,
            },
            parameters: Parameters {
                position_mask: 0,
                literal_context_bits: 0,
                literal_mask: 0,
            },
            uncompressed_size: None,
            allow_end_marker: true,
            sequence: Sequence::IsMatch,
        }
    }

    pub(super) fn set_uncompressed(
        &mut self,
        uncompressed_size: Option<u64>,
        allow_end_marker: bool,
    ) {
        self.uncompressed_size = uncompressed_size.filter(|&size| size != VLI_UNKNOWN);
        self.allow_end_marker = allow_end_marker;
    }

    pub(super) fn reset(&mut self, lc: u32, lp: u32, pb: u32) {
        let position_states = 1_usize << pb;
        self.parameters = Parameters {
            position_mask: position_states - 1,
            literal_context_bits: lc,
            literal_mask: literal_mask(lc, lp),
        };
        let probabilities = &mut *self.probabilities;
        probabilities[LITERAL..LITERAL + (LITERAL_CODER_SIZE << (lc + lp))]
            .fill(PROBABILITY_INITIAL);
        for state in 0..STATES {
            let row = state * POSITION_STATES_MAX;
            probabilities[IS_MATCH + row..IS_MATCH + row + position_states]
                .fill(PROBABILITY_INITIAL);
            probabilities[IS_REP0_LONG + row..IS_REP0_LONG + row + position_states]
                .fill(PROBABILITY_INITIAL);
        }
        probabilities[IS_REP..DIST_SLOT - STATES * POSITION_STATES_MAX].fill(PROBABILITY_INITIAL);
        probabilities[DIST_SLOT..MATCH_LEN].fill(PROBABILITY_INITIAL);
        for base in [MATCH_LEN, REP_LEN] {
            probabilities[base + LEN_CHOICE] = PROBABILITY_INITIAL;
            probabilities[base + LEN_CHOICE2] = PROBABILITY_INITIAL;
            probabilities[base + LEN_LOW..base + LEN_LOW + position_states * LEN_LOW_SYMBOLS]
                .fill(PROBABILITY_INITIAL);
            probabilities[base + LEN_MID..base + LEN_MID + position_states * LEN_MID_SYMBOLS]
                .fill(PROBABILITY_INITIAL);
            probabilities[base + LEN_HIGH..base + LEN_PROBABILITIES].fill(PROBABILITY_INITIAL);
        }
        self.registers = Registers {
            state: State::LiteralLiteral,
            rep0: 0,
            rep1: 0,
            rep2: 0,
            rep3: 0,
        };
        self.range_decoder = RangeDecoder::new();
        self.sequence = Sequence::IsMatch;
    }

    fn finish(&mut self, exit: Exit, produced: usize) -> Result<LzStatus, Error> {
        let mut result = match exit {
            Exit::Suspend(sequence) => {
                self.sequence = sequence;
                Ok(LzStatus::Ok)
            }
            Exit::End => Ok(LzStatus::StreamEnd),
            Exit::Corrupt => Err(Error::Data),
        };
        if let Some(size) = self.uncompressed_size {
            let produced = u64::try_from(produced).expect("a dictionary position fits in 64 bits");
            assert!(produced <= size);
            let remaining = size - produced;
            self.uncompressed_size = Some(remaining);
            if remaining == 0 && result == Ok(LzStatus::Ok) && self.sequence.writes_output() {
                result = Err(Error::Data);
            }
        }
        if result == Ok(LzStatus::StreamEnd) {
            self.range_decoder = RangeDecoder::new();
            self.sequence = Sequence::IsMatch;
        }
        result
    }
}

impl LzDecode for Lzma1Decoder {
    fn decode(
        &mut self,
        dictionary: &mut Dictionary,
        input: &[u8],
        input_position: &mut usize,
    ) -> Result<LzStatus, Error> {
        if self.range_decoder.read_init(input, input_position)? == Initialization::NeedsInput {
            return Ok(LzStatus::Ok);
        }
        let start = dictionary.position;
        let limit = dictionary.limit;
        let mut might_finish_without_end_marker = false;
        if let Some(size) = self.uncompressed_size
            && size <= u64::try_from(limit - start).expect("a dictionary span fits in 64 bits")
        {
            dictionary.limit = start
                + usize::try_from(size).expect("a size below a dictionary span fits in usize");
            might_finish_without_end_marker = true;
        }
        let fast_end = if input.len() - *input_position <= FAST_INPUT_REQUIRED_BYTES {
            *input_position
        } else {
            input.len() - FAST_INPUT_REQUIRED_BYTES
        };
        let mut session = Session {
            probabilities: &mut self.probabilities,
            dictionary,
            rc: self.range_decoder.cursor(input, *input_position),
            registers: self.registers,
            parameters: self.parameters,
            might_finish_without_end_marker,
            end_marker_valid: self.uncompressed_size.is_none(),
            allow_end_marker: self.allow_end_marker,
        };
        let exit = session.run(self.sequence, fast_end);
        let rc = session.rc;
        self.registers = session.registers;
        self.range_decoder.save(&rc);
        *input_position = rc.position;
        dictionary.limit = limit;
        let produced = dictionary.position - start;
        self.finish(exit, produced)
    }
}

struct Session<'a> {
    probabilities: &'a mut Probabilities,
    dictionary: &'a mut Dictionary,
    rc: RangeCursor<'a>,
    registers: Registers,
    parameters: Parameters,
    might_finish_without_end_marker: bool,
    end_marker_valid: bool,
    allow_end_marker: bool,
}

impl Session<'_> {
    fn run(&mut self, resume: Sequence, fast_end: usize) -> Exit {
        let mut entry = Some(resume);
        loop {
            let sequence = match entry.take() {
                Some(sequence) => sequence,
                None => match fast::decode(
                    self.probabilities,
                    self.dictionary,
                    &mut self.rc,
                    &mut self.registers,
                    self.parameters,
                    fast_end,
                ) {
                    FastExit::Slow => Sequence::IsMatch,
                    FastExit::Copy(len) => return Exit::Suspend(Sequence::Copy { len }),
                    FastExit::EndMarker if self.end_marker_valid => Sequence::EndMarker,
                    FastExit::EndMarker | FastExit::Corrupt => return Exit::Corrupt,
                },
            };
            if let Some(exit) = self.decode_slow(sequence) {
                return exit;
            }
        }
    }

    #[inline(always)]
    const fn position_state(&self) -> usize {
        self.parameters.position_state(self.dictionary)
    }

    fn literal_probabilities(&self) -> usize {
        self.parameters.literal_probabilities(self.dictionary)
    }
}
