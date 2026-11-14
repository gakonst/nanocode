// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

use core::fmt::Debug;
use core::marker::PhantomData;
use core::mem;
use core::mem::size_of;

macro_rules! _asn1_struct_ty(
    ([$context:literal] EXPLICIT $($itty:ident)+) => { $crate::high::asn1::Context<'a, $context, true, $crate::high::asn1::_asn1_struct_ty!($($itty)+)> };
    ([$context:literal] $($itty:ident)+) => { $crate::high::asn1::Context<'a, $context, false, $crate::high::asn1::_asn1_struct_ty!($($itty)+)> };
    (INTEGER) => { $crate::high::asn1::Integer<'a> };
    (OBJECT IDENTIFIER) => { $crate::high::asn1::ObjectId };
    (ANY OPTIONAL) => { Option<$crate::high::asn1::Any<'a>> };
    (OCTET STRING) => { $crate::high::asn1::OctetString<'a> };
    (BIT STRING) => { $crate::high::asn1::BitString<'a> };
    ($ty:tt REF) => { $ty<'a> };
    ($ty:tt) => { $ty };
);
pub(crate) use _asn1_struct_ty;

macro_rules! _asn1_struct_parse_ty(
    ($p:ident, [$context:literal] EXPLICIT $($itty:ident)+) => { $crate::high::asn1::Context::parse(&mut $p)? };
    ($p:ident, [$context:literal] $($itty:ident)+) => { $crate::high::asn1::Context::parse(&mut $p)? };
    ($p:ident, INTEGER) => { $crate::high::asn1::Integer::parse(&mut $p)? };
    ($p:ident, OBJECT IDENTIFIER) => { $crate::high::asn1::ObjectId::parse(&mut $p)? };
    ($p:ident, ANY OPTIONAL) => { Option::<$crate::high::asn1::Any<'_>>::parse(&mut $p)? };
    ($p:ident, OCTET STRING) => { $crate::high::asn1::OctetString::parse(&mut $p)? };
    ($p:ident, BIT STRING) => { $crate::high::asn1::BitString::parse(&mut $p)? };
    ($p:ident, $ty:tt REF) => { $ty::parse(&mut $p)? };
    ($p:ident, $ty:tt) => { $ty::parse(&mut $p)? };
);
pub(crate) use _asn1_struct_parse_ty;

macro_rules! asn1_struct {
    ($name:ident ::= SEQUENCE { $($itname:ident $([$context:literal])? $($itty:ident)+ ),+ }) => {
        #[allow(non_snake_case)]
        #[derive(Clone, Debug)]
        pub(crate) struct $name<'a> {
            $( pub(crate) $itname: $crate::high::asn1::_asn1_struct_ty!($([$context])? $($itty)+), )+
        }

        impl<'a> $name<'a> {
            #[allow(dead_code)]
            pub(crate) fn body_len(&self) -> usize {
                use $crate::high::asn1::Type;
                let mut result = 0;
                $( result += self.$itname.encoded_len(); )+
                result
            }
        }

        impl<'a> $crate::high::asn1::Type<'a> for $name<'a> {
            fn parse_with_tag(
                parser: &mut $crate::high::asn1::Parser<'a>,
                tag: $crate::high::asn1::Tag
            ) -> Result<Self, $crate::high::asn1::Error> {
                let (_, mut sub) = parser.descend(tag)?;
                let r = Self {
                $( $itname: $crate::high::asn1::_asn1_struct_parse_ty!(sub, $([$context])? $($itty)+ ), )+
                };
                sub.check_end()?;
                Ok(r)
            }

            fn encoded_len(&self) -> usize {
                $crate::high::asn1::encoded_length_for(self.body_len())
            }

            fn encode_with_tag(&self,
                encoder: &mut $crate::high::asn1::Encoder<'_>,
                tag: $crate::high::asn1::Tag
            ) -> Result<usize, $crate::high::asn1::Error> {
                let mut body = encoder.begin(tag, self.body_len())?;
                $( self.$itname.encode(&mut body)?; )+
                Ok(body.finish())
            }

            const TAG: $crate::high::asn1::Tag = $crate::high::asn1::Tag($crate::high::asn1::Tag::SEQUENCE);
        }
    }
}

macro_rules! asn1_enum {
    ($name:ident ::= INTEGER { $( $vname:ident($num:expr) ),+ }) => {
        #[allow(non_camel_case_types)]
        #[derive(Copy, Clone, Debug, PartialEq)]
        pub(crate) enum $name {
            $( $vname = $num, )+
        }

        impl $crate::high::asn1::Type<'_> for $name {
            fn parse_with_tag(p: &mut $crate::high::asn1::Parser<'_>, tag: $crate::high::asn1::Tag) -> Result<Self, $crate::high::asn1::Error> {
                match $crate::high::asn1::Integer::parse_with_tag(p, tag).and_then(|i| i.as_usize())? {
                    $( $num => Ok(Self::$vname), )+
                    _ => Err($crate::high::asn1::Error::UnhandledEnumValue),
                }
            }

            fn encode_with_tag(&self,
                encoder: &mut $crate::high::asn1::Encoder<'_>,
                tag: $crate::high::asn1::Tag
            ) -> Result<usize, $crate::high::asn1::Error> {
                let value = *self as usize;
                let bytes = value.to_be_bytes();
                $crate::high::asn1::Integer::new(&bytes).encode_with_tag(encoder, tag)
            }

            fn encoded_len(&self) -> usize {
                let value = *self as usize;
                let bytes = value.to_be_bytes();
                $crate::high::asn1::Integer::new(&bytes).encoded_len()
            }

            const TAG: $crate::high::asn1::Tag = $crate::high::asn1::Tag($crate::high::asn1::Tag::INTEGER);
        }
    }
}

macro_rules! asn1_oid_indices {
    ([$($accum:tt)*] -> ) => { [ $($accum,)* ] };
    ([$($accum:tt)*] -> $name:ident ($val:literal) $($rest:tt)*) => { asn1_oid_indices!([$($accum)* $val] -> $($rest)*) };
    ([$($accum:tt)*] -> $val:literal $($rest:tt)*) => { asn1_oid_indices!([$($accum)* $val] ->  $($rest)*) };
}

macro_rules! asn1_oid {
    ($name:ident OBJECT IDENTIFIER ::= { $( $item:tt )+ }) => {
        pub(crate) static $name: crate::high::asn1::ObjectId = crate::high::asn1::ObjectId::from_path(&asn1_oid_indices!( [] -> $( $item )+));
    }
}

pub(crate) trait Type<'a>: Debug + Sized {
    fn parse(p: &mut Parser<'a>) -> Result<Self, Error> {
        Self::parse_with_tag(p, Self::TAG)
    }
    fn parse_with_tag(p: &mut Parser<'a>, tag: Tag) -> Result<Self, Error>;

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<usize, Error> {
        self.encode_with_tag(encoder, Self::TAG)
    }
    fn encode_with_tag(&self, encoder: &mut Encoder<'_>, tag: Tag) -> Result<usize, Error>;
    fn encoded_len(&self) -> usize;

    fn from_bytes(b: &'a [u8]) -> Result<Self, Error> {
        let mut p = Parser::new(b);
        let t = Self::parse(&mut p)?;
        p.check_end()?;
        Ok(t)
    }

    const TAG: Tag;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Any<'a> {
    Null(Null),
    Integer(Integer<'a>),
    OctetString(OctetString<'a>),
    BitString(BitString<'a>),
    ObjectId(ObjectId),
}

impl<'a> Type<'a> for Any<'a> {
    fn parse_with_tag(p: &mut Parser<'a>, _: Tag) -> Result<Self, Error> {
        match p.peek_tag()?.0 {
            Tag::NULL => Ok(Self::Null(Null::parse(p)?)),
            Tag::INTEGER => Ok(Self::Integer(Integer::parse(p)?)),
            Tag::OCTET_STRING => Ok(Self::OctetString(OctetString::parse(p)?)),
            Tag::BIT_STRING => Ok(Self::BitString(BitString::parse(p)?)),
            Tag::OBJECT_ID => Ok(Self::ObjectId(ObjectId::parse(p)?)),
            _ => Err(Error::UnexpectedTag),
        }
    }

    fn encode_with_tag(&self, encoder: &mut Encoder<'_>, _: Tag) -> Result<usize, Error> {
        match self {
            Self::Null(n) => n.encode(encoder),
            Self::Integer(i) => i.encode(encoder),
            Self::BitString(bs) => bs.encode(encoder),
            Self::OctetString(os) => os.encode(encoder),
            Self::ObjectId(obj) => obj.encode(encoder),
        }
    }

    fn encoded_len(&self) -> usize {
        match self {
            Self::Null(n) => n.encoded_len(),
            Self::Integer(i) => i.encoded_len(),
            Self::BitString(bs) => bs.encoded_len(),
            Self::OctetString(os) => os.encoded_len(),
            Self::ObjectId(obj) => obj.encoded_len(),
        }
    }

    const TAG: Tag = Tag(0);
}

impl<'a, T: Type<'a>> Type<'a> for Option<T> {
    fn parse_with_tag(p: &mut Parser<'a>, tag: Tag) -> Result<Self, Error> {
        if p.left() > 0 {
            T::parse_with_tag(p, tag).map(Some)
        } else {
            Ok(None)
        }
    }

    fn encode_with_tag(&self, encoder: &mut Encoder<'_>, tag: Tag) -> Result<usize, Error> {
        match self {
            None => Ok(0),
            Some(a) => a.encode_with_tag(encoder, tag),
        }
    }

    fn encoded_len(&self) -> usize {
        match self {
            None => 0,
            Some(a) => a.encoded_len(),
        }
    }

    const TAG: Tag = T::TAG;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Context<'a, const ID: u8, const EXPLICIT: bool, T: Type<'a>>(
    Option<T>,
    PhantomData<&'a ()>,
);

impl<'a, const ID: u8, const EXPLICIT: bool, T: Type<'a>> Context<'a, ID, EXPLICIT, T> {
    pub(crate) fn absent() -> Self {
        Self(None, PhantomData)
    }

    pub(crate) fn inner(&self) -> &Option<T> {
        &self.0
    }
}

impl<'a, const ID: u8, const EXPLICIT: bool, T: Type<'a>> Type<'a>
    for Context<'a, ID, EXPLICIT, T>
{
    fn parse_with_tag(p: &mut Parser<'a>, tag: Tag) -> Result<Self, Error> {
        match p.peek_tag() {
            Ok(tt) if tag.acceptable(tt.0) => {}
            _ => return Ok(Self(None, PhantomData)),
        };

        let t = if EXPLICIT || tag.constructed() {
            let (_, mut sub) = p.descend(tag)?;
            let t = T::parse(&mut sub)?;
            sub.check_end()?;
            t
        } else {
            T::parse_with_tag(p, tag)?
        };
        Ok(Self(Some(t), PhantomData))
    }

    fn encode_with_tag(&self, encoder: &mut Encoder<'_>, tag: Tag) -> Result<usize, Error> {
        let item = match &self.0 {
            None => return Ok(0),
            Some(a) => a,
        };

        if EXPLICIT || tag.constructed() {
            let body_len = item.encoded_len();
            let mut body = encoder.begin(tag, body_len)?;
            item.encode(&mut body)?;
            Ok(body.finish())
        } else {
            item.encode_with_tag(encoder, tag)
        }
    }

    fn encoded_len(&self) -> usize {
        let item = match &self.0 {
            None => return 0,
            Some(a) => a,
        };

        if Self::TAG.constructed() {
            encoded_length_for(item.encoded_len())
        } else {
            item.encoded_len()
        }
    }

    const TAG: Tag = match EXPLICIT {
        true => Tag::context(ID, true),
        false => Tag::context(ID, T::TAG.constructed()),
    };
}

impl<'a, const ID: u8, const CONSTRUCTED: bool, T: Type<'a>> From<T>
    for Context<'a, ID, CONSTRUCTED, T>
{
    fn from(t: T) -> Self {
        Self(Some(t), PhantomData)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Null;

impl Type<'_> for Null {
    fn parse_with_tag(p: &mut Parser<'_>, tag: Tag) -> Result<Self, Error> {
        let (_, body) = p.take(tag)?;
        if !body.is_empty() {
            return Err(Error::IllegalNull);
        }
        Ok(Self)
    }

    fn encode_with_tag(&self, encoder: &mut Encoder<'_>, tag: Tag) -> Result<usize, Error> {
        Ok(encoder.begin(tag, 0)?.finish())
    }

    fn encoded_len(&self) -> usize {
        encoded_length_for(0)
    }

    const TAG: Tag = Tag(Tag::NULL);
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectId {
    buf: [u8; Self::MAX_LEN],
    used: usize,
}

impl ObjectId {
    const fn from_path(path: &[usize]) -> Self {
        let mut r = Self {
            buf: [0u8; Self::MAX_LEN],
            used: 0,
        };

        match path.len() {
            0 => {}
            1 => {
                r.buf[r.used] = path[0] as u8;
                r.used += 1;
            }
            _ => {
                r.buf[r.used] = (path[0] as u8) * 40 + (path[1] as u8);
                r.used += 1;

                let mut i = 2;
                loop {
                    if i == path.len() {
                        break;
                    }
                    let item = path[i];
                    i += 1;

                    if item < 0x7f {
                        r.buf[r.used] = item as u8;
                        r.used += 1;
                    } else {
                        let mut chunks = (item.ilog2() + 1).div_ceil(7);

                        while chunks > 1 {
                            chunks -= 1;
                            r.buf[r.used] = (item >> (chunks * 7)) as u8 | 0x80u8;
                            r.used += 1;
                        }

                        r.buf[r.used] = (item & 0x7f) as u8;
                        r.used += 1;
                    }
                }
            }
        }

        r
    }

    const MAX_LEN: usize = 16;
}

impl Type<'_> for ObjectId {
    fn parse_with_tag(p: &mut Parser<'_>, tag: Tag) -> Result<Self, Error> {
        let (_, body) = p.take(tag)?;
        if body.len() > Self::MAX_LEN {
            return Err(Error::UnsupportedLargeObjectId);
        }

        let mut buf = [0u8; Self::MAX_LEN];
        buf[..body.len()].copy_from_slice(body);
        Ok(Self {
            buf,
            used: body.len(),
        })
    }

    fn encode_with_tag(&self, encoder: &mut Encoder<'_>, tag: Tag) -> Result<usize, Error> {
        let mut body = encoder.begin(tag, self.used)?;
        body.append_slice(self.as_ref())?;
        Ok(body.finish())
    }

    fn encoded_len(&self) -> usize {
        encoded_length_for(self.used)
    }

    const TAG: Tag = Tag(Tag::OBJECT_ID);
}

impl AsRef<[u8]> for ObjectId {
    fn as_ref(&self) -> &[u8] {
        &self.buf[..self.used]
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Integer<'a> {
    twos_complement: &'a [u8],
}

impl<'a> Integer<'a> {
    /// Returns a positive, zero, or negative ASN.1 integer.
    ///
    /// If `bytes` is empty or only consists of zero bytes,
    /// then zero is returned.
    ///
    /// If the top bit of `bytes` is set, then a negative
    /// number is returned, with any excess leading 0xff
    /// bytes stripped.
    ///
    /// Otherwise, a positive number is returned, with any
    /// excess leading 0x00 bytes stripped.
    pub(crate) fn new(mut bytes: &'a [u8]) -> Self {
        static ZERO: &[u8] = &[0];

        if bytes.is_empty() || bytes.iter().all(|b| *b == 0x00) {
            return Integer {
                twos_complement: ZERO,
            };
        }

        let negative = bytes[0] & 0x80 == 0x80;

        if negative {
            while bytes.len() > 1 && bytes[0] == 0xff && bytes[1] & 0x80 == 0x80 {
                bytes = &bytes[1..];
            }
        } else {
            while bytes.len() > 1 && bytes[0] == 0x00 && bytes[1] & 0x80 == 0x00 {
                bytes = &bytes[1..];
            }
        }

        Integer {
            twos_complement: bytes,
        }
    }

    /// Returns a positive ASN.1 integer.
    ///
    /// `value` should be a big-endian aka. radix-256 integer.
    ///
    /// It may have leading zeroes, these are removed if needed.
    ///
    /// It may have the top bit set, this function will prepend
    /// a zero byte if needed.
    ///
    /// The number of leading zeros is deemed a public property.
    ///
    /// `buffer` must be at least 1 octet larger than `magnitude`.
    pub(crate) fn new_positive(buffer: &'a mut [u8], mut value: &'_ [u8]) -> Self {
        // strip leading zero bytes
        while !value.is_empty() && value[0] == 0x00 {
            value = &value[1..];
        }

        let buf_len = if !value.is_empty() && value[0] & 0x80 == 0x80 {
            buffer[0] = 0x00;
            buffer[1..value.len() + 1].copy_from_slice(value);
            value.len() + 1
        } else if value.is_empty() {
            buffer[0] = 0;
            1
        } else {
            buffer[..value.len()].copy_from_slice(value);
            value.len()
        };

        Self {
            twos_complement: &buffer[..buf_len],
        }
    }

    pub(crate) fn is_negative(&self) -> bool {
        self.twos_complement
            .first()
            .map(|b| b & 0x80 == 0x80)
            .unwrap_or_default()
    }

    pub(crate) fn as_usize(&self) -> Result<usize, Error> {
        if self.is_negative() || self.twos_complement.len() > size_of::<usize>() {
            return Err(Error::IntegerOutOfRange);
        }

        let mut bytes = [0u8; 8];
        bytes[8 - self.twos_complement.len()..].copy_from_slice(self.twos_complement);
        Ok(usize::from_be_bytes(bytes))
    }

    fn minimum_magnitude_check(twos_complement: &'a [u8]) -> Result<Self, Error> {
        // ref. https://www.itu.int/ITU-T/studygroups/com17/languages/X.690-0207.pdf

        // 8.3.1. "one or more octets"
        if twos_complement.is_empty() {
            return Err(Error::IntegerOutOfRange);
        }

        // 8.3.2. "... more than one octet, then the bits of
        // the first octet and bit 8 of the second octet
        // a) shall not be ones; and
        // b) shall not be zero"
        if let Some(first_two) = twos_complement.get(..2) {
            if first_two[0] == 0xff && first_two[1] & 0x80 == 0x80 {
                return Err(Error::IntegerOutOfRange);
            }

            if first_two[0] == 0x00 && first_two[1] & 0x80 == 0x00 {
                return Err(Error::IntegerOutOfRange);
            }
        }

        Ok(Self { twos_complement })
    }
}

impl<'a> AsRef<[u8]> for Integer<'a> {
    fn as_ref(&self) -> &'a [u8] {
        self.twos_complement
    }
}

impl<'a> Type<'a> for Integer<'a> {
    fn parse_with_tag(p: &mut Parser<'a>, tag: Tag) -> Result<Self, Error> {
        let (_, twos_complement) = p.take(tag)?;
        Self::minimum_magnitude_check(twos_complement)
    }

    fn encode_with_tag(&self, encoder: &mut Encoder<'_>, tag: Tag) -> Result<usize, Error> {
        let mut body = encoder.begin(tag, self.twos_complement.len())?;
        body.append_slice(self.twos_complement)?;
        Ok(body.finish())
    }

    fn encoded_len(&self) -> usize {
        encoded_length_for(self.twos_complement.len())
    }

    const TAG: Tag = Tag(Tag::INTEGER);
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OctetString<'a> {
    octets: &'a [u8],
}

impl<'a> OctetString<'a> {
    pub(crate) fn new(octets: &'a [u8]) -> Self {
        Self { octets }
    }

    pub(crate) fn as_octets(&self) -> &'a [u8] {
        self.octets
    }
}

impl<'a> Type<'a> for OctetString<'a> {
    fn parse_with_tag(p: &mut Parser<'a>, tag: Tag) -> Result<Self, Error> {
        let (_, octets) = p.take(tag)?;
        Ok(Self { octets })
    }

    fn encode_with_tag(&self, encoder: &mut Encoder<'_>, tag: Tag) -> Result<usize, Error> {
        let mut body = encoder.begin(tag, self.octets.len())?;
        body.append_slice(self.octets)?;
        Ok(body.finish())
    }

    fn encoded_len(&self) -> usize {
        encoded_length_for(self.octets.len())
    }

    const TAG: Tag = Tag(Tag::OCTET_STRING);
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BitString<'a> {
    octets: &'a [u8],
}

impl<'a> BitString<'a> {
    pub(crate) fn new(octets: &'a [u8]) -> Self {
        Self { octets }
    }

    pub(crate) fn as_octets(&self) -> &'a [u8] {
        self.octets
    }
}

impl<'a> Type<'a> for BitString<'a> {
    fn parse_with_tag(p: &mut Parser<'a>, tag: Tag) -> Result<Self, Error> {
        let (_, octets) = p.take(tag)?;
        let octets = match octets.split_first() {
            None => return Err(Error::UnexpectedEof),
            Some((&0, rest)) => rest,
            Some((_, _)) => {
                return Err(Error::UnhandledBitString);
            }
        };
        Ok(Self { octets })
    }

    fn encode_with_tag(&self, encoder: &mut Encoder<'_>, tag: Tag) -> Result<usize, Error> {
        let mut body = encoder.begin(tag, self.octets.len() + 1)?;
        body.push(0)?;
        body.append_slice(self.octets)?;
        Ok(body.finish())
    }

    fn encoded_len(&self) -> usize {
        encoded_length_for(self.octets.len() + 1)
    }

    const TAG: Tag = Tag(Tag::BIT_STRING);
}

pub(crate) struct Parser<'a> {
    input: &'a [u8],
}

impl<'a, 's> Parser<'a> {
    pub(crate) fn new(buf: &'a [u8]) -> Self {
        Self { input: buf }
    }

    fn take(&'s mut self, want_tag: Tag) -> Result<(Tag, &'a [u8]), Error> {
        let tag = self.one_byte()?;
        if !want_tag.acceptable(tag) {
            return Err(Error::UnexpectedTag);
        }

        let len = self.take_len()?;
        let body = self.input.get(..len).ok_or(Error::UnexpectedEof)?;
        let (_, rest) = self.input.split_at(len);
        self.input = rest;
        Ok((Tag::from(tag), body))
    }

    fn descend(&'s mut self, want_tag: Tag) -> Result<(Tag, Self), Error> {
        let (tag, body) = self.take(want_tag)?;
        Ok((tag, Parser::new(body)))
    }

    fn peek_tag(&'s mut self) -> Result<Tag, Error> {
        self.input
            .first()
            .cloned()
            .map(Tag)
            .ok_or(Error::UnexpectedEof)
    }

    fn left(&self) -> usize {
        self.input.len()
    }

    fn check_end(self) -> Result<(), Error> {
        if self.left() != 0 {
            Err(Error::UnexpectedTrailingData)
        } else {
            Ok(())
        }
    }

    #[inline]
    fn one_byte(&'s mut self) -> Result<u8, Error> {
        let (byte, rest) = self.input.split_first().ok_or(Error::UnexpectedEof)?;
        self.input = rest;
        Ok(*byte)
    }

    fn take_len(&'s mut self) -> Result<usize, Error> {
        let len_byte = self.one_byte()?;

        match len_byte {
            0x00..=0x7f => Ok(len_byte as usize),
            0x81 => {
                let len = self.one_byte()?;
                if len < 0x80 {
                    return Err(Error::NonCanonicalEncoding);
                }
                Ok(len as usize)
            }
            0x82 => {
                let len = ((self.one_byte()? as usize) << 8) | self.one_byte()? as usize;
                if len < 0x100 {
                    return Err(Error::NonCanonicalEncoding);
                }
                Ok(len)
            }
            _ => Err(Error::UnsupportedLargeObjectLength),
        }
    }
}

pub(crate) struct Encoder<'a> {
    out: &'a mut [u8],
    written: usize,
}

impl<'a, 's> Encoder<'a> {
    pub(crate) fn new(out: &'a mut [u8]) -> Self {
        Encoder { out, written: 0 }
    }

    fn push(&'s mut self, byte: u8) -> Result<(), Error> {
        if self.out.is_empty() {
            return Err(Error::UnexpectedEof);
        }
        let out = mem::take(&mut self.out);
        out[0] = byte;
        self.out = &mut out[1..];
        self.written += 1;
        Ok(())
    }

    fn append_slice(&'s mut self, bytes: &[u8]) -> Result<(), Error> {
        if self.out.len() < bytes.len() {
            return Err(Error::UnexpectedEof);
        }

        let out = mem::take(&mut self.out);
        out[..bytes.len()].copy_from_slice(bytes);
        self.out = &mut out[bytes.len()..];
        self.written += bytes.len();
        Ok(())
    }

    fn split(&'s mut self, len: usize) -> Result<Self, Error> {
        if self.out.len() < len {
            return Err(Error::UnexpectedEof);
        }

        let buffer = mem::take(&mut self.out);
        let (split, rest) = buffer.split_at_mut(len);
        self.out = rest;
        let before = self.written;
        self.written += len;
        Ok(Encoder {
            out: split,
            written: before,
        })
    }

    fn begin(&'s mut self, tag: Tag, body_len: usize) -> Result<Self, Error> {
        self.push(tag.0)?;

        match body_len {
            0..=0x7f => self.push(body_len as u8)?,
            _ => {
                let bytes = (body_len.ilog2() + 1).div_ceil(8) as usize;
                self.push(0x80 + bytes as u8)?;
                let len_encoded = body_len.to_be_bytes();
                for i in 0..bytes {
                    self.push(len_encoded[len_encoded.len() - bytes + i])?;
                }
            }
        }

        self.split(body_len)
    }

    fn finish(&mut self) -> usize {
        self.written
    }
}

fn encoded_length_for(len: usize) -> usize {
    const TAG: usize = 1;
    match len {
        0..=0x7f => TAG + 1 + len,
        0x80..=0xff => TAG + 1 + 1 + len,
        0x01_00..=0xff_ff => TAG + 1 + 2 + len,
        0x01_00_00..=0xff_ff_ff => TAG + 1 + 3 + len,
        0x01_00_00_00..=0xff_ff_ff_ff => TAG + 1 + 4 + len,
        _ => unimplemented!("extremely long asn1 object"),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    UnexpectedTag,
    UnexpectedEof,
    UnexpectedTrailingData,
    NonCanonicalEncoding,
    UnhandledEnumValue,
    IntegerOutOfRange,
    IllegalNull,
    UnsupportedLargeObjectId,
    UnsupportedLargeObjectLength,
    UnhandledBitString,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnexpectedTag => write!(f, "unexpected tag"),
            Self::UnexpectedEof => write!(f, "unexpected end of input"),
            Self::UnexpectedTrailingData => write!(f, "unexpected trailing data"),
            Self::NonCanonicalEncoding => write!(f, "non-canonical encoding"),
            Self::UnhandledEnumValue => write!(f, "unhandled enum value"),
            Self::IntegerOutOfRange => write!(f, "integer out of range"),
            Self::IllegalNull => write!(f, "illegal null"),
            Self::UnsupportedLargeObjectId => write!(f, "unsupported large object identifier"),
            Self::UnsupportedLargeObjectLength => write!(f, "unsupported large object length"),
            Self::UnhandledBitString => write!(f, "unhandled bit string"),
        }
    }
}

impl core::error::Error for Error {}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Tag(u8);

impl Tag {
    const fn acceptable(&self, offered: u8) -> bool {
        self.0 == offered
    }

    const fn constructed(&self) -> bool {
        matches!(self.0, Self::SEQUENCE) || self.0 & Self::CONSTRUCTED == Self::CONSTRUCTED
    }

    const fn context(id: u8, constructed: bool) -> Self {
        let constructed = match constructed {
            true => Self::CONSTRUCTED,
            false => 0,
        };
        Self(Self::CONTEXT_SPECIFIC | constructed | id)
    }

    const INTEGER: u8 = 0x02;
    const BIT_STRING: u8 = 0x03;
    const OCTET_STRING: u8 = 0x04;
    const NULL: u8 = 0x05;
    const OBJECT_ID: u8 = 0x06;

    const SEQUENCE: u8 = 0x30;

    const CONSTRUCTED: u8 = 0x20;
    const CONTEXT_SPECIFIC: u8 = 0x80;
}

impl From<u8> for Tag {
    fn from(b: u8) -> Self {
        Self(b)
    }
}

pub(crate) mod oid;
pub(crate) mod pkix;

