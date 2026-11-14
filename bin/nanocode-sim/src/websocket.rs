mod model;
mod script;
mod socket;
mod wire;

use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    future::poll_fn,
    ops::Range,
    process::ExitCode,
    rc::Rc,
    sync::Arc,
    task::{Poll, Waker},
    time::Duration,
};

use futures_util::{SinkExt as _, StreamExt as _};
use nanocode::codex::transport::InflateStream;
use nanocode_websocket::{
    CloseCode, CloseFrame, IntoClientRequest, Message, Utf8Bytes, WebSocketStream,
    client_async_with_config,
};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::{
    executor::{Clock, Ended, Executor, Task},
    rng::{FNV_OFFSET, Rng, fnv},
};
use model::{Expected, Seen, expect, failure_name, reset};
use script::{Cut, DEFLATE_OFFER, Ending, Script, SendKind};
use socket::{ClientSide, Wire};
use wire::{
    OPCODE_CLOSE, OPCODE_PING, OPCODE_PONG, ServerFrame, accept_key, decode_client, is_valid_key,
};

pub const RUN_MS_MAX: u64 = 30 * 60 * 1000;
const REQUEST_URL: &str = "ws://sim.test/socket";
const REQUEST_LINE: &str = "GET /socket HTTP/1.1";
const REQUEST_HOST: &str = "sim.test";
const HEAD_END: &[u8] = b"\r\n\r\n";
const OBSERVATIONS_SLACK: usize = 4;
const CORRUPT_BITS: u8 = 0x01;
const DESCRIBED_BYTES_MAX: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebSocketMutation {
    None,
    CorruptPayload,
    DropClientByte,
    HoldEof,
}

impl WebSocketMutation {
    const ALL: [Self; 4] = [
        Self::None,
        Self::CorruptPayload,
        Self::DropClientByte,
        Self::HoldEof,
    ];

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|mutation| mutation.name() == name)
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::CorruptPayload => "corrupt_payload",
            Self::DropClientByte => "drop_client_byte",
            Self::HoldEof => "hold_eof",
        }
    }

    pub fn names() -> String {
        Self::ALL.map(Self::name).join("|")
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct WebSocketStats {
    pub seeds: u64,
    pub handshakes_failed: u64,
    pub delivered: u64,
    pub delivered_bytes: u64,
    pub pings: u64,
    pub server_closes: u64,
    pub client_closes: u64,
    pub cuts: u64,
    pub violations: u64,
    pub client_messages: u64,
    pub client_bytes: u64,
}

impl WebSocketStats {
    pub const fn add(&mut self, other: &Self) {
        self.seeds += other.seeds;
        self.handshakes_failed += other.handshakes_failed;
        self.delivered += other.delivered;
        self.delivered_bytes += other.delivered_bytes;
        self.pings += other.pings;
        self.server_closes += other.server_closes;
        self.client_closes += other.client_closes;
        self.cuts += other.cuts;
        self.violations += other.violations;
        self.client_messages += other.client_messages;
        self.client_bytes += other.client_bytes;
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

#[derive(Default)]
struct ClientLog {
    seen: Vec<Seen>,
    sent: Vec<(u8, Vec<u8>)>,
    send_failed: Option<(u8, Vec<u8>, String)>,
    close_failed: Option<String>,
}

#[derive(Default)]
struct ServerLog {
    key: Option<String>,
    request_failed: bool,
    violations: Vec<String>,
    messages: Vec<(u8, Vec<u8>)>,
    pongs: Vec<Vec<u8>>,
    closes: Vec<Vec<u8>>,
    client_gone: bool,
    waiter: Option<Waker>,
}

impl ServerLog {
    fn wake(&mut self) {
        if let Some(waiter) = self.waiter.take() {
            waiter.wake();
        }
    }
}

fn describe_bytes(bytes: &[u8]) -> String {
    let shown = &bytes[..bytes.len().min(DESCRIBED_BYTES_MAX)];
    format!(
        "{} bytes fnv {:016x} starting {shown:02x?}",
        bytes.len(),
        fnv(FNV_OFFSET, bytes)
    )
}

fn describe(seen: Option<&Seen>) -> String {
    match seen {
        None => "nothing".to_owned(),
        Some(Seen::Text(bytes)) => format!("Text({})", describe_bytes(bytes)),
        Some(Seen::Binary(bytes)) => format!("Binary({})", describe_bytes(bytes)),
        Some(Seen::Ping(bytes)) => format!("Ping({})", describe_bytes(bytes)),
        Some(Seen::Pong(bytes)) => format!("Pong({})", describe_bytes(bytes)),
        Some(Seen::Close(frame)) => format!(
            "Close({:?})",
            frame
                .as_ref()
                .map(|(code, reason)| (code, String::from_utf8_lossy(reason)))
        ),
        Some(Seen::Failed(failure)) => format!("Failed({failure})"),
        Some(Seen::End) => "End".to_owned(),
        Some(Seen::Unexpected(what)) => format!("Unexpected({what})"),
    }
}

fn seen_of(message: Message) -> Seen {
    match message {
        Message::Text(text) => Seen::Text(text.as_str().as_bytes().to_vec()),
        Message::Binary(bytes) => Seen::Binary(bytes),
        Message::Ping(bytes) => Seen::Ping(bytes),
        Message::Pong(bytes) => Seen::Pong(bytes),
        Message::Close(frame) => Seen::Close(
            frame.map(|frame| (u16::from(frame.code), frame.reason.as_bytes().to_vec())),
        ),
        Message::Frame(_) => Seen::Unexpected("a raw frame".to_owned()),
    }
}

async fn until(log: &RefCell<ServerLog>, ready: impl Fn(&ServerLog) -> bool) {
    poll_fn(|context| {
        let mut log = log.borrow_mut();
        if ready(&log) {
            return Poll::Ready(());
        }
        log.waiter = Some(context.waker().clone());
        Poll::Pending
    })
    .await;
}

fn record(log: &RefCell<ClientLog>, trace: &Trace, seen: Seen) {
    trace.line(&format!("client {}", describe(Some(&seen))));
    log.borrow_mut().seen.push(seen);
}

fn client_message(kind: SendKind, payload: &[u8]) -> Message {
    match kind {
        SendKind::Text => {
            Message::text(String::from_utf8(payload.to_vec()).expect("the script sends UTF-8 text"))
        }
        SendKind::Binary => Message::Binary(payload.to_vec()),
        SendKind::Ping => Message::Ping(payload.to_vec()),
    }
}

async fn client(script: &Script, side: ClientSide, log: &RefCell<ClientLog>, trace: &Trace) {
    if script.deflate.is_none() {
        match client_async_with_config(REQUEST_URL, side, Some(script.config)).await {
            Ok((socket, _response)) => drive(script, socket, log, trace).await,
            Err(error) => record(log, trace, Seen::Failed(failure_name(&error))),
        }
        return;
    }
    let mut request = REQUEST_URL
        .into_client_request()
        .expect("the simulated URL is a WebSocket request");
    request.headers_mut().insert(
        http::header::SEC_WEBSOCKET_EXTENSIONS,
        http::HeaderValue::from_static(DEFLATE_OFFER),
    );
    let stream = InflateStream::new(side);
    match client_async_with_config(request, stream, Some(script.config)).await {
        Ok((socket, _response)) => drive(script, socket, log, trace).await,
        Err(error) => record(log, trace, Seen::Failed(failure_name(&error))),
    }
}

async fn drive<S: AsyncRead + AsyncWrite + Unpin>(
    script: &Script,
    mut socket: WebSocketStream<S>,
    log: &RefCell<ClientLog>,
    trace: &Trace,
) {
    trace.line("client connected");
    let mut received = 0;
    let mut next_send = 0;
    let mut closed = false;
    for _ in 0..script.frames.len() + OBSERVATIONS_SLACK {
        while let Some(send) = script.client.sends.get(next_send)
            && send.after == received
        {
            next_send += 1;
            if log.borrow().send_failed.is_some() {
                continue;
            }
            let sent = socket.send(client_message(send.kind, &send.payload)).await;
            trace.line(&format!(
                "client sent {:?} {} {}",
                send.kind,
                send.payload.len(),
                sent.is_ok()
            ));
            let mut log = log.borrow_mut();
            match sent {
                Ok(()) => log.sent.push((send.opcode(), send.payload.clone())),
                Err(error) => {
                    log.send_failed =
                        Some((send.opcode(), send.payload.clone(), failure_name(&error)));
                }
            }
        }
        if let Some((after, frame)) = &script.client.close
            && *after == received
            && !closed
        {
            closed = true;
            let frame = frame.as_ref().map(|(code, reason)| CloseFrame {
                code: CloseCode::from(*code),
                reason: Utf8Bytes::from(reason.as_str()),
            });
            let result = socket.close(frame).await;
            trace.line(&format!("client closed {}", result.is_ok()));
            if let Err(error) = result {
                log.borrow_mut().close_failed = Some(failure_name(&error));
            }
        }
        let seen = match socket.next().await {
            None => Seen::End,
            Some(Ok(message)) => seen_of(message),
            Some(Err(error)) => Seen::Failed(failure_name(&error)),
        };
        if matches!(seen, Seen::Text(_) | Seen::Binary(_)) {
            received += 1;
        }
        let end = seen == Seen::End;
        record(log, trace, seen);
        if end {
            return;
        }
    }
    record(
        log,
        trace,
        Seen::Unexpected("more observations than the script has frames".to_owned()),
    );
}

async fn serve(
    script: &Script,
    wire: &Wire,
    log: &RefCell<ServerLog>,
    trace: &Trace,
    mutation: WebSocketMutation,
    seed: u64,
) {
    let eof = || {
        if mutation != WebSocketMutation::HoldEof {
            wire.server_eof();
        }
    };
    until(log, |log| {
        log.key.is_some() || log.request_failed || log.client_gone
    })
    .await;
    let key = log.borrow().key.clone();
    let Some(key) = key else {
        trace.line("server saw no request key");
        eof();
        return;
    };
    let mut stream = script.head.render(&accept_key(&key));
    assert_eq!(
        stream.len(),
        script.head_len,
        "the head has the length the script planned"
    );
    stream.extend_from_slice(&script.frame_bytes);
    if mutation == WebSocketMutation::CorruptPayload
        && let Some(at) = script.corrupt_at
    {
        stream[script.head_len + at] ^= CORRUPT_BITS;
    }
    let limit = match script.ending {
        Ending::Cut { at, .. } => at,
        Ending::ServerClose | Ending::ClientClose | Ending::Finish => stream.len(),
    };
    let mut rng = Rng::derive(seed, "websocket-pieces");
    let mut offset = 0;
    for (gap_ms, end) in script.pieces(&mut rng, limit) {
        nanocode_platform::sleep(Duration::from_millis(gap_ms)).await;
        wire.server_write(&stream[offset..end]);
        offset = end;
    }
    trace.line(&format!("server wrote {offset} bytes"));
    let closed = |log: &ServerLog| !log.closes.is_empty() || log.client_gone;
    match script.ending {
        Ending::Cut {
            kind: Cut::Reset, ..
        } => {
            trace.line("server reset");
            wire.reset();
        }
        Ending::Cut { kind: Cut::Eof, .. } | Ending::Finish => eof(),
        Ending::ServerClose => {
            until(log, closed).await;
            eof();
        }
        Ending::ClientClose => {
            until(log, closed).await;
            let payload = log.borrow().closes.first().cloned().unwrap_or_default();
            let mut echo = Vec::new();
            ServerFrame::new(OPCODE_CLOSE, true, payload).encode_into(&mut echo);
            trace.line("server echoed the close");
            wire.server_write(&echo);
            eof();
        }
    }
}

fn parse_request(head: &[u8], deflate: bool) -> Result<String, String> {
    let text = std::str::from_utf8(head)
        .map_err(|error| format!("the request head is not UTF-8: {error}"))?;
    let mut lines = text.split("\r\n");
    let line = lines.next().unwrap_or_default();
    if line != REQUEST_LINE {
        return Err(format!("the request line is {line:?}"));
    }
    let mut headers = BTreeMap::new();
    for line in lines.take_while(|line| !line.is_empty()) {
        let (name, value) = line
            .split_once(": ")
            .ok_or_else(|| format!("the request header line {line:?} has no ': '"))?;
        if headers
            .insert(name.to_ascii_lowercase(), value.to_owned())
            .is_some()
        {
            return Err(format!("the request repeats {name}"));
        }
    }
    let header = |name: &str| headers.get(name).map(String::as_str).unwrap_or_default();
    if header("host") != REQUEST_HOST {
        return Err(format!("the request's Host is {:?}", header("host")));
    }
    if !header("connection")
        .split(',')
        .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
    {
        return Err(format!(
            "the request's Connection is {:?}",
            header("connection")
        ));
    }
    if !header("upgrade").eq_ignore_ascii_case("websocket") {
        return Err(format!("the request's Upgrade is {:?}", header("upgrade")));
    }
    if header("sec-websocket-version") != "13" {
        return Err(format!(
            "the request's version is {:?}",
            header("sec-websocket-version")
        ));
    }
    let offer = if deflate { DEFLATE_OFFER } else { "" };
    if header("sec-websocket-extensions") != offer {
        return Err(format!(
            "the request's extensions are {:?}",
            header("sec-websocket-extensions")
        ));
    }
    let key = header("sec-websocket-key");
    if !is_valid_key(key) {
        return Err(format!(
            "the request's key {key:?} is not 16 bytes of base64"
        ));
    }
    Ok(key.to_owned())
}

fn take_frames(buffer: &mut Vec<u8>, log: &RefCell<ServerLog>, trace: &Trace) -> bool {
    loop {
        match decode_client(buffer) {
            Ok(None) => return true,
            Ok(Some((frame, used))) => {
                buffer.drain(..used);
                trace.line(&format!(
                    "server read opcode {:#x} {}",
                    frame.opcode,
                    describe_bytes(&frame.payload)
                ));
                let mut log = log.borrow_mut();
                if !frame.fin {
                    log.violations.push(format!(
                        "the client fragmented a message: opcode {:#x}",
                        frame.opcode
                    ));
                    return false;
                }
                if !log.closes.is_empty()
                    && frame.opcode != OPCODE_PONG
                    && frame.opcode != OPCODE_PING
                {
                    log.violations.push(format!(
                        "the client sent opcode {:#x} after its close",
                        frame.opcode
                    ));
                    return false;
                }
                match frame.opcode {
                    OPCODE_PONG => log.pongs.push(frame.payload),
                    OPCODE_CLOSE => log.closes.push(frame.payload),
                    opcode => log.messages.push((opcode, frame.payload)),
                }
            }
            Err(violation) => {
                log.borrow_mut().violations.push(violation);
                return false;
            }
        }
    }
}

async fn listen(
    script: &Script,
    wire: &Wire,
    log: &RefCell<ServerLog>,
    trace: &Trace,
    mutation: WebSocketMutation,
    seed: u64,
) {
    let mut rng = Rng::derive(seed, "websocket-reader");
    let plan = &script.client;
    let low = u64::try_from(plan.read_bytes_min.max(1)).expect("fits");
    let high = u64::try_from(plan.read_bytes_max.max(1)).expect("fits");
    let bytes_bound = plan.read_bytes_min.max(1) * script::PIECES_MAX * 2 + script.frames.len() + 1;
    let mut buffer = Vec::new();
    let mut head_done = false;
    let mut parsing = true;
    let mut dropped = false;
    for _ in 0..bytes_bound {
        nanocode_platform::sleep(Duration::from_millis(rng.range(0, plan.gap_ms_max))).await;
        let chunk = usize::try_from(rng.range(low, high.max(low))).expect("fits");
        match wire.server_read(chunk).await {
            Ok(bytes) if bytes.is_empty() => break,
            Ok(bytes) => buffer.extend_from_slice(&bytes),
            Err(error) => {
                trace.line(&format!("server read failed: {:?}", error.kind()));
                break;
            }
        }
        if !head_done {
            let Some(end) = buffer
                .windows(HEAD_END.len())
                .position(|window| window == HEAD_END)
            else {
                continue;
            };
            let head: Vec<u8> = buffer.drain(..end + HEAD_END.len()).collect();
            head_done = true;
            let mut log = log.borrow_mut();
            match parse_request(&head, script.deflate.is_some()) {
                Ok(key) => log.key = Some(key),
                Err(violation) => {
                    log.violations.push(violation);
                    log.request_failed = true;
                    parsing = false;
                }
            }
        }
        if mutation == WebSocketMutation::DropClientByte && !dropped && buffer.len() >= 2 {
            buffer.remove(1);
            dropped = true;
        }
        if parsing {
            parsing = take_frames(&mut buffer, log, trace);
        }
        log.borrow_mut().wake();
    }
    trace.line("server saw the client end");
    let mut log = log.borrow_mut();
    if !head_done {
        log.request_failed = true;
    }
    log.client_gone = true;
    log.wake();
}

fn check_seen(expected: &Expected, seen: &[Seen]) -> Result<(), String> {
    if seen == expected.seen.as_slice() {
        return Ok(());
    }
    if expected.reset
        && let [before @ .., Seen::Failed(failure), Seen::End] = seen
        && *failure == reset()
        && expected.seen.starts_with(before)
        && !before.iter().any(|seen| matches!(seen, Seen::Failed(_)))
    {
        return Ok(());
    }
    let at = seen
        .iter()
        .zip(&expected.seen)
        .position(|(seen, expected)| seen != expected)
        .unwrap_or_else(|| seen.len().min(expected.seen.len()));
    Err(format!(
        "observation {at}: the client saw {} where the server's script means {} ({} observed, {} expected)",
        describe(seen.get(at)),
        describe(expected.seen.get(at)),
        seen.len(),
        expected.seen.len()
    ))
}

fn check_list(
    what: &str,
    expected: &[Vec<u8>],
    got: &[Vec<u8>],
    prefix: bool,
) -> Result<(), String> {
    if got == expected || (prefix && expected.starts_with(got)) {
        return Ok(());
    }
    let at = got
        .iter()
        .zip(expected)
        .position(|(got, expected)| got != expected)
        .unwrap_or_else(|| got.len().min(expected.len()));
    Err(format!(
        "{what} {at}: the server read {} where the client owes {} ({} read, {} owed)",
        got.get(at)
            .map_or_else(|| "nothing".to_owned(), |bytes| describe_bytes(bytes)),
        expected
            .get(at)
            .map_or_else(|| "nothing".to_owned(), |bytes| describe_bytes(bytes)),
        got.len(),
        expected.len()
    ))
}

fn check_sends(client: &ClientLog, server: &ServerLog) -> Result<(), String> {
    let mut owed = client.sent.clone();
    if server.messages.len() == owed.len() + 1
        && let Some((opcode, payload, _)) = &client.send_failed
    {
        owed.push((*opcode, payload.clone()));
    }
    if server.messages == owed {
        return Ok(());
    }
    let at = server
        .messages
        .iter()
        .zip(&owed)
        .position(|(got, owed)| got != owed)
        .unwrap_or_else(|| server.messages.len().min(owed.len()));
    let show = |message: Option<&(u8, Vec<u8>)>| {
        message.map_or_else(
            || "nothing".to_owned(),
            |(opcode, bytes)| format!("opcode {opcode:#x} {}", describe_bytes(bytes)),
        )
    };
    Err(format!(
        "client message {at}: the server read {} where the client sent {} ({} read, {} sent)",
        show(server.messages.get(at)),
        show(owed.get(at)),
        server.messages.len(),
        owed.len()
    ))
}

fn judge_run(
    expected: &Expected,
    client: &ClientLog,
    server: &ServerLog,
    ended: Ended,
    now_ns: u64,
) -> Result<(), String> {
    if ended == Ended::Stuck {
        return Err(format!(
            "every task waits and no timer is set: a hang (client saw {:?})",
            client
                .seen
                .iter()
                .map(|seen| describe(Some(seen)))
                .collect::<Vec<_>>()
        ));
    }
    let run_ms = now_ns / 1_000_000;
    if run_ms > RUN_MS_MAX {
        return Err(format!(
            "the run took {run_ms} ms of virtual time, over {RUN_MS_MAX} ms"
        ));
    }
    check_seen(expected, &client.seen)?;
    if let Some(violation) = server.violations.first() {
        return Err(format!("the client broke the protocol: {violation}"));
    }
    check_sends(client, server)?;
    let failed = expected.reset
        || expected
            .seen
            .iter()
            .any(|seen| matches!(seen, Seen::Failed(_)));
    check_list("pong", &expected.pongs, &server.pongs, failed)?;
    check_list("close", &expected.closes, &server.closes, failed)?;
    if !expected.reset {
        if let Some((opcode, _, failure)) = &client.send_failed {
            return Err(format!(
                "sending opcode {opcode:#x} failed without a fault: {failure}"
            ));
        }
        if let Some(failure) = &client.close_failed {
            return Err(format!("closing failed without a fault: {failure}"));
        }
    }
    Ok(())
}

fn stats_of(script: &Script, client: &ClientLog) -> WebSocketStats {
    let mut stats = WebSocketStats {
        seeds: 1,
        cuts: u64::from(matches!(script.ending, Ending::Cut { .. })),
        violations: u64::from(script.violation.is_some()),
        client_messages: u64::try_from(client.sent.len()).expect("fits"),
        client_bytes: client
            .sent
            .iter()
            .map(|(_, bytes)| u64::try_from(bytes.len()).expect("fits"))
            .sum(),
        client_closes: u64::from(script.ending == Ending::ClientClose),
        ..WebSocketStats::default()
    };
    for seen in &client.seen {
        match seen {
            Seen::Text(bytes) | Seen::Binary(bytes) => {
                stats.delivered += 1;
                stats.delivered_bytes += u64::try_from(bytes.len()).expect("fits");
            }
            Seen::Ping(_) => stats.pings += 1,
            Seen::Close(_) => stats.server_closes += 1,
            Seen::Failed(_) if client.seen.len() == 1 => stats.handshakes_failed += 1,
            Seen::Pong(_) | Seen::Failed(_) | Seen::End | Seen::Unexpected(_) => {}
        }
    }
    stats
}

pub struct WebSocketOutcome {
    pub digest: u64,
    pub wrong: Option<String>,
    pub stats: WebSocketStats,
    pub shape: String,
}

fn simulate(seed: u64, mutation: WebSocketMutation, verbose: bool) -> WebSocketOutcome {
    let script = Script::of(seed);
    let shape = script.describe();
    let expected = expect(&script);
    let clock = Clock::new();
    crate::host::enter(seed, &clock);
    let trace = Trace {
        clock: Arc::clone(&clock),
        digest: Cell::new(FNV_OFFSET),
        verbose,
    };
    trace.line(&format!("seed {seed} {shape}"));
    let wire = Rc::new(Wire::new(script.client.capacity_bytes));
    let client_log = RefCell::new(ClientLog::default());
    let server_log = RefCell::new(ServerLog::default());
    let side = ClientSide::new(Rc::clone(&wire), script.client.vectored);
    let tasks: Vec<Task<'_>> = vec![
        Box::pin(client(&script, side, &client_log, &trace)),
        Box::pin(serve(&script, &wire, &server_log, &trace, mutation, seed)),
        Box::pin(listen(&script, &wire, &server_log, &trace, mutation, seed)),
    ];
    let ended = Executor::new(Arc::clone(&clock), Rng::derive(seed, "scheduler")).run(tasks, None);
    let client_log = client_log.into_inner();
    let server_log = server_log.into_inner();
    let wrong = judge_run(&expected, &client_log, &server_log, ended, clock.now_ns()).err();
    trace.line(&format!("verdict {wrong:?}"));
    WebSocketOutcome {
        digest: trace.digest.get(),
        wrong,
        stats: stats_of(&script, &client_log),
        shape,
    }
}

pub fn run_seed(seed: u64, mutation: WebSocketMutation, verbose: bool) -> WebSocketOutcome {
    std::panic::catch_unwind(|| simulate(seed, mutation, verbose)).unwrap_or_else(|panic| {
        let message = panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|text| (*text).to_owned()))
            .unwrap_or_default();
        WebSocketOutcome {
            digest: 0,
            wrong: Some(format!("panic: {message}")),
            stats: WebSocketStats::default(),
            shape: String::new(),
        }
    })
}

pub fn judge(
    seed: u64,
    mutation: WebSocketMutation,
    twice: bool,
    verbose: bool,
    totals: &mut WebSocketStats,
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

pub fn replay(seed: u64, mutation: WebSocketMutation) -> String {
    let mut command = format!("nanocode-sim --scenario websocket --seed {seed}");
    if mutation != WebSocketMutation::None {
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
    mutation: WebSocketMutation,
    totals: &WebSocketStats,
) -> ExitCode {
    println!(
        "RESULT seeds={}..{} scenario=websocket mutation={} passed={passed} failed={failed} twice={twice} stats={totals:?}",
        seeds.start,
        seeds.end,
        mutation.name()
    );
    match mutation {
        WebSocketMutation::None if failed == 0 => ExitCode::SUCCESS,
        WebSocketMutation::None => ExitCode::FAILURE,
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
