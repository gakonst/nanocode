use nanocode_websocket::{CapacityError, Error, OpCode, ProtocolError, SubProtocolError};

use super::{
    script::{Cut, Ending, Head, Script},
    wire::{
        CLOSE_CODE_BYTES, CONTROL_PAYLOAD_BYTES_MAX, Inflated, OPCODE_BINARY, OPCODE_CLOSE,
        OPCODE_CONTINUE, OPCODE_PING, OPCODE_PONG, OPCODE_TEXT, RSV1_BIT, ServerFrame,
        close_payload, is_control, is_known,
    },
};

const PROTOCOL_VIOLATION_CODE: u16 = 1002;
const PROTOCOL_VIOLATION_REASON: &[u8] = b"Protocol violation";
pub const UTF8: &str = "utf8";
pub const ATTACK: &str = "attack";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Seen {
    Text(Vec<u8>),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Close(Option<(u16, Vec<u8>)>),
    Failed(String),
    End,
    Unexpected(String),
}

pub fn failure_name(error: &Error) -> String {
    match error {
        Error::Protocol(error) => format!("protocol {error:?}"),
        Error::Capacity(error) => format!("capacity {error:?}"),
        Error::Utf8(_) => UTF8.to_owned(),
        Error::Io(error) => format!("io {:?}", error.kind()),
        Error::Http(response) => format!("http {}", response.status().as_u16()),
        Error::AttackAttempt => ATTACK.to_owned(),
        other => format!("other {other}"),
    }
}

pub fn protocol(error: ProtocolError) -> String {
    failure_name(&Error::Protocol(error))
}

fn capacity(error: CapacityError) -> String {
    failure_name(&Error::Capacity(error))
}

fn too_long(size: u64, max_size: usize) -> String {
    capacity(CapacityError::MessageTooLong { size, max_size })
}

pub fn reset() -> String {
    failure_name(&Error::Io(std::io::ErrorKind::ConnectionReset.into()))
}

fn invalid_data() -> String {
    failure_name(&Error::Io(std::io::ErrorKind::InvalidData.into()))
}

const fn close_code_allowed(code: u16) -> bool {
    matches!(code, 1000..=1003 | 1007..=1013 | 3000..=4999)
}

fn definitely_invalid(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_err_and(|error| error.error_len().is_some())
}

fn count(bytes: usize) -> u64 {
    u64::try_from(bytes).expect("a byte count fits in u64")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Active,
    ClosedByUs,
    ClosedByPeer,
    Acknowledged,
}

pub struct Expected {
    pub seen: Vec<Seen>,
    pub pongs: Vec<Vec<u8>>,
    pub closes: Vec<Vec<u8>>,
    pub reset: bool,
}

struct Model {
    frame_max: usize,
    message_max: usize,
    negotiated: bool,
    phase: Phase,
    incomplete: Option<(u8, bool, Vec<u8>)>,
    data: usize,
    close: Option<(usize, Vec<u8>)>,
    expected: Expected,
}

impl Model {
    fn header_failure(&self, frame: &ServerFrame) -> Option<String> {
        if !is_known(frame.opcode) {
            return Some(protocol(ProtocolError::InvalidOpcode(frame.opcode)));
        }
        let declared = frame.declared_length();
        if usize::try_from(declared).is_ok_and(|length| length <= self.frame_max) {
            return None;
        }
        Some(too_long(declared, self.frame_max))
    }

    fn client_closes_now(&mut self) {
        if let Some((after, payload)) = &self.close
            && *after == self.data
            && self.phase == Phase::Active
        {
            self.phase = Phase::ClosedByUs;
            self.expected.closes.push(payload.clone());
        }
    }

    fn deliver(&mut self, opcode: u8, payload: Vec<u8>) {
        self.expected.seen.push(if opcode == OPCODE_TEXT {
            Seen::Text(payload)
        } else {
            Seen::Binary(payload)
        });
        self.data += 1;
        self.client_closes_now();
    }

    fn step(&mut self, frame: &ServerFrame) -> Result<(), String> {
        if matches!(self.phase, Phase::ClosedByPeer | Phase::Acknowledged) {
            return Err(protocol(ProtocolError::ReceivedAfterClosing));
        }
        let compressed = frame.reserved & RSV1_BIT != 0;
        if frame.reserved & !RSV1_BIT != 0
            || (compressed && (!self.negotiated || is_control(frame.opcode)))
        {
            return Err(protocol(ProtocolError::NonZeroReservedBits));
        }
        if compressed && frame.opcode == OPCODE_CONTINUE {
            return match self.incomplete {
                Some((_, true, _)) => Err(invalid_data()),
                Some((_, false, _)) | None => Err(protocol(ProtocolError::NonZeroReservedBits)),
            };
        }
        if frame.mask.is_some() {
            return Err(protocol(ProtocolError::MaskedFrameFromServer));
        }
        if is_control(frame.opcode) {
            return self.control(frame);
        }
        self.data_frame(frame)
    }

    fn control(&mut self, frame: &ServerFrame) -> Result<(), String> {
        if !frame.fin {
            return Err(protocol(ProtocolError::FragmentedControlFrame));
        }
        if frame.payload.len() > CONTROL_PAYLOAD_BYTES_MAX {
            return Err(protocol(ProtocolError::ControlFrameTooBig));
        }
        let payload = frame.payload.clone();
        match frame.opcode {
            OPCODE_PING => {
                if self.phase == Phase::Active {
                    self.expected.pongs.push(payload.clone());
                }
                self.expected.seen.push(Seen::Ping(payload));
            }
            OPCODE_PONG => self.expected.seen.push(Seen::Pong(payload)),
            OPCODE_CLOSE => self.close_frame(&payload)?,
            other => unreachable!("opcode {other:#x} is not a control opcode"),
        }
        Ok(())
    }

    fn close_frame(&mut self, payload: &[u8]) -> Result<(), String> {
        let parsed = match payload.len() {
            0 => None,
            1 => return Err(protocol(ProtocolError::InvalidCloseSequence)),
            _ => {
                let reason = &payload[CLOSE_CODE_BYTES..];
                if std::str::from_utf8(reason).is_err() {
                    return Err(UTF8.to_owned());
                }
                Some((
                    u16::from_be_bytes([payload[0], payload[1]]),
                    reason.to_vec(),
                ))
            }
        };
        match self.phase {
            Phase::Active => {
                self.phase = Phase::ClosedByPeer;
                let answered = parsed.map(|(code, reason)| {
                    if close_code_allowed(code) {
                        (code, reason)
                    } else {
                        (PROTOCOL_VIOLATION_CODE, PROTOCOL_VIOLATION_REASON.to_vec())
                    }
                });
                self.expected.closes.push(
                    answered
                        .as_ref()
                        .map_or_else(Vec::new, |(code, reason)| close_payload(*code, reason)),
                );
                self.expected.seen.push(Seen::Close(answered));
            }
            Phase::ClosedByUs => {
                self.phase = Phase::Acknowledged;
                self.expected.seen.push(Seen::Close(parsed));
            }
            Phase::ClosedByPeer | Phase::Acknowledged => {
                unreachable!("a frame after the close failed above")
            }
        }
        Ok(())
    }

    fn inflated(&mut self, opcode: u8, inflated: &Inflated) -> Result<(), String> {
        match inflated {
            Inflated::To(bytes) if bytes.len() > self.message_max => Err(invalid_data()),
            Inflated::To(bytes) => {
                if opcode == OPCODE_TEXT && std::str::from_utf8(bytes).is_err() {
                    return Err(UTF8.to_owned());
                }
                self.deliver(opcode, bytes.clone());
                Ok(())
            }
            Inflated::Over(_) | Inflated::Corrupt => Err(invalid_data()),
            Inflated::Plain => unreachable!("a compressed message ends on a frame that knows its bytes"),
        }
    }

    fn data_frame(&mut self, frame: &ServerFrame) -> Result<(), String> {
        let payload = &frame.payload;
        let max = self.message_max;
        let compressed = frame.reserved & RSV1_BIT != 0;
        match (frame.opcode, self.incomplete.take()) {
            (OPCODE_CONTINUE, None) => Err(protocol(ProtocolError::UnexpectedContinueFrame)),
            (OPCODE_CONTINUE, Some((opcode, true, mut collected))) => {
                collected.extend_from_slice(payload);
                if frame.fin {
                    return self.inflated(opcode, &frame.inflated);
                }
                self.incomplete = Some((opcode, true, collected));
                Ok(())
            }
            (_, Some((_, true, _))) => Err(invalid_data()),
            (opcode, None) if compressed => {
                if frame.fin {
                    return self.inflated(opcode, &frame.inflated);
                }
                self.incomplete = Some((opcode, true, payload.clone()));
                Ok(())
            }
            (OPCODE_CONTINUE, Some((opcode, false, mut collected))) => {
                if collected.len() > max || payload.len() > max - collected.len() {
                    return Err(too_long(count(collected.len()) + count(payload.len()), max));
                }
                collected.extend_from_slice(payload);
                if opcode == OPCODE_TEXT && definitely_invalid(&collected) {
                    return Err(UTF8.to_owned());
                }
                if !frame.fin {
                    self.incomplete = Some((opcode, false, collected));
                    return Ok(());
                }
                if opcode == OPCODE_TEXT && std::str::from_utf8(&collected).is_err() {
                    return Err(UTF8.to_owned());
                }
                self.deliver(opcode, collected);
                Ok(())
            }
            (opcode, Some(_)) => {
                let opcode = if opcode == OPCODE_TEXT {
                    OpCode::Text
                } else {
                    OpCode::Binary
                };
                Err(protocol(ProtocolError::ExpectedFragment(opcode)))
            }
            (opcode, None) if frame.fin => {
                if payload.len() > max {
                    return Err(too_long(count(payload.len()), max));
                }
                if opcode == OPCODE_TEXT && std::str::from_utf8(payload).is_err() {
                    return Err(UTF8.to_owned());
                }
                self.deliver(opcode, payload.clone());
                Ok(())
            }
            (opcode, None) => {
                assert!(opcode == OPCODE_TEXT || opcode == OPCODE_BINARY);
                if payload.len() > max {
                    return Err(too_long(count(payload.len()), max));
                }
                if opcode == OPCODE_TEXT && definitely_invalid(payload) {
                    return Err(UTF8.to_owned());
                }
                self.incomplete = Some((opcode, false, payload.clone()));
                Ok(())
            }
        }
    }

    fn fail(mut self, failure: String) -> Expected {
        self.expected.seen.push(Seen::Failed(failure));
        self.expected.seen.push(Seen::End);
        self.expected
    }

    fn end(mut self) -> Expected {
        match self.phase {
            Phase::ClosedByPeer | Phase::Acknowledged => {
                self.expected.seen.push(Seen::End);
                self.expected
            }
            Phase::Active | Phase::ClosedByUs => {
                self.fail(protocol(ProtocolError::ResetWithoutClosingHandshake))
            }
        }
    }
}

fn head_failure(head: Head) -> Option<String> {
    let failure = match head {
        Head::Valid => return None,
        Head::WrongAccept | Head::MissingAccept => {
            protocol(ProtocolError::SecWebSocketAcceptKeyMismatch)
        }
        Head::MissingUpgrade => protocol(ProtocolError::MissingUpgradeWebSocketHeader),
        Head::MissingConnection => protocol(ProtocolError::MissingConnectionUpgradeHeader),
        Head::Status(code) => format!("http {code}"),
        Head::OldVersion => protocol(ProtocolError::WrongHttpVersion),
        Head::SubProtocol => protocol(ProtocolError::SecWebSocketSubProtocolError(
            SubProtocolError::ServerSentSubProtocolNoneRequested,
        )),
        Head::TooManyHeaders => capacity(CapacityError::TooManyHeaders),
        Head::Oversized => ATTACK.to_owned(),
        Head::BadStatus => "protocol HttparseError(Status)".to_owned(),
        Head::BadVersion => "protocol HttparseError(Version)".to_owned(),
    };
    Some(failure)
}

fn cut_failure(kind: Cut, in_head: bool) -> String {
    match (kind, in_head) {
        (Cut::Reset, _) => reset(),
        (Cut::Eof, true) => protocol(ProtocolError::HandshakeIncomplete),
        (Cut::Eof, false) => protocol(ProtocolError::ResetWithoutClosingHandshake),
    }
}

pub fn expect(script: &Script) -> Expected {
    let cut = match script.ending {
        Ending::Cut { at, kind } => Some((at, kind)),
        Ending::ServerClose | Ending::ClientClose | Ending::Finish => None,
    };
    let mut model = Model {
        frame_max: script.config.frame_bytes_max,
        message_max: script.config.message_bytes_max,
        negotiated: script.deflate.is_some_and(|deflate| deflate.negotiated),
        phase: Phase::Active,
        incomplete: None,
        data: 0,
        close: script.client.close.as_ref().map(|(after, frame)| {
            let payload = frame.as_ref().map_or_else(Vec::new, |(code, reason)| {
                close_payload(*code, reason.as_bytes())
            });
            (*after, payload)
        }),
        expected: Expected {
            seen: Vec::new(),
            pongs: Vec::new(),
            closes: Vec::new(),
            reset: matches!(cut, Some((at, Cut::Reset)) if at >= script.head_len),
        },
    };
    if let Some((at, kind)) = cut
        && at < script.head_len
    {
        model
            .expected
            .seen
            .push(Seen::Failed(cut_failure(kind, true)));
        return model.expected;
    }
    if let Some(failure) = head_failure(script.head.kind) {
        model.expected.seen.push(Seen::Failed(failure));
        return model.expected;
    }
    model.client_closes_now();
    let mut offset = script.head_len;
    for frame in &script.frames {
        let header_end = offset + frame.header_bytes();
        let end = header_end + frame.payload.len();
        if let Some((at, kind)) = cut
            && at <= offset
        {
            return model.fail(cut_failure(kind, false));
        }
        if let Some(failure) = model.header_failure(frame) {
            return match cut {
                Some((at, kind)) if at < header_end => model.fail(cut_failure(kind, false)),
                _ => model.fail(failure),
            };
        }
        if let Some((at, kind)) = cut
            && at < end
        {
            return model.fail(cut_failure(kind, false));
        }
        if let Err(failure) = model.step(frame) {
            return model.fail(failure);
        }
        offset = end;
    }
    assert!(cut.is_none(), "a cut falls inside the server's bytes");
    if script.ending == Ending::ClientClose {
        let echo = model
            .close
            .as_ref()
            .map_or_else(Vec::new, |(_, payload)| payload.clone());
        if let Err(failure) = model.step(&ServerFrame::new(OPCODE_CLOSE, true, echo)) {
            return model.fail(failure);
        }
    }
    model.end()
}
