use crate::error::{Error, ErrorCode, Result};
use crate::number::Number;
use crate::read::{self, Fused, Reference};
use alloc::string::String;
use alloc::vec::Vec;
use core::iter;
use core::iter::FusedIterator;
use core::marker::PhantomData;
use core::result;
use core::str::{self, FromStr};
use serde::de::{self, Expected, Unexpected};
use serde::forward_to_deserialize_any;

const F64_EXACT_SIGNIFICAND_MAX: u64 = 1 << f64::MANTISSA_DIGITS;
const F32_EXACT_SIGNIFICAND_MAX: u64 = 1 << f32::MANTISSA_DIGITS;
const F64_EXACT_POW10: [f64; 23] = [
    1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16,
    1e17, 1e18, 1e19, 1e20, 1e21, 1e22,
];
const F32_EXACT_POW10: [f32; 11] = [1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10];

pub use crate::read::{Read, SliceRead, StrRead};

pub use crate::read::IoRead;

pub struct Deserializer<R> {
    read: R,
    scratch: Vec<u8>,
    remaining_depth: u8,
    single_precision: bool,
}

impl<'de, R> Deserializer<R>
where
    R: read::Read<'de>,
{
    pub fn new(read: R) -> Self {
        Deserializer {
            read,
            scratch: Vec::new(),
            remaining_depth: 128,
            single_precision: false,
        }
    }
}

impl<R> Deserializer<read::IoRead<R>>
where
    R: crate::io::Read,
{
    pub fn from_reader(reader: R) -> Self {
        Deserializer::new(read::IoRead::new(reader))
    }
}

impl<'a> Deserializer<read::SliceRead<'a>> {
    pub fn from_slice(bytes: &'a [u8]) -> Self {
        Deserializer::new(read::SliceRead::new(bytes))
    }
}

impl<'a> Deserializer<read::StrRead<'a>> {
    pub fn from_str(s: &'a str) -> Self {
        Deserializer::new(read::StrRead::new(s))
    }
}

macro_rules! overflow {
    ($a:ident * 10 + $b:ident, $c:expr) => {
        match $c {
            c => $a >= c / 10 && ($a > c / 10 || $b > c % 10),
        }
    };
}

pub(crate) enum ParserNumber {
    F64(f64),
    U64(u64),
    I64(i64),
}

impl ParserNumber {
    fn visit<'de, V>(self, visitor: V) -> Result<V::Value>
    where
        V: de::Visitor<'de>,
    {
        match self {
            ParserNumber::F64(x) => visitor.visit_f64(x),
            ParserNumber::U64(x) => visitor.visit_u64(x),
            ParserNumber::I64(x) => visitor.visit_i64(x),
        }
    }

    fn invalid_type(self, exp: &dyn Expected) -> Error {
        match self {
            ParserNumber::F64(x) => de::Error::invalid_type(Unexpected::Float(x), exp),
            ParserNumber::U64(x) => de::Error::invalid_type(Unexpected::Unsigned(x), exp),
            ParserNumber::I64(x) => de::Error::invalid_type(Unexpected::Signed(x), exp),
        }
    }
}

impl<'de, R: Read<'de>> Deserializer<R> {
    pub fn end(&mut self) -> Result<()> {
        match tri!(self.parse_whitespace()) {
            Some(_) => Err(self.peek_error(ErrorCode::TrailingCharacters)),
            None => Ok(()),
        }
    }

    pub fn into_iter<T>(self) -> StreamDeserializer<'de, R, T>
    where
        T: de::Deserialize<'de>,
    {
        let offset = self.read.byte_offset();
        StreamDeserializer {
            de: self,
            offset,
            failed: false,
            output: PhantomData,
            lifetime: PhantomData,
        }
    }

    pub(crate) fn peek(&mut self) -> Result<Option<u8>> {
        self.read.peek()
    }

    fn peek_or_null(&mut self) -> Result<u8> {
        Ok(tri!(self.peek()).unwrap_or(b'\x00'))
    }

    fn eat_char(&mut self) {
        self.read.discard();
    }

    fn next_char(&mut self) -> Result<Option<u8>> {
        self.read.next()
    }

    fn next_char_or_null(&mut self) -> Result<u8> {
        Ok(tri!(self.next_char()).unwrap_or(b'\x00'))
    }

    #[cold]
    fn error(&self, reason: ErrorCode) -> Error {
        let position = self.read.position();
        Error::syntax(reason, position.line, position.column)
    }

    #[cold]
    fn peek_error(&self, reason: ErrorCode) -> Error {
        let position = self.read.peek_position();
        Error::syntax(reason, position.line, position.column)
    }

    fn parse_whitespace(&mut self) -> Result<Option<u8>> {
        loop {
            match tri!(self.peek()) {
                Some(b' ' | b'\n' | b'\t' | b'\r') => {
                    self.eat_char();
                }
                other => {
                    return Ok(other);
                }
            }
        }
    }

    #[cold]
    fn peek_invalid_type(&mut self, exp: &dyn Expected) -> Error {
        let err = match self.peek_or_null().unwrap_or(b'\x00') {
            b'n' => {
                self.eat_char();
                if let Err(err) = self.parse_ident(b"ull") {
                    return err;
                }
                de::Error::invalid_type(Unexpected::Unit, exp)
            }
            b't' => {
                self.eat_char();
                if let Err(err) = self.parse_ident(b"rue") {
                    return err;
                }
                de::Error::invalid_type(Unexpected::Bool(true), exp)
            }
            b'f' => {
                self.eat_char();
                if let Err(err) = self.parse_ident(b"alse") {
                    return err;
                }
                de::Error::invalid_type(Unexpected::Bool(false), exp)
            }
            b'-' => {
                self.eat_char();
                match self.parse_any_number(false) {
                    Ok(n) => n.invalid_type(exp),
                    Err(err) => return err,
                }
            }
            b'0'..=b'9' => match self.parse_any_number(true) {
                Ok(n) => n.invalid_type(exp),
                Err(err) => return err,
            },
            b'"' => {
                self.eat_char();
                self.scratch.clear();
                match self.read.parse_str(&mut self.scratch) {
                    Ok(s) => de::Error::invalid_type(Unexpected::Str(&s), exp),
                    Err(err) => return err,
                }
            }
            b'[' => de::Error::invalid_type(Unexpected::Seq, exp),
            b'{' => de::Error::invalid_type(Unexpected::Map, exp),
            _ => self.peek_error(ErrorCode::ExpectedSomeValue),
        };

        self.fix_position(err)
    }

    pub(crate) fn deserialize_number<'any, V>(&mut self, visitor: V) -> Result<V::Value>
    where
        V: de::Visitor<'any>,
    {
        let peek = match tri!(self.parse_whitespace()) {
            Some(b) => b,
            None => {
                return Err(self.peek_error(ErrorCode::EofWhileParsingValue));
            }
        };

        let value = match peek {
            b'-' => {
                self.eat_char();
                tri!(self.parse_integer(false)).visit(visitor)
            }
            b'0'..=b'9' => tri!(self.parse_integer(true)).visit(visitor),
            _ => Err(self.peek_invalid_type(&visitor)),
        };

        match value {
            Ok(value) => Ok(value),
            Err(err) => Err(self.fix_position(err)),
        }
    }

    pub(crate) fn do_deserialize_f32<'any, V>(&mut self, visitor: V) -> Result<V::Value>
    where
        V: de::Visitor<'any>,
    {
        self.single_precision = true;
        let val = self.deserialize_number(visitor);
        self.single_precision = false;
        val
    }

    pub(crate) fn do_deserialize_i128<'any, V>(&mut self, visitor: V) -> Result<V::Value>
    where
        V: de::Visitor<'any>,
    {
        let mut buf = String::new();

        match tri!(self.parse_whitespace()) {
            Some(b'-') => {
                self.eat_char();
                buf.push('-');
            }
            Some(_) => {}
            None => {
                return Err(self.peek_error(ErrorCode::EofWhileParsingValue));
            }
        }

        tri!(self.scan_integer128(&mut buf));

        let value = match buf.parse() {
            Ok(int) => visitor.visit_i128(int),
            Err(_) => {
                return Err(self.error(ErrorCode::NumberOutOfRange));
            }
        };

        match value {
            Ok(value) => Ok(value),
            Err(err) => Err(self.fix_position(err)),
        }
    }

    pub(crate) fn do_deserialize_u128<'any, V>(&mut self, visitor: V) -> Result<V::Value>
    where
        V: de::Visitor<'any>,
    {
        match tri!(self.parse_whitespace()) {
            Some(b'-') => {
                return Err(self.peek_error(ErrorCode::NumberOutOfRange));
            }
            Some(_) => {}
            None => {
                return Err(self.peek_error(ErrorCode::EofWhileParsingValue));
            }
        }

        let mut buf = String::new();
        tri!(self.scan_integer128(&mut buf));

        let value = match buf.parse() {
            Ok(int) => visitor.visit_u128(int),
            Err(_) => {
                return Err(self.error(ErrorCode::NumberOutOfRange));
            }
        };

        match value {
            Ok(value) => Ok(value),
            Err(err) => Err(self.fix_position(err)),
        }
    }

    fn scan_integer128(&mut self, buf: &mut String) -> Result<()> {
        match tri!(self.next_char_or_null()) {
            b'0' => {
                buf.push('0');
                match tri!(self.peek_or_null()) {
                    b'0'..=b'9' => Err(self.peek_error(ErrorCode::InvalidNumber)),
                    _ => Ok(()),
                }
            }
            c @ b'1'..=b'9' => {
                buf.push(c as char);
                while let c @ b'0'..=b'9' = tri!(self.peek_or_null()) {
                    self.eat_char();
                    buf.push(c as char);
                }
                Ok(())
            }
            _ => Err(self.error(ErrorCode::InvalidNumber)),
        }
    }

    #[cold]
    fn fix_position(&self, err: Error) -> Error {
        err.fix_position(move |code| self.error(code))
    }

    fn parse_ident(&mut self, ident: &[u8]) -> Result<()> {
        for expected in ident {
            match tri!(self.next_char()) {
                None => {
                    return Err(self.error(ErrorCode::EofWhileParsingValue));
                }
                Some(next) => {
                    if next != *expected {
                        return Err(self.error(ErrorCode::ExpectedSomeIdent));
                    }
                }
            }
        }

        Ok(())
    }

    fn parse_integer(&mut self, positive: bool) -> Result<ParserNumber> {
        let next = match tri!(self.next_char()) {
            Some(b) => b,
            None => {
                return Err(self.error(ErrorCode::EofWhileParsingValue));
            }
        };

        match next {
            b'0' => {
                match tri!(self.peek_or_null()) {
                    b'0'..=b'9' => Err(self.peek_error(ErrorCode::InvalidNumber)),
                    _ => self.parse_number(positive, 0),
                }
            }
            c @ b'1'..=b'9' => {
                let mut significand = (c - b'0') as u64;

                loop {
                    match tri!(self.peek_or_null()) {
                        c @ b'0'..=b'9' => {
                            let digit = (c - b'0') as u64;

                            if overflow!(significand * 10 + digit, u64::MAX) {
                                return Ok(ParserNumber::F64(tri!(
                                    self.parse_long_integer(positive, significand),
                                )));
                            }

                            self.eat_char();
                            significand = significand * 10 + digit;
                        }
                        _ => {
                            return self.parse_number(positive, significand);
                        }
                    }
                }
            }
            _ => Err(self.error(ErrorCode::InvalidNumber)),
        }
    }

    fn parse_number(&mut self, positive: bool, significand: u64) -> Result<ParserNumber> {
        Ok(match tri!(self.peek_or_null()) {
            b'.' => ParserNumber::F64(tri!(self.parse_decimal(positive, significand, 0))),
            b'e' | b'E' => ParserNumber::F64(tri!(self.parse_exponent(positive, significand, 0))),
            _ => {
                if positive {
                    ParserNumber::U64(significand)
                } else {
                    let neg = (significand as i64).wrapping_neg();

                    if neg >= 0 {
                        ParserNumber::F64(-(significand as f64))
                    } else {
                        ParserNumber::I64(neg)
                    }
                }
            }
        })
    }

    fn parse_decimal(
        &mut self,
        positive: bool,
        mut significand: u64,
        exponent_before_decimal_point: i32,
    ) -> Result<f64> {
        self.eat_char();

        let mut exponent_after_decimal_point = 0;
        while let c @ b'0'..=b'9' = tri!(self.peek_or_null()) {
            let digit = (c - b'0') as u64;

            if overflow!(significand * 10 + digit, u64::MAX) {
                let exponent = exponent_before_decimal_point + exponent_after_decimal_point;
                return self.parse_decimal_overflow(positive, significand, exponent);
            }

            self.eat_char();
            significand = significand * 10 + digit;
            exponent_after_decimal_point -= 1;
        }

        if exponent_after_decimal_point == 0 {
            match tri!(self.peek()) {
                Some(_) => return Err(self.peek_error(ErrorCode::InvalidNumber)),
                None => return Err(self.peek_error(ErrorCode::EofWhileParsingValue)),
            }
        }

        let exponent = exponent_before_decimal_point + exponent_after_decimal_point;
        match tri!(self.peek_or_null()) {
            b'e' | b'E' => self.parse_exponent(positive, significand, exponent),
            _ => self.f64_from_parts(positive, significand, exponent),
        }
    }

    fn parse_exponent(
        &mut self,
        positive: bool,
        significand: u64,
        starting_exp: i32,
    ) -> Result<f64> {
        self.eat_char();

        let positive_exp = match tri!(self.peek_or_null()) {
            b'+' => {
                self.eat_char();
                true
            }
            b'-' => {
                self.eat_char();
                false
            }
            _ => true,
        };

        let next = match tri!(self.next_char()) {
            Some(b) => b,
            None => {
                return Err(self.error(ErrorCode::EofWhileParsingValue));
            }
        };

        let mut exp = match next {
            c @ b'0'..=b'9' => (c - b'0') as i32,
            _ => {
                return Err(self.error(ErrorCode::InvalidNumber));
            }
        };

        while let c @ b'0'..=b'9' = tri!(self.peek_or_null()) {
            self.eat_char();
            let digit = (c - b'0') as i32;

            if overflow!(exp * 10 + digit, i32::MAX) {
                let zero_significand = significand == 0;
                return self.parse_exponent_overflow(positive, zero_significand, positive_exp);
            }

            exp = exp * 10 + digit;
        }

        let final_exp = if positive_exp {
            starting_exp.saturating_add(exp)
        } else {
            starting_exp.saturating_sub(exp)
        };

        self.f64_from_parts(positive, significand, final_exp)
    }

    fn f64_from_parts(&mut self, positive: bool, significand: u64, exponent: i32) -> Result<f64> {
        if let Some(f) = self.exact_from_parts(significand, exponent) {
            return Ok(if positive { f } else { -f });
        }
        self.scratch.clear();
        self.scratch
            .extend_from_slice(itoa::Buffer::new().format(significand).as_bytes());
        self.scratch.push(b'e');
        self.scratch
            .extend_from_slice(itoa::Buffer::new().format(exponent).as_bytes());
        self.f64_from_scratch(positive)
    }

    fn exact_from_parts(&self, significand: u64, exponent: i32) -> Option<f64> {
        if self.single_precision {
            if significand > F32_EXACT_SIGNIFICAND_MAX || exponent.unsigned_abs() as usize >= F32_EXACT_POW10.len() {
                return None;
            }
            let pow = F32_EXACT_POW10[exponent.unsigned_abs() as usize];
            let f = significand as f32;
            Some(f64::from(if exponent < 0 { f / pow } else { f * pow }))
        } else {
            if significand > F64_EXACT_SIGNIFICAND_MAX || exponent.unsigned_abs() as usize >= F64_EXACT_POW10.len() {
                return None;
            }
            let pow = F64_EXACT_POW10[exponent.unsigned_abs() as usize];
            let f = significand as f64;
            Some(if exponent < 0 { f / pow } else { f * pow })
        }
    }

    fn f64_from_scratch(&mut self, positive: bool) -> Result<f64> {
        let text = str::from_utf8(&self.scratch).unwrap_or_default();
        let f = if self.single_precision {
            f32::from_str(text).map(f64::from)
        } else {
            f64::from_str(text)
        };
        match f {
            Ok(f) if !f.is_infinite() => Ok(if positive { f } else { -f }),
            _ => Err(self.error(ErrorCode::NumberOutOfRange)),
        }
    }

    #[cold]
    #[inline(never)]
    fn parse_long_integer(&mut self, positive: bool, partial_significand: u64) -> Result<f64> {
        self.scratch.clear();
        self.scratch
            .extend_from_slice(itoa::Buffer::new().format(partial_significand).as_bytes());

        loop {
            match tri!(self.peek_or_null()) {
                c @ b'0'..=b'9' => {
                    self.scratch.push(c);
                    self.eat_char();
                }
                b'.' => {
                    self.eat_char();
                    return self.parse_long_decimal(positive, self.scratch.len());
                }
                b'e' | b'E' => {
                    return self.parse_long_exponent(positive, self.scratch.len());
                }
                _ => {
                    return self.f64_long_from_parts(positive, self.scratch.len(), 0);
                }
            }
        }
    }

    #[cold]
    fn parse_long_decimal(&mut self, positive: bool, integer_end: usize) -> Result<f64> {
        let mut at_least_one_digit = integer_end < self.scratch.len();
        while let c @ b'0'..=b'9' = tri!(self.peek_or_null()) {
            self.scratch.push(c);
            self.eat_char();
            at_least_one_digit = true;
        }

        if !at_least_one_digit {
            match tri!(self.peek()) {
                Some(_) => return Err(self.peek_error(ErrorCode::InvalidNumber)),
                None => return Err(self.peek_error(ErrorCode::EofWhileParsingValue)),
            }
        }

        match tri!(self.peek_or_null()) {
            b'e' | b'E' => self.parse_long_exponent(positive, integer_end),
            _ => self.f64_long_from_parts(positive, integer_end, 0),
        }
    }

    fn parse_long_exponent(&mut self, positive: bool, integer_end: usize) -> Result<f64> {
        self.eat_char();

        let positive_exp = match tri!(self.peek_or_null()) {
            b'+' => {
                self.eat_char();
                true
            }
            b'-' => {
                self.eat_char();
                false
            }
            _ => true,
        };

        let next = match tri!(self.next_char()) {
            Some(b) => b,
            None => {
                return Err(self.error(ErrorCode::EofWhileParsingValue));
            }
        };

        let mut exp = match next {
            c @ b'0'..=b'9' => (c - b'0') as i32,
            _ => {
                return Err(self.error(ErrorCode::InvalidNumber));
            }
        };

        while let c @ b'0'..=b'9' = tri!(self.peek_or_null()) {
            self.eat_char();
            let digit = (c - b'0') as i32;

            if overflow!(exp * 10 + digit, i32::MAX) {
                let zero_significand = self.scratch.iter().all(|&digit| digit == b'0');
                return self.parse_exponent_overflow(positive, zero_significand, positive_exp);
            }

            exp = exp * 10 + digit;
        }

        let final_exp = if positive_exp { exp } else { -exp };

        self.f64_long_from_parts(positive, integer_end, final_exp)
    }

    #[cold]
    #[inline(never)]
    fn parse_decimal_overflow(
        &mut self,
        positive: bool,
        significand: u64,
        exponent: i32,
    ) -> Result<f64> {
        let mut buffer = itoa::Buffer::new();
        let significand = buffer.format(significand);
        let fraction_digits = -exponent as usize;
        self.scratch.clear();
        if let Some(zeros) = fraction_digits.checked_sub(significand.len() + 1) {
            self.scratch.extend(iter::repeat(b'0').take(zeros + 1));
        }
        self.scratch.extend_from_slice(significand.as_bytes());
        let integer_end = self.scratch.len() - fraction_digits;
        self.parse_long_decimal(positive, integer_end)
    }

    #[cold]
    #[inline(never)]
    fn parse_exponent_overflow(
        &mut self,
        positive: bool,
        zero_significand: bool,
        positive_exp: bool,
    ) -> Result<f64> {
        if !zero_significand && positive_exp {
            return Err(self.error(ErrorCode::NumberOutOfRange));
        }

        while let b'0'..=b'9' = tri!(self.peek_or_null()) {
            self.eat_char();
        }
        Ok(if positive { 0.0 } else { -0.0 })
    }

    fn f64_long_from_parts(
        &mut self,
        positive: bool,
        integer_end: usize,
        exponent: i32,
    ) -> Result<f64> {
        if integer_end == self.scratch.len() {
            self.scratch.push(b'0');
        }
        self.scratch.insert(integer_end, b'.');
        if integer_end == 0 {
            self.scratch.insert(0, b'0');
        }
        self.scratch.push(b'e');
        self.scratch
            .extend_from_slice(itoa::Buffer::new().format(exponent).as_bytes());
        self.f64_from_scratch(positive)
    }

    fn parse_any_signed_number(&mut self) -> Result<ParserNumber> {
        let peek = match tri!(self.peek()) {
            Some(b) => b,
            None => {
                return Err(self.peek_error(ErrorCode::EofWhileParsingValue));
            }
        };

        let value = match peek {
            b'-' => {
                self.eat_char();
                self.parse_any_number(false)
            }
            b'0'..=b'9' => self.parse_any_number(true),
            _ => Err(self.peek_error(ErrorCode::InvalidNumber)),
        };

        let value = match tri!(self.peek()) {
            Some(_) => Err(self.peek_error(ErrorCode::InvalidNumber)),
            None => value,
        };

        match value {
            Ok(value) => Ok(value),
            Err(err) => Err(self.fix_position(err)),
        }
    }

    fn parse_any_number(&mut self, positive: bool) -> Result<ParserNumber> {
        self.parse_integer(positive)
    }

    fn parse_object_colon(&mut self) -> Result<()> {
        match tri!(self.parse_whitespace()) {
            Some(b':') => {
                self.eat_char();
                Ok(())
            }
            Some(_) => Err(self.peek_error(ErrorCode::ExpectedColon)),
            None => Err(self.peek_error(ErrorCode::EofWhileParsingObject)),
        }
    }

    fn end_seq(&mut self) -> Result<()> {
        match tri!(self.parse_whitespace()) {
            Some(b']') => {
                self.eat_char();
                Ok(())
            }
            Some(b',') => {
                self.eat_char();
                match self.parse_whitespace() {
                    Ok(Some(b']')) => Err(self.peek_error(ErrorCode::TrailingComma)),
                    _ => Err(self.peek_error(ErrorCode::TrailingCharacters)),
                }
            }
            Some(_) => Err(self.peek_error(ErrorCode::TrailingCharacters)),
            None => Err(self.peek_error(ErrorCode::EofWhileParsingList)),
        }
    }

    fn end_map(&mut self) -> Result<()> {
        match tri!(self.parse_whitespace()) {
            Some(b'}') => {
                self.eat_char();
                Ok(())
            }
            Some(b',') => Err(self.peek_error(ErrorCode::TrailingComma)),
            Some(_) => Err(self.peek_error(ErrorCode::TrailingCharacters)),
            None => Err(self.peek_error(ErrorCode::EofWhileParsingObject)),
        }
    }

    fn ignore_value(&mut self) -> Result<()> {
        self.scratch.clear();
        let mut enclosing = None;

        loop {
            let peek = match tri!(self.parse_whitespace()) {
                Some(b) => b,
                None => {
                    return Err(self.peek_error(ErrorCode::EofWhileParsingValue));
                }
            };

            let frame = match peek {
                b'n' => {
                    self.eat_char();
                    tri!(self.parse_ident(b"ull"));
                    None
                }
                b't' => {
                    self.eat_char();
                    tri!(self.parse_ident(b"rue"));
                    None
                }
                b'f' => {
                    self.eat_char();
                    tri!(self.parse_ident(b"alse"));
                    None
                }
                b'-' => {
                    self.eat_char();
                    tri!(self.ignore_integer());
                    None
                }
                b'0'..=b'9' => {
                    tri!(self.ignore_integer());
                    None
                }
                b'"' => {
                    self.eat_char();
                    tri!(self.read.ignore_str());
                    None
                }
                frame @ (b'[' | b'{') => {
                    self.scratch.extend(enclosing.take());
                    self.eat_char();
                    Some(frame)
                }
                _ => return Err(self.peek_error(ErrorCode::ExpectedSomeValue)),
            };

            let (mut accept_comma, mut frame) = match frame {
                Some(frame) => (false, frame),
                None => match enclosing.take() {
                    Some(frame) => (true, frame),
                    None => match self.scratch.pop() {
                        Some(frame) => (true, frame),
                        None => return Ok(()),
                    },
                },
            };

            loop {
                match tri!(self.parse_whitespace()) {
                    Some(b',') if accept_comma => {
                        self.eat_char();
                        break;
                    }
                    Some(b']') if frame == b'[' => {}
                    Some(b'}') if frame == b'{' => {}
                    Some(_) => {
                        if accept_comma {
                            return Err(self.peek_error(match frame {
                                b'[' => ErrorCode::ExpectedListCommaOrEnd,
                                b'{' => ErrorCode::ExpectedObjectCommaOrEnd,
                                _ => unreachable!(),
                            }));
                        } else {
                            break;
                        }
                    }
                    None => {
                        return Err(self.peek_error(match frame {
                            b'[' => ErrorCode::EofWhileParsingList,
                            b'{' => ErrorCode::EofWhileParsingObject,
                            _ => unreachable!(),
                        }));
                    }
                }

                self.eat_char();
                frame = match self.scratch.pop() {
                    Some(frame) => frame,
                    None => return Ok(()),
                };
                accept_comma = true;
            }

            if frame == b'{' {
                match tri!(self.parse_whitespace()) {
                    Some(b'"') => self.eat_char(),
                    Some(_) => return Err(self.peek_error(ErrorCode::KeyMustBeAString)),
                    None => return Err(self.peek_error(ErrorCode::EofWhileParsingObject)),
                }
                tri!(self.read.ignore_str());
                match tri!(self.parse_whitespace()) {
                    Some(b':') => self.eat_char(),
                    Some(_) => return Err(self.peek_error(ErrorCode::ExpectedColon)),
                    None => return Err(self.peek_error(ErrorCode::EofWhileParsingObject)),
                }
            }

            enclosing = Some(frame);
        }
    }

    fn ignore_integer(&mut self) -> Result<()> {
        match tri!(self.next_char_or_null()) {
            b'0' => {
                if let b'0'..=b'9' = tri!(self.peek_or_null()) {
                    return Err(self.peek_error(ErrorCode::InvalidNumber));
                }
            }
            b'1'..=b'9' => {
                while let b'0'..=b'9' = tri!(self.peek_or_null()) {
                    self.eat_char();
                }
            }
            _ => {
                return Err(self.error(ErrorCode::InvalidNumber));
            }
        }

        match tri!(self.peek_or_null()) {
            b'.' => self.ignore_decimal(),
            b'e' | b'E' => self.ignore_exponent(),
            _ => Ok(()),
        }
    }

    fn ignore_decimal(&mut self) -> Result<()> {
        self.eat_char();

        let mut at_least_one_digit = false;
        while let b'0'..=b'9' = tri!(self.peek_or_null()) {
            self.eat_char();
            at_least_one_digit = true;
        }

        if !at_least_one_digit {
            return Err(self.peek_error(ErrorCode::InvalidNumber));
        }

        match tri!(self.peek_or_null()) {
            b'e' | b'E' => self.ignore_exponent(),
            _ => Ok(()),
        }
    }

    fn ignore_exponent(&mut self) -> Result<()> {
        self.eat_char();

        match tri!(self.peek_or_null()) {
            b'+' | b'-' => self.eat_char(),
            _ => {}
        }

        match tri!(self.next_char_or_null()) {
            b'0'..=b'9' => {}
            _ => {
                return Err(self.error(ErrorCode::InvalidNumber));
            }
        }

        while let b'0'..=b'9' = tri!(self.peek_or_null()) {
            self.eat_char();
        }

        Ok(())
    }

    fn deserialize_raw_value<V>(&mut self, visitor: V) -> Result<V::Value>
    where
        V: de::Visitor<'de>,
    {
        tri!(self.parse_whitespace());
        self.read.begin_raw_buffering();
        tri!(self.ignore_value());
        self.read.end_raw_buffering(visitor)
    }
}

impl FromStr for Number {
    type Err = Error;

    fn from_str(s: &str) -> result::Result<Self, Self::Err> {
        Deserializer::from_str(s)
            .parse_any_signed_number()
            .map(Into::into)
    }
}

macro_rules! deserialize_number {
    ($method:ident) => {
        deserialize_number!($method, deserialize_number);
    };

    ($method:ident, $using:ident) => {
        fn $method<V>(self, visitor: V) -> Result<V::Value>
        where
            V: de::Visitor<'de>,
        {
            self.$using(visitor)
        }
    };
}

macro_rules! if_checking_recursion_limit {
    ($($body:tt)*) => {
        $($body)*
    };
}

macro_rules! check_recursion {
    ($this:ident $($body:tt)*) => {
        if_checking_recursion_limit! {
            $this.remaining_depth -= 1;
            if $this.remaining_depth == 0 {
                return Err($this.peek_error(ErrorCode::RecursionLimitExceeded));
            }
        }

        $this $($body)*

        if_checking_recursion_limit! {
            $this.remaining_depth += 1;
        }
    };
}

impl<'de, R: Read<'de>> de::Deserializer<'de> for &mut Deserializer<R> {
    type Error = Error;

    #[inline]
    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value>
    where
        V: de::Visitor<'de>,
    {
        let peek = match tri!(self.parse_whitespace()) {
            Some(b) => b,
            None => {
                return Err(self.peek_error(ErrorCode::EofWhileParsingValue));
            }
        };

        let value = match peek {
            b'n' => {
                self.eat_char();
                tri!(self.parse_ident(b"ull"));
                visitor.visit_unit()
            }
            b't' => {
                self.eat_char();
                tri!(self.parse_ident(b"rue"));
                visitor.visit_bool(true)
            }
            b'f' => {
                self.eat_char();
                tri!(self.parse_ident(b"alse"));
                visitor.visit_bool(false)
            }
            b'-' => {
                self.eat_char();
                tri!(self.parse_any_number(false)).visit(visitor)
            }
            b'0'..=b'9' => tri!(self.parse_any_number(true)).visit(visitor),
            b'"' => {
                self.eat_char();
                self.scratch.clear();
                match tri!(self.read.parse_str(&mut self.scratch)) {
                    Reference::Borrowed(s) => visitor.visit_borrowed_str(s),
                    Reference::Copied(s) => visitor.visit_str(s),
                }
            }
            b'[' => {
                check_recursion! {
                    self.eat_char();
                    let ret = visitor.visit_seq(SeqAccess::new(self));
                }

                match (ret, self.end_seq()) {
                    (Ok(ret), Ok(())) => Ok(ret),
                    (Err(err), _) | (_, Err(err)) => Err(err),
                }
            }
            b'{' => {
                check_recursion! {
                    self.eat_char();
                    let ret = visitor.visit_map(MapAccess::new(self));
                }

                match (ret, self.end_map()) {
                    (Ok(ret), Ok(())) => Ok(ret),
                    (Err(err), _) | (_, Err(err)) => Err(err),
                }
            }
            _ => Err(self.peek_error(ErrorCode::ExpectedSomeValue)),
        };

        match value {
            Ok(value) => Ok(value),
            Err(err) => Err(self.fix_position(err)),
        }
    }

    fn deserialize_bool<V>(self, visitor: V) -> Result<V::Value>
    where
        V: de::Visitor<'de>,
    {
        let peek = match tri!(self.parse_whitespace()) {
            Some(b) => b,
            None => {
                return Err(self.peek_error(ErrorCode::EofWhileParsingValue));
            }
        };

        let value = match peek {
            b't' => {
                self.eat_char();
                tri!(self.parse_ident(b"rue"));
                visitor.visit_bool(true)
            }
            b'f' => {
                self.eat_char();
                tri!(self.parse_ident(b"alse"));
                visitor.visit_bool(false)
            }
            _ => Err(self.peek_invalid_type(&visitor)),
        };

        match value {
            Ok(value) => Ok(value),
            Err(err) => Err(self.fix_position(err)),
        }
    }

    deserialize_number!(deserialize_i8);
    deserialize_number!(deserialize_i16);
    deserialize_number!(deserialize_i32);
    deserialize_number!(deserialize_i64);
    deserialize_number!(deserialize_u8);
    deserialize_number!(deserialize_u16);
    deserialize_number!(deserialize_u32);
    deserialize_number!(deserialize_u64);
    deserialize_number!(deserialize_f64);

    deserialize_number!(deserialize_f32, do_deserialize_f32);
    deserialize_number!(deserialize_i128, do_deserialize_i128);
    deserialize_number!(deserialize_u128, do_deserialize_u128);

    fn deserialize_char<V>(self, visitor: V) -> Result<V::Value>
    where
        V: de::Visitor<'de>,
    {
        self.deserialize_str(visitor)
    }

    fn deserialize_str<V>(self, visitor: V) -> Result<V::Value>
    where
        V: de::Visitor<'de>,
    {
        let peek = match tri!(self.parse_whitespace()) {
            Some(b) => b,
            None => {
                return Err(self.peek_error(ErrorCode::EofWhileParsingValue));
            }
        };

        let value = match peek {
            b'"' => {
                self.eat_char();
                self.scratch.clear();
                match tri!(self.read.parse_str(&mut self.scratch)) {
                    Reference::Borrowed(s) => visitor.visit_borrowed_str(s),
                    Reference::Copied(s) => visitor.visit_str(s),
                }
            }
            _ => Err(self.peek_invalid_type(&visitor)),
        };

        match value {
            Ok(value) => Ok(value),
            Err(err) => Err(self.fix_position(err)),
        }
    }

    fn deserialize_string<V>(self, visitor: V) -> Result<V::Value>
    where
        V: de::Visitor<'de>,
    {
        self.deserialize_str(visitor)
    }

    fn deserialize_bytes<V>(self, visitor: V) -> Result<V::Value>
    where
        V: de::Visitor<'de>,
    {
        let peek = match tri!(self.parse_whitespace()) {
            Some(b) => b,
            None => {
                return Err(self.peek_error(ErrorCode::EofWhileParsingValue));
            }
        };

        let value = match peek {
            b'"' => {
                self.eat_char();
                self.scratch.clear();
                match tri!(self.read.parse_str_raw(&mut self.scratch)) {
                    Reference::Borrowed(b) => visitor.visit_borrowed_bytes(b),
                    Reference::Copied(b) => visitor.visit_bytes(b),
                }
            }
            b'[' => self.deserialize_seq(visitor),
            _ => Err(self.peek_invalid_type(&visitor)),
        };

        match value {
            Ok(value) => Ok(value),
            Err(err) => Err(self.fix_position(err)),
        }
    }

    #[inline]
    fn deserialize_byte_buf<V>(self, visitor: V) -> Result<V::Value>
    where
        V: de::Visitor<'de>,
    {
        self.deserialize_bytes(visitor)
    }

    #[inline]
    fn deserialize_option<V>(self, visitor: V) -> Result<V::Value>
    where
        V: de::Visitor<'de>,
    {
        match tri!(self.parse_whitespace()) {
            Some(b'n') => {
                self.eat_char();
                tri!(self.parse_ident(b"ull"));
                visitor.visit_none()
            }
            _ => visitor.visit_some(self),
        }
    }

    fn deserialize_unit<V>(self, visitor: V) -> Result<V::Value>
    where
        V: de::Visitor<'de>,
    {
        let peek = match tri!(self.parse_whitespace()) {
            Some(b) => b,
            None => {
                return Err(self.peek_error(ErrorCode::EofWhileParsingValue));
            }
        };

        let value = match peek {
            b'n' => {
                self.eat_char();
                tri!(self.parse_ident(b"ull"));
                visitor.visit_unit()
            }
            _ => Err(self.peek_invalid_type(&visitor)),
        };

        match value {
            Ok(value) => Ok(value),
            Err(err) => Err(self.fix_position(err)),
        }
    }

    fn deserialize_unit_struct<V>(self, _name: &'static str, visitor: V) -> Result<V::Value>
    where
        V: de::Visitor<'de>,
    {
        self.deserialize_unit(visitor)
    }

    #[inline]
    fn deserialize_newtype_struct<V>(self, name: &str, visitor: V) -> Result<V::Value>
    where
        V: de::Visitor<'de>,
    {
        {
            if name == crate::raw::TOKEN {
                return self.deserialize_raw_value(visitor);
            }
        }

        let _ = name;
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V>(self, visitor: V) -> Result<V::Value>
    where
        V: de::Visitor<'de>,
    {
        let peek = match tri!(self.parse_whitespace()) {
            Some(b) => b,
            None => {
                return Err(self.peek_error(ErrorCode::EofWhileParsingValue));
            }
        };

        let value = match peek {
            b'[' => {
                check_recursion! {
                    self.eat_char();
                    let ret = visitor.visit_seq(SeqAccess::new(self));
                }

                match (ret, self.end_seq()) {
                    (Ok(ret), Ok(())) => Ok(ret),
                    (Err(err), _) | (_, Err(err)) => Err(err),
                }
            }
            _ => Err(self.peek_invalid_type(&visitor)),
        };

        match value {
            Ok(value) => Ok(value),
            Err(err) => Err(self.fix_position(err)),
        }
    }

    fn deserialize_tuple<V>(self, _len: usize, visitor: V) -> Result<V::Value>
    where
        V: de::Visitor<'de>,
    {
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value>
    where
        V: de::Visitor<'de>,
    {
        self.deserialize_seq(visitor)
    }

    fn deserialize_map<V>(self, visitor: V) -> Result<V::Value>
    where
        V: de::Visitor<'de>,
    {
        let peek = match tri!(self.parse_whitespace()) {
            Some(b) => b,
            None => {
                return Err(self.peek_error(ErrorCode::EofWhileParsingValue));
            }
        };

        let value = match peek {
            b'{' => {
                check_recursion! {
                    self.eat_char();
                    let ret = visitor.visit_map(MapAccess::new(self));
                }

                match (ret, self.end_map()) {
                    (Ok(ret), Ok(())) => Ok(ret),
                    (Err(err), _) | (_, Err(err)) => Err(err),
                }
            }
            _ => Err(self.peek_invalid_type(&visitor)),
        };

        match value {
            Ok(value) => Ok(value),
            Err(err) => Err(self.fix_position(err)),
        }
    }

    fn deserialize_struct<V>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value>
    where
        V: de::Visitor<'de>,
    {
        let peek = match tri!(self.parse_whitespace()) {
            Some(b) => b,
            None => {
                return Err(self.peek_error(ErrorCode::EofWhileParsingValue));
            }
        };

        let value = match peek {
            b'[' => {
                check_recursion! {
                    self.eat_char();
                    let ret = visitor.visit_seq(SeqAccess::new(self));
                }

                match (ret, self.end_seq()) {
                    (Ok(ret), Ok(())) => Ok(ret),
                    (Err(err), _) | (_, Err(err)) => Err(err),
                }
            }
            b'{' => {
                check_recursion! {
                    self.eat_char();
                    let ret = visitor.visit_map(MapAccess::new(self));
                }

                match (ret, self.end_map()) {
                    (Ok(ret), Ok(())) => Ok(ret),
                    (Err(err), _) | (_, Err(err)) => Err(err),
                }
            }
            _ => Err(self.peek_invalid_type(&visitor)),
        };

        match value {
            Ok(value) => Ok(value),
            Err(err) => Err(self.fix_position(err)),
        }
    }

    #[inline]
    fn deserialize_enum<V>(
        self,
        _name: &str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value>
    where
        V: de::Visitor<'de>,
    {
        match tri!(self.parse_whitespace()) {
            Some(b'{') => {
                check_recursion! {
                    self.eat_char();
                    let ret = visitor.visit_enum(VariantAccess::new(self));
                }
                let value = tri!(ret);

                match tri!(self.parse_whitespace()) {
                    Some(b'}') => {
                        self.eat_char();
                        Ok(value)
                    }
                    Some(_) => Err(self.error(ErrorCode::ExpectedSomeValue)),
                    None => Err(self.error(ErrorCode::EofWhileParsingObject)),
                }
            }
            Some(b'"') => visitor.visit_enum(UnitVariantAccess::new(self)),
            Some(_) => Err(self.peek_error(ErrorCode::ExpectedSomeValue)),
            None => Err(self.peek_error(ErrorCode::EofWhileParsingValue)),
        }
    }

    fn deserialize_identifier<V>(self, visitor: V) -> Result<V::Value>
    where
        V: de::Visitor<'de>,
    {
        self.deserialize_str(visitor)
    }

    fn deserialize_ignored_any<V>(self, visitor: V) -> Result<V::Value>
    where
        V: de::Visitor<'de>,
    {
        tri!(self.ignore_value());
        visitor.visit_unit()
    }
}

struct SeqAccess<'a, R: 'a> {
    de: &'a mut Deserializer<R>,
    first: bool,
}

impl<'a, R: 'a> SeqAccess<'a, R> {
    fn new(de: &'a mut Deserializer<R>) -> Self {
        SeqAccess { de, first: true }
    }
}

impl<'de, 'a, R: Read<'de> + 'a> de::SeqAccess<'de> for SeqAccess<'a, R> {
    type Error = Error;

    fn next_element_seed<T>(&mut self, seed: T) -> Result<Option<T::Value>>
    where
        T: de::DeserializeSeed<'de>,
    {
        fn has_next_element<'de, 'a, R: Read<'de> + 'a>(
            seq: &mut SeqAccess<'a, R>,
        ) -> Result<bool> {
            let peek = match tri!(seq.de.parse_whitespace()) {
                Some(b) => b,
                None => {
                    return Err(seq.de.peek_error(ErrorCode::EofWhileParsingList));
                }
            };

            if peek == b']' {
                Ok(false)
            } else if seq.first {
                seq.first = false;
                Ok(true)
            } else if peek == b',' {
                seq.de.eat_char();
                match tri!(seq.de.parse_whitespace()) {
                    Some(b']') => Err(seq.de.peek_error(ErrorCode::TrailingComma)),
                    Some(_) => Ok(true),
                    None => Err(seq.de.peek_error(ErrorCode::EofWhileParsingValue)),
                }
            } else {
                Err(seq.de.peek_error(ErrorCode::ExpectedListCommaOrEnd))
            }
        }

        if tri!(has_next_element(self)) {
            Ok(Some(tri!(seed.deserialize(&mut *self.de))))
        } else {
            Ok(None)
        }
    }
}

struct MapAccess<'a, R: 'a> {
    de: &'a mut Deserializer<R>,
    first: bool,
}

impl<'a, R: 'a> MapAccess<'a, R> {
    fn new(de: &'a mut Deserializer<R>) -> Self {
        MapAccess { de, first: true }
    }
}

impl<'de, 'a, R: Read<'de> + 'a> de::MapAccess<'de> for MapAccess<'a, R> {
    type Error = Error;

    fn next_key_seed<K>(&mut self, seed: K) -> Result<Option<K::Value>>
    where
        K: de::DeserializeSeed<'de>,
    {
        fn has_next_key<'de, 'a, R: Read<'de> + 'a>(map: &mut MapAccess<'a, R>) -> Result<bool> {
            let peek = match tri!(map.de.parse_whitespace()) {
                Some(b) => b,
                None => {
                    return Err(map.de.peek_error(ErrorCode::EofWhileParsingObject));
                }
            };

            if peek == b'}' {
                Ok(false)
            } else if map.first {
                map.first = false;
                if peek == b'"' {
                    Ok(true)
                } else {
                    Err(map.de.peek_error(ErrorCode::KeyMustBeAString))
                }
            } else if peek == b',' {
                map.de.eat_char();
                match tri!(map.de.parse_whitespace()) {
                    Some(b'"') => Ok(true),
                    Some(b'}') => Err(map.de.peek_error(ErrorCode::TrailingComma)),
                    Some(_) => Err(map.de.peek_error(ErrorCode::KeyMustBeAString)),
                    None => Err(map.de.peek_error(ErrorCode::EofWhileParsingValue)),
                }
            } else {
                Err(map.de.peek_error(ErrorCode::ExpectedObjectCommaOrEnd))
            }
        }

        if tri!(has_next_key(self)) {
            Ok(Some(tri!(seed.deserialize(MapKey { de: &mut *self.de }))))
        } else {
            Ok(None)
        }
    }

    fn next_value_seed<V>(&mut self, seed: V) -> Result<V::Value>
    where
        V: de::DeserializeSeed<'de>,
    {
        tri!(self.de.parse_object_colon());

        seed.deserialize(&mut *self.de)
    }
}

struct VariantAccess<'a, R: 'a> {
    de: &'a mut Deserializer<R>,
}

impl<'a, R: 'a> VariantAccess<'a, R> {
    fn new(de: &'a mut Deserializer<R>) -> Self {
        VariantAccess { de }
    }
}

impl<'de, 'a, R: Read<'de> + 'a> de::EnumAccess<'de> for VariantAccess<'a, R> {
    type Error = Error;
    type Variant = Self;

    fn variant_seed<V>(self, seed: V) -> Result<(V::Value, Self)>
    where
        V: de::DeserializeSeed<'de>,
    {
        match tri!(self.de.parse_whitespace()) {
            Some(b'"') => {}
            Some(b'}') => return Err(self.de.peek_error(ErrorCode::ExpectedSomeValue)),
            Some(_) => return Err(self.de.peek_error(ErrorCode::KeyMustBeAString)),
            None => return Err(self.de.peek_error(ErrorCode::EofWhileParsingObject)),
        }

        let val = match seed.deserialize(MapKey { de: &mut *self.de }) {
            Ok(value) => value,
            Err(err) => return Err(self.de.fix_position(err)),
        };
        tri!(self.de.parse_object_colon());
        Ok((val, self))
    }
}

impl<'de, 'a, R: Read<'de> + 'a> de::VariantAccess<'de> for VariantAccess<'a, R> {
    type Error = Error;

    fn unit_variant(self) -> Result<()> {
        de::Deserialize::deserialize(self.de)
    }

    fn newtype_variant_seed<T>(self, seed: T) -> Result<T::Value>
    where
        T: de::DeserializeSeed<'de>,
    {
        seed.deserialize(self.de)
    }

    fn tuple_variant<V>(self, _len: usize, visitor: V) -> Result<V::Value>
    where
        V: de::Visitor<'de>,
    {
        de::Deserializer::deserialize_seq(self.de, visitor)
    }

    fn struct_variant<V>(self, fields: &'static [&'static str], visitor: V) -> Result<V::Value>
    where
        V: de::Visitor<'de>,
    {
        de::Deserializer::deserialize_struct(self.de, "", fields, visitor)
    }
}

struct UnitVariantAccess<'a, R: 'a> {
    de: &'a mut Deserializer<R>,
}

impl<'a, R: 'a> UnitVariantAccess<'a, R> {
    fn new(de: &'a mut Deserializer<R>) -> Self {
        UnitVariantAccess { de }
    }
}

impl<'de, 'a, R: Read<'de> + 'a> de::EnumAccess<'de> for UnitVariantAccess<'a, R> {
    type Error = Error;
    type Variant = Self;

    fn variant_seed<V>(self, seed: V) -> Result<(V::Value, Self)>
    where
        V: de::DeserializeSeed<'de>,
    {
        let variant = tri!(seed.deserialize(&mut *self.de));
        Ok((variant, self))
    }
}

impl<'de, 'a, R: Read<'de> + 'a> de::VariantAccess<'de> for UnitVariantAccess<'a, R> {
    type Error = Error;

    fn unit_variant(self) -> Result<()> {
        Ok(())
    }

    fn newtype_variant_seed<T>(self, _seed: T) -> Result<T::Value>
    where
        T: de::DeserializeSeed<'de>,
    {
        Err(de::Error::invalid_type(
            Unexpected::UnitVariant,
            &"newtype variant",
        ))
    }

    fn tuple_variant<V>(self, _len: usize, _visitor: V) -> Result<V::Value>
    where
        V: de::Visitor<'de>,
    {
        Err(de::Error::invalid_type(
            Unexpected::UnitVariant,
            &"tuple variant",
        ))
    }

    fn struct_variant<V>(self, _fields: &'static [&'static str], _visitor: V) -> Result<V::Value>
    where
        V: de::Visitor<'de>,
    {
        Err(de::Error::invalid_type(
            Unexpected::UnitVariant,
            &"struct variant",
        ))
    }
}

struct MapKey<'a, R: 'a> {
    de: &'a mut Deserializer<R>,
}

macro_rules! deserialize_numeric_key {
    ($method:ident) => {
        fn $method<V>(self, visitor: V) -> Result<V::Value>
        where
            V: de::Visitor<'de>,
        {
            self.deserialize_number(visitor)
        }
    };

    ($method:ident, $delegate:ident) => {
        fn $method<V>(self, visitor: V) -> Result<V::Value>
        where
            V: de::Visitor<'de>,
        {
            self.de.eat_char();

            match tri!(self.de.peek()) {
                Some(b'0'..=b'9' | b'-') => {}
                _ => return Err(self.de.error(ErrorCode::ExpectedNumericKey)),
            }

            let value = tri!(self.de.$delegate(visitor));

            match tri!(self.de.peek()) {
                Some(b'"') => self.de.eat_char(),
                _ => return Err(self.de.peek_error(ErrorCode::ExpectedDoubleQuote)),
            }

            Ok(value)
        }
    };
}

impl<'de, 'a, R> MapKey<'a, R>
where
    R: Read<'de>,
{
    deserialize_numeric_key!(deserialize_number, deserialize_number);
}

impl<'de, 'a, R> de::Deserializer<'de> for MapKey<'a, R>
where
    R: Read<'de>,
{
    type Error = Error;

    #[inline]
    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value>
    where
        V: de::Visitor<'de>,
    {
        self.de.eat_char();
        self.de.scratch.clear();
        match tri!(self.de.read.parse_str(&mut self.de.scratch)) {
            Reference::Borrowed(s) => visitor.visit_borrowed_str(s),
            Reference::Copied(s) => visitor.visit_str(s),
        }
    }

    deserialize_numeric_key!(deserialize_i8);
    deserialize_numeric_key!(deserialize_i16);
    deserialize_numeric_key!(deserialize_i32);
    deserialize_numeric_key!(deserialize_i64);
    deserialize_numeric_key!(deserialize_i128, deserialize_i128);
    deserialize_numeric_key!(deserialize_u8);
    deserialize_numeric_key!(deserialize_u16);
    deserialize_numeric_key!(deserialize_u32);
    deserialize_numeric_key!(deserialize_u64);
    deserialize_numeric_key!(deserialize_u128, deserialize_u128);
    deserialize_numeric_key!(deserialize_f32, deserialize_f32);
    deserialize_numeric_key!(deserialize_f64);

    fn deserialize_bool<V>(self, visitor: V) -> Result<V::Value>
    where
        V: de::Visitor<'de>,
    {
        self.de.eat_char();

        let peek = match tri!(self.de.next_char()) {
            Some(b) => b,
            None => {
                return Err(self.de.peek_error(ErrorCode::EofWhileParsingValue));
            }
        };

        let value = match peek {
            b't' => {
                tri!(self.de.parse_ident(b"rue\""));
                visitor.visit_bool(true)
            }
            b'f' => {
                tri!(self.de.parse_ident(b"alse\""));
                visitor.visit_bool(false)
            }
            _ => {
                self.de.scratch.clear();
                let s = tri!(self.de.read.parse_str(&mut self.de.scratch));
                Err(de::Error::invalid_type(Unexpected::Str(&s), &visitor))
            }
        };

        match value {
            Ok(value) => Ok(value),
            Err(err) => Err(self.de.fix_position(err)),
        }
    }

    #[inline]
    fn deserialize_option<V>(self, visitor: V) -> Result<V::Value>
    where
        V: de::Visitor<'de>,
    {
        visitor.visit_some(self)
    }

    #[inline]
    fn deserialize_newtype_struct<V>(self, name: &'static str, visitor: V) -> Result<V::Value>
    where
        V: de::Visitor<'de>,
    {
        {
            if name == crate::raw::TOKEN {
                return self.de.deserialize_raw_value(visitor);
            }
        }

        let _ = name;
        visitor.visit_newtype_struct(self)
    }

    #[inline]
    fn deserialize_enum<V>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value>
    where
        V: de::Visitor<'de>,
    {
        self.de.deserialize_enum(name, variants, visitor)
    }

    #[inline]
    fn deserialize_bytes<V>(self, visitor: V) -> Result<V::Value>
    where
        V: de::Visitor<'de>,
    {
        self.de.deserialize_bytes(visitor)
    }

    #[inline]
    fn deserialize_byte_buf<V>(self, visitor: V) -> Result<V::Value>
    where
        V: de::Visitor<'de>,
    {
        self.de.deserialize_bytes(visitor)
    }

    forward_to_deserialize_any! {
        char str string unit unit_struct seq tuple tuple_struct map struct
        identifier ignored_any
    }
}

pub struct StreamDeserializer<'de, R, T> {
    de: Deserializer<R>,
    offset: usize,
    failed: bool,
    output: PhantomData<T>,
    lifetime: PhantomData<&'de ()>,
}

impl<'de, R, T> StreamDeserializer<'de, R, T>
where
    R: read::Read<'de>,
    T: de::Deserialize<'de>,
{
    pub fn new(read: R) -> Self {
        let offset = read.byte_offset();
        StreamDeserializer {
            de: Deserializer::new(read),
            offset,
            failed: false,
            output: PhantomData,
            lifetime: PhantomData,
        }
    }

    pub fn byte_offset(&self) -> usize {
        self.offset
    }

    fn peek_end_of_value(&mut self) -> Result<()> {
        match tri!(self.de.peek()) {
            Some(b' ' | b'\n' | b'\t' | b'\r' | b'"' | b'[' | b']' | b'{' | b'}' | b',' | b':')
            | None => Ok(()),
            Some(_) => {
                let position = self.de.read.peek_position();
                Err(Error::syntax(
                    ErrorCode::TrailingCharacters,
                    position.line,
                    position.column,
                ))
            }
        }
    }
}

impl<'de, R, T> Iterator for StreamDeserializer<'de, R, T>
where
    R: Read<'de>,
    T: de::Deserialize<'de>,
{
    type Item = Result<T>;

    fn next(&mut self) -> Option<Result<T>> {
        if R::should_early_return_if_failed && self.failed {
            return None;
        }

        match self.de.parse_whitespace() {
            Ok(None) => {
                self.offset = self.de.read.byte_offset();
                None
            }
            Ok(Some(b)) => {
                let self_delineated_value = match b {
                    b'[' | b'"' | b'{' => true,
                    _ => false,
                };
                self.offset = self.de.read.byte_offset();
                let result = de::Deserialize::deserialize(&mut self.de);

                Some(match result {
                    Ok(value) => {
                        self.offset = self.de.read.byte_offset();
                        if self_delineated_value {
                            Ok(value)
                        } else {
                            self.peek_end_of_value().map(|()| value)
                        }
                    }
                    Err(e) => {
                        self.de.read.set_failed(&mut self.failed);
                        Err(e)
                    }
                })
            }
            Err(e) => {
                self.de.read.set_failed(&mut self.failed);
                Some(Err(e))
            }
        }
    }
}

impl<'de, R, T> FusedIterator for StreamDeserializer<'de, R, T>
where
    R: Read<'de> + Fused,
    T: de::Deserialize<'de>,
{
}

fn from_trait<'de, R, T>(read: R) -> Result<T>
where
    R: Read<'de>,
    T: de::Deserialize<'de>,
{
    let mut de = Deserializer::new(read);
    let value = tri!(de::Deserialize::deserialize(&mut de));

    tri!(de.end());
    Ok(value)
}

pub fn from_reader<R, T>(rdr: R) -> Result<T>
where
    R: crate::io::Read,
    T: de::DeserializeOwned,
{
    from_trait(read::IoRead::new(rdr))
}

pub fn from_slice<'a, T>(v: &'a [u8]) -> Result<T>
where
    T: de::Deserialize<'a>,
{
    from_trait(read::SliceRead::new(v))
}

pub fn from_str<'a, T>(s: &'a str) -> Result<T>
where
    T: de::Deserialize<'a>,
{
    from_trait(read::StrRead::new(s))
}
