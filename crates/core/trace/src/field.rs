use std::{
    borrow::Borrow,
    fmt::{self, Write},
    hash::{Hash, Hasher},
    num::{
        NonZeroI8, NonZeroI16, NonZeroI32, NonZeroI64, NonZeroI128, NonZeroIsize, NonZeroU8,
        NonZeroU16, NonZeroU32, NonZeroU64, NonZeroU128, NonZeroUsize, Wrapping,
    },
    ops::Range,
};

use crate::{Metadata, callsite::Identifier};

#[derive(Debug)]
pub struct Field {
    index: usize,
    fields: FieldSet,
}

#[derive(Debug, Eq, PartialEq)]
pub struct Empty;

#[derive(Clone, Copy)]
pub struct FieldSet {
    names: &'static [&'static str],
    callsite: Identifier,
}

pub struct ValueSet<'a> {
    values: Values<'a>,
    fields: &'a FieldSet,
}

type Pair<'a> = (&'a Field, Option<&'a (dyn Value + 'a)>);

enum Values<'a> {
    Explicit(&'a [Pair<'a>]),
    All(&'a [Option<&'a (dyn Value + 'a)>]),
}

#[derive(Debug)]
pub struct Iter {
    indexes: Range<usize>,
    fields: FieldSet,
}

pub trait Visit {
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.record_debug(field, &value);
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.record_debug(field, &value);
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.record_debug(field, &value);
    }

    fn record_i128(&mut self, field: &Field, value: i128) {
        self.record_debug(field, &value);
    }

    fn record_u128(&mut self, field: &Field, value: u128) {
        self.record_debug(field, &value);
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.record_debug(field, &value);
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.record_debug(field, &value);
    }

    fn record_bytes(&mut self, field: &Field, value: &[u8]) {
        self.record_debug(field, &HexBytes(value));
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.record_debug(field, &DisplayValue(value));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug);
}

pub trait Value {
    fn record(&self, key: &Field, visitor: &mut dyn Visit);
}

pub trait AsField {
    fn as_field(&self, metadata: &Metadata<'_>) -> Option<Field>;
}

#[derive(Clone)]
pub struct DisplayValue<T: fmt::Display>(T);

#[derive(Clone)]
pub struct DebugValue<T: fmt::Debug>(T);

pub const fn display<T: fmt::Display>(value: T) -> DisplayValue<T> {
    DisplayValue(value)
}

pub const fn debug<T: fmt::Debug>(value: T) -> DebugValue<T> {
    DebugValue(value)
}

struct HexBytes<'a>(&'a [u8]);

impl fmt::Debug for HexBytes<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_char('[')?;
        let mut bytes = self.0.iter();
        if let Some(byte) = bytes.next() {
            write!(formatter, "{byte:02x}")?;
        }
        for byte in bytes {
            write!(formatter, " {byte:02x}")?;
        }
        formatter.write_char(']')
    }
}

impl Visit for fmt::DebugStruct<'_, '_> {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.field(field.name(), value);
    }
}

impl Visit for fmt::DebugMap<'_, '_> {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.entry(&format_args!("{field}"), value);
    }
}

impl<F> Visit for F
where
    F: FnMut(&Field, &dyn fmt::Debug),
{
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        (self)(field, value);
    }
}

macro_rules! values {
    ($record:ident, $wide:ty: $($narrow:ty, $nonzero:ty);+) => {
        $(
            impl Value for $narrow {
                fn record(&self, key: &Field, visitor: &mut dyn Visit) {
                    visitor.$record(key, <$wide>::from(*self));
                }
            }

            impl Value for $nonzero {
                fn record(&self, key: &Field, visitor: &mut dyn Visit) {
                    visitor.$record(key, <$wide>::from(self.get()));
                }
            }
        )+
    };
}

values!(record_u64, u64: u64, NonZeroU64; u32, NonZeroU32; u16, NonZeroU16; u8, NonZeroU8);
values!(record_i64, i64: i64, NonZeroI64; i32, NonZeroI32; i16, NonZeroI16; i8, NonZeroI8);
values!(record_u128, u128: u128, NonZeroU128);
values!(record_i128, i128: i128, NonZeroI128);

impl Value for usize {
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        visitor.record_u64(key, u64::try_from(*self).expect("usize fits in u64"));
    }
}

impl Value for NonZeroUsize {
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        self.get().record(key, visitor);
    }
}

impl Value for isize {
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        visitor.record_i64(key, i64::try_from(*self).expect("isize fits in i64"));
    }
}

impl Value for NonZeroIsize {
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        self.get().record(key, visitor);
    }
}

impl Value for bool {
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        visitor.record_bool(key, *self);
    }
}

impl Value for f64 {
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        visitor.record_f64(key, *self);
    }
}

impl Value for f32 {
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        visitor.record_f64(key, f64::from(*self));
    }
}

impl<T: Value> Value for Wrapping<T> {
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        self.0.record(key, visitor);
    }
}

impl Value for str {
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        visitor.record_str(key, self);
    }
}

impl Value for [u8] {
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        visitor.record_bytes(key, self);
    }
}

impl Value for dyn std::error::Error + 'static {
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        visitor.record_error(key, self);
    }
}

impl Value for dyn std::error::Error + Send + 'static {
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        visitor.record_error(key, self);
    }
}

impl Value for dyn std::error::Error + Sync + 'static {
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        visitor.record_error(key, self);
    }
}

impl Value for dyn std::error::Error + Send + Sync + 'static {
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        visitor.record_error(key, self);
    }
}

impl<T: Value + ?Sized> Value for &T {
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        (*self).record(key, visitor);
    }
}

impl<T: Value + ?Sized> Value for &mut T {
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        T::record(self, key, visitor);
    }
}

impl Value for fmt::Arguments<'_> {
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        visitor.record_debug(key, self);
    }
}

impl<T: Value + ?Sized> Value for Box<T> {
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        self.as_ref().record(key, visitor);
    }
}

impl Value for String {
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        visitor.record_str(key, self.as_str());
    }
}

impl<T: fmt::Display> Value for DisplayValue<T> {
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        visitor.record_debug(key, self);
    }
}

impl<T: fmt::Display> fmt::Debug for DisplayValue<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, formatter)
    }
}

impl<T: fmt::Display> fmt::Display for DisplayValue<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl<T: fmt::Debug> Value for DebugValue<T> {
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        visitor.record_debug(key, &self.0);
    }
}

impl<T: fmt::Debug> fmt::Debug for DebugValue<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Value for Empty {
    #[inline]
    fn record(&self, _: &Field, _: &mut dyn Visit) {}
}

impl<T: Value> Value for Option<T> {
    fn record(&self, key: &Field, visitor: &mut dyn Visit) {
        if let Some(value) = self {
            value.record(key, visitor);
        }
    }
}

impl Field {
    #[must_use]
    pub const fn callsite(&self) -> Identifier {
        self.fields.callsite
    }

    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.fields.names[self.index]
    }

    #[must_use]
    pub const fn index(&self) -> usize {
        self.index
    }
}

impl fmt::Display for Field {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.pad(self.name())
    }
}

impl AsRef<str> for Field {
    fn as_ref(&self) -> &str {
        self.name()
    }
}

impl PartialEq for Field {
    fn eq(&self, other: &Self) -> bool {
        self.callsite() == other.callsite() && self.index == other.index
    }
}

impl Eq for Field {}

impl Hash for Field {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.callsite().hash(state);
        self.index.hash(state);
    }
}

impl Clone for Field {
    fn clone(&self) -> Self {
        Self {
            index: self.index,
            fields: self.fields,
        }
    }
}

impl AsField for Field {
    fn as_field(&self, metadata: &Metadata<'_>) -> Option<Field> {
        (self.callsite() == metadata.callsite()).then(|| self.clone())
    }
}

impl AsField for &Field {
    fn as_field(&self, metadata: &Metadata<'_>) -> Option<Field> {
        (*self).as_field(metadata)
    }
}

impl AsField for str {
    fn as_field(&self, metadata: &Metadata<'_>) -> Option<Field> {
        metadata.fields().field(self)
    }
}

impl FieldSet {
    #[must_use]
    pub const fn new(names: &'static [&'static str], callsite: Identifier) -> Self {
        Self { names, callsite }
    }

    #[must_use]
    pub const fn callsite(&self) -> Identifier {
        self.callsite
    }

    pub fn field<Q: Borrow<str> + ?Sized>(&self, name: &Q) -> Option<Field> {
        let name = name.borrow();
        self.names
            .iter()
            .position(|candidate| *candidate == name)
            .map(|index| Field {
                index,
                fields: *self,
            })
    }

    #[must_use]
    pub fn contains(&self, field: &Field) -> bool {
        field.callsite() == self.callsite && field.index <= self.len()
    }

    #[must_use]
    pub const fn iter(&self) -> Iter {
        Iter {
            indexes: 0..self.names.len(),
            fields: *self,
        }
    }

    #[must_use]
    pub fn value_set<'v>(&'v self, values: &'v [Pair<'v>]) -> ValueSet<'v> {
        ValueSet {
            fields: self,
            values: Values::Explicit(values),
        }
    }

    #[must_use]
    pub fn value_set_all<'v>(&'v self, values: &'v [Option<&'v (dyn Value + 'v)>]) -> ValueSet<'v> {
        assert_eq!(
            values.len(),
            self.len(),
            "a value set has one value per field"
        );
        ValueSet {
            fields: self,
            values: Values::All(values),
        }
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.names.len()
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

impl IntoIterator for &FieldSet {
    type IntoIter = Iter;
    type Item = Field;

    fn into_iter(self) -> Iter {
        self.iter()
    }
}

impl fmt::Debug for FieldSet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FieldSet")
            .field("names", &self.names)
            .field("callsite", &self.callsite)
            .finish()
    }
}

impl fmt::Display for FieldSet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_set()
            .entries(self.names.iter().map(display))
            .finish()
    }
}

impl Eq for FieldSet {}

impl PartialEq for FieldSet {
    fn eq(&self, other: &Self) -> bool {
        self.callsite == other.callsite
    }
}

impl Iterator for Iter {
    type Item = Field;

    fn next(&mut self) -> Option<Field> {
        let index = self.indexes.next()?;
        Some(Field {
            index,
            fields: self.fields,
        })
    }
}

impl ValueSet<'_> {
    #[must_use]
    pub const fn callsite(&self) -> Identifier {
        self.fields.callsite
    }

    pub fn record(&self, visitor: &mut dyn Visit) {
        match self.values {
            Values::Explicit(values) => {
                let callsite = self.callsite();
                for (field, value) in values {
                    if field.callsite() != callsite {
                        continue;
                    }
                    if let Some(value) = *value {
                        value.record(field, visitor);
                    }
                }
            }
            Values::All(values) => {
                for (field, value) in self.fields.iter().zip(values) {
                    if let Some(value) = *value {
                        value.record(&field, visitor);
                    }
                }
            }
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        match self.values {
            Values::Explicit(values) => {
                let callsite = self.callsite();
                values
                    .iter()
                    .filter(|(field, _)| field.callsite() == callsite)
                    .count()
            }
            Values::All(values) => values.len(),
        }
    }

    #[must_use]
    pub fn contains(&self, field: &Field) -> bool {
        if field.callsite() != self.callsite() {
            return false;
        }
        match self.values {
            Values::Explicit(values) => values
                .iter()
                .any(|(key, value)| *key == field && value.is_some()),
            Values::All(values) => values[field.index].is_some(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        match self.values {
            Values::All(values) => values.iter().all(Option::is_none),
            Values::Explicit(values) => {
                let callsite = self.callsite();
                values
                    .iter()
                    .all(|(key, value)| value.is_none() || key.callsite() != callsite)
            }
        }
    }

    #[must_use]
    pub const fn field_set(&self) -> &FieldSet {
        self.fields
    }
}

impl fmt::Debug for ValueSet<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut set = formatter.debug_struct("ValueSet");
        self.record(&mut set);
        set.field("callsite", &self.callsite()).finish()
    }
}

impl fmt::Display for ValueSet<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut map = formatter.debug_map();
        self.record(&mut map);
        map.finish()
    }
}
