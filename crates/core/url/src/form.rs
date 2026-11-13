use std::borrow::{Borrow, Cow};

use crate::encode::{percent_decode, push_byte_encoded};

const fn unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'*' | b'-' | b'.' | b'_')
}

#[must_use]
pub fn byte_serialize(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len());
    push_serialized(&mut out, input);
    out
}

fn push_serialized(out: &mut String, input: &[u8]) {
    for &byte in input {
        if unreserved(byte) {
            out.push(char::from(byte));
        } else if byte == b' ' {
            out.push('+');
        } else {
            push_byte_encoded(out, byte);
        }
    }
}

pub(crate) fn push_pair(out: &mut String, name: &str, value: &str) {
    push_key(out, name);
    out.push('=');
    push_serialized(out, value.as_bytes());
}

pub(crate) fn push_key(out: &mut String, name: &str) {
    if !out.is_empty() {
        out.push('&');
    }
    push_serialized(out, name.as_bytes());
}

fn decode(input: &[u8]) -> Cow<'_, str> {
    if !input.contains(&b'+') {
        return match percent_decode(input) {
            Cow::Borrowed(bytes) => String::from_utf8_lossy(bytes),
            Cow::Owned(bytes) => Cow::Owned(String::from_utf8_lossy(&bytes).into_owned()),
        };
    }
    let spaced: Vec<u8> = input
        .iter()
        .map(|&byte| if byte == b'+' { b' ' } else { byte })
        .collect();
    Cow::Owned(String::from_utf8_lossy(&percent_decode(&spaced)).into_owned())
}

#[derive(Clone, Copy)]
pub struct Parse<'a> {
    input: &'a [u8],
}

#[must_use]
pub const fn parse(input: &[u8]) -> Parse<'_> {
    Parse { input }
}

impl<'a> Iterator for Parse<'a> {
    type Item = (Cow<'a, str>, Cow<'a, str>);

    fn next(&mut self) -> Option<Self::Item> {
        while !self.input.is_empty() {
            let end = self
                .input
                .iter()
                .position(|&byte| byte == b'&')
                .unwrap_or(self.input.len());
            let sequence = &self.input[..end];
            self.input = self.input.get(end + 1..).unwrap_or(&[]);
            if sequence.is_empty() {
                continue;
            }
            let (name, value) = match sequence.iter().position(|&byte| byte == b'=') {
                Some(split) => (&sequence[..split], &sequence[split + 1..]),
                None => (sequence, &[][..]),
            };
            return Some((decode(name), decode(value)));
        }
        None
    }
}

pub struct Serializer {
    target: String,
    start: usize,
}

impl Serializer {
    #[must_use]
    pub const fn new(target: String) -> Self {
        let start = target.len();
        Self { target, start }
    }

    fn push(&mut self) {
        if self.target.len() > self.start {
            self.target.push('&');
        }
    }

    pub fn append_pair(&mut self, name: &str, value: &str) -> &mut Self {
        self.push();
        push_serialized(&mut self.target, name.as_bytes());
        self.target.push('=');
        push_serialized(&mut self.target, value.as_bytes());
        self
    }

    pub fn append_key_only(&mut self, name: &str) -> &mut Self {
        self.push();
        push_serialized(&mut self.target, name.as_bytes());
        self
    }

    pub fn extend_pairs<I, K, V>(&mut self, pairs: I) -> &mut Self
    where
        I: IntoIterator,
        I::Item: Borrow<(K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        for pair in pairs {
            let (name, value) = pair.borrow();
            self.append_pair(name.as_ref(), value.as_ref());
        }
        self
    }

    pub fn finish(&mut self) -> String {
        self.start = 0;
        std::mem::take(&mut self.target)
    }
}
