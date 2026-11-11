use crate::{ArcSwapAny, RefCnt, Strategy};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

impl<T, S> Serialize for ArcSwapAny<T, S>
where
    T: RefCnt + Serialize,
    S: Strategy<T>,
{
    fn serialize<Ser: Serializer>(&self, serializer: Ser) -> Result<Ser::Ok, Ser::Error> {
        self.load().serialize(serializer)
    }
}

impl<'de, T, S> Deserialize<'de> for ArcSwapAny<T, S>
where
    T: RefCnt + Deserialize<'de>,
    S: Strategy<T> + Default,
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::from(T::deserialize(deserializer)?))
    }
}
