use std::{error::Error as StdError, fmt, io};

use http::StatusCode;
use nanocode_url::Url;

use super::body::BoxError;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Builder,
    Request,
    Redirect,
    Status(StatusCode),
    Body,
    Decode,
}

pub struct Error {
    inner: Box<Inner>,
}

struct Inner {
    kind: Kind,
    connect: bool,
    url: Option<Url>,
    source: Option<BoxError>,
}

#[derive(Debug)]
pub struct TimedOut;

impl fmt::Display for TimedOut {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("operation timed out")
    }
}

impl StdError for TimedOut {}

impl Error {
    fn new(kind: Kind, source: Option<BoxError>) -> Self {
        Self {
            inner: Box::new(Inner {
                kind,
                connect: false,
                url: None,
                source,
            }),
        }
    }

    pub(crate) fn builder(source: impl Into<BoxError>) -> Self {
        Self::new(Kind::Builder, Some(source.into()))
    }

    pub(crate) fn request(source: impl Into<BoxError>) -> Self {
        Self::new(Kind::Request, Some(source.into()))
    }

    pub(crate) fn connect(source: impl Into<BoxError>) -> Self {
        let mut error = Self::new(Kind::Request, Some(source.into()));
        error.inner.connect = true;
        error
    }

    pub(crate) fn redirect(source: impl Into<BoxError>) -> Self {
        Self::new(Kind::Redirect, Some(source.into()))
    }

    pub(crate) fn body(source: impl Into<BoxError>) -> Self {
        Self::new(Kind::Body, Some(source.into()))
    }

    pub(crate) fn decode(source: impl Into<BoxError>) -> Self {
        Self::new(Kind::Decode, Some(source.into()))
    }

    pub(crate) fn for_status(status: StatusCode, url: Url) -> Self {
        Self::new(Kind::Status(status), None).with_url(url)
    }

    pub(crate) fn timed_out() -> Self {
        Self::request(TimedOut)
    }

    #[must_use]
    pub fn with_url(mut self, url: Url) -> Self {
        self.inner.url = Some(url);
        self
    }

    #[must_use]
    pub fn without_url(mut self) -> Self {
        self.inner.url = None;
        self
    }

    #[must_use]
    pub fn url(&self) -> Option<&Url> {
        self.inner.url.as_ref()
    }

    pub fn url_mut(&mut self) -> Option<&mut Url> {
        self.inner.url.as_mut()
    }

    #[must_use]
    pub fn is_builder(&self) -> bool {
        self.inner.kind == Kind::Builder
    }

    #[must_use]
    pub fn is_redirect(&self) -> bool {
        self.inner.kind == Kind::Redirect
    }

    #[must_use]
    pub fn is_status(&self) -> bool {
        matches!(self.inner.kind, Kind::Status(_))
    }

    #[must_use]
    pub fn is_request(&self) -> bool {
        self.inner.kind == Kind::Request
    }

    #[must_use]
    pub fn is_connect(&self) -> bool {
        self.inner.connect
    }

    #[must_use]
    pub fn is_body(&self) -> bool {
        self.inner.kind == Kind::Body
    }

    #[must_use]
    pub fn is_decode(&self) -> bool {
        self.inner.kind == Kind::Decode
    }

    #[must_use]
    pub fn is_timeout(&self) -> bool {
        let mut source = self.source();
        while let Some(error) = source {
            if error.is::<TimedOut>() {
                return true;
            }
            if let Some(io) = error.downcast_ref::<io::Error>()
                && io.kind() == io::ErrorKind::TimedOut
            {
                return true;
            }
            source = error.source();
        }
        false
    }

    #[must_use]
    pub fn status(&self) -> Option<StatusCode> {
        match self.inner.kind {
            Kind::Status(status) => Some(status),
            _ => None,
        }
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct("nanocode_http::Error");
        debug.field("kind", &self.inner.kind);
        if let Some(url) = &self.inner.url {
            debug.field("url", &url.as_str());
        }
        if let Some(source) = &self.inner.source {
            debug.field("source", source);
        }
        debug.finish()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.inner.kind {
            Kind::Builder => formatter.write_str("builder error")?,
            Kind::Request => formatter.write_str("error sending request")?,
            Kind::Body => formatter.write_str("request or response body error")?,
            Kind::Decode => formatter.write_str("error decoding response body")?,
            Kind::Redirect => formatter.write_str("error following redirect")?,
            Kind::Status(status) => {
                let prefix = if status.is_client_error() {
                    "HTTP status client error"
                } else {
                    "HTTP status server error"
                };
                write!(formatter, "{prefix} ({status})")?;
            }
        }
        if let Some(url) = &self.inner.url {
            write!(formatter, " for url ({url})")?;
        }
        Ok(())
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.inner
            .source
            .as_ref()
            .map(|source| &**source as &(dyn StdError + 'static))
    }
}
