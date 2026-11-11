//! Decoder tests: a test-only Rust encoder for building buffers, a canonical dump of heap
//! values (shared with the cross-language test), validation errors, and fuzzing.

extern crate std;

use super::*;
use crate::heap::{Heap, ObjectClass};
use crate::num;
use crate::value::Kind;
use std::format;
use std::string::{String, ToString};
use std::vec;
use std::vec::Vec;

/// A test value, in the shapes the format can carry.
#[derive(Clone, Debug)]
pub(super) enum T {
    Undef,
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Str16(Vec<u16>),
    Arr(Vec<Option<T>>),
    Obj(Vec<(String, T)>),
    Bytes(Vec<u8>),
}

/// The reference encoder (what js/src/wire.ts writes, without its padding tricks).
pub(super) struct Enc {
    pub(super) out: Vec<u8>,
    keys: Vec<String>,
}

impl Enc {
    pub(super) fn new() -> Enc {
        Enc {
            out: vec![b'D', b'J', b'W', VERSION],
            keys: Vec::new(),
        }
    }

    fn varint(&mut self, mut v: u32) {
        while v >= 0x80 {
            self.out.push((v as u8 & 0x7F) | 0x80);
            v >>= 7;
        }
        self.out.push(v as u8);
    }

    fn string(&mut self, s: &str) {
        self.out.push(tag::STR_UTF8);
        self.varint(s.encode_utf16().count() as u32);
        self.varint(s.len() as u32);
        self.out.extend_from_slice(s.as_bytes());
    }

    pub(super) fn value(&mut self, v: &T) {
        match v {
            T::Undef => self.out.push(tag::UNDEFINED),
            T::Null => self.out.push(tag::NULL),
            T::Bool(b) => self.out.push(if *b { tag::TRUE } else { tag::FALSE }),
            T::Num(n) => {
                let i = *n as i32;
                if i as f64 == *n && !(i == 0 && n.is_sign_negative()) {
                    if (-128..=127).contains(&i) {
                        self.out.push(tag::INT8);
                        self.out.push(i as i8 as u8);
                    } else {
                        self.out.push(tag::INT32);
                        self.out.extend_from_slice(&i.to_le_bytes());
                    }
                } else {
                    self.out.push(tag::FLOAT64);
                    self.out.extend_from_slice(&n.to_le_bytes());
                }
            }
            T::Str(s) => self.string(s),
            T::Str16(u) => {
                self.out.push(tag::STR_UTF16);
                self.varint(u.len() as u32);
                for &x in u {
                    self.out.extend_from_slice(&x.to_le_bytes());
                }
            }
            T::Arr(items) => {
                self.out.push(tag::ARRAY);
                self.varint(items.len() as u32);
                for it in items {
                    match it {
                        Some(v) => self.value(v),
                        None => self.out.push(tag::HOLE),
                    }
                }
            }
            T::Obj(props) => {
                self.out.push(tag::OBJECT);
                self.varint(props.len() as u32);
                for (k, v) in props {
                    match self.keys.iter().position(|x| x == k) {
                        Some(i) => self.varint(i as u32 + 1),
                        None => {
                            self.keys.push(k.clone());
                            self.out.push(0);
                            self.string(k);
                        }
                    }
                    self.value(v);
                }
            }
            T::Bytes(b) => {
                self.out.push(tag::BYTES);
                self.varint(b.len() as u32);
                self.out.extend_from_slice(b);
            }
        }
    }
}

pub(super) fn encode(v: &T) -> Vec<u8> {
    let mut e = Enc::new();
    e.value(v);
    e.out
}

fn quote(units: &[u16]) -> String {
    let mut s = String::from("\"");
    for &u in units {
        if (0x20..0x7F).contains(&u) && u != b'"' as u16 && u != b'\\' as u16 {
            s.push(u as u8 as char);
        } else {
            s.push_str(&format!("\\u{u:04x}"));
        }
    }
    s.push('"');
    s
}

/// A canonical dump: `undefined`, `null`, `true`, `n:<String(x)>`, `s:"..."` (non-printable
/// ASCII as `\uXXXX`), `[a,<hole>]`, `{"k":v}`. js/test/gen-cross.ts writes the same.
pub(super) fn dump(h: &mut Heap, v: Value) -> String {
    match v.kind() {
        Kind::Undefined => "undefined".into(),
        Kind::Null => "null".into(),
        Kind::Bool => format!("{}", v.as_bool().unwrap()),
        Kind::Number => {
            let n = v.as_number().unwrap();
            if n == 0.0 && n.is_sign_negative() {
                "n:-0".into()
            } else {
                format!("n:{}", num::to_string(n).as_str())
            }
        }
        Kind::String => format!("s:{}", quote(&h.str_to_utf16(v.as_string().unwrap()))),
        Kind::Object => {
            let o = v.as_object().unwrap();
            if h.is_typed_array(o) {
                let n = h.ta_length(o).unwrap();
                // As js/test/dump.ts prints a Uint8Array.
                let items: Vec<String> = (0..n)
                    .map(|i| format!("n:{}", h.ta_get(o, i).unwrap().as_int().unwrap()))
                    .collect();
                return format!("[{}]", items.join(","));
            }
            if h.object_class(o) == ObjectClass::Array {
                let mut items: Vec<String> = Vec::new();
                for i in 0..h.array_length(o) {
                    items.push(match h.get_own_property(o, Atom::int(i).unwrap()) {
                        Some(p) => dump(h, p.value().unwrap()),
                        None => "<hole>".into(),
                    });
                }
                format!("[{}]", items.join(","))
            } else {
                let mut items: Vec<String> = Vec::new();
                for k in h.own_keys(o) {
                    // The key's UTF-16 (atom_name would be lossy for lone surrogates).
                    let name_str = h.atom_string(k).unwrap();
                    let name = h.str_to_utf16(name_str);
                    let p = h.get_own_property(o, k).unwrap();
                    items.push(format!("{}:{}", quote(&name), dump(h, p.value().unwrap())));
                }
                format!("{{{}}}", items.join(","))
            }
        }
        k => format!("<{k:?}>"),
    }
}

fn dump_t(v: &T) -> String {
    match v {
        T::Undef => "undefined".into(),
        T::Null => "null".into(),
        T::Bool(b) => b.to_string(),
        T::Num(n) => {
            if *n == 0.0 && n.is_sign_negative() {
                "n:-0".into()
            } else {
                format!("n:{}", num::to_string(*n).as_str())
            }
        }
        T::Str(s) => format!("s:{}", quote(&s.encode_utf16().collect::<Vec<_>>())),
        T::Str16(u) => format!("s:{}", quote(u)),
        T::Arr(items) => {
            let parts: Vec<String> = items
                .iter()
                .map(|i| i.as_ref().map_or("<hole>".into(), dump_t))
                .collect();
            format!("[{}]", parts.join(","))
        }
        T::Obj(props) => {
            // JS property order: integer keys ascending, then the rest in order.
            let mut ints: Vec<(u32, &String, &T)> = Vec::new();
            let mut rest = Vec::new();
            for (k, v) in props {
                match k.parse::<u32>() {
                    Ok(i) if i.to_string() == *k && i < u32::MAX => ints.push((i, k, v)),
                    _ => rest.push((k, v)),
                }
            }
            ints.sort_by_key(|x| x.0);
            let parts: Vec<String> = ints
                .iter()
                .map(|(_, k, v)| (*k, *v))
                .chain(rest)
                .map(|(k, v)| {
                    format!(
                        "{}:{}",
                        quote(&k.encode_utf16().collect::<Vec<_>>()),
                        dump_t(v)
                    )
                })
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        T::Bytes(b) => {
            let parts: Vec<String> = b.iter().map(|x| format!("n:{x}")).collect();
            format!("[{}]", parts.join(","))
        }
    }
}

fn decode_ok(bytes: &[u8]) -> (Heap, Value) {
    let mut h = Heap::new().unwrap();
    let v = decode(&mut h, bytes, &DecodeOptions::default()).unwrap_or_else(|e| panic!("{e}"));
    h.verify().unwrap();
    (h, v)
}

fn check(v: T) {
    let bytes = encode(&v);
    let (mut h, out) = decode_ok(&bytes);
    assert_eq!(dump(&mut h, out), dump_t(&v), "{v:?}");
}

fn s(x: &str) -> T {
    T::Str(x.into())
}

fn invalid(bytes: &[u8]) -> (usize, &'static str) {
    let mut h = Heap::new().unwrap();
    match decode(&mut h, bytes, &DecodeOptions::default()) {
        Err(WireError::Invalid { offset, reason }) => (offset, reason),
        other => panic!("expected an error, got {other:?}"),
    }
}

#[test]
fn primitives() {
    for v in [
        T::Undef,
        T::Null,
        T::Bool(true),
        T::Bool(false),
        T::Num(0.0),
        T::Num(-0.0),
        T::Num(1.0),
        T::Num(-128.0),
        T::Num(300.0),
        T::Num(-(2f64.powi(31))),
        T::Num(2f64.powi(31)),
        T::Num(1.5),
        T::Num(f64::NAN),
        T::Num(f64::INFINITY),
        T::Num(f64::MIN_POSITIVE),
    ] {
        check(v);
    }
    // A float64 that is integral decodes to the int32 form, like JSON.parse.
    let mut b = vec![b'D', b'J', b'W', 1, tag::FLOAT64];
    b.extend_from_slice(&7.0f64.to_le_bytes());
    let (_, v) = decode_ok(&b);
    assert_eq!(v.as_int(), Some(7));
}

#[test]
fn strings_choose_their_width() {
    for (text, two_byte) in [
        ("", false),
        ("ascii", false),
        ("caf\u{e9} \u{ff}", false),
        ("\u{100}", true),
        ("日本語", true),
        ("😀 smile", true),
        ("a\u{0}b", false),
    ] {
        let (h, v) = decode_ok(&encode(&s(text)));
        let st = v.as_string().unwrap();
        assert_eq!(h.str_to_utf16(st), text.encode_utf16().collect::<Vec<_>>());
        assert_eq!(h.str_is_two_byte(st), two_byte, "{text}");
    }
    // The UTF-16 form, with lone surrogates; narrowed when it can be.
    let (h, v) = decode_ok(&encode(&T::Str16(vec![0x61, 0xD800, 0x62])));
    assert_eq!(h.str_to_utf16(v.as_string().unwrap()), [0x61, 0xD800, 0x62]);
    let (h, v) = decode_ok(&encode(&T::Str16(vec![0x61, 0xE9])));
    assert!(!h.str_is_two_byte(v.as_string().unwrap()));
    // A padded byte length varint is accepted.
    let b = [
        b'D',
        b'J',
        b'W',
        1,
        tag::STR_UTF8,
        2,
        0x82,
        0x80,
        0x00,
        b'h',
        b'i',
    ];
    let (h, v) = decode_ok(&b);
    assert_eq!(h.str_to_string(v.as_string().unwrap()), "hi");
    check(s(&"x".repeat(100_000)));
    check(s(&"é".repeat(70_000)));
}

#[test]
fn containers() {
    check(T::Arr(vec![]));
    check(T::Obj(vec![]));
    check(T::Arr(vec![Some(T::Num(1.0)), None, Some(s("x")), None]));
    check(T::Obj(vec![
        ("a".into(), T::Num(1.0)),
        (
            "b".into(),
            T::Obj(vec![("a".into(), T::Arr(vec![Some(T::Null)]))]),
        ),
        ("".into(), T::Undef),
        ("10".into(), s("ten")),
        ("2".into(), s("two")),
        ("日本".into(), T::Bool(true)),
    ]));
    let rows: Vec<Option<T>> = (0..1000)
        .map(|i| {
            Some(T::Obj(vec![
                ("id".into(), T::Num(i as f64)),
                ("name".into(), s(&format!("row {i}"))),
                ("ok".into(), T::Bool(i % 2 == 0)),
            ]))
        })
        .collect();
    check(T::Arr(rows));
    check(T::Bytes(vec![0, 1, 2, 255]));
}

#[test]
fn depth_limit() {
    // Built and checked without recursion (the test helpers recurse).
    let nested = |n: u32| {
        let mut b = vec![b'D', b'J', b'W', 1];
        for _ in 0..n {
            b.extend_from_slice(&[tag::ARRAY, 1]);
        }
        b.extend_from_slice(&[tag::INT8, 7]);
        b
    };
    let (h, mut v) = decode_ok(&nested(DEFAULT_MAX_DEPTH));
    for _ in 0..DEFAULT_MAX_DEPTH {
        let o = v.as_object().unwrap();
        v = h
            .get_own_property(o, Atom::int(0).unwrap())
            .unwrap()
            .value()
            .unwrap();
    }
    assert_eq!(v.as_int(), Some(7));
    assert_eq!(
        invalid(&nested(DEFAULT_MAX_DEPTH + 1)).1,
        "nesting too deep"
    );
}

#[test]
fn keys_are_interned_once_and_repeated_keys_follow_js() {
    let b = encode(&T::Arr(vec![
        Some(T::Obj(vec![("k".into(), T::Num(1.0))])),
        Some(T::Obj(vec![("k".into(), T::Num(2.0))])),
    ]));
    // The second object refers to the key table: [.., OBJECT, 1, 1, INT8, 2].
    assert_eq!(&b[b.len() - 5..], &[tag::OBJECT, 1, 1, tag::INT8, 2]);
    check(T::Obj(vec![
        ("0".into(), T::Num(0.0)),
        ("x".into(), T::Num(1.0)),
    ]));
    // A repeated key: first position, last value.
    let mut e = Enc::new();
    e.out.extend_from_slice(&[tag::OBJECT, 3, 0]);
    e.string("a");
    e.out.extend_from_slice(&[tag::INT8, 1, 0]);
    e.string("b");
    e.out.extend_from_slice(&[tag::INT8, 2, 1, tag::INT8, 3]);
    let (mut h, v) = decode_ok(&e.out);
    assert_eq!(dump(&mut h, v), "{\"a\":n:3,\"b\":n:2}");
}

#[test]
fn protos_are_used() {
    let mut h = Heap::new().unwrap();
    let op = h.new_object(None).unwrap();
    let ap = h.new_object(None).unwrap();
    let opts = DecodeOptions {
        object_proto: Some(op),
        array_proto: Some(ap),
        ..DecodeOptions::default()
    };
    let b = encode(&T::Arr(vec![Some(T::Obj(vec![])), Some(T::Bytes(vec![1]))]));
    let v = decode(&mut h, &b, &opts).unwrap();
    let arr = v.as_object().unwrap();
    assert_eq!(h.get_prototype_of(arr), Some(ap));
    let el = |i| {
        h.get_own_property(arr, Atom::int(i).unwrap())
            .unwrap()
            .value()
            .unwrap()
    };
    assert_eq!(h.get_prototype_of(el(0).as_object().unwrap()), Some(op));
    // Bytes are a Uint8Array: without a realm (a bare heap) it has no prototype.
    let bytes = el(1).as_object().unwrap();
    assert!(h.is_typed_array(bytes));
    assert_eq!(h.get_prototype_of(bytes), None);
}

#[test]
fn invalid_input() {
    let hdr = [b'D', b'J', b'W', 1];
    let with = |rest: &[u8]| {
        let mut b = hdr.to_vec();
        b.extend_from_slice(rest);
        b
    };
    assert_eq!(invalid(b""), (0, "unexpected end"));
    assert_eq!(invalid(b"XJW\x01\x01"), (0, "bad magic"));
    assert_eq!(invalid(b"DJW\x02\x01"), (3, "unsupported version"));
    assert_eq!(invalid(&hdr), (4, "unexpected end"));
    assert_eq!(invalid(&with(&[0x01, 0x01])), (5, "trailing bytes"));
    assert_eq!(invalid(&with(&[0x99])), (4, "unknown tag"));
    assert_eq!(invalid(&with(&[tag::HOLE])), (4, "hole outside an array"));
    assert_eq!(
        invalid(&with(&[tag::HOST_REF, 1])),
        (4, "host refs are not supported")
    );
    assert_eq!(invalid(&with(&[tag::INT32, 1, 2])).1, "unexpected end");
    assert_eq!(
        invalid(&with(&[tag::ARRAY, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F])).1,
        "varint overflow"
    );
    assert_eq!(
        invalid(&with(&[tag::ARRAY, 0x80, 0x80, 0x80, 0x80, 0x80, 0])).1,
        "varint too long"
    );
    assert_eq!(
        invalid(&with(&[tag::ARRAY, 100, 1])).1,
        "count exceeds the input"
    );
    assert_eq!(
        invalid(&with(&[tag::OBJECT, 2, 0, tag::STR_UTF8])).1,
        "count exceeds the input"
    );
    assert_eq!(
        invalid(&with(&[tag::STR_UTF8, 1, 2, 0xC3, 0x28])).1,
        "invalid UTF-8"
    );
    assert_eq!(
        invalid(&with(&[tag::STR_UTF8, 2, 3, 0xED, 0xA0, 0x80])).1,
        "invalid UTF-8"
    );
    assert_eq!(
        invalid(&with(&[tag::STR_UTF8, 1, 2, b'a', b'b'])).1,
        "string lengths do not match"
    );
    assert_eq!(
        invalid(&with(&[tag::STR_UTF8, 3, 2, b'a', b'b'])).1,
        "string lengths do not match"
    );
    assert_eq!(
        invalid(&with(&[tag::STR_UTF8, 3, 4, 0xC3, 0xA9, 0xC3, 0xA9])).1,
        "string lengths do not match"
    );
    assert_eq!(
        invalid(&with(&[tag::OBJECT, 1, 1, tag::NULL])).1,
        "key index out of range"
    );
    assert_eq!(
        invalid(&with(&[tag::OBJECT, 1, 0, tag::NULL, tag::NULL])).1,
        "expected a key string"
    );
    assert_eq!(
        invalid(&with(&[tag::OBJECT, 1, 0, tag::STR_UTF8, 2, 1, b'a', 0])).1,
        "string lengths do not match"
    );
}

/// A deterministic xorshift generator.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn random_value(r: &mut Rng, depth: u32) -> T {
    let words = ["", "a", "key", "é", "日本", "😀", "x_y", "0", "17"];
    match r.below(if depth > 3 { 6 } else { 9 }) {
        0 => [T::Undef, T::Null, T::Bool(true), T::Bool(false)][r.below(4) as usize].clone(),
        1 => T::Num([0.0, -0.0, 1.5, 300.0, -5.0, 1e300, f64::NAN][r.below(7) as usize]),
        2 => T::Num(r.below(100_000) as f64 - 50_000.0),
        3 | 4 => s(&format!(
            "{}{}",
            words[r.below(words.len() as u64) as usize],
            words[r.below(words.len() as u64) as usize]
        )),
        5 => T::Str16(vec![0x61, 0xD800 + r.below(0x800) as u16]),
        6 => T::Arr(
            (0..r.below(6))
                .map(|_| (r.below(8) != 0).then(|| random_value(r, depth + 1)))
                .collect(),
        ),
        7 => T::Obj(
            (0..r.below(6))
                .map(|_| {
                    (
                        format!(
                            "{}{}",
                            words[r.below(words.len() as u64) as usize],
                            r.below(4)
                        ),
                        random_value(r, depth + 1),
                    )
                })
                .collect(),
        ),
        _ => T::Bytes((0..r.below(5)).map(|_| r.next() as u8).collect()),
    }
}

#[test]
fn random_values_round_trip() {
    let mut r = Rng(0x1234_5678_9abc_def1);
    for _ in 0..3000 {
        let v = random_value(&mut r, 0);
        let mut v = v;
        // Repeated keys are legal input but make the expectation differ; dedupe them.
        dedupe(&mut v);
        check(v);
    }
}

fn dedupe(v: &mut T) {
    match v {
        T::Arr(items) => items.iter_mut().flatten().for_each(dedupe),
        T::Obj(props) => {
            let mut seen: Vec<String> = Vec::new();
            props.retain(|(k, _)| {
                let new = !seen.contains(k);
                seen.push(k.clone());
                new
            });
            props.iter_mut().for_each(|(_, v)| dedupe(v));
        }
        _ => {}
    }
}

/// Random and corrupted buffers never panic, and whatever decodes leaves a valid heap.
#[test]
fn fuzz_never_panics() {
    fuzz(0xdead_beef_cafe_f00d, 12_000);
}

/// A longer run: `cargo test -p djs-core --release fuzz_long -- --ignored`.
#[test]
#[ignore]
fn fuzz_long() {
    for seed in 1..=20 {
        fuzz(seed * 0x9e37_79b9_7f4a_7c15, 100_000);
    }
}

fn fuzz(seed: u64, iterations: usize) {
    let mut r = Rng(seed);
    let mut valid = Vec::new();
    for i in 0..200 {
        let mut v = random_value(&mut r, 0);
        dedupe(&mut v);
        // Half of them as the Rust `Writer` writes them (uncounted UTF-8 strings, which
        // only builds with `embed` decode).
        valid.push(if i % 2 == 0 || !cfg!(feature = "embed") {
            encode(&v)
        } else {
            write(&v)
        });
    }
    let mut ok = 0;
    for i in 0..iterations {
        let mut b = valid[i % valid.len()].clone();
        match r.below(6) {
            0 => {
                // Random bytes after a valid header.
                b.truncate(4);
                for _ in 0..r.below(40) {
                    b.push(r.next() as u8);
                }
            }
            1 => {
                let n = r.below(b.len() as u64 + 1) as usize;
                b.truncate(n);
            }
            2 => {
                for _ in 0..1 + r.below(3) {
                    let at = r.below(b.len() as u64) as usize;
                    b[at] ^= 1 << r.below(8);
                }
            }
            3 => {
                let at = r.below(b.len() as u64) as usize;
                b[at] = r.next() as u8;
            }
            4 => {
                let at = r.below(b.len() as u64 + 1) as usize;
                b.insert(
                    at,
                    [0x00, 0x7F, 0x80, 0xFF, 0x20, 0x21, 0x10][r.below(7) as usize],
                );
            }
            _ => {
                let at = r.below(b.len() as u64) as usize;
                b.remove(at);
            }
        }
        let mut h = Heap::with_limit(1 << 24).unwrap();
        if decode(&mut h, &b, &DecodeOptions::default()).is_ok() {
            ok += 1;
            h.verify().unwrap();
        }
    }
    // Some mutations still decode (e.g. a flipped number byte).
    assert!(ok > iterations / 60, "{ok}");
}

#[test]
fn heap_limits_are_errors() {
    let mut h = Heap::with_limit(1 << 16).unwrap();
    let b = encode(&s(&"x".repeat(200_000)));
    assert!(matches!(
        decode(&mut h, &b, &DecodeOptions::default()),
        Err(WireError::Heap(_))
    ));
}

/// Decodes what js/test/gen-cross.ts wrote (into `DJS_WIRE_CROSS`) and compares with the
/// node-side dump, and with `json::parse_utf8` of the same value where JSON can carry it.
/// The gate runs it (scripts/gate.sh).
#[test]
#[ignore]
fn wire_cross() {
    use crate::json::{ParseOptions, parse_utf8};
    let dir = std::env::var("DJS_WIRE_CROSS").expect("set DJS_WIRE_CROSS");
    let cases = std::fs::read_to_string(format!("{dir}/cases.tsv")).unwrap();
    let (mut n, mut with_json) = (0, 0);
    for line in cases.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        let wire = std::fs::read(format!("{dir}/{}.wire", f[0])).unwrap();
        let (mut h, v) = decode_ok(&wire);
        let got = dump(&mut h, v);
        assert_eq!(got, f[1], "case {}", f[0]);
        if f[2] == "1" {
            let text = std::fs::read(format!("{dir}/{}.json", f[0])).unwrap();
            let mut h2 = Heap::new().unwrap();
            let j = parse_utf8(&mut h2, &text, &ParseOptions::default()).unwrap();
            assert_eq!(dump(&mut h2, j), got, "case {} via JSON", f[0]);
            with_json += 1;
        }
        n += 1;
    }
    assert!(n > 1000 && with_json > 300, "{n} {with_json}");
    std::println!("{n} cases match ({with_json} also via JSON)");
}

/// Decoder benchmark against `json::parse_utf8` on the datasets js/bench/wire.bench.ts
/// writes into `DJS_WIRE_BENCH`.
#[test]
#[ignore]
fn bench_wire() {
    use crate::json::{ParseOptions, parse_utf8};
    use std::time::Instant;
    let dir = std::env::var("DJS_WIRE_BENCH").expect("set DJS_WIRE_BENCH");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| {
            let n = e.unwrap().file_name().into_string().unwrap();
            n.strip_suffix(".wire").map(String::from)
        })
        .collect();
    names.sort();
    fn time(mut f: impl FnMut()) -> f64 {
        for _ in 0..2 {
            f();
        }
        let mut runs = Vec::new();
        let start = Instant::now();
        while runs.len() < 5 || (start.elapsed().as_secs_f64() < 1.0 && runs.len() < 200) {
            let t = Instant::now();
            f();
            runs.push(t.elapsed().as_secs_f64() * 1e3);
        }
        runs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        runs[runs.len() / 2]
    }
    std::println!("dataset              decode ms   parse_utf8 ms");
    for name in names {
        let wire = std::fs::read(format!("{dir}/{name}.wire")).unwrap();
        let json = std::fs::read(format!("{dir}/{name}.json")).unwrap();
        // Same result both ways.
        let mut h = Heap::with_limit(1 << 30).unwrap();
        let a = decode(&mut h, &wire, &DecodeOptions::default()).unwrap();
        let b = parse_utf8(&mut h, &json, &ParseOptions::default()).unwrap();
        assert_eq!(dump(&mut h, a).len(), dump(&mut h, b).len());
        let t_wire = time(|| {
            let mut h = Heap::with_limit(1 << 30).unwrap();
            core::hint::black_box(decode(&mut h, &wire, &DecodeOptions::default()).unwrap());
        });
        let t_json = time(|| {
            let mut h = Heap::with_limit(1 << 30).unwrap();
            core::hint::black_box(parse_utf8(&mut h, &json, &ParseOptions::default()).unwrap());
        });
        let t_heap = time(|| {
            core::hint::black_box(Heap::with_limit(1 << 30).unwrap());
        });
        std::println!(
            "{name:20} {:9.2} {:15.2}   (heap creation {t_heap:.3} ms included)",
            t_wire,
            t_json
        );
    }
}

/// Writes heap images for the outbound reader's cross-language test
/// (js/test/reader-cross.ts) into `DJS_READER_DIR`:
/// - `corpus.heap` + `corpus.tsv` (`name \t value bits`): every case of the wire corpus in
///   `DJS_WIRE_CROSS`, decoded into one heap;
/// - `extra.heap` + `extra.tsv` (`name \t bits \t dump or -`): ropes, big strings, symbols,
///   property kinds, sharing and cycles, built directly.
///
/// The gate runs it (scripts/gate.sh).
#[test]
#[ignore]
fn reader_images() {
    use crate::heap::{PropFlags, PropertyDescriptor};
    let wire_dir = std::env::var("DJS_WIRE_CROSS").expect("set DJS_WIRE_CROSS");
    let out = std::env::var("DJS_READER_DIR").expect("set DJS_READER_DIR");
    std::fs::create_dir_all(&out).unwrap();

    // The wire corpus, all in one heap (nothing collects, so every value stays valid).
    let cases = std::fs::read_to_string(format!("{wire_dir}/cases.tsv")).unwrap();
    let mut h = Heap::with_limit(1 << 30).unwrap();
    let mut tsv = String::new();
    for line in cases.lines() {
        let name = line.split('\t').next().unwrap();
        let wire = std::fs::read(format!("{wire_dir}/{name}.wire")).unwrap();
        let v = decode(&mut h, &wire, &DecodeOptions::default()).unwrap();
        tsv.push_str(&format!("{name}\t{:016x}\n", v.to_bits()));
    }
    h.verify().unwrap();
    std::fs::write(format!("{out}/corpus.heap"), h.bytes()).unwrap();
    std::fs::write(format!("{out}/corpus.tsv"), tsv).unwrap();

    // Hand-built values.
    let mut h = Heap::with_limit(1 << 30).unwrap();
    let mut extra: Vec<(String, Value, Option<String>)> = Vec::new();
    let mut add = |h: &mut Heap, name: &str, v: Value, dumped: bool| {
        let d = dumped.then(|| dump(h, v));
        extra.push((name.into(), v, d));
    };
    let atom = |h: &mut Heap, s: &str| h.atom_str(s).unwrap();

    // Ropes: plain, two-byte with a lone surrogate, deep, and flattened.
    let a = h.alloc_str(&"left side ".repeat(5)).unwrap();
    let b = h.alloc_str(&"right side ".repeat(5)).unwrap();
    let rope = h.concat(a, b).unwrap();
    assert_eq!(h.cell_kind(rope.offset()), crate::heap::CellKind::Rope);
    add(&mut h, "rope", Value::string(rope), true);
    let w = h.alloc_utf16(&[0x65E5, 0xD800, 0x61]).unwrap();
    let long = h.alloc_str(&"x".repeat(40)).unwrap();
    let r2 = h.concat(long, w).unwrap();
    let r2 = h.concat(r2, long).unwrap();
    add(&mut h, "rope-two-byte", Value::string(r2), true);
    let mut deep = h.alloc_str(&"0123456789".repeat(4)).unwrap();
    for i in 0..2000 {
        let piece = h.alloc_str(&format!("[{i:04}]")).unwrap();
        deep = if i % 2 == 0 {
            h.concat(deep, piece).unwrap()
        } else {
            h.concat(piece, deep).unwrap()
        };
    }
    add(&mut h, "rope-deep", Value::string(deep), true);
    let flat_src = h.concat(a, b).unwrap();
    h.flatten(flat_src).unwrap();
    add(&mut h, "rope-flattened", Value::string(flat_src), true);
    // Append ropes (D1610): `s += piece` chains share one buffer; an older, shorter value
    // stays a prefix, and a two-byte piece moves to a two-byte buffer.
    let mut s = h.alloc_str(&"line ".repeat(60)).unwrap();
    let mut older = s;
    for i in 0..300 {
        let piece = h.alloc_str(&format!("{i},")).unwrap();
        s = h.concat(s, piece).unwrap();
        if i == 150 {
            older = s;
        }
    }
    add(&mut h, "append-rope", Value::string(s), true);
    add(&mut h, "append-rope-prefix", Value::string(older), true);
    let s2 = h.concat(s, w).unwrap();
    let s2 = h.concat(s2, a).unwrap();
    add(&mut h, "append-rope-two-byte", Value::string(s2), true);

    // Big strings: ASCII, Latin-1 with and without the C1 range, two-byte, lone surrogate.
    let mib = 1 << 20;
    let s = h.alloc_str(&"a".repeat(mib)).unwrap();
    add(&mut h, "big-ascii", Value::string(s), false);
    let latin: Vec<u8> = (0..mib).map(|i| 0xA0 + (i % 96) as u8).collect();
    let s = h.alloc_latin1(&latin).unwrap();
    add(&mut h, "big-latin1", Value::string(s), false);
    let c1: Vec<u8> = (0..mib).map(|i| (i % 256) as u8).collect();
    let s = h.alloc_latin1(&c1).unwrap();
    add(&mut h, "big-latin1-c1", Value::string(s), false);
    let two: Vec<u16> = (0..mib).map(|i| 0x400 + (i % 0x400) as u16).collect();
    let s = h.alloc_utf16(&two).unwrap();
    add(&mut h, "big-two-byte", Value::string(s), false);
    let mut lone = two.clone();
    lone[mib / 2] = 0xDC00;
    let s = h.alloc_utf16(&lone).unwrap();
    add(&mut h, "big-lone-surrogate", Value::string(s), false);

    // A symbol, alone and as a property key and value.
    let desc = h.alloc_str("tag").unwrap();
    let sym = h.new_symbol(Value::string(desc)).unwrap();
    add(&mut h, "symbol", Value::symbol(sym), false);

    // BigInts (0046): zero, small, negative, multi-limb; the reader gives native bigints.
    for (name, v) in [
        ("bigint-zero", crate::bigint::BigInt::zero()),
        ("bigint-small", crate::bigint::BigInt::from_i64(42)),
        (
            "bigint-neg",
            crate::bigint::BigInt::from_i64(-0x1_2345_6789),
        ),
        (
            "bigint-big",
            crate::bigint::BigInt::from_parts(true, &[1, 2, 3, 0xFFFF_FFFF, 5]),
        ),
    ] {
        let b = h.alloc_bigint(&v).unwrap();
        add(&mut h, name, b, false);
    }

    // Property kinds: accessor, non-enumerable, symbol key, deleted, __proto__, index order.
    let o = h.new_object(None).unwrap();
    for (k, v) in [
        ("b", 1),
        ("10", 10),
        ("a", 2),
        ("2", 3),
        ("__proto__", 4),
        ("gone", 5),
    ] {
        let key = atom(&mut h, k);
        h.create_data_property(o, key, Value::int(v)).unwrap();
    }
    let gone = atom(&mut h, "gone");
    assert!(h.delete_own_property(o, gone).unwrap());
    let hidden = atom(&mut h, "hidden");
    let desc = PropertyDescriptor::data(Value::int(6), PropFlags::new(true, false, true));
    h.define_own_property(o, hidden, &desc).unwrap();
    let getter_obj = h.new_object(None).unwrap();
    let getter = atom(&mut h, "getter");
    let acc = PropertyDescriptor {
        get: Some(Value::object(getter_obj)),
        set: Some(Value::UNDEFINED),
        enumerable: Some(true),
        configurable: Some(true),
        ..PropertyDescriptor::default()
    };
    h.define_own_property(o, getter, &acc).unwrap();
    let sym_key = h.symbol_atom(sym);
    h.create_data_property(o, sym_key, Value::int(7)).unwrap();
    let with_sym = atom(&mut h, "sym");
    h.create_data_property(o, with_sym, Value::symbol(sym))
        .unwrap();
    add(&mut h, "props", Value::object(o), false);

    // An object of another class (an Error) inside plain data.
    let err = h
        .alloc_object(
            crate::heap::ObjectClass::Error,
            None,
            crate::heap::ObjectClass::Error.slots(),
        )
        .unwrap();
    let msg = h.alloc_str("boom").unwrap();
    let mk = atom(&mut h, "message");
    h.create_data_property(err, mk, Value::string(msg)).unwrap();
    let holder2 = h
        .new_array_from_values(None, &[Value::object(err)])
        .unwrap();
    add(&mut h, "error", Value::object(holder2), false);

    // Sharing and cycles.
    let shared = h.new_object(None).unwrap();
    let x = atom(&mut h, "x");
    h.create_data_property(shared, x, Value::int(1)).unwrap();
    let holder = h
        .new_array_from_values(None, &[Value::object(shared), Value::object(shared)])
        .unwrap();
    add(&mut h, "shared", Value::object(holder), false);
    let cyc = h.new_object(None).unwrap();
    let selfk = atom(&mut h, "self");
    h.create_data_property(cyc, selfk, Value::object(cyc))
        .unwrap();
    add(&mut h, "cycle", Value::object(cyc), false);
    let cyc2 = h.new_object(None).unwrap();
    let arr = h.new_array_from_values(None, &[Value::int(0)]).unwrap();
    let listk = atom(&mut h, "list");
    h.create_data_property(cyc2, listk, Value::object(arr))
        .unwrap();
    h.set_element_fast(arr, 1, Value::object(arr)).unwrap();
    add(&mut h, "cycle-array", Value::object(cyc2), false);
    // Sharing at every level: 2^40 paths through 40 small arrays.
    let mut level = h.new_array_from_values(None, &[Value::int(1)]).unwrap();
    for _ in 0..40 {
        let l = Value::object(level);
        level = h.new_array_from_values(None, &[l, l]).unwrap();
    }
    add(&mut h, "blowup", Value::object(level), false);

    // Arrays: holes, an element far beyond the dense part, a non-index property, length.
    let sparse = h
        .new_array_from_values(None, &[Value::int(1), Value::HOLE, Value::int(3)])
        .unwrap();
    let k500 = h.atom_from_index(500).unwrap();
    h.create_data_property(sparse, k500, Value::int(500))
        .unwrap();
    let extra_prop = atom(&mut h, "extra");
    h.create_data_property(sparse, extra_prop, Value::int(9))
        .unwrap();
    let len = atom(&mut h, "length");
    let d = PropertyDescriptor {
        value: Some(Value::int(1000)),
        ..PropertyDescriptor::default()
    };
    h.define_own_property(sparse, len, &d).unwrap();
    add(&mut h, "sparse", Value::object(sparse), false);

    // Typed arrays (0039): a Uint8Array, a Float64Array view at an offset into its
    // buffer, a detached one, and the buffer itself.
    let mut data = vec![0u8; 24];
    data[0] = 7;
    data[8..16].copy_from_slice(&1.5f64.to_le_bytes());
    data[16..24].copy_from_slice(&(-2.0f64).to_le_bytes());
    let u8a = h.new_uint8_array(&data, None, None).unwrap();
    add(&mut h, "uint8array", Value::object(u8a), false);
    let buffer = h.object_slot(u8a, 0);
    let f64a = h
        .alloc_object(crate::heap::ObjectClass::TypedArray, None, 4)
        .unwrap();
    for (i, v) in [buffer, Value::int(9), Value::int(8), Value::int(2)]
        .into_iter()
        .enumerate()
    {
        h.set_object_slot(f64a, i as u32, v);
    }
    add(&mut h, "float64array", Value::object(f64a), false);
    add(&mut h, "arraybuffer", buffer, false);
    let gone = h.new_uint8_array(&[1, 2, 3], None, None).unwrap();
    let gone_buffer = h.object_slot(gone, 0).as_object().unwrap();
    h.set_object_slot(gone_buffer, 0, Value::UNDEFINED);
    add(&mut h, "detached", Value::object(gone), false);

    // Numbers in both forms, and the other primitives.
    for (name, v) in [
        ("int", Value::int(-7)),
        ("float", Value::number(2.5)),
        ("negzero", Value::number(-0.0)),
        ("nan", Value::number(f64::NAN)),
        ("undefined", Value::UNDEFINED),
        ("null", Value::NULL),
        ("true", Value::TRUE),
    ] {
        add(&mut h, name, v, true);
    }

    h.verify().unwrap();
    let mut tsv = String::new();
    for (name, v, d) in &extra {
        tsv.push_str(&format!(
            "{name}\t{:016x}\t{}\n",
            v.to_bits(),
            d.as_deref().unwrap_or("-")
        ));
    }
    std::fs::write(format!("{out}/extra.heap"), h.bytes()).unwrap();
    std::fs::write(format!("{out}/extra.tsv"), tsv).unwrap();
}

/// Heap images for the reader benchmark (js/bench/reader.bench.ts): the datasets of
/// 0018's benchmark (`DJS_WIRE_BENCH`, written by js/bench/wire.bench.ts) plus Latin-1
/// strings, each decoded into a heap together with its `json::stringify_fast` result.
/// Writes `DJS_READER_BENCH/<name>.heap` and `index.tsv` (`name \t value bits \t JSON bits
/// \t stringify ms`).
#[test]
#[ignore]
fn reader_bench_images() {
    use crate::json::stringify_fast;
    use std::time::Instant;
    let src = std::env::var("DJS_WIRE_BENCH").expect("set DJS_WIRE_BENCH");
    let out = std::env::var("DJS_READER_BENCH").expect("set DJS_READER_BENCH");
    std::fs::create_dir_all(&out).unwrap();
    let mut index = String::new();
    let mut names: Vec<String> = std::fs::read_dir(&src)
        .unwrap()
        .filter_map(|e| {
            let n = e.unwrap().file_name().into_string().unwrap();
            n.strip_suffix(".wire").map(String::from)
        })
        .collect();
    names.sort();
    names.push("1mib-latin1".into());
    names.push("1mib-latin1-c1".into());
    for name in names {
        let mut h = Heap::with_limit(1 << 30).unwrap();
        let v = match name.as_str() {
            "1mib-latin1" => {
                let b: Vec<u8> = (0..1 << 20)
                    .map(|i| {
                        if i % 7 == 0 {
                            0xE9
                        } else {
                            b'a' + (i % 26) as u8
                        }
                    })
                    .collect();
                Value::string(h.alloc_latin1(&b).unwrap())
            }
            "1mib-latin1-c1" => {
                let b: Vec<u8> = (0..1 << 20)
                    .map(|i| {
                        if i % 7 == 0 {
                            0x85
                        } else {
                            b'a' + (i % 26) as u8
                        }
                    })
                    .collect();
                Value::string(h.alloc_latin1(&b).unwrap())
            }
            _ => {
                let wire = std::fs::read(format!("{src}/{name}.wire")).unwrap();
                decode(&mut h, &wire, &DecodeOptions::default()).unwrap()
            }
        };
        let json = stringify_fast(&mut h, v, &[]).unwrap().unwrap();
        // Time stringify_fast on a copy of the heap each round (it allocates its result).
        let image = h.bytes().to_vec();
        let mut runs = Vec::new();
        for _ in 0..20 {
            let mut hc = Heap::from_bytes(image.clone(), 1 << 30).unwrap();
            let t = Instant::now();
            core::hint::black_box(stringify_fast(&mut hc, v, &[]).unwrap());
            runs.push(t.elapsed().as_secs_f64() * 1e3);
        }
        runs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        std::fs::write(format!("{out}/{name}.heap"), h.bytes()).unwrap();
        index.push_str(&format!(
            "{name}\t{:016x}\t{:016x}\t{:.3}\n",
            v.to_bits(),
            Value::string(json).to_bits(),
            runs[runs.len() / 2]
        ));
    }
    std::fs::write(format!("{out}/index.tsv"), index).unwrap();
}

/// D1781: the one-pass validating `transcode` agrees with `str::from_utf8` plus UTF-16
/// counting on every input: the same verdict (valid, invalid UTF-8, wrong length) and the
/// same units, also for overlong forms, surrogates, truncated sequences and code points
/// above U+10FFFF, and for every length around the right one.
#[test]
fn transcode_matches_from_utf8() {
    let mut r = Rng(0x5eed_0f78);
    let pieces: [&[u8]; 16] = [
        b"a",
        b"hello wo",
        "é".as_bytes(),
        "€".as_bytes(),
        "😀".as_bytes(),
        "\u{FFFF}".as_bytes(),
        &[0xC0, 0x80],
        &[0xED, 0xA0, 0x80],
        &[0xE0, 0x9F, 0xBF],
        &[0xF0, 0x8F, 0xBF, 0xBF],
        &[0xF4, 0x90, 0x80, 0x80],
        &[0xF5, 0x80],
        &[0x80],
        &[0xE2, 0x82],
        &[0xFF],
        b"12345678",
    ];
    for _ in 0..200_000 {
        let mut src = Vec::new();
        for _ in 0..r.below(8) {
            if r.below(4) == 0 {
                src.push(r.next() as u8);
            } else {
                src.extend_from_slice(pieces[r.below(16) as usize]);
            }
        }
        let want_units: Option<Vec<u16>> = core::str::from_utf8(&src)
            .ok()
            .map(|t| t.encode_utf16().collect());
        let n = want_units.as_ref().map_or(src.len(), |u| u.len());
        for len in [n.saturating_sub(1), n, n + 1] {
            let mut dst = vec![0u16; len];
            let got = transcode(&src, &mut dst, |d, u| *d = u);
            let want = match &want_units {
                None => Transcoded::InvalidUtf8,
                Some(u) if u.len() != len => Transcoded::WrongLength,
                Some(_) => Transcoded::Ok,
            };
            assert_eq!(got, want, "{src:x?} into {len}");
            if got == Transcoded::Ok {
                assert_eq!(&dst, want_units.as_ref().unwrap(), "{src:x?}");
            }
        }
    }
}

/// `v` as the Rust host encoder (`Writer`) writes it.
fn write(v: &T) -> Vec<u8> {
    fn go(w: &mut Writer, v: &T) {
        match v {
            T::Undef => w.undefined(),
            T::Null => w.null(),
            T::Bool(b) => w.bool(*b),
            T::Num(n) => w.number(*n),
            T::Str(s) => w.str(s),
            T::Str16(u) => w.str_utf16(u),
            T::Arr(items) => {
                w.array(items.len());
                for it in items {
                    match it {
                        Some(v) => go(w, v),
                        None => w.hole(),
                    }
                }
            }
            T::Obj(props) => {
                w.object(props.len());
                for (k, v) in props {
                    w.key(k);
                    go(w, v);
                }
            }
            T::Bytes(b) => w.bytes(b),
        }
    }
    let mut w = Writer::new();
    go(&mut w, v);
    w.finish()
}

#[cfg(feature = "embed")]
#[test]
fn writer_round_trips() {
    let mut r = Rng(0x0bad_cafe_1234_5678);
    for _ in 0..3000 {
        let mut v = random_value(&mut r, 0);
        dedupe(&mut v);
        let (mut h, out) = decode_ok(&write(&v));
        assert_eq!(dump(&mut h, out), dump_t(&v), "{v:?}");
    }
    // Numbers take the smallest exact form; Latin-1 is written counted.
    let mut w = Writer::new();
    w.array(5);
    for n in [5.0, -129.0, 0.5, -0.0] {
        w.number(n);
    }
    w.str_latin1(b"caf\xe9");
    let b = w.finish();
    assert_eq!(&b[4..9], &[tag::ARRAY, 5, tag::INT8, 5, tag::INT32]);
    let (mut h, out) = decode_ok(&b);
    assert_eq!(
        dump(&mut h, out),
        r#"[n:5,n:-129,n:0.5,n:-0,s:"caf\u00e9"]"#
    );
    // Keys are sent once each, a UTF-16 key apart from its UTF-8 twin.
    let mut w = Writer::new();
    w.array(3);
    for _ in 0..2 {
        w.object(1);
        w.key("k");
        w.null();
    }
    w.object(1);
    w.key_utf16(&[0xD800]);
    w.null();
    let (mut h, out) = decode_ok(&w.finish());
    assert_eq!(
        dump(&mut h, out),
        r#"[{"k":null},{"k":null},{"\ud800":null}]"#
    );
}

#[cfg(feature = "embed")]
#[test]
fn uncounted_strings_are_validated() {
    let buf = |payload: &[u8]| {
        let mut b = vec![
            b'D',
            b'J',
            b'W',
            VERSION,
            tag::STR_UTF8_UNCOUNTED,
            payload.len() as u8,
        ];
        b.extend_from_slice(payload);
        b
    };
    for good in ["", "ascii", "é", "€ and 😀", "\u{FF}"] {
        let (mut h, out) = decode_ok(&buf(good.as_bytes()));
        assert_eq!(dump(&mut h, out), dump_t(&s(good)));
    }
    for bad in [
        &b"\xF0"[..],
        b"\xF0\x80",
        b"\xC3",
        b"\xED\xA0\x80",
        b"a\xFFb",
        b"\xE2\x82",
    ] {
        // (A length that cannot be right is reported as such, before the bytes are read.)
        let (_, reason) = invalid(&buf(bad));
        assert!(
            ["invalid UTF-8", "string lengths do not match"].contains(&reason),
            "{bad:?}: {reason}"
        );
    }
    // As a key too.
    let mut b = vec![
        b'D',
        b'J',
        b'W',
        VERSION,
        tag::OBJECT,
        1,
        0,
        tag::STR_UTF8_UNCOUNTED,
        1,
        0xC3,
    ];
    b.push(tag::NULL);
    assert_eq!(invalid(&b).1, "invalid UTF-8");
}

/// The outbound encoder gives back what the decoder made (plain data in a heap without a
/// realm: objects without a prototype count as plain).
#[cfg(feature = "embed")]
#[test]
fn outbound_round_trips() {
    let mut r = Rng(0x5eed_0f0b_b000_0001);
    for _ in 0..3000 {
        let mut v = random_value(&mut r, 0);
        dedupe(&mut v);
        let (mut h, out) = decode_ok(&encode(&v));
        let bytes = super::encode(&h, out, usize::MAX).unwrap_or_else(|e| panic!("{e}"));
        let (mut h2, back) = decode_ok(&bytes);
        assert_eq!(dump(&mut h2, back), dump(&mut h, out), "{v:?}");
    }
}

#[cfg(feature = "embed")]
#[test]
fn outbound_limits() {
    let v = T::Arr(vec![Some(s("x")); 100]);
    let (h, out) = decode_ok(&encode(&v));
    assert_eq!(
        super::encode(&h, out, 50),
        Err(EncodeError::TooLarge { limit: 50 })
    );
    assert!(super::encode(&h, out, 1000).is_ok());
}
