use super::{Cursor, Match, MatchFinder, memcmplen};

impl MatchFinder {
    pub(super) fn hc3_find(&mut self, matches: &mut [Match]) -> usize {
        let Some(len_limit) = self.len_limit(3) else {
            self.move_pending();
            return 0;
        };
        let cur = self.read_pos as usize;
        let pos = self.position();
        let (delta2, cur_match) = self.update_hash_3(cur, pos);
        self.grow_son();
        let cursor = self.cursor(len_limit, pos, cur);
        let mut len_best = 2;
        let mut count = 0;
        if delta2 < self.cyclic_size && self.buffer[cur - delta2 as usize] == self.buffer[cur] {
            len_best = memcmplen(
                &self.buffer,
                cur - delta2 as usize,
                cur,
                len_best,
                len_limit,
            );
            matches[0] = Match {
                len: len_best,
                dist: delta2 - 1,
            };
            count = 1;
            if len_best == len_limit {
                self.son[self.cyclic_pos as usize] = cur_match;
                self.move_pos();
                return count;
            }
        }
        let count = chain_find(
            &self.buffer,
            &mut self.son,
            cursor,
            cur_match,
            matches,
            count,
            len_best,
        );
        self.move_pos();
        count
    }

    pub(super) fn hc4_find(&mut self, matches: &mut [Match]) -> usize {
        let Some(len_limit) = self.len_limit(4) else {
            self.move_pending();
            return 0;
        };
        let cur = self.read_pos as usize;
        let pos = self.position();
        let (mut delta2, delta3, cur_match) = self.update_hash_4(cur, pos);
        self.grow_son();
        let cursor = self.cursor(len_limit, pos, cur);
        let mut len_best = 1;
        let mut count = 0;
        if delta2 < self.cyclic_size && self.buffer[cur - delta2 as usize] == self.buffer[cur] {
            len_best = 2;
            matches[0] = Match {
                len: 2,
                dist: delta2 - 1,
            };
            count = 1;
        }
        if delta2 != delta3
            && delta3 < self.cyclic_size
            && self.buffer[cur - delta3 as usize] == self.buffer[cur]
        {
            len_best = 3;
            matches[count].dist = delta3 - 1;
            count += 1;
            delta2 = delta3;
        }
        if count != 0 {
            len_best = memcmplen(
                &self.buffer,
                cur - delta2 as usize,
                cur,
                len_best,
                len_limit,
            );
            matches[count - 1].len = len_best;
            if len_best == len_limit {
                self.son[self.cyclic_pos as usize] = cur_match;
                self.move_pos();
                return count;
            }
        }
        let len_best = len_best.max(3);
        let count = chain_find(
            &self.buffer,
            &mut self.son,
            cursor,
            cur_match,
            matches,
            count,
            len_best,
        );
        self.move_pos();
        count
    }

    pub(super) fn hc3_skip(&mut self) {
        if self.avail() < 3 {
            self.move_pending();
            return;
        }
        let cur = self.read_pos as usize;
        let pos = self.position();
        let (_, cur_match) = self.update_hash_3(cur, pos);
        self.grow_son();
        self.son[self.cyclic_pos as usize] = cur_match;
        self.move_pos();
    }

    pub(super) fn hc4_skip(&mut self) {
        if self.avail() < 4 {
            self.move_pending();
            return;
        }
        let cur = self.read_pos as usize;
        let pos = self.position();
        let (_, _, cur_match) = self.update_hash_4(cur, pos);
        self.grow_son();
        self.son[self.cyclic_pos as usize] = cur_match;
        self.move_pos();
    }
}

fn chain_find(
    buffer: &[u8],
    son: &mut [u32],
    cursor: Cursor,
    cur_match: u32,
    matches: &mut [Match],
    count: usize,
    len_best: u32,
) -> usize {
    let mut cur_match = cur_match;
    let mut count = count;
    let mut len_best = len_best;
    let mut depth = cursor.depth;
    let cur = cursor.cur;
    son[cursor.cyclic_pos as usize] = cur_match;
    loop {
        let delta = cursor.pos.wrapping_sub(cur_match);
        if depth == 0 || delta >= cursor.cyclic_size {
            return count;
        }
        depth -= 1;
        let back = cur - delta as usize;
        cur_match = son[cursor.slot(delta)];
        let best = len_best as usize;
        if buffer[back + best] == buffer[cur + best] && buffer[back] == buffer[cur] {
            let len = memcmplen(buffer, back, cur, 1, cursor.len_limit);
            if len_best < len {
                len_best = len;
                matches[count] = Match {
                    len,
                    dist: delta - 1,
                };
                count += 1;
                if len == cursor.len_limit {
                    return count;
                }
            }
        }
    }
}
