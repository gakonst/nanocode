use alloc::string::String;
use alloc::vec::Vec;

use crate::ParseBigIntError;
use crate::limbs::{self, LIMB_BITS, LIMB_BITS_U64, Limb};

const RADIX_MIN: u32 = 2;
const RADIX_MAX: u32 = 36;
const DIVIDE_AND_CONQUER_LIMBS_MIN: usize = 32;
const PARSE_DIVIDE_AND_CONQUER_CHUNKS_MIN: usize = 1024;
const DECIMAL_RADIX: Limb = 10;
const DIGITS: &[u8; RADIX_MAX as usize] = b"0123456789abcdefghijklmnopqrstuvwxyz";

#[derive(Clone, Copy)]
struct Chunking {
    radix: Limb,
    digits_per_limb: usize,
    limb_base: Limb,
}

impl Chunking {
    fn new(radix: u32) -> Self {
        assert!((RADIX_MIN..=RADIX_MAX).contains(&radix), "base out of range");
        let radix = Limb::from(radix);
        let mut digits_per_limb = 1;
        let mut limb_base = radix;
        while let Some(next) = limb_base.checked_mul(radix) {
            limb_base = next;
            digits_per_limb += 1;
        }
        Self {
            radix,
            digits_per_limb,
            limb_base,
        }
    }
}

pub(crate) fn digit_value(byte: u8) -> Option<u32> {
    char::from(byte).to_digit(RADIX_MAX)
}

pub(crate) fn to_string(value: &[Limb], radix: u32) -> String {
    let digits = to_digits(value, radix);
    let mut text = String::with_capacity(digits.len());
    text.extend(digits.iter().map(|&digit| char::from(DIGITS[usize::from(digit)])));
    text
}

pub(crate) fn to_digits(value: &[Limb], radix: u32) -> Vec<u8> {
    assert!((RADIX_MIN..=RADIX_MAX).contains(&radix), "base out of range");
    if value.is_empty() {
        return alloc::vec![0];
    }
    if radix.is_power_of_two() {
        return to_digits_power_of_two(value, radix.trailing_zeros());
    }
    let chunking = Chunking::new(radix);
    let mut digits = Vec::new();
    if value.len() < DIVIDE_AND_CONQUER_LIMBS_MIN {
        write_digits_simple(value, chunking, &mut digits, None);
        return digits;
    }
    let mut powers = alloc::vec![alloc::vec![chunking.limb_base]];
    while 2 * powers[powers.len() - 1].len() <= value.len() {
        let last = &powers[powers.len() - 1];
        let next = limbs::square(last);
        powers.push(next);
    }
    write_digits_recursive(value, chunking, &powers, &mut digits, None);
    digits
}

fn to_digits_power_of_two(value: &[Limb], bits_per_digit: u32) -> Vec<u8> {
    let bits_per_digit = u64::from(bits_per_digit);
    let count = limbs::bits(value).div_ceil(bits_per_digit);
    let mask: Limb = (1 << bits_per_digit) - 1;
    let mut digits = Vec::with_capacity(count as usize);
    for index in (0..count).rev() {
        let start = index * bits_per_digit;
        let limb_index = (start / LIMB_BITS_U64) as usize;
        let offset = start % LIMB_BITS_U64;
        let mut digit = value[limb_index] >> offset;
        if offset + bits_per_digit > LIMB_BITS_U64
            && let Some(&next) = value.get(limb_index + 1)
        {
            digit |= next << (LIMB_BITS_U64 - offset);
        }
        digits.push((digit & mask) as u8);
    }
    digits
}

fn write_digits_simple(value: &[Limb], chunking: Chunking, digits: &mut Vec<u8>, width: Option<usize>) {
    let start = digits.len();
    let divisor = limbs::LimbDivisor::new(chunking.limb_base);
    let mut rest = value.to_vec();
    while !rest.is_empty() {
        let chunk = limbs::div_rem_limb_in_place_with(&mut rest, &divisor);
        let count = if rest.is_empty() && width.is_none() {
            0
        } else {
            chunking.digits_per_limb
        };
        if chunking.radix == DECIMAL_RADIX {
            push_chunk_digits::<DECIMAL_RADIX>(digits, chunk, count);
        } else {
            push_chunk_digits_radix(digits, chunk, count, chunking.radix);
        }
    }
    if let Some(width) = width {
        assert!(digits.len() - start <= width);
        digits.resize(start + width, 0);
    }
    digits[start..].reverse();
}

fn push_chunk_digits<const RADIX: Limb>(digits: &mut Vec<u8>, mut chunk: Limb, count: usize) {
    let mut written = 0;
    while chunk != 0 || written < count {
        digits.push((chunk % RADIX) as u8);
        chunk /= RADIX;
        written += 1;
    }
}

fn push_chunk_digits_radix(digits: &mut Vec<u8>, mut chunk: Limb, count: usize, radix: Limb) {
    let mut written = 0;
    while chunk != 0 || written < count {
        digits.push((chunk % radix) as u8);
        chunk /= radix;
        written += 1;
    }
}

fn write_digits_recursive(
    value: &[Limb],
    chunking: Chunking,
    powers: &[Vec<Limb>],
    digits: &mut Vec<u8>,
    width: Option<usize>,
) {
    if value.len() < DIVIDE_AND_CONQUER_LIMBS_MIN {
        if value.is_empty() {
            if let Some(width) = width {
                digits.resize(digits.len() + width, 0);
            }
            return;
        }
        write_digits_simple(value, chunking, digits, width);
        return;
    }
    let mut level = powers.len() - 1;
    while level > 0 && 2 * powers[level].len() > value.len() + 1 {
        level -= 1;
    }
    let low_width = chunking.digits_per_limb << level;
    let (high, low) = limbs::div_rem(value, &powers[level]);
    let high_width = width.map(|width| {
        assert!(width >= low_width);
        width - low_width
    });
    write_digits_recursive(&high, chunking, powers, digits, high_width);
    write_digits_recursive(&low, chunking, powers, digits, Some(low_width));
}

pub(crate) fn parse_natural(text: &str, radix: u32) -> Result<Vec<Limb>, ParseBigIntError> {
    let mut text = text;
    if let Some(tail) = text.strip_prefix('+')
        && !tail.starts_with('+')
    {
        text = tail;
    }
    if let Some(value) = parse_digits_text(text.as_bytes(), radix) {
        return Ok(value);
    }
    if text.is_empty() {
        return Err(ParseBigIntError::empty());
    }
    if text.starts_with('_') {
        return Err(ParseBigIntError::invalid());
    }
    let filtered: Vec<u8> = text.bytes().filter(|&byte| byte != b'_').collect();
    parse_digits_text(&filtered, radix).ok_or_else(ParseBigIntError::invalid)
}

fn parse_digits_text(text: &[u8], radix: u32) -> Option<Vec<Limb>> {
    assert!((RADIX_MIN..=RADIX_MAX).contains(&radix), "base out of range");
    let mut text = text;
    if let Some((&b'+', rest)) = text.split_first() {
        if matches!(rest.first(), Some(b'+' | b'-')) {
            return None;
        }
        text = rest;
    }
    if text.is_empty() {
        return None;
    }
    let mut digits = Vec::with_capacity(text.len());
    for &byte in text {
        let digit = digit_value(byte)?;
        if digit >= radix {
            return None;
        }
        digits.push(digit as u8);
    }
    Some(from_digits(&digits, radix))
}

pub(crate) fn from_digits(digits: &[u8], radix: u32) -> Vec<Limb> {
    if radix.is_power_of_two() {
        return from_digits_power_of_two(digits, radix.trailing_zeros());
    }
    let chunking = Chunking::new(radix);
    let chunks = digits.len().div_ceil(chunking.digits_per_limb);
    if chunks < PARSE_DIVIDE_AND_CONQUER_CHUNKS_MIN {
        return from_digits_simple(digits, chunking);
    }
    let mut powers = alloc::vec![alloc::vec![chunking.limb_base]];
    while (chunking.digits_per_limb << powers.len()) < digits.len() {
        let last = &powers[powers.len() - 1];
        let next = limbs::square(last);
        powers.push(next);
    }
    from_digits_recursive(digits, chunking, &powers)
}

fn from_digits_power_of_two(digits: &[u8], bits_per_digit: u32) -> Vec<Limb> {
    let mut limbs_out = alloc::vec![0; (digits.len() * bits_per_digit as usize).div_ceil(LIMB_BITS as usize)];
    for (index, &digit) in digits.iter().rev().enumerate() {
        let start = index as u64 * u64::from(bits_per_digit);
        let limb_index = (start / LIMB_BITS_U64) as usize;
        let offset = start % LIMB_BITS_U64;
        let digit = Limb::from(digit);
        limbs_out[limb_index] |= digit << offset;
        if offset + u64::from(bits_per_digit) > LIMB_BITS_U64 {
            limbs_out[limb_index + 1] |= digit >> (LIMB_BITS_U64 - offset);
        }
    }
    limbs::normalize(&mut limbs_out);
    limbs_out
}

fn chunk_value(digits: &[u8], radix: Limb) -> Limb {
    digits.iter().fold(0, |value, &digit| value * radix + Limb::from(digit))
}

fn from_digits_simple(digits: &[u8], chunking: Chunking) -> Vec<Limb> {
    let first = match digits.len() % chunking.digits_per_limb {
        0 => chunking.digits_per_limb.min(digits.len()),
        partial => partial,
    };
    let mut value = Vec::with_capacity(digits.len().div_ceil(chunking.digits_per_limb));
    let (head, tail) = digits.split_at(first);
    let head_value = chunk_value(head, chunking.radix);
    if head_value != 0 {
        value.push(head_value);
    }
    for chunk in tail.chunks(chunking.digits_per_limb) {
        let addend = chunk_value(chunk, chunking.radix);
        if value.is_empty() {
            if addend != 0 {
                value.push(addend);
            }
            continue;
        }
        limbs::mul_limb_add_assign(&mut value, chunking.limb_base, addend);
    }
    value
}

fn from_digits_recursive(digits: &[u8], chunking: Chunking, powers: &[Vec<Limb>]) -> Vec<Limb> {
    if digits.len().div_ceil(chunking.digits_per_limb) < PARSE_DIVIDE_AND_CONQUER_CHUNKS_MIN {
        return from_digits_simple(digits, chunking);
    }
    let mut level = powers.len() - 1;
    while (chunking.digits_per_limb << level) >= digits.len() {
        level -= 1;
    }
    let low_length = chunking.digits_per_limb << level;
    let (high_digits, low_digits) = digits.split_at(digits.len() - low_length);
    let high = from_digits_recursive(high_digits, chunking, powers);
    let low = from_digits_recursive(low_digits, chunking, powers);
    let mut value = limbs::mul(&high, &powers[level]);
    if value.len() < low.len() {
        value.resize(low.len(), 0);
    }
    if limbs::add_assign(&mut value, &low) {
        value.push(1);
    }
    limbs::normalize(&mut value);
    value
}


