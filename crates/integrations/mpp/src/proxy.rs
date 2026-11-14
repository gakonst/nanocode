use std::collections::HashMap;
use std::fmt::Display;
use std::future::Future;
use std::io::Write as _;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use nanocode_bytes::Bytes;
use nanocode_http::Url;
use nanocode_http::header::{
    CONNECTION, CONTENT_TYPE, HOST, HeaderMap, HeaderName, HeaderValue, PROXY_AUTHENTICATE,
    PROXY_AUTHORIZATION,
};
use nanocode_http::server::{self, Outgoing, Reply, Upgraded};
use nanocode_http::types::uri::Authority;
use nanocode_http::{Method, StatusCode};
use nanocode_sync::Atom;
use nanocode_sync::StreamExt;
use nanocode_tls::rustls::ServerConfig;
use nanocode_tls::rustls::crypto::CryptoProvider;
use nanocode_tls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

use super::error::Fault;
use super::events::Events;
use super::flow::{DEFAULT_MAX_PAYMENT_RETRIES, FlowError, Policy, send_with_exchange};
use super::identity::{Identity, Proved};
use super::provider::PaymentProvider;
use super::x509::{self, CertificateFault, Issuer, KeyPair};

type Providers<P> = Box<dyn Fn(&Url) -> Result<P, String> + Send + Sync>;

const HOP_BY_HOP: [&str; 9] = [
    "connection",
    "proxy-connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];
const PROXY_USER: &str = "mpp";
const PROXY_CHALLENGE: &str = "Basic realm=\"mpp\"";
const THREAD_NAME: &str = "mpp-forward-proxy";
const BUNDLE_PREFIX: &str = "mpp-forward-proxy-";
const BUNDLE_EXTENSION: &str = "pem";
const NO_PROXY_VARIABLES: [&str; 2] = ["NO_PROXY", "no_proxy"];
const LOOPBACK_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "::1"];
const NODE_USE_ENV_PROXY: &str = "1";
const ALPN_HTTP1: &[u8] = b"http/1.1";
const EVENT_STREAM: &str = "text/event-stream";
const TOKEN_BYTES: usize = 16;

pub struct ForwardProxy<P> {
    listener: std::net::TcpListener,
    address: SocketAddr,
    token: String,
    certificates: PathBuf,
    providers: Providers<P>,
    events: Events,
    bypass: Vec<String>,
    identity: Option<Arc<Identity>>,
}

impl<P: PaymentProvider + 'static> ForwardProxy<P> {
    pub fn bind<E: Display>(
        providers: impl Fn(&Url) -> Result<P, E> + Send + Sync + 'static,
    ) -> Result<Self, Fault> {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let address = listener.local_addr()?;
        let certificates = std::env::temp_dir().join(format!(
            "{BUNDLE_PREFIX}{}.{BUNDLE_EXTENSION}",
            std::process::id()
        ));
        Ok(Self {
            listener,
            address,
            token: token()?,
            certificates,
            providers: Box::new(move |url| providers(url).map_err(|error| error.to_string())),
            events: Events::default(),
            bypass: Vec::new(),
            identity: None,
        })
    }

    #[must_use]
    pub fn bypass(mut self, hosts: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.bypass.extend(hosts.into_iter().map(Into::into));
        self
    }

    #[must_use]
    pub fn events(mut self, events: Events) -> Self {
        self.events = events;
        self
    }

    #[must_use]
    pub fn identity(mut self, identity: Arc<Identity>) -> Self {
        self.identity = Some(identity);
        self
    }

    pub fn certificates(&self) -> &Path {
        &self.certificates
    }

    pub fn environment(&self) -> Vec<(&'static str, String)> {
        let proxy = format!("http://{PROXY_USER}:{}@{}", self.token, self.address);
        let mut bypass: Vec<String> = NO_PROXY_VARIABLES
            .into_iter()
            .filter_map(|name| std::env::var(name).ok())
            .flat_map(|hosts| {
                hosts
                    .split(',')
                    .map(|host| host.trim().to_owned())
                    .collect::<Vec<_>>()
            })
            .chain(LOOPBACK_HOSTS.map(String::from))
            .chain(self.bypass.iter().cloned())
            .filter(|host| !host.is_empty())
            .collect();
        bypass.sort();
        bypass.dedup();
        let bypass = bypass.join(",");
        let certificates = self.certificates.display().to_string();
        vec![
            ("HTTP_PROXY", proxy.clone()),
            ("HTTPS_PROXY", proxy.clone()),
            ("http_proxy", proxy.clone()),
            ("https_proxy", proxy),
            ("NO_PROXY", bypass.clone()),
            ("no_proxy", bypass),
            ("NODE_USE_ENV_PROXY", NODE_USE_ENV_PROXY.to_owned()),
            ("SSL_CERT_FILE", certificates.clone()),
            ("CURL_CA_BUNDLE", certificates.clone()),
            ("REQUESTS_CA_BUNDLE", certificates.clone()),
            ("NODE_EXTRA_CA_CERTS", certificates),
        ]
    }

    pub fn spawn(self, policy: impl Future<Output = Policy> + Send + 'static) -> Result<(), Fault> {
        let (authority, root) = CertificateAuthority::generate()?;
        let system = match nanocode_platform::trust::native_bundle_file() {
            Some(file) => Ok(std::fs::File::open(file)?),
            None => {
                let native = nanocode_platform::trust::load_native_certificates();
                if native.certificates.is_empty() {
                    return Err(Fault::config(format!(
                        "no trusted root certificates to bundle with the proxy's: {:?}",
                        native.errors
                    )));
                }
                Err(native.certificates)
            }
        };
        let mut bundle = std::fs::File::create(&self.certificates)?;
        bundle.write_all(x509::pem(&root).as_bytes())?;
        match system {
            Ok(mut file) => {
                std::io::copy(&mut file, &mut bundle)?;
            }
            Err(certificates) => {
                for certificate in &certificates {
                    bundle.write_all(x509::pem(certificate.as_ref()).as_bytes())?;
                }
            }
        }
        drop(bundle);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        self.listener.set_nonblocking(true)?;
        let listener = {
            let _runtime = runtime.enter();
            tokio::net::TcpListener::from_std(self.listener)?
        };
        let client = nanocode_http::Client::builder()
            .no_proxy()
            .redirect(nanocode_http::redirect::Policy::none())
            .build()
            .map_err(|error| {
                Fault::http(format!("failed to build the proxy's HTTP client: {error}"))
            })?;
        let credentials = x509::base64(format!("{PROXY_USER}:{}", self.token).as_bytes());
        let authorization = HeaderValue::from_str(&format!("Basic {credentials}"))
            .map_err(|error| Fault::config(format!("invalid proxy credentials: {error}")))?;
        let (providers, events, identity) = (self.providers, self.events, self.identity);
        std::thread::Builder::new()
            .name(THREAD_NAME.into())
            .spawn(move || {
                runtime.block_on(async move {
                    let proxy = Arc::new(Proxy {
                        client,
                        policy: policy.await,
                        events,
                        authorization,
                        authority,
                        providers,
                        paying: Atom::default(),
                        identity,
                    });
                    loop {
                        let Ok((stream, _)) = listener.accept().await else {
                            continue;
                        };
                        let proxy = proxy.clone();
                        nanocode_platform::spawn_task(async move {
                            let _ = server::serve(
                                Box::new(stream),
                                server::Options::default(),
                                move |request| proxy.clone().handle(request),
                            )
                            .await;
                        });
                    }
                })
            })?;
        Ok(())
    }
}

fn token() -> Result<String, Fault> {
    let mut random = [0u8; TOKEN_BYTES];
    nanocode_platform::fill_random(&mut random)
        .map_err(|error| Fault::Internal(error.to_string()))?;
    Ok(nanocode_platform::Uuid::from_random_bytes(random)
        .simple()
        .to_string())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let difference = left
        .iter()
        .zip(right)
        .fold(0u8, |difference, (left, right)| {
            std::hint::black_box(difference | (left ^ right))
        });
    difference == 0
}

struct Proxy<P> {
    client: nanocode_http::Client,
    policy: Policy,
    events: Events,
    authorization: HeaderValue,
    authority: CertificateAuthority,
    providers: Providers<P>,
    paying: Atom<HashMap<String, P>>,
    identity: Option<Arc<Identity>>,
}

impl<P: PaymentProvider + 'static> Proxy<P> {
    async fn handle(self: Arc<Self>, mut request: server::Request) -> Reply {
        let authorized = request
            .headers
            .get(PROXY_AUTHORIZATION)
            .is_some_and(|value| constant_time_eq(value.as_bytes(), self.authorization.as_bytes()));
        if !authorized {
            let mut response = respond(
                StatusCode::PROXY_AUTHENTICATION_REQUIRED,
                "proxy authentication required",
            );
            response.headers.insert(
                PROXY_AUTHENTICATE,
                HeaderValue::from_static(PROXY_CHALLENGE),
            );
            return response;
        }
        if request.method != Method::CONNECT {
            return match Url::parse(&request.target) {
                Ok(url) if url.scheme() == "http" => self.forward(request, url).await,
                _ => respond(
                    StatusCode::BAD_REQUEST,
                    "expected CONNECT or an absolute http:// URL",
                ),
            };
        }
        let (Ok(authority), Some(upgrade)) =
            (request.target.parse::<Authority>(), request.upgrade.take())
        else {
            return respond(StatusCode::BAD_REQUEST, "CONNECT needs host:port");
        };
        let intercept =
            Url::parse(&format!("https://{authority}/")).is_ok_and(|url| self.policy.allows(&url));
        if intercept {
            nanocode_platform::spawn_task(async move {
                if let Ok(upgraded) = upgrade.await {
                    self.intercept(upgraded, authority).await;
                }
            });
            return Reply::empty();
        }
        match tokio::net::TcpStream::connect(authority.as_str()).await {
            Ok(mut server) => {
                nanocode_platform::spawn_task(async move {
                    if let Ok(mut upgraded) = upgrade.await {
                        let _ = tokio::io::copy_bidirectional(&mut upgraded, &mut server).await;
                    }
                });
                Reply::empty()
            }
            Err(error) => respond(StatusCode::BAD_GATEWAY, &format!("{authority}: {error}")),
        }
    }

    async fn intercept(self: Arc<Self>, upgraded: Upgraded, authority: Authority) {
        let host = authority
            .host()
            .trim_start_matches('[')
            .trim_end_matches(']');
        let Ok(config) = self.authority.server_config(host) else {
            return;
        };
        let Ok(stream) = nanocode_tls::accept(config, upgraded).await else {
            return;
        };
        let _ = server::serve(
            Box::new(stream),
            server::Options::default(),
            move |request: server::Request| {
                let proxy = self.clone();
                let url = Url::parse(&format!("https://{authority}{}", request.target));
                async move {
                    match url {
                        Ok(url) => proxy.forward(request, url).await,
                        Err(error) => respond(StatusCode::BAD_REQUEST, &error.to_string()),
                    }
                }
            },
        )
        .await;
    }

    async fn forward(&self, request: server::Request, url: Url) -> Reply {
        match self.pay(request, url).await {
            Ok(response) => response,
            Err((status, message)) => respond(status, &message),
        }
    }

    async fn pay(&self, request: server::Request, url: Url) -> Result<Reply, (StatusCode, String)> {
        let server::Request {
            method,
            mut headers,
            body,
            ..
        } = request;
        let body = body
            .collect()
            .await
            .map_err(|error| (StatusCode::BAD_REQUEST, error.to_string()))?;
        strip_hop_by_hop(&mut headers);
        headers.remove(HOST);
        let mut forwarded = nanocode_http::Request::new(method, url.clone());
        *forwarded.headers_mut() = headers;
        *forwarded.body_mut() = Some(body.into());
        let provider = match self.policy.allows(&url) {
            true => Some(
                self.provider(&url)
                    .map_err(|error| (StatusCode::BAD_GATEWAY, error))?,
            ),
            false => None,
        };
        let response = match &provider {
            Some(provider) => send_with_exchange(
                &mut Proved {
                    exchange: self.client.clone(),
                    identity: self.identity.as_deref(),
                },
                forwarded,
                provider,
                &self.policy,
                &self.events,
                DEFAULT_MAX_PAYMENT_RETRIES,
            )
            .await
            .map_err(|error| match error {
                FlowError::Payment(error) => (
                    StatusCode::PAYMENT_REQUIRED,
                    format!("MPP payment failed: {error}"),
                ),
                FlowError::Send(error) => (StatusCode::BAD_GATEWAY, error.to_string()),
            })?,
            None => self
                .client
                .execute(forwarded)
                .await
                .map_err(|error| (StatusCode::BAD_GATEWAY, error.to_string()))?,
        };
        let stream = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with(EVENT_STREAM));
        let status = response.status();
        let mut headers = response.headers().clone();
        strip_hop_by_hop(&mut headers);
        let body = response.bytes_stream();
        let body = match provider.filter(|_| stream) {
            Some(provider) => self.answer_events(body, provider, url),
            None => body.map(|chunk| chunk.map_err(Into::into)).boxed(),
        };
        let mut reply = Reply::new(status, Outgoing::Stream(body));
        reply.headers = headers;
        Ok(reply)
    }

    fn answer_events(
        &self,
        body: nanocode_sync::stream::BoxStream<'static, nanocode_http::Result<Bytes>>,
        provider: P,
        url: Url,
    ) -> server::BodyStream {
        let client = self.client.clone();
        let mut stream = ServerSentEvents::default();
        body.map(move |chunk| {
            let chunk = chunk?;
            for (name, data) in stream.push(&chunk) {
                let Some(work) = provider.stream_event(&client, url.as_str(), &name, &data) else {
                    continue;
                };
                nanocode_platform::spawn_task(async move {
                    let _ = work.await;
                });
            }
            Ok(chunk)
        })
        .boxed()
    }

    fn provider(&self, url: &Url) -> Result<P, String> {
        let origin = url.origin().ascii_serialization();
        if let Some(provider) = self.paying.read(|paying| paying.get(&origin).cloned()) {
            return Ok(provider);
        }
        let provider = (self.providers)(url)?;
        Ok(self.paying.update(|paying| {
            paying
                .entry(origin.clone())
                .or_insert_with(|| provider.clone())
                .clone()
        }))
    }
}

struct CertificateAuthority {
    issuer: Issuer,
    key: KeyPair,
    crypto: Arc<CryptoProvider>,
    configs: Atom<HashMap<String, Arc<ServerConfig>>>,
}

fn context(message: &'static str) -> impl FnOnce(CertificateFault) -> Fault {
    move |error| Fault::config(format!("{message}: {error}"))
}

impl CertificateAuthority {
    fn generate() -> Result<(Self, Vec<u8>), Fault> {
        nanocode_tls::cpu::check().map_err(|error| Fault::config(error.to_string()))?;
        let crypto = CryptoProvider::get_default().cloned().ok_or_else(|| {
            Fault::config("install a default rustls CryptoProvider before binding a ForwardProxy")
        })?;
        let key = KeyPair::generate().map_err(context("failed to generate the proxy CA key"))?;
        let (issuer, root) =
            Issuer::self_signed(key).map_err(context("failed to sign the proxy CA"))?;
        Ok((
            Self {
                issuer,
                key: KeyPair::generate().map_err(context("failed to generate the proxy key"))?,
                crypto,
                configs: Atom::default(),
            },
            root,
        ))
    }

    fn server_config(&self, host: &str) -> Result<Arc<ServerConfig>, Fault> {
        if let Some(config) = self.configs.read(|configs| configs.get(host).cloned()) {
            return Ok(config);
        }
        let certificate = self
            .issuer
            .sign_server(host, &self.key)
            .map_err(context("failed to sign a proxy certificate"))?;
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key.pkcs8().to_vec()));
        let mut config = ServerConfig::builder_with_provider(self.crypto.clone())
            .with_safe_default_protocol_versions()
            .map_err(|error| Fault::config(format!("unsupported TLS versions: {error}")))?
            .with_no_client_auth()
            .with_single_cert(vec![CertificateDer::from(certificate)], key)
            .map_err(|error| Fault::config(format!("invalid proxy certificate: {error}")))?;
        config.alpn_protocols = vec![ALPN_HTTP1.to_vec()];
        let config = Arc::new(config);
        Ok(self.configs.update(|configs| {
            configs
                .entry(host.to_owned())
                .or_insert_with(|| config.clone())
                .clone()
        }))
    }
}

#[derive(Default)]
struct ServerSentEvents {
    buffer: Vec<u8>,
    name: Option<String>,
    data: Vec<String>,
}

impl ServerSentEvents {
    fn push(&mut self, chunk: &[u8]) -> Vec<(String, String)> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        while let Some(end) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim_end_matches(['\n', '\r']);
            let field = |prefix: &str| {
                line.strip_prefix(prefix)
                    .map(|value| value.strip_prefix(' ').unwrap_or(value).to_owned())
            };
            if line.is_empty() {
                let data = std::mem::take(&mut self.data).join("\n");
                events.extend(self.name.take().map(|name| (name, data)));
            } else if let Some(name) = field("event:") {
                self.name = Some(name);
            } else if let Some(data) = field("data:") {
                self.data.push(data);
            }
        }
        events
    }
}

fn strip_hop_by_hop(headers: &mut HeaderMap) {
    let nominated: Vec<HeaderName> = headers
        .get_all(CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|name| HeaderName::from_bytes(name.trim().as_bytes()).ok())
        .collect();
    for name in nominated {
        headers.remove(name);
    }
    for name in HOP_BY_HOP {
        headers.remove(name);
    }
}

fn respond(status: StatusCode, message: &str) -> Reply {
    Reply::new(status, Outgoing::Full(Bytes::from(format!("{message}\n"))))
}
