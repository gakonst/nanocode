use nanocode_sync::TryStreamExt;

use crate::layer::{self, BoxFuture, Error, Layer, Next, Request, Response, Sender};

pub struct ClientSender<'a> {
    client: &'a crate::Client,
}

impl<'a> ClientSender<'a> {
    #[must_use]
    pub const fn new(client: &'a crate::Client) -> Self {
        Self { client }
    }
}

impl Sender for ClientSender<'_> {
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
                .map_err(client_error)?;
            let status = response.status();
            let headers = response.headers().clone();
            Ok(Response::new(
                status,
                headers,
                Box::pin(
                    response
                        .bytes_stream()
                        .map_ok(Vec::from)
                        .map_err(client_error),
                ),
            ))
        })
    }
}

#[must_use]
pub fn retryable(error: &crate::Error) -> bool {
    !crate::connect::untrusted_certificate(error)
        && (error.is_connect() || error.is_body() || error.is_request() || error.is_decode())
}

pub fn client_error(error: crate::Error) -> Error {
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
    Http(crate::Error),
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
    request: crate::RequestBuilder,
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
    client: &crate::Client,
    layer: Option<&dyn Layer>,
    request: crate::Request,
) -> Result<crate::Response, Error> {
    let Some(layer) = layer else {
        return client
            .execute(request)
            .await
            .map(crate::decode_flate_response)
            .map_err(client_error);
    };
    let url = request.url().clone();
    let sent = Request {
        method: request.method().clone(),
        url: url.to_string(),
        headers: request.headers().clone(),
        body: request
            .body()
            .and_then(crate::Body::as_bytes)
            .map(<[u8]>::to_vec)
            .unwrap_or_default(),
    };
    let (status, headers, body, _) = layer
        .send(sent, Next::new(&ClientSender::new(client)))
        .await?
        .into_parts();
    Ok(crate::Response::from_stream(
        status,
        headers,
        url,
        body.map_ok(bytes::Bytes::from)
            .map_err(|error| crate::Error::body(layer::Error::new(error.detail()))),
    ))
}
