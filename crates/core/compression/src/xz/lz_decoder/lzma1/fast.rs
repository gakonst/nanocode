use super::super::super::lzma_common::{
    ALIGN_BITS, ALIGN_SIZE, DIST_MODEL_END, DIST_MODEL_START, DIST_SLOTS, LEN_HIGH_SYMBOLS,
    LEN_LOW_SYMBOLS, LEN_MID_SYMBOLS, LITERAL_CODER_SIZE, MATCH_LEN_MIN, POSITION_STATES_MAX,
    dist_state,
};
use super::super::dictionary::Dictionary;
use super::super::range_decoder::RangeCursor;
use super::{
    DIST_ALIGN, DIST_SLOT, DIST_SPECIAL, END_MARKER_DISTANCE, FastExit, IS_MATCH, IS_REP, IS_REP0,
    IS_REP0_LONG, IS_REP1, IS_REP2, LEN_CHOICE, LEN_CHOICE2, LEN_HIGH, LEN_HIGH_MIN, LEN_LOW,
    LEN_MID, LEN_MID_MIN, LITERAL_MATCH_OFFSET, LITERAL_SYMBOLS, LengthKind, Parameters,
    Probabilities, Registers,
};

const LITERAL_BYTE_MASK: usize = LITERAL_SYMBOLS as usize - 1;

#[inline(never)]
pub(super) fn decode(
    probabilities: &mut Probabilities,
    dictionary: &mut Dictionary,
    rc: &mut RangeCursor<'_>,
    registers: &mut Registers,
    parameters: Parameters,
    fast_end: usize,
) -> FastExit {
    let mut fast = Fast {
        probabilities,
        dictionary,
        rc: *rc,
        registers: *registers,
        parameters,
    };
    let exit = fast.run(fast_end);
    *rc = fast.rc;
    *registers = fast.registers;
    exit
}

struct Fast<'a, 'b> {
    probabilities: &'a mut Probabilities,
    dictionary: &'a mut Dictionary,
    rc: RangeCursor<'b>,
    registers: Registers,
    parameters: Parameters,
}

impl Fast<'_, '_> {
    #[inline(always)]
    fn run(&mut self, fast_end: usize) -> FastExit {
        while self.rc.position < fast_end && !self.dictionary.is_at_limit() {
            let position_state = self.parameters.position_state(self.dictionary);
            let state = self.registers.state.index();
            let is_match = IS_MATCH + state * POSITION_STATES_MAX + position_state;
            if !self.rc.bit(&mut self.probabilities[is_match]) {
                self.literal();
                continue;
            }
            let len = if !self.rc.bit(&mut self.probabilities[IS_REP + state]) {
                match self.simple_match(position_state) {
                    Ok(len) => len,
                    Err(exit) => return exit,
                }
            } else {
                match self.rep_match(position_state) {
                    Ok(Some(len)) => len,
                    Ok(None) => continue,
                    Err(exit) => return exit,
                }
            };
            let mut len = len;
            if self.dictionary.repeat(self.registers.rep0, &mut len) {
                return FastExit::Copy(len);
            }
        }
        FastExit::Slow
    }

    #[inline(always)]
    fn bittree<const SYMBOLS: usize>(&mut self, base: usize) -> u32 {
        let probabilities: &mut [u16; SYMBOLS] = (&mut self.probabilities[base..base + SYMBOLS])
            .try_into()
            .expect("a bit tree spans its symbols");
        let mut symbol = 1_usize;
        for _ in 0..SYMBOLS.trailing_zeros() {
            let bit = self.rc.bit(&mut probabilities[symbol & (SYMBOLS - 1)]);
            symbol = (symbol << 1) | usize::from(bit);
        }
        u32::try_from(symbol - SYMBOLS).expect("a bit tree symbol fits in 32 bits")
    }

    #[inline(always)]
    fn literal(&mut self) {
        let base = self.parameters.literal_probabilities(self.dictionary);
        let state = self.registers.state;
        self.registers.state = state.after_literal();
        let probabilities: &mut [u16; LITERAL_CODER_SIZE] = (&mut self.probabilities
            [base..base + LITERAL_CODER_SIZE])
            .try_into()
            .expect("a literal coder spans its probabilities");
        let byte = if state.is_literal() {
            plain_literal(&mut self.rc, probabilities)
        } else {
            let match_byte = self.dictionary.get(self.registers.rep0);
            matched_literal(&mut self.rc, probabilities, match_byte)
        };
        self.dictionary.put(byte);
    }

    #[inline(always)]
    fn len(&mut self, kind: LengthKind, position_state: usize) -> u32 {
        let base = kind.base();
        if !self.rc.bit(&mut self.probabilities[base + LEN_CHOICE]) {
            let low = base + LEN_LOW + position_state * LEN_LOW_SYMBOLS;
            return self.bittree::<LEN_LOW_SYMBOLS>(low) + MATCH_LEN_MIN as u32;
        }
        if !self.rc.bit(&mut self.probabilities[base + LEN_CHOICE2]) {
            let mid = base + LEN_MID + position_state * LEN_MID_SYMBOLS;
            return self.bittree::<LEN_MID_SYMBOLS>(mid) + LEN_MID_MIN;
        }
        self.bittree::<LEN_HIGH_SYMBOLS>(base + LEN_HIGH) + LEN_HIGH_MIN
    }

    #[inline(always)]
    fn simple_match(&mut self, position_state: usize) -> Result<u32, FastExit> {
        self.registers.state = self.registers.state.after_match();
        self.registers.shift_reps();
        let len = self.len(LengthKind::Match, position_state);
        let symbol = self.bittree::<DIST_SLOTS>(DIST_SLOT + dist_state(len) * DIST_SLOTS);
        self.registers.rep0 = if symbol < DIST_MODEL_START {
            symbol
        } else if symbol < DIST_MODEL_END {
            self.model_distance(symbol)
        } else {
            let rep0 = self.direct_distance(symbol);
            if rep0 == END_MARKER_DISTANCE {
                self.registers.rep0 = rep0;
                return Err(FastExit::EndMarker);
            }
            rep0
        };
        if !self.dictionary.is_distance_valid(self.registers.rep0) {
            return Err(FastExit::Corrupt);
        }
        Ok(len)
    }

    #[inline(always)]
    fn model_distance(&mut self, symbol: u32) -> u32 {
        let limit = (symbol >> 1) - 1;
        let mut rep0 = (2 + (symbol & 1)) << limit;
        let base = DIST_SPECIAL + rep0 as usize - symbol as usize - 1;
        let mut reverse = 1_usize;
        for bit_index in 0..limit {
            let bit = self.rc.bit(&mut self.probabilities[base + reverse]);
            reverse = (reverse << 1) | usize::from(bit);
            rep0 += u32::from(bit) << bit_index;
        }
        rep0
    }

    #[inline(always)]
    fn direct_distance(&mut self, symbol: u32) -> u32 {
        let limit = (symbol >> 1) - 1;
        let mut rep0 = 2 + (symbol & 1);
        for _ in 0..limit - ALIGN_BITS {
            rep0 = self.rc.direct(rep0);
        }
        rep0 <<= ALIGN_BITS;
        let mut align = 0_usize;
        let mut offset = 1_usize;
        while offset < ALIGN_SIZE {
            if self
                .rc
                .bit(&mut self.probabilities[DIST_ALIGN + offset + align])
            {
                align += offset;
            }
            offset <<= 1;
        }
        rep0 + u32::try_from(align).expect("an align symbol fits in 32 bits")
    }

    #[inline(always)]
    fn rep_match(&mut self, position_state: usize) -> Result<Option<u32>, FastExit> {
        if !self.dictionary.is_distance_valid(0) {
            return Err(FastExit::Corrupt);
        }
        let state = self.registers.state;
        let index = state.index();
        let registers = &mut self.registers;
        if !self.rc.bit(&mut self.probabilities[IS_REP0 + index]) {
            let long = IS_REP0_LONG + index * POSITION_STATES_MAX + position_state;
            if !self.rc.bit(&mut self.probabilities[long]) {
                registers.state = state.after_short_rep();
                let byte = self.dictionary.get(registers.rep0);
                self.dictionary.put(byte);
                return Ok(None);
            }
        } else if !self.rc.bit(&mut self.probabilities[IS_REP1 + index]) {
            core::mem::swap(&mut registers.rep1, &mut registers.rep0);
        } else {
            let distance = if self.rc.bit(&mut self.probabilities[IS_REP2 + index]) {
                let distance = registers.rep3;
                registers.rep3 = registers.rep2;
                distance
            } else {
                registers.rep2
            };
            registers.rep2 = registers.rep1;
            registers.rep1 = registers.rep0;
            registers.rep0 = distance;
        }
        registers.state = state.after_long_rep();
        Ok(Some(self.len(LengthKind::Rep, position_state)))
    }
}

#[inline(never)]
fn plain_literal(rc: &mut RangeCursor<'_>, probabilities: &mut [u16; LITERAL_CODER_SIZE]) -> u8 {
    let mut cursor = *rc;
    let mut symbol = 1_usize;
    for _ in 0..LITERAL_SYMBOLS.trailing_zeros() {
        let bit = cursor.bit(&mut probabilities[symbol & LITERAL_BYTE_MASK]);
        symbol = (symbol << 1) | usize::from(bit);
    }
    *rc = cursor;
    u8::try_from(symbol & LITERAL_BYTE_MASK).expect("a masked literal fits in a byte")
}

#[inline(never)]
fn matched_literal(
    rc: &mut RangeCursor<'_>,
    probabilities: &mut [u16; LITERAL_CODER_SIZE],
    match_byte: u8,
) -> u8 {
    let mut cursor = *rc;
    let mut match_byte = usize::from(match_byte);
    let mut offset = LITERAL_MATCH_OFFSET as usize;
    let mut symbol = 1_usize;
    for _ in 0..LITERAL_SYMBOLS.trailing_zeros() {
        match_byte <<= 1;
        let match_bit = match_byte & offset;
        let index = offset + match_bit + (symbol & LITERAL_BYTE_MASK);
        let bit = cursor.bit(&mut probabilities[index]);
        symbol = (symbol << 1) | usize::from(bit);
        offset &= if bit { match_bit } else { !match_bit };
    }
    *rc = cursor;
    u8::try_from(symbol & LITERAL_BYTE_MASK).expect("a masked literal fits in a byte")
}
