use std::{
    collections::BTreeMap,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use nanocode_sync::{Atom, Slot};

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

struct Buffer {
    at: Instant,
    bytes: Slot<Vec<u8>>,
}

static BUFFERS: OnceLock<Atom<BTreeMap<u64, Arc<Buffer>>>> = OnceLock::new();
const UNCLAIMED: Duration = Duration::from_secs(600);
static NEXT: AtomicU64 = AtomicU64::new(1);

const LENIENT: GeneralPurpose = GeneralPurpose::new(
    &STANDARD,
    GeneralPurposeConfig::new()
        .with_decode_allow_trailing_bits(true)
        .with_decode_padding_mode(DecodePaddingMode::RequireNone),
);

fn buffers() -> &'static Atom<BTreeMap<u64, Arc<Buffer>>> {
    BUFFERS.get_or_init(Atom::default)
}

pub fn put(bytes: Vec<u8>) -> u64 {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let buffer = Arc::new(Buffer {
        at: Instant::now(),
        bytes: Slot::new(Some(bytes)),
    });
    buffers().update(|buffers| {
        buffers.retain(|_, buffer| buffer.at.elapsed() < UNCLAIMED);
        buffers.insert(id, Arc::clone(&buffer));
    });
    id
}

pub fn take(id: u64) -> Option<Vec<u8>> {
    buffers()
        .update(|buffers| buffers.remove(&id))?
        .bytes
        .take()
}

const CHUNK: usize = 1 << 20;

const LATIN1: &str = "(buffer, offset, length) => { const view = new Uint8Array(buffer, offset ?? 0, length ?? buffer.byteLength - (offset ?? 0)); let text = \"\"; for (let at = 0; at < view.length; at += 8192) text += String.fromCharCode.apply(null, view.subarray(at, at + 8192)); return text; }";

const SLICE: &str = "(text, start, size) => { let end = Math.min(text.length, start + size); if (end < text.length) { const code = text.charCodeAt(end - 1); if (code >= 0xd800 && code < 0xdc00) end--; } return [start === 0 && end === text.length ? text : text.substring(start, end), end, text.length]; }";

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
        let text = text.to_cstring()?;
        let mut units = AsRef::<[u8]>::as_ref(&text).iter();
        while let Some(&byte) = units.next() {
            bytes.push(if byte < 0x80 {
                byte
            } else {
                ((byte & 0x03) << 6) | (units.next().copied().unwrap_or(0) & 0x3f)
            });
        }
        at += length;
    }
    Ok(bytes)
}

fn utf8<'js>(ctx: &Ctx<'js>, text: rquickjs::String<'js>) -> rquickjs::Result<Vec<u8>> {
    let slice: rquickjs::Function<'js> = ctx.eval(SLICE)?;
    let mut bytes = Vec::new();
    let mut at = 0_usize;
    loop {
        let result: rquickjs::Array<'js> = slice.call((text.clone(), at, CHUNK))?;
        let piece: rquickjs::String<'js> = result.get(0)?;
        let end = result.get::<f64>(1)? as usize;
        let total = result.get::<f64>(2)? as usize;
        if at == 0 {
            bytes.reserve_exact(total);
        }
        let start = bytes.len();
        bytes.extend_from_slice(AsRef::<[u8]>::as_ref(&piece.to_cstring()?));
        let mut index = start;
        while index + 2 < bytes.len() {
            if bytes[index] == 0xed && bytes[index + 1] >= 0xa0 {
                bytes[index..index + 3].copy_from_slice("\u{fffd}".as_bytes());
                index += 3;
            } else {
                index += 1;
            }
        }
        if end >= total || end <= at {
            return Ok(bytes);
        }
        at = end;
    }
}

fn source<'js>(
    ctx: &Ctx<'js>,
    value: &Value<'js>,
    offset: Opt<usize>,
    length: Opt<usize>,
) -> rquickjs::Result<Vec<u8>> {
    if let Some(text) = value.as_string() {
        return utf8(ctx, text.clone());
    }
    let buffer = ArrayBuffer::from_value(value.clone())
        .ok_or_else(|| Exception::throw_type(ctx, "expected a string or an ArrayBuffer"))?;
    view(ctx, &buffer, offset, length)
}

fn text<'js>(ctx: &Ctx<'js>, bytes: &[u8], fatal: bool) -> rquickjs::Result<rquickjs::String<'js>> {
    match std::str::from_utf8(bytes) {
        Ok(text) => rquickjs::String::from_str(ctx.clone(), text),
        Err(_) if fatal => Err(Exception::throw_type(
            ctx,
            "The encoded data was not valid utf-8",
        )),
        Err(_) => rquickjs::String::from_str(ctx.clone(), &String::from_utf8_lossy(bytes)),
    }
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
            text(&ctx, &bytes, fatal.0.unwrap_or(false))
        },
    );
    let take_text = Func::from(
        |ctx: Ctx<'js>, id: f64, fatal: Opt<bool>| -> rquickjs::Result<rquickjs::String<'js>> {
            let bytes = take(id as u64).ok_or_else(|| {
                Exception::throw_reference(&ctx, &format!("byte buffer {id} is gone"))
            })?;
            text(&ctx, &bytes, fatal.0.unwrap_or(false))
        },
    );
    let discard = Func::from(|id: f64| {
        take(id as u64);
    });
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
        .and_then(|()| bytes.set("text", take_text))
        .and_then(|()| bytes.set("discard", discard))
        .and_then(|()| bytes.set("decode", decode))
        .and_then(|()| bytes.set("encode", encode))
        .and_then(|()| bytes.set("toBase64", to_base64))
        .and_then(|()| bytes.set("fromBase64", from_base64))
        .and_then(|()| ctx.globals().set("__nanocodeBytes", bytes))
        .catch(ctx)
        .map_err(|error| format!("failed to install QuickJS byte buffers: {error}"))
}
