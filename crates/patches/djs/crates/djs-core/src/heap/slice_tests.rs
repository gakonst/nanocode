//! Slices (D2542): substrings that share their string's units, how they read, flatten,
//! verify, collect and copy canonically.

use alloc::string::String;
use alloc::vec::Vec;

use super::cells::{Header, Slice, flags};
use super::compactor::{SLICE_KEEP_LEN, SLICE_KEEP_SHARE};
use super::string::{SLICE_MIN_LEN, SLICE_PARENT_MIN_LEN};
use super::*;
use crate::value::{Str, Value};

/// A one-byte string of `n` units: a pattern, so that slices at different places differ.
fn text(n: usize) -> String {
    (0..n).map(|i| (b'a' + (i % 26) as u8) as char).collect()
}

fn slice(h: &mut Heap, s: Str, a: u32, b: u32) -> Str {
    h.substring_slice(s, a, b).unwrap().expect("sliced")
}

fn kind(h: &Heap, s: Str) -> CellKind {
    h.cell_kind(s.0)
}

#[test]
fn a_slice_reads_like_a_copy() {
    let mut h = Heap::new().unwrap();
    let t = text(1000);
    let p = h.alloc_str(&t).unwrap();
    let s = slice(&mut h, p, 100, 400);
    assert_eq!(kind(&h, s), CellKind::Slice);
    let copy = h.alloc_str(&t[100..400]).unwrap();
    assert_eq!(h.str_len(s), 300);
    assert_eq!(h.str_to_string(s), &t[100..400]);
    assert!(h.str_eq(s, copy) && h.str_eq(copy, s));
    assert_eq!(h.str_cmp(s, copy), core::cmp::Ordering::Equal);
    assert_eq!(h.str_hash(s), h.str_hash(copy));
    assert_eq!(h.char_code_at(s, 0), Some(t.as_bytes()[100] as u16));
    assert_eq!(h.char_code_at(s, 299), Some(t.as_bytes()[399] as u16));
    assert_eq!(h.char_code_at(s, 300), None);
    assert!(!h.str_is_two_byte(s));
    assert_eq!(h.str_view(s).unwrap().bytes(), &t.as_bytes()[100..400]);
    // A slice of a slice reads the same parent.
    let ss = slice(&mut h, s, 10, 60);
    assert_eq!(h.arena().get(ss.0, Slice::PARENT).0, p.0);
    assert_eq!(h.arena().get(ss.0, Slice::OFF), 110);
    assert_eq!(h.str_to_string(ss), &t[110..160]);
    // Concatenations: short (a flat copy), rope, append buffer.
    let x = h.alloc_str("xy").unwrap();
    let short = slice(&mut h, p, 0, SLICE_MIN_LEN);
    let r = h.concat(short, x).unwrap();
    assert_eq!(
        h.str_to_string(r),
        [&t[..SLICE_MIN_LEN as usize], "xy"].concat()
    );
    let mut acc = h.concat(s, ss).unwrap();
    let mut want = [&t[100..400], &t[110..160]].concat();
    for _ in 0..200 {
        acc = h.concat(acc, ss).unwrap();
        want.push_str(&t[110..160]);
    }
    assert_eq!(h.str_to_string(acc), want);
    h.verify().unwrap();
    // Flattening copies once and redirects the slice (now a flattened rope).
    let f = h.flatten(s).unwrap();
    assert_eq!(kind(&h, f), CellKind::Str8);
    assert_eq!(kind(&h, s), CellKind::Rope);
    assert!(h.cell_flags(s.0) & flags::ROPE_FLATTENED != 0);
    assert_eq!(h.str_to_string(s), &t[100..400]);
    assert_eq!(h.flatten(s).unwrap(), f);
    // A slice as an atom is a flat copy.
    let a = h.intern(ss).unwrap();
    let as_ = h.atom_string(a).unwrap();
    assert_eq!(kind(&h, as_), CellKind::Str8);
    h.verify().unwrap();
}

#[test]
fn short_pieces_short_strings_and_two_byte_strings_are_copied() {
    let mut h = Heap::new().unwrap();
    let p = h.alloc_str(&text(1000)).unwrap();
    assert_eq!(h.substring_slice(p, 0, SLICE_MIN_LEN - 1).unwrap(), None);
    let small = h
        .alloc_str(&text(SLICE_PARENT_MIN_LEN as usize - 1))
        .unwrap();
    assert_eq!(h.substring_slice(small, 0, 100).unwrap(), None);
    let wide = h.alloc_str(&"→".repeat(500)).unwrap();
    assert_eq!(h.substring_slice(wide, 0, 100).unwrap(), None);
    // A concatenation is not sliced; flattened, it is (a flattened rope stands for its
    // flat string).
    let half = h.alloc_str(&text(300)).unwrap();
    let rope = h.concat(half, half).unwrap();
    assert_eq!(h.substring_slice(rope, 250, 350).unwrap(), None);
    h.flatten(rope).unwrap();
    let s = slice(&mut h, rope, 250, 350);
    assert_eq!(kind(&h, s), CellKind::Slice);
    let t = text(300);
    assert_eq!(h.str_to_string(s), [&t[250..], &t[..50]].concat());
}

#[test]
fn verify_checks_a_slices_range_and_parent() {
    let mut h = Heap::new().unwrap();
    let p = h.alloc_str(&text(1000)).unwrap();
    let s = slice(&mut h, p, 900, 1000);
    h.new_handle(Value::string(s)).unwrap();
    h.verify().unwrap();
    h.arena_mut().set(s.0, Slice::OFF, 901);
    assert!(h.verify().is_err());
    h.arena_mut().set(s.0, Slice::OFF, 900);
    let o = h
        .alloc_cell(CellKind::Rope, Slice::LAYOUT.min_size)
        .unwrap();
    h.arena_mut().set(s.0, Slice::PARENT, super::layout::Ref(o));
    assert!(h.verify().is_err());
}

/// A heap holding a parent of `parent` units only through slices of `pieces` units each.
fn only_slices(parent: u32, pieces: &[u32]) -> (Heap, Vec<Handle>) {
    let mut h = Heap::new().unwrap();
    let p = h.alloc_str(&text(parent as usize)).unwrap();
    let mut hs = Vec::new();
    let mut at = 0;
    for &n in pieces {
        let s = slice(&mut h, p, at, at + n);
        hs.push(h.new_handle(Value::string(s)).unwrap());
        at += n;
    }
    (h, hs)
}

fn is_slice(h: &Heap, hd: Handle) -> bool {
    let s = h.get_handle(hd).as_string().unwrap();
    h.cell_kind(s.0) == CellKind::Slice
}

#[test]
fn a_full_collection_drops_a_parent_its_slices_barely_cover() {
    let n = 4 * SLICE_KEEP_LEN;
    let t = text(n as usize);
    // 64 units of 16K: copied out, and the parent goes at the next collection.
    let (mut h, hs) = only_slices(n, &[64]);
    h.collect().unwrap();
    assert!(!is_slice(&h, hs[0]));
    let s = h.get_handle(hs[0]).as_string().unwrap();
    assert_eq!(h.str_to_string(s), &t[..64]);
    assert_eq!(h.cell_flags(s.0) & flags::SLICE_DETACH, 0);
    h.verify().unwrap();
    // The same collection dropped the parent.
    assert!(h.arena().len() < n);
    let s = h.get_handle(hs[0]).as_string().unwrap();
    assert_eq!(h.str_to_string(s), &t[..64]);
    // A quarter covered: kept, the slices stay.
    let quarter = n / SLICE_KEEP_SHARE as u32;
    let (mut h, hs) = only_slices(n, &[quarter / 2, quarter / 2]);
    h.collect().unwrap();
    assert!(hs.iter().all(|&x| is_slice(&h, x)));
    h.verify().unwrap();
    // A parent below SLICE_KEEP_LEN is kept whatever covers it.
    let (mut h, hs) = only_slices(SLICE_KEEP_LEN - 1, &[40]);
    h.collect().unwrap();
    assert!(is_slice(&h, hs[0]));
    // A parent something else holds is kept too.
    let (mut h, hs) = only_slices(n, &[64]);
    let s = h.get_handle(hs[0]).as_string().unwrap();
    let p = super::layout::Ref::get(h.arena().get(s.0, Slice::PARENT)).unwrap();
    h.new_handle(Value::string(Str(p))).unwrap();
    h.collect().unwrap();
    assert!(is_slice(&h, hs[0]));
    // Minor collections keep parents.
    let (mut h, hs) = only_slices(n, &[64]);
    h.collect_minor().unwrap();
    assert!(is_slice(&h, hs[0]));
    let _ = Header::KIND;
}

#[test]
fn a_canonical_copy_writes_slices_as_flat_strings() {
    let t = text(2000);
    let image = |sliced: bool| {
        let mut h = Heap::new().unwrap();
        let p = h.alloc_str(&t).unwrap();
        h.new_handle(Value::string(p)).unwrap();
        let s = if sliced {
            slice(&mut h, p, 500, 900)
        } else {
            h.alloc_str(&t[500..900]).unwrap()
        };
        h.new_handle(Value::string(s)).unwrap();
        h.collect_canonical().unwrap();
        h.verify().unwrap();
        h.bytes().to_vec()
    };
    assert!(image(true) == image(false));
}

#[test]
fn json_parse_slices_long_plain_strings_of_a_long_text() {
    let mut h = Heap::new().unwrap();
    let long = text(100);
    let src = alloc::format!(
        r#"{{"a":"{long}","b":"short","c":"{long}\n","d":["{long}"],"e":"é{long}"}}"#
    );
    let mut src = src;
    while src.len() < SLICE_PARENT_MIN_LEN as usize {
        src.push(' ');
    }
    let input = h.alloc_str(&src).unwrap();
    let opts = crate::json::ParseOptions::default();
    let v = crate::json::parse(&mut h, input, &opts).unwrap();
    let o = v.as_object().unwrap();
    let get = |h: &mut Heap, k: &str| {
        let a = h.atom_str(k).unwrap();
        match h.get_own_property(o, a) {
            Some(object::Property::Data { value, .. }) => value.as_string().unwrap(),
            _ => panic!("no {k}"),
        }
    };
    let a = get(&mut h, "a");
    assert_eq!(kind(&h, a), CellKind::Slice);
    assert_eq!(h.str_to_string(a), long);
    // Short, escaped: copies. (`é` is one Latin-1 unit: still sliced.)
    let b = get(&mut h, "b");
    assert_eq!(kind(&h, b), CellKind::Str8);
    let c = get(&mut h, "c");
    assert_eq!(kind(&h, c), CellKind::Str8);
    assert_eq!(h.str_to_string(c), alloc::format!("{long}\n"));
    let e = get(&mut h, "e");
    assert_eq!(kind(&h, e), CellKind::Slice);
    assert_eq!(h.str_to_string(e), alloc::format!("é{long}"));
    // Parsing a slice reads it in place, and its strings slice the same parent.
    let outer = h
        .alloc_str(&alloc::format!(
            "{:300}[\"{long}\",\"{long}\",\"{long}\"]",
            ""
        ))
        .unwrap();
    let inner = h
        .substring_slice(outer, 300, h.str_len(outer))
        .unwrap()
        .unwrap();
    let v = crate::json::parse(&mut h, inner, &opts).unwrap();
    let x = h
        .get_element_fast(v.as_object().unwrap(), 0)
        .unwrap()
        .as_string()
        .unwrap();
    assert_eq!(kind(&h, x), CellKind::Slice);
    assert_eq!(h.arena().get(x.0, Slice::PARENT).0, outer.0);
    assert_eq!(h.arena().get(x.0, Slice::OFF), 302);
    assert_eq!(h.str_to_string(x), long);
    h.verify().unwrap();
}
