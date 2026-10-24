use nanocode_sync::Atom;
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use rustls::{
    ClientConfig, RootCertStore, ServerConfig,
    crypto::CryptoProvider,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName},
};
use std::{
    collections::HashMap,
    error::Error,
    future::Future,
    io,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf},
    net::TcpStream,
    task::JoinHandle,
};
use tokio_rustls::{TlsAcceptor, TlsConnector};

const MAX_HEAD: usize = 8 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Connections one proxy serves at once; more are refused with 503.
const MAX_CONNECTIONS: usize = 1024;
/// How long a client may take to send its request head.
const HEAD_TIMEOUT: Duration = Duration::from_secs(30);
/// How long the accept loop waits to tell an extra client the proxy is busy.
const BUSY_TIMEOUT: Duration = Duration::from_secs(1);

fn other(error: impl Into<Box<dyn Error + Send + Sync>>) -> io::Error {
    io::Error::other(error)
}

#[derive(Clone)]
pub struct Injection {
    pub host: String,
    pub header: String,
    pub value: String,
}

impl Injection {
    pub fn is_valid(&self) -> bool {
        let token = |c: u8| c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&c);
        !self.host.is_empty()
            && !self.header.is_empty()
            && self.header.bytes().all(token)
            && !self.value.bytes().any(|c| matches!(c, b'\r' | b'\n' | 0))
    }
}

pub struct Tls {
    issuer: Issuer<'static, KeyPair>,
    ca_der: CertificateDer<'static>,
    ca_pem: String,
    upstream: Arc<ClientConfig>,
    leaves: Atom<HashMap<String, Arc<ServerConfig>>>,
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

impl Tls {
    pub fn new(name: &str) -> io::Result<Self> {
        let found = rustls_native_certs::load_native_certs();
        let mut roots = RootCertStore::empty();
        let (added, _) = roots.add_parsable_certificates(found.certs);
        if added == 0 {
            return Err(other(format!(
                "no trusted root certificates found ({} errors loading them)",
                found.errors.len()
            )));
        }
        Self::with_roots(name, roots)
    }

    pub fn with_roots(name: &str, roots: RootCertStore) -> io::Result<Self> {
        let key = KeyPair::generate().map_err(other)?;
        let mut params = CertificateParams::default();
        params.distinguished_name.push(DnType::CommonName, name);
        params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        let certificate = params.self_signed(&key).map_err(other)?;
        let mut upstream = ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .map_err(other)?
            .with_root_certificates(roots)
            .with_no_client_auth();
        upstream.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Self {
            ca_pem: certificate.pem(),
            ca_der: certificate.der().clone(),
            issuer: Issuer::new(params, key),
            upstream: Arc::new(upstream),
            leaves: Atom::new(HashMap::new()),
        })
    }

    pub fn ca_pem(&self) -> &str {
        &self.ca_pem
    }

    pub const fn ca_der(&self) -> &CertificateDer<'static> {
        &self.ca_der
    }

    fn server(&self, host: &str) -> io::Result<Arc<ServerConfig>> {
        if let Some(config) = self.leaves.read(|leaves| leaves.get(host).cloned()) {
            return Ok(config);
        }
        let key = KeyPair::generate().map_err(other)?;
        let mut params = CertificateParams::new(vec![host.to_owned()]).map_err(other)?;
        params.distinguished_name.push(DnType::CommonName, host);
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let leaf = params.signed_by(&key, &self.issuer).map_err(other)?;
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
        let mut config = ServerConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .map_err(other)?
            .with_no_client_auth()
            .with_single_cert(vec![leaf.der().clone(), self.ca_der.clone()], key)
            .map_err(other)?;
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        let config = Arc::new(config);
        Ok(self.leaves.update(|leaves| {
            Arc::clone(
                leaves
                    .entry(host.to_owned())
                    .or_insert_with(|| Arc::clone(&config)),
            )
        }))
    }
}

pub struct Allowlist {
    pub hosts: Vec<String>,
    /// Host patterns, matched like `hosts`, that may reach non-public addresses (see
    /// [`is_public`]) when dialed directly. Every other host is resolved, and the proxy
    /// connects only to a public address it found, so neither an allowlisted name that resolves
    /// to, say, 127.0.0.1 or the cloud metadata service nor an IP literal reaches them. Hosts
    /// reached through `tailnet` are not checked: the SOCKS server resolves them.
    pub private_hosts: Vec<String>,
    pub injections: Vec<Injection>,
    pub tls: Arc<Tls>,
    pub tailnet: Option<SocketAddr>,
    pub refusal: fn(&str) -> String,
}

/// Whether the proxy may dial `address` for a host outside `Allowlist::private_hosts`: not
/// an address of this machine or its networks in the IANA special-purpose registries (RFC 6890):
/// unspecified and "this network" (0.0.0.0/8, ::), loopback (127.0.0.0/8, ::1), private
/// (10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16) and unique local (fc00::/7, which holds AWS's
/// IPv6 metadata address), link-local (169.254.0.0/16, which holds the cloud metadata service,
/// and fe80::/10), multicast and broadcast. IPv4-mapped IPv6 addresses are judged as IPv4.
/// Shared address space (100.64.0.0/10) stays reachable: Tailscale addresses its devices there.
#[must_use]
pub const fn is_public(address: IpAddr) -> bool {
    match address.to_canonical() {
        IpAddr::V4(v4) => {
            !(v4.octets()[0] == 0
                || v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_broadcast())
        }
        IpAddr::V6(v6) => {
            !(v6.is_unspecified()
                || v6.is_loopback()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
                || v6.is_multicast())
        }
    }
}

#[derive(Clone, Default)]
struct Log {
    denied: Arc<Atom<Vec<String>>>,
    unreachable: Arc<Atom<Vec<String>>>,
}

pub struct Egress {
    port: u16,
    socket: Option<PathBuf>,
    log: Log,
    task: JoinHandle<()>,
}

impl Drop for Egress {
    fn drop(&mut self) {
        self.task.abort();
        if let Some(socket) = &self.socket {
            std::fs::remove_file(socket).ok();
        }
    }
}

struct Rules {
    allowlist: Allowlist,
    log: Log,
}

trait Client: AsyncRead + AsyncWrite + Unpin + Send + 'static {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + 'static> Client for T {}

impl Egress {
    pub async fn tcp(allowlist: Allowlist) -> io::Result<Self> {
        let listener = Arc::new(tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?);
        let port = listener.local_addr()?.port();
        Ok(Self::spawn(port, allowlist, move || {
            let listener = Arc::clone(&listener);
            async move { listener.accept().await.ok().map(|(client, _)| client) }
        }))
    }

    pub fn unix(path: &Path, allowlist: Allowlist) -> io::Result<Self> {
        std::fs::remove_file(path).ok();
        let listener = Arc::new(tokio::net::UnixListener::bind(path)?);
        let mut egress = Self::spawn(0, allowlist, move || {
            let listener = Arc::clone(&listener);
            async move { listener.accept().await.ok().map(|(client, _)| client) }
        });
        egress.socket = Some(path.to_owned());
        Ok(egress)
    }

    pub const fn port(&self) -> u16 {
        self.port
    }

    pub fn denied(&self) -> Vec<String> {
        self.log.denied.get().as_ref().clone()
    }

    pub fn unreachable(&self) -> Vec<String> {
        self.log.unreachable.get().as_ref().clone()
    }

    fn spawn<C, F, A>(port: u16, mut allowlist: Allowlist, mut accept: A) -> Self
    where
        C: Client,
        F: Future<Output = Option<C>> + Send,
        A: FnMut() -> F + Send + 'static,
    {
        allowlist.injections.retain(Injection::is_valid);
        let log = Log::default();
        let rules = Arc::new(Rules {
            allowlist,
            log: log.clone(),
        });
        let task = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            while let Some(mut client) = accept().await {
                while connections.try_join_next().is_some() {}
                if connections.len() >= MAX_CONNECTIONS {
                    // Refuse rather than wait: open tunnels must not stall
                    // new connections behind them.
                    let busy = respond(&mut client, "503 Service Unavailable");
                    drop(tokio::time::timeout(BUSY_TIMEOUT, busy).await);
                    continue;
                }
                connections.spawn(serve(client, Arc::clone(&rules)));
            }
        });
        Self {
            port,
            socket: None,
            log,
            task,
        }
    }
}

async fn serve<C: Client>(mut client: C, rules: Arc<Rules>) {
    let Ok(Some((head, end))) = tokio::time::timeout(HEAD_TIMEOUT, read_head(&mut client)).await
    else {
        return;
    };
    let line = head.split(|&b| b == b'\r').next().unwrap_or_default();
    let line = std::str::from_utf8(line).unwrap_or_default();
    let mut parts = line.split(' ');
    let (Some(method), Some(target), Some(version)) = (parts.next(), parts.next(), parts.next())
    else {
        respond(&mut client, "400 Bad Request").await;
        return;
    };
    let rest = &head[end..];
    if method == "CONNECT" {
        connect(client, &rules, target, rest).await;
    } else if let Some(url) = target
        .get(..7)
        .filter(|scheme| scheme.eq_ignore_ascii_case("http://"))
        .map(|_| &target[7..])
    {
        let line = (method.to_owned(), version.to_owned());
        forward(client, &rules, url, line, &head[..end], rest).await;
    } else {
        respond(&mut client, "405 Method Not Allowed").await;
    }
}

async fn read_head<S: AsyncRead + Unpin>(stream: &mut S) -> Option<(Vec<u8>, usize)> {
    let mut head = Vec::new();
    let mut buffer = [0u8; 1024];
    loop {
        if let Some(end) = head.windows(4).position(|w| w == b"\r\n\r\n") {
            return Some((head, end + 4));
        }
        match stream.read(&mut buffer).await {
            Ok(read) if read > 0 && head.len() + read <= MAX_HEAD => {
                head.extend_from_slice(&buffer[..read]);
            }
            _ => return None,
        }
    }
}

async fn respond<S: AsyncWrite + Unpin>(client: &mut S, status: &str) {
    respond_text(client, status, "").await;
}

/// Answer `status` with a plain-text `body` (none when empty) and close.
async fn respond_text<S: AsyncWrite + Unpin>(client: &mut S, status: &str, body: &str) {
    let content_type = if body.is_empty() {
        ""
    } else {
        "Content-Type: text/plain\r\n"
    };
    let response = format!(
        "HTTP/1.1 {status}\r\n{content_type}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    client.write_all(response.as_bytes()).await.ok();
}

fn split_authority(authority: &str, default: Option<u16>) -> Option<(String, u16)> {
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !authority.ends_with(']') => (host, port.parse().ok()?),
        _ => (authority, default?),
    };
    (!host.is_empty() && !host.contains(['@', '/', ' '])).then(|| (host.to_owned(), port))
}

async fn allowed<S: AsyncWrite + Unpin>(client: &mut S, rules: &Rules, host: &str) -> bool {
    if rules
        .allowlist
        .hosts
        .iter()
        .any(|pattern| host_matches(pattern, host))
    {
        return true;
    }
    let body = (rules.allowlist.refusal)(host);
    rules
        .log
        .denied
        .update(|denied| denied.push(host.to_owned()));
    respond_text(client, "403 Forbidden", &body).await;
    false
}

async fn unreachable<S: AsyncWrite + Unpin>(client: &mut S, rules: &Rules, host: String) {
    rules
        .log
        .unreachable
        .update(|unreachable| unreachable.push(host.clone()));
    respond(client, "502 Bad Gateway").await;
}

enum Dialed {
    Connected(TcpStream),
    /// The host resolved only to addresses `is_public` refuses.
    Private,
    Failed,
}

async fn dial(rules: &Rules, host: &str, port: u16) -> Dialed {
    let name = host.trim_end_matches('.').to_ascii_lowercase();
    let connect = async {
        match rules.allowlist.tailnet {
            Some(server) if name.ends_with(".ts.net") => socks(server, &name, port).await.map(Some),
            _ => direct(rules, host, port).await,
        }
    };
    match tokio::time::timeout(CONNECT_TIMEOUT, connect).await {
        Ok(Ok(Some(stream))) => Dialed::Connected(stream),
        Ok(Ok(None)) => Dialed::Private,
        _ => Dialed::Failed,
    }
}

/// Resolves `host` once and connects to the first address it may reach, so a name that
/// resolves again to another address (DNS rebinding) cannot slip past the check. `None` when
/// every address was refused.
async fn direct(rules: &Rules, host: &str, port: u16) -> io::Result<Option<TcpStream>> {
    let private = rules
        .allowlist
        .private_hosts
        .iter()
        .any(|pattern| host_matches(pattern, host));
    let literal = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    let mut failure = None;
    for address in tokio::net::lookup_host((literal, port)).await? {
        if !private && !is_public(address.ip()) {
            continue;
        }
        match TcpStream::connect(address).await {
            Ok(stream) => return Ok(Some(stream)),
            Err(error) => failure = Some(error),
        }
    }
    failure.map_or(Ok(None), Err)
}

async fn upstream<S: AsyncWrite + Unpin>(
    client: &mut S,
    rules: &Rules,
    host: String,
    port: u16,
) -> Option<TcpStream> {
    match dial(rules, &host, port).await {
        Dialed::Connected(stream) => Some(stream),
        Dialed::Private => {
            let body = format!(
                "{host} resolves only to private, loopback or link-local addresses, which this proxy does not reach."
            );
            rules
                .log
                .unreachable
                .update(|unreachable| unreachable.push(host.clone()));
            respond_text(client, "403 Forbidden", &body).await;
            None
        }
        Dialed::Failed => {
            unreachable(client, rules, host).await;
            None
        }
    }
}

async fn socks(server: SocketAddr, host: &str, port: u16) -> io::Result<TcpStream> {
    let refused = |what: &str| other(format!("SOCKS5 {what}"));
    let length = u8::try_from(host.len()).map_err(|_| refused("host too long"))?;
    let mut stream = TcpStream::connect(server).await?;
    stream.write_all(&[5, 1, 0]).await?;
    let mut choice = [0u8; 2];
    stream.read_exact(&mut choice).await?;
    if choice != [5, 0] {
        return Err(refused("server wants authentication"));
    }
    let mut request = vec![5, 1, 0, 3, length];
    request.extend_from_slice(host.as_bytes());
    request.extend_from_slice(&port.to_be_bytes());
    stream.write_all(&request).await?;
    let mut reply = [0u8; 4];
    stream.read_exact(&mut reply).await?;
    if reply[..2] != [5, 0] {
        return Err(refused("connect failed"));
    }
    let address = match reply[3] {
        1 => 4,
        4 => 16,
        3 => usize::from(stream.read_u8().await?),
        _ => return Err(refused("reply malformed")),
    };
    let mut bound = vec![0u8; address + 2];
    stream.read_exact(&mut bound).await?;
    Ok(stream)
}

async fn connect<C: Client>(mut client: C, rules: &Rules, authority: &str, rest: &[u8]) {
    let Some((host, port)) = split_authority(authority, None) else {
        respond(&mut client, "400 Bad Request").await;
        return;
    };
    if !allowed(&mut client, rules, &host).await {
        return;
    }
    let injection = rules
        .allowlist
        .injections
        .iter()
        .find(|injection| host_matches(&injection.host, &host));
    let Some(mut upstream) = upstream(&mut client, rules, host.clone(), port).await else {
        return;
    };
    if let Some(injection) = injection {
        intercept(client, rules, host, upstream, injection, rest).await;
        return;
    }
    if client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await
        .is_err()
        || upstream.write_all(rest).await.is_err()
    {
        return;
    }
    tokio::io::copy_bidirectional(&mut client, &mut upstream)
        .await
        .ok();
}

async fn intercept<C: Client>(
    mut client: C,
    rules: &Rules,
    host: String,
    upstream: TcpStream,
    injection: &Injection,
    rest: &[u8],
) {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let tls = &rules.allowlist.tls;
    let (Ok(server), Ok(name)) = (tls.server(&host), ServerName::try_from(host.clone())) else {
        respond(&mut client, "502 Bad Gateway").await;
        return;
    };
    let connector = TlsConnector::from(Arc::clone(&tls.upstream));
    let upstream = tokio::time::timeout(CONNECT_TIMEOUT, connector.connect(name, upstream)).await;
    let Ok(Ok(mut upstream)) = upstream else {
        unreachable(&mut client, rules, host).await;
        return;
    };
    if client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await
        .is_err()
    {
        return;
    }
    let client = Rewind {
        prefix: rest.to_vec(),
        inner: client,
    };
    let accepted =
        tokio::time::timeout(CONNECT_TIMEOUT, TlsAcceptor::from(server).accept(client)).await;
    let Ok(Ok(mut client)) = accepted else {
        return;
    };
    let Ok(Some((head, end))) = tokio::time::timeout(HEAD_TIMEOUT, read_head(&mut client)).await
    else {
        return;
    };
    let name = injection.header.as_bytes();
    let rewritten = rewrite_head(
        &head[..end],
        None,
        |header| {
            header.eq_ignore_ascii_case(name)
                || header.eq_ignore_ascii_case(b"connection")
                || header.eq_ignore_ascii_case(b"keep-alive")
        },
        &[
            (&injection.header, &injection.value),
            ("Connection", "close"),
        ],
    );
    if upstream.write_all(&rewritten).await.is_err()
        || upstream.write_all(&head[end..]).await.is_err()
    {
        return;
    }
    tokio::io::copy_bidirectional(&mut client, &mut upstream)
        .await
        .ok();
}

async fn forward<C: Client>(
    mut client: C,
    rules: &Rules,
    url: &str,
    (method, version): (String, String),
    head: &[u8],
    rest: &[u8],
) {
    let split = url.find(['/', '?', '#']).unwrap_or(url.len());
    let (authority, path) = url.split_at(split);
    let path = match path.split('#').next().unwrap_or_default() {
        "" => "/".to_owned(),
        path if path.starts_with('?') => format!("/{path}"),
        path => path.to_owned(),
    };
    let Some((host, port)) = split_authority(authority, Some(80)) else {
        respond(&mut client, "400 Bad Request").await;
        return;
    };
    if !allowed(&mut client, rules, &host).await {
        return;
    }
    let Some(mut upstream) = upstream(&mut client, rules, host, port).await else {
        return;
    };
    let has_host = head
        .split(|&b| b == b'\n')
        .skip(1)
        .any(|line| line.len() > 5 && line[..5].eq_ignore_ascii_case(b"host:"));
    let line = format!("{method} {path} {version}");
    let mut extra = vec![("Connection", "close")];
    if !has_host {
        extra.insert(0, ("Host", authority));
    }
    let rewritten = rewrite_head(
        head,
        Some(line.as_bytes()),
        |header| {
            header.len() >= 6 && header[..6].eq_ignore_ascii_case(b"proxy-")
                || header.eq_ignore_ascii_case(b"connection")
                || header.eq_ignore_ascii_case(b"keep-alive")
        },
        &extra,
    );
    if upstream.write_all(&rewritten).await.is_err() || upstream.write_all(rest).await.is_err() {
        return;
    }
    tokio::io::copy_bidirectional(&mut client, &mut upstream)
        .await
        .ok();
}

fn rewrite_head(
    head: &[u8],
    line: Option<&[u8]>,
    drop: impl Fn(&[u8]) -> bool,
    extra: &[(&str, &str)],
) -> Vec<u8> {
    let body = head.strip_suffix(b"\r\n\r\n").unwrap_or(head);
    let mut lines = body
        .split(|&b| b == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line));
    let first = lines.next().unwrap_or_default();
    let mut out = Vec::with_capacity(head.len() + 256);
    out.extend_from_slice(line.unwrap_or(first));
    out.extend_from_slice(b"\r\n");
    let mut dropping = false;
    for header in lines {
        if !header.starts_with(b" ") && !header.starts_with(b"\t") {
            let name = header.split(|&b| b == b':').next().unwrap_or_default();
            dropping = drop(name.trim_ascii());
        }
        if !dropping {
            out.extend_from_slice(header);
            out.extend_from_slice(b"\r\n");
        }
    }
    for (name, value) in extra {
        out.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
    }
    out.extend_from_slice(b"\r\n");
    out
}

struct Rewind<S> {
    prefix: Vec<u8>,
    inner: S,
}

impl<S: AsyncRead + Unpin> AsyncRead for Rewind<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.prefix.is_empty() {
            return Pin::new(&mut this.inner).poll_read(cx, buf);
        }
        let take = this.prefix.len().min(buf.remaining());
        buf.put_slice(&this.prefix[..take]);
        this.prefix.drain(..take);
        Poll::Ready(Ok(()))
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Rewind<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

pub fn host_matches(pattern: &str, host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    pattern.strip_prefix("*.").map_or_else(
        || host == pattern,
        |domain| {
            host.strip_suffix(domain)
                .is_some_and(|prefix| prefix.len() > 1 && prefix.ends_with('.'))
        },
    )
}
