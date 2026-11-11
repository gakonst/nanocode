use std::collections::{HashMap, hash_map::Entry};

use super::Record;

const DIGEST_HEX: usize = 64;
const BLAKE: &str = "b3:";
const CHUNK: &str = "c:";

#[derive(Clone, PartialEq, Eq, Hash)]
enum RecordKey {
    Blake([u8; 32]),
    Chunk([u8; 32]),
    Text(Box<str>),
}

fn digest(hex: &str) -> Option<[u8; 32]> {
    let hex = hex.as_bytes();
    if hex.len() != DIGEST_HEX {
        return None;
    }
    let nibble = |byte: u8| match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    };
    let mut digest = [0; 32];
    for (byte, pair) in digest.iter_mut().zip(hex.chunks_exact(2)) {
        *byte = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Some(digest)
}

impl RecordKey {
    fn new(key: &str) -> Self {
        if let Some(digest) = key.strip_prefix(BLAKE).and_then(digest) {
            return Self::Blake(digest);
        }
        if let Some(digest) = key.strip_prefix(CHUNK).and_then(digest) {
            return Self::Chunk(digest);
        }
        Self::Text(key.into())
    }

    fn text(&self) -> String {
        let (prefix, digest) = match self {
            Self::Blake(digest) => (BLAKE, digest),
            Self::Chunk(digest) => (CHUNK, digest),
            Self::Text(text) => return text.to_string(),
        };
        let mut text = String::with_capacity(prefix.len() + DIGEST_HEX);
        text.push_str(prefix);
        for byte in digest {
            for nibble in [byte >> 4, byte & 0xf] {
                text.push(char::from(b"0123456789abcdef"[usize::from(nibble)]));
            }
        }
        text
    }
}

#[derive(Default)]
pub(super) struct RecordMap(HashMap<RecordKey, Record>);

impl RecordMap {
    pub(super) fn get(&self, key: &str) -> Option<&Record> {
        self.0.get(&RecordKey::new(key))
    }

    pub(super) fn get_mut(&mut self, key: &str) -> Option<&mut Record> {
        self.0.get_mut(&RecordKey::new(key))
    }

    pub(super) fn contains_key(&self, key: &str) -> bool {
        self.0.contains_key(&RecordKey::new(key))
    }

    pub(super) fn add(&mut self, key: &str, record: Record) -> bool {
        match self.0.entry(RecordKey::new(key)) {
            Entry::Vacant(added) => {
                added.insert(record);
                true
            }
            Entry::Occupied(_) => false,
        }
    }

    pub(super) fn merge(&mut self, later: Self) -> bool {
        later
            .0
            .into_iter()
            .all(|(key, later)| match self.0.entry(key) {
                Entry::Vacant(added) => {
                    added.insert(later);
                    true
                }
                Entry::Occupied(stored) => matches!(
                    (stored.get(), later),
                    (Record::Value { digest, .. }, Record::Value { digest: repeated, .. })
                        if *digest == repeated
                ),
            })
    }

    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(super) fn keys(&self) -> impl Iterator<Item = String> + '_ {
        self.0.keys().map(RecordKey::text)
    }

    pub(super) fn values(&self) -> impl Iterator<Item = &Record> {
        self.0.values()
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = (String, &Record)> {
        self.0.iter().map(|(key, record)| (key.text(), record))
    }

    pub(super) fn iter_mut(&mut self) -> impl Iterator<Item = (String, &mut Record)> {
        self.0.iter_mut().map(|(key, record)| (key.text(), record))
    }
}

impl FromIterator<(String, Record)> for RecordMap {
    fn from_iter<T: IntoIterator<Item = (String, Record)>>(records: T) -> Self {
        Self(
            records
                .into_iter()
                .map(|(key, record)| (RecordKey::new(&key), record))
                .collect(),
        )
    }
}
