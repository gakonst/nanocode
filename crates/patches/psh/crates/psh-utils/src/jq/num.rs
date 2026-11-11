//! jq 1.7's numbers: IEEE doubles that remember the literal they were read from. A
//! number that comes from the input or the program unchanged prints as its literal in
//! decNumber's canonical form (`1.000`, `1E+1000`, `100000000000000000000`) and compares
//! with full precision against another literal; arithmetic gives a plain double, printed
//! as jq prints doubles (the shortest digits that read back, `1e+17`, `1e-05`).
//! Written for psh after jq's manual and observed output.

use std::cmp::Ordering;
use std::rc::Rc;

/// A number.
#[derive(Clone, Debug)]
pub struct Num {
    pub f: f64,
    /// The canonical literal, for a number that is still what was read.
    lit: Option<Rc<str>>,
}

impl Num {
    pub fn float(f: f64) -> Num {
        Num { f, lit: None }
    }

    /// A JSON or jq number literal (`-1.50e3`, also jq's `.5`); `None` if it is not one.
    pub fn literal(text: &str) -> Option<Num> {
        let d = Dec::parse(text)?;
        let f = text.parse::<f64>().ok().or_else(|| {
            // `.5` and `5.` are jq literals but not Rust's.
            let fixed = if text.starts_with('.') {
                format!("0{text}")
            } else {
                text.replace(".e", ".0e").replace(".E", ".0E")
            };
            fixed.parse().ok()
        })?;
        Some(Num {
            f,
            lit: Some(d.canonical().into()),
        })
    }

    /// The literal, when the number is one.
    pub fn lit(&self) -> Option<&str> {
        self.lit.as_deref()
    }

    /// Whether this is an integer (as jq sees it: the double has no fraction).
    pub fn is_int(&self) -> bool {
        self.f.is_finite() && self.f.fract() == 0.0
    }

    /// jq's text of the number.
    pub fn write(&self, out: &mut Vec<u8>) {
        match &self.lit {
            Some(l) => out.extend_from_slice(l.as_bytes()),
            None => out.extend_from_slice(format_double(self.f).as_bytes()),
        }
    }

    pub fn text(&self) -> String {
        let mut v = Vec::new();
        self.write(&mut v);
        String::from_utf8(v).unwrap_or_default()
    }
}

impl PartialEq for Num {
    fn eq(&self, other: &Num) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Num {}

impl PartialOrd for Num {
    fn partial_cmp(&self, other: &Num) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Num {
    fn cmp(&self, other: &Num) -> Ordering {
        if let (Some(a), Some(b)) = (&self.lit, &other.lit)
            && let (Some(a), Some(b)) = (Dec::parse(a), Dec::parse(b))
        {
            return a.cmp_value(&b);
        }
        float_cmp(self.f, other.f)
    }
}

/// jq's order of doubles: NaN below every number, -0 equal to 0.
pub fn float_cmp(a: f64, b: f64) -> Ordering {
    if a.is_nan() {
        Ordering::Less
    } else if b.is_nan() {
        Ordering::Greater
    } else {
        a.partial_cmp(&b).unwrap_or(Ordering::Equal)
    }
}

/// A decimal literal: sign, coefficient digits and exponent.
struct Dec {
    neg: bool,
    digits: String,
    exp: i64,
}

impl Dec {
    fn parse(t: &str) -> Option<Dec> {
        let b = t.as_bytes();
        let mut i = 0;
        let neg = match b.first() {
            Some(b'-') => {
                i = 1;
                true
            }
            Some(b'+') => {
                i = 1;
                false
            }
            _ => false,
        };
        let mut digits = String::new();
        let mut frac = 0i64;
        let mut seen = false;
        while i < b.len() && b[i].is_ascii_digit() {
            digits.push(b[i] as char);
            i += 1;
            seen = true;
        }
        if i < b.len() && b[i] == b'.' {
            i += 1;
            while i < b.len() && b[i].is_ascii_digit() {
                digits.push(b[i] as char);
                frac += 1;
                i += 1;
                seen = true;
            }
        }
        if !seen {
            return None;
        }
        let mut exp = 0i64;
        if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
            i += 1;
            let eneg = match b.get(i) {
                Some(b'-') => {
                    i += 1;
                    true
                }
                Some(b'+') => {
                    i += 1;
                    false
                }
                _ => false,
            };
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                exp = exp
                    .saturating_mul(10)
                    .saturating_add(i64::from(b[i] - b'0'));
                i += 1;
            }
            if i == start {
                return None;
            }
            if eneg {
                exp = -exp;
            }
        }
        if i != b.len() {
            return None;
        }
        let trimmed = digits.trim_start_matches('0');
        let digits = if trimmed.is_empty() {
            "0".to_string()
        } else {
            trimmed.to_string()
        };
        Some(Dec {
            neg,
            digits,
            exp: exp.saturating_sub(frac),
        })
    }

    /// decNumber's to-scientific-string.
    fn canonical(&self) -> String {
        let mut s = String::new();
        if self.neg {
            s.push('-');
        }
        let c = &self.digits;
        let n = c.len() as i64;
        let adjusted = self.exp + n - 1;
        if self.exp <= 0 && adjusted >= -6 {
            if self.exp == 0 {
                s.push_str(c);
            } else {
                let pos = n + self.exp;
                if pos > 0 {
                    s.push_str(&c[..pos as usize]);
                    s.push('.');
                    s.push_str(&c[pos as usize..]);
                } else {
                    s.push_str("0.");
                    for _ in 0..-pos {
                        s.push('0');
                    }
                    s.push_str(c);
                }
            }
        } else {
            s.push_str(&c[..1]);
            if n > 1 {
                s.push('.');
                s.push_str(&c[1..]);
            }
            s.push('E');
            s.push(if adjusted >= 0 { '+' } else { '-' });
            s.push_str(&adjusted.unsigned_abs().to_string());
        }
        s
    }

    fn is_zero(&self) -> bool {
        self.digits == "0"
    }

    fn cmp_value(&self, o: &Dec) -> Ordering {
        match (self.is_zero(), o.is_zero()) {
            (true, true) => return Ordering::Equal,
            (true, false) => {
                return if o.neg {
                    Ordering::Greater
                } else {
                    Ordering::Less
                };
            }
            (false, true) => {
                return if self.neg {
                    Ordering::Less
                } else {
                    Ordering::Greater
                };
            }
            _ => {}
        }
        if self.neg != o.neg {
            return if self.neg {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        let mag = self.cmp_magnitude(o);
        if self.neg { mag.reverse() } else { mag }
    }

    fn cmp_magnitude(&self, o: &Dec) -> Ordering {
        let a = self.digits.trim_end_matches('0');
        let b = o.digits.trim_end_matches('0');
        let adj_a = self.exp + self.digits.len() as i64;
        let adj_b = o.exp + o.digits.len() as i64;
        adj_a.cmp(&adj_b).then_with(|| {
            let len = a.len().max(b.len());
            let pa = format!("{a:0<len$}");
            let pb = format!("{b:0<len$}");
            pa.cmp(&pb)
        })
    }
}

/// jq's text of a double: the shortest digits that read back; exponent form when the
/// decimal point is more than 15 places past the digits or 4 or more before them, with
/// a two-digit exponent at least. Infinities print as the largest double, NaN as null.
pub fn format_double(f: f64) -> String {
    if f.is_nan() {
        return "null".into();
    }
    let f = if f.is_infinite() {
        f64::MAX.copysign(f)
    } else {
        f
    };
    if f == 0.0 {
        return if f.is_sign_negative() { "-0" } else { "0" }.into();
    }
    let sci = format!("{:e}", f.abs());
    let (mant, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let exp: i64 = exp.parse().unwrap_or(0);
    let decpt = exp + 1;
    let n = digits.len() as i64;
    let mut s = String::new();
    if f < 0.0 {
        s.push('-');
    }
    if decpt <= -4 || decpt > n + 15 {
        s.push_str(&digits[..1]);
        if n > 1 {
            s.push('.');
            s.push_str(&digits[1..]);
        }
        s.push('e');
        s.push(if exp < 0 { '-' } else { '+' });
        s.push_str(&format!("{:02}", exp.unsigned_abs()));
    } else if decpt <= 0 {
        s.push_str("0.");
        for _ in 0..-decpt {
            s.push('0');
        }
        s.push_str(&digits);
    } else if decpt >= n {
        s.push_str(&digits);
        for _ in 0..decpt - n {
            s.push('0');
        }
    } else {
        s.push_str(&digits[..decpt as usize]);
        s.push('.');
        s.push_str(&digits[decpt as usize..]);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_print_as_decnumber_does() {
        let c = |t: &str| Num::literal(t).unwrap().text();
        assert_eq!(c("1.000"), "1.000");
        assert_eq!(c("1e1000"), "1E+1000");
        assert_eq!(c("1.0e2"), "1.0E+2");
        assert_eq!(c("1e-5"), "0.00001");
        assert_eq!(c("1e-7"), "1E-7");
        assert_eq!(c("100000000000000000000"), "100000000000000000000");
        assert_eq!(c("0.1"), "0.1");
        assert_eq!(c(".1e1"), "1");
        assert_eq!(c("-0"), "-0");
        assert_eq!(c("0.0"), "0.0");
        assert_eq!(c("1.7976931348623157e309"), "1.7976931348623157E+309");
    }

    #[test]
    fn doubles_print_as_jq_does() {
        assert_eq!(format_double(1e17), "1e+17");
        assert_eq!(format_double(1e16), "1e+16");
        assert_eq!(format_double(1e15), "1000000000000000");
        assert_eq!(format_double(1e-5), "1e-05");
        assert_eq!(format_double(0.0001), "0.0001");
        assert_eq!(format_double(1.5e-7), "1.5e-07");
        assert_eq!(format_double(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(format_double(1.0 / 3.0), "0.3333333333333333");
        assert_eq!(format_double(f64::INFINITY), "1.7976931348623157e+308");
        assert_eq!(format_double(5e-324), "5e-324");
        assert_eq!(format_double(1.5), "1.5");
        assert_eq!(format_double(123456789012.0), "123456789012");
    }

    #[test]
    fn literals_compare_exactly() {
        let n = |t: &str| Num::literal(t).unwrap();
        assert_ne!(n("100000000000000000000"), n("100000000000000000001"));
        assert_eq!(n("1.000"), n("1"));
        assert_eq!(n("1.000"), Num::float(1.0));
        assert!(n("1e1000") > n("1e999"));
        assert!(n("-2") < n("-1"));
    }
}
