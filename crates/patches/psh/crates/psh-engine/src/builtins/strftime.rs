//! `printf '%(fmt)T'`: time zones (TZif files and POSIX `TZ` strings) and `strftime`
//! in the C locale.
//!
//! Written from RFC 8536 (TZif), POSIX's description of `TZ` and the C library's
//! `strftime` documentation.

use crate::host::OpenOptions;
use crate::state::Vp;

/// What a zone says about one instant.
struct Local {
    /// Seconds east of UTC.
    utoff: i64,
    abbr: Vec<u8>,
}

/// A time zone: transitions from a TZif file and/or a POSIX rule for later times.
pub(crate) struct Zone {
    transitions: Vec<(i64, usize)>,
    types: Vec<(i64, bool, Vec<u8>)>,
    rule: Option<Rule>,
}

/// A POSIX `TZ` rule: `std offset [dst [offset] [,start[/time],end[/time]]]`.
struct Rule {
    std_name: Vec<u8>,
    std_off: i64,
    dst: Option<Dst>,
}

struct Dst {
    name: Vec<u8>,
    off: i64,
    start: (Date, i64),
    end: (Date, i64),
}

#[derive(Clone, Copy)]
enum Date {
    /// `Jn`: day 1..=365, February 29 never counted.
    Julian1(i64),
    /// `n`: day 0..=365.
    Julian0(i64),
    /// `Mm.w.d`
    Month(i64, i64, i64),
}

impl Zone {
    pub(crate) fn utc() -> Zone {
        Zone {
            transitions: Vec::new(),
            types: Vec::new(),
            rule: Some(Rule {
                std_name: b"UTC".to_vec(),
                std_off: 0,
                dst: None,
            }),
        }
    }

    fn at(&self, t: i64) -> Local {
        let last = self.transitions.last().map(|l| l.0);
        if let Some(rule) = &self.rule
            && (self.transitions.is_empty() || last.is_some_and(|l| t >= l))
        {
            return rule.at(t);
        }
        let idx = match self.transitions.iter().rposition(|&(at, _)| at <= t) {
            Some(i) => self.transitions[i].1,
            None => 0,
        };
        match self.types.get(idx) {
            Some((off, _, abbr)) => Local {
                utoff: *off,
                abbr: abbr.clone(),
            },
            None => Local {
                utoff: 0,
                abbr: b"UTC".to_vec(),
            },
        }
    }
}

impl Rule {
    fn at(&self, t: i64) -> Local {
        let std = Local {
            utoff: self.std_off,
            abbr: self.std_name.clone(),
        };
        let Some(d) = &self.dst else {
            return std;
        };
        let (year, _, _) = civil(days_floor(t + self.std_off));
        let start = day_of(d.start.0, year) * 86400 + d.start.1 - self.std_off;
        let end = day_of(d.end.0, year) * 86400 + d.end.1 - d.off;
        let in_dst = if start < end {
            t >= start && t < end
        } else {
            !(t >= end && t < start)
        };
        if in_dst {
            Local {
                utoff: d.off,
                abbr: d.name.clone(),
            }
        } else {
            std
        }
    }
}

/// Days since the epoch of a rule date in `year`.
fn day_of(d: Date, year: i64) -> i64 {
    let jan1 = days_from_civil(year, 1, 1);
    match d {
        Date::Julian1(n) => {
            let leap = is_leap(year) && n >= 60;
            jan1 + n - 1 + i64::from(leap)
        }
        Date::Julian0(n) => jan1 + n,
        Date::Month(m, w, wd) => {
            let first = days_from_civil(year, m, 1);
            let first_wd = (first + 4).rem_euclid(7);
            let mut day = first + (wd - first_wd).rem_euclid(7) + (w - 1) * 7;
            let len = month_len(year, m);
            while day >= first + len {
                day -= 7;
            }
            day
        }
    }
}

/// Parses a POSIX `TZ` string.
fn parse_rule(s: &[u8]) -> Option<Rule> {
    let mut p = 0;
    let std_name = parse_name(s, &mut p)?;
    let std_off = -parse_offset(s, &mut p)?;
    if p == s.len() {
        return Some(Rule {
            std_name,
            std_off,
            dst: None,
        });
    }
    let name = parse_name(s, &mut p)?;
    let off = if p < s.len() && s[p] != b',' {
        -parse_offset(s, &mut p)?
    } else {
        std_off + 3600
    };
    // Without rules: the US rules (what C libraries assume).
    let (start, end) = if p < s.len() && s[p] == b',' {
        p += 1;
        let start = parse_date(s, &mut p)?;
        if s.get(p) != Some(&b',') {
            return None;
        }
        p += 1;
        let end = parse_date(s, &mut p)?;
        (start, end)
    } else {
        ((Date::Month(3, 2, 0), 7200), (Date::Month(11, 1, 0), 7200))
    };
    (p == s.len()).then_some(Rule {
        std_name,
        std_off,
        dst: Some(Dst {
            name,
            off,
            start,
            end,
        }),
    })
}

fn parse_name(s: &[u8], p: &mut usize) -> Option<Vec<u8>> {
    if s.get(*p) == Some(&b'<') {
        let end = s[*p..].iter().position(|&b| b == b'>')? + *p;
        let name = s[*p + 1..end].to_vec();
        *p = end + 1;
        return Some(name);
    }
    let start = *p;
    while *p < s.len() && s[*p].is_ascii_alphabetic() {
        *p += 1;
    }
    (*p - start >= 3).then(|| s[start..*p].to_vec())
}

/// `[+-]hh[:mm[:ss]]` in seconds (positive west of Greenwich, as written).
fn parse_offset(s: &[u8], p: &mut usize) -> Option<i64> {
    let neg = match s.get(*p) {
        Some(b'-') => {
            *p += 1;
            true
        }
        Some(b'+') => {
            *p += 1;
            false
        }
        _ => false,
    };
    let mut total = 0;
    for (k, mult) in [3600, 60, 1].into_iter().enumerate() {
        if k > 0 {
            if s.get(*p) != Some(&b':') {
                break;
            }
            *p += 1;
        }
        let start = *p;
        while *p < s.len() && s[*p].is_ascii_digit() {
            *p += 1;
        }
        if *p == start {
            return None;
        }
        let n: i64 = std::str::from_utf8(&s[start..*p]).ok()?.parse().ok()?;
        total += n * mult;
    }
    Some(if neg { -total } else { total })
}

fn parse_num(s: &[u8], p: &mut usize) -> Option<i64> {
    let start = *p;
    while *p < s.len() && s[*p].is_ascii_digit() {
        *p += 1;
    }
    std::str::from_utf8(&s[start..*p]).ok()?.parse().ok()
}

fn parse_date(s: &[u8], p: &mut usize) -> Option<(Date, i64)> {
    let date = match s.get(*p) {
        Some(b'J') => {
            *p += 1;
            Date::Julian1(parse_num(s, p)?)
        }
        Some(b'M') => {
            *p += 1;
            let m = parse_num(s, p)?;
            if s.get(*p) != Some(&b'.') {
                return None;
            }
            *p += 1;
            let w = parse_num(s, p)?;
            if s.get(*p) != Some(&b'.') {
                return None;
            }
            *p += 1;
            Date::Month(m, w, parse_num(s, p)?)
        }
        _ => Date::Julian0(parse_num(s, p)?),
    };
    let time = if s.get(*p) == Some(&b'/') {
        *p += 1;
        parse_offset(s, p)?
    } else {
        7200
    };
    Some((date, time))
}

/// Parses a TZif file (RFC 8536), version 1 or the 64-bit data of later versions.
fn parse_tzif(d: &[u8]) -> Option<Zone> {
    fn be32(d: &[u8], at: usize) -> Option<i64> {
        d.get(at..at + 4)
            .map(|b| i64::from(i32::from_be_bytes([b[0], b[1], b[2], b[3]])))
    }
    if d.get(..4) != Some(b"TZif") {
        return None;
    }
    let version = d[4];
    let counts = |at: usize| -> Option<[usize; 6]> {
        let mut c = [0usize; 6];
        for (k, v) in c.iter_mut().enumerate() {
            *v = usize::try_from(be32(d, at + 20 + 4 * k)?).ok()?;
        }
        Some(c)
    };
    let [isut, isstd, leap, time, typ, chars] = counts(0)?;
    let mut at = 0;
    let mut tsize = 4;
    if version >= b'2' {
        // Skip the 32-bit block to the 64-bit one.
        at = 44 + time * 5 + typ * 6 + chars + leap * 8 + isstd + isut;
        tsize = 8;
    }
    let [isut, isstd, leap, time, typ, chars] = counts(at)?;
    let mut p = at + 44;
    let mut transitions = Vec::with_capacity(time);
    for k in 0..time {
        let t = if tsize == 8 {
            let b = d.get(p + 8 * k..p + 8 * k + 8)?;
            i64::from_be_bytes(b.try_into().ok()?)
        } else {
            be32(d, p + 4 * k)?
        };
        transitions.push((t, 0));
    }
    p += time * tsize;
    for (k, tr) in transitions.iter_mut().enumerate() {
        tr.1 = usize::from(*d.get(p + k)?);
    }
    p += time;
    let abbr_at = p + typ * 6;
    let mut types = Vec::with_capacity(typ);
    for k in 0..typ {
        let off = be32(d, p + 6 * k)?;
        let dst = *d.get(p + 6 * k + 4)? != 0;
        let idx = usize::from(*d.get(p + 6 * k + 5)?);
        let abbr = d.get(abbr_at + idx..abbr_at + chars)?;
        let end = abbr.iter().position(|&b| b == 0).unwrap_or(abbr.len());
        types.push((off, dst, abbr[..end].to_vec()));
    }
    p = abbr_at + chars + leap * (tsize + 4) + isstd + isut;
    let rule = if version >= b'2' && d.get(p) == Some(&b'\n') {
        let rest = &d[p + 1..];
        let end = rest.iter().position(|&b| b == b'\n').unwrap_or(rest.len());
        parse_rule(&rest[..end])
    } else {
        None
    };
    Some(Zone {
        transitions,
        types,
        rule,
    })
}

/// The zone `printf %(…)T` uses: an exported `TZ` (a zone file name, a path, or a
/// POSIX rule), else the system's local time zone; UTC when none can be read.
pub(crate) async fn zone(vp: &Vp) -> Zone {
    let tz = vp
        .vars
        .get("TZ")
        .filter(|v| v.exported)
        .and_then(|v| v.get().map(<[u8]>::to_vec));
    load(vp.host(), tz.as_deref())
        .await
        .unwrap_or_else(Zone::utc)
}

/// The zone a `TZ` value names (`None`: unset, the system's `/etc/localtime`), or `None`
/// when nothing could be read (no zone files, as in wasm, and no POSIX rule).
pub(crate) async fn load(host: &dyn crate::Host, tz: Option<&[u8]>) -> Option<Zone> {
    let files: Vec<Vec<u8>> = match tz {
        None => vec![b"/etc/localtime".to_vec()],
        Some(t) => {
            let t = t.strip_prefix(b":").unwrap_or(t);
            if t.is_empty() {
                return Some(Zone::utc());
            }
            if t.first() == Some(&b'/') {
                vec![t.to_vec()]
            } else if t.contains(&b'.') && t.windows(2).any(|w| w == b"..") {
                Vec::new()
            } else {
                [
                    "/usr/share/zoneinfo/",
                    "/usr/lib/zoneinfo/",
                    "/usr/share/lib/zoneinfo/",
                ]
                .iter()
                .map(|d| {
                    let mut p = d.as_bytes().to_vec();
                    p.extend_from_slice(t);
                    p
                })
                .collect()
            }
        }
    };
    for f in files {
        if let Ok(file) = host.open(&f, OpenOptions::read()).await {
            let obj = crate::fd::FdObj::File(file);
            let mut data = Vec::new();
            if obj.read_to_end(&mut data).await.is_ok()
                && let Some(z) = parse_tzif(&data)
            {
                return Some(z);
            }
        }
    }
    if let Some(t) = tz
        && let Some(rule) = parse_rule(t.strip_prefix(b":").unwrap_or(t))
    {
        return Some(Zone {
            transitions: Vec::new(),
            types: Vec::new(),
            rule: Some(rule),
        });
    }
    None
}

impl Zone {
    /// The offset east of UTC and the abbreviation at `t` (seconds since the epoch).
    pub(crate) fn local_at(&self, t: i64) -> (i64, Vec<u8>) {
        let l = self.at(t);
        (l.utoff, l.abbr)
    }
}

fn is_leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

fn month_len(y: i64, m: i64) -> i64 {
    match m {
        2 if is_leap(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn days_floor(t: i64) -> i64 {
    t.div_euclid(86400)
}

/// Days since 1970-01-01 of a civil date (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil date of a day number: (year, month 1..=12, day 1..=31).
fn civil(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

const DAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MONTHS: [&str; 12] = [
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

/// Broken-down local time.
struct Tm {
    t: i64,
    year: i64,
    mon: i64,
    mday: i64,
    hour: i64,
    min: i64,
    sec: i64,
    /// 0 = Sunday
    wday: i64,
    /// 0-based day of the year
    yday: i64,
    local: Local,
}

fn tm(zone: &Zone, t: i64) -> Tm {
    let local = zone.at(t);
    let lt = t + local.utoff;
    let days = days_floor(lt);
    let secs = lt - days * 86400;
    let (year, mon, mday) = civil(days);
    Tm {
        t,
        year,
        mon,
        mday,
        hour: secs / 3600,
        min: secs / 60 % 60,
        sec: secs % 60,
        wday: (days + 4).rem_euclid(7),
        yday: days - days_from_civil(year, 1, 1),
        local,
    }
}

/// ISO 8601 week-based year and week number.
fn iso_week(tm: &Tm) -> (i64, i64) {
    let wday = (tm.wday + 6) % 7; // Monday = 0
    let week = (tm.yday - wday + 10) / 7;
    if week < 1 {
        let py = tm.year - 1;
        let pdays = if is_leap(py) { 366 } else { 365 };
        let pyday = tm.yday + pdays;
        return (py, (pyday - wday + 10) / 7);
    }
    let ydays = if is_leap(tm.year) { 366 } else { 365 };
    if week == 53 && tm.yday - wday + 3 >= ydays {
        return (tm.year + 1, 1);
    }
    (tm.year, week)
}

/// Formats `t` (seconds since the epoch) in `zone` like C's `strftime` in the C
/// locale, with glibc's `-`, `_`, `0`, `^`, `#` flags and field widths.
pub(crate) fn strftime(fmt: &[u8], zone: &Zone, t: i64) -> Vec<u8> {
    let tm = tm(zone, t);
    let mut out = Vec::new();
    format_into(&mut out, fmt, &tm);
    out
}

fn format_into(out: &mut Vec<u8>, fmt: &[u8], tm: &Tm) {
    let mut i = 0;
    while i < fmt.len() {
        if fmt[i] != b'%' {
            out.push(fmt[i]);
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        let mut flag = None;
        let mut upper = false;
        while i < fmt.len() && b"-_0^#".contains(&fmt[i]) {
            match fmt[i] {
                b'^' | b'#' => upper = true,
                f => flag = Some(f),
            }
            i += 1;
        }
        let wstart = i;
        while i < fmt.len() && fmt[i].is_ascii_digit() {
            i += 1;
        }
        let width: Option<usize> = std::str::from_utf8(&fmt[wstart..i])
            .ok()
            .and_then(|w| w.parse().ok());
        // E and O modifiers change nothing in the C locale.
        while i < fmt.len() && (fmt[i] == b'E' || fmt[i] == b'O') {
            i += 1;
        }
        let Some(&c) = fmt.get(i) else {
            out.extend_from_slice(&fmt[start..]);
            break;
        };
        i += 1;
        let num = |v: i64, w: usize, pad: u8| Field::Num(v, w, pad);
        let field = match c {
            b'a' => Field::Text(DAYS[tm.wday as usize][..3].into()),
            b'A' => Field::Text(DAYS[tm.wday as usize].into()),
            b'b' | b'h' => Field::Text(MONTHS[tm.mon as usize - 1][..3].into()),
            b'B' => Field::Text(MONTHS[tm.mon as usize - 1].into()),
            b'c' => Field::Sub(b"%a %b %e %H:%M:%S %Y"),
            b'C' => num(tm.year.div_euclid(100), 2, b'0'),
            b'd' => num(tm.mday, 2, b'0'),
            b'D' | b'x' => Field::Sub(b"%m/%d/%y"),
            b'e' => num(tm.mday, 2, b' '),
            b'F' => Field::Sub(b"%Y-%m-%d"),
            b'g' => num(iso_week(tm).0.rem_euclid(100), 2, b'0'),
            b'G' => num(iso_week(tm).0, 1, b'0'),
            b'H' => num(tm.hour, 2, b'0'),
            b'I' => num((tm.hour + 11) % 12 + 1, 2, b'0'),
            b'j' => num(tm.yday + 1, 3, b'0'),
            b'k' => num(tm.hour, 2, b' '),
            b'l' => num((tm.hour + 11) % 12 + 1, 2, b' '),
            b'm' => num(tm.mon, 2, b'0'),
            b'M' => num(tm.min, 2, b'0'),
            b'n' => Field::Text("\n".into()),
            b'p' => Field::Text(if tm.hour < 12 { "AM" } else { "PM" }.into()),
            b'P' => Field::Text(if tm.hour < 12 { "am" } else { "pm" }.into()),
            b'r' => Field::Sub(b"%I:%M:%S %p"),
            b'R' => Field::Sub(b"%H:%M"),
            b's' => num(tm.t, 1, b'0'),
            b'S' => num(tm.sec, 2, b'0'),
            b't' => Field::Text("\t".into()),
            b'T' | b'X' => Field::Sub(b"%H:%M:%S"),
            b'u' => num((tm.wday + 6) % 7 + 1, 1, b'0'),
            b'U' => num((tm.yday + 7 - tm.wday) / 7, 2, b'0'),
            b'V' => num(iso_week(tm).1, 2, b'0'),
            b'w' => num(tm.wday, 1, b'0'),
            b'W' => num((tm.yday + 7 - (tm.wday + 6) % 7) / 7, 2, b'0'),
            b'y' => num(tm.year.rem_euclid(100), 2, b'0'),
            b'Y' => num(tm.year, 1, b'0'),
            b'z' => {
                let off = tm.local.utoff;
                let sign = if off < 0 { '-' } else { '+' };
                let a = off.abs();
                Field::Text(format!("{sign}{:02}{:02}", a / 3600, a / 60 % 60))
            }
            b'Z' => Field::Text(String::from_utf8_lossy(&tm.local.abbr).into_owned()),
            b'%' => Field::Text("%".into()),
            _ => {
                out.extend_from_slice(&fmt[start..i]);
                continue;
            }
        };
        let text = match field {
            Field::Sub(f) => {
                let mut sub = Vec::new();
                format_into(&mut sub, f, tm);
                String::from_utf8_lossy(&sub).into_owned()
            }
            Field::Text(t) => t,
            Field::Num(v, w, pad) => {
                let w = width.unwrap_or(w);
                let pad = match flag {
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
        let text = if upper { text.to_uppercase() } else { text };
        if let Some(w) = width {
            let pad = if flag == Some(b'0') { '0' } else { ' ' };
            for _ in text.len()..w {
                out.push(pad as u8);
            }
        }
        out.extend_from_slice(text.as_bytes());
    }
}

enum Field {
    Text(String),
    Sub(&'static [u8]),
    Num(i64, usize, u8),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(fmt: &str, rule: &str, t: i64) -> String {
        let zone = Zone {
            transitions: Vec::new(),
            types: Vec::new(),
            rule: parse_rule(rule.as_bytes()),
        };
        String::from_utf8(strftime(fmt.as_bytes(), &zone, t)).unwrap()
    }

    #[test]
    fn formats() {
        assert_eq!(
            f("%F %T %Z %z", "UTC0", 1234567890),
            "2009-02-13 23:31:30 UTC +0000"
        );
        assert_eq!(
            f("%a %A %b %B %e %j %u %w", "UTC0", 0),
            "Thu Thursday Jan January  1 001 4 4"
        );
        assert_eq!(f("%c", "UTC0", 0), "Thu Jan  1 00:00:00 1970");
        assert_eq!(
            f("%I %p %l %k %-d %_m %s", "UTC0", 3600 * 13),
            "01 PM  1 13 1  1 46800"
        );
        assert_eq!(
            f("%G-W%V-%u", "UTC0", days_from_civil(2021, 1, 3) * 86400),
            "2020-W53-7"
        );
        assert_eq!(
            f("%U %W", "UTC0", days_from_civil(2019, 5, 16) * 86400),
            "19 19"
        );
        assert_eq!(f("%^a %10Y|%%|%Q", "UTC0", 0), "THU 0000001970|%|%Q");
    }

    #[test]
    fn rules() {
        let ny = "EST5EDT,M3.2.0,M11.1.0";
        assert_eq!(f("%F %T %Z", ny, 1557978599), "2019-05-15 23:49:59 EDT");
        assert_eq!(f("%F %T %Z", ny, 1547978599), "2019-01-20 05:03:19 EST");
        assert_eq!(f("%F %H %z", "JST-9", 1557978599), "2019-05-16 12 +0900");
        assert_eq!(f("%H %Z", "<+0330>-3:30", 0), "03 +0330");
        // Southern hemisphere: DST across the new year.
        let syd = "AEST-10AEDT,M10.1.0,M4.1.0/3";
        assert_eq!(f("%Z", syd, days_from_civil(2020, 1, 10) * 86400), "AEDT");
        assert_eq!(f("%Z", syd, days_from_civil(2020, 6, 10) * 86400), "AEST");
    }
}
