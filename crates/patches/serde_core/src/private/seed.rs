use crate::de::{Deserialize, DeserializeSeed, Deserializer};

pub struct InPlaceSeed<'a, T: 'a>(pub &'a mut T);

#[diagnostic::do_not_recommend]
impl<'a, 'de, T> DeserializeSeed<'de> for InPlaceSeed<'a, T>
where
    T: Deserialize<'de>,
{
    type Value = ();
    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        T::deserialize_in_place(deserializer, self.0)
    }
}
