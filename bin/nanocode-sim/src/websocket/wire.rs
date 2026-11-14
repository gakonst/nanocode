use nanocode_digest::Sha1;
use nanocode_sys::base64::STANDARD;

pub const FINAL_BIT: u8 = 0x80;
pub const RSV1_BIT: u8 = 0x40;
pub const RSV2_BIT: u8 = 0x20;
pub const RSV3_BIT: u8 = 0x10;
const RESERVED_BITS: u8 = RSV1_BIT | RSV2_BIT | RSV3_BIT;
const OPCODE_BITS: u8 = 0x0f;
const MASK_BIT: u8 = 0x80;
const LENGTH_BITS: u8 = 0x7f;
const LENGTH_16_MARKER: u8 = 126;
const LENGTH_64_MARKER: u8 = 127;
const BASE_HEADER_BYTES: usize = 2;
const LENGTH_16_BYTES: usize = 2;
const LENGTH_64_BYTES: usize = 8;
const LENGTH_64_HIGH_BIT: u64 = 1 << 63;
pub const LENGTH_7_MAX: usize = 125;
pub const LENGTH_16_MAX: usize = 65_535;
pub const MASK_BYTES: usize = 4;
pub const CONTROL_PAYLOAD_BYTES_MAX: usize = 125;
pub const CLOSE_CODE_BYTES: usize = 2;
pub const OPCODE_CONTINUE: u8 = 0x0;
pub const OPCODE_TEXT: u8 = 0x1;
pub const OPCODE_BINARY: u8 = 0x2;
pub const OPCODE_CLOSE: u8 = 0x8;
pub const OPCODE_PING: u8 = 0x9;
pub const OPCODE_PONG: u8 = 0xa;
const ACCEPT_GUID: &[u8] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const KEY_BYTES: usize = 16;
const ENCODED_KEY_BYTES: usize = 24;

pub const fn is_known(opcode: u8) -> bool {
    matches!(
        opcode,
        OPCODE_CONTINUE | OPCODE_TEXT | OPCODE_BINARY | OPCODE_CLOSE | OPCODE_PING | OPCODE_PONG
    )
}

pub const fn is_control(opcode: u8) -> bool {
    matches!(opcode, OPCODE_CLOSE | OPCODE_PING | OPCODE_PONG)
}

pub fn accept_key(key: &str) -> String {
    let mut input = Vec::with_capacity(key.len() + ACCEPT_GUID.len());
    input.extend_from_slice(key.as_bytes());
    input.extend_from_slice(ACCEPT_GUID);
    STANDARD.encode(Sha1::digest(&input))
}

pub fn is_valid_key(key: &str) -> bool {
    key.len() == ENCODED_KEY_BYTES
        && STANDARD
            .decode(key)
            .is_ok_and(|decoded| decoded.len() == KEY_BYTES)
}

pub fn close_payload(code: u16, reason: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(CLOSE_CODE_BYTES + reason.len());
    payload.extend_from_slice(&code.to_be_bytes());
    payload.extend_from_slice(reason);
    payload
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LengthForm {
    Minimal,
    Wide16,
    Wide64,
    HighBit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Extended {
    None,
    Sixteen,
    SixtyFour,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inflated {
    Plain,
    To(Vec<u8>),
    Over(usize),
    Corrupt,
}

#[derive(Clone, Debug)]
pub struct ServerFrame {
    pub fin: bool,
    pub reserved: u8,
    pub opcode: u8,
    pub mask: Option<[u8; MASK_BYTES]>,
    pub form: LengthForm,
    pub payload: Vec<u8>,
    pub inflated: Inflated,
}

impl ServerFrame {
    pub const fn new(opcode: u8, fin: bool, payload: Vec<u8>) -> Self {
        Self {
            fin,
            reserved: 0,
            opcode,
            mask: None,
            form: LengthForm::Minimal,
            payload,
            inflated: Inflated::Plain,
        }
    }

    pub fn declared_length(&self) -> u64 {
        let length = u64::try_from(self.payload.len()).expect("a payload length fits in u64");
        match self.form {
            LengthForm::HighBit => length | LENGTH_64_HIGH_BIT,
            LengthForm::Minimal | LengthForm::Wide16 | LengthForm::Wide64 => length,
        }
    }

    fn extended(&self) -> Extended {
        let length = self.payload.len();
        match self.form {
            LengthForm::Minimal if length <= LENGTH_7_MAX => Extended::None,
            LengthForm::Minimal if length <= LENGTH_16_MAX => Extended::Sixteen,
            LengthForm::Wide16 => {
                assert!(
                    length <= LENGTH_16_MAX,
                    "a 16-bit length carries at most 65535 bytes"
                );
                Extended::Sixteen
            }
            LengthForm::Minimal | LengthForm::Wide64 | LengthForm::HighBit => Extended::SixtyFour,
        }
    }

    pub fn header_bytes(&self) -> usize {
        let extended = match self.extended() {
            Extended::None => 0,
            Extended::Sixteen => LENGTH_16_BYTES,
            Extended::SixtyFour => LENGTH_64_BYTES,
        };
        let mask = if self.mask.is_some() { MASK_BYTES } else { 0 };
        BASE_HEADER_BYTES + extended + mask
    }

    pub fn encode_into(&self, output: &mut Vec<u8>) {
        let start = output.len();
        let fin = if self.fin { FINAL_BIT } else { 0 };
        let first = self.opcode | self.reserved | fin;
        let mask = if self.mask.is_some() { MASK_BIT } else { 0 };
        match self.extended() {
            Extended::None => {
                let length = u8::try_from(self.payload.len()).expect("a 7-bit length fits in u8");
                output.extend_from_slice(&[first, mask | length]);
            }
            Extended::Sixteen => {
                let length =
                    u16::try_from(self.payload.len()).expect("a 16-bit length fits in u16");
                output.extend_from_slice(&[first, mask | LENGTH_16_MARKER]);
                output.extend_from_slice(&length.to_be_bytes());
            }
            Extended::SixtyFour => {
                output.extend_from_slice(&[first, mask | LENGTH_64_MARKER]);
                output.extend_from_slice(&self.declared_length().to_be_bytes());
            }
        }
        if let Some(key) = self.mask {
            output.extend_from_slice(&key);
        }
        assert_eq!(
            output.len() - start,
            self.header_bytes(),
            "the header has its planned size"
        );
        let payload_start = output.len();
        output.extend_from_slice(&self.payload);
        if let Some(key) = self.mask {
            for (index, byte) in output[payload_start..].iter_mut().enumerate() {
                *byte ^= key[index % MASK_BYTES];
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientFrame {
    pub fin: bool,
    pub opcode: u8,
    pub payload: Vec<u8>,
}

fn client_length(bytes: &[u8], second: u8) -> Result<Option<(usize, usize)>, String> {
    match second & LENGTH_BITS {
        LENGTH_16_MARKER => {
            let end = BASE_HEADER_BYTES + LENGTH_16_BYTES;
            let Some(extended) = bytes.get(BASE_HEADER_BYTES..end) else {
                return Ok(None);
            };
            let length = usize::from(u16::from_be_bytes([extended[0], extended[1]]));
            if length <= LENGTH_7_MAX {
                return Err(format!(
                    "the client spent a 16-bit length on {length} bytes"
                ));
            }
            Ok(Some((length, end)))
        }
        LENGTH_64_MARKER => {
            let end = BASE_HEADER_BYTES + LENGTH_64_BYTES;
            let Some(extended) = bytes.get(BASE_HEADER_BYTES..end) else {
                return Ok(None);
            };
            let mut wide = [0; LENGTH_64_BYTES];
            wide.copy_from_slice(extended);
            let length = u64::from_be_bytes(wide);
            if length & LENGTH_64_HIGH_BIT != 0 {
                return Err("the client set the high bit of a 64-bit length".to_owned());
            }
            let length = usize::try_from(length)
                .map_err(|_| format!("the client declared {length} bytes"))?;
            if length <= LENGTH_16_MAX {
                return Err(format!(
                    "the client spent a 64-bit length on {length} bytes"
                ));
            }
            Ok(Some((length, end)))
        }
        short => Ok(Some((usize::from(short), BASE_HEADER_BYTES))),
    }
}

pub fn decode_client(bytes: &[u8]) -> Result<Option<(ClientFrame, usize)>, String> {
    let [first, second, ..] = *bytes else {
        return Ok(None);
    };
    if first & RESERVED_BITS != 0 {
        return Err(format!("a client frame sets reserved bits: {first:#04x}"));
    }
    let opcode = first & OPCODE_BITS;
    if !is_known(opcode) {
        return Err(format!("a client frame has opcode {opcode:#x}"));
    }
    if second & MASK_BIT == 0 {
        return Err("a client frame is not masked".to_owned());
    }
    let Some((length, header)) = client_length(bytes, second)? else {
        return Ok(None);
    };
    let fin = first & FINAL_BIT != 0;
    if is_control(opcode) && !fin {
        return Err(format!("a client control frame {opcode:#x} is fragmented"));
    }
    if is_control(opcode) && length > CONTROL_PAYLOAD_BYTES_MAX {
        return Err(format!("a client control frame carries {length} bytes"));
    }
    let Some(key) = bytes.get(header..header + MASK_BYTES) else {
        return Ok(None);
    };
    let mut mask = [0; MASK_BYTES];
    mask.copy_from_slice(key);
    let start = header + MASK_BYTES;
    let Some(masked) = bytes.get(start..start + length) else {
        return Ok(None);
    };
    let payload = masked
        .iter()
        .enumerate()
        .map(|(index, byte)| byte ^ mask[index % MASK_BYTES])
        .collect();
    Ok(Some((
        ClientFrame {
            fin,
            opcode,
            payload,
        },
        start + length,
    )))
}
