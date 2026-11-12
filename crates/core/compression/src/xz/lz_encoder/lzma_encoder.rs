use alloc::boxed::Box;

use super::super::lzma_common::{
    ALIGN_BITS, ALIGN_MASK, ALIGN_SIZE, DIST_MODEL_END, DIST_MODEL_START, DIST_SLOT_BITS,
    DIST_SLOTS, DIST_STATES, FULL_DISTANCES, LEN_HIGH_BITS, LEN_HIGH_SYMBOLS, LEN_LOW_BITS,
    LEN_LOW_SYMBOLS, LEN_MID_BITS, LEN_MID_SYMBOLS, LITERAL_CODER_SIZE, LITERAL_CODERS_MAX,
    MATCH_LEN_MAX, MATCH_LEN_MIN, POSITION_STATES_MAX, PROBABILITY_INITIAL, REPS, STATES, State,
    dist_state, is_lclppb_valid, literal_mask, literal_subcoder_offset,
};
use super::super::options::{LzmaOptions, Mode};
use super::super::stream::{Action, Error, Status};
use super::match_finder::{DICT_SIZE_MAX, Match, MatchFinder, Settings, hash_bytes};
use super::optimum_normal::NormalParser;
use super::range_encoder::RangeEncoder;

pub(super) const OPTS: u32 = 1 << 12;
pub(super) const LOOP_INPUT_MAX: u32 = OPTS + 1;
pub(super) const MATCHES_MAX: usize = MATCH_LEN_MAX + 1;
pub(super) const MATCH_LEN_MIN_U32: u32 = MATCH_LEN_MIN as u32;
pub(super) const MATCH_LEN_MAX_U32: u32 = MATCH_LEN_MAX as u32;
pub(super) const REPS_U32: u32 = REPS as u32;
pub(super) const BACK_LITERAL: u32 = u32::MAX;
pub(super) const LZMA2_CHUNK_MAX: u32 = 1 << 16;
const END_MARKER_DISTANCE: u32 = u32::MAX;
const LITERAL_BITS: u32 = 8;
const LITERAL_SYMBOL_LIMIT: u32 = 1 << 16;
const LITERAL_OFFSET_INITIAL: u32 = 0x100;
const LITERAL_PROBABILITIES: usize = LITERAL_CODER_SIZE * LITERAL_CODERS_MAX;
pub(super) const IS_MATCH: usize = LITERAL_PROBABILITIES;
pub(super) const IS_REP: usize = IS_MATCH + STATES * POSITION_STATES_MAX;
pub(super) const IS_REP0: usize = IS_REP + STATES;
pub(super) const IS_REP1: usize = IS_REP0 + STATES;
pub(super) const IS_REP2: usize = IS_REP1 + STATES;
pub(super) const IS_REP0_LONG: usize = IS_REP2 + STATES;
pub(super) const DIST_SLOT: usize = IS_REP0_LONG + STATES * POSITION_STATES_MAX;
pub(super) const DIST_SPECIAL: usize = DIST_SLOT + DIST_STATES * DIST_SLOTS;
pub(super) const DIST_ALIGN: usize = DIST_SPECIAL + FULL_DISTANCES - DIST_MODEL_END as usize;
pub(super) const MATCH_LENGTH: usize = DIST_ALIGN + ALIGN_SIZE;
pub(super) const REP_LENGTH: usize = MATCH_LENGTH + LENGTH_PROBABILITIES;
pub(super) const PROBABILITIES: usize = REP_LENGTH + LENGTH_PROBABILITIES;
pub(super) const LENGTH_CHOICE: usize = 0;
pub(super) const LENGTH_CHOICE2: usize = 1;
pub(super) const LENGTH_LOW: usize = 2;
pub(super) const LENGTH_MID: usize = LENGTH_LOW + POSITION_STATES_MAX * LEN_LOW_SYMBOLS;
pub(super) const LENGTH_HIGH: usize = LENGTH_MID + POSITION_STATES_MAX * LEN_MID_SYMBOLS;
const LENGTH_PROBABILITIES: usize = LENGTH_HIGH + LEN_HIGH_SYMBOLS;

pub(super) struct Model {
    pub(super) probabilities: [u16; PROBABILITIES],
    pub(super) lc: u32,
    pub(super) literal_mask: u32,
    pub(super) pos_mask: u32,
}

impl Model {
    pub(super) const fn literal_base(&self, position: u32, previous_byte: u8) -> usize {
        literal_subcoder_offset(self.lc, self.literal_mask, position, previous_byte)
    }

    pub(super) const fn pos_state(&self, position: u32) -> usize {
        (position & self.pos_mask) as usize
    }
}

pub(super) const fn is_match(state: State, pos_state: usize) -> usize {
    IS_MATCH + state.index() * POSITION_STATES_MAX + pos_state
}

pub(super) const fn is_rep0_long(state: State, pos_state: usize) -> usize {
    IS_REP0_LONG + state.index() * POSITION_STATES_MAX + pos_state
}

pub(super) const fn dist_slot_base(dist_state: usize) -> usize {
    DIST_SLOT + dist_state * DIST_SLOTS
}

pub(super) const fn dist_special_base(base: u32, dist_slot: u32) -> usize {
    DIST_SPECIAL + base as usize - dist_slot as usize - 1
}

pub(super) const fn dist_footer(dist_slot: u32) -> (u32, u32) {
    let footer_bits = (dist_slot >> 1) - 1;
    (footer_bits, (2 | (dist_slot & 1)) << footer_bits)
}

#[derive(Clone, Copy)]
pub(super) enum LengthKind {
    Match,
    Rep,
}

impl LengthKind {
    pub(super) const fn base(self) -> usize {
        match self {
            Self::Match => MATCH_LENGTH,
            Self::Rep => REP_LENGTH,
        }
    }
}

pub(super) enum Parser {
    Fast,
    Normal(Box<NormalParser>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Uninitialized,
    Encoding,
    Flushed,
}

pub(super) struct LzmaEncoder {
    pub(super) rc: RangeEncoder,
    pub(super) model: Model,
    pub(super) state: State,
    pub(super) reps: [u32; REPS],
    pub(super) matches: [Match; MATCHES_MAX],
    pub(super) matches_count: u32,
    pub(super) longest_match_length: u32,
    pub(super) parser: Parser,
    phase: Phase,
    writes_end_marker: bool,
    uncompressed_size: u64,
    options: LzmaOptions,
}

const fn is_options_valid(options: &LzmaOptions) -> bool {
    is_lclppb_valid(options.lc, options.lp, options.pb)
        && options.nice_len >= MATCH_LEN_MIN_U32
        && options.nice_len <= MATCH_LEN_MAX_U32
}

impl LzmaEncoder {
    pub(super) fn new(
        options: &LzmaOptions,
        writes_end_marker: bool,
    ) -> Result<(Box<Self>, Settings), Error> {
        if !is_options_valid(options) {
            return Err(Error::Options);
        }
        let nice_len = hash_bytes(options.match_finder).max(options.nice_len);
        let parser = match options.mode {
            Mode::Fast => Parser::Fast,
            Mode::Normal => {
                if options.dict_size > DICT_SIZE_MAX {
                    return Err(Error::Options);
                }
                let log_size = options.dict_size.next_power_of_two().trailing_zeros();
                Parser::Normal(NormalParser::new(
                    log_size * 2,
                    nice_len + 1 - MATCH_LEN_MIN_U32,
                ))
            }
        };
        let mut encoder = Box::new(Self {
            rc: RangeEncoder::new(),
            model: Model {
                probabilities: [PROBABILITY_INITIAL; PROBABILITIES],
                lc: options.lc,
                literal_mask: literal_mask(options.lc, options.lp),
                pos_mask: (1 << options.pb) - 1,
            },
            state: State::LiteralLiteral,
            reps: [0; REPS],
            matches: [Match::EMPTY; MATCHES_MAX],
            matches_count: 0,
            longest_match_length: 0,
            parser,
            phase: Phase::Uninitialized,
            writes_end_marker,
            uncompressed_size: 0,
            options: *options,
        });
        encoder.reset();
        let settings = Settings {
            before_size: OPTS,
            dict_size: options.dict_size,
            after_size: LOOP_INPUT_MAX,
            match_len_max: MATCH_LEN_MAX_U32,
            nice_len,
            match_finder: options.match_finder,
            depth: options.depth,
        };
        Ok((encoder, settings))
    }

    pub(super) fn reset(&mut self) {
        let options = self.options;
        self.model.pos_mask = (1 << options.pb) - 1;
        self.model.lc = options.lc;
        self.model.literal_mask = literal_mask(options.lc, options.lp);
        self.rc.reset();
        self.state = State::LiteralLiteral;
        self.reps = [0; REPS];
        self.model.probabilities.fill(PROBABILITY_INITIAL);
        if let Parser::Normal(normal) = &mut self.parser {
            normal.reset(&self.model.probabilities, 1 << options.pb);
        }
    }

    fn position(&self) -> u32 {
        u32::try_from(self.uncompressed_size & u64::from(u32::MAX))
            .expect("the low 32 bits of a 64-bit value fit in 32 bits")
    }

    pub(super) fn encode(
        &mut self,
        mf: &mut MatchFinder,
        output: &mut [u8],
        output_position: &mut usize,
        limit: Option<u32>,
    ) -> Status {
        if self.phase == Phase::Uninitialized && !self.encode_init(mf) {
            return Status::Ok;
        }
        if self
            .rc
            .encode(&mut self.model.probabilities, output, output_position)
        {
            return Status::Ok;
        }
        if self.phase == Phase::Flushed {
            return Status::StreamEnd;
        }
        loop {
            if let Some(limit) = limit {
                let chunk_full = *output_position as u64 + self.rc.pending()
                    >= u64::from(LZMA2_CHUNK_MAX - LOOP_INPUT_MAX);
                if mf.read_pos - mf.read_ahead >= limit || chunk_full {
                    break;
                }
            }
            if mf.read_pos >= mf.read_limit {
                if mf.action == Action::Run {
                    return Status::Ok;
                }
                if mf.read_ahead == 0 {
                    break;
                }
            }
            let position = self.position();
            let (back, len) = match self.parser {
                Parser::Fast => self.optimum_fast(mf),
                Parser::Normal(_) => self.optimum_normal(mf, position),
            };
            self.encode_symbol(mf, back, len, position);
            self.uncompressed_size += u64::from(len);
            if self
                .rc
                .encode(&mut self.model.probabilities, output, output_position)
            {
                return Status::Ok;
            }
        }
        self.finish(output, output_position)
    }

    fn finish(&mut self, output: &mut [u8], output_position: &mut usize) -> Status {
        if self.writes_end_marker {
            let pos_state = self.model.pos_state(self.position());
            self.rc.bit(is_match(self.state, pos_state), 1);
            self.rc.bit(IS_REP + self.state.index(), 0);
            self.encode_match(pos_state, END_MARKER_DISTANCE, MATCH_LEN_MIN_U32);
        }
        self.rc.flush();
        if self
            .rc
            .encode(&mut self.model.probabilities, output, output_position)
        {
            self.phase = Phase::Flushed;
            return Status::Ok;
        }
        Status::StreamEnd
    }

    fn encode_init(&mut self, mf: &mut MatchFinder) -> bool {
        if mf.read_pos == mf.read_limit {
            if mf.action == Action::Run {
                return false;
            }
        } else {
            mf.skip(1);
            mf.read_ahead = 0;
            self.rc.bit(is_match(self.state, 0), 0);
            self.rc.bittree(0, LITERAL_BITS, u32::from(mf.buffer()[0]));
            self.uncompressed_size += 1;
        }
        self.phase = Phase::Encoding;
        true
    }

    fn encode_symbol(&mut self, mf: &mut MatchFinder, back: u32, len: u32, position: u32) {
        let pos_state = self.model.pos_state(position);
        if back == BACK_LITERAL {
            self.rc.bit(is_match(self.state, pos_state), 0);
            self.encode_literal(mf, position);
        } else {
            self.rc.bit(is_match(self.state, pos_state), 1);
            if back < REPS_U32 {
                self.rc.bit(IS_REP + self.state.index(), 1);
                self.encode_rep_match(pos_state, back, len);
            } else {
                self.rc.bit(IS_REP + self.state.index(), 0);
                self.encode_match(pos_state, back - REPS_U32, len);
            }
        }
        mf.read_ahead -= len;
    }

    fn encode_literal(&mut self, mf: &MatchFinder, position: u32) {
        let index = (mf.read_pos - mf.read_ahead) as usize;
        let buffer = mf.buffer();
        let current_byte = buffer[index];
        let base = self.model.literal_base(position, buffer[index - 1]);
        if self.state.is_literal() {
            self.state = self.state.after_literal();
            self.rc.bittree(base, LITERAL_BITS, u32::from(current_byte));
        } else {
            self.state = self.state.after_literal();
            let match_byte = buffer[index - self.reps[0] as usize - 1];
            self.encode_matched_literal(base, u32::from(match_byte), u32::from(current_byte));
        }
    }

    const fn encode_matched_literal(&mut self, base: usize, match_byte: u32, symbol: u32) {
        let mut match_byte = match_byte;
        let mut offset = LITERAL_OFFSET_INITIAL;
        let mut symbol = symbol + (1 << LITERAL_BITS);
        while symbol < LITERAL_SYMBOL_LIMIT {
            match_byte <<= 1;
            let match_bit = match_byte & offset;
            let subcoder_index = offset + match_bit + (symbol >> LITERAL_BITS);
            let bit = (symbol >> (LITERAL_BITS - 1)) & 1;
            self.rc.bit(base + subcoder_index as usize, bit);
            symbol <<= 1;
            offset &= !(match_byte ^ symbol);
        }
    }

    fn encode_length(&mut self, kind: LengthKind, pos_state: usize, len: u32) {
        let base = kind.base();
        let mut len = len - MATCH_LEN_MIN_U32;
        if len < LEN_LOW_SYMBOLS as u32 {
            self.rc.bit(base + LENGTH_CHOICE, 0);
            let low = base + LENGTH_LOW + pos_state * LEN_LOW_SYMBOLS;
            self.rc.bittree(low, LEN_LOW_BITS, len);
        } else {
            self.rc.bit(base + LENGTH_CHOICE, 1);
            len -= LEN_LOW_SYMBOLS as u32;
            if len < LEN_MID_SYMBOLS as u32 {
                self.rc.bit(base + LENGTH_CHOICE2, 0);
                let mid = base + LENGTH_MID + pos_state * LEN_MID_SYMBOLS;
                self.rc.bittree(mid, LEN_MID_BITS, len);
            } else {
                self.rc.bit(base + LENGTH_CHOICE2, 1);
                len -= LEN_MID_SYMBOLS as u32;
                self.rc.bittree(base + LENGTH_HIGH, LEN_HIGH_BITS, len);
            }
        }
        if let Parser::Normal(normal) = &mut self.parser {
            normal.length_encoded(&self.model.probabilities, kind, pos_state);
        }
    }

    fn encode_match(&mut self, pos_state: usize, distance: u32, len: u32) {
        self.state = self.state.after_match();
        self.encode_length(LengthKind::Match, pos_state, len);
        let dist_slot = super::price::dist_slot(distance);
        self.rc
            .bittree(dist_slot_base(dist_state(len)), DIST_SLOT_BITS, dist_slot);
        if dist_slot >= DIST_MODEL_START {
            let (footer_bits, base) = dist_footer(dist_slot);
            let dist_reduced = distance - base;
            if dist_slot < DIST_MODEL_END {
                self.rc.bittree_reverse(
                    dist_special_base(base, dist_slot),
                    footer_bits,
                    dist_reduced,
                );
            } else {
                self.rc
                    .direct(dist_reduced >> ALIGN_BITS, footer_bits - ALIGN_BITS);
                self.rc
                    .bittree_reverse(DIST_ALIGN, ALIGN_BITS, dist_reduced & ALIGN_MASK);
                if let Parser::Normal(normal) = &mut self.parser {
                    normal.align_price_count += 1;
                }
            }
        }
        self.reps = [distance, self.reps[0], self.reps[1], self.reps[2]];
        if let Parser::Normal(normal) = &mut self.parser {
            normal.match_price_count += 1;
        }
    }

    fn encode_rep_match(&mut self, pos_state: usize, rep: u32, len: u32) {
        let state = self.state.index();
        if rep == 0 {
            self.rc.bit(IS_REP0 + state, 0);
            self.rc
                .bit(is_rep0_long(self.state, pos_state), u32::from(len != 1));
        } else {
            let distance = self.reps[rep as usize];
            self.rc.bit(IS_REP0 + state, 1);
            if rep == 1 {
                self.rc.bit(IS_REP1 + state, 0);
            } else {
                self.rc.bit(IS_REP1 + state, 1);
                self.rc.bit(IS_REP2 + state, rep - 2);
                if rep == 3 {
                    self.reps[3] = self.reps[2];
                }
                self.reps[2] = self.reps[1];
            }
            self.reps[1] = self.reps[0];
            self.reps[0] = distance;
        }
        if len == 1 {
            self.state = self.state.after_short_rep();
        } else {
            self.encode_length(LengthKind::Rep, pos_state, len);
            self.state = self.state.after_long_rep();
        }
    }
}
