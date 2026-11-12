//! jq's values for psh's jq interpreter, with jq 1.7's semantics and error texts
//! (indexing, iteration, slices, updates, arithmetic, order).
//!
//! The structure follows jaq-json's `Val` (crates.io `jaq-json` 2.0.3, `src/lib.rs`),
//! by Michael Färber, MIT license (see NOTICE): the trait methods
//! and their update logic are adapted from there; the value model (doubles with
//! literals, string keys, UTF-8 strings) and the messages are jq's.

use std::cmp::Ordering;
use std::fmt;
use std::rc::Rc;

use super::eval::{Exn, ValX};
use super::iter::box_once;
use super::map::Map;
use super::num::Num;
use super::path::Opt;

/// A jq value.
#[derive(Clone, Debug, Default)]
pub enum Val {
    #[default]
    Null,
    Bool(bool),
    Num(Num),
    Str(Rc<str>),
    Arr(Rc<Vec<Val>>),
    Obj(Rc<Map>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    Value(Val),
    PathExpr(Val),
}

impl Error {
    pub fn new(v: Val) -> Error {
        Error::Value(v)
    }

    pub fn path_expr(v: Val) -> Error {
        Error::PathExpr(v)
    }

    pub fn into_val(self) -> Val {
        match self {
            Error::Value(v) => v,
            Error::PathExpr(v) => Val::from(format!("invalid path expression with input {v}")),
        }
    }
}

pub type ValR = Result<Val, Error>;

/// An error with jq's message (a string value, so `catch` sees the text).
pub fn err(msg: impl Into<String>) -> Error {
    Error::new(Val::from(msg.into()))
}

fn rc_take<T: Clone>(a: Rc<T>) -> T {
    Rc::try_unwrap(a).unwrap_or_else(|a| (*a).clone())
}

impl Val {
    pub fn str(s: impl Into<Rc<str>>) -> Val {
        Val::Str(s.into())
    }

    pub fn num(f: f64) -> Val {
        Val::Num(Num::float(f))
    }

    pub fn obj(m: Map) -> Val {
        Val::Obj(Rc::new(m))
    }

    pub fn arr(v: Vec<Val>) -> Val {
        Val::Arr(Rc::new(v))
    }

    /// jq's type name.
    pub fn kind(&self) -> &'static str {
        match self {
            Val::Null => "null",
            Val::Bool(_) => "boolean",
            Val::Num(_) => "number",
            Val::Str(_) => "string",
            Val::Arr(_) => "array",
            Val::Obj(_) => "object",
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Val::Num(n) => Some(n.f),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Val::Str(s) => Some(s),
            _ => None,
        }
    }

    /// Compact JSON, as `tojson` and messages show values.
    pub fn to_json(&self) -> String {
        let mut out = Vec::new();
        super::json::write_compact(&mut out, self);
        String::from_utf8(out).unwrap_or_default()
    }

    /// The value as jq abbreviates it in messages: at most 11 bytes of its JSON, then
    /// `...`.
    pub fn trunc(&self) -> String {
        let s = self.to_json();
        if s.len() <= 14 {
            return s;
        }
        let mut end = 11;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }

    /// `type (value)`, as jq's messages say it.
    pub fn described(&self) -> String {
        format!("{} ({})", self.kind(), self.trunc())
    }

    fn index_error(&self, key: &Val) -> Error {
        match key {
            Val::Str(k) => err(format!("Cannot index {} with string \"{k}\"", self.kind())),
            k => err(format!("Cannot index {} with {}", self.kind(), k.kind())),
        }
    }

    fn iterate_error(&self) -> Error {
        err(format!("Cannot iterate over {}", self.described()))
    }

    /// An array index: jq floors fractions; negative indices count from the end.
    fn array_index(i: f64, len: usize) -> Option<usize> {
        let i = i.floor();
        let i = if i < 0.0 { i + len as f64 } else { i };
        (i >= 0.0 && i < len as f64).then_some(i as usize)
    }

    /// `.[k]` without a value: `None` (null) where the key is absent.
    pub fn get(&self, key: &Val) -> ValR {
        match (self, key) {
            (Val::Null, Val::Str(_) | Val::Num(_) | Val::Null | Val::Obj(_)) => Ok(Val::Null),
            (Val::Obj(o), Val::Str(k)) => Ok(o.get(k).cloned().unwrap_or_default()),
            (Val::Arr(a), Val::Num(i)) => Ok(Val::array_index(i.f, a.len())
                .map(|i| a[i].clone())
                .unwrap_or_default()),
            (Val::Arr(a), Val::Arr(b)) => Ok(indices_of(a, b)),
            (Val::Arr(_) | Val::Str(_), Val::Obj(o)) => {
                let start = o.get("start").cloned().unwrap_or_default();
                let end = o.get("end").cloned().unwrap_or_default();
                self.clone().slice(&start, &end)
            }
            _ => Err(self.index_error(key)),
        }
    }

    /// `.[from:to]`.
    pub fn slice(self, from: &Val, to: &Val) -> ValR {
        let bound = |v: &Val, len: usize, def: usize, up: bool| -> Result<usize, Error> {
            match v {
                Val::Null => Ok(def),
                Val::Num(n) if n.f.is_nan() => Ok(def),
                Val::Num(n) => {
                    let f = if up { n.f.ceil() } else { n.f.floor() };
                    let f = if f < 0.0 { f + len as f64 } else { f };
                    Ok(f.clamp(0.0, len as f64) as usize)
                }
                _ => Err(err(
                    "Start and end indices of an array slice must be numbers",
                )),
            }
        };
        let check = |v: &Val| matches!(v, Val::Null | Val::Num(_));
        if !check(from) || !check(to) {
            return match self {
                Val::Null => Ok(Val::Null),
                _ => Err(err("Array/string slice indices must be integers")),
            };
        }
        match self {
            Val::Null => Ok(Val::Null),
            Val::Arr(a) => {
                let s = bound(from, a.len(), 0, false)?;
                let e = bound(to, a.len(), a.len(), true)?.max(s);
                Ok(Val::arr(a[s..e].to_vec()))
            }
            Val::Str(st) => {
                let n = st.chars().count();
                let s = bound(from, n, 0, false)?;
                let e = bound(to, n, n, true)?.max(s);
                Ok(Val::str(st.chars().skip(s).take(e - s).collect::<String>()))
            }
            v => Err(err(format!("Cannot index {} with object", v.kind()))),
        }
    }

    /// `has(k)`.
    pub fn has(&self, key: &Val) -> ValR {
        match (self, key) {
            (Val::Obj(o), Val::Str(k)) => Ok(Val::Bool(o.contains_key(k))),
            (Val::Arr(a), Val::Num(i)) => Ok(Val::Bool(i.f >= 0.0 && i.f < a.len() as f64)),
            _ => Err(err(format!(
                "Cannot check whether {} has a {} key",
                self.kind(),
                key.kind()
            ))),
        }
    }

    fn key_rc(key: &Val) -> Option<Rc<str>> {
        match key {
            Val::Str(s) => Some(s.clone()),
            _ => None,
        }
    }
}

/// `[1,2,1] | .[[1]]`: where the array `b` occurs in `a`.
fn indices_of(a: &[Val], b: &[Val]) -> Val {
    if b.is_empty() {
        return Val::arr(Vec::new());
    }
    let found = a
        .windows(b.len())
        .enumerate()
        .filter(|(_, w)| *w == b)
        .map(|(i, _)| Val::from(i as isize));
    found.collect()
}

impl Val {
    pub fn from_num(n: &str) -> ValR {
        Num::literal(n)
            .map(Val::Num)
            .ok_or_else(|| err(format!("Invalid numeric literal {n}")))
    }

    pub fn from_map<I: IntoIterator<Item = (Self, Self)>>(iter: I) -> ValR {
        let mut m = Map::new();
        for (k, v) in iter {
            match k {
                Val::Str(k) => m.insert(k, v),
                _ => return Err(err("Object keys must be strings")),
            }
        }
        Ok(Val::obj(m))
    }

    pub fn key_values(self) -> Box<dyn Iterator<Item = Result<(Val, Val), Error>>> {
        match self {
            Val::Arr(a) => Box::new(
                rc_take(a)
                    .into_iter()
                    .enumerate()
                    .map(|(i, x)| Ok((Val::from(i as isize), x))),
            ),
            Val::Obj(o) => Box::new(
                rc_take(o)
                    .into_entries()
                    .into_iter()
                    .map(|(k, v)| Ok((Val::Str(k), v))),
            ),
            v => box_once(Err(v.iterate_error())),
        }
    }

    pub fn values(self) -> Box<dyn Iterator<Item = ValR>> {
        match self {
            Val::Arr(a) => Box::new(rc_take(a).into_iter().map(Ok)),
            Val::Obj(o) => Box::new(rc_take(o).into_entries().into_iter().map(|(_, v)| Ok(v))),
            v => box_once(Err(v.iterate_error())),
        }
    }

    pub fn index(self, index: &Self) -> ValR {
        self.get(index)
    }

    pub fn range(self, from: Option<&Val>, upto: Option<&Val>) -> ValR {
        let from = from.cloned().unwrap_or_default();
        let to = upto.cloned().unwrap_or_default();
        self.slice(&from, &to)
    }

    pub fn map_values<I: Iterator<Item = ValX>>(self, opt: Opt, f: impl Fn(Self) -> I) -> ValX {
        match self {
            Val::Arr(a) => {
                let mut out = Vec::new();
                for x in rc_take(a) {
                    if let Some(y) = f(x).next() {
                        out.push(y?);
                    }
                }
                Ok(Val::arr(out))
            }
            Val::Obj(o) => {
                let mut out = Map::new();
                for (k, v) in rc_take(o).into_entries() {
                    if let Some(y) = f(v).next() {
                        out.insert(k, y?);
                    }
                }
                Ok(Val::obj(out))
            }
            v => opt.fail(v, |v| Exn::from(v.iterate_error())),
        }
    }

    pub fn map_index<I: Iterator<Item = ValX>>(
        self,
        index: &Self,
        opt: Opt,
        f: impl Fn(Self) -> I,
    ) -> ValX {
        if let (Val::Arr(_) | Val::Str(_) | Val::Null, Val::Obj(o)) = (&self, index) {
            let start = o.get("start");
            let end = o.get("end");
            return self.map_range(start, end, opt, f);
        }
        match (self, index) {
            (Val::Null, Val::Str(_)) => Val::obj(Map::new()).map_index(index, opt, f),
            (Val::Null, Val::Num(_)) => Val::arr(Vec::new()).map_index(index, opt, f),
            (Val::Obj(mut o), Val::Str(k)) => {
                let m = Rc::make_mut(&mut o);
                let old = m.get(k).cloned().unwrap_or_default();
                match f(old).next().transpose()? {
                    Some(y) => m.insert(k.clone(), y),
                    None => {
                        m.remove(k);
                    }
                }
                Ok(Val::Obj(o))
            }
            (Val::Arr(a), Val::Num(i)) if i.f.is_nan() => opt.fail(Val::Arr(a), |_| {
                Exn::from(err("Cannot set array element at NaN index"))
            }),
            (Val::Arr(mut a), Val::Num(i)) => {
                let len = a.len();
                let fi = i.f.floor();
                let pos = if fi < 0.0 { fi + len as f64 } else { fi };
                if pos < 0.0 {
                    return opt.fail(Val::Arr(a), |_| {
                        Exn::from(err("Out of bounds negative array index"))
                    });
                }
                let pos = pos as usize;
                let v = Rc::make_mut(&mut a);
                let old = v.get(pos).cloned().unwrap_or_default();
                match f(old).next().transpose()? {
                    Some(y) => {
                        if pos >= v.len() {
                            v.resize(pos + 1, Val::Null);
                        }
                        v[pos] = y;
                    }
                    None => {
                        if pos < v.len() {
                            v.remove(pos);
                        }
                    }
                }
                Ok(Val::Arr(a))
            }
            (v, i) => {
                let e = match (&v, i) {
                    (Val::Arr(_) | Val::Obj(_) | Val::Null, _) => v.index_error(i),
                    _ => v.index_error(i),
                };
                opt.fail(v, |_| Exn::from(e))
            }
        }
    }

    pub fn map_range<I: Iterator<Item = ValX>>(
        self,
        from: Option<&Val>,
        upto: Option<&Val>,
        opt: Opt,
        f: impl Fn(Self) -> I,
    ) -> ValX {
        let from = from.cloned().unwrap_or_default();
        let to = upto.cloned().unwrap_or_default();
        let a = match self {
            Val::Null => Rc::new(Vec::new()),
            Val::Arr(a) => a,
            Val::Str(_) => {
                return opt.fail(self, |_| Exn::from(err("Cannot update string slices")));
            }
            v => {
                return opt.fail(v, |v| {
                    Exn::from(err(format!(
                        "Cannot update field at object index of {}",
                        v.kind()
                    )))
                });
            }
        };
        let len = a.len();
        let bound = |v: &Val, def: usize, up: bool| match v {
            Val::Num(n) if !n.f.is_nan() => {
                let f = if up { n.f.ceil() } else { n.f.floor() };
                let f = if f < 0.0 { f + len as f64 } else { f };
                f.clamp(0.0, len as f64) as usize
            }
            _ => def,
        };
        let s = bound(&from, 0, false);
        let e = bound(&to, len, true).max(s);
        let part = Val::arr(a[s..e].to_vec());
        let y = match f(part).next().transpose()? {
            Some(Val::Arr(y)) => rc_take(y),
            Some(Val::Null) | None => Vec::new(),
            Some(_) => {
                return Err(Exn::from(err(
                    "A slice of an array can only be assigned another array",
                )));
            }
        };
        let mut a = a;
        Rc::make_mut(&mut a).splice(s..e, y);
        Ok(Val::Arr(a))
    }

    pub fn as_bool(&self) -> bool {
        !matches!(self, Val::Null | Val::Bool(false))
    }

    pub fn into_string(self) -> Self {
        match self {
            Val::Str(_) => self,
            v => Val::from(v.to_json()),
        }
    }
}

impl From<bool> for Val {
    fn from(b: bool) -> Val {
        Val::Bool(b)
    }
}

impl From<isize> for Val {
    fn from(i: isize) -> Val {
        // Integer literals of the program arrive as integers: one a double cannot hold
        // keeps its digits, as jq's literals do.
        if i.unsigned_abs() as u64 > 1u64 << 53 {
            return Num::literal(&i.to_string()).map_or(Val::num(i as f64), Val::Num);
        }
        Val::num(i as f64)
    }
}

impl From<usize> for Val {
    fn from(i: usize) -> Val {
        Val::num(i as f64)
    }
}

impl From<f64> for Val {
    fn from(f: f64) -> Val {
        Val::num(f)
    }
}

impl From<String> for Val {
    fn from(s: String) -> Val {
        // The interpreter's own message for a path expression on a value, in jq's words.
        if let Some(rest) = s.strip_prefix("invalid path expression with input ") {
            return Val::Str(format!("Invalid path expression with result {rest}").into());
        }
        Val::Str(s.into())
    }
}

impl From<&str> for Val {
    fn from(s: &str) -> Val {
        Val::Str(s.into())
    }
}

impl Val {
    pub fn from_range(start: Option<Val>, end: Option<Val>) -> Val {
        let mut m = Map::new();
        if let Some(s) = start {
            m.insert("start".into(), s);
        }
        if let Some(e) = end {
            m.insert("end".into(), e);
        }
        Val::obj(m)
    }
}

impl FromIterator<Val> for Val {
    fn from_iter<T: IntoIterator<Item = Val>>(iter: T) -> Val {
        Val::arr(iter.into_iter().collect())
    }
}

fn binop_error(l: &Val, r: &Val, what: &str) -> Error {
    err(format!(
        "{} and {} cannot be {what}",
        l.described(),
        r.described()
    ))
}

impl std::ops::Add for Val {
    type Output = ValR;
    fn add(self, rhs: Val) -> ValR {
        match (self, rhs) {
            (Val::Null, x) | (x, Val::Null) => Ok(x),
            (Val::Num(a), Val::Num(b)) => Ok(Val::num(a.f + b.f)),
            (Val::Str(a), Val::Str(b)) => Ok(Val::str(format!("{a}{b}"))),
            (Val::Arr(mut a), Val::Arr(b)) => {
                Rc::make_mut(&mut a).extend(b.iter().cloned());
                Ok(Val::Arr(a))
            }
            (Val::Obj(mut a), Val::Obj(b)) => {
                let m = Rc::make_mut(&mut a);
                for (k, v) in b.iter() {
                    m.insert(k.clone(), v.clone());
                }
                Ok(Val::Obj(a))
            }
            (l, r) => Err(binop_error(&l, &r, "added")),
        }
    }
}

impl std::ops::Sub for Val {
    type Output = ValR;
    fn sub(self, rhs: Val) -> ValR {
        match (self, rhs) {
            (Val::Num(a), Val::Num(b)) => Ok(Val::num(a.f - b.f)),
            (Val::Arr(mut a), Val::Arr(b)) => {
                Rc::make_mut(&mut a).retain(|x| !b.contains(x));
                Ok(Val::Arr(a))
            }
            (l, r) => Err(binop_error(&l, &r, "subtracted")),
        }
    }
}

fn deep_merge(a: &mut Rc<Map>, b: &Map) {
    let m = Rc::make_mut(a);
    for (k, v) in b.iter() {
        match (m.get_mut(k), v) {
            (Some(Val::Obj(l)), Val::Obj(r)) => deep_merge(l, r),
            _ => m.insert(k.clone(), v.clone()),
        }
    }
}

fn repeat(s: &str, n: f64) -> Val {
    if n < 0.0 || n.is_nan() {
        return Val::Null;
    }
    let n = if n > 1e9 { 1e9 } else { n } as usize;
    Val::str(s.repeat(n))
}

impl std::ops::Mul for Val {
    type Output = ValR;
    fn mul(self, rhs: Val) -> ValR {
        match (self, rhs) {
            (Val::Num(a), Val::Num(b)) => Ok(Val::num(a.f * b.f)),
            (Val::Str(s), Val::Num(n)) | (Val::Num(n), Val::Str(s)) => Ok(repeat(&s, n.f)),
            (Val::Obj(mut a), Val::Obj(b)) => {
                deep_merge(&mut a, &b);
                Ok(Val::Obj(a))
            }
            (l, r) => Err(binop_error(&l, &r, "multiplied")),
        }
    }
}

impl std::ops::Div for Val {
    type Output = ValR;
    fn div(self, rhs: Val) -> ValR {
        match (self, rhs) {
            (Val::Num(a), Val::Num(b)) => {
                if b.f == 0.0 {
                    let (l, r) = (Val::Num(a), Val::Num(b));
                    return Err(err(format!(
                        "{} and {} cannot be divided because the divisor is zero",
                        l.described(),
                        r.described()
                    )));
                }
                Ok(Val::num(a.f / b.f))
            }
            (Val::Str(a), Val::Str(b)) => Ok(split_str(&a, &b)),
            (l, r) => Err(binop_error(&l, &r, "divided")),
        }
    }
}

/// `"a,b" / ","`: jq splits on the separator; an empty input gives `[]`.
pub fn split_str(s: &str, sep: &str) -> Val {
    if s.is_empty() {
        return Val::arr(Vec::new());
    }
    if sep.is_empty() {
        return s.chars().map(|c| Val::str(c.to_string())).collect();
    }
    s.split(sep).map(Val::str).collect()
}

impl std::ops::Rem for Val {
    type Output = ValR;
    fn rem(self, rhs: Val) -> ValR {
        match (self, rhs) {
            (Val::Num(a), Val::Num(b)) => {
                let (x, y) = (a.f, b.f);
                if x.is_nan() || y.is_nan() {
                    return Ok(Val::num(f64::NAN));
                }
                let to_i = |f: f64| f.clamp(i64::MIN as f64, i64::MAX as f64) as i64;
                let (xi, yi) = (to_i(x), to_i(y));
                if yi == 0 {
                    let (l, r) = (Val::Num(a), Val::Num(b));
                    return Err(err(format!(
                        "{} and {} cannot be divided (remainder) because the divisor is zero",
                        l.described(),
                        r.described()
                    )));
                }
                Ok(Val::num(xi.checked_rem(yi).unwrap_or(0) as f64))
            }
            (l, r) => Err(binop_error(&l, &r, "divided")),
        }
    }
}

impl std::ops::Neg for Val {
    type Output = ValR;
    fn neg(self) -> ValR {
        match self {
            Val::Num(n) => Ok(Val::num(-n.f)),
            v => Err(err(format!("{} cannot be negated", v.described()))),
        }
    }
}

impl PartialEq for Val {
    fn eq(&self, other: &Val) -> bool {
        match (self, other) {
            (Val::Null, Val::Null) => true,
            (Val::Bool(a), Val::Bool(b)) => a == b,
            (Val::Num(a), Val::Num(b)) => a == b,
            (Val::Str(a), Val::Str(b)) => a == b,
            (Val::Arr(a), Val::Arr(b)) => a == b,
            (Val::Obj(a), Val::Obj(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for Val {}

impl PartialOrd for Val {
    fn partial_cmp(&self, other: &Val) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Val {
    fn cmp(&self, other: &Val) -> Ordering {
        fn rank(v: &Val) -> u8 {
            match v {
                Val::Null => 0,
                Val::Bool(false) => 1,
                Val::Bool(true) => 2,
                Val::Num(_) => 3,
                Val::Str(_) => 4,
                Val::Arr(_) => 5,
                Val::Obj(_) => 6,
            }
        }
        match (self, other) {
            (Val::Num(a), Val::Num(b)) => a.cmp(b),
            (Val::Str(a), Val::Str(b)) => a.as_bytes().cmp(b.as_bytes()),
            (Val::Arr(a), Val::Arr(b)) => a.iter().cmp(b.iter()),
            (Val::Obj(a), Val::Obj(b)) => {
                let (sa, sb) = (a.sorted(), b.sorted());
                let ka = sa.iter().map(|(k, _)| k.as_bytes());
                let kb = sb.iter().map(|(k, _)| k.as_bytes());
                ka.cmp(kb)
                    .then_with(|| sa.iter().map(|(_, v)| *v).cmp(sb.iter().map(|(_, v)| *v)))
            }
            (a, b) => rank(a).cmp(&rank(b)),
        }
    }
}

impl fmt::Display for Val {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_json())
    }
}

/// The number of a value for the natives: jq truncates doubles.
pub fn as_isize(v: &Val) -> Option<isize> {
    match v {
        Val::Num(n) if n.f.is_finite() => Some(n.f as isize),
        _ => None,
    }
}

#[allow(dead_code)]
fn _key(v: &Val) -> Option<Rc<str>> {
    Val::key_rc(v)
}
