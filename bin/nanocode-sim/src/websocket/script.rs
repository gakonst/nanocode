use nanocode_websocket::{READ_BUFFER_BYTES, WebSocketConfig};

use super::wire::{
    CLOSE_CODE_BYTES, CONTROL_PAYLOAD_BYTES_MAX, LENGTH_16_MAX, LengthForm, MASK_BYTES,
    OPCODE_BINARY, OPCODE_CLOSE, OPCODE_CONTINUE, OPCODE_PING, OPCODE_PONG, OPCODE_TEXT, RSV1_BIT,
    RSV2_BIT, RSV3_BIT, ServerFrame, close_payload,
};
use crate::rng::{PPM, Rng};

pub const MESSAGES_MAX: u64 = 10;
pub const FRAGMENTS_MAX: u64 = 6;
pub const CLIENT_SENDS_MAX: u64 = 6;
const BOUNDARY_SIZES: [usize; 11] = [
    0, 1, 125, 126, 127, 65_535, 65_536, 65_537, 131_071, 131_072, 131_073,
];
pub const SMALL_BYTES_MAX: u64 = 300;
pub const MEDIUM_BYTES_MAX: u64 = 70_000;
pub const LARGE_BYTES_MAX: u64 = 1_200_000;
const SIZE_ROLLS: u64 = 100;
const BOUNDARY_ROLLS_END: u64 = 30;
const SMALL_ROLLS_END: u64 = 70;
const MEDIUM_ROLLS_END: u64 = 98;
pub const VIOLATION_PPM: u64 = 250_000;
pub const FRAGMENTED_PPM: u64 = 500_000;
pub const CONTROL_BETWEEN_PPM: u64 = 300_000;
pub const PING_BETWEEN_PPM: u64 = 200_000;
pub const WIDE_LENGTH_PPM: u64 = 150_000;
pub const SMALL_LIMITS_PPM: u64 = 250_000;
pub const FRAME_LIMIT_BYTES_MAX: u64 = 200_000;
pub const MESSAGE_LIMIT_BYTES_MAX: u64 = 400_000;
const READ_BUFFER_CHOICES: [usize; 5] = [1, 14, 100, 4096, READ_BUFFER_BYTES];
pub const VALID_HEAD_PPM: u64 = 800_000;
pub const BIG_HEAD_PPM: u64 = 50_000;
pub const PADDING_HEADERS_MAX: u64 = 100;
pub const HEADERS_ACCEPTED_MAX: usize = 124;
pub const FLOOD_BYTES_MAX: usize = 65_536;
pub const HANDSHAKE_READ_BYTES: usize = 4096;
pub const BIG_HEAD_BYTES_MAX: usize = FLOOD_BYTES_MAX - HANDSHAKE_READ_BYTES;
pub const OVERSIZED_HEAD_BYTES: usize = FLOOD_BYTES_MAX + HANDSHAKE_READ_BYTES;
pub const HEAD_PIECES_MAX: usize = 8;
pub const PIECES_MAX: usize = 4096;
pub const TINY_PIECE_BYTES_MAX: u64 = 16;
pub const SMALL_PIECE_BYTES_MAX: u64 = 1500;
pub const LARGE_PIECE_BYTES_MAX: u64 = 262_144;
pub const GAP_MS_MAX: u64 = 100;
pub const BURSTY_GAP_MS_MAX: u64 = 5;
pub const CLIENT_CAPACITY_BYTES_MAX: u64 = 300_000;
pub const TINY_CAPACITY_BYTES_MAX: u64 = 64;
pub const REQUEST_HEAD_BYTES_MAX: usize = 1024;
pub const CLIENT_FRAME_OVERHEAD_BYTES: usize = 14;
const PLACEHOLDER_ACCEPT: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAA=";
const PROTOCOL_HEADER: &str = "Sec-WebSocket-Protocol";
const CONNECTION_VALUES: [&str; 4] = ["Upgrade", "upgrade", "UPGRADE", "keep-alive, Upgrade"];
const UPGRADE_VALUES: [&str; 3] = ["websocket", "WebSocket", "WEBSOCKET"];
const STATUSES: [u16; 8] = [200, 400, 401, 403, 404, 426, 500, 503];
const ALLOWED_CODES: [u16; 14] = [
    1000, 1001, 1002, 1003, 1007, 1008, 1009, 1010, 1011, 1012, 1013, 3000, 3999, 4999,
];
const FORBIDDEN_CODES: [u16; 11] = [
    0, 999, 1004, 1005, 1006, 1015, 1016, 2999, 5000, 9999, 65535,
];
const INVALID_UTF8: [&[u8]; 5] = [
    &[0xff],
    &[0xc0, 0x80],
    &[0xed, 0xa0, 0x80],
    &[0x80],
    &[0xf4, 0x90, 0x80, 0x80],
];
const TRUNCATED_UTF8: [&[u8]; 3] = [&[0xe2, 0x82], &[0xc3], &[0xf0, 0x9f, 0x98]];
const UNKNOWN_OPCODES: [u8; 10] = [0x3, 0x4, 0x5, 0x6, 0x7, 0xb, 0xc, 0xd, 0xe, 0xf];
const TEXT_CHARACTERS: [char; 5] = ['a', ' ', '\u{e9}', '\u{20ac}', '\u{1f600}'];
const HEAD_FAULTS: [Head; 11] = [
    Head::WrongAccept,
    Head::MissingAccept,
    Head::MissingUpgrade,
    Head::MissingConnection,
    Head::Status(0),
    Head::OldVersion,
    Head::SubProtocol,
    Head::TooManyHeaders,
    Head::Oversized,
    Head::BadStatus,
    Head::BadVersion,
];
const ENDING_ROLLS: u64 = 20;
const SERVER_CLOSE_ROLLS_END: u64 = 6;
const CLIENT_CLOSE_ROLLS_END: u64 = 10;
const CUT_ROLLS_END: u64 = 16;
const EMPTY_CLOSE_PPM: u64 = 200_000;
const FORBIDDEN_CODE_PPM: u64 = 200_000;
const CLIENT_CLOSE_FRAME_PPM: u64 = 800_000;
const CLIENT_PING_PPM: u64 = 200_000;
const CLIENT_TEXT_PPM: u64 = 500_000;
const UNBOUNDED_CAPACITY_PPM: u64 = 250_000;
const TINY_CAPACITY_PPM: u64 = 333_333;
const NO_GAP_PPM: u64 = 333_333;
const BURSTY_GAP_PPM: u64 = 500_000;
const OVERSIZED_CONTROL_BYTES_MAX: u64 = 400;
const CLOSE_REASON_BYTES_MAX: usize = CONTROL_PAYLOAD_BYTES_MAX - CLOSE_CODE_BYTES;
const NORMAL_CLOSE: u16 = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Head {
    Valid,
    WrongAccept,
    MissingAccept,
    MissingUpgrade,
    MissingConnection,
    Status(u16),
    OldVersion,
    SubProtocol,
    TooManyHeaders,
    Oversized,
    BadStatus,
    BadVersion,
}

impl Head {
    pub const fn allows_cut(self) -> bool {
        matches!(
            self,
            Self::Valid
                | Self::WrongAccept
                | Self::MissingAccept
                | Self::MissingUpgrade
                | Self::MissingConnection
                | Self::Status(_)
                | Self::OldVersion
                | Self::SubProtocol
        )
    }
}

pub struct HeadPlan {
    pub kind: Head,
    connection: &'static str,
    upgrade: &'static str,
    names_lower: bool,
    padding: Vec<(String, String)>,
}

impl HeadPlan {
    fn of(rng: &mut Rng) -> Self {
        let kind = if rng.chance(VALID_HEAD_PPM) {
            Head::Valid
        } else {
            match HEAD_FAULTS[rng.pick(HEAD_FAULTS.len())] {
                Head::Status(_) => Head::Status(STATUSES[rng.pick(STATUSES.len())]),
                other => other,
            }
        };
        let mut padding = Vec::new();
        match kind {
            Head::TooManyHeaders => {
                for index in 0..=HEADERS_ACCEPTED_MAX {
                    padding.push((format!("X-Pad-{index}"), "v".to_owned()));
                }
            }
            Head::Oversized => padding.push(("X-Big".to_owned(), "b".repeat(OVERSIZED_HEAD_BYTES))),
            _ if rng.chance(BIG_HEAD_PPM) => {
                let bytes = rng.range(1, u64::try_from(BIG_HEAD_BYTES_MAX / 2).expect("fits"));
                padding.push((
                    "X-Big".to_owned(),
                    "b".repeat(usize::try_from(bytes).expect("a header size fits in usize")),
                ));
            }
            _ => {
                for index in 0..rng.range(0, PADDING_HEADERS_MAX) {
                    padding.push((format!("X-Pad-{index}"), format!("value {index}")));
                }
            }
        }
        Self {
            kind,
            connection: CONNECTION_VALUES[rng.pick(CONNECTION_VALUES.len())],
            upgrade: UPGRADE_VALUES[rng.pick(UPGRADE_VALUES.len())],
            names_lower: rng.chance(PPM / 2),
            padding,
        }
    }

    pub fn render(&self, accept: &str) -> Vec<u8> {
        let status = match self.kind {
            Head::Status(code) => format!("HTTP/1.1 {code} Not Switching\r\n"),
            Head::OldVersion => "HTTP/1.0 101 Switching Protocols\r\n".to_owned(),
            Head::BadStatus => "HTTP/1.1 1x1 Switching Protocols\r\n".to_owned(),
            Head::BadVersion => "HTTP/9.x 101 Switching Protocols\r\n".to_owned(),
            _ => "HTTP/1.1 101 Switching Protocols\r\n".to_owned(),
        };
        let mut headers: Vec<(String, String)> = Vec::new();
        if self.kind != Head::MissingUpgrade {
            headers.push(("Upgrade".to_owned(), self.upgrade.to_owned()));
        }
        if self.kind != Head::MissingConnection {
            headers.push(("Connection".to_owned(), self.connection.to_owned()));
        }
        match self.kind {
            Head::MissingAccept => {}
            Head::WrongAccept => {
                let mut wrong = accept.to_owned();
                let replacement = if wrong.starts_with('A') { "B" } else { "A" };
                wrong.replace_range(0..1, replacement);
                headers.push(("Sec-WebSocket-Accept".to_owned(), wrong));
            }
            _ => headers.push(("Sec-WebSocket-Accept".to_owned(), accept.to_owned())),
        }
        if self.kind == Head::SubProtocol {
            headers.push((PROTOCOL_HEADER.to_owned(), "chat".to_owned()));
        }
        headers.extend(self.padding.iter().cloned());
        let mut head = status;
        for (name, value) in headers {
            let name = if self.names_lower {
                name.to_ascii_lowercase()
            } else {
                name
            };
            head.push_str(&name);
            head.push_str(": ");
            head.push_str(&value);
            head.push_str("\r\n");
        }
        head.push_str("\r\n");
        head.into_bytes()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Violation {
    Reserved,
    Opcode,
    Masked,
    ControlTooBig,
    ControlFragmented,
    BadUtf8,
    TruncatedUtf8,
    CloseOneByte,
    CloseBadReason,
    HighBitLength,
    ContinueWithoutStart,
    NewMessageMidFragment,
    AfterClose,
}

const VIOLATIONS: [Violation; 13] = [
    Violation::Reserved,
    Violation::Opcode,
    Violation::Masked,
    Violation::ControlTooBig,
    Violation::ControlFragmented,
    Violation::BadUtf8,
    Violation::TruncatedUtf8,
    Violation::CloseOneByte,
    Violation::CloseBadReason,
    Violation::HighBitLength,
    Violation::ContinueWithoutStart,
    Violation::NewMessageMidFragment,
    Violation::AfterClose,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cut {
    Eof,
    Reset,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ending {
    ServerClose,
    ClientClose,
    Cut { at: usize, kind: Cut },
    Finish,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendKind {
    Text,
    Binary,
    Ping,
}

#[derive(Clone, Debug)]
pub struct ClientSend {
    pub after: usize,
    pub kind: SendKind,
    pub payload: Vec<u8>,
}

impl ClientSend {
    pub const fn opcode(&self) -> u8 {
        match self.kind {
            SendKind::Text => OPCODE_TEXT,
            SendKind::Binary => OPCODE_BINARY,
            SendKind::Ping => OPCODE_PING,
        }
    }
}

pub struct ClientPlan {
    pub sends: Vec<ClientSend>,
    pub close: Option<(usize, Option<(u16, String)>)>,
    pub vectored: bool,
    pub capacity_bytes: Option<usize>,
    pub read_bytes_min: usize,
    pub read_bytes_max: usize,
    pub gap_ms_max: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PieceStyle {
    Tiny,
    Small,
    Large,
}

pub struct Script {
    pub config: WebSocketConfig,
    pub head: HeadPlan,
    pub head_len: usize,
    pub frames: Vec<ServerFrame>,
    pub frame_bytes: Vec<u8>,
    pub ending: Ending,
    pub violation: Option<Violation>,
    pub data_messages: usize,
    pub corrupt_at: Option<usize>,
    pub client: ClientPlan,
    pub pieces: PieceStyle,
    pub gap_ms_max: u64,
}

fn size(rng: &mut Rng) -> usize {
    let roll = rng.below(SIZE_ROLLS);
    let bytes = if roll < BOUNDARY_ROLLS_END {
        return BOUNDARY_SIZES[rng.pick(BOUNDARY_SIZES.len())];
    } else if roll < SMALL_ROLLS_END {
        rng.range(0, SMALL_BYTES_MAX)
    } else if roll < MEDIUM_ROLLS_END {
        rng.range(0, MEDIUM_BYTES_MAX)
    } else {
        rng.range(0, LARGE_BYTES_MAX)
    };
    usize::try_from(bytes).expect("a message size fits in usize")
}

pub fn binary(rng: &mut Rng, bytes: usize) -> Vec<u8> {
    let mut payload = Vec::with_capacity(bytes + size_of::<u64>());
    while payload.len() < bytes {
        payload.extend_from_slice(&rng.draw().to_le_bytes());
    }
    payload.truncate(bytes);
    payload
}

pub fn text(rng: &mut Rng, bytes: usize) -> Vec<u8> {
    let mut text = String::with_capacity(bytes);
    while text.len() < bytes {
        let left = bytes - text.len();
        let character = TEXT_CHARACTERS[rng.pick(TEXT_CHARACTERS.len())];
        if character.len_utf8() <= left {
            text.push(character);
        } else {
            text.push('a');
        }
    }
    text.into_bytes()
}

fn control_size(rng: &mut Rng) -> usize {
    let bytes = rng.range(0, u64::try_from(CONTROL_PAYLOAD_BYTES_MAX).expect("fits"));
    usize::try_from(bytes).expect("a control payload size fits in usize")
}

fn form(rng: &mut Rng, length: usize) -> LengthForm {
    if !rng.chance(WIDE_LENGTH_PPM) {
        return LengthForm::Minimal;
    }
    if length <= LENGTH_16_MAX && rng.chance(PPM / 2) {
        LengthForm::Wide16
    } else {
        LengthForm::Wide64
    }
}

fn cuts(rng: &mut Rng, length: usize, pieces: usize) -> Vec<usize> {
    let bound = u64::try_from(length).expect("a length fits in u64");
    let mut cuts: Vec<usize> = (1..pieces)
        .map(|_| usize::try_from(rng.range(0, bound)).expect("a cut fits in usize"))
        .collect();
    cuts.sort_unstable();
    cuts.push(length);
    cuts
}

struct Builder<'r> {
    rng: &'r mut Rng,
    frames: Vec<ServerFrame>,
    data_messages: usize,
    corrupt: Option<(usize, usize)>,
}

impl Builder<'_> {
    fn push(&mut self, mut frame: ServerFrame) {
        if frame.form == LengthForm::Minimal {
            frame.form = form(self.rng, frame.payload.len());
        }
        self.frames.push(frame);
    }

    fn control(&mut self) {
        let opcode = if self.rng.chance(PPM / 2) {
            OPCODE_PING
        } else {
            OPCODE_PONG
        };
        let bytes = control_size(self.rng);
        let payload = binary(self.rng, bytes);
        self.push(ServerFrame::new(opcode, true, payload));
    }

    fn message(&mut self, opcode: u8, payload: &[u8]) {
        let pieces = if self.rng.chance(FRAGMENTED_PPM) {
            usize::try_from(self.rng.range(2, FRAGMENTS_MAX)).expect("fits")
        } else {
            1
        };
        let mut from = 0;
        let ends = cuts(self.rng, payload.len(), pieces);
        let last = ends.len() - 1;
        for (index, end) in ends.into_iter().enumerate() {
            let code = if index == 0 { opcode } else { OPCODE_CONTINUE };
            if index > 0 && self.rng.chance(CONTROL_BETWEEN_PPM) {
                self.control();
            }
            if self.corrupt.is_none() && end > from {
                self.corrupt = Some((self.frames.len(), end - from - 1));
            }
            self.push(ServerFrame::new(
                code,
                index == last,
                payload[from..end].to_vec(),
            ));
            from = end;
        }
        self.data_messages += 1;
    }

    fn data(&mut self) {
        let bytes = size(self.rng);
        if self.rng.chance(PPM / 2) {
            let payload = text(self.rng, bytes);
            self.message(OPCODE_TEXT, &payload);
        } else {
            let payload = binary(self.rng, bytes);
            self.message(OPCODE_BINARY, &payload);
        }
    }

    fn violation(&mut self, violation: Violation) {
        let rng = &mut *self.rng;
        let small = usize::try_from(rng.range(0, SMALL_BYTES_MAX)).expect("fits");
        match violation {
            Violation::Reserved => {
                let bits = [RSV1_BIT, RSV2_BIT, RSV3_BIT];
                let reserved = bits[rng.pick(bits.len())];
                let opcode = [OPCODE_TEXT, OPCODE_BINARY, OPCODE_PING][rng.pick(3)];
                let mut frame = ServerFrame::new(
                    opcode,
                    true,
                    text(rng, small.min(CONTROL_PAYLOAD_BYTES_MAX)),
                );
                frame.reserved = reserved;
                self.frames.push(frame);
            }
            Violation::Opcode => {
                let opcode = UNKNOWN_OPCODES[rng.pick(UNKNOWN_OPCODES.len())];
                let payload = binary(rng, small);
                self.frames.push(ServerFrame::new(opcode, true, payload));
            }
            Violation::Masked => {
                let mut frame = ServerFrame::new(OPCODE_BINARY, true, binary(rng, small));
                let mut key = [0; MASK_BYTES];
                key.copy_from_slice(&rng.draw().to_le_bytes()[..MASK_BYTES]);
                frame.mask = Some(key);
                self.frames.push(frame);
            }
            Violation::ControlTooBig => {
                let opcode = [OPCODE_PING, OPCODE_PONG, OPCODE_CLOSE][rng.pick(3)];
                let low = u64::try_from(CONTROL_PAYLOAD_BYTES_MAX + 1).expect("fits");
                let bytes =
                    usize::try_from(rng.range(low, OVERSIZED_CONTROL_BYTES_MAX)).expect("fits");
                let mut payload = close_payload(NORMAL_CLOSE, &[]);
                payload.extend(text(rng, bytes - payload.len()));
                self.frames.push(ServerFrame::new(opcode, true, payload));
            }
            Violation::ControlFragmented => {
                let opcode = [OPCODE_PING, OPCODE_PONG, OPCODE_CLOSE][rng.pick(3)];
                self.frames
                    .push(ServerFrame::new(opcode, false, Vec::new()));
            }
            Violation::BadUtf8 | Violation::TruncatedUtf8 => {
                let mut payload = text(rng, small);
                let at = usize::try_from(rng.range(0, u64::try_from(payload.len()).expect("fits")))
                    .expect("fits");
                let at = (0..=at)
                    .rev()
                    .find(|at| std::str::from_utf8(&payload[..*at]).is_ok())
                    .unwrap_or(0);
                if violation == Violation::BadUtf8 {
                    let bad = INVALID_UTF8[rng.pick(INVALID_UTF8.len())];
                    payload.splice(at..at, bad.iter().copied());
                } else {
                    payload.extend_from_slice(TRUNCATED_UTF8[rng.pick(TRUNCATED_UTF8.len())]);
                }
                self.message(OPCODE_TEXT, &payload);
                self.data_messages -= 1;
            }
            Violation::CloseOneByte => {
                self.frames
                    .push(ServerFrame::new(OPCODE_CLOSE, true, vec![0x03]));
            }
            Violation::CloseBadReason => {
                let bad = INVALID_UTF8[rng.pick(INVALID_UTF8.len())];
                let payload = close_payload(NORMAL_CLOSE, bad);
                self.frames
                    .push(ServerFrame::new(OPCODE_CLOSE, true, payload));
            }
            Violation::HighBitLength => {
                let mut frame = ServerFrame::new(OPCODE_BINARY, true, binary(rng, small));
                frame.form = LengthForm::HighBit;
                self.frames.push(frame);
            }
            Violation::ContinueWithoutStart => {
                let payload = binary(rng, small);
                self.frames.push(ServerFrame::new(
                    OPCODE_CONTINUE,
                    rng.chance(PPM / 2),
                    payload,
                ));
            }
            Violation::NewMessageMidFragment => {
                let first = binary(rng, small);
                let second = binary(rng, small);
                self.frames
                    .push(ServerFrame::new(OPCODE_TEXT, false, Vec::new()));
                self.frames
                    .push(ServerFrame::new(OPCODE_BINARY, false, first));
                self.frames
                    .push(ServerFrame::new(OPCODE_CONTINUE, true, second));
            }
            Violation::AfterClose => {
                self.frames.push(ServerFrame::new(
                    OPCODE_CLOSE,
                    true,
                    close_payload(NORMAL_CLOSE, b"bye"),
                ));
                let payload = binary(rng, small);
                self.frames
                    .push(ServerFrame::new(OPCODE_BINARY, true, payload));
            }
        }
    }
}

fn config(rng: &mut Rng) -> WebSocketConfig {
    let read = READ_BUFFER_CHOICES[rng.pick(READ_BUFFER_CHOICES.len())];
    let config = WebSocketConfig::default().read_buffer_bytes(read);
    if !rng.chance(SMALL_LIMITS_PPM) {
        return config;
    }
    let frame = usize::try_from(rng.range(0, FRAME_LIMIT_BYTES_MAX)).expect("fits");
    let message = usize::try_from(rng.range(0, MESSAGE_LIMIT_BYTES_MAX)).expect("fits");
    config.frame_bytes_max(frame).message_bytes_max(message)
}

fn close_reason(rng: &mut Rng) -> String {
    let bound = u64::try_from(CLOSE_REASON_BYTES_MAX).expect("fits");
    let bytes = usize::try_from(rng.range(0, bound)).expect("fits");
    String::from_utf8(text(rng, bytes)).expect("generated text is UTF-8")
}

fn server_close(rng: &mut Rng) -> Vec<u8> {
    if rng.chance(EMPTY_CLOSE_PPM) {
        return Vec::new();
    }
    let code = if rng.chance(FORBIDDEN_CODE_PPM) {
        FORBIDDEN_CODES[rng.pick(FORBIDDEN_CODES.len())]
    } else {
        ALLOWED_CODES[rng.pick(ALLOWED_CODES.len())]
    };
    close_payload(code, close_reason(rng).as_bytes())
}

fn client_plan(
    rng: &mut Rng,
    data_messages: usize,
    close: Option<usize>,
    frames: usize,
) -> ClientPlan {
    let latest = close.unwrap_or(data_messages);
    let latest_bound = u64::try_from(latest).expect("fits");
    let mut sends: Vec<ClientSend> = (0..rng.range(0, CLIENT_SENDS_MAX))
        .map(|_| {
            let after = usize::try_from(rng.range(0, latest_bound)).expect("fits");
            let (kind, payload) = if rng.chance(CLIENT_PING_PPM) {
                let bytes = control_size(rng);
                (SendKind::Ping, binary(rng, bytes))
            } else if rng.chance(CLIENT_TEXT_PPM) {
                let bytes = size(rng);
                (SendKind::Text, text(rng, bytes))
            } else {
                let bytes = size(rng);
                (SendKind::Binary, binary(rng, bytes))
            };
            ClientSend {
                after,
                kind,
                payload,
            }
        })
        .collect();
    sends.sort_by_key(|send| send.after);
    let close = close.map(|after| {
        let frame = rng.chance(CLIENT_CLOSE_FRAME_PPM).then(|| {
            let code = ALLOWED_CODES[rng.pick(ALLOWED_CODES.len())];
            (code, close_reason(rng))
        });
        (after, frame)
    });
    let client_bytes = REQUEST_HEAD_BYTES_MAX
        + sends
            .iter()
            .map(|send| send.payload.len() + CLIENT_FRAME_OVERHEAD_BYTES)
            .sum::<usize>()
        + (frames + 1) * (CONTROL_PAYLOAD_BYTES_MAX + CLIENT_FRAME_OVERHEAD_BYTES);
    let floor = u64::try_from(client_bytes.div_ceil(PIECES_MAX)).expect("fits");
    let capacity_bytes = if rng.chance(UNBOUNDED_CAPACITY_PPM) {
        None
    } else if rng.chance(TINY_CAPACITY_PPM) {
        Some(rng.range(floor, floor.max(TINY_CAPACITY_BYTES_MAX)))
    } else {
        Some(rng.range(floor, floor.max(CLIENT_CAPACITY_BYTES_MAX)))
    }
    .map(|bytes| usize::try_from(bytes).expect("fits"));
    let read_max = [
        TINY_PIECE_BYTES_MAX,
        SMALL_PIECE_BYTES_MAX,
        LARGE_PIECE_BYTES_MAX,
    ][rng.pick(3)];
    let read_bytes_max = usize::try_from(rng.range(floor, floor.max(read_max))).expect("fits");
    ClientPlan {
        sends,
        close,
        vectored: rng.chance(PPM / 2),
        capacity_bytes,
        read_bytes_min: usize::try_from(floor).expect("fits"),
        read_bytes_max,
        gap_ms_max: gap(rng),
    }
}

fn gap(rng: &mut Rng) -> u64 {
    if rng.chance(NO_GAP_PPM) {
        0
    } else if rng.chance(BURSTY_GAP_PPM) {
        rng.range(1, BURSTY_GAP_MS_MAX)
    } else {
        rng.range(BURSTY_GAP_MS_MAX, GAP_MS_MAX)
    }
}

impl Script {
    pub fn of(seed: u64) -> Self {
        let mut rng = Rng::derive(seed, "websocket-script");
        let config = config(&mut rng);
        let head = HeadPlan::of(&mut rng);
        let head_len = head.render(PLACEHOLDER_ACCEPT).len();
        let messages = rng.range(0, MESSAGES_MAX);
        let violation = rng
            .chance(VIOLATION_PPM)
            .then(|| VIOLATIONS[rng.pick(VIOLATIONS.len())]);
        let violation_at = rng.range(0, messages);
        let mut builder = Builder {
            rng: &mut rng,
            frames: Vec::new(),
            data_messages: 0,
            corrupt: None,
        };
        for index in 0..=messages {
            if index == violation_at
                && let Some(violation) = violation
            {
                builder.violation(violation);
                break;
            }
            if index == messages {
                break;
            }
            if builder.rng.chance(PING_BETWEEN_PPM) {
                builder.control();
            }
            builder.data();
        }
        let Builder {
            mut frames,
            data_messages,
            corrupt,
            ..
        } = builder;
        let ending = if violation.is_some() {
            Ending::Finish
        } else {
            match rng.below(ENDING_ROLLS) {
                0..SERVER_CLOSE_ROLLS_END => Ending::ServerClose,
                SERVER_CLOSE_ROLLS_END..CLIENT_CLOSE_ROLLS_END => Ending::ClientClose,
                CLIENT_CLOSE_ROLLS_END..CUT_ROLLS_END if head.kind.allows_cut() => Ending::Cut {
                    at: 0,
                    kind: if rng.chance(PPM / 2) {
                        Cut::Eof
                    } else {
                        Cut::Reset
                    },
                },
                _ => Ending::Finish,
            }
        };
        if ending == Ending::ServerClose {
            frames.push(ServerFrame::new(OPCODE_CLOSE, true, server_close(&mut rng)));
        }
        let mut frame_bytes = Vec::new();
        let mut corrupt_at = None;
        for (index, frame) in frames.iter().enumerate() {
            if let Some((corrupt_frame, offset)) = corrupt
                && corrupt_frame == index
            {
                corrupt_at = Some(frame_bytes.len() + frame.header_bytes() + offset);
            }
            frame.encode_into(&mut frame_bytes);
        }
        let ending = match ending {
            Ending::Cut { kind, .. } => {
                let total = u64::try_from(head_len + frame_bytes.len()).expect("fits");
                let at = usize::try_from(rng.range(0, total - 1)).expect("fits");
                Ending::Cut { at, kind }
            }
            other => other,
        };
        let close = (ending == Ending::ClientClose).then(|| {
            usize::try_from(rng.range(0, u64::try_from(data_messages).expect("fits")))
                .expect("fits")
        });
        let client = client_plan(&mut rng, data_messages, close, frames.len());
        let pieces = [PieceStyle::Tiny, PieceStyle::Small, PieceStyle::Large][rng.pick(3)];
        Self {
            config,
            head,
            head_len,
            frames,
            frame_bytes,
            ending,
            violation,
            data_messages,
            corrupt_at,
            client,
            pieces,
            gap_ms_max: gap(&mut rng),
        }
    }

    pub fn pieces(&self, rng: &mut Rng, limit: usize) -> Vec<(u64, usize)> {
        let head_floor = self.head_len.div_ceil(HEAD_PIECES_MAX);
        let floor = limit.div_ceil(PIECES_MAX).max(1);
        let style_max = match self.pieces {
            PieceStyle::Tiny => TINY_PIECE_BYTES_MAX,
            PieceStyle::Small => SMALL_PIECE_BYTES_MAX,
            PieceStyle::Large => LARGE_PIECE_BYTES_MAX,
        };
        let mut pieces = Vec::new();
        let mut offset = 0;
        while offset < limit {
            let low = if offset < self.head_len {
                floor.max(head_floor)
            } else {
                floor
            };
            let low = u64::try_from(low).expect("fits");
            let bytes = usize::try_from(rng.range(low, low.max(style_max))).expect("fits");
            let end = (offset + bytes).min(limit);
            pieces.push((rng.range(0, self.gap_ms_max), end));
            offset = end;
        }
        assert!(
            pieces.len() <= PIECES_MAX + HEAD_PIECES_MAX,
            "the pieces stay under their bound"
        );
        pieces
    }

    pub fn describe(&self) -> String {
        format!(
            "scenario=websocket head={:?} head_bytes={} frames={} frame_bytes={} data_messages={} violation={:?} ending={:?} frame_max={} message_max={} read_buffer={} pieces={:?} gap_ms<={} sends={} close={:?} capacity={:?} vectored={}",
            self.head.kind,
            self.head_len,
            self.frames.len(),
            self.frame_bytes.len(),
            self.data_messages,
            self.violation,
            self.ending,
            self.config.frame_bytes_max,
            self.config.message_bytes_max,
            self.config.read_buffer_bytes,
            self.pieces,
            self.gap_ms_max,
            self.client.sends.len(),
            self.client
                .close
                .as_ref()
                .map(|(after, frame)| (after, frame.as_ref().map(|(code, _)| *code))),
            self.client.capacity_bytes,
            self.client.vectored
        )
    }
}
