use alloc::boxed::Box;
use alloc::vec;

use super::super::lzma_common::{
    ALIGN_BITS, ALIGN_MASK, ALIGN_SIZE, DIST_MODEL_END, DIST_MODEL_START, DIST_SLOT_BITS,
    DIST_SLOTS, DIST_STATES, FULL_DISTANCES, LEN_HIGH_BITS, LEN_LOW_BITS, LEN_LOW_SYMBOLS,
    LEN_MID_BITS, LEN_MID_SYMBOLS, LEN_SYMBOLS, POSITION_STATES_MAX, REPS, State, dist_state,
};
use super::lzma_encoder::{
    BACK_LITERAL, DIST_ALIGN, IS_REP, IS_REP0, IS_REP1, IS_REP2, LENGTH_CHOICE, LENGTH_CHOICE2,
    LENGTH_HIGH, LENGTH_LOW, LENGTH_MID, LengthKind, LzmaEncoder, MATCH_LEN_MIN_U32, Model, OPTS,
    Parser, dist_footer, dist_slot_base, dist_special_base, is_match, is_rep0_long,
};
use super::match_finder::{MatchFinder, not_equal_16};
use super::price::{
    bit_0_price, bit_1_price, bit_price, bittree_price, bittree_reverse_price, direct_price,
    dist_slot,
};

mod helper;

const DIST_PRICES_UPDATE_COUNT: u32 = 1 << 7;
const PRICE_COUNT_INITIAL: u32 = u32::MAX / 2;
const LITERAL_BITS: u32 = 8;
const LITERAL_SYMBOL_LIMIT: u32 = 1 << 16;
const LITERAL_OFFSET_INITIAL: u32 = 0x100;
const OPTS_COUNT: usize = OPTS as usize;

#[derive(Clone, Copy)]
pub(super) struct Optimal {
    state: State,
    prev_1_is_literal: bool,
    prev_2: bool,
    pos_prev_2: u32,
    back_prev_2: u32,
    price: u32,
    pos_prev: u32,
    back_prev: u32,
    backs: [u32; REPS],
}

impl Optimal {
    const INITIAL: Self = Self {
        state: State::LiteralLiteral,
        prev_1_is_literal: false,
        prev_2: false,
        pos_prev_2: 0,
        back_prev_2: 0,
        price: 0,
        pos_prev: 0,
        back_prev: 0,
        backs: [0; REPS],
    };

    const fn make_literal(&mut self) {
        self.back_prev = BACK_LITERAL;
        self.prev_1_is_literal = false;
    }

    const fn make_short_rep(&mut self) {
        self.back_prev = 0;
        self.prev_1_is_literal = false;
    }

    const fn is_short_rep(&self) -> bool {
        self.back_prev == 0
    }
}

pub(super) struct LengthPrices {
    prices: [[u32; LEN_SYMBOLS]; POSITION_STATES_MAX],
    table_size: u32,
    counters: [u32; POSITION_STATES_MAX],
}

impl LengthPrices {
    const fn new(table_size: u32) -> Self {
        Self {
            prices: [[0; LEN_SYMBOLS]; POSITION_STATES_MAX],
            table_size,
            counters: [0; POSITION_STATES_MAX],
        }
    }

    fn update(&mut self, probabilities: &[u16], base: usize, pos_state: usize) {
        let table_size = self.table_size as usize;
        self.counters[pos_state] = self.table_size;
        let a0 = bit_0_price(probabilities[base + LENGTH_CHOICE]);
        let a1 = bit_1_price(probabilities[base + LENGTH_CHOICE]);
        let b0 = a1 + bit_0_price(probabilities[base + LENGTH_CHOICE2]);
        let b1 = a1 + bit_1_price(probabilities[base + LENGTH_CHOICE2]);
        let low = base + LENGTH_LOW + pos_state * LEN_LOW_SYMBOLS;
        let mid = base + LENGTH_MID + pos_state * LEN_MID_SYMBOLS;
        let high = base + LENGTH_HIGH;
        let prices = &mut self.prices[pos_state];
        for (index, price) in prices.iter_mut().enumerate().take(table_size) {
            let symbol = index as u32;
            *price = if index < LEN_LOW_SYMBOLS {
                a0 + bittree_price(probabilities, low, LEN_LOW_BITS, symbol)
            } else if index < LEN_LOW_SYMBOLS + LEN_MID_SYMBOLS {
                let symbol = symbol - LEN_LOW_SYMBOLS as u32;
                b0 + bittree_price(probabilities, mid, LEN_MID_BITS, symbol)
            } else {
                let symbol = symbol - (LEN_LOW_SYMBOLS + LEN_MID_SYMBOLS) as u32;
                b1 + bittree_price(probabilities, high, LEN_HIGH_BITS, symbol)
            };
        }
    }

    const fn price(&self, len: u32, pos_state: usize) -> u32 {
        self.prices[pos_state][(len - MATCH_LEN_MIN_U32) as usize]
    }
}

pub(super) struct NormalParser {
    match_len_prices: LengthPrices,
    rep_len_prices: LengthPrices,
    dist_slot_prices: [[u32; DIST_SLOTS]; DIST_STATES],
    dist_prices: [[u32; FULL_DISTANCES]; DIST_STATES],
    dist_table_size: u32,
    pub(super) match_price_count: u32,
    align_prices: [u32; ALIGN_SIZE],
    pub(super) align_price_count: u32,
    opts_end_index: u32,
    opts_current_index: u32,
    opts: Box<[Optimal]>,
}

impl NormalParser {
    pub(super) fn new(dist_table_size: u32, len_table_size: u32) -> Box<Self> {
        assert!(dist_table_size as usize <= DIST_SLOTS);
        assert!(len_table_size as usize <= LEN_SYMBOLS);
        Box::new(Self {
            match_len_prices: LengthPrices::new(len_table_size),
            rep_len_prices: LengthPrices::new(len_table_size),
            dist_slot_prices: [[0; DIST_SLOTS]; DIST_STATES],
            dist_prices: [[0; FULL_DISTANCES]; DIST_STATES],
            dist_table_size,
            match_price_count: PRICE_COUNT_INITIAL,
            align_prices: [0; ALIGN_SIZE],
            align_price_count: PRICE_COUNT_INITIAL,
            opts_end_index: 0,
            opts_current_index: 0,
            opts: vec![Optimal::INITIAL; OPTS_COUNT].into_boxed_slice(),
        })
    }

    pub(super) fn reset(&mut self, probabilities: &[u16], pos_states: usize) {
        for pos_state in 0..pos_states {
            self.match_len_prices
                .update(probabilities, LengthKind::Match.base(), pos_state);
        }
        for pos_state in 0..pos_states {
            self.rep_len_prices
                .update(probabilities, LengthKind::Rep.base(), pos_state);
        }
        self.match_price_count = PRICE_COUNT_INITIAL;
        self.align_price_count = PRICE_COUNT_INITIAL;
        self.opts_end_index = 0;
        self.opts_current_index = 0;
    }

    pub(super) fn length_encoded(
        &mut self,
        probabilities: &[u16],
        kind: LengthKind,
        pos_state: usize,
    ) {
        let prices = match kind {
            LengthKind::Match => &mut self.match_len_prices,
            LengthKind::Rep => &mut self.rep_len_prices,
        };
        prices.counters[pos_state] -= 1;
        if prices.counters[pos_state] == 0 {
            prices.update(probabilities, kind.base(), pos_state);
        }
    }

    fn fill_dist_prices(&mut self, probabilities: &[u16]) {
        let table_size = self.dist_table_size as usize;
        for dist_state in 0..DIST_STATES {
            let slot_prices = &mut self.dist_slot_prices[dist_state];
            let base = dist_slot_base(dist_state);
            for (slot, price) in slot_prices.iter_mut().enumerate().take(table_size) {
                *price = bittree_price(probabilities, base, DIST_SLOT_BITS, slot as u32);
            }
            for slot in DIST_MODEL_END as usize..table_size {
                let slot_u32 = slot as u32;
                slot_prices[slot] += direct_price((slot_u32 >> 1) - 1 - ALIGN_BITS);
            }
            let direct = DIST_MODEL_START as usize;
            self.dist_prices[dist_state][..direct].copy_from_slice(&slot_prices[..direct]);
        }
        for dist in DIST_MODEL_START..FULL_DISTANCES as u32 {
            let slot = dist_slot(dist);
            let (footer_bits, base) = dist_footer(slot);
            let price = bittree_reverse_price(
                probabilities,
                dist_special_base(base, slot),
                footer_bits,
                dist - base,
            );
            for dist_state in 0..DIST_STATES {
                self.dist_prices[dist_state][dist as usize] =
                    price + self.dist_slot_prices[dist_state][slot as usize];
            }
        }
        self.match_price_count = 0;
    }

    fn fill_align_prices(&mut self, probabilities: &[u16]) {
        for (index, price) in self.align_prices.iter_mut().enumerate() {
            *price = bittree_reverse_price(probabilities, DIST_ALIGN, ALIGN_BITS, index as u32);
        }
        self.align_price_count = 0;
    }

    fn dist_len_price(&self, dist: u32, len: u32, pos_state: usize) -> u32 {
        let dist_state = dist_state(len);
        let price = if dist < FULL_DISTANCES as u32 {
            self.dist_prices[dist_state][dist as usize]
        } else {
            self.dist_slot_prices[dist_state][dist_slot(dist) as usize]
                + self.align_prices[(dist & ALIGN_MASK) as usize]
        };
        price + self.match_len_prices.price(len, pos_state)
    }

    fn rep_price(
        &self,
        model: &Model,
        rep_index: u32,
        len: u32,
        state: State,
        pos_state: usize,
    ) -> u32 {
        self.rep_len_prices.price(len, pos_state)
            + pure_rep_price(model, rep_index, state, pos_state)
    }

    fn backward(&mut self, cur: u32) -> (u32, u32) {
        let mut cur = cur as usize;
        self.opts_end_index = cur as u32;
        let mut pos_mem = self.opts[cur].pos_prev as usize;
        let mut back_mem = self.opts[cur].back_prev;
        loop {
            let opt_cur = self.opts[cur];
            if opt_cur.prev_1_is_literal {
                self.opts[pos_mem].make_literal();
                assert!(pos_mem > 0);
                self.opts[pos_mem].pos_prev = pos_mem as u32 - 1;
                if opt_cur.prev_2 {
                    let opt_prev = &mut self.opts[pos_mem - 1];
                    opt_prev.prev_1_is_literal = false;
                    opt_prev.pos_prev = opt_cur.pos_prev_2;
                    opt_prev.back_prev = opt_cur.back_prev_2;
                }
            }
            let pos_prev = pos_mem;
            let back_cur = back_mem;
            back_mem = self.opts[pos_prev].back_prev;
            pos_mem = self.opts[pos_prev].pos_prev as usize;
            self.opts[pos_prev].back_prev = back_cur;
            self.opts[pos_prev].pos_prev = cur as u32;
            cur = pos_prev;
            if cur == 0 {
                break;
            }
        }
        self.opts_current_index = self.opts[0].pos_prev;
        (self.opts[0].back_prev, self.opts[0].pos_prev)
    }
}

fn literal_price(
    model: &Model,
    position: u32,
    previous_byte: u8,
    match_mode: bool,
    match_byte: u8,
    symbol: u8,
) -> u32 {
    let base = model.literal_base(position, previous_byte);
    let probabilities = &model.probabilities;
    if !match_mode {
        return bittree_price(probabilities, base, LITERAL_BITS, u32::from(symbol));
    }
    let mut price = 0;
    let mut match_byte = u32::from(match_byte);
    let mut offset = LITERAL_OFFSET_INITIAL;
    let mut symbol = u32::from(symbol) + (1 << LITERAL_BITS);
    while symbol < LITERAL_SYMBOL_LIMIT {
        match_byte <<= 1;
        let match_bit = match_byte & offset;
        let subcoder_index = offset + match_bit + (symbol >> LITERAL_BITS);
        let bit = (symbol >> (LITERAL_BITS - 1)) & 1;
        price += bit_price(probabilities[base + subcoder_index as usize], bit);
        symbol <<= 1;
        offset &= !(match_byte ^ symbol);
    }
    price
}

fn short_rep_price(model: &Model, state: State, pos_state: usize) -> u32 {
    let probabilities = &model.probabilities;
    bit_0_price(probabilities[IS_REP0 + state.index()])
        + bit_0_price(probabilities[is_rep0_long(state, pos_state)])
}

fn pure_rep_price(model: &Model, rep_index: u32, state: State, pos_state: usize) -> u32 {
    let probabilities = &model.probabilities;
    let state_index = state.index();
    if rep_index == 0 {
        bit_0_price(probabilities[IS_REP0 + state_index])
            + bit_1_price(probabilities[is_rep0_long(state, pos_state)])
    } else {
        let price = bit_1_price(probabilities[IS_REP0 + state_index]);
        if rep_index == 1 {
            price + bit_0_price(probabilities[IS_REP1 + state_index])
        } else {
            price
                + bit_1_price(probabilities[IS_REP1 + state_index])
                + bit_price(probabilities[IS_REP2 + state_index], rep_index - 2)
        }
    }
}

const fn match_probability(model: &Model, state: State, pos_state: usize) -> u16 {
    model.probabilities[is_match(state, pos_state)]
}

const fn rep_probability(model: &Model, state: State) -> u16 {
    model.probabilities[IS_REP + state.index()]
}

impl LzmaEncoder {
    pub(super) fn optimum_normal(&mut self, mf: &mut MatchFinder, position: u32) -> (u32, u32) {
        let Self {
            model,
            state,
            reps,
            matches,
            matches_count,
            longest_match_length,
            parser,
            ..
        } = self;
        let Parser::Normal(normal) = parser else {
            unreachable!("the normal parser runs only in normal mode");
        };
        if normal.opts_end_index != normal.opts_current_index {
            let current = normal.opts_current_index;
            let opt = normal.opts[current as usize];
            normal.opts_current_index = opt.pos_prev;
            return (opt.back_prev, opt.pos_prev - current);
        }
        if mf.read_ahead == 0 {
            if normal.match_price_count >= DIST_PRICES_UPDATE_COUNT {
                normal.fill_dist_prices(&model.probabilities);
            }
            if normal.align_price_count >= ALIGN_SIZE as u32 {
                normal.fill_align_prices(&model.probabilities);
            }
        }
        let first = helper::First {
            model,
            state: *state,
            reps,
            matches_count: *matches_count,
            longest_match_length: *longest_match_length,
        };
        let mut len_end = match first.run(normal, matches, mf, position) {
            Ok(len_end) => len_end,
            Err(result) => return result,
        };
        let mut reps = *reps;
        let mut cur = 1;
        while cur < len_end {
            let (count, longest) = mf.find(matches);
            *matches_count = count;
            *longest_match_length = longest;
            if longest >= mf.nice_len {
                break;
            }
            let buf_avail_full = (mf.avail() + 1).min(OPTS - 1 - cur);
            let step = helper::Step {
                model,
                buffer: mf.buffer(),
                buf: mf.read_pos as usize - 1,
                position: position.wrapping_add(cur),
                cur,
                nice_len: mf.nice_len,
                buf_avail_full,
            };
            len_end = step.run(normal, matches, count, longest, &mut reps, len_end);
            cur += 1;
        }
        normal.backward(cur)
    }
}
