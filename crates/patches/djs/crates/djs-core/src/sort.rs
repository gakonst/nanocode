//! One small stable sort (0043): `core`'s slice sorts are fast but monomorphize several
//! kilobytes of wasm per element type (small-sort networks, pivot selection), and none of
//! djs's sorts is hot enough to pay for that.

use alloc::vec;
use alloc::vec::Vec;

/// Sorts `v` stably by `less`: a bottom-up merge sort of indexes, then one permutation.
pub(crate) fn sort_by<T>(v: &mut Vec<T>, mut less: impl FnMut(&T, &T) -> bool) {
    let n = v.len();
    if n < 2 {
        return;
    }
    let mut idx: Vec<u32> = (0..n as u32).collect();
    let mut tmp: Vec<u32> = vec![0; n];
    let mut width = 1;
    while width < n {
        let mut i = 0;
        while i < n {
            let mid = (i + width).min(n);
            let end = (i + 2 * width).min(n);
            let (mut a, mut b) = (i, mid);
            for slot in &mut tmp[i..end] {
                // Take from the right run only when strictly less: stable.
                let right = b < end && (a >= mid || less(&v[idx[b] as usize], &v[idx[a] as usize]));
                if right {
                    *slot = idx[b];
                    b += 1;
                } else {
                    *slot = idx[a];
                    a += 1;
                }
            }
            i = end;
        }
        core::mem::swap(&mut idx, &mut tmp);
        width *= 2;
    }
    let mut items: Vec<Option<T>> = v.drain(..).map(Some).collect();
    v.extend(
        idx.iter()
            .map(|&i| items[i as usize].take().expect("a permutation")),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_stably() {
        for n in 0..70u32 {
            let mut v: Vec<(u32, u32)> = (0..n).map(|i| ((i * 7919) % 13, i)).collect();
            let mut want = v.clone();
            want.sort_by_key(|e| e.0);
            sort_by(&mut v, |a, b| a.0 < b.0);
            assert_eq!(v, want);
        }
    }
}
