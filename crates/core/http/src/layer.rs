use std::{fmt, future::Future, pin::Pin};

use http::{HeaderMap, Method, StatusCode};
use nanocode_sync::Stream;

pub mod origin;
pub mod retry;

#[cfg(not(target_family = "wasm"))]
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
#[cfg(target_family = "wasm")]
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

#[cfg(not(target_family = "wasm"))]
pub type Body = Pin<Box<dyn Stream<Item = Result<Vec<u8>, Error>> + Send>>;
#[cfg(target_family = "wasm")]
pub type Body = Pin<Box<dyn Stream<Item = Result<Vec<u8>, Error>>>>;

pub trait Layer: Send + Sync + 'static {
    fn send<'a>(
        &'a self,
        request: Request,
        next: Next<'a>,
    ) -> BoxFuture<'a, Result<Response, Error>>;
}

pub trait Sender: Send + Sync {
    fn send(&self, request: Request) -> BoxFuture<'_, Result<Response, Error>>;
}

#[derive(Clone, Copy)]
pub struct Next<'a> {
    sender: &'a dyn Sender,
}

impl<'a> Next<'a> {
    #[must_use]
    pub const fn new(sender: &'a dyn Sender) -> Self {
        Self { sender }
    }

    pub fn send(self, request: Request) -> BoxFuture<'a, Result<Response, Error>> {
        self.sender.send(request)
    }
}

#[derive(Clone, Debug)]
pub struct Request {
    pub method: Method,
    pub url: String,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

pub struct Response {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Body,
    events: Option<Box<dyn EventHook>>,
}

impl Response {
    #[must_use]
    pub fn new(status: StatusCode, headers: HeaderMap, body: Body) -> Self {
        Self {
            status,
            headers,
            body,
            events: None,
        }
    }

    #[must_use]
    pub fn with_events(mut self, events: impl EventHook) -> Self {
        self.events = Some(Box::new(events));
        self
    }

    pub fn take_events(&mut self) -> Option<Box<dyn EventHook>> {
        self.events.take()
    }

    pub fn into_parts(self) -> (StatusCode, HeaderMap, Body, Option<Box<dyn EventHook>>) {
        (self.status, self.headers, self.body, self.events)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Event<'a> {
    pub name: Option<&'a str>,
    pub data: &'a str,
}

pub trait EventHook: Send + 'static {
    fn event(&mut self, event: Event<'_>) -> Option<BoxFuture<'static, Result<(), Error>>>;
}

#[derive(Clone, Debug)]
pub struct Error {
    detail: String,
    retryable: bool,
    timeout: bool,
    unsent: bool,
}

impl Error {
    #[must_use]
    pub fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
            retryable: false,
            timeout: false,
            unsent: false,
        }
    }

    #[must_use]
    pub const fn retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }

    #[must_use]
    pub const fn timeout(mut self, timeout: bool) -> Self {
        self.timeout = timeout;
        self
    }

    #[must_use]
    pub const fn unsent(mut self, unsent: bool) -> Self {
        self.unsent = unsent;
        self
    }

    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }

    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        self.retryable
    }

    #[must_use]
    pub const fn is_timeout(&self) -> bool {
        self.timeout
    }

    #[must_use]
    pub const fn is_unsent(&self) -> bool {
        self.unsent
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl std::error::Error for Error {}
