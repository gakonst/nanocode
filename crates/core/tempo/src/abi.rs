use crate::{Address, Error, U256, keccak::keccak256};

pub const WORD: usize = 32;
pub const SELECTOR: usize = 4;

pub fn selector(signature: &str) -> [u8; SELECTOR] {
    let hash = keccak256(signature.as_bytes());
    let mut selector = [0u8; SELECTOR];
    selector.copy_from_slice(&hash[..SELECTOR]);
    selector
}

#[derive(Clone, Debug)]
pub enum Token {
    Address(Address),
    Uint(U256),
    Bool(bool),
    FixedBytes([u8; WORD]),
    Bytes(Vec<u8>),
    Tuple(Vec<Self>),
    Array(Vec<Self>),
}

impl Token {
    fn is_dynamic(&self) -> bool {
        match self {
            Self::Bytes(_) | Self::Array(_) => true,
            Self::Tuple(tokens) => tokens.iter().any(Self::is_dynamic),
            _ => false,
        }
    }

    fn head_size(&self) -> usize {
        match self {
            Self::Tuple(tokens) if !self.is_dynamic() => tokens.iter().map(Self::head_size).sum(),
            _ => WORD,
        }
    }
}

pub fn encode(tokens: &[Token]) -> Vec<u8> {
    let head_size: usize = tokens.iter().map(Token::head_size).sum();
    let mut head = Vec::with_capacity(head_size);
    let mut tail = Vec::new();
    for token in tokens {
        if token.is_dynamic() {
            head.extend_from_slice(&U256::from_u64((head_size + tail.len()) as u64).to_be_bytes());
            encode_tail(token, &mut tail);
        } else {
            encode_static(token, &mut head);
        }
    }
    head.extend_from_slice(&tail);
    head
}

fn encode_static(token: &Token, out: &mut Vec<u8>) {
    match token {
        Token::Address(address) => out.extend_from_slice(&address.word()),
        Token::Uint(value) => out.extend_from_slice(&value.to_be_bytes()),
        Token::Bool(value) => out.extend_from_slice(&U256::from_u64(*value as u64).to_be_bytes()),
        Token::FixedBytes(bytes) => out.extend_from_slice(bytes),
        Token::Tuple(tokens) => {
            for token in tokens {
                encode_static(token, out);
            }
        }
        Token::Bytes(_) | Token::Array(_) => unreachable!("dynamic tokens are encoded in the tail"),
    }
}

fn encode_tail(token: &Token, out: &mut Vec<u8>) {
    match token {
        Token::Bytes(bytes) => {
            out.extend_from_slice(&U256::from_u64(bytes.len() as u64).to_be_bytes());
            out.extend_from_slice(bytes);
            out.resize(out.len() + (WORD - bytes.len() % WORD) % WORD, 0);
        }
        Token::Array(tokens) => {
            out.extend_from_slice(&U256::from_u64(tokens.len() as u64).to_be_bytes());
            out.extend_from_slice(&encode(tokens));
        }
        Token::Tuple(tokens) => out.extend_from_slice(&encode(tokens)),
        _ => encode_static(token, out),
    }
}

pub fn call(signature: &str, tokens: &[Token]) -> Vec<u8> {
    let mut data = selector(signature).to_vec();
    data.extend_from_slice(&encode(tokens));
    data
}

pub struct Decoder<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> Decoder<'a> {
    pub const fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    pub fn word(&mut self) -> Result<[u8; WORD], Error> {
        let word = self
            .data
            .get(self.offset..self.offset + WORD)
            .ok_or(Error::Abi("data too short"))?;
        self.offset += WORD;
        let mut array = [0u8; WORD];
        array.copy_from_slice(word);
        Ok(array)
    }

    pub fn uint(&mut self) -> Result<U256, Error> {
        self.word().map(U256::from_be_bytes)
    }

    pub fn address(&mut self) -> Result<Address, Error> {
        let word = self.word()?;
        if word[..WORD - crate::address::ADDRESS_BYTES].iter().any(|byte| *byte != 0) {
            return Err(Error::Abi("address has dirty high bytes"));
        }
        Address::from_slice(&word[WORD - crate::address::ADDRESS_BYTES..])
    }

    pub fn boolean(&mut self) -> Result<bool, Error> {
        match self.uint()? {
            value if value == U256::ZERO => Ok(false),
            value if value == U256::ONE => Ok(true),
            _ => Err(Error::Abi("boolean is neither 0 nor 1")),
        }
    }

    pub const fn finish(&self) -> Result<(), Error> {
        match self.offset == self.data.len() {
            true => Ok(()),
            false => Err(Error::Abi("trailing data")),
        }
    }
}
