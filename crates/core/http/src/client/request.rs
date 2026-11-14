use std::{fmt, time::Duration};

use crate::types::{
    HeaderMap, HeaderName, HeaderValue, Method,
    header::{AUTHORIZATION, CONTENT_TYPE, Entry, OccupiedEntry},
};
use nanocode_url::Url;
use serde::Serialize;

use super::{Body, Client, Error, Pending, Result};

pub trait IntoUrl {
    fn into_url(self) -> Result<Url>;
}

impl IntoUrl for Url {
    fn into_url(self) -> Result<Url> {
        if self.has_host() {
            Ok(self)
        } else {
            Err(Error::builder(format!("URL {self} has no host")).with_url(self))
        }
    }
}

impl IntoUrl for &Url {
    fn into_url(self) -> Result<Url> {
        self.clone().into_url()
    }
}

impl IntoUrl for &str {
    fn into_url(self) -> Result<Url> {
        Url::parse(self).map_err(Error::builder)?.into_url()
    }
}

impl IntoUrl for String {
    fn into_url(self) -> Result<Url> {
        self.as_str().into_url()
    }
}

impl IntoUrl for &String {
    fn into_url(self) -> Result<Url> {
        self.as_str().into_url()
    }
}

pub struct Request {
    method: Method,
    url: Url,
    headers: HeaderMap,
    body: Option<Body>,
    timeout: Option<Duration>,
}

impl Request {
    #[must_use]
    pub fn new(method: Method, url: Url) -> Self {
        Self {
            method,
            url,
            headers: HeaderMap::new(),
            body: None,
            timeout: None,
        }
    }

    #[must_use]
    pub const fn method(&self) -> &Method {
        &self.method
    }

    pub const fn method_mut(&mut self) -> &mut Method {
        &mut self.method
    }

    #[must_use]
    pub const fn url(&self) -> &Url {
        &self.url
    }

    pub const fn url_mut(&mut self) -> &mut Url {
        &mut self.url
    }

    #[must_use]
    pub const fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    pub const fn headers_mut(&mut self) -> &mut HeaderMap {
        &mut self.headers
    }

    #[must_use]
    pub const fn body(&self) -> Option<&Body> {
        self.body.as_ref()
    }

    pub const fn body_mut(&mut self) -> &mut Option<Body> {
        &mut self.body
    }

    #[must_use]
    pub const fn timeout(&self) -> Option<&Duration> {
        self.timeout.as_ref()
    }

    pub const fn timeout_mut(&mut self) -> &mut Option<Duration> {
        &mut self.timeout
    }

    #[must_use]
    pub fn try_clone(&self) -> Option<Self> {
        let body = match &self.body {
            Some(body) => Some(body.try_clone()?),
            None => None,
        };
        Some(Self {
            method: self.method.clone(),
            url: self.url.clone(),
            headers: self.headers.clone(),
            body,
            timeout: self.timeout,
        })
    }

    pub(crate) fn into_parts(self) -> (Method, Url, HeaderMap, Option<Body>, Option<Duration>) {
        (self.method, self.url, self.headers, self.body, self.timeout)
    }
}

impl fmt::Debug for Request {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Request")
            .field("method", &self.method)
            .field("url", &self.url.as_str())
            .field("headers", &self.headers)
            .finish()
    }
}

#[must_use = "a RequestBuilder does nothing until it is sent"]
pub struct RequestBuilder {
    client: Client,
    request: Result<Request>,
}

impl RequestBuilder {
    pub(crate) const fn new(client: Client, request: Result<Request>) -> Self {
        Self { client, request }
    }

    pub const fn from_parts(client: Client, request: Request) -> Self {
        Self::new(client, Ok(request))
    }

    pub fn header<K, V>(self, key: K, value: V) -> Self
    where
        HeaderName: TryFrom<K>,
        <HeaderName as TryFrom<K>>::Error: Into<crate::types::Error>,
        HeaderValue: TryFrom<V>,
        <HeaderValue as TryFrom<V>>::Error: Into<crate::types::Error>,
    {
        self.header_sensitive(key, value, false)
    }

    fn header_sensitive<K, V>(mut self, key: K, value: V, sensitive: bool) -> Self
    where
        HeaderName: TryFrom<K>,
        <HeaderName as TryFrom<K>>::Error: Into<crate::types::Error>,
        HeaderValue: TryFrom<V>,
        <HeaderValue as TryFrom<V>>::Error: Into<crate::types::Error>,
    {
        let mut error = None;
        if let Ok(request) = &mut self.request {
            match HeaderName::try_from(key).map_err(Into::into) {
                Ok(key) => match HeaderValue::try_from(value).map_err(Into::into) {
                    Ok(mut value) => {
                        value.set_sensitive(sensitive);
                        request.headers_mut().append(key, value);
                    }
                    Err(failure) => error = Some(Error::builder(failure)),
                },
                Err(failure) => error = Some(Error::builder(failure)),
            }
        }
        if let Some(error) = error {
            self.request = Err(error);
        }
        self
    }

    pub fn headers(mut self, headers: HeaderMap) -> Self {
        if let Ok(request) = &mut self.request {
            replace_headers(request.headers_mut(), headers);
        }
        self
    }

    pub fn bearer_auth<T: fmt::Display>(self, token: T) -> Self {
        self.header_sensitive(AUTHORIZATION, format!("Bearer {token}"), true)
    }

    pub fn body<T: Into<Body>>(mut self, body: T) -> Self {
        if let Ok(request) = &mut self.request {
            *request.body_mut() = Some(body.into());
        }
        self
    }

    pub const fn timeout(mut self, timeout: Duration) -> Self {
        if let Ok(request) = &mut self.request {
            *request.timeout_mut() = Some(timeout);
        }
        self
    }

    pub fn json<T: Serialize + ?Sized>(mut self, json: &T) -> Self {
        let mut error = None;
        if let Ok(request) = &mut self.request {
            match serde_json::to_vec(json) {
                Ok(body) => {
                    if !request.headers().contains_key(CONTENT_TYPE) {
                        request
                            .headers_mut()
                            .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
                    }
                    *request.body_mut() = Some(body.into());
                }
                Err(failure) => error = Some(Error::builder(failure)),
            }
        }
        if let Some(error) = error {
            self.request = Err(error);
        }
        self
    }

    pub fn build(self) -> Result<Request> {
        self.request
    }

    #[must_use]
    pub fn build_split(self) -> (Client, Result<Request>) {
        (self.client, self.request)
    }

    pub fn send(self) -> Pending {
        match self.request {
            Ok(request) => self.client.execute(request),
            Err(error) => Box::pin(std::future::ready(Err(error))),
        }
    }

    #[must_use]
    pub fn try_clone(&self) -> Option<Self> {
        let request = match &self.request {
            Ok(request) => Ok(request.try_clone()?),
            Err(_) => return None,
        };
        Some(Self {
            client: self.client.clone(),
            request,
        })
    }
}

impl fmt::Debug for RequestBuilder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.request {
            Ok(request) => request.fmt(formatter),
            Err(error) => formatter
                .debug_tuple("RequestBuilder")
                .field(error)
                .finish(),
        }
    }
}

pub fn replace_headers(destination: &mut HeaderMap, source: HeaderMap) {
    let mut previous: Option<OccupiedEntry<'_, HeaderValue>> = None;
    for (key, value) in source {
        match key {
            Some(key) => match destination.entry(key) {
                Entry::Occupied(mut entry) => {
                    entry.insert(value);
                    previous = Some(entry);
                }
                Entry::Vacant(entry) => previous = Some(entry.insert_entry(value)),
            },
            None => previous
                .as_mut()
                .expect("a HeaderMap yields a name before its extra values")
                .append(value),
        }
    }
}
