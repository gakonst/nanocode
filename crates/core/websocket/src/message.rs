use std::{fmt, ops::Deref};

use crate::{
    error::{CapacityError, Error, Result},
    frame::{CloseFrame, Frame},
    protocol::byte_count,
};

const UTF8_SEQUENCE_BYTES_MAX: usize = 4;

#[derive(Debug, Default, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Utf8Bytes(String);

impl Utf8Bytes {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl Deref for Utf8Bytes {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for Utf8Bytes {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl AsRef<[u8]> for Utf8Bytes {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl fmt::Display for Utf8Bytes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<String> for Utf8Bytes {
    fn from(text: String) -> Self {
        Self(text)
    }
}

impl From<&str> for Utf8Bytes {
    fn from(text: &str) -> Self {
        Self(text.to_owned())
    }
}

impl From<&String> for Utf8Bytes {
    fn from(text: &String) -> Self {
        Self(text.clone())
    }
}

impl From<Utf8Bytes> for String {
    fn from(text: Utf8Bytes) -> Self {
        text.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Text(Utf8Bytes),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Close(Option<CloseFrame>),
    Frame(Frame),
}

impl Message {
    pub fn text(text: impl Into<Utf8Bytes>) -> Self {
        Self::Text(text.into())
    }
}

pub(crate) enum IncompleteMessage {
    Text(TextCollector),
    Binary(Vec<u8>),
}

impl IncompleteMessage {
    pub(crate) const fn text() -> Self {
        Self::Text(TextCollector {
            data: String::new(),
            incomplete: None,
        })
    }

    fn len(&self) -> usize {
        match self {
            Self::Text(text) => text.len(),
            Self::Binary(binary) => binary.len(),
        }
    }

    pub(crate) fn extend(&mut self, tail: &[u8], size_max: usize) -> Result<()> {
        let size = self.len();
        if size > size_max || tail.len() > size_max - size {
            return Err(Error::Capacity(CapacityError::MessageTooLong {
                size: byte_count(size) + byte_count(tail.len()),
                max_size: size_max,
            }));
        }
        match self {
            Self::Binary(binary) => {
                binary.extend_from_slice(tail);
                Ok(())
            }
            Self::Text(text) => text.extend(tail),
        }
    }

    pub(crate) fn complete(self) -> Result<Message> {
        match self {
            Self::Binary(binary) => Ok(Message::Binary(binary)),
            Self::Text(text) => Ok(Message::Text(text.into_string()?.into())),
        }
    }
}

pub(crate) struct TextCollector {
    data: String,
    incomplete: Option<Incomplete>,
}

#[derive(Debug, Clone, Copy)]
struct Incomplete {
    buffer: [u8; UTF8_SEQUENCE_BYTES_MAX],
    buffer_len: u8,
}

enum Completion {
    Valid { consumed: usize },
    Invalid,
    Pending,
}

impl Incomplete {
    fn new(bytes: &[u8]) -> Self {
        assert!(bytes.len() < UTF8_SEQUENCE_BYTES_MAX);
        let mut buffer = [0; UTF8_SEQUENCE_BYTES_MAX];
        buffer[..bytes.len()].copy_from_slice(bytes);
        Self {
            buffer,
            buffer_len: u8::try_from(bytes.len()).expect("an incomplete sequence is under 4 bytes"),
        }
    }

    fn try_complete(&mut self, input: &[u8]) -> Completion {
        let initial = usize::from(self.buffer_len);
        let copied = (UTF8_SEQUENCE_BYTES_MAX - initial).min(input.len());
        self.buffer[initial..initial + copied].copy_from_slice(&input[..copied]);
        let spliced = &self.buffer[..initial + copied];
        let set_length =
            |length: usize| u8::try_from(length).expect("a UTF-8 sequence is at most 4 bytes");
        match std::str::from_utf8(spliced) {
            Ok(_) => {
                self.buffer_len = set_length(spliced.len());
                Completion::Valid { consumed: copied }
            }
            Err(error) if error.valid_up_to() > 0 => {
                let valid = error.valid_up_to();
                self.buffer_len = set_length(valid);
                Completion::Valid {
                    consumed: valid
                        .checked_sub(initial)
                        .expect("a completed sequence covers the buffered bytes"),
                }
            }
            Err(error) => match error.error_len() {
                Some(invalid) => {
                    self.buffer_len = set_length(invalid);
                    Completion::Invalid
                }
                None => {
                    self.buffer_len = set_length(spliced.len());
                    Completion::Pending
                }
            },
        }
    }

    fn bytes(&self) -> &[u8] {
        &self.buffer[..usize::from(self.buffer_len)]
    }
}

impl TextCollector {
    fn len(&self) -> usize {
        self.data.len().saturating_add(
            self.incomplete
                .map_or(0, |incomplete| usize::from(incomplete.buffer_len)),
        )
    }

    fn extend(&mut self, tail: &[u8]) -> Result<()> {
        let mut input = tail;
        if let Some(mut incomplete) = self.incomplete.take() {
            match incomplete.try_complete(input) {
                Completion::Valid { consumed } => {
                    input = &input[consumed..];
                    self.data.push_str(
                        std::str::from_utf8(incomplete.bytes())
                            .expect("a completed sequence is valid UTF-8"),
                    );
                }
                Completion::Invalid => {
                    return Err(Error::Utf8(
                        String::from_utf8_lossy(incomplete.bytes()).into(),
                    ));
                }
                Completion::Pending => {
                    input = &[];
                    self.incomplete = Some(incomplete);
                }
            }
        }
        if input.is_empty() {
            return Ok(());
        }
        let error = match std::str::from_utf8(input) {
            Ok(text) => {
                self.data.push_str(text);
                return Ok(());
            }
            Err(error) => error,
        };
        let (valid, after) = input.split_at(error.valid_up_to());
        self.data
            .push_str(std::str::from_utf8(valid).expect("the prefix before an error is valid"));
        match error.error_len() {
            Some(invalid) => Err(Error::Utf8(
                String::from_utf8_lossy(&after[..invalid]).into(),
            )),
            None => {
                self.incomplete = Some(Incomplete::new(after));
                Ok(())
            }
        }
    }

    fn into_string(self) -> Result<String> {
        match self.incomplete {
            Some(incomplete) => Err(Error::Utf8(format!("incomplete string: {incomplete:?}"))),
            None => Ok(self.data),
        }
    }
}
