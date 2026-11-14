use crate::egress::{
    address::{Cidr, TAILNET_SUFFIX, host_matches, is_public, reachable},
    addresses::Addresses,
    audit::{Audit, Kind, Reporting},
    authority::Authority,
    lookup::{Caching, Client, Lookups, Purpose, Verdict},
    redact::{self, Injecting},
    tls::Tls,
    wire::{Credential, Secret},
};
use bytes::Bytes;
use http::{
    Method, StatusCode, Uri,
    header::{self, HeaderMap, HeaderValue},
    uri::Authority as UriAuthority,
};
use nanocode_http::{
    server::{self, Incoming, OnUpgrade, Outgoing, Reply, Upgraded},
    wire::WireError,
};
use nanocode_platform::timeout;
use nanocode_sync::mpsc;
use nanocode_sync::{Atom, Duplex, pipe};
use nanocode_sys::base64::STANDARD;
use nanocode_trace::{debug, error, info, warn};
use rustls::pki_types::ServerName;
use std::{
    any::Any,
    collections::HashMap,
    io,
    io::ErrorKind,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpStream, lookup_host},
};

const HOP: &str = "connection keep-alive proxy-authenticate proxy-authorization proxy-connection te trailer transfer-encoding";

pub type Slot = Arc<dyn Any + Send + Sync>;

pub type Upstream = Box<dyn Duplex>;

pub type Resolving = Pin<Box<dyn Future<Output = io::Result<Vec<SocketAddr>>> + Send>>;

pub type Dialing = Pin<Box<dyn Future<Output = io::Result<Upstream>> + Send>>;

pub trait Dial: Send + Sync + 'static {
    fn resolve(&self, host: &str, port: u16) -> Resolving;

    fn connect(&self, address: SocketAddr) -> Dialing;
}

struct Network;

impl Dial for Network {
    fn resolve(&self, host: &str, port: u16) -> Resolving {
        let host = host.to_owned();
        Box::pin(async move { Ok(lookup_host((host.as_str(), port)).await?.collect()) })
    }

    fn connect(&self, address: SocketAddr) -> Dialing {
        Box::pin(async move {
            let stream = TcpStream::connect(address).await?;
            drop(stream.set_nodelay(true));
            Ok(Box::new(stream) as Upstream)
        })
    }
}

type Refusal = Box<Reply>;

pub enum Identity {
    Fixed(Credential),
    Basic,
    Addresses(Arc<Addresses>),
}

pub enum Reach {
    Public {
        deny: Vec<Cidr>,
    },
    Listed {
        hosts: Vec<String>,
        private_hosts: Vec<String>,
        tailnet: Option<SocketAddr>,
    },
}

#[derive(Clone, Copy)]
pub struct Texts {
    pub client: &'static str,
    pub needs_credential: &'static str,
    pub realm: &'static str,
    pub unreachable: &'static str,
    pub stale: &'static str,
    pub private: fn(&str) -> String,
    pub refused: fn(&str) -> String,
    pub compressed: &'static str,
}

#[derive(Clone, Copy)]
pub struct Limits {
    pub connections: usize,
    pub per_source: usize,
    pub connect: Duration,
    pub idle_headers: Duration,
    pub idle_tunnel: Duration,
}

pub struct Rules {
    pub caching: Caching,
    pub reporting: Option<Reporting>,
    pub identity: Identity,
    pub reach: Reach,
    pub passthrough: Vec<String>,
    pub injecting: Injecting,
    pub intercept_port: Option<u16>,
    pub texts: Texts,
    pub limits: Limits,
}

#[derive(Default)]
pub(crate) struct Log {
    pub(crate) denied: Atom<Vec<String>>,
    pub(crate) unreachable: Atom<Vec<String>>,
}

pub struct Proxy<A: Authority> {
    lookups: Lookups<A>,
    audit: Option<Audit>,
    log: Option<Arc<Log>>,
    tls: Arc<Tls>,
    rules: Rules,
    dial: Arc<dyn Dial>,
}

struct Admitted {
    source: IpAddr,
    ended: mpsc::UnboundedSender<IpAddr>,
}

impl Drop for Admitted {
    fn drop(&mut self) {
        self.ended.send(self.source).ok();
    }
}

impl<A: Authority> Proxy<A> {
    pub fn new(authority: Arc<A>, tls: Arc<Tls>, rules: Rules) -> Self {
        Self {
            lookups: Lookups::new(Arc::clone(&authority), rules.texts.client, rules.caching),
            audit: rules
                .reporting
                .map(|reporting| Audit::start(authority, reporting)),
            log: None,
            tls,
            rules,
            dial: Arc::new(Network),
        }
    }

    #[must_use]
    pub fn dialing(mut self, dial: Arc<dyn Dial>) -> Self {
        self.dial = dial;
        self
    }

    pub(crate) fn logged(mut self, log: Arc<Log>) -> Self {
        self.log = Some(log);
        self
    }

    pub async fn accept<T, F, Fut>(self: Arc<Self>, mut next: F)
    where
        T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
        F: FnMut() -> Fut,
        Fut: Future<Output = Option<(T, IpAddr)>>,
    {
        let (ended, mut endings) = mpsc::unbounded_channel();
        let (mut open, mut total) = (HashMap::<IpAddr, usize>::new(), 0usize);
        let limits = self.rules.limits;
        while let Some((stream, source)) = next().await {
            while let Ok(gone) = endings.try_recv() {
                total = total.saturating_sub(1);
                if let Some(count) = open.get_mut(&gone) {
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        open.remove(&gone);
                    }
                }
            }
            let count = open.entry(source).or_default();
            if total >= limits.connections || *count >= limits.per_source {
                if *count == 0 {
                    open.remove(&source);
                }
                continue;
            }
            *count += 1;
            total += 1;
            let slot: Slot = Arc::new(Admitted {
                source,
                ended: ended.clone(),
            });
            let proxy = Arc::clone(&self);
            nanocode_platform::spawn(async move { proxy.serve(stream, slot, source).await });
        }
    }

    pub async fn serve<T>(self: Arc<Self>, stream: T, slot: Slot, source: IpAddr)
    where
        T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let proxy = Arc::clone(&self);
        let served = self
            .served(stream, move |request| {
                Arc::clone(&proxy).handle(request, Arc::clone(&slot), source)
            })
            .await;
        if let Err(error) = served
            && !matches!(error, WireError::Closed { .. } | WireError::Truncated)
        {
            warn!("egress connection: {error}");
        }
    }

    async fn client(
        &self,
        headers: &HeaderMap,
        source: IpAddr,
        purpose: Purpose,
    ) -> Result<Arc<Client>, Refusal> {
        let credential = match &self.rules.identity {
            Identity::Fixed(credential) => Some(credential.clone()),
            Identity::Addresses(addresses) => addresses.name(source).map(|name| Credential {
                name,
                token: String::new(),
            }),
            Identity::Basic => credential(headers),
        };
        let credential = credential.ok_or_else(|| self.needs_credential())?;
        match self.lookups.resolve(&Arc::new(credential), purpose).await {
            Verdict::Granted(client) => Ok(client),
            Verdict::Refused => Err(self.needs_credential()),
            Verdict::Unreachable => Err(self.unreachable()),
        }
    }

    fn count(&self, client: &Client, host: &str, port: u16, kind: Kind) {
        let Some(audit) = &self.audit else { return };
        let pattern = if kind == Kind::Intercepted {
            client.pattern(host)
        } else {
            None
        };
        audit.count(&client.credential, host, port, kind, pattern);
    }

    fn noted(&self, host: &str, denied: bool) {
        if let Some(log) = &self.log {
            let list = if denied {
                &log.denied
            } else {
                &log.unreachable
            };
            list.update(|hosts| hosts.push(host.to_owned()));
        }
    }

    fn allowed(&self, host: &str) -> Result<(), Refusal> {
        let Reach::Listed { hosts, .. } = &self.rules.reach else {
            return Ok(());
        };
        if hosts.iter().any(|pattern| host_matches(pattern, host)) {
            return Ok(());
        }
        self.noted(host, true);
        Err(refuse(
            StatusCode::FORBIDDEN,
            &(self.rules.texts.refused)(host),
        ))
    }

    fn secrets<'a>(&self, client: &'a Client, host: &str) -> Vec<&'a Secret> {
        match self.rules.injecting {
            Injecting::Swap { .. } => client.bound(host),
            Injecting::Set => client.set(host),
        }
    }

    fn intercepts(&self, client: &Client, host: &str, port: u16) -> bool {
        let bound = match self.rules.injecting {
            Injecting::Swap { .. } => client.pattern(host).is_some(),
            Injecting::Set => !client.set(host).is_empty(),
        };
        let passthrough = self
            .rules
            .passthrough
            .iter()
            .any(|pattern| host_matches(pattern, host));
        self.rules.intercept_port.is_none_or(|only| only == port) && bound && !passthrough
    }

    async fn handle(
        self: Arc<Self>,
        request: server::Request,
        slot: Slot,
        source: IpAddr,
    ) -> Reply {
        let connect = request.method == Method::CONNECT;
        let mut client = self.client(&request.headers, source, Purpose::Route).await;
        if let Ok(known) = &client
            && connect
            && let Ok(authority) = request.target.parse::<UriAuthority>()
            && known
                .pattern(&authority.host().to_ascii_lowercase())
                .is_none()
        {
            client = self
                .client(&request.headers, source, Purpose::Recheck)
                .await;
        }
        match client {
            Err(response) => *response,
            Ok(client) if connect => self.connect(client, request, slot).await,
            Ok(client) => self
                .plain(&client, request, slot)
                .await
                .unwrap_or_else(|response| *response),
        }
    }

    async fn dial(&self, host: &str, port: u16) -> Result<Upstream, Refusal> {
        let connect = self.rules.limits.connect;
        let failed = |host: &str| {
            self.noted(host, false);
            refuse(
                StatusCode::BAD_GATEWAY,
                &format!("{host}:{port} can't be reached."),
            )
        };
        let (private_hosts, deny) = match &self.rules.reach {
            Reach::Public { deny } => (None, deny.as_slice()),
            Reach::Listed {
                private_hosts,
                tailnet,
                ..
            } => {
                let name = host.trim_end_matches('.').to_ascii_lowercase();
                if let Some(server) = tailnet
                    && name.ends_with(TAILNET_SUFFIX)
                {
                    return match timeout(connect, socks(*server, &name, port)).await {
                        Ok(Ok(stream)) => Ok(Box::new(stream)),
                        _ => Err(failed(host)),
                    };
                }
                (Some(private_hosts), &[][..])
            }
        };
        let literal = host
            .strip_prefix('[')
            .and_then(|host| host.strip_suffix(']'))
            .unwrap_or(host);
        let addresses: Vec<SocketAddr> = match literal.parse::<IpAddr>() {
            Ok(ip) => vec![SocketAddr::new(ip, port)],
            Err(_) => timeout(connect, self.dial.resolve(literal, port))
                .await
                .ok()
                .and_then(Result::ok)
                .ok_or_else(|| {
                    self.noted(host, false);
                    refuse(
                        StatusCode::BAD_GATEWAY,
                        &format!("{host} does not resolve."),
                    )
                })?,
        };
        let addresses: Vec<SocketAddr> = match private_hosts {
            None if addresses.is_empty()
                || !addresses
                    .iter()
                    .all(|address| reachable(address.ip(), deny)) =>
            {
                Vec::new()
            }
            None => addresses,
            Some(private) if private.iter().any(|pattern| host_matches(pattern, host)) => addresses,
            Some(_) => addresses
                .into_iter()
                .filter(|address| is_public(address.ip()))
                .collect(),
        };
        if addresses.is_empty() {
            self.noted(host, false);
            let message = (self.rules.texts.private)(host);
            return Err(refuse(StatusCode::FORBIDDEN, &message));
        }
        for address in addresses {
            if let Ok(Ok(stream)) = timeout(connect, self.dial.connect(address)).await {
                return Ok(stream);
            }
        }
        Err(failed(host))
    }

    async fn connect(
        self: Arc<Self>,
        client: Arc<Client>,
        mut request: server::Request,
        slot: Slot,
    ) -> Reply {
        let (Ok(authority), Some(upgrade)) = (
            request.target.parse::<UriAuthority>(),
            request.upgrade.take(),
        ) else {
            return text(StatusCode::BAD_REQUEST, "CONNECT needs host:port.");
        };
        let (host, port) = (host_of(&authority), authority.port_u16().unwrap_or(443));
        if let Err(response) = self.allowed(&host) {
            return *response;
        }
        if self.intercepts(&client, &host, port) {
            self.count(&client, &host, port, Kind::Intercepted);
            nanocode_platform::spawn(async move {
                match upgrade.await {
                    Ok(upgraded) => self.intercept(client, host, port, upgraded, slot).await,
                    Err(error) => warn!("egress upgrade: {error}"),
                }
            });
            return Reply::empty();
        }
        let upstream = match self.dial(&host, port).await {
            Ok(upstream) => upstream,
            Err(response) => {
                self.count(&client, &host, port, Kind::Refused);
                return *response;
            }
        };
        self.count(&client, &host, port, Kind::Tunneled);
        let idle = self.rules.limits.idle_tunnel;
        nanocode_platform::spawn(async move {
            match upgrade.await {
                Ok(upgraded) => pipe(upgraded, upstream, idle).await,
                Err(error) => warn!("egress upgrade: {error}"),
            }
            drop(slot);
        });
        Reply::empty()
    }

    async fn intercept(
        self: Arc<Self>,
        client: Arc<Client>,
        host: String,
        port: u16,
        upgraded: Upgraded,
        slot: Slot,
    ) {
        let config = match self.tls.server(&host) {
            Ok(config) => config,
            Err(error) => {
                return error!("egress certificate for {host} failed: {error:#}");
            }
        };
        let accept = nanocode_tls::accept(config, upgraded);
        let error = match timeout(self.rules.limits.connect, accept).await {
            Ok(Ok(tls)) => {
                let proxy = Arc::clone(&self);
                let served = self.served(tls, move |request| {
                    let (proxy, client, host, slot) = (
                        Arc::clone(&proxy),
                        Arc::clone(&client),
                        host.clone(),
                        Arc::clone(&slot),
                    );
                    async move {
                        let response = proxy.forward(&client, &host, port, request, slot).await;
                        response.unwrap_or_else(|response| *response)
                    }
                });
                return drop(served.await);
            }
            Ok(Err(error)) => error.to_string(),
            Err(_) => "no handshake in time".to_owned(),
        };
        let name = &client.credential.name;
        info!(%name, "egress TLS from {name} for {host}: {error}");
    }

    async fn forward(
        &self,
        client: &Client,
        host: &str,
        port: u16,
        request: server::Request,
        slot: Slot,
    ) -> Result<Reply, Refusal> {
        let client = match self
            .lookups
            .resolve(&client.credential, Purpose::Inject)
            .await
        {
            Verdict::Granted(client) => client,
            Verdict::Refused => {
                let message = self.rules.texts.stale;
                return Err(refuse(StatusCode::PROXY_AUTHENTICATION_REQUIRED, message));
            }
            Verdict::Unreachable => return Err(self.unreachable()),
        };
        let (upgrade, mut request) = outgoing(request)?;
        let headers = &mut request.headers;
        headers.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static("identity"),
        );
        let injecting = self.rules.injecting;
        let bound = self.secrets(&client, host);
        let written = redact::inject(headers, &bound, injecting);
        let pairs = redact::redactions(&bound, &written, injecting);
        if let Ok(value) = HeaderValue::try_from(host) {
            headers.insert(header::HOST, value);
        }
        let stream = self.dial(host, port).await?;
        let name = ServerName::try_from(host.to_owned())
            .map_err(|_| refuse(StatusCode::BAD_REQUEST, "Invalid host."))?;
        let connect = nanocode_tls::connect(self.tls.upstream(), name, stream);
        let tls = timeout(self.rules.limits.connect, connect)
            .await
            .unwrap_or_else(|_| Err(ErrorKind::TimedOut.into()))
            .map_err(|error| {
                self.noted(host, false);
                refuse(
                    StatusCode::BAD_GATEWAY,
                    &format!("TLS to {host} failed: {error}"),
                )
            })?;
        let sealed = injecting == Injecting::Set && !bound.is_empty();
        let response = self.send(tls, request).await?;
        Ok(self.relay(response, upgrade, Some(pairs), slot, sealed))
    }

    async fn plain(
        &self,
        client: &Client,
        request: server::Request,
        slot: Slot,
    ) -> Result<Reply, Refusal> {
        let uri = request
            .target
            .parse::<Uri>()
            .map_err(|_| refuse(StatusCode::BAD_REQUEST, "Invalid request target."))?;
        let (Some("http"), Some(authority)) = (uri.scheme_str(), uri.authority()) else {
            let message = "Send HTTPS through CONNECT and plain HTTP as an absolute URL.";
            return Err(refuse(StatusCode::BAD_REQUEST, message));
        };
        let (host, port) = (host_of(authority), authority.port_u16().unwrap_or(80));
        self.allowed(&host)?;
        let dialed = self.dial(&host, port).await;
        let kind = if dialed.is_ok() {
            Kind::Tunneled
        } else {
            Kind::Refused
        };
        self.count(client, &host, port, kind);
        let stream = dialed?;
        let (upgrade, request) = outgoing(request)?;
        let response = self.send(stream, request).await?;
        Ok(self.relay(response, upgrade, None, slot, false))
    }

    fn needs_credential(&self) -> Refusal {
        let texts = &self.rules.texts;
        let mut response = text(
            StatusCode::PROXY_AUTHENTICATION_REQUIRED,
            texts.needs_credential,
        );
        if let Ok(realm) = HeaderValue::from_str(texts.realm) {
            response.headers.insert(header::PROXY_AUTHENTICATE, realm);
        }
        Box::new(response)
    }

    fn unreachable(&self) -> Refusal {
        refuse(StatusCode::BAD_GATEWAY, self.rules.texts.unreachable)
    }

    fn served<T, H, F>(
        &self,
        io: T,
        handler: H,
    ) -> impl Future<Output = Result<(), WireError>> + use<T, H, F, A>
    where
        T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
        H: FnMut(server::Request) -> F,
        F: Future<Output = Reply>,
    {
        let options = server::Options {
            header_read_timeout: Some(self.rules.limits.idle_headers),
        };
        server::serve(Box::new(io), options, handler)
    }

    async fn send<T: Duplex + 'static>(
        &self,
        io: T,
        request: Forwarded,
    ) -> Result<server::Exchanged, Refusal> {
        let failed =
            |error: WireError| refuse(StatusCode::BAD_GATEWAY, &format!("upstream: {error}"));
        let exchange = server::send(
            Box::new(io),
            &request.method,
            &request.target,
            request.headers,
            request.body,
            request.length,
        );
        let answer = timeout(self.rules.limits.idle_tunnel, exchange)
            .await
            .map_err(|_| {
                refuse(
                    StatusCode::GATEWAY_TIMEOUT,
                    "The host sent no answer in time.",
                )
            })?;
        answer.map_err(failed)
    }

    fn relay(
        &self,
        response: server::Exchanged,
        client: Option<OnUpgrade>,
        pairs: Option<redact::Redactions>,
        slot: Slot,
        sealed: bool,
    ) -> Reply {
        let server::Exchanged { head, body } = response;
        let (status, mut reason, mut headers) = (head.status, head.reason, head.headers);
        let pairs = pairs.filter(|pairs| !pairs.is_empty());
        let coding = headers.get(header::CONTENT_ENCODING);
        let compressed = pairs.is_some()
            && coding.is_some_and(|coding| !coding.as_bytes().eq_ignore_ascii_case(b"identity"));
        let upgraded = status == StatusCode::SWITCHING_PROTOCOLS;
        if compressed || (sealed && upgraded) {
            return text(StatusCode::BAD_GATEWAY, self.rules.texts.compressed);
        }
        if let Some(pairs) = &pairs {
            redact::headers(&mut headers, pairs);
            redact::reason(&mut reason, pairs);
        }
        let idle = self.rules.limits.idle_tunnel;
        let body = if upgraded {
            let upstream = body.into_upgraded();
            nanocode_platform::spawn(async move {
                if let (Some(client), Some(upstream)) = (client, upstream)
                    && let Ok(client) = client.await
                {
                    pipe(client, upstream, idle).await;
                }
                drop(slot);
            });
            Outgoing::Empty
        } else {
            strip_hop(&mut headers, false);
            if pairs.is_some() {
                headers.remove(header::CONTENT_LENGTH);
            }
            Outgoing::Stream(redact::body(body, pairs.unwrap_or_default(), idle))
        };
        let mut reply = Reply::new(status, body);
        reply.reason = reason;
        reply.headers = headers;
        reply
    }
}

fn text(status: StatusCode, message: &str) -> Reply {
    let mut response = Reply::new(status, Outgoing::Full(Bytes::from(format!("{message}\n"))));
    let plain = HeaderValue::from_static("text/plain");
    response.headers.insert(header::CONTENT_TYPE, plain);
    response
}

fn refuse(status: StatusCode, message: &str) -> Refusal {
    Box::new(text(status, message))
}

fn strip_hop(headers: &mut HeaderMap, upgrade: bool) {
    let listed: Vec<String> = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|name| name.trim().to_ascii_lowercase())
        .filter(|name| !(upgrade && name == "upgrade"))
        .collect();
    for name in HOP.split(' ').map(str::to_owned).chain(listed) {
        if !(upgrade && name == "connection") {
            headers.remove(name.as_str());
        }
    }
}

struct Forwarded {
    method: Method,
    target: String,
    headers: HeaderMap,
    body: Incoming,
    length: Option<u64>,
}

fn outgoing(request: server::Request) -> Result<(Option<OnUpgrade>, Forwarded), Refusal> {
    let server::Request {
        method,
        target,
        mut headers,
        body,
        upgrade: client,
        ..
    } = request;
    let length = server::length(&headers);
    let upgrade = headers.contains_key(header::UPGRADE);
    strip_hop(&mut headers, upgrade);
    let target = target
        .parse::<Uri>()
        .map_err(|error| {
            debug!(%error, "invalid request target");
            refuse(StatusCode::BAD_REQUEST, "Invalid request target.")
        })?
        .path_and_query()
        .map_or("/", |p| p.as_str())
        .to_owned();
    Ok((
        client,
        Forwarded {
            method,
            target,
            headers,
            body,
            length,
        },
    ))
}

fn credential(headers: &HeaderMap) -> Option<Credential> {
    let value = headers.get(header::PROXY_AUTHORIZATION)?.to_str().ok()?;
    let encoded = value
        .strip_prefix("Basic ")
        .or_else(|| value.strip_prefix("basic "))?;
    let decoded = String::from_utf8(STANDARD.decode(encoded.trim()).ok()?).ok()?;
    let (name, token) = decoded.split_once(':')?;
    (!name.is_empty() && !token.is_empty()).then(|| Credential {
        name: name.to_owned(),
        token: token.to_owned(),
    })
}

fn host_of(authority: &UriAuthority) -> String {
    let host = authority
        .host()
        .trim_start_matches('[')
        .trim_end_matches(']');
    host.to_ascii_lowercase()
}

async fn socks(server: SocketAddr, host: &str, port: u16) -> io::Result<TcpStream> {
    let refused = |what: &str| io::Error::other(format!("SOCKS5 {what}"));
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
