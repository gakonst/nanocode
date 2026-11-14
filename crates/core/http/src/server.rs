use std::{
    future::Future,
    io,
    pin::{Pin, pin},
    task::{Context, Poll},
    time::Duration,
};

use bytes::Bytes;
use http::{
    HeaderMap, HeaderValue, Method, StatusCode, Version,
    header::{CONTENT_LENGTH, DATE, TRANSFER_ENCODING, UPGRADE},
};
use nanocode_platform::timeout;
use nanocode_sync::{
    Stream, StreamExt,
    future::{Either, select},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::{mpsc, oneshot},
};

use crate::{
    client::BoxError,
    wire::{self, BodyReader, Connection, Framing, Io, ResponseHead, WireError},
};

const BODY_CHUNKS: usize = 16;

pub type BodyStream = Pin<Box<dyn Stream<Item = Result<Bytes, BoxError>> + Send>>;

pub struct Incoming(Kind);

enum Kind {
    Empty,
    Channel(mpsc::Receiver<Result<Bytes, WireError>>),
    Reader(Box<BodyReader>),
}

impl Incoming {
    #[must_use]
    pub const fn empty() -> Self {
        Self(Kind::Empty)
    }

    pub async fn chunk(&mut self) -> Result<Option<Bytes>, WireError> {
        match &mut self.0 {
            Kind::Empty => Ok(None),
            Kind::Channel(receiver) => receiver.recv().await.transpose(),
            Kind::Reader(reader) => reader.next().await,
        }
    }

    pub async fn collect(mut self) -> Result<Bytes, WireError> {
        let mut bytes = Vec::new();
        while let Some(chunk) = self.chunk().await? {
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes.into())
    }

    #[must_use]
    pub fn into_stream(self) -> BodyStream {
        nanocode_sync::stream::unfold(self, |mut body| async move {
            body.chunk()
                .await
                .map_err(BoxError::from)
                .transpose()
                .map(|chunk| (chunk, body))
        })
        .boxed()
    }

    #[must_use]
    pub fn into_upgraded(self) -> Option<Upgraded> {
        match self.0 {
            Kind::Reader(reader) => {
                let (io, unread) = reader.into_connection().into_parts();
                Some(Upgraded { io, unread })
            }
            Kind::Empty | Kind::Channel(_) => None,
        }
    }
}

pub struct Upgraded {
    io: Io,
    unread: Vec<u8>,
}

impl AsyncRead for Upgraded {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.unread.is_empty() {
            return Pin::new(&mut self.io).poll_read(context, buffer);
        }
        let count = self.unread.len().min(buffer.remaining());
        buffer.put_slice(&self.unread[..count]);
        self.unread.drain(..count);
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for Upgraded {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(context, bytes)
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(context)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(context)
    }
}

pub type OnUpgrade = oneshot::Receiver<Upgraded>;

pub struct Request {
    pub method: Method,
    pub target: String,
    pub version: Version,
    pub headers: HeaderMap,
    pub body: Incoming,
    pub upgrade: Option<OnUpgrade>,
}

pub enum Outgoing {
    Empty,
    Full(Bytes),
    Stream(BodyStream),
}

pub struct Reply {
    pub status: StatusCode,
    pub reason: Option<String>,
    pub headers: HeaderMap,
    pub body: Outgoing,
}

impl Reply {
    #[must_use]
    pub fn new(status: StatusCode, body: Outgoing) -> Self {
        Self {
            status,
            reason: None,
            headers: HeaderMap::new(),
            body,
        }
    }

    #[must_use]
    pub fn empty() -> Self {
        Self::new(StatusCode::OK, Outgoing::Empty)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    pub header_read_timeout: Option<Duration>,
}

fn upgrades(method: &Method, status: StatusCode) -> bool {
    status == StatusCode::SWITCHING_PROTOCOLS || (method == Method::CONNECT && status.is_success())
}

fn bodiless(status: StatusCode) -> bool {
    status.is_informational()
        || status == StatusCode::NO_CONTENT
        || status == StatusCode::NOT_MODIFIED
}

pub async fn serve<H, F>(io: Io, options: Options, mut handler: H) -> Result<(), WireError>
where
    H: FnMut(Request) -> F,
    F: Future<Output = Reply>,
{
    let mut connection = Connection::new(io);
    loop {
        let read = connection.request_head();
        let head = match options.header_read_timeout {
            Some(limit) => match timeout(limit, read).await {
                Ok(head) => head,
                Err(_) => return Ok(()),
            },
            None => read.await,
        };
        let head = match head {
            Ok(head) => head,
            Err(WireError::Closed { partial: false }) => return Ok(()),
            Err(error) => return Err(error),
        };
        let framing = wire::request_framing(&head.headers)?;
        let wants_upgrade = head.method == Method::CONNECT || head.headers.contains_key(UPGRADE);
        let (upgrade, on_upgrade) = if wants_upgrade {
            let (sender, receiver) = oneshot::channel();
            (Some(sender), Some(receiver))
        } else {
            (None, None)
        };
        let mut reader = BodyReader::new(connection, framing);
        let (sender, receiver) = mpsc::channel(BODY_CHUNKS);
        let body = if reader.is_done() {
            Incoming::empty()
        } else {
            Incoming(Kind::Channel(receiver))
        };
        let method = head.method.clone();
        let keeps_alive = wire::keeps_alive(head.version, &head.headers);
        let version = head.version;
        let reply = handler(Request {
            method: head.method,
            target: head.target,
            version: head.version,
            headers: head.headers,
            body,
            upgrade: on_upgrade,
        });
        let reply = pump(&mut reader, sender, reply).await;
        let read_all = reader.is_done();
        connection = reader.into_connection();
        let upgrading = upgrades(&method, reply.status);
        let keep =
            respond(&mut connection, &method, version, reply).await? && keeps_alive && read_all;
        if upgrading {
            if let Some(upgrade) = upgrade {
                let (io, unread) = connection.into_parts();
                drop(upgrade.send(Upgraded { io, unread }));
            }
            return Ok(());
        }
        if !keep {
            drop(connection.shutdown().await);
            return Ok(());
        }
    }
}

async fn pump<F: Future<Output = Reply>>(
    reader: &mut BodyReader,
    sender: mpsc::Sender<Result<Bytes, WireError>>,
    reply: F,
) -> Reply {
    let mut reply = pin!(reply);
    if reader.is_done() {
        drop(sender);
        return reply.await;
    }
    let forward = async move {
        loop {
            match reader.next().await {
                Ok(Some(chunk)) => {
                    if sender.send(Ok(chunk)).await.is_err() {
                        return;
                    }
                }
                Ok(None) => return,
                Err(error) => {
                    drop(sender.send(Err(error)).await);
                    return;
                }
            }
        }
    };
    match select(pin!(forward), reply.as_mut()).await {
        Either::Left(((), reply)) => reply.await,
        Either::Right((reply, _)) => reply,
    }
}

async fn respond(
    connection: &mut Connection,
    method: &Method,
    version: Version,
    reply: Reply,
) -> Result<bool, WireError> {
    let Reply {
        status,
        reason,
        mut headers,
        body,
    } = reply;
    if !headers.contains_key(DATE)
        && let Some(date) = nanocode_platform::format_http_date(std::time::SystemTime::now())
        && let Ok(value) = HeaderValue::from_str(&date)
    {
        headers.insert(DATE, value);
    }
    let no_body = bodiless(status) || upgrades(method, status) || method == Method::HEAD;
    let mut keep = true;
    let mut bounded = None;
    let chunked = match &body {
        _ if bodiless(status) || upgrades(method, status) => {
            headers.remove(CONTENT_LENGTH);
            headers.remove(TRANSFER_ENCODING);
            false
        }
        Outgoing::Empty => {
            headers
                .entry(CONTENT_LENGTH)
                .or_insert(HeaderValue::from_static("0"));
            false
        }
        Outgoing::Full(bytes) => {
            if !headers.contains_key(TRANSFER_ENCODING) {
                headers
                    .entry(CONTENT_LENGTH)
                    .or_insert_with(|| HeaderValue::from(bytes.len()));
            }
            headers.contains_key(TRANSFER_ENCODING)
        }
        Outgoing::Stream(_) if headers.contains_key(CONTENT_LENGTH) => {
            bounded = declared_length(&headers);
            keep = bounded.is_some();
            false
        }
        Outgoing::Stream(_) if version == Version::HTTP_11 => {
            headers.insert(TRANSFER_ENCODING, HeaderValue::from_static("chunked"));
            true
        }
        Outgoing::Stream(_) => {
            keep = false;
            false
        }
    };
    connection
        .write_all(&wire::response_head(status, reason.as_deref(), &headers))
        .await?;
    if !no_body {
        match body {
            Outgoing::Empty => {}
            Outgoing::Full(bytes) if chunked => {
                connection.write_all(&wire::chunk(&bytes)).await?;
                connection.write_all(wire::LAST_CHUNK).await?;
            }
            Outgoing::Full(bytes) => connection.write_all(&bytes).await?,
            Outgoing::Stream(mut stream) => {
                connection.flush().await?;
                while let Some(chunk) = stream.next().await {
                    let chunk = match chunk {
                        Ok(chunk) => chunk,
                        Err(_) => return Ok(false),
                    };
                    if chunk.is_empty() {
                        continue;
                    }
                    if chunked {
                        connection.write_all(&wire::chunk(&chunk)).await?;
                    } else if let Some(left) = bounded {
                        let take =
                            usize::try_from(left).map_or(chunk.len(), |left| left.min(chunk.len()));
                        connection.write_all(&chunk[..take]).await?;
                        bounded =
                            Some(left - u64::try_from(take).expect("a chunk length fits in u64"));
                        if take < chunk.len() {
                            connection.flush().await?;
                            return Ok(false);
                        }
                    } else {
                        connection.write_all(&chunk).await?;
                    }
                    connection.flush().await?;
                }
                if chunked {
                    connection.write_all(wire::LAST_CHUNK).await?;
                }
                if bounded.is_some_and(|left| left > 0) {
                    connection.flush().await?;
                    return Ok(false);
                }
            }
        }
    }
    connection.flush().await?;
    Ok(keep)
}

pub struct Exchanged {
    pub head: ResponseHead,
    pub body: Incoming,
}

pub async fn send(
    io: Io,
    method: &Method,
    target: &str,
    mut headers: HeaderMap,
    mut body: Incoming,
    length: Option<u64>,
) -> Result<Exchanged, WireError> {
    let mut connection = Connection::new(io);
    let chunked = match (&body.0, length) {
        (Kind::Empty, _) => false,
        (_, Some(length)) => {
            headers.insert(CONTENT_LENGTH, HeaderValue::from(length));
            false
        }
        (_, None) => {
            headers.insert(TRANSFER_ENCODING, HeaderValue::from_static("chunked"));
            true
        }
    };
    connection
        .write_all(&wire::request_head(method, target, &headers))
        .await?;
    let written = write_body(&mut connection, &mut body, chunked).await?;
    let head = match final_head(&mut connection).await {
        Ok(head) => head,
        Err(error) => return Err(written.err().map_or(error, WireError::from)),
    };
    let framing = if head.status == StatusCode::SWITCHING_PROTOCOLS {
        Framing::Close
    } else {
        wire::response_framing(method, head.status, &head.headers)?
    };
    let reader = BodyReader::new(connection, framing);
    Ok(Exchanged {
        head,
        body: Incoming(Kind::Reader(Box::new(reader))),
    })
}

async fn write_body(
    connection: &mut Connection,
    body: &mut Incoming,
    chunked: bool,
) -> Result<io::Result<()>, WireError> {
    while let Some(chunk) = body.chunk().await? {
        let written = if chunked {
            connection.write_all(&wire::chunk(&chunk)).await
        } else {
            connection.write_all(&chunk).await
        };
        if written.is_err() {
            return Ok(written);
        }
    }
    if chunked && let Err(error) = connection.write_all(wire::LAST_CHUNK).await {
        return Ok(Err(error));
    }
    Ok(connection.flush().await)
}

async fn final_head(connection: &mut Connection) -> Result<ResponseHead, WireError> {
    loop {
        let head = connection.response_head().await?;
        if !head.status.is_informational() || head.status == StatusCode::SWITCHING_PROTOCOLS {
            return Ok(head);
        }
    }
}

fn declared_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
}

#[must_use]
pub fn length(headers: &HeaderMap) -> Option<u64> {
    match wire::request_framing(headers) {
        Ok(Framing::Length(length)) => Some(length),
        _ => None,
    }
}
