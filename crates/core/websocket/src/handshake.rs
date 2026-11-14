use std::io::Write as _;

use nanocode_sys::base64::STANDARD;
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri, Version};
use httparse::Status;
use tokio::io::{AsyncRead, AsyncReadExt as _};

use crate::error::{Error, ProtocolError, Result, SubProtocolError, UrlError};

const ACCEPT_GUID: &[u8] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const HEADERS_MAX: usize = 124;
const READ_CHUNK_BYTES: usize = 4096;
const KEY_BYTES: usize = 16;
const ENCODED_KEY_BYTES: usize = 24;
const KEY_HEADER: &str = "Sec-WebSocket-Key";
const PROTOCOL_HEADER: &str = "Sec-WebSocket-Protocol";
const WEBSOCKET_HEADERS: [&str; 5] = [
    "Host",
    "Connection",
    "Upgrade",
    "Sec-WebSocket-Version",
    KEY_HEADER,
];
const ATTACK_BYTES_MAX: usize = 65536;
const ATTACK_PACKETS_MAX: usize = 512;
const ATTACK_PACKET_BYTES_MIN: usize = 128;
const ATTACK_PACKETS_CHECKED_MIN: usize = 64;
const HTTP_MINOR_VERSION_MIN: u8 = 1;

pub mod client {
    pub type Request = http::Request<()>;
    pub type Response = http::Response<Option<Vec<u8>>>;
}

pub mod server {
    pub type Request = http::Request<()>;
    pub type Response = http::Response<()>;
    pub type ErrorResponse = http::Response<Option<String>>;

    pub trait Callback: Sized {
        fn on_request(
            self,
            request: &Request,
            response: Response,
        ) -> Result<Response, ErrorResponse>;
    }

    impl<F> Callback for F
    where
        F: FnOnce(&Request, Response) -> Result<Response, ErrorResponse>,
    {
        fn on_request(
            self,
            request: &Request,
            response: Response,
        ) -> Result<Response, ErrorResponse> {
            self(request, response)
        }
    }
}

pub trait IntoClientRequest {
    fn into_client_request(self) -> Result<client::Request>;
}

impl IntoClientRequest for &str {
    fn into_client_request(self) -> Result<client::Request> {
        self.parse::<Uri>()?.into_client_request()
    }
}

impl IntoClientRequest for &String {
    fn into_client_request(self) -> Result<client::Request> {
        self.as_str().into_client_request()
    }
}

impl IntoClientRequest for String {
    fn into_client_request(self) -> Result<client::Request> {
        self.as_str().into_client_request()
    }
}

impl IntoClientRequest for &Uri {
    fn into_client_request(self) -> Result<client::Request> {
        self.clone().into_client_request()
    }
}

impl IntoClientRequest for Uri {
    fn into_client_request(self) -> Result<client::Request> {
        let authority = self
            .authority()
            .ok_or(Error::Url(UrlError::NoHostName))?
            .as_str();
        let host = authority
            .find('@')
            .map_or(authority, |index| authority.split_at(index + 1).1);
        if host.is_empty() {
            return Err(Error::Url(UrlError::EmptyHostName));
        }
        let request = http::Request::builder()
            .method("GET")
            .header("Host", host)
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header(KEY_HEADER, generate_key()?)
            .uri(self)
            .body(())?;
        Ok(request)
    }
}

impl IntoClientRequest for client::Request {
    fn into_client_request(self) -> Result<client::Request> {
        Ok(self)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Plain,
    Tls,
}

pub(crate) fn uri_mode(uri: &Uri) -> Result<Mode> {
    match uri.scheme_str() {
        Some("ws") => Ok(Mode::Plain),
        Some("wss") => Ok(Mode::Tls),
        _ => Err(Error::Url(UrlError::UnsupportedUrlScheme)),
    }
}

fn generate_key() -> Result<String> {
    let mut key = [0; KEY_BYTES];
    nanocode_platform::fill_random(&mut key).map_err(Error::Io)?;
    Ok(STANDARD.encode(key))
}

pub(crate) fn derive_accept_key(key: &[u8]) -> String {
    let mut input = Vec::with_capacity(key.len() + ACCEPT_GUID.len());
    input.extend_from_slice(key);
    input.extend_from_slice(ACCEPT_GUID);
    STANDARD.encode(nanocode_digest::Sha1::digest(&input))
}

const fn version_text(version: Version) -> Result<&'static str> {
    match version {
        Version::HTTP_09 => Ok("HTTP/0.9"),
        Version::HTTP_10 => Ok("HTTP/1.0"),
        Version::HTTP_11 => Ok("HTTP/1.1"),
        _ => Err(Error::Protocol(ProtocolError::WrongHttpVersion)),
    }
}

pub(crate) struct ClientStart {
    pub(crate) request: Vec<u8>,
    pub(crate) verify: VerifyData,
}

pub(crate) fn start_client(request: client::Request) -> Result<ClientStart> {
    if request.method() != Method::GET {
        return Err(Error::Protocol(ProtocolError::WrongHttpMethod));
    }
    if request.version() < Version::HTTP_11 {
        return Err(Error::Protocol(ProtocolError::WrongHttpVersion));
    }
    uri_mode(request.uri())?;
    let subprotocols = request
        .headers()
        .get(PROTOCOL_HEADER)
        .map(|value| {
            value.to_str().map(|value| {
                value
                    .split(',')
                    .map(|part| part.trim().to_owned())
                    .collect()
            })
        })
        .transpose()?;
    let (request, key) = generate_request(request)?;
    Ok(ClientStart {
        request,
        verify: VerifyData {
            accept_key: derive_accept_key(key.as_bytes()),
            subprotocols,
        },
    })
}

fn invalid_header(name: &str) -> Error {
    Error::Protocol(ProtocolError::InvalidHeader(Box::new(
        HeaderName::from_bytes(name.as_bytes()).expect("a WebSocket header name is valid"),
    )))
}

fn generate_request(mut request: client::Request) -> Result<(Vec<u8>, String)> {
    let mut output = Vec::new();
    let path = request
        .uri()
        .path_and_query()
        .ok_or(Error::Url(UrlError::NoPathOrQuery))?
        .as_str();
    write!(
        output,
        "GET {path} {}\r\n",
        version_text(request.version())?
    )
    .expect("writing to a vector succeeds");
    let key = request
        .headers()
        .get(KEY_HEADER)
        .ok_or_else(|| invalid_header(KEY_HEADER))?
        .to_str()?
        .to_owned();
    let headers = request.headers_mut();
    for header in WEBSOCKET_HEADERS {
        let value = headers
            .remove(header)
            .ok_or_else(|| invalid_header(header))?;
        let text = value.to_str().map_err(|error| {
            Error::Utf8(format!(
                "{error} for header name '{header}' with value: {value:?}"
            ))
        })?;
        write!(output, "{header}: {text}\r\n").expect("writing to a vector succeeds");
    }
    for (name, value) in headers.iter() {
        let mut name_text = name.as_str();
        if WEBSOCKET_HEADERS
            .iter()
            .any(|header| header.eq_ignore_ascii_case(name_text))
        {
            return Err(Error::Protocol(ProtocolError::InvalidHeader(Box::new(
                name.clone(),
            ))));
        }
        if name_text == "sec-websocket-protocol" {
            name_text = PROTOCOL_HEADER;
        }
        if name_text == "origin" {
            name_text = "Origin";
        }
        output.extend_from_slice(name_text.as_bytes());
        output.extend_from_slice(b": ");
        output.extend_from_slice(value.as_bytes());
        output.extend_from_slice(b"\r\n");
    }
    output.extend_from_slice(b"\r\n");
    Ok((output, key))
}

pub(crate) struct VerifyData {
    accept_key: String,
    subprotocols: Option<Vec<String>>,
}

impl VerifyData {
    pub(crate) fn verify(&self, response: client::Response) -> Result<client::Response> {
        if response.status() != StatusCode::SWITCHING_PROTOCOLS {
            return Err(Error::Http(Box::new(response)));
        }
        let headers = response.headers();
        let text = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
        if !text("Upgrade").is_some_and(|value| value.eq_ignore_ascii_case("websocket")) {
            return Err(Error::Protocol(
                ProtocolError::MissingUpgradeWebSocketHeader,
            ));
        }
        if !text("Connection").is_some_and(|value| {
            value
                .split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("Upgrade"))
        }) {
            return Err(Error::Protocol(
                ProtocolError::MissingConnectionUpgradeHeader,
            ));
        }
        if headers
            .get("Sec-WebSocket-Accept")
            .is_none_or(|value| value != self.accept_key.as_str())
        {
            return Err(Error::Protocol(
                ProtocolError::SecWebSocketAcceptKeyMismatch,
            ));
        }
        let rejected = match (headers.get(PROTOCOL_HEADER), &self.subprotocols) {
            (None, Some(_)) => Some(SubProtocolError::NoSubProtocol),
            (Some(_), None) => Some(SubProtocolError::ServerSentSubProtocolNoneRequested),
            (Some(returned), Some(accepted)) => {
                let returned = returned.to_str()?;
                (!accepted.iter().any(|accepted| accepted == returned))
                    .then_some(SubProtocolError::InvalidSubProtocol)
            }
            (None, None) => None,
        };
        match rejected {
            Some(error) => Err(Error::Protocol(
                ProtocolError::SecWebSocketSubProtocolError(error),
            )),
            None => Ok(response),
        }
    }
}

#[derive(Default)]
struct AttackCheck {
    packets: usize,
    bytes: usize,
}

impl AttackCheck {
    const fn check(&mut self, size: usize) -> Result<()> {
        self.packets += 1;
        self.bytes += size;
        if self.bytes > ATTACK_BYTES_MAX || self.packets > ATTACK_PACKETS_MAX {
            return Err(Error::AttackAttempt);
        }
        if self.packets > ATTACK_PACKETS_CHECKED_MIN
            && self.packets * ATTACK_PACKET_BYTES_MIN > self.bytes
        {
            return Err(Error::AttackAttempt);
        }
        Ok(())
    }
}

pub(crate) trait TryParse: Sized {
    fn try_parse(buffer: &[u8]) -> Result<Option<(usize, Self)>>;
}

pub(crate) async fn read_handshake<T: TryParse, S: AsyncRead + Unpin>(
    stream: &mut S,
) -> Result<(T, Vec<u8>)> {
    let mut buffer = Vec::new();
    let mut chunk = vec![0; READ_CHUNK_BYTES];
    let mut attack = AttackCheck::default();
    loop {
        let count = stream.read(&mut chunk).await?;
        if count == 0 {
            return Err(Error::Protocol(ProtocolError::HandshakeIncomplete));
        }
        attack.check(count)?;
        buffer.extend_from_slice(&chunk[..count]);
        if let Some((size, parsed)) = T::try_parse(&buffer)? {
            let tail = buffer.split_off(size);
            return Ok((parsed, tail));
        }
    }
}

fn header_map(raw: &[httparse::Header<'_>]) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    for header in raw {
        headers.append(
            HeaderName::from_bytes(header.name.as_bytes())?,
            HeaderValue::from_bytes(header.value)?,
        );
    }
    Ok(headers)
}

impl TryParse for client::Response {
    fn try_parse(buffer: &[u8]) -> Result<Option<(usize, Self)>> {
        let mut headers = [httparse::EMPTY_HEADER; HEADERS_MAX];
        let mut raw = httparse::Response::new(&mut headers);
        let Status::Complete(size) = raw.parse(buffer)? else {
            return Ok(None);
        };
        if raw.version.expect("a complete response has a version") < HTTP_MINOR_VERSION_MIN {
            return Err(Error::Protocol(ProtocolError::WrongHttpVersion));
        }
        let headers = header_map(raw.headers)?;
        let mut response = Self::new(None);
        *response.status_mut() =
            StatusCode::from_u16(raw.code.expect("a complete response has a status code"))?;
        *response.headers_mut() = headers;
        *response.version_mut() = Version::HTTP_11;
        Ok(Some((size, response)))
    }
}

impl TryParse for server::Request {
    fn try_parse(buffer: &[u8]) -> Result<Option<(usize, Self)>> {
        let mut headers = [httparse::EMPTY_HEADER; HEADERS_MAX];
        let mut raw = httparse::Request::new(&mut headers);
        let Status::Complete(size) = raw.parse(buffer)? else {
            return Ok(None);
        };
        if raw.method.expect("a complete request has a method") != "GET" {
            return Err(Error::Protocol(ProtocolError::WrongHttpMethod));
        }
        if raw.version.expect("a complete request has a version") < HTTP_MINOR_VERSION_MIN {
            return Err(Error::Protocol(ProtocolError::WrongHttpVersion));
        }
        let headers = header_map(raw.headers)?;
        let mut request = Self::new(());
        *request.method_mut() = Method::GET;
        *request.headers_mut() = headers;
        *request.uri_mut() = raw.path.expect("a complete request has a path").parse()?;
        *request.version_mut() = Version::HTTP_11;
        Ok(Some((size, request)))
    }
}

pub(crate) fn create_response(request: &server::Request) -> Result<server::Response> {
    if request.method() != Method::GET {
        return Err(Error::Protocol(ProtocolError::WrongHttpMethod));
    }
    if request.version() < Version::HTTP_11 {
        return Err(Error::Protocol(ProtocolError::WrongHttpVersion));
    }
    let headers = request.headers();
    let text = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
    if !text("Connection").is_some_and(|value| {
        value
            .split([' ', ','])
            .any(|part| part.eq_ignore_ascii_case("Upgrade"))
    }) {
        return Err(Error::Protocol(
            ProtocolError::MissingConnectionUpgradeHeader,
        ));
    }
    if !text("Upgrade").is_some_and(|value| value.eq_ignore_ascii_case("websocket")) {
        return Err(Error::Protocol(
            ProtocolError::MissingUpgradeWebSocketHeader,
        ));
    }
    if headers
        .get("Sec-WebSocket-Version")
        .is_none_or(|value| value != "13")
    {
        return Err(Error::Protocol(
            ProtocolError::MissingSecWebSocketVersionHeader,
        ));
    }
    let key = headers
        .get(KEY_HEADER)
        .ok_or(Error::Protocol(ProtocolError::MissingSecWebSocketKey))?;
    if !is_valid_key(key) {
        return Err(Error::Protocol(ProtocolError::InvalidSecWebSocketKey));
    }
    Ok(server::Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .version(request.version())
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Accept", derive_accept_key(key.as_bytes()))
        .body(())?)
}

fn is_valid_key(key: &HeaderValue) -> bool {
    key.len() == ENCODED_KEY_BYTES
        && STANDARD
            .decode(key.as_bytes())
            .is_ok_and(|decoded| decoded.len() == KEY_BYTES)
}

pub(crate) fn write_response<T>(output: &mut Vec<u8>, response: &http::Response<T>) -> Result<()> {
    write!(
        output,
        "{} {}\r\n",
        version_text(response.version())?,
        response.status()
    )?;
    for (name, value) in response.headers() {
        write!(output, "{name}: {}\r\n", value.to_str()?)?;
    }
    write!(output, "\r\n")?;
    Ok(())
}
