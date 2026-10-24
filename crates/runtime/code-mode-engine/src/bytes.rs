use std::{
    borrow::Cow,
    collections::BTreeMap,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use base64::{
    Engine,
    alphabet::STANDARD,
    engine::{
        DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig,
        general_purpose::STANDARD as STANDARD_ENGINE,
    },
};
use nanocode_platform::Instant;
use rquickjs::{
    ArrayBuffer, CatchResultExt, Ctx, Exception, Object, Value,
    function::{Func, Opt},
};

static BUFFERS: Mutex<BTreeMap<u64, (Instant, Vec<u8>)>> = Mutex::new(BTreeMap::new());
const UNCLAIMED: Duration = Duration::from_secs(600);
static NEXT: AtomicU64 = AtomicU64::new(1);

const LENIENT: GeneralPurpose = GeneralPurpose::new(
    &STANDARD,
    GeneralPurposeConfig::new()
        .with_decode_allow_trailing_bits(true)
        .with_decode_padding_mode(DecodePaddingMode::RequireNone),
);

pub fn put(bytes: Vec<u8>) -> u64 {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let mut buffers = BUFFERS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    buffers.retain(|_, (at, _)| at.elapsed() < UNCLAIMED);
    buffers.insert(id, (Instant::now(), bytes));
    id
}

pub fn take(id: u64) -> Option<Vec<u8>> {
    BUFFERS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&id)
        .map(|(_, bytes)| bytes)
}

const CHUNK: usize = 1 << 20;

const LATIN1: &str = "(buffer, offset, length) => { const view = new Uint8Array(buffer, offset ?? 0, length ?? buffer.byteLength - (offset ?? 0)); let text = \"\"; for (let at = 0; at < view.length; at += 8192) text += String.fromCharCode.apply(null, view.subarray(at, at + 8192)); return text; }";

fn view<'js>(
    ctx: &Ctx<'js>,
    buffer: &ArrayBuffer<'js>,
    offset: Opt<usize>,
    length: Opt<usize>,
) -> rquickjs::Result<Vec<u8>> {
    if buffer.as_raw().is_none() {
        return Err(Exception::throw_type(ctx, "the ArrayBuffer is detached"));
    }
    let size = buffer.len();
    let start = offset.0.unwrap_or(0);
    let end = length
        .0
        .map_or(Some(size), |length| start.checked_add(length))
        .filter(|end| *end <= size && start <= *end)
        .ok_or_else(|| Exception::throw_range(ctx, "byte range is outside the ArrayBuffer"))?;
    let latin1: rquickjs::Function<'js> = ctx.eval(LATIN1)?;
    let mut bytes = Vec::with_capacity(end - start);
    let mut at = start;
    while at < end {
        let length = (end - at).min(CHUNK);
        let text: rquickjs::String<'js> = latin1.call((buffer.clone(), at, length))?;
        bytes.extend(
            text.to_string()?
                .chars()
                .map(|character| character as u32 as u8),
        );
        at += length;
    }
    Ok(bytes)
}

fn source<'js>(
    ctx: &Ctx<'js>,
    value: &Value<'js>,
    offset: Opt<usize>,
    length: Opt<usize>,
) -> rquickjs::Result<Vec<u8>> {
    if let Some(text) = value.as_string() {
        return text
            .to_string()
            .map(String::into_bytes)
            .map_err(|_| Exception::throw_type(ctx, "the string is not well-formed UTF-16"));
    }
    let buffer = ArrayBuffer::from_value(value.clone())
        .ok_or_else(|| Exception::throw_type(ctx, "expected a string or an ArrayBuffer"))?;
    view(ctx, &buffer, offset, length)
}

pub(crate) fn install<'js>(ctx: &Ctx<'js>) -> Result<(), String> {
    let bytes = Object::new(ctx.clone())
        .catch(ctx)
        .map_err(|error| error.to_string())?;
    let put_bytes = Func::from(
        |ctx: Ctx<'js>,
         value: Value<'js>,
         offset: Opt<usize>,
         length: Opt<usize>|
         -> rquickjs::Result<f64> {
            source(&ctx, &value, offset, length).map(|bytes| put(bytes) as f64)
        },
    );
    let take_bytes = Func::from(
        |ctx: Ctx<'js>, id: f64| -> rquickjs::Result<ArrayBuffer<'js>> {
            let bytes = take(id as u64).ok_or_else(|| {
                Exception::throw_reference(&ctx, &format!("byte buffer {id} is gone"))
            })?;
            ArrayBuffer::new(ctx, bytes)
        },
    );
    let decode = Func::from(
        |ctx: Ctx<'js>,
         buffer: ArrayBuffer<'js>,
         offset: Opt<usize>,
         length: Opt<usize>,
         fatal: Opt<bool>|
         -> rquickjs::Result<rquickjs::String<'js>> {
            let bytes = view(&ctx, &buffer, offset, length)?;
            let text = if fatal.0.unwrap_or(false) {
                Cow::Borrowed(std::str::from_utf8(&bytes).map_err(|_| {
                    Exception::throw_type(&ctx, "The encoded data was not valid utf-8")
                })?)
            } else {
                String::from_utf8_lossy(&bytes)
            };
            rquickjs::String::from_str(ctx.clone(), &text)
        },
    );
    let encode = Func::from(
        |ctx: Ctx<'js>, value: Value<'js>| -> rquickjs::Result<ArrayBuffer<'js>> {
            let bytes = source(&ctx, &value, Opt(None), Opt(None))?;
            ArrayBuffer::new(ctx, bytes)
        },
    );
    let to_base64 = Func::from(
        |ctx: Ctx<'js>,
         buffer: ArrayBuffer<'js>,
         offset: Opt<usize>,
         length: Opt<usize>|
         -> rquickjs::Result<rquickjs::String<'js>> {
            let text = STANDARD_ENGINE.encode(view(&ctx, &buffer, offset, length)?);
            rquickjs::String::from_str(ctx.clone(), &text)
        },
    );
    let from_base64 = Func::from(
        |ctx: Ctx<'js>, text: rquickjs::String<'js>| -> rquickjs::Result<ArrayBuffer<'js>> {
            let mut clean = text
                .to_string()?
                .bytes()
                .filter_map(|byte| match byte {
                    b'-' => Some(b'+'),
                    b'_' => Some(b'/'),
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'+' | b'/' => Some(byte),
                    _ => None,
                })
                .collect::<Vec<_>>();
            if clean.len() % 4 == 1 {
                clean.pop();
            }
            let bytes = LENIENT.decode(&clean).map_err(|error| {
                Exception::throw_type(&ctx, &format!("invalid base64: {error}"))
            })?;
            ArrayBuffer::new(ctx, bytes)
        },
    );
    bytes
        .set("put", put_bytes)
        .and_then(|()| bytes.set("take", take_bytes))
        .and_then(|()| bytes.set("decode", decode))
        .and_then(|()| bytes.set("encode", encode))
        .and_then(|()| bytes.set("toBase64", to_base64))
        .and_then(|()| bytes.set("fromBase64", from_base64))
        .and_then(|()| ctx.globals().set("__nanocodeBytes", bytes))
        .catch(ctx)
        .map_err(|error| format!("failed to install QuickJS byte buffers: {error}"))
}
