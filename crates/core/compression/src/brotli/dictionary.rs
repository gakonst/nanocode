use super::DecodeError;
use super::bits::low_mask;

const DICTIONARY: &[u8] = include_bytes!("dictionary.bin");
pub(super) const WORD_LENGTH_MIN: u32 = 4;
pub(super) const WORD_LENGTH_MAX: u32 = 24;
const WORD_LENGTHS: usize = WORD_LENGTH_MAX as usize + 1;
const WORD_COUNT_BITS: [u8; WORD_LENGTHS] =
    [0, 0, 0, 0, 10, 10, 11, 11, 10, 10, 10, 10, 10, 9, 9, 8, 7, 7, 8, 7, 7, 6, 6, 5, 5];
const UPPERCASE_ASCII_LIMIT: u8 = 0xc0;
const UPPERCASE_TWO_BYTE_LIMIT: u8 = 0xe0;
const UPPERCASE_TWO_BYTE_FLIP: u8 = 0x20;
const UPPERCASE_THREE_BYTE_FLIP: u8 = 0x05;
const ASCII_CASE_BIT: u8 = 0x20;

const fn word_offsets() -> [usize; WORD_LENGTHS + 1] {
    let mut offsets = [0; WORD_LENGTHS + 1];
    let mut length = 0;
    while length < WORD_LENGTHS {
        let count = if WORD_COUNT_BITS[length] == 0 {
            0
        } else {
            1 << WORD_COUNT_BITS[length]
        };
        offsets[length + 1] = offsets[length] + length * count;
        length += 1;
    }
    offsets
}

const WORD_OFFSETS: [usize; WORD_LENGTHS + 1] = word_offsets();
const _: () = assert!(WORD_OFFSETS[WORD_LENGTHS] == DICTIONARY.len());

#[derive(Clone, Copy)]
enum Kind {
    Identity,
    OmitLast(usize),
    UppercaseFirst,
    UppercaseAll,
    OmitFirst(usize),
}

struct Transform {
    prefix: &'static [u8],
    kind: Kind,
    suffix: &'static [u8],
}

const fn transform(prefix: &'static [u8], kind: Kind, suffix: &'static [u8]) -> Transform {
    Transform { prefix, kind, suffix }
}

const TRANSFORMS: [Transform; 121] = [
    transform(b"", Kind::Identity, b""),
    transform(b"", Kind::Identity, b" "),
    transform(b" ", Kind::Identity, b" "),
    transform(b"", Kind::OmitFirst(1), b""),
    transform(b"", Kind::UppercaseFirst, b" "),
    transform(b"", Kind::Identity, b" the "),
    transform(b" ", Kind::Identity, b""),
    transform(b"s ", Kind::Identity, b" "),
    transform(b"", Kind::Identity, b" of "),
    transform(b"", Kind::UppercaseFirst, b""),
    transform(b"", Kind::Identity, b" and "),
    transform(b"", Kind::OmitFirst(2), b""),
    transform(b"", Kind::OmitLast(1), b""),
    transform(b", ", Kind::Identity, b" "),
    transform(b"", Kind::Identity, b", "),
    transform(b" ", Kind::UppercaseFirst, b" "),
    transform(b"", Kind::Identity, b" in "),
    transform(b"", Kind::Identity, b" to "),
    transform(b"e ", Kind::Identity, b" "),
    transform(b"", Kind::Identity, b"\""),
    transform(b"", Kind::Identity, b"."),
    transform(b"", Kind::Identity, b"\">"),
    transform(b"", Kind::Identity, b"\n"),
    transform(b"", Kind::OmitLast(3), b""),
    transform(b"", Kind::Identity, b"]"),
    transform(b"", Kind::Identity, b" for "),
    transform(b"", Kind::OmitFirst(3), b""),
    transform(b"", Kind::OmitLast(2), b""),
    transform(b"", Kind::Identity, b" a "),
    transform(b"", Kind::Identity, b" that "),
    transform(b" ", Kind::UppercaseFirst, b""),
    transform(b"", Kind::Identity, b". "),
    transform(b".", Kind::Identity, b""),
    transform(b" ", Kind::Identity, b", "),
    transform(b"", Kind::OmitFirst(4), b""),
    transform(b"", Kind::Identity, b" with "),
    transform(b"", Kind::Identity, b"'"),
    transform(b"", Kind::Identity, b" from "),
    transform(b"", Kind::Identity, b" by "),
    transform(b"", Kind::OmitFirst(5), b""),
    transform(b"", Kind::OmitFirst(6), b""),
    transform(b" the ", Kind::Identity, b""),
    transform(b"", Kind::OmitLast(4), b""),
    transform(b"", Kind::Identity, b". The "),
    transform(b"", Kind::UppercaseAll, b""),
    transform(b"", Kind::Identity, b" on "),
    transform(b"", Kind::Identity, b" as "),
    transform(b"", Kind::Identity, b" is "),
    transform(b"", Kind::OmitLast(7), b""),
    transform(b"", Kind::OmitLast(1), b"ing "),
    transform(b"", Kind::Identity, b"\n\t"),
    transform(b"", Kind::Identity, b":"),
    transform(b" ", Kind::Identity, b". "),
    transform(b"", Kind::Identity, b"ed "),
    transform(b"", Kind::OmitFirst(9), b""),
    transform(b"", Kind::OmitFirst(7), b""),
    transform(b"", Kind::OmitLast(6), b""),
    transform(b"", Kind::Identity, b"("),
    transform(b"", Kind::UppercaseFirst, b", "),
    transform(b"", Kind::OmitLast(8), b""),
    transform(b"", Kind::Identity, b" at "),
    transform(b"", Kind::Identity, b"ly "),
    transform(b" the ", Kind::Identity, b" of "),
    transform(b"", Kind::OmitLast(5), b""),
    transform(b"", Kind::OmitLast(9), b""),
    transform(b" ", Kind::UppercaseFirst, b", "),
    transform(b"", Kind::UppercaseFirst, b"\""),
    transform(b".", Kind::Identity, b"("),
    transform(b"", Kind::UppercaseAll, b" "),
    transform(b"", Kind::UppercaseFirst, b"\">"),
    transform(b"", Kind::Identity, b"=\""),
    transform(b" ", Kind::Identity, b"."),
    transform(b".com/", Kind::Identity, b""),
    transform(b" the ", Kind::Identity, b" of the "),
    transform(b"", Kind::UppercaseFirst, b"'"),
    transform(b"", Kind::Identity, b". This "),
    transform(b"", Kind::Identity, b","),
    transform(b".", Kind::Identity, b" "),
    transform(b"", Kind::UppercaseFirst, b"("),
    transform(b"", Kind::UppercaseFirst, b"."),
    transform(b"", Kind::Identity, b" not "),
    transform(b" ", Kind::Identity, b"=\""),
    transform(b"", Kind::Identity, b"er "),
    transform(b" ", Kind::UppercaseAll, b" "),
    transform(b"", Kind::Identity, b"al "),
    transform(b" ", Kind::UppercaseAll, b""),
    transform(b"", Kind::Identity, b"='"),
    transform(b"", Kind::UppercaseAll, b"\""),
    transform(b"", Kind::UppercaseFirst, b". "),
    transform(b" ", Kind::Identity, b"("),
    transform(b"", Kind::Identity, b"ful "),
    transform(b" ", Kind::UppercaseFirst, b". "),
    transform(b"", Kind::Identity, b"ive "),
    transform(b"", Kind::Identity, b"less "),
    transform(b"", Kind::UppercaseAll, b"'"),
    transform(b"", Kind::Identity, b"est "),
    transform(b" ", Kind::UppercaseFirst, b"."),
    transform(b"", Kind::UppercaseAll, b"\">"),
    transform(b" ", Kind::Identity, b"='"),
    transform(b"", Kind::UppercaseFirst, b","),
    transform(b"", Kind::Identity, b"ize "),
    transform(b"", Kind::UppercaseAll, b"."),
    transform(b"\xc2\xa0", Kind::Identity, b""),
    transform(b" ", Kind::Identity, b","),
    transform(b"", Kind::UppercaseFirst, b"=\""),
    transform(b"", Kind::UppercaseAll, b"=\""),
    transform(b"", Kind::Identity, b"ous "),
    transform(b"", Kind::UppercaseAll, b", "),
    transform(b"", Kind::UppercaseFirst, b"='"),
    transform(b" ", Kind::UppercaseFirst, b","),
    transform(b" ", Kind::UppercaseAll, b"=\""),
    transform(b" ", Kind::UppercaseAll, b", "),
    transform(b"", Kind::UppercaseAll, b","),
    transform(b"", Kind::UppercaseAll, b"("),
    transform(b"", Kind::UppercaseAll, b". "),
    transform(b" ", Kind::UppercaseAll, b"."),
    transform(b"", Kind::UppercaseAll, b"='"),
    transform(b" ", Kind::UppercaseAll, b". "),
    transform(b" ", Kind::UppercaseFirst, b"=\""),
    transform(b" ", Kind::UppercaseAll, b"='"),
    transform(b" ", Kind::UppercaseFirst, b"='"),
];

const fn word_bytes_max() -> usize {
    let mut affixes = 0;
    let mut index = 0;
    while index < TRANSFORMS.len() {
        let length = TRANSFORMS[index].prefix.len() + TRANSFORMS[index].suffix.len();
        if length > affixes {
            affixes = length;
        }
        index += 1;
    }
    affixes + WORD_LENGTH_MAX as usize
}

pub(super) const WORD_BYTES_MAX: usize = word_bytes_max();

#[derive(Clone, Copy)]
pub(super) struct Word {
    pub bytes: [u8; WORD_BYTES_MAX],
    pub length: u8,
}

impl Word {
    pub(super) const EMPTY: Self = Self { bytes: [0; WORD_BYTES_MAX], length: 0 };
}

pub(super) fn word(length: u32, word_id: u32) -> Result<Word, DecodeError> {
    assert!(length >= WORD_LENGTH_MIN);
    assert!(length <= WORD_LENGTH_MAX);
    let count_bits = u32::from(WORD_COUNT_BITS[length as usize]);
    let index = (u64::from(word_id) & low_mask(count_bits)) as usize;
    let Some(transform) = TRANSFORMS.get((word_id >> count_bits) as usize) else {
        return Err(DecodeError::InvalidTransform);
    };
    let start = WORD_OFFSETS[length as usize] + index * length as usize;
    let source = &DICTIONARY[start..start + length as usize];
    let body = match transform.kind {
        Kind::OmitFirst(count) => &source[count.min(source.len())..],
        Kind::OmitLast(count) => &source[..source.len().saturating_sub(count)],
        Kind::Identity | Kind::UppercaseFirst | Kind::UppercaseAll => source,
    };
    let mut bytes = [0; WORD_BYTES_MAX];
    let prefix_end = transform.prefix.len();
    let body_end = prefix_end + body.len();
    let end = body_end + transform.suffix.len();
    bytes[..prefix_end].copy_from_slice(transform.prefix);
    bytes[prefix_end..body_end].copy_from_slice(body);
    match transform.kind {
        Kind::UppercaseFirst => uppercase(&mut bytes[prefix_end..body_end], false),
        Kind::UppercaseAll => uppercase(&mut bytes[prefix_end..body_end], true),
        Kind::Identity | Kind::OmitFirst(_) | Kind::OmitLast(_) => {}
    }
    bytes[body_end..end].copy_from_slice(transform.suffix);
    Ok(Word {
        bytes,
        length: end as u8,
    })
}

fn uppercase(body: &mut [u8], all: bool) {
    let mut index = 0;
    while index < body.len() {
        let first = body[index];
        let step = if first < UPPERCASE_ASCII_LIMIT {
            if first.is_ascii_lowercase() {
                body[index] ^= ASCII_CASE_BIT;
            }
            1
        } else if first < UPPERCASE_TWO_BYTE_LIMIT {
            if let Some(byte) = body.get_mut(index + 1) {
                *byte ^= UPPERCASE_TWO_BYTE_FLIP;
            }
            2
        } else {
            if let Some(byte) = body.get_mut(index + 2) {
                *byte ^= UPPERCASE_THREE_BYTE_FLIP;
            }
            3
        };
        if !all {
            return;
        }
        index += step;
    }
}
