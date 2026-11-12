use crate::Error;

const DIGITS: &[u8; 16] = b"0123456789abcdef";
const NIBBLE_BITS: u32 = 4;
const NIBBLE_MASK: u8 = 0x0f;

pub fn encode(bytes: impl AsRef<[u8]>) -> String {
    let bytes = bytes.as_ref();
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(DIGITS[(byte >> NIBBLE_BITS) as usize] as char);
        text.push(DIGITS[(byte & NIBBLE_MASK) as usize] as char);
    }
    text
}

pub fn encode_prefixed(bytes: impl AsRef<[u8]>) -> String {
    format!("0x{}", encode(bytes))
}

pub fn decode(text: &str) -> Result<Vec<u8>, Error> {
    let text = text.strip_prefix("0x").unwrap_or(text);
    let digits = text.as_bytes();
    if !digits.len().is_multiple_of(2) {
        return Err(Error::InvalidHex);
    }
    let mut bytes = Vec::with_capacity(digits.len() / 2);
    let mut invalid = 0u8;
    for pair in digits.chunks(2) {
        let (high, high_invalid) = nibble(pair[0]);
        let (low, low_invalid) = nibble(pair[1]);
        invalid |= high_invalid | low_invalid;
        bytes.push((high << NIBBLE_BITS) | low);
    }
    if invalid != 0 {
        return Err(Error::InvalidHex);
    }
    Ok(bytes)
}

pub fn decode_array<const N: usize>(text: &str) -> Result<[u8; N], Error> {
    let bytes = decode(text)?;
    let actual = bytes.len();
    bytes
        .try_into()
        .map_err(|_| Error::InvalidLength { expected: N, actual })
}

const fn nibble(digit: u8) -> (u8, u8) {
    let decimal = digit.wrapping_sub(b'0');
    let lower = (digit | 0x20).wrapping_sub(b'a');
    let is_decimal = mask_below(decimal, 10);
    let is_letter = mask_below(lower, 6);
    let value = (decimal & is_decimal) | (lower.wrapping_add(10) & is_letter);
    (value, !(is_decimal | is_letter) & 1)
}

const fn mask_below(value: u8, bound: u8) -> u8 {
    ((value as u16).wrapping_sub(bound as u16) >> u8::BITS) as u8
}
