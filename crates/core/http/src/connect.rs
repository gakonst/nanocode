use std::{
    collections::VecDeque, env, future::Future, io, net::SocketAddr, sync::Arc, time::Duration,
};

use nanocode_platform::sleep_until;
use nanocode_sync::{Duplex, SwapOption};
use nanocode_sync::{StreamExt, stream::FuturesUnordered};
use nanocode_sys::base64::STANDARD;
use nanocode_tls::{
    ClientStream,
    rustls::{ClientConfig, pki_types::ServerName},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
};

const CONNECTION_ATTEMPT_DELAY: Duration = Duration::from_millis(250);
const CONNECT_RESPONSE_BYTES_MAX: usize = 8_192;
const HTTP_PORT: u16 = 80;
const HTTPS_PORT: u16 = 443;
const SOCKS_PORT: u16 = 1080;
const ALPN_HTTP1: &[u8] = b"http/1.1";
const SOCKS_VERSION: u8 = 0x05;
const SOCKS_NO_AUTHENTICATION: u8 = 0x00;
const SOCKS_USERNAME_PASSWORD: u8 = 0x02;
const SOCKS_NO_METHOD: u8 = 0xff;
const SOCKS_CONNECT: u8 = 0x01;
const SOCKS_RESERVED: u8 = 0x00;
const SOCKS_SUCCEEDED: u8 = 0x00;
const SOCKS_IPV4: u8 = 0x01;
const SOCKS_DOMAIN: u8 = 0x03;
const SOCKS_IPV6: u8 = 0x04;
const SOCKS_AUTHENTICATION_VERSION: u8 = 0x01;
const SOCKS_IPV4_BYTES: usize = 4;
const SOCKS_IPV6_BYTES: usize = 16;
const SOCKS_PORT_BYTES: usize = 2;
const HEX_RADIX: u32 = 16;
const NIBBLE_BITS: u32 = 4;
const SOCKS_METHOD_REPLY_BYTES: usize = 2;
const SOCKS_REPLY_HEADER_BYTES: usize = 4;
const SOCKS_AUTHENTICATION_REPLY_BYTES: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProxyScheme {
    Http,
    Https,
    Socks5,
    Socks5h,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyAuth {
    pub username: String,
    pub password: String,
}

impl ProxyAuth {
    #[must_use]
    pub fn basic(&self) -> String {
        format!(
            "Basic {}",
            STANDARD.encode(format!("{}:{}", self.username, self.password))
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proxy {
    pub scheme: ProxyScheme,
    pub host: String,
    pub port: u16,
    pub auth: Option<ProxyAuth>,
}

impl Proxy {
    pub fn parse(value: &str) -> io::Result<Self> {
        let value = value.trim();
        let value = if value.contains("://") {
            value.to_owned()
        } else {
            format!("http://{value}")
        };
        let uri = value
            .parse::<http::Uri>()
            .map_err(|_| invalid_input(format!("invalid proxy URL {value:?}")))?;
        let scheme = match uri.scheme_str() {
            Some("http") => ProxyScheme::Http,
            Some("https") => ProxyScheme::Https,
            Some("socks5") => ProxyScheme::Socks5,
            Some("socks5h") => ProxyScheme::Socks5h,
            _ => return Err(invalid_input("unsupported proxy URL scheme")),
        };
        let authority = uri
            .authority()
            .ok_or_else(|| invalid_input("proxy URL has no authority"))?
            .as_str();
        let (userinfo, host_port) = authority
            .rsplit_once('@')
            .map_or((None, authority), |(userinfo, host_port)| {
                (Some(userinfo), host_port)
            });
        let host_uri = format!("http://{host_port}")
            .parse::<http::Uri>()
            .map_err(|_| invalid_input("proxy URL has an invalid host"))?;
        let host = host_uri
            .host()
            .ok_or_else(|| invalid_input("proxy URL has no host"))?
            .to_owned();
        let port = host_uri.port_u16().unwrap_or(match scheme {
            ProxyScheme::Http => HTTP_PORT,
            ProxyScheme::Https => HTTPS_PORT,
            ProxyScheme::Socks5 | ProxyScheme::Socks5h => SOCKS_PORT,
        });
        let auth = userinfo.map(parse_userinfo).transpose()?;
        Ok(Self {
            scheme,
            host,
            port,
            auth,
        })
    }

    #[must_use]
    pub fn authority(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    pub fn from_env(host: &str, port: u16, secure: bool) -> io::Result<Option<Self>> {
        if bypasses_proxy(host, port) {
            return Ok(None);
        }
        let value = if secure {
            env_first(&["HTTPS_PROXY", "https_proxy"])
        } else if env::var_os("REQUEST_METHOD").is_some() {
            None
        } else {
            env_first(&["HTTP_PROXY", "http_proxy"])
        }
        .or_else(|| env_first(&["ALL_PROXY", "all_proxy"]));
        value.map(|value| Self::parse(&value)).transpose()
    }
}

#[must_use]
pub fn env_first(keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| env::var(key).ok())
        .filter(|value| !value.is_empty())
}

#[must_use]
pub fn bypasses_proxy(host: &str, port: u16) -> bool {
    let Some(no_proxy) = env_first(&["NO_PROXY", "no_proxy"]) else {
        return false;
    };
    let no_proxy = no_proxy.trim();
    if no_proxy == "*" {
        return true;
    }
    let host = normalize_host(host);
    let address = host.parse::<std::net::IpAddr>().ok();
    no_proxy.split(',').any(|token| {
        let token = token.trim();
        if token.is_empty() {
            return false;
        }
        if let Some(address) = address
            && let Some(matched) = cidr_contains(token, address)
        {
            return matched;
        }
        let (token_host, token_port) = split_host_port(token);
        if token_port.is_some_and(|token_port| token_port != port) {
            return false;
        }
        let token_host = normalize_host(token_host);
        let token_host = token_host.strip_prefix("*.").unwrap_or(token_host);
        let token_host = token_host.strip_prefix('.').unwrap_or(token_host);
        host.eq_ignore_ascii_case(token_host)
            || host
                .len()
                .checked_sub(token_host.len() + 1)
                .is_some_and(|start| {
                    host.as_bytes()[start] == b'.'
                        && host[start + 1..].eq_ignore_ascii_case(token_host)
                })
    })
}

fn cidr_contains(token: &str, address: std::net::IpAddr) -> Option<bool> {
    let (network, prefix) = token.split_once('/').unwrap_or((token, ""));
    let network = normalize_host(network).parse::<std::net::IpAddr>().ok()?;
    let bits = match (network, address) {
        (std::net::IpAddr::V4(network), std::net::IpAddr::V4(address)) => (
            u128::from(network.to_bits()),
            u128::from(address.to_bits()),
            u32::BITS,
        ),
        (std::net::IpAddr::V6(network), std::net::IpAddr::V6(address)) => {
            (network.to_bits(), address.to_bits(), u128::BITS)
        }
        _ => return Some(false),
    };
    let (network, address, width) = bits;
    let prefix = if prefix.is_empty() {
        width
    } else {
        prefix
            .parse::<u32>()
            .ok()
            .filter(|prefix| *prefix <= width)?
    };
    let shift = width - prefix;
    Some(prefix == 0 || network >> shift == address >> shift)
}

fn split_host_port(token: &str) -> (&str, Option<u16>) {
    if token.starts_with('[') {
        if let Some(close) = token.find(']') {
            let host = &token[..=close];
            let port = token[close + 1..]
                .strip_prefix(':')
                .and_then(|port| port.parse().ok());
            return (host, port);
        }
        return (token, None);
    }
    if token.matches(':').count() == 1 {
        let (host, port) = token.rsplit_once(':').unwrap_or((token, ""));
        return (host, port.parse().ok());
    }
    (token, None)
}

#[must_use]
pub fn normalize_host(host: &str) -> &str {
    host.strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host)
}

fn parse_userinfo(userinfo: &str) -> io::Result<ProxyAuth> {
    let (username, password) = userinfo.split_once(':').unwrap_or((userinfo, ""));
    Ok(ProxyAuth {
        username: percent_decode(username)?,
        password: percent_decode(password)?,
    })
}

fn percent_decode(value: &str) -> io::Result<String> {
    let mut output = Vec::with_capacity(value.len());
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let high = bytes
                .next()
                .ok_or_else(|| invalid_input("invalid proxy credential encoding"))?;
            let low = bytes
                .next()
                .ok_or_else(|| invalid_input("invalid proxy credential encoding"))?;
            output.push((from_hex(high)? << NIBBLE_BITS) | from_hex(low)?);
        } else {
            output.push(byte);
        }
    }
    String::from_utf8(output).map_err(|_| invalid_input("proxy credentials are not UTF-8"))
}

fn from_hex(byte: u8) -> io::Result<u8> {
    char::from(byte)
        .to_digit(HEX_RADIX)
        .and_then(|digit| u8::try_from(digit).ok())
        .ok_or_else(|| invalid_input("invalid proxy credential encoding"))
}

pub async fn tunnel<S>(
    stream: &mut S,
    proxy: &Proxy,
    host: &str,
    port: u16,
    user_agent: Option<&str>,
) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    match proxy.scheme {
        ProxyScheme::Http | ProxyScheme::Https => {
            http_connect(stream, host, port, proxy.auth.as_ref(), user_agent).await
        }
        ProxyScheme::Socks5 | ProxyScheme::Socks5h => {
            socks5_connect(stream, host, port, proxy.auth.as_ref()).await
        }
    }
}

async fn http_connect<S>(
    stream: &mut S,
    host: &str,
    port: u16,
    auth: Option<&ProxyAuth>,
    user_agent: Option<&str>,
) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let authority = format!("{host}:{port}");
    let mut request = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
    let auth = auth.map(ProxyAuth::basic);
    match (user_agent, auth) {
        (None, None) => {}
        (None, Some(auth)) => request.push_str(&format!("Proxy-Authorization: {auth}\r\n")),
        (Some(user_agent), auth) => {
            request.push_str(&format!("user-agent: {user_agent}\r\n"));
            if let Some(auth) = auth {
                request.push_str(&format!("proxy-authorization: {auth}\r\n"));
            }
        }
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).await?;
    stream.flush().await?;
    let response = read_connect_response(stream).await?;
    let status = std::str::from_utf8(&response)
        .ok()
        .and_then(|response| response.lines().next())
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| http::StatusCode::from_bytes(status.as_bytes()).ok())
        .ok_or_else(|| invalid_input("HTTP CONNECT response has no valid status"))?;
    if !status.is_success() {
        return Err(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            format!("HTTP CONNECT failed with status {status}"),
        ));
    }
    Ok(())
}

async fn read_connect_response<S>(stream: &mut S) -> io::Result<Vec<u8>>
where
    S: AsyncRead + Unpin,
{
    let mut response = Vec::new();
    loop {
        if response.len() >= CONNECT_RESPONSE_BYTES_MAX {
            return Err(invalid_input("HTTP CONNECT response is too large"));
        }
        let byte = stream.read_u8().await?;
        response.push(byte);
        if response.ends_with(b"\r\n\r\n") {
            return Ok(response);
        }
    }
}

async fn socks5_connect<S>(
    stream: &mut S,
    host: &str,
    port: u16,
    auth: Option<&ProxyAuth>,
) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let offered: &[u8] = if auth.is_some() {
        &[SOCKS_NO_AUTHENTICATION, SOCKS_USERNAME_PASSWORD]
    } else {
        &[SOCKS_NO_AUTHENTICATION]
    };
    let offered_count = u8::try_from(offered.len()).expect("SOCKS5 offers at most two methods");
    let greeting = [SOCKS_VERSION, offered_count];
    let mut methods = Vec::with_capacity(greeting.len() + offered.len());
    methods.extend_from_slice(&greeting);
    methods.extend_from_slice(offered);
    stream.write_all(&methods).await?;
    stream.flush().await?;
    let mut choice = [0; SOCKS_METHOD_REPLY_BYTES];
    stream.read_exact(&mut choice).await?;
    if choice[0] != SOCKS_VERSION {
        return Err(invalid_input("SOCKS5 proxy returned an invalid version"));
    }
    match choice[1] {
        SOCKS_NO_AUTHENTICATION => {}
        SOCKS_USERNAME_PASSWORD => socks5_authenticate(stream, auth).await?,
        SOCKS_NO_METHOD => return Err(invalid_input("SOCKS5 proxy rejected authentication")),
        _ => return Err(invalid_input("SOCKS5 proxy selected an unsupported method")),
    }
    let host = host.as_bytes();
    let host_length = u8::try_from(host.len())
        .map_err(|_| invalid_input("SOCKS5 destination host is too long"))?;
    let prefix = [
        SOCKS_VERSION,
        SOCKS_CONNECT,
        SOCKS_RESERVED,
        SOCKS_DOMAIN,
        host_length,
    ];
    let mut request = Vec::with_capacity(prefix.len() + host.len() + SOCKS_PORT_BYTES);
    request.extend_from_slice(&prefix);
    request.extend_from_slice(host);
    request.extend_from_slice(&port.to_be_bytes());
    stream.write_all(&request).await?;
    stream.flush().await?;
    let mut header = [0; SOCKS_REPLY_HEADER_BYTES];
    stream.read_exact(&mut header).await?;
    let [version, reply, _, address_type] = header;
    if version != SOCKS_VERSION || reply != SOCKS_SUCCEEDED {
        return Err(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            format!("SOCKS5 connection failed with code {reply}"),
        ));
    }
    let address_bytes = match address_type {
        SOCKS_IPV4 => SOCKS_IPV4_BYTES,
        SOCKS_DOMAIN => usize::from(stream.read_u8().await?),
        SOCKS_IPV6 => SOCKS_IPV6_BYTES,
        _ => {
            return Err(invalid_input(
                "SOCKS5 proxy returned an invalid address type",
            ));
        }
    };
    let mut remainder = vec![0; address_bytes + SOCKS_PORT_BYTES];
    stream.read_exact(&mut remainder).await?;
    Ok(())
}

async fn socks5_authenticate<S>(stream: &mut S, auth: Option<&ProxyAuth>) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let auth = auth.ok_or_else(|| invalid_input("SOCKS5 proxy requested credentials"))?;
    let username = auth.username.as_bytes();
    let password = auth.password.as_bytes();
    let username_length = u8::try_from(username.len())
        .map_err(|_| invalid_input("SOCKS5 proxy username is too long"))?;
    let password_length = u8::try_from(password.len())
        .map_err(|_| invalid_input("SOCKS5 proxy password is too long"))?;
    let prefix = [SOCKS_AUTHENTICATION_VERSION, username_length];
    let mut request =
        Vec::with_capacity(prefix.len() + username.len() + size_of::<u8>() + password.len());
    request.extend_from_slice(&prefix);
    request.extend_from_slice(username);
    request.push(password_length);
    request.extend_from_slice(password);
    stream.write_all(&request).await?;
    stream.flush().await?;
    let mut response = [0; SOCKS_AUTHENTICATION_REPLY_BYTES];
    stream.read_exact(&mut response).await?;
    if response != [SOCKS_AUTHENTICATION_VERSION, SOCKS_SUCCEEDED] {
        return Err(invalid_input("SOCKS5 authentication failed"));
    }
    Ok(())
}

pub async fn tcp(host: &str, port: u16, resolved: &[SocketAddr]) -> io::Result<TcpStream> {
    let addresses = if resolved.is_empty() {
        let literal = normalize_host(host);
        match literal.parse::<std::net::IpAddr>() {
            Ok(address) => vec![SocketAddr::new(address, port)],
            Err(_) => tokio::net::lookup_host((literal, port)).await?.collect(),
        }
    } else {
        resolved.to_vec()
    };
    let stream = happy_eyeballs(addresses, TcpStream::connect).await?;
    stream.set_nodelay(true)?;
    Ok(stream)
}

pub async fn happy_eyeballs<T, F, Fut>(addresses: Vec<SocketAddr>, mut connect: F) -> io::Result<T>
where
    F: FnMut(SocketAddr) -> Fut,
    Fut: Future<Output = io::Result<T>>,
{
    let mut addresses = interleave(addresses);
    let Some(first) = addresses.pop_front() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "host did not resolve to any address",
        ));
    };
    let mut attempts = FuturesUnordered::new();
    attempts.push(connect(first));
    let mut next_attempt = nanocode_platform::now() + CONNECTION_ATTEMPT_DELAY;
    let mut last_error = None;
    loop {
        if addresses.is_empty() {
            match attempts.next().await {
                Some(Ok(stream)) => return Ok(stream),
                Some(Err(error)) => last_error = Some(error),
                None => break,
            }
            continue;
        }
        nanocode_sync::select! {
            result = attempts.next() => match result {
                Some(Ok(stream)) => return Ok(stream),
                Some(Err(error)) => last_error = Some(error),
                None => {}
            },
            () = sleep_until(next_attempt) => {}
        }
        if let Some(address) = addresses.pop_front() {
            attempts.push(connect(address));
            next_attempt = nanocode_platform::now() + CONNECTION_ATTEMPT_DELAY;
        }
    }
    Err(last_error.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::ConnectionRefused,
            "all connection attempts failed",
        )
    }))
}

fn interleave(addresses: Vec<SocketAddr>) -> VecDeque<SocketAddr> {
    let mut addresses = addresses.into_iter();
    let Some(first) = addresses.next() else {
        return VecDeque::new();
    };
    let first_is_ipv4 = first.is_ipv4();
    let mut preferred = VecDeque::from([first]);
    let mut alternate = VecDeque::new();
    for address in addresses {
        if address.is_ipv4() == first_is_ipv4 {
            preferred.push_back(address);
        } else {
            alternate.push_back(address);
        }
    }
    let mut interleaved = VecDeque::with_capacity(preferred.len() + alternate.len());
    while !preferred.is_empty() || !alternate.is_empty() {
        interleaved.extend(preferred.pop_front());
        interleaved.extend(alternate.pop_front());
    }
    interleaved
}

struct Derived {
    base: Arc<ClientConfig>,
    http1: Arc<ClientConfig>,
}

static HTTP1: SwapOption<Derived> = SwapOption::const_empty();

async fn http1_config() -> io::Result<Arc<ClientConfig>> {
    let base = nanocode_tls::native_client_config().await?;
    if let Some(derived) = &*HTTP1.load()
        && Arc::ptr_eq(&derived.base, &base)
    {
        return Ok(Arc::clone(&derived.http1));
    }
    let mut config = ClientConfig::clone(&base);
    config.alpn_protocols = vec![ALPN_HTTP1.to_vec()];
    let http1 = Arc::new(config);
    HTTP1.store(Some(Arc::new(Derived {
        base,
        http1: Arc::clone(&http1),
    })));
    Ok(http1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alpn {
    None,
    Http1,
}

pub async fn tls<S>(stream: S, host: &str, alpn: Alpn) -> io::Result<ClientStream<S>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let config = match alpn {
        Alpn::None => nanocode_tls::native_client_config().await?,
        Alpn::Http1 => http1_config().await?,
    };
    let name = ServerName::try_from(normalize_host(host).to_owned())
        .map_err(|_| invalid_input(format!("invalid TLS server name {host:?}")))?;
    nanocode_tls::connect(config, name, stream)
        .await
        .map_err(|error| {
            let rejected = error
                .get_ref()
                .and_then(|inner| inner.downcast_ref::<nanocode_tls::rustls::Error>())
                .is_some_and(|tls| matches!(tls, nanocode_tls::rustls::Error::InvalidCertificate(_)));
            if rejected {
                io::Error::new(io::ErrorKind::InvalidData, UntrustedCertificate(error))
            } else {
                error
            }
        })
}

#[derive(Debug)]
pub struct UntrustedCertificate(io::Error);

impl std::fmt::Display for UntrustedCertificate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(
            "the server's TLS certificate is not trusted; if a proxy intercepts TLS, set SSL_CERT_FILE to its CA bundle",
        )
    }
}

impl std::error::Error for UntrustedCertificate {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

pub fn untrusted_certificate(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut next = Some(error);
    while let Some(cause) = next {
        let untrusted = cause.is::<UntrustedCertificate>()
            || cause
                .downcast_ref::<io::Error>()
                .and_then(io::Error::get_ref)
                .is_some_and(|inner| inner.is::<UntrustedCertificate>());
        if untrusted {
            return true;
        }
        next = cause.source();
    }
    false
}

pub async fn stream(
    host: &str,
    port: u16,
    secure: bool,
    proxy: Option<&Proxy>,
    resolved: &[SocketAddr],
    user_agent: Option<&str>,
    alpn: Alpn,
) -> io::Result<Box<dyn Duplex>> {
    let Some(proxy) = proxy else {
        let socket = tcp(host, port, resolved).await?;
        return Ok(if secure {
            Box::new(tls(socket, host, alpn).await?)
        } else {
            Box::new(socket)
        });
    };
    let socket = tcp(&proxy.host, proxy.port, &[]).await?;
    let mut socket: Box<dyn Duplex> = if proxy.scheme == ProxyScheme::Https {
        Box::new(tls(socket, &proxy.host, Alpn::Http1).await?)
    } else {
        Box::new(socket)
    };
    let tunnels = secure || matches!(proxy.scheme, ProxyScheme::Socks5 | ProxyScheme::Socks5h);
    if !tunnels {
        return Ok(socket);
    }
    tunnel(&mut socket, proxy, host, port, user_agent).await?;
    Ok(if secure {
        Box::new(tls(socket, host, alpn).await?)
    } else {
        socket
    })
}

pub fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}
