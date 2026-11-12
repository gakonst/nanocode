use crate::Error;

const SHORT_STRING: u8 = 0x80;
const LONG_STRING: u8 = 0xb7;
const SHORT_LIST: u8 = 0xc0;
const LONG_LIST: u8 = 0xf7;
const SHORT_LIMIT: usize = 55;

pub const EMPTY_STRING: u8 = SHORT_STRING;
pub const EMPTY_LIST: u8 = SHORT_LIST;

pub fn encode_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    if bytes.len() == 1 && bytes[0] < SHORT_STRING {
        out.push(bytes[0]);
        return;
    }
    header(out, bytes.len(), SHORT_STRING, LONG_STRING);
    out.extend_from_slice(bytes);
}

pub fn encode_uint(out: &mut Vec<u8>, big_endian: &[u8]) {
    let start = big_endian
        .iter()
        .position(|byte| *byte != 0)
        .unwrap_or(big_endian.len());
    encode_bytes(out, &big_endian[start..]);
}

pub fn encode_u64(out: &mut Vec<u8>, value: u64) {
    encode_uint(out, &value.to_be_bytes());
}

pub fn encode_u128(out: &mut Vec<u8>, value: u128) {
    encode_uint(out, &value.to_be_bytes());
}

pub fn encode_list(out: &mut Vec<u8>, payload: &[u8]) {
    header(out, payload.len(), SHORT_LIST, LONG_LIST);
    out.extend_from_slice(payload);
}

pub fn list_header(out: &mut Vec<u8>, payload_length: usize) {
    header(out, payload_length, SHORT_LIST, LONG_LIST);
}

fn header(out: &mut Vec<u8>, length: usize, short: u8, long: u8) {
    if length <= SHORT_LIMIT {
        out.push(short + length as u8);
        return;
    }
    let bytes = (length as u64).to_be_bytes();
    let start = bytes.iter().position(|byte| *byte != 0).unwrap_or(bytes.len());
    out.push(long + (bytes.len() - start) as u8);
    out.extend_from_slice(&bytes[start..]);
}

#[derive(Clone, Copy, Debug)]
pub enum Item<'a> {
    Bytes(&'a [u8]),
    List(Reader<'a>),
}

#[derive(Clone, Copy, Debug)]
pub struct Reader<'a> {
    data: &'a [u8],
}

impl<'a> Reader<'a> {
    pub const fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    pub const fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub const fn peek(&self) -> Option<u8> {
        self.data.first().copied()
    }

    pub const fn remaining(&self) -> usize {
        self.data.len()
    }

    pub fn item(&mut self) -> Result<Item<'a>, Error> {
        let (&first, rest) = self.data.split_first().ok_or(Error::Rlp("input too short"))?;
        let (list, offset, length) = match first {
            byte if byte < SHORT_STRING => {
                let item = &self.data[..1];
                self.data = rest;
                return Ok(Item::Bytes(item));
            }
            byte if byte <= LONG_STRING => (false, 0, (byte - SHORT_STRING) as usize),
            byte if byte < SHORT_LIST => {
                let size = (byte - LONG_STRING) as usize;
                (false, size, long_length(rest, size)?)
            }
            byte if byte <= LONG_LIST => (true, 0, (byte - SHORT_LIST) as usize),
            byte => {
                let size = (byte - LONG_LIST) as usize;
                (true, size, long_length(rest, size)?)
            }
        };
        let body = rest.get(offset..).ok_or(Error::Rlp("input too short"))?;
        let payload = body.get(..length).ok_or(Error::Rlp("input too short"))?;
        self.data = &body[length..];
        if !list && offset == 0 && length == 1 && payload[0] < SHORT_STRING {
            return Err(Error::Rlp("non-canonical single byte"));
        }
        Ok(match list {
            true => Item::List(Reader::new(payload)),
            false => Item::Bytes(payload),
        })
    }

    pub fn bytes(&mut self) -> Result<&'a [u8], Error> {
        match self.item()? {
            Item::Bytes(bytes) => Ok(bytes),
            Item::List(_) => Err(Error::Rlp("expected a string")),
        }
    }

    pub fn list(&mut self) -> Result<Self, Error> {
        match self.item()? {
            Item::List(reader) => Ok(reader),
            Item::Bytes(_) => Err(Error::Rlp("expected a list")),
        }
    }

    pub fn uint(&mut self, width: usize) -> Result<&'a [u8], Error> {
        let bytes = self.bytes()?;
        if bytes.first() == Some(&0) {
            return Err(Error::Rlp("leading zero"));
        }
        if bytes.len() > width {
            return Err(Error::Rlp("integer overflow"));
        }
        Ok(bytes)
    }

    pub fn u64(&mut self) -> Result<u64, Error> {
        let bytes = self.uint(std::mem::size_of::<u64>())?;
        Ok(bytes.iter().fold(0u64, |value, byte| (value << u8::BITS) | *byte as u64))
    }

    pub fn fixed<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        let bytes = self.bytes()?;
        bytes.try_into().map_err(|_| Error::Rlp("unexpected length"))
    }

    pub const fn finish(&self) -> Result<(), Error> {
        match self.data.is_empty() {
            true => Ok(()),
            false => Err(Error::Rlp("trailing bytes")),
        }
    }
}

fn long_length(data: &[u8], size: usize) -> Result<usize, Error> {
    let bytes = data.get(..size).ok_or(Error::Rlp("input too short"))?;
    if bytes.first() == Some(&0) {
        return Err(Error::Rlp("leading zero in length"));
    }
    if size > std::mem::size_of::<usize>() {
        return Err(Error::Rlp("length overflow"));
    }
    let length = bytes.iter().fold(0usize, |value, byte| (value << u8::BITS) | *byte as usize);
    if length <= SHORT_LIMIT {
        return Err(Error::Rlp("non-canonical length"));
    }
    Ok(length)
}
