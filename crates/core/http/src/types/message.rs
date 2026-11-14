use std::{convert::Infallible, error, fmt};

use super::{
    header::{HeaderMap, HeaderName, HeaderValue, InvalidHeaderName, InvalidHeaderValue},
    method::{InvalidMethod, Method},
    status::{InvalidStatusCode, StatusCode},
    uri::{InvalidUri, Uri},
};

#[derive(PartialEq, PartialOrd, Copy, Clone, Eq, Ord, Hash)]
enum Http {
    Http09,
    Http10,
    Http11,
    H2,
    H3,
}

#[derive(PartialEq, PartialOrd, Copy, Clone, Eq, Ord, Hash)]
pub struct Version(Http);

impl Version {
    pub const HTTP_09: Self = Self(Http::Http09);
    pub const HTTP_10: Self = Self(Http::Http10);
    pub const HTTP_11: Self = Self(Http::Http11);
    pub const HTTP_2: Self = Self(Http::H2);
    pub const HTTP_3: Self = Self(Http::H3);
}

impl Default for Version {
    fn default() -> Self {
        Self::HTTP_11
    }
}

impl fmt::Debug for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.0 {
            Http::Http09 => "HTTP/0.9",
            Http::Http10 => "HTTP/1.0",
            Http::Http11 => "HTTP/1.1",
            Http::H2 => "HTTP/2.0",
            Http::H3 => "HTTP/3.0",
        })
    }
}

enum ErrorKind {
    StatusCode(InvalidStatusCode),
    Method(InvalidMethod),
    Uri(InvalidUri),
    HeaderName(InvalidHeaderName),
    HeaderValue(InvalidHeaderValue),
}

pub struct Error {
    kind: ErrorKind,
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn is<T: error::Error + 'static>(&self) -> bool {
        self.get_ref().is::<T>()
    }

    pub fn get_ref(&self) -> &(dyn error::Error + 'static) {
        match &self.kind {
            ErrorKind::StatusCode(error) => error,
            ErrorKind::Method(error) => error,
            ErrorKind::Uri(error) => error,
            ErrorKind::HeaderName(error) => error,
            ErrorKind::HeaderValue(error) => error,
        }
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("http::Error")
            .field(&self.get_ref())
            .finish()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.get_ref(), formatter)
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        self.get_ref().source()
    }
}

macro_rules! error_from {
    ($($source:ty => $kind:ident),* $(,)?) => {
        $(impl From<$source> for Error {
            fn from(error: $source) -> Self {
                Self {
                    kind: ErrorKind::$kind(error),
                }
            }
        })*
    };
}

error_from!(
    InvalidStatusCode => StatusCode,
    InvalidMethod => Method,
    InvalidUri => Uri,
    InvalidHeaderName => HeaderName,
    InvalidHeaderValue => HeaderValue,
);

impl From<Infallible> for Error {
    fn from(never: Infallible) -> Self {
        match never {}
    }
}

fn append_header<K, V>(headers: &mut HeaderMap, key: K, value: V) -> Result<()>
where
    HeaderName: TryFrom<K>,
    <HeaderName as TryFrom<K>>::Error: Into<Error>,
    HeaderValue: TryFrom<V>,
    <HeaderValue as TryFrom<V>>::Error: Into<Error>,
{
    let name = HeaderName::try_from(key).map_err(Into::into)?;
    let value = HeaderValue::try_from(value).map_err(Into::into)?;
    headers.append(name, value);
    Ok(())
}

pub mod request {
    use super::{Error, HeaderMap, HeaderName, HeaderValue, Method, Result, Uri, Version};

    #[derive(Clone, Debug, Default)]
    pub struct Parts {
        pub method: Method,
        pub uri: Uri,
        pub version: Version,
        pub headers: HeaderMap,
    }

    #[derive(Debug)]
    pub struct Builder {
        inner: Result<Parts>,
    }

    impl Builder {
        pub fn new() -> Self {
            Self::default()
        }

        fn and_then(self, change: impl FnOnce(Parts) -> Result<Parts>) -> Self {
            Self {
                inner: self.inner.and_then(change),
            }
        }

        #[must_use]
        pub fn method<T>(self, method: T) -> Self
        where
            Method: TryFrom<T>,
            <Method as TryFrom<T>>::Error: Into<Error>,
        {
            self.and_then(|mut parts| {
                parts.method = Method::try_from(method).map_err(Into::into)?;
                Ok(parts)
            })
        }

        #[must_use]
        pub fn uri<T>(self, uri: T) -> Self
        where
            Uri: TryFrom<T>,
            <Uri as TryFrom<T>>::Error: Into<Error>,
        {
            self.and_then(|mut parts| {
                parts.uri = Uri::try_from(uri).map_err(Into::into)?;
                Ok(parts)
            })
        }

        #[must_use]
        pub fn version(self, version: Version) -> Self {
            self.and_then(|mut parts| {
                parts.version = version;
                Ok(parts)
            })
        }

        #[must_use]
        pub fn header<K, V>(self, key: K, value: V) -> Self
        where
            HeaderName: TryFrom<K>,
            <HeaderName as TryFrom<K>>::Error: Into<Error>,
            HeaderValue: TryFrom<V>,
            <HeaderValue as TryFrom<V>>::Error: Into<Error>,
        {
            self.and_then(|mut parts| {
                super::append_header(&mut parts.headers, key, value)?;
                Ok(parts)
            })
        }

        pub fn body<T>(self, body: T) -> Result<super::Request<T>> {
            self.inner.map(|head| super::Request { head, body })
        }
    }

    impl Default for Builder {
        fn default() -> Self {
            Self {
                inner: Ok(Parts::default()),
            }
        }
    }
}

pub mod response {
    use super::{Error, HeaderMap, HeaderName, HeaderValue, Result, StatusCode, Version};

    #[derive(Clone, Debug, Default)]
    pub struct Parts {
        pub status: StatusCode,
        pub version: Version,
        pub headers: HeaderMap,
    }

    #[derive(Debug)]
    pub struct Builder {
        inner: Result<Parts>,
    }

    impl Builder {
        pub fn new() -> Self {
            Self::default()
        }

        fn and_then(self, change: impl FnOnce(Parts) -> Result<Parts>) -> Self {
            Self {
                inner: self.inner.and_then(change),
            }
        }

        #[must_use]
        pub fn status<T>(self, status: T) -> Self
        where
            StatusCode: TryFrom<T>,
            <StatusCode as TryFrom<T>>::Error: Into<Error>,
        {
            self.and_then(|mut parts| {
                parts.status = StatusCode::try_from(status).map_err(Into::into)?;
                Ok(parts)
            })
        }

        #[must_use]
        pub fn version(self, version: Version) -> Self {
            self.and_then(|mut parts| {
                parts.version = version;
                Ok(parts)
            })
        }

        #[must_use]
        pub fn header<K, V>(self, key: K, value: V) -> Self
        where
            HeaderName: TryFrom<K>,
            <HeaderName as TryFrom<K>>::Error: Into<Error>,
            HeaderValue: TryFrom<V>,
            <HeaderValue as TryFrom<V>>::Error: Into<Error>,
        {
            self.and_then(|mut parts| {
                super::append_header(&mut parts.headers, key, value)?;
                Ok(parts)
            })
        }

        pub fn body<T>(self, body: T) -> Result<super::Response<T>> {
            self.inner.map(|head| super::Response { head, body })
        }
    }

    impl Default for Builder {
        fn default() -> Self {
            Self {
                inner: Ok(Parts::default()),
            }
        }
    }
}

#[derive(Clone, Default)]
pub struct Request<T> {
    head: request::Parts,
    body: T,
}

impl Request<()> {
    pub fn builder() -> request::Builder {
        request::Builder::new()
    }
}

impl<T> Request<T> {
    pub fn new(body: T) -> Self {
        Self {
            head: request::Parts::default(),
            body,
        }
    }

    pub const fn from_parts(head: request::Parts, body: T) -> Self {
        Self { head, body }
    }

    pub const fn method(&self) -> &Method {
        &self.head.method
    }

    pub const fn method_mut(&mut self) -> &mut Method {
        &mut self.head.method
    }

    pub const fn uri(&self) -> &Uri {
        &self.head.uri
    }

    pub const fn uri_mut(&mut self) -> &mut Uri {
        &mut self.head.uri
    }

    pub const fn version(&self) -> Version {
        self.head.version
    }

    pub const fn version_mut(&mut self) -> &mut Version {
        &mut self.head.version
    }

    pub const fn headers(&self) -> &HeaderMap {
        &self.head.headers
    }

    pub const fn headers_mut(&mut self) -> &mut HeaderMap {
        &mut self.head.headers
    }

    pub const fn body(&self) -> &T {
        &self.body
    }

    pub const fn body_mut(&mut self) -> &mut T {
        &mut self.body
    }

    pub fn into_body(self) -> T {
        self.body
    }

    pub fn into_parts(self) -> (request::Parts, T) {
        (self.head, self.body)
    }

    pub fn map<U>(self, change: impl FnOnce(T) -> U) -> Request<U> {
        Request {
            head: self.head,
            body: change(self.body),
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for Request<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Request")
            .field("method", self.method())
            .field("uri", self.uri())
            .field("version", &self.version())
            .field("headers", self.headers())
            .field("body", self.body())
            .finish()
    }
}

#[derive(Clone, Default)]
pub struct Response<T> {
    head: response::Parts,
    body: T,
}

impl Response<()> {
    pub fn builder() -> response::Builder {
        response::Builder::new()
    }
}

impl<T> Response<T> {
    pub fn new(body: T) -> Self {
        Self {
            head: response::Parts::default(),
            body,
        }
    }

    pub const fn from_parts(head: response::Parts, body: T) -> Self {
        Self { head, body }
    }

    pub const fn status(&self) -> StatusCode {
        self.head.status
    }

    pub const fn status_mut(&mut self) -> &mut StatusCode {
        &mut self.head.status
    }

    pub const fn version(&self) -> Version {
        self.head.version
    }

    pub const fn version_mut(&mut self) -> &mut Version {
        &mut self.head.version
    }

    pub const fn headers(&self) -> &HeaderMap {
        &self.head.headers
    }

    pub const fn headers_mut(&mut self) -> &mut HeaderMap {
        &mut self.head.headers
    }

    pub const fn body(&self) -> &T {
        &self.body
    }

    pub const fn body_mut(&mut self) -> &mut T {
        &mut self.body
    }

    pub fn into_body(self) -> T {
        self.body
    }

    pub fn into_parts(self) -> (response::Parts, T) {
        (self.head, self.body)
    }

    pub fn map<U>(self, change: impl FnOnce(T) -> U) -> Response<U> {
        Response {
            head: self.head,
            body: change(self.body),
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for Response<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Response")
            .field("status", &self.status())
            .field("version", &self.version())
            .field("headers", self.headers())
            .field("body", self.body())
            .finish()
    }
}
