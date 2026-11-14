use std::{
    cell::{Cell, RefCell},
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    ops::Range,
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use nanocode_http::{header, wire};
use nanocode_sandbox::egress::{
    Caching, Credential, Dial, Dialing, Fixed, Grant, Identity, Injecting, Limits, Proxy, Reach,
    Resolving, Rules, Texts, Tls, Upstream,
};
use nanocode_sync::Atom;
use rustls::RootCertStore;

use crate::{
    executor::{Clock, Ended, Executor, Task},
    rng::{FNV_OFFSET, Rng, fnv},
    socket::{self, ServerSide},
};

pub const RUN_MS_MAX: u64 = 30 * 60 * 1000;
const REQUESTS_MAX: u64 = 5;
const PAYLOAD_BYTES_MAX: u64 = 6000;
const PATH_SEGMENTS_MAX: u64 = 3;
const CONNECT_PPM: u64 = 450_000;
const POST_PPM: u64 = 400_000;
const PIECE_MAX: u64 = 4096;
const TINY_PIECES_PPM: u64 = 200_000;
const TINY_PIECE_MAX: u64 = 7;
const LATENCY_MS_MAX: u64 = 400;
const CORRUPT_BITS: u8 = 0x10;
const READ_BYTES: usize = 8192;
const BYTE_VALUES: u64 = 256;
const LETTERS: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const IDLE_HEADERS: Duration = Duration::from_secs(30);
const IDLE_TUNNEL: Duration = Duration::from_secs(120);
const CONNECTIONS_MAX: usize = 64;
const TUNNEL_PORT: u16 = 443;
const PLAIN_PORT: u16 = 80;
const SOURCE: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
const STATUS_OK: u16 = 200;
const STATUS_FORBIDDEN: u16 = 403;
const STATUS_BAD_GATEWAY: u16 = 502;
const ORIGIN_STATUSES: [u16; 4] = [200, 201, 404, 500];
const HOP_HEADER: &str = "proxy-connection";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EgressMutation {
    None,
    WidenAllowlist,
    CorruptTunnelByte,
}

impl EgressMutation {
    const ALL: [Self; 3] = [Self::None, Self::WidenAllowlist, Self::CorruptTunnelByte];

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|mutation| mutation.name() == name)
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::WidenAllowlist => "widen_allowlist",
            Self::CorruptTunnelByte => "corrupt_tunnel_byte",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Public,
    PrivateListed,
    PrivateRefused,
    Denied,
    Unresolvable,
    Unreachable,
}

struct HostEntry {
    name: &'static str,
    class: Class,
    address: Option<Ipv4Addr>,
}

const HOSTS: [HostEntry; 7] = [
    HostEntry {
        name: "api.sim",
        class: Class::Public,
        address: Some(Ipv4Addr::new(93, 184, 0, 1)),
    },
    HostEntry {
        name: "files.sim",
        class: Class::Public,
        address: Some(Ipv4Addr::new(93, 184, 0, 2)),
    },
    HostEntry {
        name: "db.sim",
        class: Class::PrivateListed,
        address: Some(Ipv4Addr::new(10, 0, 0, 9)),
    },
    HostEntry {
        name: "intra.sim",
        class: Class::PrivateRefused,
        address: Some(Ipv4Addr::new(10, 0, 0, 5)),
    },
    HostEntry {
        name: "evil.sim",
        class: Class::Denied,
        address: Some(Ipv4Addr::new(93, 184, 0, 66)),
    },
    HostEntry {
        name: "ghost.sim",
        class: Class::Unresolvable,
        address: None,
    },
    HostEntry {
        name: "down.sim",
        class: Class::Unreachable,
        address: Some(Ipv4Addr::new(93, 184, 0, 99)),
    },
];

fn allowed_hosts(mutation: EgressMutation) -> Vec<String> {
    HOSTS
        .iter()
        .filter(|host| host.class != Class::Denied || mutation == EgressMutation::WidenAllowlist)
        .map(|host| host.name.to_owned())
        .collect()
}

fn private_hosts() -> Vec<String> {
    HOSTS
        .iter()
        .filter(|host| host.class == Class::PrivateListed)
        .map(|host| host.name.to_owned())
        .collect()
}

fn host_at(address: SocketAddr) -> Option<&'static HostEntry> {
    HOSTS
        .iter()
        .find(|host| host.address.map(IpAddr::V4) == Some(address.ip()))
}

#[derive(Clone, Debug)]
enum Kind {
    Tunnel {
        payload: Vec<u8>,
    },
    Plain {
        post: bool,
        path: String,
        body: Vec<u8>,
        status: u16,
        answer: Vec<u8>,
    },
}

struct Request {
    host: &'static HostEntry,
    kind: Kind,
    latency_ms: u64,
}

struct Script {
    requests: Vec<Request>,
    read_piece_max: usize,
    write_piece_max: usize,
}

fn random_bytes(rng: &mut Rng, count: u64) -> Vec<u8> {
    (0..count)
        .map(|_| u8::try_from(rng.below(BYTE_VALUES)).expect("a byte value fits in u8"))
        .collect()
}

fn word(rng: &mut Rng) -> String {
    (0..rng.range(1, 8))
        .map(|_| char::from(LETTERS[rng.pick(LETTERS.len())]))
        .collect()
}

fn size(value: u64) -> usize {
    usize::try_from(value).expect("a simulated length fits in usize")
}

fn count(value: usize) -> u64 {
    u64::try_from(value).expect("a length fits in u64")
}

impl Script {
    fn of(seed: u64) -> Self {
        let mut rng = Rng::derive(seed, "egress-script");
        let requests = (0..rng.range(1, REQUESTS_MAX))
            .map(|index| {
                let host = &HOSTS[rng.pick(HOSTS.len())];
                let kind = if rng.chance(CONNECT_PPM) {
                    let length = rng.range(1, PAYLOAD_BYTES_MAX);
                    Kind::Tunnel {
                        payload: random_bytes(&mut rng, length),
                    }
                } else {
                    let post = rng.chance(POST_PPM);
                    let path = format!(
                        "/q{index}{}",
                        (0..rng.range(1, PATH_SEGMENTS_MAX))
                            .map(|_| format!("/{}", word(&mut rng)))
                            .collect::<String>()
                    );
                    let body = if post {
                        let length = rng.range(0, PAYLOAD_BYTES_MAX);
                        random_bytes(&mut rng, length)
                    } else {
                        Vec::new()
                    };
                    let status = ORIGIN_STATUSES[rng.pick(ORIGIN_STATUSES.len())];
                    let length = rng.range(0, PAYLOAD_BYTES_MAX);
                    Kind::Plain {
                        post,
                        path,
                        body,
                        status,
                        answer: random_bytes(&mut rng, length),
                    }
                };
                Request {
                    host,
                    kind,
                    latency_ms: rng.range(0, LATENCY_MS_MAX),
                }
            })
            .collect();
        let tiny = rng.chance(TINY_PIECES_PPM);
        let piece_max = if tiny { TINY_PIECE_MAX } else { PIECE_MAX };
        Self {
            requests,
            read_piece_max: size(rng.range(1, piece_max)),
            write_piece_max: size(rng.range(1, piece_max)),
        }
    }

    fn describe(&self) -> String {
        let requests = self
            .requests
            .iter()
            .map(|request| match &request.kind {
                Kind::Tunnel { payload } => {
                    format!("CONNECT {} {}B", request.host.name, payload.len())
                }
                Kind::Plain {
                    post,
                    path,
                    body,
                    status,
                    answer,
                } => format!(
                    "{} {}{path} {}B -> {status} {}B",
                    if *post { "POST" } else { "GET" },
                    request.host.name,
                    body.len(),
                    answer.len()
                ),
            })
            .collect::<Vec<_>>()
            .join(" | ");
        format!(
            "scenario=egress read_piece_max={} write_piece_max={} requests=[{requests}]",
            self.read_piece_max, self.write_piece_max
        )
    }
}

#[derive(Clone, Debug, Default)]
struct Dialed {
    host: String,
    received: Vec<u8>,
}

struct SimDial {
    seed: u64,
    connections: AtomicU64,
    dialed: Arc<Atom<Vec<Dialed>>>,
    plain: Arc<Vec<(String, u16, Vec<u8>)>>,
    clock: Arc<Clock>,
    latency_ms: u64,
    read_piece_max: usize,
    write_piece_max: usize,
    mutation: EgressMutation,
}

impl Dial for SimDial {
    fn resolve(&self, host: &str, port: u16) -> Resolving {
        let found = HOSTS
            .iter()
            .find(|entry| entry.name == host)
            .and_then(|entry| entry.address)
            .map(|address| vec![SocketAddr::new(IpAddr::V4(address), port)]);
        Box::pin(async move {
            found.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "sim: no such host"))
        })
    }

    fn connect(&self, address: SocketAddr) -> Dialing {
        let Some(entry) = host_at(address).filter(|entry| entry.class != Class::Unreachable) else {
            return Box::pin(async {
                Err(io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    "sim: connection refused",
                ))
            });
        };
        let number = self.connections.fetch_add(1, Ordering::SeqCst);
        let (client, server) = socket::pair(
            Rng::derive(self.seed ^ number, "egress-upstream"),
            self.read_piece_max,
            self.write_piece_max,
        );
        let index = self.dialed.update(|dialed| {
            dialed.push(Dialed {
                host: entry.name.to_owned(),
                received: Vec::new(),
            });
            dialed.len() - 1
        });
        let upstream = UpstreamTask {
            server,
            index,
            port: address.port(),
            host: entry.name.to_owned(),
            dialed: Arc::clone(&self.dialed),
            plain: Arc::clone(&self.plain),
            clock: Arc::clone(&self.clock),
            latency_ms: self.latency_ms,
            corrupt: self.mutation == EgressMutation::CorruptTunnelByte,
        };
        nanocode_platform::spawn(upstream.run());
        Box::pin(async move { Ok(Box::new(client) as Upstream) })
    }
}

struct UpstreamTask {
    server: ServerSide,
    index: usize,
    port: u16,
    host: String,
    dialed: Arc<Atom<Vec<Dialed>>>,
    plain: Arc<Vec<(String, u16, Vec<u8>)>>,
    clock: Arc<Clock>,
    latency_ms: u64,
    corrupt: bool,
}

impl UpstreamTask {
    fn record(&self, bytes: &[u8]) {
        let index = self.index;
        self.dialed
            .update(|dialed| dialed[index].received.extend_from_slice(bytes));
    }

    async fn run(self) {
        if self.port == TUNNEL_PORT {
            self.echo().await;
        } else {
            self.origin().await;
        }
    }

    async fn echo(self) {
        let mut chunk = [0_u8; READ_BYTES];
        let mut corrupted = !self.corrupt;
        loop {
            match self.server.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    self.record(&chunk[..read]);
                    if !corrupted {
                        chunk[0] ^= CORRUPT_BITS;
                        corrupted = true;
                    }
                    if self.server.write(&chunk[..read]).is_err() {
                        break;
                    }
                }
            }
        }
        self.server.close();
    }

    async fn origin(self) {
        let mut buffer = Vec::new();
        let mut chunk = [0_u8; READ_BYTES];
        let complete = loop {
            if let Some(length) = request_length(&buffer)
                && buffer.len() >= length
            {
                break true;
            }
            match self.server.read(&mut chunk).await {
                Ok(0) | Err(_) => break false,
                Ok(read) => {
                    self.record(&chunk[..read]);
                    buffer.extend_from_slice(&chunk[..read]);
                }
            }
        };
        if complete {
            self.clock
                .sleep(Duration::from_millis(self.latency_ms))
                .await;
            let target = request_target(&buffer).unwrap_or_default();
            let answer = self
                .plain
                .iter()
                .find(|(host, _, _)| *host == format!("{}{target}", self.host))
                .map_or_else(
                    || (404, Vec::new()),
                    |(_, status, answer)| (*status, answer.clone()),
                );
            let mut response = format!(
                "HTTP/1.1 {} Sim\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                answer.0,
                answer.1.len()
            )
            .into_bytes();
            response.extend_from_slice(&answer.1);
            drop(self.server.write(&response));
        }
        self.server.close();
    }
}

fn head_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|at| at + 4)
}

fn request_length(bytes: &[u8]) -> Option<usize> {
    let end = head_end(bytes)?;
    let head = std::str::from_utf8(&bytes[..end]).ok()?;
    let body = head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    Some(end + body)
}

fn request_target(bytes: &[u8]) -> Option<String> {
    let end = head_end(bytes)?;
    let head = std::str::from_utf8(&bytes[..end]).ok()?;
    head.lines().next()?.split(' ').nth(1).map(str::to_owned)
}

#[derive(Debug, PartialEq, Eq)]
enum Seen {
    Status(u16),
    Tunnel(Vec<u8>),
    Body(Vec<u8>),
    Failed(String),
}

struct Reader<'a> {
    side: &'a ServerSide,
    buffer: Vec<u8>,
    eof: bool,
}

impl Reader<'_> {
    async fn more(&mut self) -> bool {
        if self.eof {
            return false;
        }
        let mut chunk = [0_u8; READ_BYTES];
        match self.side.read(&mut chunk).await {
            Ok(0) | Err(_) => {
                self.eof = true;
                false
            }
            Ok(read) => {
                self.buffer.extend_from_slice(&chunk[..read]);
                true
            }
        }
    }

    async fn head(&mut self) -> Result<(u16, Option<usize>), String> {
        loop {
            if let Some((head, length)) =
                wire::parse_response(&self.buffer).map_err(|error| error.to_string())?
            {
                self.buffer.drain(..length);
                let declared = head
                    .headers
                    .get(header::CONTENT_LENGTH)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.trim().parse::<usize>().ok());
                return Ok((head.status.as_u16(), declared));
            }
            if !self.more().await {
                return Err(format!(
                    "the connection closed inside a response head ({} bytes)",
                    self.buffer.len()
                ));
            }
        }
    }

    async fn exactly(&mut self, length: usize) -> Vec<u8> {
        while self.buffer.len() < length && self.more().await {}
        let take = length.min(self.buffer.len());
        self.buffer.drain(..take).collect()
    }

    async fn rest(&mut self) -> Vec<u8> {
        while self.more().await {}
        std::mem::take(&mut self.buffer)
    }
}

async fn client(request: &Request, side: ServerSide, log: &RefCell<Vec<Seen>>, trace: &Trace) {
    let host = request.host.name;
    let mut reader = Reader {
        side: &side,
        buffer: Vec::new(),
        eof: false,
    };
    let push = |seen: Seen| log.borrow_mut().push(seen);
    match &request.kind {
        Kind::Tunnel { payload } => {
            let head = format!(
                "CONNECT {host}:{TUNNEL_PORT} HTTP/1.1\r\nhost: {host}:{TUNNEL_PORT}\r\n\r\n"
            );
            if side.write(head.as_bytes()).is_err() {
                return push(Seen::Failed(
                    "the proxy closed before the request".to_owned(),
                ));
            }
            match reader.head().await {
                Err(error) => push(Seen::Failed(error)),
                Ok((status, declared)) => {
                    push(Seen::Status(status));
                    if status == STATUS_OK {
                        if side.write(payload).is_ok() {
                            let echoed = reader.exactly(payload.len()).await;
                            push(Seen::Tunnel(echoed));
                        }
                    } else {
                        let body = match declared {
                            Some(length) => reader.exactly(length).await,
                            None => reader.rest().await,
                        };
                        push(Seen::Body(body));
                    }
                }
            }
        }
        Kind::Plain {
            post, path, body, ..
        } => {
            let method = if *post { "POST" } else { "GET" };
            let mut bytes = format!(
                "{method} http://{host}{path} HTTP/1.1\r\nhost: {host}\r\n{HOP_HEADER}: keep-alive\r\ncontent-length: {}\r\n\r\n",
                body.len()
            )
            .into_bytes();
            bytes.extend_from_slice(body);
            if side.write(&bytes).is_err() {
                return push(Seen::Failed(
                    "the proxy closed before the request".to_owned(),
                ));
            }
            match reader.head().await {
                Err(error) => push(Seen::Failed(error)),
                Ok((status, declared)) => {
                    push(Seen::Status(status));
                    let body = match declared {
                        Some(length) => reader.exactly(length).await,
                        None => reader.rest().await,
                    };
                    push(Seen::Body(body));
                }
            }
        }
    }
    trace.line(&format!("client for {host} saw {:?}", log.borrow().first()));
    side.close();
}

struct Trace {
    clock: Arc<Clock>,
    digest: Cell<u64>,
    verbose: bool,
}

impl Trace {
    fn line(&self, text: &str) {
        let now_ns = self.clock.now_ns();
        let digest = fnv(
            fnv(self.digest.get(), &now_ns.to_le_bytes()),
            text.as_bytes(),
        );
        self.digest.set(digest);
        if self.verbose {
            println!("  {now_ns:>14} {text}");
        }
    }
}

fn private_text(host: &str) -> String {
    format!("{host} resolves only to private addresses.")
}

fn refused_text(host: &str) -> String {
    format!("{host} is not on the allowlist.")
}

fn proxy(mutation: EgressMutation, dial: Arc<SimDial>) -> Result<Arc<Proxy<Fixed>>, String> {
    let tls = Tls::with_roots("nanocode-sim", RootCertStore::empty())
        .map_err(|error| format!("egress CA: {error}"))?;
    let rules = Rules {
        caching: Caching {
            granted: Duration::ZERO,
            refused: Duration::ZERO,
            recheck: Duration::ZERO,
            gather: Duration::ZERO,
            most: 0,
            flight: Duration::ZERO,
        },
        reporting: None,
        identity: Identity::Fixed(Credential {
            name: "sim".into(),
            token: String::new(),
        }),
        reach: Reach::Listed {
            hosts: allowed_hosts(mutation),
            private_hosts: private_hosts(),
            tailnet: None,
        },
        passthrough: Vec::new(),
        injecting: Injecting::Set,
        intercept_port: None,
        texts: Texts {
            client: "sim",
            needs_credential: "",
            realm: "",
            unreachable: "",
            stale: "",
            private: private_text,
            refused: refused_text,
            compressed: "compressed",
        },
        limits: Limits {
            connections: CONNECTIONS_MAX,
            per_source: CONNECTIONS_MAX,
            connect: CONNECT_TIMEOUT,
            idle_headers: IDLE_HEADERS,
            idle_tunnel: IDLE_TUNNEL,
        },
    };
    let fixed = Arc::new(Fixed(Grant {
        secrets: Vec::new(),
    }));
    Ok(Arc::new(
        Proxy::new(fixed, Arc::new(tls), rules).dialing(dial),
    ))
}

fn expect(request: &Request) -> Vec<Seen> {
    let host = request.host.name;
    let refusal = |status: u16, text: String| {
        vec![
            Seen::Status(status),
            Seen::Body(format!("{text}\n").into_bytes()),
        ]
    };
    match request.host.class {
        Class::Denied => refusal(STATUS_FORBIDDEN, refused_text(host)),
        Class::PrivateRefused => refusal(STATUS_FORBIDDEN, private_text(host)),
        Class::Unresolvable => refusal(STATUS_BAD_GATEWAY, format!("{host} does not resolve.")),
        Class::Unreachable => {
            let port = match request.kind {
                Kind::Tunnel { .. } => TUNNEL_PORT,
                Kind::Plain { .. } => PLAIN_PORT,
            };
            refusal(
                STATUS_BAD_GATEWAY,
                format!("{host}:{port} can't be reached."),
            )
        }
        Class::Public | Class::PrivateListed => match &request.kind {
            Kind::Tunnel { payload } => {
                vec![Seen::Status(STATUS_OK), Seen::Tunnel(payload.clone())]
            }
            Kind::Plain { status, answer, .. } => {
                vec![Seen::Status(*status), Seen::Body(answer.clone())]
            }
        },
    }
}

const fn reaches(request: &Request) -> bool {
    matches!(request.host.class, Class::Public | Class::PrivateListed)
}

fn check_upstream(request: &Request, received: &[u8]) -> Result<(), String> {
    match &request.kind {
        Kind::Tunnel { payload } => {
            if received != payload.as_slice() {
                return Err(format!(
                    "the upstream of {} received {} tunnelled bytes where the client sent {}",
                    request.host.name,
                    received.len(),
                    payload.len()
                ));
            }
        }
        Kind::Plain {
            post, path, body, ..
        } => {
            let end = head_end(received)
                .ok_or_else(|| format!("the origin {} got no request head", request.host.name))?;
            let head = String::from_utf8_lossy(&received[..end]).to_ascii_lowercase();
            let method = if *post { "post" } else { "get" };
            let line = format!("{method} {path} http/1.1\r\n");
            if !head.starts_with(&line) {
                return Err(format!(
                    "the origin {} got {:?} where the proxy owes {line:?}",
                    request.host.name,
                    head.lines().next().unwrap_or_default()
                ));
            }
            if !head.contains(&format!("\r\nhost: {}\r\n", request.host.name)) {
                return Err(format!(
                    "the origin {} got no host header naming it",
                    request.host.name
                ));
            }
            if head.contains(&format!("\r\n{HOP_HEADER}:")) {
                return Err(format!(
                    "the origin {} got the hop-by-hop {HOP_HEADER} header",
                    request.host.name
                ));
            }
            if &received[end..] != body.as_slice() {
                return Err(format!(
                    "the origin {} got a body of {} bytes where the client sent {}",
                    request.host.name,
                    received.len() - end,
                    body.len()
                ));
            }
        }
    }
    Ok(())
}

fn judge(
    script: &Script,
    seen: &[Vec<Seen>],
    dialed: &[Dialed],
    ended: Ended,
    now_ns: u64,
) -> Result<(), String> {
    if ended == Ended::Stuck {
        return Err(format!(
            "every task waits and no timer is set: a hang (client saw {seen:?})"
        ));
    }
    let run_ms = now_ns / 1_000_000;
    if run_ms > RUN_MS_MAX {
        return Err(format!(
            "the run took {run_ms} ms of virtual time, over {RUN_MS_MAX} ms"
        ));
    }
    for (index, request) in script.requests.iter().enumerate() {
        let wanted = expect(request);
        if seen[index] != wanted {
            let show = |list: &[Seen]| {
                list.iter()
                    .map(|seen| match seen {
                        Seen::Status(status) => format!("status {status}"),
                        Seen::Tunnel(bytes) => format!("tunnel {}B", bytes.len()),
                        Seen::Body(bytes) => format!(
                            "body {:?}",
                            String::from_utf8_lossy(&bytes[..bytes.len().min(80)])
                        ),
                        Seen::Failed(error) => format!("failed {error}"),
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            return Err(format!(
                "request {index} to {}: the client saw [{}] where the policy says [{}]",
                request.host.name,
                show(&seen[index]),
                show(&wanted)
            ));
        }
    }
    for host in HOSTS.iter() {
        let owed = script
            .requests
            .iter()
            .filter(|request| request.host.name == host.name && reaches(request))
            .collect::<Vec<_>>();
        let reached = dialed
            .iter()
            .filter(|dialed| dialed.host == host.name)
            .collect::<Vec<_>>();
        if owed.len() != reached.len() {
            return Err(format!(
                "the proxy opened {} upstream connections to {} where the policy allows {}",
                reached.len(),
                host.name,
                owed.len()
            ));
        }
        let mut unmatched: Vec<&Dialed> = reached;
        for request in owed {
            let at = unmatched
                .iter()
                .position(|dialed| check_upstream(request, &dialed.received).is_ok())
                .ok_or_else(|| {
                    unmatched.first().map_or_else(String::new, |dialed| {
                        check_upstream(request, &dialed.received)
                            .err()
                            .unwrap_or_default()
                    })
                })?;
            unmatched.remove(at);
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default)]
pub struct EgressStats {
    pub seeds: u64,
    pub tunnels: u64,
    pub plain: u64,
    pub refused: u64,
    pub upstream: u64,
}

impl EgressStats {
    pub const fn add(&mut self, other: &Self) {
        self.seeds += other.seeds;
        self.tunnels += other.tunnels;
        self.plain += other.plain;
        self.refused += other.refused;
        self.upstream += other.upstream;
    }
}

pub struct EgressOutcome {
    pub digest: u64,
    pub wrong: Option<String>,
    pub stats: EgressStats,
    pub shape: String,
}

fn simulate(seed: u64, mutation: EgressMutation, verbose: bool) -> EgressOutcome {
    let script = Script::of(seed);
    let shape = script.describe();
    let clock = Clock::new();
    crate::host::enter(seed, &clock);
    let trace = Trace {
        clock: Arc::clone(&clock),
        digest: Cell::new(FNV_OFFSET),
        verbose,
    };
    trace.line(&format!("seed {seed} {shape}"));
    let plain = script
        .requests
        .iter()
        .filter_map(|request| match &request.kind {
            Kind::Plain {
                path,
                status,
                answer,
                ..
            } => Some((
                format!("{}{path}", request.host.name),
                *status,
                answer.clone(),
            )),
            Kind::Tunnel { .. } => None,
        })
        .collect::<Vec<_>>();
    let dialed = Arc::new(Atom::new(Vec::new()));
    let dial = Arc::new(SimDial {
        seed,
        connections: AtomicU64::new(0),
        dialed: Arc::clone(&dialed),
        plain: Arc::new(plain),
        clock: Arc::clone(&clock),
        latency_ms: script
            .requests
            .first()
            .map_or(0, |request| request.latency_ms),
        read_piece_max: script.read_piece_max,
        write_piece_max: script.write_piece_max,
        mutation,
    });
    let proxy = match proxy(mutation, dial) {
        Ok(proxy) => proxy,
        Err(error) => {
            return EgressOutcome {
                digest: 0,
                wrong: Some(error),
                stats: EgressStats::default(),
                shape,
            };
        }
    };
    let logs = script
        .requests
        .iter()
        .map(|_| RefCell::new(Vec::new()))
        .collect::<Vec<_>>();
    let mut tasks: Vec<Task<'_>> = Vec::new();
    for (index, request) in script.requests.iter().enumerate() {
        let (inbound, side) = socket::pair(
            Rng::derive(seed ^ count(index), "egress-inbound"),
            script.read_piece_max,
            script.write_piece_max,
        );
        let slot: nanocode_sandbox::egress::Slot = Arc::new(());
        tasks.push(Box::pin(Arc::clone(&proxy).serve(inbound, slot, SOURCE)));
        tasks.push(Box::pin(client(request, side, &logs[index], &trace)));
    }
    let ended = Executor::new(Arc::clone(&clock), Rng::derive(seed, "scheduler")).run(tasks, None);
    drop(proxy);
    let seen = logs
        .into_iter()
        .map(RefCell::into_inner)
        .collect::<Vec<_>>();
    let dialed = dialed.get().as_ref().clone();
    let wrong = judge(&script, &seen, &dialed, ended, clock.now_ns()).err();
    trace.line(&format!("verdict {wrong:?}"));
    let stats = EgressStats {
        seeds: 1,
        tunnels: count(
            script
                .requests
                .iter()
                .filter(|request| matches!(request.kind, Kind::Tunnel { .. }))
                .count(),
        ),
        plain: count(
            script
                .requests
                .iter()
                .filter(|request| matches!(request.kind, Kind::Plain { .. }))
                .count(),
        ),
        refused: count(
            script
                .requests
                .iter()
                .filter(|request| !reaches(request))
                .count(),
        ),
        upstream: count(dialed.len()),
    };
    EgressOutcome {
        digest: trace.digest.get(),
        wrong,
        stats,
        shape,
    }
}

pub fn run_seed(seed: u64, mutation: EgressMutation, verbose: bool) -> EgressOutcome {
    std::panic::catch_unwind(|| simulate(seed, mutation, verbose)).unwrap_or_else(|panic| {
        let message = panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|text| (*text).to_owned()))
            .unwrap_or_default();
        EgressOutcome {
            digest: 0,
            wrong: Some(format!("panic: {message}")),
            stats: EgressStats::default(),
            shape: String::new(),
        }
    })
}

pub fn judge_seed(
    seed: u64,
    mutation: EgressMutation,
    twice: bool,
    verbose: bool,
    totals: &mut EgressStats,
) -> Option<String> {
    let first = run_seed(seed, mutation, verbose);
    totals.add(&first.stats);
    if verbose {
        println!("seed {seed} digest {:016x} {}", first.digest, first.shape);
    }
    if first.wrong.is_some() || !twice {
        return first
            .wrong
            .map(|message| format!("{} :: {message}", first.shape));
    }
    let second = run_seed(seed, mutation, false);
    (second.digest != first.digest || second.wrong.is_some()).then(|| {
        format!(
            "{} :: nondeterminism: trace digest {:016x} then {:016x}",
            first.shape, first.digest, second.digest
        )
    })
}

pub fn replay(seed: u64, mutation: EgressMutation) -> String {
    let mut command = format!("nanocode-sim --scenario egress --seed {seed}");
    if mutation != EgressMutation::None {
        command.push_str(" --mutation ");
        command.push_str(mutation.name());
    }
    command
}

pub fn report(
    seeds: &Range<u64>,
    passed: u64,
    failed: u64,
    twice: bool,
    mutation: EgressMutation,
    totals: &EgressStats,
) -> ExitCode {
    println!(
        "RESULT seeds={}..{} scenario=egress mutation={} passed={passed} failed={failed} twice={twice} stats={totals:?}",
        seeds.start,
        seeds.end,
        mutation.name()
    );
    match mutation {
        EgressMutation::None if failed == 0 => ExitCode::SUCCESS,
        EgressMutation::None => ExitCode::FAILURE,
        _ if failed > 0 => {
            println!("MUTATION {} caught by {failed} seeds", mutation.name());
            ExitCode::SUCCESS
        }
        _ => {
            println!("MUTATION {} NOT caught", mutation.name());
            ExitCode::FAILURE
        }
    }
}
