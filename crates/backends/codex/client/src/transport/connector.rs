use std::io;

use nanocode_executor::TcpStream;
use nanocode_http::connect::{self, Alpn, Proxy, ProxyScheme};
use nanocode_http::types::Uri;
use nanocode_websocket::{
    Error, MaybeTlsStream, Result, WebSocketStream, client::Request, client::Response,
    client_async_with_config,
};

use super::deflate::InflateStream;

const WS_PORT: u16 = 80;
const WSS_PORT: u16 = 443;

pub async fn connect_async(
    request: Request,
) -> Result<(WebSocketStream<MaybeTlsStream<TcpStream>>, Response)> {
    let stream = connect_stream(request.uri()).await?;
    client_async_with_config(request, stream, None).await
}

pub(crate) async fn connect_deflate(
    request: Request,
) -> Result<(
    WebSocketStream<InflateStream<MaybeTlsStream<TcpStream>>>,
    Response,
)> {
    let stream = connect_stream(request.uri()).await?;
    client_async_with_config(request, InflateStream::new(stream), None).await
}

async fn connect_stream(uri: &Uri) -> Result<MaybeTlsStream<TcpStream>> {
    let host = uri
        .host()
        .ok_or_else(|| invalid_input("WebSocket URL has no host"))?
        .to_owned();
    let secure = match uri.scheme_str() {
        Some("wss") => true,
        Some("ws") => false,
        _ => return Err(invalid_input("unsupported WebSocket URL scheme")),
    };
    let port = uri
        .port_u16()
        .unwrap_or(if secure { WSS_PORT } else { WS_PORT });
    let proxy = proxy(&host, port, secure).map_err(Error::Io)?;
    let socket = async {
        match &proxy {
            Some(proxy) => {
                let mut socket = connect::tcp(&proxy.host, proxy.port, &[]).await?;
                connect::tunnel(&mut socket, proxy, &host, port, None).await?;
                Ok(socket)
            }
            None => connect::tcp(&host, port, &[]).await,
        }
    };
    let socket = socket.await.map_err(Error::Io)?;
    if !secure {
        return Ok(MaybeTlsStream::Plain(socket));
    }
    let stream = connect::tls(socket, &host, Alpn::None)
        .await
        .map_err(Error::Io)?;
    Ok(MaybeTlsStream::Rustls(stream))
}

fn proxy(host: &str, port: u16, secure: bool) -> io::Result<Option<Proxy>> {
    if connect::bypasses_proxy(host, port) {
        return Ok(None);
    }
    let value = if secure {
        connect::env_first(&["HTTPS_PROXY", "https_proxy"])
            .or_else(|| connect::env_first(&["HTTP_PROXY", "http_proxy"]))
    } else {
        connect::env_first(&["HTTP_PROXY", "http_proxy"])
    }
    .or_else(|| connect::env_first(&["ALL_PROXY", "all_proxy"]));
    let Some(proxy) = value.map(|value| Proxy::parse(&value)).transpose()? else {
        return Ok(None);
    };
    if proxy.scheme == ProxyScheme::Https {
        return Err(connect::invalid_input("unsupported proxy URL scheme"));
    }
    Ok(Some(proxy))
}

fn invalid_input(message: &str) -> Error {
    Error::Io(connect::invalid_input(message))
}
