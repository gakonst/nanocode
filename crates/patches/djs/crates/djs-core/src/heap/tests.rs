//! Heap tests: strings, atoms, symbols, objects, arrays, walking, images, OOM.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::cmp::Ordering;

use super::cells::{Roots, flags};
use super::walk::{Slot, VerifyError};
use super::*;
use crate::value::{Kind, Obj, Str, Value};

fn heap() -> Heap {
    Heap::new().unwrap()
}

fn s(h: &mut Heap, t: &str) -> Str {
    h.alloc_str(t).unwrap()
}

/// A long rope: `n` single-character appends, the codemode `+=` pattern.
fn appended(h: &mut Heap, n: u32) -> (Str, String) {
    let mut acc = h.alloc_str("").unwrap();
    let mut expect = String::new();
    for i in 0..n {
        let c = (b'a' + (i % 26) as u8) as char;
        let piece = h.alloc_latin1(&[c as u8]).unwrap();
        acc = h.concat(acc, piece).unwrap();
        expect.push(c);
    }
    (acc, expect)
}

// ------------------------------------------------------------------ strings

#[test]
fn string_constructors() {
    let mut h = heap();
    let a = s(&mut h, "hello");
    assert_eq!(h.cell_kind(a.0), CellKind::Str8);
    assert_eq!(h.str_len(a), 5);
    assert_eq!(h.char_code_at(a, 1), Some(b'e' as u16));
    assert_eq!(h.char_code_at(a, 5), None);

    // Non-ASCII Latin-1 from UTF-8 stays one-byte.
    let e = s(&mut h, "café");
    assert_eq!(h.cell_kind(e.0), CellKind::Str8);
    assert_eq!(h.str_len(e), 4);
    assert_eq!(h.char_code_at(e, 3), Some(0xE9));

    // BMP above Latin-1 and astral (surrogate pair).
    let u = s(&mut h, "a€😀");
    assert_eq!(h.cell_kind(u.0), CellKind::Str16);
    assert_eq!(h.str_len(u), 4);
    assert_eq!(h.str_to_utf16(u), vec![0x61, 0x20AC, 0xD83D, 0xDE00]);
    assert_eq!(h.str_to_string(u), "a€😀");

    // UTF-16 input narrows when possible.
    let n = h.alloc_utf16(&[0x68, 0xE9]).unwrap();
    assert_eq!(h.cell_kind(n.0), CellKind::Str8);
    let n2 = h.alloc_latin1(&[0x68, 0xE9]).unwrap();
    assert!(h.str_eq(n, n2));

    // Lone surrogates survive in the heap, and are replaced only on conversion.
    let lone = h.alloc_utf16(&[0xD800, 0x41]).unwrap();
    assert_eq!(h.cell_kind(lone.0), CellKind::Str16);
    assert_eq!(h.char_code_at(lone, 0), Some(0xD800));
    assert_eq!(h.str_to_string(lone), "\u{FFFD}A");

    let empty = s(&mut h, "");
    assert_eq!(h.str_len(empty), 0);
    assert_eq!(h.str_to_string(empty), "");
    h.verify().unwrap();
}

#[test]
fn string_views() {
    let mut h = heap();
    let a = s(&mut h, "ab");
    let v = h.str_view(a).unwrap();
    assert!(!v.is_two_byte());
    assert_eq!(v.bytes(), b"ab");
    let u = s(&mut h, "€");
    let v = h.str_view(u).unwrap();
    assert!(v.is_two_byte());
    assert_eq!(v.bytes(), &[0xAC, 0x20]);
    assert_eq!(v.get(0), 0x20AC);
}

#[test]
fn string_equality_and_hash_across_representations() {
    let mut h = heap();
    let long = "the quick brown fox jumps over the lazy dog";
    let flat = s(&mut h, long);
    let l = s(&mut h, &long[..20]);
    let r = s(&mut h, &long[20..]);
    let rope = h.concat(l, r).unwrap();
    assert_eq!(h.cell_kind(rope.0), CellKind::Rope);
    // A rope of a different shape.
    let l2 = s(&mut h, &long[..5]);
    let r2a = s(&mut h, &long[5..33]);
    let r2b = s(&mut h, &long[33..]);
    let r2 = h.concat(r2a, r2b).unwrap();
    let rope2 = h.concat(l2, r2).unwrap();

    assert!(h.str_eq(flat, rope));
    assert!(h.str_eq(rope, rope2));
    assert_eq!(h.str_cmp(flat, rope2), Ordering::Equal);
    let hf = h.str_hash(flat);
    assert_eq!(h.str_hash_uncached(rope), hf);
    assert_eq!(h.str_hash(rope2), hf);
    assert!(h.str_eq_str(rope2, long));
    assert!(!h.str_eq_str(rope2, "the quick"));

    let other = s(&mut h, "the quick brown fox jumps over the lazy cat");
    assert!(!h.str_eq(flat, other));
    assert_ne!(h.str_hash(other), hf);

    // Same length, different width: never equal.
    let a = s(&mut h, "ab");
    let b = s(&mut h, "a€");
    assert!(!h.str_eq(a, b));
    h.verify().unwrap();
}

#[test]
fn string_ordering() {
    let mut h = heap();
    let pairs = [
        ("a", "b", Ordering::Less),
        ("ab", "a", Ordering::Greater),
        ("", "a", Ordering::Less),
        ("abc", "abc", Ordering::Equal),
        ("Z", "a", Ordering::Less),
        ("é", "z", Ordering::Greater),
        // Code unit order, not code point order: U+D83D (the lead surrogate) < U+FFFF.
        ("😀", "\u{FFFF}", Ordering::Less),
        ("€", "é", Ordering::Greater),
    ];
    for (x, y, o) in pairs {
        let (a, b) = (s(&mut h, x), s(&mut h, y));
        assert_eq!(h.str_cmp(a, b), o, "{x:?} vs {y:?}");
        assert_eq!(h.str_cmp(b, a), o.reverse(), "{y:?} vs {x:?}");
    }
    // Ropes compare by content.
    let (r, t) = appended(&mut h, 100);
    let f = s(&mut h, &t);
    assert_eq!(h.str_cmp(r, f), Ordering::Equal);
    let g = s(&mut h, &(t.clone() + "!"));
    assert_eq!(h.str_cmp(r, g), Ordering::Less);
}

#[test]
fn concat_short_is_flat_long_is_rope() {
    let mut h = heap();
    let a = s(&mut h, "foo");
    let b = s(&mut h, "bar");
    let ab = h.concat(a, b).unwrap();
    assert_eq!(h.cell_kind(ab.0), CellKind::Str8);
    assert!(h.str_eq_str(ab, "foobar"));

    let e = s(&mut h, "");
    assert_eq!(h.concat(a, e).unwrap(), a);
    assert_eq!(h.concat(e, b).unwrap(), b);

    // Widening copy: Latin-1 + UTF-16.
    let w = s(&mut h, "€");
    let aw = h.concat(a, w).unwrap();
    assert_eq!(h.cell_kind(aw.0), CellKind::Str16);
    assert_eq!(h.str_to_string(aw), "foo€");

    let x = s(&mut h, &"x".repeat(20));
    let y = s(&mut h, &"€".repeat(20));
    let xy = h.concat(x, y).unwrap();
    assert_eq!(h.cell_kind(xy.0), CellKind::Rope);
    assert!(h.str_is_two_byte(xy));
    assert_eq!(h.str_len(xy), 40);
    assert_eq!(h.char_code_at(xy, 19), Some(b'x' as u16));
    assert_eq!(h.char_code_at(xy, 20), Some(0x20AC));
    assert_eq!(h.char_code_at(xy, 40), None);
    let xx = h.concat(x, x).unwrap();
    assert!(!h.str_is_two_byte(xx));
    h.verify().unwrap();
}

#[test]
fn flatten_redirects_rope() {
    let mut h = heap();
    let x = s(&mut h, &"x".repeat(20));
    let y = s(&mut h, &"€".repeat(20));
    let rope = h.concat(x, y).unwrap();
    let hash = h.str_hash(rope);
    assert!(h.str_view(rope).is_none());
    let flat = h.flatten(rope).unwrap();
    assert_eq!(h.cell_kind(flat.0), CellKind::Str16);
    assert_ne!(flat, rope);
    assert!(h.cell_flags(rope.0) & flags::ROPE_FLATTENED != 0);
    // The rope now reads through the flat copy, and flattening again is free.
    assert!(h.str_view(rope).is_some());
    assert_eq!(h.flatten(rope).unwrap(), flat);
    assert_eq!(h.str_hash(flat), hash);
    assert_eq!(h.str_to_string(rope), "x".repeat(20) + &"€".repeat(20));
    // A rope over a flattened rope.
    let z = s(&mut h, &"z".repeat(40));
    let rz = h.concat(rope, z).unwrap();
    assert_eq!(h.str_len(rz), 80);
    assert_eq!(h.char_code_at(rz, 21), Some(0x20AC));
    assert_eq!(
        h.str_view_flat(rz).unwrap().len(),
        80,
        "flattened view has the full length"
    );
    h.verify().unwrap();
}

#[test]
fn deep_ropes_do_not_recurse() {
    let mut h = heap();
    // Left-deep (s += c) and right-deep (s = c + s) ropes.
    let (left, expect) = appended(&mut h, 100_000);
    assert_eq!(h.str_len(left), 100_000);
    assert_eq!(
        h.char_code_at(left, 99_999),
        Some(expect.as_bytes()[99_999] as u16)
    );
    let mut right = h.alloc_str("").unwrap();
    for c in expect.bytes().rev() {
        let piece = h.alloc_latin1(&[c]).unwrap();
        right = h.concat(piece, right).unwrap();
    }
    assert!(h.str_eq(left, right));
    assert_eq!(h.str_hash(left), h.str_hash(right));
    let flat = h.flatten(left).unwrap();
    assert!(h.str_eq_str(flat, &expect));
    assert_eq!(h.str_cmp(left, right), Ordering::Equal);
    h.verify().unwrap();
}

#[test]
fn string_length_limit() {
    let mut h = heap();
    let mut x = s(&mut h, &"x".repeat(32));
    // Doubling ropes is cheap; 2^30 code units is one past the limit.
    for _ in 0..24 {
        x = h.concat(x, x).unwrap();
    }
    assert_eq!(h.str_len(x), 1 << 29);
    assert_eq!(h.concat(x, x), Err(HeapError::StringTooLong));
}

// ------------------------------------------------------------------ atoms

#[test]
fn well_known_atoms() {
    let mut h = heap();
    assert_eq!(h.atom_str("length").unwrap(), atoms::LENGTH);
    assert_eq!(h.atom_str("").unwrap(), atoms::EMPTY);
    assert_eq!(h.atom_str("__proto__").unwrap(), atoms::PROTO);
    assert_eq!(h.atom_name(atoms::CONSTRUCTOR), "constructor");
    assert_eq!(h.atom_name(atoms::SYM_ITERATOR), "[Symbol.iterator]");
    let sym = h.atom_symbol(atoms::SYM_ITERATOR).unwrap();
    assert_eq!(h.symbol_atom(sym), atoms::SYM_ITERATOR);
    let d = h.symbol_description(sym).as_string().unwrap();
    assert!(h.str_eq_str(d, "Symbol.iterator"));
    // Interning a heap string finds the existing atom.
    let t = s(&mut h, "prototype");
    assert_eq!(h.intern(t).unwrap(), atoms::PROTOTYPE);
    let st = h.atom_string(atoms::VALUE_OF).unwrap();
    assert!(h.str_eq_str(st, "valueOf"));
    assert_eq!(
        h.atom_to_value(atoms::SYM_SPLIT).unwrap().kind(),
        Kind::Symbol
    );
}

#[test]
fn integer_atoms() {
    let mut h = heap();
    let before = h.atom_count();
    for (t, i) in [("0", 0u32), ("7", 7), ("2147483647", 2147483647)] {
        let a = h.atom_str(t).unwrap();
        assert_eq!(a.as_int(), Some(i), "{t}");
        let hs = s(&mut h, t);
        assert_eq!(h.intern(hs).unwrap(), a);
        assert_eq!(h.atom_array_index(a), Some(i));
        let back = h.atom_string(a).unwrap();
        assert!(h.str_eq_str(back, t));
    }
    // Integer atoms are never interned.
    assert_eq!(h.atom_count(), before);
    // Array indexes beyond the int atom range are string atoms that still count as indexes.
    let big = h.atom_str("4294967294").unwrap();
    assert!(big.is_string());
    assert_eq!(h.atom_array_index(big), Some(4294967294));
    assert_eq!(h.atom_from_index(4294967294).unwrap(), big);
    assert_eq!(h.atom_from_index(5).unwrap(), Atom::int(5).unwrap());
    let a2 = h.atom_str("2147483648").unwrap();
    assert!(a2.is_string());
    assert_eq!(h.atom_array_index(a2), Some(2147483648));
    // Not array indexes.
    for t in ["4294967295", "01", "-1", "1.0", "1e3", " 1", "99999999999"] {
        let a = h.atom_str(t).unwrap();
        assert!(a.is_string(), "{t}");
        assert_eq!(h.atom_array_index(a), None, "{t}");
    }
}

#[test]
fn many_atoms_grow_the_table() {
    let mut h = heap();
    let mut ids = Vec::new();
    for i in 0..5000 {
        let name = alloc::format!("key_{i}");
        ids.push(h.atom_str(&name).unwrap());
    }
    // Distinct, stable, and found again both ways.
    let mut sorted = ids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), 5000);
    for (i, &a) in ids.iter().enumerate() {
        let name = alloc::format!("key_{i}");
        assert_eq!(h.atom_str(&name).unwrap(), a);
        let hs = s(&mut h, &name);
        assert_eq!(h.intern(hs).unwrap(), a);
        assert_eq!(h.atom_name(a), name);
    }
    // Non-ASCII and two-byte names.
    let a = h.atom_str("größe").unwrap();
    let b = h.atom_str("名前").unwrap();
    assert_ne!(a, b);
    assert_eq!(h.atom_str("größe").unwrap(), a);
    let u = h
        .alloc_utf16(&"名前".encode_utf16().collect::<Vec<_>>())
        .unwrap();
    assert_eq!(h.intern(u).unwrap(), b);
    h.verify().unwrap();
}

#[test]
fn interning_a_rope() {
    let mut h = heap();
    let l = s(&mut h, "abcdefghijklmnopqrstuvwxyz");
    let r = s(&mut h, "0123456789");
    let rope = h.concat(l, r).unwrap();
    let a = h.intern(rope).unwrap();
    assert_eq!(
        h.atom_str("abcdefghijklmnopqrstuvwxyz0123456789").unwrap(),
        a
    );
    // The table holds a flat string.
    let st = h.atom_string(a).unwrap();
    assert_ne!(h.cell_kind(st.0), CellKind::Rope);
}

#[test]
fn symbols() {
    let mut h = heap();
    let d = s(&mut h, "tag");
    let a = h.new_symbol(Value::string(d)).unwrap();
    let b = h.new_symbol(Value::string(d)).unwrap();
    let c = h.new_symbol(Value::UNDEFINED).unwrap();
    assert_ne!(a, b);
    let (ka, kb) = (h.symbol_atom(a), h.symbol_atom(b));
    assert!(ka.is_symbol() && kb.is_symbol());
    assert_ne!(ka, kb);
    assert_eq!(h.atom_symbol(ka), Some(a));
    assert_eq!(h.symbol_description(c), Value::UNDEFINED);
    assert_eq!(h.atom_name(h.symbol_atom(c)), "[]");
    assert_eq!(h.atom_symbol(atoms::LENGTH), None);
    h.verify().unwrap();
}

// ------------------------------------------------------------------ objects

fn get(h: &Heap, o: Obj, k: Atom) -> Option<Value> {
    h.get_own_property(o, k).and_then(|p| p.value())
}

#[test]
fn basic_properties() {
    let mut h = heap();
    let o = h.new_object(None).unwrap();
    let x = h.atom_str("x").unwrap();
    let y = h.atom_str("y").unwrap();
    assert!(!h.has_own_property(o, x));
    assert!(h.create_data_property(o, x, Value::int(1)).unwrap());
    assert!(h.create_data_property(o, y, Value::TRUE).unwrap());
    assert_eq!(get(&h, o, x), Some(Value::int(1)));
    assert_eq!(
        h.get_own_property(o, y),
        Some(Property::data(Value::TRUE, PropFlags::ALL))
    );
    // Overwrite keeps the position.
    assert!(h.create_data_property(o, x, Value::int(2)).unwrap());
    assert_eq!(get(&h, o, x), Some(Value::int(2)));
    assert_eq!(h.own_keys(o), vec![x, y]);
    assert!(h.delete_own_property(o, x).unwrap());
    assert!(!h.has_own_property(o, x));
    assert!(
        h.delete_own_property(o, x).unwrap(),
        "deleting a missing key succeeds"
    );
    assert_eq!(h.own_keys(o), vec![y]);
    h.verify().unwrap();
}

#[test]
fn key_order() {
    let mut h = heap();
    let o = h.new_object(None).unwrap();
    let sym = h.new_symbol(Value::UNDEFINED).unwrap();
    let ks = h.symbol_atom(sym);
    let b = h.atom_str("b").unwrap();
    let a = h.atom_str("a").unwrap();
    let big = h.atom_str("4294967294").unwrap();
    let not_index = h.atom_str("4294967295").unwrap();
    let i10 = Atom::int(10).unwrap();
    let i2 = Atom::int(2).unwrap();
    for k in [ks, b, i10, big, not_index, a, i2] {
        assert!(h.create_data_property(o, k, Value::NULL).unwrap());
    }
    assert_eq!(h.own_keys(o), vec![i2, i10, big, b, not_index, a, ks]);
}

#[test]
fn hashed_tables() {
    let mut h = heap();
    let o = h.new_object(None).unwrap();
    let keys: Vec<Atom> = (0..100)
        .map(|i| h.atom_str(&alloc::format!("p{i}")).unwrap())
        .collect();
    for (i, &k) in keys.iter().enumerate() {
        assert!(h.create_data_property(o, k, Value::int(i as i32)).unwrap());
    }
    let t = h.arena().get(o.0, cells::Object::PROPS).0;
    assert_ne!(
        h.arena().get(t, cells::PropTable::INDEX).0,
        0,
        "hashed mode"
    );
    for (i, &k) in keys.iter().enumerate() {
        assert_eq!(get(&h, o, k), Some(Value::int(i as i32)));
    }
    // Delete every other key; order of the rest is preserved.
    for &k in keys.iter().step_by(2) {
        assert!(h.delete_own_property(o, k).unwrap());
    }
    let rest: Vec<Atom> = keys.iter().copied().skip(1).step_by(2).collect();
    assert_eq!(h.own_keys(o), rest);
    for &k in keys.iter().step_by(2) {
        assert!(!h.has_own_property(o, k));
    }
    // Re-adding goes to the end, through the tombstones and a compacting growth.
    for &k in keys.iter().step_by(2) {
        assert!(h.create_data_property(o, k, Value::NULL).unwrap());
    }
    let mut expect = rest.clone();
    expect.extend(keys.iter().copied().step_by(2));
    assert_eq!(h.own_keys(o), expect);
    for &k in &keys {
        assert!(h.has_own_property(o, k));
    }
    h.verify().unwrap();
}

#[test]
fn linear_delete_preserves_order() {
    let mut h = heap();
    let o = h.new_object(None).unwrap();
    let keys: Vec<Atom> = ["a", "b", "c", "d"]
        .iter()
        .map(|t| h.atom_str(t).unwrap())
        .collect();
    for &k in &keys {
        h.create_data_property(o, k, Value::NULL).unwrap();
    }
    assert!(h.delete_own_property(o, keys[1]).unwrap());
    assert_eq!(h.own_keys(o), vec![keys[0], keys[2], keys[3]]);
    h.create_data_property(o, keys[1], Value::NULL).unwrap();
    assert_eq!(h.own_keys(o), vec![keys[0], keys[2], keys[3], keys[1]]);
}

#[test]
fn prototype_chain() {
    let mut h = heap();
    let base = h.new_object(None).unwrap();
    let mid = h.new_object(Some(base)).unwrap();
    let o = h.new_object(Some(mid)).unwrap();
    let k = h.atom_str("k").unwrap();
    let g = h.atom_str("g").unwrap();
    let getter = h.new_object(None).unwrap();
    h.create_data_property(base, k, Value::int(1)).unwrap();
    let desc = PropertyDescriptor::accessor(
        Value::object(getter),
        Value::UNDEFINED,
        PropFlags::CONFIGURABLE,
    );
    assert!(h.define_own_property(mid, g, &desc).unwrap());
    assert_eq!(
        h.find_property(o, k),
        Some((base, Property::data(Value::int(1), PropFlags::ALL)))
    );
    assert_eq!(
        h.find_property(o, g),
        Some((
            mid,
            Property::Accessor {
                getter: Value::object(getter),
                setter: Value::UNDEFINED,
                flags: PropFlags::CONFIGURABLE
            }
        ))
    );
    assert!(h.has_property(o, k));
    assert!(!h.has_own_property(o, k));
    let missing = h.atom_str("missing").unwrap();
    assert_eq!(h.find_property(o, missing), None);

    // Cycles are rejected; non-extensible objects keep their prototype.
    assert!(!h.set_prototype_of(base, Some(o)));
    assert!(!h.set_prototype_of(base, Some(base)));
    assert!(h.set_prototype_of(o, Some(base)));
    assert_eq!(h.get_prototype_of(o), Some(base));
    assert!(h.prevent_extensions(o));
    assert!(!h.set_prototype_of(o, None));
    assert!(h.set_prototype_of(o, Some(base)), "same prototype is fine");
    h.verify().unwrap();
}

#[test]
fn define_validation() {
    let mut h = heap();
    let o = h.new_object(None).unwrap();
    let k = h.atom_str("k").unwrap();
    let f1 = Value::object(h.new_object(None).unwrap());
    let f2 = Value::object(h.new_object(None).unwrap());

    // A new property from a partial descriptor gets false/undefined defaults.
    let partial = PropertyDescriptor {
        value: Some(Value::int(1)),
        ..Default::default()
    };
    assert!(h.define_own_property(o, k, &partial).unwrap());
    assert_eq!(
        h.get_own_property(o, k),
        Some(Property::data(Value::int(1), PropFlags::NONE))
    );
    // Non-writable, non-configurable: same value is fine, a different one is not.
    assert!(h.define_own_property(o, k, &partial).unwrap());
    let same_num = PropertyDescriptor {
        value: Some(Value::number(1.0)),
        ..Default::default()
    };
    assert!(
        h.define_own_property(o, k, &same_num).unwrap(),
        "int vs f64 1"
    );
    let other = PropertyDescriptor {
        value: Some(Value::int(2)),
        ..Default::default()
    };
    assert!(!h.define_own_property(o, k, &other).unwrap());
    let make_writable = PropertyDescriptor {
        writable: Some(true),
        ..Default::default()
    };
    assert!(!h.define_own_property(o, k, &make_writable).unwrap());
    let make_conf = PropertyDescriptor {
        configurable: Some(true),
        ..Default::default()
    };
    assert!(!h.define_own_property(o, k, &make_conf).unwrap());
    let to_accessor = PropertyDescriptor {
        get: Some(f1),
        ..Default::default()
    };
    assert!(!h.define_own_property(o, k, &to_accessor).unwrap());
    assert!(!h.delete_own_property(o, k).unwrap());
    assert!(
        h.define_own_property(o, k, &PropertyDescriptor::default())
            .unwrap()
    );

    // Configurable: data -> accessor -> data, keeping enumerable/configurable.
    let c = h.atom_str("c").unwrap();
    let flags = PropFlags::ENUMERABLE | PropFlags::CONFIGURABLE;
    assert!(
        h.define_own_property(o, c, &PropertyDescriptor::data(Value::NULL, flags))
            .unwrap()
    );
    assert!(h.define_own_property(o, c, &to_accessor).unwrap());
    assert_eq!(
        h.get_own_property(o, c),
        Some(Property::Accessor {
            getter: f1,
            setter: Value::UNDEFINED,
            flags
        })
    );
    let set_only = PropertyDescriptor {
        set: Some(f2),
        ..Default::default()
    };
    assert!(h.define_own_property(o, c, &set_only).unwrap());
    assert_eq!(
        h.get_own_property(o, c),
        Some(Property::Accessor {
            getter: f1,
            setter: f2,
            flags
        })
    );
    let back = PropertyDescriptor {
        writable: Some(true),
        ..Default::default()
    };
    assert!(h.define_own_property(o, c, &back).unwrap());
    assert_eq!(
        h.get_own_property(o, c),
        Some(Property::data(
            Value::UNDEFINED,
            flags | PropFlags::WRITABLE
        ))
    );

    // Non-configurable accessor: same getter ok, different rejected.
    let a = h.atom_str("a").unwrap();
    let acc = PropertyDescriptor::accessor(f1, Value::UNDEFINED, PropFlags::NONE);
    assert!(h.define_own_property(o, a, &acc).unwrap());
    assert!(h.define_own_property(o, a, &to_accessor).unwrap());
    let other_get = PropertyDescriptor {
        get: Some(f2),
        ..Default::default()
    };
    assert!(!h.define_own_property(o, a, &other_get).unwrap());
    let enum_change = PropertyDescriptor {
        enumerable: Some(true),
        ..Default::default()
    };
    assert!(!h.define_own_property(o, a, &enum_change).unwrap());

    // Non-extensible: no new properties, existing ones still change.
    assert!(h.prevent_extensions(o));
    let n = h.atom_str("n").unwrap();
    assert!(!h.create_data_property(o, n, Value::NULL).unwrap());
    assert!(h.create_data_property(o, c, Value::TRUE).unwrap());
    h.verify().unwrap();
}

#[test]
fn same_value() {
    let mut h = heap();
    assert!(h.same_value(Value::NAN, Value::number(f64::NAN)));
    assert!(!h.same_value(Value::number(0.0), Value::number(-0.0)));
    assert!(h.same_value(Value::int(0), Value::number(0.0)));
    assert!(!h.same_value(Value::int(0), Value::number(-0.0)));
    assert!(h.same_value(Value::int(5), Value::number(5.0)));
    let a = s(&mut h, "str");
    let b = s(&mut h, "str");
    assert!(h.same_value(Value::string(a), Value::string(b)));
    let o1 = h.new_object(None).unwrap();
    let o2 = h.new_object(None).unwrap();
    assert!(!h.same_value(Value::object(o1), Value::object(o2)));
    assert!(h.same_value(Value::object(o1), Value::object(o1)));
    assert!(!h.same_value(Value::UNDEFINED, Value::NULL));
}

#[test]
fn object_slots() {
    let mut h = heap();
    let o = h.alloc_object(ObjectClass::Ordinary, None, 3).unwrap();
    assert_eq!(h.object_slot_count(o), 3);
    assert_eq!(h.object_slot(o, 2), Value::UNDEFINED);
    h.set_object_slot(o, 1, Value::int(9));
    assert_eq!(h.object_slot(o, 1), Value::int(9));
}

// ------------------------------------------------------------------ arrays

fn idx(i: u32) -> Atom {
    Atom::int(i).unwrap()
}

#[test]
fn array_fast_paths() {
    let mut h = heap();
    let proto = h.new_object(None).unwrap();
    let a = h.new_array(Some(proto), 0).unwrap();
    assert_eq!(h.array_length(a), 0);
    for i in 0..100 {
        assert!(h.set_element_fast(a, i, Value::int(i as i32)).unwrap());
    }
    assert_eq!(h.array_length(a), 100);
    assert_eq!(h.dense_len(a), 100);
    assert_eq!(h.get_element_fast(a, 42), Some(Value::int(42)));
    assert_eq!(h.get_element_fast(a, 100), None);
    // Overwrite.
    assert!(h.set_element_fast(a, 5, Value::TRUE).unwrap());
    assert_eq!(get(&h, a, idx(5)), Some(Value::TRUE));
    // A gap is not a fast path.
    assert!(!h.set_element_fast(a, 102, Value::NULL).unwrap());
    // Holes.
    assert!(h.delete_own_property(a, idx(7)).unwrap());
    assert_eq!(h.get_element_fast(a, 7), None);
    assert!(!h.has_own_property(a, idx(7)));
    assert!(
        h.set_element_fast(a, 7, Value::NULL).unwrap(),
        "filling a hole"
    );
    assert_eq!(h.array_length(a), 100);

    // A prototype with index properties disables adding elements on the fast path.
    assert!(h.delete_own_property(a, idx(8)).unwrap());
    h.create_data_property(proto, idx(8), Value::FALSE).unwrap();
    assert!(!h.set_element_fast(a, 8, Value::NULL).unwrap());
    assert!(!h.set_element_fast(a, 100, Value::NULL).unwrap());
    assert!(
        h.set_element_fast(a, 9, Value::NULL).unwrap(),
        "overwrite still fast"
    );
    assert_eq!(
        h.find_property(a, idx(8)),
        Some((proto, Property::data(Value::FALSE, PropFlags::ALL)))
    );

    // Not an array: no element fast path for adding.
    let o = h.new_object(None).unwrap();
    assert!(!h.set_element_fast(o, 0, Value::NULL).unwrap());
    h.verify().unwrap();
}

#[test]
fn array_length_property() {
    let mut h = heap();
    let a = h.new_array(None, 4).unwrap();
    h.create_data_property(a, idx(0), Value::int(1)).unwrap();
    h.create_data_property(a, idx(2), Value::int(3)).unwrap();
    assert_eq!(h.array_length(a), 3);
    assert_eq!(
        h.get_own_property(a, atoms::LENGTH),
        Some(Property::data(Value::number(3.0), PropFlags::WRITABLE))
    );
    assert_eq!(h.own_keys(a), vec![idx(0), idx(2), atoms::LENGTH]);
    assert!(!h.delete_own_property(a, atoms::LENGTH).unwrap());

    // Sparse writes go to the table; keys stay sorted.
    h.create_data_property(a, idx(1000), Value::NULL).unwrap();
    assert_eq!(h.array_length(a), 1001);
    assert_eq!(h.dense_len(a), 3);
    let big = h.atom_from_index(4_000_000_000).unwrap();
    h.create_data_property(a, big, Value::NULL).unwrap();
    assert_eq!(h.array_length(a), 4_000_000_001);
    h.create_data_property(a, idx(1), Value::NULL).unwrap();
    let x = h.atom_str("x").unwrap();
    h.create_data_property(a, x, Value::NULL).unwrap();
    assert_eq!(
        h.own_keys(a),
        vec![idx(0), idx(1), idx(2), idx(1000), big, atoms::LENGTH, x]
    );

    // Truncation removes dense and sparse elements.
    let len = |n: f64| PropertyDescriptor {
        value: Some(Value::number(n)),
        ..Default::default()
    };
    assert!(h.define_own_property(a, atoms::LENGTH, &len(2.0)).unwrap());
    assert_eq!(h.array_length(a), 2);
    assert_eq!(h.own_keys(a), vec![idx(0), idx(1), atoms::LENGTH, x]);
    assert!(!h.has_own_property(a, idx(2)));
    // Growing length adds no elements.
    assert!(h.define_own_property(a, atoms::LENGTH, &len(10.0)).unwrap());
    assert_eq!(h.array_length(a), 10);
    assert_eq!(h.own_keys(a), vec![idx(0), idx(1), atoms::LENGTH, x]);
    // Invalid lengths.
    for bad in [-1.0, 1.5, f64::NAN, 4294967296.0, f64::INFINITY] {
        assert_eq!(
            h.define_own_property(a, atoms::LENGTH, &len(bad)),
            Err(HeapError::InvalidArrayLength)
        );
    }
    assert!(
        !h.define_own_property(
            a,
            atoms::LENGTH,
            &PropertyDescriptor {
                enumerable: Some(true),
                ..Default::default()
            }
        )
        .unwrap()
    );
    h.verify().unwrap();
}

#[test]
fn array_non_configurable_element_stops_truncation() {
    let mut h = heap();
    let a = h.new_array(None, 0).unwrap();
    for i in 0..10 {
        assert!(h.set_element_fast(a, i, Value::int(i as i32)).unwrap());
    }
    // Make element 5 non-configurable: it moves out of dense storage.
    let frozen = PropertyDescriptor {
        configurable: Some(false),
        ..Default::default()
    };
    assert!(h.define_own_property(a, idx(5), &frozen).unwrap());
    assert_eq!(h.get_element_fast(a, 5), None);
    assert_eq!(
        h.get_own_property(a, idx(5)),
        Some(Property::data(
            Value::int(5),
            PropFlags::WRITABLE | PropFlags::ENUMERABLE
        ))
    );
    assert_eq!(h.own_keys(a).len(), 11);
    // Writing it via define keeps it in the table.
    assert!(
        h.define_own_property(
            a,
            idx(5),
            &PropertyDescriptor {
                value: Some(Value::int(50)),
                ..Default::default()
            }
        )
        .unwrap()
    );
    assert_eq!(get(&h, a, idx(5)), Some(Value::int(50)));
    // The object has indexed props now: appends take the slow path.
    assert!(!h.set_element_fast(a, 10, Value::NULL).unwrap());

    let shrink = PropertyDescriptor {
        value: Some(Value::int(2)),
        writable: Some(false),
        ..Default::default()
    };
    assert!(!h.define_own_property(a, atoms::LENGTH, &shrink).unwrap());
    assert_eq!(h.array_length(a), 6);
    let keys = h.own_keys(a);
    assert_eq!(
        keys,
        vec![
            idx(0),
            idx(1),
            idx(2),
            idx(3),
            idx(4),
            idx(5),
            atoms::LENGTH
        ]
    );
    // Length became read-only even though the truncation failed.
    assert_eq!(
        h.get_own_property(a, atoms::LENGTH),
        Some(Property::data(Value::number(6.0), PropFlags::NONE))
    );
    assert!(!h.create_data_property(a, idx(6), Value::NULL).unwrap());
    assert!(!h.set_element_fast(a, 6, Value::NULL).unwrap());
    assert!(
        h.create_data_property(a, idx(1), Value::NULL).unwrap(),
        "below length ok"
    );
    h.verify().unwrap();
}

#[test]
fn readonly_length_without_value() {
    let mut h = heap();
    let a = h.new_array(None, 0).unwrap();
    h.set_element_fast(a, 0, Value::NULL).unwrap();
    let ro = PropertyDescriptor {
        writable: Some(false),
        ..Default::default()
    };
    assert!(h.define_own_property(a, atoms::LENGTH, &ro).unwrap());
    assert!(!h.set_element_fast(a, 1, Value::NULL).unwrap());
    assert!(h.set_element_fast(a, 0, Value::TRUE).unwrap());
    let rw = PropertyDescriptor {
        writable: Some(true),
        ..Default::default()
    };
    assert!(!h.define_own_property(a, atoms::LENGTH, &rw).unwrap());
    let same = PropertyDescriptor {
        value: Some(Value::int(1)),
        ..Default::default()
    };
    assert!(h.define_own_property(a, atoms::LENGTH, &same).unwrap());
    let diff = PropertyDescriptor {
        value: Some(Value::int(0)),
        ..Default::default()
    };
    assert!(!h.define_own_property(a, atoms::LENGTH, &diff).unwrap());
}

#[test]
fn index_keys_on_ordinary_objects() {
    let mut h = heap();
    let o = h.new_object(None).unwrap();
    h.create_data_property(o, idx(3), Value::int(3)).unwrap();
    h.create_data_property(o, idx(1), Value::int(1)).unwrap();
    assert_eq!(h.dense_len(o), 0);
    assert_eq!(get(&h, o, idx(1)), Some(Value::int(1)));
    assert_eq!(h.own_keys(o), vec![idx(1), idx(3)]);
    assert!(h.delete_own_property(o, idx(1)).unwrap());
    assert_eq!(h.own_keys(o), vec![idx(3)]);
}

// ------------------------------------------------------------------ walking and images

/// Builds a heap with a bit of everything, deterministically.
fn build_sample(h: &mut Heap) -> Obj {
    let proto = h.new_object(None).unwrap();
    let o = h.new_object(Some(proto)).unwrap();
    for i in 0..20 {
        let k = h.atom_str(&alloc::format!("field{i}")).unwrap();
        let v = h.alloc_str(&alloc::format!("value number {i} €")).unwrap();
        h.create_data_property(o, k, Value::string(v)).unwrap();
    }
    let arr = h.new_array(Some(proto), 0).unwrap();
    for i in 0..50 {
        h.set_element_fast(arr, i, Value::number(i as f64 * 0.5))
            .unwrap();
    }
    let ka = h.atom_str("arr").unwrap();
    h.create_data_property(o, ka, Value::object(arr)).unwrap();
    let (rope, _) = appended(h, 200);
    let kr = h.atom_str("rope").unwrap();
    h.create_data_property(o, kr, Value::string(rope)).unwrap();
    let d = h.alloc_str("mine").unwrap();
    let sym = h.new_symbol(Value::string(d)).unwrap();
    let ks = h.symbol_atom(sym);
    h.create_data_property(o, ks, Value::symbol(sym)).unwrap();
    let getter = h.new_object(None).unwrap();
    let kg = h.atom_str("g").unwrap();
    h.define_own_property(
        o,
        kg,
        &PropertyDescriptor::accessor(Value::object(getter), Value::UNDEFINED, PropFlags::ALL),
    )
    .unwrap();
    h.set_realm(Value::object(o));
    o
}

/// Reads back what `build_sample` stored, through the root.
fn check_sample(h: &mut Heap) {
    let o = h.realm().as_object().unwrap();
    for i in 0..20 {
        let k = h.atom_str(&alloc::format!("field{i}")).unwrap();
        let v = get(h, o, k).unwrap().as_string().unwrap();
        assert_eq!(h.str_to_string(v), alloc::format!("value number {i} €"));
    }
    let ka = h.atom_str("arr").unwrap();
    let arr = get(h, o, ka).unwrap().as_object().unwrap();
    assert_eq!(h.array_length(arr), 50);
    assert_eq!(h.get_element_fast(arr, 49), Some(Value::number(24.5)));
    let kr = h.atom_str("rope").unwrap();
    let rope = get(h, o, kr).unwrap().as_string().unwrap();
    assert_eq!(h.str_len(rope), 200);
    let keys = h.own_keys(o);
    assert_eq!(keys.len(), 24);
    let sym = h.atom_symbol(keys[23]).unwrap();
    let d = h.symbol_description(sym).as_string().unwrap();
    assert!(h.str_eq_str(d, "mine"));
    assert_eq!(h.atom_str("length").unwrap(), atoms::LENGTH);
    assert!(h.get_own_property(o, keys[22]).unwrap().is_accessor());
}

#[test]
fn walker_visits_every_cell() {
    let mut h = heap();
    let start = h.arena().len();
    let mut allocated = Vec::new();
    for (i, kind) in [
        CellKind::Object,
        CellKind::Str8,
        CellKind::ValueArray,
        CellKind::PropTable,
        CellKind::Rope,
        CellKind::Symbol,
        CellKind::U32Array,
        CellKind::Str16,
    ]
    .into_iter()
    .cycle()
    .take(64)
    .enumerate()
    {
        let off = h.alloc_cell(kind, (i as u32 % 5) * 24).unwrap();
        allocated.push((off, kind));
    }
    let walked: Vec<(u32, CellKind)> = h
        .cells()
        .filter(|c| c.offset >= start)
        .map(|c| (c.offset, c.kind))
        .collect();
    assert_eq!(walked, allocated);
    // Zeroed cells are safe to trace: no references. (They are not all semantically valid,
    // e.g. a rope without children, so `verify` would reject some of them; no safepoint
    // can see a cell between allocation and initialization.)
    for &(off, _) in &allocated {
        assert!(h.cell_refs(off).is_empty());
    }
    let mut h = heap();

    // The whole arena is covered, starting with the roots.
    build_sample(&mut h);
    let cells: Vec<_> = h.cells().collect();
    assert_eq!(cells[0].kind, CellKind::Roots);
    assert_eq!(cells[0].offset, 0);
    let total: u32 = cells.iter().map(|c| c.size).sum();
    assert_eq!(total, h.arena().len());
    // Every reference points at a walked cell.
    for c in &cells {
        for t in h.cell_refs(c.offset) {
            assert!(cells.iter().any(|d| d.offset == t), "dangling {t}");
        }
    }
    h.verify().unwrap();
}

#[test]
fn tracing_follows_layouts() {
    let mut h = heap();
    let o = build_sample(&mut h);
    // The roots reference the atom table and the realm object.
    let refs = h.cell_refs(0);
    assert!(refs.contains(&h.arena().get(0, Roots::ATOMS).0));
    assert!(refs.contains(&o.0));
    // An object references its prototype and property table.
    let refs = h.cell_refs(o.0);
    assert_eq!(refs.len(), 2);
    // Every slot of a values block is visited, used or not, plus its shape.
    let v = h.arena().get(o.0, cells::Object::PROPS).0;
    assert_eq!(h.cell_kind(v), CellKind::PropValues);
    let cap = cells::PropValues::LAYOUT.tail_capacity(h.cell_size(v));
    let (mut values, mut refs) = (0, 0);
    h.trace_cell(v, |slot| match slot {
        Slot::Value(_) => values += 1,
        Slot::Ref(_) => refs += 1,
    });
    assert_eq!((values, refs), (cap, 1), "VALUES, SHAPE");
    // Every slot of a PropTable entry tail is visited, used or not.
    h.make_dictionary(o).unwrap();
    let t = h.arena().get(o.0, cells::Object::PROPS).0;
    let cap = cells::PropTable::LAYOUT.tail_capacity(h.cell_size(t));
    let mut values = 0;
    let mut refs = 0;
    h.trace_cell(t, |slot| match slot {
        Slot::Value(_) => values += 1,
        Slot::Ref(_) => refs += 1,
    });
    assert_eq!(values, cap);
    assert_eq!(refs, 1, "INDEX");
    // Strings have no traced slots; ropes have two.
    let a = s(&mut h, "plain");
    assert!(h.cell_refs(a.0).is_empty());
}

#[test]
fn image_round_trip() {
    let mut h = heap();
    build_sample(&mut h);
    h.verify().unwrap();
    let bytes = h.bytes().to_vec();
    let mut copy = Heap::from_bytes(bytes.clone(), DEFAULT_LIMIT).unwrap();
    check_sample(&mut copy);
    check_sample(&mut h);
    // Keep using the restored heap.
    let o = copy.realm().as_object().unwrap();
    let k = copy.atom_str("after").unwrap();
    copy.create_data_property(o, k, Value::TRUE).unwrap();
    assert_eq!(get(&copy, o, k), Some(Value::TRUE));
    copy.verify().unwrap();
}

#[test]
fn images_are_deterministic() {
    // Two heaps built the same way in separate allocations have identical bytes: the arena
    // holds no host pointers or other process state.
    let mut a = heap();
    let mut b = heap();
    let _pad: Vec<u8> = vec![0; 12345]; // shift host addresses
    build_sample(&mut a);
    build_sample(&mut b);
    assert_eq!(a.bytes(), b.bytes());
    assert_eq!(heap().bytes(), heap().bytes());
}

#[test]
fn image_validation() {
    let mut h = heap();
    build_sample(&mut h);
    let good = h.bytes().to_vec();

    let mut bad = good.clone();
    bad[8] ^= 1; // magic
    assert!(matches!(
        Heap::from_bytes(bad, DEFAULT_LIMIT),
        Err(LoadError::BadHeader)
    ));
    let mut bad = good.clone();
    bad[12] ^= 1; // layout version
    assert!(matches!(
        Heap::from_bytes(bad, DEFAULT_LIMIT),
        Err(LoadError::BadHeader)
    ));
    assert!(matches!(
        Heap::from_bytes(good[..good.len() - 8].to_vec(), DEFAULT_LIMIT),
        Err(LoadError::Corrupt(_))
    ));
    assert!(matches!(
        Heap::from_bytes(good.clone(), 64),
        Err(LoadError::TooLarge)
    ));
    assert!(matches!(
        Heap::from_bytes(Vec::new(), DEFAULT_LIMIT),
        Err(LoadError::BadHeader)
    ));

    // A realm value pointing into the middle of a cell.
    let mut bad = good.clone();
    let realm = Roots::REALM.offset as usize;
    let v = u64::from_le_bytes(bad[realm..realm + 8].try_into().unwrap());
    bad[realm..realm + 8].copy_from_slice(&(v + 8).to_le_bytes());
    assert!(matches!(
        Heap::from_bytes(bad, DEFAULT_LIMIT),
        Err(LoadError::Corrupt(VerifyError::BadRef { .. }))
    ));
    // An object reference retagged as a string.
    let mut bad = good.clone();
    let retagged = Value::string(Str(v as u32)).to_bits();
    bad[realm..realm + 8].copy_from_slice(&retagged.to_le_bytes());
    assert!(matches!(
        Heap::from_bytes(bad, DEFAULT_LIMIT),
        Err(LoadError::Corrupt(VerifyError::BadRef { .. }))
    ));
    // A non-canonical NaN.
    let mut bad = good.clone();
    bad[realm..realm + 8].copy_from_slice(&0x7FF0_0000_0000_0001u64.to_le_bytes());
    assert!(matches!(
        Heap::from_bytes(bad, DEFAULT_LIMIT),
        Err(LoadError::Corrupt(VerifyError::BadValue { .. }))
    ));

    // A rope that contains itself.
    let mut h2 = heap();
    let x = s(&mut h2, &"x".repeat(40));
    let r = h2.concat(x, x).unwrap();
    h2.set_realm(Value::string(r));
    h2.verify().unwrap();
    let mut bad = h2.bytes().to_vec();
    let left = (r.0 + cells::Rope::LEFT.offset) as usize;
    bad[left..left + 4].copy_from_slice(&r.0.to_le_bytes());
    assert_eq!(
        Heap::from_bytes(bad, DEFAULT_LIMIT).err(),
        Some(LoadError::Corrupt(VerifyError::Invalid { offset: r.0 }))
    );

    // A prototype cycle.
    let mut h3 = heap();
    let p = h3.new_object(None).unwrap();
    let o = h3.new_object(Some(p)).unwrap();
    h3.set_realm(Value::object(o));
    let mut bad = h3.bytes().to_vec();
    let proto = (p.0 + cells::Object::PROTO.offset) as usize;
    bad[proto..proto + 4].copy_from_slice(&o.0.to_le_bytes());
    assert!(matches!(
        Heap::from_bytes(bad, DEFAULT_LIMIT),
        Err(LoadError::Corrupt(VerifyError::ProtoCycle { .. }))
    ));

    // Random corruption never panics the verifier.
    let mut x: u32 = 12345;
    for _ in 0..2000 {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        let mut bad = good.clone();
        let pos = x as usize % bad.len();
        bad[pos] = (x >> 8) as u8;
        let _ = Heap::from_bytes(bad, DEFAULT_LIMIT);
    }
}

// ------------------------------------------------------------------ out of memory

#[test]
fn out_of_memory_leaves_heap_valid() {
    let base = heap().arena().len() as usize;
    for extra in [0, 100, 1000, 5000, 20_000] {
        // The cap keeps 1/32 back as the out-of-memory reserve (D1600).
        let mut h = Heap::with_limit((base + extra) * 32 / 31 + 64).unwrap();
        let mut steps = 0;
        let r: Result<(), HeapError> = (|| {
            let o = h.new_object(None)?;
            let a = h.new_array(None, 0)?;
            let mut acc = h.alloc_str("")?;
            for i in 0u32.. {
                steps += 1;
                let k = h.atom_str(&alloc::format!("k{i}"))?;
                let v = h.alloc_str("some value")?;
                h.create_data_property(o, k, Value::string(v))?;
                h.set_element_fast(a, i, Value::int(i as i32))?;
                acc = h.concat(acc, v)?;
                if i % 7 == 0 {
                    h.flatten(acc)?;
                }
            }
            Ok(())
        })();
        assert_eq!(r, Err(HeapError::OutOfMemory));
        assert!(h.arena().len() <= h.arena().limit());
        // The reserve gives room for more, up to the cap.
        let soft = h.arena().limit();
        h.open_memory_reserve();
        assert!(h.arena().limit() > soft);
        assert!(h.alloc_str("after the reserve opened").is_ok());
        h.verify().unwrap();
        let _ = steps;
    }
    assert_eq!(Heap::with_limit(64).err(), Some(HeapError::OutOfMemory));
}

/// D1750: Float32 and Float16 elements and `Math.fround` write the canonical quiet NaN
/// whatever the input NaN's bits, and finite values round as before.
#[test]
fn float_nan_encoding_is_canonical() {
    use super::typed::{Kind, decode, encode, f32_bits_to_f64, f64_to_f32_bits};
    for bits in [
        0x7FF8_0000_0000_0000u64,
        0xFFF8_0000_0000_0000,
        0x7FF0_0000_0000_0001,
        0x7FFF_FFFF_FFFF_FFFF,
        0xFFF4_0000_DEAD_BEEF,
    ] {
        let n = f64::from_bits(bits);
        assert_eq!(f64_to_f32_bits(n), 0x7FC0_0000, "{bits:#x}");
        let mut out = [0u8; 8];
        encode(Kind::Float32, n, &mut out);
        assert_eq!(out[..4], [0x00, 0x00, 0xC0, 0x7F], "{bits:#x}");
        encode(Kind::Float16, n, &mut out);
        assert_eq!(out[..2], [0x00, 0x7E], "{bits:#x}");
    }
    for bits in [0x7FC0_0000u32, 0xFFC0_0001, 0x7F80_0001, 0xFFFF_FFFF] {
        assert_eq!(
            f32_bits_to_f64(bits).to_bits(),
            f64::NAN.to_bits(),
            "{bits:#x}"
        );
        assert_eq!(
            decode(Kind::Float32, &bits.to_le_bytes()).to_bits(),
            f64::NAN.to_bits()
        );
    }
    assert_eq!(f64_to_f32_bits(1.0 / 3.0), (1.0f32 / 3.0).to_bits());
    assert_eq!(f64_to_f32_bits(-0.0), 0x8000_0000);
    assert_eq!(f64_to_f32_bits(f64::INFINITY), 0x7F80_0000);
    assert_eq!(f64_to_f32_bits(1e300), 0x7F80_0000);
    assert_eq!(f32_bits_to_f64(0x3EAA_AAAB), (1.0f32 / 3.0) as f64);
}

/// D1710: a suspension record is allocated past the cap without making a collection
/// pending, and freeing it from the top restores the arena byte for byte.
#[test]
fn unbudgeted_records_and_release_top() {
    let base = heap().arena().len() as usize;
    let mut h = Heap::with_limit((base + 4096) * 32 / 31 + 64).unwrap();
    // Fill up to the cap.
    while h.alloc_value_array(16).is_ok() {}
    assert!(h.gc_pending());
    h.collect().unwrap();
    while h.alloc_value_array(16).is_ok() {}
    let before = h.bytes().to_vec();
    let pending = h.gc_pending();
    assert!(h.alloc_u32_array(64).is_err());
    let saved = h.begin_unbudgeted();
    let a = h.alloc_u32_array(64).unwrap();
    h.u32_array_set(a, 3, 0xDEAD_BEEF);
    let b = h.alloc_value_array(8).unwrap();
    h.end_unbudgeted(saved);
    assert_eq!(h.gc_pending(), pending);
    // Still capped for everything else.
    assert!(h.alloc_u32_array(64).is_err());
    // Not the top: nothing happens.
    h.release_top(a, b);
    assert!(h.bytes().len() > before.len());
    h.release_top(a, b + h.cell_size(b));
    assert_eq!(h.bytes(), &before[..]);
    // The freed bytes are zeros again for the next allocation.
    let c = h.begin_unbudgeted();
    let a2 = h.alloc_u32_array(64).unwrap();
    h.end_unbudgeted(c);
    assert_eq!((a2, h.u32_array_get(a2, 3)), (a, 0));
}

/// D1780: the block ASCII check (wasm32's) agrees with `<[u8]>::is_ascii` for every
/// length up to a few blocks and a non-ASCII byte at every position (and alignment).
#[test]
fn block_ascii_check() {
    use super::string::is_ascii_blocks as is_ascii;
    let base: Vec<u8> = (0..200u32).map(|i| (i % 128) as u8).collect();
    for start in 0..8 {
        for len in 0..base.len() - start {
            let s = &base[start..start + len];
            assert!(is_ascii(s));
            for at in 0..len {
                for high in [0x80u8, 0xC3, 0xFF] {
                    let mut t = s.to_vec();
                    t[at] = high;
                    assert_eq!(is_ascii(&t), t.is_ascii(), "len {len} at {at}");
                }
            }
        }
    }
}
