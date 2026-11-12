use std::{fmt, io, str, string};

use crate::frame::OpCode;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug)]
pub enum Error {
    ConnectionClosed,
    AlreadyClosed,
    Io(io::Error),
    Tls(TlsError),
    Capacity(CapacityError),
    Protocol(ProtocolError),
    Utf8(String),
    AttackAttempt,
    Url(UrlError),
    Http(Box<http::Response<Option<Vec<u8>>>>),
    HttpFormat(http::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConnectionClosed => formatter.write_str("Connection closed normally"),
            Self::AlreadyClosed => formatter.write_str("Trying to work with closed connection"),
            Self::Io(error) => write!(formatter, "IO error: {error}"),
            Self::Tls(error) => write!(formatter, "TLS error: {error}"),
            Self::Capacity(error) => write!(formatter, "Space limit exceeded: {error}"),
            Self::Protocol(error) => write!(formatter, "WebSocket protocol error: {error}"),
            Self::Utf8(error) => write!(formatter, "UTF-8 encoding error: {error}"),
            Self::AttackAttempt => formatter.write_str("Attack attempt detected"),
            Self::Url(error) => write!(formatter, "URL error: {error}"),
            Self::Http(response) => write!(formatter, "HTTP error: {}", response.status()),
            Self::HttpFormat(error) => write!(formatter, "HTTP format error: {error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Tls(error) => Some(error),
            Self::Capacity(error) => Some(error),
            Self::Protocol(error) => Some(error),
            Self::Url(error) => Some(error),
            Self::HttpFormat(error) => Some(error),
            Self::ConnectionClosed
            | Self::AlreadyClosed
            | Self::Utf8(_)
            | Self::AttackAttempt
            | Self::Http(_) => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<TlsError> for Error {
    fn from(error: TlsError) -> Self {
        Self::Tls(error)
    }
}

impl From<CapacityError> for Error {
    fn from(error: CapacityError) -> Self {
        Self::Capacity(error)
    }
}

impl From<ProtocolError> for Error {
    fn from(error: ProtocolError) -> Self {
        Self::Protocol(error)
    }
}

impl From<UrlError> for Error {
    fn from(error: UrlError) -> Self {
        Self::Url(error)
    }
}

impl From<http::Error> for Error {
    fn from(error: http::Error) -> Self {
        Self::HttpFormat(error)
    }
}

impl From<str::Utf8Error> for Error {
    fn from(error: str::Utf8Error) -> Self {
        Self::Utf8(error.to_string())
    }
}

impl From<string::FromUtf8Error> for Error {
    fn from(error: string::FromUtf8Error) -> Self {
        Self::Utf8(error.to_string())
    }
}

impl From<http::header::InvalidHeaderValue> for Error {
    fn from(error: http::header::InvalidHeaderValue) -> Self {
        Self::HttpFormat(error.into())
    }
}

impl From<http::header::InvalidHeaderName> for Error {
    fn from(error: http::header::InvalidHeaderName) -> Self {
        Self::HttpFormat(error.into())
    }
}

impl From<http::header::ToStrError> for Error {
    fn from(error: http::header::ToStrError) -> Self {
        Self::Utf8(error.to_string())
    }
}

impl From<http::uri::InvalidUri> for Error {
    fn from(error: http::uri::InvalidUri) -> Self {
        Self::HttpFormat(error.into())
    }
}

impl From<http::status::InvalidStatusCode> for Error {
    fn from(error: http::status::InvalidStatusCode) -> Self {
        Self::HttpFormat(error.into())
    }
}

impl From<httparse::Error> for Error {
    fn from(error: httparse::Error) -> Self {
        match error {
            httparse::Error::TooManyHeaders => Self::Capacity(CapacityError::TooManyHeaders),
            error => Self::Protocol(ProtocolError::HttparseError(error)),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum CapacityError {
    TooManyHeaders,
    MessageTooLong { size: u64, max_size: usize },
}

impl fmt::Display for CapacityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyHeaders => formatter.write_str("Too many headers"),
            Self::MessageTooLong { size, max_size } => {
                write!(formatter, "Message too long: {size} > {max_size}")
            }
        }
    }
}

impl std::error::Error for CapacityError {}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum SubProtocolError {
    ServerSentSubProtocolNoneRequested,
    InvalidSubProtocol,
    NoSubProtocol,
}

impl fmt::Display for SubProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ServerSentSubProtocolNoneRequested => {
                "Server sent a subprotocol but none was requested"
            }
            Self::InvalidSubProtocol => "Server sent an invalid subprotocol",
            Self::NoSubProtocol => "Server sent no subprotocol",
        })
    }
}

impl std::error::Error for SubProtocolError {}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum ProtocolError {
    WrongHttpMethod,
    WrongHttpVersion,
    MissingConnectionUpgradeHeader,
    MissingUpgradeWebSocketHeader,
    MissingSecWebSocketVersionHeader,
    MissingSecWebSocketKey,
    InvalidSecWebSocketKey,
    SecWebSocketAcceptKeyMismatch,
    SecWebSocketSubProtocolError(SubProtocolError),
    JunkAfterRequest,
    CustomResponseSuccessful,
    InvalidHeader(Box<http::header::HeaderName>),
    HandshakeIncomplete,
    HttparseError(httparse::Error),
    SendAfterClosing,
    ReceivedAfterClosing,
    NonZeroReservedBits,
    UnmaskedFrameFromClient,
    MaskedFrameFromServer,
    FragmentedControlFrame,
    ControlFrameTooBig,
    UnexpectedContinueFrame,
    ExpectedFragment(OpCode),
    ResetWithoutClosingHandshake,
    InvalidOpcode(u8),
    InvalidCloseSequence,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongHttpMethod => {
                formatter.write_str("Unsupported HTTP method used - only GET is allowed")
            }
            Self::WrongHttpVersion => formatter.write_str("HTTP version must be 1.1 or higher"),
            Self::MissingConnectionUpgradeHeader => {
                formatter.write_str("No \"Connection: upgrade\" header")
            }
            Self::MissingUpgradeWebSocketHeader => {
                formatter.write_str("No \"Upgrade: websocket\" header")
            }
            Self::MissingSecWebSocketVersionHeader => {
                formatter.write_str("No \"Sec-WebSocket-Version: 13\" header")
            }
            Self::MissingSecWebSocketKey => formatter.write_str("No \"Sec-WebSocket-Key\" header"),
            Self::InvalidSecWebSocketKey => {
                formatter.write_str("Invalid \"Sec-WebSocket-Key\" header value")
            }
            Self::SecWebSocketAcceptKeyMismatch => {
                formatter.write_str("Key mismatch in \"Sec-WebSocket-Accept\" header")
            }
            Self::SecWebSocketSubProtocolError(error) => {
                write!(formatter, "SubProtocol error: {error}")
            }
            Self::JunkAfterRequest => formatter.write_str("Junk after client request"),
            Self::CustomResponseSuccessful => {
                formatter.write_str("Custom response must not be successful")
            }
            Self::InvalidHeader(name) => {
                write!(formatter, "Missing, duplicated or incorrect header {name}")
            }
            Self::HandshakeIncomplete => formatter.write_str("Handshake not finished"),
            Self::HttparseError(error) => write!(formatter, "httparse error: {error}"),
            Self::SendAfterClosing => formatter.write_str("Sending after closing is not allowed"),
            Self::ReceivedAfterClosing => formatter.write_str("Remote sent after having closed"),
            Self::NonZeroReservedBits => formatter.write_str("Reserved bits are non-zero"),
            Self::UnmaskedFrameFromClient => {
                formatter.write_str("Received an unmasked frame from client")
            }
            Self::MaskedFrameFromServer => {
                formatter.write_str("Received a masked frame from server")
            }
            Self::FragmentedControlFrame => formatter.write_str("Fragmented control frame"),
            Self::ControlFrameTooBig => {
                formatter.write_str("Control frame too big (payload must be 125 bytes or less)")
            }
            Self::UnexpectedContinueFrame => {
                formatter.write_str("Continue frame but nothing to continue")
            }
            Self::ExpectedFragment(opcode) => {
                write!(
                    formatter,
                    "While waiting for more fragments received: {opcode}"
                )
            }
            Self::ResetWithoutClosingHandshake => {
                formatter.write_str("Connection reset without closing handshake")
            }
            Self::InvalidOpcode(opcode) => {
                write!(formatter, "Encountered invalid opcode: {opcode}")
            }
            Self::InvalidCloseSequence => formatter.write_str("Invalid close sequence"),
        }
    }
}

impl std::error::Error for ProtocolError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::HttparseError(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum UrlError {
    TlsFeatureNotEnabled,
    NoHostName,
    UnsupportedUrlScheme,
    EmptyHostName,
    NoPathOrQuery,
}

impl fmt::Display for UrlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::TlsFeatureNotEnabled => "TLS support not compiled in",
            Self::NoHostName => "No host name in the URL",
            Self::UnsupportedUrlScheme => "URL scheme not supported",
            Self::EmptyHostName => "URL contains empty host name",
            Self::NoPathOrQuery => "No path/query in URL",
        })
    }
}

impl std::error::Error for UrlError {}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TlsError {
    InvalidDnsName,
}

impl fmt::Display for TlsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDnsName => formatter.write_str("Invalid DNS name"),
        }
    }
}

impl std::error::Error for TlsError {}
