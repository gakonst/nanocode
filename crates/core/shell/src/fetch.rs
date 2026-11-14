use nanocode_bytes::Bytes;
use std::{cell::RefCell, future::Future, pin::Pin, rc::Rc, time::Duration};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
    pub follow: Option<u32>,
    pub timeout: Option<Duration>,
    pub limit: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub protocol: String,
    pub status: u16,
    pub reason: String,
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
    pub url: String,
    pub redirects: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    Resolve,
    Connect,
    Timeout,
    Redirects,
    Secure(String),
    TooLarge,
    Denied(String),
    Other(String),
}

pub trait Fetch {
    fn fetch(&self, request: Request) -> Pin<Box<dyn Future<Output = Result<Response, Failure>>>>;
}

#[derive(Clone)]
pub(crate) struct Network {
    pub(crate) fetch: Rc<dyn Fetch>,
    pub(crate) limit: u64,
}

thread_local! {
    static CURRENT: RefCell<Option<Network>> = const { RefCell::new(None) };
}

pub(crate) struct Stepping(Option<Network>);

impl Stepping {
    pub(crate) fn with(network: Option<Network>) -> Self {
        Self(CURRENT.replace(network))
    }
}

impl Drop for Stepping {
    fn drop(&mut self) {
        CURRENT.set(self.0.take());
    }
}

pub(crate) fn current() -> Option<Network> {
    CURRENT.with_borrow(Clone::clone)
}
