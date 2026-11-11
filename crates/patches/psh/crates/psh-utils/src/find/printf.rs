//! GNU `find -printf`: `%` directives with flags, width and precision, `\` escapes,
//! times through `%Tk` (`strftime` letters, with GNU's ten-digit fractions for `%T@`,
//! `%TS`, `%TT`, `%T+` and `%t`). Written for psh after GNU's manual.

use psh_engine::host::{ExtraMetadata, FileKind, Metadata};
use psh_engine::util::UtilCtx;

use crate::timefmt::{self, NS, Tm};

/// What the directives need beyond `lstat`/`stat`, fetched before formatting.
#[derive(Clone, Copy, Debug, Default)]
pub struct Needs {
    pub extra: bool,
    pub link: bool,
    pub follow: bool,
}

/// Everything a directive may show about a file.
pub struct Facts<'a> {
    pub path: &'a [u8],
    pub root: &'a [u8],
    pub depth: usize,
    pub meta: &'a Metadata,
    pub extra: Option<ExtraMetadata>,
    /// The link's target (`%l`).
    pub link: Option<Vec<u8>>,
    /// `%Y`: the type through links (`N` broken, `L` a loop).
    pub followed: Option<u8>,
}

/// One piece of a format.
enum Piece<'a> {
    Lit(&'a [u8]),
    Byte(u8),
    /// `\c`: stop here.
    Stop,
    Dir {
        flags: &'a [u8],
        width: Option<usize>,
        prec: Option<usize>,
        conv: u8,
        /// The letter after `T`, `A`, `C`, `B`.
        time: u8,
    },
}

/// Splits a format; unknown directives and escapes come back as warnings and literal
/// text.
fn pieces<'f>(f: &'f [u8], warn: &mut Vec<String>) -> Vec<Piece<'f>> {
    let mut out = Vec::new();
    let mut i = 0;
    let mut lit = 0;
    while i < f.len() {
        match f[i] {
            b'\\' if i + 1 < f.len() => {
                out.push(Piece::Lit(&f[lit..i]));
                let c = f[i + 1];
                i += 2;
                match c {
                    b'a' => out.push(Piece::Byte(7)),
                    b'b' => out.push(Piece::Byte(8)),
                    b'c' => out.push(Piece::Stop),
                    b'f' => out.push(Piece::Byte(12)),
                    b'n' => out.push(Piece::Byte(b'\n')),
                    b'r' => out.push(Piece::Byte(b'\r')),
                    b't' => out.push(Piece::Byte(b'\t')),
                    b'v' => out.push(Piece::Byte(11)),
                    b'\\' => out.push(Piece::Byte(b'\\')),
                    b'0'..=b'7' => {
                        let mut n = u32::from(c - b'0');
                        let mut k = 0;
                        while k < 2 && i < f.len() && (b'0'..=b'7').contains(&f[i]) {
                            n = n * 8 + u32::from(f[i] - b'0');
                            i += 1;
                            k += 1;
                        }
                        out.push(Piece::Byte(n as u8));
                    }
                    _ => {
                        warn.push(format!("warning: unrecognized escape `\\{}'", c as char));
                        out.push(Piece::Lit(&f[i - 2..i]));
                    }
                }
                lit = i;
            }
            b'%' => {
                out.push(Piece::Lit(&f[lit..i]));
                let start = i;
                i += 1;
                let fstart = i;
                while i < f.len() && b"-+ #0".contains(&f[i]) {
                    i += 1;
                }
                let flags = &f[fstart..i];
                let num = |i: &mut usize| {
                    let s = *i;
                    while *i < f.len() && f[*i].is_ascii_digit() {
                        *i += 1;
                    }
                    std::str::from_utf8(&f[s..*i])
                        .ok()
                        .and_then(|t| t.parse().ok())
                };
                let width = num(&mut i);
                let prec = if f.get(i) == Some(&b'.') {
                    i += 1;
                    Some(num(&mut i).unwrap_or(0))
                } else {
                    None
                };
                let Some(&conv) = f.get(i) else {
                    warn.push("warning: format directive `%' is incomplete".into());
                    out.push(Piece::Lit(&f[start..]));
                    lit = f.len();
                    break;
                };
                i += 1;
                let mut time = 0;
                if b"TACB".contains(&conv) {
                    match f.get(i) {
                        Some(&k) if TIME_LETTERS.contains(&k) => {
                            time = k;
                            i += 1;
                        }
                        Some(&k) => {
                            warn.push(format!(
                                "warning: unrecognized time format directive `{}'",
                                k as char
                            ));
                            i += 1;
                            out.push(Piece::Lit(&f[start..i]));
                            lit = i;
                            continue;
                        }
                        None => {
                            warn.push("warning: format directive `%T' is incomplete".into());
                            out.push(Piece::Lit(&f[start..]));
                            lit = f.len();
                            break;
                        }
                    }
                }
                if conv == b'%' {
                    out.push(Piece::Byte(b'%'));
                } else if b"aAbcCdDfFgGhHiklmMnpPsStTuUyYZB".contains(&conv) {
                    out.push(Piece::Dir {
                        flags,
                        width,
                        prec,
                        conv,
                        time,
                    });
                } else {
                    warn.push(format!(
                        "warning: unrecognized format directive `%{}'",
                        conv as char
                    ));
                    out.push(Piece::Lit(&f[start..i]));
                }
                lit = i;
            }
            _ => i += 1,
        }
    }
    out.push(Piece::Lit(&f[lit.min(f.len())..]));
    out
}

const TIME_LETTERS: &[u8] = b"@aAbBcCdDeFgGhHIjklmMnprRsStTuUVwWxXyYzZ+";

/// The warnings a format gives (GNU prints them once, before the walk).
pub fn check(f: &[u8]) -> Vec<String> {
    let mut w = Vec::new();
    pieces(f, &mut w);
    w
}

/// What a format needs fetched.
pub fn needs(f: &[u8]) -> Needs {
    let mut n = Needs::default();
    for p in pieces(f, &mut Vec::new()) {
        if let Piece::Dir { conv, .. } = p {
            match conv {
                b'a' | b'A' | b'c' | b'C' | b'B' | b'b' | b'k' | b'n' | b'S' => n.extra = true,
                b'l' => n.link = true,
                b'Y' => n.follow = true,
                _ => {}
            }
        }
    }
    n
}

/// The type letter of a kind.
pub fn type_letter(k: FileKind) -> u8 {
    match k {
        FileKind::File => b'f',
        FileKind::Dir => b'd',
        FileKind::Symlink => b'l',
        FileKind::Fifo => b'p',
        FileKind::Socket => b's',
        FileKind::CharDevice => b'c',
        FileKind::BlockDevice => b'b',
        _ => b'U',
    }
}

/// Formats one file.
pub fn format(ctx: &UtilCtx<'_>, f: &[u8], x: &Facts<'_>) -> Vec<u8> {
    let mut out = Vec::new();
    for p in pieces(f, &mut Vec::new()) {
        match p {
            Piece::Lit(l) => out.extend_from_slice(l),
            Piece::Byte(b) => out.push(b),
            Piece::Stop => break,
            Piece::Dir {
                flags,
                width,
                prec,
                conv,
                time,
            } => {
                let (text, numeric) = directive(ctx, x, conv, time, flags);
                pad(
                    &mut out,
                    &text,
                    flags,
                    width,
                    if numeric { None } else { prec },
                    numeric,
                );
            }
        }
    }
    out
}

fn pad(
    out: &mut Vec<u8>,
    text: &[u8],
    flags: &[u8],
    width: Option<usize>,
    prec: Option<usize>,
    numeric: bool,
) {
    let text = match prec {
        Some(p) if p < text.len() => &text[..p],
        _ => text,
    };
    let w = width.unwrap_or(0);
    let fill = w.saturating_sub(text.len());
    if flags.contains(&b'-') {
        out.extend_from_slice(text);
        out.extend(std::iter::repeat_n(b' ', fill));
    } else if numeric && flags.contains(&b'0') {
        out.extend(std::iter::repeat_n(b'0', fill));
        out.extend_from_slice(text);
    } else {
        out.extend(std::iter::repeat_n(b' ', fill));
        out.extend_from_slice(text);
    }
}

/// `%h` and `%f`: split at the last slash (GNU's rule: a root `a/` is `%h` `a`, `%f`
/// `a/`; a name without a slash is in `.`).
fn split(path: &[u8]) -> (&[u8], &[u8]) {
    match path.iter().rposition(|&b| b == b'/') {
        None => (b".", path),
        Some(0) => (b"/", if path.len() == 1 { path } else { &path[1..] }),
        Some(i) if i + 1 == path.len() => (&path[..i], path),
        Some(i) => (&path[..i], &path[i + 1..]),
    }
}

fn directive(
    ctx: &UtilCtx<'_>,
    x: &Facts<'_>,
    conv: u8,
    time: u8,
    flags: &[u8],
) -> (Vec<u8>, bool) {
    let m = x.meta;
    let extra = x.extra.unwrap_or_else(|| ExtraMetadata::derived(m));
    let num = |n: u64| (n.to_string().into_bytes(), true);
    match conv {
        b'p' => (x.path.to_vec(), false),
        b'f' => (split(x.path).1.to_vec(), false),
        b'h' => (split(x.path).0.to_vec(), false),
        b'H' => (x.root.to_vec(), false),
        b'P' => {
            let rest = x.path.strip_prefix(x.root).unwrap_or(x.path);
            (rest.strip_prefix(b"/").unwrap_or(rest).to_vec(), false)
        }
        b'd' => num(x.depth as u64),
        b's' => num(m.size),
        b'D' => num(m.dev),
        b'i' => num(m.ino),
        b'n' => num(extra.nlink),
        b'b' => num(extra.blocks),
        b'k' => num(extra.blocks.div_ceil(2)),
        b'U' => num(u64::from(m.uid)),
        b'G' => num(u64::from(m.gid)),
        b'u' => match ctx.user_name(m.uid) {
            Some(n) => (n, false),
            None => num(u64::from(m.uid)),
        },
        b'g' => match ctx.group_name(m.gid) {
            Some(n) => (n, false),
            None => num(u64::from(m.gid)),
        },
        b'm' => {
            let mut t = format!("{:o}", m.mode & 0o7777);
            if flags.contains(&b'#') {
                t.insert(0, '0');
            }
            (t.into_bytes(), true)
        }
        b'M' => (
            crate::modes::mode_string(m.kind, m.mode).into_bytes(),
            false,
        ),
        b'l' => (x.link.clone().unwrap_or_default(), false),
        b'y' => (vec![type_letter(m.kind)], false),
        b'Y' => (
            vec![x.followed.unwrap_or_else(|| type_letter(m.kind))],
            false,
        ),
        b'S' => {
            let s = if m.size == 0 {
                1.0
            } else {
                (extra.blocks as f64 * 512.0) / m.size as f64
            };
            (format!("{s}").into_bytes(), false)
        }
        b'F' => (b"unknown".to_vec(), false),
        b'Z' => (Vec::new(), false),
        b't' | b'a' | b'c' => {
            let ns = match conv {
                b't' => m.mtime_ns,
                b'a' => extra.atime_ns,
                _ => extra.ctime_ns,
            };
            let tm = timefmt::local(ctx, ns);
            (ctime(&tm).into_bytes(), false)
        }
        b'T' | b'A' | b'C' | b'B' => {
            let ns = match conv {
                b'T' => Some(m.mtime_ns),
                b'A' => Some(extra.atime_ns),
                b'C' => Some(extra.ctime_ns),
                _ => extra.btime_ns,
            };
            let Some(ns) = ns else {
                return (Vec::new(), false);
            };
            (strftime(ctx, ns, time).into_bytes(), false)
        }
        _ => (Vec::new(), false),
    }
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

fn frac(tm: &Tm) -> String {
    format!("{:09}0", tm.nsec)
}

/// `%t`: ctime's layout with GNU's fraction.
fn ctime(tm: &Tm) -> String {
    format!(
        "{} {} {:2} {:02}:{:02}:{:02}.{} {}",
        &DAYS[tm.wday as usize][..3],
        &MONTHS[tm.mon as usize - 1][..3],
        tm.day,
        tm.hour,
        tm.min,
        tm.sec,
        frac(tm),
        tm.year
    )
}

/// One `strftime` letter (the C locale), with GNU find's fractions.
fn strftime(ctx: &UtilCtx<'_>, ns: i64, k: u8) -> String {
    let tm = timefmt::local(ctx, ns);
    let h12 = if tm.hour.is_multiple_of(12) {
        12
    } else {
        tm.hour % 12
    };
    let off = tm.offset;
    match k {
        b'@' => format!("{}.{:09}0", ns.div_euclid(NS), ns.rem_euclid(NS)),
        b'a' => DAYS[tm.wday as usize][..3].to_string(),
        b'A' => DAYS[tm.wday as usize].to_string(),
        b'b' | b'h' => MONTHS[tm.mon as usize - 1][..3].to_string(),
        b'B' => MONTHS[tm.mon as usize - 1].to_string(),
        b'c' => format!(
            "{} {} {:2} {:02}:{:02}:{:02} {}",
            &DAYS[tm.wday as usize][..3],
            &MONTHS[tm.mon as usize - 1][..3],
            tm.day,
            tm.hour,
            tm.min,
            tm.sec,
            tm.year
        ),
        b'C' => format!("{:02}", tm.year.div_euclid(100)),
        b'd' => format!("{:02}", tm.day),
        b'D' | b'x' => format!("{:02}/{:02}/{:02}", tm.mon, tm.day, tm.year.rem_euclid(100)),
        b'e' => format!("{:2}", tm.day),
        b'F' => format!("{:04}-{:02}-{:02}", tm.year, tm.mon, tm.day),
        b'H' => format!("{:02}", tm.hour),
        b'I' => format!("{h12:02}"),
        b'j' => format!("{:03}", tm.yday + 1),
        b'k' => format!("{:2}", tm.hour),
        b'l' => format!("{h12:2}"),
        b'm' => format!("{:02}", tm.mon),
        b'M' => format!("{:02}", tm.min),
        b'n' => "\n".into(),
        b't' => "\t".into(),
        b'p' => if tm.hour < 12 { "AM" } else { "PM" }.into(),
        b'r' => format!(
            "{h12:02}:{:02}:{:02} {}",
            tm.min,
            tm.sec,
            if tm.hour < 12 { "AM" } else { "PM" }
        ),
        b'R' => format!("{:02}:{:02}", tm.hour, tm.min),
        b's' => ns.div_euclid(NS).to_string(),
        b'S' => format!("{:02}.{}", tm.sec, frac(&tm)),
        b'T' => format!("{:02}:{:02}:{:02}.{}", tm.hour, tm.min, tm.sec, frac(&tm)),
        b'X' => format!("{:02}:{:02}:{:02}", tm.hour, tm.min, tm.sec),
        b'u' => (if tm.wday == 0 { 7 } else { tm.wday }).to_string(),
        b'w' => tm.wday.to_string(),
        b'U' => format!("{:02}", (tm.yday + 7 - tm.wday) / 7),
        b'W' => format!("{:02}", (tm.yday + 7 - (tm.wday + 6) % 7) / 7),
        b'G' | b'g' | b'V' => {
            let (y, w) = iso_week(&tm);
            match k {
                b'G' => y.to_string(),
                b'g' => format!("{:02}", y.rem_euclid(100)),
                _ => format!("{w:02}"),
            }
        }
        b'y' => format!("{:02}", tm.year.rem_euclid(100)),
        b'Y' => tm.year.to_string(),
        b'z' => {
            let sign = if off < 0 { '-' } else { '+' };
            let a = off.unsigned_abs();
            format!("{sign}{:02}{:02}", a / 3600, a / 60 % 60)
        }
        b'Z' => match ctx.var(b"TZ") {
            Some(tz) if !tz.is_empty() && tz.iter().all(u8::is_ascii_alphabetic) => {
                String::from_utf8_lossy(tz).into_owned()
            }
            _ if off == 0 => "UTC".into(),
            _ => {
                let sign = if off < 0 { '-' } else { '+' };
                let a = off.unsigned_abs();
                format!("{sign}{:02}", a / 3600)
            }
        },
        b'+' => format!(
            "{:04}-{:02}-{:02}+{:02}:{:02}:{:02}.{}",
            tm.year,
            tm.mon,
            tm.day,
            tm.hour,
            tm.min,
            tm.sec,
            frac(&tm)
        ),
        _ => String::new(),
    }
}

/// The ISO 8601 year and week of a date.
fn iso_week(tm: &Tm) -> (i64, u32) {
    let wday = (tm.wday + 6) % 7; // Monday = 0
    let yday = i64::from(tm.yday);
    let week = (yday - i64::from(wday) + 10) / 7;
    let leap = |y: i64| (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let weeks_in = |y: i64| -> i64 {
        let jan1 = (timefmt::days_from_civil(y, 1, 1) + 3).rem_euclid(7); // Monday = 0
        if jan1 == 3 || (jan1 == 2 && leap(y)) {
            53
        } else {
            52
        }
    };
    if week < 1 {
        (tm.year - 1, weeks_in(tm.year - 1) as u32)
    } else if week > weeks_in(tm.year) {
        (tm.year + 1, 1)
    } else {
        (tm.year, week as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_like_gnu() {
        assert_eq!(split(b"a/"), (&b"a"[..], &b"a/"[..]));
        assert_eq!(split(b"a"), (&b"."[..], &b"a"[..]));
        assert_eq!(split(b"./a/b"), (&b"./a"[..], &b"b"[..]));
    }

    #[test]
    fn warns_once_per_problem() {
        assert_eq!(
            check(br"%Q\q%p"),
            vec![
                "warning: unrecognized format directive `%Q'".to_string(),
                "warning: unrecognized escape `\\q'".to_string()
            ]
        );
    }
}
