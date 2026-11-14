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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_integer_encoding() {
        check_integer_from_magnitude(&[0u8; 16], &[0x02, 0x01, 0x00]);
        check_integer_from_positive_magnitude(&[0u8; 16], &[0x02, 0x01, 0x00]);
        check_integer_from_magnitude(&[], &[0x02, 0x01, 0x00]);
        check_integer_from_positive_magnitude(&[], &[0x02, 0x01, 0x00]);
        check_integer_from_isize(0, &[0x02, 0x01, 0x00]);

        // negative sign contraction
        check_integer_from_magnitude(&[0xffu8; 32], &[0x02, 0x01, 0xff]);
        check_integer_from_magnitude(&[0xff, 0xff, 0x01], &[0x02, 0x02, 0xff, 0x01]);
        check_integer_from_isize(-1, &[0x02, 0x01, 0xff]);
        check_integer_from_isize(-127, &[0x02, 0x01, 0x81]);

        // positive zero constraction
        check_integer_from_magnitude(&[0x00, 0x00, 0x80], &[0x02, 0x02, 0x00, 0x80]);
        check_integer_from_magnitude(&[0x00, 0x00, 0x7f], &[0x02, 0x01, 0x7f]);
        check_integer_from_positive_magnitude(&[0x00, 0x00, 0x80], &[0x02, 0x02, 0x00, 0x80]);
        check_integer_from_positive_magnitude(&[0x00, 0x00, 0x7f], &[0x02, 0x01, 0x7f]);
        check_integer_from_isize(128, &[0x02, 0x02, 0x00, 0x80]);
        check_integer_from_isize(127, &[0x02, 0x01, 0x7f]);

        let mut buf = [0u8; 4];
        let i = Integer::new(&[0x01, 0x23]);
        assert_eq!(
            i.encode(&mut Encoder::new(&mut buf[..0])).unwrap_err(),
            Error::UnexpectedEof
        );
        assert_eq!(
            i.encode(&mut Encoder::new(&mut buf[..2])).unwrap_err(),
            Error::UnexpectedEof
        );
        assert_eq!(
            i.encode(&mut Encoder::new(&mut buf[..3])).unwrap_err(),
            Error::UnexpectedEof
        );
        assert_eq!(i.encode(&mut Encoder::new(&mut buf[..4])).unwrap(), 4);

        let mut ibuf = [0u8; 5];
        let i = Integer::new_positive(&mut ibuf, &[0xff]);
        assert_eq!(i.encode(&mut Encoder::new(&mut buf)).unwrap(), 4);
        let i = Integer::new_positive(&mut ibuf, &[0x00, 0x01]);
        assert_eq!(i.encode(&mut Encoder::new(&mut buf)).unwrap(), 3);
    }

    fn check_integer_from_magnitude(bytes: &[u8], encoding: &[u8]) {
        let mut buf = vec![0u8; encoding.len()];
        let len = Integer::new(bytes)
            .encode(&mut Encoder::new(&mut buf))
            .unwrap();
        buf.truncate(len);
        assert_eq!(&buf, encoding);
    }

    fn check_integer_from_positive_magnitude(bytes: &[u8], encoding: &[u8]) {
        let mut buf = vec![0u8; encoding.len()];
        let mut ibuf = vec![0u8; bytes.len() + 1];
        let len = Integer::new_positive(&mut ibuf, bytes)
            .encode(&mut Encoder::new(&mut buf))
            .unwrap();
        buf.truncate(len);
        assert_eq!(&buf, encoding);
    }

    fn check_integer_from_isize(num: isize, encoding: &[u8]) {
        let mut buf = vec![0u8; encoding.len()];
        let len = Integer::new(&num.to_be_bytes())
            .encode(&mut Encoder::new(&mut buf))
            .unwrap();
        buf.truncate(len);
        assert_eq!(&buf, encoding);
    }

    #[test]
    fn test_integer_as_usize() {
        assert_eq!(
            Integer::new(&usize::MIN.to_be_bytes()).as_usize().unwrap(),
            usize::MIN,
        );
        assert_eq!(
            Integer::new_positive(&mut [0x00; 8], &[0xff; 7])
                .as_usize()
                .unwrap(),
            0x00ffffff_ffffffff
        );
        assert_eq!(
            Integer::new_positive(&mut [0x00; 9], &[0xff; 8])
                .as_usize()
                .unwrap_err(),
            Error::IntegerOutOfRange
        );
    }

    #[test]
    fn test_invalid_integer_encodings() {
        // empty
        assert_eq!(
            Integer::from_bytes(&[0x02, 0x00]).unwrap_err(),
            Error::IntegerOutOfRange
        );

        // excess negative
        assert_eq!(
            Integer::from_bytes(&[0x02, 0x02, 0xff, 0xff]).unwrap_err(),
            Error::IntegerOutOfRange
        );

        // excess positive
        assert_eq!(
            Integer::from_bytes(&[0x02, 0x02, 0x00, 0x01]).unwrap_err(),
            Error::IntegerOutOfRange
        );

        // necessary positive
        test_round_trip(&[0x02, 0x02, 0x00, 0xff], Integer::new(&[0x00, 0xff]));
    }

    #[test]
    fn test_invalid_bitstring_encodings() {
        // empty
        assert_eq!(
            BitString::from_bytes(&[0x03, 0x00]).unwrap_err(),
            Error::UnexpectedEof,
        );

        // not byte-aligned
        assert_eq!(
            BitString::from_bytes(&[0x03, 0x01, 0x01]).unwrap_err(),
            Error::UnhandledBitString,
        );

        // empty; valid
        test_round_trip(&[0x03, 0x01, 0x00], BitString { octets: &[] });

        // non-empty; valid
        test_round_trip(&[0x03, 0x02, 0x00, 0x12], BitString { octets: &[0x12] });
    }

    #[test]
    fn test_oid() {
        // very degenerate case
        test_round_trip(&[0x06, 0x00], ObjectId::from_path(&[]));

        test_round_trip(&[0x06, 0x01, 0x27], ObjectId::from_path(&[39]));

        test_round_trip(
            &[0x06, 0x05, 0x2b, 0x81, 0x04, 0x00, 0x22],
            ObjectId::from_path(&[1, 3, 132, 0, 34]),
        );

        // meets our self-imposed length limit
        assert_eq!(
            ObjectId::from_bytes(&[
                0x06, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00
            ])
            .unwrap(),
            ObjectId {
                buf: [0x0; 16],
                used: 16
            },
        );

        // exceeds our self-imposed length limit
        assert_eq!(
            ObjectId::from_bytes(&[
                0x06, 0x11, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00
            ])
            .unwrap_err(),
            Error::UnsupportedLargeObjectId,
        );
    }

    #[test]
    fn test_null() {
        test_round_trip(&[0x05, 0x00], Null);

        assert_eq!(
            Null::from_bytes(&[0x05, 0x01, 0x00]).unwrap_err(),
            Error::IllegalNull
        );
    }

    #[test]
    fn test_any() {
        test_round_trip(&[0x05, 0x00], Any::Null(Null));
        test_round_trip(
            &[0x02, 0x02, 0x12, 0x34],
            Any::Integer(Integer::new(&[0x12, 0x34])),
        );
        test_round_trip(
            &[0x04, 0x02, 0x12, 0x34],
            Any::OctetString(OctetString::new(&[0x12, 0x34])),
        );
        test_round_trip(
            &[0x03, 0x03, 0x00, 0x12, 0x34],
            Any::BitString(BitString {
                octets: &[0x12, 0x34],
            }),
        );
        test_round_trip(
            &[0x06, 0x01, 0x27],
            Any::ObjectId(ObjectId::from_path(&[39])),
        );

        assert_eq!(Any::from_bytes(&[0x30]).unwrap_err(), Error::UnexpectedTag);
    }

    #[test]
    fn test_optional() {
        test_round_trip(&[0x06, 0x01, 0x27], Some(ObjectId::from_path(&[39])));
        test_round_trip(&[], None::<ObjectId>);
    }

    #[allow(clippy::enum_clike_unportable_variant)]
    #[test]
    fn test_enum() {
        asn1_enum! {
            Enum ::= INTEGER {
                zero(0),
                one(1),
                u8(255),
                u16(65535),
                u24(16777215),
                u32(4294967295),
                u48(281474976710655)
            }
        }

        test_round_trip(&[0x02, 0x01, 0x00], Enum::zero);
        test_round_trip(&[0x02, 0x01, 0x01], Enum::one);
        test_round_trip(&[0x02, 0x02, 0x00, 0xff], Enum::u8);
        test_round_trip(&[0x02, 0x03, 0x00, 0xff, 0xff], Enum::u16);
        test_round_trip(&[0x02, 0x04, 0x00, 0xff, 0xff, 0xff], Enum::u24);
        test_round_trip(&[0x02, 0x05, 0x00, 0xff, 0xff, 0xff, 0xff], Enum::u32);
        test_round_trip(
            &[0x02, 0x07, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
            Enum::u48,
        );
    }

    /// Verify that `value.encode` yields `encoding`, and that decoding
    /// `encoding` yields a value equal to `value`.
    fn test_round_trip<'a, T: Type<'a> + PartialEq>(encoding: &'a [u8], value: T) {
        let decoded = T::from_bytes(encoding).unwrap();
        assert_eq!(decoded, value);
        assert_eq!(decoded.encoded_len(), value.encoded_len());

        let mut buffer = vec![0u8; encoding.len()];
        let mut enc = Encoder::new(&mut buffer);
        let enc_dec = decoded.encode(&mut enc).unwrap();
        assert_eq!(encoding, buffer);

        let mut buffer = vec![0u8; encoding.len()];
        let mut enc = Encoder::new(&mut buffer);
        let enc_val = value.encode(&mut enc).unwrap();
        assert_eq!(encoding, buffer);

        assert_eq!(enc_val, enc_dec);
        test_truncated_encode(value, enc_val);
    }

    /// Verify that `value.encode` yields `Error::UnexpectedEof` when encoding
    /// into a buffer that is shorter than `expected_len`.
    fn test_truncated_encode<'a, T: Type<'a>>(value: T, expected_len: usize) {
        for i in 0..expected_len {
            let mut buffer = vec![0u8; i];
            let mut enc = Encoder::new(&mut buffer);
            assert_eq!(value.encode(&mut enc).unwrap_err(), Error::UnexpectedEof);
        }

        // check base case to see we're testing something
        let mut buffer = vec![0u8; expected_len];
        let mut enc = Encoder::new(&mut buffer);
        assert_eq!(value.encode(&mut enc).unwrap(), expected_len);
    }

    #[test]
    fn test_encode_errors() {
        // these branches are generally unreachable, as begin/split do this
        // work for normal type encoding
        let mut buf = [0];
        assert_eq!(
            Encoder::new(&mut buf).append_slice(&[0, 0]).unwrap_err(),
            Error::UnexpectedEof
        );
    }

    #[test]
    fn test_context_tag() {
        assert_eq!(Tag::context(1, true).0, 0xa1);
        assert_eq!(Tag::context(1, false).0, 0x81);
    }

    #[test]
    fn test_encode_len() {
        assert_eq!(encoded_length_for(0), 2);
        assert_eq!(encoded_length_for(1), 2 + 1);
        assert_eq!(encoded_length_for(2), 2 + 2);
        assert_eq!(encoded_length_for(3), 2 + 3);
        assert_eq!(encoded_length_for(127), 2 + 127);
        assert_eq!(encoded_length_for(128), 3 + 128);
        assert_eq!(encoded_length_for(255), 3 + 255);
        assert_eq!(encoded_length_for(256), 4 + 256);
        assert_eq!(encoded_length_for(65535), 4 + 65535);
        assert_eq!(encoded_length_for(65536), 5 + 65536);
        assert_eq!(encoded_length_for(16777215), 5 + 16777215);
        assert_eq!(encoded_length_for(16777216), 6 + 16777216);
        assert_eq!(encoded_length_for(4294967295), 6 + 4294967295);
    }

    #[test]
    #[should_panic]
    fn test_encode_len_limit() {
        let _ = encoded_length_for(4294967296);
    }

    #[test]
    fn test_canonical_length_requirements() {
        fn take_len(input: &[u8]) -> Result<usize, Error> {
            Parser::new(input).take_len()
        }

        // no length byte at all
        assert_eq!(take_len(&[]), Err(Error::UnexpectedEof));

        // short form
        assert_eq!(take_len(&[0x00]), Ok(0x00));
        assert_eq!(take_len(&[0x7f]), Ok(0x7f));

        // 0x80 is the (unsupported) indefinite length form
        assert_eq!(take_len(&[0x80]), Err(Error::UnsupportedLargeObjectLength));

        // 0x81 long form: one length byte
        assert_eq!(take_len(&[0x81]), Err(Error::UnexpectedEof));
        // ... value < 0x80 should have used the short form
        assert_eq!(take_len(&[0x81, 0x00]), Err(Error::NonCanonicalEncoding));
        assert_eq!(take_len(&[0x81, 0x7f]), Err(Error::NonCanonicalEncoding));
        // ... valid
        assert_eq!(take_len(&[0x81, 0x80]), Ok(0x80));
        assert_eq!(take_len(&[0x81, 0xff]), Ok(0xff));

        // 0x82 long form: two length bytes
        assert_eq!(take_len(&[0x82]), Err(Error::UnexpectedEof));
        assert_eq!(take_len(&[0x82, 0x01]), Err(Error::UnexpectedEof));
        // ... value <= 0xff should have used a shorter form
        assert_eq!(
            take_len(&[0x82, 0x00, 0x00]),
            Err(Error::NonCanonicalEncoding)
        );
        assert_eq!(
            take_len(&[0x82, 0x00, 0xfe]),
            Err(Error::NonCanonicalEncoding)
        );
        assert_eq!(
            take_len(&[0x82, 0x00, 0xff]),
            Err(Error::NonCanonicalEncoding)
        );
        // ... valid
        assert_eq!(take_len(&[0x82, 0x01, 0x00]), Ok(0x100));
        assert_eq!(take_len(&[0x82, 0xff, 0xff]), Ok(0xffff));

        // 0x83 and above: more than two length bytes, unsupported
        assert_eq!(take_len(&[0x83]), Err(Error::UnsupportedLargeObjectLength));
        assert_eq!(take_len(&[0xff]), Err(Error::UnsupportedLargeObjectLength));
    }

    #[test]
    fn test_error_display() {
        assert_eq!(format!("{}", Error::UnexpectedTag), "unexpected tag");
        assert_eq!(
            format!("{}", Error::UnexpectedEof),
            "unexpected end of input"
        );
        assert_eq!(
            format!("{}", Error::UnexpectedTrailingData),
            "unexpected trailing data"
        );
        assert_eq!(
            format!("{}", Error::NonCanonicalEncoding),
            "non-canonical encoding"
        );
        assert_eq!(
            format!("{}", Error::UnhandledEnumValue),
            "unhandled enum value"
        );
        assert_eq!(
            format!("{}", Error::IntegerOutOfRange),
            "integer out of range"
        );
        assert_eq!(format!("{}", Error::IllegalNull), "illegal null");
        assert_eq!(
            format!("{}", Error::UnsupportedLargeObjectId),
            "unsupported large object identifier"
        );
        assert_eq!(
            format!("{}", Error::UnsupportedLargeObjectLength),
            "unsupported large object length"
        );
        assert_eq!(
            format!("{}", Error::UnhandledBitString),
            "unhandled bit string"
        );
    }
}
