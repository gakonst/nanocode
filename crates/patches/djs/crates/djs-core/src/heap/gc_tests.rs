//! GC and handle table tests.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::*;
use crate::value::{Kind, Value};

// ------------------------------------------------------------------ deep compare

/// A canonical text dump of everything reachable from `roots`, by walking the graph from
/// the handles. Objects and symbols get ids in visit order, so two heaps with the same
/// graph give the same dump regardless of where cells are.
fn dump(h: &Heap, roots: &[Handle]) -> String {
    struct D<'a> {
        h: &'a Heap,
        ids: BTreeMap<u64, usize>,
        queue: Vec<Value>,
        out: String,
    }
    impl D<'_> {
        fn val(&mut self, v: Value) -> String {
            match v.kind() {
                Kind::Object | Kind::Symbol => {
                    let next = self.ids.len();
                    let id = *self.ids.entry(v.to_bits()).or_insert_with(|| {
                        self.queue.push(v);
                        next
                    });
                    let p = if v.kind() == Kind::Object { "o" } else { "s" };
                    format!("{p}{id}")
                }
                Kind::String => format!("{:?}", self.h.str_to_string(v.as_string().unwrap())),
                Kind::Number => format!("n{:?}", v.as_number().unwrap()),
                _ => format!("{v:?}"),
            }
        }
        fn key(&mut self, k: Atom) -> String {
            match self.h.atom_symbol(k) {
                Some(s) => format!("[{}]", self.val(Value::symbol(s))),
                None => self.h.atom_name(k),
            }
        }
    }
    let mut d = D {
        h,
        ids: BTreeMap::new(),
        queue: Vec::new(),
        out: String::new(),
    };
    for &r in roots {
        let v = d.val(h.get_handle(r));
        d.out += &format!("root {} = {v}\n", r.index());
    }
    let mut i = 0;
    while i < d.queue.len() {
        let v = d.queue[i];
        i += 1;
        let line = if let Some(s) = v.as_symbol() {
            let desc = d.val(h.symbol_description(s));
            format!("{} symbol {desc}\n", d.val(v))
        } else {
            let o = v.as_object().unwrap();
            let proto = match h.get_prototype_of(o) {
                Some(p) => d.val(Value::object(p)),
                None => String::from("null"),
            };
            let mut line = format!(
                "{} {:?} proto={proto} ext={}",
                d.val(v),
                h.object_class(o),
                h.is_extensible(o)
            );
            for k in h.own_keys(o) {
                let ks = d.key(k);
                let p = h.get_own_property(o, k).unwrap();
                let f = p.flags().bits();
                let ps = match p {
                    Property::Data { value, .. } => d.val(value),
                    Property::Accessor { getter, setter, .. } => {
                        format!("get {} set {}", d.val(getter), d.val(setter))
                    }
                };
                line += &format!(" {ks}:{f}={ps}");
            }
            line + "\n"
        };
        d.out += &line;
    }
    d.out
}

// ------------------------------------------------------------------ random graphs

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn chance(&mut self, pct: u64) -> bool {
        self.next() % 100 < pct
    }
}

/// Handles whose value satisfies `pred`.
fn pick(h: &Heap, rng: &mut Rng, hs: &[Handle], pred: impl Fn(Value) -> bool) -> Option<Handle> {
    let c: Vec<Handle> = hs
        .iter()
        .copied()
        .filter(|&x| pred(h.get_handle(x)))
        .collect();
    if c.is_empty() {
        None
    } else {
        Some(c[rng.below(c.len())])
    }
}

fn is_obj(v: Value) -> bool {
    v.as_object().is_some()
}

fn random_key(h: &mut Heap, rng: &mut Rng, hs: &[Handle]) -> Atom {
    match rng.below(4) {
        0 => Atom::int(rng.below(40) as u32).unwrap(),
        1 => {
            if let Some(s) = pick(h, rng, hs, |v| v.as_symbol().is_some()) {
                h.symbol_atom(h.get_handle(s).as_symbol().unwrap())
            } else {
                atoms::NAME
            }
        }
        _ => h.atom_str(&format!("k{}", rng.below(30))).unwrap(),
    }
}

fn random_value(h: &mut Heap, rng: &mut Rng, hs: &[Handle]) -> Value {
    match rng.below(5) {
        0 => Value::int(rng.below(1000) as i32),
        1 => Value::number(rng.below(1000) as f64 / 7.0),
        2 => [Value::UNDEFINED, Value::NULL, Value::TRUE][rng.below(3)],
        _ => {
            if hs.is_empty() {
                Value::NULL
            } else {
                h.get_handle(hs[rng.below(hs.len())])
            }
        }
    }
}

/// Runs `ops` random mutations, holding everything through handles. With `gc_every`,
/// collects every that many ops. Returns the live handles.
fn build_random(h: &mut Heap, seed: u64, ops: usize, gc_every: Option<usize>) -> Vec<Handle> {
    let base = h.new_object(None).unwrap();
    let mut hs = alloc::vec![h.new_handle(Value::object(base)).unwrap()];
    grow_random(h, seed, ops, &mut hs, gc_every, |h| {
        h.collect().unwrap();
    });
    hs
}

/// [`build_random`] on the graph of the handles `hs` (mutating and dropping them too),
/// collecting with `collect` every `gc_every` ops.
fn grow_random(
    h: &mut Heap,
    seed: u64,
    ops: usize,
    hs: &mut Vec<Handle>,
    gc_every: Option<usize>,
    mut collect: impl FnMut(&mut Heap),
) {
    let mut rng = Rng(seed | 1);
    for op in 0..ops {
        if let Some(n) = gc_every
            && op % n == n - 1
        {
            collect(h);
        }
        match rng.below(14) {
            0 | 1 => {
                let proto = pick(h, &mut rng, hs, is_obj)
                    .filter(|_| rng.chance(70))
                    .map(|p| h.get_handle(p).as_object().unwrap());
                let o = h.new_object(proto).unwrap();
                hs.push(h.new_handle(Value::object(o)).unwrap());
            }
            2 => {
                let a = h.new_array(None, rng.below(4) as u32).unwrap();
                hs.push(h.new_handle(Value::object(a)).unwrap());
            }
            3 => {
                let len = [1, 5, 40, 100][rng.below(4)];
                let t: String = (0..len)
                    .map(|i| {
                        if rng.chance(10) {
                            '€'
                        } else {
                            (b'a' + (i % 26) as u8) as char
                        }
                    })
                    .collect();
                let s = h.alloc_str(&t).unwrap();
                hs.push(h.new_handle(Value::string(s)).unwrap());
            }
            4 => {
                // Ropes, sometimes flattened.
                let a = pick(h, &mut rng, hs, |v| v.as_string().is_some());
                let b = pick(h, &mut rng, hs, |v| v.as_string().is_some());
                if let (Some(a), Some(b)) = (a, b) {
                    let sa = h.get_handle(a).as_string().unwrap();
                    let sb = h.get_handle(b).as_string().unwrap();
                    let r = h.concat(sa, sb).unwrap();
                    if rng.chance(30) {
                        h.flatten(r).unwrap();
                    }
                    hs.push(h.new_handle(Value::string(r)).unwrap());
                }
            }
            5..=7 => {
                if let Some(o) = pick(h, &mut rng, hs, is_obj) {
                    let k = random_key(h, &mut rng, hs);
                    let v = random_value(h, &mut rng, hs);
                    let o = h.get_handle(o).as_object().unwrap();
                    let _ = h.create_data_property(o, k, v);
                }
            }
            8 => {
                if let Some(o) = pick(h, &mut rng, hs, is_obj) {
                    let o = h.get_handle(o).as_object().unwrap();
                    let keys = h.own_keys(o);
                    if !keys.is_empty() {
                        let k = keys[rng.below(keys.len())];
                        h.delete_own_property(o, k).unwrap();
                    }
                }
            }
            9 => {
                if hs.len() > 2 {
                    let i = 1 + rng.below(hs.len() - 1);
                    h.drop_handle(hs.swap_remove(i));
                }
            }
            10 => {
                if let Some(o) = pick(h, &mut rng, hs, is_obj) {
                    let k = random_key(h, &mut rng, hs);
                    let g = random_value(h, &mut rng, hs);
                    let g = if is_obj(g) { g } else { Value::UNDEFINED };
                    let fl = PropFlags::from_bits(rng.below(8) as u8);
                    let o = h.get_handle(o).as_object().unwrap();
                    let d = if rng.chance(50) {
                        PropertyDescriptor::accessor(g, Value::UNDEFINED, fl)
                    } else {
                        PropertyDescriptor::data(g, fl)
                    };
                    let _ = h.define_own_property(o, k, &d);
                }
            }
            11 => {
                let desc = if rng.chance(50) {
                    Value::string(h.alloc_str(&format!("sym{op}")).unwrap())
                } else {
                    Value::UNDEFINED
                };
                let s = h.new_symbol(desc).unwrap();
                hs.push(h.new_handle(Value::symbol(s)).unwrap());
            }
            12 => {
                // Array elements: dense appends, holes, sparse.
                if let Some(a) = pick(h, &mut rng, hs, |v| v.as_object().is_some()) {
                    let o = h.get_handle(a).as_object().unwrap();
                    if h.object_class(o) == ObjectClass::Array {
                        let v = random_value(h, &mut rng, hs);
                        let i = if rng.chance(80) {
                            h.array_length(o)
                        } else {
                            rng.below(500) as u32
                        };
                        if !h.set_element_fast(o, i, v).unwrap() {
                            let _ = h.create_data_property(o, Atom::int(i).unwrap(), v);
                        }
                    }
                }
            }
            _ => {
                if let (Some(x), Some(y)) = (
                    pick(h, &mut rng, hs, |_| true),
                    pick(h, &mut rng, hs, |_| true),
                ) {
                    let v = h.get_handle(y);
                    h.set_handle(x, v);
                }
            }
        }
    }
}

#[test]
fn random_graphs_survive_collection() {
    for seed in 1..=40u64 {
        let mut h = Heap::new().unwrap();
        let hs = build_random(&mut h, seed.wrapping_mul(0x9E37_79B9_7F4A_7C15), 400, None);
        h.verify().unwrap();
        let d0 = dump(&h, &hs);
        let stats = h.collect().unwrap();
        assert!(stats.after <= stats.before);
        h.verify().unwrap();
        assert_eq!(dump(&h, &hs), d0, "seed {seed}");
        // Collecting again is a fixpoint, byte for byte.
        let b1 = h.bytes().to_vec();
        let s2 = h.collect().unwrap();
        assert_eq!(s2.before, s2.after);
        assert_eq!(h.bytes(), &b1[..], "seed {seed}");
        // The image round trip still works, handles included.
        let mut r = Heap::from_bytes(b1, DEFAULT_LIMIT).unwrap();
        assert_eq!(dump(&r, &hs), d0);
        r.collect().unwrap();
        assert_eq!(dump(&r, &hs), d0);
    }
}

#[test]
fn collecting_during_construction_changes_nothing() {
    // The same mutation sequence with and without collections in between ends in the same
    // graph.
    for seed in 1..=20u64 {
        let seed = seed.wrapping_mul(0xD1B5_4A32_D192_ED03);
        let mut a = Heap::new().unwrap();
        let mut b = Heap::new().unwrap();
        let ha = build_random(&mut a, seed, 600, None);
        let hb = build_random(&mut b, seed, 600, Some(37));
        assert_eq!(ha, hb);
        b.verify().unwrap();
        assert_eq!(dump(&a, &ha), dump(&b, &hb), "seed {seed}");
        // Both collected: identical images.
        a.collect().unwrap();
        b.collect().unwrap();
        assert_eq!(a.bytes(), b.bytes(), "seed {seed}");
    }
}

#[test]
fn both_collectors_keep_the_same_graph() {
    // 0101: mark-compact (in place, address order) and Cheney (BFS) keep the same cells:
    // the same graph and the same live bytes, also with a tiny mark stack (overflow
    // rescans), and a mark-compact heap collects to itself.
    for cap in [0, 3] {
        super::compactor::TEST_STACK_CAP.with(|c| c.set(cap));
        for seed in 1..=30u64 {
            let seed = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
            let mut a = Heap::new().unwrap();
            let ha = build_random(&mut a, seed, 500, None);
            let mut b = Heap::from_bytes_trusted(a.bytes().to_vec(), DEFAULT_LIMIT).unwrap();
            let d0 = dump(&a, &ha);
            let sa = a.collect().unwrap();
            let sb = b.collect_canonical().unwrap();
            assert_eq!(sa.after, sb.after, "seed {seed} cap {cap}");
            a.verify().unwrap();
            assert_eq!(dump(&a, &ha), d0, "seed {seed} cap {cap}");
            assert_eq!(dump(&b, &ha), d0, "seed {seed} cap {cap}");
            let image = a.bytes().to_vec();
            a.collect().unwrap();
            assert_eq!(a.bytes(), &image[..], "seed {seed} cap {cap}");
            // Cheney of the compacted heap is Cheney of the original.
            a.collect_canonical().unwrap();
            assert_eq!(a.bytes(), b.bytes(), "seed {seed} cap {cap}");
        }
    }
    super::compactor::TEST_STACK_CAP.with(|c| c.set(0));
}

#[test]
fn collection_points_do_not_change_compacted_images() {
    // Sliding keeps allocation order, so collecting along the way (in place) ends in the
    // same bytes as collecting once at the end.
    for seed in 1..=20u64 {
        let seed = seed.wrapping_mul(0xA24B_AED4_963E_E407);
        let mut a = Heap::new().unwrap();
        let mut b = Heap::new().unwrap();
        build_random(&mut a, seed, 600, None);
        build_random(&mut b, seed, 600, Some(11));
        a.collect().unwrap();
        b.collect().unwrap();
        assert_eq!(a.bytes(), b.bytes(), "seed {seed}");
    }
}

// ------------------------------------------------------------------ focused tests

#[test]
fn large_live_strings_raise_the_threshold_less() {
    // D3144: the same live size, once as one large string and once as many small ones;
    // the large string counts for less of the next collection's growth.
    let threshold = |big: bool| {
        let mut h = Heap::new().unwrap();
        h.collect().unwrap();
        let parts = if big { 1 } else { 2000 };
        for _ in 0..parts {
            let s = h.alloc_str(&"x".repeat(1_000_000 / parts)).unwrap();
            h.new_handle(Value::string(s)).unwrap();
        }
        let st = h.collect().unwrap();
        (st.after, h.gc_threshold())
    };
    let (small_live, small_t) = threshold(false);
    let (big_live, big_t) = threshold(true);
    assert!(small_live > big_live, "{small_live} {big_live}");
    assert!(
        small_t - small_live > big_t - big_live + 500_000,
        "{small_t} {small_live} {big_t} {big_live}"
    );
}

#[test]
fn garbage_is_freed() {
    let mut h = Heap::new().unwrap();
    let base = h.collect().unwrap().after;
    let keep = h.alloc_str("keep me").unwrap();
    let hk = h.new_handle(Value::string(keep)).unwrap();
    for i in 0..10_000 {
        let o = h.new_object(None).unwrap();
        let s = h.alloc_str(&format!("garbage {i}")).unwrap();
        h.create_data_property(o, atoms::VALUE, Value::string(s))
            .unwrap();
    }
    let st = h.collect().unwrap();
    assert!(st.before > 500_000, "{st:?}");
    assert!(st.after < base + 1024, "{st:?} base {base}");
    assert_eq!(h.arena().len(), st.after);
    let s = h.get_handle(hk).as_string().unwrap();
    assert!(h.str_eq_str(s, "keep me"));
    h.verify().unwrap();
}

#[test]
fn handles() {
    let mut h = Heap::new().unwrap();
    let o = h.new_object(None).unwrap();
    let a = h.new_handle(Value::object(o)).unwrap();
    let b = h.new_handle(Value::int(1)).unwrap();
    assert_eq!(h.handle_count(), 2);
    assert_eq!(h.get_handle(b), Value::int(1));
    h.set_handle(b, Value::TRUE);
    assert_eq!(h.get_handle(b), Value::TRUE);
    h.drop_handle(a);
    assert_eq!(h.handle_count(), 1);
    // Freed slots are reused.
    let c = h.new_handle(Value::NULL).unwrap();
    assert_eq!(c, a);
    // Growth keeps every handle.
    let many: Vec<Handle> = (0..100)
        .map(|i| h.new_handle(Value::int(i)).unwrap())
        .collect();
    for (i, &x) in many.iter().enumerate() {
        assert_eq!(h.get_handle(x), Value::int(i as i32));
    }
    for &x in many.iter().step_by(3) {
        h.drop_handle(x);
    }
    h.collect().unwrap();
    for (i, &x) in many.iter().enumerate() {
        if i % 3 != 0 {
            assert_eq!(h.get_handle(x), Value::int(i as i32));
        }
    }
    assert_eq!(h.handle_count(), 2 + 66);
    h.verify().unwrap();
}

#[test]
#[should_panic(expected = "dropped")]
fn double_drop_panics() {
    let mut h = Heap::new().unwrap();
    let a = h.new_handle(Value::NULL).unwrap();
    h.drop_handle(a);
    h.drop_handle(a);
}

#[test]
fn handles_keep_objects_alive_and_follow_moves() {
    let mut h = Heap::new().unwrap();
    // Garbage first, so the live object moves down.
    for _ in 0..100 {
        h.new_object(None).unwrap();
    }
    let o = h.new_object(None).unwrap();
    let k = h.atom_str("answer").unwrap();
    h.create_data_property(o, k, Value::int(42)).unwrap();
    let ho = h.new_handle(Value::object(o)).unwrap();
    h.collect().unwrap();
    let o2 = h.get_handle(ho).as_object().unwrap();
    assert_ne!(o2, o);
    assert_eq!(
        h.get_own_property(o2, k).and_then(|p| p.value()),
        Some(Value::int(42))
    );
    // Without the handle the object is collected.
    h.drop_handle(ho);
    let before = h.arena().len();
    h.collect().unwrap();
    assert!(h.arena().len() < before);
}

#[test]
fn flattened_ropes_are_short_circuited() {
    let mut h = Heap::new().unwrap();
    let x = h.alloc_str(&"x".repeat(40)).unwrap();
    let y = h.alloc_str(&"€".repeat(40)).unwrap();
    let flat_rope = h.concat(x, y).unwrap();
    h.flatten(flat_rope).unwrap();
    let rope = h.concat(y, x).unwrap();
    let hf = h.new_handle(Value::string(flat_rope)).unwrap();
    let hr = h.new_handle(Value::string(rope)).unwrap();
    // A rope whose child is the flattened rope.
    let outer = h.concat(flat_rope, x).unwrap();
    let ho = h.new_handle(Value::string(outer)).unwrap();
    h.collect().unwrap();
    let f = h.get_handle(hf).as_string().unwrap();
    assert_eq!(h.cell_kind(f.0), CellKind::Str16);
    assert_eq!(h.str_to_string(f), "x".repeat(40) + &"€".repeat(40));
    let r = h.get_handle(hr).as_string().unwrap();
    assert_eq!(h.cell_kind(r.0), CellKind::Rope);
    assert_eq!(h.str_to_string(r), "€".repeat(40) + &"x".repeat(40));
    let o = h.get_handle(ho).as_string().unwrap();
    let left = h.arena().get(o.0, cells::Rope::LEFT).0;
    assert_eq!(left, f.0, "child short-circuited to the shared flat string");
    assert_eq!(h.str_len(o), 120);
    // No flattened rope survives.
    assert!(
        h.cells().all(|c| c.kind != CellKind::Rope
            || h.cell_flags(c.offset) & cells::flags::ROPE_FLATTENED == 0)
    );
    h.verify().unwrap();
}

#[test]
fn atoms_and_symbols_survive() {
    let mut h = Heap::new().unwrap();
    let ids: Vec<Atom> = (0..500)
        .map(|i| h.atom_str(&format!("atom{i}")).unwrap())
        .collect();
    let d = h.alloc_str("desc").unwrap();
    let sym = h.new_symbol(Value::string(d)).unwrap();
    let ks = h.symbol_atom(sym);
    // Garbage between atoms.
    for _ in 0..1000 {
        h.alloc_str("junk junk junk").unwrap();
    }
    h.collect().unwrap();
    for (i, &a) in ids.iter().enumerate() {
        assert_eq!(h.atom_str(&format!("atom{i}")).unwrap(), a);
        assert_eq!(h.atom_name(a), format!("atom{i}"));
    }
    assert_eq!(h.atom_str("length").unwrap(), atoms::LENGTH);
    // The symbol is kept by the atom table (atoms are not collected yet).
    let sym2 = h.atom_symbol(ks).unwrap();
    assert_eq!(h.symbol_atom(sym2), ks);
    let d2 = h.symbol_description(sym2).as_string().unwrap();
    assert!(h.str_eq_str(d2, "desc"));
    let it = h.atom_symbol(atoms::SYM_ITERATOR).unwrap();
    assert_eq!(h.atom_name(h.symbol_atom(it)), "[Symbol.iterator]");
    // New atoms still intern correctly after the move.
    let n = h.atom_str("fresh").unwrap();
    assert_eq!(h.atom_str("fresh").unwrap(), n);
    h.verify().unwrap();
}

#[test]
fn gc_pending_follows_the_threshold() {
    let mut h = Heap::new().unwrap();
    assert!(!h.gc_pending());
    let t = h.gc_threshold();
    assert!(t >= h.arena().len() + gc::MIN_GROWTH.min(1 << 20) / 2);
    while h.arena().len() < t {
        assert!(!h.gc_pending());
        h.alloc_str("some garbage string").unwrap();
    }
    assert!(h.gc_pending());
    h.collect().unwrap();
    assert!(!h.gc_pending());
    assert_eq!(
        h.gc_threshold(),
        gc::next_threshold(h.arena().len(), h.base_end(), h.arena().limit())
    );
}

#[test]
fn failed_collection_leaves_heap_intact() {
    let mut h = Heap::new().unwrap();
    let hs = build_random(&mut h, 77, 300, None);
    let bytes = h.bytes().to_vec();
    let d = dump(&h, &hs);
    h.fail_next_gc_reserve = true;
    assert_eq!(h.collect(), Err(HeapError::HostOutOfMemory));
    assert_eq!(h.bytes(), &bytes[..]);
    assert_eq!(dump(&h, &hs), d);
    h.collect().unwrap();
    assert_eq!(dump(&h, &hs), d);
}

#[test]
fn collection_under_a_tight_cap() {
    // Fill a small heap to its cap with mostly garbage; collecting makes room again.
    let mut h = Heap::with_limit(256 * 1024).unwrap();
    let keep = h.new_object(None).unwrap();
    let hk = h.new_handle(Value::object(keep)).unwrap();
    let mut rounds = 0;
    for i in 0..20_000u32 {
        let r = (|| -> Result<(), HeapError> {
            let s = h.alloc_str(&format!("garbage string number {i}"))?;
            if i % 100 == 0 {
                let o = h.get_handle(hk).as_object().unwrap();
                let k = h.atom_from_index(i / 100)?;
                h.create_data_property(o, k, Value::string(s))?;
            }
            Ok(())
        })();
        if r.is_err() || h.gc_pending() {
            h.collect().unwrap();
            rounds += 1;
            if r.is_err() {
                // Retry once after collecting; it must fit now.
                let s = h.alloc_str(&format!("garbage string number {i}")).unwrap();
                if i % 100 == 0 {
                    let o = h.get_handle(hk).as_object().unwrap();
                    let k = h.atom_from_index(i / 100).unwrap();
                    h.create_data_property(o, k, Value::string(s)).unwrap();
                }
            }
        }
    }
    assert!(rounds > 3, "{rounds}");
    let o = h.get_handle(hk).as_object().unwrap();
    assert_eq!(h.own_keys(o).len(), 200);
    h.verify().unwrap();
}

#[test]
fn empty_heap_collects() {
    let mut h = Heap::new().unwrap();
    let b = h.bytes().to_vec();
    let st = h.collect().unwrap();
    assert!(st.after <= st.before);
    h.verify().unwrap();
    let again = h.bytes().to_vec();
    h.collect().unwrap();
    assert_eq!(h.bytes(), &again[..]);
    let _ = b;
}

/// Rough collection throughput. Run with
/// `cargo test -p djs-core --release gc_throughput -- --ignored --nocapture`.
#[test]
#[ignore]
fn gc_throughput() {
    let mut h = Heap::new().unwrap();
    // A codemode-like live set: arrays of similar small objects with string values.
    let root = h.new_array(None, 0).unwrap();
    let hr = h.new_handle(Value::object(root)).unwrap();
    let keys: Vec<Atom> = ["id", "name", "path", "size", "tags"]
        .iter()
        .map(|k| h.atom_str(k).unwrap())
        .collect();
    for i in 0..200_000u32 {
        let o = h.new_object(None).unwrap();
        for (j, &k) in keys.iter().enumerate() {
            let v = if j % 2 == 0 {
                Value::int(i as i32)
            } else {
                Value::string(h.alloc_str(&format!("value {i} {j}")).unwrap())
            };
            h.create_data_property(o, k, v).unwrap();
        }
        let root = h.get_handle(hr).as_object().unwrap();
        h.set_element_fast(root, i, Value::object(o)).unwrap();
    }
    report("records", &mut h);

    // Pointer dense: a linked list of small objects, plus ropes.
    let mut h = Heap::new().unwrap();
    let k = h.atom_str("next").unwrap();
    let head = h.new_handle(Value::NULL).unwrap();
    let piece = h.alloc_str(&"x".repeat(40)).unwrap();
    let hp = h.new_handle(Value::string(piece)).unwrap();
    for _ in 0..500_000 {
        let o = h.new_object(None).unwrap();
        let next = h.get_handle(head);
        h.create_data_property(o, k, next).unwrap();
        let p = h.get_handle(hp).as_string().unwrap();
        let r = h.concat(p, p).unwrap();
        h.create_data_property(o, atoms::VALUE, Value::string(r))
            .unwrap();
        h.set_handle(head, Value::object(o));
    }
    report("linked list", &mut h);
}

/// Milliseconds per collection of copies of `h` by both collectors (0101).
fn compare(name: &str, h: &Heap) {
    extern crate std;
    use std::time::Instant;
    let n = 10;
    let mut times = [0.0f64; 2];
    let mut after = [0u32; 2];
    for (i, t) in times.iter_mut().enumerate() {
        for _ in 0..n {
            let mut c = Heap::from_bytes_trusted(h.bytes().to_vec(), DEFAULT_LIMIT).unwrap();
            let start = Instant::now();
            let s = if i == 0 {
                c.collect_canonical()
            } else {
                c.collect()
            }
            .unwrap();
            *t += start.elapsed().as_secs_f64() / n as f64;
            after[i] = s.after;
        }
    }
    assert_eq!(after[0], after[1], "both collectors keep the same bytes");
    std::println!(
        "{name}: {:.1} MiB -> {:.1} MiB live; cheney {:.2} ms, mark-compact {:.2} ms ({:.2}x)",
        h.arena().len() as f64 / (1 << 20) as f64,
        after[0] as f64 / (1 << 20) as f64,
        times[0] * 1e3,
        times[1] * 1e3,
        times[1] / times[0]
    );
}

/// Both collectors on live and half-garbage heaps: `cargo test --release -p djs-core --lib
/// gc_collectors_compared -- --ignored --nocapture`.
#[test]
#[ignore]
fn gc_collectors_compared() {
    for garbage in [false, true] {
        let mut h = Heap::new().unwrap();
        let root = h.new_array(None, 0).unwrap();
        let hr = h.new_handle(Value::object(root)).unwrap();
        let keys: Vec<Atom> = ["id", "name", "path", "size", "tags"]
            .iter()
            .map(|k| h.atom_str(k).unwrap())
            .collect();
        let mut n = 0;
        for i in 0..200_000u32 {
            let o = h.new_object(None).unwrap();
            for (j, &k) in keys.iter().enumerate() {
                let v = if j % 2 == 0 {
                    Value::int(i as i32)
                } else {
                    Value::string(h.alloc_str(&format!("value {i} {j}")).unwrap())
                };
                h.create_data_property(o, k, v).unwrap();
            }
            if !garbage || i % 2 == 0 {
                let root = h.get_handle(hr).as_object().unwrap();
                h.set_element_fast(root, n, Value::object(o)).unwrap();
                n += 1;
            }
        }
        compare(
            if garbage {
                "records, half garbage"
            } else {
                "records"
            },
            &h,
        );
        let mut h = Heap::new().unwrap();
        let k = h.atom_str("next").unwrap();
        let head = h.new_handle(Value::NULL).unwrap();
        let piece = h.alloc_str(&"x".repeat(40)).unwrap();
        let hp = h.new_handle(Value::string(piece)).unwrap();
        for i in 0..500_000 {
            let o = h.new_object(None).unwrap();
            let p = h.get_handle(hp).as_string().unwrap();
            let r = h.concat(p, p).unwrap();
            h.create_data_property(o, atoms::VALUE, Value::string(r))
                .unwrap();
            if !garbage || i % 2 == 0 {
                let next = h.get_handle(head);
                h.create_data_property(o, k, next).unwrap();
                h.set_handle(head, Value::object(o));
            }
        }
        compare(
            if garbage {
                "linked list, half garbage"
            } else {
                "linked list"
            },
            &h,
        );
    }
}

fn report(name: &str, h: &mut Heap) {
    extern crate std;
    use std::time::Instant;
    h.collect().unwrap();
    let live = h.arena().len() as f64 / (1 << 20) as f64;
    let cells = h.cells().count();
    let n = 10;
    let t = Instant::now();
    for _ in 0..n {
        h.collect().unwrap();
    }
    let per = t.elapsed().as_secs_f64() / n as f64;
    std::println!(
        "{name}: live {live:.1} MiB in {cells} cells, {:.2} ms per collection, {:.2} ms per MiB",
        per * 1e3,
        per * 1e3 / live
    );
}

#[test]
fn internal_refs_are_traced_and_verified() {
    let mut h = Heap::new().unwrap();
    for _ in 0..100 {
        h.alloc_str("garbage before the internal cell").unwrap();
    }
    // An internal cell (a ValueArray holding a string) in an object slot.
    let arr = h.alloc_value_array(2).unwrap();
    let s = h.alloc_str("inside").unwrap();
    h.value_array_set(arr, 0, Value::string(s));
    h.arena_mut().set(arr, cells::ValueArray::LEN, 1);
    let o = h.alloc_object(ObjectClass::Ordinary, None, 1).unwrap();
    h.set_object_slot(o, 0, Value::internal(arr));
    let ho = h.new_handle(Value::object(o)).unwrap();
    h.verify().unwrap();
    h.collect().unwrap();
    h.verify().unwrap();
    let o = h.get_handle(ho).as_object().unwrap();
    let arr2 = h.object_slot(o, 0).as_internal().unwrap();
    assert_ne!(arr2, arr, "moved");
    assert_eq!(h.cell_kind(arr2), CellKind::ValueArray);
    let s = h.value_array_get(arr2, 0).as_string().unwrap();
    assert!(h.str_eq_str(s, "inside"));

    // An internal reference to a JS cell is rejected.
    let p = h.new_object(None).unwrap();
    h.set_object_slot(o, 0, Value::internal(p.offset()));
    assert!(matches!(h.verify(), Err(walk::VerifyError::BadRef { .. })));
}

// ------------------------------------------------------------------ collections, ephemerons

/// A new object with `k = v` (k a fresh atom name).
fn obj_with(h: &mut Heap, k: &str, v: Value) -> Value {
    let o = h.new_object(None).unwrap();
    let key = h.atom_str(k).unwrap();
    h.create_data_property(o, key, v).unwrap();
    Value::object(o)
}

fn prop(h: &mut Heap, o: Value, k: &str) -> Value {
    let key = h.atom_str(k).unwrap();
    h.get_own_property(o.as_object().unwrap(), key)
        .and_then(|p| p.value())
        .unwrap()
}

fn weak(h: &mut Heap, class: ObjectClass) -> Handle {
    let proto = h.new_object(None).unwrap();
    let m = h.collection_new(class, proto).unwrap();
    h.new_handle(Value::object(m)).unwrap()
}

fn coll(h: &Heap, hd: Handle) -> crate::value::Obj {
    h.get_handle(hd).as_object().unwrap()
}

/// Collects twice: the images must be identical (D1090) and verify.
fn collect_twice(h: &mut Heap) {
    h.collect().unwrap();
    h.verify().unwrap();
    let first = h.bytes().to_vec();
    h.collect().unwrap();
    assert_eq!(h.bytes(), &first[..], "collecting twice changed the image");
}

#[test]
fn weakmap_entries_die_with_their_keys() {
    let mut h = Heap::new().unwrap();
    let wm = weak(&mut h, ObjectClass::WeakMap);
    let live = Value::object(h.new_object(None).unwrap());
    let hl = h.new_handle(live).unwrap();
    for i in 0..20 {
        let k = Value::object(h.new_object(None).unwrap());
        let v = obj_with(&mut h, "i", Value::int(i));
        let m = coll(&h, wm);
        h.collection_set(m, k, v).unwrap();
    }
    let v = obj_with(&mut h, "live", Value::TRUE);
    let m = coll(&h, wm);
    h.collection_set(m, live, v).unwrap();
    assert_eq!(h.collection_size(m), 21);
    collect_twice(&mut h);
    let m = coll(&h, wm);
    assert_eq!(h.collection_size(m), 1);
    let live = h.get_handle(hl);
    let v = h.collection_get(m, live).unwrap();
    assert_eq!(prop(&mut h, v, "live"), Value::TRUE);
    // Once the key dies, so does the entry.
    h.drop_handle(hl);
    collect_twice(&mut h);
    let m = coll(&h, wm);
    assert_eq!(h.collection_size(m), 0);
}

#[test]
fn ephemeron_chains_and_cycles() {
    let mut h = Heap::new().unwrap();
    let wm = weak(&mut h, ObjectClass::WeakMap);
    // k1 -> v1 { next: k2 }, k2 -> v2 { next: k3 }, k3 -> v3. The pairs are inserted in
    // reverse order, so the fixpoint needs several rounds.
    let k3 = Value::object(h.new_object(None).unwrap());
    let v3 = obj_with(&mut h, "end", Value::TRUE);
    let k2 = Value::object(h.new_object(None).unwrap());
    let v2 = obj_with(&mut h, "next", k3);
    let k1 = Value::object(h.new_object(None).unwrap());
    let v1 = obj_with(&mut h, "next", k2);
    let m = coll(&h, wm);
    h.collection_set(m, k3, v3).unwrap();
    h.collection_set(m, k2, v2).unwrap();
    h.collection_set(m, k1, v1).unwrap();
    // A cycle through its own value: k4 -> v4 { back: k4 }, k4 otherwise unreachable.
    let k4 = Value::object(h.new_object(None).unwrap());
    let v4 = obj_with(&mut h, "back", k4);
    h.collection_set(m, k4, v4).unwrap();
    let hk1 = h.new_handle(k1).unwrap();
    collect_twice(&mut h);
    let m = coll(&h, wm);
    assert_eq!(h.collection_size(m), 3, "the chain lives, the cycle dies");
    let k1 = h.get_handle(hk1);
    let v1 = h.collection_get(m, k1).unwrap();
    let k2 = prop(&mut h, v1, "next");
    let v2 = h.collection_get(m, k2).unwrap();
    let k3 = prop(&mut h, v2, "next");
    let v3 = h.collection_get(m, k3).unwrap();
    assert_eq!(prop(&mut h, v3, "end"), Value::TRUE);
    h.drop_handle(hk1);
    collect_twice(&mut h);
    assert_eq!(h.collection_size(coll(&h, wm)), 0);
}

#[test]
fn ephemeron_key_reachable_through_another_table() {
    let mut h = Heap::new().unwrap();
    let a = weak(&mut h, ObjectClass::WeakMap);
    let b = weak(&mut h, ObjectClass::WeakMap);
    // b: x -> "found"; a: root -> { x }. x is reachable only through a's value.
    let x = Value::object(h.new_object(None).unwrap());
    let found = Value::string(h.alloc_str("found").unwrap());
    let mb = coll(&h, b);
    h.collection_set(mb, x, found).unwrap();
    let root = Value::object(h.new_object(None).unwrap());
    let holder = obj_with(&mut h, "x", x);
    let ma = coll(&h, a);
    h.collection_set(ma, root, holder).unwrap();
    let hr = h.new_handle(root).unwrap();
    collect_twice(&mut h);
    let (ma, mb) = (coll(&h, a), coll(&h, b));
    assert_eq!((h.collection_size(ma), h.collection_size(mb)), (1, 1));
    let root = h.get_handle(hr);
    let holder = h.collection_get(ma, root).unwrap();
    let x = prop(&mut h, holder, "x");
    let v = h.collection_get(mb, x).unwrap();
    assert_eq!(h.str_to_string(v.as_string().unwrap()), "found");
}

#[test]
fn weakset_and_image_round_trip() {
    let mut h = Heap::new().unwrap();
    let ws = weak(&mut h, ObjectClass::WeakSet);
    let keep = Value::object(h.new_object(None).unwrap());
    let hk = h.new_handle(keep).unwrap();
    let s = coll(&h, ws);
    h.collection_set(s, keep, Value::TRUE).unwrap();
    for _ in 0..10 {
        let k = Value::object(h.new_object(None).unwrap());
        let s = coll(&h, ws);
        h.collection_set(s, k, Value::TRUE).unwrap();
    }
    collect_twice(&mut h);
    assert_eq!(h.collection_size(coll(&h, ws)), 1);
    // The image loads, verifies, and still finds the key.
    let mut h2 = Heap::from_bytes(h.bytes().to_vec(), usize::MAX).unwrap();
    h2.verify().unwrap();
    let s = coll(&h2, ws);
    let keep = h2.get_handle(hk);
    assert!(h2.collection_has(s, keep));
    h2.collect().unwrap();
    let s = coll(&h2, ws);
    let keep = h2.get_handle(hk);
    assert!(h2.collection_has(s, keep));
}

#[test]
fn maps_rehash_object_keys_after_moves() {
    let mut h = Heap::new().unwrap();
    let hm = weak(&mut h, ObjectClass::Map);
    let mut keys = Vec::new();
    for i in 0..50 {
        // Garbage in between, so the keys move.
        h.new_object(None).unwrap();
        let k = Value::object(h.new_object(None).unwrap());
        keys.push(h.new_handle(k).unwrap());
        let m = coll(&h, hm);
        h.collection_set(m, k, Value::int(i)).unwrap();
        let s = Value::string(h.alloc_str(&format!("s{i}")).unwrap());
        h.collection_set(m, s, Value::int(-i)).unwrap();
    }
    collect_twice(&mut h);
    let m = coll(&h, hm);
    assert_eq!(h.collection_size(m), 100);
    for (i, &hk) in keys.iter().enumerate() {
        let k = h.get_handle(hk);
        assert_eq!(h.collection_get(m, k), Some(Value::int(i as i32)));
        let s = Value::string(h.alloc_str(&format!("s{i}")).unwrap());
        assert_eq!(h.collection_get(m, s), Some(Value::int(-(i as i32))));
    }
    // -0 and +0 are one key, NaN equals NaN.
    h.collection_set(m, Value::number(-0.0), Value::int(1))
        .unwrap();
    h.collection_set(m, Value::NAN, Value::int(2)).unwrap();
    assert_eq!(h.collection_get(m, Value::int(0)), Some(Value::int(1)));
    assert_eq!(
        h.collection_get(m, Value::number(f64::NAN)),
        Some(Value::int(2))
    );
}

#[test]
fn iterators_survive_deletes_compaction_and_clear() {
    let mut h = Heap::new().unwrap();
    let hm = weak(&mut h, ObjectClass::Map);
    let m = coll(&h, hm);
    for i in 0..40 {
        h.collection_set(m, Value::int(i), Value::int(i)).unwrap();
    }
    let proto = h.new_object(None).unwrap();
    let it = h.collection_iterator(m, 0, proto).unwrap();
    let hi = h.new_handle(Value::object(it)).unwrap();
    let mut seen = Vec::new();
    for _ in 0..5 {
        let it = coll(&h, hi);
        seen.push(h.collection_iterator_next(it).unwrap().0);
    }
    // Delete most entries (compacts), add new ones, collect: the iterator continues
    // after 4 with the survivors and the additions.
    let m = coll(&h, hm);
    for i in 0..40 {
        if i % 10 != 9 {
            h.collection_delete(m, Value::int(i)).unwrap();
        }
    }
    h.collection_set(m, Value::int(100), Value::int(100))
        .unwrap();
    collect_twice(&mut h);
    loop {
        let it = coll(&h, hi);
        match h.collection_iterator_next(it) {
            Some((k, _)) => seen.push(k),
            None => break,
        }
    }
    let nums: Vec<i32> = seen.iter().map(|v| v.as_int().unwrap()).collect();
    assert_eq!(nums, [0, 1, 2, 3, 4, 9, 19, 29, 39, 100]);
    // After clear, entries added later are still visited by a new iterator walk.
    let m = coll(&h, hm);
    let proto = h.new_object(None).unwrap();
    let it2 = h.collection_iterator(m, 0, proto).unwrap();
    let h2 = h.new_handle(Value::object(it2)).unwrap();
    let it2 = coll(&h, h2);
    assert_eq!(h.collection_iterator_next(it2).unwrap().0, Value::int(9));
    let m = coll(&h, hm);
    h.collection_clear(m).unwrap();
    h.collection_set(m, Value::int(7), Value::int(7)).unwrap();
    let it2 = coll(&h, h2);
    assert_eq!(h.collection_iterator_next(it2).unwrap().0, Value::int(7));
    assert!(h.collection_iterator_next(it2).is_none());
}

// ------------------------------------------------------------------ old region (0120)

/// A heap whose first `ops` random mutations are old (collected fully), then `more`
/// mutations of the same graph (old cells included) that are young.
fn old_and_young(seed: u64, ops: usize, more: usize) -> (Heap, Vec<Handle>) {
    let mut h = Heap::new().unwrap();
    let mut hs = build_random(&mut h, seed, ops, None);
    h.seal().unwrap();
    assert_eq!(h.old_end(), h.arena().len());
    grow_random(&mut h, seed ^ 0x55, more, &mut hs, None, |_| {});
    (h, hs)
}

#[test]
fn minor_collections_keep_the_graph() {
    // A minor collection keeps the old cells where they are and everything reachable;
    // a full collection afterwards leaves exactly what a full one alone would (sliding
    // keeps allocation order), also with a tiny mark stack.
    for cap in [0, 3] {
        super::compactor::TEST_STACK_CAP.with(|c| c.set(cap));
        for seed in 1..=30u64 {
            let seed = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
            let (mut a, hs) = old_and_young(seed, 300, 400);
            let mut b = Heap::from_bytes_trusted(a.bytes().to_vec(), DEFAULT_LIMIT).unwrap();
            let old = a.old_end();
            let old_bytes = a.bytes()[..old as usize].to_vec();
            let d0 = dump(&a, &hs);
            let sa = a.collect_minor().unwrap();
            a.verify().unwrap();
            assert_eq!(dump(&a, &hs), d0, "seed {seed} cap {cap}");
            assert_eq!(a.old_end(), old);
            assert!(sa.after >= old && sa.after <= sa.before);
            // Old cells do not move; only reference slots to young cells and the roots'
            // counters change.
            let same = a.bytes()[..old as usize]
                .chunks(8)
                .zip(old_bytes.chunks(8))
                .filter(|(x, y)| x == y)
                .count();
            assert!(same * 8 > old as usize * 9 / 10, "seed {seed}");
            // A second minor collection finds nothing more.
            let image = a.bytes().len();
            a.collect_minor().unwrap();
            assert_eq!(a.bytes().len(), image, "seed {seed} cap {cap}");
            let sb = b.collect().unwrap();
            assert!(sb.after <= sa.after, "seed {seed}");
            a.collect().unwrap();
            assert_eq!(a.bytes(), b.bytes(), "seed {seed} cap {cap}");
        }
    }
    super::compactor::TEST_STACK_CAP.with(|c| c.set(0));
}

#[test]
fn scheduled_collections_do_not_change_compacted_images() {
    // Collecting on the schedule (mostly minor) along the way ends, after a final full
    // collection, in the same bytes as collecting once at the end.
    for seed in 1..=20u64 {
        let seed = seed.wrapping_mul(0xA24B_AED4_963E_E407);
        let mut a = Heap::new().unwrap();
        let mut b = Heap::new().unwrap();
        let mut ha = build_random(&mut a, seed, 300, None);
        let mut hb = build_random(&mut b, seed, 300, None);
        a.seal().unwrap();
        b.seal().unwrap();
        grow_random(&mut a, seed, 900, &mut ha, None, |_| {});
        let mut minors = 0;
        grow_random(&mut b, seed, 900, &mut hb, Some(7), |h| {
            let full = h.arena().get(0, Roots::MINORS) + 1 >= gc::FULL_EVERY;
            h.collect_auto().unwrap();
            minors += usize::from(!full);
            h.verify().unwrap();
        });
        assert!(minors > 0);
        assert_eq!(dump(&a, &ha), dump(&b, &hb), "seed {seed}");
        a.collect().unwrap();
        b.collect().unwrap();
        assert_eq!(a.bytes(), b.bytes(), "seed {seed}");
    }
}

#[test]
fn the_schedule_is_image_state() {
    // Restoring the image at every step collects exactly as continuing does.
    let (mut a, mut ha) = old_and_young(7, 200, 200);
    let mut hb = ha.clone();
    let mut b = Heap::from_bytes(a.bytes().to_vec(), DEFAULT_LIMIT).unwrap();
    let mut kinds = Vec::new();
    for i in 0..2 * gc::FULL_EVERY as usize {
        grow_random(&mut a, i as u64, 50, &mut ha, None, |_| {});
        grow_random(&mut b, i as u64, 50, &mut hb, None, |_| {});
        let full = a.arena().get(0, Roots::MINORS) + 1 >= gc::FULL_EVERY;
        kinds.push(full);
        a.collect_auto().unwrap();
        b.collect_auto().unwrap();
        b = Heap::from_bytes(b.bytes().to_vec(), DEFAULT_LIMIT).unwrap();
        assert_eq!(a.bytes(), b.bytes(), "step {i}");
    }
    // Every FULL_EVERY-th at least (more when the young survivors outgrow the old).
    assert!(kinds.iter().filter(|&&f| f).count() >= 2, "{kinds:?}");
    assert!(kinds.iter().filter(|&&f| !f).count() >= 16, "{kinds:?}");
}

#[test]
fn dead_old_cells_keep_young_cells_until_a_full_collection() {
    let mut h = Heap::new().unwrap();
    let holder = h.new_object(None).unwrap();
    let hh = h.new_handle(Value::object(holder)).unwrap();
    h.seal().unwrap();
    // A young string, referenced only by the old holder; then the holder dies.
    let s = Value::string(h.alloc_str(&"young".repeat(100)).unwrap());
    let holder = h.get_handle(hh).as_object().unwrap();
    let k = h.atom_str("s").unwrap();
    h.create_data_property(holder, k, s).unwrap();
    h.drop_handle(hh);
    let old = h.old_end();
    // The minor collection keeps it (the dead holder is old)...
    let minor = h.collect_minor().unwrap();
    assert!(minor.after >= old + 500, "{minor:?} {old}");
    // ... and a full one frees both (what is left are the shapes the holder's property
    // made: they never die).
    let full = h.collect().unwrap();
    assert!(full.after + 500 < minor.after, "{full:?} {minor:?}");
    // The old region shrinks to its live cells (the holder died); the rest stays young.
    assert!(
        h.old_end() < old && h.old_end() < full.after,
        "{} {old}",
        h.old_end()
    );
    h.verify().unwrap();
}

#[test]
fn old_collections_with_young_keys() {
    // An old Map with young object keys is rehashed (they move); an old WeakMap's pairs
    // with young keys take part in the fixpoint and are cleared when the key dies.
    let mut h = Heap::new().unwrap();
    let hm = weak(&mut h, ObjectClass::Map);
    let hw = weak(&mut h, ObjectClass::WeakMap);
    let old_key = h.new_object(None).unwrap();
    let hok = h.new_handle(Value::object(old_key)).unwrap();
    h.seal().unwrap();
    let mut keep = Vec::new();
    for i in 0..30 {
        h.new_object(None).unwrap(); // garbage, so the keys move
        let k = Value::object(h.new_object(None).unwrap());
        let m = coll(&h, hm);
        h.collection_set(m, k, Value::int(i)).unwrap();
        let w = coll(&h, hw);
        let v = obj_with(&mut h, "i", Value::int(i));
        h.collection_set(w, k, v).unwrap();
        if i % 3 == 0 {
            keep.push(h.new_handle(k).unwrap());
        }
        // A pair with a dead young key.
        let dead = Value::object(h.new_object(None).unwrap());
        h.collection_set(w, dead, Value::int(-1)).unwrap();
    }
    // An old key with a young value.
    let ok = h.get_handle(hok);
    let v = obj_with(&mut h, "old", Value::TRUE);
    let w = coll(&h, hw);
    h.collection_set(w, ok, v).unwrap();
    let old = h.old_end();
    h.collect_minor().unwrap();
    h.verify().unwrap();
    assert_eq!(h.old_end(), old);
    let (m, w) = (coll(&h, hm), coll(&h, hw));
    assert!(m.0 < old && w.0 < old);
    assert_eq!(h.collection_size(m), 30);
    // The Map keeps the keys alive; the pairs with dead keys are gone.
    assert_eq!(h.collection_size(w), 31);
    for (j, &hk) in keep.iter().enumerate() {
        let k = h.get_handle(hk);
        assert_eq!(h.collection_get(m, k), Some(Value::int(3 * j as i32)));
        let v = h.collection_get(w, k).unwrap();
        assert_eq!(prop(&mut h, v, "i"), Value::int(3 * j as i32));
    }
    let ok = h.get_handle(hok);
    let v = h.collection_get(w, ok).unwrap();
    assert_eq!(prop(&mut h, v, "old"), Value::TRUE);
}

#[test]
fn old_ropes_flattened_later() {
    // A rope made old, then flattened: its flat string is young, kept through the rope.
    let mut h = Heap::new().unwrap();
    let a = h.alloc_str(&"a".repeat(40)).unwrap();
    let b = h.alloc_str(&"b".repeat(40)).unwrap();
    let r = h.concat(a, b).unwrap();
    let hr = h.new_handle(Value::string(r)).unwrap();
    h.seal().unwrap();
    let r = h.get_handle(hr).as_string().unwrap();
    h.flatten(r).unwrap();
    // A young reference to the old rope too.
    let o = obj_with(&mut h, "r", Value::string(r));
    let ho = h.new_handle(o).unwrap();
    h.collect_minor().unwrap();
    h.verify().unwrap();
    let r = h.get_handle(hr).as_string().unwrap();
    assert_eq!(h.str_to_string(r), "a".repeat(40) + &"b".repeat(40));
    let o = h.get_handle(ho);
    let r2 = prop(&mut h, o, "r").as_string().unwrap();
    assert_eq!(h.str_to_string(r2), "a".repeat(40) + &"b".repeat(40));
}

#[test]
fn the_first_collection_after_a_canonical_image_is_full() {
    // D2045: a VM made from a canonical image (the base) collects in full first, so the
    // cells its setup killed in the old region are gone before they shift thresholds.
    let mut h = Heap::new().unwrap();
    build_random(&mut h, 3, 300, None);
    let o = h.new_object(None).unwrap();
    let ho = h.new_handle(Value::object(o)).unwrap();
    h.collect_canonical().unwrap();
    let old = h.old_end();
    assert_eq!(old, h.arena().len());
    // Grow an old object: its old property storage dies.
    let o = h.get_handle(ho).as_object().unwrap();
    for i in 0..40 {
        let k = h.atom_str(&format!("k{i}")).unwrap();
        h.create_data_property(o, k, Value::int(i)).unwrap();
    }
    h.collect_auto().unwrap();
    assert_eq!(h.arena().get(0, Roots::MINORS), 0, "full");
    assert!(h.old_end() <= old);
    h.collect_auto().unwrap();
    assert_eq!(h.arena().get(0, Roots::MINORS), 1, "then minor");
    h.verify().unwrap();
}

#[test]
fn collections_never_move_the_prefix() {
    // D2064: below BASE_END nothing moves or is freed, in full, minor and canonical
    // collections; dead cells there stay (retained), live cells above it are collected.
    let mut h = Heap::new().unwrap();
    let hs = build_random(&mut h, 5, 300, None);
    let o = h.new_object(None).unwrap();
    for i in 0..3 {
        let k = h.atom_str(&format!("q{i}")).unwrap();
        h.create_data_property(o, k, Value::int(i)).unwrap();
    }
    let ho = h.new_handle(Value::object(o)).unwrap();
    h.seal().unwrap();
    let end = h.arena().len();
    h.arena_mut().set(0, Roots::BASE_END, end);
    let prefix = h.bytes()[88..end as usize].to_vec();
    // Garbage below the prefix end: the old object's storage dies when it grows.
    let o = h.get_handle(ho).as_object().unwrap();
    for i in 0..40 {
        let k = h.atom_str(&format!("p{i}")).unwrap();
        h.create_data_property(o, k, Value::int(i)).unwrap();
    }
    let changed = |h: &Heap| {
        h.bytes()[88..end as usize]
            .chunks(8)
            .zip(prefix.chunks(8))
            .filter(|(a, b)| a != b)
            .count()
    };
    let written = changed(&h);
    h.collect_minor().unwrap();
    h.collect().unwrap();
    assert_eq!(h.base_end(), end);
    assert_eq!(
        changed(&h),
        written,
        "only the words the mutation wrote differ"
    );
    h.verify().unwrap();
    let d = dump(&h, &hs);
    h.collect_canonical().unwrap();
    assert_eq!(h.base_end(), end);
    assert_eq!(changed(&h), written);
    assert_eq!(dump(&h, &hs), d);
    h.verify().unwrap();
}
