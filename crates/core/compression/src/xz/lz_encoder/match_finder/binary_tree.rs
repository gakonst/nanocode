use super::{Cursor, EMPTY_HASH_VALUE, Match, MatchFinder, memcmplen};

impl MatchFinder {
    pub(super) fn bt2_find(&mut self, matches: &mut [Match]) -> usize {
        let Some(len_limit) = self.len_limit(2) else {
            self.move_pending();
            return 0;
        };
        let cur = self.read_pos as usize;
        let pos = self.position();
        let cur_match = self.update_hash_2(cur, pos);
        self.grow_son();
        let cursor = self.cursor(len_limit, pos, cur);
        let count = tree_find(
            &self.buffer,
            &mut self.son,
            cursor,
            cur_match,
            matches,
            0,
            1,
        );
        self.move_pos();
        count
    }

    pub(super) fn bt3_find(&mut self, matches: &mut [Match]) -> usize {
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
                cur,
                cur - delta2 as usize,
                len_best,
                len_limit,
            );
            matches[0] = Match {
                len: len_best,
                dist: delta2 - 1,
            };
            count = 1;
            if len_best == len_limit {
                tree_skip(&self.buffer, &mut self.son, cursor, cur_match);
                self.move_pos();
                return count;
            }
        }
        let count = tree_find(
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

    pub(super) fn bt4_find(&mut self, matches: &mut [Match]) -> usize {
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
                cur,
                cur - delta2 as usize,
                len_best,
                len_limit,
            );
            matches[count - 1].len = len_best;
            if len_best == len_limit {
                tree_skip(&self.buffer, &mut self.son, cursor, cur_match);
                self.move_pos();
                return count;
            }
        }
        let len_best = len_best.max(3);
        let count = tree_find(
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

    pub(super) fn bt2_skip(&mut self) {
        let Some(len_limit) = self.len_limit(2) else {
            self.move_pending();
            return;
        };
        let cur = self.read_pos as usize;
        let pos = self.position();
        let cur_match = self.update_hash_2(cur, pos);
        self.grow_son();
        let cursor = self.cursor(len_limit, pos, cur);
        tree_skip(&self.buffer, &mut self.son, cursor, cur_match);
        self.move_pos();
    }

    pub(super) fn bt3_skip(&mut self) {
        let Some(len_limit) = self.len_limit(3) else {
            self.move_pending();
            return;
        };
        let cur = self.read_pos as usize;
        let pos = self.position();
        let (_, cur_match) = self.update_hash_3(cur, pos);
        self.grow_son();
        let cursor = self.cursor(len_limit, pos, cur);
        tree_skip(&self.buffer, &mut self.son, cursor, cur_match);
        self.move_pos();
    }

    pub(super) fn bt4_skip(&mut self) {
        let Some(len_limit) = self.len_limit(4) else {
            self.move_pending();
            return;
        };
        let cur = self.read_pos as usize;
        let pos = self.position();
        let (_, _, cur_match) = self.update_hash_4(cur, pos);
        self.grow_son();
        let cursor = self.cursor(len_limit, pos, cur);
        tree_skip(&self.buffer, &mut self.son, cursor, cur_match);
        self.move_pos();
    }
}

fn tree_find(
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
    let mut ptr0 = cursor.cyclic_pos as usize * 2 + 1;
    let mut ptr1 = cursor.cyclic_pos as usize * 2;
    let mut len0 = 0;
    let mut len1 = 0;
    loop {
        let delta = cursor.pos.wrapping_sub(cur_match);
        if depth == 0 || delta >= cursor.cyclic_size {
            son[ptr0] = EMPTY_HASH_VALUE;
            son[ptr1] = EMPTY_HASH_VALUE;
            return count;
        }
        depth -= 1;
        let pair = cursor.slot(delta) * 2;
        let back = cur - delta as usize;
        let mut len = len0.min(len1);
        if buffer[back + len as usize] == buffer[cur + len as usize] {
            len = memcmplen(buffer, back, cur, len + 1, cursor.len_limit);
            if len_best < len {
                len_best = len;
                matches[count] = Match {
                    len,
                    dist: delta - 1,
                };
                count += 1;
                if len == cursor.len_limit {
                    son[ptr1] = son[pair];
                    son[ptr0] = son[pair + 1];
                    return count;
                }
            }
        }
        if buffer[back + len as usize] < buffer[cur + len as usize] {
            son[ptr1] = cur_match;
            ptr1 = pair + 1;
            cur_match = son[ptr1];
            len1 = len;
        } else {
            son[ptr0] = cur_match;
            ptr0 = pair;
            cur_match = son[ptr0];
            len0 = len;
        }
    }
}

fn tree_skip(buffer: &[u8], son: &mut [u32], cursor: Cursor, cur_match: u32) {
    let mut cur_match = cur_match;
    let mut depth = cursor.depth;
    let cur = cursor.cur;
    let mut ptr0 = cursor.cyclic_pos as usize * 2 + 1;
    let mut ptr1 = cursor.cyclic_pos as usize * 2;
    let mut len0 = 0;
    let mut len1 = 0;
    loop {
        let delta = cursor.pos.wrapping_sub(cur_match);
        if depth == 0 || delta >= cursor.cyclic_size {
            son[ptr0] = EMPTY_HASH_VALUE;
            son[ptr1] = EMPTY_HASH_VALUE;
            return;
        }
        depth -= 1;
        let pair = cursor.slot(delta) * 2;
        let back = cur - delta as usize;
        let mut len = len0.min(len1);
        if buffer[back + len as usize] == buffer[cur + len as usize] {
            len = memcmplen(buffer, back, cur, len + 1, cursor.len_limit);
            if len == cursor.len_limit {
                son[ptr1] = son[pair];
                son[ptr0] = son[pair + 1];
                return;
            }
        }
        if buffer[back + len as usize] < buffer[cur + len as usize] {
            son[ptr1] = cur_match;
            ptr1 = pair + 1;
            cur_match = son[ptr1];
            len1 = len;
        } else {
            son[ptr0] = cur_match;
            ptr0 = pair;
            cur_match = son[ptr0];
            len0 = len;
        }
    }
}
