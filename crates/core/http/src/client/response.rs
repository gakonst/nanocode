use std::{collections::VecDeque, fmt, pin::Pin, sync::Arc, time::Duration};

use bytes::Bytes;
use futures_util::{FutureExt, Stream, StreamExt, stream::BoxStream};
use http::{HeaderMap, StatusCode, Version, header::CONTENT_LENGTH};
use serde::de::DeserializeOwned;
use tokio::time::{Instant, timeout_at};
use url::Url;

use super::{
    Error, Result,
    pool::{Key, Pool},
};
use crate::wire::{BodyReader, WireError};

pub type ResponseStream = Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>;

pub(crate) struct Incoming {
    pub(crate) reader: BodyReader,
    pub(crate) reuse: Option<(Arc<Pool>, Key)>,
    pub(crate) deadline: Option<Instant>,
    pub(crate) read_timeout: Option<Duration>,
    pub(crate) buffered: VecDeque<Bytes>,
    pub(crate) failed: Option<WireError>,
}

impl Incoming {
    fn settle(&mut self) {
        while self.failed.is_none() && !self.reader.is_done() && self.reader.has_unread() {
            match self.reader.next().now_or_never() {
                Some(Ok(Some(chunk))) => self.buffered.push_back(chunk),
                Some(Ok(None)) | None => return,
                Some(Err(error)) => self.failed = Some(error),
            }
        }
    }

    fn read_deadline(&self) -> Option<Instant> {
        let read = self.read_timeout.map(|timeout| Instant::now() + timeout);
        match (self.deadline, read) {
            (Some(deadline), Some(read)) => Some(deadline.min(read)),
            (deadline, read) => deadline.or(read),
        }
    }

    async fn next(&mut self) -> Result<Option<Bytes>> {
        if let Some(chunk) = self.buffered.pop_front() {
            return Ok(Some(chunk));
        }
        if let Some(error) = self.failed.take() {
            return Err(Error::body(error));
        }
        let deadline = self.read_deadline();
        let read = self.reader.next();
        let chunk = match deadline {
            Some(deadline) => timeout_at(deadline, read)
                .await
                .map_err(|_| Error::body(super::error::TimedOut))?,
            None => read.await,
        }
        .map_err(Error::body)?;
        Ok(chunk)
    }

    fn finish(self) {
        assert!(self.reader.is_done());
        if let Some((pool, key)) = self.reuse {
            pool.checkin(key, self.reader.into_connection());
        }
    }
}

enum Body {
    Incoming(Box<Incoming>),
    Buffered(VecDeque<Bytes>),
    Stream(ResponseStream),
    Done,
}

pub struct Response {
    status: StatusCode,
    version: Version,
    headers: HeaderMap,
    url: Url,
    body: Body,
    extensions: http::Extensions,
}

impl Response {
    pub(crate) fn incoming(
        status: StatusCode,
        version: Version,
        headers: HeaderMap,
        url: Url,
        mut incoming: Incoming,
    ) -> Self {
        if incoming.reuse.is_some() {
            incoming.settle();
        }
        let body = if incoming.reader.is_done() && incoming.buffered.is_empty() {
            incoming.finish();
            Body::Done
        } else if incoming.reader.is_done() && incoming.failed.is_none() {
            let buffered = std::mem::take(&mut incoming.buffered);
            incoming.finish();
            Body::Buffered(buffered)
        } else {
            Body::Incoming(Box::new(incoming))
        };
        Self {
            status,
            version,
            headers,
            url,
            body,
            extensions: http::Extensions::new(),
        }
    }

    pub fn from_stream<S>(status: StatusCode, headers: HeaderMap, url: Url, body: S) -> Self
    where
        S: Stream<Item = Result<Bytes>> + Send + 'static,
    {
        Self {
            status,
            version: Version::HTTP_11,
            headers,
            url,
            body: Body::Stream(Box::pin(body)),
            extensions: http::Extensions::new(),
        }
    }

    #[must_use]
    pub const fn status(&self) -> StatusCode {
        self.status
    }

    pub const fn status_mut(&mut self) -> &mut StatusCode {
        &mut self.status
    }

    #[must_use]
    pub const fn version(&self) -> Version {
        self.version
    }

    #[must_use]
    pub const fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    pub const fn headers_mut(&mut self) -> &mut HeaderMap {
        &mut self.headers
    }

    #[must_use]
    pub const fn url(&self) -> &Url {
        &self.url
    }

    #[must_use]
    pub const fn extensions(&self) -> &http::Extensions {
        &self.extensions
    }

    pub const fn extensions_mut(&mut self) -> &mut http::Extensions {
        &mut self.extensions
    }

    #[must_use]
    pub fn content_length(&self) -> Option<u64> {
        match &self.body {
            Body::Done => Some(0),
            Body::Stream(_) => None,
            Body::Incoming(_) | Body::Buffered(_) => self
                .headers
                .get(CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse().ok()),
        }
    }

    pub async fn chunk(&mut self) -> Result<Option<Bytes>> {
        loop {
            let chunk = match &mut self.body {
                Body::Done => return Ok(None),
                Body::Incoming(incoming) => incoming.next().await,
                Body::Buffered(buffered) => Ok(buffered.pop_front()),
                Body::Stream(stream) => stream.next().await.transpose(),
            };
            match chunk {
                Ok(Some(chunk)) if chunk.is_empty() => {}
                Ok(Some(chunk)) => return Ok(Some(chunk)),
                Ok(None) => {
                    if let Body::Incoming(incoming) = std::mem::replace(&mut self.body, Body::Done)
                    {
                        incoming.finish();
                    }
                    return Ok(None);
                }
                Err(error) => {
                    self.body = Body::Done;
                    return Err(error);
                }
            }
        }
    }

    pub async fn bytes(mut self) -> Result<Bytes> {
        let Some(first) = self.chunk().await? else {
            return Ok(Bytes::new());
        };
        let Some(second) = self.chunk().await? else {
            return Ok(first);
        };
        let mut bytes = Vec::with_capacity(first.len() + second.len());
        bytes.extend_from_slice(&first);
        bytes.extend_from_slice(&second);
        while let Some(chunk) = self.chunk().await? {
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes.into())
    }

    pub async fn text(self) -> Result<String> {
        let bytes = self.bytes().await?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    pub async fn json<T: DeserializeOwned>(self) -> Result<T> {
        let bytes = self.bytes().await?;
        serde_json::from_slice(&bytes).map_err(Error::decode)
    }

    #[must_use]
    pub fn bytes_stream(self) -> BoxStream<'static, Result<Bytes>> {
        futures_util::stream::unfold(self, |mut response| async move {
            response
                .chunk()
                .await
                .transpose()
                .map(|chunk| (chunk, response))
        })
        .boxed()
    }

    pub fn error_for_status(self) -> Result<Self> {
        if self.status.is_client_error() || self.status.is_server_error() {
            Err(Error::for_status(self.status, self.url))
        } else {
            Ok(self)
        }
    }

    pub fn error_for_status_ref(&self) -> Result<&Self> {
        if self.status.is_client_error() || self.status.is_server_error() {
            Err(Error::for_status(self.status, self.url.clone()))
        } else {
            Ok(self)
        }
    }

    #[must_use]
    pub fn map_body(
        self,
        body: impl FnOnce(BoxStream<'static, Result<Bytes>>) -> ResponseStream,
    ) -> Self {
        let status = self.status;
        let version = self.version;
        let headers = self.headers.clone();
        let url = self.url.clone();
        let extensions = self.extensions.clone();
        let mut stripped = self;
        stripped.extensions = http::Extensions::new();
        Self {
            status,
            version,
            headers,
            url,
            body: Body::Stream(body(stripped.bytes_stream())),
            extensions,
        }
    }
}

impl fmt::Debug for Response {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Response")
            .field("url", &self.url.as_str())
            .field("status", &self.status)
            .field("headers", &self.headers)
            .finish()
    }
}
