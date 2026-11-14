use std::{
    cell::{Cell, RefCell},
    ops::Range,
    process::ExitCode,
    sync::Arc,
    time::Duration,
};

use nanocode_bytes::Bytes;
use nanocode_http::types::{HeaderValue, Method, StatusCode, header};
use nanocode_http::{
    client::BoxError,
    server::{self, Options, Outgoing, Reply, Request},
    wire::{self, WireError},
};

use crate::{
    executor::{Clock, Ended, Executor, Task},
    rng::{FNV_OFFSET, Rng, fnv},
    socket::{self, ClientSide, ServerSide},
};

pub const RUN_MS_MAX: u64 = 30 * 60 * 1000;
const EXCHANGES_MAX: u64 = 6;
const BODY_BYTES_MAX: u64 = 4096;
const LARGE_BODY_PPM: u64 = 100_000;
const LARGE_BODY_BYTES_MAX: u64 = 65_536;
const CHUNKS_MAX: u64 = 6;
const REPLY_CHUNKS_MAX: u64 = 5;
const REPLY_CHUNK_BYTES_MAX: u64 = 1024;
const MISMATCH_EXTRA_MAX: u64 = 16;
const HTTP_10_PPM: u64 = 150_000;
const CLOSE_PPM: u64 = 100_000;
const KEEP_ALIVE_PPM: u64 = 500_000;
const EARLY_PPM: u64 = 150_000;
const PARTIAL_PPM: u64 = 100_000;
const EXTENSION_PPM: u64 = 300_000;
const TRAILER_PPM: u64 = 200_000;
const PIPELINED_PPM: u64 = 500_000;
const MALFORMED_PPM: u64 = 80_000;
const CUT_PPM: u64 = 150_000;
const RESET_PPM: u64 = 400_000;
const MISMATCH_PPM: u64 = 150_000;
const HALF_PPM: u64 = 500_000;
const PIECE_MAX: u64 = 4096;
const TINY_PIECES_PPM: u64 = 200_000;
const TINY_PIECE_MAX: u64 = 7;
const GAP_MS_MAX: u64 = 50;
const GAP_PPM: u64 = 10_000;
const CORRUPT_OFFSET_MAX: u64 = 400;
const CORRUPT_BITS: u8 = 0x20;
const READ_BYTES: usize = 8192;
const BYTE_VALUES: u64 = 256;
const METHODS: [&str; 5] = ["GET", "POST", "PUT", "DELETE", "HEAD"];
const STATUSES: [u16; 7] = [200, 201, 204, 304, 404, 500, 503];
const NO_COLON: &[u8] = b"GET /broken HTTP/1.1\r\nhost: sim.test\r\nno colon here\r\n\r\n";
const SIGNED_LENGTH: &[u8] =
    b"POST /broken HTTP/1.1\r\nhost: sim.test\r\ncontent-length: +5\r\n\r\nhello";
const CHUNK_JUNK_HEAD: &[u8] =
    b"POST /broken HTTP/1.1\r\nhost: sim.test\r\ntransfer-encoding: chunked\r\n\r\n";
const CHUNK_JUNK_BODY: &[u8] = b"5x\r\nhello\r\n0\r\n\r\n";
const BROKEN_TARGET: &str = "/broken";
const CRLF: &[u8] = b"\r\n";
const HEX_RADIX: u32 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpServerMutation {
    None,
    CorruptResponseByte,
    DropRequestByte,
}

impl HttpServerMutation {
    const ALL: [Self; 3] = [Self::None, Self::CorruptResponseByte, Self::DropRequestByte];

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|mutation| mutation.name() == name)
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::CorruptResponseByte => "corrupt_response_byte",
            Self::DropRequestByte => "drop_request_byte",
        }
    }

    pub fn names() -> String {
        Self::ALL.map(Self::name).join("|")
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct HttpServerStats {
    pub seeds: u64,
    pub requests: u64,
    pub handled: u64,
    pub responses: u64,
    pub chunked: u64,
    pub early: u64,
    pub mismatched: u64,
    pub malformed: u64,
    pub cuts: u64,
    pub resets: u64,
}

impl HttpServerStats {
    pub const fn add(&mut self, other: &Self) {
        self.seeds += other.seeds;
        self.requests += other.requests;
        self.handled += other.handled;
        self.responses += other.responses;
        self.chunked += other.chunked;
        self.early += other.early;
        self.mismatched += other.mismatched;
        self.malformed += other.malformed;
        self.cuts += other.cuts;
        self.resets += other.resets;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Framing {
    None,
    Length,
    Chunked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Handling {
    ReadAll,
    Early,
    Partial,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Declared {
    Exact,
    Short(u64),
    Long(u64),
    Absent,
}

#[derive(Clone, Debug)]
enum Body {
    Empty,
    Full(Vec<u8>),
    Stream(Vec<Vec<u8>>, Declared),
}

struct Exchange {
    method: Method,
    http_10: bool,
    connection: Option<&'static str>,
    framing: Framing,
    body: Vec<u8>,
    bytes: Vec<u8>,
    head_len: usize,
    handling: Handling,
    status: StatusCode,
    reply: Body,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Malformed {
    NoColon,
    SignedLength,
    ChunkJunk,
}

impl Malformed {
    const ALL: [Self; 3] = [Self::NoColon, Self::SignedLength, Self::ChunkJunk];

    fn bytes(self) -> Vec<u8> {
        match self {
            Self::NoColon => NO_COLON.to_vec(),
            Self::SignedLength => SIGNED_LENGTH.to_vec(),
            Self::ChunkJunk => [CHUNK_JUNK_HEAD, CHUNK_JUNK_BODY].concat(),
        }
    }

    const fn head_len(self) -> usize {
        match self {
            Self::NoColon => NO_COLON.len(),
            Self::SignedLength => SIGNED_LENGTH.len(),
            Self::ChunkJunk => CHUNK_JUNK_HEAD.len(),
        }
    }

    const fn handled(self) -> bool {
        matches!(self, Self::ChunkJunk)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cut {
    Eof,
    Reset,
}

struct Script {
    exchanges: Vec<Exchange>,
    malformed: Option<(usize, Malformed)>,
    cut: Option<(usize, Cut)>,
    pipelined: bool,
    read_piece_max: usize,
    write_piece_max: usize,
    client_piece_max: u64,
    gap_ms_max: u64,
    stream: Vec<u8>,
    starts: Vec<usize>,
    heads: Vec<usize>,
    ends: Vec<usize>,
}

fn random_bytes(rng: &mut Rng, count: u64) -> Vec<u8> {
    (0..count)
        .map(|_| u8::try_from(rng.below(BYTE_VALUES)).expect("a byte value fits in u8"))
        .collect()
}

fn count(value: usize) -> u64 {
    u64::try_from(value).expect("a length fits in u64")
}

fn size(value: u64) -> usize {
    usize::try_from(value).expect("a simulated length fits in usize")
}

fn chunked_body(rng: &mut Rng, body: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    let mut at = 0;
    let pieces = rng.range(1, CHUNKS_MAX);
    for piece in 0..pieces {
        if at == body.len() {
            break;
        }
        let left = body.len() - at;
        let take = if piece + 1 == pieces {
            left
        } else {
            size(rng.range(1, count(left)))
        };
        output.extend_from_slice(format!("{take:x}").as_bytes());
        if rng.chance(EXTENSION_PPM) {
            output.extend_from_slice(b";sim=1");
        }
        output.extend_from_slice(CRLF);
        output.extend_from_slice(&body[at..at + take]);
        output.extend_from_slice(CRLF);
        at += take;
    }
    output.extend_from_slice(b"0");
    if rng.chance(EXTENSION_PPM) {
        output.extend_from_slice(b";last=1");
    }
    output.extend_from_slice(CRLF);
    if rng.chance(TRAILER_PPM) {
        output.extend_from_slice(b"x-sim-trailer: done\r\n");
    }
    output.extend_from_slice(CRLF);
    output
}

fn exchange(rng: &mut Rng, index: usize) -> Exchange {
    let method = Method::from_bytes(METHODS[rng.pick(METHODS.len())].as_bytes())
        .expect("the scripted methods are valid");
    let http_10 = rng.chance(HTTP_10_PPM);
    let connection = if rng.chance(CLOSE_PPM) {
        Some("close")
    } else if http_10 && rng.chance(KEEP_ALIVE_PPM) {
        Some("keep-alive")
    } else {
        None
    };
    let framing = match rng.below(3) {
        0 => Framing::None,
        1 => Framing::Length,
        _ if http_10 => Framing::Length,
        _ => Framing::Chunked,
    };
    let body_max = if rng.chance(LARGE_BODY_PPM) {
        LARGE_BODY_BYTES_MAX
    } else {
        BODY_BYTES_MAX
    };
    let body = match framing {
        Framing::None => Vec::new(),
        Framing::Length | Framing::Chunked => {
            let length = rng.range(0, body_max);
            random_bytes(rng, length)
        }
    };
    let minor = if http_10 { 0 } else { 1 };
    let mut head = format!("{method} /r{index} HTTP/1.{minor}\r\nhost: sim.test\r\n");
    if let Some(connection) = connection {
        head.push_str(&format!("connection: {connection}\r\n"));
    }
    match framing {
        Framing::None => {}
        Framing::Length => head.push_str(&format!("content-length: {}\r\n", body.len())),
        Framing::Chunked => head.push_str("transfer-encoding: chunked\r\n"),
    }
    head.push_str("\r\n");
    let mut bytes = head.into_bytes();
    let head_len = bytes.len();
    match framing {
        Framing::None => {}
        Framing::Length => bytes.extend_from_slice(&body),
        Framing::Chunked => bytes.extend(chunked_body(rng, &body)),
    }
    let handling = if rng.chance(EARLY_PPM) {
        Handling::Early
    } else if rng.chance(PARTIAL_PPM) {
        Handling::Partial
    } else {
        Handling::ReadAll
    };
    let status = StatusCode::from_u16(STATUSES[rng.pick(STATUSES.len())])
        .expect("the scripted statuses are valid");
    let reply = match rng.below(3) {
        0 => Body::Empty,
        1 => {
            let length = rng.range(0, BODY_BYTES_MAX);
            Body::Full(random_bytes(rng, length))
        }
        _ => {
            let chunks = (0..rng.range(0, REPLY_CHUNKS_MAX))
                .map(|_| {
                    let length = rng.range(0, REPLY_CHUNK_BYTES_MAX);
                    random_bytes(rng, length)
                })
                .collect::<Vec<_>>();
            let total = chunks.iter().map(|chunk| count(chunk.len())).sum::<u64>();
            let declared = if rng.chance(MISMATCH_PPM) {
                if total > 0 && rng.chance(HALF_PPM) {
                    Declared::Long(rng.range(0, total - 1))
                } else {
                    Declared::Short(total + rng.range(1, MISMATCH_EXTRA_MAX))
                }
            } else if rng.chance(HALF_PPM) {
                Declared::Exact
            } else {
                Declared::Absent
            };
            Body::Stream(chunks, declared)
        }
    };
    Exchange {
        method,
        http_10,
        connection,
        framing,
        body,
        bytes,
        head_len,
        handling,
        status,
        reply,
    }
}

impl Script {
    fn malformed_at(&self, index: usize) -> Option<Malformed> {
        self.malformed
            .and_then(|(at, kind)| (at == index).then_some(kind))
    }

    fn method_at(&self, index: usize) -> Method {
        match self.malformed_at(index) {
            Some(Malformed::NoColon) => Method::GET,
            Some(Malformed::SignedLength | Malformed::ChunkJunk) => Method::POST,
            None => self
                .exchanges
                .get(index)
                .map_or(Method::GET, |exchange| exchange.method.clone()),
        }
    }

    fn of(seed: u64, mutation: HttpServerMutation) -> Self {
        let mut rng = Rng::derive(seed, "http-server-script");
        let exchanges = (0..size(rng.range(1, EXCHANGES_MAX)))
            .map(|index| exchange(&mut rng, index))
            .collect::<Vec<_>>();
        let malformed = rng.chance(MALFORMED_PPM).then(|| {
            (
                rng.pick(exchanges.len()),
                Malformed::ALL[rng.pick(Malformed::ALL.len())],
            )
        });
        let mut stream = Vec::new();
        let mut starts = Vec::new();
        let mut heads = Vec::new();
        let mut ends = Vec::new();
        for (index, exchange) in exchanges.iter().enumerate() {
            starts.push(stream.len());
            match malformed {
                Some((at, kind)) if at == index => {
                    heads.push(kind.head_len());
                    stream.extend(kind.bytes());
                }
                _ => {
                    heads.push(exchange.head_len);
                    stream.extend_from_slice(&exchange.bytes);
                }
            }
            ends.push(stream.len());
        }
        if mutation == HttpServerMutation::DropRequestByte {
            let at = size(rng.range(0, count(ends[0] - 1)));
            stream.remove(at);
        }
        let cut = rng.chance(CUT_PPM).then(|| {
            let at = size(rng.range(0, count(stream.len() - 1)));
            let kind = if rng.chance(RESET_PPM) {
                Cut::Reset
            } else {
                Cut::Eof
            };
            (at, kind)
        });
        let tiny = rng.chance(TINY_PIECES_PPM);
        let piece_max = if tiny { TINY_PIECE_MAX } else { PIECE_MAX };
        let read_piece_max = size(rng.range(1, piece_max));
        let write_piece_max = size(rng.range(1, piece_max));
        let client_piece_max = rng.range(1, piece_max);
        Self {
            exchanges,
            malformed,
            cut,
            pipelined: rng.chance(PIPELINED_PPM),
            read_piece_max,
            write_piece_max,
            client_piece_max,
            gap_ms_max: rng.range(0, GAP_MS_MAX),
            stream,
            starts,
            heads,
            ends,
        }
    }

    fn describe(&self) -> String {
        let exchanges = self
            .exchanges
            .iter()
            .map(|exchange| {
                let reply = match &exchange.reply {
                    Body::Empty => "empty".to_owned(),
                    Body::Full(bytes) => format!("full{}", bytes.len()),
                    Body::Stream(chunks, declared) => {
                        format!("stream{}:{declared:?}", chunks.len())
                    }
                };
                format!(
                    "{} 1.{} {:?} {:?} body={} {:?} {} {reply}",
                    exchange.method,
                    u8::from(!exchange.http_10),
                    exchange.connection,
                    exchange.framing,
                    exchange.body.len(),
                    exchange.handling,
                    exchange.status.as_u16()
                )
            })
            .collect::<Vec<_>>()
            .join(" | ");
        format!(
            "scenario=http_server pipelined={} malformed={:?} cut={:?} read_piece_max={} write_piece_max={} client_piece_max={} gap_ms<={} exchanges=[{exchanges}]",
            self.pipelined,
            self.malformed,
            self.cut,
            self.read_piece_max,
            self.write_piece_max,
            self.client_piece_max,
            self.gap_ms_max
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum After {
    Continue,
    May,
    Must,
}

struct Expected {
    status: StatusCode,
    body: Vec<u8>,
    truncated: bool,
    after: After,
}

fn bodiless(status: StatusCode) -> bool {
    status.is_informational()
        || status == StatusCode::NO_CONTENT
        || status == StatusCode::NOT_MODIFIED
}

fn keeps_alive(exchange: &Exchange) -> bool {
    match exchange.connection {
        Some("close") => false,
        Some(_) => true,
        None => !exchange.http_10,
    }
}

fn expect(exchange: &Exchange) -> Expected {
    let no_body = bodiless(exchange.status) || exchange.method == Method::HEAD;
    let mut truncated = false;
    let mut after = After::Continue;
    let body = match &exchange.reply {
        _ if no_body => Vec::new(),
        Body::Empty => Vec::new(),
        Body::Full(bytes) => bytes.clone(),
        Body::Stream(chunks, declared) => {
            let mut bytes = chunks.concat();
            match declared {
                Declared::Exact | Declared::Absent => {}
                Declared::Short(_) => {
                    truncated = true;
                    after = After::Must;
                }
                Declared::Long(length) => {
                    bytes.truncate(size(*length));
                    after = After::Must;
                }
            }
            bytes
        }
    };
    let unread = exchange.handling != Handling::ReadAll
        && (exchange.framing == Framing::Chunked
            || (exchange.framing == Framing::Length && !exchange.body.is_empty()));
    if unread && after == After::Continue {
        after = After::May;
    }
    let close_delimited = matches!(exchange.reply, Body::Stream(_, Declared::Absent))
        && exchange.http_10
        && !bodiless(exchange.status);
    if !keeps_alive(exchange) || close_delimited {
        after = After::Must;
    }
    Expected {
        status: exchange.status,
        body,
        truncated,
        after,
    }
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

struct Handled {
    method: Method,
    target: String,
    http_10: bool,
    body: Option<Result<Vec<u8>, String>>,
}

#[derive(Default)]
struct ServerLog {
    handled: Vec<Handled>,
    result: Option<Result<(), String>>,
}

struct Response {
    status: StatusCode,
    body: Vec<u8>,
    truncated: bool,
}

#[derive(Default)]
struct ClientLog {
    responses: Vec<Response>,
    parse_error: Option<String>,
}

fn declared_length(chunks: &[Vec<u8>], declared: Declared) -> Option<u64> {
    let total = chunks.iter().map(|chunk| count(chunk.len())).sum::<u64>();
    match declared {
        Declared::Exact => Some(total),
        Declared::Short(length) | Declared::Long(length) => Some(length),
        Declared::Absent => None,
    }
}

fn reply_of(exchange: &Exchange) -> Reply {
    match &exchange.reply {
        Body::Empty => Reply::new(exchange.status, Outgoing::Empty),
        Body::Full(bytes) => Reply::new(
            exchange.status,
            Outgoing::Full(Bytes::copy_from_slice(bytes)),
        ),
        Body::Stream(chunks, declared) => {
            let items = chunks
                .iter()
                .map(|chunk| Ok::<_, BoxError>(Bytes::copy_from_slice(chunk)))
                .collect::<Vec<_>>();
            let mut reply = Reply::new(
                exchange.status,
                Outgoing::Stream(Box::pin(nanocode_sync::stream::iter(items))),
            );
            if let Some(length) = declared_length(chunks, *declared) {
                reply
                    .headers
                    .insert(header::CONTENT_LENGTH, HeaderValue::from(length));
            }
            reply
        }
    }
}

async fn handle(
    script: &Script,
    request: Request,
    log: &RefCell<ServerLog>,
    trace: &Trace,
) -> Reply {
    let index = log.borrow().handled.len();
    trace.line(&format!(
        "server handles {} {}",
        request.method, request.target
    ));
    log.borrow_mut().handled.push(Handled {
        method: request.method.clone(),
        target: request.target.clone(),
        http_10: request.version == nanocode_http::types::Version::HTTP_10,
        body: None,
    });
    let Some(exchange) = script.exchanges.get(index) else {
        return Reply::new(StatusCode::INTERNAL_SERVER_ERROR, Outgoing::Empty);
    };
    let mut body = request.body;
    let handling = if script.malformed_at(index).is_some() {
        Handling::ReadAll
    } else {
        exchange.handling
    };
    match handling {
        Handling::ReadAll => {
            let read = body
                .collect()
                .await
                .map(|bytes| bytes.to_vec())
                .map_err(|error| error.to_string());
            trace.line(&format!("server read {:?}", read.as_ref().map(Vec::len)));
            if let Some(handled) = log.borrow_mut().handled.last_mut() {
                handled.body = Some(read);
            }
        }
        Handling::Partial => {
            let read = body.chunk().await;
            trace.line(&format!(
                "server read part {:?}",
                read.as_ref().map(|chunk| chunk.as_ref().map(Bytes::len))
            ));
        }
        Handling::Early => {}
    }
    reply_of(exchange)
}

async fn serve(script: &Script, duplex: ClientSide, log: &RefCell<ServerLog>, trace: &Trace) {
    let served = server::serve(Box::new(duplex), Options::default(), |request| {
        handle(script, request, log, trace)
    })
    .await;
    let result = served.map_err(|error| match error {
        WireError::Parse(detail) => format!("parse: {detail}"),
        other => other.to_string(),
    });
    trace.line(&format!("server ended {result:?}"));
    log.borrow_mut().result = Some(result);
}

fn last_coding_chunked(value: &str) -> bool {
    value
        .rsplit(',')
        .next()
        .is_some_and(|coding| coding.trim().eq_ignore_ascii_case("chunked"))
}

fn find_crlf(bytes: &[u8]) -> Option<usize> {
    bytes.windows(CRLF.len()).position(|window| window == CRLF)
}

fn dechunk(bytes: &[u8]) -> Result<Option<(Vec<u8>, usize)>, String> {
    let mut at = 0;
    let mut body = Vec::new();
    loop {
        let Some(end) = find_crlf(&bytes[at..]) else {
            return Ok(None);
        };
        let line = std::str::from_utf8(&bytes[at..at + end])
            .map_err(|_| "a chunk size line is not text".to_owned())?;
        let digits = line.split(';').next().unwrap_or_default().trim();
        let length = u64::from_str_radix(digits, HEX_RADIX)
            .map_err(|_| format!("chunk size line {line:?}"))?;
        at += end + CRLF.len();
        if length == 0 {
            loop {
                let Some(end) = find_crlf(&bytes[at..]) else {
                    return Ok(None);
                };
                at += end + CRLF.len();
                if end == 0 {
                    return Ok(Some((body, at)));
                }
            }
        }
        let length = size(length);
        if bytes.len() < at + length + CRLF.len() {
            return Ok(None);
        }
        body.extend_from_slice(&bytes[at..at + length]);
        at += length;
        if &bytes[at..at + CRLF.len()] != CRLF {
            return Err("chunk data without CRLF".to_owned());
        }
        at += CRLF.len();
    }
}

fn parse(bytes: &[u8], method: &Method, eof: bool) -> Result<Option<(Response, usize)>, String> {
    let Some((head, head_len)) =
        wire::parse_response(bytes).map_err(|error| format!("response head: {error}"))?
    else {
        return if eof {
            Err(format!(
                "the connection closed inside a response head ({} bytes)",
                bytes.len()
            ))
        } else {
            Ok(None)
        };
    };
    let rest = &bytes[head_len..];
    let done = |body: Vec<u8>, used: usize, truncated: bool| {
        Ok(Some((
            Response {
                status: head.status,
                body,
                truncated,
            },
            head_len + used,
        )))
    };
    if method == Method::HEAD || bodiless(head.status) {
        return done(Vec::new(), 0, false);
    }
    if let Some(value) = head.headers.get(header::TRANSFER_ENCODING) {
        let value = value.to_str().unwrap_or_default();
        if last_coding_chunked(value) {
            return match dechunk(rest)? {
                Some((body, used)) => done(body, used, false),
                None if eof => Err("the connection closed inside a chunked body".to_owned()),
                None => Ok(None),
            };
        }
    } else if let Some(value) = head.headers.get(header::CONTENT_LENGTH) {
        let length = value
            .to_str()
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .ok_or_else(|| format!("content-length {value:?}"))?;
        let length = size(length);
        if rest.len() >= length {
            return done(rest[..length].to_vec(), length, false);
        }
        return if eof {
            done(rest.to_vec(), rest.len(), true)
        } else {
            Ok(None)
        };
    }
    if eof {
        done(rest.to_vec(), rest.len(), false)
    } else {
        Ok(None)
    }
}

struct Reader {
    buffer: Vec<u8>,
    eof: bool,
    read: u64,
    corrupt_at: Option<u64>,
}

impl Reader {
    async fn next(
        &mut self,
        side: &ServerSide,
        method: &Method,
    ) -> Option<Result<Response, String>> {
        loop {
            if !self.buffer.is_empty() || !self.eof {
                match parse(&self.buffer, method, self.eof) {
                    Err(error) => return Some(Err(error)),
                    Ok(Some((response, used))) => {
                        self.buffer.drain(..used);
                        return Some(Ok(response));
                    }
                    Ok(None) if self.buffer.is_empty() && self.eof => return None,
                    Ok(None) => {}
                }
            }
            if self.eof {
                return None;
            }
            let mut chunk = [0_u8; READ_BYTES];
            match side.read(&mut chunk).await {
                Ok(0) | Err(_) => self.eof = true,
                Ok(read) => {
                    let from = self.read;
                    self.read += count(read);
                    if let Some(at) = self.corrupt_at
                        && (from..self.read).contains(&at)
                    {
                        chunk[size(at - from)] ^= CORRUPT_BITS;
                    }
                    self.buffer.extend_from_slice(&chunk[..read]);
                }
            }
        }
    }
}

async fn read_responses(
    script: &Script,
    side: &ServerSide,
    reader: &mut Reader,
    log: &RefCell<ClientLog>,
    trace: &Trace,
    until: usize,
) {
    while log.borrow().responses.len() < until && log.borrow().parse_error.is_none() {
        let index = log.borrow().responses.len();
        let method = script.method_at(index);
        match reader.next(side, &method).await {
            None => return,
            Some(Ok(response)) => {
                trace.line(&format!(
                    "client read {} body={} truncated={}",
                    response.status.as_u16(),
                    response.body.len(),
                    response.truncated
                ));
                log.borrow_mut().responses.push(response);
            }
            Some(Err(error)) => {
                trace.line(&format!("client could not read: {error}"));
                log.borrow_mut().parse_error = Some(error);
                return;
            }
        }
    }
}

async fn client(
    script: &Script,
    side: ServerSide,
    clock: &Arc<Clock>,
    log: &RefCell<ClientLog>,
    trace: &Trace,
    mutation: HttpServerMutation,
    seed: u64,
) {
    let mut rng = Rng::derive(seed, "http-server-client");
    let corrupt_at = (mutation == HttpServerMutation::CorruptResponseByte)
        .then(|| rng.range(0, CORRUPT_OFFSET_MAX));
    let mut reader = Reader {
        buffer: Vec::new(),
        eof: false,
        read: 0,
        corrupt_at,
    };
    let limit = script.cut.map_or(script.stream.len(), |(at, _)| at);
    let mut written = 0;
    let mut gone = false;
    for index in 0..script.exchanges.len() {
        let end = script.ends[index].min(script.stream.len()).min(limit);
        while written < end {
            let piece = size(rng.range(1, script.client_piece_max)).min(end - written);
            if side
                .write(&script.stream[written..written + piece])
                .is_err()
            {
                gone = true;
                break;
            }
            written += piece;
            if script.gap_ms_max > 0 && rng.chance(GAP_PPM) {
                clock
                    .sleep(Duration::from_millis(rng.range(0, script.gap_ms_max)))
                    .await;
            }
        }
        if gone || written >= limit {
            break;
        }
        if !script.pipelined {
            read_responses(script, &side, &mut reader, log, trace, index + 1).await;
        }
    }
    trace.line(&format!(
        "client wrote {written} of {} bytes",
        script.stream.len()
    ));
    match script.cut {
        Some((_, Cut::Reset)) => side.reset(),
        Some((_, Cut::Eof)) | None => side.close(),
    }
    read_responses(script, &side, &mut reader, log, trace, usize::MAX).await;
}

fn bounds(script: &Script, expected: &[Option<Expected>]) -> (usize, usize) {
    let total = script.exchanges.len();
    let mut lowest = total;
    let mut highest = total;
    for (index, expected) in expected.iter().enumerate() {
        match expected {
            None => {
                let reached =
                    index + usize::from(script.malformed_at(index).is_some_and(Malformed::handled));
                lowest = lowest.min(reached);
                highest = highest.min(reached);
            }
            Some(expected) => match expected.after {
                After::Continue => {}
                After::May => lowest = lowest.min(index + 1),
                After::Must => {
                    lowest = lowest.min(index + 1);
                    highest = highest.min(index + 1);
                }
            },
        }
    }
    if let Some((at, kind)) = script.cut {
        let delivered = script.ends.iter().filter(|end| **end <= at).count();
        let headed = (0..total)
            .filter(|index| script.starts[*index] + script.heads[*index] <= at)
            .count();
        lowest = lowest.min(if kind == Cut::Reset { 0 } else { delivered });
        highest = highest.min(headed);
    }
    (lowest, highest)
}

fn check_handled(
    script: &Script,
    server: &ServerLog,
    bounds: (usize, usize),
) -> Result<(), String> {
    let (lowest, highest) = bounds;
    let handled = server.handled.len();
    if handled < lowest || handled > highest {
        return Err(format!(
            "the server handled {handled} requests where the script allows {lowest} to {highest}"
        ));
    }
    for (index, handled) in server.handled.iter().enumerate() {
        if script.malformed_at(index).is_some() {
            if handled.method != Method::POST || handled.target != BROKEN_TARGET {
                return Err(format!(
                    "request {index}: the server saw {} {} where the client sent POST {BROKEN_TARGET}",
                    handled.method, handled.target
                ));
            }
            if let Some(Ok(body)) = &handled.body {
                return Err(format!(
                    "request {index}: the handler read a body of {} bytes behind a malformed chunk size",
                    body.len()
                ));
            }
            continue;
        }
        let exchange = &script.exchanges[index];
        let target = format!("/r{index}");
        if handled.method != exchange.method
            || handled.target != target
            || handled.http_10 != exchange.http_10
        {
            return Err(format!(
                "request {index}: the server saw {} {} (1.0={}) where the client sent {} {target} (1.0={})",
                handled.method, handled.target, handled.http_10, exchange.method, exchange.http_10
            ));
        }
        match &handled.body {
            Some(Ok(body)) if *body != exchange.body => {
                let at = body
                    .iter()
                    .zip(&exchange.body)
                    .position(|(got, sent)| got != sent)
                    .unwrap_or_else(|| body.len().min(exchange.body.len()));
                return Err(format!(
                    "request {index}: the server read a body of {} bytes where the client sent {}, first difference at {at}",
                    body.len(),
                    exchange.body.len()
                ));
            }
            Some(Err(error)) if script.cut.is_none() => {
                return Err(format!(
                    "request {index}: reading the body failed without a fault: {error}"
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

fn check_responses(
    script: &Script,
    expected: &[Option<Expected>],
    client: &ClientLog,
    server: &ServerLog,
) -> Result<(), String> {
    let reset = matches!(script.cut, Some((_, Cut::Reset)));
    let answered = client.responses.len();
    if let Some(error) = &client.parse_error
        && !reset
    {
        return Err(format!(
            "response {answered}: the client could not read it: {error}"
        ));
    }
    if answered > server.handled.len() || (!reset && answered != server.handled.len()) {
        return Err(format!(
            "the client read {answered} responses where the server handled {} requests",
            server.handled.len()
        ));
    }
    for (index, response) in client.responses.iter().enumerate() {
        if script.malformed_at(index).is_some_and(Malformed::handled) {
            continue;
        }
        let Some(Some(wanted)) = expected.get(index) else {
            return Err(format!(
                "response {index} answers a request the script never sent whole"
            ));
        };
        if response.status != wanted.status {
            return Err(format!(
                "response {index}: status {} where the handler answered {}",
                response.status.as_u16(),
                wanted.status.as_u16()
            ));
        }
        let last = index + 1 == answered;
        let cut_short =
            reset && last && response.truncated && wanted.body.starts_with(&response.body);
        if response.body != wanted.body && !cut_short {
            return Err(format!(
                "response {index}: a body of {} bytes where the handler answered {} (truncated={})",
                response.body.len(),
                wanted.body.len(),
                response.truncated
            ));
        }
        if response.truncated && !wanted.truncated && !cut_short {
            return Err(format!(
                "response {index}: the connection closed before the body its content-length declared"
            ));
        }
        if index > 0
            && let Some(Some(previous)) = expected.get(index - 1)
            && previous.after == After::Must
        {
            return Err(format!(
                "response {index}: the server answered after response {} told it to close",
                index - 1
            ));
        }
    }
    Ok(())
}

fn check_result(
    script: &Script,
    server: &ServerLog,
    expected: &[Option<Expected>],
) -> Result<(), String> {
    let Some(result) = &server.result else {
        return Err("the server never returned".to_owned());
    };
    let handled = server.handled.len();
    let reached_malformed = script
        .malformed_at(handled)
        .is_some_and(|kind| !kind.handled())
        && (handled == 0
            || expected
                .get(handled - 1)
                .and_then(Option::as_ref)
                .is_some_and(|previous| previous.after == After::Continue));
    match result {
        Ok(())
            if reached_malformed && script.cut.is_none_or(|(at, _)| at >= script.ends[handled]) =>
        {
            Err(format!(
                "the server accepted the malformed request {handled} without an error"
            ))
        }
        Err(error)
            if error.starts_with("parse:")
                && script.malformed_at(handled).is_none_or(Malformed::handled) =>
        {
            Err(format!(
                "the server failed to parse request {handled}, which is well formed: {error}"
            ))
        }
        Err(error) if !error.starts_with("parse:") && script.cut.is_none() => {
            Err(format!("the server failed without a fault: {error}"))
        }
        _ => Ok(()),
    }
}

fn judge_run(
    script: &Script,
    client: &ClientLog,
    server: &ServerLog,
    ended: Ended,
    now_ns: u64,
) -> Result<(), String> {
    if ended == Ended::Stuck {
        return Err(format!(
            "every task waits and no timer is set: a hang ({} handled, {} answered)",
            server.handled.len(),
            client.responses.len()
        ));
    }
    let run_ms = now_ns / 1_000_000;
    if run_ms > RUN_MS_MAX {
        return Err(format!(
            "the run took {run_ms} ms of virtual time, over {RUN_MS_MAX} ms"
        ));
    }
    let expected = script
        .exchanges
        .iter()
        .enumerate()
        .map(|(index, exchange)| {
            script
                .malformed_at(index)
                .is_none()
                .then(|| expect(exchange))
        })
        .collect::<Vec<_>>();
    check_handled(script, server, bounds(script, &expected))?;
    check_responses(script, &expected, client, server)?;
    check_result(script, server, &expected)
}

fn stats_of(script: &Script, client: &ClientLog, server: &ServerLog) -> HttpServerStats {
    let exchanges = |keep: fn(&Exchange) -> bool| {
        count(
            script
                .exchanges
                .iter()
                .filter(|exchange| keep(exchange))
                .count(),
        )
    };
    HttpServerStats {
        seeds: 1,
        requests: count(script.exchanges.len()),
        handled: count(server.handled.len()),
        responses: count(client.responses.len()),
        chunked: exchanges(|exchange| exchange.framing == Framing::Chunked),
        early: exchanges(|exchange| exchange.handling != Handling::ReadAll),
        mismatched: exchanges(|exchange| {
            matches!(
                exchange.reply,
                Body::Stream(_, Declared::Short(_) | Declared::Long(_))
            )
        }),
        malformed: u64::from(script.malformed.is_some()),
        cuts: u64::from(script.cut.is_some()),
        resets: u64::from(matches!(script.cut, Some((_, Cut::Reset)))),
    }
}

pub struct HttpServerOutcome {
    pub digest: u64,
    pub wrong: Option<String>,
    pub stats: HttpServerStats,
    pub shape: String,
}

fn simulate(seed: u64, mutation: HttpServerMutation, verbose: bool) -> HttpServerOutcome {
    let script = Script::of(seed, mutation);
    let shape = script.describe();
    let clock = Clock::new();
    crate::host::enter(seed, &clock);
    let trace = Trace {
        clock: Arc::clone(&clock),
        digest: Cell::new(FNV_OFFSET),
        verbose,
    };
    trace.line(&format!("seed {seed} {shape}"));
    let (duplex, side) = socket::pair(
        Rng::derive(seed, "http-server-socket"),
        script.read_piece_max,
        script.write_piece_max,
    );
    let client_log = RefCell::new(ClientLog::default());
    let server_log = RefCell::new(ServerLog::default());
    let tasks: Vec<Task<'_>> = vec![
        Box::pin(serve(&script, duplex, &server_log, &trace)),
        Box::pin(client(
            &script,
            side,
            &clock,
            &client_log,
            &trace,
            mutation,
            seed,
        )),
    ];
    let ended = Executor::new(Arc::clone(&clock), Rng::derive(seed, "scheduler")).run(tasks, None);
    let client_log = client_log.into_inner();
    let server_log = server_log.into_inner();
    let wrong = judge_run(&script, &client_log, &server_log, ended, clock.now_ns()).err();
    trace.line(&format!("verdict {wrong:?}"));
    HttpServerOutcome {
        digest: trace.digest.get(),
        wrong,
        stats: stats_of(&script, &client_log, &server_log),
        shape,
    }
}

pub fn run_seed(seed: u64, mutation: HttpServerMutation, verbose: bool) -> HttpServerOutcome {
    std::panic::catch_unwind(|| simulate(seed, mutation, verbose)).unwrap_or_else(|panic| {
        let message = panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|text| (*text).to_owned()))
            .unwrap_or_default();
        HttpServerOutcome {
            digest: 0,
            wrong: Some(format!("panic: {message}")),
            stats: HttpServerStats::default(),
            shape: String::new(),
        }
    })
}

pub fn judge(
    seed: u64,
    mutation: HttpServerMutation,
    twice: bool,
    verbose: bool,
    totals: &mut HttpServerStats,
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

pub fn replay(seed: u64, mutation: HttpServerMutation) -> String {
    let mut command = format!("nanocode-sim --scenario http_server --seed {seed}");
    if mutation != HttpServerMutation::None {
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
    mutation: HttpServerMutation,
    totals: &HttpServerStats,
) -> ExitCode {
    println!(
        "RESULT seeds={}..{} scenario=http_server mutation={} passed={passed} failed={failed} twice={twice} stats={totals:?}",
        seeds.start,
        seeds.end,
        mutation.name()
    );
    match mutation {
        HttpServerMutation::None if failed == 0 => ExitCode::SUCCESS,
        HttpServerMutation::None => ExitCode::FAILURE,
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
