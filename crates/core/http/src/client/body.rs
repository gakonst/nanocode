use std::{fmt, pin::Pin};

use bytes::Bytes;
use nanocode_sync::Slot;
use nanocode_sync::{Stream, TryStreamExt};

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;
pub type ChunkStream = Pin<Box<dyn Stream<Item = Result<Bytes, BoxError>> + Send>>;

pub struct Body(Kind);

enum Kind {
    Bytes(Bytes),
    Stream(Slot<ChunkStream>, Option<u64>),
}

impl Body {
    #[must_use]
    pub const fn empty() -> Self {
        Self(Kind::Bytes(Bytes::new()))
    }

    pub fn wrap_stream<S>(stream: S) -> Self
    where
        S: nanocode_sync::TryStream + Send + 'static,
        S::Error: Into<BoxError>,
        Bytes: From<S::Ok>,
    {
        Self(Kind::Stream(
            Slot::new(Some(Box::pin(
                stream.map_ok(Bytes::from).map_err(Into::into),
            ))),
            None,
        ))
    }

    pub fn sized_stream<S>(stream: S, length: u64) -> Self
    where
        S: nanocode_sync::TryStream + Send + 'static,
        S::Error: Into<BoxError>,
        Bytes: From<S::Ok>,
    {
        Self(Kind::Stream(
            Slot::new(Some(Box::pin(
                stream.map_ok(Bytes::from).map_err(Into::into),
            ))),
            Some(length),
        ))
    }

    #[must_use]
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match &self.0 {
            Kind::Bytes(bytes) => Some(bytes),
            Kind::Stream(..) => None,
        }
    }

    #[must_use]
    pub fn try_clone(&self) -> Option<Self> {
        match &self.0 {
            Kind::Bytes(bytes) => Some(Self(Kind::Bytes(bytes.clone()))),
            Kind::Stream(..) => None,
        }
    }

    pub(crate) fn into_parts(self) -> Result<Bytes, (ChunkStream, Option<u64>)> {
        match self.0 {
            Kind::Bytes(bytes) => Ok(bytes),
            Kind::Stream(stream, length) => Err((
                stream
                    .take()
                    .expect("a body stream is taken once, when the body is sent"),
                length,
            )),
        }
    }
}

impl Default for Body {
    fn default() -> Self {
        Self::empty()
    }
}

impl fmt::Debug for Body {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Kind::Bytes(bytes) => formatter.debug_tuple("Body").field(&bytes.len()).finish(),
            Kind::Stream(..) => formatter.write_str("Body(stream)"),
        }
    }
}

impl From<Bytes> for Body {
    fn from(bytes: Bytes) -> Self {
        Self(Kind::Bytes(bytes))
    }
}

impl From<Vec<u8>> for Body {
    fn from(bytes: Vec<u8>) -> Self {
        Self(Kind::Bytes(bytes.into()))
    }
}

impl From<String> for Body {
    fn from(text: String) -> Self {
        Self(Kind::Bytes(text.into()))
    }
}

impl From<&'static str> for Body {
    fn from(text: &'static str) -> Self {
        Self(Kind::Bytes(Bytes::from_static(text.as_bytes())))
    }
}

impl From<&'static [u8]> for Body {
    fn from(bytes: &'static [u8]) -> Self {
        Self(Kind::Bytes(Bytes::from_static(bytes)))
    }
}
