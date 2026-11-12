use super::super::super::lzma_common::{REPS, State};
use super::super::lzma_encoder::{BACK_LITERAL, MATCH_LEN_MAX_U32, Model, REPS_U32};
use super::super::match_finder::{Match, MatchFinder, memcmplen};
use super::super::price::{INFINITY_PRICE, bit_0_price, bit_1_price};
use super::{
    NormalParser, literal_price, match_probability, not_equal_16, pure_rep_price, rep_probability,
    short_rep_price,
};

pub(super) struct First<'a> {
    pub(super) model: &'a Model,
    pub(super) state: State,
    pub(super) reps: &'a [u32; REPS],
    pub(super) matches_count: u32,
    pub(super) longest_match_length: u32,
}

impl First<'_> {
    pub(super) fn run(
        self,
        normal: &mut NormalParser,
        matches: &mut [Match],
        mf: &mut MatchFinder,
        position: u32,
    ) -> Result<u32, (u32, u32)> {
        let nice_len = mf.nice_len;
        let (len_main, matches_count) = if mf.read_ahead == 0 {
            let (count, len) = mf.find(matches);
            (len, count)
        } else {
            assert!(mf.read_ahead == 1);
            (self.longest_match_length, self.matches_count)
        };
        let buf_avail = (mf.avail() + 1).min(MATCH_LEN_MAX_U32);
        if buf_avail < 2 {
            return Err((BACK_LITERAL, 1));
        }
        let buf = mf.read_pos as usize - 1;
        let mut rep_lens = [0; REPS];
        let mut rep_max_index = 0;
        let buffer = mf.buffer();
        for (index, rep_len) in rep_lens.iter_mut().enumerate() {
            let back = buf - self.reps[index] as usize - 1;
            if !not_equal_16(buffer, buf, back) {
                *rep_len = memcmplen(buffer, buf, back, 2, buf_avail);
            }
        }
        for index in 0..REPS {
            if rep_lens[index] > rep_lens[rep_max_index] {
                rep_max_index = index;
            }
        }
        let rep_max_len = rep_lens[rep_max_index];
        if rep_max_len >= nice_len {
            mf.skip(rep_max_len - 1);
            return Err((rep_max_index as u32, rep_max_len));
        }
        if len_main >= nice_len {
            assert!(matches_count > 0);
            let back = matches[matches_count as usize - 1].dist + REPS_U32;
            mf.skip(len_main - 1);
            return Err((back, len_main));
        }
        let buffer = mf.buffer();
        let current_byte = buffer[buf];
        let match_byte = buffer[buf - self.reps[0] as usize - 1];
        if len_main < 2 && current_byte != match_byte && rep_max_len < 2 {
            return Err((BACK_LITERAL, 1));
        }
        let (model, state) = (self.model, self.state);
        let pos_state = model.pos_state(position);
        normal.opts[0].state = state;
        normal.opts[1].price = bit_0_price(match_probability(model, state, pos_state))
            + literal_price(
                model,
                position,
                buffer[buf - 1],
                !state.is_literal(),
                match_byte,
                current_byte,
            );
        normal.opts[1].make_literal();
        let match_price = bit_1_price(match_probability(model, state, pos_state));
        let rep_match_price = match_price + bit_1_price(rep_probability(model, state));
        if match_byte == current_byte {
            let short_rep = rep_match_price + short_rep_price(model, state, pos_state);
            if short_rep < normal.opts[1].price {
                normal.opts[1].price = short_rep;
                normal.opts[1].make_short_rep();
            }
        }
        let len_end = len_main.max(rep_max_len);
        if len_end < 2 {
            return Err((normal.opts[1].back_prev, 1));
        }
        normal.opts[1].pos_prev = 0;
        normal.opts[0].backs = *self.reps;
        for len in 2..=len_end as usize {
            normal.opts[len].price = INFINITY_PRICE;
        }
        self.price_reps(normal, &rep_lens, rep_match_price, pos_state);
        let normal_match_price = match_price + bit_0_price(rep_probability(model, state));
        let len_start = if rep_lens[0] >= 2 { rep_lens[0] + 1 } else { 2 };
        if len_start <= len_main {
            price_matches(
                normal,
                &matches[..matches_count as usize],
                len_start,
                normal_match_price,
                pos_state,
            );
        }
        Ok(len_end)
    }

    fn price_reps(
        &self,
        normal: &mut NormalParser,
        rep_lens: &[u32; REPS],
        rep_match_price: u32,
        pos_state: usize,
    ) {
        for (rep_index, &rep_len) in rep_lens.iter().enumerate() {
            if rep_len < 2 {
                continue;
            }
            let rep_index = rep_index as u32;
            let price =
                rep_match_price + pure_rep_price(self.model, rep_index, self.state, pos_state);
            for len in (2..=rep_len).rev() {
                let total = price + normal.rep_len_prices.price(len, pos_state);
                let opt = &mut normal.opts[len as usize];
                if total < opt.price {
                    opt.price = total;
                    opt.pos_prev = 0;
                    opt.back_prev = rep_index;
                    opt.prev_1_is_literal = false;
                }
            }
        }
    }
}

fn price_matches(
    normal: &mut NormalParser,
    matches: &[Match],
    len_start: u32,
    normal_match_price: u32,
    pos_state: usize,
) {
    let mut index = 0;
    while len_start > matches[index].len {
        index += 1;
    }
    let mut len = len_start;
    loop {
        let dist = matches[index].dist;
        let total = normal_match_price + normal.dist_len_price(dist, len, pos_state);
        let opt = &mut normal.opts[len as usize];
        if total < opt.price {
            opt.price = total;
            opt.pos_prev = 0;
            opt.back_prev = dist + REPS_U32;
            opt.prev_1_is_literal = false;
        }
        if len == matches[index].len {
            index += 1;
            if index == matches.len() {
                return;
            }
        }
        len += 1;
    }
}

pub(super) struct Step<'a> {
    pub(super) model: &'a Model,
    pub(super) buffer: &'a [u8],
    pub(super) buf: usize,
    pub(super) position: u32,
    pub(super) cur: u32,
    pub(super) nice_len: u32,
    pub(super) buf_avail_full: u32,
}

struct Prices {
    state: State,
    pos_state: usize,
    match_price: u32,
    rep_match_price: u32,
}

impl Step<'_> {
    pub(super) fn run(
        &self,
        normal: &mut NormalParser,
        matches: &mut [Match],
        matches_count: u32,
        longest_match_length: u32,
        reps: &mut [u32; REPS],
        len_end: u32,
    ) -> u32 {
        let mut len_end = len_end;
        let state = self.state_and_reps(normal, reps);
        let (prices, next_is_literal, cur_and_1_price) = self.price_next(normal, state, reps);
        if self.buf_avail_full < 2 {
            return len_end;
        }
        let buf_avail = self.buf_avail_full.min(self.nice_len);
        let match_byte = self.buffer[self.buf - reps[0] as usize - 1];
        if !next_is_literal && match_byte != self.buffer[self.buf] {
            self.price_literal_rep0(normal, &mut len_end, &prices, reps[0], cur_and_1_price);
        }
        let start_len = self.price_reps(normal, &mut len_end, &prices, reps, buf_avail);
        let mut matches_count = matches_count as usize;
        let mut new_len = longest_match_length;
        if new_len > buf_avail {
            new_len = buf_avail;
            matches_count = 0;
            while new_len > matches[matches_count].len {
                matches_count += 1;
            }
            matches[matches_count].len = new_len;
            matches_count += 1;
        }
        if new_len >= start_len {
            while len_end < self.cur + new_len {
                len_end += 1;
                normal.opts[len_end as usize].price = INFINITY_PRICE;
            }
            self.price_matches(
                normal,
                &mut len_end,
                &prices,
                &matches[..matches_count],
                start_len,
            );
        }
        len_end
    }

    fn state_and_reps(&self, normal: &mut NormalParser, reps: &mut [u32; REPS]) -> State {
        let cur = self.cur as usize;
        let opt_cur = normal.opts[cur];
        let mut pos_prev = opt_cur.pos_prev as usize;
        let mut state;
        if opt_cur.prev_1_is_literal {
            pos_prev -= 1;
            if opt_cur.prev_2 {
                state = normal.opts[opt_cur.pos_prev_2 as usize].state;
                state = if opt_cur.back_prev_2 < REPS_U32 {
                    state.after_long_rep()
                } else {
                    state.after_match()
                };
            } else {
                state = normal.opts[pos_prev].state;
            }
            state = state.after_literal();
        } else {
            state = normal.opts[pos_prev].state;
        }
        if pos_prev == cur - 1 {
            state = if opt_cur.is_short_rep() {
                state.after_short_rep()
            } else {
                state.after_literal()
            };
        } else {
            let back;
            if opt_cur.prev_1_is_literal && opt_cur.prev_2 {
                pos_prev = opt_cur.pos_prev_2 as usize;
                back = opt_cur.back_prev_2;
                state = state.after_long_rep();
            } else {
                back = opt_cur.back_prev;
                state = if back < REPS_U32 {
                    state.after_long_rep()
                } else {
                    state.after_match()
                };
            }
            let prev_backs = normal.opts[pos_prev].backs;
            if back < REPS_U32 {
                let back = back as usize;
                reps[0] = prev_backs[back];
                reps[1..=back].copy_from_slice(&prev_backs[..back]);
                reps[back + 1..].copy_from_slice(&prev_backs[back + 1..]);
            } else {
                reps[0] = back - REPS_U32;
                reps[1..].copy_from_slice(&prev_backs[..REPS - 1]);
            }
        }
        normal.opts[cur].state = state;
        normal.opts[cur].backs = *reps;
        state
    }

    fn price_next(
        &self,
        normal: &mut NormalParser,
        state: State,
        reps: &[u32; REPS],
    ) -> (Prices, bool, u32) {
        let model = self.model;
        let cur = self.cur as usize;
        let cur_price = normal.opts[cur].price;
        let current_byte = self.buffer[self.buf];
        let match_byte = self.buffer[self.buf - reps[0] as usize - 1];
        let pos_state = model.pos_state(self.position);
        let cur_and_1_price = cur_price
            + bit_0_price(match_probability(model, state, pos_state))
            + literal_price(
                model,
                self.position,
                self.buffer[self.buf - 1],
                !state.is_literal(),
                match_byte,
                current_byte,
            );
        let mut next_is_literal = false;
        let next = &mut normal.opts[cur + 1];
        if cur_and_1_price < next.price {
            next.price = cur_and_1_price;
            next.pos_prev = self.cur;
            next.make_literal();
            next_is_literal = true;
        }
        let match_price = cur_price + bit_1_price(match_probability(model, state, pos_state));
        let rep_match_price = match_price + bit_1_price(rep_probability(model, state));
        if match_byte == current_byte && !(next.pos_prev < self.cur && next.back_prev == 0) {
            let short_rep = rep_match_price + short_rep_price(model, state, pos_state);
            if short_rep <= next.price {
                next.price = short_rep;
                next.pos_prev = self.cur;
                next.make_short_rep();
                next_is_literal = true;
            }
        }
        let prices = Prices {
            state,
            pos_state,
            match_price,
            rep_match_price,
        };
        (prices, next_is_literal, cur_and_1_price)
    }

    fn price_literal_rep0(
        &self,
        normal: &mut NormalParser,
        len_end: &mut u32,
        prices: &Prices,
        rep0: u32,
        cur_and_1_price: u32,
    ) {
        let back = self.buf - rep0 as usize - 1;
        let limit = self.buf_avail_full.min(self.nice_len + 1);
        let len_test = memcmplen(self.buffer, self.buf, back, 1, limit) - 1;
        if len_test < 2 {
            return;
        }
        let model = self.model;
        let state_2 = prices.state.after_literal();
        let pos_state_next = model.pos_state(self.position.wrapping_add(1));
        let next_rep_match_price = cur_and_1_price
            + bit_1_price(match_probability(model, state_2, pos_state_next))
            + bit_1_price(rep_probability(model, state_2));
        let offset = self.cur + 1 + len_test;
        extend(normal, len_end, offset);
        let total =
            next_rep_match_price + normal.rep_price(model, 0, len_test, state_2, pos_state_next);
        let opt = &mut normal.opts[offset as usize];
        if total < opt.price {
            opt.price = total;
            opt.pos_prev = self.cur + 1;
            opt.back_prev = 0;
            opt.prev_1_is_literal = true;
            opt.prev_2 = false;
        }
    }

    fn price_reps(
        &self,
        normal: &mut NormalParser,
        len_end: &mut u32,
        prices: &Prices,
        reps: &[u32; REPS],
        buf_avail: u32,
    ) -> u32 {
        let mut start_len = 2;
        for (rep_index, &rep) in reps.iter().enumerate() {
            let back = self.buf - rep as usize - 1;
            if not_equal_16(self.buffer, self.buf, back) {
                continue;
            }
            let rep_index = rep_index as u32;
            let len_test = memcmplen(self.buffer, self.buf, back, 2, buf_avail);
            extend(normal, len_end, self.cur + len_test);
            let price = prices.rep_match_price
                + pure_rep_price(self.model, rep_index, prices.state, prices.pos_state);
            for len in (2..=len_test).rev() {
                let total = price + normal.rep_len_prices.price(len, prices.pos_state);
                let opt = &mut normal.opts[(self.cur + len) as usize];
                if total < opt.price {
                    opt.price = total;
                    opt.pos_prev = self.cur;
                    opt.back_prev = rep_index;
                    opt.prev_1_is_literal = false;
                }
            }
            if rep_index == 0 {
                start_len = len_test + 1;
            }
            let len_test_2 = self.extension(back, len_test);
            if len_test_2 >= 2 {
                let after = After {
                    back,
                    len_test,
                    len_test_2,
                    price: price + normal.rep_len_prices.price(len_test, prices.pos_state),
                    state: prices.state.after_long_rep(),
                    back_prev_2: rep_index,
                };
                self.consider_literal_after(normal, len_end, &after);
            }
        }
        start_len
    }

    fn extension(&self, back: usize, len_test: u32) -> u32 {
        let mut len_test_2 = len_test + 1;
        let limit = self.buf_avail_full.min(len_test_2 + self.nice_len);
        if len_test_2 < limit {
            len_test_2 = memcmplen(self.buffer, self.buf, back, len_test_2, limit);
        }
        len_test_2 - (len_test + 1)
    }

    fn price_matches(
        &self,
        normal: &mut NormalParser,
        len_end: &mut u32,
        prices: &Prices,
        matches: &[Match],
        start_len: u32,
    ) {
        let normal_match_price =
            prices.match_price + bit_0_price(rep_probability(self.model, prices.state));
        let mut index = 0;
        while start_len > matches[index].len {
            index += 1;
        }
        let mut len_test = start_len;
        loop {
            let cur_back = matches[index].dist;
            let total =
                normal_match_price + normal.dist_len_price(cur_back, len_test, prices.pos_state);
            let opt = &mut normal.opts[(self.cur + len_test) as usize];
            if total < opt.price {
                opt.price = total;
                opt.pos_prev = self.cur;
                opt.back_prev = cur_back + REPS_U32;
                opt.prev_1_is_literal = false;
            }
            if len_test == matches[index].len {
                let back = self.buf - cur_back as usize - 1;
                let len_test_2 = self.extension(back, len_test);
                if len_test_2 >= 2 {
                    let after = After {
                        back,
                        len_test,
                        len_test_2,
                        price: total,
                        state: prices.state.after_match(),
                        back_prev_2: cur_back + REPS_U32,
                    };
                    self.consider_literal_after(normal, len_end, &after);
                }
                index += 1;
                if index == matches.len() {
                    return;
                }
            }
            len_test += 1;
        }
    }

    fn consider_literal_after(&self, normal: &mut NormalParser, len_end: &mut u32, after: &After) {
        let model = self.model;
        let len_test = after.len_test;
        let literal_position = self.position.wrapping_add(len_test);
        let pos_state_next = model.pos_state(literal_position);
        let index = len_test as usize;
        let cur_and_len_literal_price = after.price
            + bit_0_price(match_probability(model, after.state, pos_state_next))
            + literal_price(
                model,
                literal_position,
                self.buffer[self.buf + index - 1],
                true,
                self.buffer[after.back + index],
                self.buffer[self.buf + index],
            );
        let state_2 = after.state.after_literal();
        let pos_state_next = model.pos_state(literal_position.wrapping_add(1));
        let next_rep_match_price = cur_and_len_literal_price
            + bit_1_price(match_probability(model, state_2, pos_state_next))
            + bit_1_price(rep_probability(model, state_2));
        let offset = self.cur + len_test + 1 + after.len_test_2;
        extend(normal, len_end, offset);
        let total = next_rep_match_price
            + normal.rep_price(model, 0, after.len_test_2, state_2, pos_state_next);
        let opt = &mut normal.opts[offset as usize];
        if total < opt.price {
            opt.price = total;
            opt.pos_prev = self.cur + len_test + 1;
            opt.back_prev = 0;
            opt.prev_1_is_literal = true;
            opt.prev_2 = true;
            opt.pos_prev_2 = self.cur;
            opt.back_prev_2 = after.back_prev_2;
        }
    }
}

struct After {
    back: usize,
    len_test: u32,
    len_test_2: u32,
    price: u32,
    state: State,
    back_prev_2: u32,
}

fn extend(normal: &mut NormalParser, len_end: &mut u32, offset: u32) {
    while *len_end < offset {
        *len_end += 1;
        normal.opts[*len_end as usize].price = INFINITY_PRICE;
    }
}
