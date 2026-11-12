use crate::Error;

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
const SEXTET_BITS: u32 = 6;
const SEXTET_MASK: u32 = 0x3f;
const GROUP_BYTES: usize = 3;
const GROUP_CHARACTERS: usize = 4;

pub fn encode_url(bytes: impl AsRef<[u8]>) -> String {
    let bytes = bytes.as_ref();
    let mut text = String::with_capacity(bytes.len().div_ceil(GROUP_BYTES) * GROUP_CHARACTERS);
    for group in bytes.chunks(GROUP_BYTES) {
        let mut word = 0u32;
        for (index, byte) in group.iter().enumerate() {
            word |= (*byte as u32) << (u8::BITS * (2 - index as u32));
        }
        for index in 0..=group.len() {
            let shift = SEXTET_BITS * (GROUP_CHARACTERS - 1 - index) as u32;
            text.push(ALPHABET[((word >> shift) & SEXTET_MASK) as usize] as char);
        }
    }
    text
}

pub fn decode_url(text: &str) -> Result<Vec<u8>, Error> {
    let characters = text.as_bytes();
    if characters.len() % GROUP_CHARACTERS == 1 {
        return Err(Error::InvalidBase64);
    }
    let mut bytes = Vec::with_capacity(characters.len() * GROUP_BYTES / GROUP_CHARACTERS);
    let mut invalid = 0u8;
    for group in characters.chunks(GROUP_CHARACTERS) {
        let mut word = 0u32;
        for (index, character) in group.iter().enumerate() {
            let (value, bad) = sextet(*character);
            invalid |= bad;
            word |= (value as u32) << (SEXTET_BITS * (GROUP_CHARACTERS - 1 - index) as u32);
        }
        let produced = group.len() - 1;
        let unused_bits = (GROUP_BYTES - produced) as u32 * u8::BITS;
        if unused_bits > 0 && word & ((1 << unused_bits) - 1) != 0 {
            invalid |= 1;
        }
        for index in 0..produced {
            bytes.push((word >> (u8::BITS * (2 - index as u32))) as u8);
        }
    }
    if invalid != 0 {
        return Err(Error::InvalidBase64);
    }
    Ok(bytes)
}

const fn sextet(character: u8) -> (u8, u8) {
    let upper = range(character, b'A', b'Z');
    let lower = range(character, b'a', b'z');
    let digit = range(character, b'0', b'9');
    let dash = equal(character, b'-');
    let underscore = equal(character, b'_');
    let value = (character.wrapping_sub(b'A') & upper)
        | (character.wrapping_sub(b'a').wrapping_add(26) & lower)
        | (character.wrapping_sub(b'0').wrapping_add(52) & digit)
        | (62 & dash)
        | (63 & underscore);
    (value, !(upper | lower | digit | dash | underscore) & 1)
}

const fn range(value: u8, low: u8, high: u8) -> u8 {
    let above = ((value as u16).wrapping_sub(low as u16) >> u8::BITS) as u8;
    let below = ((high as u16).wrapping_sub(value as u16) >> u8::BITS) as u8;
    !(above | below)
}

const fn equal(value: u8, expected: u8) -> u8 {
    range(value, expected, expected)
}
