//! `sort` with `-b -d -f -g -h -i -M -n -R -r -V` (globally and per key), `-k -t -s -u -z
//! -o -c -C -m`, `--sort=WORD`; `-S -T --parallel --batch-size` are accepted and ignored.
//! Written for psh after GNU's documented behavior (key fields, the last-resort
//! comparison, version order as documented for `sort -V`/filevercmp).
//!
//! Collation is byte order in every locale (D1160), as `LC_ALL=C sort`: the same output
//! on every OS. `--compress-program`, `--files0-from`, `--random-source` and `--debug`
//! fall through to the real `sort`.

use std::cmp::Ordering;
use std::hash::{BuildHasher, Hasher};

use psh_engine::host::OpenOptions;
use psh_engine::util::UtilCtx;

use crate::common;
use crate::opts::{Spec, both, flag, long};
use crate::textio;

pub const SPEC: Spec = Spec {
    name: "sort",
    opts: &[
        both('b', "ignore-leading-blanks"),
        both('d', "dictionary-order"),
        both('f', "ignore-case"),
        both('g', "general-numeric-sort"),
        both('h', "human-numeric-sort"),
        both('i', "ignore-nonprinting"),
        both('M', "month-sort"),
        both('n', "numeric-sort"),
        both('R', "random-sort"),
        long("random-source").value(),
        both('r', "reverse"),
        long("sort").value(),
        both('V', "version-sort"),
        long("batch-size").value(),
        both('c', "check").optional(),
        flag('C'),
        long("compress-program").value(),
        long("debug"),
        long("files0-from").value(),
        both('k', "key").value(),
        both('m', "merge"),
        both('o', "output").value(),
        both('s', "stable"),
        both('S', "buffer-size").value(),
        both('t', "field-separator").value(),
        both('T', "temporary-directory").value(),
        long("parallel").value(),
        both('u', "unique"),
        both('z', "zero-terminated"),
    ],
    posix_order: false,
};

#[derive(Clone, Copy, PartialEq, Debug, Default)]
enum Kind {
    #[default]
    Text,
    Numeric,
    General,
    Human,
    Month,
    Version,
    Random,
}

/// The ordering options of a key (or the global ones).
#[derive(Clone, Copy, Debug, Default)]
struct Order {
    blank_start: bool,
    blank_end: bool,
    dictionary: bool,
    fold: bool,
    printable: bool,
    kind: Kind,
    reverse: bool,
}

impl Order {
    /// Sets the option for one letter; false if it is not an ordering option.
    fn set(&mut self, c: u8, end: bool) -> bool {
        match c {
            b'b' if end => self.blank_end = true,
            b'b' => self.blank_start = true,
            b'd' => self.dictionary = true,
            b'f' => self.fold = true,
            b'i' => self.printable = true,
            b'g' => self.kind = Kind::General,
            b'h' => self.kind = Kind::Human,
            b'M' => self.kind = Kind::Month,
            b'n' => self.kind = Kind::Numeric,
            b'R' => self.kind = Kind::Random,
            b'V' => self.kind = Kind::Version,
            b'r' => self.reverse = true,
            _ => return false,
        }
        true
    }

    fn any(&self) -> bool {
        self.blank_start
            || self.blank_end
            || self.dictionary
            || self.fold
            || self.printable
            || self.kind != Kind::Text
            || self.reverse
    }
}

/// `-k F1[.C1][OPTS][,F2[.C2][OPTS]]`; positions are 1-based, `end_char` 0 is the end
/// of the field.
#[derive(Clone, Copy, Debug)]
struct Key {
    start_field: usize,
    start_char: usize,
    end: Option<(usize, usize)>,
    order: Order,
}

fn parse_key(spec: &[u8], global: &Order, utf8: bool) -> Result<Key, String> {
    let text = String::from_utf8_lossy(spec).into_owned();
    let q = |s: &str| common::quote_in(utf8, s.as_bytes());
    let bad = |why: &str| format!("{why}: invalid field specification {}", q(&text));
    let (a, b) = match text.split_once(',') {
        Some((a, b)) => (a, Some(b)),
        None => (text.as_str(), None),
    };
    let mut order = Order::default();
    // F[.C]OPTS
    let mut pos = |s: &str, end: bool| -> Result<(usize, usize), String> {
        let digits = s.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return Err(format!(
                "invalid number at field start: invalid count at start of {}",
                q(s)
            ));
        }
        let field: usize = s[..digits].parse().unwrap_or(usize::MAX);
        let mut rest = &s[digits..];
        let mut ch = 0;
        if let Some(r) = rest.strip_prefix('.') {
            let d = r.bytes().take_while(u8::is_ascii_digit).count();
            if d == 0 {
                return Err(format!(
                    "invalid number after '.': invalid count at start of {}",
                    q(r)
                ));
            }
            ch = r[..d].parse().unwrap_or(usize::MAX);
            rest = &r[d..];
        }
        for c in rest.bytes() {
            if !order.set(c, end) {
                return Err(format!(
                    "stray character in field spec: invalid field specification {}",
                    q(&text)
                ));
            }
        }
        Ok((field, ch))
    };
    let (sf, sc) = pos(a, false)?;
    if sf == 0 {
        return Err(bad("field number is zero"));
    }
    let start_char = match (a.contains('.'), sc) {
        (true, 0) => return Err(bad("character offset is zero")),
        (true, c) => c,
        (false, _) => 1,
    };
    let end = match b {
        Some(b) => {
            let (ef, ec) = pos(b, true)?;
            if ef == 0 {
                return Err(bad("field number is zero"));
            }
            Some((ef, ec))
        }
        None => None,
    };
    // A key without ordering options takes the global ones.
    if !order.any() {
        order = *global;
    }
    Ok(Key {
        start_field: sf,
        start_char,
        end,
        order,
    })
}

fn is_blank(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

/// Where field `f` (1-based) begins: after the (f-1)th separator, or (without `-t`) at
/// the blanks before the f-th run of non-blanks.
fn field_start(line: &[u8], f: usize, tab: Option<u8>) -> usize {
    let mut pos = 0;
    for _ in 1..f {
        match tab {
            Some(t) => match line[pos..].iter().position(|&b| b == t) {
                Some(i) => pos += i + 1,
                None => return line.len(),
            },
            None => {
                while pos < line.len() && is_blank(line[pos]) {
                    pos += 1;
                }
                while pos < line.len() && !is_blank(line[pos]) {
                    pos += 1;
                }
            }
        }
    }
    pos
}

fn skip_blanks(line: &[u8], mut pos: usize) -> usize {
    while pos < line.len() && is_blank(line[pos]) {
        pos += 1;
    }
    pos
}

fn key_slice<'a>(line: &'a [u8], k: &Key, tab: Option<u8>) -> &'a [u8] {
    let mut s = field_start(line, k.start_field, tab);
    if k.order.blank_start {
        s = skip_blanks(line, s);
    }
    s = s.saturating_add(k.start_char - 1).min(line.len());
    let e = match k.end {
        None => line.len(),
        Some((ef, 0)) => {
            let fs = field_start(line, ef, tab);
            match tab {
                Some(t) => line[fs..]
                    .iter()
                    .position(|&b| b == t)
                    .map_or(line.len(), |i| fs + i),
                None => {
                    let mut p = skip_blanks(line, fs);
                    while p < line.len() && !is_blank(line[p]) {
                        p += 1;
                    }
                    p
                }
            }
        }
        Some((ef, ec)) => {
            let mut fs = field_start(line, ef, tab);
            if k.order.blank_end {
                fs = skip_blanks(line, fs);
            }
            fs.saturating_add(ec).min(line.len())
        }
    };
    if e <= s { &line[s..s] } else { &line[s..e] }
}

/// `-n`: sign, integer digits without leading zeros, fraction digits without trailing
/// zeros. Compared as strings, so any length is exact.
fn numeric_parts(s: &[u8]) -> (i8, &[u8], &[u8]) {
    let mut i = skip_blanks(s, 0);
    let neg = s.get(i) == Some(&b'-');
    if neg {
        i += 1;
    }
    let start = i;
    while i < s.len() && s[i].is_ascii_digit() {
        i += 1;
    }
    let mut int = &s[start..i];
    let mut frac: &[u8] = &[];
    if s.get(i) == Some(&b'.') {
        let fs = i + 1;
        let mut j = fs;
        while j < s.len() && s[j].is_ascii_digit() {
            j += 1;
        }
        frac = &s[fs..j];
    }
    while int.first() == Some(&b'0') {
        int = &int[1..];
    }
    while frac.last() == Some(&b'0') {
        frac = &frac[..frac.len() - 1];
    }
    let sign = if int.is_empty() && frac.is_empty() {
        0
    } else if neg {
        -1
    } else {
        1
    };
    (sign, int, frac)
}

fn cmp_magnitude(a: (&[u8], &[u8]), b: (&[u8], &[u8])) -> Ordering {
    a.0.len()
        .cmp(&b.0.len())
        .then_with(|| a.0.cmp(b.0))
        .then_with(|| a.1.cmp(b.1))
}

fn cmp_numeric(a: &[u8], b: &[u8]) -> Ordering {
    cmp_parts(numeric_parts(a), numeric_parts(b))
}

type Parts<'a> = (i8, &'a [u8], &'a [u8]);

fn cmp_parts((sa, ia, fa): Parts<'_>, (sb, ib, fb): Parts<'_>) -> Ordering {
    sa.cmp(&sb).then_with(|| match sa {
        1 => cmp_magnitude((ia, fa), (ib, fb)),
        -1 => cmp_magnitude((ib, fb), (ia, fa)),
        _ => Ordering::Equal,
    })
}

/// `-h`: the sign, then the SI suffix, then the number.
fn cmp_human(a: &[u8], b: &[u8]) -> Ordering {
    fn suffix(s: &[u8]) -> usize {
        let mut i = skip_blanks(s, 0);
        if s.get(i) == Some(&b'-') {
            i += 1;
        }
        while i < s.len() && (s[i].is_ascii_digit() || s[i] == b'.') {
            i += 1;
        }
        match s.get(i) {
            Some(b'k' | b'K') => 1,
            Some(b'M') => 2,
            Some(b'G') => 3,
            Some(b'T') => 4,
            Some(b'P') => 5,
            Some(b'E') => 6,
            Some(b'Z') => 7,
            Some(b'Y') => 8,
            Some(b'R') => 9,
            Some(b'Q') => 10,
            _ => 0,
        }
    }
    let (sa, ia, fa) = numeric_parts(a);
    let (sb, ib, fb) = numeric_parts(b);
    sa.cmp(&sb).then_with(|| {
        let m = suffix(a)
            .cmp(&suffix(b))
            .then_with(|| cmp_magnitude((ia, fa), (ib, fb)));
        if sa < 0 { m.reverse() } else { m }
    })
}

/// `-g`: a leading floating-point number (decimal, hex, `inf`, `nan`), parsed as
/// strtod would. Non-numbers sort first, then NaN, then numbers.
fn general_value(s: &[u8]) -> (u8, f64) {
    let s = &s[skip_blanks(s, 0)..];
    let t = String::from_utf8_lossy(s);
    let t = t.as_ref();
    let (neg, body) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let sign = if neg { -1.0 } else { 1.0 };
    let lower = body.to_ascii_lowercase();
    if lower.starts_with("nan") {
        return (1, f64::NAN);
    }
    if lower.starts_with("inf") {
        return (2, sign * f64::INFINITY);
    }
    if let Some(h) = lower.strip_prefix("0x") {
        let d = h.bytes().take_while(u8::is_ascii_hexdigit).count();
        if d > 0 {
            let v = u64::from_str_radix(&h[..d.min(16)], 16).unwrap_or(u64::MAX) as f64;
            return (2, sign * v);
        }
    }
    // The longest prefix that parses: digits, point, exponent.
    let b = body.as_bytes();
    let mut i = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let mut digits = i;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        digits += i - digits - 1;
    }
    if digits == 0 {
        return (0, 0.0);
    }
    let mut end = i;
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let ds = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > ds {
            end = j;
        }
    }
    match body[..end].parse::<f64>() {
        Ok(v) => (2, sign * v),
        Err(_) => (0, 0.0),
    }
}

fn cmp_general(a: &[u8], b: &[u8]) -> Ordering {
    let (ca, va) = general_value(a);
    let (cb, vb) = general_value(b);
    ca.cmp(&cb)
        .then_with(|| va.partial_cmp(&vb).unwrap_or(Ordering::Equal))
}

fn month(s: &[u8]) -> u8 {
    const MONTHS: [&[u8]; 12] = [
        b"JAN", b"FEB", b"MAR", b"APR", b"MAY", b"JUN", b"JUL", b"AUG", b"SEP", b"OCT", b"NOV",
        b"DEC",
    ];
    let s = &s[skip_blanks(s, 0)..];
    if s.len() < 3 {
        return 0;
    }
    let head = s[..3].to_ascii_uppercase();
    MONTHS
        .iter()
        .position(|m| *m == head.as_slice())
        .map_or(0, |i| i as u8 + 1)
}

/// The order of a non-digit character in a version: `~` before the end, letters
/// before everything else.
fn ver_order(c: Option<u8>) -> i32 {
    match c {
        None => 0,
        Some(c) if c.is_ascii_digit() => 0,
        Some(c) if c.is_ascii_alphabetic() => i32::from(c),
        Some(b'~') => -1,
        Some(c) => i32::from(c) + 256,
    }
}

/// Debian's version comparison: non-digit runs by `ver_order`, digit runs by value.
fn verrevcmp(a: &[u8], b: &[u8]) -> Ordering {
    let digit = |s: &[u8], i: usize| s.get(i).is_some_and(u8::is_ascii_digit);
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        while (i < a.len() && !digit(a, i)) || (j < b.len() && !digit(b, j)) {
            let (ca, cb) = (ver_order(a.get(i).copied()), ver_order(b.get(j).copied()));
            if ca != cb {
                return ca.cmp(&cb);
            }
            i += 1;
            j += 1;
        }
        while a.get(i) == Some(&b'0') {
            i += 1;
        }
        while b.get(j) == Some(&b'0') {
            j += 1;
        }
        let mut first = Ordering::Equal;
        while digit(a, i) && digit(b, j) {
            if first == Ordering::Equal {
                first = a[i].cmp(&b[j]);
            }
            i += 1;
            j += 1;
        }
        if digit(a, i) {
            return Ordering::Greater;
        }
        if digit(b, j) {
            return Ordering::Less;
        }
        if first != Ordering::Equal {
            return first;
        }
    }
    Ordering::Equal
}

/// The length of a version's file suffix: the longest match of
/// `(\.[A-Za-z~][A-Za-z0-9~]*)*$`.
fn suffix_len(s: &[u8]) -> usize {
    let ok = |c: u8| c.is_ascii_alphanumeric() || c == b'~';
    // Segments are found from the end: each is the run of [A-Za-z0-9~] before the
    // current start, preceded by a `.` and starting with a letter or `~`.
    let mut start = s.len();
    loop {
        let mut i = start;
        while i > 0 && ok(s[i - 1]) {
            i -= 1;
        }
        if i > 0 && i < start && s[i - 1] == b'.' && (s[i].is_ascii_alphabetic() || s[i] == b'~') {
            start = i - 1;
        } else {
            return s.len() - start;
        }
    }
}

/// GNU's version order (`sort -V`, `ls -v`).
fn filevercmp(a: &[u8], b: &[u8]) -> Ordering {
    if a == b {
        return Ordering::Equal;
    }
    for special in [&b""[..], b".", b".."] {
        match (a == special, b == special) {
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
            _ => {}
        }
    }
    let (mut a, mut b) = (a, b);
    match (a.first() == Some(&b'.'), b.first() == Some(&b'.')) {
        (true, false) => return Ordering::Less,
        (false, true) => return Ordering::Greater,
        (true, true) => {
            a = &a[1..];
            b = &b[1..];
        }
        _ => {}
    }
    let pa = &a[..a.len() - suffix_len(a)];
    let pb = &b[..b.len() - suffix_len(b)];
    let r = if pa == pb {
        Ordering::Equal
    } else {
        verrevcmp(pa, pb)
    };
    if r == Ordering::Equal {
        verrevcmp(a, b)
    } else {
        r
    }
}

/// Text comparison with `-d -f -i`.
fn cmp_text(a: &[u8], b: &[u8], o: &Order) -> Ordering {
    if !o.dictionary && !o.fold && !o.printable {
        return a.cmp(b);
    }
    let keep = |c: &u8| {
        (!o.dictionary || is_blank(*c) || c.is_ascii_alphanumeric())
            && (!o.printable || (0x20..0x7f).contains(c))
    };
    let map = |c: &u8| if o.fold { c.to_ascii_uppercase() } else { *c };
    a.iter()
        .filter(|c| keep(c))
        .map(map)
        .cmp(b.iter().filter(|c| keep(c)).map(map))
}

struct Num {
    rank: u128,
    prefix: u64,
    line: usize,
    exact: bool,
}

impl Num {
    fn new(line: usize, text: &[u8], k: &Key, tab: Option<u8>) -> Num {
        let (sign, int, frac) = numeric_parts(key_slice(text, k, tab));
        let exact = int.len() <= 18 && frac.len() <= 18;
        let mut rank = 1u128 << 120;
        if exact && sign != 0 {
            let int = int
                .iter()
                .fold(0u128, |v, &d| v * 10 + u128::from(d - b'0'));
            let frac = (0..18).fold(0u128, |v, j| {
                v * 10 + frac.get(j).map_or(0, |&d| u128::from(d - b'0'))
            });
            let m = int << 60 | frac;
            rank = if sign > 0 {
                (2u128 << 120) + m
            } else {
                (1u128 << 120) - 1 - m
            };
        }
        let mut prefix = [0u8; 8];
        let n = text.len().min(8);
        prefix[..n].copy_from_slice(&text[..n]);
        Num {
            rank,
            prefix: u64::from_be_bytes(prefix),
            line,
            exact,
        }
    }
}

struct Sorter {
    keys: Vec<Key>,
    tab: Option<u8>,
    /// The last-resort comparison of whole lines (not with `-s` or `-u`).
    last_resort: bool,
    reverse: bool,
    seed: std::collections::hash_map::RandomState,
}

impl Sorter {
    fn hash(&self, s: &[u8]) -> u64 {
        let mut h = self.seed.build_hasher();
        h.write(s);
        h.finish()
    }

    fn cmp_keys(&self, a: &[u8], b: &[u8]) -> Ordering {
        for k in &self.keys {
            let r = self.cmp_key(k, key_slice(a, k, self.tab), key_slice(b, k, self.tab));
            if r != Ordering::Equal {
                return r;
            }
        }
        Ordering::Equal
    }

    fn keyed<'a>(&self, line: &'a [u8], k: &Key) -> (&'a [u8], Parts<'a>) {
        let text = key_slice(line, k, self.tab);
        match k.order.kind {
            Kind::Numeric => (text, numeric_parts(text)),
            _ => (text, (0, &[], &[])),
        }
    }

    fn cmp_keyed(
        &self,
        ka: &[(&[u8], Parts<'_>)],
        kb: &[(&[u8], Parts<'_>)],
        a: &[u8],
        b: &[u8],
    ) -> Ordering {
        for ((k, x), y) in self.keys.iter().zip(ka).zip(kb) {
            let r = match k.order.kind {
                Kind::Numeric if k.order.reverse => cmp_parts(x.1, y.1).reverse(),
                Kind::Numeric => cmp_parts(x.1, y.1),
                _ => self.cmp_key(k, x.0, y.0),
            };
            if r != Ordering::Equal {
                return r;
            }
        }
        if !self.last_resort {
            return Ordering::Equal;
        }
        if self.reverse { b.cmp(a) } else { a.cmp(b) }
    }

    fn cmp_num(&self, k: &Key, a: &Num, b: &Num, lines: &[&[u8]]) -> Ordering {
        let r = if a.exact && b.exact {
            a.rank.cmp(&b.rank)
        } else {
            cmp_parts(
                numeric_parts(key_slice(lines[a.line], k, self.tab)),
                numeric_parts(key_slice(lines[b.line], k, self.tab)),
            )
        };
        let r = if k.order.reverse { r.reverse() } else { r };
        if r != Ordering::Equal || !self.last_resort {
            return r;
        }
        let (x, y) = if self.reverse { (b, a) } else { (a, b) };
        x.prefix
            .cmp(&y.prefix)
            .then_with(|| lines[x.line].cmp(lines[y.line]))
    }

    fn cmp_key(&self, k: &Key, ka: &[u8], kb: &[u8]) -> Ordering {
        let o = &k.order;
        let r = match o.kind {
            Kind::Text => cmp_text(ka, kb, o),
            Kind::Numeric => cmp_numeric(ka, kb),
            Kind::General => cmp_general(ka, kb),
            Kind::Human => cmp_human(ka, kb),
            Kind::Month => month(ka).cmp(&month(kb)),
            Kind::Version => filevercmp(ka, kb),
            Kind::Random => self
                .hash(ka)
                .cmp(&self.hash(kb))
                .then_with(|| cmp_text(ka, kb, o)),
        };
        if o.reverse { r.reverse() } else { r }
    }

    fn cmp(&self, a: &[u8], b: &[u8]) -> Ordering {
        let r = self.cmp_keys(a, b);
        if r != Ordering::Equal || !self.last_resort {
            return r;
        }
        if self.reverse { b.cmp(a) } else { a.cmp(b) }
    }
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &SPEC, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    for unsupported in ["compress-program", "files0-from", "random-source", "debug"] {
        if p.has(&SPEC, unsupported) {
            return textio::fall_through(ctx, argv, &format!("option '--{unsupported}'")).await;
        }
    }
    let mut global = Order::default();
    let mut key_specs = Vec::new();
    let mut check: Option<bool> = None; // Some(quiet)
    let mut tab: Option<u8> = None;
    for (k, v) in p.iter(&SPEC) {
        let letter = match k.as_str() {
            "ignore-leading-blanks" => Some(b'b'),
            "dictionary-order" => Some(b'd'),
            "ignore-case" => Some(b'f'),
            "general-numeric-sort" => Some(b'g'),
            "human-numeric-sort" => Some(b'h'),
            "ignore-nonprinting" => Some(b'i'),
            "month-sort" => Some(b'M'),
            "numeric-sort" => Some(b'n'),
            "random-sort" => Some(b'R'),
            "reverse" => Some(b'r'),
            "version-sort" => Some(b'V'),
            "sort" => match v.unwrap_or_default() {
                b"general-numeric" => Some(b'g'),
                b"human-numeric" => Some(b'h'),
                b"month" => Some(b'M'),
                b"numeric" => Some(b'n'),
                b"random" => Some(b'R'),
                b"version" => Some(b'V'),
                other => {
                    let msg = format!(
                        "invalid argument {} for {}",
                        common::quote(ctx, other),
                        common::quote(ctx, b"--sort")
                    );
                    return common::usage(ctx, &msg).await;
                }
            },
            _ => None,
        };
        if let Some(c) = letter {
            global.set(c, false);
            if c == b'b' {
                global.set(c, true);
            }
            continue;
        }
        match k.as_str() {
            "key" => key_specs.push(v.unwrap_or_default().to_vec()),
            "check" => {
                check = Some(match v {
                    None | Some(b"diagnose-first") => false,
                    Some(b"quiet" | b"silent") => true,
                    Some(o) => {
                        let msg = format!(
                            "invalid argument {} for {}",
                            common::quote(ctx, o),
                            common::quote(ctx, b"--check")
                        );
                        return common::usage(ctx, &msg).await;
                    }
                })
            }
            "C" => check = Some(true),
            "field-separator" => {
                let v = v.unwrap_or_default();
                tab = Some(match v {
                    b"" => {
                        ctx.error("empty tab").await;
                        return 2;
                    }
                    b"\\0" => 0,
                    [c] => *c,
                    _ => {
                        let msg = format!("multi-character tab {}", common::quote(ctx, v));
                        ctx.error(&msg).await;
                        return 2;
                    }
                });
            }
            _ => {}
        }
    }
    let mut keys = Vec::new();
    for spec in &key_specs {
        match parse_key(spec, &global, ctx.utf8()) {
            Ok(k) => keys.push(k),
            Err(e) => {
                ctx.error(&e).await;
                return 2;
            }
        }
    }
    if keys.is_empty() {
        keys.push(Key {
            start_field: 1,
            start_char: 1,
            end: None,
            order: global,
        });
    }
    let unique = p.has(&SPEC, "unique");
    let sorter = Sorter {
        keys,
        tab,
        last_resort: !unique && !p.has(&SPEC, "stable"),
        reverse: global.reverse,
        seed: std::collections::hash_map::RandomState::new(),
    };
    let eol = if p.has(&SPEC, "zero-terminated") {
        0
    } else {
        b'\n'
    };

    // Every input, read whole; each one's last line is terminated.
    let names = common::operands_or_stdin(&p);
    if check.is_some() && names.len() > 1 {
        let o = String::from_utf8_lossy(&names[1]).into_owned();
        return common::usage(ctx, &format!("extra operand '{o}' not allowed with -c")).await;
    }
    let mut data = Vec::new();
    for name in &names {
        let input = match common::Input::open(ctx, name).await {
            Ok(i) => i,
            Err(e) => {
                let text = format!("cannot read: {}", String::from_utf8_lossy(name));
                common::file_error(ctx, text.as_bytes(), &e).await;
                return 2;
            }
        };
        let start = data.len();
        if let Err(e) = input.read_to_end(ctx, &mut data).await {
            common::file_error(ctx, name, &e).await;
            return 2;
        }
        if data.len() > start && data.last() != Some(&eol) {
            data.push(eol);
        }
        ctx.check().await;
    }
    let mut lines: Vec<&[u8]> = if data.is_empty() {
        Vec::new()
    } else {
        data[..data.len() - 1].split(|&b| b == eol).collect()
    };

    if let Some(quiet) = check {
        for i in 1..lines.len() {
            let r = sorter.cmp(lines[i - 1], lines[i]);
            if r == Ordering::Greater || (unique && r == Ordering::Equal) {
                if !quiet {
                    let name = String::from_utf8_lossy(&names[0]).into_owned();
                    let mut msg = format!("sort: {name}:{}: disorder: ", i + 1).into_bytes();
                    msg.extend_from_slice(lines[i]);
                    msg.push(b'\n');
                    ctx.error_raw(&msg).await;
                }
                return 1;
            }
        }
        return 0;
    }

    let whole = |k: &Key| {
        k.start_field == 1
            && k.start_char == 1
            && k.end.is_none()
            && !k.order.blank_start
            && matches!(k.order.kind, Kind::Text)
    };
    if sorter.keys.iter().all(whole) {
        lines.sort_by(|a, b| sorter.cmp(a, b));
    } else {
        let n = sorter.keys.len();
        if let [k] = &sorter.keys[..]
            && matches!(k.order.kind, Kind::Numeric)
        {
            let mut recs: Vec<Num> = lines
                .iter()
                .enumerate()
                .map(|(i, l)| Num::new(i, l, k, sorter.tab))
                .collect();
            let cmp = |a: &Num, b: &Num| sorter.cmp_num(k, a, b, &lines);
            if sorter.last_resort {
                recs.sort_unstable_by(cmp);
            } else {
                recs.sort_by(cmp);
            }
            lines = recs.into_iter().map(|r| lines[r.line]).collect();
        } else {
            let keyed: Vec<(&[u8], Parts<'_>)> = lines
                .iter()
                .flat_map(|l| sorter.keys.iter().map(|k| sorter.keyed(l, k)))
                .collect();
            let mut order: Vec<usize> = (0..lines.len()).collect();
            order.sort_by(|&a, &b| {
                sorter.cmp_keyed(
                    &keyed[a * n..][..n],
                    &keyed[b * n..][..n],
                    lines[a],
                    lines[b],
                )
            });
            lines = order.into_iter().map(|i| lines[i]).collect();
        }
    }
    ctx.check().await;
    if unique {
        lines.dedup_by(|b, a| sorter.cmp_keys(a, b) == Ordering::Equal);
    }
    let mut out = Vec::with_capacity(data.len());
    for l in &lines {
        out.extend_from_slice(l);
        out.push(eol);
    }
    // -o may name an input: everything is read before it is opened.
    if let Some(o) = p.last(&SPEC, "output") {
        let o = o.to_vec();
        let r = match ctx.open(&o, OpenOptions::write()).await {
            Ok(f) => f.write_all(&out).await,
            Err(e) => Err(e),
        };
        if let Err(e) = r {
            let name = String::from_utf8_lossy(&o).into_owned();
            ctx.error(&format!(
                "open failed: {name}: {}",
                psh_engine::host::error_text(&e)
            ))
            .await;
            return 2;
        }
        return 0;
    }
    if let Err(e) = ctx.write(&out).await {
        return common::write_failed(ctx, &e).await;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(lines: &[&str], keys: &[&str], global: &str) -> Vec<String> {
        let mut g = Order::default();
        for c in global.bytes() {
            g.set(c, false);
        }
        let mut ks: Vec<Key> = keys
            .iter()
            .map(|k| parse_key(k.as_bytes(), &g, false).unwrap())
            .collect();
        if ks.is_empty() {
            ks.push(Key {
                start_field: 1,
                start_char: 1,
                end: None,
                order: g,
            });
        }
        let s = Sorter {
            keys: ks,
            tab: None,
            last_resort: true,
            reverse: g.reverse,
            seed: Default::default(),
        };
        let mut v: Vec<&[u8]> = lines.iter().map(|l| l.as_bytes()).collect();
        v.sort_by(|a, b| s.cmp(a, b));
        v.iter()
            .map(|l| String::from_utf8_lossy(l).into_owned())
            .collect()
    }

    #[test]
    fn numeric() {
        assert_eq!(
            sorted(
                &["x", "-1", "1a", "", " 3", "1.5", "-0", "0", "+2", "1,000"],
                &[],
                "n"
            ),
            ["-1", "", "+2", "-0", "0", "x", "1,000", "1a", "1.5", " 3"]
        );
        assert_eq!(
            sorted(&["1K", "2M", "500", "1G", "-1K", "x", "1.5K"], &[], "h"),
            ["-1K", "x", "500", "1K", "1.5K", "2M", "1G"]
        );
        assert_eq!(
            sorted(&["nan", "1e3", "x", "-inf", "5", "0x10"], &[], "g"),
            ["x", "nan", "-inf", "5", "0x10", "1e3"]
        );
        assert_eq!(
            sorted(&["FEB", "jan", "xyz", " dec"], &[], "M"),
            ["xyz", "jan", "FEB", " dec"]
        );
    }

    #[test]
    fn versions() {
        assert_eq!(
            sorted(
                &[
                    "a-1.10",
                    "a-1.2",
                    "a-1.2a",
                    "a-1.2~rc",
                    ".hidden",
                    "file.tar.gz",
                    "file.txt",
                    "file1.txt"
                ],
                &[],
                "V"
            ),
            [
                ".hidden",
                "a-1.2~rc",
                "a-1.2",
                "a-1.2a",
                "a-1.10",
                "file.tar.gz",
                "file.txt",
                "file1.txt"
            ]
        );
        assert_eq!(suffix_len(b"file.tar.gz"), 7);
        assert_eq!(suffix_len(b"a-1.2"), 0);
        assert_eq!(suffix_len(b"x.1a"), 0);
    }

    #[test]
    fn keys() {
        // A key with options does not inherit the global ones; -r still reverses the
        // last resort.
        assert_eq!(
            sorted(&["1 b", "10 a", "2 c"], &["1b"], "n"),
            ["1 b", "10 a", "2 c"]
        );
        assert_eq!(
            sorted(&["1 b", "10 a", "2 c"], &["1,1"], "n"),
            ["1 b", "2 c", "10 a"]
        );
        assert_eq!(
            sorted(&["b 2", "a 2", "c 1"], &["2,2n"], "r"),
            ["c 1", "b 2", "a 2"]
        );
        assert_eq!(
            sorted(&["b 2", "a 2", "c 1"], &["2,2nr"], ""),
            ["a 2", "b 2", "c 1"]
        );
        assert_eq!(sorted(&["a b", "a  a"], &["2"], ""), ["a  a", "a b"]);
        assert_eq!(
            sorted(&["x 1.b", "y 1.a"], &["2.3"], ""),
            ["y 1.a", "x 1.b"]
        );
        assert!(parse_key(b"0", &Order::default(), false).is_err());
        assert!(parse_key(b"1.0", &Order::default(), false).is_err());
        assert!(parse_key(b"1x", &Order::default(), false).is_err());
        assert!(parse_key(b"2,1.0", &Order::default(), false).is_ok());
    }
}
