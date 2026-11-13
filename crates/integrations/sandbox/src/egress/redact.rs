use crate::egress::wire::Secret;
use nanocode_sys::base64::STANDARD;
use bytes::Bytes;
use futures_util::stream::unfold;
use http::header::{self, HeaderMap, HeaderName, HeaderValue};
use nanocode_http::{
    server::{BodyStream, Incoming},
    wire::WireError,
};
use std::{cmp::Reverse, error::Error, fmt, sync::Arc, time::Duration};
use tokio::time::timeout;

const MIN_MASKED: usize = 4;
const MIN_MASKED_TOKEN: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Injecting {
    Swap { prefix: &'static str },
    Set,
}

#[must_use]
pub fn masks(value: &str) -> Vec<Vec<u8>> {
    let mut forms = vec![value.as_bytes().to_vec()];
    if let Some((_, token)) = value.trim().rsplit_once([' ', '\t'])
        && token.len() >= MIN_MASKED_TOKEN
    {
        forms.push(token.as_bytes().to_vec());
    }
    let escaped: Vec<Vec<u8>> = forms
        .iter()
        .flat_map(|form| [json_escape(form, true), json_escape(form, false)])
        .collect();
    forms.extend(escaped);
    forms.retain(|form| form.len() >= MIN_MASKED);
    forms.sort_by_key(|form| Reverse(form.len()));
    forms.dedup();
    forms
}

fn json_escape(bytes: &[u8], slash: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for &b in bytes {
        match b {
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'"' => out.extend_from_slice(b"\\\""),
            b'/' if slash => out.extend_from_slice(b"\\/"),
            0..=0x1f => out.extend_from_slice(format!("\\u{b:04x}").as_bytes()),
            _ => out.push(b),
        }
    }
    out
}

pub fn mask(bytes: &mut [u8], forms: &[Vec<u8>]) {
    let masked = Redactions::stars(forms).redact(bytes);
    bytes.copy_from_slice(&masked);
}

#[derive(Clone, Default)]
pub struct Redactions {
    pairs: Arc<[(Vec<u8>, Vec<u8>)]>,
}

impl Redactions {
    pub fn new(pairs: impl IntoIterator<Item = (Vec<u8>, Vec<u8>)>) -> Self {
        let mut pairs: Vec<(Vec<u8>, Vec<u8>)> = pairs
            .into_iter()
            .filter(|(value, _)| value.len() >= MIN_MASKED)
            .collect();
        let escaped: Vec<(Vec<u8>, Vec<u8>)> = pairs
            .iter()
            .flat_map(|(value, replacement)| {
                [true, false]
                    .map(|slash| (json_escape(value, slash), json_escape(replacement, slash)))
            })
            .filter(|(value, _)| !pairs.iter().any(|(raw, _)| raw == value))
            .collect();
        pairs.extend(escaped);
        Self::longest_first(pairs)
    }

    #[must_use]
    pub fn masking(value: &str) -> Self {
        Self::stars(&masks(value))
    }

    fn stars(forms: &[Vec<u8>]) -> Self {
        let pairs: Vec<(Vec<u8>, Vec<u8>)> = forms
            .iter()
            .map(|form| (form.clone(), vec![b'*'; form.len()]))
            .collect();
        Self::longest_first(pairs)
    }

    fn longest_first(mut pairs: Vec<(Vec<u8>, Vec<u8>)>) -> Self {
        pairs.sort_by_key(|(value, _)| Reverse(value.len()));
        pairs.dedup_by(|a, b| a.0 == b.0);
        Self {
            pairs: pairs.into(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }

    #[must_use]
    pub fn redact(&self, bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(bytes.len());
        let mut at = 0;
        'scan: while at < bytes.len() {
            for (value, replacement) in self.pairs.iter() {
                if bytes[at..].starts_with(value) {
                    out.extend_from_slice(replacement);
                    at += value.len();
                    continue 'scan;
                }
            }
            out.push(bytes[at]);
            at += 1;
        }
        out
    }

    fn held(&self, bytes: &[u8]) -> usize {
        self.pairs
            .iter()
            .filter_map(|(value, _)| {
                (1..value.len().min(bytes.len() + 1))
                    .rev()
                    .find(|len| bytes.ends_with(&value[..*len]))
            })
            .max()
            .unwrap_or(0)
    }
}

#[derive(Default)]
pub struct Masker {
    redactions: Redactions,
    carry: Vec<u8>,
}

impl Masker {
    #[must_use]
    pub fn new(forms: &[Vec<u8>]) -> Self {
        Self::replacing(Redactions::stars(forms))
    }

    #[must_use]
    pub const fn replacing(redactions: Redactions) -> Self {
        Self {
            redactions,
            carry: Vec::new(),
        }
    }

    pub fn push(&mut self, data: &[u8]) -> Vec<u8> {
        self.carry.extend_from_slice(data);
        self.carry = self.redactions.redact(&self.carry);
        let keep = self.redactions.held(&self.carry);
        self.carry.drain(..self.carry.len() - keep).collect()
    }

    pub fn finish(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.carry)
    }
}

#[derive(Debug)]
pub enum Failed {
    Quiet,
    Host(WireError),
}

impl fmt::Display for Failed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Quiet => f.write_str("the host sent nothing in time"),
            Self::Host(error) => error.fmt(f),
        }
    }
}

impl Error for Failed {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Quiet => None,
            Self::Host(error) => Some(error),
        }
    }
}

pub(crate) fn inject(
    headers: &mut HeaderMap,
    secrets: &[&Secret],
    injecting: Injecting,
) -> Vec<(String, String)> {
    let prefix = match injecting {
        Injecting::Set => {
            for secret in secrets {
                if let (Ok(name), Ok(mut value)) = (
                    HeaderName::try_from(secret.header.as_str()),
                    HeaderValue::try_from(secret.value.as_str()),
                ) {
                    value.set_sensitive(true);
                    headers.insert(name, value);
                }
            }
            return Vec::new();
        }
        Injecting::Swap { prefix } => prefix,
    };
    let mut names: Vec<HeaderName> = secrets
        .iter()
        .filter_map(|secret| HeaderName::try_from(secret.header.as_str()).ok())
        .collect();
    names.dedup();
    let mut written = Vec::new();
    for name in names {
        let own: Vec<&Secret> = secrets
            .iter()
            .copied()
            .filter(|secret| secret.header.eq_ignore_ascii_case(name.as_str()))
            .collect();
        let Some(current) = headers
            .get(&name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
        else {
            continue;
        };
        let replaced = swap(&current, &own, prefix).or_else(|| {
            let encoded = current
                .strip_prefix("Basic ")
                .filter(|_| name == header::AUTHORIZATION)?
                .trim();
            let decoded = String::from_utf8(STANDARD.decode(encoded).ok()?).ok()?;
            let payload = STANDARD.encode(swap(&decoded, &own, prefix)?);
            written.push((payload.clone(), encoded.to_owned()));
            Some(format!("Basic {payload}"))
        });
        if let Some(replaced) = replaced
            && let Ok(mut value) = HeaderValue::try_from(replaced.as_str())
        {
            value.set_sensitive(true);
            headers.insert(name, value);
            written.push((replaced, current));
        }
    }
    written
}

pub(crate) fn redactions(
    secrets: &[&Secret],
    written: &[(String, String)],
    injecting: Injecting,
) -> Redactions {
    if injecting == Injecting::Set {
        let forms: Vec<Vec<u8>> = secrets
            .iter()
            .flat_map(|secret| masks(&secret.value))
            .collect();
        return Redactions::stars(&forms);
    }
    Redactions::new(
        secrets
            .iter()
            .map(|secret| (secret.value.as_str(), secret.placeholder.as_str()))
            .chain(
                written
                    .iter()
                    .map(|(sent, original)| (sent.as_str(), original.as_str())),
            )
            .map(|(value, original)| (value.as_bytes().to_vec(), original.as_bytes().to_vec())),
    )
}

pub(crate) fn headers(headers: &mut HeaderMap, pairs: &Redactions) {
    for value in headers.values_mut() {
        let redacted = pairs.redact(value.as_bytes());
        if redacted != value.as_bytes()
            && let Ok(clean) = HeaderValue::from_bytes(&redacted)
        {
            *value = clean;
        }
    }
}

pub(crate) fn reason(reason: &mut Option<String>, pairs: &Redactions) {
    let Some(text) = reason.as_deref() else {
        return;
    };
    let redacted = pairs.redact(text.as_bytes());
    if redacted == text.as_bytes() {
        return;
    }
    let printable = redacted
        .iter()
        .all(|byte| matches!(byte, b'\t' | b' ' | b'!'..=b'~' | 0x80..));
    *reason = String::from_utf8(redacted).ok().filter(|_| printable);
}

#[derive(Default)]
struct Tail {
    stream: Option<Incoming>,
    masker: Masker,
}

pub(crate) fn body(inner: Incoming, pairs: Redactions, idle: Duration) -> BodyStream {
    let start = Tail {
        stream: Some(inner),
        masker: Masker::replacing(pairs.clone()),
    };
    let chunks = unfold(start, move |mut tail: Tail| {
        let pairs = pairs.clone();
        async move {
            loop {
                let Some(inner) = tail.stream.as_mut() else {
                    let carry = tail.masker.finish();
                    if !carry.is_empty() {
                        let rest = Bytes::from(pairs.redact(&carry));
                        return Some((Ok(rest), tail));
                    }
                    return None;
                };
                let Ok(next) = timeout(idle, inner.chunk()).await else {
                    return Some((Err(Failed::Quiet.into()), Tail::default()));
                };
                let data = match next {
                    Ok(None) => {
                        tail.stream = None;
                        continue;
                    }
                    Err(error) => {
                        return Some((Err(Failed::Host(error).into()), Tail::default()));
                    }
                    Ok(Some(data)) => data,
                };
                if pairs.is_empty() {
                    return Some((Ok(data), tail));
                }
                let out = tail.masker.push(&data);
                if !out.is_empty() {
                    return Some((Ok(Bytes::from(out)), tail));
                }
            }
        }
    });
    Box::pin(chunks)
}

fn swap(text: &str, secrets: &[&Secret], prefix: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut changed = false;
    while let Some(at) = rest.find(prefix) {
        out.push_str(&rest[..at]);
        let after = &rest[at + prefix.len()..];
        let len = after
            .bytes()
            .take_while(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || *b == b'_')
            .count();
        let token = &rest[at..at + prefix.len() + len];
        match secrets.iter().find(|secret| secret.placeholder == token) {
            Some(secret) => {
                out.push_str(&secret.value);
                changed = true;
            }
            None => out.push_str(token),
        }
        rest = &after[len..];
    }
    out.push_str(rest);
    changed.then_some(out)
}
