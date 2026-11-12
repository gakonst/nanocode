use super::super::super::lzma_common::{
    ALIGN_BITS, ALIGN_SIZE, DIST_MODEL_END, DIST_MODEL_START, DIST_SLOTS, LEN_HIGH_SYMBOLS,
    LEN_LOW_SYMBOLS, LEN_MID_SYMBOLS, MATCH_LEN_MIN, POSITION_STATES_MAX, dist_state,
};
use super::{
    DIST_ALIGN, DIST_SLOT, DIST_SPECIAL, END_MARKER_DISTANCE, Exit, IS_MATCH, IS_REP, IS_REP0,
    IS_REP0_LONG, IS_REP1, IS_REP2, LEN_CHOICE, LEN_CHOICE2, LEN_HIGH, LEN_HIGH_MIN, LEN_LOW,
    LEN_MID, LEN_MID_MIN, LITERAL_MATCH_OFFSET, LITERAL_SYMBOLS, LengthKind, Sequence, Session,
};

enum Step {
    Continue(Sequence),
    Done,
    Exit(Exit),
}

const fn suspend(sequence: Sequence) -> Step {
    Step::Exit(Exit::Suspend(sequence))
}

impl Session<'_> {
    pub(super) fn decode_slow(&mut self, entry: Sequence) -> Option<Exit> {
        let mut sequence = entry;
        loop {
            match self.step(sequence) {
                Step::Continue(next) => sequence = next,
                Step::Done => return None,
                Step::Exit(exit) => return Some(exit),
            }
        }
    }

    fn step(&mut self, sequence: Sequence) -> Step {
        match sequence {
            Sequence::IsMatch => self.is_match(),
            Sequence::Literal {
                probabilities,
                symbol,
            } => self.literal(probabilities, symbol),
            Sequence::LiteralMatched {
                probabilities,
                symbol,
                match_byte,
                offset,
            } => self.literal_matched(probabilities, symbol, match_byte, offset),
            Sequence::LiteralWrite { symbol } => self.literal_write(symbol),
            Sequence::IsRep => self.is_rep(),
            Sequence::LenChoice(kind) => self.len_choice(kind),
            Sequence::LenChoice2(kind) => self.len_choice2(kind),
            Sequence::LenBittree {
                kind,
                probabilities,
                limit,
                symbol,
                len,
            } => self.len_bittree(kind, probabilities, limit, symbol, len),
            Sequence::DistSlot {
                probabilities,
                symbol,
                len,
            } => self.dist_slot(probabilities, symbol, len),
            Sequence::DistModel {
                probabilities,
                symbol,
                offset,
                limit,
                len,
            } => self.dist_model(probabilities, symbol, offset, limit, len),
            Sequence::Direct { limit, len } => self.direct(limit, len),
            Sequence::Align {
                symbol,
                offset,
                len,
            } => self.align(symbol, offset, len),
            Sequence::EndMarker => self.end_marker(),
            Sequence::IsRep0 => self.is_rep0(),
            Sequence::IsRep0Long => self.is_rep0_long(),
            Sequence::ShortRep => self.short_rep(),
            Sequence::IsRep1 => self.is_rep1(),
            Sequence::IsRep2 => self.is_rep2(),
            Sequence::Copy { len } => self.copy(len),
        }
    }

    fn bit(&mut self, index: usize) -> Option<bool> {
        self.rc.bit_safe(&mut self.probabilities[index])
    }

    fn is_match(&mut self) -> Step {
        if self.might_finish_without_end_marker && self.dictionary.is_at_limit() {
            if !self.rc.normalize_safe() {
                return suspend(Sequence::IsMatch);
            }
            if self.rc.is_finished() {
                return Step::Exit(Exit::End);
            }
            if !self.allow_end_marker {
                return Step::Exit(Exit::Corrupt);
            }
            self.end_marker_valid = true;
        }
        let state = self.registers.state;
        let index = IS_MATCH + state.index() * POSITION_STATES_MAX + self.position_state();
        let Some(bit) = self.bit(index) else {
            return suspend(Sequence::IsMatch);
        };
        if bit {
            return Step::Continue(Sequence::IsRep);
        }
        let probabilities = self.literal_probabilities();
        self.registers.state = state.after_literal();
        if state.is_literal() {
            return Step::Continue(Sequence::Literal {
                probabilities,
                symbol: 1,
            });
        }
        Step::Continue(Sequence::LiteralMatched {
            probabilities,
            symbol: 1,
            match_byte: u32::from(self.dictionary.get(self.registers.rep0)) << 1,
            offset: LITERAL_MATCH_OFFSET,
        })
    }

    fn literal(&mut self, probabilities: usize, symbol: u32) -> Step {
        let mut symbol = symbol;
        while symbol < LITERAL_SYMBOLS {
            let Some(bit) = self.bit(probabilities + symbol as usize) else {
                return suspend(Sequence::Literal {
                    probabilities,
                    symbol,
                });
            };
            symbol = (symbol << 1) | u32::from(bit);
        }
        Step::Continue(Sequence::LiteralWrite { symbol })
    }

    fn literal_matched(
        &mut self,
        probabilities: usize,
        symbol: u32,
        match_byte: u32,
        offset: u32,
    ) -> Step {
        let (mut symbol, mut match_byte, mut offset) = (symbol, match_byte, offset);
        while symbol < LITERAL_SYMBOLS {
            let match_bit = match_byte & offset;
            let index = probabilities + (offset + match_bit + symbol) as usize;
            let Some(bit) = self.bit(index) else {
                return suspend(Sequence::LiteralMatched {
                    probabilities,
                    symbol,
                    match_byte,
                    offset,
                });
            };
            symbol = (symbol << 1) | u32::from(bit);
            offset &= if bit { match_bit } else { !match_bit };
            match_byte <<= 1;
        }
        Step::Continue(Sequence::LiteralWrite { symbol })
    }

    fn literal_write(&mut self, symbol: u32) -> Step {
        if !self.dictionary.try_put((symbol - LITERAL_SYMBOLS) as u8) {
            return suspend(Sequence::LiteralWrite { symbol });
        }
        Step::Done
    }

    fn is_rep(&mut self) -> Step {
        let state = self.registers.state;
        let Some(bit) = self.bit(IS_REP + state.index()) else {
            return suspend(Sequence::IsRep);
        };
        if !bit {
            self.registers.state = state.after_match();
            self.registers.shift_reps();
            return Step::Continue(Sequence::LenChoice(LengthKind::Match));
        }
        if !self.dictionary.is_distance_valid(0) {
            return Step::Exit(Exit::Corrupt);
        }
        Step::Continue(Sequence::IsRep0)
    }

    fn len_choice(&mut self, kind: LengthKind) -> Step {
        let base = kind.base();
        let Some(bit) = self.bit(base + LEN_CHOICE) else {
            return suspend(Sequence::LenChoice(kind));
        };
        if bit {
            return Step::Continue(Sequence::LenChoice2(kind));
        }
        Step::Continue(Sequence::LenBittree {
            kind,
            probabilities: base + LEN_LOW + self.position_state() * LEN_LOW_SYMBOLS,
            limit: LEN_LOW_SYMBOLS as u32,
            symbol: 1,
            len: MATCH_LEN_MIN as u32,
        })
    }

    fn len_choice2(&mut self, kind: LengthKind) -> Step {
        let base = kind.base();
        let Some(bit) = self.bit(base + LEN_CHOICE2) else {
            return suspend(Sequence::LenChoice2(kind));
        };
        let (probabilities, limit, len) = if bit {
            (base + LEN_HIGH, LEN_HIGH_SYMBOLS, LEN_HIGH_MIN)
        } else {
            let mid = base + LEN_MID + self.position_state() * LEN_MID_SYMBOLS;
            (mid, LEN_MID_SYMBOLS, LEN_MID_MIN)
        };
        Step::Continue(Sequence::LenBittree {
            kind,
            probabilities,
            limit: limit as u32,
            symbol: 1,
            len,
        })
    }

    fn len_bittree(
        &mut self,
        kind: LengthKind,
        probabilities: usize,
        limit: u32,
        symbol: u32,
        len: u32,
    ) -> Step {
        let mut symbol = symbol;
        while symbol < limit {
            let Some(bit) = self.bit(probabilities + symbol as usize) else {
                return suspend(Sequence::LenBittree {
                    kind,
                    probabilities,
                    limit,
                    symbol,
                    len,
                });
            };
            symbol = (symbol << 1) | u32::from(bit);
        }
        let len = len + symbol - limit;
        match kind {
            LengthKind::Match => Step::Continue(Sequence::DistSlot {
                probabilities: DIST_SLOT + dist_state(len) * DIST_SLOTS,
                symbol: 1,
                len,
            }),
            LengthKind::Rep => Step::Continue(Sequence::Copy { len }),
        }
    }

    fn dist_slot(&mut self, probabilities: usize, symbol: u32, len: u32) -> Step {
        let mut symbol = symbol;
        while symbol < DIST_SLOTS as u32 {
            let Some(bit) = self.bit(probabilities + symbol as usize) else {
                return suspend(Sequence::DistSlot {
                    probabilities,
                    symbol,
                    len,
                });
            };
            symbol = (symbol << 1) | u32::from(bit);
        }
        let symbol = symbol - DIST_SLOTS as u32;
        if symbol < DIST_MODEL_START {
            self.registers.rep0 = symbol;
            return self.validate_distance(len);
        }
        let limit = (symbol >> 1) - 1;
        self.registers.rep0 = 2 + (symbol & 1);
        if symbol < DIST_MODEL_END {
            self.registers.rep0 <<= limit;
            return Step::Continue(Sequence::DistModel {
                probabilities: DIST_SPECIAL + self.registers.rep0 as usize - symbol as usize - 1,
                symbol: 1,
                offset: 0,
                limit,
                len,
            });
        }
        Step::Continue(Sequence::Direct {
            limit: limit - ALIGN_BITS,
            len,
        })
    }

    fn dist_model(
        &mut self,
        probabilities: usize,
        symbol: u32,
        offset: u32,
        limit: u32,
        len: u32,
    ) -> Step {
        let (mut symbol, mut offset) = (symbol, offset);
        while offset < limit {
            let Some(bit) = self.bit(probabilities + symbol as usize) else {
                return suspend(Sequence::DistModel {
                    probabilities,
                    symbol,
                    offset,
                    limit,
                    len,
                });
            };
            symbol = (symbol << 1) | u32::from(bit);
            self.registers.rep0 += u32::from(bit) << offset;
            offset += 1;
        }
        self.validate_distance(len)
    }

    fn direct(&mut self, limit: u32, len: u32) -> Step {
        let mut limit = limit;
        while limit > 0 {
            let Some(rep0) = self.rc.direct_safe(self.registers.rep0) else {
                return suspend(Sequence::Direct { limit, len });
            };
            self.registers.rep0 = rep0;
            limit -= 1;
        }
        self.registers.rep0 <<= ALIGN_BITS;
        Step::Continue(Sequence::Align {
            symbol: 0,
            offset: 1,
            len,
        })
    }

    fn align(&mut self, symbol: u32, offset: u32, len: u32) -> Step {
        let (mut symbol, mut offset) = (symbol, offset);
        while offset < ALIGN_SIZE as u32 {
            let Some(bit) = self.bit(DIST_ALIGN + (offset + symbol) as usize) else {
                return suspend(Sequence::Align {
                    symbol,
                    offset,
                    len,
                });
            };
            if bit {
                symbol += offset;
            }
            offset <<= 1;
        }
        self.registers.rep0 += symbol;
        if self.registers.rep0 == END_MARKER_DISTANCE {
            if !self.end_marker_valid {
                return Step::Exit(Exit::Corrupt);
            }
            return Step::Continue(Sequence::EndMarker);
        }
        self.validate_distance(len)
    }

    fn end_marker(&mut self) -> Step {
        if !self.rc.normalize_safe() {
            return suspend(Sequence::EndMarker);
        }
        if self.rc.is_finished() {
            Step::Exit(Exit::End)
        } else {
            Step::Exit(Exit::Corrupt)
        }
    }

    const fn validate_distance(&self, len: u32) -> Step {
        if !self.dictionary.is_distance_valid(self.registers.rep0) {
            return Step::Exit(Exit::Corrupt);
        }
        Step::Continue(Sequence::Copy { len })
    }

    fn is_rep0(&mut self) -> Step {
        let Some(bit) = self.bit(IS_REP0 + self.registers.state.index()) else {
            return suspend(Sequence::IsRep0);
        };
        if bit {
            Step::Continue(Sequence::IsRep1)
        } else {
            Step::Continue(Sequence::IsRep0Long)
        }
    }

    fn is_rep0_long(&mut self) -> Step {
        let state = self.registers.state;
        let index = IS_REP0_LONG + state.index() * POSITION_STATES_MAX + self.position_state();
        let Some(bit) = self.bit(index) else {
            return suspend(Sequence::IsRep0Long);
        };
        if bit {
            return self.long_rep();
        }
        self.registers.state = state.after_short_rep();
        Step::Continue(Sequence::ShortRep)
    }

    fn short_rep(&mut self) -> Step {
        let byte = self.dictionary.get(self.registers.rep0);
        if !self.dictionary.try_put(byte) {
            return suspend(Sequence::ShortRep);
        }
        Step::Done
    }

    fn is_rep1(&mut self) -> Step {
        let Some(bit) = self.bit(IS_REP1 + self.registers.state.index()) else {
            return suspend(Sequence::IsRep1);
        };
        if bit {
            return Step::Continue(Sequence::IsRep2);
        }
        core::mem::swap(&mut self.registers.rep1, &mut self.registers.rep0);
        self.long_rep()
    }

    fn is_rep2(&mut self) -> Step {
        let Some(bit) = self.bit(IS_REP2 + self.registers.state.index()) else {
            return suspend(Sequence::IsRep2);
        };
        let distance = if bit {
            let distance = self.registers.rep3;
            self.registers.rep3 = self.registers.rep2;
            distance
        } else {
            self.registers.rep2
        };
        self.registers.rep2 = self.registers.rep1;
        self.registers.rep1 = self.registers.rep0;
        self.registers.rep0 = distance;
        self.long_rep()
    }

    const fn long_rep(&mut self) -> Step {
        self.registers.state = self.registers.state.after_long_rep();
        Step::Continue(Sequence::LenChoice(LengthKind::Rep))
    }

    fn copy(&mut self, len: u32) -> Step {
        let mut len = len;
        if self.dictionary.repeat(self.registers.rep0, &mut len) {
            return suspend(Sequence::Copy { len });
        }
        Step::Done
    }
}
