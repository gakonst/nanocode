//! The exception handler table (docs/compiler.md 7.4).
//!
//! One 16-byte record per `try` region, four little-endian `u32`s: `start`, `end`,
//! `target`, `depth`. Records are in innermost-first order (the compiler appends a
//! region when its `try` block closes), so the first record covering a pc is the
//! innermost. Stored at the end of the template cell (`template::handlers`, D2444) and read
//! in place.

use alloc::vec::Vec;

/// A try region: `[start, end)` instruction positions, the handler position and the
/// operand stack depth (relative to the frame's operand base) at handler entry, before
/// the exception is pushed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Handler {
    pub start: u32,
    pub end: u32,
    pub target: u32,
    pub depth: u32,
}

const RECORD: usize = 16;

impl Handler {
    pub fn covers(&self, pc: u32) -> bool {
        self.start <= pc && pc < self.end
    }
}

/// Encodes handler records.
pub fn encode_handlers(handlers: &[Handler]) -> Vec<u8> {
    let mut out = alloc::vec![0; handlers.len() * RECORD];
    write_handlers(handlers, &mut out);
    out
}

/// Encodes handler records into `out`, which is that many records long.
pub(super) fn write_handlers(handlers: &[Handler], out: &mut [u8]) {
    for (h, r) in handlers.iter().zip(out.chunks_exact_mut(RECORD)) {
        for (w, b) in [h.start, h.end, h.target, h.depth]
            .iter()
            .zip(r.chunks_exact_mut(4))
        {
            b.copy_from_slice(&w.to_le_bytes());
        }
    }
}

/// A read-only view of an encoded handler table.
#[derive(Clone, Copy, Debug)]
pub struct HandlerTable<'a> {
    bytes: &'a [u8],
}

/// The bytes are not a whole number of records.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BadHandlerTable;

impl<'a> HandlerTable<'a> {
    pub fn new(bytes: &'a [u8]) -> Result<Self, BadHandlerTable> {
        if !bytes.len().is_multiple_of(RECORD) {
            return Err(BadHandlerTable);
        }
        Ok(HandlerTable { bytes })
    }

    pub fn empty() -> Self {
        HandlerTable { bytes: &[] }
    }

    pub fn len(&self) -> usize {
        self.bytes.len() / RECORD
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn get(&self, i: usize) -> Option<Handler> {
        let r = self.bytes.get(i * RECORD..(i + 1) * RECORD)?;
        let w = |k: usize| u32::from_le_bytes([r[k], r[k + 1], r[k + 2], r[k + 3]]);
        Some(Handler {
            start: w(0),
            end: w(4),
            target: w(8),
            depth: w(12),
        })
    }

    pub fn iter(&self) -> impl Iterator<Item = Handler> + '_ {
        (0..self.len()).filter_map(|i| self.get(i))
    }

    /// The innermost handler covering `pc`. When unwinding into a caller frame, pass
    /// `ret_pc - 1`, a byte inside the call instruction.
    pub fn find(&self, pc: u32) -> Option<Handler> {
        self.iter().find(|h| h.covers(pc))
    }
}
