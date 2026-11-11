//! Date leaf intrinsics (ECMA-262 21.4): time value arithmetic, formatting and parsing.
//! The algorithms follow QuickJS (quickjs.c `get_date_fields`, `set_date_fields`,
//! `get_date_string`, `js_Date_parse`), so output and parsing match it. Local time is a
//! fixed offset the host sets (D1383).
use crate::PushOut;

use alloc::string::String;

const MONTH_DAYS: [i64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
const MONTH_NAMES: &[u8] = b"JanFebMarAprMayJunJulAugSepOctNovDec";
const DAY_NAMES: &[u8] = b"SunMonTueWedThuFriSat";

fn math_mod(a: i64, b: i64) -> i64 {
    let m = a % b;
    m + (m < 0) as i64 * b
}

fn floor_div(a: i64, b: i64) -> i64 {
    let m = a % b;
    (a - (m + (m < 0) as i64 * b)) / b
}

fn days_from_year(y: i64) -> i64 {
    365 * (y - 1970) + floor_div(y - 1969, 4) - floor_div(y - 1901, 100) + floor_div(y - 1601, 400)
}

fn days_in_year(y: i64) -> i64 {
    365 + (y % 4 == 0) as i64 - (y % 100 == 0) as i64 + (y % 400 == 0) as i64
}

/// The year of day number `days`; `days` becomes the day within that year.
fn year_from_days(days: &mut i64) -> i64 {
    let d = *days;
    let mut y = floor_div(d * 10000, 3652425) + 1970;
    loop {
        let d1 = d - days_from_year(y);
        if d1 < 0 {
            y -= 1;
        } else if d1 < days_in_year(y) {
            *days = d1;
            return y;
        } else {
            y += 1;
        }
    }
}

/// TimeClip.
pub(crate) fn time_clip(t: f64) -> f64 {
    if (-8.64e15..=8.64e15).contains(&t) {
        djs_libm::trunc(t) + 0.0
    } else {
        f64::NAN
    }
}

/// The fields of time value `t`: year, month, date, hours, minutes, seconds, ms,
/// weekday, and the offset applied (minutes). `None` for NaN (unless `force`, which
/// gives the fields of 0).
pub(crate) fn fields(t: f64, local: bool, offset: i64, force: bool) -> Option<[f64; 9]> {
    let (mut d, mut tz) = (0i64, 0i64);
    if t.is_nan() {
        if !force {
            return None;
        }
    } else {
        d = t as i64;
        if local {
            tz = offset;
            d += tz * 60000;
        }
    }
    let mut h = math_mod(d, 86400000);
    let mut days = (d - h) / 86400000;
    let ms = h % 1000;
    h = (h - ms) / 1000;
    let s = h % 60;
    h = (h - s) / 60;
    let m = h % 60;
    h = (h - m) / 60;
    let wd = math_mod(days + 4, 7);
    let y = year_from_days(&mut days);
    let mut i = 0;
    while i < 11 {
        let mut md = MONTH_DAYS[i];
        if i == 1 {
            md += days_in_year(y) - 365;
        }
        if days < md {
            break;
        }
        days -= md;
        i += 1;
    }
    Some([
        y as f64,
        i as f64,
        (days + 1) as f64,
        h as f64,
        m as f64,
        s as f64,
        ms as f64,
        wd as f64,
        tz as f64,
    ])
}

/// MakeDay/MakeTime/MakeDate of 7 fields, then local → UTC and TimeClip.
pub(crate) fn from_fields(f: &[f64; 7], local: bool, offset: i64) -> f64 {
    let (y, m, dt) = (f[0], f[1], f[2]);
    let ym = y + djs_libm::floor(m / 12.0);
    let mut mn = djs_libm::fmod(m, 12.0);
    if mn < 0.0 {
        mn += 12.0;
    }
    if !(-271821.0..=275760.0).contains(&ym) {
        return f64::NAN;
    }
    let (yi, mi) = (ym as i64, mn as usize);
    let mut days = days_from_year(yi);
    for (i, md) in MONTH_DAYS.iter().enumerate().take(mi) {
        days += md;
        if i == 1 {
            days += days_in_year(yi) - 365;
        }
    }
    let day = days as f64 + dt - 1.0;
    let mut time = f[3] * 3600000.0;
    time += f[4] * 60000.0;
    time += f[5] * 1000.0;
    time += f[6];
    let mut tv = day * 86400000.0 + time;
    if !tv.is_finite() {
        return f64::NAN;
    }
    if local {
        tv -= (offset * 60000) as f64;
    }
    time_clip(tv)
}

/// The constructor's and Date.UTC's form: every field finite and truncated, years
/// 0..=99 meaning 1900..=1999.
pub(crate) fn from_fields_checked(f: &mut [f64; 7], local: bool, offset: i64) -> f64 {
    for (i, a) in f.iter_mut().enumerate() {
        if !a.is_finite() {
            return f64::NAN;
        }
        *a = djs_libm::trunc(*a);
        if i == 0 && *a >= 0.0 && *a < 100.0 {
            *a += 1900.0;
        }
    }
    from_fields(f, local, offset)
}

/// A setter (setMinutes and friends, as qjs's `set_date_field`): fields `first ..
/// first + args.len()` of `t` (local or UTC) replaced by `args` (already ToNumber).
pub(crate) fn set_fields(t: f64, first: usize, args: &[f64], local: bool, offset: i64) -> f64 {
    let Some(mut f) = fields(t, local, offset, first == 0) else {
        return f64::NAN;
    };
    if t.is_nan() && first != 0 {
        return f64::NAN;
    }
    let mut ok = true;
    for (i, &a) in args.iter().enumerate() {
        ok &= a.is_finite();
        f[first + i] = djs_libm::trunc(a);
    }
    if !ok || args.is_empty() {
        return f64::NAN;
    }
    let f7 = [f[0], f[1], f[2], f[3], f[4], f[5], f[6]];
    from_fields(&f7, local, offset)
}

fn name(list: &[u8], i: usize) -> &str {
    core::str::from_utf8(&list[i * 3..i * 3 + 3]).unwrap()
}

/// Formats `t` as qjs does. fmt: 0 toUTCString, 1 toString, 2 toISOString, 3 toLocale*;
/// part: 1 date, 2 time, 3 both. `None` for NaN.
pub(crate) fn format(t: f64, fmt: u8, part: u8, offset: i64) -> Option<String> {
    let f = fields(t, fmt == 1 || fmt == 3, offset, false)?;
    let (y, mon, d, h, m, s, ms, wd) = (
        f[0] as i64,
        f[1] as usize,
        f[2] as i64,
        f[3] as i64,
        f[4] as i64,
        f[5] as i64,
        f[6] as i64,
        f[7] as usize,
    );
    let mut tz = f[8] as i64;
    let year = |y: i64| {
        if y < 0 {
            crate::text::cat!("-", crate::text::pad(-y, 4))
        } else {
            crate::text::cat!(crate::text::pad(y, 4))
        }
    };
    let mut out = String::new();
    if part & 1 != 0 {
        match fmt {
            0 => {
                crate::text::cat_to!(
                    &mut out,
                    name(DAY_NAMES, wd),
                    ", ",
                    crate::text::pad(d, 2),
                    " ",
                    name(MONTH_NAMES, mon),
                    " ",
                    year(y),
                    " "
                )
            }
            1 => {
                crate::text::cat_to!(
                    &mut out,
                    name(DAY_NAMES, wd),
                    " ",
                    name(MONTH_NAMES, mon),
                    " ",
                    crate::text::pad(d, 2),
                    " ",
                    year(y)
                );
                if part == 3 {
                    out.push_out(' ');
                }
            }
            2 => {
                if (0..=9999).contains(&y) {
                    crate::text::cat_to!(&mut out, crate::text::pad(y, 4));
                } else if y < 0 {
                    crate::text::cat_to!(&mut out, "-", crate::text::pad(-y, 6));
                } else {
                    crate::text::cat_to!(&mut out, "+", crate::text::pad(y, 6));
                }
                crate::text::cat_to!(
                    &mut out,
                    "-",
                    crate::text::pad(mon as i64 + 1, 2),
                    "-",
                    crate::text::pad(d, 2),
                    "T"
                );
            }
            _ => {
                crate::text::cat_to!(
                    &mut out,
                    crate::text::pad(mon as i64 + 1, 2),
                    "/",
                    crate::text::pad(d, 2),
                    "/",
                    year(y)
                );
                if part == 3 {
                    out += ", ";
                }
            }
        }
    }
    if part & 2 != 0 {
        match fmt {
            0 => crate::text::cat_to!(
                &mut out,
                crate::text::pad(h, 2),
                ":",
                crate::text::pad(m, 2),
                ":",
                crate::text::pad(s, 2),
                " GMT"
            ),
            1 => {
                crate::text::cat_to!(
                    &mut out,
                    crate::text::pad(h, 2),
                    ":",
                    crate::text::pad(m, 2),
                    ":",
                    crate::text::pad(s, 2),
                    " GMT"
                );
                if tz < 0 {
                    out.push_out('-');
                    tz = -tz;
                } else {
                    out.push_out('+');
                }
                crate::text::cat_to!(
                    &mut out,
                    crate::text::pad(tz / 60, 2),
                    crate::text::pad(tz % 60, 2)
                );
            }
            2 => crate::text::cat_to!(
                &mut out,
                crate::text::pad(h, 2),
                ":",
                crate::text::pad(m, 2),
                ":",
                crate::text::pad(s, 2),
                ".",
                crate::text::pad(ms, 3),
                "Z"
            ),
            _ => {
                crate::text::cat_to!(
                    &mut out,
                    crate::text::pad((h + 11) % 12 + 1, 2),
                    ":",
                    crate::text::pad(m, 2),
                    ":",
                    crate::text::pad(s, 2),
                    " ",
                    if h < 12 { 'A' } else { 'P' },
                    "M"
                )
            }
        }
    }
    Some(out)
}

// ---- Date.parse (qjs's js_Date_parse) ---------------------------------------------------

struct P<'a> {
    s: &'a [u8],
    p: usize,
}

impl P<'_> {
    fn at(&self, i: usize) -> u8 {
        self.s.get(i).copied().unwrap_or(0)
    }
    fn cur(&self) -> u8 {
        self.at(self.p)
    }
    fn skip_char(&mut self, c: u8) -> bool {
        if self.cur() == c {
            self.p += 1;
            true
        } else {
            false
        }
    }
    fn skip_spaces(&mut self) -> u8 {
        while self.cur() == b' ' {
            self.p += 1;
        }
        self.cur()
    }
    fn skip_separators(&mut self) {
        while matches!(self.cur(), b'-' | b'/' | b'.' | b',') {
            self.p += 1;
        }
    }
    fn skip_until(&mut self, stop: &[u8]) {
        while !stop.contains(&self.cur()) && self.cur() != 0 {
            self.p += 1;
        }
    }
    fn digits(&mut self, min: usize, max: usize) -> Option<i64> {
        let start = self.p;
        let mut p = self.p;
        let mut v: i64 = 0;
        while self.at(p).is_ascii_digit() {
            if v >= 100000000 {
                return None;
            }
            v = v * 10 + (self.at(p) - b'0') as i64;
            p += 1;
            if p - start == max {
                break;
            }
        }
        if p - start < min {
            return None;
        }
        self.p = p;
        Some(v)
    }
    fn milliseconds(&mut self) -> Option<i64> {
        let c = self.cur();
        if c != b'.' && c != b',' {
            return None;
        }
        let mut p = self.p + 1;
        let start = p;
        let (mut mul, mut ms) = (100, 0);
        while self.at(p).is_ascii_digit() {
            ms += (self.at(p) - b'0') as i64 * mul;
            mul /= 10;
            p += 1;
            if p - start == 9 {
                break;
            }
        }
        if p > start {
            self.p = p;
            Some(ms)
        } else {
            None
        }
    }
    fn tzoffset(&mut self, strict: bool) -> Option<i64> {
        let save = self.p;
        let sgn = self.cur();
        self.p += 1;
        let r = (|| {
            if sgn == b'+' || sgn == b'-' {
                let n0 = self.p;
                let mut hh = self.digits(1, 0)?;
                let mut n = self.p - n0;
                if strict && n != 2 && n != 4 {
                    return None;
                }
                while n > 4 {
                    n -= 2;
                    hh /= 100;
                }
                let mm;
                if n > 2 {
                    mm = hh % 100;
                    hh /= 100;
                } else if self.skip_char(b':') {
                    mm = self.digits(2, 2)?;
                } else {
                    if strict {
                        return None;
                    }
                    mm = 0;
                }
                if hh > 23 || mm > 59 {
                    return None;
                }
                let tz = hh * 60 + mm;
                Some(if sgn == b'+' { tz } else { -tz })
            } else if sgn == b'Z' {
                Some(0)
            } else {
                None
            }
        })();
        if r.is_none() {
            self.p = save;
        }
        r
    }
    fn matches(&mut self, word: &str) -> bool {
        let w = word.as_bytes();
        for (i, &c) in w.iter().enumerate() {
            if !self.at(self.p + i).eq_ignore_ascii_case(&c) {
                return false;
            }
        }
        self.p += w.len();
        true
    }
    fn month(&mut self) -> Option<i64> {
        for n in 0..12 {
            let abbr = &MONTH_NAMES[n * 3..n * 3 + 3];
            if (0..3).all(|i| self.at(self.p + i).eq_ignore_ascii_case(&abbr[i])) {
                self.p += 3;
                return Some(n as i64 + 1);
            }
        }
        None
    }
}

const TZ_ABBR: &[(&str, i64)] = &[
    ("GMT", 0),
    ("UTC", 0),
    ("UT", 0),
    ("Z", 0),
    ("EDT", -4 * 60),
    ("EST", -5 * 60),
    ("CDT", -5 * 60),
    ("CST", -6 * 60),
    ("MDT", -6 * 60),
    ("MST", -7 * 60),
    ("PDT", -7 * 60),
    ("PST", -8 * 60),
    ("WET", 0),
    ("WEST", 60),
    ("CET", 60),
    ("CEST", 120),
    ("EET", 120),
    ("EEST", 180),
];

/// The ISO format (toISOString's and its date-only forms).
fn parse_iso(s: &[u8], f: &mut [i64; 9], local: &mut bool) -> bool {
    *f = [0, 0, 1, 0, 0, 0, 0, 0, 0];
    *local = false;
    let mut p = P { s, p: 0 };
    let sgn = p.cur();
    if sgn == b'-' || sgn == b'+' {
        p.p += 1;
        let Some(y) = p.digits(6, 6) else {
            return false;
        };
        if sgn == b'-' && y == 0 {
            return false;
        }
        f[0] = if sgn == b'-' { -y } else { y };
    } else {
        let Some(y) = p.digits(4, 4) else {
            return false;
        };
        f[0] = y;
    }
    if p.skip_char(b'-') {
        match p.digits(2, 2) {
            Some(m) if m >= 1 => f[1] = m - 1,
            _ => return false,
        }
        if p.skip_char(b'-') {
            match p.digits(2, 2) {
                Some(d) if d >= 1 => f[2] = d,
                _ => return false,
            }
        }
    }
    if p.skip_char(b'T') {
        *local = true;
        let hm = (|| {
            let h = p.digits(2, 2)?;
            if !p.skip_char(b':') {
                return None;
            }
            Some((h, p.digits(2, 2)?))
        })();
        let Some((h, m)) = hm else {
            f[3] = 100; // rejected below
            return true;
        };
        f[3] = h;
        f[4] = m;
        if p.skip_char(b':') {
            let Some(sec) = p.digits(2, 2) else {
                return false;
            };
            f[5] = sec;
            if let Some(ms) = p.milliseconds() {
                f[6] = ms;
            }
        }
    }
    if p.cur() != 0 {
        *local = false;
        match p.tzoffset(true) {
            Some(tz) => f[8] = tz,
            None => return false,
        }
    }
    p.p >= s.len()
}

/// toString, toUTCString and the other legacy formats.
fn parse_other(s: &[u8], f: &mut [i64; 9], local: &mut bool) -> bool {
    *f = [2001, 1, 1, 0, 0, 0, 0, 0, 0];
    *local = true;
    let mut p = P { s, p: 0 };
    let mut num = [0i64; 3];
    let (mut has_year, mut has_mon, mut has_time) = (false, false, false);
    let mut num_index = 0;
    loop {
        let c = p.skip_spaces();
        if c == 0 {
            break;
        }
        let start = p.p;
        if c == b'+' || c == b'-' {
            if has_time && let Some(tz) = p.tzoffset(false) {
                f[8] = tz;
                *local = false;
            } else {
                p.p += 1;
                if let Some(mut v) = p.digits(1, 0) {
                    if c == b'-' {
                        if v == 0 {
                            return false;
                        }
                        v = -v;
                    }
                    f[0] = v;
                    has_year = true;
                }
            }
        } else if let Some(v) = p.digits(1, 0) {
            if p.skip_char(b':') {
                f[3] = v;
                let Some(m) = p.digits(1, 2) else {
                    return false;
                };
                f[4] = m;
                if p.skip_char(b':') {
                    let Some(sec) = p.digits(1, 2) else {
                        return false;
                    };
                    f[5] = sec;
                    if let Some(ms) = p.milliseconds() {
                        f[6] = ms;
                    }
                }
                has_time = true;
                if matches!(p.cur(), b'+' | b'-')
                    && let Some(tz) = p.tzoffset(false)
                {
                    f[8] = tz;
                    *local = false;
                }
            } else if p.p - start > 2 && !has_year {
                f[0] = v;
                has_year = true;
            } else if !(1..=31).contains(&v) && !has_year {
                f[0] = v + (v < 100) as i64 * 1900 + (v < 50) as i64 * 100;
                has_year = true;
            } else {
                if num_index == 3 {
                    return false;
                }
                num[num_index] = v;
                num_index += 1;
            }
        } else if let Some(m) = p.month() {
            f[1] = m;
            has_mon = true;
            p.skip_until(b"0123456789 -/(");
        } else if has_time && p.matches("PM") {
            if f[3] < 12 {
                f[3] += 12;
            }
            continue;
        } else if has_time && p.matches("AM") {
            if f[3] == 12 {
                f[3] -= 12;
            }
            continue;
        } else if let Some(&(_, off)) = TZ_ABBR.iter().find(|(n, _)| p.matches(n)) {
            f[8] = off;
            *local = false;
            continue;
        } else if c == b'(' {
            let mut level = 0;
            loop {
                let c = p.cur();
                if c == 0 {
                    break;
                }
                p.p += 1;
                level += (c == b'(') as i32;
                level -= (c == b')') as i32;
                if level == 0 {
                    break;
                }
            }
            if level > 0 {
                return false;
            }
        } else if c == b')' {
            return false;
        } else {
            if has_year || has_mon || has_time || num_index > 0 {
                return false;
            }
            p.skip_until(b" -/(");
        }
        p.skip_separators();
    }
    if num_index + has_year as usize + has_mon as usize > 3 {
        return false;
    }
    let century = |y: i64| y + (y < 100) as i64 * 1900 + (y < 50) as i64 * 100;
    match num_index {
        0 if !has_year => return false,
        0 => {}
        1 => {
            if has_mon {
                f[2] = num[0];
            } else {
                f[1] = num[0];
            }
        }
        2 => {
            if has_year {
                f[1] = num[0];
                f[2] = num[1];
            } else if has_mon {
                f[0] = century(num[1]);
                f[2] = num[0];
            } else {
                f[1] = num[0];
                f[2] = num[1];
            }
        }
        _ => {
            f[0] = century(num[2]);
            f[1] = num[0];
            f[2] = num[1];
        }
    }
    if f[1] < 1 || f[2] < 1 {
        return false;
    }
    f[1] -= 1;
    true
}

/// Date.parse of a string (its code units, as qjs maps them to bytes).
pub(crate) fn parse(units: impl Iterator<Item = u16>, offset: i64) -> f64 {
    let mut buf = alloc::vec::Vec::new();
    for c in units.take(127) {
        let c = c as u32;
        let b = if c != 0x2028 && c != 0x2029 && is_space(c) {
            b' '
        } else if c > 255 {
            if c == 0x2212 { b'-' } else { b'x' }
        } else {
            c as u8
        };
        buf.push_out(b);
    }
    let mut f = [0i64; 9];
    let mut local = false;
    if !(parse_iso(&buf, &mut f, &mut local) || parse_other(&buf, &mut f, &mut local)) {
        return f64::NAN;
    }
    let max = [0, 11, 31, 24, 59, 59];
    if (1..6).any(|i| f[i] > max[i]) {
        return f64::NAN;
    }
    if f[3] == 24 && (f[4] | f[5] | f[6]) != 0 {
        return f64::NAN;
    }
    let f7 = [
        f[0] as f64,
        f[1] as f64,
        f[2] as f64,
        f[3] as f64,
        f[4] as f64,
        f[5] as f64,
        f[6] as f64,
    ];
    from_fields(&f7, local, offset) - (f[8] * 60000) as f64
}

fn is_space(c: u32) -> bool {
    crate::unicode::is_js_whitespace(c) || crate::unicode::is_js_line_terminator(c)
}
