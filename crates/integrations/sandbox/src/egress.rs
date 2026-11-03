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
const MAX_CONNECTIONS: usize = 1024;
const HEAD_TIMEOUT: Duration = Duration::from_secs(30);
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
    pub private_hosts: Vec<String>,
    pub injections: Vec<Injection>,
    pub tls: Arc<Tls>,
    pub tailnet: Option<SocketAddr>,
    pub refusal: fn(&str) -> String,
}

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
                || header.eq_ignore_ascii_case(b"accept-encoding")
                || header.eq_ignore_ascii_case(b"upgrade")
                || header.eq_ignore_ascii_case(b"te")
        },
        &[
            (&injection.header, &injection.value),
            ("Accept-Encoding", "identity"),
            ("Connection", "close"),
        ],
    );
    if upstream.write_all(&rewritten).await.is_err()
        || upstream.write_all(&head[end..]).await.is_err()
    {
        return;
    }
    let head_only = head.starts_with(b"HEAD ");
    let forms = masks(&injection.value);
    let (mut client_read, mut client_write) = tokio::io::split(client);
    let (upstream_read, mut upstream_write) = tokio::io::split(upstream);
    let requests = async {
        tokio::io::copy(&mut client_read, &mut upstream_write)
            .await
            .ok();
        upstream_write.shutdown().await.ok();
        std::future::pending::<()>().await;
    };
    tokio::select! {
        () = requests => {}
        _ = relay(upstream_read, &mut client_write, head_only, &forms) => {}
    }
}

const MAX_RESPONSE_HEAD: usize = 64 * 1024;
const MAX_CHUNK_LINE: usize = 1024;
const MIN_MASKED: usize = 4;
const MIN_MASKED_TOKEN: usize = 8;

pub fn masks(value: &str) -> Vec<Vec<u8>> {
    let mut forms = vec![value.as_bytes().to_vec()];
    if let Some((_, token)) = value.trim().rsplit_once([' ', '\t'])
        && token.len() >= MIN_MASKED_TOKEN
    {
        forms.push(token.as_bytes().to_vec());
    }
    let escaped: Vec<Vec<u8>> = forms
        .iter()
        .flat_map(|form| [json_escape(form, true), json_escape(form, false)])
        .collect();
    forms.extend(escaped);
    forms.retain(|form| form.len() >= MIN_MASKED);
    forms.sort_by_key(|form| std::cmp::Reverse(form.len()));
    forms.dedup();
    forms
}

fn json_escape(bytes: &[u8], slash: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for &b in bytes {
        match b {
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'"' => out.extend_from_slice(b"\\\""),
            b'/' if slash => out.extend_from_slice(b"\\/"),
            0..=0x1f => out.extend_from_slice(format!("\\u{b:04x}").as_bytes()),
            _ => out.push(b),
        }
    }
    out
}

pub fn mask(bytes: &mut [u8], forms: &[Vec<u8>]) {
    let mut at = 0;
    'scan: while at < bytes.len() {
        for form in forms {
            if bytes[at..].starts_with(form) {
                bytes[at..at + form.len()].fill(b'*');
                at += form.len();
                continue 'scan;
            }
        }
        at += 1;
    }
}

fn partial(bytes: &[u8], forms: &[Vec<u8>]) -> usize {
    forms
        .iter()
        .filter_map(|form| {
            (1..form.len().min(bytes.len() + 1))
                .rev()
                .find(|len| bytes.ends_with(&form[..*len]))
        })
        .max()
        .unwrap_or(0)
}

pub struct Masker<'a> {
    forms: &'a [Vec<u8>],
    carry: Vec<u8>,
}

impl<'a> Masker<'a> {
    pub const fn new(forms: &'a [Vec<u8>]) -> Self {
        Self {
            forms,
            carry: Vec::new(),
        }
    }

    pub fn push(&mut self, data: &[u8]) -> Vec<u8> {
        self.carry.extend_from_slice(data);
        mask(&mut self.carry, self.forms);
        let keep = partial(&self.carry, self.forms);
        self.carry.drain(..self.carry.len() - keep).collect()
    }

    pub fn finish(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.carry)
    }
}

struct Buffered<R> {
    inner: R,
    buffer: Vec<u8>,
}

impl<R: AsyncRead + Unpin> Buffered<R> {
    async fn fill(&mut self) -> io::Result<bool> {
        let mut chunk = [0u8; 16 * 1024];
        let read = self.inner.read(&mut chunk).await?;
        self.buffer.extend_from_slice(&chunk[..read]);
        Ok(read > 0)
    }

    async fn until(&mut self, delimiter: &[u8], max: usize) -> io::Result<Vec<u8>> {
        loop {
            if let Some(at) = self
                .buffer
                .windows(delimiter.len())
                .position(|window| window == delimiter)
            {
                if at > max {
                    return Err(other("response line too long"));
                }
                return Ok(self.buffer.drain(..at + delimiter.len()).collect());
            }
            if self.buffer.len() > max + delimiter.len() {
                return Err(other("response line too long"));
            }
            if !self.fill().await? {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
        }
    }

    async fn some(&mut self, limit: u64) -> io::Result<Vec<u8>> {
        if self.buffer.is_empty() && !self.fill().await? {
            return Ok(Vec::new());
        }
        let take =
            usize::try_from(limit).map_or(self.buffer.len(), |limit| limit.min(self.buffer.len()));
        Ok(self.buffer.drain(..take).collect())
    }
}

struct Response {
    status: u16,
    chunked: bool,
    length: Option<u64>,
    encoded: bool,
}

fn parse_response(head: &[u8]) -> Option<Response> {
    let text = std::str::from_utf8(head).ok()?;
    let mut lines = text.split("\r\n");
    let status = lines.next()?.split(' ').nth(1)?.parse().ok()?;
    let mut response = Response {
        status,
        chunked: false,
        length: None,
        encoded: false,
    };
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let (name, value) = (name.trim(), value.trim());
        if name.eq_ignore_ascii_case("content-encoding") {
            response.encoded |= value
                .split(',')
                .map(str::trim)
                .any(|coding| !coding.is_empty() && !coding.eq_ignore_ascii_case("identity"));
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            response.chunked = value
                .rsplit(',')
                .next()
                .is_some_and(|coding| coding.trim().eq_ignore_ascii_case("chunked"));
            response.encoded |= value.split(',').map(str::trim).any(|coding| {
                !coding.eq_ignore_ascii_case("chunked") && !coding.eq_ignore_ascii_case("identity")
            });
        } else if name.eq_ignore_ascii_case("content-length") {
            let length = value.parse().ok()?;
            if response.length.is_some_and(|seen| seen != length) {
                return None;
            }
            response.length = Some(length);
        }
    }
    Some(response)
}

async fn write_chunk<W: AsyncWrite + Unpin>(client: &mut W, data: &[u8]) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    client
        .write_all(format!("{:x}\r\n", data.len()).as_bytes())
        .await?;
    client.write_all(data).await?;
    client.write_all(b"\r\n").await
}

async fn relay<R, W>(
    upstream: R,
    client: &mut W,
    head_only: bool,
    forms: &[Vec<u8>],
) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut upstream = Buffered {
        inner: upstream,
        buffer: Vec::new(),
    };
    loop {
        let mut head = upstream.until(b"\r\n\r\n", MAX_RESPONSE_HEAD).await?;
        let Some(response) = parse_response(&head) else {
            respond(client, "502 Bad Gateway").await;
            return Ok(());
        };
        if response.status == 101 || response.encoded {
            respond_text(
                client,
                "502 Bad Gateway",
                "The egress proxy refused a compressed or upgraded response to a request it added a secret to.",
            )
            .await;
            return Ok(());
        }
        mask(&mut head, forms);
        client.write_all(&head).await?;
        if (100..200).contains(&response.status) {
            continue;
        }
        if head_only || response.status == 204 || response.status == 304 {
            break;
        }
        let mut masker = Masker::new(forms);
        if response.chunked {
            loop {
                let line = upstream.until(b"\r\n", MAX_CHUNK_LINE).await?;
                let size = std::str::from_utf8(&line)
                    .ok()
                    .and_then(|line| line.trim().split(';').next())
                    .and_then(|size| u64::from_str_radix(size.trim(), 16).ok())
                    .ok_or_else(|| other("malformed chunk size"))?;
                if size == 0 {
                    break;
                }
                let mut left = size;
                while left > 0 {
                    let data = upstream.some(left).await?;
                    if data.is_empty() {
                        return Err(io::ErrorKind::UnexpectedEof.into());
                    }
                    left -= data.len() as u64;
                    write_chunk(client, &masker.push(&data)).await?;
                }
                upstream.until(b"\r\n", 0).await?;
            }
            write_chunk(client, &masker.finish()).await?;
            let mut trailers = b"0\r\n".to_vec();
            loop {
                let line = upstream.until(b"\r\n", MAX_RESPONSE_HEAD).await?;
                let end = line == b"\r\n";
                trailers.extend_from_slice(&line);
                if end || trailers.len() > MAX_RESPONSE_HEAD {
                    break;
                }
            }
            mask(&mut trailers, forms);
            client.write_all(&trailers).await?;
        } else if let Some(length) = response.length {
            let mut left = length;
            while left > 0 {
                let data = upstream.some(left).await?;
                if data.is_empty() {
                    break;
                }
                left -= data.len() as u64;
                client.write_all(&masker.push(&data)).await?;
            }
            client.write_all(&masker.finish()).await?;
        } else {
            loop {
                match upstream.some(u64::MAX).await {
                    Ok(data) if !data.is_empty() => client.write_all(&masker.push(&data)).await?,
                    _ => break,
                }
            }
            client.write_all(&masker.finish()).await?;
        }
        break;
    }
    client.flush().await?;
    client.shutdown().await
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
