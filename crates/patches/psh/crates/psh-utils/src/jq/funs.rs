//! jq's builtins that jq implements in C, as natives of psh's jq interpreter over psh's
//! values, with jq 1.7's behavior and messages; and the data the natives share (inputs,
//! environment, stderr). The shape of the natives follows jaq-std's `lib.rs` (crates.io
//! `jaq-std` 3.0.4, by Michael Färber, MIT license, see NOTICE).

use std::cell::RefCell;
use std::rc::Rc;

use super::compile::{Bind, Lut, NativeDef};
use super::eval::{Cv, Exn, Native, RunPtr, bome, core_natives, unary, v};
use super::iter::box_once;

use super::json;
use super::map::Map;
use super::time;
use super::val::{Error, Val, ValR, as_isize, err, split_str};
use crate::math;

/// What the natives see.
pub struct Data<'a> {
    pub lut: &'a Lut,
    pub env: &'a Env,
    pub inputs: &'a RefCell<dyn Inputs + 'a>,
}

/// The source of `input`, `inputs`, `input_filename` and `input_line_number`.
pub trait Inputs {
    /// The next input value; `Err` is a parse error's message.
    fn next_input(&mut self) -> Option<Result<Val, String>>;
    fn filename(&self) -> Val;
    fn line(&self) -> usize;
}

/// The environment of a run.
pub struct Env {
    pub env: Val,
    /// What `debug`, `stderr` and `input` errors print, collected until the next output.
    pub stderr: RefCell<Vec<u8>>,
    /// The local zone's offset (seconds east) and name, for `localtime`.
    pub utc_offset: i64,
    pub zone: String,
    pub now: f64,
    pub builtins: Val,
}

type Natives = Box<[(&'static str, Box<[Bind]>, RunPtr)]>;

fn data<'a>(cv: &Cv<'a>) -> &'a Data<'a> {
    cv.0.data()
}

fn s_err(v: &Val, what: &str) -> Error {
    err(format!("{} {what}", v.described()))
}

fn need_str<'v>(v: &'v Val, what: &str) -> Result<&'v str, Error> {
    v.as_str().ok_or_else(|| s_err(v, what))
}

fn length(v: &Val) -> ValR {
    match v {
        Val::Null => Ok(Val::num(0.0)),
        Val::Bool(_) => Err(s_err(v, "has no length")),
        Val::Num(n) => Ok(Val::num(n.f.abs())),
        Val::Str(s) => Ok(Val::from(s.chars().count())),
        Val::Arr(a) => Ok(Val::from(a.len())),
        Val::Obj(o) => Ok(Val::from(o.len())),
    }
}

fn keys(v: &Val, sorted: bool) -> ValR {
    match v {
        Val::Obj(o) => {
            let mut k: Vec<Rc<str>> = o.keys().cloned().collect();
            if sorted {
                k.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
            }
            Ok(k.into_iter().map(Val::Str).collect())
        }
        Val::Arr(a) => Ok((0..a.len()).map(Val::from).collect()),
        _ => Err(s_err(v, "has no keys")),
    }
}

/// jq's `contains`: an error for different kinds at the top, false below.
fn contains(a: &Val, b: &Val) -> Result<bool, Error> {
    if a.kind() != b.kind() {
        return Err(err(format!(
            "{} and {} cannot have their containment checked",
            a.described(),
            b.described()
        )));
    }
    Ok(contained(a, b))
}

fn contained(a: &Val, b: &Val) -> bool {
    match (a, b) {
        (Val::Obj(x), Val::Obj(y)) => y
            .iter()
            .all(|(k, yv)| x.get(k).is_some_and(|xv| contained(xv, yv))),
        (Val::Arr(x), Val::Arr(y)) => y.iter().all(|yv| x.iter().any(|xv| contained(xv, yv))),
        (Val::Str(x), Val::Str(y)) => x.contains(&**y),
        _ => a.kind() == b.kind() && a == b,
    }
}

fn arr(v: Val, what: &str) -> Result<Vec<Val>, Error> {
    match v {
        Val::Arr(a) => Ok(Rc::try_unwrap(a).unwrap_or_else(|a| (*a).clone())),
        v => Err(s_err(&v, what)),
    }
}

/// `_sort_by_impl`, `_group_by_impl`, ...: the input and its keys, paired.
fn keyed(v: Val, keys: Val, what: &str) -> Result<Vec<(Val, Val)>, Error> {
    let xs = arr(v, what)?;
    let ks = match keys {
        Val::Arr(k) => k,
        k => return Err(s_err(&k, what)),
    };
    Ok(ks.iter().cloned().zip(xs).collect())
}

fn tonumber(v: &Val) -> ValR {
    match v {
        Val::Num(_) => Ok(v.clone()),
        Val::Str(s) => {
            let t = s.trim();
            match super::num::Num::literal(t) {
                Some(n) => Ok(Val::Num(n)),
                None => {
                    let msg = match json::parse_all(t.as_bytes()) {
                        Err(json::ParseError(m)) => m,
                        Ok(_) => format!(
                            "Invalid numeric literal at EOF at line 1, column {}",
                            t.chars().count()
                        ),
                    };
                    Err(err(format!("{msg} (while parsing '{s}')")))
                }
            }
        }
        v => Err(s_err(v, "cannot be parsed as a number")),
    }
}

fn fromjson(v: &Val) -> ValR {
    let s = need_str(v, "only strings can be parsed")?;
    match json::parse_all(s.as_bytes()) {
        Ok(vals) if vals.len() == 1 => Ok(vals.into_iter().next().unwrap_or_default()),
        Ok(vals) if vals.is_empty() => {
            Err(err(format!("Expected JSON value (while parsing '{s}')")))
        }
        Ok(_) => Err(err(format!(
            "Unexpected extra JSON values (while parsing '{s}')"
        ))),
        Err(json::ParseError(m)) => Err(err(format!("{m} (while parsing '{s}')"))),
    }
}

fn implode(v: Val) -> ValR {
    let xs = match v {
        Val::Arr(a) => a,
        _ => return Err(err("implode input must be an array")),
    };
    let mut s = String::new();
    for x in xs.iter() {
        let n = match x {
            Val::Num(n) if !n.f.is_nan() => n.f,
            x => {
                let shown = if x.as_f64().is_some() {
                    "number (null)".to_string()
                } else {
                    x.described()
                };
                return Err(err(format!(
                    "{shown} can't be imploded, unicode codepoint needs to be numeric"
                )));
            }
        };
        let c = if n < 0.0 || n > f64::from(u32::MAX) {
            '\u{FFFD}'
        } else {
            char::from_u32(n as u32).unwrap_or('\u{FFFD}')
        };
        s.push(c);
    }
    Ok(Val::str(s))
}

/// `"abcab" | _strindices("ab")`: byte offsets, as jq gives them.
fn strindices(s: &str, t: &str) -> Val {
    if t.is_empty() {
        return Val::arr(Vec::new());
    }
    let b = s.as_bytes();
    let n = t.len();
    (0..b.len().saturating_sub(n - 1))
        .filter(|&i| &b[i..i + n] == t.as_bytes())
        .map(Val::from)
        .collect()
}

/// `delpaths`: all paths at once, as jq deletes them: the indices of an array are taken
/// before any element goes, negative ones counted from the end.
fn delpaths(v: Val, paths: Val) -> ValR {
    let Val::Arr(ps) = paths else {
        return Err(err("Paths must be specified as an array"));
    };
    let mut list = Vec::new();
    for p in ps.iter() {
        match p {
            Val::Arr(p) => list.push(p.to_vec()),
            _ => return Err(err("Path must be specified as an array")),
        }
    }
    delete(v, list)
}

fn delete(v: Val, paths: Vec<Vec<Val>>) -> ValR {
    if paths.iter().any(Vec::is_empty) {
        return Ok(Val::Null);
    }
    match v {
        Val::Null => Ok(Val::Null),
        Val::Obj(o) => {
            let mut m = Rc::try_unwrap(o).unwrap_or_else(|o| (*o).clone());
            let mut groups: Vec<(Rc<str>, Vec<Vec<Val>>)> = Vec::new();
            for p in paths {
                let Val::Str(k) = &p[0] else {
                    return Err(err(format!(
                        "Cannot delete field at {} index of object",
                        p[0].kind()
                    )));
                };
                let rest = p[1..].to_vec();
                match groups.iter_mut().find(|(g, _)| g == k) {
                    Some((_, rs)) => rs.push(rest),
                    None => groups.push((k.clone(), vec![rest])),
                }
            }
            for (k, rests) in groups {
                if rests.iter().any(Vec::is_empty) {
                    m.remove(&k);
                } else if let Some(child) = m.get(&k).cloned() {
                    let new = delete(child, rests)?;
                    m.insert(k, new);
                }
            }
            Ok(Val::obj(m))
        }
        Val::Arr(a) => {
            let mut xs = Rc::try_unwrap(a).unwrap_or_else(|a| (*a).clone());
            let len = xs.len();
            let mut gone = vec![false; len];
            let mut deeper: Vec<(usize, Vec<Vec<Val>>)> = Vec::new();
            for p in paths {
                match &p[0] {
                    Val::Num(n) => {
                        let i = n.f.floor();
                        let i = if i < 0.0 { i + len as f64 } else { i };
                        if i < 0.0 || i >= len as f64 || n.f.is_nan() {
                            continue;
                        }
                        let i = i as usize;
                        if p.len() == 1 {
                            gone[i] = true;
                        } else {
                            match deeper.iter_mut().find(|(j, _)| *j == i) {
                                Some((_, rs)) => rs.push(p[1..].to_vec()),
                                None => deeper.push((i, vec![p[1..].to_vec()])),
                            }
                        }
                    }
                    Val::Obj(r) if p.len() == 1 => {
                        let bound = |x: Option<&Val>, d: usize, up: bool| match x {
                            Some(Val::Num(n)) if !n.f.is_nan() => {
                                let f = if up { n.f.ceil() } else { n.f.floor() };
                                let f = if f < 0.0 { f + len as f64 } else { f };
                                f.clamp(0.0, len as f64) as usize
                            }
                            _ => d,
                        };
                        let s = bound(r.get("start"), 0, false);
                        let e = bound(r.get("end"), len, true);
                        for g in gone.iter_mut().take(e).skip(s) {
                            *g = true;
                        }
                    }
                    k => {
                        return Err(err(format!(
                            "Cannot delete field at {} index of array",
                            if matches!(k, Val::Obj(_)) {
                                "object"
                            } else {
                                k.kind()
                            }
                        )));
                    }
                }
            }
            for (i, rests) in deeper {
                if !gone[i] {
                    xs[i] = delete(std::mem::take(&mut xs[i]), rests)?;
                }
            }
            let mut out = Vec::with_capacity(len);
            for (x, g) in xs.into_iter().zip(gone) {
                if !g {
                    out.push(x);
                }
            }
            Ok(Val::arr(out))
        }
        v => Err(err(format!("Cannot delete fields from {}", v.kind()))),
    }
}

/// `@csv`, `@tsv`, `@sh`, `@html`, `@uri`, `@base64(d)`, `@base32(d)`, `@json`, `@text`.
fn format(v: Val, name: &str) -> ValR {
    let text = |v: &Val| match v {
        Val::Str(s) => s.to_string(),
        v => v.to_json(),
    };
    match name {
        "text" => Ok(Val::str(text(&v))),
        "json" => Ok(Val::str(v.to_json())),
        "html" => {
            let mut out = String::new();
            for c in text(&v).chars() {
                match c {
                    '<' => out.push_str("&lt;"),
                    '>' => out.push_str("&gt;"),
                    '&' => out.push_str("&amp;"),
                    '\'' => out.push_str("&apos;"),
                    '"' => out.push_str("&quot;"),
                    c => out.push(c),
                }
            }
            Ok(Val::str(out))
        }
        "uri" => {
            let mut out = String::new();
            for b in text(&v).bytes() {
                if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                    out.push(b as char);
                } else {
                    out.push_str(&format!("%{b:02X}"));
                }
            }
            Ok(Val::str(out))
        }
        "csv" | "tsv" => {
            let xs = match &v {
                Val::Arr(a) => a.clone(),
                _ => {
                    return Err(err(format!(
                        "{} cannot be {}-formatted, only an array can be",
                        v.described(),
                        name
                    )));
                }
            };
            let mut parts = Vec::new();
            for x in xs.iter() {
                parts.push(match x {
                    Val::Num(n) => n.text(),
                    Val::Bool(b) => b.to_string(),
                    Val::Null => String::new(),
                    Val::Str(s) if name == "csv" => format!("\"{}\"", s.replace('"', "\"\"")),
                    Val::Str(s) => s
                        .replace('\\', "\\\\")
                        .replace('\t', "\\t")
                        .replace('\n', "\\n")
                        .replace('\r', "\\r"),
                    x => {
                        return Err(err(format!(
                            "{} is not valid in a {name} row",
                            x.described()
                        )));
                    }
                });
            }
            Ok(Val::str(parts.join(if name == "csv" { "," } else { "\t" })))
        }
        "sh" => {
            let one = |x: &Val| -> Result<String, Error> {
                match x {
                    Val::Str(s) => Ok(format!("'{}'", s.replace('\'', "'\\''"))),
                    Val::Arr(_) | Val::Obj(_) => Err(err(format!(
                        "{} can not be escaped for shell",
                        x.described()
                    ))),
                    x => Ok(x.to_json()),
                }
            };
            match &v {
                Val::Arr(a) => {
                    let parts: Result<Vec<_>, _> = a.iter().map(one).collect();
                    Ok(Val::str(parts?.join(" ")))
                }
                x => Ok(Val::str(one(x)?)),
            }
        }
        "base64" => Ok(Val::str(base64_encode(text(&v).as_bytes()))),
        "base64d" => {
            let t = text(&v);
            let clean = t.trim_end_matches('=');
            if clean.len() % 4 == 1 && clean.bytes().all(|c| B64.contains(&c)) {
                return Err(err(format!("{} trailing base64 byte found", v.described())));
            }
            match base64_decode(t.as_bytes()) {
                Some(b) => Ok(Val::str(String::from_utf8_lossy(&b).into_owned())),
                None => Err(err(format!("{} is not valid base64 data", v.described()))),
            }
        }
        "base32" => Ok(Val::str(base32_encode(text(&v).as_bytes()))),
        "base32d" => {
            let t = text(&v);
            Ok(Val::str(
                String::from_utf8_lossy(&base32_decode(t.as_bytes())).into_owned(),
            ))
        }
        other => Err(err(format!("{other} is not a valid format"))),
    }
}

const B64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(b: &[u8]) -> String {
    let mut out = String::new();
    for chunk in b.chunks(3) {
        let n = chunk.len();
        let v = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= n {
                out.push(B64[(v >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// jq decodes what it can and ignores trailing garbage after padding.
fn base64_decode(b: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut acc = 0u32;
    let mut bits = 0;
    let mut seen = 0;
    for &c in b {
        if c == b'=' {
            break;
        }
        let v = B64.iter().position(|&x| x == c)? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        seen += 1;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    if seen % 4 == 1 {
        return None;
    }
    Some(out)
}

const B32: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

fn base32_encode(b: &[u8]) -> String {
    let mut out = String::new();
    for chunk in b.chunks(5) {
        let mut buf = [0u8; 5];
        buf[..chunk.len()].copy_from_slice(chunk);
        let v = buf.iter().fold(0u64, |a, &x| (a << 8) | u64::from(x));
        let chars = (chunk.len() * 8).div_ceil(5);
        for i in 0..8 {
            if i < chars {
                out.push(B32[(v >> (35 - 5 * i) & 31) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn base32_decode(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut acc = 0u64;
    let mut bits = 0;
    for &c in b {
        let Some(v) = B32.iter().position(|&x| x == c) else {
            continue;
        };
        acc = (acc << 5) | v as u64;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

/// `_match_impl(re; flags; test)`: jq's matches over the `regex` crate with Oniguruma's
/// Ruby defaults (`^`/`$` at line ends), offsets and lengths in code points.
fn match_impl(input: &Val, re: &Val, flags: &Val, test: bool) -> ValR {
    let s = match input {
        Val::Str(s) => s.clone(),
        v => {
            return Err(err(format!(
                "{} cannot be matched, as it is not a string",
                v.described()
            )));
        }
    };
    let re_s = match re {
        Val::Str(r) => r.clone(),
        v => {
            return Err(err(format!(
                "{} cannot be matched, as it is not a string",
                v.described()
            )));
        }
    };
    let flags_s = match flags {
        Val::Null => String::new(),
        Val::Str(f) => f.to_string(),
        v => return Err(err(format!("{} is not a string", v.described()))),
    };
    let mut global = false;
    let mut skip_empty = false;
    let mut b = regex::RegexBuilder::new(&re_s);
    b.multi_line(true);
    for c in flags_s.chars() {
        match c {
            'g' => global = true,
            'i' => {
                b.case_insensitive(true);
            }
            'x' => {
                b.ignore_whitespace(true);
            }
            'n' => skip_empty = true,
            's' => {
                b.multi_line(false);
            }
            'p' => {
                b.multi_line(false);
                b.ignore_whitespace(true);
            }
            'l' => {}
            _ => {
                return Err(err(format!("{flags_s} is not a valid modifier string")));
            }
        }
    }
    let regex = b
        .build()
        .map_err(|e| err(format!("{re_s} (at offset 0) is not a valid regex: {e}")))?;
    let names: Vec<Option<&str>> = regex.capture_names().collect();
    let cp = |byte: usize| s[..byte].chars().count();
    let mut out = Vec::new();
    // jq's loop: after an empty match one character on, else at the match's end, while
    // the start is within the text (so an empty match at the very end counts).
    let mut start = 0;
    let mut locs = regex.capture_locations();
    while start <= s.len() {
        let Some(m) = regex.captures_read_at(&mut locs, &s, start) else {
            break;
        };
        let empty = m.start() == m.end();
        let next = if empty {
            s[m.end()..]
                .chars()
                .next()
                .map_or(s.len() + 1, |c| m.end() + c.len_utf8())
        } else {
            m.end()
        };
        if !(skip_empty && empty) {
            if test {
                return Ok(Val::Bool(true));
            }
            let mut captures = Vec::new();
            for (i, name) in names.iter().enumerate().skip(1) {
                let mut c = Map::new();
                match locs.get(i) {
                    Some((gs, ge)) => {
                        c.insert("offset".into(), Val::from(cp(gs)));
                        c.insert("length".into(), Val::from(s[gs..ge].chars().count()));
                        c.insert("string".into(), Val::str(&s[gs..ge]));
                    }
                    None => {
                        c.insert("offset".into(), Val::num(-1.0));
                        c.insert("length".into(), Val::num(0.0));
                        c.insert("string".into(), Val::Null);
                    }
                }
                c.insert("name".into(), name.map_or(Val::Null, Val::str));
                captures.push(Val::obj(c));
            }
            let mut o = Map::new();
            o.insert("offset".into(), Val::from(cp(m.start())));
            o.insert("length".into(), Val::from(m.as_str().chars().count()));
            o.insert("string".into(), Val::str(m.as_str()));
            o.insert("captures".into(), Val::arr(captures));
            out.push(Val::obj(o));
            if !global {
                break;
            }
        }
        start = next;
    }
    if test {
        return Ok(Val::Bool(false));
    }
    Ok(Val::arr(out))
}

fn num_arg(v: &Val, _who: &str) -> Result<f64, Error> {
    v.as_f64().ok_or_else(|| s_err(v, "number required"))
}

macro_rules! math1 {
    ($name:literal, $f:expr) => {
        ($name, v(0), |cv| {
            let f: fn(f64) -> f64 = $f;
            bome(num_arg(&cv.1, "").map(|x| Val::num(f(x))))
        })
    };
}

macro_rules! math2 {
    ($name:literal, $f:expr) => {
        ($name, v(2), |mut cv| {
            let f: fn(f64, f64) -> f64 = $f;
            let b = cv.0.pop_var();
            let a = cv.0.pop_var();
            bome((|| Ok(Val::num(f(num_arg(&a, "")?, num_arg(&b, "")?))))())
        })
    };
}

fn natives() -> Natives {
    Box::new([
        ("length", v(0), |cv| bome(length(&cv.1))),
        ("utf8bytelength", v(0), |cv| {
            bome(need_str(&cv.1, "only strings have UTF-8 byte length").map(|s| Val::from(s.len())))
        }),
        ("keys", v(0), |cv| bome(keys(&cv.1, true))),
        ("keys_unsorted", v(0), |cv| bome(keys(&cv.1, false))),
        ("has", v(1), |cv| unary(cv, |x, k| x.has(&k))),
        ("contains", v(1), |cv| {
            unary(cv, |a, b| contains(&a, &b).map(Val::Bool))
        }),
        ("type", v(0), |cv| bome(Ok(Val::str(cv.1.kind())))),
        ("infinite", v(0), |_| bome(Ok(Val::num(f64::INFINITY)))),
        ("nan", v(0), |_| bome(Ok(Val::num(f64::NAN)))),
        ("isinfinite", v(0), |cv| {
            bome(Ok(Val::Bool(cv.1.as_f64().is_some_and(f64::is_infinite))))
        }),
        ("isnan", v(0), |cv| {
            bome(Ok(Val::Bool(cv.1.as_f64().is_some_and(f64::is_nan))))
        }),
        ("isnormal", v(0), |cv| {
            bome(Ok(Val::Bool(cv.1.as_f64().is_some_and(f64::is_normal))))
        }),
        ("sort", v(0), |cv| {
            bome(
                arr(cv.1, "cannot be sorted, as it is not an array").map(|mut a| {
                    a.sort();
                    Val::arr(a)
                }),
            )
        }),
        ("_sort_by_impl", v(1), |cv| {
            unary(cv, |x, k| {
                let mut kv = keyed(x, k, "cannot be sorted, as it is not an array")?;
                kv.sort_by(|a, b| a.0.cmp(&b.0));
                Ok(kv.into_iter().map(|(_, v)| v).collect())
            })
        }),
        ("_group_by_impl", v(1), |cv| {
            unary(cv, |x, k| {
                let mut kv = keyed(x, k, "cannot be grouped, as it is not an array")?;
                kv.sort_by(|a, b| a.0.cmp(&b.0));
                let mut groups: Vec<Val> = Vec::new();
                let mut cur: Vec<Val> = Vec::new();
                let mut last: Option<Val> = None;
                for (k, v) in kv {
                    if last.as_ref().is_some_and(|l| *l != k) {
                        groups.push(Val::arr(std::mem::take(&mut cur)));
                    }
                    last = Some(k);
                    cur.push(v);
                }
                if !cur.is_empty() {
                    groups.push(Val::arr(cur));
                }
                Ok(Val::arr(groups))
            })
        }),
        ("min", v(0), |cv| {
            bome(
                arr(cv.1, "cannot be iterated over")
                    .map(|a| a.into_iter().min_by(|a, b| a.cmp(b)).unwrap_or_default()),
            )
        }),
        ("max", v(0), |cv| {
            bome(arr(cv.1, "cannot be iterated over").map(|a| {
                let mut best: Option<Val> = None;
                for x in a {
                    if best.as_ref().is_none_or(|b| x >= *b) {
                        best = Some(x);
                    }
                }
                best.unwrap_or_default()
            }))
        }),
        ("_min_by_impl", v(1), |cv| {
            unary(cv, |x, k| {
                let kv = keyed(x, k, "cannot be iterated over")?;
                let mut best: Option<(Val, Val)> = None;
                for (k, v) in kv {
                    if best.as_ref().is_none_or(|b| k < b.0) {
                        best = Some((k, v));
                    }
                }
                Ok(best.map(|b| b.1).unwrap_or_default())
            })
        }),
        ("_max_by_impl", v(1), |cv| {
            unary(cv, |x, k| {
                let kv = keyed(x, k, "cannot be iterated over")?;
                let mut best: Option<(Val, Val)> = None;
                for (k, v) in kv {
                    if best.as_ref().is_none_or(|b| k >= b.0) {
                        best = Some((k, v));
                    }
                }
                Ok(best.map(|b| b.1).unwrap_or_default())
            })
        }),
        ("error", v(0), |cv| bome(Err(Error::new(cv.1)))),
        ("format", v(1), |cv| {
            unary(cv, |x, f| match &f {
                Val::Str(name) => format(x, name),
                f => Err(s_err(f, "is not a valid format")),
            })
        }),
        ("env", v(0), |cv| bome(Ok(data(&cv).env.env.clone()))),
        ("builtins", v(0), |cv| {
            bome(Ok(data(&cv).env.builtins.clone()))
        }),
        ("modulemeta", v(0), |cv| {
            bome(Err(err(format!(
                "modulemeta input module not found: {}",
                cv.1.to_json()
            ))))
        }),
        ("get_search_list", v(0), |_| bome(Ok(Val::arr(Vec::new())))),
        ("get_prog_origin", v(0), |_| bome(Ok(Val::Null))),
        ("get_jq_origin", v(0), |_| bome(Ok(Val::Null))),
        ("halt", v(0), |_| box_once(Err(Exn::halt(0)))),
        ("halt_error", v(1), |mut cv| {
            let code = cv.0.pop_var();
            let Some(code) = as_isize(&code) else {
                return bome(Err(err("halt_error/1: number required")));
            };
            let d = data(&cv);
            let mut e = d.env.stderr.borrow_mut();
            match &cv.1 {
                Val::Str(s) => e.extend_from_slice(s.as_bytes()),
                v => {
                    e.extend_from_slice(v.to_json().as_bytes());
                    e.push(b'\n');
                }
            }
            box_once(Err(Exn::halt(code as i32)))
        }),
        ("input", v(0), |cv| {
            let d = data(&cv);
            let next = d.inputs.borrow_mut().next_input();
            bome(match next {
                Some(Ok(v)) => Ok(v),
                Some(Err(m)) => Err(err(m)),
                None => Err(err("break")),
            })
        }),
        ("inputs", v(0), |cv| {
            let d = data(&cv);
            Box::new(std::iter::from_fn(move || {
                let next = d.inputs.borrow_mut().next_input();
                next.map(|r| r.map_err(|m| Exn::from(err(m))))
            }))
        }),
        ("input_filename", v(0), |cv| {
            bome(Ok(data(&cv).inputs.borrow().filename()))
        }),
        ("input_line_number", v(0), |cv| {
            bome(Ok(Val::from(data(&cv).inputs.borrow().line())))
        }),
        ("debug", v(0), |cv| {
            let d = data(&cv);
            let mut e = d.env.stderr.borrow_mut();
            e.extend_from_slice(b"[\"DEBUG:\",");
            e.extend_from_slice(cv.1.to_json().as_bytes());
            e.extend_from_slice(b"]\n");
            box_once(Ok(cv.1.clone()))
        }),
        ("stderr", v(0), |cv| {
            let d = data(&cv);
            let mut e = d.env.stderr.borrow_mut();
            match &cv.1 {
                Val::Str(s) => e.extend_from_slice(s.as_bytes()),
                v => e.extend_from_slice(v.to_json().as_bytes()),
            }
            box_once(Ok(cv.1.clone()))
        }),
        ("now", v(0), |cv| bome(Ok(Val::num(data(&cv).env.now)))),
        ("mktime", v(0), |cv| bome(time::mktime(&cv.1))),
        ("gmtime", v(0), |cv| {
            bome(match &cv.1 {
                Val::Num(n) => Ok(time::breakdown(n.f, 0).to_val()),
                _ => Err(err("gmtime() requires a number")),
            })
        }),
        ("localtime", v(0), |cv| {
            let off = data(&cv).env.utc_offset;
            bome(match &cv.1 {
                Val::Num(n) => Ok(time::breakdown(n.f, off).to_val()),
                _ => Err(err("localtime() requires a number")),
            })
        }),
        ("strftime", v(1), |cv| {
            unary(cv, |x, f| {
                let fmt = need_str(&f, "strftime/1 requires a string format")
                    .map_err(|_| err("strftime/1 requires a string format"))?;
                let tm = time::input_tm(&x, 0, "strftime/1")?;
                Ok(Val::str(time::strftime(&tm, fmt, "UTC", 0)))
            })
        }),
        ("strflocaltime", v(1), |mut cv| {
            let f = cv.0.pop_var();
            let env = data(&cv).env;
            let x = cv.1;
            bome((|| {
                let fmt = f
                    .as_str()
                    .ok_or_else(|| err("strflocaltime/1 requires a string format"))?;
                let tm = time::input_tm(&x, env.utc_offset, "strflocaltime/1")?;
                Ok(Val::str(time::strftime(
                    &tm,
                    fmt,
                    &env.zone,
                    env.utc_offset,
                )))
            })())
        }),
        ("strptime", v(1), |cv| {
            unary(cv, |x, f| {
                let s = x
                    .as_str()
                    .ok_or_else(|| err("strptime/1 requires string inputs and arguments"))?;
                let fmt = f
                    .as_str()
                    .ok_or_else(|| err("strptime/1 requires string inputs and arguments"))?;
                time::strptime(s, fmt)
            })
        }),
        ("tojson", v(0), |cv| bome(Ok(Val::str(cv.1.to_json())))),
        ("fromjson", v(0), |cv| bome(fromjson(&cv.1))),
        ("tonumber", v(0), |cv| bome(tonumber(&cv.1))),
        ("startswith", v(1), |cv| {
            unary(cv, |x, p| match (&x, &p) {
                (Val::Str(a), Val::Str(b)) => Ok(Val::Bool(a.starts_with(&**b))),
                _ => Err(err("startswith() requires string inputs")),
            })
        }),
        ("endswith", v(1), |cv| {
            unary(cv, |x, p| match (&x, &p) {
                (Val::Str(a), Val::Str(b)) => Ok(Val::Bool(a.ends_with(&**b))),
                _ => Err(err("endswith() requires string inputs")),
            })
        }),
        ("ltrimstr", v(1), |cv| {
            unary(cv, |x, p| match (&x, &p) {
                (Val::Str(a), Val::Str(b)) => Ok(match a.strip_prefix(&**b) {
                    Some(r) => Val::str(r),
                    None => x.clone(),
                }),
                _ => Ok(x),
            })
        }),
        ("rtrimstr", v(1), |cv| {
            unary(cv, |x, p| match (&x, &p) {
                (Val::Str(a), Val::Str(b)) => Ok(match a.strip_suffix(&**b) {
                    Some(r) => Val::str(r),
                    None => x.clone(),
                }),
                _ => Ok(x),
            })
        }),
        ("split", v(1), |cv| {
            unary(cv, |x, sep| match (&x, &sep) {
                (Val::Str(a), Val::Str(b)) => Ok(split_str(a, b)),
                _ => Err(err("split input and separator must be strings")),
            })
        }),
        ("explode", v(0), |cv| {
            bome(
                need_str(&cv.1, "cannot be exploded, must be a string")
                    .map(|s| s.chars().map(|c| Val::from(c as usize)).collect()),
            )
        }),
        ("implode", v(0), |cv| bome(implode(cv.1))),
        ("_strindices", v(1), |cv| {
            unary(cv, |x, t| match (&x, &t) {
                (Val::Str(a), Val::Str(b)) => Ok(strindices(a, b)),
                _ => Err(err("Cannot determine indices of non-strings")),
            })
        }),
        ("delpaths", v(1), |cv| unary(cv, delpaths)),
        (
            "_match_impl",
            [Bind::Var(()), Bind::Var(()), Bind::Var(())].into(),
            |mut cv| {
                let test = cv.0.pop_var();
                let flags = cv.0.pop_var();
                let re = cv.0.pop_var();
                bome(match_impl(&cv.1, &re, &flags, test.as_bool()))
            },
        ),
        ("frexp", v(0), |cv| {
            bome(num_arg(&cv.1, "").map(|x| {
                let (m, e) = math::frexp(x);
                [Val::num(m), Val::num(f64::from(e))].into_iter().collect()
            }))
        }),
        ("modf", v(0), |cv| {
            bome(num_arg(&cv.1, "").map(|x| {
                let (f, i) = math::modf(x);
                [Val::num(f), Val::num(i)].into_iter().collect()
            }))
        }),
        ("lgamma_r", v(0), |cv| {
            bome(num_arg(&cv.1, "").map(|x| {
                let (g, s) = math::lgamma_r(x);
                [Val::num(g), Val::num(f64::from(s))].into_iter().collect()
            }))
        }),
        ("significand", v(0), |cv| {
            bome(num_arg(&cv.1, "").map(|x| {
                if x == 0.0 || !x.is_finite() {
                    Val::num(x)
                } else {
                    Val::num(math::scalbn(x, -math::ilogb(x)))
                }
            }))
        }),
        ("logb", v(0), |cv| {
            bome(num_arg(&cv.1, "").map(|x| {
                Val::num(if x == 0.0 {
                    f64::NEG_INFINITY
                } else if x.is_infinite() {
                    f64::INFINITY
                } else if x.is_nan() {
                    x
                } else {
                    f64::from(math::ilogb(x))
                })
            }))
        }),
        math1!("floor", f64::floor),
        math1!("ceil", f64::ceil),
        math1!("round", math::round),
        math1!("rint", math::rint),
        math1!("nearbyint", math::rint),
        math1!("trunc", f64::trunc),
        math1!("fabs", f64::abs),
        math1!("sqrt", f64::sqrt),
        math1!("cbrt", math::cbrt),
        math1!("exp", math::exp),
        math1!("exp2", math::exp2),
        math1!("exp10", math::exp10),
        math1!("pow10", math::exp10),
        math1!("expm1", math::expm1),
        math1!("log", math::log),
        math1!("log2", math::log2),
        math1!("log10", math::log10),
        math1!("log1p", math::log1p),
        math1!("sin", math::sin),
        math1!("cos", math::cos),
        math1!("tan", math::tan),
        math1!("asin", math::asin),
        math1!("acos", math::acos),
        math1!("atan", math::atan),
        math1!("sinh", math::sinh),
        math1!("cosh", math::cosh),
        math1!("tanh", math::tanh),
        math1!("asinh", math::asinh),
        math1!("acosh", math::acosh),
        math1!("atanh", math::atanh),
        math1!("erf", math::erf),
        math1!("erfc", math::erfc),
        math1!("gamma", math::lgamma),
        math1!("lgamma", math::lgamma),
        math1!("tgamma", math::tgamma),
        math1!("j0", math::j0),
        math1!("j1", math::j1),
        math1!("y0", math::y0),
        math1!("y1", math::y1),
        math2!("pow", math::pow),
        math2!("atan2", math::atan2),
        math2!("fmod", math::fmod),
        math2!("hypot", math::hypot),
        math2!("fdim", math::fdim),
        math2!("fmax", math::fmax),
        math2!("fmin", math::fmin),
        math2!("copysign", f64::copysign),
        math2!("nextafter", math::nextafter),
        math2!("nexttoward", math::nextafter),
        math2!("remainder", math::remainder),
        math2!("drem", math::remainder),
        math2!("ldexp", |x, e| math::ldexp(x, e as i32)),
        math2!("scalb", |x, e| x * math::exp2(e)),
        math2!("scalbln", |x, e| math::scalbn(x, e as i32)),
        math2!("jn", |n, x| math::jn(n as i32, x)),
        math2!("yn", |n, x| math::yn(n as i32, x)),
        (
            "fma",
            [Bind::Var(()), Bind::Var(()), Bind::Var(())].into(),
            |mut cv| {
                let c = cv.0.pop_var();
                let b = cv.0.pop_var();
                let a = cv.0.pop_var();
                bome((|| {
                    Ok(Val::num(f64::mul_add(
                        num_arg(&a, "")?,
                        num_arg(&b, "")?,
                        num_arg(&c, "")?,
                    )))
                })())
            },
        ),
    ])
}

/// All natives: the interpreter's path-aware ones under the names the definitions use,
/// then psh's.
pub fn funs() -> Vec<NativeDef> {
    let mut all = core_natives();
    for (name, args, run) in natives() {
        all.push((name, args, Native { run, paths: None }));
    }
    all
}

/// The names of the natives and definitions, as `builtins` lists them.
pub fn builtin_names(defs: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for (name, args, _) in natives().iter() {
        if !name.starts_with('_') {
            names.push(format!("{name}/{}", args.len()));
        }
    }
    names.extend(["path/1".to_string(), "first/1".to_string()]);
    for line in defs.lines() {
        let Some(rest) = line.strip_prefix("def ") else {
            continue;
        };
        let head: String = rest.chars().take_while(|c| *c != ':').collect();
        let (name, args) = match head.split_once('(') {
            Some((n, a)) => (n.trim().to_string(), a.split(';').count()),
            None => (head.trim().to_string(), 0),
        };
        if name.starts_with('_')
            || name.starts_with('@')
            || matches!(name.as_str(), "null" | "true" | "false")
        {
            continue;
        }
        let entry = format!("{name}/{args}");
        if !names.contains(&entry) {
            names.push(entry);
        }
    }
    names
}
