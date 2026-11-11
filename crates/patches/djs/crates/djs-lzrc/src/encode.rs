//! The encoder (djs-prelude's build): a price-based parse, then the model of `decode`.

use super::*;

struct Encoder {
    low: u64,
    range: u32,
    cache: u8,
    cache_size: u64,
    out: Vec<u8>,
}

impl Encoder {
    fn shift_low(&mut self) {
        if (self.low as u32) < 0xFF00_0000 || (self.low >> 32) != 0 {
            let carry = (self.low >> 32) as u8;
            let mut temp = self.cache;
            loop {
                self.out.push(temp.wrapping_add(carry));
                temp = 0xFF;
                self.cache_size -= 1;
                if self.cache_size == 0 {
                    break;
                }
            }
            self.cache = ((self.low >> 24) & 0xFF) as u8;
        }
        self.cache_size += 1;
        self.low = (self.low & 0x00FF_FFFF) << 8;
    }

    fn bit(&mut self, p: &mut u16, bit: u32) {
        let bound = (self.range >> PROB_BITS) * *p as u32;
        if bit == 0 {
            self.range = bound;
            *p += ((1 << PROB_BITS) - *p) >> MOVE;
        } else {
            self.low += bound as u64;
            self.range -= bound;
            *p -= *p >> MOVE;
        }
        while self.range < 1 << 24 {
            self.range <<= 8;
            self.shift_low();
        }
    }

    fn tree(&mut self, probs: &mut [u16], bits: u32, v: u32) {
        let mut m = 1usize;
        for k in (0..bits).rev() {
            let b = (v >> k) & 1;
            self.bit(&mut probs[m], b);
            m = (m << 1) | b as usize;
        }
    }

    fn direct(&mut self, bits: u32, v: u32) {
        for k in (0..bits).rev() {
            self.range >>= 1;
            if (v >> k) & 1 == 1 {
                self.low += self.range as u64;
            }
            while self.range < 1 << 24 {
                self.range <<= 8;
                self.shift_low();
            }
        }
    }

    fn len(&mut self, l: &mut Len, pc: usize, len: usize) {
        let x = (len - 2) as u32;
        if x < 8 {
            self.bit(&mut l.choice, 0);
            self.tree(&mut l.low[pc], 3, x);
        } else if x < 16 {
            self.bit(&mut l.choice, 1);
            self.bit(&mut l.choice2, 0);
            self.tree(&mut l.mid[pc], 3, x - 8);
        } else {
            self.bit(&mut l.choice, 1);
            self.bit(&mut l.choice2, 1);
            self.tree(&mut l.high, 8, x - 16);
        }
    }
}

#[derive(Clone, Copy)]
enum Tok {
    Lit,
    Rep(u8, u16),
    Match(u32, u16),
}

/// Approximate prices in 1/16 bits, for the parse only (static, from the shape of the
/// model; the coder adapts anyway).
fn len_price(l: usize) -> u32 {
    if l < 10 {
        5 * 16
    } else {
        5 * 16 + 16 * (usize::BITS - l.leading_zeros())
    }
}

fn dist_price(d: u32) -> u32 {
    let n = 32 - d.leading_zeros();
    6 * 16 + n.saturating_sub(2) * 15
}

const LIT_PRICE: u32 = 7 * 16;
const REP_PRICE: [u32; 4] = [3 * 16, 4 * 16, 72, 5 * 16];
const DEPTH: usize = 48;

fn match_len(b: &[u8], i: usize, j: usize) -> usize {
    let max = (b.len() - i).min(MAX_LEN);
    let mut l = 0;
    while l < max && b[i + l] == b[j + l] {
        l += 1;
    }
    l
}

/// The tokens of a cheapest parse under the static prices.
fn parse(b: &[u8]) -> Vec<Tok> {
    let n = b.len();
    let hash = |i: usize| -> usize {
        let v = b[i] as u32 | (b[i + 1] as u32) << 8 | (b[i + 2] as u32) << 16;
        (v.wrapping_mul(0x9E37_79B1) >> 15) as usize
    };
    let mut head = vec![-1i64; 1 << 17];
    let mut prev = vec![-1i64; n];
    let mut cost = vec![u32::MAX; n + 1];
    let mut back: Vec<(u32, Tok)> = vec![(0, Tok::Lit); n + 1];
    let mut reps: Vec<[u32; 4]> = vec![[1; 4]; n + 1];
    cost[0] = 0;
    for i in 0..n {
        // Candidates before inserting `i`: earlier positions only.
        let mut cands: Vec<(usize, u32)> = Vec::new();
        if i + 3 <= n {
            let h = hash(i);
            let mut j = head[h];
            let mut best = 2;
            let mut depth = 0;
            while j >= 0 && depth < DEPTH {
                let l = match_len(b, i, j as usize);
                if l > best {
                    best = l;
                    cands.push((l, (i - j as usize) as u32));
                }
                j = prev[j as usize];
                depth += 1;
            }
            prev[i] = head[h];
            head[h] = i as i64;
        }
        let c = cost[i];
        if c == u32::MAX {
            continue;
        }
        let r = reps[i];
        let mut relax = |to: usize, nc: u32, tok: Tok, nr: [u32; 4]| {
            if nc < cost[to] {
                cost[to] = nc;
                back[to] = (i as u32, tok);
                reps[to] = nr;
            }
        };
        relax(i + 1, c + LIT_PRICE, Tok::Lit, r);
        for k in 0..4 {
            let d = r[k] as usize;
            if d > i {
                continue;
            }
            let l = match_len(b, i, i - d);
            if l >= 2 {
                let mut nr = r;
                nr.copy_within(0..k, 1);
                nr[0] = d as u32;
                relax(
                    i + l,
                    c + 16 + REP_PRICE[k] + len_price(l),
                    Tok::Rep(k as u8, l as u16),
                    nr,
                );
            }
        }
        for &(l, d) in &cands {
            let nr = [d, r[0], r[1], r[2]];
            for len in [l, (l - 1).max(3)] {
                relax(
                    i + len,
                    c + 16 + len_price(len) + dist_price(d),
                    Tok::Match(d, len as u16),
                    nr,
                );
            }
        }
    }
    let mut toks = Vec::new();
    let mut p = n;
    while p > 0 {
        let (from, t) = back[p];
        toks.push(t);
        p = from as usize;
    }
    toks.reverse();
    toks
}

/// Encodes `data` (at most 4 GiB - 1).
pub fn encode(data: &[u8]) -> Vec<u8> {
    let mut e = Encoder {
        low: 0,
        range: u32::MAX,
        cache: 0,
        cache_size: 1,
        out: Vec::new(),
    };
    let mut m = Model::new();
    let mut reps = [1u32; 4];
    let mut st = ST_LIT;
    let mut pos = 0;
    for t in parse(data) {
        let pc = pos & 3;
        match t {
            Tok::Lit => {
                e.bit(&mut m.is_match[st][pc], 0);
                let prev = if pos > 0 { data[pos - 1] } else { 0 };
                let ctx = lit_ctx(pos, prev);
                let v = data[pos] as u32;
                let r0 = reps[0] as usize;
                if st != ST_LIT && r0 <= pos {
                    let mb = data[pos - r0] as u32;
                    let mut x = 1usize;
                    let mut same = true;
                    for k in (0..8).rev() {
                        let b = (v >> k) & 1;
                        if same {
                            let mbit = ((mb >> k) & 1) as usize;
                            e.bit(&mut m.matched[mbit][x], b);
                            same = b as usize == mbit;
                        } else {
                            e.bit(&mut m.lit[ctx][x], b);
                        }
                        x = (x << 1) | b as usize;
                    }
                } else {
                    e.tree(&mut m.lit[ctx], 8, v);
                }
                pos += 1;
                st = ST_LIT;
            }
            Tok::Rep(k, len) => {
                let k = k as usize;
                e.bit(&mut m.is_match[st][pc], 1);
                e.bit(&mut m.is_rep[st], 1);
                if k == 0 {
                    e.bit(&mut m.rep0[st], 0);
                } else {
                    e.bit(&mut m.rep0[st], 1);
                    if k == 1 {
                        e.bit(&mut m.rep1, 0);
                    } else {
                        e.bit(&mut m.rep1, 1);
                        e.bit(&mut m.rep2, if k == 2 { 0 } else { 1 });
                    }
                }
                let dist = reps[k];
                reps.copy_within(0..k, 1);
                reps[0] = dist;
                e.len(&mut m.rep_len, pc, len as usize);
                pos += len as usize;
                st = ST_REP;
            }
            Tok::Match(dist, len) => {
                let len = len as usize;
                e.bit(&mut m.is_match[st][pc], 1);
                e.bit(&mut m.is_rep[st], 0);
                e.len(&mut m.match_len, pc, len);
                let (s, nb, extra) = slot_of(dist);
                e.tree(&mut m.slot[(len - 2).min(3)], 6, s as u32);
                if s >= 4 {
                    if nb <= 4 {
                        e.tree(&mut m.extra[s], nb, extra);
                    } else {
                        e.direct(nb - 4, extra >> 4);
                        e.tree(&mut m.align, 4, extra & 15);
                    }
                }
                reps.copy_within(0..3, 1);
                reps[0] = dist;
                pos += len;
                st = ST_MATCH;
            }
        }
    }
    for _ in 0..5 {
        e.shift_low();
    }
    let mut out = Vec::with_capacity(HEADER + e.out.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&fnv64(data).to_le_bytes());
    out.extend_from_slice(&e.out);
    out
}
