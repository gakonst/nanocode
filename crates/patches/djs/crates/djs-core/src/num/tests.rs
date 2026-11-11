//! Behavior tests for the public conversion API: edge cases, and randomized comparisons
//! against Rust's std (shortest formatting, correctly rounded parsing) and against reference
//! implementations of the spec algorithms built on Rust's exact decimal expansion.
//! The qjs corpus comparison lives in `corpus_tests.rs`.

extern crate std;

use super::*;
use std::format;
use std::string::{String, ToString};
use std::vec::Vec;

fn s(x: f64) -> String {
    to_string(x).as_str().to_string()
}

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// The Latin-1 encoding of `s`, if it has one.
fn latin1(s: &str) -> Option<Vec<u8>> {
    s.chars().map(|c| u8::try_from(c as u32).ok()).collect()
}

/// Number(s) for both string representations, checking they agree.
fn num(s: &str) -> f64 {
    let b = string_to_number(&utf16(s)[..]);
    if let Some(l) = latin1(s) {
        let a = string_to_number(&l[..]);
        assert_eq!(a.to_bits(), b.to_bits(), "u8/u16 disagree on {s:?}");
    }
    b
}

fn same(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
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

    /// A random finite double: half uniformly random bits, half "interesting" values.
    fn f64(&mut self) -> f64 {
        loop {
            let x = match self.below(4) {
                0 | 1 => f64::from_bits(self.next()),
                2 => {
                    // short decimals like 1.25e-3
                    let m = self.below(100_000) as f64;
                    let e = self.below(40) as i32 - 20;
                    m * 10f64.powi(e)
                }
                _ => {
                    // values near powers of two and ten
                    let e = self.below(2046) as i64 - 1022;
                    let base = 2f64.powi(e as i32);
                    f64::from_bits(base.to_bits().wrapping_add(self.below(5)).wrapping_sub(2))
                }
            };
            if x.is_finite() {
                return if self.below(2) == 0 { x } else { -x };
            }
        }
    }
}

#[test]
fn to_string_edge_cases() {
    let cases: &[(f64, &str)] = &[
        (0.0, "0"),
        (-0.0, "0"),
        (f64::NAN, "NaN"),
        (f64::INFINITY, "Infinity"),
        (f64::NEG_INFINITY, "-Infinity"),
        (1.0, "1"),
        (-1.0, "-1"),
        (0.1, "0.1"),
        (0.1 + 0.2, "0.30000000000000004"),
        (1.0 / 3.0, "0.3333333333333333"),
        (100.0, "100"),
        (123456789.0, "123456789"),
        (9007199254740991.0, "9007199254740991"),
        (9007199254740992.0, "9007199254740992"),
        (9007199254740994.0, "9007199254740994"),
        (2f64.powi(60), "1152921504606847000"),
        (1e20, "100000000000000000000"),
        (123e18, "123000000000000000000"),
        (1e21, "1e+21"),
        (1.5e21, "1.5e+21"),
        (999999999999999900000.0, "999999999999999900000"),
        (1e-6, "0.000001"),
        (1.5e-6, "0.0000015"),
        (1e-7, "1e-7"),
        (1.5e-7, "1.5e-7"),
        (-1.2345678901234566e-7, "-1.2345678901234566e-7"),
        (-1.2345678901234567e-6, "-0.0000012345678901234567"),
        (f64::MAX, "1.7976931348623157e+308"),
        (-f64::MAX, "-1.7976931348623157e+308"),
        (f64::MIN_POSITIVE, "2.2250738585072014e-308"),
        (5e-324, "5e-324"),
        (-5e-324, "-5e-324"),
        (1e23, "1e+23"),
        (9.5e-5, "0.000095"),
        (4.35, "4.35"),
        (2f64.powi(-1022) * 3.0, "6.675221575521604e-308"),
        (1.7976931348623155e308, "1.7976931348623155e+308"),
    ];
    for &(x, expect) in cases {
        assert_eq!(s(x), expect, "{x:e}");
    }
}

/// Our output as (significant digits, decimal point position).
fn js_digits(out: &str) -> (String, i32) {
    let out = out.trim_start_matches('-');
    let (mant, exp) = match out.split_once('e') {
        Some((m, e)) => (m, e.parse::<i32>().unwrap()),
        None => (out, 0),
    };
    let (int, frac) = mant.split_once('.').unwrap_or((mant, ""));
    let all = format!("{int}{frac}");
    let lead = all.len() - all.trim_start_matches('0').len();
    let digits = all
        .trim_start_matches('0')
        .trim_end_matches('0')
        .to_string();
    (digits, int.len() as i32 - lead as i32 + exp)
}

#[test]
fn shortest_matches_std_random() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for _ in 0..300_000 {
        let x = rng.f64();
        if x == 0.0 {
            continue;
        }
        let ours = s(x);
        // Rust's `{:e}` is the shortest round-trip representation, closest to the value.
        let std_e = format!("{:e}", x);
        let (od, oe) = js_digits(&ours);
        let (sd, se) = js_digits(&std_e);
        if (&od, oe) != (&sd, se) {
            // Both are shortest; they may differ only when the exact value lies exactly
            // halfway between two candidates. The spec then requires the even one, std may
            // pick either.
            let (exact, ee) = exact_decimal(x);
            let p = od.len();
            assert!(
                od.len() == sd.len() && oe == ee && exact.len() == p + 1 && exact[p] == b'5',
                "{x:e}: ours {ours}, std {std_e}"
            );
            assert!(
                (od.as_bytes()[p - 1] - b'0').is_multiple_of(2),
                "{ours} is not even"
            );
        }
        assert!(same(num(&ours), x), "round trip of {ours}");
        // The JS layout rules.
        let (_, e) = js_digits(&ours);
        let exponential = ours.contains('e');
        assert_eq!(exponential, !(-5..=21).contains(&e), "{ours}");
        assert!(ours.len() <= MAX_SHORT_LEN);
    }
}

#[test]
fn parse_edge_cases() {
    let nan = f64::NAN;
    let inf = f64::INFINITY;
    let cases: &[(&str, f64)] = &[
        ("", 0.0),
        ("   ", 0.0),
        (
            "\t\n\u{b}\u{c}\r \u{a0}\u{feff}\u{2028}\u{2029}\u{3000}",
            0.0,
        ),
        (" 12 ", 12.0),
        ("\u{2000}1\u{200a}", 1.0),
        ("\u{85}1", nan),
        ("\u{180e}1", nan),
        ("\u{1c}1", nan),
        ("1\0", nan),
        ("+1", 1.0),
        ("-1", -1.0),
        ("--1", nan),
        ("+-1", nan),
        ("-0", -0.0),
        ("+0", 0.0),
        ("0", 0.0),
        ("00012", 12.0),
        ("08", 8.0),
        (".5", 0.5),
        ("5.", 5.0),
        ("-.5", -0.5),
        (".", nan),
        ("-.", nan),
        ("5.e1", 50.0),
        (".e1", nan),
        ("1e", nan),
        ("1e+", nan),
        ("1e-", nan),
        ("1E5", 1e5),
        ("1e+5", 1e5),
        ("1e-5", 1e-5),
        ("1_0", nan),
        ("1 2", nan),
        ("Infinity", inf),
        ("+Infinity", inf),
        ("-Infinity", -inf),
        (" -Infinity ", -inf),
        ("infinity", nan),
        ("INFINITY", nan),
        ("Infinityx", nan),
        ("Infinit", nan),
        ("inf", nan),
        ("NaN", nan),
        ("0x10", 16.0),
        ("0X1f", 31.0),
        ("0xFFFFFFFFFFFFFFFF", 18446744073709551615.0),
        ("0x", nan),
        ("0xg", nan),
        ("0x1.8", nan),
        ("0x1p3", nan),
        ("+0x10", nan),
        ("-0x10", nan),
        ("0b101", 5.0),
        ("0B11", 3.0),
        ("0b2", nan),
        ("0b", nan),
        ("0o17", 15.0),
        ("0O17", 15.0),
        ("0o8", nan),
        ("1e1000", inf),
        ("-1e1000", -inf),
        ("1e-1000", 0.0),
        ("-1e-1000", -0.0),
        ("0e99999999999999999999", 0.0),
        ("0.0e-99999999999999999999", 0.0),
        ("1e99999999999999999999", inf),
        ("1e-99999999999999999999", 0.0),
        ("1.7976931348623157e308", f64::MAX),
        ("1.7976931348623158e308", f64::MAX),
        ("1.7976931348623159e308", inf),
        ("2.4703282292062327e-324", 0.0),
        ("2.4703282292062328e-324", 5e-324),
        ("4.9406564584124654e-324", 5e-324),
        ("2.2250738585072011e-308", 2.225073858507201e-308),
        ("2.2250738585072012e-308", f64::MIN_POSITIVE),
        ("9007199254740993", 9007199254740992.0),
        ("9007199254740995", 9007199254740996.0),
        ("0.1", 0.1),
        ("0.30000000000000004", 0.30000000000000004),
        ("123456789012345678901234567890", 1.2345678901234568e29),
    ];
    for &(input, expect) in cases {
        let got = num(input);
        assert!(
            same(got, expect),
            "Number({input:?}) = {got:e}, expected {expect:e}"
        );
    }
}

#[test]
fn parse_long_inputs() {
    // A halfway case decided by a digit far beyond 19 (and beyond qjs' 38).
    let mut s1 = String::from("9007199254740993");
    s1.push('.');
    s1.push_str(&"0".repeat(1000));
    s1.push('1');
    assert_eq!(num(&s1), 9007199254740994.0);
    let s2 = format!("9007199254740993.{}", "0".repeat(1000));
    assert_eq!(num(&s2), 9007199254740992.0);
    // Leading zeros and huge exponents compensating each other.
    let s3 = format!("0.{}1e400", "0".repeat(399));
    assert_eq!(num(&s3), 1.0);
    let s4 = format!("{}e-400", "1".to_string() + &"0".repeat(400));
    assert_eq!(num(&s4), 1.0);
    // The exact smallest subnormal halfway point with many digits after it.
    let half = "2.4703282292062327208828439643411068618252990130716238221279284125033775363510437593264991818081799618989828234772285886546332835517796989819938739800539093906315035659515570226392290858392449105184435931802849936536152500319370457678249219365623669863658480757001585769269903706311928279558551332927834338409351978015531246597263579574622766465272827220056374006485499977096599470454020828166226237857393450736339007967761930577506740176324673600968951340535537458516661134223766678604162159680461914467291840300530057530849048765391711386591646239524912623653881879636239373280423891018672348497668235089863388587925628302755995657524455507255189313690836254779186948667994968324049705821028513185451396213837722826145437693412532098591327667236328125";
    assert_eq!(num(&format!("{half}e-324")), 0.0);
    assert_eq!(
        num(&format!("{half}00000000000000000000000000000000001e-324")),
        5e-324
    );
    assert_eq!(num(&format!("{half}0000e-324")), 0.0);
    // Just below the halfway point.
    let below = half.replace("328125", "328124");
    assert_eq!(num(&format!("{below}999999e-324")), 0.0);
}

#[test]
fn parse_matches_std_random() {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let mut buf = String::new();
    for _ in 0..300_000 {
        buf.clear();
        let n_int = rng.below(25) as usize;
        let n_frac = rng.below(25) as usize;
        for _ in 0..n_int {
            buf.push((b'0' + rng.below(10) as u8) as char);
        }
        if n_frac > 0 || rng.below(4) == 0 {
            buf.push('.');
        }
        for _ in 0..n_frac {
            buf.push((b'0' + rng.below(10) as u8) as char);
        }
        if n_int + n_frac == 0 {
            buf.push('7');
        }
        if rng.below(2) == 0 {
            buf.push('e');
            let e = rng.below(700) as i64 - 350;
            buf.push_str(&e.to_string());
        }
        let expect: f64 = buf.parse().unwrap();
        assert!(same(num(&buf), expect), "{buf}");
        assert!(
            same(parse_float(buf.as_bytes()), expect),
            "parseFloat {buf}"
        );
        assert!(same(literal_value(buf.as_bytes()), expect), "literal {buf}");
    }
    // Shortest and 17-digit representations of random doubles.
    for _ in 0..300_000 {
        let x = rng.f64().abs();
        for text in [format!("{x:e}"), format!("{x:.16e}"), format!("{x:.20e}")] {
            assert!(same(num(&text), x), "{text}");
        }
    }
}

#[test]
fn parse_float_prefixes() {
    let nan = f64::NAN;
    let cases: &[(&str, f64)] = &[
        ("", nan),
        ("  ", nan),
        ("\u{a0}7", 7.0),
        ("  1.5xyz", 1.5),
        ("1e", 1.0),
        ("1e+", 1.0),
        ("1e5x", 1e5),
        (".5.5", 0.5),
        ("5..", 5.0),
        ("-.5e-1", -0.05),
        ("-0", -0.0),
        ("0x10", 0.0),
        ("Infinityx", f64::INFINITY),
        ("-Infinity1", f64::NEG_INFINITY),
        ("+-1", nan),
        ("e5", nan),
        ("1_000", 1.0),
        ("\u{3000}42", 42.0),
        ("1\0", 1.0),
    ];
    for &(input, expect) in cases {
        let a = parse_float(&utf16(input)[..]);
        assert!(same(a, expect), "parseFloat({input:?}) = {a}");
        if let Some(l) = latin1(input) {
            assert!(same(a, parse_float(&l[..])));
        }
    }
}

#[test]
fn parse_int_cases() {
    let nan = f64::NAN;
    let cases: &[(&str, i32, f64)] = &[
        ("", 0, nan),
        ("12", 0, 12.0),
        ("  -12abc", 0, -12.0),
        ("0x10", 0, 16.0),
        ("0x10", 16, 16.0),
        ("0x10", 10, 0.0),
        ("-0x10", 0, -16.0),
        ("+0X1F", 0, 31.0),
        ("0x", 0, nan),
        ("0x", 16, nan),
        ("0b11", 0, 0.0),
        ("0o7", 0, 0.0),
        ("-0", 0, -0.0),
        (" -0", 10, -0.0),
        ("-", 10, nan),
        ("z", 36, 35.0),
        ("Z", 36, 35.0),
        ("10", 36, 36.0),
        ("12", 1, nan),
        ("12", 37, nan),
        ("12", -1, nan),
        ("1.9", 10, 1.0),
        ("1e3", 10, 1.0),
        ("123", 2, 1.0),
        ("777", 8, 511.0),
        ("ff", 16, 255.0),
        ("Infinity", 0, nan),
        ("9007199254740993", 10, 9007199254740992.0),
        (
            "10000000000000000000000000000000000000000000000000000001",
            2,
            3.602879701896397e16,
        ),
        ("1fffffffffffff1", 16, 144115188075855860.0),
    ];
    for &(input, radix, expect) in cases {
        let a = parse_int(input.as_bytes(), radix);
        let b = parse_int(&utf16(input)[..], radix);
        assert!(same(a, expect), "parseInt({input:?}, {radix}) = {a}");
        assert!(same(a, b));
    }
    assert_eq!(
        parse_int(b"900719925474099267123", 10),
        "900719925474099267123".parse::<f64>().unwrap()
    );
    // Long power-of-two inputs are exact; overflow gives Infinity.
    let ones = "1".repeat(54);
    assert_eq!(parse_int(ones.as_bytes(), 2), 2f64.powi(54));
    assert_eq!(parse_int("f".repeat(256).as_bytes(), 16), f64::INFINITY);
    assert_eq!(parse_int("f".repeat(255).as_bytes(), 16), 2f64.powi(1020));
    assert_eq!(parse_int("z".repeat(1000).as_bytes(), 36), f64::INFINITY);
    assert_eq!(parse_int("1".repeat(400).as_bytes(), 10), f64::INFINITY);
    assert_eq!(
        parse_int("1".repeat(100).as_bytes(), 3),
        2.5768876036600566e47
    );
}

#[test]
fn literals() {
    let cases: &[(&str, f64)] = &[
        ("0", 0.0),
        ("1_000", 1000.0),
        ("1_000.000_1", 1000.0001),
        ("1e1_0", 1e10),
        (".5", 0.5),
        ("5.", 5.0),
        ("0.0", 0.0),
        ("0x1F_FF", 8191.0),
        ("0XFF", 255.0),
        ("0o1_7", 15.0),
        ("0b1010_1010", 170.0),
        ("0x1fffffffffffff", 9007199254740991.0),
        ("0x20000000000001", 9007199254740992.0),
        ("0x20000000000003", 9007199254740996.0),
        ("0x20000000000002_1", 144115188075855900.0),
        ("1e400", f64::INFINITY),
        ("1e-400", 0.0),
        ("08", 8.0),
        ("x", f64::NAN),
    ];
    for &(input, expect) in cases {
        assert!(same(literal_value(input.as_bytes()), expect), "{input}");
    }
    // The lexer's split form.
    assert_eq!(literal_digits_value(16, b"1F_FF"), 8191.0);
    assert_eq!(literal_digits_value(8, b"17"), 15.0);
    assert_eq!(literal_digits_value(2, b"1_0"), 2.0);
    assert_eq!(literal_digits_value(10, b"1_000.5e-3"), 1.0005);
    assert_eq!(literal_digits_value(10, b".5"), 0.5);
    assert!(literal_digits_value(16, b"").is_nan());
    assert!(literal_digits_value(8, b"8").is_nan());
}

/// The exact decimal expansion of finite non-zero `|x|` as (digits, point position).
fn exact_decimal(x: f64) -> (Vec<u8>, i32) {
    let t = format!("{:.800e}", x.abs());
    let (m, e) = t.split_once('e').unwrap();
    let mut digits: Vec<u8> = m.bytes().filter(|b| b.is_ascii_digit()).collect();
    while digits.last() == Some(&b'0') {
        digits.pop();
    }
    (digits, e.parse::<i32>().unwrap() + 1)
}

/// Rounds `digits` (point position `e`) half up to `n` digits after the point position `pos`
/// (that is, keeps the digits with index < e + pos... ). Returns the integer as a digit string.
fn round_half_up_at(digits: &[u8], keep: i32) -> Vec<u8> {
    // Keep the first `keep` digits (keep may be <= 0 or exceed the length).
    let mut out: Vec<u8> = Vec::new();
    for i in 0..keep.max(0) {
        out.push(*digits.get(i as usize).unwrap_or(&b'0'));
    }
    let next = if keep >= 0 {
        *digits.get(keep as usize).unwrap_or(&b'0')
    } else {
        b'0'
    };
    if next >= b'5' {
        let mut i = out.len();
        loop {
            if i == 0 {
                out.insert(0, b'1');
                break;
            }
            i -= 1;
            if out[i] == b'9' {
                out[i] = b'0';
            } else {
                out[i] += 1;
                break;
            }
        }
    }
    let lead = out.iter().take_while(|&&d| d == b'0').count();
    let out = out[lead.min(out.len().saturating_sub(1))..].to_vec();
    if out.is_empty() { b"0".to_vec() } else { out }
}

/// The spec's Number.prototype.toFixed for finite |x| < 1e21.
fn ref_to_fixed(x: f64, f: usize) -> String {
    let mut s = String::new();
    let n = if x == 0.0 {
        b"0".to_vec()
    } else {
        if x < 0.0 {
            s.push('-');
        }
        let (digits, e) = exact_decimal(x);
        round_half_up_at(&digits, e + f as i32)
    };
    let mut m = String::from_utf8(n).unwrap();
    if f != 0 {
        let mut k = m.len();
        if k <= f {
            m = "0".repeat(f + 1 - k) + &m;
            k = f + 1;
        }
        m = format!("{}.{}", &m[..k - f], &m[k - f..]);
    }
    s + &m
}

/// The digits of finite non-zero `x` rounded half up to `p` significant digits, and the
/// spec's exponent `e` (x ~ d.ddd * 10^e).
fn ref_precision(x: f64, p: usize) -> (String, i32) {
    let (digits, e) = exact_decimal(x);
    let mut n = round_half_up_at(&digits, p as i32);
    let mut e = e - 1;
    if n.len() > p {
        n.truncate(p);
        e += 1;
    }
    (String::from_utf8(n).unwrap(), e)
}

fn ref_to_exponential(x: f64, f: usize) -> String {
    let (n, e) = if x == 0.0 {
        ("0".repeat(f + 1), 0)
    } else {
        ref_precision(x, f + 1)
    };
    let sign = if x < 0.0 { "-" } else { "" };
    let m = if f == 0 {
        n
    } else {
        format!("{}.{}", &n[..1], &n[1..])
    };
    format!("{sign}{m}e{}{}", if e < 0 { '-' } else { '+' }, e.abs())
}

fn ref_to_precision(x: f64, p: usize) -> String {
    let (n, e) = if x == 0.0 {
        ("0".repeat(p), 0)
    } else {
        ref_precision(x, p)
    };
    let sign = if x < 0.0 { "-" } else { "" };
    if e < -6 || e >= p as i32 {
        let m = if p == 1 {
            n
        } else {
            format!("{}.{}", &n[..1], &n[1..])
        };
        return format!("{sign}{m}e{}{}", if e < 0 { '-' } else { '+' }, e.abs());
    }
    if e == p as i32 - 1 {
        return format!("{sign}{n}");
    }
    if e >= 0 {
        let k = (e + 1) as usize;
        return format!("{sign}{}.{}", &n[..k], &n[k..]);
    }
    format!("{sign}0.{}{n}", "0".repeat((-(e + 1)) as usize))
}

#[test]
fn fixed_precision_exponential_match_spec_random() {
    let mut rng = Rng(0xdead_beef_1234_5678);
    for i in 0..60_000 {
        let mut x = rng.f64();
        if i % 7 == 0 {
            x = 0.0;
        }
        let f = if rng.below(3) == 0 {
            rng.below(101) as i32
        } else {
            rng.below(21) as i32
        };
        let fixed = to_fixed(x, f).unwrap();
        if x.abs() < 1e21 {
            assert_eq!(
                fixed.as_str(),
                ref_to_fixed(x, f as usize),
                "{x:e}.toFixed({f})"
            );
        } else {
            assert_eq!(fixed.as_str(), s(x));
        }
        let exp = to_exponential(x, Some(f)).unwrap();
        assert_eq!(
            exp.as_str(),
            ref_to_exponential(x, f as usize),
            "{x:e}.toExponential({f})"
        );
        let p = f.max(1);
        let prec = to_precision(x, Some(p)).unwrap();
        assert_eq!(
            prec.as_str(),
            ref_to_precision(x, p as usize),
            "{x:e}.toPrecision({p})"
        );
    }
}

#[test]
fn fixed_precision_exponential_edge_cases() {
    let fx = |x: f64, f: i32| to_fixed(x, f).unwrap().as_str().to_string();
    let ex = |x: f64, f: Option<i32>| to_exponential(x, f).unwrap().as_str().to_string();
    let pr = |x: f64, p: Option<i32>| to_precision(x, p).unwrap().as_str().to_string();
    assert_eq!(fx(0.5, 0), "1");
    assert_eq!(fx(1.5, 0), "2");
    assert_eq!(fx(2.5, 0), "3");
    assert_eq!(fx(-2.5, 0), "-3");
    assert_eq!(fx(1.005, 2), "1.00"); // 1.005 is below the tie
    assert_eq!(fx(1.25, 1), "1.3");
    assert_eq!(fx(-0.0, 2), "0.00");
    assert_eq!(fx(-0.0001, 2), "-0.00");
    assert_eq!(fx(0.0, 0), "0");
    assert_eq!(fx(f64::NAN, 2), "NaN");
    assert_eq!(fx(f64::INFINITY, 2), "Infinity");
    assert_eq!(fx(-f64::INFINITY, 2), "-Infinity");
    assert_eq!(fx(1e21, 2), "1e+21");
    assert_eq!(fx(-1e21, 2), "-1e+21");
    assert_eq!(fx(999999999999999900000.0, 2), "999999999999999868928.00");
    assert_eq!(fx(1.45, 1), "1.4");
    assert_eq!(fx(123.456, 10), "123.4560000000");
    assert_eq!(fx(0.000001, 7), "0.0000010");
    assert_eq!(fx(5e-324, 100).len(), 102);
    assert_eq!(to_fixed(1.0, -1), Err(RangeError::Digits));
    assert_eq!(to_fixed(1.0, 101), Err(RangeError::Digits));
    assert_eq!(to_fixed(f64::NAN, 101), Err(RangeError::Digits));
    // The longest output.
    let longest = fx(-999999999999999900000.0, 100);
    assert_eq!(longest.len(), MAX_FIXED_LEN);

    assert_eq!(ex(0.0, None), "0e+0");
    assert_eq!(ex(-0.0, Some(2)), "0.00e+0");
    assert_eq!(ex(123.0, None), "1.23e+2");
    assert_eq!(ex(123.0, Some(0)), "1e+2");
    assert_eq!(ex(125.0, Some(1)), "1.3e+2");
    assert_eq!(ex(1.0, Some(3)), "1.000e+0");
    assert_eq!(ex(5e-324, None), "5e-324");
    assert_eq!(ex(5e-324, Some(3)), "4.941e-324");
    assert_eq!(ex(f64::MAX, Some(100)).len(), 107);
    assert_eq!(ex(f64::INFINITY, Some(1000)), "Infinity");
    assert_eq!(ex(f64::NAN, Some(-1)), "NaN");
    assert_eq!(to_exponential(1.0, Some(101)), Err(RangeError::Digits));
    assert_eq!(to_exponential(1.0, Some(-1)), Err(RangeError::Digits));

    assert_eq!(pr(0.0, Some(1)), "0");
    assert_eq!(pr(0.0, Some(3)), "0.00");
    assert_eq!(pr(123.456, None), "123.456");
    assert_eq!(pr(123.456, Some(2)), "1.2e+2");
    assert_eq!(pr(123.456, Some(3)), "123");
    assert_eq!(pr(123.456, Some(4)), "123.5");
    assert_eq!(pr(0.000001234, Some(2)), "0.0000012");
    assert_eq!(pr(0.0000001234, Some(2)), "1.2e-7");
    assert_eq!(pr(1e21, Some(3)), "1.00e+21");
    assert_eq!(pr(99.99, Some(3)), "100");
    assert_eq!(pr(999.9, Some(3)), "1.00e+3");
    assert_eq!(pr(f64::NAN, Some(1000)), "NaN");
    assert_eq!(pr(-f64::INFINITY, Some(0)), "-Infinity");
    assert_eq!(to_precision(1.0, Some(0)), Err(RangeError::Digits));
    assert_eq!(to_precision(1.0, Some(101)), Err(RangeError::Digits));
    assert!(pr(-1.2345e-6, Some(100)).len() <= MAX_FIXED_LEN);
    assert!(pr(-f64::MAX, Some(100)).len() <= MAX_FIXED_LEN);
}

/// Reads a radix string back exactly.
fn parse_radix_exact(s: &str, radix: u32) -> f64 {
    let (neg, s) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let (int, frac) = s.split_once('.').unwrap_or((s, ""));
    let mut b = big::Big::zero();
    for c in int.chars().chain(frac.chars()) {
        b.mul_add_small(radix, c.to_digit(radix).unwrap());
    }
    let v = exact::scale_to_f64(b, 0, radix, -(frac.len() as i32), false);
    if neg { -v } else { v }
}

#[test]
fn radix_round_trips() {
    let mut rng = Rng(0x0123_4567_89ab_cdef);
    for i in 0..20_000 {
        let x = if i % 5 == 0 {
            (rng.below(1 << 40) as f64) * if i % 2 == 0 { 1.0 } else { -1.0 }
        } else {
            rng.f64()
        };
        let radix = 2 + rng.below(35) as i32;
        if radix == 10 {
            continue;
        }
        let out = to_string_radix(x, radix).unwrap();
        let text = out.as_str();
        assert!(!text.contains('e') || radix > 14, "{text}");
        assert!(
            same(parse_radix_exact(text, radix as u32), x),
            "{x:e} radix {radix}: {text}"
        );
        // Integers print exactly.
        if x.abs() < 9007199254740992.0 && x == (x as i64) as f64 {
            let expect = parse_int(text.as_bytes(), radix);
            assert!(same(expect, x) || x == 0.0);
        }
    }
    assert_eq!(to_string_radix(255.0, 16).unwrap().as_str(), "ff");
    assert_eq!(to_string_radix(-255.0, 2).unwrap().as_str(), "-11111111");
    assert_eq!(to_string_radix(0.5, 2).unwrap().as_str(), "0.1");
    assert_eq!(to_string_radix(-0.0, 2).unwrap().as_str(), "0");
    assert_eq!(to_string_radix(f64::NAN, 2).unwrap().as_str(), "NaN");
    assert_eq!(
        to_string_radix(f64::NEG_INFINITY, 36).unwrap().as_str(),
        "-Infinity"
    );
    assert_eq!(to_string_radix(0.1, 10).unwrap().as_str(), "0.1");
    assert_eq!(to_string_radix(1.0, 1), Err(RangeError::Radix));
    assert_eq!(to_string_radix(1.0, 37), Err(RangeError::Radix));
    let longest = to_string_radix(-5e-324, 2).unwrap();
    assert_eq!(longest.len(), MAX_RADIX_LEN);
    assert_eq!(to_string_radix(f64::MAX, 2).unwrap().len(), 1024);
}

/// Rough timings. Run with
/// `cargo test -p djs-core --release bench_conversions -- --ignored --nocapture`.
#[test]
#[ignore]
fn bench_conversions() {
    use core::hint::black_box;
    use std::fmt::Write;
    use std::println;
    use std::time::Instant;

    fn time<T>(name: &str, inputs: &[T], mut f: impl FnMut(&T)) {
        let rounds = (2_000_000 / inputs.len()).max(1);
        let start = Instant::now();
        for _ in 0..rounds {
            for x in inputs {
                f(x);
            }
        }
        let ns = start.elapsed().as_nanos() as f64 / (rounds * inputs.len()) as f64;
        println!("{name:40} {ns:8.1} ns/op");
    }

    let mut rng = Rng(42);
    let ints: Vec<f64> = (0..1000).map(|_| rng.below(1_000_000) as f64).collect();
    let short: Vec<f64> = (0..1000)
        .map(|_| rng.below(100_000) as f64 / 100.0)
        .collect();
    let random: Vec<f64> = (0..1000)
        .map(|_| f64::from_bits(rng.next() >> 2).abs())
        .filter(|x| x.is_finite())
        .collect();
    let mut buf = String::new();
    time("to_string(int)", &ints, |&x| {
        black_box(to_string(black_box(x)));
    });
    time("to_string(short decimal)", &short, |&x| {
        black_box(to_string(black_box(x)));
    });
    time("to_string(random bits)", &random, |&x| {
        black_box(to_string(black_box(x)));
    });
    time("std format {} (random bits)", &random, |&x| {
        buf.clear();
        write!(buf, "{}", black_box(x)).unwrap();
        black_box(&buf);
    });
    time("to_fixed(short decimal, 2)", &short, |&x| {
        black_box(to_fixed(black_box(x), 2).unwrap());
    });
    time("to_precision(random bits, 6)", &random, |&x| {
        black_box(to_precision(black_box(x), Some(6)).unwrap());
    });
    time("to_string_radix(random bits, 16)", &random, |&x| {
        black_box(to_string_radix(black_box(x), 16).unwrap());
    });

    let int_strs: Vec<String> = ints.iter().map(|&x| s(x)).collect();
    let short_strs: Vec<String> = short.iter().map(|&x| s(x)).collect();
    let random_strs: Vec<String> = random.iter().map(|&x| s(x)).collect();
    let long_strs: Vec<String> = random.iter().map(|&x| format!("{x:.30e}")).collect();
    time("string_to_number(int)", &int_strs, |t| {
        black_box(string_to_number(black_box(t.as_bytes())));
    });
    time("string_to_number(short decimal)", &short_strs, |t| {
        black_box(string_to_number(black_box(t.as_bytes())));
    });
    time("string_to_number(shortest of random)", &random_strs, |t| {
        black_box(string_to_number(black_box(t.as_bytes())));
    });
    time("std parse (shortest of random)", &random_strs, |t| {
        black_box(black_box(t.as_str()).parse::<f64>().unwrap());
    });
    time("string_to_number(31 digits)", &long_strs, |t| {
        black_box(string_to_number(black_box(t.as_bytes())));
    });
    let wide: Vec<Vec<u16>> = random_strs.iter().map(|t| utf16(t)).collect();
    time("string_to_number(u16, shortest)", &wide, |t| {
        black_box(string_to_number(black_box(&t[..])));
    });
}

/// Literals as the lexer splits them.
#[test]
fn literals_from_lexer() {
    use crate::lexer::{Lexer, TokenKind};
    let src = b"0 1_000.5e-3 .5 5. 0x1F_FF 0o17 0B1_0 9007199254740993 1e400 0x20000000000003";
    let expect = [
        0.0,
        1.0005,
        0.5,
        5.0,
        8191.0,
        15.0,
        2.0,
        9007199254740992.0,
        f64::INFINITY,
        9007199254740996.0,
    ];
    let mut lx = Lexer::new(src).unwrap();
    let mut got = Vec::new();
    loop {
        let tok = lx.next_token().unwrap();
        if tok.kind == TokenKind::Eof {
            break;
        }
        assert_eq!(tok.kind, TokenKind::Number);
        let lit = lx.number_literal(&tok);
        got.push(literal_digits_value(lit.base.radix(), lit.digits));
        assert!(same(literal_value(lx.text(&tok)), got[got.len() - 1]));
    }
    assert_eq!(got, expect);
}

/// D2021: parsing two-byte input narrowed to bytes gives what the two-byte instance gives.
#[test]
fn narrowed_units_parse_the_same() {
    use super::narrow_units;
    let pieces: [&str; 22] = [
        "1", "2.5", "e7", "-", "+", "0x1F", "0b101", "0o17", "Infinity", ".", " ", "\u{a0}",
        "\u{2028}", "\u{feff}", "\u{3000}", "\u{1680}", "\u{e9}", "\u{663}", "\u{ff11}", "_", "x",
        "\t",
    ];
    let mut seed = 0x0104_u64;
    let mut next = |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n as u64) as usize
    };
    let same = |a: f64, b: f64| a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan());
    for _ in 0..50_000 {
        let s: alloc::string::String = (0..next(5)).map(|_| pieces[next(pieces.len())]).collect();
        let u: alloc::vec::Vec<u16> = s.encode_utf16().collect();
        let b = narrow_units(u.iter().copied());
        assert!(
            same(string_to_number(&u[..]), string_to_number(&b[..])),
            "{s:?}"
        );
        assert!(same(parse_float(&u[..]), parse_float(&b[..])), "{s:?}");
        for r in [0, 10, 16, 2, 36] {
            assert!(
                same(parse_int(&u[..], r), parse_int(&b[..], r)),
                "{s:?} {r}"
            );
        }
    }
}
