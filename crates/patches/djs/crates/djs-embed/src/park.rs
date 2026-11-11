//! [`Parked`]: an instance between two steps, held as what differs from its snapshot.
//!
//! An instance is a copy of its snapshot's image that a call then changes: for a call that
//! waits on a host call, a few hundred bytes of the image and what it allocated since. A
//! host with many waiting calls parks them ([`Instance::park`]): the heap is given back
//! and the difference kept, and [`Parked::resume`] makes the instance again, as it was.
//! The parked state is also bytes ([`Parked::to_bytes`]) a host can store and load in
//! another process of the same build ([`Snapshot::parked`]).

use std::sync::OnceLock;

use djs_core::heap::{Handle, Heap, LoadError};
use djs_core::vm::Vm;

use crate::instance::{Call, guard_vm};
use crate::snapshot::{IMAGE_ROOM, vm_options};
use crate::{Error, Instance, Limits, Snapshot};

/// A run's header in the delta: its offset and its length (`u32`, little-endian).
const RUN: usize = 8;

/// What the scan compares at once before it looks for the words that differ.
const BLOCK: usize = 1024;

/// The first bytes of [`Parked::to_bytes`].
const MAGIC: &[u8; 8] = b"djspark1";

/// An instance that is not running, as its difference from its snapshot's image: the
/// state of [`Instance::park`], until [`Parked::resume`]. `Send`.
pub struct Parked {
    snapshot: Snapshot,
    limits: Limits,
    call: Option<Call>,
    /// The heap's length in bytes
    heap: usize,
    /// The runs of the heap that differ from the image, ascending: for each its offset
    /// and length ([`RUN`]) and its bytes. What the heap has beyond the image is one run
    delta: Vec<u8>,
}

/// The runs of `heap` that differ from `image` (see [`Parked`]), looked for in the
/// `blocks` of the image (ascending ranges). Runs closer than a header are one.
fn delta(image: &[u8], heap: &[u8], blocks: impl Iterator<Item = (usize, usize)>) -> Vec<u8> {
    fn flush(out: &mut Vec<u8>, heap: &[u8], run: Option<(usize, usize)>) {
        if let Some((start, end)) = run {
            out.extend_from_slice(&(start as u32).to_le_bytes());
            out.extend_from_slice(&((end - start) as u32).to_le_bytes());
            out.extend_from_slice(&heap[start..end]);
        }
    }
    let shared = image.len().min(heap.len());
    let mut out = Vec::new();
    let mut run: Option<(usize, usize)> = None;
    for (start, end) in blocks {
        let end = end.min(shared);
        let mut at = start;
        while at < end {
            // A block at a time, then the words of one that differs.
            let stop = (at + BLOCK).min(end);
            let (old, new) = (&image[at..stop], &heap[at..stop]);
            if old != new {
                // (Words as integers: a comparison of slices is a call.)
                let word = |b: &[u8]| b.try_into().map_or(0, u64::from_ne_bytes);
                let words = old.chunks(8).zip(new.chunks(8));
                for (i, (a, b)) in words.enumerate() {
                    if a.len() == 8 && word(a) == word(b) || a == b {
                        continue;
                    }
                    let (w, e) = (at + 8 * i, at + 8 * i + a.len());
                    match &mut run {
                        Some((_, last)) if w <= *last + RUN => *last = e,
                        _ => {
                            flush(&mut out, heap, run.take());
                            run = Some((w, e));
                        }
                    }
                }
            }
            at = stop;
        }
    }
    if heap.len() > shared {
        match &mut run {
            Some((_, stop)) if shared <= *stop + RUN => *stop = heap.len(),
            _ => {
                flush(&mut out, heap, run.take());
                run = Some((shared, heap.len()));
            }
        }
    }
    flush(&mut out, heap, run);
    out
}

/// The runs of a delta that [`patched`] applied: their offsets and lengths.
fn runs(delta: &[u8]) -> impl Iterator<Item = (u32, u32)> + '_ {
    let mut rest = delta;
    std::iter::from_fn(move || {
        let (head, tail) = rest.split_at_checked(RUN)?;
        let start = u32::from_le_bytes(head[..4].try_into().ok()?);
        let n = u32::from_le_bytes(head[4..].try_into().ok()?);
        rest = tail.get(n as usize..)?;
        Some((start, n))
    })
}

/// A copy of the first `len` bytes of `image` (all of it if shorter) in a buffer with
/// room for a heap of `len` bytes to grow.
fn copied(image: &[u8], len: usize, memory: usize) -> Option<Vec<u8>> {
    let room = if cfg!(target_arch = "wasm32") {
        0
    } else {
        IMAGE_ROOM
    };
    let mut bytes = Vec::with_capacity(Heap::image_capacity(len.checked_add(room)?, memory));
    bytes.extend_from_slice(&image[..image.len().min(len)]);
    Some(bytes)
}

/// The heap of `len` bytes that `delta` was taken from, made of `bytes`, a copy of the
/// image: `None` if the runs do not fit it (bytes that are not a delta of this image).
fn patched(mut bytes: Vec<u8>, len: usize, delta: &[u8]) -> Option<Vec<u8>> {
    bytes.truncate(len);
    let mut rest = delta;
    while !rest.is_empty() {
        let (head, tail) = rest.split_at_checked(RUN)?;
        let start = u32::from_le_bytes(head[..4].try_into().ok()?) as usize;
        let n = u32::from_le_bytes(head[4..].try_into().ok()?) as usize;
        let (data, tail) = tail.split_at_checked(n)?;
        if start > bytes.len() || start.checked_add(n)? > len {
            return None;
        }
        // The run that reaches past the image appends what the heap has beyond it.
        let inside = (bytes.len() - start).min(n);
        bytes[start..start + inside].copy_from_slice(&data[..inside]);
        bytes.extend_from_slice(&data[inside..]);
        rest = tail;
    }
    (bytes.len() == len).then_some(bytes)
}

impl Instance {
    /// Parks the instance: gives its heap back and keeps what differs from the snapshot's
    /// image. Between steps, in any state of a call; what the host holds of the call
    /// ([`crate::CallId`]s) stays valid for the resumed instance. For a host with many
    /// calls that wait ([`crate::Step::Wait`]) at once: a parked call holds its changes
    /// (kilobytes for a call that started and waits) instead of the heap.
    ///
    /// With the parked call comes the instance back in the snapshot's state, when that
    /// cost only what the call wrote ([`Instance::reset_written`]): a spare for another
    /// call, or for [`Parked::resume_in`].
    pub fn park(mut self) -> Result<(Parked, Option<Instance>), Error> {
        let snapshot = self.snapshot.clone();
        // The image form: the dead stack slots cleared, so they are no difference.
        let (heap, delta) = self.guard(|vm| {
            let image = snapshot.image();
            let len = vm.image().len();
            // The pages the calls wrote, when the heap tracked them: only they differ.
            let delta = match vm.heap().arena().written(image.len()) {
                Some(written) => delta(image, vm.bytes(), written),
                None => delta(image, vm.bytes(), std::iter::once((0, image.len()))),
            };
            (len, delta)
        })?;
        let parked = Parked {
            snapshot,
            limits: self.limits,
            call: self.call.take(),
            heap,
            delta,
        };
        let spare = self.reset_written()?.then_some(self);
        Ok((parked, spare))
    }
}

impl Parked {
    /// The instance again, as it was parked: a copy of the image with the difference
    /// applied. A call's deadline is as it was.
    pub fn resume(self) -> Result<Instance, Error> {
        self.load(true, None)
    }

    /// [`Parked::resume`] in the heap of `spare`, an instance of the same snapshot whose
    /// call is over or abandoned: it is reset and the difference applied, so resuming
    /// costs what the parked call had written, not a copy of the image.
    pub fn resume_in(self, mut spare: Instance) -> Result<Instance, Error> {
        if !self.snapshot.same_image(&spare.snapshot) {
            return Err(Error::Usage(
                "the spare instance is of another snapshot".into(),
            ));
        }
        spare.reset()?;
        let bytes = spare.guard(|vm| vm.take_heap())?;
        self.load(true, Some(bytes))
    }

    fn load(self, trusted: bool, spare: Option<Vec<u8>>) -> Result<Instance, Error> {
        let Parked {
            snapshot,
            limits,
            call,
            heap,
            delta,
        } = self;
        let foreign = || Error::Usage("the parked state is not of this snapshot".into());
        let bytes = spare
            .or_else(|| copied(snapshot.image(), heap, limits.memory))
            .and_then(|bytes| patched(bytes, heap, &delta))
            .ok_or_else(foreign)?;
        let opts = vm_options(limits.memory, snapshot.frozen_builtins());
        let mut vm = guard_vm(|| {
            if trusted {
                Vm::from_bytes_trusted(bytes, opts)
            } else {
                Vm::from_bytes(bytes, opts)
            }
        })?
        .map_err(|e| match e {
            LoadError::TooLarge => Error::OutOfMemory,
            e => Error::Usage(format!("cannot load the parked state: {e:?}")),
        })?;
        // The heap is the image with the delta's runs written.
        vm.track_image(snapshot.image_len(), runs(&delta));
        Ok(Instance {
            vm,
            snapshot,
            limits,
            call,
            poisoned: false,
        })
    }

    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    /// The host memory the parked state holds for the heap, in `bytes`.
    pub fn size(&self) -> usize {
        self.delta.capacity()
    }

    /// The length of the heap [`Parked::resume`] makes, in `bytes`.
    pub fn heap_used(&self) -> usize {
        self.heap
    }

    /// The parked state as bytes, for a host that stores it: [`Snapshot::parked`] of a
    /// snapshot with the same image (the same build, host functions, globals and
    /// sources: [`Snapshot::fingerprint`]) loads it, in this process or another. The
    /// limits and a call's deadline are not in it.
    pub fn to_bytes(&self) -> Vec<u8> {
        let call = self.call.as_ref();
        let handle = |h: Option<Handle>| h.map_or(u32::MAX, Handle::index);
        let mut out = Vec::with_capacity(MAGIC.len() + 40 + self.delta.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&self.snapshot.fingerprint().to_le_bytes());
        out.extend_from_slice(&(self.heap as u64).to_le_bytes());
        out.extend_from_slice(&call.map_or(0, |c| c.fuel_used).to_le_bytes());
        out.extend_from_slice(&handle(call.and_then(|c| c.entry)).to_le_bytes());
        out.extend_from_slice(&handle(call.and_then(|c| c.result)).to_le_bytes());
        let state = match call {
            None => 0u32,
            Some(c) if c.over => 2,
            Some(_) => 1,
        };
        out.extend_from_slice(&state.to_le_bytes());
        out.extend_from_slice(&self.delta);
        out
    }
}

impl Snapshot {
    /// A hash of the image: equal for snapshots that load each other's
    /// [`Parked::to_bytes`]. FNV-1a over the image's bytes, made once.
    pub fn fingerprint(&self) -> u64 {
        static_fingerprint(self)
    }

    /// The parked state of [`Parked::to_bytes`], taken from an instance of a snapshot
    /// with this image, to resume with `limits`. The bytes come from storage, so the
    /// heap they make is verified cell by cell when it is resumed (a damaged one is
    /// [`Error::Usage`]); a call in progress gets the deadline of `limits` anew.
    pub fn parked(&self, bytes: &[u8], limits: Limits) -> Result<Instance, Error> {
        let bad = || Error::Usage("not a parked state of this snapshot".into());
        let word = |at: usize| -> Option<u64> {
            Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
        };
        let half = |at: usize| -> Option<u32> {
            Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
        };
        let head = MAGIC.len();
        if bytes.get(..head) != Some(MAGIC) || word(head) != Some(self.fingerprint()) {
            return Err(bad());
        }
        let heap = usize::try_from(word(head + 8).ok_or_else(bad)?).map_err(|_| bad())?;
        let fuel_used = word(head + 16).ok_or_else(bad)?;
        let handle = |at: usize| -> Option<Option<Handle>> {
            let i = half(at)?;
            Some((i != u32::MAX).then(|| Handle::from_index(i)))
        };
        let entry = handle(head + 24).ok_or_else(bad)?;
        let result = handle(head + 28).ok_or_else(bad)?;
        let call = match half(head + 32).ok_or_else(bad)? {
            0 => None,
            state @ (1 | 2) => Some(Call {
                deadline: limits.deadline.map(|d| std::time::Instant::now() + d),
                fuel_used,
                entry,
                result,
                over: state == 2,
            }),
            _ => return Err(bad()),
        };
        Parked {
            snapshot: self.clone(),
            limits,
            call,
            heap,
            delta: bytes[head + 36..].to_vec(),
        }
        .load(false, None)
    }
}

fn static_fingerprint(snapshot: &Snapshot) -> u64 {
    *snapshot.fingerprint_cell().get_or_init(|| {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in snapshot.image() {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
        h
    })
}

/// The cell [`Snapshot::fingerprint`] is kept in.
pub(crate) type Fingerprint = OnceLock<u64>;

const _: () = {
    const fn send<T: Send>() {}
    send::<Parked>();
};
