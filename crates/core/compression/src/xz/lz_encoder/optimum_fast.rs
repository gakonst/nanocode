use super::super::lzma_common::REPS;
use super::lzma_encoder::{BACK_LITERAL, LzmaEncoder, MATCH_LEN_MAX_U32, REPS_U32};
use super::match_finder::{MatchFinder, memcmplen, not_equal_16};

const CHANGE_PAIR_SHIFT_BITS: u32 = 7;
const SHORT_MATCH_DIST_LIMIT: u32 = 0x80;
const REP_PREFERENCE_DIST_1: u32 = 1 << 9;
const REP_PREFERENCE_DIST_2: u32 = 1 << 15;

const fn change_pair(small_dist: u32, big_dist: u32) -> bool {
    (big_dist >> CHANGE_PAIR_SHIFT_BITS) > small_dist
}

impl LzmaEncoder {
    pub(super) fn optimum_fast(&mut self, mf: &mut MatchFinder) -> (u32, u32) {
        let nice_len = mf.nice_len;
        let (mut len_main, mut matches_count) = if mf.read_ahead == 0 {
            let (count, len) = mf.find(&mut self.matches);
            (len, count)
        } else {
            assert!(mf.read_ahead == 1);
            (self.longest_match_length, self.matches_count)
        };
        let buf = mf.read_pos as usize - 1;
        let buf_avail = (mf.avail() + 1).min(MATCH_LEN_MAX_U32);
        if buf_avail < 2 {
            return (BACK_LITERAL, 1);
        }
        let mut rep_len = 0;
        let mut rep_index = 0;
        let mut nice_rep = None;
        let buffer = mf.buffer();
        for index in 0..REPS {
            let back = buf - self.reps[index] as usize - 1;
            if not_equal_16(buffer, buf, back) {
                continue;
            }
            let len = memcmplen(buffer, buf, back, 2, buf_avail);
            if len >= nice_len {
                nice_rep = Some((index as u32, len));
                break;
            }
            if len > rep_len {
                rep_index = index as u32;
                rep_len = len;
            }
        }
        if let Some((index, len)) = nice_rep {
            mf.skip(len - 1);
            return (index, len);
        }
        if len_main >= nice_len {
            let back = self.matches[matches_count as usize - 1].dist + REPS_U32;
            mf.skip(len_main - 1);
            return (back, len_main);
        }
        let mut back_main = 0;
        if len_main >= 2 {
            back_main = self.matches[matches_count as usize - 1].dist;
            while matches_count > 1 && len_main == self.matches[matches_count as usize - 2].len + 1
            {
                if !change_pair(self.matches[matches_count as usize - 2].dist, back_main) {
                    break;
                }
                matches_count -= 1;
                len_main = self.matches[matches_count as usize - 1].len;
                back_main = self.matches[matches_count as usize - 1].dist;
            }
            if len_main == 2 && back_main >= SHORT_MATCH_DIST_LIMIT {
                len_main = 1;
            }
        }
        if rep_len >= 2
            && (rep_len + 1 >= len_main
                || (rep_len + 2 >= len_main && back_main > REP_PREFERENCE_DIST_1)
                || (rep_len + 3 >= len_main && back_main > REP_PREFERENCE_DIST_2))
        {
            mf.skip(rep_len - 1);
            return (rep_index, rep_len);
        }
        if len_main < 2 || buf_avail <= 2 {
            return (BACK_LITERAL, 1);
        }
        self.optimum_fast_lookahead(mf, buf, len_main, back_main)
    }

    fn optimum_fast_lookahead(
        &mut self,
        mf: &mut MatchFinder,
        buf: usize,
        len_main: u32,
        back_main: u32,
    ) -> (u32, u32) {
        let (count, longest) = mf.find(&mut self.matches);
        self.matches_count = count;
        self.longest_match_length = longest;
        if longest >= 2 {
            let new_dist = self.matches[count as usize - 1].dist;
            if (longest >= len_main && new_dist < back_main)
                || (longest == len_main + 1 && !change_pair(back_main, new_dist))
                || longest > len_main + 1
                || (longest + 1 >= len_main && len_main >= 3 && change_pair(new_dist, back_main))
            {
                return (BACK_LITERAL, 1);
            }
        }
        let buf = buf + 1;
        let limit = 2.max(len_main - 1) as usize;
        let buffer = mf.buffer();
        for &rep in &self.reps {
            let back = buf - rep as usize - 1;
            if buffer[buf..buf + limit] == buffer[back..back + limit] {
                return (BACK_LITERAL, 1);
            }
        }
        mf.skip(len_main - 2);
        (back_main + REPS_U32, len_main)
    }
}
