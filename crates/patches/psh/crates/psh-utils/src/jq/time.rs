//! jq's date builtins: `gmtime`, `localtime`, `mktime`, `strftime`, `strflocaltime`,
//! `strptime` over jq's broken-down time `[year, month0, mday, hours, minutes, seconds,
//! wday, yday]`, with the C locale's names. Written for psh after jq's manual and the
//! POSIX `strftime`/`strptime` descriptions.

use crate::timefmt::{civil_from_days, days_from_civil};

use super::val::{Error, Val, ValR, err};

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

/// A broken-down time.
#[derive(Clone, Copy, Debug)]
pub struct Tm {
    pub year: i64,
    /// 0-11.
    pub mon: i64,
    pub mday: i64,
    pub hour: i64,
    pub min: i64,
    pub sec: f64,
    pub wday: i64,
    pub yday: i64,
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Seconds since the epoch, broken down (in UTC, or shifted by `offset` seconds).
pub fn breakdown(t: f64, offset: i64) -> Tm {
    let whole = t.floor();
    let frac = t - whole;
    let secs = whole as i64 + offset;
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    Tm {
        year: y,
        mon: i64::from(m) - 1,
        mday: i64::from(d),
        hour: rem / 3600,
        min: rem / 60 % 60,
        sec: (rem % 60) as f64 + frac,
        wday: (days + 4).rem_euclid(7),
        yday: days - days_from_civil(y, 1, 1),
    }
}

impl Tm {
    pub fn to_val(self) -> Val {
        [
            self.year as f64,
            self.mon as f64,
            self.mday as f64,
            self.hour as f64,
            self.min as f64,
            self.sec,
            self.wday as f64,
            self.yday as f64,
        ]
        .into_iter()
        .map(Val::num)
        .collect()
    }

    /// From jq's array (at least six numbers).
    pub fn from_val(v: &Val, who: &str) -> Result<Tm, Error> {
        let fail = || err(format!("{who} requires parsed datetime inputs"));
        let Val::Arr(a) = v else {
            return Err(fail());
        };
        if a.len() < 6 {
            return Err(fail());
        }
        let mut n = [0f64; 8];
        for (i, x) in a.iter().take(8).enumerate() {
            n[i] = x.as_f64().ok_or_else(fail)?;
        }
        Ok(Tm {
            year: n[0] as i64,
            mon: n[1] as i64,
            mday: n[2] as i64,
            hour: n[3] as i64,
            min: n[4] as i64,
            sec: n[5],
            wday: n[6] as i64,
            yday: n[7] as i64,
        })
    }

    /// `timegm`: the fields normalized, seconds since the epoch.
    pub fn epoch(&self) -> i64 {
        let y = self.year + self.mon.div_euclid(12);
        let m = self.mon.rem_euclid(12) + 1;
        let days = days_from_civil(y, m as u32, 1) + self.mday - 1;
        days * 86400 + self.hour * 3600 + self.min * 60 + self.sec.floor() as i64
    }

    /// The fields recomputed from the instant (wday and yday filled in).
    fn normalized(&self) -> Tm {
        breakdown(self.epoch() as f64 + self.sec.fract(), 0)
    }
}

/// `mktime`.
pub fn mktime(v: &Val) -> ValR {
    let Val::Arr(a) = v else {
        return Err(err("mktime requires array of 6 numbers"));
    };
    if a.len() < 6 {
        return Err(err("mktime requires array of 6 numbers"));
    }
    if a.iter().take(6).any(|x| x.as_f64().is_none()) {
        return Err(err("mktime requires parsed datetime inputs"));
    }
    let tm = Tm::from_val(v, "mktime")?;
    Ok(Val::num(tm.epoch() as f64))
}

/// The time a `strftime` input names: a number of seconds or a broken-down time.
pub fn input_tm(v: &Val, offset: i64, who: &str) -> Result<Tm, Error> {
    match v {
        Val::Num(n) => Ok(breakdown(n.f, offset)),
        Val::Arr(_) => Tm::from_val(v, who),
        _ => Err(err(format!("{who} requires parsed datetime inputs"))),
    }
}

/// `strftime` in the C locale; `zone` is the abbreviation `%Z` shows, `offset` the
/// zone's distance from UTC for `%z` (and `%s`).
pub fn strftime(tm: &Tm, fmt: &str, zone: &str, offset: i64) -> String {
    let mut out = String::new();
    let mut chars = fmt.chars().peekable();
    let h12 = |h: i64| if h % 12 == 0 { 12 } else { h % 12 };
    let sec = tm.sec.floor() as i64;
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let Some(mut d) = chars.next() else {
            out.push('%');
            break;
        };
        // E and O modifiers change nothing in the C locale.
        if (d == 'E' || d == 'O')
            && let Some(n) = chars.next()
        {
            d = n;
        }
        let wday = tm.wday.rem_euclid(7) as usize;
        let mon = tm.mon.rem_euclid(12) as usize;
        match d {
            'a' => out.push_str(&DAYS[wday][..3]),
            'A' => out.push_str(DAYS[wday]),
            'b' | 'h' => out.push_str(&MONTHS[mon][..3]),
            'B' => out.push_str(MONTHS[mon]),
            'c' => out.push_str(&format!(
                "{} {} {:2} {:02}:{:02}:{:02} {}",
                &DAYS[wday][..3],
                &MONTHS[mon][..3],
                tm.mday,
                tm.hour,
                tm.min,
                sec,
                tm.year
            )),
            'C' => out.push_str(&format!("{:02}", tm.year.div_euclid(100))),
            'd' => out.push_str(&format!("{:02}", tm.mday)),
            'D' | 'x' => out.push_str(&format!(
                "{:02}/{:02}/{:02}",
                tm.mon + 1,
                tm.mday,
                tm.year.rem_euclid(100)
            )),
            'e' => out.push_str(&format!("{:2}", tm.mday)),
            'F' => out.push_str(&format!("{}-{:02}-{:02}", tm.year, tm.mon + 1, tm.mday)),
            'G' | 'g' | 'V' => {
                let (y, w) = iso_week(tm);
                out.push_str(&match d {
                    'G' => y.to_string(),
                    'g' => format!("{:02}", y.rem_euclid(100)),
                    _ => format!("{w:02}"),
                });
            }
            'H' => out.push_str(&format!("{:02}", tm.hour)),
            'I' => out.push_str(&format!("{:02}", h12(tm.hour))),
            'j' => out.push_str(&format!("{:03}", tm.yday + 1)),
            'k' => out.push_str(&format!("{:2}", tm.hour)),
            'l' => out.push_str(&format!("{:2}", h12(tm.hour))),
            'm' => out.push_str(&format!("{:02}", tm.mon + 1)),
            'M' => out.push_str(&format!("{:02}", tm.min)),
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'p' => out.push_str(if tm.hour < 12 { "AM" } else { "PM" }),
            'P' => out.push_str(if tm.hour < 12 { "am" } else { "pm" }),
            'r' => out.push_str(&format!(
                "{:02}:{:02}:{:02} {}",
                h12(tm.hour),
                tm.min,
                sec,
                if tm.hour < 12 { "AM" } else { "PM" }
            )),
            'R' => out.push_str(&format!("{:02}:{:02}", tm.hour, tm.min)),
            's' => out.push_str(&(tm.epoch() - offset).to_string()),
            'S' => out.push_str(&format!("{sec:02}")),
            'T' | 'X' => out.push_str(&format!("{:02}:{:02}:{:02}", tm.hour, tm.min, sec)),
            'u' => out.push_str(&(if wday == 0 { 7 } else { wday }).to_string()),
            'w' => out.push_str(&wday.to_string()),
            'U' => out.push_str(&format!("{:02}", (tm.yday + 7 - tm.wday) / 7)),
            'W' => out.push_str(&format!(
                "{:02}",
                (tm.yday + 7 - (tm.wday + 6).rem_euclid(7)) / 7
            )),
            'y' => out.push_str(&format!("{:02}", tm.year.rem_euclid(100))),
            'Y' => out.push_str(&tm.year.to_string()),
            'z' => {
                let sign = if offset < 0 { '-' } else { '+' };
                let a = offset.unsigned_abs();
                out.push_str(&format!("{sign}{:02}{:02}", a / 3600, a / 60 % 60));
            }
            'Z' => out.push_str(zone),
            '+' => out.push_str(&format!(
                "{} {} {:2} {:02}:{:02}:{:02} {} {}",
                &DAYS[wday][..3],
                &MONTHS[mon][..3],
                tm.mday,
                tm.hour,
                tm.min,
                sec,
                zone,
                tm.year
            )),
            '%' => out.push('%'),
            other => {
                out.push('%');
                out.push(other);
            }
        }
    }
    out
}

fn iso_week(tm: &Tm) -> (i64, i64) {
    let wday = (tm.wday + 6).rem_euclid(7);
    let week = (tm.yday - wday + 10) / 7;
    let weeks_in = |y: i64| {
        let jan1 = (days_from_civil(y, 1, 1) + 3).rem_euclid(7);
        if jan1 == 3 || (jan1 == 2 && is_leap(y)) {
            53
        } else {
            52
        }
    };
    if week < 1 {
        (tm.year - 1, weeks_in(tm.year - 1))
    } else if week > weeks_in(tm.year) {
        (tm.year + 1, 1)
    } else {
        (tm.year, week)
    }
}

/// `strptime` (the C locale): the broken-down time, with wday and yday computed.
pub fn strptime(s: &str, fmt: &str) -> ValR {
    let fail = || err(format!("date \"{s}\" does not match format \"{fmt}\""));
    let b = s.as_bytes();
    let mut i = 0;
    let mut tm = Tm {
        year: 1900,
        mon: 0,
        mday: 0,
        hour: 0,
        min: 0,
        sec: 0.0,
        wday: 0,
        yday: 0,
    };
    let mut offset: i64 = 0;
    let mut pm: Option<bool> = None;
    let mut epoch: Option<i64> = None;
    let num = |i: &mut usize, max: usize| -> Option<i64> {
        let start = *i;
        let neg = b.get(*i) == Some(&b'-') || b.get(*i) == Some(&b'+');
        if neg {
            *i += 1;
        }
        let ds = *i;
        while *i < b.len() && *i - ds < max && b[*i].is_ascii_digit() {
            *i += 1;
        }
        if *i == ds {
            *i = start;
            return None;
        }
        let v: i64 = std::str::from_utf8(&b[ds..*i]).ok()?.parse().ok()?;
        Some(if b[start] == b'-' { -v } else { v })
    };
    let name = |i: &mut usize, names: &[&str]| -> Option<i64> {
        for (k, n) in names.iter().enumerate() {
            for cand in [*n, &n[..3]] {
                let l = cand.len();
                if b.len() >= *i + l && b[*i..*i + l].eq_ignore_ascii_case(cand.as_bytes()) {
                    *i += l;
                    return Some(k as i64);
                }
            }
        }
        None
    };
    let mut f = fmt.chars();
    while let Some(c) = f.next() {
        if c.is_whitespace() {
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            continue;
        }
        if c != '%' {
            let mut tmp = [0u8; 4];
            let enc = c.encode_utf8(&mut tmp).as_bytes();
            if b.len() < i + enc.len() || &b[i..i + enc.len()] != enc {
                return Err(fail());
            }
            i += enc.len();
            continue;
        }
        let Some(mut d) = f.next() else {
            return Err(fail());
        };
        if d == 'E' || d == 'O' {
            d = f.next().ok_or_else(fail)?;
        }
        match d {
            'Y' => tm.year = num(&mut i, 4).ok_or_else(fail)?,
            'm' => tm.mon = num(&mut i, 2).ok_or_else(fail)? - 1,
            'd' | 'e' => tm.mday = num(&mut i, 2).ok_or_else(fail)?,
            'H' | 'k' => tm.hour = num(&mut i, 2).ok_or_else(fail)?,
            'I' | 'l' => tm.hour = num(&mut i, 2).ok_or_else(fail)?,
            'M' => tm.min = num(&mut i, 2).ok_or_else(fail)?,
            'S' => tm.sec = num(&mut i, 2).ok_or_else(fail)? as f64,
            'j' => tm.yday = num(&mut i, 3).ok_or_else(fail)? - 1,
            'y' => {
                let y = num(&mut i, 2).ok_or_else(fail)?;
                tm.year = if y < 69 { 2000 + y } else { 1900 + y };
            }
            'C' => {
                let c = num(&mut i, 2).ok_or_else(fail)?;
                tm.year = c * 100 + tm.year.rem_euclid(100);
            }
            'a' | 'A' => tm.wday = name(&mut i, &DAYS).ok_or_else(fail)?,
            'b' | 'B' | 'h' => tm.mon = name(&mut i, &MONTHS).ok_or_else(fail)?,
            'p' => {
                let rest = &b[i..];
                if rest.len() >= 2 && rest[..2].eq_ignore_ascii_case(b"AM") {
                    pm = Some(false);
                } else if rest.len() >= 2 && rest[..2].eq_ignore_ascii_case(b"PM") {
                    pm = Some(true);
                } else {
                    return Err(fail());
                }
                i += 2;
            }
            'z' => {
                if b.get(i) == Some(&b'Z') {
                    i += 1;
                } else {
                    let sign = match b.get(i) {
                        Some(b'+') => 1,
                        Some(b'-') => -1,
                        _ => return Err(fail()),
                    };
                    i += 1;
                    let start = i;
                    while i < b.len() && (b[i].is_ascii_digit() || b[i] == b':') {
                        i += 1;
                    }
                    let digits: String = s[start..i].chars().filter(|c| *c != ':').collect();
                    if digits.len() < 2 {
                        return Err(fail());
                    }
                    let h: i64 = digits[..2].parse().map_err(|_| fail())?;
                    let m: i64 = digits.get(2..4).unwrap_or("0").parse().unwrap_or(0);
                    offset = sign * (h * 3600 + m * 60);
                }
            }
            'Z' => {
                while i < b.len() && b[i].is_ascii_alphabetic() {
                    i += 1;
                }
            }
            's' => epoch = Some(num(&mut i, 20).ok_or_else(fail)?),
            'T' => {
                tm.hour = num(&mut i, 2).ok_or_else(fail)?;
                if b.get(i) != Some(&b':') {
                    return Err(fail());
                }
                i += 1;
                tm.min = num(&mut i, 2).ok_or_else(fail)?;
                if b.get(i) != Some(&b':') {
                    return Err(fail());
                }
                i += 1;
                tm.sec = num(&mut i, 2).ok_or_else(fail)? as f64;
            }
            'F' => {
                tm.year = num(&mut i, 6).ok_or_else(fail)?;
                for field in 0..2 {
                    if b.get(i) != Some(&b'-') {
                        return Err(fail());
                    }
                    i += 1;
                    let v = num(&mut i, 2).ok_or_else(fail)?;
                    if field == 0 {
                        tm.mon = v - 1;
                    } else {
                        tm.mday = v;
                    }
                }
            }
            'n' | 't' => {
                while i < b.len() && b[i].is_ascii_whitespace() {
                    i += 1;
                }
            }
            '%' => {
                if b.get(i) != Some(&b'%') {
                    return Err(fail());
                }
                i += 1;
            }
            _ => return Err(fail()),
        }
    }
    if i != b.len() {
        return Err(fail());
    }
    if let Some(pm) = pm {
        tm.hour %= 12;
        if pm {
            tm.hour += 12;
        }
    }
    if let Some(e) = epoch {
        return Ok(breakdown(e as f64, 0).to_val());
    }
    let _ = offset;
    // wday and yday follow from the date (glibc computes them).
    let n = tm.normalized();
    tm.wday = n.wday;
    tm.yday = n.yday;
    Ok(tm.to_val())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breakdown_and_back() {
        let tm = breakdown(1425599507.0, 0);
        assert_eq!(tm.to_val().to_json(), "[2015,2,5,23,51,47,4,63]");
        assert_eq!(tm.epoch(), 1425599507);
        assert_eq!(
            strftime(&tm, "%Y-%m-%dT%H:%M:%SZ %A %j", "UTC", 0),
            "2015-03-05T23:51:47Z Thursday 064"
        );
    }

    #[test]
    fn parses() {
        let v = strptime("10:15 2015-03-05", "%H:%M %Y-%m-%d").unwrap();
        assert_eq!(v.to_json(), "[2015,2,5,10,15,0,4,63]");
        assert!(strptime("x", "%Y").is_err());
    }
}
