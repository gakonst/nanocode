use futures_util::TryStreamExt;
use reqwest::ResponseBuilderExt;

use crate::layer::{self, BoxFuture, Error, Layer, Next, Request, Response, Sender};

pub struct ReqwestSender<'a> {
    client: &'a reqwest::Client,
}

impl<'a> ReqwestSender<'a> {
    #[must_use]
    pub const fn new(client: &'a reqwest::Client) -> Self {
        Self { client }
    }
}

impl Sender for ReqwestSender<'_> {
    fn send(&self, request: Request) -> BoxFuture<'_, Result<Response, Error>> {
        Box::pin(async move {
            let response = self
                .client
                .request(request.method, request.url)
                .headers(request.headers)
                .body(request.body)
                .send()
                .await
                .map(crate::decode_flate_response)
                .map_err(reqwest_error)?;
            let status = response.status();
            let headers = response.headers().clone();
            Ok(Response::new(
                status,
                headers,
                Box::pin(
                    response
                        .bytes_stream()
                        .map_ok(Vec::from)
                        .map_err(reqwest_error),
                ),
            ))
        })
    }
}

#[must_use]
pub fn retryable(error: &reqwest::Error) -> bool {
    error.is_connect() || error.is_body() || error.is_request() || error.is_decode()
}

pub fn reqwest_error(error: reqwest::Error) -> Error {
    let mut detail = error.to_string();
    let mut source = std::error::Error::source(&error);
    while let Some(cause) = source {
        detail.push_str(": ");
        detail.push_str(&cause.to_string());
        source = cause.source();
    }
    Error::new(detail)
        .retryable(retryable(&error))
        .timeout(error.is_timeout())
        .unsent(error.is_connect() || error.is_builder())
}

#[derive(Debug)]
pub enum BoundedError {
    Method(http::method::InvalidMethod),
    Http(reqwest::Error),
    Over(usize),
    Text(std::string::FromUtf8Error),
}

impl std::fmt::Display for BoundedError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Method(error) => error.fmt(formatter),
            Self::Http(error) => error.fmt(formatter),
            Self::Over(limit) => write!(formatter, "response over {limit} bytes"),
            Self::Text(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for BoundedError {}

pub async fn bounded_text(
    request: reqwest::RequestBuilder,
    limit: usize,
) -> Result<(u16, String), BoundedError> {
    let mut response = request
        .send()
        .await
        .map(crate::decode_flate_response)
        .map_err(BoundedError::Http)?;
    let status = response.status().as_u16();
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(BoundedError::Http)? {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > limit {
            return Err(BoundedError::Over(limit));
        }
    }
    Ok((
        status,
        String::from_utf8(bytes).map_err(BoundedError::Text)?,
    ))
}

pub async fn execute(
    client: &reqwest::Client,
    layer: Option<&dyn Layer>,
    request: reqwest::Request,
) -> Result<reqwest::Response, Error> {
    let Some(layer) = layer else {
        return client
            .execute(request)
            .await
            .map(crate::decode_flate_response)
            .map_err(reqwest_error);
    };
    let url = request.url().clone();
    let sent = Request {
        method: request.method().clone(),
        url: url.to_string(),
        headers: request.headers().clone(),
        body: request
            .body()
            .and_then(reqwest::Body::as_bytes)
            .map(<[u8]>::to_vec)
            .unwrap_or_default(),
    };
    let (status, headers, body, _) = layer
        .send(sent, Next::new(&ReqwestSender::new(client)))
        .await?
        .into_parts();
    let mut response = http::Response::builder()
        .status(status)
        .url(url)
        .body(reqwest::Body::wrap_stream(body))
        .map_err(|error| layer::Error::new(error.to_string()))?;
    *response.headers_mut() = headers;
    Ok(reqwest::Response::from(response))
}
