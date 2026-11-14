use std::{fmt, sync::mpsc as sync_channel, thread, time::Duration};

use crate::types::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use nanocode_bytes::Bytes;
use nanocode_sync::mpsc;
use nanocode_url::Url;

use super::{Body, IntoUrl, Request, Result, redirect};

type Job = (Request, sync_channel::SyncSender<Result<Response>>);

#[derive(Clone)]
pub struct Client {
    client: super::Client,
    jobs: mpsc::UnboundedSender<Job>,
}

#[must_use]
pub struct ClientBuilder {
    builder: super::ClientBuilder,
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientBuilder {
    pub fn new() -> Self {
        Self {
            builder: super::ClientBuilder::new(),
        }
    }

    pub fn default_headers(self, headers: HeaderMap) -> Self {
        Self {
            builder: self.builder.default_headers(headers),
        }
    }

    pub fn timeout(self, timeout: Duration) -> Self {
        Self {
            builder: self.builder.timeout(timeout),
        }
    }

    pub fn redirect(self, policy: redirect::Policy) -> Self {
        Self {
            builder: self.builder.redirect(policy),
        }
    }

    pub fn build(self) -> Result<Client> {
        let client = self.builder.build()?;
        let (jobs, mut queue) = mpsc::unbounded_channel::<Job>();
        let runtime = nanocode_executor::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(super::Error::builder)?;
        let worker = client.clone();
        thread::Builder::new()
            .name("nanocode-http-blocking".to_owned())
            .spawn(move || {
                runtime.block_on(async move {
                    while let Some((request, reply)) = queue.recv().await {
                        let response = run(&worker, request).await;
                        drop(reply.send(response));
                    }
                });
            })
            .map_err(super::Error::builder)?;
        Ok(Client { client, jobs })
    }
}

async fn run(client: &super::Client, request: Request) -> Result<Response> {
    let response = client.execute(request).await?;
    let status = response.status();
    let headers = response.headers().clone();
    let url = response.url().clone();
    let body = response.bytes().await?;
    Ok(Response {
        status,
        headers,
        url,
        body,
    })
}

impl fmt::Debug for Client {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("blocking::Client")
            .field("client", &self.client)
            .finish()
    }
}

impl Client {
    pub fn builder() -> ClientBuilder {
        ClientBuilder::new()
    }

    pub fn post<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(crate::types::Method::POST, url)
    }

    pub fn get<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(crate::types::Method::GET, url)
    }

    pub fn request<U: IntoUrl>(&self, method: crate::types::Method, url: U) -> RequestBuilder {
        RequestBuilder {
            client: self.clone(),
            inner: self.client.request(method, url),
        }
    }

    pub fn execute(&self, request: Request) -> Result<Response> {
        let (reply, answer) = sync_channel::sync_channel(1);
        self.jobs
            .send((request, reply))
            .map_err(|_| super::Error::request("the blocking HTTP worker stopped"))?;
        answer
            .recv()
            .map_err(|_| super::Error::request("the blocking HTTP worker stopped"))?
    }
}

#[must_use]
pub struct RequestBuilder {
    client: Client,
    inner: super::RequestBuilder,
}

impl RequestBuilder {
    pub fn header<K, V>(self, key: K, value: V) -> Self
    where
        HeaderName: TryFrom<K>,
        <HeaderName as TryFrom<K>>::Error: Into<crate::types::Error>,
        HeaderValue: TryFrom<V>,
        <HeaderValue as TryFrom<V>>::Error: Into<crate::types::Error>,
    {
        Self {
            client: self.client,
            inner: self.inner.header(key, value),
        }
    }

    pub fn headers(self, headers: HeaderMap) -> Self {
        Self {
            client: self.client,
            inner: self.inner.headers(headers),
        }
    }

    pub fn body<T: Into<Body>>(self, body: T) -> Self {
        Self {
            client: self.client,
            inner: self.inner.body(body),
        }
    }

    pub fn timeout(self, timeout: Duration) -> Self {
        Self {
            client: self.client,
            inner: self.inner.timeout(timeout),
        }
    }

    pub fn send(self) -> Result<Response> {
        self.client.execute(self.inner.build()?)
    }
}

#[derive(Debug)]
pub struct Response {
    status: StatusCode,
    headers: HeaderMap,
    url: Url,
    body: Bytes,
}

impl Response {
    #[must_use]
    pub const fn status(&self) -> StatusCode {
        self.status
    }

    #[must_use]
    pub const fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    #[must_use]
    pub const fn url(&self) -> &Url {
        &self.url
    }

    pub fn bytes(self) -> Result<Bytes> {
        Ok(self.body)
    }

    pub fn text(self) -> Result<String> {
        Ok(String::from_utf8_lossy(&self.body).into_owned())
    }

    pub fn error_for_status(self) -> Result<Self> {
        if self.status.is_client_error() || self.status.is_server_error() {
            Err(super::Error::for_status(self.status, self.url))
        } else {
            Ok(self)
        }
    }
}
