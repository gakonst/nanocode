use std::{collections::HashMap, error::Error};

use serde::de::IgnoredAny;
use serde_json::Value;
use std::sync::LazyLock;

#[derive(Clone, Copy)]
pub struct Decoder {
    pub name: &'static str,

    #[allow(clippy::type_complexity)]
    pub func: fn(s: &str) -> Result<Vec<u8>, Box<dyn Error>>,
}

pub(crate) static DECODERS: LazyLock<HashMap<&'static str, Decoder>> = LazyLock::new(|| {
    let mut m = HashMap::<&'static str, Decoder>::new();
    m.insert(
        "base64",
        Decoder {
            name: "base64",
            func: decode_base64,
        },
    );
    m
});

fn decode_base64(s: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    Ok(nanocode_sys::base64::STANDARD.decode(s)?)
}

#[derive(Clone, Copy)]
pub struct MediaType {
    pub name: &'static str,

    pub json_compatible: bool,

    #[allow(clippy::type_complexity)]
    pub func: fn(bytes: &[u8], deserialize: bool) -> Result<Option<Value>, Box<dyn Error>>,
}

pub(crate) static MEDIA_TYPES: LazyLock<HashMap<&'static str, MediaType>> = LazyLock::new(|| {
    let mut m = HashMap::<&'static str, MediaType>::new();
    m.insert(
        "application/json",
        MediaType {
            name: "application/json",
            json_compatible: true,
            func: check_json,
        },
    );
    m
});

fn check_json(bytes: &[u8], deserialize: bool) -> Result<Option<Value>, Box<dyn Error>> {
    if deserialize {
        return Ok(Some(serde_json::from_slice(bytes)?));
    }
    serde_json::from_slice::<IgnoredAny>(bytes)?;
    Ok(None)
}
