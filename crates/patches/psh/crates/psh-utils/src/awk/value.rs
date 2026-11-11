// The number and string rules follow GoAWK's interp/value.go,
// Copyright (c) 2022 Ben Hoyt, MIT License (see NOTICE).

//! awk values: numbers, strings and "strnums" (input that looks numeric compares as a
//! number), and the conversions between them.

use std::rc::Rc;

pub type S = Rc<[u8]>;

#[derive(Clone, Debug, Default)]
pub enum Value {
    /// Never assigned: "" and 0 at once.
    #[default]
    Uninit,
    Num(f64),
    Str(S),
    /// From input (fields, getline, `var=value`, ARGV, ENVIRON, split): a number in
    /// comparisons if it looks like one.
    StrNum(S),
}

impl Value {
    pub fn str(s: &[u8]) -> Value {
        Value::Str(Rc::from(s))
    }

    pub fn strnum(s: &[u8]) -> Value {
        Value::StrNum(Rc::from(s))
    }

    pub fn bool(b: bool) -> Value {
        Value::Num(if b { 1.0 } else { 0.0 })
    }

    pub fn num(&self) -> f64 {
        match self {
            Value::Uninit => 0.0,
            Value::Num(n) => *n,
            Value::Str(s) | Value::StrNum(s) => parse_prefix(s),
        }
    }

    pub fn truth(&self) -> bool {
        match self {
            Value::Uninit => false,
            Value::Num(n) => *n != 0.0,
            Value::Str(s) => !s.is_empty(),
            Value::StrNum(s) => match looks_numeric(s) {
                Some(n) => n != 0.0,
                None => !s.is_empty(),
            },
        }
    }

    /// The number this value compares as, or `None` when it compares as a string.
    pub fn cmp_num(&self) -> Option<f64> {
        match self {
            Value::Uninit => Some(0.0),
            Value::Num(n) => Some(*n),
            Value::Str(_) => None,
            Value::StrNum(s) => looks_numeric(s),
        }
    }
}

/// Whitespace as awk skips it around numbers.
fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// The number at the start of `s` (leading blanks skipped), 0 if none: `"3x"` is 3.
/// Decimal only, as gawk without `--non-decimal-data`; `inf`/`nan` only with a sign.
pub fn parse_prefix(s: &[u8]) -> f64 {
    let mut i = 0;
    while i < s.len() && is_space(s[i]) {
        i += 1;
    }
    let (n, _) = scan_number(&s[i..]);
    n
}

/// The number `s` is entirely (blanks around it allowed), if it is one.
pub fn looks_numeric(s: &[u8]) -> Option<f64> {
    let mut i = 0;
    while i < s.len() && is_space(s[i]) {
        i += 1;
    }
    let mut j = s.len();
    while j > i && is_space(s[j - 1]) {
        j -= 1;
    }
    if i == j {
        return None;
    }
    let (n, len) = scan_number(&s[i..j]);
    (len == j - i && len > 0).then_some(n)
}

/// A decimal number at the start of `s`: its value and length (0 when there is none).
fn scan_number(s: &[u8]) -> (f64, usize) {
    let mut i = 0;
    if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
        i += 1;
    }
    // gawk: "+inf", "-nan" (with a sign only).
    if i == 1 && s.len() >= 4 {
        let w = s[1..4].to_ascii_lowercase();
        let neg = s[0] == b'-';
        if w == b"inf" {
            let len = if s.len() >= 9 && s[1..9].eq_ignore_ascii_case(b"infinity") {
                9
            } else {
                4
            };
            return (
                if neg {
                    f64::NEG_INFINITY
                } else {
                    f64::INFINITY
                },
                len,
            );
        }
        if w == b"nan" {
            return (if neg { -f64::NAN } else { f64::NAN }, 4);
        }
    }
    let digits_start = i;
    let mut whole = 0u64;
    while i < s.len() && s[i].is_ascii_digit() {
        whole = whole.wrapping_mul(10).wrapping_add(u64::from(s[i] - b'0'));
        i += 1;
    }
    // Digits only, few enough that every such number is a double exactly: no parse.
    if (1..=f64::DIGITS as usize).contains(&(i - digits_start))
        && !matches!(s.get(i), Some(b'.' | b'e' | b'E'))
    {
        let n = whole as f64;
        return (if s[0] == b'-' { -n } else { n }, i);
    }
    let mut got_digit = i > digits_start;
    if i < s.len() && s[i] == b'.' {
        i += 1;
        let frac = i;
        while i < s.len() && s[i].is_ascii_digit() {
            i += 1;
        }
        got_digit |= i > frac;
    }
    if !got_digit {
        return (0.0, 0);
    }
    let mut end = i;
    if i < s.len() && (s[i] == b'e' || s[i] == b'E') {
        let mut k = i + 1;
        if k < s.len() && (s[k] == b'+' || s[k] == b'-') {
            k += 1;
        }
        let exp_digits = k;
        while k < s.len() && s[k].is_ascii_digit() {
            k += 1;
        }
        if k > exp_digits {
            end = k;
        }
    }
    let text = std::str::from_utf8(&s[..end]).unwrap_or("0");
    // A lone "." with digits after, "5." etc. parse fine in Rust; "+.5" too.
    let n = text.parse::<f64>().unwrap_or(0.0);
    (n, end)
}

/// An integral number as awk prints it: all its digits (`1e30` is
/// `1000000000000000019884624838656`, as gawk prints it).
pub fn int_str(n: f64) -> String {
    if n.abs() < 1e18 {
        format!("{}", n as i64)
    } else {
        format!("{n:.0}")
    }
}

/// `nan` and `inf` as gawk prints them: with a sign always.
pub fn special_str(n: f64) -> &'static str {
    if n.is_nan() {
        if n.is_sign_negative() { "-nan" } else { "+nan" }
    } else if n > 0.0 {
        "+inf"
    } else {
        "-inf"
    }
}

/// Whether a number converts without CONVFMT/OFMT: integers, nan and inf.
pub fn is_integral(n: f64) -> bool {
    !n.is_finite() || n == n.trunc()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_from_strings() {
        assert_eq!(parse_prefix(b"  3x"), 3.0);
        assert_eq!(parse_prefix(b"1e3"), 1000.0);
        assert_eq!(parse_prefix(b"1e"), 1.0);
        assert_eq!(parse_prefix(b".5."), 0.5);
        assert_eq!(parse_prefix(b"0x1A"), 0.0);
        assert_eq!(parse_prefix(b"-inf"), f64::NEG_INFINITY);
        assert_eq!(parse_prefix(b"inf"), 0.0);
        assert_eq!(looks_numeric(b" 12 "), Some(12.0));
        assert_eq!(looks_numeric(b"12a"), None);
        assert_eq!(looks_numeric(b""), None);
        assert_eq!(looks_numeric(b"."), None);
        assert_eq!(looks_numeric(b"+1.5e-3"), Some(0.0015));
        assert_eq!(int_str(1e30), "1000000000000000019884624838656");
        assert_eq!(int_str(-3.0), "-3");
    }
}
