use std::{
    borrow::Cow,
    collections::BTreeMap,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use nanocode_sync::{Atom, Slot};

use base64::{
    Engine as _, alphabet,
    engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
};
use djs_embed::{ErrorKind, HostCall, Throw};
use nanocode_platform::Instant;

use crate::cell::{ExecutionState, Reply};

struct Buffer {
    at: Instant,
    bytes: Slot<Vec<u8>>,
}

static BUFFERS: OnceLock<Atom<BTreeMap<u64, Arc<Buffer>>>> = OnceLock::new();
const BASE64_ERROR: &str = "Base64Error";
const FORGIVING: GeneralPurpose = GeneralPurpose::new(
    &alphabet::STANDARD,
    GeneralPurposeConfig::new()
        .with_decode_allow_trailing_bits(true)
        .with_decode_padding_mode(DecodePaddingMode::Indifferent),
);
const UNCLAIMED: Duration = Duration::from_secs(600);
static NEXT: AtomicU64 = AtomicU64::new(1);

fn buffers() -> &'static Atom<BTreeMap<u64, Arc<Buffer>>> {
    BUFFERS.get_or_init(Atom::default)
}

pub fn put(bytes: Vec<u8>) -> u64 {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let buffer = Arc::new(Buffer {
        at: nanocode_platform::now(),
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

const PUT: u32 = 20;
const TAKE: u32 = 21;
const TEXT: u32 = 22;
const DISCARD: u32 = 23;
const DECODE: u32 = 24;
const ENCODE: u32 = 25;
const TO_BASE64: u32 = 26;
const FROM_BASE64: u32 = 27;
const DIGEST: u32 = 28;
const RANDOM: u32 = 29;

pub(crate) const HOST_FNS: [(u32, &str); 10] = [
    (PUT, "__nanocodeBytesPut"),
    (TAKE, "__nanocodeBytesTake"),
    (TEXT, "__nanocodeBytesText"),
    (DISCARD, "__nanocodeBytesDiscard"),
    (DECODE, "__nanocodeBytesDecode"),
    (ENCODE, "__nanocodeBytesEncode"),
    (TO_BASE64, "__nanocodeBytesToBase64"),
    (FROM_BASE64, "__nanocodeBytesFromBase64"),
    (DIGEST, "__nanocodeBytesDigest"),
    (RANDOM, "__nanocodeBytesRandom"),
];

fn index(call: &HostCall<'_>, at: usize) -> Option<usize> {
    call.arg(at).as_number().map(|number| number as usize)
}

fn view<'a>(
    call: &'a HostCall<'_>,
    buffer: usize,
    offset: usize,
    length: usize,
) -> Result<&'a [u8], Throw> {
    let bytes = call
        .arg(buffer)
        .as_bytes()
        .ok_or_else(|| Throw::type_error("expected an ArrayBuffer"))?;
    let start = index(call, offset).unwrap_or(0);
    let end = index(call, length)
        .map_or(Some(bytes.len()), |length| start.checked_add(length))
        .filter(|end| *end <= bytes.len() && start <= *end)
        .ok_or_else(|| Throw::range_error("byte range is outside the ArrayBuffer"))?;
    Ok(&bytes[start..end])
}

fn source<'a>(
    call: &'a HostCall<'_>,
    value: usize,
    offset: usize,
    length: usize,
) -> Result<Cow<'a, [u8]>, Throw> {
    let argument = call.arg(value);
    if let Some(text) = argument.as_str() {
        return Ok(Cow::Owned(text.into_owned().into_bytes()));
    }
    if argument.as_bytes().is_none() {
        return Err(Throw::type_error("expected a string or an ArrayBuffer"));
    }
    view(call, value, offset, length).map(Cow::Borrowed)
}

fn text(bytes: Vec<u8>, fatal: bool) -> Result<Reply<'static>, Throw> {
    match String::from_utf8(bytes) {
        Ok(text) => Ok(Reply::Text(Cow::Owned(text))),
        Err(_) if fatal => Err(Throw::type_error("The encoded data was not valid utf-8")),
        Err(error) => Ok(Reply::Lossy(error.into_bytes())),
    }
}

fn stored(call: &HostCall<'_>, at: usize) -> Result<Vec<u8>, Throw> {
    let id = call.arg(at).as_number().unwrap_or(0.0);
    take(id as u64).ok_or_else(|| {
        Throw::new(
            ErrorKind::ReferenceError,
            format!("byte buffer {id} is gone"),
        )
    })
}

pub(crate) fn host_call(
    state: &mut ExecutionState,
    call: &HostCall<'_>,
) -> Result<Reply<'static>, Throw> {
    match call.fn_id() {
        PUT => Ok(Reply::Number(
            put(source(call, 0, 1, 2)?.into_owned()) as f64
        )),
        TAKE => stored(call, 0).map(Reply::Bytes),
        TEXT => text(stored(call, 0)?, call.arg(1).as_bool().unwrap_or(false)),
        DISCARD => {
            take(call.arg(0).as_number().unwrap_or(0.0) as u64);
            Ok(Reply::Nothing)
        }
        DECODE => text(
            view(call, 0, 1, 2)?.to_vec(),
            call.arg(3).as_bool().unwrap_or(false),
        ),
        ENCODE => Ok(Reply::Bytes(source(call, 0, 1, 2)?.into_owned())),
        TO_BASE64 => Ok(Reply::Text(Cow::Owned(
            base64::engine::general_purpose::STANDARD.encode(view(call, 0, 1, 2)?),
        ))),
        FROM_BASE64 => {
            let mut clean = crate::cell::text(call, 0)?
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
            FORGIVING
                .decode(&clean)
                .map(Reply::Bytes)
                .map_err(|_| Throw::type_error(format!("invalid base64: {BASE64_ERROR}")))
        }
        DIGEST => {
            let bytes = view(call, 1, 2, 3)?;
            let digest = match crate::cell::text(call, 0)?.as_str() {
                "SHA-1" => nanocode_digest::Sha1::digest(bytes).to_vec(),
                "SHA-256" => nanocode_digest::Sha256::digest(bytes).to_vec(),
                "SHA-384" => nanocode_digest::Sha384::digest(bytes).to_vec(),
                "SHA-512" => nanocode_digest::Sha512::digest(bytes).to_vec(),
                other => return Err(Throw::type_error(format!("unsupported digest {other}"))),
            };
            Ok(Reply::Bytes(digest))
        }
        RANDOM => {
            let mut bytes = vec![0; index(call, 0).unwrap_or(0)];
            nanocode_platform::fill_random(&mut bytes)
                .map_err(|error| Throw::error(format!("no randomness: {error}")))?;
            Ok(Reply::Bytes(state.random_bytes(bytes)))
        }
        other => Err(Throw::error(format!("unknown host function {other}"))),
    }
}
