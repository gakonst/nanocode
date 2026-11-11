//! Dates for the file utilities: the civil calendar, local time through the host's
//! zone, GNU date strings for `touch -d` (a subset: ISO dates and times, `@SECONDS`,
//! month names, zones, relative items like `2 hours ago`) and `touch -t` stamps.
//! Written for psh after GNU's "Date input formats" manual and POSIX `touch`.

use psh_engine::util::UtilCtx;

pub const NS: i64 = 1_000_000_000;

/// Days since 1970-01-01 of a proleptic Gregorian date.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(m);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// The date of a day number.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// A broken-down time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tm {
    pub year: i64,
    pub mon: u32,
    pub day: u32,
    pub hour: u32,
    pub min: u32,
    pub sec: u32,
    pub nsec: u32,
    /// 0 = Sunday.
    pub wday: u32,
    /// 0-based day of the year.
    pub yday: u32,
    /// Seconds east of UTC.
    pub offset: i32,
}

/// `ns` since the epoch broken down with `offset`.
pub fn tm_at(ns: i64, offset: i32) -> Tm {
    let secs = ns.div_euclid(NS) + i64::from(offset);
    let nsec = ns.rem_euclid(NS) as u32;
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (year, mon, day) = civil_from_days(days);
    Tm {
        year,
        mon,
        day,
        hour: (rem / 3600) as u32,
        min: (rem / 60 % 60) as u32,
        sec: (rem % 60) as u32,
        nsec,
        wday: (days + 4).rem_euclid(7) as u32,
        yday: (days - days_from_civil(year, 1, 1)) as u32,
        offset,
    }
}

/// Local time of `ns` in the command's zone.
pub fn local(ctx: &UtilCtx<'_>, ns: i64) -> Tm {
    tm_at(ns, ctx.utc_offset(ns.div_euclid(NS)))
}

/// Seconds since the epoch of a wall-clock time; `offset` given, or the local zone's.
pub fn epoch_of(ctx: &UtilCtx<'_>, wall: i64, offset: Option<i32>) -> i64 {
    match offset {
        Some(o) => wall - i64::from(o),
        None => {
            let guess = wall - i64::from(ctx.utc_offset(wall));
            wall - i64::from(ctx.utc_offset(guess))
        }
    }
}

pub fn wall_secs(y: i64, m: u32, d: u32, h: u32, mi: u32, s: u32) -> i64 {
    days_from_civil(y, m, d) * 86400 + i64::from(h * 3600 + mi * 60 + s)
}

/// `touch -t [[CC]YY]MMDDhhmm[.ss]`, local time: ns since the epoch.
pub fn parse_stamp(ctx: &UtilCtx<'_>, s: &[u8], now_ns: i64) -> Option<i64> {
    let (main, secs) = match s.iter().position(|&b| b == b'.') {
        Some(i) => (&s[..i], Some(&s[i + 1..])),
        None => (s, None),
    };
    if !main.iter().all(u8::is_ascii_digit)
        || secs.is_some_and(|x| x.len() != 2 || !x.iter().all(u8::is_ascii_digit))
    {
        return None;
    }
    let num = |b: &[u8]| -> u32 {
        std::str::from_utf8(b)
            .ok()
            .and_then(|t| t.parse().ok())
            .unwrap_or(99)
    };
    let (year, rest) = match main.len() {
        8 => (local(ctx, now_ns).year, main),
        10 => {
            let yy = i64::from(num(&main[..2]));
            (if yy >= 69 { 1900 + yy } else { 2000 + yy }, &main[2..])
        }
        12 => (i64::from(num(&main[..4])), &main[4..]),
        _ => return None,
    };
    let (mo, d, h, mi) = (
        num(&rest[..2]),
        num(&rest[2..4]),
        num(&rest[4..6]),
        num(&rest[6..8]),
    );
    let sec = secs.map_or(0, num);
    if !(1..=12).contains(&mo)
        || d == 0
        || d > days_in_month(year, mo)
        || h > 23
        || mi > 59
        || sec > 60
    {
        return None;
    }
    let wall = wall_secs(year, mo, d, h, mi, sec.min(59));
    Some(epoch_of(ctx, wall, None) * NS)
}

const MONTHS: [&str; 12] = [
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];

fn month_named(w: &str) -> Option<u32> {
    if w.len() < 3 {
        return None;
    }
    let w = if w == "sept" { "sep" } else { w };
    MONTHS
        .iter()
        .position(|m| m.starts_with(w) && (w.len() == 3 || *m == w))
        .map(|i| i as u32 + 1)
}

/// A relative unit: (months, seconds) per unit.
fn unit(w: &str) -> Option<(i64, i64)> {
    let w = w.strip_suffix('s').filter(|s| !s.is_empty()).unwrap_or(w);
    Some(match w {
        "year" => (12, 0),
        "month" => (1, 0),
        "fortnight" => (0, 14 * 86400),
        "week" => (0, 7 * 86400),
        "day" => (0, 86400),
        "hour" => (0, 3600),
        "minute" | "min" => (0, 60),
        "second" | "sec" => (0, 1),
        _ => return None,
    })
}

#[derive(Default)]
struct Parsed {
    date: Option<(i64, u32, u32)>,
    time: Option<(u32, u32, u32, u32)>,
    zone: Option<i32>,
    months: i64,
    days: i64,
    secs: i64,
    /// A day of the week (0 = Sunday) and its ordinal: `this`/none 0, `next` 1, `last`
    /// -1, `2 monday` 2.
    weekday: Option<(u32, i64)>,
}

/// A day name or its abbreviation (`mon`, `tues`, `wednes`, `thur`, `thurs`).
fn weekday_named(w: &str) -> Option<u32> {
    const DAYS: [&str; 7] = [
        "sunday",
        "monday",
        "tuesday",
        "wednesday",
        "thursday",
        "friday",
        "saturday",
    ];
    let extra = ["tues", "wednes", "thur", "thurs"];
    DAYS.iter()
        .position(|d| {
            *d == w || (w.len() == 3 && d.starts_with(w)) || extra.contains(&w) && d.starts_with(w)
        })
        .map(|i| i as u32)
}

/// A zone abbreviation GNU's parser knows (a subset): its offset east of UTC.
fn zone_named(w: &str) -> Option<i32> {
    let h = 3600;
    Some(match w {
        "utc" | "gmt" | "ut" | "z" | "wet" => 0,
        "bst" | "west" | "cet" | "met" => h,
        "cest" | "mest" | "eet" => 2 * h,
        "eest" | "msk" => 3 * h,
        "jst" | "kst" => 9 * h,
        "edt" => -4 * h,
        "est" | "cdt" => -5 * h,
        "cst" | "mdt" => -6 * h,
        "mst" | "pdt" => -7 * h,
        "pst" => -8 * h,
        "akst" => -9 * h,
        "hst" => -10 * h,
        _ => return None,
    })
}

struct Lexer<'a> {
    s: &'a [u8],
    i: usize,
}

impl Lexer<'_> {
    fn skip_space(&mut self) {
        while self.i < self.s.len()
            && (self.s[self.i].is_ascii_whitespace() || self.s[self.i] == b',')
        {
            self.i += 1;
        }
    }
    fn digits(&mut self) -> Option<(u32, usize)> {
        let start = self.i;
        while self.i < self.s.len() && self.s[self.i].is_ascii_digit() {
            self.i += 1;
        }
        let n = &self.s[start..self.i];
        if n.is_empty() || n.len() > 9 {
            self.i = start;
            return None;
        }
        Some((std::str::from_utf8(n).ok()?.parse().ok()?, n.len()))
    }
    fn eat(&mut self, c: u8) -> bool {
        if self.s.get(self.i) == Some(&c) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn word(&mut self) -> String {
        let start = self.i;
        while self.i < self.s.len() && self.s[self.i].is_ascii_alphabetic() {
            self.i += 1;
        }
        String::from_utf8_lossy(&self.s[start..self.i]).to_ascii_lowercase()
    }
    fn peek_word(&self) -> String {
        let mut j = self.i;
        while j < self.s.len() && self.s[j].is_ascii_whitespace() {
            j += 1;
        }
        let start = j;
        while j < self.s.len() && self.s[j].is_ascii_alphabetic() {
            j += 1;
        }
        String::from_utf8_lossy(&self.s[start..j]).to_ascii_lowercase()
    }
}

/// A numeric zone right after a time or alone: `+0200`, `-05:30`, `+3`.
fn zone_number(lx: &mut Lexer<'_>) -> Option<i32> {
    let save = lx.i;
    let sign = match lx.s.get(lx.i) {
        Some(b'+') => 1,
        Some(b'-') => -1,
        _ => return None,
    };
    lx.i += 1;
    let Some((n, len)) = lx.digits() else {
        lx.i = save;
        return None;
    };
    let (h, m) = if lx.eat(b':') {
        let Some((m, 2)) = lx.digits() else {
            lx.i = save;
            return None;
        };
        (n, m)
    } else if len <= 2 {
        (n, 0)
    } else if len == 4 {
        (n / 100, n % 100)
    } else {
        lx.i = save;
        return None;
    };
    Some(sign * (h as i32 * 3600 + m as i32 * 60))
}

/// A time `HH:MM[:SS[.frac]]` at the cursor, with `am`/`pm` and a zone after it.
fn time_of_day(lx: &mut Lexer<'_>, p: &mut Parsed, h: u32) -> Option<()> {
    let (mi, _) = lx.digits()?;
    let (mut sec, mut nsec) = (0, 0);
    if lx.eat(b':') {
        sec = lx.digits()?.0;
        if lx.eat(b'.') || lx.eat(b',') {
            let start = lx.i;
            lx.digits()?;
            let frac = std::str::from_utf8(&lx.s[start..lx.i]).ok()?;
            let mut f = frac.to_string();
            f.truncate(9);
            while f.len() < 9 {
                f.push('0');
            }
            nsec = f.parse().ok()?;
        }
    }
    let mut h = h;
    let w = lx.peek_word();
    if w == "am" || w == "pm" {
        lx.skip_space();
        lx.word();
        if h == 0 || h > 12 {
            return None;
        }
        h = h % 12 + if w == "pm" { 12 } else { 0 };
    }
    if h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    p.time = Some((h, mi, sec.min(59), nsec));
    if lx.eat(b'z') || lx.eat(b'Z') {
        p.zone = Some(0);
    } else if let Some(z) = zone_number(lx) {
        p.zone = Some(z);
    }
    Some(())
}

/// A GNU date string (the subset above): ns since the epoch, relative to `now_ns`.
pub fn parse_date(ctx: &UtilCtx<'_>, s: &[u8], now_ns: i64) -> Option<i64> {
    let text = std::str::from_utf8(s).ok()?.trim();
    if let Some(n) = text.strip_prefix('@') {
        let (neg, n) = match n.strip_prefix('-') {
            Some(r) => (true, r),
            None => (false, n.strip_prefix('+').unwrap_or(n)),
        };
        let (whole, frac) = n.split_once(['.', ',']).unwrap_or((n, ""));
        if whole.is_empty()
            || !whole.bytes().all(|b| b.is_ascii_digit())
            || !frac.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        let mut f = frac.to_string();
        f.truncate(9);
        while f.len() < 9 {
            f.push('0');
        }
        let v = whole.parse::<i64>().ok()?.checked_mul(NS)? + f.parse::<i64>().ok()?;
        return Some(if neg { -v } else { v });
    }
    // `+ 2 weeks`: a sign apart from its number.
    let mut joined = Vec::with_capacity(text.len());
    let tb = text.as_bytes();
    let mut k = 0;
    while k < tb.len() {
        joined.push(tb[k]);
        if matches!(tb[k], b'+' | b'-') {
            let mut j = k + 1;
            while tb.get(j) == Some(&b' ') {
                j += 1;
            }
            if j > k + 1 && tb.get(j).is_some_and(u8::is_ascii_digit) {
                k = j;
                continue;
            }
        }
        k += 1;
    }
    let mut p = Parsed::default();
    let mut lx = Lexer { s: &joined, i: 0 };
    // The last relative item, for `ago`.
    let mut last_rel: Option<(i64, i64, i64)> = None;
    loop {
        lx.skip_space();
        if lx.i >= lx.s.len() {
            break;
        }
        let c = lx.s[lx.i];
        if c.is_ascii_digit()
            || ((c == b'+' || c == b'-') && lx.s.get(lx.i + 1).is_some_and(u8::is_ascii_digit))
        {
            // A zone after a date and time: `2024-01-01 10:00 +0200`.
            if (c == b'+' || c == b'-') && p.time.is_some() && p.zone.is_none() {
                let save = lx.i;
                if let Some(z) = zone_number(&mut lx)
                    && unit(&lx.peek_word()).is_none()
                {
                    p.zone = Some(z);
                    continue;
                }
                lx.i = save;
            }
            let sign = if lx.eat(b'-') {
                -1
            } else {
                lx.eat(b'+');
                1
            };
            let start = lx.i;
            let (n, len) = lx.digits()?;
            if sign == 1 && lx.s.get(lx.i) == Some(&b'-') && len == 4 && c != b'+' {
                // ISO 8601 date.
                lx.i += 1;
                let (m, _) = lx.digits()?;
                lx.eat(b'-').then_some(())?;
                let (d, _) = lx.digits()?;
                let y = i64::from(n);
                if !(1..=12).contains(&m) || d == 0 || d > days_in_month(y, m) {
                    return None;
                }
                p.date = Some((y, m, d));
                if lx.eat(b'T') || lx.eat(b't') {
                    let (h, _) = lx.digits()?;
                    lx.eat(b':').then_some(())?;
                    time_of_day(&mut lx, &mut p, h)?;
                }
                continue;
            }
            if sign == 1 && c != b'+' && lx.eat(b':') {
                time_of_day(&mut lx, &mut p, n)?;
                continue;
            }
            if sign == 1 && c != b'+' && lx.eat(b'/') {
                if len == 4 {
                    // YYYY/MM/DD.
                    let (m, _) = lx.digits()?;
                    lx.eat(b'/').then_some(())?;
                    let (d, _) = lx.digits()?;
                    let y = i64::from(n);
                    if !(1..=12).contains(&m) || d == 0 || d > days_in_month(y, m) {
                        return None;
                    }
                    p.date = Some((y, m, d));
                    continue;
                }
                // US date: MM/DD[/YYYY].
                let (d, _) = lx.digits()?;
                let y = if lx.eat(b'/') {
                    let (y, ylen) = lx.digits()?;
                    let y = i64::from(y);
                    if ylen == 2 {
                        if y >= 69 { 1900 + y } else { 2000 + y }
                    } else {
                        y
                    }
                } else {
                    local(ctx, now_ns).year
                };
                if !(1..=12).contains(&n) || d == 0 || d > days_in_month(y, n) {
                    return None;
                }
                p.date = Some((y, n, d));
                continue;
            }
            let w = lx.peek_word();
            if (w == "am" || w == "pm") && sign == 1 && c != b'+' {
                lx.skip_space();
                lx.word();
                if n == 0 || n > 12 {
                    return None;
                }
                p.time = Some((n % 12 + if w == "pm" { 12 } else { 0 }, 0, 0, 0));
                continue;
            }
            if let Some(mon) = month_named(&w).filter(|_| sign == 1 && c != b'+') {
                // `2 Jan 2024`.
                lx.skip_space();
                lx.word();
                let y = year_after(&mut lx).unwrap_or_else(|| local(ctx, now_ns).year);
                p.date = Some((y, mon, n));
                continue;
            }
            if let Some(wd) = weekday_named(&w).filter(|_| sign == 1 && c != b'+') {
                // `2 monday`.
                lx.skip_space();
                lx.word();
                p.weekday = Some((wd, i64::from(n)));
                continue;
            }
            if len == 8 && sign == 1 && c != b'+' && unit(&w).is_none() {
                // `20240115`.
                let (y, m, d) = (i64::from(n / 10000), n / 100 % 100, n % 100);
                if !(1..=12).contains(&m) || d == 0 || d > days_in_month(y, m) {
                    return None;
                }
                p.date = Some((y, m, d));
                continue;
            }
            if let Some((months, secs)) = unit(&w) {
                lx.skip_space();
                lx.word();
                let k = i64::from(n) * sign;
                let rel = (months * k, 0, secs * k);
                add_rel(&mut p, rel);
                last_rel = Some(rel);
                continue;
            }
            let _ = start;
            return None;
        }
        let w = lx.word();
        if w.is_empty() {
            return None;
        }
        if let Some(mon) = month_named(&w) {
            // `Jan 2 [2024]`.
            lx.skip_space();
            let (d, _) = lx.digits()?;
            let y = year_after(&mut lx).unwrap_or_else(|| local(ctx, now_ns).year);
            p.date = Some((y, mon, d));
            continue;
        }
        let rel = match w.as_str() {
            "now" | "today" => (0, 0, 0),
            "yesterday" => (0, -1, 0),
            "tomorrow" => (0, 1, 0),
            z if zone_named(z).is_some() => {
                p.zone = zone_named(z);
                continue;
            }
            d if weekday_named(d).is_some() => {
                p.weekday = weekday_named(d).map(|wd| (wd, 0));
                continue;
            }
            "ago" => {
                let (m, d, s) = last_rel.take()?;
                add_rel(&mut p, (-2 * m, -2 * d, -2 * s));
                continue;
            }
            "last" | "next" | "this" => {
                lx.skip_space();
                let u = lx.word();
                let k = match w.as_str() {
                    "last" => -1,
                    "next" => 1,
                    _ => 0,
                };
                if let Some(wd) = weekday_named(&u) {
                    p.weekday = Some((wd, k));
                    continue;
                }
                let (months, secs) = unit(&u)?;
                (months * k, 0, secs * k)
            }
            _ => {
                let (months, secs) = unit(&w)?;
                (months, 0, secs)
            }
        };
        add_rel(&mut p, rel);
        last_rel = Some(rel);
    }
    // Assemble: the date and time given, else now's; then the relative items.
    let now = local(ctx, now_ns);
    let base_off = p.zone;
    let (mut y, mut m, mut d) = p.date.unwrap_or((now.year, now.mon, now.day));
    // A day of the week moves a date that was not given (GNU ignores it otherwise).
    let mut dated = p.date.is_some();
    if let (Some((wd, ord)), None) = (p.weekday, p.date) {
        let ahead = i64::from((wd + 7 - now.wday) % 7);
        let shift = match ord {
            0 => ahead,
            n if n < 0 => ahead + 7 * n,
            n => ahead + 7 * (n - i64::from(ahead > 0)),
        };
        let (yy, mm, dd) = civil_from_days(days_from_civil(y, m, d) + shift);
        (y, m, d) = (yy, mm, dd);
        dated = true;
    }
    let (h, mi, s, ns) = match p.time {
        Some(t) => t,
        None if dated => (0, 0, 0, 0),
        None => (now.hour, now.min, now.sec, now.nsec),
    };
    if p.months != 0 {
        let total = y * 12 + i64::from(m - 1) + p.months;
        y = total.div_euclid(12);
        m = (total.rem_euclid(12) + 1) as u32;
    }
    // Day overflow after a month shift rolls into the next month, as GNU's mktime does.
    let day_num = days_from_civil(y, m, 1) + i64::from(d) - 1 + p.days;
    let (yy, mm, dd) = civil_from_days(day_num);
    (y, m, d) = (yy, mm, dd);
    let wall = wall_secs(y, m, d, h, mi, s);
    let secs = epoch_of(ctx, wall, base_off) + p.secs;
    secs.checked_mul(NS)?.checked_add(i64::from(ns))
}

/// A four-digit year after a month and day (`Jan 2 2024`), not a time (`Jan 2 10:00`).
fn year_after(lx: &mut Lexer<'_>) -> Option<i64> {
    let save = lx.i;
    lx.skip_space();
    match lx.digits() {
        Some((y, 4)) if lx.s.get(lx.i) != Some(&b':') => Some(i64::from(y)),
        _ => {
            lx.i = save;
            None
        }
    }
}

fn add_rel(p: &mut Parsed, (m, d, s): (i64, i64, i64)) {
    p.months += m;
    p.days += d;
    p.secs += s;
}

pub const DAY_NAMES: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
pub const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// The ISO 8601 week-based year and week of a broken-down time.
fn iso_week(tm: &Tm) -> (i64, i64) {
    let wday = i64::from((tm.wday + 6) % 7); // Monday = 0
    let yday = i64::from(tm.yday);
    let week = (yday - wday + 10) / 7;
    if week < 1 {
        let py = tm.year - 1;
        let pyday = yday + if is_leap(py) { 366 } else { 365 };
        return (py, (pyday - wday + 10) / 7);
    }
    let ydays = if is_leap(tm.year) { 366 } else { 365 };
    if week == 53 && yday - wday + 3 >= ydays {
        return (tm.year + 1, 1);
    }
    (tm.year, week)
}

/// `+HHMM` with `colons` colons (`%z`, `%:z`, `%::z`), or for 3 the shortest of `+HH`,
/// `+HH:MM`, `+HH:MM:SS` (`%:::z`).
fn numeric_zone(off: i32, colons: usize) -> String {
    let sign = if off < 0 { '-' } else { '+' };
    let a = off.unsigned_abs();
    let (h, m, s) = (a / 3600, a / 60 % 60, a % 60);
    match colons {
        0 => format!("{sign}{h:02}{m:02}"),
        1 => format!("{sign}{h:02}:{m:02}"),
        2 => format!("{sign}{h:02}:{m:02}:{s:02}"),
        _ if s != 0 => format!("{sign}{h:02}:{m:02}:{s:02}"),
        _ if m != 0 => format!("{sign}{h:02}:{m:02}"),
        _ => format!("{sign}{h:02}"),
    }
}

/// Formats a broken-down time like GNU `date +FORMAT` in the C locale: C's `strftime`
/// conversions plus `%N` (nanoseconds; a width truncates), `%q`, `%P`, `%:z %::z
/// %:::z`, `%s`, the flags `- _ 0 ^ #` and field widths. `abbr` is the zone's name
/// for `%Z`; `epoch` the seconds for `%s`.
pub fn format(fmt: &[u8], tm: &Tm, abbr: &str, epoch: i64) -> Vec<u8> {
    let mut out = Vec::new();
    format_into(&mut out, fmt, tm, abbr, epoch);
    out
}

/// `format` for a broken-down time alone (`ls --time-style`, `stat -t`): `%Z` is `UTC`
/// at offset 0, else the numeric offset; `%s` from the fields.
pub fn strftime(fmt: &[u8], t: &Tm) -> Vec<u8> {
    let epoch = wall_secs(t.year, t.mon, t.day, t.hour, t.min, t.sec) - i64::from(t.offset);
    let abbr = if t.offset == 0 {
        "UTC".to_string()
    } else {
        numeric_zone(t.offset, 0)
    };
    format(fmt, t, &abbr, epoch)
}

enum Field {
    Text(String),
    Num(i64, usize, u8),
    Sub(&'static [u8]),
}

fn format_into(out: &mut Vec<u8>, fmt: &[u8], tm: &Tm, abbr: &str, epoch: i64) {
    let mut i = 0;
    while i < fmt.len() {
        if fmt[i] != b'%' {
            out.push(fmt[i]);
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        let (mut pad_flag, mut upper, mut swap) = (None, false, false);
        while let Some(&f) = fmt.get(i) {
            match f {
                b'-' | b'_' | b'0' => pad_flag = Some(f),
                b'^' => upper = true,
                b'#' => swap = true,
                _ => break,
            }
            i += 1;
        }
        let wstart = i;
        while fmt.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        let width: Option<usize> = std::str::from_utf8(&fmt[wstart..i])
            .ok()
            .and_then(|w| w.parse().ok());
        let mut colons = 0;
        while fmt.get(i) == Some(&b':') {
            colons += 1;
            i += 1;
        }
        while matches!(fmt.get(i), Some(b'E' | b'O')) {
            i += 1;
        }
        let Some(&c) = fmt.get(i) else {
            out.extend_from_slice(&fmt[start..]);
            break;
        };
        i += 1;
        if colons > 0 && c != b'z' {
            out.extend_from_slice(&fmt[start..i]);
            continue;
        }
        let num = |v: i64, w: usize, pad: u8| Field::Num(v, w, pad);
        let hour12 = i64::from((tm.hour + 11) % 12 + 1);
        let field = match c {
            b'a' => Field::Text(DAY_NAMES[tm.wday as usize][..3].into()),
            b'A' => Field::Text(DAY_NAMES[tm.wday as usize].into()),
            b'b' | b'h' => Field::Text(MONTH_NAMES[tm.mon as usize - 1][..3].into()),
            b'B' => Field::Text(MONTH_NAMES[tm.mon as usize - 1].into()),
            b'c' => Field::Sub(b"%a %b %e %H:%M:%S %Y"),
            b'C' => num(tm.year.div_euclid(100), 2, b'0'),
            b'd' => num(i64::from(tm.day), 2, b'0'),
            b'D' | b'x' => Field::Sub(b"%m/%d/%y"),
            b'e' => num(i64::from(tm.day), 2, b' '),
            b'F' => Field::Sub(b"%Y-%m-%d"),
            b'g' => num(iso_week(tm).0.rem_euclid(100), 2, b'0'),
            b'G' => num(iso_week(tm).0, 1, b'0'),
            b'H' => num(i64::from(tm.hour), 2, b'0'),
            b'I' => num(hour12, 2, b'0'),
            b'j' => num(i64::from(tm.yday) + 1, 3, b'0'),
            b'k' => num(i64::from(tm.hour), 2, b' '),
            b'l' => num(hour12, 2, b' '),
            b'm' => num(i64::from(tm.mon), 2, b'0'),
            b'M' => num(i64::from(tm.min), 2, b'0'),
            b'n' => Field::Text("\n".into()),
            b'N' => {
                let digits = format!("{:09}", tm.nsec);
                let w = width.unwrap_or(9);
                let mut d = digits[..w.min(9)].to_string();
                while d.len() < w {
                    d.push('0');
                }
                out.extend_from_slice(d.as_bytes());
                continue;
            }
            b'p' => {
                let t = if tm.hour < 12 { "AM" } else { "PM" };
                Field::Text(if swap {
                    t.to_ascii_lowercase()
                } else {
                    t.into()
                })
            }
            b'P' => Field::Text(if tm.hour < 12 { "am" } else { "pm" }.into()),
            b'q' => num(i64::from((tm.mon - 1) / 3 + 1), 1, b'0'),
            b'r' => Field::Sub(b"%I:%M:%S %p"),
            b'R' => Field::Sub(b"%H:%M"),
            b's' => num(epoch, 1, b'0'),
            b'S' => num(i64::from(tm.sec), 2, b'0'),
            b't' => Field::Text("\t".into()),
            b'T' | b'X' => Field::Sub(b"%H:%M:%S"),
            b'u' => num(i64::from((tm.wday + 6) % 7 + 1), 1, b'0'),
            b'U' => num(i64::from((tm.yday + 7 - tm.wday) / 7), 2, b'0'),
            b'V' => num(iso_week(tm).1, 2, b'0'),
            b'w' => num(i64::from(tm.wday), 1, b'0'),
            b'W' => num(i64::from((tm.yday + 7 - (tm.wday + 6) % 7) / 7), 2, b'0'),
            b'y' => num(tm.year.rem_euclid(100), 2, b'0'),
            b'Y' => num(tm.year, 1, b'0'),
            b'z' => Field::Text(numeric_zone(tm.offset, colons)),
            b'Z' => Field::Text(if swap {
                abbr.to_ascii_lowercase()
            } else {
                abbr.to_string()
            }),
            b'%' => Field::Text("%".into()),
            _ => {
                out.extend_from_slice(&fmt[start..i]);
                continue;
            }
        };
        let text = match field {
            Field::Sub(f) => {
                let mut sub = Vec::new();
                format_into(&mut sub, f, tm, abbr, epoch);
                String::from_utf8_lossy(&sub).into_owned()
            }
            Field::Text(t) => t,
            Field::Num(v, w, pad) => {
                let w = width.unwrap_or(w);
                let pad = match pad_flag {
                    Some(b'-') => None,
                    Some(b'_') => Some(b' '),
                    Some(b'0') => Some(b'0'),
                    _ => Some(pad),
                };
                let digits = v.unsigned_abs().to_string();
                let sign = if v < 0 { "-" } else { "" };
                let fill = pad.map_or(0, |_| w.saturating_sub(sign.len() + digits.len()));
                let s = match pad {
                    Some(b'0') => format!("{sign}{}{digits}", "0".repeat(fill)),
                    _ => format!("{}{sign}{digits}", " ".repeat(fill)),
                };
                out.extend_from_slice(s.as_bytes());
                continue;
            }
        };
        let text = if upper || (swap && !matches!(c, b'p' | b'Z')) {
            text.to_uppercase()
        } else {
            text
        };
        if let Some(w) = width
            && pad_flag != Some(b'-')
        {
            let pad = if pad_flag == Some(b'0') { b'0' } else { b' ' };
            for _ in text.len()..w {
                out.push(pad);
            }
        }
        out.extend_from_slice(text.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11017);
        assert_eq!(civil_from_days(11017), (2000, 3, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        let t = tm_at(1_700_000_000 * NS, 0);
        assert_eq!(
            (t.year, t.mon, t.day, t.hour, t.min, t.sec),
            (2023, 11, 14, 22, 13, 20)
        );
        assert_eq!((t.wday, t.yday), (2, 317));
    }
}
