//! awk's `printf` and `sprintf` formats: C's conversions over awk values, with
//! widths and precisions counted in characters in a UTF-8 locale (as gawk counts them).

use super::value::{S, Value, int_str, special_str};

/// What formatting needs of the interpreter: the locale and string conversion (CONVFMT).
pub struct Env<'a> {
    pub utf8: bool,
    pub to_str: &'a dyn Fn(&Value) -> S,
}

/// The error of a format that wants more arguments than it has.
pub const NOT_ENOUGH: &str = "not enough arguments to satisfy format string";

pub fn sprintf(env: &Env, fmt: &[u8], args: &[Value]) -> Result<Vec<u8>, &'static str> {
    let mut out = Vec::with_capacity(fmt.len() + 16);
    let mut args = args.iter();
    let mut i = 0;
    while i < fmt.len() {
        let c = fmt[i];
        if c != b'%' {
            // Copy the literal run.
            let next = fmt[i..]
                .iter()
                .position(|&b| b == b'%')
                .map_or(fmt.len(), |p| i + p);
            out.extend_from_slice(&fmt[i..next]);
            i = next;
            continue;
        }
        let start = i;
        i += 1;
        if i >= fmt.len() {
            out.push(b'%');
            break;
        }
        if fmt[i] == b'%' {
            out.push(b'%');
            i += 1;
            continue;
        }
        let mut spec = Spec::default();
        while i < fmt.len() {
            match fmt[i] {
                b'-' => spec.left = true,
                b'+' => spec.plus = true,
                b' ' => spec.space = true,
                b'#' => spec.alt = true,
                b'0' => spec.zero = true,
                b'\'' => {}
                _ => break,
            }
            i += 1;
        }
        if i < fmt.len() && fmt[i] == b'*' {
            i += 1;
            let w = args.next().ok_or(NOT_ENOUGH)?.num();
            if w < 0.0 {
                spec.left = true;
            }
            spec.width = w.abs().min(1e6) as usize;
        } else {
            while i < fmt.len() && fmt[i].is_ascii_digit() {
                spec.width = (spec.width * 10 + (fmt[i] - b'0') as usize).min(1_000_000);
                i += 1;
            }
        }
        if i < fmt.len() && fmt[i] == b'.' {
            i += 1;
            if i < fmt.len() && fmt[i] == b'*' {
                i += 1;
                let p = args.next().ok_or(NOT_ENOUGH)?.num();
                spec.prec = (p >= 0.0).then_some(p.min(1e6) as usize);
            } else {
                let mut p = 0usize;
                while i < fmt.len() && fmt[i].is_ascii_digit() {
                    p = (p * 10 + (fmt[i] - b'0') as usize).min(1_000_000);
                    i += 1;
                }
                spec.prec = Some(p);
            }
        }
        // Length modifiers mean nothing in awk.
        while i < fmt.len() && matches!(fmt[i], b'h' | b'l' | b'L' | b'q' | b'j' | b'z' | b't') {
            i += 1;
        }
        if i >= fmt.len() {
            // An incomplete conversion is copied as it is.
            out.extend_from_slice(&fmt[start..]);
            break;
        }
        let conv = fmt[i];
        i += 1;
        match conv {
            b'd' | b'i' | b'o' | b'x' | b'X' | b'u' | b'e' | b'E' | b'f' | b'F' | b'g' | b'G'
            | b'a' | b'A' | b'c' | b's' => {
                let arg = args.next().ok_or(NOT_ENOUGH)?;
                format_one(env, &mut out, &spec, conv, arg);
            }
            _ => {
                // Not a conversion: copied as it is (gawk).
                out.extend_from_slice(&fmt[start..i]);
            }
        }
    }
    Ok(out)
}

#[derive(Default, Clone)]
pub struct Spec {
    pub left: bool,
    pub plus: bool,
    pub space: bool,
    pub alt: bool,
    pub zero: bool,
    pub width: usize,
    pub prec: Option<usize>,
}

fn char_len(env: &Env, s: &[u8]) -> usize {
    if env.utf8 {
        crate::textio::char_lengths(s).count()
    } else {
        s.len()
    }
}

/// The first `n` characters of `s`.
fn prefix_chars<'a>(env: &Env, s: &'a [u8], n: usize) -> &'a [u8] {
    if !env.utf8 {
        return &s[..n.min(s.len())];
    }
    let mut end = 0;
    for (k, len) in crate::textio::char_lengths(s).enumerate() {
        if k == n {
            break;
        }
        end += len;
    }
    &s[..end]
}

fn pad(env: &Env, out: &mut Vec<u8>, spec: &Spec, body: &[u8]) {
    let len = char_len(env, body);
    let fill = spec.width.saturating_sub(len);
    if spec.left {
        out.extend_from_slice(body);
        out.extend(std::iter::repeat_n(b' ', fill));
    } else {
        out.extend(std::iter::repeat_n(b' ', fill));
        out.extend_from_slice(body);
    }
}

/// A number's digits padded per the flags: `sign` and `prefix` (`0x`) stay in front
/// of zero padding.
fn pad_num(out: &mut Vec<u8>, spec: &Spec, sign: &str, prefix: &str, digits: &str, zero_ok: bool) {
    let len = sign.len() + prefix.len() + digits.len();
    let fill = spec.width.saturating_sub(len);
    if spec.left {
        out.extend_from_slice(sign.as_bytes());
        out.extend_from_slice(prefix.as_bytes());
        out.extend_from_slice(digits.as_bytes());
        out.extend(std::iter::repeat_n(b' ', fill));
    } else if spec.zero && zero_ok {
        out.extend_from_slice(sign.as_bytes());
        out.extend_from_slice(prefix.as_bytes());
        out.extend(std::iter::repeat_n(b'0', fill));
        out.extend_from_slice(digits.as_bytes());
    } else {
        out.extend(std::iter::repeat_n(b' ', fill));
        out.extend_from_slice(sign.as_bytes());
        out.extend_from_slice(prefix.as_bytes());
        out.extend_from_slice(digits.as_bytes());
    }
}

fn sign_of(spec: &Spec, neg: bool) -> &'static str {
    if neg {
        "-"
    } else if spec.plus {
        "+"
    } else if spec.space {
        " "
    } else {
        ""
    }
}

pub fn format_one(env: &Env, out: &mut Vec<u8>, spec: &Spec, conv: u8, arg: &Value) {
    match conv {
        b's' => {
            let s = (env.to_str)(arg);
            let body = match spec.prec {
                Some(p) => prefix_chars(env, &s, p),
                None => &s,
            };
            pad(env, out, spec, body);
        }
        b'c' => {
            let mut buf = [0u8; 4];
            // Numbers (and input that looks like one) are character codes; a string
            // gives its first character, "" a NUL (as gawk).
            let code = match arg {
                Value::Str(_) => None,
                _ => arg.cmp_num(),
            };
            let body: Vec<u8> = match code {
                Some(n) => {
                    let code = n as i64;
                    if env.utf8 && code >= 128 {
                        match char::from_u32(code as u32) {
                            Some(c) => c.encode_utf8(&mut buf).as_bytes().to_vec(),
                            None => vec![code as u8],
                        }
                    } else {
                        vec![code as u8]
                    }
                }
                None => {
                    let s = (env.to_str)(arg);
                    if s.is_empty() {
                        vec![0]
                    } else {
                        prefix_chars(env, &s, 1).to_vec()
                    }
                }
            };
            pad(env, out, spec, &body);
        }
        b'd' | b'i' => {
            let n = arg.num();
            if !n.is_finite() {
                let s = special_str(n);
                let s = if n.is_sign_negative() || spec.plus {
                    s
                } else {
                    &s[1..]
                };
                pad_num(out, spec, "", "", s, false);
                return;
            }
            let t = n.trunc();
            let mut digits = int_str(t.abs());
            if let Some(p) = spec.prec {
                if p == 0 && t == 0.0 {
                    digits.clear();
                } else if digits.len() < p {
                    digits = format!("{}{digits}", "0".repeat(p - digits.len()));
                }
            }
            pad_num(
                out,
                spec,
                sign_of(spec, t < 0.0),
                "",
                &digits,
                spec.prec.is_none(),
            );
        }
        b'o' | b'x' | b'X' | b'u' => {
            let n = arg.num();
            if !n.is_finite() || n.abs() >= 1.8e19 {
                // Out of the unsigned range: as %g would (gawk switches formats too).
                format_float(out, spec, b'g', n);
                return;
            }
            let t = n.trunc();
            let v: u64 = if t < 0.0 { (t as i64) as u64 } else { t as u64 };
            let mut digits = match conv {
                b'o' => format!("{v:o}"),
                b'x' => format!("{v:x}"),
                b'X' => format!("{v:X}"),
                _ => format!("{v}"),
            };
            if let Some(p) = spec.prec {
                if p == 0 && v == 0 {
                    digits.clear();
                } else if digits.len() < p {
                    digits = format!("{}{digits}", "0".repeat(p - digits.len()));
                }
            }
            let prefix = match conv {
                b'o' if spec.alt && !digits.starts_with('0') => "0",
                b'x' if spec.alt && v != 0 => "0x",
                b'X' if spec.alt && v != 0 => "0X",
                _ => "",
            };
            pad_num(out, spec, "", prefix, &digits, spec.prec.is_none());
        }
        _ => format_float(out, spec, conv, arg.num()),
    }
}

/// `%e %f %g %a` (and upper case).
pub fn format_float(out: &mut Vec<u8>, spec: &Spec, conv: u8, n: f64) {
    let neg = n.is_sign_negative() && !(n.is_nan());
    let sign = sign_of(spec, neg);
    let a = n.abs();
    if !n.is_finite() {
        let s = special_str(n);
        // gawk: "-inf", "+inf" / "inf" (with + only when asked), "nan" likewise.
        let body = if n.is_nan() {
            let sign = if n.is_sign_negative() {
                "-"
            } else {
                sign_of(spec, false)
            };
            format!("{sign}nan")
        } else {
            format!("{sign}{}", &s[1..])
        };
        let body = if conv.is_ascii_uppercase() {
            body.to_ascii_uppercase()
        } else {
            body
        };
        pad_num(out, spec, "", "", &body, false);
        return;
    }
    let prec = spec.prec.unwrap_or(6);
    let mut digits = match conv {
        b'f' | b'F' => {
            let mut s = format!("{a:.prec$}");
            if spec.alt && prec == 0 {
                s.push('.');
            }
            s
        }
        b'e' | b'E' => fmt_e(a, prec, spec.alt),
        b'g' | b'G' => fmt_g(a, prec, spec.alt),
        _ => fmt_a(a, spec.prec),
    };
    if conv.is_ascii_uppercase() {
        digits = digits.to_ascii_uppercase();
    }
    pad_num(out, spec, sign, "", &digits, true);
}

/// C's `%e` of a non-negative finite number.
pub fn fmt_e(a: f64, prec: usize, alt: bool) -> String {
    let s = format!("{a:.prec$e}");
    let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let dot = if alt && prec == 0 { "." } else { "" };
    format!(
        "{mant}{dot}e{}{:02}",
        if exp < 0 { '-' } else { '+' },
        exp.abs()
    )
}

/// C's `%g` of a non-negative finite number.
pub fn fmt_g(a: f64, prec: usize, alt: bool) -> String {
    let p = prec.max(1);
    let exp = if a == 0.0 {
        0
    } else {
        let e = format!("{a:.*e}", p - 1);
        e.split_once('e')
            .and_then(|(_, x)| x.parse::<i32>().ok())
            .unwrap_or(0)
    };
    let out = if exp < -4 || exp >= p as i32 {
        fmt_e(a, p - 1, alt)
    } else {
        let decimals = (p as i32 - 1 - exp).max(0) as usize;
        let mut s = format!("{a:.decimals$}");
        if alt && !s.contains('.') {
            s.push('.');
        }
        s
    };
    if alt {
        return out;
    }
    let (num, suffix) = match out.find('e') {
        Some(i) => (&out[..i], &out[i..]),
        None => (out.as_str(), ""),
    };
    let num = if num.contains('.') {
        num.trim_end_matches('0').trim_end_matches('.')
    } else {
        num
    };
    format!("{num}{suffix}")
}

/// C's `%a`: hexadecimal floating point.
fn fmt_a(a: f64, prec: Option<usize>) -> String {
    if a == 0.0 {
        return match prec {
            Some(p) if p > 0 => format!("0x0.{}p+0", "0".repeat(p)),
            _ => "0x0p+0".into(),
        };
    }
    let bits = a.to_bits();
    let mut exp = ((bits >> 52) & 0x7ff) as i64;
    let mut mant = bits & ((1u64 << 52) - 1);
    let lead = if exp == 0 {
        // Subnormal: normalize.
        exp = -1022;
        while mant & (1u64 << 52) == 0 {
            mant <<= 1;
            exp -= 1;
        }
        mant &= (1u64 << 52) - 1;
        1
    } else {
        exp -= 1023;
        1
    };
    let mut hex = format!("{mant:013x}");
    match prec {
        Some(p) if p < 13 => {
            // Round to p hex digits.
            let shift = (13 - p) * 4;
            let full = (1u64 << 52) | mant;
            let rounded = (full + (1u64 << (shift - 1))) >> shift;
            let lead = rounded >> (p * 4);
            let frac = rounded & ((1u64 << (p * 4)) - 1);
            let frac = if p == 0 {
                String::new()
            } else {
                format!(".{frac:0w$x}", w = p)
            };
            return format!(
                "0x{lead}{frac}p{}{}",
                if exp < 0 { '-' } else { '+' },
                exp.abs()
            );
        }
        Some(p) => {
            while hex.len() < p {
                hex.push('0');
            }
        }
        None => {
            while hex.ends_with('0') {
                hex.pop();
            }
        }
    }
    let frac = if hex.is_empty() {
        String::new()
    } else {
        format!(".{hex}")
    };
    format!(
        "0x{lead}{frac}p{}{}",
        if exp < 0 { '-' } else { '+' },
        exp.abs()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    fn f(fmt: &str, args: &[Value]) -> String {
        let to_str = |v: &Value| -> S {
            match v {
                Value::Num(n) => Rc::from(format!("{n}").as_bytes()),
                Value::Str(s) | Value::StrNum(s) => s.clone(),
                Value::Uninit => Rc::from(&b""[..]),
            }
        };
        let env = Env {
            utf8: true,
            to_str: &to_str,
        };
        String::from_utf8(sprintf(&env, fmt.as_bytes(), args).unwrap()).unwrap()
    }

    #[test]
    fn conversions() {
        let n = Value::Num;
        assert_eq!(
            f("%d|%5d|%-5d|%05d", &[n(3.9), n(-4.0), n(5.0), n(-6.0)]),
            "3|   -4|5    |-0006"
        );
        assert_eq!(
            f("%*d|%.3d|%+d", &[n(4.0), n(7.0), n(8.0), n(9.0)]),
            "   7|008|+9"
        );
        assert_eq!(
            f(
                "%x %X %o %#x %u",
                &[n(255.0), n(255.0), n(8.0), n(255.0), n(-1.0)]
            ),
            "ff FF 10 0xff 18446744073709551615"
        );
        assert_eq!(
            f(
                "%.2f %e %g %G",
                &[n(1.23456), n(12345.678), n(0.0001), n(1e-5)]
            ),
            "1.23 1.234568e+04 0.0001 1E-05"
        );
        assert_eq!(
            f("%g %g %#g", &[n(100000.0), n(1000000.0), n(1.0)]),
            "100000 1e+06 1.00000"
        );
        assert_eq!(
            f("%c%c%c", &[n(65.0), Value::str(b"hello"), n(233.0)]),
            "Ahé"
        );
        assert_eq!(
            f(
                "%5s|%-5s|%.2s",
                &[
                    Value::str(b"ab"),
                    Value::str(b"ab"),
                    Value::str("héllo".as_bytes())
                ]
            ),
            "   ab|ab   |hé"
        );
        assert_eq!(f("100%% %z", &[]), "100% %z");
        assert_eq!(f("%d", &[n(2f64.powi(70))]), "1180591620717411303424");
        assert_eq!(f("%a", &[n(1.0)]), "0x1p+0");
    }
}
