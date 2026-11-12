use std::fmt;

use crate::{
    error::{Error, ProtocolError, Result},
    message::Utf8Bytes,
};

pub(crate) const HEADER_BYTES_MAX: usize = 14;
pub(crate) const CONTROL_PAYLOAD_BYTES_MAX: usize = 125;
const FINAL_BIT: u8 = 0x80;
const RSV1_BIT: u8 = 0x40;
const RSV2_BIT: u8 = 0x20;
const RSV3_BIT: u8 = 0x10;
const OPCODE_BITS: u8 = 0x0f;
const MASK_BIT: u8 = 0x80;
const LENGTH_BITS: u8 = 0x7f;
const LENGTH_16_MARKER: u8 = 126;
const LENGTH_64_MARKER: u8 = 127;
const LENGTH_16_MIN: u64 = 126;
const LENGTH_64_MIN: u64 = 1 << 16;
const BASE_HEADER_BYTES: usize = 2;
const LENGTH_16_BYTES: usize = 2;
const LENGTH_64_BYTES: usize = 8;
const MASK_BYTES: usize = 4;
const CLOSE_CODE_BYTES: usize = 2;
const OPCODE_CONTINUE: u8 = 0x0;
const OPCODE_TEXT: u8 = 0x1;
const OPCODE_BINARY: u8 = 0x2;
const OPCODE_CLOSE: u8 = 0x8;
const OPCODE_PING: u8 = 0x9;
const OPCODE_PONG: u8 = 0xa;
const MASK_WORD_BYTES: usize = 8;
const MASK_POOL_BYTES: usize = 64 * MASK_BYTES;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum OpCode {
    Continue,
    Text,
    Binary,
    Close,
    Ping,
    Pong,
}

impl OpCode {
    const fn parse(bits: u8) -> Result<Self> {
        match bits {
            OPCODE_CONTINUE => Ok(Self::Continue),
            OPCODE_TEXT => Ok(Self::Text),
            OPCODE_BINARY => Ok(Self::Binary),
            OPCODE_CLOSE => Ok(Self::Close),
            OPCODE_PING => Ok(Self::Ping),
            OPCODE_PONG => Ok(Self::Pong),
            other => Err(Error::Protocol(ProtocolError::InvalidOpcode(other))),
        }
    }

    const fn bits(self) -> u8 {
        match self {
            Self::Continue => OPCODE_CONTINUE,
            Self::Text => OPCODE_TEXT,
            Self::Binary => OPCODE_BINARY,
            Self::Close => OPCODE_CLOSE,
            Self::Ping => OPCODE_PING,
            Self::Pong => OPCODE_PONG,
        }
    }

    #[must_use]
    pub const fn is_control(self) -> bool {
        matches!(self, Self::Close | Self::Ping | Self::Pong)
    }
}

impl fmt::Display for OpCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Continue => "CONTINUE",
            Self::Text => "TEXT",
            Self::Binary => "BINARY",
            Self::Close => "CLOSE",
            Self::Ping => "PING",
            Self::Pong => "PONG",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    pub is_final: bool,
    pub rsv1: bool,
    pub rsv2: bool,
    pub rsv3: bool,
    pub opcode: OpCode,
    pub mask: Option<[u8; MASK_BYTES]>,
}

impl FrameHeader {
    const fn new(opcode: OpCode, is_final: bool) -> Self {
        Self {
            is_final,
            rsv1: false,
            rsv2: false,
            rsv3: false,
            opcode,
            mask: None,
        }
    }

    pub(crate) fn parse(bytes: &[u8]) -> Result<Option<(Self, u64, usize)>> {
        let [first, second, ..] = *bytes else {
            return Ok(None);
        };
        let (length, mut consumed) = match second & LENGTH_BITS {
            LENGTH_16_MARKER => {
                let Some(extended) =
                    bytes.get(BASE_HEADER_BYTES..BASE_HEADER_BYTES + LENGTH_16_BYTES)
                else {
                    return Ok(None);
                };
                let mut length = [0; LENGTH_16_BYTES];
                length.copy_from_slice(extended);
                (
                    u64::from(u16::from_be_bytes(length)),
                    BASE_HEADER_BYTES + LENGTH_16_BYTES,
                )
            }
            LENGTH_64_MARKER => {
                let Some(extended) =
                    bytes.get(BASE_HEADER_BYTES..BASE_HEADER_BYTES + LENGTH_64_BYTES)
                else {
                    return Ok(None);
                };
                let mut length = [0; LENGTH_64_BYTES];
                length.copy_from_slice(extended);
                (
                    u64::from_be_bytes(length),
                    BASE_HEADER_BYTES + LENGTH_64_BYTES,
                )
            }
            length => (u64::from(length), BASE_HEADER_BYTES),
        };
        let mask = if second & MASK_BIT == 0 {
            None
        } else {
            let Some(key) = bytes.get(consumed..consumed + MASK_BYTES) else {
                return Ok(None);
            };
            consumed += MASK_BYTES;
            let mut mask = [0; MASK_BYTES];
            mask.copy_from_slice(key);
            Some(mask)
        };
        let header = Self {
            is_final: first & FINAL_BIT != 0,
            rsv1: first & RSV1_BIT != 0,
            rsv2: first & RSV2_BIT != 0,
            rsv3: first & RSV3_BIT != 0,
            opcode: OpCode::parse(first & OPCODE_BITS)?,
            mask,
        };
        assert!(consumed <= HEADER_BYTES_MAX);
        Ok(Some((header, length, consumed)))
    }

    fn format(&self, length: u64, output: &mut Vec<u8>) {
        let flag = |set: bool, bit: u8| if set { bit } else { 0 };
        let first = self.opcode.bits()
            | flag(self.is_final, FINAL_BIT)
            | flag(self.rsv1, RSV1_BIT)
            | flag(self.rsv2, RSV2_BIT)
            | flag(self.rsv3, RSV3_BIT);
        let mask = flag(self.mask.is_some(), MASK_BIT);
        if let Ok(short) = u8::try_from(length)
            && length < LENGTH_16_MIN
        {
            output.extend_from_slice(&[first, short | mask]);
        } else if let Ok(medium) = u16::try_from(length)
            && length < LENGTH_64_MIN
        {
            output.extend_from_slice(&[first, LENGTH_16_MARKER | mask]);
            output.extend_from_slice(&medium.to_be_bytes());
        } else {
            output.extend_from_slice(&[first, LENGTH_64_MARKER | mask]);
            output.extend_from_slice(&length.to_be_bytes());
        }
        if let Some(key) = self.mask {
            output.extend_from_slice(&key);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    header: FrameHeader,
    payload: Vec<u8>,
}

impl Frame {
    #[must_use]
    pub fn message(payload: Vec<u8>, opcode: OpCode, is_final: bool) -> Self {
        assert!(!opcode.is_control());
        Self {
            header: FrameHeader::new(opcode, is_final),
            payload,
        }
    }

    pub(crate) const fn ping(payload: Vec<u8>) -> Self {
        Self {
            header: FrameHeader::new(OpCode::Ping, true),
            payload,
        }
    }

    pub(crate) const fn pong(payload: Vec<u8>) -> Self {
        Self {
            header: FrameHeader::new(OpCode::Pong, true),
            payload,
        }
    }

    pub(crate) fn close(frame: Option<CloseFrame>) -> Self {
        let payload = frame.map_or_else(Vec::new, |CloseFrame { code, reason }| {
            let mut payload = Vec::with_capacity(CLOSE_CODE_BYTES + reason.len());
            payload.extend_from_slice(&u16::from(code).to_be_bytes());
            payload.extend_from_slice(reason.as_bytes());
            payload
        });
        Self {
            header: FrameHeader::new(OpCode::Close, true),
            payload,
        }
    }

    pub(crate) const fn from_payload(header: FrameHeader, payload: Vec<u8>) -> Self {
        Self { header, payload }
    }

    #[must_use]
    pub const fn header(&self) -> &FrameHeader {
        &self.header
    }

    pub const fn header_mut(&mut self) -> &mut FrameHeader {
        &mut self.header
    }

    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    pub(crate) fn into_parts(self) -> (FrameHeader, Vec<u8>) {
        (self.header, self.payload)
    }

    #[must_use]
    pub fn into_payload(self) -> Vec<u8> {
        self.payload
    }

    pub(crate) fn into_close(self) -> Result<Option<CloseFrame>> {
        match self.payload.len() {
            0 => Ok(None),
            1 => Err(Error::Protocol(ProtocolError::InvalidCloseSequence)),
            _ => {
                let code = u16::from_be_bytes([self.payload[0], self.payload[1]]).into();
                let reason = std::str::from_utf8(&self.payload[CLOSE_CODE_BYTES..])?;
                Ok(Some(CloseFrame {
                    code,
                    reason: reason.into(),
                }))
            }
        }
    }

    pub(crate) fn format_into(self, output: &mut Vec<u8>) {
        let Self { header, payload } = self;
        let length = u64::try_from(payload.len()).expect("a payload length fits in 64 bits");
        header.format(length, output);
        let start = output.len();
        output.extend_from_slice(&payload);
        if let Some(mask) = header.mask {
            apply_mask(&mut output[start..], mask);
        }
    }
}

pub(crate) fn apply_mask(buffer: &mut [u8], mask: [u8; MASK_BYTES]) {
    let word = u64::from_ne_bytes([
        mask[0], mask[1], mask[2], mask[3], mask[0], mask[1], mask[2], mask[3],
    ]);
    let (words, remainder) = buffer.as_chunks_mut::<MASK_WORD_BYTES>();
    for bytes in words {
        *bytes = (u64::from_ne_bytes(*bytes) ^ word).to_ne_bytes();
    }
    for (index, byte) in remainder.iter_mut().enumerate() {
        *byte ^= mask[index % MASK_BYTES];
    }
}

pub(crate) struct MaskSource {
    pool: [u8; MASK_POOL_BYTES],
    used: usize,
}

impl MaskSource {
    pub(crate) const fn new() -> Self {
        Self {
            pool: [0; MASK_POOL_BYTES],
            used: MASK_POOL_BYTES,
        }
    }

    pub(crate) fn next(&mut self) -> Result<[u8; MASK_BYTES]> {
        if self.used == MASK_POOL_BYTES {
            getrandom::fill(&mut self.pool)
                .map_err(|error| Error::Io(std::io::Error::other(error)))?;
            self.used = 0;
        }
        let mut mask = [0; MASK_BYTES];
        mask.copy_from_slice(&self.pool[self.used..self.used + MASK_BYTES]);
        self.used += MASK_BYTES;
        Ok(mask)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseFrame {
    pub code: CloseCode,
    pub reason: Utf8Bytes,
}

impl fmt::Display for CloseFrame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} ({})", self.reason, self.code)
    }
}

const CLOSE_NORMAL: u16 = 1000;
const CLOSE_AWAY: u16 = 1001;
const CLOSE_PROTOCOL: u16 = 1002;
const CLOSE_UNSUPPORTED: u16 = 1003;
const CLOSE_STATUS: u16 = 1005;
const CLOSE_ABNORMAL: u16 = 1006;
const CLOSE_INVALID: u16 = 1007;
const CLOSE_POLICY: u16 = 1008;
const CLOSE_SIZE: u16 = 1009;
const CLOSE_EXTENSION: u16 = 1010;
const CLOSE_ERROR: u16 = 1011;
const CLOSE_RESTART: u16 = 1012;
const CLOSE_AGAIN: u16 = 1013;
const CLOSE_TLS: u16 = 1015;
const CLOSE_BAD_FIRST: u16 = 1;
const CLOSE_BAD_LAST: u16 = 999;
const CLOSE_RESERVED_FIRST: u16 = 1016;
const CLOSE_RESERVED_LAST: u16 = 2999;
const CLOSE_IANA_FIRST: u16 = 3000;
const CLOSE_IANA_LAST: u16 = 3999;
const CLOSE_LIBRARY_FIRST: u16 = 4000;
const CLOSE_LIBRARY_LAST: u16 = 4999;

#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub enum CloseCode {
    Normal,
    Away,
    Protocol,
    Unsupported,
    Status,
    Abnormal,
    Invalid,
    Policy,
    Size,
    Extension,
    Error,
    Restart,
    Again,
    Tls,
    Reserved(u16),
    Iana(u16),
    Library(u16),
    Bad(u16),
}

impl CloseCode {
    #[must_use]
    pub const fn is_allowed(self) -> bool {
        !matches!(
            self,
            Self::Bad(_) | Self::Reserved(_) | Self::Status | Self::Abnormal | Self::Tls
        )
    }
}

impl fmt::Display for CloseCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", u16::from(*self))
    }
}

impl From<CloseCode> for u16 {
    fn from(code: CloseCode) -> Self {
        match code {
            CloseCode::Normal => CLOSE_NORMAL,
            CloseCode::Away => CLOSE_AWAY,
            CloseCode::Protocol => CLOSE_PROTOCOL,
            CloseCode::Unsupported => CLOSE_UNSUPPORTED,
            CloseCode::Status => CLOSE_STATUS,
            CloseCode::Abnormal => CLOSE_ABNORMAL,
            CloseCode::Invalid => CLOSE_INVALID,
            CloseCode::Policy => CLOSE_POLICY,
            CloseCode::Size => CLOSE_SIZE,
            CloseCode::Extension => CLOSE_EXTENSION,
            CloseCode::Error => CLOSE_ERROR,
            CloseCode::Restart => CLOSE_RESTART,
            CloseCode::Again => CLOSE_AGAIN,
            CloseCode::Tls => CLOSE_TLS,
            CloseCode::Reserved(code)
            | CloseCode::Iana(code)
            | CloseCode::Library(code)
            | CloseCode::Bad(code) => code,
        }
    }
}

impl From<u16> for CloseCode {
    fn from(code: u16) -> Self {
        match code {
            CLOSE_NORMAL => Self::Normal,
            CLOSE_AWAY => Self::Away,
            CLOSE_PROTOCOL => Self::Protocol,
            CLOSE_UNSUPPORTED => Self::Unsupported,
            CLOSE_STATUS => Self::Status,
            CLOSE_ABNORMAL => Self::Abnormal,
            CLOSE_INVALID => Self::Invalid,
            CLOSE_POLICY => Self::Policy,
            CLOSE_SIZE => Self::Size,
            CLOSE_EXTENSION => Self::Extension,
            CLOSE_ERROR => Self::Error,
            CLOSE_RESTART => Self::Restart,
            CLOSE_AGAIN => Self::Again,
            CLOSE_TLS => Self::Tls,
            CLOSE_BAD_FIRST..=CLOSE_BAD_LAST => Self::Bad(code),
            CLOSE_RESERVED_FIRST..=CLOSE_RESERVED_LAST => Self::Reserved(code),
            CLOSE_IANA_FIRST..=CLOSE_IANA_LAST => Self::Iana(code),
            CLOSE_LIBRARY_FIRST..=CLOSE_LIBRARY_LAST => Self::Library(code),
            _ => Self::Bad(code),
        }
    }
}
