use crate::lib::*;

mod fmt;
mod impls;
mod impossible;

pub use self::impossible::Impossible;

#[doc(no_inline)]
pub use std::error::Error as StdError;

macro_rules! declare_error_trait {
    (Error: Sized $(+ $($supertrait:ident)::+)*) => {
        #[diagnostic::on_unimplemented(
                message = "the trait bound `{Self}: serde::ser::Error` is not satisfied",
            )]
        pub trait Error: Sized $(+ $($supertrait)::+)* {
            fn custom<T>(msg: T) -> Self
            where
                T: Display;
        }
    }
}

declare_error_trait!(Error: Sized + StdError);

#[diagnostic::on_unimplemented(
        message = "the trait bound `{Self}: serde::Serialize` is not satisfied",
        note = "for local types consider adding `#[derive(serde::Serialize)]` to your `{Self}` type",
        note = "for types from other crates check whether the crate offers a `serde` feature flag",
    )]
pub trait Serialize {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer;
}

#[diagnostic::on_unimplemented(
        message = "the trait bound `{Self}: serde::Serializer` is not satisfied",
    )]
pub trait Serializer: Sized {
    type Ok;

    type Error: Error;

    type SerializeSeq: SerializeSeq<Ok = Self::Ok, Error = Self::Error>;

    type SerializeTuple: SerializeTuple<Ok = Self::Ok, Error = Self::Error>;

    type SerializeTupleStruct: SerializeTupleStruct<Ok = Self::Ok, Error = Self::Error>;

    type SerializeTupleVariant: SerializeTupleVariant<Ok = Self::Ok, Error = Self::Error>;

    type SerializeMap: SerializeMap<Ok = Self::Ok, Error = Self::Error>;

    type SerializeStruct: SerializeStruct<Ok = Self::Ok, Error = Self::Error>;

    type SerializeStructVariant: SerializeStructVariant<Ok = Self::Ok, Error = Self::Error>;

    fn serialize_bool(self, v: bool) -> Result<Self::Ok, Self::Error>;

    fn serialize_i8(self, v: i8) -> Result<Self::Ok, Self::Error>;

    fn serialize_i16(self, v: i16) -> Result<Self::Ok, Self::Error>;

    fn serialize_i32(self, v: i32) -> Result<Self::Ok, Self::Error>;

    fn serialize_i64(self, v: i64) -> Result<Self::Ok, Self::Error>;

    fn serialize_i128(self, v: i128) -> Result<Self::Ok, Self::Error> {
        let _ = v;
        Err(Error::custom("i128 is not supported"))
    }

    fn serialize_u8(self, v: u8) -> Result<Self::Ok, Self::Error>;

    fn serialize_u16(self, v: u16) -> Result<Self::Ok, Self::Error>;

    fn serialize_u32(self, v: u32) -> Result<Self::Ok, Self::Error>;

    fn serialize_u64(self, v: u64) -> Result<Self::Ok, Self::Error>;

    fn serialize_u128(self, v: u128) -> Result<Self::Ok, Self::Error> {
        let _ = v;
        Err(Error::custom("u128 is not supported"))
    }

    fn serialize_f32(self, v: f32) -> Result<Self::Ok, Self::Error>;

    fn serialize_f64(self, v: f64) -> Result<Self::Ok, Self::Error>;

    fn serialize_char(self, v: char) -> Result<Self::Ok, Self::Error>;

    fn serialize_str(self, v: &str) -> Result<Self::Ok, Self::Error>;

    fn serialize_bytes(self, v: &[u8]) -> Result<Self::Ok, Self::Error>;

    fn serialize_none(self) -> Result<Self::Ok, Self::Error>;

    fn serialize_some<T>(self, value: &T) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + Serialize;

    fn serialize_unit(self) -> Result<Self::Ok, Self::Error>;

    fn serialize_unit_struct(self, name: &'static str) -> Result<Self::Ok, Self::Error>;

    fn serialize_unit_variant(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
    ) -> Result<Self::Ok, Self::Error>;

    fn serialize_newtype_struct<T>(
        self,
        name: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + Serialize;

    fn serialize_newtype_variant<T>(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + Serialize;

    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error>;

    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, Self::Error>;

    fn serialize_tuple_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error>;

    fn serialize_tuple_variant(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error>;

    fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap, Self::Error>;

    fn serialize_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error>;

    fn serialize_struct_variant(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error>;

    fn collect_seq<I>(self, iter: I) -> Result<Self::Ok, Self::Error>
    where
        I: IntoIterator,
        <I as IntoIterator>::Item: Serialize,
    {
        let mut iter = iter.into_iter();
        let mut serializer = tri!(self.serialize_seq(iterator_len_hint(&iter)));
        tri!(iter.try_for_each(|item| serializer.serialize_element(&item)));
        serializer.end()
    }

    fn collect_map<K, V, I>(self, iter: I) -> Result<Self::Ok, Self::Error>
    where
        K: Serialize,
        V: Serialize,
        I: IntoIterator<Item = (K, V)>,
    {
        let mut iter = iter.into_iter();
        let mut serializer = tri!(self.serialize_map(iterator_len_hint(&iter)));
        tri!(iter.try_for_each(|(key, value)| serializer.serialize_entry(&key, &value)));
        serializer.end()
    }

    fn collect_str<T>(self, value: &T) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + Display,
    {
        self.serialize_str(&value.to_string())
    }

    #[inline]
    fn is_human_readable(&self) -> bool {
        true
    }
}

#[diagnostic::on_unimplemented(
        message = "the trait bound `{Self}: serde::ser::SerializeSeq` is not satisfied",
    )]
pub trait SerializeSeq {
    type Ok;

    type Error: Error;

    fn serialize_element<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize;

    fn end(self) -> Result<Self::Ok, Self::Error>;
}

#[diagnostic::on_unimplemented(
        message = "the trait bound `{Self}: serde::ser::SerializeTuple` is not satisfied",
    )]
pub trait SerializeTuple {
    type Ok;

    type Error: Error;

    fn serialize_element<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize;

    fn end(self) -> Result<Self::Ok, Self::Error>;
}

#[diagnostic::on_unimplemented(
        message = "the trait bound `{Self}: serde::ser::SerializeTupleStruct` is not satisfied",
    )]
pub trait SerializeTupleStruct {
    type Ok;

    type Error: Error;

    fn serialize_field<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize;

    fn end(self) -> Result<Self::Ok, Self::Error>;
}

#[diagnostic::on_unimplemented(
        message = "the trait bound `{Self}: serde::ser::SerializeTupleVariant` is not satisfied",
    )]
pub trait SerializeTupleVariant {
    type Ok;

    type Error: Error;

    fn serialize_field<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize;

    fn end(self) -> Result<Self::Ok, Self::Error>;
}

#[diagnostic::on_unimplemented(
        message = "the trait bound `{Self}: serde::ser::SerializeMap` is not satisfied",
    )]
pub trait SerializeMap {
    type Ok;

    type Error: Error;

    fn serialize_key<T>(&mut self, key: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize;

    fn serialize_value<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize;

    fn serialize_entry<K, V>(&mut self, key: &K, value: &V) -> Result<(), Self::Error>
    where
        K: ?Sized + Serialize,
        V: ?Sized + Serialize,
    {
        tri!(self.serialize_key(key));
        self.serialize_value(value)
    }

    fn end(self) -> Result<Self::Ok, Self::Error>;
}

#[diagnostic::on_unimplemented(
        message = "the trait bound `{Self}: serde::ser::SerializeStruct` is not satisfied",
    )]
pub trait SerializeStruct {
    type Ok;

    type Error: Error;

    fn serialize_field<T>(&mut self, key: &'static str, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize;

    #[inline]
    fn skip_field(&mut self, key: &'static str) -> Result<(), Self::Error> {
        let _ = key;
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error>;
}

#[diagnostic::on_unimplemented(
        message = "the trait bound `{Self}: serde::ser::SerializeStructVariant` is not satisfied",
    )]
pub trait SerializeStructVariant {
    type Ok;

    type Error: Error;

    fn serialize_field<T>(&mut self, key: &'static str, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize;

    #[inline]
    fn skip_field(&mut self, key: &'static str) -> Result<(), Self::Error> {
        let _ = key;
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error>;
}

fn iterator_len_hint<I>(iter: &I) -> Option<usize>
where
    I: Iterator,
{
    match iter.size_hint() {
        (lo, Some(hi)) if lo == hi => Some(lo),
        _ => None,
    }
}
