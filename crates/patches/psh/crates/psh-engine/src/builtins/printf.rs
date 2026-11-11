//! `printf [-v var] format [arguments]`: the format is reused until the arguments are
//! consumed; conversions `d i o u x X c s b q f F e E g G %`.

use crate::escape::{Dialect, decode};
use crate::state::{R, Vp, is_name};

use super::{out, shell_quote};

fn s(b: &[u8]) -> std::borrow::Cow<'_, str> {
    String::from_utf8_lossy(b)
}

pub(super) async fn printf(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    let mut i = 1;
    let mut var: Option<String> = None;
    while i < args.len() {
        match &args[i][..] {
            b"--" => {
                i += 1;
                break;
            }
            a if a.starts_with(b"-v") => {
                // `-v NAME` or `-vNAME`.
                let attached = a.len() > 2;
                let Some(n) = (if attached {
                    Some(&a[2..])
                } else {
                    args.get(i + 1).map(Vec::as_slice)
                }) else {
                    vp.error("printf: -v: option requires an argument").await;
                    vp.error_raw(b"printf: usage: printf [-v var] format [arguments]\n")
                        .await;
                    return Ok(2);
                };
                if !valid_target(n) {
                    vp.error(&format!("printf: `{}': not a valid identifier", s(n)))
                        .await;
                    return Ok(2);
                }
                var = Some(s(n).into_owned());
                i += if attached { 1 } else { 2 };
            }
            a if a.len() > 1 && a[0] == b'-' => {
                vp.error(&format!("printf: {}: invalid option", s(&a[..2])))
                    .await;
                vp.error_raw(b"printf: usage: printf [-v var] format [arguments]\n")
                    .await;
                return Ok(2);
            }
            _ => break,
        }
    }
    let Some(format) = args.get(i) else {
        vp.error_raw(b"printf: usage: printf [-v var] format [arguments]\n")
            .await;
        return Ok(2);
    };
    // `%(…)T` needs the time zone, read before formatting.
    let zone = if format.windows(2).any(|w| w == b"%(") {
        Some(super::strftime::zone(vp).await)
    } else {
        None
    };
    let mut f = Formatter {
        zone,
        now: vp.host().now().as_secs() as i64,
        args: &args[i + 1..],
        next: 0,
        out: Vec::new(),
        errors: Vec::new(),
        stop: false,
    };
    loop {
        let before = f.next;
        f.run(format);
        if f.stop || f.next >= f.args.len() || f.next == before {
            break;
        }
    }
    let status = i32::from(!f.errors.is_empty());
    for e in &f.errors {
        vp.error(&format!("printf: {e}")).await;
    }
    match var {
        Some(n) => {
            // `NAME` or `NAME[SUB]`, with the variable's attributes.
            let r = match n.split_once('[') {
                Some((base, sub)) => {
                    let sub = sub.strip_suffix(']').unwrap_or(sub);
                    vp.assign_elem(base, sub.as_bytes(), f.out, false)
                }
                None => vp.assign(&n, f.out),
            };
            if let Err(msg) = r {
                vp.error(&msg).await;
                return Ok(1);
            }
            Ok(status)
        }
        None => {
            let st = out(vp, "printf", &f.out).await?;
            Ok(if st != 0 { st } else { status })
        }
    }
}

/// `%#q`: always single quotes (`'it'\''s'`), unless control characters need `$'…'`.
fn single_quote(v: &[u8]) -> Vec<u8> {
    if v.iter().any(u8::is_ascii_control) {
        return shell_quote(v);
    }
    let mut out = vec![b'\''];
    for &b in v {
        if b == b'\'' {
            out.extend_from_slice(b"'\\''");
        } else {
            out.push(b);
        }
    }
    out.push(b'\'');
    out
}

/// `-v`'s target: a name, or a name with a non-empty subscript ending the word.
fn valid_target(n: &[u8]) -> bool {
    match n.iter().position(|&b| b == b'[') {
        None => is_name(n),
        Some(open) => {
            is_name(&n[..open]) && n.len() > open + 2 && n.last() == Some(&b']') && {
                let sub = &n[open + 1..n.len() - 1];
                !sub.contains(&b']')
            }
        }
    }
}

struct Formatter<'a> {
    zone: Option<super::strftime::Zone>,
    /// The time for `%(…)T` with `-1`, `-2` or no argument.
    now: i64,
    args: &'a [Vec<u8>],
    next: usize,
    out: Vec<u8>,
    errors: Vec<String>,
    stop: bool,
}

#[derive(Default)]
struct Spec {
    left: bool,
    zero: bool,
    plus: bool,
    space: bool,
    alt: bool,
    width: Option<usize>,
    prec: Option<usize>,
}

impl Formatter<'_> {
    fn arg(&mut self) -> Option<&[u8]> {
        let a = self.args.get(self.next);
        if a.is_some() {
            self.next += 1;
        }
        a.map(Vec::as_slice)
    }

    /// An integer argument for a signed conversion (also `*` widths): clamped to
    /// the range of `i64` with an error, as `strtoimax` does.
    fn num_arg(&mut self) -> i64 {
        let Some(a) = self.arg().map(<[u8]>::to_vec) else {
            return 0;
        };
        let n = parse_num(&a);
        if !n.ok {
            self.errors.push(format!("{}: invalid number", s(&a)));
        }
        let limit = if n.neg {
            1u128 << 63
        } else {
            (1u128 << 63) - 1
        };
        if n.overflow || n.mag > limit {
            self.errors
                .push(format!("{}: Numerical result out of range", s(&a)));
            return if n.neg { i64::MIN } else { i64::MAX };
        }
        if n.neg {
            (n.mag as i128).wrapping_neg() as i64
        } else {
            n.mag as i64
        }
    }

    /// An integer argument for an unsigned conversion: negative values wrap around,
    /// as `strtoumax` does.
    fn unsigned_arg(&mut self) -> u64 {
        let Some(a) = self.arg().map(<[u8]>::to_vec) else {
            return 0;
        };
        let n = parse_num(&a);
        if !n.ok {
            self.errors.push(format!("{}: invalid number", s(&a)));
        }
        if n.overflow {
            self.errors
                .push(format!("{}: Numerical result out of range", s(&a)));
            return u64::MAX;
        }
        let v = n.mag as u64;
        if n.neg { v.wrapping_neg() } else { v }
    }

    fn float_arg(&mut self) -> f64 {
        match self.arg() {
            None => 0.0,
            Some(a) => {
                let a = a.to_vec();
                let t = s(&a);
                let t = t.trim();
                if let Some(c) = t.strip_prefix(['\'', '"']) {
                    return c.chars().next().map_or(0.0, |c| f64::from(c as u32));
                }
                match t.parse::<f64>() {
                    Ok(v) => v,
                    Err(_) => {
                        let n = parse_num(&a);
                        if !n.ok {
                            self.errors.push(format!("{}: invalid number", s(&a)));
                        }
                        if n.neg { -(n.mag as f64) } else { n.mag as f64 }
                    }
                }
            }
        }
    }

    fn run(&mut self, fmt: &[u8]) {
        let mut i = 0;
        while i < fmt.len() && !self.stop {
            let c = fmt[i];
            if c == b'\\' {
                // One escape at a time, so `\c` is literal here and octal is `\nnn`.
                let mut j = i + 1;
                if j < fmt.len() {
                    j += 1;
                    let lead = fmt[i + 1];
                    let max = match lead {
                        b'0'..=b'7' => 2,
                        b'x' => 2,
                        b'u' => 4,
                        b'U' => 8,
                        _ => 0,
                    };
                    let mut n = 0;
                    while n < max && j < fmt.len() && fmt[j].is_ascii_hexdigit() {
                        if matches!(lead, b'0'..=b'7') && !(b'0'..=b'7').contains(&fmt[j]) {
                            break;
                        }
                        j += 1;
                        n += 1;
                    }
                }
                self.out
                    .extend_from_slice(&decode(&fmt[i..j], Dialect::Printf).bytes);
                i = j;
                continue;
            }
            if c != b'%' {
                self.out.push(c);
                i += 1;
                continue;
            }
            i += 1;
            if i >= fmt.len() {
                self.out.push(b'%');
                break;
            }
            if fmt[i] == b'%' {
                self.out.push(b'%');
                i += 1;
                continue;
            }
            let start = i - 1;
            let mut spec = Spec::default();
            while i < fmt.len() {
                match fmt[i] {
                    b'-' => spec.left = true,
                    b'0' => spec.zero = true,
                    b'+' => spec.plus = true,
                    b' ' => spec.space = true,
                    b'#' => spec.alt = true,
                    _ => break,
                }
                i += 1;
            }
            if i < fmt.len() && fmt[i] == b'*' {
                let w = self.num_arg();
                if w < 0 {
                    spec.left = true;
                }
                spec.width = Some(w.unsigned_abs() as usize);
                i += 1;
            } else {
                let st = i;
                while i < fmt.len() && fmt[i].is_ascii_digit() {
                    i += 1;
                }
                if i > st {
                    spec.width = s(&fmt[st..i]).parse().ok();
                }
            }
            if i < fmt.len() && fmt[i] == b'.' {
                i += 1;
                if i < fmt.len() && fmt[i] == b'*' {
                    spec.prec = Some(self.num_arg().max(0) as usize);
                    i += 1;
                } else {
                    let st = i;
                    while i < fmt.len() && fmt[i].is_ascii_digit() {
                        i += 1;
                    }
                    spec.prec = Some(s(&fmt[st..i]).parse().unwrap_or(0));
                }
            }
            if fmt.get(i) == Some(&b'(') {
                // `%(strftime format)T`
                let Some(close) = fmt[i..].iter().position(|&b| b == b')') else {
                    self.errors
                        .push(format!("`{}': missing format character", s(&fmt[start..])));
                    self.stop = true;
                    break;
                };
                let tfmt = fmt[i + 1..i + close].to_vec();
                i += close + 1;
                if fmt.get(i) != Some(&b'T') {
                    let shown = fmt
                        .get(i)
                        .map_or(String::new(), |&c| (c as char).to_string());
                    self.errors
                        .push(format!("`{shown}': invalid format character"));
                    self.stop = true;
                    break;
                }
                i += 1;
                self.time(&tfmt, &spec);
                continue;
            }
            // Length modifiers are accepted and ignored.
            while i < fmt.len() && b"hlLjzt".contains(&fmt[i]) {
                i += 1;
            }
            let Some(&conv) = fmt.get(i) else {
                self.errors
                    .push(format!("`{}': missing format character", s(&fmt[start..])));
                self.stop = true;
                break;
            };
            i += 1;
            self.convert(conv, &spec);
        }
    }

    /// `%(fmt)T`: an argument of seconds since the epoch (`-1` or none: now, `-2`:
    /// when the shell started) formatted with `strftime`.
    fn time(&mut self, tfmt: &[u8], spec: &Spec) {
        let t = match self.arg().map(<[u8]>::to_vec) {
            None => self.now,
            Some(a) if a.is_empty() => self.now,
            Some(a) => {
                let n = parse_num(&a);
                if !n.ok {
                    self.errors.push(format!("{}: invalid number", s(&a)));
                }
                let v = if n.neg { -(n.mag as i64) } else { n.mag as i64 };
                if v == -1 || v == -2 { self.now } else { v }
            }
        };
        let utc = super::strftime::Zone::utc();
        let zone = self.zone.as_ref().unwrap_or(&utc);
        let mut body = super::strftime::strftime(tfmt, zone, t);
        // bash formats into a buffer of 128 bytes; what does not fit is dropped.
        if body.len() >= 128 {
            body.clear();
        }
        if let Some(p) = spec.prec {
            body.truncate(p);
        }
        self.pad(&body, spec, false);
    }

    fn pad(&mut self, body: &[u8], spec: &Spec, numeric: bool) {
        let w = spec.width.unwrap_or(0);
        if body.len() >= w {
            self.out.extend_from_slice(body);
            return;
        }
        let fill = w - body.len();
        if spec.left {
            self.out.extend_from_slice(body);
            self.out.extend(std::iter::repeat_n(b' ', fill));
        } else if spec.zero && numeric {
            // Zeros go after the sign or 0x prefix.
            let split = if body.starts_with(b"0x") || body.starts_with(b"0X") {
                2
            } else {
                usize::from(matches!(body.first(), Some(b'-' | b'+' | b' ')))
            };
            self.out.extend_from_slice(&body[..split]);
            self.out.extend(std::iter::repeat_n(b'0', fill));
            self.out.extend_from_slice(&body[split..]);
        } else {
            self.out.extend(std::iter::repeat_n(b' ', fill));
            self.out.extend_from_slice(body);
        }
    }

    fn convert(&mut self, conv: u8, spec: &Spec) {
        match conv {
            b's' | b'b' | b'q' | b'Q' => {
                let mut a = self.arg().map(<[u8]>::to_vec).unwrap_or_default();
                // `%Q`: the precision cuts the argument, before it is quoted.
                if conv == b'Q'
                    && let Some(p) = spec.prec
                {
                    a.truncate(p);
                }
                let quote = |a: &[u8]| {
                    if spec.alt {
                        single_quote(a)
                    } else {
                        shell_quote(a)
                    }
                };
                let mut body = match conv {
                    b's' => a,
                    b'q' => quote(&a),
                    b'Q' => {
                        let q = quote(&a);
                        self.pad(&q, spec, false);
                        return;
                    }
                    _ => {
                        let d = decode(&a, Dialect::PercentB);
                        if d.stop {
                            self.stop = true;
                        }
                        d.bytes
                    }
                };
                if let Some(p) = spec.prec {
                    body.truncate(p);
                }
                self.pad(&body, spec, false);
            }
            b'c' => {
                let a = self.arg().map(<[u8]>::to_vec).unwrap_or_default();
                let body: Vec<u8> = a.first().map(|b| vec![*b]).unwrap_or_default();
                self.pad(&body, spec, false);
            }
            b'd' | b'i' => {
                let v = self.num_arg();
                let mut digits = v.unsigned_abs().to_string();
                if let Some(p) = spec.prec {
                    while digits.len() < p {
                        digits.insert(0, '0');
                    }
                    if p == 0 && v == 0 {
                        digits.clear();
                    }
                }
                let sign = if v < 0 {
                    "-"
                } else if spec.plus {
                    "+"
                } else if spec.space {
                    " "
                } else {
                    ""
                };
                let body = format!("{sign}{digits}");
                let numeric = spec.prec.is_none();
                self.pad(body.as_bytes(), spec, numeric);
            }
            b'u' | b'o' | b'x' | b'X' => {
                let v = self.unsigned_arg();
                let mut digits = match conv {
                    b'u' => v.to_string(),
                    b'o' => format!("{v:o}"),
                    b'x' => format!("{v:x}"),
                    _ => format!("{v:X}"),
                };
                if let Some(p) = spec.prec {
                    while digits.len() < p {
                        digits.insert(0, '0');
                    }
                }
                if spec.alt && v != 0 {
                    match conv {
                        b'o' if !digits.starts_with('0') => digits.insert(0, '0'),
                        b'x' => digits.insert_str(0, "0x"),
                        b'X' => digits.insert_str(0, "0X"),
                        _ => {}
                    }
                }
                let numeric = spec.prec.is_none();
                self.pad(digits.as_bytes(), spec, numeric);
            }
            b'f' | b'F' | b'e' | b'E' | b'g' | b'G' => {
                let v = self.float_arg();
                let prec = spec.prec.unwrap_or(6);
                let mut body = match conv {
                    // `#` keeps the point even without decimals.
                    b'f' | b'F' if spec.alt && prec == 0 && v.is_finite() => format!("{v:.0}."),
                    b'f' | b'F' => format!("{v:.prec$}"),
                    b'e' | b'E' if spec.alt && prec == 0 && v.is_finite() => {
                        fmt_e(v, 0).replacen('e', ".e", 1)
                    }
                    b'e' | b'E' => fmt_e(v, prec),
                    _ => fmt_g(v, prec, spec.alt),
                };
                if conv.is_ascii_uppercase() {
                    body = body.to_uppercase();
                }
                if v >= 0.0 && !body.starts_with('-') {
                    if spec.plus {
                        body.insert(0, '+');
                    } else if spec.space {
                        body.insert(0, ' ');
                    }
                }
                self.pad(body.as_bytes(), spec, true);
            }
            other => {
                self.errors
                    .push(format!("`{}': invalid format character", other as char));
                self.stop = true;
            }
        }
    }
}

/// C's `%e`: `d.dddddde+XX`.
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

/// C's `%g`: the shorter of `%e` and `%f` with `prec` significant digits, trailing
/// zeros removed unless `#`.
fn fmt_g(v: f64, prec: usize, alt: bool) -> String {
    if !v.is_finite() {
        return fmt_e(v, 0);
    }
    let p = if prec == 0 { 1 } else { prec };
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

struct Num {
    neg: bool,
    /// The magnitude, if it fits in 64 bits.
    mag: u128,
    /// More than 64 bits.
    overflow: bool,
    /// The whole argument was a number.
    ok: bool,
}

/// Parses an integer argument like `strtoimax` with base 0, plus `'c` for a char code.
fn parse_num(a: &[u8]) -> Num {
    let t = s(a);
    let t = t.trim_start();
    if let Some(c) = t.strip_prefix(['\'', '"']) {
        let v = c.chars().next().map_or(0, |c| u128::from(u32::from(c)));
        return Num {
            neg: false,
            mag: v,
            overflow: false,
            ok: true,
        };
    }
    let (neg, body) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let (radix, digits) =
        if let Some(h) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
            (16, h)
        } else if body.len() > 1 && body.starts_with('0') {
            (8, &body[1..])
        } else {
            (10, body)
        };
    let end = digits
        .find(|c: char| !c.is_digit(radix))
        .unwrap_or(digits.len());
    let mut mag = 0u128;
    let mut overflow = false;
    for c in digits[..end].chars() {
        mag = mag * u128::from(radix) + u128::from(c.to_digit(radix).unwrap_or(0));
        if mag > u128::from(u64::MAX) {
            overflow = true;
            mag = u128::from(u64::MAX);
        }
    }
    let ok = end == digits.len() && (end > 0 || radix == 8) && !t.is_empty();
    Num {
        neg,
        mag,
        overflow,
        ok,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats() {
        assert_eq!(fmt_e(1234.5, 6), "1.234500e+03");
        assert_eq!(fmt_g(0.0001, 6, false), "0.0001");
        assert_eq!(fmt_g(1e10, 6, false), "1e+10");
        assert_eq!(fmt_g(3.0, 6, false), "3");
    }

    #[test]
    fn numbers() {
        let p = |a: &[u8]| {
            let n = parse_num(a);
            (n.mag, n.ok)
        };
        assert_eq!(p(b"0x1f"), (31, true));
        assert_eq!(p(b"010"), (8, true));
        assert_eq!(p(b"'A"), (65, true));
        assert_eq!(p(b"12abc"), (12, false));
        assert!(parse_num(b"18446744073709551616").overflow);
        assert!(!parse_num(b"18446744073709551615").overflow);
    }
}
