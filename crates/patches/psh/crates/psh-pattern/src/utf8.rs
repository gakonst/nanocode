use alloc::vec::Vec;

const SURROGATE_FIRST: u32 = 0xD800;
const SURROGATE_LAST: u32 = 0xDFFF;
const ONE_BYTE_MAX: u32 = 0x7F;
const TWO_BYTES_MAX: u32 = 0x7FF;
const THREE_BYTES_MAX: u32 = 0xFFFF;
const SCALAR_MAX: u32 = 0x10FFFF;
const CONTINUATION_BITS: u32 = 6;
const UTF8_LENGTH_MAX: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ByteRange {
    pub start: u8,
    pub end: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Sequence {
    ranges: [ByteRange; UTF8_LENGTH_MAX],
    len: usize,
}

impl Sequence {
    pub fn as_slice(&self) -> &[ByteRange] {
        &self.ranges[..self.len]
    }
}

fn encode(c: u32, out: &mut [u8; UTF8_LENGTH_MAX]) -> usize {
    let c = char::from_u32(c).expect("UTF-8 sequences are built from scalar values");
    c.encode_utf8(out).len()
}

pub fn sequences(start: u32, end: u32, out: &mut Vec<Sequence>) {
    assert!(start <= end);
    assert!(end <= SCALAR_MAX);
    let mut stack = alloc::vec![(start, end)];
    while let Some((start, end)) = stack.pop() {
        if start > end {
            continue;
        }
        if start < SURROGATE_FIRST && end > SURROGATE_LAST {
            stack.push((SURROGATE_LAST + 1, end));
            stack.push((start, SURROGATE_FIRST - 1));
            continue;
        }
        if (SURROGATE_FIRST..=SURROGATE_LAST).contains(&start)
            || (SURROGATE_FIRST..=SURROGATE_LAST).contains(&end)
        {
            let start = if (SURROGATE_FIRST..=SURROGATE_LAST).contains(&start) {
                SURROGATE_LAST + 1
            } else {
                start
            };
            let end = if (SURROGATE_FIRST..=SURROGATE_LAST).contains(&end) {
                SURROGATE_FIRST - 1
            } else {
                end
            };
            stack.push((start, end));
            continue;
        }
        let mut split = false;
        for boundary in [ONE_BYTE_MAX, TWO_BYTES_MAX, THREE_BYTES_MAX] {
            if start <= boundary && boundary < end {
                stack.push((boundary + 1, end));
                stack.push((start, boundary));
                split = true;
                break;
            }
        }
        if split {
            continue;
        }
        if end <= ONE_BYTE_MAX {
            let mut ranges = [ByteRange { start: 0, end: 0 }; UTF8_LENGTH_MAX];
            ranges[0] = ByteRange {
                start: start as u8,
                end: end as u8,
            };
            out.push(Sequence { ranges, len: 1 });
            continue;
        }
        for i in 1..UTF8_LENGTH_MAX as u32 {
            let mask = (1u32 << (CONTINUATION_BITS * i)) - 1;
            if start & !mask != end & !mask {
                if start & mask != 0 {
                    stack.push(((start | mask) + 1, end));
                    stack.push((start, start | mask));
                    split = true;
                    break;
                }
                if end & mask != mask {
                    stack.push((end & !mask, end));
                    stack.push((start, (end & !mask) - 1));
                    split = true;
                    break;
                }
            }
        }
        if split {
            continue;
        }
        let mut low = [0; UTF8_LENGTH_MAX];
        let mut high = [0; UTF8_LENGTH_MAX];
        let n = encode(start, &mut low);
        let m = encode(end, &mut high);
        assert_eq!(n, m);
        let mut ranges = [ByteRange { start: 0, end: 0 }; UTF8_LENGTH_MAX];
        for k in 0..n {
            ranges[k] = ByteRange {
                start: low[k],
                end: high[k],
            };
        }
        out.push(Sequence { ranges, len: n });
    }
}

pub fn decode(bytes: &[u8]) -> Option<Result<char, u8>> {
    let first = *bytes.first()?;
    let len = match first {
        0x00..=0x7F => return Some(Ok(char::from(first))),
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => return Some(Err(first)),
    };
    if len > bytes.len() {
        return Some(Err(first));
    }
    match core::str::from_utf8(&bytes[..len]) {
        Ok(s) => Some(Ok(s.chars().next().expect("one character"))),
        Err(_) => Some(Err(first)),
    }
}

pub fn decode_last(bytes: &[u8]) -> Option<Result<char, u8>> {
    if bytes.is_empty() {
        return None;
    }
    let mut start = bytes.len() - 1;
    let limit = bytes.len().saturating_sub(UTF8_LENGTH_MAX);
    while start > limit && (bytes[start] & 0b1100_0000) == 0b1000_0000 {
        start -= 1;
    }
    match decode(&bytes[start..]) {
        None => None,
        Some(Ok(c)) => Some(Ok(c)),
        Some(Err(_)) => Some(Err(bytes[bytes.len() - 1])),
    }
}

pub fn is_char_boundary(bytes: &[u8], at: usize) -> bool {
    match bytes.get(at) {
        None => at == bytes.len(),
        Some(&b) => b <= 0x7F || b >= 0xC0,
    }
}
