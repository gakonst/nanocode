pub mod host;

pub use host::{
    HttpBody, HttpError, HttpFuture, HttpRequest, HttpResponse, HttpTransport, SenderTransport,
};
pub use nanocode_http::layer::{self, Layer};

#[cfg(not(target_family = "wasm"))]
pub type HttpClient = reqwest::Client;
#[cfg(target_family = "wasm")]
pub use host::HttpClient;

pub(crate) const fn is_transient(error: &HttpError) -> bool {
    matches!(
        error,
        HttpError::Transport {
            retryable: true,
            ..
        } | HttpError::Transport { timeout: true, .. }
    )
}

#[cfg(not(target_family = "wasm"))]
const GZIP_LEVEL: u32 = 2;
#[cfg(not(target_family = "wasm"))]
const GZIP_PADDING_LENGTHS: usize = 257;

#[cfg(not(target_family = "wasm"))]
pub(crate) async fn post(
    client: &HttpClient,
    endpoint: &str,
    mut headers: http::HeaderMap,
    body: std::sync::Arc<String>,
    gzip: bool,
    layer: Option<&dyn Layer>,
) -> Result<HttpResponse, HttpError> {
    headers
        .entry(http::header::ACCEPT_ENCODING)
        .or_insert(http::HeaderValue::from_static("gzip, br, deflate, zstd"));
    let span = tracing::Span::current();
    span.record("request.bytes", body.len());
    let body = if gzip {
        let padded = gzip_padded(&body)?;
        let encoded = tokio::task::spawn_blocking(move || nanocode_http::gzip(&padded, GZIP_LEVEL))
            .await
            .map_err(|error| encode_error(&error))?
            .map_err(|error| encode_error(&error))?;
        span.record("request.gzip_bytes", encoded.len());
        headers.insert(
            http::header::CONTENT_ENCODING,
            http::HeaderValue::from_static("gzip"),
        );
        reqwest::Body::from(encoded)
    } else {
        reqwest::Body::from(bytes::Bytes::from_owner(SharedBody(body)))
    };
    let request = client
        .post(endpoint)
        .header("anthropic-version", crate::ANTHROPIC_VERSION)
        .headers(headers)
        .header("content-type", "application/json")
        .body(body)
        .build()
        .map_err(reqwest_error)?;
    let Some(layer) = layer else {
        return send_reqwest(client, request)
            .await
            .map(HttpResponse::from_layer)
            .map_err(HttpError::from);
    };
    let request = layer::Request {
        method: request.method().clone(),
        url: request.url().to_string(),
        headers: request.headers().clone(),
        body: request
            .body()
            .and_then(reqwest::Body::as_bytes)
            .map(<[u8]>::to_vec)
            .unwrap_or_default(),
    };
    layer
        .send(request, layer::Next::new(&ReqwestSender { client }))
        .await
        .map(HttpResponse::from_layer)
        .map_err(HttpError::from)
}

#[cfg(not(target_family = "wasm"))]
async fn send_reqwest(
    client: &HttpClient,
    request: reqwest::Request,
) -> Result<layer::Response, layer::Error> {
    use futures_util::TryStreamExt;

    let again = request.try_clone();
    let mut response = match (client.execute(request).await, again) {
        (Err(error), Some(again)) if nanocode_tls::peer_discarded_record(&error) => {
            tracing::warn!(
                target: "nanocode_claude",
                %error,
                "the peer could not read a record of a Claude Messages request; sending the request again"
            );
            tracing::Span::current().record("request.resent", true);
            client.execute(again).await
        }
        (sent, _) => sent,
    }
    .map(nanocode_http::decode_response)
    .map_err(|error| layer::Error::from(reqwest_error(error)))?;
    let status = response.status();
    let headers = std::mem::take(response.headers_mut());
    Ok(layer::Response::new(
        status,
        headers,
        Box::pin(
            response
                .bytes_stream()
                .map_ok(Vec::from)
                .map_err(|error| layer::Error::from(reqwest_error(error))),
        ),
    ))
}

#[cfg(not(target_family = "wasm"))]
struct ReqwestSender<'a> {
    client: &'a HttpClient,
}

#[cfg(not(target_family = "wasm"))]
impl layer::Sender for ReqwestSender<'_> {
    fn send(
        &self,
        request: layer::Request,
    ) -> layer::BoxFuture<'_, Result<layer::Response, layer::Error>> {
        Box::pin(async move {
            let request = self
                .client
                .request(request.method, request.url)
                .headers(request.headers)
                .body(request.body)
                .build()
                .map_err(|error| layer::Error::from(reqwest_error(error)))?;
            send_reqwest(self.client, request).await
        })
    }
}

#[cfg(not(target_family = "wasm"))]
struct SharedBody(std::sync::Arc<String>);

#[cfg(not(target_family = "wasm"))]
impl AsRef<[u8]> for SharedBody {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

#[cfg(not(target_family = "wasm"))]
fn gzip_padded(body: &str) -> Result<Vec<u8>, HttpError> {
    let mut random = [0_u8; 2 + GZIP_PADDING_LENGTHS];
    nanocode_platform::fill_random(&mut random).map_err(|error| encode_error(&error))?;
    let length = usize::from(u16::from_le_bytes([random[0], random[1]])) % GZIP_PADDING_LENGTHS;
    let mut padded = body.as_bytes().to_vec();
    padded.push(b' ');
    padded.extend(
        random[2..2 + length]
            .iter()
            .map(|byte| if byte & 1 == 0 { b' ' } else { b'\t' }),
    );
    Ok(padded)
}

#[cfg(not(target_family = "wasm"))]
fn encode_error(error: &dyn std::error::Error) -> HttpError {
    HttpError::Transport {
        retryable: false,
        timeout: false,
        unsent: true,
        detail: format!("gzip request body: {error}"),
    }
}

#[cfg(not(target_family = "wasm"))]
fn reqwest_error(error: reqwest::Error) -> HttpError {
    let mut detail = error.to_string();
    let mut source = std::error::Error::source(&error);
    while let Some(cause) = source {
        detail.push_str(": ");
        detail.push_str(&cause.to_string());
        source = cause.source();
    }
    HttpError::Transport {
        retryable: nanocode_http::retryable(&error),
        timeout: error.is_timeout(),
        unsent: error.is_connect()
            || error.is_builder()
            || nanocode_tls::peer_discarded_record(&error),
        detail,
    }
}

#[cfg(target_family = "wasm")]
pub(crate) async fn post(
    client: &HttpClient,
    endpoint: &str,
    headers: http::HeaderMap,
    body: std::sync::Arc<String>,
    _gzip: bool,
    layer: Option<&dyn Layer>,
) -> Result<HttpResponse, HttpError> {
    post_with(
        client.0.as_ref(),
        endpoint,
        headers,
        body.to_string(),
        layer,
    )
    .await
}

pub(crate) async fn post_with(
    transport: &dyn HttpTransport,
    endpoint: &str,
    mut headers: http::HeaderMap,
    body: String,
    layer: Option<&dyn Layer>,
) -> Result<HttpResponse, HttpError> {
    headers
        .entry("anthropic-version")
        .or_insert(http::HeaderValue::from_static(crate::ANTHROPIC_VERSION));
    headers.append(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static("application/json"),
    );
    let Some(layer) = layer else {
        return transport
            .post(HttpRequest {
                endpoint: endpoint.to_owned(),
                headers,
                body,
            })
            .await;
    };
    let request = layer::Request {
        method: http::Method::POST,
        url: endpoint.to_owned(),
        headers,
        body: body.into_bytes(),
    };
    layer
        .send(request, layer::Next::new(&TransportSender { transport }))
        .await
        .map(HttpResponse::from_layer)
        .map_err(HttpError::from)
}

struct TransportSender<'a> {
    transport: &'a dyn HttpTransport,
}

impl layer::Sender for TransportSender<'_> {
    fn send(
        &self,
        request: layer::Request,
    ) -> layer::BoxFuture<'_, Result<layer::Response, layer::Error>> {
        Box::pin(async move {
            let body = String::from_utf8(request.body).map_err(|_| {
                layer::Error::new("Messages request body is not UTF-8").unsent(true)
            })?;
            self.transport
                .post(HttpRequest {
                    endpoint: request.url,
                    headers: request.headers,
                    body,
                })
                .await
                .map(HttpResponse::into_layer)
                .map_err(layer::Error::from)
        })
    }
}
