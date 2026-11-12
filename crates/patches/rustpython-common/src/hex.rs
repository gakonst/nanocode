use alloc::string::String;

const DIGITS_LOWER: &[u8; 16] = b"0123456789abcdef";
const DIGITS_PER_BYTE: usize = 2;
const NIBBLE_BITS: u32 = 4;
const NIBBLE_MASK: u8 = 0x0f;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputLengthMismatch;

pub fn encode_to_slice(input: &[u8], output: &mut [u8]) -> Result<(), OutputLengthMismatch> {
    if input.len().checked_mul(DIGITS_PER_BYTE) != Some(output.len()) {
        return Err(OutputLengthMismatch);
    }
    for (byte, pair) in input.iter().zip(output.chunks_exact_mut(DIGITS_PER_BYTE)) {
        pair[0] = DIGITS_LOWER[usize::from(byte >> NIBBLE_BITS)];
        pair[1] = DIGITS_LOWER[usize::from(byte & NIBBLE_MASK)];
    }
    Ok(())
}

#[must_use]
pub fn encode(input: impl AsRef<[u8]>) -> String {
    let input = input.as_ref();
    let mut text = String::with_capacity(input.len() * DIGITS_PER_BYTE);
    for byte in input {
        text.push(char::from(DIGITS_LOWER[usize::from(byte >> NIBBLE_BITS)]));
        text.push(char::from(DIGITS_LOWER[usize::from(byte & NIBBLE_MASK)]));
    }
    text
}
