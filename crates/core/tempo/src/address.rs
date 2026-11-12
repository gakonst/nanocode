use std::{fmt, str::FromStr};

use crate::{Error, hex, keccak::keccak256};

pub const ADDRESS_BYTES: usize = 20;
const HASH_BYTES: usize = 32;
const CHECKSUM_UPPER: u8 = 8;
const NIBBLE_BITS: u32 = 4;
const NIBBLE_MASK: u8 = 0x0f;

#[derive(Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Address(pub [u8; ADDRESS_BYTES]);

impl Address {
    pub const ZERO: Self = Self([0; ADDRESS_BYTES]);

    pub fn from_slice(bytes: &[u8]) -> Result<Self, Error> {
        let array: [u8; ADDRESS_BYTES] = bytes.try_into().map_err(|_| Error::InvalidLength {
            expected: ADDRESS_BYTES,
            actual: bytes.len(),
        })?;
        Ok(Self(array))
    }

    pub fn from_public_key(x: &[u8; HASH_BYTES], y: &[u8; HASH_BYTES]) -> Self {
        let mut encoded = [0u8; 2 * HASH_BYTES];
        encoded[..HASH_BYTES].copy_from_slice(x);
        encoded[HASH_BYTES..].copy_from_slice(y);
        let hash = keccak256(encoded);
        let mut address = [0u8; ADDRESS_BYTES];
        address.copy_from_slice(&hash[HASH_BYTES - ADDRESS_BYTES..]);
        Self(address)
    }

    pub const fn as_bytes(&self) -> &[u8; ADDRESS_BYTES] {
        &self.0
    }

    pub fn is_zero(&self) -> bool {
        *self == Self::ZERO
    }

    pub fn to_checksum(&self) -> String {
        let lower = hex::encode(self.0);
        let hash = keccak256(lower.as_bytes());
        let mut text = String::with_capacity(2 + 2 * ADDRESS_BYTES);
        text.push_str("0x");
        for (index, character) in lower.chars().enumerate() {
            let byte = hash[index / 2];
            let nibble = match index % 2 {
                0 => byte >> NIBBLE_BITS,
                _ => byte & NIBBLE_MASK,
            };
            match nibble >= CHECKSUM_UPPER {
                true => text.push(character.to_ascii_uppercase()),
                false => text.push(character),
            }
        }
        text
    }

    pub fn word(&self) -> [u8; HASH_BYTES] {
        let mut word = [0u8; HASH_BYTES];
        word[HASH_BYTES - ADDRESS_BYTES..].copy_from_slice(&self.0);
        word
    }
}

impl FromStr for Address {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self, Error> {
        hex::decode_array(text).map(Self)
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_checksum())
    }
}

impl fmt::Debug for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_checksum())
    }
}

impl fmt::LowerHex for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if f.alternate() {
            f.write_str("0x")?;
        }
        f.write_str(&hex::encode(self.0))
    }
}
