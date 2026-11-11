//! Shapes and dictionary mode (0083): sharing, transitions, the fallbacks, the limits,
//! and collections.

use alloc::format;
use alloc::vec::Vec;

use super::cells::{Object, PropValues, Roots, Shape, flags::OBJ_DICTIONARY};
use super::shape::{SHAPE_BUDGET, SHAPE_MAX};
use super::*;
use crate::value::{Obj, Value};

fn heap() -> Heap {
    Heap::new().unwrap()
}

fn key(h: &mut Heap, k: &str) -> Atom {
    h.atom_str(k).unwrap()
}

/// An ordinary object with the data properties `keys` (values 0, 1, ...), added one by
/// one as assignments would.
fn obj(h: &mut Heap, keys: &[&str]) -> Obj {
    let o = h.new_object(None).unwrap();
    for (i, k) in keys.iter().enumerate() {
        let k = key(h, k);
        assert!(h.create_data_property(o, k, Value::int(i as i32)).unwrap());
    }
    o
}

fn is_dict(h: &Heap, o: Obj) -> bool {
    h.cell_flags(o.0) & OBJ_DICTIONARY != 0
}

/// The shape of a shape-mode object with properties.
fn shape_of(h: &Heap, o: Obj) -> u32 {
    assert!(!is_dict(h, o));
    let v = h.arena().get(o.0, Object::PROPS).0;
    h.arena().get(v, PropValues::SHAPE).0
}

fn descs(h: &Heap, s: u32) -> u32 {
    h.arena().get(s, Shape::DESCS).0
}

fn get(h: &Heap, o: Obj, k: Atom) -> Option<Value> {
    h.get_own_property(o, k).and_then(|p| p.value())
}

fn names(h: &Heap, o: Obj) -> Vec<alloc::string::String> {
    h.own_keys(o).into_iter().map(|k| h.atom_name(k)).collect()
}

#[test]
fn objects_with_the_same_keys_share_a_shape() {
    let mut h = heap();
    let a = obj(&mut h, &["x", "y", "z"]);
    let b = obj(&mut h, &["x", "y", "z"]);
    let c = obj(&mut h, &["y", "x", "z"]);
    assert_eq!(shape_of(&h, a), shape_of(&h, b));
    assert_ne!(shape_of(&h, a), shape_of(&h, c));
    // A chain shares its descriptors; a second child of a parent copies the prefix.
    let ab = obj(&mut h, &["x", "y"]);
    let aw = obj(&mut h, &["x", "w"]);
    assert_eq!(descs(&h, shape_of(&h, ab)), descs(&h, shape_of(&h, a)));
    assert_ne!(descs(&h, shape_of(&h, aw)), descs(&h, shape_of(&h, a)));
    let y = key(&mut h, "y");
    let w = key(&mut h, "w");
    assert_eq!(get(&h, b, y), Some(Value::int(1)));
    assert_eq!(get(&h, aw, w), Some(Value::int(1)));
    assert_eq!(get(&h, aw, y), None);
    assert_eq!(names(&h, c), ["y", "x", "z"]);
    // Different flags are a different transition.
    let o = h.new_object(None).unwrap();
    let x = key(&mut h, "x");
    let d = PropertyDescriptor::data(Value::int(0), PropFlags::WRITABLE);
    assert!(h.define_own_property(o, x, &d).unwrap());
    let plain = obj(&mut h, &["x"]);
    assert_ne!(shape_of(&h, o), shape_of(&h, plain));
    h.verify().unwrap();
}

#[test]
fn bulk_creation_walks_the_same_tree() {
    let mut h = heap();
    let one = obj(&mut h, &["id", "name", "ok"]);
    let keys = [key(&mut h, "id"), key(&mut h, "name"), key(&mut h, "ok")];
    let vals = [Value::int(7), Value::int(8), Value::int(9)];
    let bulk = h.new_object_from_entries(None, &keys, &vals).unwrap();
    assert_eq!(shape_of(&h, bulk), shape_of(&h, one));
    // The values block is allocated at its exact size.
    let v = h.arena().get(bulk.0, Object::PROPS).0;
    assert_eq!(h.cell_size(v), PropValues::VALUES.cell_size(3).unwrap());
    // A repeated key keeps its first position and its last value.
    let rk = [keys[0], keys[1], keys[0]];
    let bulk = h.new_object_from_entries(None, &rk, &vals).unwrap();
    assert_eq!(names(&h, bulk), ["id", "name"]);
    assert_eq!(get(&h, bulk, keys[0]), Some(Value::int(9)));
    // Index keys and more than SHAPE_MAX keys give dictionaries.
    let i = Atom::int(3).unwrap();
    let ik = [keys[0], i];
    let d = h.new_object_from_entries(None, &ik, &vals[..2]).unwrap();
    assert!(is_dict(&h, d));
    assert_eq!(get(&h, d, i), Some(Value::int(8)));
    let many: Vec<Atom> = (0..=SHAPE_MAX)
        .map(|i| key(&mut h, &format!("k{i}")))
        .collect();
    let mv: Vec<Value> = (0..=SHAPE_MAX).map(|i| Value::int(i as i32)).collect();
    let d = h.new_object_from_entries(None, &many, &mv).unwrap();
    assert!(is_dict(&h, d));
    assert_eq!(get(&h, d, many[32]), Some(Value::int(32)));
    h.verify().unwrap();
}

#[test]
fn deleting_the_last_key_goes_back_to_the_parent() {
    let mut h = heap();
    let ab = obj(&mut h, &["a", "b"]);
    let abc = obj(&mut h, &["a", "b", "c"]);
    let c = key(&mut h, "c");
    assert!(h.delete_own_property(abc, c).unwrap());
    assert!(!is_dict(&h, abc));
    assert_eq!(shape_of(&h, abc), shape_of(&h, ab));
    assert_eq!(names(&h, abc), ["a", "b"]);
    // Adding it again takes the existing transition.
    assert!(h.create_data_property(abc, c, Value::int(5)).unwrap());
    let fresh = obj(&mut h, &["a", "b", "c"]);
    assert_eq!(shape_of(&h, abc), shape_of(&h, fresh));
    // Deleting another key re-walks the tree from the parent of the key, if at most one
    // shape is new: `{b}` exists, `{b, c}` is new.
    obj(&mut h, &["b"]);
    let a = key(&mut h, "a");
    assert!(h.delete_own_property(abc, a).unwrap());
    assert!(!is_dict(&h, abc));
    assert_eq!(names(&h, abc), ["b", "c"]);
    assert_eq!(get(&h, abc, c), Some(Value::int(5)));
    let bc = obj(&mut h, &["b", "c"]);
    assert_eq!(shape_of(&h, abc), shape_of(&h, bc));
    // Two new shapes are too many: `{q, r, s}` from `{p, q, r, s}`.
    let o = obj(&mut h, &["p", "q", "r", "s"]);
    let p = key(&mut h, "p");
    assert!(h.delete_own_property(o, p).unwrap());
    assert!(is_dict(&h, o));
    assert_eq!(names(&h, o), ["q", "r", "s"]);
    h.verify().unwrap();
}

#[test]
fn attribute_changes_make_dictionaries() {
    let mut h = heap();
    let o = obj(&mut h, &["a", "b"]);
    let a = key(&mut h, "a");
    // The same attributes: the value changes in place.
    let d = PropertyDescriptor::data(Value::int(9), PropFlags::ALL);
    assert!(h.define_own_property(o, a, &d).unwrap());
    assert!(!is_dict(&h, o));
    assert_eq!(get(&h, o, a), Some(Value::int(9)));
    let d = PropertyDescriptor {
        writable: Some(false),
        ..Default::default()
    };
    assert!(h.define_own_property(o, a, &d).unwrap());
    assert!(is_dict(&h, o));
    assert_eq!(
        h.get_own_property(o, a),
        Some(Property::data(
            Value::int(9),
            PropFlags::ENUMERABLE | PropFlags::CONFIGURABLE
        ))
    );
    // Accessors: redefining one with the same attributes keeps the shape and its pair.
    let o = h.new_object(None).unwrap();
    let g = Value::object(h.new_object(None).unwrap());
    let acc = PropertyDescriptor::accessor(g, Value::UNDEFINED, PropFlags::CONFIGURABLE);
    assert!(h.define_own_property(o, a, &acc).unwrap());
    let pairs = h
        .cells()
        .filter(|c| c.kind == CellKind::AccessorPair)
        .count();
    let set = PropertyDescriptor {
        set: Some(g),
        ..Default::default()
    };
    assert!(h.define_own_property(o, a, &set).unwrap());
    assert!(!is_dict(&h, o));
    assert_eq!(
        h.cells()
            .filter(|c| c.kind == CellKind::AccessorPair)
            .count(),
        pairs
    );
    assert_eq!(
        h.get_own_property(o, a),
        Some(Property::Accessor {
            getter: g,
            setter: g,
            flags: PropFlags::CONFIGURABLE
        })
    );
    // Accessor to data is an attribute change.
    let d = PropertyDescriptor::data(Value::int(1), PropFlags::CONFIGURABLE);
    assert!(h.define_own_property(o, a, &d).unwrap());
    assert!(is_dict(&h, o));
    h.verify().unwrap();
}

#[test]
fn index_keys_make_dictionaries() {
    let mut h = heap();
    let o = obj(&mut h, &["a"]);
    let i = Atom::int(0).unwrap();
    assert!(h.create_data_property(o, i, Value::int(3)).unwrap());
    assert!(is_dict(&h, o));
    assert_eq!(h.cell_flags(o.0) & cells::flags::OBJ_INDEXED_PROPS, 2);
    assert_eq!(names(&h, o), ["0", "a"]);
}

#[test]
fn shape_limits() {
    let mut h = heap();
    // SHAPE_MAX properties, then a dictionary.
    let keys: Vec<alloc::string::String> = (0..=SHAPE_MAX).map(|i| format!("p{i}")).collect();
    let refs: Vec<&str> = keys.iter().map(|k| k.as_str()).collect();
    let o = obj(&mut h, &refs[..SHAPE_MAX as usize]);
    assert!(!is_dict(&h, o));
    let last = key(&mut h, refs[SHAPE_MAX as usize]);
    assert!(h.create_data_property(o, last, Value::int(1)).unwrap());
    assert!(is_dict(&h, o));
    assert_eq!(h.own_keys(o).len(), SHAPE_MAX as usize + 1);
    // Any number of children per shape: 100 first keys, each found again.
    let firsts: Vec<Obj> = (0..100)
        .map(|i| obj(&mut h, &[&format!("c{}", (i * 37) % 100)]))
        .collect();
    assert!(firsts.iter().all(|&o| !is_dict(&h, o)));
    for (i, &first) in firsts.iter().enumerate() {
        let again = obj(&mut h, &[&format!("c{}", (i * 37) % 100)]);
        assert_eq!(shape_of(&h, again), shape_of(&h, first));
    }
    h.verify().unwrap();
}

#[test]
fn the_shape_budget_bounds_the_tree() {
    let mut h = heap();
    let early = obj(&mut h, &["early", "bird"]);
    // Chains of unique keys under 32 different first keys, until the budget is spent.
    let mut i = 0;
    while h.arena().get(0, Roots::SHAPES) < SHAPE_BUDGET {
        let first = format!("f{}", i % 32);
        let rest: Vec<alloc::string::String> = (0..20).map(|j| format!("u{i}_{j}")).collect();
        let mut keys = alloc::vec![first.as_str()];
        keys.extend(rest.iter().map(|k| k.as_str()));
        obj(&mut h, &keys);
        i += 1;
    }
    assert_eq!(h.arena().get(0, Roots::SHAPES), SHAPE_BUDGET);
    let before = h.cells().filter(|c| c.kind == CellKind::Shape).count();
    assert_eq!(before, SHAPE_BUDGET as usize);
    // New key sets are dictionaries now; existing shapes are still shared.
    let late = obj(&mut h, &["late", "comer"]);
    assert!(is_dict(&h, late));
    let again = obj(&mut h, &["early", "bird"]);
    assert_eq!(shape_of(&h, again), shape_of(&h, early));
    let partly = obj(&mut h, &["early", "worm"]);
    assert!(is_dict(&h, partly));
    assert_eq!(names(&h, partly), ["early", "worm"]);
    assert_eq!(
        h.cells().filter(|c| c.kind == CellKind::Shape).count(),
        before
    );
    h.verify().unwrap();
}

/// Shapes survive collections unchanged: transitions are strong (D1831), so the tree
/// is the same whether or when a collection ran, and a collected heap collects to the
/// same bytes.
#[test]
fn shapes_and_collections() {
    let build = |gc_every: Option<usize>| {
        let mut h = heap();
        let keep = h.new_array(None, 0).unwrap();
        let hk = h.new_handle(Value::object(keep)).unwrap();
        for i in 0..200usize {
            let k1 = format!("a{}", i % 7);
            let k2 = format!("b{}", i % 5);
            let o = obj(&mut h, &[&k1, &k2, "z"]);
            // Every third object stays alive; the others are garbage.
            if i % 3 == 0 {
                let keep = h.get_handle(hk).as_object().unwrap();
                let n = h.array_length(keep);
                assert!(h.set_element_fast(keep, n, Value::object(o)).unwrap());
            }
            if i % 4 == 0 {
                let z = key(&mut h, "z");
                h.delete_own_property(o, z).unwrap();
            }
            if gc_every.is_some_and(|n| i % n == 0) {
                h.collect().unwrap();
            }
        }
        h.collect().unwrap();
        h.verify().unwrap();
        let image = h.bytes().to_vec();
        h.collect().unwrap();
        assert!(image == h.bytes(), "a second collection changed the image");
        image
    };
    let never = build(None);
    assert!(
        never == build(Some(1)),
        "collection points changed the image"
    );
    assert!(
        never == build(Some(17)),
        "collection points changed the image"
    );
}

#[test]
fn dictionaries_and_shapes_round_trip_images() {
    let mut h = heap();
    let a = obj(&mut h, &["x", "y"]);
    let d = obj(&mut h, &["x", "y"]);
    h.make_dictionary(d).unwrap();
    let arr = h
        .new_array_from_values(None, &[Value::object(a), Value::object(d)])
        .unwrap();
    h.set_realm(Value::object(arr));
    h.collect().unwrap();
    let mut h2 = Heap::from_bytes(h.bytes().to_vec(), DEFAULT_LIMIT).unwrap();
    let arr = h2.realm().as_object().unwrap();
    let a = h2.get_element_fast(arr, 0).unwrap().as_object().unwrap();
    let d = h2.get_element_fast(arr, 1).unwrap().as_object().unwrap();
    assert!(!is_dict(&h2, a) && is_dict(&h2, d));
    let y = key(&mut h2, "y");
    assert_eq!(get(&h2, a, y), Some(Value::int(1)));
    assert_eq!(get(&h2, d, y), Some(Value::int(1)));
}

#[test]
fn compacted_shapes_keep_working() {
    // 0098: compact_for_image shrinks descriptor and transition arrays to their use; the
    // tree then grows by copying.
    let mut h = heap();
    let a = obj(&mut h, &["a", "b", "c"]);
    let b = obj(&mut h, &["a", "x"]);
    let arr = h
        .new_array_from_values(None, &[Value::object(a), Value::object(b)])
        .unwrap();
    h.set_realm(Value::object(arr));
    h.compact_for_image().unwrap();
    h.collect().unwrap();
    h.verify().unwrap();
    let arr = h.realm().as_object().unwrap();
    let a = h.get_element_fast(arr, 0).unwrap().as_object().unwrap();
    let d = descs(&h, shape_of(&h, a));
    assert_eq!(h.u32_array_len(d), 1 + 2 * 3);
    // Extending the full chain copies; the old shapes still match new objects.
    let d1 = key(&mut h, "d");
    assert!(h.create_data_property(a, d1, Value::int(9)).unwrap());
    let again = obj(&mut h, &["a", "b", "c"]);
    let c = key(&mut h, "c");
    assert_eq!(get(&h, again, c), Some(Value::int(2)));
    assert_eq!(names(&h, a), ["a", "b", "c", "d"]);
    let ax = obj(&mut h, &["a", "x"]);
    let b = h.get_element_fast(arr, 1).unwrap().as_object().unwrap();
    assert_eq!(shape_of(&h, ax), shape_of(&h, b));
    h.verify().unwrap();
}
