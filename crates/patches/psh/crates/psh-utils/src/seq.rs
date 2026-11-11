//! `seq [FIRST [INCREMENT]] LAST` with `-w -s -f` and BSD's `-t`. Written for psh after
//! GNU's documented behavior. Where GNU and BSD differ, GNU (D016): `seq 0` prints
//! nothing, the separator goes between numbers and a newline ends the output, `-s` is
//! taken literally (no backslash escapes), and decimals follow the operands (`seq 1 0.5
//! 2` prints `1.0 1.5 2.0`).

use psh_engine::util::UtilCtx;

use crate::common;
use crate::opts::{Spec, both, flag};

pub const SPEC: Spec = Spec {
    name: "seq",
    opts: &[
        both('f', "format").value(),
        both('s', "separator").value(),
        both('w', "equal-width"),
        flag('t').value(),
    ],
    posix_order: false,
};

/// `-5` and `-.5` are numbers, not options: `--` goes in front of the first one that is
/// not an option's value.
fn rewrite_negative(argv: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut out = argv.to_vec();
    let mut i = 1;
    while i < argv.len() {
        let a = &argv[i];
        if a == b"--" {
            break;
        }
        if a.len() > 1 && a[0] == b'-' && (a[1].is_ascii_digit() || a[1] == b'.') {
            out.insert(i, b"--".to_vec());
            break;
        }
        // An option whose value is the next word.
        let takes_next = match a.strip_prefix(b"--") {
            Some(l) => {
                !l.contains(&b'=')
                    && !l.is_empty()
                    && (b"format".starts_with(l) || b"separator".starts_with(l))
            }
            None => {
                a.len() > 1
                    && a[0] == b'-'
                    && a[1..].iter().position(|c| b"fst".contains(c)) == Some(a.len() - 2)
            }
        };
        i += if takes_next { 2 } else { 1 };
    }
    out
}

/// One operand: its value, whether it is an integer as written, and the decimals it
/// shows (`1.50` 2, `1e-3` 3).
#[derive(Clone, Copy)]
struct Num {
    value: f64,
    int: Option<i128>,
    decimals: usize,
}

fn parse_num(s: &[u8]) -> Option<Num> {
    let t = std::str::from_utf8(s).ok()?.trim_start();
    let body = t.strip_prefix(['+', '-']).unwrap_or(t);
    let lower = body.to_ascii_lowercase();
    if lower == "inf" || lower == "infinity" {
        let value = if t.starts_with('-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
        return Some(Num {
            value,
            int: None,
            decimals: 0,
        });
    }
    let (mant, exp) = match body.find(['e', 'E']) {
        Some(i) => (&body[..i], Some(&body[i + 1..])),
        None => (body, None),
    };
    let (ip, fp) = match mant.split_once('.') {
        Some((a, b)) => (a, Some(b)),
        None => (mant, None),
    };
    let digits = |x: &str| x.bytes().all(|b| b.is_ascii_digit());
    if !digits(ip) || !fp.is_none_or(digits) || ip.is_empty() && fp.is_none_or(str::is_empty) {
        return None;
    }
    let exp: i64 = match exp {
        Some(e) => {
            let d = e.strip_prefix(['+', '-']).unwrap_or(e);
            if d.is_empty() || !digits(d) {
                return None;
            }
            e.parse().ok()?
        }
        None => 0,
    };
    let value: f64 = t.parse().ok()?;
    let frac = fp.map_or(0, str::len) as i64;
    let int = if fp.is_none() && exp == 0 {
        t.parse::<i128>().ok()
    } else {
        None
    };
    Some(Num {
        value,
        int,
        decimals: (frac - exp).max(0) as usize,
    })
}

/// A `-f` format: text around one floating-point conversion.
struct Format {
    prefix: String,
    suffix: String,
    left: bool,
    plus: bool,
    space: bool,
    alt: bool,
    zero: bool,
    width: usize,
    prec: Option<usize>,
    conv: char,
}

fn parse_format(f: &str, utf8: bool) -> Result<Format, String> {
    let qf = common::quote_in(utf8, f.as_bytes());
    let mut prefix = String::new();
    let mut chars = f.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c != '%' {
            prefix.push(c);
            continue;
        }
        if chars.peek().map(|&(_, c)| c) == Some('%') {
            chars.next();
            prefix.push('%');
            continue;
        }
        let rest = &f[i + 1..];
        let b = rest.as_bytes();
        let mut j = 0;
        let mut fmt = Format {
            prefix: String::new(),
            suffix: String::new(),
            left: false,
            plus: false,
            space: false,
            alt: false,
            zero: false,
            width: 0,
            prec: None,
            conv: 'g',
        };
        while j < b.len() {
            match b[j] {
                b'-' => fmt.left = true,
                b'+' => fmt.plus = true,
                b' ' => fmt.space = true,
                b'#' => fmt.alt = true,
                b'0' => fmt.zero = true,
                b'\'' => {}
                _ => break,
            }
            j += 1;
        }
        let start = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        fmt.width = rest[start..j].parse().unwrap_or(0);
        if j < b.len() && b[j] == b'.' {
            j += 1;
            let start = j;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            fmt.prec = Some(rest[start..j].parse().unwrap_or(0));
        }
        // Length modifiers GNU allows (`%Lg`).
        if j < b.len() && b[j] == b'L' {
            j += 1;
        }
        let Some(conv) = rest[j..].chars().next() else {
            return Err(format!("format {qf} ends in %"));
        };
        if !"eEfFgG".contains(conv) {
            return Err(format!("format {qf} has unknown %{conv} directive"));
        }
        fmt.conv = conv;
        let tail = &rest[j + conv.len_utf8()..];
        let mut suffix = String::new();
        let mut it = tail.chars().peekable();
        while let Some(c) = it.next() {
            if c == '%' {
                if it.peek() == Some(&'%') {
                    it.next();
                    suffix.push('%');
                    continue;
                }
                return Err(format!("format {qf} has too many % directives"));
            }
            suffix.push(c);
        }
        fmt.prefix = prefix;
        fmt.suffix = suffix;
        return Ok(fmt);
    }
    Err(format!("format {qf} has no % directive"))
}

impl Format {
    fn apply(&self, v: f64) -> String {
        let prec = self.prec.unwrap_or(6);
        let lower = self.conv.to_ascii_lowercase();
        let mut body = match lower {
            'f' if !v.is_finite() => fmt_e(v, 0),
            'f' if self.alt && prec == 0 => format!("{v:.0}."),
            'f' => format!("{v:.prec$}"),
            'e' => fmt_e(v, prec),
            _ => fmt_g(v, prec, self.alt),
        };
        if self.conv.is_ascii_uppercase() {
            body = body.to_uppercase();
        }
        if !body.starts_with('-') {
            if self.plus {
                body.insert(0, '+');
            } else if self.space {
                body.insert(0, ' ');
            }
        }
        let len = body.chars().count();
        if len < self.width {
            let fill = self.width - len;
            if self.left {
                body.push_str(&" ".repeat(fill));
            } else if self.zero && v.is_finite() {
                let sign = usize::from(body.starts_with(['-', '+', ' ']));
                body.insert_str(sign, &"0".repeat(fill));
            } else {
                body.insert_str(0, &" ".repeat(fill));
            }
        }
        format!("{}{body}{}", self.prefix, self.suffix)
    }
}

/// C's `%e`: `d.dddddde+XX`. (As in the engine's `printf`.)
fn fmt_e(v: f64, prec: usize) -> String {
    if !v.is_finite() {
        return if v.is_nan() {
            "nan".into()
        } else if v > 0.0 {
            "inf".into()
        } else {
            "-inf".into()
        };
    }
    let s = format!("{v:.prec$e}");
    let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let sign = if exp < 0 { '-' } else { '+' };
    format!("{mant}e{sign}{:02}", exp.abs())
}

/// C's `%g`. (As in the engine's `printf`.)
fn fmt_g(v: f64, prec: usize, alt: bool) -> String {
    if !v.is_finite() {
        return fmt_e(v, 0);
    }
    let p = prec.max(1);
    let exp = if v == 0.0 {
        0
    } else {
        let e = format!("{v:.*e}", p - 1);
        e.split_once('e')
            .and_then(|(_, x)| x.parse::<i32>().ok())
            .unwrap_or(0)
    };
    let mut out = if exp < -4 || exp >= p as i32 {
        fmt_e(v, p - 1)
    } else {
        let decimals = (p as i32 - 1 - exp).max(0) as usize;
        format!("{v:.decimals$}")
    };
    if !alt {
        let (num, suffix) = match out.find('e') {
            Some(i) => (out[..i].to_string(), out[i..].to_string()),
            None => (out.clone(), String::new()),
        };
        let num = if num.contains('.') {
            num.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            num
        };
        out = format!("{num}{suffix}");
    }
    out
}

/// Pads a formatted number with zeros after its sign to `width`.
fn zero_pad(s: String, width: usize) -> String {
    if s.len() >= width {
        return s;
    }
    let sign = usize::from(s.starts_with('-'));
    let mut s = s;
    s.insert_str(sign, &"0".repeat(width - s.len()));
    s
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let argv = rewrite_negative(argv);
    let p = match common::parse(ctx, &SPEC, &argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let sep = p.last(&SPEC, "separator").unwrap_or(b"\n").to_vec();
    let term = p.last(&SPEC, "t").unwrap_or(b"\n").to_vec();
    let equal = p.has(&SPEC, "equal-width");
    let format = match p.last(&SPEC, "format") {
        Some(f) => {
            if equal {
                return common::usage(
                    ctx,
                    "format string may not be specified when printing equal width strings",
                )
                .await;
            }
            match parse_format(&String::from_utf8_lossy(f), ctx.utf8()) {
                Ok(f) => Some(f),
                Err(e) => {
                    ctx.error(&e).await;
                    return 1;
                }
            }
        }
        None => None,
    };
    let ops = &p.operands;
    if ops.is_empty() {
        return common::usage(ctx, "missing operand").await;
    }
    if ops.len() > 3 {
        let extra = common::quote(ctx, &ops[3]);
        return common::usage(ctx, &format!("extra operand {extra}")).await;
    }
    let mut nums = Vec::new();
    for o in ops {
        match parse_num(o) {
            Some(n) if !n.value.is_nan() => nums.push(n),
            _ => {
                let o = common::quote(ctx, o);
                return common::usage(ctx, &format!("invalid floating point argument: {o}")).await;
            }
        }
    }
    let one = Num {
        value: 1.0,
        int: Some(1),
        decimals: 0,
    };
    let (first, step, last) = match nums.as_slice() {
        [l] => (one, one, *l),
        [f, l] => (*f, one, *l),
        [f, s, l] => (*f, *s, *l),
        _ => unreachable!(),
    };
    if step.value == 0.0 {
        let s = common::quote(ctx, &ops[1]);
        return common::usage(ctx, &format!("invalid Zero increment value: {s}")).await;
    }
    let mut out = Vec::new();
    let mut count = 0u64;
    macro_rules! emit {
        ($s:expr) => {{
            if count > 0 {
                out.extend_from_slice(&sep);
            }
            out.extend_from_slice($s.as_bytes());
            count += 1;
            if out.len() >= 32 * 1024 {
                if let Err(e) = ctx.write(&out).await {
                    return common::write_failed(ctx, &e).await;
                }
                out.clear();
                ctx.check().await;
            }
        }};
    }
    if let (None, Some(f), Some(s), Some(l)) = (&format, first.int, step.int, last.int) {
        // Integers stay exact.
        let width = if equal {
            f.to_string().len().max(l.to_string().len())
        } else {
            0
        };
        let mut v = f;
        while (s > 0 && v <= l) || (s < 0 && v >= l) {
            emit!(zero_pad(v.to_string(), width));
            match v.checked_add(s) {
                Some(n) => v = n,
                None => break,
            }
        }
    } else {
        let decimals = first.decimals.max(step.decimals);
        let plain = |v: f64| {
            if v.is_finite() {
                format!("{v:.decimals$}")
            } else {
                fmt_e(v, 0)
            }
        };
        let width = if equal {
            plain(first.value).len().max(plain(last.value).len())
        } else {
            0
        };
        let show = |v: f64| match &format {
            Some(f) => f.apply(v),
            None => zero_pad(plain(v), width),
        };
        let last_shown = show(last.value);
        let mut i: u64 = 0;
        loop {
            let v = first.value + i as f64 * step.value;
            let past = if step.value > 0.0 {
                v > last.value
            } else {
                v < last.value
            };
            if past {
                // Rounding (0.1 * 3) must not lose the last number when it prints
                // the same as LAST.
                let s = show(v);
                if s == last_shown {
                    emit!(s);
                }
                break;
            }
            emit!(show(v));
            if v == last.value {
                break;
            }
            i += 1;
        }
    }
    if count > 0 {
        out.extend_from_slice(&term);
    }
    if let Err(e) = ctx.write(&out).await {
        return common::write_failed(ctx, &e).await;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<Vec<u8>> {
        a.iter().map(|s| s.as_bytes().to_vec()).collect()
    }

    #[test]
    fn negative_operands() {
        assert_eq!(
            rewrite_negative(&args(&["seq", "-w", "-5", "5"])),
            args(&["seq", "-w", "--", "-5", "5"])
        );
        assert_eq!(
            rewrite_negative(&args(&["seq", "-s", "-1", "-2", "3"])),
            args(&["seq", "-s", "-1", "--", "-2", "3"])
        );
        assert_eq!(
            rewrite_negative(&args(&["seq", "1", "3"])),
            args(&["seq", "1", "3"])
        );
    }

    #[test]
    fn numbers() {
        let n = parse_num(b"1.50").unwrap();
        assert_eq!((n.value, n.int, n.decimals), (1.5, None, 2));
        let n = parse_num(b"1e-3").unwrap();
        assert_eq!(n.decimals, 3);
        assert_eq!(parse_num(b"-7").unwrap().int, Some(-7));
        assert!(parse_num(b"x").is_none());
        assert!(parse_num(b".").is_none());
        assert!(parse_num(b"1e").is_none());
    }

    #[test]
    fn formats() {
        let f = parse_format("x%%%5.1fy", false).unwrap();
        assert_eq!(f.apply(1.0), "x%  1.0y");
        assert_eq!(parse_format("%03g", false).unwrap().apply(1.0), "001");
        assert_eq!(
            parse_format("%e", false).unwrap().apply(2.0),
            "2.000000e+00"
        );
        assert!(parse_format("%d", false).is_err());
        assert!(parse_format("%g %g", false).is_err());
        assert!(parse_format("abc", false).is_err());
    }
}
