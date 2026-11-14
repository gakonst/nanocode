pub mod blocking;
mod body;
mod error;
mod pool;
pub mod redirect;
mod request;
mod response;

use std::{fmt, future::Future, net::SocketAddr, pin::Pin, sync::Arc, time::Duration};

use bytes::Bytes;
use futures_util::StreamExt;
use http::{
    HeaderMap, HeaderValue, Method, StatusCode, Version,
    header::{
        CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, Entry, HOST, LOCATION, PROXY_AUTHORIZATION,
        REFERER, TRANSFER_ENCODING, USER_AGENT,
    },
};
use nanocode_platform::{Instant, now, timeout};
use nanocode_url::Url;

pub use body::{Body, BoxError, ChunkStream};
pub use error::{Error, Result, TimedOut};
use pool::{Key, Pool};
use redirect::ActionKind;
pub use request::{IntoUrl, Request, RequestBuilder, replace_headers};
use response::Incoming;
pub use response::{Response, ResponseStream};

use crate::{
    connect::{self, Alpn, Proxy, ProxyScheme},
    wire::{self, BodyReader, Connection, Framing, WireError},
};

pub type Pending = Pin<Box<dyn Future<Output = Result<Response>> + Send>>;

pub type Connecting = Pin<Box<dyn Future<Output = std::io::Result<wire::Io>> + Send>>;

pub trait Connect: Send + Sync + 'static {
    fn connect(&self, host: &str, port: u16, secure: bool) -> Connecting;
}

async fn timeout_at<F: Future>(
    deadline: Instant,
    future: F,
) -> std::result::Result<F::Output, nanocode_platform::Elapsed> {
    timeout(deadline.saturating_duration_since(now()), future).await
}

const IDLE_TIMEOUT: Duration = Duration::from_secs(90);
const ATTEMPTS_MAX: usize = 2;

#[derive(Debug)]
struct BadScheme;

impl fmt::Display for BadScheme {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("URL scheme is not allowed")
    }
}

impl std::error::Error for BadScheme {}

#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

struct Inner {
    headers: HeaderMap,
    redirect: redirect::Policy,
    referer: bool,
    timeout: Option<Duration>,
    connect_timeout: Option<Duration>,
    read_timeout: Option<Duration>,
    proxies: bool,
    https_only: bool,
    resolved: Vec<(String, Vec<SocketAddr>)>,
    connector: Option<Arc<dyn Connect>>,
    pool: Arc<Pool>,
}

#[must_use]
pub struct ClientBuilder {
    headers: HeaderMap,
    redirect: redirect::Policy,
    referer: bool,
    timeout: Option<Duration>,
    connect_timeout: Option<Duration>,
    read_timeout: Option<Duration>,
    proxies: bool,
    https_only: bool,
    resolved: Vec<(String, Vec<SocketAddr>)>,
    idle_per_host_max: usize,
    idle_timeout: Option<Duration>,
    connector: Option<Arc<dyn Connect>>,
    error: Option<Error>,
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientBuilder {
    pub fn new() -> Self {
        let mut headers = HeaderMap::new();
        headers.insert(http::header::ACCEPT, HeaderValue::from_static("*/*"));
        Self {
            headers,
            redirect: redirect::Policy::default(),
            referer: true,
            timeout: None,
            connect_timeout: None,
            read_timeout: None,
            proxies: true,
            https_only: false,
            resolved: Vec::new(),
            idle_per_host_max: usize::MAX,
            idle_timeout: Some(IDLE_TIMEOUT),
            connector: None,
            error: None,
        }
    }

    pub fn build(self) -> Result<Client> {
        if let Some(error) = self.error {
            return Err(error);
        }
        Ok(Client {
            inner: Arc::new(Inner {
                headers: self.headers,
                redirect: self.redirect,
                referer: self.referer,
                timeout: self.timeout,
                connect_timeout: self.connect_timeout,
                read_timeout: self.read_timeout,
                proxies: self.proxies,
                https_only: self.https_only,
                resolved: self.resolved,
                connector: self.connector,
                pool: Arc::new(Pool::new(self.idle_per_host_max, self.idle_timeout)),
            }),
        })
    }

    pub fn default_headers(mut self, headers: HeaderMap) -> Self {
        for (key, value) in &headers {
            self.headers.insert(key, value.clone());
        }
        self
    }

    pub fn user_agent<V>(mut self, value: V) -> Self
    where
        HeaderValue: TryFrom<V>,
        <HeaderValue as TryFrom<V>>::Error: Into<http::Error>,
    {
        match HeaderValue::try_from(value) {
            Ok(value) => {
                self.headers.insert(USER_AGENT, value);
            }
            Err(error) => self.error = Some(Error::builder(error.into())),
        }
        self
    }

    pub const fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub const fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = Some(timeout);
        self
    }

    pub const fn read_timeout(mut self, timeout: Duration) -> Self {
        self.read_timeout = Some(timeout);
        self
    }

    pub fn redirect(mut self, policy: redirect::Policy) -> Self {
        self.redirect = policy;
        self
    }

    pub const fn referer(mut self, enable: bool) -> Self {
        self.referer = enable;
        self
    }

    pub const fn https_only(mut self, enabled: bool) -> Self {
        self.https_only = enabled;
        self
    }

    pub const fn no_proxy(mut self) -> Self {
        self.proxies = false;
        self
    }

    pub const fn pool_max_idle_per_host(mut self, max: usize) -> Self {
        self.idle_per_host_max = max;
        self
    }

    pub fn pool_idle_timeout<D: Into<Option<Duration>>>(mut self, timeout: D) -> Self {
        self.idle_timeout = timeout.into();
        self
    }

    pub fn connector(mut self, connector: impl Connect) -> Self {
        self.connector = Some(Arc::new(connector));
        self
    }

    pub fn resolve(self, domain: &str, address: SocketAddr) -> Self {
        self.resolve_to_addrs(domain, &[address])
    }

    pub fn resolve_to_addrs(mut self, domain: &str, addresses: &[SocketAddr]) -> Self {
        self.resolved
            .push((domain.to_ascii_lowercase(), addresses.to_vec()));
        self
    }
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Client {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Client")
            .field("default_headers", &self.inner.headers)
            .field("redirect", &self.inner.redirect)
            .field("timeout", &self.inner.timeout)
            .finish_non_exhaustive()
    }
}

struct Outgoing {
    started: Url,
    method: Method,
    url: Url,
    headers: HeaderMap,
    deadline: Option<Instant>,
}

impl Client {
    #[must_use]
    pub fn new() -> Self {
        ClientBuilder::new()
            .build()
            .expect("the default HTTP client builds")
    }

    pub fn builder() -> ClientBuilder {
        ClientBuilder::new()
    }

    pub fn get<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::GET, url)
    }

    pub fn post<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::POST, url)
    }

    pub fn put<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::PUT, url)
    }

    pub fn patch<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::PATCH, url)
    }

    pub fn delete<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::DELETE, url)
    }

    pub fn head<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::HEAD, url)
    }

    pub fn request<U: IntoUrl>(&self, method: Method, url: U) -> RequestBuilder {
        let request = url.into_url().map(|url| Request::new(method, url));
        RequestBuilder::new(self.clone(), request)
    }

    pub fn execute(&self, request: Request) -> Pending {
        let client = self.clone();
        Box::pin(async move { client.run(request).await })
    }

    pub fn preconnect<U: IntoUrl>(
        &self,
        url: U,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> {
        let client = self.clone();
        let url = url.into_url();
        Box::pin(async move {
            let (key, proxy) = client.destination(&url?)?;
            let connection = match client.inner.pool.checkout(&key) {
                Some(connection) => connection,
                None => client.connect(&key, proxy.as_ref()).await?,
            };
            client.inner.pool.checkin(key, connection);
            Ok(())
        })
    }

    fn destination(&self, url: &Url) -> Result<(Key, Option<Proxy>)> {
        let secure = match url.scheme() {
            "https" => true,
            "http" if !self.inner.https_only => false,
            _ => return Err(Error::builder(BadScheme)),
        };
        let host = url
            .host_str()
            .ok_or_else(|| Error::builder("URL has no host"))?
            .to_owned();
        let port = url
            .port_or_known_default()
            .ok_or_else(|| Error::builder("URL has no port"))?;
        let proxy = if self.inner.proxies {
            Proxy::from_env(&host, port, secure).map_err(Error::builder)?
        } else {
            None
        };
        let key = Key {
            secure,
            host,
            port,
            proxy: proxy.as_ref().map(Proxy::authority),
        };
        Ok((key, proxy))
    }

    async fn run(&self, request: Request) -> Result<Response> {
        let (method, url, mut headers, body, timeout) = request.into_parts();
        for (key, value) in &self.inner.headers {
            if let Entry::Vacant(entry) = headers.entry(key) {
                entry.insert(value.clone());
            }
        }
        let deadline = timeout
            .or(self.inner.timeout)
            .map(|timeout| now() + timeout);
        let started = url.clone();
        let outgoing = Outgoing {
            started: started.clone(),
            method,
            url,
            headers,
            deadline,
        };
        let follow = self.follow(outgoing, body.unwrap_or_default());
        match deadline {
            Some(deadline) => timeout_at(deadline, follow)
                .await
                .unwrap_or_else(|_| Err(Error::timed_out().with_url(started))),
            None => follow.await,
        }
    }

    async fn follow(&self, mut outgoing: Outgoing, mut body: Body) -> Result<Response> {
        let mut previous: Vec<Url> = Vec::new();
        let mut headers = outgoing.headers.clone();
        loop {
            let replay = body.try_clone();
            let response = self
                .send(&outgoing, headers, body)
                .await
                .map_err(|error| error.with_url(outgoing.started.clone()))?;
            if !self.inner.redirect.follows() {
                return Ok(response);
            }
            let status = response.status();
            let next = match status {
                StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND
                    if outgoing.method == Method::POST =>
                {
                    outgoing.method = Method::GET;
                    drop_payload_headers(&mut outgoing.headers);
                    Some(Body::empty())
                }
                StatusCode::SEE_OTHER => {
                    if outgoing.method != Method::HEAD {
                        outgoing.method = Method::GET;
                    }
                    drop_payload_headers(&mut outgoing.headers);
                    Some(Body::empty())
                }
                StatusCode::MOVED_PERMANENTLY
                | StatusCode::FOUND
                | StatusCode::TEMPORARY_REDIRECT
                | StatusCode::PERMANENT_REDIRECT => replay,
                _ => return Ok(response),
            };
            let Some(next) = next else {
                return Ok(response);
            };
            let Some(mut location) = response
                .headers()
                .get(LOCATION)
                .and_then(|location| location.to_str().ok())
                .and_then(|location| outgoing.url.join(location).ok())
            else {
                return Ok(response);
            };
            location.set_fragment(None);
            previous.push(outgoing.url.clone());
            match self.inner.redirect.check(status, &location, &previous) {
                ActionKind::Follow => {}
                ActionKind::Stop => return Ok(response),
                ActionKind::Error(error) => {
                    return Err(Error::redirect(error).with_url(outgoing.url));
                }
            }
            if location.scheme() != "http" && location.scheme() != "https" {
                return Err(Error::builder(BadScheme).with_url(location));
            }
            if self.inner.https_only && location.scheme() != "https" {
                return Err(Error::redirect(BadScheme).with_url(location));
            }
            headers = outgoing.headers.clone();
            remove_sensitive_headers(&mut headers, &location, &previous);
            if self.inner.referer
                && let Some(referer) = make_referer(&location, &outgoing.url)
            {
                headers.insert(REFERER, referer);
            }
            outgoing.url = location;
            body = next;
        }
    }

    async fn send(
        &self,
        outgoing: &Outgoing,
        mut headers: HeaderMap,
        body: Body,
    ) -> Result<Response> {
        let url = &outgoing.url;
        let (key, proxy) = self.destination(url)?;
        let forwards = !key.secure
            && proxy.as_ref().is_some_and(|proxy| {
                matches!(proxy.scheme, ProxyScheme::Http | ProxyScheme::Https)
            });
        if forwards
            && let Some(auth) = proxy.as_ref().and_then(|proxy| proxy.auth.as_ref())
            && !headers.contains_key(PROXY_AUTHORIZATION)
        {
            let mut value = HeaderValue::from_str(&auth.basic()).map_err(Error::builder)?;
            value.set_sensitive(true);
            headers.insert(PROXY_AUTHORIZATION, value);
        }
        let target = if forwards {
            absolute_form(url, &key.host)
        } else {
            origin_form(url)
        };
        if let Entry::Vacant(entry) = headers.entry(HOST) {
            let value = match url.port() {
                Some(port) => format!("{}:{port}", key.host),
                None => key.host.clone(),
            };
            entry.insert(HeaderValue::from_str(&value).map_err(Error::builder)?);
        }
        let mut body = Payload::new(body, &outgoing.method, &mut headers)?;
        let head = wire::request_head(&outgoing.method, &target, &headers);
        let reusable = wire::keeps_alive(Version::HTTP_11, &headers);
        let idempotent = matches!(
            outgoing.method,
            Method::GET
                | Method::HEAD
                | Method::OPTIONS
                | Method::TRACE
                | Method::PUT
                | Method::DELETE
        );
        for attempt in 1..=ATTEMPTS_MAX {
            let pooled = if attempt == 1 {
                self.inner.pool.checkout(&key)
            } else {
                None
            };
            let reused = pooled.is_some();
            let mut connection = match pooled {
                Some(connection) => connection,
                None => self.connect(&key, proxy.as_ref()).await?,
            };
            let retry = attempt < ATTEMPTS_MAX && reused && body.replayable();
            let written = match body.write(&mut connection, &head).await {
                Ok(()) => None,
                Err(error) if retry && !error.is_body() => continue,
                Err(error) if error.is_body() => return Err(error),
                Err(error) => Some(error),
            };
            let head = match (
                self.response_head(&mut connection, outgoing.deadline).await,
                written,
            ) {
                (Ok(head), _) => head,
                (Err(_), Some(error)) => return Err(error),
                (Err(WireError::Closed { partial: false }), None) if retry && idempotent => {
                    continue;
                }
                (Err(error), None) => return Err(Error::request(error)),
            };
            let framing = wire::response_framing(&outgoing.method, head.status, &head.headers)
                .map_err(Error::request)?;
            let upgraded = head.status == StatusCode::SWITCHING_PROTOCOLS
                || (outgoing.method == Method::CONNECT && head.status.is_success());
            let keep = reusable
                && !upgraded
                && framing != Framing::Close
                && wire::keeps_alive(head.version, &head.headers);
            let incoming = Incoming {
                reader: BodyReader::new(connection, framing),
                reuse: keep.then(|| (Arc::clone(&self.inner.pool), key.clone())),
                deadline: outgoing.deadline,
                read_timeout: self.inner.read_timeout,
                buffered: std::collections::VecDeque::new(),
                failed: None,
            };
            return Ok(Response::incoming(
                head.status,
                head.version,
                head.headers,
                url.clone(),
                incoming,
            ));
        }
        unreachable!("the last attempt returns")
    }

    async fn connect(&self, key: &Key, proxy: Option<&Proxy>) -> Result<Connection> {
        let resolved: Vec<SocketAddr> = self
            .inner
            .resolved
            .iter()
            .find(|(domain, _)| domain.eq_ignore_ascii_case(&key.host))
            .map(|(_, addresses)| {
                addresses
                    .iter()
                    .map(|address| {
                        if address.port() == 0 {
                            SocketAddr::new(address.ip(), key.port)
                        } else {
                            *address
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let user_agent = self
            .inner
            .headers
            .get(USER_AGENT)
            .and_then(|value| value.to_str().ok());
        let connecting: Pin<Box<dyn Future<Output = std::io::Result<wire::Io>> + Send + '_>> =
            match &self.inner.connector {
                Some(connector) => connector.connect(&key.host, key.port, key.secure),
                None => Box::pin(connect::stream(
                    &key.host,
                    key.port,
                    key.secure,
                    proxy,
                    &resolved,
                    user_agent,
                    Alpn::Http1,
                )),
            };
        let stream = match self.inner.connect_timeout {
            Some(limit) => timeout(limit, connecting)
                .await
                .map_err(|_| Error::connect(TimedOut))?,
            None => connecting.await,
        }
        .map_err(Error::connect)?;
        Ok(Connection::new(stream))
    }

    async fn response_head(
        &self,
        connection: &mut Connection,
        deadline: Option<Instant>,
    ) -> std::result::Result<wire::ResponseHead, WireError> {
        loop {
            let read = connection.response_head();
            let head = match self.inner.read_timeout {
                Some(limit) => {
                    let limit = now() + limit;
                    let limit = deadline.map_or(limit, |deadline| deadline.min(limit));
                    timeout_at(limit, read).await.map_err(|_| {
                        WireError::Io(std::io::Error::new(std::io::ErrorKind::TimedOut, TimedOut))
                    })?
                }
                None => read.await,
            }?;
            if head.status.is_informational() && head.status != StatusCode::SWITCHING_PROTOCOLS {
                continue;
            }
            return Ok(head);
        }
    }
}

enum Encoding {
    Length,
    Chunked,
    Skip,
}

struct Payload {
    body: std::result::Result<Bytes, (ChunkStream, Option<u64>)>,
    encoding: Encoding,
    declared: Option<u64>,
    sent: bool,
}

impl Payload {
    fn new(body: Body, method: &Method, headers: &mut HeaderMap) -> Result<Self> {
        let body = body.into_parts();
        let declared = headers
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok());
        let chunked = headers.contains_key(TRANSFER_ENCODING);
        let encoding = match &body {
            Ok(bytes) if bytes.is_empty() => {
                headers.remove(TRANSFER_ENCODING);
                Encoding::Skip
            }
            _ if chunked => Encoding::Chunked,
            _ if declared.is_some() => Encoding::Length,
            Ok(bytes) => {
                headers.insert(CONTENT_LENGTH, HeaderValue::from(bytes.len()));
                Encoding::Length
            }
            Err((_, Some(length))) => {
                headers.insert(CONTENT_LENGTH, HeaderValue::from(*length));
                Encoding::Length
            }
            Err(_) if matches!(*method, Method::GET | Method::HEAD | Method::CONNECT) => {
                Encoding::Skip
            }
            Err(_) => {
                headers.insert(TRANSFER_ENCODING, HeaderValue::from_static("chunked"));
                Encoding::Chunked
            }
        };
        let declared = match (&encoding, &body) {
            (Encoding::Length, Ok(bytes)) => Some(declared.unwrap_or(bytes.len() as u64)),
            (Encoding::Length, Err((_, length))) => declared.or(*length),
            _ => None,
        };
        Ok(Self {
            body,
            encoding,
            declared,
            sent: false,
        })
    }

    const fn replayable(&self) -> bool {
        self.body.is_ok()
    }

    async fn write(&mut self, connection: &mut Connection, head: &[u8]) -> Result<()> {
        assert!(!self.sent || self.replayable());
        self.sent = true;
        let written = match (&mut self.body, &self.encoding) {
            (_, Encoding::Skip) => connection.write_all(head).await,
            (Ok(bytes), Encoding::Length) => {
                if self.declared != Some(bytes.len() as u64) {
                    return Err(Error::body("body length differs from content-length"));
                }
                let mut message = Vec::with_capacity(head.len() + bytes.len());
                message.extend_from_slice(head);
                message.extend_from_slice(bytes);
                connection.write_all(&message).await
            }
            (Ok(bytes), Encoding::Chunked) => {
                let mut message = head.to_vec();
                message.extend_from_slice(&wire::chunk(bytes));
                message.extend_from_slice(wire::LAST_CHUNK);
                connection.write_all(&message).await
            }
            (Err((stream, _)), encoding) => {
                connection.write_all(head).await.map_err(Error::request)?;
                let chunked = matches!(encoding, Encoding::Chunked);
                let mut total = 0u64;
                while let Some(chunk) = stream.next().await {
                    let chunk = chunk.map_err(Error::body)?;
                    if chunk.is_empty() {
                        continue;
                    }
                    total += chunk.len() as u64;
                    if chunked {
                        connection.write_all(&wire::chunk(&chunk)).await
                    } else {
                        connection.write_all(&chunk).await
                    }
                    .map_err(Error::request)?;
                }
                if !chunked && self.declared != Some(total) {
                    return Err(Error::body("body length differs from content-length"));
                }
                if chunked {
                    connection.write_all(wire::LAST_CHUNK).await
                } else {
                    Ok(())
                }
            }
        };
        written.map_err(Error::request)?;
        connection.flush().await.map_err(Error::request)
    }
}

fn origin_form(url: &Url) -> String {
    let path = if url.path().is_empty() {
        "/"
    } else {
        url.path()
    };
    match url.query() {
        Some(query) => format!("{path}?{query}"),
        None => path.to_owned(),
    }
}

fn absolute_form(url: &Url, host: &str) -> String {
    let authority = match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    };
    format!("{}://{authority}{}", url.scheme(), origin_form(url))
}

fn drop_payload_headers(headers: &mut HeaderMap) {
    for name in [
        CONTENT_TYPE,
        CONTENT_LENGTH,
        CONTENT_ENCODING,
        TRANSFER_ENCODING,
    ] {
        headers.remove(name);
    }
}

fn remove_sensitive_headers(headers: &mut HeaderMap, next: &Url, previous: &[Url]) {
    let Some(previous) = previous.last() else {
        return;
    };
    let cross_host = next.host_str() != previous.host_str()
        || next.port_or_known_default() != previous.port_or_known_default()
        || next.scheme() != previous.scheme();
    if cross_host {
        for name in [
            "authorization",
            "cookie",
            "cookie2",
            "proxy-authorization",
            "www-authenticate",
        ] {
            headers.remove(name);
        }
    }
}

fn make_referer(next: &Url, previous: &Url) -> Option<HeaderValue> {
    if next.scheme() == "http" && previous.scheme() == "https" {
        return None;
    }
    let mut referer = previous.clone();
    referer.set_username("").ok()?;
    referer.set_password(None).ok()?;
    referer.set_fragment(None);
    referer.as_str().parse().ok()
}
