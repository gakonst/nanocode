pub(crate) const LC_MAX: u32 = 4;
pub(crate) const LP_MAX: u32 = 4;
pub(crate) const PB_MAX: u32 = 4;
pub(crate) const LCLP_MAX: u32 = 4;
pub(crate) const DICT_SIZE_MIN: u32 = 4096;
pub(crate) const STATES: usize = 12;
pub(crate) const LITERAL_STATES: usize = 7;
pub(crate) const POSITION_STATES_MAX: usize = 1 << PB_MAX;
pub(crate) const LITERAL_CODER_SIZE: usize = 0x300;
pub(crate) const LITERAL_CODERS_MAX: usize = 1 << LCLP_MAX;
pub(crate) const MATCH_LEN_MIN: usize = 2;
pub(crate) const LEN_LOW_BITS: u32 = 3;
pub(crate) const LEN_MID_BITS: u32 = 3;
pub(crate) const LEN_HIGH_BITS: u32 = 8;
pub(crate) const LEN_LOW_SYMBOLS: usize = 1 << LEN_LOW_BITS;
pub(crate) const LEN_MID_SYMBOLS: usize = 1 << LEN_MID_BITS;
pub(crate) const LEN_HIGH_SYMBOLS: usize = 1 << LEN_HIGH_BITS;
pub(crate) const LEN_SYMBOLS: usize = LEN_LOW_SYMBOLS + LEN_MID_SYMBOLS + LEN_HIGH_SYMBOLS;
pub(crate) const MATCH_LEN_MAX: usize = MATCH_LEN_MIN + LEN_SYMBOLS - 1;
pub(crate) const DIST_STATES: usize = 4;
pub(crate) const DIST_SLOT_BITS: u32 = 6;
pub(crate) const DIST_SLOTS: usize = 1 << DIST_SLOT_BITS;
pub(crate) const DIST_MODEL_START: u32 = 4;
pub(crate) const DIST_MODEL_END: u32 = 14;
pub(crate) const FULL_DISTANCES_BITS: u32 = DIST_MODEL_END / 2;
pub(crate) const FULL_DISTANCES: usize = 1 << FULL_DISTANCES_BITS;
pub(crate) const ALIGN_BITS: u32 = 4;
pub(crate) const ALIGN_SIZE: usize = 1 << ALIGN_BITS;
pub(crate) const ALIGN_MASK: u32 = (1 << ALIGN_BITS) - 1;
pub(crate) const REPS: usize = 4;
pub(crate) const RANGE_SHIFT_BITS: u32 = 8;
pub(crate) const RANGE_TOP_BITS: u32 = 24;
pub(crate) const RANGE_TOP_VALUE: u32 = 1 << RANGE_TOP_BITS;
pub(crate) const BIT_MODEL_TOTAL_BITS: u32 = 11;
pub(crate) const BIT_MODEL_TOTAL: u32 = 1 << BIT_MODEL_TOTAL_BITS;
pub(crate) const MOVE_BITS: u32 = 5;
pub(crate) const PROBABILITY_INITIAL: u16 = (BIT_MODEL_TOTAL / 2) as u16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    LiteralLiteral,
    MatchLiteralLiteral,
    RepLiteralLiteral,
    ShortRepLiteralLiteral,
    MatchLiteral,
    RepLiteral,
    ShortRepLiteral,
    LiteralMatch,
    LiteralLongRep,
    LiteralShortRep,
    NonLiteralMatch,
    NonLiteralRep,
}

impl State {
    pub(crate) const fn index(self) -> usize {
        self as usize
    }

    pub(crate) const fn is_literal(self) -> bool {
        self.index() < LITERAL_STATES
    }

    pub(crate) const fn after_literal(self) -> Self {
        match self {
            Self::LiteralLiteral
            | Self::MatchLiteralLiteral
            | Self::RepLiteralLiteral
            | Self::ShortRepLiteralLiteral => Self::LiteralLiteral,
            Self::MatchLiteral => Self::MatchLiteralLiteral,
            Self::RepLiteral => Self::RepLiteralLiteral,
            Self::ShortRepLiteral => Self::ShortRepLiteralLiteral,
            Self::LiteralMatch | Self::NonLiteralMatch => Self::MatchLiteral,
            Self::LiteralLongRep | Self::NonLiteralRep => Self::RepLiteral,
            Self::LiteralShortRep => Self::ShortRepLiteral,
        }
    }

    pub(crate) const fn after_match(self) -> Self {
        if self.is_literal() {
            Self::LiteralMatch
        } else {
            Self::NonLiteralMatch
        }
    }

    pub(crate) const fn after_long_rep(self) -> Self {
        if self.is_literal() {
            Self::LiteralLongRep
        } else {
            Self::NonLiteralRep
        }
    }

    pub(crate) const fn after_short_rep(self) -> Self {
        if self.is_literal() {
            Self::LiteralShortRep
        } else {
            Self::NonLiteralRep
        }
    }
}

pub(crate) const fn literal_mask(lc: u32, lp: u32) -> u32 {
    (0x100 << lp) - (0x100 >> lc)
}

pub(crate) const fn literal_subcoder_offset(
    lc: u32,
    literal_mask: u32,
    position: u32,
    previous_byte: u8,
) -> usize {
    (3 * ((((position << 8) + previous_byte as u32) & literal_mask) << lc)) as usize
}

pub(crate) const fn dist_state(len: u32) -> usize {
    let state = len as usize - MATCH_LEN_MIN;
    if state < DIST_STATES - 1 {
        state
    } else {
        DIST_STATES - 1
    }
}

pub(crate) const fn is_lclppb_valid(lc: u32, lp: u32, pb: u32) -> bool {
    lc <= LC_MAX && lp <= LP_MAX && lc + lp <= LCLP_MAX && pb <= PB_MAX
}
