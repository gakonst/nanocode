use std::{
    borrow::{Borrow, Cow},
    fmt::Display,
    hash::{Hash, Hasher},
    str::FromStr,
};

use serde_json::Value;
use std::collections::{HashMap, hash_map::DefaultHasher};
use url::Url;

use crate::CompileError;

const ASCII_LIMIT: u8 = 0x80;
const FRAGMENT_ESCAPED: &[u8] = b"% \"<>`";
const HEX_UPPER: &[u8; 16] = b"0123456789ABCDEF";

fn hex_value(byte: u8) -> Option<u8> {
    char::from(byte).to_digit(16).map(|digit| digit as u8)
}

fn percent_decode(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let escaped = (bytes[index] == b'%')
            .then(|| {
                Some((
                    hex_value(*bytes.get(index + 1)?)?,
                    hex_value(*bytes.get(index + 2)?)?,
                ))
            })
            .flatten();
        match escaped {
            Some((high, low)) => {
                decoded.push(high << 4 | low);
                index += 3;
            }
            None => {
                decoded.push(bytes[index]);
                index += 1;
            }
        }
    }
    decoded
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub(crate) struct JsonPointer(pub(crate) String);

impl JsonPointer {
    pub(crate) fn escape(token: &str) -> Cow<'_, str> {
        const SPECIAL: [char; 2] = ['~', '/'];
        if token.contains(SPECIAL) {
            token.replace('~', "~0").replace('/', "~1").into()
        } else {
            token.into()
        }
    }

    pub(crate) fn unescape(mut tok: &str) -> Result<Cow<'_, str>, ()> {
        let Some(mut tilde) = tok.find('~') else {
            return Ok(Cow::Borrowed(tok));
        };
        let mut s = String::with_capacity(tok.len());
        loop {
            s.push_str(&tok[..tilde]);
            tok = &tok[tilde + 1..];
            match tok.chars().next() {
                Some('1') => s.push('/'),
                Some('0') => s.push('~'),
                _ => return Err(()),
            }
            tok = &tok[1..];
            let Some(i) = tok.find('~') else {
                s.push_str(tok);
                break;
            };
            tilde = i;
        }
        Ok(Cow::Owned(s))
    }

    pub(crate) fn lookup<'a>(
        &self,
        mut v: &'a Value,
        v_url: &Url,
    ) -> Result<&'a Value, CompileError> {
        for tok in self.0.split('/').skip(1) {
            let Ok(tok) = Self::unescape(tok) else {
                let loc = UrlFrag::format(v_url, self.as_str());
                return Err(CompileError::InvalidJsonPointer(loc));
            };
            match v {
                Value::Object(obj) => {
                    if let Some(pvalue) = obj.get(tok.as_ref()) {
                        v = pvalue;
                        continue;
                    }
                }
                Value::Array(arr) => {
                    if let Ok(i) = usize::from_str(tok.as_ref())
                        && let Some(item) = arr.get(i)
                    {
                        v = item;
                        continue;
                    };
                }
                _ => {}
            }
            let loc = UrlFrag::format(v_url, self.as_str());
            return Err(CompileError::JsonPointerNotFound(loc));
        }
        Ok(v)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn concat(&self, next: &Self) -> Self {
        Self(format!("{}{}", self.0, next.0))
    }

    pub(crate) fn append(&self, tok: &str) -> Self {
        Self(format!("{}/{}", self, Self::escape(tok)))
    }

    pub(crate) fn append2(&self, tok1: &str, tok2: &str) -> Self {
        Self(format!(
            "{}/{}/{}",
            self,
            Self::escape(tok1),
            Self::escape(tok2)
        ))
    }
}

impl Display for JsonPointer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl Borrow<str> for JsonPointer {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl From<&str> for JsonPointer {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub(crate) struct Anchor(pub(crate) String);

impl Display for Anchor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl Borrow<str> for Anchor {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl From<&str> for Anchor {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum Fragment {
    Anchor(Anchor),
    JsonPointer(JsonPointer),
}

impl Fragment {
    pub(crate) fn split(s: &str) -> Result<(&str, Self), CompileError> {
        let (u, frag) = split(s);
        let frag =
            String::from_utf8(percent_decode(frag)).map_err(|src| CompileError::ParseUrlError {
                url: s.to_string(),
                src: src.utf8_error().into(),
            })?;
        let frag = if frag.is_empty() || frag.starts_with('/') {
            Self::JsonPointer(JsonPointer(frag))
        } else {
            Self::Anchor(Anchor(frag))
        };
        Ok((u, frag))
    }

    pub(crate) fn encode(frag: &str) -> String {
        let mut encoded = String::with_capacity(frag.len());
        for &byte in frag.as_bytes() {
            if byte.is_ascii_control() || byte >= ASCII_LIMIT || FRAGMENT_ESCAPED.contains(&byte) {
                encoded.push('%');
                encoded.push(char::from(HEX_UPPER[usize::from(byte >> 4)]));
                encoded.push(char::from(HEX_UPPER[usize::from(byte & 0x0F)]));
            } else {
                encoded.push(char::from(byte));
            }
        }
        encoded
    }

    pub(crate) fn as_str(&self) -> &str {
        match self {
            Self::Anchor(s) => &s.0,
            Self::JsonPointer(s) => &s.0,
        }
    }
}

#[derive(Clone)]
pub(crate) struct UrlFrag {
    pub(crate) url: Url,
    pub(crate) frag: Fragment,
}

impl UrlFrag {
    pub(crate) fn absolute(input: &str) -> Result<Self, CompileError> {
        let (u, frag) = Fragment::split(input)?;

        match Url::parse(u) {
            Ok(url) => Ok(Self { url, frag }),
            Err(e) => Err(CompileError::ParseUrlError {
                url: u.to_owned(),
                src: e.into(),
            }),
        }
    }

    pub(crate) fn join(url: &Url, input: &str) -> Result<Self, CompileError> {
        let (input, frag) = Fragment::split(input)?;
        if input.is_empty() {
            return Ok(Self {
                url: url.clone(),
                frag,
            });
        }
        let url = url.join(input).map_err(|e| CompileError::ParseUrlError {
            url: input.to_string(),
            src: e.into(),
        })?;

        Ok(Self { url, frag })
    }

    pub(crate) fn format(url: &Url, frag: &str) -> String {
        if frag.is_empty() {
            url.to_string()
        } else {
            format!("{}#{}", url, Fragment::encode(frag))
        }
    }
}

impl Display for UrlFrag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}#{}", self.url, Fragment::encode(self.frag.as_str()))
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub(crate) struct UrlPtr {
    pub(crate) url: Url,
    pub(crate) ptr: JsonPointer,
}

impl UrlPtr {
    pub(crate) fn lookup<'a>(&self, doc: &'a Value) -> Result<&'a Value, CompileError> {
        self.ptr.lookup(doc, &self.url)
    }

    pub(crate) fn format(&self, tok: &str) -> String {
        format!(
            "{}#{}/{}",
            self.url,
            Fragment::encode(self.ptr.as_str()),
            Fragment::encode(JsonPointer::escape(tok).as_ref()),
        )
    }
}

impl Display for UrlPtr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}#{}", self.url, Fragment::encode(self.ptr.as_str()))
    }
}

pub(crate) fn is_integer(v: &Value) -> bool {
    match v {
        Value::Number(n) => {
            n.is_i64() || n.is_u64() || n.as_f64().filter(|n| n.fract() == 0.0).is_some()
        }
        _ => false,
    }
}

pub(crate) fn quote<T>(s: &T) -> String
where
    T: AsRef<str> + std::fmt::Debug + ?Sized,
{
    let s = format!("{s:?}").replace(r#"\""#, "\"").replace('\'', r"\'");
    format!("'{}'", &s[1..s.len() - 1])
}

pub(crate) fn join_iter<T>(iterable: T, sep: &str) -> String
where
    T: IntoIterator,
    T::Item: Display,
{
    iterable
        .into_iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(sep)
}

pub(crate) fn escape(token: &str) -> Cow<'_, str> {
    JsonPointer::escape(token)
}

pub(crate) fn split(url: &str) -> (&str, &str) {
    if let Some(i) = url.find('#') {
        (&url[..i], &url[i + 1..])
    } else {
        (url, "")
    }
}

pub(crate) fn equals(v1: &Value, v2: &Value) -> bool {
    match (v1, v2) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(b1), Value::Bool(b2)) => b1 == b2,
        (Value::Number(n1), Value::Number(n2)) => crate::numbers::compare(n1, n2).is_eq(),
        (Value::String(s1), Value::String(s2)) => s1 == s2,
        (Value::Array(arr1), Value::Array(arr2)) => {
            if arr1.len() != arr2.len() {
                return false;
            }
            arr1.iter().zip(arr2).all(|(e1, e2)| equals(e1, e2))
        }
        (Value::Object(obj1), Value::Object(obj2)) => {
            if obj1.len() != obj2.len() {
                return false;
            }
            for (k1, v1) in obj1 {
                if let Some(v2) = obj2.get(k1) {
                    if !equals(v1, v2) {
                        return false;
                    }
                } else {
                    return false;
                }
            }
            true
        }
        _ => false,
    }
}

pub(crate) fn duplicates(arr: &Vec<Value>) -> Option<(usize, usize)> {
    match arr.as_slice() {
        [e0, e1] => {
            if equals(e0, e1) {
                return Some((0, 1));
            }
        }
        [e0, e1, e2] => {
            if equals(e0, e1) {
                return Some((0, 1));
            } else if equals(e0, e2) {
                return Some((0, 2));
            } else if equals(e1, e2) {
                return Some((1, 2));
            }
        }
        _ => {
            let len = arr.len();
            if len <= 20 {
                for i in 0..len - 1 {
                    for j in i + 1..len {
                        if equals(&arr[i], &arr[j]) {
                            return Some((i, j));
                        }
                    }
                }
            } else {
                let mut seen = HashMap::with_capacity(len);
                for (i, item) in arr.iter().enumerate() {
                    if let Some(j) = seen.insert(HashedValue(item), i) {
                        return Some((j, i));
                    }
                }
            }
        }
    }
    None
}

pub(crate) struct HashedValue<'a>(pub(crate) &'a Value);

impl PartialEq for HashedValue<'_> {
    fn eq(&self, other: &Self) -> bool {
        equals(self.0, other.0)
    }
}

impl Eq for HashedValue<'_> {}

impl Hash for HashedValue<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self.0 {
            Value::Null => state.write_u32(3_221_225_473),
            Value::Bool(b) => b.hash(state),
            Value::Number(num) => {
                if let Some(num) = num.as_f64() {
                    if num == 0.0 {
                        0.0f64.to_bits().hash(state);
                    } else {
                        num.to_bits().hash(state);
                    }
                } else if let Some(num) = num.as_u64() {
                    num.hash(state);
                } else if let Some(num) = num.as_i64() {
                    num.hash(state);
                }
            }
            Value::String(str) => str.hash(state),
            Value::Array(arr) => {
                for item in arr {
                    HashedValue(item).hash(state);
                }
            }
            Value::Object(obj) => {
                let mut hash = 0;
                for (pname, pvalue) in obj {
                    let mut hasher = DefaultHasher::default();
                    pname.hash(&mut hasher);
                    HashedValue(pvalue).hash(&mut hasher);
                    hash ^= hasher.finish();
                }
                state.write_u64(hash);
            }
        }
    }
}
