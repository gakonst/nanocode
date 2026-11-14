use crate::lib::*;

use crate::de::{
    Deserialize, Deserializer, EnumAccess, Error, MapAccess, SeqAccess, VariantAccess, Visitor,
};

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct IgnoredAny;

impl<'de> Visitor<'de> for IgnoredAny {
    type Value = IgnoredAny;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("anything at all")
    }

    #[inline]
    fn visit_bool<E>(self, x: bool) -> Result<Self::Value, E> {
        let _ = x;
        Ok(IgnoredAny)
    }

    #[inline]
    fn visit_i64<E>(self, x: i64) -> Result<Self::Value, E> {
        let _ = x;
        Ok(IgnoredAny)
    }

    #[inline]
    fn visit_i128<E>(self, x: i128) -> Result<Self::Value, E> {
        let _ = x;
        Ok(IgnoredAny)
    }

    #[inline]
    fn visit_u64<E>(self, x: u64) -> Result<Self::Value, E> {
        let _ = x;
        Ok(IgnoredAny)
    }

    #[inline]
    fn visit_u128<E>(self, x: u128) -> Result<Self::Value, E> {
        let _ = x;
        Ok(IgnoredAny)
    }

    #[inline]
    fn visit_f64<E>(self, x: f64) -> Result<Self::Value, E> {
        let _ = x;
        Ok(IgnoredAny)
    }

    #[inline]
    fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
    where
        E: Error,
    {
        let _ = s;
        Ok(IgnoredAny)
    }

    #[inline]
    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(IgnoredAny)
    }

    #[inline]
    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        IgnoredAny::deserialize(deserializer)
    }

    #[inline]
    fn visit_newtype_struct<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        IgnoredAny::deserialize(deserializer)
    }

    #[inline]
    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(IgnoredAny)
    }

    #[inline]
    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        while let Some(IgnoredAny) = tri!(seq.next_element()) {
        }
        Ok(IgnoredAny)
    }

    #[inline]
    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        while let Some((IgnoredAny, IgnoredAny)) = tri!(map.next_entry()) {
        }
        Ok(IgnoredAny)
    }

    #[inline]
    fn visit_bytes<E>(self, bytes: &[u8]) -> Result<Self::Value, E>
    where
        E: Error,
    {
        let _ = bytes;
        Ok(IgnoredAny)
    }

    fn visit_enum<A>(self, data: A) -> Result<Self::Value, A::Error>
    where
        A: EnumAccess<'de>,
    {
        tri!(data.variant::<IgnoredAny>()).1.newtype_variant()
    }
}

impl<'de> Deserialize<'de> for IgnoredAny {
    #[inline]
    fn deserialize<D>(deserializer: D) -> Result<IgnoredAny, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_ignored_any(IgnoredAny)
    }
}
