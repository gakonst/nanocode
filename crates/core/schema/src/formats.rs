use std::{
    collections::HashMap,
    error::Error,
    net::{Ipv4Addr, Ipv6Addr},
};

use serde_json::Value;
use std::sync::LazyLock;

use crate::ecma;

#[derive(Clone, Copy)]
pub struct Format {
    pub name: &'static str,

    pub func: fn(v: &Value) -> Result<(), Box<dyn Error>>,
}

pub(crate) static FORMATS: LazyLock<HashMap<&'static str, Format>> = LazyLock::new(|| {
    let mut m = HashMap::<&'static str, Format>::new();
    let mut register = |name, func| m.insert(name, Format { name, func });
    register("regex", validate_regex);
    register("ipv4", validate_ipv4);
    register("ipv6", validate_ipv6);
    register("hostname", validate_hostname);
    register("idn-hostname", validate_idn_hostname);
    register("email", validate_email);
    register("idn-email", validate_idn_email);
    register("date", validate_date);
    register("time", validate_time);
    register("date-time", validate_date_time);
    register("duration", validate_duration);
    register("json-pointer", validate_json_pointer);
    register("relative-json-pointer", validate_relative_json_pointer);
    register("uuid", validate_uuid);
    register("uri", validate_uri);
    register("iri", validate_iri);
    register("uri-reference", validate_uri_reference);
    register("iri-reference", validate_iri_reference);
    register("uri-template", validate_uri_template);
    m
});

fn validate_regex(v: &Value) -> Result<(), Box<dyn Error>> {
    let Value::String(s) = v else {
        return Ok(());
    };
    ecma::convert(s).map(|_| ())
}

fn validate_ipv4(v: &Value) -> Result<(), Box<dyn Error>> {
    let Value::String(s) = v else {
        return Ok(());
    };
    s.parse::<Ipv4Addr>()?;
    Ok(())
}

fn validate_ipv6(v: &Value) -> Result<(), Box<dyn Error>> {
    let Value::String(s) = v else {
        return Ok(());
    };
    s.parse::<Ipv6Addr>()?;
    Ok(())
}

fn validate_date(v: &Value) -> Result<(), Box<dyn Error>> {
    let Value::String(s) = v else {
        return Ok(());
    };
    check_date(s)
}

fn matches_char(s: &str, index: usize, ch: char) -> bool {
    s.is_char_boundary(index) && s[index..].starts_with(ch)
}

fn check_date(s: &str) -> Result<(), Box<dyn Error>> {
    if s.len() != 10 {
        Err("must be 10 characters long")?;
    }
    if !matches_char(s, 4, '-') || !matches_char(s, 7, '-') {
        Err("missing hyphen in correct place")?;
    }

    let mut ymd = s.splitn(3, '-').filter_map(|t| t.parse::<usize>().ok());
    let (Some(y), Some(m), Some(d)) = (ymd.next(), ymd.next(), ymd.next()) else {
        Err("non-positive year/month/day")?
    };

    if !matches!(m, 1..=12) {
        Err(format!("{m} months in year"))?;
    }
    if !matches!(d, 1..=31) {
        Err(format!("{d} days in month"))?;
    }

    match m {
        2 => {
            let mut feb_days = 28;
            if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
                feb_days += 1;
            };
            if d > feb_days {
                Err(format!("february has {feb_days} days only"))?;
            }
        }
        4 | 6 | 9 | 11 if d > 30 => {
            Err("month has 30 days only")?;
        }
        _ => {}
    }
    Ok(())
}

fn validate_time(v: &Value) -> Result<(), Box<dyn Error>> {
    let Value::String(s) = v else {
        return Ok(());
    };
    check_time(s)
}

fn check_time(mut str: &str) -> Result<(), Box<dyn Error>> {
    if str.len() < 9 {
        Err("less than 9 characters long")?
    }
    if !matches_char(str, 2, ':') || !matches_char(str, 5, ':') {
        Err("missing colon in correct place")?
    }

    if !str.is_char_boundary(8) {
        Err("contains non-ascii char")?
    }
    let mut hms = (str[..8])
        .splitn(3, ':')
        .filter_map(|t| t.parse::<usize>().ok());
    let (Some(mut h), Some(mut m), Some(s)) = (hms.next(), hms.next(), hms.next()) else {
        Err("non-positive hour/min/sec")?
    };
    if h > 23 || m > 59 || s > 60 {
        Err("hour/min/sec out of range")?
    }
    str = &str[8..];

    if let Some(rem) = str.strip_prefix('.') {
        let n_digits = rem.chars().take_while(char::is_ascii_digit).count();
        if n_digits == 0 {
            Err("no digits in second fraction")?;
        }
        str = &rem[n_digits..];
    }

    if str != "z" && str != "Z" {
        if str.len() != 6 {
            Err("offset must be 6 characters long")?;
        }
        let sign: isize = match str.chars().next() {
            Some('+') => -1,
            Some('-') => 1,
            _ => return Err("offset must begin with plus/minus")?,
        };
        str = &str[1..];
        if !matches_char(str, 2, ':') {
            Err("missing colon in offset at correct place")?
        }

        let mut zhm = str.splitn(2, ':').filter_map(|t| t.parse::<usize>().ok());
        let (Some(zh), Some(zm)) = (zhm.next(), zhm.next()) else {
            Err("non-positive hour/min in offset")?
        };
        if zh > 23 || zm > 59 {
            Err("hour/min in offset out of range")?
        }

        let mut hm = (h * 60 + m) as isize + sign * (zh * 60 + zm) as isize;
        if hm < 0 {
            hm += 24 * 60;
            debug_assert!(hm >= 0);
        }
        let hm = hm as usize;
        (h, m) = (hm / 60, hm % 60);
    }

    if !(s < 60 || (h == 23 && m == 59)) {
        Err("invalid leap second")?
    }
    Ok(())
}

fn validate_date_time(v: &Value) -> Result<(), Box<dyn Error>> {
    let Value::String(s) = v else {
        return Ok(());
    };
    check_date_time(s)
}

fn check_date_time(s: &str) -> Result<(), Box<dyn Error>> {
    if s.len() < 20 {
        Err("less than 20 characters long")?;
    }
    if !s.is_char_boundary(10) || !s[10..].starts_with(['t', 'T']) {
        Err("11th character must be t or T")?;
    }
    if let Err(e) = check_date(&s[..10]) {
        Err(format!("invalid date element: {e}"))?;
    }
    if let Err(e) = check_time(&s[11..]) {
        Err(format!("invalid time element: {e}"))?;
    }
    Ok(())
}

fn validate_duration(v: &Value) -> Result<(), Box<dyn Error>> {
    let Value::String(s) = v else {
        return Ok(());
    };
    check_duration(s)
}

fn check_duration(s: &str) -> Result<(), Box<dyn Error>> {
    if crate::format_compat::is_valid_duration(s) {
        Ok(())
    } else {
        Err("invalid duration".into())
    }
}

fn validate_hostname(v: &Value) -> Result<(), Box<dyn Error>> {
    let Value::String(s) = v else {
        return Ok(());
    };
    check_hostname(s)
}

fn check_hostname(s: &str) -> Result<(), Box<dyn Error>> {
    if crate::format_compat::is_valid_hostname(s) {
        Ok(())
    } else {
        Err("invalid hostname".into())
    }
}

fn validate_idn_hostname(v: &Value) -> Result<(), Box<dyn Error>> {
    let Value::String(s) = v else {
        return Ok(());
    };
    if crate::format_compat::is_valid_idn_hostname(s) {
        Ok(())
    } else {
        Err("invalid IDN hostname".into())
    }
}

fn validate_email(v: &Value) -> Result<(), Box<dyn Error>> {
    let Value::String(s) = v else {
        return Ok(());
    };
    if crate::format_compat::is_valid_email(s) {
        Ok(())
    } else {
        Err("invalid email".into())
    }
}

fn validate_idn_email(v: &Value) -> Result<(), Box<dyn Error>> {
    let Value::String(s) = v else {
        return Ok(());
    };
    if crate::format_compat::is_valid_idn_email(s) {
        Ok(())
    } else {
        Err("invalid IDN email".into())
    }
}

fn validate_json_pointer(v: &Value) -> Result<(), Box<dyn Error>> {
    let Value::String(s) = v else {
        return Ok(());
    };
    check_json_pointer(s)
}

fn check_json_pointer(s: &str) -> Result<(), Box<dyn Error>> {
    if s.is_empty() {
        return Ok(());
    }
    if !s.starts_with('/') {
        Err("not starting with slash")?;
    }
    for token in s.split('/').skip(1) {
        let mut chars = token.chars();
        while let Some(ch) = chars.next() {
            if ch == '~' {
                if !matches!(chars.next(), Some('0' | '1')) {
                    Err("~ must be followed by 0 or 1")?;
                }
            } else if !matches!(ch, '\x00'..='\x2E' | '\x30'..='\x7D' | '\x7F'..='\u{10FFFF}') {
                Err("contains disallowed character")?;
            }
        }
    }
    Ok(())
}

fn validate_relative_json_pointer(v: &Value) -> Result<(), Box<dyn Error>> {
    let Value::String(s) = v else {
        return Ok(());
    };

    let num_digits = s.chars().take_while(char::is_ascii_digit).count();
    if num_digits == 0 {
        Err("must start with non-negative integer")?;
    }
    if num_digits > 1 && s.starts_with('0') {
        Err("starts with zero")?;
    }
    let s = &s[num_digits..];

    if s == "#" {
        return Ok(());
    }
    if let Err(e) = check_json_pointer(s) {
        Err(format!("invalid json-pointer element: {e}"))?;
    }
    Ok(())
}

fn validate_uuid(v: &Value) -> Result<(), Box<dyn Error>> {
    let Value::String(s) = v else {
        return Ok(());
    };

    static HEX_GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let mut i = 0;
    for group in s.split('-') {
        if i >= HEX_GROUPS.len() {
            Err("more than 5 elements")?;
        }
        if group.len() != HEX_GROUPS[i] {
            Err(format!(
                "element {} must be {} characters long",
                i + 1,
                HEX_GROUPS[i]
            ))?;
        }
        if let Some(ch) = group.chars().find(|c| !c.is_ascii_hexdigit()) {
            Err(format!("non-hex character {ch:?}"))?;
        }
        i += 1;
    }
    if i != HEX_GROUPS.len() {
        Err("must have 5 elements")?;
    }
    Ok(())
}

fn validate_uri(v: &Value) -> Result<(), Box<dyn Error>> {
    let Value::String(s) = v else {
        return Ok(());
    };
    if !crate::uri::parse(s.as_str(), crate::uri::Syntax::UriReference)?.has_scheme {
        Err("relative url")?;
    };
    Ok(())
}

fn validate_iri(v: &Value) -> Result<(), Box<dyn Error>> {
    let Value::String(s) = v else {
        return Ok(());
    };
    crate::uri::parse(s.as_str(), crate::uri::Syntax::Iri)?;
    Ok(())
}

fn validate_uri_reference(v: &Value) -> Result<(), Box<dyn Error>> {
    let Value::String(s) = v else {
        return Ok(());
    };
    crate::uri::parse(s.as_str(), crate::uri::Syntax::UriReference)?;
    Ok(())
}

fn validate_iri_reference(v: &Value) -> Result<(), Box<dyn Error>> {
    let Value::String(s) = v else {
        return Ok(());
    };
    crate::uri::parse(s.as_str(), crate::uri::Syntax::IriReference)?;
    Ok(())
}

fn validate_uri_template(v: &Value) -> Result<(), Box<dyn Error>> {
    let Value::String(s) = v else {
        return Ok(());
    };
    if crate::format_compat::is_valid_uri_template(s) {
        Ok(())
    } else {
        Err("invalid URI template".into())
    }
}

pub(crate) fn validate_legacy_hostname(v: &Value) -> Result<(), Box<dyn Error>> {
    let Value::String(s) = v else {
        return Ok(());
    };
    if crate::format_compat::is_valid_hostname_rfc1034(s) {
        Ok(())
    } else {
        Err("invalid hostname".into())
    }
}
