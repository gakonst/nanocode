//! `date [-u] [-d STRING | -r FILE|SECONDS | -j [-f INFMT] [DATE]] [-v ADJ]...
//! [-I[FMT] | -R | --rfc-3339=FMT | +FORMAT]`: the union of GNU's and BSD's flags
//! (D016, D1204). GNU: `-d` date strings (timefmt's parser), `-I`, `-R`,
//! `--rfc-3339`, `+%N` and the other GNU conversions, `-f FILE` without `-j`. BSD:
//! `-j` (parse, don't set), `-f INFMT` with `-j`, `-v±N[ymwdHMS]`. `-r`: all digits and
//! no such file is BSD's epoch seconds, else GNU's reference file. The zone is the
//! command's `TZ` (zone files, POSIX rules) or the system's. Setting the clock runs the
//! real program. Written for psh after the GNU coreutils manual and FreeBSD's date(1).

use psh_engine::util::UtilCtx;

use crate::common;
use crate::opts::{Spec, both, flag, long};
use crate::timefmt::{self, MONTH_NAMES, NS, Tm};

pub const SPEC: Spec = Spec {
    name: "date",
    opts: &[
        both('d', "date").value(),
        both('f', "file").value(),
        both('r', "reference").value(),
        both('s', "set").value(),
        both('u', "utc"),
        long("universal"),
        both('R', "rfc-email"),
        long("rfc-2822"),
        long("iso-8601").optional(),
        long("rfc-3339").value(),
        flag('j'),
        flag('n'),
        flag('v').value(),
    ],
    posix_order: false,
};

const DEFAULT_FORMAT: &[u8] = b"%a %b %e %H:%M:%S %Z %Y";

/// `-I` and `-IFMT` are `--iso-8601[=FMT]` (an optional value only attached).
fn rewrite_iso(argv: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut out = Vec::with_capacity(argv.len());
    let mut rest = false;
    for (i, a) in argv.iter().enumerate() {
        if i == 0 || rest || !a.starts_with(b"-I") {
            rest |= a == b"--";
            out.push(a.clone());
            continue;
        }
        let mut long = b"--iso-8601".to_vec();
        if a.len() > 2 {
            long.push(b'=');
            long.extend_from_slice(&a[2..]);
        }
        out.push(long);
    }
    out
}

/// The `-I`/`--rfc-3339` precision.
#[derive(Clone, Copy, PartialEq)]
enum Precision {
    Date,
    Hours,
    Minutes,
    Seconds,
    Ns,
}

fn precision(v: &[u8], hours_ok: bool) -> Option<Precision> {
    let v = std::str::from_utf8(v).ok()?;
    let all: &[(&str, Precision)] = &[
        ("date", Precision::Date),
        ("hours", Precision::Hours),
        ("minutes", Precision::Minutes),
        ("seconds", Precision::Seconds),
        ("ns", Precision::Ns),
    ];
    let found: Vec<Precision> = all
        .iter()
        .filter(|(n, p)| {
            !v.is_empty()
                && n.starts_with(v)
                && (hours_ok || matches!(p, Precision::Date | Precision::Seconds | Precision::Ns))
        })
        .map(|(_, p)| *p)
        .collect();
    match found.as_slice() {
        [p] => Some(*p),
        _ => None,
    }
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let args = rewrite_iso(argv);
    let p = match common::parse(ctx, &SPEC, &args).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    if p.has(&SPEC, "set") {
        return set_clock(ctx, argv).await;
    }
    if p.has(&SPEC, "utc") || p.has(&SPEC, "universal") {
        ctx.use_utc();
    } else {
        ctx.load_zone().await;
    }
    let bsd_parse = p.has(&SPEC, "j");
    // The output format: `+FORMAT`, or the other operand (a date to parse with -j).
    let mut format: Option<Vec<u8>> = None;
    let mut date_operand: Option<Vec<u8>> = None;
    for op in &p.operands {
        if let Some(f) = op.strip_prefix(b"+")
            && format.is_none()
        {
            format = Some(f.to_vec());
        } else if date_operand.is_none() && format.is_none() {
            date_operand = Some(op.clone());
        } else {
            let q = common::quote(ctx, op);
            return common::usage(ctx, &format!("extra operand {q}")).await;
        }
    }
    let mut styles = 0;
    let mut iso: Option<Precision> = None;
    let mut rfc3339: Option<Precision> = None;
    let mut rfc_email = false;
    for (k, v) in p.iter(&SPEC) {
        match k.as_str() {
            "iso-8601" => {
                styles += 1;
                iso = match v {
                    None => Some(Precision::Date),
                    Some(v) => match precision(v, true) {
                        Some(p) => Some(p),
                        None => return bad_argument(ctx, v, "--iso-8601").await,
                    },
                };
            }
            "rfc-3339" => {
                styles += 1;
                match precision(v.unwrap_or_default(), false) {
                    Some(p) => rfc3339 = Some(p),
                    None => {
                        return bad_argument(ctx, v.unwrap_or_default(), "--rfc-3339").await;
                    }
                }
            }
            "rfc-email" | "rfc-2822" => {
                styles += 1;
                rfc_email = true;
            }
            _ => {}
        }
    }
    if styles + usize::from(format.is_some()) > 1 {
        return common::usage(ctx, "multiple output formats specified").await;
    }
    let now = ctx.now();
    let now_ns = i64::try_from(now.as_nanos()).unwrap_or(i64::MAX);
    // With -j, -f is the input format, not a source.
    let sources = ["date", "reference", "file"]
        .iter()
        .filter(|k| p.has(&SPEC, k) && !(bsd_parse && **k == "file"))
        .count();
    if sources > 1 {
        return common::usage(
            ctx,
            "the options to specify dates for printing are mutually exclusive",
        )
        .await;
    }
    // The instants to print: one, or one per line of `-f FILE` (GNU).
    let mut instants: Vec<Option<i64>> = Vec::new();
    let mut status = 0;
    if let Some(d) = p.last(&SPEC, "date") {
        match timefmt::parse_date(ctx, d, now_ns) {
            Some(t) => instants.push(Some(t)),
            None => return invalid_date(ctx, d).await,
        }
    } else if let Some(r) = p.last(&SPEC, "reference") {
        let exists = ctx.stat(r, true).await;
        let digits = !r.is_empty() && r.iter().all(u8::is_ascii_digit);
        match exists {
            Ok(m) => instants.push(Some(m.mtime_ns)),
            Err(_) if digits => {
                let secs: i64 = std::str::from_utf8(r)
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(i64::MAX / NS);
                instants.push(Some(secs.saturating_mul(NS)));
            }
            Err(e) => {
                common::file_error(ctx, r, &e).await;
                return 1;
            }
        }
    } else if bsd_parse {
        let t = match (p.last(&SPEC, "file"), &date_operand) {
            (Some(fmt), Some(s)) => match strptime(ctx, fmt, s, now_ns) {
                Ok(t) => t,
                Err(msg) => {
                    ctx.error_raw(msg.as_bytes()).await;
                    ctx.error("illegal time format").await;
                    return 1;
                }
            },
            (None, Some(s)) => match bsd_operand(ctx, s, now_ns) {
                Some(t) => t,
                None => {
                    ctx.error("illegal time format").await;
                    return 1;
                }
            },
            (_, None) => now_ns,
        };
        instants.push(Some(t));
        date_operand = None;
    } else if let Some(file) = p.last(&SPEC, "file") {
        let mut text = Vec::new();
        let read = match common::Input::open(ctx, file).await {
            Ok(input) => input.read_to_end(ctx, &mut text).await,
            Err(e) => Err(e),
        };
        if let Err(e) = read {
            common::file_error(ctx, file, &e).await;
            return 1;
        }
        for line in text.split(|&b| b == b'\n') {
            if line.is_empty() {
                continue;
            }
            match timefmt::parse_date(ctx, line, now_ns) {
                Some(t) => instants.push(Some(t)),
                None => {
                    let q = common::quote(ctx, line);
                    ctx.error(&format!("invalid date {q}")).await;
                    instants.push(None);
                    status = 1;
                }
            }
        }
    } else {
        instants.push(Some(now_ns));
    }
    if date_operand.is_some() {
        // `date MMDDhhmm`: setting the clock is the real program's business.
        return set_clock(ctx, argv).await;
    }
    let mut out = Vec::new();
    for t in instants.into_iter().flatten() {
        let mut t = t;
        for (k, v) in p.iter(&SPEC) {
            if k == "v" {
                match adjust(ctx, t, v.unwrap_or_default()) {
                    Some(n) => t = n,
                    None => {
                        let v = String::from_utf8_lossy(v.unwrap_or_default());
                        ctx.error(&format!("{v}: illegal time adjustment")).await;
                        return 1;
                    }
                }
            }
        }
        let tm = timefmt::local(ctx, t);
        let secs = t.div_euclid(NS);
        let abbr = ctx.zone_abbr(secs);
        let fmt: Vec<u8> = if let Some(f) = &format {
            f.clone()
        } else if let Some(prec) = iso {
            match prec {
                Precision::Date => b"%Y-%m-%d".to_vec(),
                Precision::Hours => b"%Y-%m-%dT%H%:z".to_vec(),
                Precision::Minutes => b"%Y-%m-%dT%H:%M%:z".to_vec(),
                Precision::Seconds => b"%Y-%m-%dT%H:%M:%S%:z".to_vec(),
                Precision::Ns => b"%Y-%m-%dT%H:%M:%S,%N%:z".to_vec(),
            }
        } else if let Some(prec) = rfc3339 {
            match prec {
                Precision::Date => b"%Y-%m-%d".to_vec(),
                Precision::Ns => b"%Y-%m-%d %H:%M:%S.%N%:z".to_vec(),
                _ => b"%Y-%m-%d %H:%M:%S%:z".to_vec(),
            }
        } else if rfc_email {
            b"%a, %d %b %Y %H:%M:%S %z".to_vec()
        } else {
            DEFAULT_FORMAT.to_vec()
        };
        out.extend_from_slice(&timefmt::format(&fmt, &tm, &abbr, secs));
        out.push(b'\n');
    }
    if let Err(e) = ctx.write(&out).await {
        return common::write_failed(ctx, &e).await;
    }
    status
}

async fn invalid_date(ctx: &mut UtilCtx<'_>, d: &[u8]) -> i32 {
    let q = common::quote(ctx, d);
    ctx.error(&format!("invalid date {q}")).await;
    1
}

async fn bad_argument(ctx: &mut UtilCtx<'_>, v: &[u8], opt: &str) -> i32 {
    let (q, o) = (common::quote(ctx, v), common::quote(ctx, opt.as_bytes()));
    common::usage(ctx, &format!("invalid argument {q} for {o}")).await
}

/// Setting the clock: the real program does it (and refuses without privileges); a
/// sandbox has none.
async fn set_clock(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    if let Some(st) = ctx.run_program(argv.to_vec()).await {
        return st;
    }
    ctx.error("cannot set date: Operation not permitted").await;
    1
}

/// A wall-clock time (local, the given zone) as ns since the epoch, normalizing
/// out-of-range fields as `mktime` does.
fn from_fields(
    ctx: &UtilCtx<'_>,
    (y, mon, d): (i64, i64, i64),
    (h, mi, s): (i64, i64, i64),
    nsec: i64,
    zone: Option<i32>,
) -> i64 {
    let total = y * 12 + (mon - 1);
    let (y, m) = (total.div_euclid(12), (total.rem_euclid(12) + 1) as u32);
    let days = timefmt::days_from_civil(y, m, 1) + d - 1;
    let wall = days * 86400 + h * 3600 + mi * 60 + s;
    timefmt::epoch_of(ctx, wall, zone) * NS + nsec
}

/// BSD `-v`: `[+|-]N` with a unit `y m w d H M S`; a sign adjusts, none sets the field.
/// Months and years keep the day within the new month (Jan 31 + 1 month is Feb 29).
fn adjust(ctx: &UtilCtx<'_>, t: i64, v: &[u8]) -> Option<i64> {
    let (sign, rest) = match v.first()? {
        b'+' => (Some(1), &v[1..]),
        b'-' => (Some(-1), &v[1..]),
        _ => (None, v),
    };
    let (num, unit) = rest.split_at(rest.len().checked_sub(1)?);
    let n: i64 = std::str::from_utf8(num).ok()?.parse().ok()?;
    let tm = timefmt::local(ctx, t);
    let nsec = i64::from(tm.nsec);
    let (mut y, mut mo, mut d) = (tm.year, i64::from(tm.mon), i64::from(tm.day));
    let (mut h, mut mi, mut s) = (i64::from(tm.hour), i64::from(tm.min), i64::from(tm.sec));
    match (unit, sign) {
        (b"S", Some(k)) => return Some(t + k * n * NS),
        (b"M", Some(k)) => return Some(t + k * n * 60 * NS),
        (b"H", Some(k)) => return Some(t + k * n * 3600 * NS),
        (b"d", Some(k)) => d += k * n,
        (b"w", Some(k)) => d += 7 * k * n,
        (b"m", Some(k)) | (b"y", Some(k)) => {
            let months = if unit == b"m" { k * n } else { 12 * k * n };
            let total = y * 12 + (mo - 1) + months;
            y = total.div_euclid(12);
            mo = total.rem_euclid(12) + 1;
            d = d.min(i64::from(timefmt::days_in_month(y, mo as u32)));
        }
        (b"S", None) if n < 60 => s = n,
        (b"M", None) if n < 60 => mi = n,
        (b"H", None) if n < 24 => h = n,
        (b"d", None) if (1..=31).contains(&n) => d = n,
        (b"m", None) if (1..=12).contains(&n) => mo = n,
        (b"y", None) => {
            y = match n {
                0..=68 => 2000 + n,
                69..=99 => 1900 + n,
                _ => n,
            }
        }
        _ => return None,
    }
    Some(from_fields(ctx, (y, mo, d), (h, mi, s), nsec, None))
}

/// BSD `-j DATE`: `[[[[mm]dd]HH]MM[[cc]yy][.SS]]`, the missing fields from now.
fn bsd_operand(ctx: &UtilCtx<'_>, s: &[u8], now_ns: i64) -> Option<i64> {
    let (main, secs) = match s.iter().position(|&b| b == b'.') {
        Some(i) => (&s[..i], Some(&s[i + 1..])),
        None => (s, None),
    };
    if !main.iter().all(u8::is_ascii_digit) || main.len() % 2 != 0 || main.is_empty() {
        return None;
    }
    let pair = |i: usize| i64::from(main[i] - b'0') * 10 + i64::from(main[i + 1] - b'0');
    let now = timefmt::local(ctx, now_ns);
    let (mut y, mut mo, mut d, mut h) = (
        now.year,
        i64::from(now.mon),
        i64::from(now.day),
        i64::from(now.hour),
    );
    let mi;
    match main.len() {
        2 => mi = pair(0),
        4 => (h, mi) = (pair(0), pair(2)),
        6 => (d, h, mi) = (pair(0), pair(2), pair(4)),
        8 | 10 | 12 => {
            (mo, d, h, mi) = (pair(0), pair(2), pair(4), pair(6));
            if main.len() == 10 {
                let yy = pair(8);
                y = if yy < 69 { 2000 + yy } else { 1900 + yy };
            } else if main.len() == 12 {
                y = pair(8) * 100 + pair(10);
            }
        }
        _ => return None,
    }
    let s = match secs {
        Some(x) if x.len() == 2 && x.iter().all(u8::is_ascii_digit) => {
            i64::from(x[0] - b'0') * 10 + i64::from(x[1] - b'0')
        }
        Some(_) => return None,
        None => 0,
    };
    if !(1..=12).contains(&mo)
        || d < 1
        || d > i64::from(timefmt::days_in_month(y, mo as u32))
        || h > 23
        || mi > 59
        || s > 60
    {
        return None;
    }
    Some(from_fields(ctx, (y, mo, d), (h, mi, s), 0, None))
}

/// BSD `-j -f FORMAT STRING` (strptime): the fields the format names, the others from
/// now. `Err` is the "Failed conversion" line.
fn strptime(ctx: &UtilCtx<'_>, fmt: &[u8], s: &[u8], now_ns: i64) -> Result<i64, String> {
    let fail = || {
        format!(
            "Failed conversion of ``{}'' using format ``{}''\n",
            String::from_utf8_lossy(s),
            String::from_utf8_lossy(fmt)
        )
    };
    let now: Tm = timefmt::local(ctx, now_ns);
    let mut f = Fields {
        y: now.year,
        mon: i64::from(now.mon),
        d: i64::from(now.day),
        h: i64::from(now.hour),
        mi: i64::from(now.min),
        s: i64::from(now.sec),
        pm: None,
        zone: None,
        epoch: None,
        yday: None,
    };
    let mut pos = 0;
    scan(fmt, s, &mut pos, &mut f).ok_or_else(fail)?;
    if let Some(e) = f.epoch {
        return Ok(e * NS);
    }
    if let Some(pm) = f.pm {
        f.h = f.h % 12 + if pm { 12 } else { 0 };
    }
    if let Some(yd) = f.yday {
        f.mon = 1;
        f.d = yd;
    }
    if !(1..=12).contains(&f.mon) || f.d < 1 || f.d > 31 || f.h > 23 || f.mi > 59 || f.s > 61 {
        return Err(fail());
    }
    Ok(from_fields(
        ctx,
        (f.y, f.mon, f.d),
        (f.h, f.mi, f.s),
        0,
        f.zone,
    ))
}

struct Fields {
    y: i64,
    mon: i64,
    d: i64,
    h: i64,
    mi: i64,
    s: i64,
    pm: Option<bool>,
    zone: Option<i32>,
    epoch: Option<i64>,
    yday: Option<i64>,
}

fn scan(fmt: &[u8], s: &[u8], pos: &mut usize, f: &mut Fields) -> Option<()> {
    let mut i = 0;
    let skip_ws = |pos: &mut usize| {
        while s.get(*pos).is_some_and(u8::is_ascii_whitespace) {
            *pos += 1;
        }
    };
    let number = |pos: &mut usize, max: usize| -> Option<i64> {
        while s.get(*pos) == Some(&b' ') {
            *pos += 1;
        }
        let neg = s.get(*pos) == Some(&b'-');
        if neg || s.get(*pos) == Some(&b'+') {
            *pos += 1;
        }
        let start = *pos;
        while *pos < s.len() && *pos - start < max && s[*pos].is_ascii_digit() {
            *pos += 1;
        }
        if *pos == start {
            return None;
        }
        let v: i64 = std::str::from_utf8(&s[start..*pos]).ok()?.parse().ok()?;
        Some(if neg { -v } else { v })
    };
    let name = |pos: &mut usize, names: &[&str]| -> Option<usize> {
        let rest = &s[*pos..];
        for (k, n) in names.iter().enumerate() {
            for cand in [n.len(), 3] {
                if rest.len() >= cand && rest[..cand].eq_ignore_ascii_case(&n.as_bytes()[..cand]) {
                    *pos += cand;
                    return Some(k);
                }
            }
        }
        None
    };
    while i < fmt.len() {
        let c = fmt[i];
        i += 1;
        if c.is_ascii_whitespace() {
            skip_ws(pos);
            continue;
        }
        if c != b'%' {
            if s.get(*pos) != Some(&c) {
                return None;
            }
            *pos += 1;
            continue;
        }
        let d = *fmt.get(i)?;
        i += 1;
        match d {
            b'Y' => f.y = number(pos, 4)?,
            b'y' => {
                let v = number(pos, 2)?;
                f.y = if v < 69 { 2000 + v } else { 1900 + v };
            }
            b'C' => f.y = number(pos, 2)? * 100 + f.y.rem_euclid(100),
            b'm' => f.mon = number(pos, 2)?,
            b'd' | b'e' => f.d = number(pos, 2)?,
            b'H' | b'k' => f.h = number(pos, 2)?,
            b'I' | b'l' => f.h = number(pos, 2)?,
            b'M' => f.mi = number(pos, 2)?,
            b'S' => f.s = number(pos, 2)?,
            b'j' => f.yday = Some(number(pos, 3)?),
            b's' => f.epoch = Some(number(pos, 19)?),
            b'b' | b'B' | b'h' => f.mon = name(pos, &MONTH_NAMES)? as i64 + 1,
            b'a' | b'A' => {
                name(pos, &timefmt::DAY_NAMES)?;
            }
            b'p' => {
                let rest = &s[*pos..];
                if rest.len() >= 2 && rest[..2].eq_ignore_ascii_case(b"am") {
                    f.pm = Some(false);
                } else if rest.len() >= 2 && rest[..2].eq_ignore_ascii_case(b"pm") {
                    f.pm = Some(true);
                } else {
                    return None;
                }
                *pos += 2;
            }
            b'z' => {
                let sign = match s.get(*pos)? {
                    b'+' => 1,
                    b'-' => -1,
                    b'Z' => {
                        *pos += 1;
                        f.zone = Some(0);
                        continue;
                    }
                    _ => return None,
                };
                *pos += 1;
                let hh = number(pos, 2)?;
                if s.get(*pos) == Some(&b':') {
                    *pos += 1;
                }
                let mm = number(pos, 2).unwrap_or(0);
                f.zone = Some(sign * (hh as i32 * 3600 + mm as i32 * 60));
            }
            b'Z' => {
                let start = *pos;
                while s.get(*pos).is_some_and(u8::is_ascii_alphabetic) {
                    *pos += 1;
                }
                let z = String::from_utf8_lossy(&s[start..*pos]).to_ascii_uppercase();
                f.zone = match z.as_str() {
                    "UTC" | "GMT" | "Z" => Some(0),
                    _ => None,
                };
            }
            b'T' => scan(b"%H:%M:%S", s, pos, f)?,
            b'R' => scan(b"%H:%M", s, pos, f)?,
            b'D' => scan(b"%m/%d/%y", s, pos, f)?,
            b'F' => scan(b"%Y-%m-%d", s, pos, f)?,
            b'n' | b't' => skip_ws(pos),
            b'%' => {
                if s.get(*pos) != Some(&b'%') {
                    return None;
                }
                *pos += 1;
            }
            _ => return None,
        }
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_rewrite() {
        let v = |a: &[&str]| -> Vec<String> {
            let argv: Vec<Vec<u8>> = a.iter().map(|s| s.as_bytes().to_vec()).collect();
            rewrite_iso(&argv)
                .into_iter()
                .map(|x| String::from_utf8(x).unwrap())
                .collect()
        };
        assert_eq!(v(&["date", "-I"]), ["date", "--iso-8601"]);
        assert_eq!(v(&["date", "-Iseconds"]), ["date", "--iso-8601=seconds"]);
        assert_eq!(v(&["date", "--", "-I"]), ["date", "--", "-I"]);
    }

    #[test]
    fn precisions() {
        assert!(precision(b"s", true) == Some(Precision::Seconds));
        assert!(precision(b"hours", false).is_none());
        assert!(precision(b"x", true).is_none());
    }
}
