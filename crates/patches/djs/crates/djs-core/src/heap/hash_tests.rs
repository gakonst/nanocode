//! The keyed hash (D2433): keys built to collide under the fixed hash spread out in the
//! VM's own atom index and in Map/Set, and a new key keeps every lookup working.

use alloc::string::String;
use alloc::vec::Vec;

use super::cells::AtomTable;
use super::*;
use crate::value::Value;

/// One step of the fixed multiply-rotate hash djs used before D2433 (FxHash-style).
fn step(s: u32, u: u8) -> u32 {
    (s.rotate_left(5) ^ u as u32).wrapping_mul(0x9E37_79B9)
}

/// That hash of an ASCII string: the steps, then the murmur3 avalanche.
fn fixed_hash(k: &str) -> u32 {
    let mut h = k.bytes().fold(0, step);
    h ^= h >> 16;
    h = h.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 13;
    h = h.wrapping_mul(0xC2B2_AE35);
    h ^ (h >> 16)
}

/// `2^blocks` printable ASCII strings with the same fixed hash: each block is a pair of
/// six-character pieces that reach the same state from the state before them (a birthday
/// search over random pieces), and any choice of piece per block gives the same final
/// state. What an attacker who knows the hash can build offline.
fn colliding_keys(blocks: u32) -> Vec<String> {
    const N: usize = 1 << 18;
    let mut rng = 0x2545_F491_4F6C_DD1Du64;
    let mut piece = || -> [u8; 6] {
        let mut p = [0u8; 6];
        for b in &mut p {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            *b = b'!' + (rng % 94) as u8;
        }
        p
    };
    let mut state = 0u32;
    let mut pairs = Vec::new();
    for _ in 0..blocks {
        let pieces: Vec<[u8; 6]> = (0..N).map(|_| piece()).collect();
        let mut ends: Vec<(u32, u32)> = pieces
            .iter()
            .enumerate()
            .map(|(i, p)| (p.iter().fold(state, |s, &u| step(s, u)), i as u32))
            .collect();
        ends.sort_unstable();
        let (s, x, y) = ends
            .windows(2)
            .find(|w| w[0].0 == w[1].0 && pieces[w[0].1 as usize] != pieces[w[1].1 as usize])
            .map(|w| (w[0].0, pieces[w[0].1 as usize], pieces[w[1].1 as usize]))
            .expect("a colliding pair among 2^18 pieces");
        pairs.push((x, y));
        state = s;
    }
    (0..1u32 << blocks)
        .map(|bits| {
            let mut k = String::new();
            for (i, (x, y)) in pairs.iter().enumerate() {
                let piece = if bits >> i & 1 == 0 { x } else { y };
                k.extend(piece.iter().map(|&b| b as char));
            }
            k
        })
        .collect()
}

/// The longest probe run in a hash index: how far an entry sits from its home bucket.
fn longest_probe(h: &Heap, index: u32, hash_of: impl Fn(u32) -> u32) -> u32 {
    let len = h.u32_array_len(index);
    (0..len)
        .filter_map(|b| {
            let v = h.u32_array_get(index, b);
            (v != 0).then(|| b.wrapping_sub(hash_of(v)) & (len - 1))
        })
        .max()
        .unwrap_or(0)
}

#[test]
fn colliding_keys_spread_out_under_the_keyed_hash() {
    let keys = colliding_keys(11);
    assert_eq!(keys.len(), 2048);
    let fixed = fixed_hash(&keys[0]);
    assert!(keys.iter().all(|k| fixed_hash(k) == fixed));
    let mut h = Heap::new().unwrap();
    // As the VM's own atoms (a heap without a base has one level, the VM's).
    let atoms: Vec<Atom> = keys.iter().map(|k| h.atom_str(k).unwrap()).collect();
    for (k, &a) in keys.iter().zip(&atoms) {
        assert_eq!(h.atom_str(k).unwrap(), a);
    }
    let table = h.atom_table();
    let index = h.arena().get::<Ref>(table, AtomTable::INDEX).0;
    let probe = longest_probe(&h, index, |id| {
        let s = h.atom_entry_at(id).unwrap().as_string().unwrap();
        h.str_hash_uncached(s)
    });
    assert!(probe < 40, "longest atom probe run {probe}");
    // As Map keys.
    let proto = h.alloc_object(ObjectClass::Ordinary, None, 0).unwrap();
    let m = h.collection_new(ObjectClass::Map, proto).unwrap();
    for (i, k) in keys.iter().enumerate() {
        let s = h.alloc_str(k).unwrap();
        h.collection_set(m, Value::string(s), Value::int(i as i32))
            .unwrap();
    }
    let index = h.collection_index(m);
    let entries = h.collection_entries(m);
    let probe = longest_probe(&h, index, |n| h.key_hash(h.coll_key(entries, 2, n - 1)));
    assert!(probe < 40, "longest Map probe run {probe}");
    for (i, k) in keys.iter().enumerate() {
        let s = h.alloc_str(k).unwrap();
        assert_eq!(
            h.collection_get(m, Value::string(s)),
            Some(Value::int(i as i32))
        );
    }
}

#[test]
fn a_new_key_keeps_atoms_maps_and_order() {
    let mut h = Heap::new().unwrap();
    let proto = h.alloc_object(ObjectClass::Ordinary, None, 0).unwrap();
    let m = h.collection_new(ObjectClass::Map, proto).unwrap();
    let keys: Vec<String> = (0..300).map(|i| alloc::format!("key_{i}")).collect();
    let mut atoms = Vec::new();
    for (i, k) in keys.iter().enumerate() {
        atoms.push(h.atom_str(k).unwrap());
        let s = h.alloc_str(k).unwrap();
        h.str_hash(s);
        h.collection_set(m, Value::string(s), Value::int(i as i32))
            .unwrap();
        h.collection_set(m, Value::number(i as f64 + 0.5), Value::int(-(i as i32)))
            .unwrap();
    }
    let order = h.collection_entries_list(m);
    for key in [1u64, 0xDEAD_BEEF, u64::MAX] {
        h.set_hash_key(key);
        assert_eq!(h.hash_key(), key);
        for (i, k) in keys.iter().enumerate() {
            assert_eq!(h.atom_str(k).unwrap(), atoms[i]);
            let s = h.alloc_str(k).unwrap();
            assert_eq!(
                h.collection_get(m, Value::string(s)),
                Some(Value::int(i as i32))
            );
            assert_eq!(
                h.collection_get(m, Value::number(i as f64 + 0.5)),
                Some(Value::int(-(i as i32)))
            );
        }
        assert_eq!(h.collection_entries_list(m), order, "insertion order");
    }
    h.verify().unwrap();
}
