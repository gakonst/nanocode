//! gawk's time functions: `strftime` (C's conversions in the C locale) and `mktime`.

use psh_engine::util::UtilCtx;

use crate::timefmt::{self, NS, Tm};

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

fn tm_of(ctx: &UtilCtx<'_>, t: i64, utc: bool) -> Tm {
    let ns = t.saturating_mul(NS);
    if utc {
        timefmt::tm_at(ns, 0)
    } else {
        timefmt::local(ctx, ns)
    }
}

/// ISO 8601 week-based year and week of a time.
fn iso_week(tm: &Tm) -> (i64, u32) {
    let wday = (tm.wday + 6) % 7; // Monday = 0
    let yday = tm.yday as i64;
    let mut year = tm.year;
    let mut week = (yday - wday as i64 + 10) / 7;
    if week < 1 {
        year -= 1;
        let prev_len = if is_leap(year) { 366 } else { 365 };
        week = (yday + prev_len - wday as i64 + 10) / 7;
    } else {
        let len = if is_leap(year) { 366 } else { 365 };
        if week == 53 && yday - wday as i64 + 3 >= len {
            year += 1;
            week = 1;
        }
    }
    (year, week as u32)
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

pub fn strftime(ctx: &UtilCtx<'_>, fmt: &[u8], t: i64, utc: bool) -> Vec<u8> {
    let tm = tm_of(ctx, t, utc);
    let mut out = Vec::new();
    format(&mut out, fmt, &tm, t);
    out
}

fn format(out: &mut Vec<u8>, fmt: &[u8], tm: &Tm, t: i64) {
    let mut i = 0;
    while i < fmt.len() {
        let c = fmt[i];
        i += 1;
        if c != b'%' || i >= fmt.len() {
            out.push(c);
            continue;
        }
        // E and O modifiers mean nothing in the C locale.
        let mut conv = fmt[i];
        i += 1;
        if (conv == b'E' || conv == b'O') && i < fmt.len() {
            conv = fmt[i];
            i += 1;
        }
        let hour12 = if tm.hour.is_multiple_of(12) {
            12
        } else {
            tm.hour % 12
        };
        let s: String = match conv {
            b'a' => DAYS[tm.wday as usize][..3].into(),
            b'A' => DAYS[tm.wday as usize].into(),
            b'b' | b'h' => MONTHS[tm.mon as usize - 1][..3].into(),
            b'B' => MONTHS[tm.mon as usize - 1].into(),
            b'c' => {
                let mut v = Vec::new();
                format(&mut v, b"%a %b %e %H:%M:%S %Y", tm, t);
                String::from_utf8_lossy(&v).into_owned()
            }
            b'C' => format!("{:02}", tm.year.div_euclid(100)),
            b'd' => format!("{:02}", tm.day),
            b'D' => format!("{:02}/{:02}/{:02}", tm.mon, tm.day, tm.year.rem_euclid(100)),
            b'e' => format!("{:2}", tm.day),
            b'F' => format!("{}-{:02}-{:02}", tm.year, tm.mon, tm.day),
            b'g' => format!("{:02}", iso_week(tm).0.rem_euclid(100)),
            b'G' => format!("{}", iso_week(tm).0),
            b'H' => format!("{:02}", tm.hour),
            b'I' => format!("{hour12:02}"),
            b'j' => format!("{:03}", tm.yday + 1),
            b'k' => format!("{:2}", tm.hour),
            b'l' => format!("{hour12:2}"),
            b'm' => format!("{:02}", tm.mon),
            b'M' => format!("{:02}", tm.min),
            b'n' => "\n".into(),
            b'p' => if tm.hour < 12 { "AM" } else { "PM" }.into(),
            b'r' => format!(
                "{hour12:02}:{:02}:{:02} {}",
                tm.min,
                tm.sec,
                if tm.hour < 12 { "AM" } else { "PM" }
            ),
            b'R' => format!("{:02}:{:02}", tm.hour, tm.min),
            b's' => format!("{t}"),
            b'S' => format!("{:02}", tm.sec),
            b't' => "\t".into(),
            b'T' => format!("{:02}:{:02}:{:02}", tm.hour, tm.min, tm.sec),
            b'u' => format!("{}", if tm.wday == 0 { 7 } else { tm.wday }),
            b'U' => format!("{:02}", (tm.yday + 7 - tm.wday) / 7),
            b'V' => format!("{:02}", iso_week(tm).1),
            b'w' => format!("{}", tm.wday),
            b'W' => format!("{:02}", (tm.yday + 7 - (tm.wday + 6) % 7) / 7),
            b'x' => format!("{:02}/{:02}/{:02}", tm.mon, tm.day, tm.year.rem_euclid(100)),
            b'X' => format!("{:02}:{:02}:{:02}", tm.hour, tm.min, tm.sec),
            b'y' => format!("{:02}", tm.year.rem_euclid(100)),
            b'Y' => format!("{}", tm.year),
            b'z' => {
                let o = tm.offset;
                let sign = if o < 0 { '-' } else { '+' };
                let o = o.abs();
                format!("{sign}{:02}{:02}", o / 3600, o / 60 % 60)
            }
            b'Z' => {
                if tm.offset == 0 {
                    "UTC".into()
                } else {
                    let o = tm.offset;
                    let sign = if o < 0 { '-' } else { '+' };
                    let o = o.abs();
                    format!("{sign}{:02}{:02}", o / 3600, o / 60 % 60)
                }
            }
            b'%' => "%".into(),
            other => {
                out.push(b'%');
                out.push(other);
                continue;
            }
        };
        out.extend_from_slice(s.as_bytes());
    }
}

/// `mktime("YYYY MM DD HH MM SS [DST]")`: seconds since the epoch, -1 if malformed.
/// Out-of-range fields carry over, as C's `mktime` normalizes them.
pub fn mktime(ctx: &UtilCtx<'_>, spec: &[u8], utc: bool) -> f64 {
    let text = String::from_utf8_lossy(spec);
    let parts: Vec<i64> = match text
        .split_ascii_whitespace()
        .map(|p| p.parse::<i64>())
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(p) => p,
        Err(_) => return -1.0,
    };
    if parts.len() < 6 || parts.len() > 7 {
        return -1.0;
    }
    let (y, mo, d, h, mi, s) = (parts[0], parts[1], parts[2], parts[3], parts[4], parts[5]);
    // Months carry into years first, then everything else is seconds.
    let m0 = mo - 1;
    let y = y + m0.div_euclid(12);
    let mo = m0.rem_euclid(12) + 1;
    let days = timefmt::days_from_civil(y, mo as u32, 1) + d - 1;
    let wall = days * 86400 + h * 3600 + mi * 60 + s;
    if utc {
        return wall as f64;
    }
    let guess = wall - i64::from(ctx.utc_offset(wall));
    (wall - i64::from(ctx.utc_offset(guess))) as f64
}
