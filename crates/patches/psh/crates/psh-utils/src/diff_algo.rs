//! The difference of two sequences of line ids: Myers' O(ND) algorithm in linear space
//! ("An O(ND) Difference Algorithm and Its Variations", 1986: the middle snake, divide
//! and conquer), after stripping the common prefix and suffix, then a pass that moves
//! each run of changed lines where equal lines let it slide (D1205): up to merge with
//! the run before, then down to where it ends next to a change in the other file, else
//! as far down as it goes. The forward search tries diagonals from the deletion side
//! first. Both choices were tuned against GNU diff's output (run as an oracle on
//! generated and real file pairs); they agree on nearly all real edits. Written for psh
//! from the paper.

/// One difference: lines `a.0..a.1` of the first sequence are replaced by `b.0..b.1` of
/// the second (either range may be empty).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    pub a: (usize, usize),
    pub b: (usize, usize),
}

/// The changes turning `a` into `b`, in order.
pub fn diff(a: &[u32], b: &[u32]) -> Vec<Change> {
    let mut ca = vec![false; a.len()];
    let mut cb = vec![false; b.len()];
    let ids = a.iter().chain(b).max().map_or(0, |&m| m as usize + 1);
    let mut in_a = vec![false; ids];
    let mut in_b = vec![false; ids];
    a.iter().for_each(|&x| in_a[x as usize] = true);
    b.iter().for_each(|&x| in_b[x as usize] = true);
    let (ka, ia) = matched(a, &in_b, &mut ca);
    let (kb, ib) = matched(b, &in_a, &mut cb);
    let mut cka = vec![false; ka.len()];
    let mut ckb = vec![false; kb.len()];
    let n = ka.len().max(kb.len()) * 2 + 2;
    let mut vf = vec![0isize; n + 2];
    let mut vb = vec![0isize; n + 2];
    compare(
        &ka,
        &kb,
        0,
        ka.len(),
        0,
        kb.len(),
        &mut cka,
        &mut ckb,
        &mut vf,
        &mut vb,
    );
    ia.iter().zip(&cka).for_each(|(&i, &c)| ca[i] = c);
    ib.iter().zip(&ckb).for_each(|(&i, &c)| cb[i] = c);
    compact(a, &mut ca, &cb);
    compact(b, &mut cb, &ca);
    changes(&ca, &cb)
}

fn matched(lines: &[u32], other: &[bool], changed: &mut [bool]) -> (Vec<u32>, Vec<usize>) {
    let mut kept = Vec::with_capacity(lines.len());
    let mut at = Vec::with_capacity(lines.len());
    for (i, &x) in lines.iter().enumerate() {
        if other[x as usize] {
            kept.push(x);
            at.push(i);
        } else {
            changed[i] = true;
        }
    }
    (kept, at)
}

#[allow(clippy::too_many_arguments)]
fn compare(
    a: &[u32],
    b: &[u32],
    mut alo: usize,
    mut ahi: usize,
    mut blo: usize,
    mut bhi: usize,
    ca: &mut [bool],
    cb: &mut [bool],
    vf: &mut [isize],
    vb: &mut [isize],
) {
    while alo < ahi && blo < bhi && a[alo] == b[blo] {
        alo += 1;
        blo += 1;
    }
    while alo < ahi && blo < bhi && a[ahi - 1] == b[bhi - 1] {
        ahi -= 1;
        bhi -= 1;
    }
    if alo == ahi {
        cb[blo..bhi].iter_mut().for_each(|c| *c = true);
        return;
    }
    if blo == bhi {
        ca[alo..ahi].iter_mut().for_each(|c| *c = true);
        return;
    }
    let (x, y) = middle_snake(&a[alo..ahi], &b[blo..bhi], vf, vb);
    compare(a, b, alo, alo + x, blo, blo + y, ca, cb, vf, vb);
    compare(a, b, alo + x, ahi, blo + y, bhi, ca, cb, vf, vb);
}

/// The edit distance from which `middle_snake` stops looking for a minimal script.
const COST_LIMIT: isize = 2048;

/// A point on an optimal path through the middle of the edit graph of `a` and `b` (both
/// non-empty, first and last elements different): splitting there halves the problem.
fn middle_snake(a: &[u32], b: &[u32], vf: &mut [isize], vb: &mut [isize]) -> (usize, usize) {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let delta = n - m;
    let odd = delta & 1 != 0;
    let max = (n + m + 1) / 2;
    // Diagonal k is stored at index k + off.
    let off = max + 1;
    let idx = |k: isize| (k + off) as usize;
    vf[idx(1)] = 0;
    vb[idx(1)] = 0;
    for d in 0..=max {
        // Forward: furthest x on diagonal k = x - y, deletion side first.
        let mut k = d;
        while k >= -d {
            let mut x = if k == -d || (k != d && vf[idx(k - 1)] < vf[idx(k + 1)]) {
                vf[idx(k + 1)]
            } else {
                vf[idx(k - 1)] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            vf[idx(k)] = x;
            // The reverse search's diagonal for k is delta - k.
            if odd && (delta - k).abs() < d && x + vb[idx(delta - k)] >= n {
                return (x as usize, y as usize);
            }
            k -= 2;
        }
        // Very different inputs: give up minimality and split at the forward point that
        // got furthest (our heuristic; it bounds the work at about COST_LIMIT * (N+M)).
        if d >= COST_LIMIT {
            let mut best = (0isize, 0isize);
            let mut k = d;
            while k >= -d {
                let x = vf[idx(k)].min(n);
                let y = (x - k).clamp(0, m);
                if x + y > best.0 + best.1 && (x, y) != (n, m) {
                    best = (x, y);
                }
                k -= 2;
            }
            if best != (0, 0) {
                return (best.0 as usize, best.1 as usize);
            }
        }
        // Reverse: furthest distance from the end on reversed diagonal k.
        let mut k = -d;
        while k <= d {
            let mut x = if k == -d || (k != d && vb[idx(k - 1)] < vb[idx(k + 1)]) {
                vb[idx(k + 1)]
            } else {
                vb[idx(k - 1)] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[(n - x - 1) as usize] == b[(m - y - 1) as usize] {
                x += 1;
                y += 1;
            }
            vb[idx(k)] = x;
            if !odd && (delta - k).abs() <= d && x + vf[idx(delta - k)] >= n {
                return ((n - x) as usize, (m - y) as usize);
            }
            k += 2;
        }
    }
    // Unreachable for valid inputs: the paths meet by D = ceil((N+M)/2).
    (a.len(), b.len())
}

/// Moves each run of changed lines of one file (`changed`) along equal lines, which
/// keeps the matched lines' contents and order: up as far as it goes (merging with the
/// run before), then down (merging with the runs after); it ends at the last position
/// where it meets a change of the other file (`other`), else at the bottom.
fn compact(lines: &[u32], changed: &mut [bool], other: &[bool]) {
    let n = lines.len();
    // The other file's unchanged lines: the u-th pairs with this file's u-th.
    let opos: Vec<usize> = (0..other.len()).filter(|&i| !other[i]).collect();
    let other_changed_at = |u: usize| {
        let lo = if u > 0 { opos[u - 1] + 1 } else { 0 };
        let hi = opos.get(u).copied().unwrap_or(other.len());
        hi > lo
    };
    // Unchanged lines before `i`.
    let mut u = 0;
    let mut i = 0;
    while i < n {
        if !changed[i] {
            u += 1;
            i += 1;
            continue;
        }
        let (mut start, mut end) = (i, i);
        while end < n && changed[end] {
            end += 1;
        }
        // `u` counts the unchanged lines before `start` throughout.
        let mut aligned_end;
        loop {
            let size = end - start;
            while start > 0 && lines[start - 1] == lines[end - 1] {
                start -= 1;
                end -= 1;
                changed[start] = true;
                changed[end] = false;
                u -= 1;
                while start > 0 && changed[start - 1] {
                    start -= 1;
                }
            }
            aligned_end = other_changed_at(u).then_some(end);
            while end < n && lines[start] == lines[end] {
                changed[start] = false;
                changed[end] = true;
                start += 1;
                end += 1;
                u += 1;
                while end < n && changed[end] {
                    end += 1;
                }
                if other_changed_at(u) {
                    aligned_end = Some(end);
                }
            }
            if end - start == size {
                break;
            }
        }
        if let Some(target) = aligned_end {
            while end > target {
                start -= 1;
                end -= 1;
                changed[start] = true;
                changed[end] = false;
                u -= 1;
            }
        }
        i = end;
    }
}

fn changes(ca: &[bool], cb: &[bool]) -> Vec<Change> {
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < ca.len() || j < cb.len() {
        if i < ca.len() && j < cb.len() && !ca[i] && !cb[j] {
            i += 1;
            j += 1;
            continue;
        }
        let (si, sj) = (i, j);
        while i < ca.len() && ca[i] {
            i += 1;
        }
        while j < cb.len() && cb[j] {
            j += 1;
        }
        if si == i && sj == j {
            // Only one side left, all unchanged: cannot happen with consistent flags.
            break;
        }
        out.push(Change {
            a: (si, i),
            b: (sj, j),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(s: &str) -> Vec<u32> {
        s.bytes().map(u32::from).collect()
    }

    /// Applies the changes to `a`, which must give `b`.
    fn check(a: &str, b: &str) -> Vec<Change> {
        let (x, y) = (ids(a), ids(b));
        let cs = diff(&x, &y);
        let mut out = Vec::new();
        let mut pos = 0;
        for c in &cs {
            out.extend_from_slice(&x[pos..c.a.0]);
            out.extend_from_slice(&y[c.b.0..c.b.1]);
            pos = c.a.1;
        }
        out.extend_from_slice(&x[pos..]);
        assert_eq!(out, y, "{a} -> {b}");
        // Minimal: the unchanged count is the LCS length.
        cs
    }

    fn lcs(a: &[u32], b: &[u32]) -> usize {
        let mut t = vec![vec![0usize; b.len() + 1]; a.len() + 1];
        for i in (0..a.len()).rev() {
            for j in (0..b.len()).rev() {
                t[i][j] = if a[i] == b[j] {
                    t[i + 1][j + 1] + 1
                } else {
                    t[i + 1][j].max(t[i][j + 1])
                };
            }
        }
        t[0][0]
    }

    #[test]
    fn edits() {
        assert_eq!(check("abc", "abc"), []);
        assert_eq!(
            check("abc", "abbc"),
            [Change {
                a: (2, 2),
                b: (2, 3)
            }]
        );
        assert_eq!(
            check("abab", "ab"),
            [Change {
                a: (2, 4),
                b: (2, 2)
            }]
        );
        check("", "xyz");
        check("xyz", "");
        check("abcabba", "cbabac");
        check("12345", "45123");
    }

    #[test]
    fn minimal_on_many_inputs() {
        // A small deterministic generator over a three-letter alphabet.
        let mut seed = 12345u32;
        let mut next = || {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            seed >> 16
        };
        for _ in 0..300 {
            let la = (next() % 12) as usize;
            let lb = (next() % 12) as usize;
            let a: Vec<u32> = (0..la).map(|_| next() % 3).collect();
            let b: Vec<u32> = (0..lb).map(|_| next() % 3).collect();
            let cs = diff(&a, &b);
            let changed: usize = cs.iter().map(|c| c.a.1 - c.a.0).sum();
            assert_eq!(a.len() - changed, lcs(&a, &b), "{a:?} {b:?}");
            let s = |v: &[u32]| {
                v.iter()
                    .map(|&x| char::from(b'a' + x as u8))
                    .collect::<String>()
            };
            check(&s(&a), &s(&b));
        }
    }
}
