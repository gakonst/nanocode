//! Append ropes and buffers (D1610): `s += piece` in place, its invariants, the GC and
//! heap images.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::cells::{Rope, flags};
use super::layout::Ref;
use super::*;
use crate::value::{Str, Value};

fn s(h: &mut Heap, t: &str) -> Str {
    h.alloc_str(t).unwrap()
}

/// The buffer under an append rope.
fn buffer(h: &Heap, v: Str) -> Option<u32> {
    (h.cell_kind(v.0) == CellKind::Rope && h.cell_flags(v.0) & flags::ROPE_APPEND != 0)
        .then(|| h.arena().get(v.0, Rope::LEFT).0)
}

fn appendable(h: &Heap, buf: u32) -> bool {
    h.cell_flags(buf) & flags::STR_APPEND_BUF != 0
}

#[test]
fn appends_share_one_buffer() {
    let mut h = Heap::new().unwrap();
    let mut acc = s(&mut h, &"#".repeat(300));
    let x = s(&mut h, "x");
    let mut expect = "#".repeat(300);
    let mut buffers = Vec::new();
    for _ in 0..10_000 {
        acc = h.concat(acc, x).unwrap();
        expect.push('x');
        if let Some(b) = buffer(&h, acc)
            && buffers.last() != Some(&b)
        {
            buffers.push(b);
        }
    }
    assert_eq!(h.str_to_string(acc), expect);
    // Doubling: a handful of buffers for 10k appends, and the last holds everything.
    assert!(buffers.len() < 12, "{} buffers", buffers.len());
    let last = *buffers.last().unwrap();
    assert_eq!(h.str_len(Str(last)), h.str_len(acc));
    assert!(
        h.str_view(acc).is_some(),
        "an append rope is viewed in place"
    );
    h.verify().unwrap();
}

#[test]
fn an_older_shorter_rope_never_writes_into_the_buffer() {
    // (b): two ropes over one buffer; only the one at the buffer's end extends it.
    let mut h = Heap::new().unwrap();
    let base = s(&mut h, &"a".repeat(300));
    let b = s(&mut h, "b");
    let c = s(&mut h, "c");
    let r1 = h.concat(base, b).unwrap(); // a plain rope
    let r2 = h.concat(r1, b).unwrap(); // starts a buffer
    let buf = buffer(&h, r2).unwrap();
    let r3 = h.concat(r2, b).unwrap(); // in place
    assert_eq!(buffer(&h, r3), Some(buf));
    // r2 is now shorter than the buffer's used part: appending to it must copy.
    let fork = h.concat(r2, c).unwrap();
    assert_ne!(buffer(&h, fork), Some(buf));
    assert_eq!(h.str_to_string(r2), format!("{}bb", "a".repeat(300)));
    assert_eq!(h.str_to_string(r3), format!("{}bbb", "a".repeat(300)));
    assert_eq!(h.str_to_string(fork), format!("{}bbc", "a".repeat(300)));
    // The newest still extends in place, and the fork has its own buffer.
    let r4 = h.concat(r3, c).unwrap();
    assert_eq!(buffer(&h, r4), Some(buf));
    let fork2 = h.concat(fork, c).unwrap();
    assert_eq!(buffer(&h, fork2), buffer(&h, fork));
    assert_eq!(h.str_to_string(r3), format!("{}bbb", "a".repeat(300)));
    assert_eq!(h.str_to_string(fork2), format!("{}bbcc", "a".repeat(300)));
    h.verify().unwrap();
}

#[test]
fn a_frozen_buffer_is_never_appended_to() {
    // (c): flatten hands out the buffer only after freezing it.
    let mut h = Heap::new().unwrap();
    let base = s(&mut h, &"q".repeat(300));
    let d = s(&mut h, "d");
    let mut r = h.concat(base, d).unwrap();
    r = h.concat(r, d).unwrap();
    r = h.concat(r, d).unwrap();
    let buf = buffer(&h, r).unwrap();
    let older = h.concat(base, d).unwrap();
    let flat = h.flatten(r).unwrap();
    assert_eq!(flat.0, buf, "the whole buffer is handed out");
    assert!(!appendable(&h, buf), "and frozen");
    let more = h.concat(r, d).unwrap();
    assert_ne!(buffer(&h, more), Some(buf));
    assert_eq!(h.str_len(flat), 303);
    assert_eq!(h.str_to_string(flat), format!("{}ddd", "q".repeat(300)));
    assert_eq!(h.str_to_string(more), format!("{}dddd", "q".repeat(300)));
    assert_eq!(h.str_to_string(older), format!("{}d", "q".repeat(300)));
    // A shorter rope over a buffer flattens to a copy.
    let mut p = h.concat(base, d).unwrap();
    p = h.concat(p, d).unwrap();
    let pbuf = buffer(&h, p).unwrap();
    let longer = h.concat(p, d).unwrap();
    let pflat = h.flatten(p).unwrap();
    assert_ne!(pflat.0, pbuf);
    assert!(appendable(&h, pbuf));
    let longest = h.concat(longer, d).unwrap();
    assert_eq!(buffer(&h, longest), Some(pbuf));
    h.verify().unwrap();
}

#[test]
fn widening_moves_to_a_two_byte_buffer() {
    let mut h = Heap::new().unwrap();
    let mut acc = s(&mut h, &"z".repeat(300));
    let x = s(&mut h, "x");
    for _ in 0..3 {
        acc = h.concat(acc, x).unwrap();
    }
    let snow = s(&mut h, "\u{2603}");
    let wide = h.concat(acc, snow).unwrap();
    let wide = h.concat(wide, x).unwrap();
    assert!(h.str_is_two_byte(wide));
    assert!(!h.str_is_two_byte(acc));
    assert_eq!(
        h.str_to_string(wide),
        format!("{}xxx\u{2603}x", "z".repeat(300))
    );
    assert_eq!(h.str_to_string(acc), format!("{}xxx", "z".repeat(300)));
    let wbuf = buffer(&h, wide).unwrap();
    assert_eq!(h.cell_kind(wbuf), CellKind::Str16);
    h.verify().unwrap();
}

#[test]
fn verify_rejects_a_buffer_outside_an_append_rope() {
    // (a): a value, or any other ref, pointing at an unfrozen buffer is a broken heap.
    let mut h = Heap::new().unwrap();
    let base = s(&mut h, &"v".repeat(300));
    let e = s(&mut h, "e");
    let r = h.concat(base, e).unwrap();
    let r = h.concat(r, e).unwrap();
    let buf = buffer(&h, r).unwrap();
    let hd = h.new_handle(Value::string(r)).unwrap();
    h.verify().unwrap();
    let escaped = h.new_handle(Value::string(Str(buf))).unwrap();
    assert!(h.verify().is_err(), "a value pointing at a buffer");
    h.set_handle(escaped, Value::UNDEFINED);
    h.verify().unwrap();
    // A concatenation rope with the buffer as a child.
    let plain = h.concat(base, base).unwrap();
    let left = h.arena().get(plain.0, Rope::LEFT);
    h.arena_mut().set(plain.0, Rope::LEFT, Ref(buf));
    let keep = h.new_handle(Value::string(plain)).unwrap();
    // (The lengths still add up when the buffer is as long as `base`; make sure.)
    let ok_len = h.str_len(Str(buf)) == h.str_len(base);
    assert!(h.verify().is_err() || !ok_len);
    h.arena_mut().set(plain.0, Rope::LEFT, left);
    h.verify().unwrap();
    let _ = (hd, keep);
}

/// Builds the same strings in a fresh heap.
fn build(h: &mut Heap) -> Vec<super::handle::Handle> {
    let mut hs = Vec::new();
    let mut acc = s(h, &"line\n".repeat(60));
    for i in 0..2000 {
        let piece = s(h, &format!("{i}:{}\n", "*".repeat(i % 7)));
        acc = h.concat(acc, piece).unwrap();
        if i % 500 == 0 {
            hs.push(h.new_handle(Value::string(acc)).unwrap());
        }
    }
    let snow = s(h, "\u{2603}");
    let wide = h.concat(acc, snow).unwrap();
    hs.push(h.new_handle(Value::string(acc)).unwrap());
    hs.push(h.new_handle(Value::string(wide)).unwrap());
    hs
}

fn strings(h: &Heap, hs: &[super::handle::Handle]) -> Vec<String> {
    hs.iter()
        .map(|&x| h.str_to_string(h.get_handle(x).as_string().unwrap()))
        .collect()
}

#[test]
fn collection_is_deterministic_with_live_buffers() {
    let mut h = Heap::new().unwrap();
    let hs = build(&mut h);
    let before = strings(&h, &hs);
    h.collect().unwrap();
    let once = h.bytes().to_vec();
    h.collect().unwrap();
    assert_eq!(
        h.bytes(),
        &once[..],
        "collecting twice gives identical bytes"
    );
    assert_eq!(strings(&h, &hs), before);
    h.verify().unwrap();
    // The same operations in another heap give the same image.
    let mut h2 = Heap::new().unwrap();
    let hs2 = build(&mut h2);
    h2.collect().unwrap();
    assert_eq!(h2.bytes(), &once[..]);
    assert_eq!(strings(&h2, &hs2), before);
    // Appending after the collection still works in place and keeps old values.
    let last = h.get_handle(hs[hs.len() - 2]).as_string().unwrap();
    let tail = s(&mut h, "tail");
    let more = h.concat(last, tail).unwrap();
    assert!(h.str_to_string(more).ends_with("tail"));
    assert_eq!(strings(&h, &hs), before);
    h.verify().unwrap();
}

#[test]
fn images_round_trip_with_live_buffers() {
    let mut h = Heap::new().unwrap();
    let hs = build(&mut h);
    let before = strings(&h, &hs);
    let image = h.bytes().to_vec();
    let mut back = Heap::from_bytes(image, 1 << 30).unwrap();
    assert_eq!(strings(&back, &hs), before);
    // The restored buffer keeps working: in place for the newest, a copy for older ones.
    let newest = back.get_handle(hs[hs.len() - 2]).as_string().unwrap();
    let older = back.get_handle(hs[0]).as_string().unwrap();
    let x = s(&mut back, "!");
    let a = back.concat(newest, x).unwrap();
    let b = back.concat(older, x).unwrap();
    assert!(back.str_to_string(a).ends_with("!"));
    assert!(back.str_to_string(b).ends_with("!"));
    assert_eq!(strings(&back, &hs), before);
    back.verify().unwrap();
}
