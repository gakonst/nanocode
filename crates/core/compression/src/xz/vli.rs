use super::stream::Error;

pub(crate) const VLI_MAX: u64 = u64::MAX / 2;
pub(crate) const VLI_BYTES_MAX: usize = 9;
const PAYLOAD_BITS: usize = 7;
const PAYLOAD_MASK: u8 = 0x7F;
const CONTINUATION_BIT: u8 = 0x80;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Progress {
    Incomplete,
    Complete,
}

pub(crate) fn size(value: u64) -> usize {
    assert!(value <= VLI_MAX);
    let mut bytes = 1;
    let mut rest = value >> PAYLOAD_BITS;
    while rest != 0 {
        rest >>= PAYLOAD_BITS;
        bytes += 1;
    }
    bytes
}

pub(crate) fn decode_whole(input: &[u8], position: &mut usize) -> Result<u64, Error> {
    let mut value = 0;
    let mut value_position = 0;
    if *position >= input.len() {
        return Err(Error::Data);
    }
    match decode(&mut value, &mut value_position, input, position)? {
        Progress::Complete => Ok(value),
        Progress::Incomplete => Err(Error::Data),
    }
}

pub(crate) fn decode(
    value: &mut u64,
    value_position: &mut usize,
    input: &[u8],
    position: &mut usize,
) -> Result<Progress, Error> {
    if *value_position == 0 {
        *value = 0;
    }
    assert!(*value_position < VLI_BYTES_MAX);
    assert!(*value >> (*value_position * PAYLOAD_BITS) == 0);
    assert!(*position < input.len());
    while *position < input.len() {
        let byte = input[*position];
        *position += 1;
        *value += u64::from(byte & PAYLOAD_MASK) << (*value_position * PAYLOAD_BITS);
        *value_position += 1;
        if byte & CONTINUATION_BIT == 0 {
            if byte == 0 && *value_position > 1 {
                return Err(Error::Data);
            }
            return Ok(Progress::Complete);
        }
        if *value_position == VLI_BYTES_MAX {
            return Err(Error::Data);
        }
    }
    Ok(Progress::Incomplete)
}

pub(crate) fn encode_whole(value: u64, output: &mut [u8], position: &mut usize) {
    let mut value_position = 0;
    let progress = encode(value, &mut value_position, output, position);
    assert_eq!(progress, Progress::Complete);
}

pub(crate) fn encode(
    value: u64,
    value_position: &mut usize,
    output: &mut [u8],
    position: &mut usize,
) -> Progress {
    assert!(*position < output.len());
    assert!(*value_position < VLI_BYTES_MAX);
    assert!(value <= VLI_MAX);
    let mut rest = value >> (*value_position * PAYLOAD_BITS);
    while rest >= u64::from(CONTINUATION_BIT) {
        *value_position += 1;
        output[*position] = (rest as u8 & PAYLOAD_MASK) | CONTINUATION_BIT;
        rest >>= PAYLOAD_BITS;
        *position += 1;
        if *position == output.len() {
            return Progress::Incomplete;
        }
    }
    output[*position] = rest as u8;
    *position += 1;
    *value_position += 1;
    Progress::Complete
}
