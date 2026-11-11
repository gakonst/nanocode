//! JSON unit tests. Expected outputs were checked against QuickJS.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::{format, vec};

use super::*;
use crate::heap::{Atom, Heap, PropFlags, PropertyDescriptor, atoms};
use crate::num;
use crate::value::{Kind, Str, Value};

/// A heap string with the units of `text`, forced into the given representation (a
/// two-byte cell even for ASCII text, to exercise the UTF-16 lexer).
fn input(h: &mut Heap, text: &str, two_byte: bool) -> Str {
    if !two_byte {
        return h.alloc_str(text).unwrap();
    }
    let units: Vec<u16> = text.encode_utf16().collect();
    let s = h.alloc_str_uninit(units.len() as u32, true).unwrap();
    let off = Heap::str_data_offset(s);
    let dst = h.arena_mut().slice_mut(off, units.len() as u32 * 2);
    for (d, u) in dst.as_chunks_mut::<2>().0.iter_mut().zip(&units) {
        *d = u.to_le_bytes();
    }
    s
}

/// Quotes code units the way JSON.stringify does (independent of the code under test).
pub(super) fn quote_units(units: &[u16]) -> String {
    let mut out = String::from("\"");
    let mut i = 0;
    while i < units.len() {
        let u = units[i];
        i += 1;
        match u {
            0x22 => out += "\\\"",
            0x5C => out += "\\\\",
            0x08 => out += "\\b",
            0x0C => out += "\\f",
            0x0A => out += "\\n",
            0x0D => out += "\\r",
            0x09 => out += "\\t",
            0..=0x1F => out += &format!("\\u{u:04x}"),
            0xD800..=0xDBFF if i < units.len() && (0xDC00..=0xDFFF).contains(&units[i]) => {
                let c = 0x10000 + (((u as u32) - 0xD800) << 10) + (units[i] as u32 - 0xDC00);
                out.push(char::from_u32(c).unwrap());
                i += 1;
            }
            0xD800..=0xDFFF => out += &format!("\\u{u:04x}"),
            _ => out.push(char::from_u32(u as u32).unwrap()),
        }
    }
    out.push('"');
    out
}

/// A canonical dump of a parsed value: `null`, `true`, `n:<String(x)>` (with `-0`),
/// `s:<quoted>`, `[a,b]`, `{"k":v}`. The corpus generator writes the same format.
pub(super) fn dump(h: &Heap, v: Value) -> String {
    match v.kind() {
        Kind::Null => "null".into(),
        Kind::Bool => format!("{}", v.as_bool().unwrap()),
        Kind::Number => {
            let n = v.as_number().unwrap();
            if n == 0.0 && n.is_sign_negative() {
                "n:-0".into()
            } else if n.is_nan() {
                "n:NaN".into()
            } else if n.is_infinite() {
                if n > 0.0 {
                    "n:Infinity".into()
                } else {
                    "n:-Infinity".into()
                }
            } else {
                format!("n:{}", num::to_string(n).as_str())
            }
        }
        Kind::String => format!("s:{}", quote_units(&h.str_to_utf16(v.as_string().unwrap()))),
        Kind::Object => {
            let o = v.as_object().unwrap();
            if h.object_class(o) == crate::heap::ObjectClass::Array {
                let items: Vec<String> = (0..h.array_length(o))
                    .map(|i| {
                        dump(
                            h,
                            h.get_own_property(o, Atom::int(i).unwrap())
                                .unwrap()
                                .value()
                                .unwrap(),
                        )
                    })
                    .collect();
                format!("[{}]", items.join(","))
            } else {
                let items: Vec<String> = h
                    .own_keys(o)
                    .into_iter()
                    .map(|k| {
                        let name: Vec<u16> = h.atom_name(k).encode_utf16().collect();
                        let v = h.get_own_property(o, k).unwrap().value().unwrap();
                        format!("{}:{}", quote_units(&name), dump(h, v))
                    })
                    .collect();
                format!("{{{}}}", items.join(","))
            }
        }
        k => format!("<{k:?}>"),
    }
}

/// Parses `text` from all three input encodings and checks they agree. Returns the dump or
/// `message @line:col`.
fn parse_all(text: &str) -> String {
    let mut results = Vec::new();
    for enc in 0..3 {
        let mut h = Heap::new().unwrap();
        let opts = ParseOptions::default();
        let r = match enc {
            0 => {
                let s = input(&mut h, text, false);
                parse(&mut h, s, &opts)
            }
            1 => {
                let s = input(&mut h, text, true);
                parse(&mut h, s, &opts)
            }
            _ => parse_utf8(&mut h, text.as_bytes(), &opts),
        };
        let out = match r {
            Ok(v) => {
                h.verify().unwrap();
                dump(&h, v)
            }
            Err(ParseError::Syntax(e)) => format!("{} @{}:{}", e.message, e.line, e.column),
            Err(ParseError::Heap(e)) => format!("heap {e:?}"),
        };
        results.push(out);
    }
    assert_eq!(results[0], results[1], "latin1 vs utf16 for {text:?}");
    assert_eq!(results[0], results[2], "latin1 vs utf8 for {text:?}");
    results.pop().unwrap()
}

#[test]
fn parse_values() {
    let cases = [
        ("null", "null"),
        (" true ", "true"),
        ("false", "false"),
        ("0", "n:0"),
        ("-0", "n:-0"),
        ("123", "n:123"),
        ("-1.5e-3", "n:-0.0015"),
        ("1E+2", "n:100"),
        ("1e400", "n:Infinity"),
        ("123456789012345678901234567890", "n:1.2345678901234568e+29"),
        ("9007199254740993", "n:9007199254740992"),
        ("\"\"", "s:\"\""),
        ("\"abc\"", "s:\"abc\""),
        (r#""a\"b\\c\/d\b\f\n\r\t""#, r#"s:"a\"b\\c/d\b\f\n\r\t""#),
        (r#""\u0041\u00e9\u20ac""#, "s:\"Aé€\""),
        (r#""\uD83D\uDE00""#, "s:\"😀\""),
        (r#""\uD800""#, r#"s:"\ud800""#),
        ("\"é€😀\"", "s:\"é€😀\""),
        ("[]", "[]"),
        ("[1,[2,[]],{}]", "[n:1,[n:2,[]],{}]"),
        (
            " { \"a\" : 1 , \"b\" : [ true ] } ",
            r#"{"a":n:1,"b":[true]}"#,
        ),
        (r#"{"a":1,"a":2}"#, r#"{"a":n:2}"#),
        (r#"{"b":1,"a":2,"b":3}"#, r#"{"b":n:3,"a":n:2}"#),
        (r#"{"1":1,"b":2,"0":0}"#, r#"{"0":n:0,"1":n:1,"b":n:2}"#),
        (r#"{"__proto__":1}"#, r#"{"__proto__":n:1}"#),
        (r#"{"":1}"#, r#"{"":n:1}"#),
        ("\r\n\t [1]", "[n:1]"),
    ];
    for (text, want) in cases {
        assert_eq!(parse_all(text), want, "{text:?}");
    }
}

#[test]
fn parse_errors() {
    let cases = [
        ("", "Unexpected end of JSON input @1:1"),
        ("{", "expecting property name @1:2"),
        ("[", "Unexpected end of JSON input @1:2"),
        ("[1,]", "unexpected token: ']' @1:4"),
        ("{\"a\":1,}", "expecting property name @1:8"),
        ("{a:1}", "expecting property name @1:2"),
        ("'x'", "unexpected token: ''' @1:1"),
        ("\"abc", "Unexpected end of JSON input @1:1"),
        ("\"\\x\"", "Bad escaped character @1:3"),
        ("\"\\u12\"", "Bad Unicode escape @1:6"),
        ("tru", "unexpected token: 'tru' @1:1"),
        ("NaN", "unexpected token: 'NaN' @1:1"),
        ("01", "Unexpected number @1:1"),
        ("1.", "Unterminated fractional number @1:3"),
        ("1e", "Exponent part is missing a number @1:3"),
        ("-", "Unexpected token ' @1:2"),
        ("--1", "Unexpected token '-' @1:2"),
        ("+1", "unexpected token: '+' @1:1"),
        ("[1 2]", "expecting ']' @1:4"),
        ("{\"a\" 1}", "expecting ':' @1:6"),
        ("{\"a\":}", "unexpected token: '}' @1:6"),
        ("1 2", "unexpected data at the end @1:3"),
        ("\"a\nb\"", "Bad control character in string literal @1:3"),
        ("\u{a0} 1", "unexpected character @1:1"),
        ("[1,\n  2,]", "unexpected token: ']' @2:5"),
        ("{\"é€\":\n tru}", "unexpected token: 'tru' @2:2"),
        ("[\"😀\", x]", "unexpected token: 'x' @1:7"),
        ("[1,\u{0}]", "unexpected token: '' @1:4"),
        ("\"\\", "Unexpected end of JSON input @1:1"),
        ("\"\\u", "Bad Unicode escape @1:4"),
        ("/*c*/1", "unexpected token: '/' @1:1"),
    ];
    for (text, want) in cases {
        assert_eq!(parse_all(text), want, "{text:?}");
    }
}

#[test]
fn parse_invalid_utf8() {
    let mut h = Heap::new().unwrap();
    let opts = ParseOptions::default();
    for bad in [
        &b"\"\xff\""[..],
        b"\"\xc3\"",
        b"\"\xed\xa0\x80\"",
        b"\"\xc0\xaf\"",
    ] {
        match parse_utf8(&mut h, bad, &opts) {
            Err(ParseError::Syntax(e)) => assert_eq!(e.message, "Bad UTF-8 sequence"),
            r => panic!("{bad:?}: {r:?}"),
        }
    }
    match parse_utf8(&mut h, b"\xff", &opts) {
        Err(ParseError::Syntax(e)) => assert_eq!(e.message, "unexpected character"),
        r => panic!("{r:?}"),
    }
}

#[test]
fn parse_depth_limit() {
    let mut h = Heap::new().unwrap();
    let opts = ParseOptions {
        max_depth: 100,
        ..Default::default()
    };
    let ok = "[".repeat(100) + &"]".repeat(100);
    let s = h.alloc_str(&ok).unwrap();
    assert!(parse(&mut h, s, &opts).is_ok());
    let deep = "[".repeat(101) + &"]".repeat(101);
    let s = h.alloc_str(&deep).unwrap();
    match parse(&mut h, s, &opts) {
        Err(ParseError::Syntax(e)) => {
            assert_eq!(e.message, "stack overflow");
            assert_eq!(e.column, 101);
        }
        r => panic!("{r:?}"),
    }
    // The default limit is far beyond any Rust recursion would allow, without recursing.
    let n = DEFAULT_MAX_DEPTH as usize;
    let text = "{\"a\":[".repeat(n / 2) + &"]}".repeat(n / 2);
    let s = h.alloc_str(&text).unwrap();
    let v = parse(&mut h, s, &ParseOptions::default()).unwrap();
    let out = stringify_fast(&mut h, v, &[]).unwrap().unwrap();
    assert!(h.str_eq_str(out, &text));
}

/// Strings with long plain runs between escapes (0129: after the first escape the scan
/// skips plain bytes 8 at a time, then one by one): every run length up to 19 around each
/// kind of escape and raw non-ASCII character, in all three input encodings.
#[test]
fn parse_long_runs_between_escapes() {
    let escapes = [
        ("\\n", "\n"),
        ("\\\"", "\""),
        ("\\\\", "\\"),
        ("\\u00e9", "é"),
        ("\\u20AC", "€"),
        ("é", "é"),
        ("€", "€"),
    ];
    for run in 0..20u8 {
        let plain: String = (0..run).map(|i| (b'a' + i % 26) as char).collect();
        for (esc, ch) in escapes {
            let text = format!("[\"{plain}{esc}{plain}{esc}{plain}\", \"{plain}\"]");
            let want = format!("{plain}{ch}{plain}{ch}{plain}");
            let want: Vec<u16> = want.encode_utf16().collect();
            let plain16: Vec<u16> = plain.encode_utf16().collect();
            assert_eq!(
                parse_all(&text),
                format!("[s:{},s:{}]", quote_units(&want), quote_units(&plain16)),
                "{text:?}"
            );
        }
    }
}

#[test]
fn parse_strings_representation() {
    let mut h = Heap::new().unwrap();
    let opts = ParseOptions::default();
    let s = h
        .alloc_str(r#"["abc", "é", "\u20ac", "x\u00ffy", ""]"#)
        .unwrap();
    let v = parse(&mut h, s, &opts).unwrap().as_object().unwrap();
    let kinds: Vec<_> = (0..5)
        .map(|i| {
            let s = h.get_element_fast(v, i).unwrap().as_string().unwrap();
            h.str_is_two_byte(s)
        })
        .collect();
    assert_eq!(kinds, [false, false, true, false, false]);
    // A two-byte input whose strings fit one byte gives one-byte strings.
    let t = input(&mut h, "[\"plain\", \"€\"]", true);
    let v = parse(&mut h, t, &opts).unwrap().as_object().unwrap();
    let a = h.get_element_fast(v, 0).unwrap().as_string().unwrap();
    assert!(!h.str_is_two_byte(a));
    assert!(h.str_eq_str(a, "plain"));
    h.verify().unwrap();
}

#[test]
fn parse_uses_prototypes_and_creates_plain_data() {
    let mut h = Heap::new().unwrap();
    let op = h.new_object(None).unwrap();
    let ap = h.new_object(None).unwrap();
    let opts = ParseOptions {
        object_proto: Some(op),
        array_proto: Some(ap),
        ..Default::default()
    };
    let s = h.alloc_str(r#"{"list":[1,2,3],"o":{}}"#).unwrap();
    let v = parse(&mut h, s, &opts).unwrap().as_object().unwrap();
    assert_eq!(h.get_prototype_of(v), Some(op));
    let list = h.atom_str("list").unwrap();
    let l = h.get_own_property(v, list).unwrap();
    assert_eq!(l.flags(), PropFlags::ALL);
    let l = l.value().unwrap().as_object().unwrap();
    assert_eq!(h.get_prototype_of(l), Some(ap));
    assert_eq!(h.array_length(l), 3);
    assert_eq!(h.dense_len(l), 3);
    assert_eq!(h.get_element_fast(l, 2), Some(Value::int(3)));
}

#[test]
fn many_keys_and_repeated_keys() {
    let mut h = Heap::new().unwrap();
    // Wide objects (hashed property tables) and many objects sharing keys (the key cache).
    let wide: Vec<String> = (0..100).map(|i| format!("\"k{i}\":{i}")).collect();
    let rows: Vec<String> = (0..200)
        .map(|i| format!("{{\"id\":{i},\"name\":\"n{i}\",\"k\\u0031\":{i}}}"))
        .collect();
    let text = format!("[{{{}}},{}]", wide.join(","), rows.join(","));
    let s = h.alloc_str(&text).unwrap();
    let v = parse(&mut h, s, &ParseOptions::default()).unwrap();
    let out = stringify_fast(&mut h, v, &[]).unwrap().unwrap();
    let expect = text.replace("\\u0031", "1");
    assert!(h.str_eq_str(out, &expect));
    // Keys are atoms: the same atom everywhere.
    let k99 = h.atom_str("k99").unwrap();
    let a = v.as_object().unwrap();
    let w = h.get_element_fast(a, 0).unwrap().as_object().unwrap();
    assert_eq!(
        h.get_own_property(w, k99).unwrap().value(),
        Some(Value::int(99))
    );
}

// ------------------------------------------------------------------ stringify

fn sfy(h: &mut Heap, v: Value, gap: &str) -> Result<Option<String>, StringifyError> {
    let gap: Vec<u16> = gap.encode_utf16().collect();
    stringify_fast(h, v, &gap).map(|o| o.map(|s| h.str_to_string(s)))
}

fn json(h: &mut Heap, text: &str) -> Value {
    let s = h.alloc_str(text).unwrap();
    parse(h, s, &ParseOptions::default()).unwrap()
}

#[test]
fn stringify_primitives() {
    let mut h = Heap::new().unwrap();
    let cases = [
        (Value::NULL, Some("null")),
        (Value::TRUE, Some("true")),
        (Value::FALSE, Some("false")),
        (Value::int(-42), Some("-42")),
        (Value::number(-0.0), Some("0")),
        (Value::number(1e21), Some("1e+21")),
        (Value::number(1.5e-7), Some("1.5e-7")),
        (Value::number(f64::NAN), Some("null")),
        (Value::number(f64::INFINITY), Some("null")),
        (Value::UNDEFINED, None),
    ];
    for (v, want) in cases {
        assert_eq!(sfy(&mut h, v, "").unwrap().as_deref(), want, "{v:?}");
    }
    let sym = h.new_symbol(Value::UNDEFINED).unwrap();
    assert_eq!(sfy(&mut h, Value::symbol(sym), "").unwrap(), None);
}

#[test]
fn stringify_escapes() {
    let mut h = Heap::new().unwrap();
    // "\u2028\u2029\ud800\udc00\udc00\ud800 \x7f\x00\x1f\b\f\n\r\t\"\\/"
    let units: Vec<u16> = vec![
        0x2028, 0x2029, 0xD800, 0xDC00, 0xDC00, 0xD800, 0x20, 0x7F, 0, 0x1F, 8, 12, 10, 13, 9,
        0x22, 0x5C, 0x2F,
    ];
    let s = h.alloc_utf16(&units).unwrap();
    let out = stringify_fast(&mut h, Value::string(s), &[])
        .unwrap()
        .unwrap();
    assert_eq!(
        h.str_to_string(out),
        "\"\u{2028}\u{2029}\u{10000}\\udc00\\ud800 \u{7f}\\u0000\\u001f\\b\\f\\n\\r\\t\\\"\\\\/\""
    );
    let l = h.alloc_str("plain é \"q\" \u{1}").unwrap();
    assert_eq!(
        sfy(&mut h, Value::string(l), "").unwrap().unwrap(),
        "\"plain é \\\"q\\\" \\u0001\""
    );
    // Ropes are fine.
    let a = h.alloc_str(&"a".repeat(30)).unwrap();
    let b = h.alloc_str(&"\n".repeat(30)).unwrap();
    let r = h.concat(a, b).unwrap();
    assert_eq!(
        sfy(&mut h, Value::string(r), "").unwrap().unwrap(),
        format!("\"{}{}\"", "a".repeat(30), "\\n".repeat(30))
    );
}

/// Two-byte strings are quoted in runs; compare with the independent quoter on random
/// mixes of escapes, surrogates (paired, lone, at either end) and wide text, after narrow
/// output (so the buffer switches mid-way).
#[test]
fn stringify_two_byte_runs() {
    const ALPHABET: [u16; 14] = [
        0x41, 0x22, 0x5C, 0x0A, 0x1F, 0xE9, 0x2028, 0x4E2D, 0xD800, 0xDBFF, 0xDC00, 0xDFFF, 0xD83D,
        0xDE00,
    ];
    let mut h = Heap::new().unwrap();
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    for _ in 0..3000 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let n = 1 + (x % 12) as usize;
        let mut units: Vec<u16> = (0..n)
            .map(|i| ALPHABET[((x >> (4 + 4 * i)) % ALPHABET.len() as u64) as usize])
            .collect();
        // The width invariant: a two-byte string has a unit above 0xFF.
        if units.iter().all(|&u| u <= 0xFF) {
            units.push(0x4E2D);
        }
        let s = h.alloc_utf16(&units).unwrap();
        let narrow = h.alloc_str("narrow \"first\"").unwrap();
        let arr = h
            .new_array_from_values(None, &[Value::string(narrow), Value::string(s)])
            .unwrap();
        let got = sfy(&mut h, Value::object(arr), "").unwrap().unwrap();
        let want = format!(
            "[{},{}]",
            quote_units(&"narrow \"first\"".encode_utf16().collect::<Vec<_>>()),
            quote_units(&units)
        );
        assert_eq!(got, want, "{units:x?}");
    }
}

/// Output past the spill size is moved into the heap at check points (D2553): compare
/// large results with the independent quoter, for narrow output, output that widens after
/// spilling (two-byte pieces), long strings, and ropes, whose flattening allocates between
/// spills (the result is then a rope).
#[test]
fn stringify_large_output_spills_into_the_heap() {
    let mut h = Heap::new().unwrap();
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let (mut in_place, mut ropes) = (0, 0);
    for round in 0..40 {
        let mut want = String::from("[");
        let mut values = Vec::new();
        let mut plain = true;
        for i in 0..(5 + rng.below(200)) {
            let n = match rng.below(8) {
                0 => SPILL_AT - 1 + rng.below(3) as usize,
                1 => 3 * SPILL_AT + rng.below(50) as usize,
                _ => rng.below(60) as usize,
            };
            let wide = round % 4 == 1 && rng.below(10) == 0;
            let mut units: Vec<u16> = (0..n)
                .map(|j| match rng.below(40) {
                    0 => 0x0A,
                    1 => 0x22,
                    2 if wide => 0x4E2D,
                    _ => 0x61 + (j % 26) as u16,
                })
                .collect();
            if wide {
                // A pair right at the first split, and the width invariant.
                if n > SPILL_AT {
                    units[SPILL_AT - 1] = 0xD83D;
                    units[SPILL_AT] = 0xDE00;
                }
                units.push(0x4E2D);
            }
            let s = if round % 4 == 2 && n >= 2 && rng.below(4) == 0 {
                plain = false;
                let (a, b) = units.split_at(n / 2);
                let (a, b) = (h.alloc_utf16(a).unwrap(), h.alloc_utf16(b).unwrap());
                h.concat(a, b).unwrap()
            } else {
                h.alloc_utf16(&units).unwrap()
            };
            if i > 0 {
                want.push(',');
            }
            want += &quote_units(&units);
            values.push(Value::string(s));
        }
        want.push(']');
        let arr = h.new_array_from_values(None, &values).unwrap();
        let out = stringify_fast(&mut h, Value::object(arr), &[])
            .unwrap()
            .unwrap();
        assert_eq!(h.str_to_string(out), want, "round {round}");
        if plain && round % 4 == 0 && want.len() > 2 * SPILL_AT {
            // Built in place: one flat cell, the arena's last.
            assert_eq!(h.cell_kind(out.0), crate::heap::CellKind::Str8);
            assert_eq!(out.0 + h.cell_size(out.0), h.arena().len());
            in_place += 1;
        }
        if h.cell_kind(out.0) == crate::heap::CellKind::Rope {
            ropes += 1;
        }
    }
    assert!(in_place > 3 && ropes > 3, "{in_place} {ropes}");
}

#[test]
fn stringify_gap() {
    let mut h = Heap::new().unwrap();
    let v = json(&mut h, r#"{"a":[1,{"b":2}],"c":"x"}"#);
    assert_eq!(
        sfy(&mut h, v, "  ").unwrap().unwrap(),
        "{\n  \"a\": [\n    1,\n    {\n      \"b\": 2\n    }\n  ],\n  \"c\": \"x\"\n}"
    );
    let v = json(&mut h, r#"{"a":[],"b":{}}"#);
    assert_eq!(
        sfy(&mut h, v, "  ").unwrap().unwrap(),
        "{\n  \"a\": [],\n  \"b\": {}\n}"
    );
    let v = json(&mut h, r#"{"1":1,"b":2,"0":0}"#);
    assert_eq!(
        sfy(&mut h, v, "--").unwrap().unwrap(),
        "{\n--\"0\": 0,\n--\"1\": 1,\n--\"b\": 2\n}"
    );
    // A two-byte gap makes the output two-byte.
    let v = json(&mut h, "[1]");
    assert_eq!(sfy(&mut h, v, "€").unwrap().unwrap(), "[\n€1\n]");
    assert_eq!(gap_from_number(20.0).len(), 10);
    assert_eq!(gap_from_number(3.7).len(), 3);
    assert_eq!(gap_from_number(-1.0).len(), 0);
    assert_eq!(gap_from_number(f64::NAN).len(), 0);
    let long: Vec<u16> = "abcdefghijklmno".encode_utf16().collect();
    assert_eq!(gap_from_units(&long).len(), 10);
}

#[test]
fn stringify_skips_and_holes() {
    let mut h = Heap::new().unwrap();
    let o = h.new_object(None).unwrap();
    let sym = h.new_symbol(Value::UNDEFINED).unwrap();
    let keys = ["a", "b", "c", "d"].map(|k| h.atom_str(k).unwrap());
    h.create_data_property(o, keys[0], Value::UNDEFINED)
        .unwrap();
    h.create_data_property(o, keys[1], Value::symbol(sym))
        .unwrap();
    h.create_data_property(o, keys[2], Value::int(1)).unwrap();
    // Non-enumerable and symbol-keyed properties are skipped.
    h.define_own_property(
        o,
        keys[3],
        &PropertyDescriptor::data(Value::int(2), PropFlags::WRITABLE),
    )
    .unwrap();
    h.create_data_property(o, h.symbol_atom(sym), Value::int(3))
        .unwrap();
    assert_eq!(
        sfy(&mut h, Value::object(o), "").unwrap().unwrap(),
        r#"{"c":1}"#
    );
    assert_eq!(
        sfy(&mut h, Value::object(o), " ").unwrap().unwrap(),
        "{\n \"c\": 1\n}"
    );

    // [undefined, symbol, , 3] with a hole and a sparse tail.
    let a = h.new_array(None, 0).unwrap();
    h.set_element_fast(a, 0, Value::UNDEFINED).unwrap();
    h.set_element_fast(a, 1, Value::symbol(sym)).unwrap();
    h.create_data_property(a, Atom::int(3).unwrap(), Value::int(3))
        .unwrap();
    h.create_data_property(a, Atom::int(200).unwrap(), Value::TRUE)
        .unwrap();
    let out = sfy(&mut h, Value::object(a), "").unwrap().unwrap();
    assert_eq!(
        out,
        format!("[null,null,null,3,{}true]", "null,".repeat(196))
    );
    // All-skipped object and an empty array with gap.
    let e = h.new_object(None).unwrap();
    h.create_data_property(e, keys[0], Value::UNDEFINED)
        .unwrap();
    assert_eq!(sfy(&mut h, Value::object(e), "  ").unwrap().unwrap(), "{}");
    let ea = h.new_array(None, 0).unwrap();
    assert_eq!(sfy(&mut h, Value::object(ea), "  ").unwrap().unwrap(), "[]");
    // A hole is looked up on the prototype chain.
    let proto = h.new_object(None).unwrap();
    h.create_data_property(proto, Atom::int(1).unwrap(), Value::int(7))
        .unwrap();
    let b = h.new_array(Some(proto), 0).unwrap();
    for i in [0, 2] {
        h.create_data_property(b, Atom::int(i).unwrap(), Value::int(i as i32))
            .unwrap();
    }
    assert_eq!(
        sfy(&mut h, Value::object(b), "").unwrap().unwrap(),
        "[0,7,2]"
    );
}

#[test]
fn stringify_bails_out_to_the_slow_path() {
    let mut h = Heap::new().unwrap();
    let getter = h.new_object(None).unwrap();
    // An enumerable accessor.
    let o = h.new_object(None).unwrap();
    let k = h.atom_str("k").unwrap();
    h.define_own_property(
        o,
        k,
        &PropertyDescriptor::accessor(Value::object(getter), Value::UNDEFINED, PropFlags::ALL),
    )
    .unwrap();
    let wrap = json(&mut h, "[1,{}]");
    let w = wrap.as_object().unwrap();
    h.set_element_fast(w, 2, Value::object(o)).unwrap();
    assert_eq!(sfy(&mut h, wrap, ""), Err(StringifyError::Slow));
    // A non-enumerable accessor is never read.
    let o2 = h.new_object(None).unwrap();
    h.define_own_property(
        o2,
        k,
        &PropertyDescriptor::accessor(Value::object(getter), Value::UNDEFINED, PropFlags::NONE),
    )
    .unwrap();
    assert_eq!(sfy(&mut h, Value::object(o2), "").unwrap().unwrap(), "{}");
    // toJSON on the prototype chain (an object might be callable).
    let proto = h.new_object(None).unwrap();
    let mid = h.new_object(Some(proto)).unwrap();
    let leaf = h.new_object(Some(mid)).unwrap();
    assert_eq!(sfy(&mut h, Value::object(leaf), "").unwrap().unwrap(), "{}");
    h.create_data_property(proto, atoms::TO_JSON, Value::object(getter))
        .unwrap();
    assert_eq!(
        sfy(&mut h, Value::object(leaf), ""),
        Err(StringifyError::Slow)
    );
    // A toJSON that is not an object is not callable: ignored (and serialized as a key).
    h.create_data_property(mid, atoms::TO_JSON, Value::int(1))
        .unwrap();
    assert_eq!(sfy(&mut h, Value::object(leaf), "").unwrap().unwrap(), "{}");
    assert_eq!(
        sfy(&mut h, Value::object(mid), "").unwrap().unwrap(),
        r#"{"toJSON":1}"#
    );
    // An accessor element reached through a hole.
    let ap = h.new_object(None).unwrap();
    h.define_own_property(
        ap,
        Atom::int(0).unwrap(),
        &PropertyDescriptor::accessor(Value::object(getter), Value::UNDEFINED, PropFlags::ALL),
    )
    .unwrap();
    let a = h.new_array(Some(ap), 0).unwrap();
    h.create_data_property(a, Atom::int(1).unwrap(), Value::NULL)
        .unwrap();
    assert_eq!(sfy(&mut h, Value::object(a), ""), Err(StringifyError::Slow));
}

#[test]
fn stringify_cycles() {
    let mut h = Heap::new().unwrap();
    let o = h.new_object(None).unwrap();
    let k = h.atom_str("self").unwrap();
    h.create_data_property(o, k, Value::object(o)).unwrap();
    assert_eq!(
        sfy(&mut h, Value::object(o), ""),
        Err(StringifyError::Circular)
    );
    assert_eq!(
        StringifyError::Circular.js_error(),
        Some(("TypeError", "circular reference"))
    );
    // Shared, non-cyclic references are fine.
    let shared = json(&mut h, "[1]");
    let a = h.new_array(None, 0).unwrap();
    h.set_element_fast(a, 0, shared).unwrap();
    h.set_element_fast(a, 1, shared).unwrap();
    assert_eq!(
        sfy(&mut h, Value::object(a), "").unwrap().unwrap(),
        "[[1],[1]]"
    );
    // A deep cycle (past the linear check).
    let text = "[".repeat(100) + &"]".repeat(100);
    let v = json(&mut h, &text);
    let mut inner = v.as_object().unwrap();
    for _ in 0..99 {
        inner = h.get_element_fast(inner, 0).unwrap().as_object().unwrap();
    }
    h.set_element_fast(inner, 0, v).unwrap();
    assert_eq!(sfy(&mut h, v, ""), Err(StringifyError::Circular));
    // Too deep.
    let n = DEFAULT_MAX_DEPTH as usize + 1;
    let mut deep = h.new_array(None, 0).unwrap();
    for _ in 1..n {
        let outer = h.new_array(None, 0).unwrap();
        h.set_element_fast(outer, 0, Value::object(deep)).unwrap();
        deep = outer;
    }
    assert_eq!(
        sfy(&mut h, Value::object(deep), ""),
        Err(StringifyError::StackOverflow)
    );
}

#[test]
fn stringify_respects_the_memory_cap() {
    let mut h = Heap::with_limit(1 << 20).unwrap();
    let a = h.new_array(None, 0).unwrap();
    // length 2^20 of holes: "null," * 2^20 is 5 MiB, more than the heap has room for.
    let len = PropertyDescriptor {
        value: Some(Value::int(1 << 20)),
        ..Default::default()
    };
    h.define_own_property(a, atoms::LENGTH, &len).unwrap();
    assert_eq!(
        sfy(&mut h, Value::object(a), ""),
        Err(StringifyError::Heap(crate::heap::HeapError::OutOfMemory))
    );
}

#[test]
fn stringify_checks_output_of_empty_containers() {
    let mut h = Heap::with_limit(4 << 20).unwrap();
    // 2000 levels of [[[...]]] with a 10-unit gap: about 20 MB of indentation from a tiny heap.
    let n = 2000;
    let text = "[".repeat(n) + &"]".repeat(n);
    let v = json(&mut h, &text);
    let gap = gap_from_number(10.0);
    assert_eq!(
        stringify_fast(&mut h, v, &gap),
        Err(StringifyError::Heap(crate::heap::HeapError::OutOfMemory))
    );
}

// ------------------------------------------------------------------ round trips and GC

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

/// Random JSON text plus the compact text JSON.stringify should give for it.
fn random_json(rng: &mut Rng, depth: u32, text: &mut String, compact: &mut String) {
    let ws = |rng: &mut Rng, t: &mut String| {
        if rng.below(4) == 0 {
            t.push_str([" ", "\n", "\t ", "\r\n"][rng.below(4) as usize]);
        }
    };
    ws(rng, text);
    let pick = if depth > 4 {
        rng.below(4)
    } else {
        rng.below(7)
    };
    match pick {
        0 => {
            let w = ["null", "true", "false"][rng.below(3) as usize];
            text.push_str(w);
            compact.push_str(w);
        }
        1 => {
            let n: f64 = match rng.below(4) {
                0 => rng.below(1000) as f64,
                1 => -(rng.below(1 << 40) as f64),
                2 => (rng.below(1 << 52) as f64) / 1024.0,
                _ => f64::from_bits(rng.next() >> 2),
            };
            let s = num::to_string(n);
            let s = s.as_str();
            // Sometimes write an equivalent non-canonical form.
            if rng.below(3) == 0 && !s.contains('e') && !s.contains('.') {
                text.push_str(s);
                text.push_str(".0e0");
            } else {
                text.push_str(s);
            }
            compact.push_str(s);
        }
        2 | 3 => {
            let mut units = Vec::new();
            for _ in 0..rng.below(12) {
                units.push(match rng.below(6) {
                    0 => rng.below(0x20) as u16,
                    1 => [0x22, 0x5C, 0x2F][rng.below(3) as usize],
                    2 => 0xE9,
                    3 => [0x20AC, 0xD83D, 0xDE00, 0xDC00][rng.below(4) as usize],
                    _ => b'a' as u16 + rng.below(26) as u16,
                });
            }
            // The input escapes everything outside printable ASCII; the expected output is
            // what QuoteJSONString makes of the units.
            text.push('"');
            for &u in &units {
                match u {
                    0x22 => text.push_str("\\\""),
                    0x5C => text.push_str("\\\\"),
                    0x20..=0x7E => text.push(u as u8 as char),
                    _ => text.push_str(&format!("\\u{u:04X}")),
                }
            }
            text.push('"');
            compact.push_str(&quote_units(&units));
        }
        4 | 5 => {
            text.push('[');
            compact.push('[');
            for i in 0..rng.below(5) {
                if i > 0 {
                    text.push(',');
                    compact.push(',');
                }
                random_json(rng, depth + 1, text, compact);
            }
            ws(rng, text);
            text.push(']');
            compact.push(']');
        }
        _ => {
            text.push('{');
            compact.push('{');
            // Distinct keys, so the expected output is just the input order.
            let n = rng.below(5);
            for i in 0..n {
                if i > 0 {
                    text.push(',');
                    compact.push(',');
                }
                ws(rng, text);
                let key = format!("k{i}_{}", rng.below(100));
                text.push_str(&format!("\"{key}\":"));
                compact.push_str(&format!("\"{key}\":"));
                random_json(rng, depth + 1, text, compact);
            }
            ws(rng, text);
            text.push('}');
            compact.push('}');
        }
    }
    ws(rng, text);
}

#[test]
fn random_round_trips() {
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    for i in 0..500 {
        let mut text = String::new();
        let mut compact = String::new();
        random_json(&mut rng, 0, &mut text, &mut compact);
        let mut h = Heap::new().unwrap();
        let v = json(&mut h, &text);
        let out = stringify_fast(&mut h, v, &[]).unwrap().unwrap();
        let got = h.str_to_utf16(out);
        assert_eq!(
            String::from_utf16_lossy(&got),
            compact,
            "case {i}: {text:?}"
        );
        // And back: parsing the output gives the same value again.
        let v2 = parse(&mut h, out, &ParseOptions::default()).unwrap();
        assert_eq!(dump(&h, v2), dump(&h, v), "case {i}");
        // Same through the UTF-8 entry point.
        let v3 = parse_utf8(&mut h, text.as_bytes(), &ParseOptions::default()).unwrap();
        assert_eq!(dump(&h, v3), dump(&h, v), "case {i}");
    }
}

#[test]
fn results_survive_collection() {
    let mut rng = Rng(42);
    let mut h = Heap::new().unwrap();
    let mut handles = Vec::new();
    let mut expect = Vec::new();
    for _ in 0..50 {
        let mut text = String::new();
        let mut compact = String::new();
        random_json(&mut rng, 0, &mut text, &mut compact);
        let v = json(&mut h, &text);
        // Garbage in between.
        for _ in 0..20 {
            h.alloc_str("garbage garbage garbage").unwrap();
        }
        handles.push(h.new_handle(v).unwrap());
        expect.push(compact);
    }
    h.collect().unwrap();
    h.verify().unwrap();
    for (hd, want) in handles.iter().zip(&expect) {
        let v = h.get_handle(*hd);
        let out = stringify_fast(&mut h, v, &[]).unwrap().unwrap();
        assert_eq!(String::from_utf16_lossy(&h.str_to_utf16(out)), *want);
    }
    // Stringify after parse, collect, parse again: stable.
    h.collect().unwrap();
    for (hd, want) in handles.iter().zip(&expect) {
        let v = h.get_handle(*hd);
        let out = stringify_fast(&mut h, v, &[]).unwrap().unwrap();
        let back = parse(&mut h, out, &ParseOptions::default()).unwrap();
        let out2 = stringify_fast(&mut h, back, &[]).unwrap().unwrap();
        assert!(h.str_eq_str(out2, &String::from_utf16_lossy(&h.str_to_utf16(out))));
        let _ = want;
    }
}

#[test]
fn parse_out_of_memory_is_clean() {
    let text = format!(
        "[{}]",
        vec!["{\"a\":\"some string value\",\"b\":[1,2,3]}"; 2000].join(",")
    );
    let mut h = Heap::with_limit(256 * 1024).unwrap();
    let r = parse_utf8(&mut h, text.as_bytes(), &ParseOptions::default());
    assert_eq!(
        r,
        Err(ParseError::Heap(crate::heap::HeapError::OutOfMemory))
    );
    h.verify().unwrap();
    h.collect().unwrap();
    h.verify().unwrap();
}

// ------------------------------------------------------------------ throughput

/// Rough throughput. Run with
/// `cargo test -p djs-core --release json_throughput -- --ignored --nocapture`.
#[test]
#[ignore]
fn json_throughput() {
    extern crate std;
    use std::time::Instant;
    let rows: Vec<String> = (0..10_000)
        .map(|i| {
            format!(
                "{{\"id\":{i},\"name\":\"item {i}\",\"path\":\"/home/user/project/src/file_{i}.ts\",\
                 \"size\":{},\"score\":{},\"tags\":[\"alpha\",\"beta\"],\"active\":{}}}",
                i * 37,
                i as f64 / 7.0,
                i % 2 == 0
            )
        })
        .collect();
    let records = format!("[{}]", rows.join(","));
    let big_string = format!(
        "\"{}\"",
        "lorem ipsum dolor sit amet, \\\"quoted\\\" \\n line ".repeat(1 << 20 >> 5)
    );
    for (name, text) in [("records", &records), ("1 MiB string", &big_string)] {
        let mb = text.len() as f64 / 1e6;
        let n: usize = std::env::var("DJS_JSON_BENCH_N")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(20);
        let mut h = Heap::new().unwrap();
        let s = h.alloc_str(text).unwrap();
        let hs = h.new_handle(Value::string(s)).unwrap();
        // Parse n times without collecting in between (allocation only bumps), then free.
        let t = Instant::now();
        for _ in 0..n {
            let s = h.get_handle(hs).as_string().unwrap();
            parse(&mut h, s, &ParseOptions::default()).unwrap();
        }
        let parse_s = t.elapsed().as_secs_f64() / n as f64;
        let heap_mb = h.arena().len() as f64 / n as f64 / 1e6;
        h.collect().unwrap();
        let t = Instant::now();
        for _ in 0..n {
            parse_utf8(&mut h, text.as_bytes(), &ParseOptions::default()).unwrap();
        }
        let utf8_s = t.elapsed().as_secs_f64() / n as f64;
        h.collect().unwrap();
        let s = h.get_handle(hs).as_string().unwrap();
        let v = parse(&mut h, s, &ParseOptions::default()).unwrap();
        let hv = h.new_handle(v).unwrap();
        h.collect().unwrap();
        let t = Instant::now();
        let mut out_len = 0;
        for _ in 0..n {
            let v = h.get_handle(hv);
            let out = stringify_fast(&mut h, v, &[]).unwrap().unwrap();
            out_len = h.str_len(out);
        }
        let sfy_s = t.elapsed().as_secs_f64() / n as f64;
        std::println!(
            "{name}: {mb:.2} MB -> {heap_mb:.1} MB heap; parse {:.0} MB/s (utf8 {:.0} MB/s), stringify {:.0} MB/s ({} units out)",
            mb / parse_s,
            mb / utf8_s,
            mb / sfy_s,
            out_len
        );
        let _ = Kind::Null;
    }
}

/// String tokens of many lengths, with escapes, quotes, controls and non-ASCII at random
/// places: the one-byte lexer (which copies the runs between escapes whole, 0116) agrees
/// with the UTF-16 and UTF-8 ones, errors and their positions included.
#[test]
fn string_tokens_agree_across_encodings() {
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let mut next = |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % n
    };
    let pieces = [
        "a", "bc", "xyz12345", "\\n", "\\\"", "\\\\", "\\u0041", "\\u00e9", "\\u20ac", "é", "€",
        "\\/", "\t", "\"", "\\x",
    ];
    for _ in 0..1500 {
        let mut text = String::from("[\"");
        for _ in 0..next(24) {
            // Mostly valid pieces; the last three are a raw control, an early end and a
            // bad escape.
            let range = if next(10) == 0 { 15 } else { 12 };
            let k = next(range) as usize;
            text.push_str(pieces[k]);
        }
        text.push_str("\",\"tail\"]");
        parse_all(&text);
    }
}
