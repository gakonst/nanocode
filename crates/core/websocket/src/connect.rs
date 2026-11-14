use std::{
    io::{self, IoSlice},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use nanocode_executor::TcpStream;
use nanocode_io::{AsyncRead, AsyncWrite, ReadBuf};
use nanocode_tls::rustls::{ClientConfig, pki_types::ServerName};

use crate::{
    error::{Error, Result, TlsError, UrlError},
    handshake::{IntoClientRequest, Mode, client, uri_mode},
    protocol::WebSocketConfig,
    stream::{WebSocketStream, client_async_with_config},
};

const WSS_PORT: u16 = 443;
const WS_PORT: u16 = 80;

pub enum MaybeTlsStream<S> {
    Plain(S),
    Rustls(nanocode_tls::ClientStream<S>),
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncRead for MaybeTlsStream<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_read(context, buffer),
            Self::Rustls(stream) => Pin::new(stream).poll_read(context, buffer),
        }
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncWrite for MaybeTlsStream<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_write(context, buffer),
            Self::Rustls(stream) => Pin::new(stream).poll_write(context, buffer),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffers: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_write_vectored(context, buffers),
            Self::Rustls(stream) => Pin::new(stream).poll_write_vectored(context, buffers),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            Self::Plain(stream) => stream.is_write_vectored(),
            Self::Rustls(stream) => stream.is_write_vectored(),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_flush(context),
            Self::Rustls(stream) => Pin::new(stream).poll_flush(context),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_shutdown(context),
            Self::Rustls(stream) => Pin::new(stream).poll_shutdown(context),
        }
    }
}

#[derive(Clone)]
pub enum Connector {
    Plain,
    Rustls(Arc<ClientConfig>),
}

fn domain(request: &client::Request) -> Result<String> {
    let host = request
        .uri()
        .host()
        .ok_or(Error::Url(UrlError::NoHostName))?;
    Ok(host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host)
        .to_owned())
}

pub async fn client_async_tls_with_config<R, S>(
    request: R,
    stream: S,
    config: Option<WebSocketConfig>,
    connector: Connector,
) -> Result<(WebSocketStream<MaybeTlsStream<S>>, client::Response)>
where
    R: IntoClientRequest,
    S: AsyncRead + AsyncWrite + Unpin,
{
    let request = request.into_client_request()?;
    let domain = domain(&request)?;
    let stream = match (uri_mode(request.uri())?, connector) {
        (Mode::Plain, _) => MaybeTlsStream::Plain(stream),
        (Mode::Tls, Connector::Plain) => return Err(Error::Url(UrlError::TlsFeatureNotEnabled)),
        (Mode::Tls, Connector::Rustls(tls)) => {
            let name =
                ServerName::try_from(domain).map_err(|_| Error::Tls(TlsError::InvalidDnsName))?;
            let stream = nanocode_tls::connect(tls, name, stream)
                .await
                .map_err(Error::Io)?;
            MaybeTlsStream::Rustls(stream)
        }
    };
    client_async_with_config(request, stream, config).await
}

pub async fn connect_async_tls_with_config<R: IntoClientRequest>(
    request: R,
    config: Option<WebSocketConfig>,
    disable_nagle: bool,
    connector: Connector,
) -> Result<(WebSocketStream<MaybeTlsStream<TcpStream>>, client::Response)> {
    let request = request.into_client_request()?;
    let domain = domain(&request)?;
    let port = request
        .uri()
        .port_u16()
        .or_else(|| match request.uri().scheme_str() {
            Some("wss") => Some(WSS_PORT),
            Some("ws") => Some(WS_PORT),
            _ => None,
        })
        .ok_or(Error::Url(UrlError::UnsupportedUrlScheme))?;
    let socket = TcpStream::connect(format!("{domain}:{port}")).await?;
    if disable_nagle {
        socket.set_nodelay(true)?;
    }
    client_async_tls_with_config(request, socket, config, connector).await
}
