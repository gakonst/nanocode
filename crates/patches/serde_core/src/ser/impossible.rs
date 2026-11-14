use crate::lib::*;

use crate::ser::{
    self, Serialize, SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant,
    SerializeTuple, SerializeTupleStruct, SerializeTupleVariant,
};

pub struct Impossible<Ok, Error> {
    void: Void,
    ok: PhantomData<Ok>,
    error: PhantomData<Error>,
}

enum Void {}

impl<Ok, Error> SerializeSeq for Impossible<Ok, Error>
where
    Error: ser::Error,
{
    type Ok = Ok;
    type Error = Error;

    fn serialize_element<T>(&mut self, value: &T) -> Result<(), Error>
    where
        T: ?Sized + Serialize,
    {
        let _ = value;
        match self.void {}
    }

    fn end(self) -> Result<Ok, Error> {
        match self.void {}
    }
}

impl<Ok, Error> SerializeTuple for Impossible<Ok, Error>
where
    Error: ser::Error,
{
    type Ok = Ok;
    type Error = Error;

    fn serialize_element<T>(&mut self, value: &T) -> Result<(), Error>
    where
        T: ?Sized + Serialize,
    {
        let _ = value;
        match self.void {}
    }

    fn end(self) -> Result<Ok, Error> {
        match self.void {}
    }
}

impl<Ok, Error> SerializeTupleStruct for Impossible<Ok, Error>
where
    Error: ser::Error,
{
    type Ok = Ok;
    type Error = Error;

    fn serialize_field<T>(&mut self, value: &T) -> Result<(), Error>
    where
        T: ?Sized + Serialize,
    {
        let _ = value;
        match self.void {}
    }

    fn end(self) -> Result<Ok, Error> {
        match self.void {}
    }
}

impl<Ok, Error> SerializeTupleVariant for Impossible<Ok, Error>
where
    Error: ser::Error,
{
    type Ok = Ok;
    type Error = Error;

    fn serialize_field<T>(&mut self, value: &T) -> Result<(), Error>
    where
        T: ?Sized + Serialize,
    {
        let _ = value;
        match self.void {}
    }

    fn end(self) -> Result<Ok, Error> {
        match self.void {}
    }
}

impl<Ok, Error> SerializeMap for Impossible<Ok, Error>
where
    Error: ser::Error,
{
    type Ok = Ok;
    type Error = Error;

    fn serialize_key<T>(&mut self, key: &T) -> Result<(), Error>
    where
        T: ?Sized + Serialize,
    {
        let _ = key;
        match self.void {}
    }

    fn serialize_value<T>(&mut self, value: &T) -> Result<(), Error>
    where
        T: ?Sized + Serialize,
    {
        let _ = value;
        match self.void {}
    }

    fn end(self) -> Result<Ok, Error> {
        match self.void {}
    }
}

impl<Ok, Error> SerializeStruct for Impossible<Ok, Error>
where
    Error: ser::Error,
{
    type Ok = Ok;
    type Error = Error;

    fn serialize_field<T>(&mut self, key: &'static str, value: &T) -> Result<(), Error>
    where
        T: ?Sized + Serialize,
    {
        let _ = key;
        let _ = value;
        match self.void {}
    }

    fn end(self) -> Result<Ok, Error> {
        match self.void {}
    }
}

impl<Ok, Error> SerializeStructVariant for Impossible<Ok, Error>
where
    Error: ser::Error,
{
    type Ok = Ok;
    type Error = Error;

    fn serialize_field<T>(&mut self, key: &'static str, value: &T) -> Result<(), Error>
    where
        T: ?Sized + Serialize,
    {
        let _ = key;
        let _ = value;
        match self.void {}
    }

    fn end(self) -> Result<Ok, Error> {
        match self.void {}
    }
}
