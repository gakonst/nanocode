// Copyright 2015 Brian Smith.
//
// Permission to use, copy, modify, and/or distribute this software for any
// purpose with or without fee is hereby granted, provided that the above
// copyright notice and this permission notice appear in all copies.
//
// THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHORS DISCLAIM ALL WARRANTIES
// WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
// MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHORS BE LIABLE FOR
// ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
// WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
// ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
// OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.

use alloc::vec::Vec;
use core::marker::PhantomData;

use crate::{Error, error::DerTypeId};

/// Iterator to parse a sequence of DER-encoded values of type `T`.
#[derive(Debug)]
pub struct DerIterator<'a, T> {
    reader: crate::untrusted::Reader<'a>,
    marker: PhantomData<T>,
}

impl<'a, T> DerIterator<'a, T> {
    /// [`DerIterator`] will consume all of the bytes in `input` reading values of type `T`.
    pub(crate) fn new(input: crate::untrusted::Input<'a>) -> Self {
        Self {
            reader: crate::untrusted::Reader::new(input),
            marker: PhantomData,
        }
    }
}

impl<'a, T: FromDer<'a>> Iterator for DerIterator<'a, T> {
    type Item = Result<T, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        (!self.reader.at_end()).then(|| T::from_der(&mut self.reader))
    }
}

pub(crate) trait FromDer<'a>: Sized + 'a {
    /// Parse a value of type `Self` from the given DER-encoded input.
    fn from_der(reader: &mut crate::untrusted::Reader<'a>) -> Result<Self, Error>;

    const TYPE_ID: DerTypeId;
}

pub(crate) fn read_all<'a, T: FromDer<'a>>(input: crate::untrusted::Input<'a>) -> Result<T, Error> {
    input.read_all(Error::TrailingData(T::TYPE_ID), T::from_der)
}

// Copied (and extended) from ring's src/der.rs
#[allow(clippy::upper_case_acronyms)]
#[derive(Clone, Copy, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum Tag {
    Boolean = 0x01,
    Integer = 0x02,
    BitString = 0x03,
    OctetString = 0x04,
    OID = 0x06,
    Enum = 0x0A,
    Sequence = CONSTRUCTED | 0x10, // 0x30
    UTCTime = 0x17,
    GeneralizedTime = 0x18,

    #[allow(clippy::identity_op)]
    ContextSpecificConstructed0 = CONTEXT_SPECIFIC | CONSTRUCTED | 0,
    ContextSpecificConstructed1 = CONTEXT_SPECIFIC | CONSTRUCTED | 1,
    ContextSpecificConstructed3 = CONTEXT_SPECIFIC | CONSTRUCTED | 3,
}

pub(crate) const CONSTRUCTED: u8 = 0x20;
pub(crate) const CONTEXT_SPECIFIC: u8 = 0x80;

impl From<Tag> for usize {
    #[allow(clippy::as_conversions)]
    fn from(tag: Tag) -> Self {
        tag as Self
    }
}

impl From<Tag> for u8 {
    #[allow(clippy::as_conversions)]
    fn from(tag: Tag) -> Self {
        tag as Self
    } // XXX: narrowing conversion.
}

#[inline(always)]
pub(crate) fn expect_tag_and_get_value_limited<'a>(
    input: &mut crate::untrusted::Reader<'a>,
    tag: Tag,
    size_limit: usize,
) -> Result<crate::untrusted::Input<'a>, Error> {
    let (actual_tag, inner) = read_tag_and_get_value_limited(input, size_limit)?;
    if usize::from(tag) != usize::from(actual_tag) {
        return Err(Error::BadDer);
    }
    Ok(inner)
}

pub(crate) fn nested_limited<'a, R>(
    input: &mut crate::untrusted::Reader<'a>,
    tag: Tag,
    error: Error,
    decoder: impl FnOnce(&mut crate::untrusted::Reader<'a>) -> Result<R, Error>,
    size_limit: usize,
) -> Result<R, Error> {
    match expect_tag_and_get_value_limited(input, tag, size_limit) {
        Ok(value) => value.read_all(error, decoder),
        Err(_) => Err(error),
    }
}

// TODO: investigate taking decoder as a reference to reduce generated code
// size.
pub(crate) fn nested<'a, R>(
    input: &mut crate::untrusted::Reader<'a>,
    tag: Tag,
    error: Error,
    decoder: impl FnOnce(&mut crate::untrusted::Reader<'a>) -> Result<R, Error>,
) -> Result<R, Error> {
    nested_limited(input, tag, error, decoder, TWO_BYTE_DER_SIZE)
}

pub(crate) fn expect_tag<'a>(
    input: &mut crate::untrusted::Reader<'a>,
    tag: Tag,
) -> Result<crate::untrusted::Input<'a>, Error> {
    let (actual_tag, value) = read_tag_and_get_value_limited(input, TWO_BYTE_DER_SIZE)?;
    if usize::from(tag) != usize::from(actual_tag) {
        return Err(Error::BadDer);
    }

    Ok(value)
}

#[inline(always)]
pub(crate) fn read_tag_and_get_value<'a>(
    input: &mut crate::untrusted::Reader<'a>,
) -> Result<(u8, crate::untrusted::Input<'a>), Error> {
    read_tag_and_get_value_limited(input, TWO_BYTE_DER_SIZE)
}

#[inline(always)]
pub(crate) fn read_tag_and_get_value_limited<'a>(
    input: &mut crate::untrusted::Reader<'a>,
    size_limit: usize,
) -> Result<(u8, crate::untrusted::Input<'a>), Error> {
    let tag = input.read_byte().map_err(end_of_input_err)?;
    if (tag & HIGH_TAG_RANGE_START) == HIGH_TAG_RANGE_START {
        return Err(Error::BadDer); // High tag number form is not allowed.
    }

    // If the high order bit of the first byte is set to zero then the length
    // is encoded in the seven remaining bits of that byte. Otherwise, those
    // seven bits represent the number of bytes used to encode the length.
    let length = match input.read_byte().map_err(end_of_input_err)? {
        n if (n & SHORT_FORM_LEN_MAX) == 0 => usize::from(n),
        LONG_FORM_LEN_ONE_BYTE => {
            let length_byte = input.read_byte().map_err(end_of_input_err)?;
            if length_byte < SHORT_FORM_LEN_MAX {
                return Err(Error::BadDer); // Not the canonical encoding.
            }
            usize::from(length_byte)
        }
        LONG_FORM_LEN_TWO_BYTES => {
            let length_byte_one = usize::from(input.read_byte().map_err(end_of_input_err)?);
            let length_byte_two = usize::from(input.read_byte().map_err(end_of_input_err)?);
            let combined = (length_byte_one << 8) | length_byte_two;
            if combined <= LONG_FORM_LEN_ONE_BYTE_MAX {
                return Err(Error::BadDer); // Not the canonical encoding.
            }
            combined
        }
        LONG_FORM_LEN_THREE_BYTES => {
            let length_byte_one = usize::from(input.read_byte().map_err(end_of_input_err)?);
            let length_byte_two = usize::from(input.read_byte().map_err(end_of_input_err)?);
            let length_byte_three = usize::from(input.read_byte().map_err(end_of_input_err)?);
            let combined = (length_byte_one << 16) | (length_byte_two << 8) | length_byte_three;
            if combined <= LONG_FORM_LEN_TWO_BYTES_MAX {
                return Err(Error::BadDer); // Not the canonical encoding.
            }
            combined
        }
        LONG_FORM_LEN_FOUR_BYTES => {
            let length_byte_one = usize::from(input.read_byte().map_err(end_of_input_err)?);
            let length_byte_two = usize::from(input.read_byte().map_err(end_of_input_err)?);
            let length_byte_three = usize::from(input.read_byte().map_err(end_of_input_err)?);
            let length_byte_four = usize::from(input.read_byte().map_err(end_of_input_err)?);
            let combined = (length_byte_one << 24)
                | (length_byte_two << 16)
                | (length_byte_three << 8)
                | length_byte_four;
            if combined <= LONG_FORM_LEN_THREE_BYTES_MAX {
                return Err(Error::BadDer); // Not the canonical encoding.
            }
            combined
        }
        _ => {
            return Err(Error::BadDer); // We don't support longer lengths.
        }
    };

    if length >= size_limit {
        return Err(Error::BadDer); // The length is larger than the caller accepts.
    }

    let inner = input.read_bytes(length).map_err(end_of_input_err)?;
    Ok((tag, inner))
}

/// Prepend `bytes` with the given ASN.1 [`Tag`] and appropriately encoded length byte(s).
/// Useful for "adding back" ASN.1 bytes to parsed content.
#[allow(clippy::as_conversions)]
pub(crate) fn asn1_wrap(tag: Tag, bytes: &[u8]) -> Vec<u8> {
    let len = bytes.len();
    // The length is encoded differently depending on how many bytes there are
    if len < usize::from(SHORT_FORM_LEN_MAX) {
        // Short form: the length is encoded using a single byte
        // Contents: Tag byte, single length byte, and passed bytes
        let mut ret = Vec::with_capacity(2 + len);
        ret.push(tag.into()); // Tag byte
        ret.push(len as u8); // Single length byte
        ret.extend_from_slice(bytes); // Passed bytes
        ret
    } else {
        // Long form: The length is encoded using multiple bytes
        // Contents: Tag byte, number-of-length-bytes byte, length bytes, and passed bytes
        // The first byte indicates how many more bytes will be used to encode the length
        // First, get a big-endian representation of the byte slice's length
        let size = len.to_be_bytes();
        // Find the number of leading empty bytes in that representation
        // This will determine the smallest number of bytes we need to encode the length
        let leading_zero_bytes = size
            .iter()
            .position(|&byte| byte != 0)
            .unwrap_or(size.len());
        assert!(leading_zero_bytes < size.len());
        // Number of bytes used - number of not needed bytes = smallest number needed
        let encoded_bytes = size.len() - leading_zero_bytes;
        let mut ret = Vec::with_capacity(2 + encoded_bytes + len);
        // Indicate this is a number-of-length-bytes byte by setting the high order bit
        let number_of_length_bytes_byte = SHORT_FORM_LEN_MAX + encoded_bytes as u8;
        ret.push(tag.into()); // Tag byte
        ret.push(number_of_length_bytes_byte); // Number-of-length-bytes byte
        ret.extend_from_slice(&size[leading_zero_bytes..]); // Length bytes
        ret.extend_from_slice(bytes); // Passed bytes
        ret
    }
}

// Long-form DER encoded lengths of two bytes can express lengths up to the following limit.
//
// The upstream ring::io::der::read_tag_and_get_value() function limits itself to up to two byte
// long-form DER lengths, and so this limit represents the maximum length that was possible to
// read before the introduction of the read_tag_and_get_value_limited function.
pub(crate) const TWO_BYTE_DER_SIZE: usize = LONG_FORM_LEN_TWO_BYTES_MAX;

// The maximum size of a DER value that Webpki can support reading.
//
// Webpki limits itself to four byte long-form DER lengths, and so this limit represents
// the maximum size tagged DER value that can be read for any purpose.
pub(crate) const MAX_DER_SIZE: usize = LONG_FORM_LEN_FOUR_BYTES_MAX;

// DER Tag identifiers have two forms:
// * Low tag number form (for tags values in the range [0..30]
// * High tag number form (for tag values in the range [31..]
// We only support low tag number form.
const HIGH_TAG_RANGE_START: u8 = 31;

// DER length octets have two forms:
// * Short form: 1 octet supporting lengths between 0 and 127.
// * Long definite form: 2 to 127 octets, number of octets encoded into first octet.
const SHORT_FORM_LEN_MAX: u8 = 128;

// Leading octet for long definite form DER length expressed in second byte.
const LONG_FORM_LEN_ONE_BYTE: u8 = 0x81;

// Maximum size that can be expressed in a one byte long form len.
const LONG_FORM_LEN_ONE_BYTE_MAX: usize = 0xff;

// Leading octet for long definite form DER length expressed in subsequent two bytes.
const LONG_FORM_LEN_TWO_BYTES: u8 = 0x82;

// Maximum size that can be expressed in a two byte long form len.
const LONG_FORM_LEN_TWO_BYTES_MAX: usize = 0xff_ff;

// Leading octet for long definite form DER length expressed in subsequent three bytes.
const LONG_FORM_LEN_THREE_BYTES: u8 = 0x83;

// Maximum size that can be expressed in a three byte long form len.
const LONG_FORM_LEN_THREE_BYTES_MAX: usize = 0xff_ff_ff;

// Leading octet for long definite form DER length expressed in subsequent four bytes.
const LONG_FORM_LEN_FOUR_BYTES: u8 = 0x84;

// Maximum size that can be expressed in a four byte long form der len.
const LONG_FORM_LEN_FOUR_BYTES_MAX: usize = 0xff_ff_ff_ff;

// TODO: investigate taking decoder as a reference to reduce generated code
// size.
pub(crate) fn nested_of_mut<'a>(
    input: &mut crate::untrusted::Reader<'a>,
    outer_tag: Tag,
    inner_tag: Tag,
    error: Error,
    allow_empty: bool,
    mut decoder: impl FnMut(&mut crate::untrusted::Reader<'a>) -> Result<(), Error>,
) -> Result<(), Error> {
    nested(input, outer_tag, error.clone(), |outer| {
        if allow_empty && outer.at_end() {
            return Ok(());
        }
        loop {
            nested(outer, inner_tag, error.clone(), |inner| decoder(inner))?;
            if outer.at_end() {
                break;
            }
        }
        Ok(())
    })
}

pub(crate) fn bit_string_with_no_unused_bits<'a>(
    input: &mut crate::untrusted::Reader<'a>,
) -> Result<crate::untrusted::Input<'a>, Error> {
    nested(
        input,
        Tag::BitString,
        Error::TrailingData(DerTypeId::BitString),
        |value| {
            let unused_bits_at_end = value.read_byte().map_err(|_| Error::BadDer)?;
            if unused_bits_at_end != 0 {
                return Err(Error::BadDer);
            }
            Ok(value.read_bytes_to_end())
        },
    )
}

pub(crate) struct BitStringFlags<'a> {
    raw_bits: &'a [u8],
}

impl BitStringFlags<'_> {
    pub(crate) fn bit_set(&self, bit: usize) -> bool {
        let byte_index = bit / 8;
        let bit_shift = 7 - (bit % 8);

        if self.raw_bits.len() < (byte_index + 1) {
            false
        } else {
            ((self.raw_bits[byte_index] >> bit_shift) & 1) != 0
        }
    }
}

// ASN.1 BIT STRING fields for sets of flags are encoded in DER with some peculiar details related
// to padding. Notably this means we expect an indicator of the number of bits of padding, and then
// the actual bit values. See this Stack Overflow discussion[0], and ITU X690-0207[1] Section 8.6
// and Section 11.2 for more information.
//
// [0]: https://security.stackexchange.com/a/10396
// [1]: https://www.itu.int/ITU-T/studygroups/com17/languages/X.690-0207.pdf
pub(crate) fn bit_string_flags(input: crate::untrusted::Input<'_>) -> Result<BitStringFlags<'_>, Error> {
    input.read_all(Error::BadDer, |bit_string| {
        // ITU X690-0207 11.2:
        //   "The initial octet shall encode, as an unsigned binary integer with bit 1 as the least
        //   significant bit, the number of unused bits in the final subsequent octet.
        //   The number shall be in the range zero to seven"
        let padding_bits = bit_string.read_byte().map_err(|_| Error::BadDer)?;
        let raw_bits = bit_string.read_bytes_to_end().as_slice_less_safe();

        match (padding_bits, raw_bits.last()) {
            // It's illegal to have more than 7 bits of padding.
            (8.., _) => Err(Error::BadDer),

            // If the raw bitflags are empty there should be no padding.
            (0, None) => Ok(BitStringFlags { raw_bits }),
            (_, None) => Err(Error::BadDer),

            // If there are padding bits then the last bit of the last raw byte must be 0 or the
            // distinguished encoding rules are not being followed.
            (1..=7, Some(last)) if last & ((1 << padding_bits) - 1) != 0 => Err(Error::BadDer),

            (_, Some(_)) => Ok(BitStringFlags { raw_bits }),
        }
    })
}

impl<'a> FromDer<'a> for u8 {
    fn from_der(reader: &mut crate::untrusted::Reader<'a>) -> Result<Self, Error> {
        match *nonnegative_integer(reader)?.as_slice_less_safe() {
            [b] => Ok(b),
            _ => Err(Error::BadDer),
        }
    }

    const TYPE_ID: DerTypeId = DerTypeId::U8;
}

pub(crate) fn nonnegative_integer<'a>(
    input: &mut crate::untrusted::Reader<'a>,
) -> Result<crate::untrusted::Input<'a>, Error> {
    let value = expect_tag(input, Tag::Integer)?;
    match value
        .as_slice_less_safe()
        .split_first()
        .ok_or(Error::BadDer)?
    {
        // Zero or leading zero.
        (0, rest) => {
            match rest.first() {
                // Zero
                None => Ok(value),
                // Necessary leading zero.
                Some(&second) if second & 0x80 == 0x80 => Ok(crate::untrusted::Input::from(rest)),
                // Unnecessary leading zero.
                _ => Err(Error::BadDer),
            }
        }
        // Positive value with no leading zero.
        (first, _) if first & 0x80 == 0x00 => Ok(value),
        // Negative value.
        (_, _) => Err(Error::BadDer),
    }
}

pub(crate) fn end_of_input_err(_: crate::untrusted::EndOfInput) -> Error {
    Error::BadDer
}

// Like mozilla::pkix, we accept the nonconformant explicit encoding of
// the default value (false) for compatibility with real-world certificates.
impl<'a> FromDer<'a> for bool {
    fn from_der(reader: &mut crate::untrusted::Reader<'a>) -> Result<Self, Error> {
        if !reader.peek(Tag::Boolean.into()) {
            return Ok(false);
        }

        nested(
            reader,
            Tag::Boolean,
            Error::TrailingData(Self::TYPE_ID),
            |input| match input.read_byte() {
                Ok(0xff) => Ok(true),
                Ok(0x00) => Ok(false),
                _ => Err(Error::BadDer),
            },
        )
    }

    const TYPE_ID: DerTypeId = DerTypeId::Bool;
}

macro_rules! oid {
    ( $first:expr, $second:expr, $( $tail:expr ),* ) =>
    (
        [(40 * $first) + $second, $( $tail ),*]
    )
}

