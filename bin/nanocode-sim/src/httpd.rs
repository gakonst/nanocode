use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use nanocode_compression::miniz::{Format, Level, compress};
use nanocode_http::{
    Connect, Connecting,
    header::{HeaderMap, HeaderValue},
    wire::{self, HEAD_BYTES_MAX, HEADERS_MAX, RequestHead},
};
use nanocode_sync::Slot;

use crate::{
    rng::{PPM, Rng},
    server::{ModelServer, Reply},
    socket::{self, ServerSide},
    world::World,
};

pub const CALM_PPM: u64 = 300_000;
pub const REFUSE_PPM_MAX: u64 = 40_000;
pub const CONNECT_HANG_PPM_MAX: u64 = 20_000;
pub const DROP_PPM_MAX: u64 = 40_000;
pub const LOST_PPM_MAX: u64 = 30_000;
pub const STATUS_PPM_MAX: u64 = 80_000;
pub const REDIRECT_PPM_MAX: u64 = 60_000;
pub const RESET_PPM_MAX: u64 = 40_000;
pub const CUT_PPM_MAX: u64 = 40_000;
pub const STALL_PPM_MAX: u64 = 20_000;
pub const SLOW_HEAD_PPM_MAX: u64 = 30_000;
pub const MALFORMED_PPM_MAX: u64 = 40_000;
pub const DUPLICATE_PPM_MAX: u64 = 30_000;
pub const INFORMATIONAL_PPM: u64 = 100_000;
pub const JUNK_PPM: u64 = 50_000;
pub const LINGER_PPM: u64 = 300_000;
pub const IDLE_CLOSE_PPM: u64 = 300_000;
pub const TINY_PIECES_PPM: u64 = 200_000;
pub const TINY_PIECE_BYTES_MAX: u64 = 7;
pub const LATENCY_MS_MIN: u64 = 20;
pub const LATENCY_MS_MAX: u64 = 3_000;
pub const CONNECT_HANG_MS_MIN: u64 = 10_000;
pub const CONNECT_HANG_MS_MAX: u64 = 60_000;
pub const SLOW_HEAD_MS_MAX: u64 = 180_000;
pub const STALL_MS_MIN: u64 = 121_000;
pub const STALL_MS_MAX: u64 = 600_000;
pub const IDLE_CLOSE_MS_MAX: u64 = 5_000;
pub const IDLE_KEEP_MS: u64 = 600_000;
pub const READ_TIMEOUT_SLACK_MS: u64 = 1_000;
pub const LINGER_MS_MAX: u64 = 2_000;
pub const PIECE_GAP_MS_MAX: u64 = 400;
pub const PIECES_MAX: u64 = 8;
pub const RETRY_AFTER_MS_MAX: u64 = 5_000;
pub const RETRY_AFTER_SECONDS_MAX: u64 = 4;
pub const REDIRECT_HOPS_MAX: u64 = 3;
pub const BROTLI_BLOCK_BYTES: usize = 1 << 16;
pub const ZSTD_BLOCK_BYTES: usize = 1 << 16;
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];
const ZSTD_WINDOW_DESCRIPTOR: u8 = 7 << 3;
const ZSTD_LAST_BLOCK: u32 = 1;
const ZSTD_BLOCK_SIZE_SHIFT: u32 = 3;
const GZIP_LEVEL: u32 = 6;
const STATUSES: [u16; 5] = [429, 500, 502, 503, 529];
const REDIRECTS: [u16; 2] = [307, 308];
const HOSTS: [&str; 2] = ["model.sim", "mirror.sim"];
const OVERSIZED_HEADER_BYTES: usize = 8_000;
const REQUEST_BYTES_MAX: usize = 64 << 20;
const READ_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireMutation {
    None,
    ReuseDirty,
    NoReadTimeout,
    ToolResultTwice,
}

impl WireMutation {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "http-reuse-dirty" => Some(Self::ReuseDirty),
            "http-no-read-timeout" => Some(Self::NoReadTimeout),
            "tool-result-twice" => Some(Self::ToolResultTwice),
            _ => None,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::ReuseDirty => "http-reuse-dirty",
            Self::NoReadTimeout => "http-no-read-timeout",
            Self::ToolResultTwice => "tool-result-twice",
        }
    }

    pub const NAMES: &'static str = "http-reuse-dirty|http-no-read-timeout|tool-result-twice";
}

#[derive(Clone, Copy, Debug)]
pub struct HttpFaults {
    pub refuse_ppm: u64,
    pub connect_hang_ppm: u64,
    pub drop_ppm: u64,
    pub lost_ppm: u64,
    pub status_ppm: u64,
    pub redirect_ppm: u64,
    pub reset_ppm: u64,
    pub cut_ppm: u64,
    pub stall_ppm: u64,
    pub slow_head_ppm: u64,
    pub malformed_ppm: u64,
    pub duplicate_ppm: u64,
    pub latency_ms_max: u64,
}

impl HttpFaults {
    pub fn of(seed: u64, calm: bool) -> Self {
        let mut rng = Rng::derive(seed, "http-faults");
        let latency_ms_max = rng.range(LATENCY_MS_MIN, LATENCY_MS_MAX);
        let calm = calm || rng.chance(CALM_PPM);
        let mut draw = |max| if calm { 0 } else { rng.range(0, max) };
        Self {
            refuse_ppm: draw(REFUSE_PPM_MAX),
            connect_hang_ppm: draw(CONNECT_HANG_PPM_MAX),
            drop_ppm: draw(DROP_PPM_MAX),
            lost_ppm: draw(LOST_PPM_MAX),
            status_ppm: draw(STATUS_PPM_MAX),
            redirect_ppm: draw(REDIRECT_PPM_MAX),
            reset_ppm: draw(RESET_PPM_MAX),
            cut_ppm: draw(CUT_PPM_MAX),
            stall_ppm: draw(STALL_PPM_MAX),
            slow_head_ppm: draw(SLOW_HEAD_PPM_MAX),
            malformed_ppm: draw(MALFORMED_PPM_MAX),
            duplicate_ppm: draw(DUPLICATE_PPM_MAX),
            latency_ms_max,
        }
    }

    pub const fn calm(&self) -> bool {
        self.refuse_ppm == 0
            && self.connect_hang_ppm == 0
            && self.drop_ppm == 0
            && self.lost_ppm == 0
            && self.status_ppm == 0
            && self.redirect_ppm == 0
            && self.reset_ppm == 0
            && self.cut_ppm == 0
            && self.stall_ppm == 0
            && self.slow_head_ppm == 0
            && self.malformed_ppm == 0
            && self.duplicate_ppm == 0
    }
}

#[derive(Clone, Copy, Debug)]
enum Coding {
    Identity,
    Gzip,
    Deflate,
    Brotli,
    Zstd,
}

const CODINGS: [Coding; 5] = [
    Coding::Identity,
    Coding::Gzip,
    Coding::Deflate,
    Coding::Brotli,
    Coding::Zstd,
];

#[derive(Clone, Copy, Debug)]
enum Framing {
    Length,
    Chunked,
    Close,
}

const FRAMINGS: [Framing; 3] = [Framing::Length, Framing::Chunked, Framing::Close];

#[derive(Clone, Copy, Debug)]
enum Malformed {
    StatusLine,
    ContentLength,
    ConflictingLength,
    ChunkSize,
    ChunkEnd,
    CodingHeader,
    TooManyHeaders,
    HeadTooLarge,
}

const MALFORMED: [Malformed; 8] = [
    Malformed::StatusLine,
    Malformed::ContentLength,
    Malformed::ConflictingLength,
    Malformed::ChunkSize,
    Malformed::ChunkEnd,
    Malformed::CodingHeader,
    Malformed::TooManyHeaders,
    Malformed::HeadTooLarge,
];

#[derive(Clone, Copy, Debug)]
enum Break {
    Reset,
    Cut,
    Stall(u64),
}

#[derive(Debug)]
enum Fate {
    Drop,
    Status(u16),
    Redirect(u16, &'static str),
    Lost,
    Answer {
        duplicate: bool,
        coding: Coding,
        framing: Framing,
        slow_head_ms: Option<u64>,
        malformed: Option<Malformed>,
        broken: Option<(Break, u64)>,
        informational: bool,
        junk: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Previous {
    Fresh,
    Complete,
    Closing(&'static str),
    Dirty(&'static str),
}

pub struct HttpServer {
    seed: u64,
    model: ModelServer,
    world: Arc<World>,
    faults: HttpFaults,
    mutation: WireMutation,
    rng: Slot<Rng>,
    connections: AtomicU64,
    dirty: Slot<&'static str>,
    read_timeout: Duration,
}

impl HttpServer {
    pub fn new(
        seed: u64,
        world: Arc<World>,
        faults: HttpFaults,
        mutation: WireMutation,
        read_timeout: Duration,
    ) -> Self {
        Self {
            seed,
            model: ModelServer::new(
                seed,
                Arc::clone(&world),
                mutation == WireMutation::ToolResultTwice,
            ),
            world,
            faults,
            mutation,
            rng: Slot::new(Some(Rng::derive(seed, "http"))),
            connections: AtomicU64::new(0),
            dirty: Slot::new(None),
            read_timeout,
        }
    }

    pub const fn mutation(&self) -> WireMutation {
        self.mutation
    }

    async fn wait_out(&self, side: &ServerSide, wait_ms: u64, what: &str) -> bool {
        nanocode_platform::sleep(Duration::from_millis(wait_ms)).await;
        let bound = self.read_timeout + Duration::from_millis(READ_TIMEOUT_SLACK_MS);
        let waited = Duration::from_millis(wait_ms);
        if waited > bound && !side.client_gone() {
            self.world.wrong(format!(
                "the client still waited on {what} after {} ms, past its read timeout of {} ms",
                waited.as_millis(),
                self.read_timeout.as_millis()
            ));
        }
        !side.client_gone()
    }

    fn draw<R>(&self, use_rng: impl FnOnce(&mut Rng) -> R) -> R {
        let mut rng = self
            .rng
            .take()
            .expect("the simulator polls one task at a time, so the server is never taken twice");
        let result = use_rng(&mut rng);
        assert!(self.rng.put(rng).is_none(), "the server has one home");
        result
    }

    fn fate(&self, hop: u64) -> Fate {
        let faults = self.faults;
        self.draw(|rng| {
            if rng.chance(faults.drop_ppm) {
                return Fate::Drop;
            }
            if rng.chance(faults.status_ppm) {
                return Fate::Status(STATUSES[rng.pick(STATUSES.len())]);
            }
            if hop < REDIRECT_HOPS_MAX && rng.chance(faults.redirect_ppm) {
                return Fate::Redirect(
                    REDIRECTS[rng.pick(REDIRECTS.len())],
                    HOSTS[rng.pick(HOSTS.len())],
                );
            }
            if rng.chance(faults.lost_ppm) {
                return Fate::Lost;
            }
            let broken = if rng.chance(faults.reset_ppm) {
                Some(Break::Reset)
            } else if rng.chance(faults.cut_ppm) {
                Some(Break::Cut)
            } else if rng.chance(faults.stall_ppm) {
                Some(Break::Stall(rng.range(STALL_MS_MIN, STALL_MS_MAX)))
            } else {
                None
            };
            Fate::Answer {
                duplicate: rng.chance(faults.duplicate_ppm),
                coding: CODINGS[rng.pick(CODINGS.len())],
                framing: FRAMINGS[rng.pick(FRAMINGS.len())],
                slow_head_ms: rng
                    .chance(faults.slow_head_ppm)
                    .then(|| rng.range(faults.latency_ms_max, SLOW_HEAD_MS_MAX)),
                malformed: rng
                    .chance(faults.malformed_ppm)
                    .then(|| MALFORMED[rng.pick(MALFORMED.len())]),
                broken: broken.map(|broken| (broken, rng.below(PPM))),
                informational: rng.chance(INFORMATIONAL_PPM),
                junk: rng.chance(JUNK_PPM),
            }
        })
    }

    async fn deliver(&self, side: &ServerSide, bytes: &[u8], cut_max: usize) -> io::Result<()> {
        assert!(
            cut_max <= bytes.len(),
            "a response is cut only inside itself"
        );
        let (cuts, gaps) = self.draw(|rng| {
            let count = rng.range(1, PIECES_MAX);
            let gaps: Vec<u64> = (0..count).map(|_| rng.range(0, PIECE_GAP_MS_MAX)).collect();
            let mut cuts: Vec<u64> = (1..count).map(|_| rng.below(PPM)).collect();
            cuts.sort_unstable();
            (cuts, gaps)
        });
        let length = u64::try_from(cut_max).expect("a response length fits in u64");
        let mut from = 0;
        for (index, gap_ms) in gaps.into_iter().enumerate() {
            let cut = cuts.get(index).map_or(bytes.len(), |cut_ppm| {
                usize::try_from(length * cut_ppm / PPM)
                    .expect("a cut inside a response fits in usize")
            });
            nanocode_platform::sleep(Duration::from_millis(gap_ms)).await;
            if cut > from {
                side.write(&bytes[from..cut])?;
                from = cut;
            }
        }
        assert_eq!(from, bytes.len(), "every piece of a response is delivered");
        Ok(())
    }
}

pub struct Connector(pub Arc<HttpServer>);

impl Connect for Connector {
    fn connect(&self, host: &str, port: u16, secure: bool) -> Connecting {
        let server = Arc::clone(&self.0);
        let host = host.to_owned();
        Box::pin(async move {
            assert!(!secure, "the simulated model speaks plain HTTP");
            assert_eq!(port, 80, "the simulated model listens on port 80");
            if !HOSTS.contains(&host.as_str()) {
                server
                    .world
                    .wrong(format!("the client connected to an unknown host {host}"));
            }
            let number = server.connections.fetch_add(1, Ordering::SeqCst);
            let faults = server.faults;
            let (latency_ms, refuse, hang_ms, tiny_read, tiny_write) = server.draw(|rng| {
                (
                    rng.range(0, faults.latency_ms_max),
                    rng.chance(faults.refuse_ppm),
                    rng.chance(faults.connect_hang_ppm)
                        .then(|| rng.range(CONNECT_HANG_MS_MIN, CONNECT_HANG_MS_MAX)),
                    rng.chance(TINY_PIECES_PPM),
                    rng.chance(TINY_PIECES_PPM),
                )
            });
            server.world.count(|counts| counts.connections += 1);
            if let Some(hang_ms) = hang_ms {
                server.world.count(|counts| counts.faults += 1);
                server
                    .world
                    .trace(&format!("http connect {number} hangs {hang_ms} ms"));
                nanocode_platform::sleep(Duration::from_millis(hang_ms)).await;
                return Err(io::Error::new(io::ErrorKind::TimedOut, "sim: connect hung"));
            }
            nanocode_platform::sleep(Duration::from_millis(latency_ms)).await;
            if refuse {
                server.world.count(|counts| counts.faults += 1);
                server
                    .world
                    .trace(&format!("http connect {number} refused"));
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    "sim: connection refused",
                ));
            }
            let previous = match server.dirty.take() {
                Some(why) => {
                    assert_eq!(
                        server.mutation,
                        WireMutation::ReuseDirty,
                        "only the mutation hands out a dirty connection"
                    );
                    server.world.trace(&format!(
                        "http connect {number} gets a connection left after {why}"
                    ));
                    Previous::Dirty(why)
                }
                None => Previous::Fresh,
            };
            let piece = |tiny: bool| {
                if tiny {
                    TINY_PIECE_BYTES_MAX
                } else {
                    READ_BYTES as u64
                }
            };
            let (client, side) = socket::pair(
                Rng::derive(server.seed, &format!("socket {number}")),
                usize::try_from(piece(tiny_read)).expect("a piece bound fits in usize"),
                usize::try_from(piece(tiny_write)).expect("a piece bound fits in usize"),
            );
            server
                .world
                .trace(&format!("http connect {number} to {host}"));
            nanocode_platform::spawn(serve(Arc::clone(&server), side, host, number, previous));
            Ok(Box::new(client) as nanocode_http::wire::Io)
        })
    }
}

struct Incoming {
    head: RequestHead,
    body: Vec<u8>,
}

async fn request(
    side: &ServerSide,
    buffer: &mut Vec<u8>,
    idle: Duration,
) -> Result<Option<Incoming>, String> {
    let mut read = vec![0_u8; READ_BYTES];
    let (head, used) = loop {
        if let Some(parsed) = wire::parse_request(buffer)
            .map_err(|error| format!("unparsable request head: {error}"))?
        {
            break parsed;
        }
        if buffer.len() > HEAD_BYTES_MAX {
            return Err(format!("a request head over {HEAD_BYTES_MAX} bytes"));
        }
        let count = match nanocode_platform::timeout(idle, side.read(&mut read)).await {
            Err(_) | Ok(Err(_)) => return Ok(None),
            Ok(Ok(count)) => count,
        };
        if count == 0 {
            return Ok(None);
        }
        buffer.extend_from_slice(&read[..count]);
    };
    buffer.drain(..used);
    let length = match wire::request_framing(&head.headers) {
        Ok(wire::Framing::Empty) => 0,
        Ok(wire::Framing::Length(length)) => {
            usize::try_from(length).map_err(|_| "a request body over usize".to_owned())?
        }
        Ok(framing) => return Err(format!("a request framed {framing:?}")),
        Err(error) => return Err(format!("request framing: {error}")),
    };
    if length > REQUEST_BYTES_MAX {
        return Err(format!("a request body of {length} bytes"));
    }
    while buffer.len() < length {
        let count = side.read(&mut read).await.unwrap_or(0);
        if count == 0 {
            return Ok(None);
        }
        buffer.extend_from_slice(&read[..count]);
    }
    let body = buffer.drain(..length).collect();
    Ok(Some(Incoming { head, body }))
}

fn hop_of(target: &str) -> u64 {
    target
        .split_once("hop=")
        .and_then(|(_, hop)| hop.parse().ok())
        .unwrap_or(0)
}

impl HttpServer {
    fn check_request(&self, host: &str, incoming: &Incoming, number: u64) -> Option<String> {
        let head = &incoming.head;
        if head.method != nanocode_http::Method::POST {
            return Some(format!("connection {number} got a {} request", head.method));
        }
        if !head.target.starts_with("/v1/messages") {
            return Some(format!(
                "connection {number} got a request for {}",
                head.target
            ));
        }
        let header = |name: &str| head.headers.get(name).and_then(|value| value.to_str().ok());
        if header("host") != Some(host) {
            return Some(format!(
                "connection {number} to {host} got host {:?}",
                header("host")
            ));
        }
        if header("x-api-key") != Some("sk-sim") {
            return Some(format!(
                "connection {number} got x-api-key {:?}",
                header("x-api-key")
            ));
        }
        if header("anthropic-version").is_none() {
            return Some(format!("connection {number} got no anthropic-version"));
        }
        if header("content-encoding").is_some() {
            return Some(format!(
                "connection {number} got a request with content-encoding {:?}",
                header("content-encoding")
            ));
        }
        None
    }

    fn respond(&self, body: &[u8], duplicate: bool) -> Result<Reply, String> {
        let body = std::str::from_utf8(body)
            .map_err(|error| format!("a request body that is not UTF-8: {error}"))?;
        if duplicate {
            self.world.count(|counts| counts.faults += 1);
            self.world.trace("http duplicate processing");
            drop(self.model.respond(body));
        }
        self.model.respond(body)
    }
}

async fn serve(
    server: Arc<HttpServer>,
    side: ServerSide,
    host: String,
    number: u64,
    mut previous: Previous,
) {
    let mut buffer = Vec::new();
    let mut idle = Duration::from_millis(IDLE_KEEP_MS);
    loop {
        let incoming = match request(&side, &mut buffer, idle).await {
            Ok(Some(incoming)) => incoming,
            Ok(None) => {
                side.close();
                return;
            }
            Err(error) => {
                server.world.wrong(format!("connection {number}: {error}"));
                side.close();
                return;
            }
        };
        if let Previous::Dirty(why) | Previous::Closing(why) = previous {
            server.world.wrong(format!(
                "connection {number} carried a new request after {why}"
            ));
            side.close();
            return;
        }
        if previous == Previous::Complete {
            server.world.count(|counts| counts.reuses += 1);
        }
        if let Some(problem) = server.check_request(&host, &incoming, number) {
            server.world.wrong(problem);
        }
        let Some(after) = server.exchange(&side, &incoming, number).await else {
            return;
        };
        previous = after;
        idle = match previous {
            Previous::Fresh => unreachable!("an exchange never leaves a fresh connection"),
            Previous::Complete => server.draw(|rng| {
                let idle_ms = if rng.chance(IDLE_CLOSE_PPM) {
                    rng.range(0, IDLE_CLOSE_MS_MAX)
                } else {
                    IDLE_KEEP_MS
                };
                Duration::from_millis(idle_ms)
            }),
            Previous::Closing(_) => {
                let linger_ms =
                    server.draw(|rng| rng.chance(LINGER_PPM).then(|| rng.range(1, LINGER_MS_MAX)));
                let Some(linger_ms) = linger_ms else {
                    side.close();
                    return;
                };
                Duration::from_millis(linger_ms)
            }
            Previous::Dirty(why) => {
                if server.mutation == WireMutation::ReuseDirty {
                    assert!(
                        server.dirty.put(why).is_none()
                            || server.mutation == WireMutation::ReuseDirty,
                        "a dirty connection is left only under its mutation"
                    );
                }
                Duration::from_millis(IDLE_KEEP_MS)
            }
        };
    }
}

impl HttpServer {
    async fn exchange(
        &self,
        side: &ServerSide,
        incoming: &Incoming,
        number: u64,
    ) -> Option<Previous> {
        let hop = hop_of(&incoming.head.target);
        let fate = self.fate(hop);
        self.world.trace(&format!(
            "http {number} {} -> {fate:?}",
            incoming.head.target
        ));
        let next = match fate {
            Fate::Drop => {
                self.world.count(|counts| counts.faults += 1);
                side.close();
                return None;
            }
            Fate::Lost => {
                self.world.count(|counts| counts.faults += 1);
                drop(self.respond(&incoming.body, false));
                side.reset();
                return None;
            }
            Fate::Status(status) => status_reply(self, status),
            Fate::Redirect(status, to) => redirect_reply(status, to, hop),
            Fate::Answer { duplicate, .. } => {
                let reply = self.respond(&incoming.body, duplicate);
                answer(self, side, fate, reply).await
            }
        };
        match next {
            Next::Gone => None,
            Next::Write(bytes, after, cut_max) => {
                self.deliver(side, &bytes, cut_max).await.ok()?;
                if let Previous::Closing("a close-delimited body") = after {
                    side.close();
                    return None;
                }
                Some(after)
            }
            Next::Stalled => Some(Previous::Dirty("a response stalled mid-body")),
        }
    }
}

enum Next {
    Gone,
    Write(Vec<u8>, Previous, usize),
    Stalled,
}

fn head_bytes(status: u16, headers: &HeaderMap) -> Vec<u8> {
    wire::response_head(
        nanocode_http::StatusCode::from_u16(status).expect("a simulated status is valid"),
        None,
        headers,
    )
}

fn status_reply(server: &HttpServer, status: u16) -> Next {
    server.world.count(|counts| counts.faults += 1);
    let (milliseconds, retry) = server.draw(|rng| {
        (
            rng.chance(PPM / 2)
                .then(|| rng.range(0, RETRY_AFTER_MS_MAX)),
            rng.range(0, RETRY_AFTER_SECONDS_MAX),
        )
    });
    let mut headers = HeaderMap::new();
    match milliseconds {
        Some(milliseconds) => headers.insert("retry-after-ms", HeaderValue::from(milliseconds)),
        None => headers.insert("retry-after", HeaderValue::from(retry)),
    };
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    headers.insert("request-id", HeaderValue::from_static("req_sim"));
    let body = format!(
        "{{\"type\":\"error\",\"error\":{{\"type\":\"overloaded_error\",\"message\":\"sim status {status}\"}}}}"
    );
    headers.insert("content-length", HeaderValue::from(body.len()));
    let mut bytes = head_bytes(status, &headers);
    bytes.extend_from_slice(body.as_bytes());
    let length = bytes.len();
    Next::Write(bytes, Previous::Complete, length)
}

fn redirect_reply(status: u16, to: &str, hop: u64) -> Next {
    let mut headers = HeaderMap::new();
    headers.insert(
        "location",
        HeaderValue::from_str(&format!("http://{to}/v1/messages?hop={}", hop + 1))
            .expect("a simulated location is a valid header"),
    );
    headers.insert("content-length", HeaderValue::from(0));
    let bytes = head_bytes(status, &headers);
    let length = bytes.len();
    Next::Write(bytes, Previous::Complete, length)
}

fn encode(coding: Coding, body: &[u8]) -> Vec<u8> {
    match coding {
        Coding::Identity => body.to_vec(),
        Coding::Gzip => nanocode_http::gzip(body, GZIP_LEVEL).expect("gzip of a simulated body"),
        Coding::Deflate => compress(body, Level::DEFAULT, Format::Zlib),
        Coding::Brotli => brotli_stored(body),
        Coding::Zstd => zstd_raw(body),
    }
}

struct Bits {
    bytes: Vec<u8>,
    used: u32,
}

impl Bits {
    fn push(&mut self, value: u32, count: u32) {
        for bit in 0..count {
            if self.used.is_multiple_of(u8::BITS) {
                self.bytes.push(0);
            }
            let last = self.bytes.last_mut().expect("a byte was pushed");
            *last |= u8::from((value >> bit) & 1 == 1) << (self.used % u8::BITS);
            self.used += 1;
        }
    }

    const fn align(&mut self) {
        self.used = self.used.next_multiple_of(u8::BITS);
    }
}

fn brotli_stored(body: &[u8]) -> Vec<u8> {
    let mut bits = Bits {
        bytes: Vec::with_capacity(body.len() + body.len() / BROTLI_BLOCK_BYTES * 3 + 4),
        used: 0,
    };
    bits.push(0, 1);
    for block in body.chunks(BROTLI_BLOCK_BYTES) {
        bits.push(0, 1);
        bits.push(0, 2);
        bits.push(
            u32::try_from(block.len() - 1).expect("a brotli block length fits in u32"),
            16,
        );
        bits.push(1, 1);
        bits.align();
        bits.bytes.extend_from_slice(block);
        bits.used += u32::try_from(block.len()).expect("a brotli block fits in u32") * u8::BITS;
    }
    bits.push(1, 1);
    bits.push(1, 1);
    bits.bytes
}

fn zstd_raw(body: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(body.len() + body.len() / ZSTD_BLOCK_BYTES * 3 + 9);
    bytes.extend_from_slice(&ZSTD_MAGIC);
    bytes.push(0);
    bytes.push(ZSTD_WINDOW_DESCRIPTOR);
    let blocks: Vec<&[u8]> = if body.is_empty() {
        vec![body]
    } else {
        body.chunks(ZSTD_BLOCK_BYTES).collect()
    };
    let count = blocks.len();
    for (index, block) in blocks.into_iter().enumerate() {
        let last = if index + 1 == count {
            ZSTD_LAST_BLOCK
        } else {
            0
        };
        let size = u32::try_from(block.len()).expect("a zstd block length fits in u32");
        let header = last | (size << ZSTD_BLOCK_SIZE_SHIFT);
        bytes.extend_from_slice(&header.to_le_bytes()[..3]);
        bytes.extend_from_slice(block);
    }
    bytes
}

const fn coding_name(coding: Coding) -> Option<&'static str> {
    match coding {
        Coding::Identity => None,
        Coding::Gzip => Some("gzip"),
        Coding::Deflate => Some("deflate"),
        Coding::Brotli => Some("br"),
        Coding::Zstd => Some("zstd"),
    }
}

fn chunked(server: &HttpServer, body: &[u8], malformed: Option<Malformed>) -> Vec<u8> {
    let sizes: Vec<usize> = server.draw(|rng| {
        let mut sizes = Vec::new();
        let mut left = body.len();
        while left > 0 {
            let size = usize::try_from(rng.range(1, 4_096))
                .expect("a chunk size fits in usize")
                .min(left);
            sizes.push(size);
            left -= size;
        }
        sizes
    });
    let mut bytes = Vec::with_capacity(body.len() + sizes.len() * 8 + 5);
    let mut from = 0;
    for (index, size) in sizes.into_iter().enumerate() {
        let chunk = &body[from..from + size];
        from += size;
        match malformed {
            Some(Malformed::ChunkSize) if index == 0 => bytes.extend_from_slice(b"zz\r\n"),
            _ if index % 3 == 1 => {
                bytes.extend_from_slice(format!("{size:X};sim=1\r\n").as_bytes())
            }
            _ => bytes.extend_from_slice(format!("{size:x}\r\n").as_bytes()),
        }
        bytes.extend_from_slice(chunk);
        match malformed {
            Some(Malformed::ChunkEnd) if index == 0 => bytes.extend_from_slice(b"XX"),
            _ => bytes.extend_from_slice(b"\r\n"),
        }
    }
    if from == 0 && matches!(malformed, Some(Malformed::ChunkSize)) {
        bytes.extend_from_slice(b"zz\r\n");
    }
    bytes.extend_from_slice(b"0\r\nx-sim-trailer: 1\r\n\r\n");
    bytes
}

fn refusal(error: &str) -> Next {
    let body = format!(
        "{{\"type\":\"error\",\"error\":{{\"type\":\"invalid_request_error\",\"message\":\"{}\"}}}}",
        error.replace('"', "'")
    );
    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    headers.insert("x-should-retry", HeaderValue::from_static("false"));
    headers.insert("content-length", HeaderValue::from(body.len()));
    let mut bytes = head_bytes(400, &headers);
    bytes.extend_from_slice(body.as_bytes());
    let length = bytes.len();
    Next::Write(bytes, Previous::Complete, length)
}

const fn takes_effect(malformed: Malformed, framing: Framing) -> bool {
    match malformed {
        Malformed::StatusLine | Malformed::TooManyHeaders | Malformed::HeadTooLarge => true,
        Malformed::ContentLength | Malformed::ConflictingLength => {
            !matches!(framing, Framing::Chunked)
        }
        Malformed::ChunkSize | Malformed::ChunkEnd => matches!(framing, Framing::Chunked),
        Malformed::CodingHeader => false,
    }
}

fn malform(head: Vec<u8>, malformed: Option<Malformed>, body_bytes: usize) -> Vec<u8> {
    match malformed {
        Some(Malformed::StatusLine) => {
            let mut head = head;
            head.splice(..12, b"HTTP/1.1 2x0".iter().copied())
                .for_each(drop);
            head
        }
        Some(Malformed::ContentLength) => splice_header(&head, "content-length: abc\r\n"),
        Some(Malformed::ConflictingLength) => splice_header(
            &head,
            &format!(
                "content-length: {body_bytes}\r\ncontent-length: {}\r\n",
                body_bytes + 1
            ),
        ),
        Some(Malformed::CodingHeader) => splice_header(&head, "content-encoding: gzip\r\n"),
        Some(Malformed::TooManyHeaders) => {
            let extra: String = (0..=HEADERS_MAX)
                .map(|index| format!("x-sim-{index}: 1\r\n"))
                .collect();
            splice_header(&head, &extra)
        }
        Some(Malformed::HeadTooLarge) => {
            let filler = "a".repeat(OVERSIZED_HEADER_BYTES);
            let count = HEAD_BYTES_MAX / OVERSIZED_HEADER_BYTES + 1;
            let extra: String = (0..count)
                .map(|index| format!("x-sim-{index}: {filler}\r\n"))
                .collect();
            splice_header(&head, &extra)
        }
        Some(Malformed::ChunkSize | Malformed::ChunkEnd) | None => head,
    }
}

fn framed(
    server: &HttpServer,
    coding: Coding,
    framing: Framing,
    malformed: Option<Malformed>,
    encoded: Vec<u8>,
) -> (HeaderMap, Vec<u8>, Previous) {
    let mut headers = HeaderMap::new();
    headers.insert(
        "content-type",
        HeaderValue::from_static("text/event-stream"),
    );
    headers.insert("request-id", HeaderValue::from_static("req_sim"));
    if let Some(name) = coding_name(coding) {
        headers.insert("content-encoding", HeaderValue::from_static(name));
    }
    let mut after = Previous::Complete;
    let body = match framing {
        Framing::Length => {
            headers.insert("content-length", HeaderValue::from(encoded.len()));
            encoded
        }
        Framing::Chunked => {
            headers.insert("transfer-encoding", HeaderValue::from_static("chunked"));
            chunked(server, &encoded, malformed)
        }
        Framing::Close => {
            headers.insert("connection", HeaderValue::from_static("close"));
            after = Previous::Closing("a close-delimited body");
            encoded
        }
    };
    if !matches!(framing, Framing::Close) && server.draw(|rng| rng.chance(PPM / 10)) {
        headers.insert("connection", HeaderValue::from_static("close"));
        after = Previous::Closing("connection: close");
    }
    (headers, body, after)
}

async fn answer(
    server: &HttpServer,
    side: &ServerSide,
    fate: Fate,
    reply: Result<Reply, String>,
) -> Next {
    let Fate::Answer {
        coding,
        framing,
        slow_head_ms,
        malformed,
        broken,
        informational,
        junk,
        ..
    } = fate
    else {
        unreachable!("answer is called only for an answer");
    };
    let reply = match reply {
        Ok(reply) => reply,
        Err(error) => {
            server
                .world
                .trace(&format!("http refused the request: {error}"));
            return refusal(&error);
        }
    };
    let encoded = encode(coding, reply.body.as_bytes());
    let (headers, body, mut after) = framed(server, coding, framing, malformed, encoded);
    let head = malform(head_bytes(200, &headers), malformed, body.len());
    if let Some(malformed) = malformed {
        server.world.count(|counts| counts.faults += 1);
        if takes_effect(malformed, framing) && matches!(after, Previous::Complete) {
            after = Previous::Dirty("a malformed response");
        }
    }
    let mut bytes = Vec::with_capacity(head.len() + body.len() + 64);
    if informational {
        bytes.extend_from_slice(
            b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 103 Early Hints\r\nlink: </sim>\r\n\r\n",
        );
    }
    bytes.extend_from_slice(&head);
    let head_end = bytes.len();
    bytes.extend_from_slice(&body);
    let mut cut_max = bytes.len();
    if junk && matches!(after, Previous::Complete) {
        cut_max = (bytes.len() - 1).max(head_end);
        bytes.extend_from_slice(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n");
        after = Previous::Dirty("bytes after a complete response");
    }
    if let Some(slow_head_ms) = slow_head_ms {
        server.world.count(|counts| counts.faults += 1);
        if !server
            .wait_out(side, slow_head_ms, "a slow response head")
            .await
        {
            return Next::Gone;
        }
    }
    let Some((broken, at_ppm)) = broken else {
        return Next::Write(bytes, after, cut_max);
    };
    server.world.count(|counts| counts.faults += 1);
    let span = u64::try_from(bytes.len() - head_end).expect("a body length fits in u64");
    let cut = head_end + usize::try_from(span * at_ppm / PPM).expect("a cut fits in usize");
    server.world.trace(&format!(
        "http {broken:?} {} at {cut} of {}",
        reply.label,
        bytes.len()
    ));
    if server.deliver(side, &bytes[..cut], cut).await.is_err() {
        return Next::Gone;
    }
    match broken {
        Break::Reset => {
            side.reset();
            Next::Gone
        }
        Break::Cut => {
            side.close();
            Next::Gone
        }
        Break::Stall(stall_ms) => {
            if server
                .wait_out(side, stall_ms, "a stalled response body")
                .await
            {
                Next::Stalled
            } else {
                Next::Gone
            }
        }
    }
}

fn splice_header(head: &[u8], extra: &str) -> Vec<u8> {
    let end = head.len() - 2;
    let mut spliced = Vec::with_capacity(head.len() + extra.len());
    spliced.extend_from_slice(&head[..end]);
    spliced.extend_from_slice(extra.as_bytes());
    spliced.extend_from_slice(&head[end..]);
    spliced
}
