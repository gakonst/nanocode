//! Walking the arena cell by cell, tracing cells, and verifying a heap.
//!
//! Tracing is driven entirely by the layout table: a field or tail element field of type
//! `Value` or `Ref` is a slot the GC must visit. This is the groundwork for the copying GC
//! (a separate issue), and [`Heap::verify`] is what restoring an untrusted image runs.

use alloc::vec::Vec;

use super::cells::{
    AtomTable, BigInt, EphemeronArray, FuncInfo, FuncTemplate, HANDLE_LIVE, HandleSlot,
    HandleTable, Header, Object, PropTable, PropValues, RefArray, Roots, Rope, Shape, Slice, Str8,
    Str16, Symbol, U32Array, ValueArray, flags,
};
use super::layout::{FieldTy, Ref};
use super::object::ObjectClass;
use super::{CellKind, Heap};
use crate::value::{Value, tag};

/// One cell found by the walker.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CellInfo {
    pub offset: u32,
    pub kind: CellKind,
    pub size: u32,
}

/// A traced slot: the arena offset of a `Value` or a `Ref`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Slot {
    Value(u32),
    Ref(u32),
}

/// What [`Heap::verify`] found wrong.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VerifyError {
    /// The cell at `offset` has an unknown kind byte.
    BadKind { offset: u32, kind: u8 },
    /// The cell size is not a multiple of 8, is below the kind's minimum, or runs past the
    /// end of the arena.
    BadSize { offset: u32, size: u32 },
    /// GC bits are set outside a collection.
    GcBits { offset: u32 },
    /// The cell at offset 0 is not the roots cell, or there is a second roots cell.
    Roots { offset: u32 },
    /// A length field does not fit the cell.
    BadLength { offset: u32 },
    /// A traced slot holds an invalid value encoding.
    BadValue { slot: u32 },
    /// A traced slot points somewhere that is not the start of a cell of a fitting kind.
    BadRef { slot: u32, target: u32 },
    /// A cell breaks an invariant of its kind (a ref field to the wrong kind of cell, an
    /// inconsistent rope, an out-of-range count, an unknown object class).
    Invalid { offset: u32 },
    /// The prototype chain through the object at `offset` is cyclic.
    ProtoCycle { offset: u32 },
    /// `Roots::OLD_END` or `Roots::BASE_END` is not a cell boundary within the arena.
    OldEnd { offset: u32 },
}

/// Iterator over all cells, in address order. Panics on a malformed header; use
/// [`Heap::verify`] on untrusted heaps first.
pub struct Cells<'a> {
    heap: &'a Heap,
    pos: u32,
}

impl Iterator for Cells<'_> {
    type Item = CellInfo;

    fn next(&mut self) -> Option<CellInfo> {
        if self.pos >= self.heap.arena().len() {
            return None;
        }
        let offset = self.pos;
        let kind = self.heap.cell_kind(offset);
        let size = self.heap.cell_size(offset);
        if size < 8 || !size.is_multiple_of(8) {
            super::fail::bad_cell_size(size, offset);
        }
        self.pos += size;
        Some(CellInfo { offset, kind, size })
    }
}

impl Heap {
    /// Walks every cell in the arena, in address order.
    pub fn cells(&self) -> Cells<'_> {
        Cells { heap: self, pos: 0 }
    }

    /// The cells from the one at `start` (a cell's offset) on.
    #[cfg(feature = "compile-cache")]
    pub(crate) fn cells_from(&self, start: u32) -> Cells<'_> {
        Cells {
            heap: self,
            pos: start,
        }
    }

    /// Visits every traced slot of the cell at `cell` (whether or not it currently holds
    /// a reference).
    pub fn trace_cell(&self, cell: u32, f: impl FnMut(Slot)) {
        for_each_slot(self.cell_kind(cell), cell, self.cell_size(cell), f);
    }

    /// The offsets of all cells the cell at `cell` references.
    pub fn cell_refs(&self, cell: u32) -> Vec<u32> {
        let mut out = Vec::new();
        self.trace_cell(cell, |slot| {
            if let Some(t) = self.slot_target(slot) {
                out.push(t)
            }
        });
        out
    }

    /// The cell a slot references, if any.
    pub fn slot_target(&self, slot: Slot) -> Option<u32> {
        match slot {
            Slot::Value(off) => self.arena().read::<Value>(off).ref_offset(),
            Slot::Ref(off) => self.arena().read::<Ref>(off).get(),
        }
    }

    /// Checks the whole heap: every header, every length field, and every traced slot
    /// (valid encoding, pointing at the start of a cell of a fitting kind). Never panics,
    /// whatever the bytes are.
    pub fn verify(&self) -> Result<(), VerifyError> {
        let a = self.arena();
        let end = a.len();
        // Cell starts as a bitmap, one bit per 8-byte word (D1991): restoring an image is
        // a memory peak in wasm, and a list of (offset, kind) cost 8 bytes per cell; this
        // is len / 64. The kind of a start is read from its (checked) header.
        let words = end.div_ceil(8) as usize;
        let mut starts = alloc::vec![0u64; words.div_ceil(64)];
        let is_start = |bits: &[u64], t: u32| {
            t.is_multiple_of(8)
                && t < end
                && bits[(t / 8) as usize / 64] & (1 << ((t / 8) % 64)) != 0
        };
        let mut pos = 0;
        while pos < end {
            if end - pos < 8 {
                return Err(VerifyError::BadSize {
                    offset: pos,
                    size: end - pos,
                });
            }
            let kb = a.get(pos, Header::KIND);
            let Some(kind) = CellKind::from_u8(kb) else {
                return Err(VerifyError::BadKind {
                    offset: pos,
                    kind: kb,
                });
            };
            let size = a.get(pos, Header::SIZE);
            if !size.is_multiple_of(8) || size < kind.min_size() || size > end - pos {
                return Err(VerifyError::BadSize { offset: pos, size });
            }
            if a.get(pos, Header::GC) != 0 {
                return Err(VerifyError::GcBits { offset: pos });
            }
            if (pos == 0) != (kind == CellKind::Roots) {
                return Err(VerifyError::Roots { offset: pos });
            }
            if !self.length_fits(pos, kind, size) {
                return Err(VerifyError::BadLength { offset: pos });
            }
            starts[(pos / 8) as usize / 64] |= 1 << ((pos / 8) % 64);
            pos += size;
        }
        if end == 0 {
            return Err(VerifyError::Roots { offset: 0 });
        }
        // The old region ends at a cell boundary (D2041): minor collections walk it.
        let old = a.get(0, super::cells::Roots::OLD_END);
        if old != end && !is_start(&starts, old) {
            return Err(VerifyError::OldEnd { offset: old });
        }
        // So does the builtin prefix (D022), when there is one.
        let base = a.get(0, super::cells::Roots::BASE_END);
        if base != 0 && base != end && !is_start(&starts, base) {
            return Err(VerifyError::OldEnd { offset: base });
        }
        let kind_at =
            |t: u32| -> Option<CellKind> { is_start(&starts, t).then(|| self.cell_kind(t)) };
        // The cells in address order (their headers are checked above).
        let cells = || {
            let mut pos = 0;
            core::iter::from_fn(move || {
                (pos < end).then(|| {
                    let cell = pos;
                    pos += a.get(cell, Header::SIZE);
                    (cell, self.cell_kind(cell))
                })
            })
        };
        for (cell, _) in cells() {
            let mut err = None;
            self.trace_cell(cell, |slot| {
                if err.is_some() {
                    return;
                }
                match slot {
                    Slot::Value(off) => {
                        let bits = a.read::<u64>(off);
                        let Some(v) = Value::from_bits(bits) else {
                            err = Some(VerifyError::BadValue { slot: off });
                            return;
                        };
                        if let Some(t) = v.ref_offset() {
                            // Append buffers never escape as values (D1610).
                            let ok = ref_fits(v.tag(), kind_at(t))
                                && !self.is_append_buffer(t, &kind_at);
                            if !ok {
                                err = Some(VerifyError::BadRef {
                                    slot: off,
                                    target: t,
                                });
                            }
                        }
                    }
                    Slot::Ref(off) => {
                        let t = a.read::<Ref>(off).0;
                        if t != 0 && !matches!(kind_at(t), Some(k) if k != CellKind::Roots) {
                            err = Some(VerifyError::BadRef {
                                slot: off,
                                target: t,
                            });
                        } else if t != 0 && self.is_append_buffer(t, &kind_at) {
                            // Only an append rope's LEFT may hold an append buffer.
                            let from_append_rope = self.cell_kind(cell) == CellKind::Rope
                                && self.cell_flags(cell) & flags::ROPE_APPEND != 0
                                && off == cell + Rope::LEFT.offset;
                            if !from_append_rope {
                                err = Some(VerifyError::BadRef {
                                    slot: off,
                                    target: t,
                                });
                            }
                        }
                    }
                }
            });
            if let Some(e) = err {
                return Err(e);
            }
        }
        for (cell, kind) in cells() {
            if !self.cell_invariants(cell, kind, &kind_at) {
                return Err(VerifyError::Invalid { offset: cell });
            }
        }
        self.check_proto_cycles(cells())
    }

    /// Kind-specific invariants that the rest of the heap code relies on to terminate:
    /// ref fields point at the right kind of cell, counts fit, and rope lengths strictly
    /// decrease towards the leaves (so ropes cannot be cyclic).
    fn cell_invariants(
        &self,
        cell: u32,
        kind: CellKind,
        kind_at: &impl Fn(u32) -> Option<CellKind>,
    ) -> bool {
        let a = self.arena();
        // A ref field: null allowed or not, and the kinds it may point at.
        let fits = |r: Ref, nullable: bool, kinds: &[CellKind]| match r.get() {
            None => nullable,
            Some(t) => kind_at(t).is_some_and(|k| kinds.contains(&k)),
        };
        let is_str = |k: Option<CellKind>| {
            matches!(
                k,
                Some(CellKind::Str8 | CellKind::Str16 | CellKind::Rope | CellKind::Slice)
            )
        };
        let pow2 = |r: Ref| self.u32_array_len(r.0).is_power_of_two();
        match kind {
            CellKind::Roots => {
                fits(a.get(cell, Roots::ATOMS), false, &[CellKind::AtomTable])
                    && fits(a.get(cell, Roots::HANDLES), true, &[CellKind::HandleTable])
            }
            CellKind::HandleTable => {
                let len = a.get(cell, HandleTable::LEN);
                len <= HandleTable::LAYOUT.tail_capacity(self.cell_size(cell))
                    && a.get(cell, HandleTable::FREE) <= len
                    && a.get(cell, HandleTable::LIVE) <= len
                    && (0..len).all(|i| {
                        let next = a.get(HandleTable::SLOTS.at(cell, i), HandleSlot::NEXT);
                        next == HANDLE_LIVE || next <= len
                    })
            }
            CellKind::AtomTable => {
                let index = a.get(cell, AtomTable::INDEX);
                // Without a base: no base index, no base entries, a count of 0.
                let base: Ref = a.get(cell, AtomTable::BASE_INDEX);
                let entries: Ref = a.get(cell, AtomTable::BASE_ENTRIES);
                let count: u32 = a.get(cell, AtomTable::BASE_COUNT);
                fits(
                    a.get(cell, AtomTable::ENTRIES),
                    false,
                    &[CellKind::ValueArray],
                ) && fits(index, false, &[CellKind::U32Array])
                    && pow2(index)
                    && match (base.get(), entries.get()) {
                        (None, None) => count == 0,
                        (Some(_), Some(e)) => {
                            fits(base, false, &[CellKind::U32Array])
                                && pow2(base)
                                && fits(entries, false, &[CellKind::ValueArray])
                                && self.value_array_len(e) == count
                        }
                        _ => false,
                    }
            }
            CellKind::Object => {
                let class = ObjectClass::from_u32(a.get(cell, Object::CLASS));
                class.is_some_and(|c| {
                    Object::LAYOUT.tail_capacity(self.cell_size(cell)) >= c.slots()
                }) && fits(a.get(cell, Object::PROTO), true, &[CellKind::Object])
                    && fits(
                        a.get(cell, Object::PROPS),
                        true,
                        if self.cell_flags(cell) & flags::OBJ_DICTIONARY != 0 {
                            &[CellKind::PropTable]
                        } else {
                            &[CellKind::PropValues]
                        },
                    )
                    // A closure's ELEMENTS field is its template (D2448).
                    && if a.get(cell, Object::CLASS) == ObjectClass::Function as u32 {
                        fits(a.get(cell, Object::ELEMENTS), false, &[CellKind::FuncTemplate])
                    } else {
                        fits(a.get(cell, Object::ELEMENTS), true, &[CellKind::ValueArray])
                    }
            }
            CellKind::PropTable => {
                let count = a.get(cell, PropTable::COUNT);
                let index = a.get(cell, PropTable::INDEX);
                count <= PropTable::LAYOUT.tail_capacity(self.cell_size(cell))
                    && a.get(cell, PropTable::DELETED) <= count
                    && fits(index, true, &[CellKind::U32Array])
                    && (index.is_null() || (pow2(index) && self.u32_array_len(index.0) > count))
                    // An accessor's value is its pair (D1830).
                    && (0..count).all(|i| {
                        let e = PropTable::ENTRIES.at(cell, i);
                        let accessor = a.get(e, super::cells::PropEntry::FLAGS)
                            & super::object::STORE_ACCESSOR
                            != 0;
                        let v: Value = a.get(e, super::cells::PropEntry::VALUE);
                        !accessor
                            || a.get(e, super::cells::PropEntry::KEY) == 0
                            || v.as_internal().and_then(kind_at) == Some(CellKind::AccessorPair)
                    })
            }
            // A values block: its shape's COUNT slots, accessors pointing at pairs.
            CellKind::PropValues => {
                let len = a.get(cell, PropValues::LEN);
                let shape = a.get(cell, PropValues::SHAPE);
                let descs = |s: Ref| a.get(s.0, Shape::DESCS);
                len <= PropValues::LAYOUT.tail_capacity(self.cell_size(cell))
                    && fits(shape, false, &[CellKind::Shape])
                    && a.get(shape.0, Shape::COUNT) == len
                    && (len == 0
                        || (fits(descs(shape), false, &[CellKind::U32Array])
                            && self.u32_array_len(descs(shape).0) > 2 * len))
                    && (0..len).all(|i| {
                        let fl: u32 = a.read(U32Array::DATA.at(descs(shape).0, 2 + 2 * i));
                        let v: Value = a.read(PropValues::VALUES.at(cell, i));
                        fl & super::object::STORE_ACCESSOR == 0
                            || v.as_internal().and_then(kind_at) == Some(CellKind::AccessorPair)
                    })
            }
            // COUNT grows by one from the parent (so PARENT chains end), the descriptors
            // cover it, and children point back.
            CellKind::Shape => {
                let parent = a.get(cell, Shape::PARENT);
                let descs = a.get(cell, Shape::DESCS);
                let count = a.get(cell, Shape::COUNT);
                let tr = a.get(cell, Shape::TRANSITIONS);
                let tree = if parent.is_null() {
                    count == 0 && descs.is_null()
                } else {
                    fits(parent, false, &[CellKind::Shape])
                        && a.get(parent.0, Shape::COUNT).checked_add(1) == Some(count)
                        && fits(descs, false, &[CellKind::U32Array])
                        && self.u32_array_len(descs.0) > 2 * count
                        && self.u32_array_get(descs.0, 0) >= count
                };
                tree && fits(tr, true, &[CellKind::RefArray])
                    && (tr.is_null() || {
                        let n = a.get(tr.0, RefArray::LEN);
                        n <= RefArray::LAYOUT.tail_capacity(self.cell_size(tr.0))
                            && (0..n).all(|i| {
                                let c: Ref = a.read(RefArray::REFS.at(tr.0, i));
                                fits(c, false, &[CellKind::Shape])
                                    && a.get(c.0, Shape::PARENT) == Ref(cell)
                            })
                    })
            }
            CellKind::Rope => {
                let len = a.get(cell, Rope::LEN);
                let (left, right) = (a.get(cell, Rope::LEFT), a.get(cell, Rope::RIGHT));
                let fl = self.cell_flags(cell);
                let two = fl & flags::ROPE_TWO_BYTE != 0;
                if fl & flags::ROPE_APPEND != 0 {
                    // A prefix of an append buffer (or a frozen one), D1610.
                    let k = left.get().and_then(kind_at);
                    let flat = matches!(k, Some(CellKind::Str8 | CellKind::Str16));
                    flat && fl & flags::ROPE_FLATTENED == 0
                        && right.is_null()
                        && len > 0
                        && len <= a.get(left.0, Str8::LEN)
                        && (k == Some(CellKind::Str16)) == two
                } else if fl & flags::ROPE_FLATTENED != 0 {
                    let k = left.get().and_then(kind_at);
                    let flat = matches!(k, Some(CellKind::Str8 | CellKind::Str16));
                    flat && right.is_null()
                        && a.get(left.0, Str8::LEN) == len
                        && (k == Some(CellKind::Str16)) == two
                } else {
                    let (kl, kr) = (left.get().and_then(kind_at), right.get().and_then(kind_at));
                    if !is_str(kl) || !is_str(kr) {
                        return false;
                    }
                    let (ll, lr) = (a.get(left.0, Str8::LEN), a.get(right.0, Str8::LEN));
                    let two_of = |off: u32, k: Option<CellKind>| match k {
                        Some(CellKind::Str16) => true,
                        Some(CellKind::Rope) => self.cell_flags(off) & flags::ROPE_TWO_BYTE != 0,
                        _ => false,
                    };
                    ll > 0
                        && lr > 0
                        && ll.checked_add(lr) == Some(len)
                        && two == (two_of(left.0, kl) || two_of(right.0, kr))
                }
            }
            // Units of a flat one-byte string (D2542).
            CellKind::Slice => {
                let parent = a.get(cell, Slice::PARENT);
                fits(parent, false, &[CellKind::Str8])
                    && a.get(cell, Slice::OFF)
                        .checked_add(a.get(cell, Slice::LEN))
                        .is_some_and(|end| end <= a.get(parent.0, Str8::LEN))
            }
            // Normalized: no high zero limb, zero is not negative.
            CellKind::BigInt => {
                let len = a.get(cell, BigInt::LEN);
                if len == 0 {
                    self.cell_flags(cell) & flags::BIGINT_NEG == 0
                } else {
                    a.read::<u32>(BigInt::LIMBS.at(cell, len - 1)) != 0
                }
            }
            CellKind::Symbol => {
                let d: Value = a.get(cell, Symbol::DESCRIPTION);
                d.is_undefined() || d.as_string().is_some()
            }
            // Pairs; a live key is an object or a symbol (or a cleared HOLE).
            CellKind::EphemeronArray => {
                let len = a.get(cell, EphemeronArray::LEN);
                len.is_multiple_of(2)
                    && (0..len).step_by(2).all(|i| {
                        let k: Value = a.read(EphemeronArray::VALUES.at(cell, i));
                        k.is_hole() || k.as_object().is_some() || k.as_symbol().is_some()
                    })
            }
            _ => true,
        }
    }

    /// Prototype chains must end in null. Two bits per 8-byte word mark objects on the
    /// current path and objects known to reach null (D1991).
    fn check_proto_cycles(
        &self,
        cells: impl Iterator<Item = (u32, CellKind)>,
    ) -> Result<(), VerifyError> {
        let words = self.arena().len().div_ceil(8) as usize;
        let mut on_path = alloc::vec![0u64; words.div_ceil(64)];
        let mut done = alloc::vec![0u64; words.div_ceil(64)];
        let bit = |o: u32| ((o / 8) as usize / 64, 1u64 << ((o / 8) % 64));
        let mut path: Vec<u32> = Vec::new();
        for (start, kind) in cells {
            if kind != CellKind::Object {
                continue;
            }
            let mut cur = Some(start);
            while let Some(o) = cur {
                let (w, m) = bit(o);
                if done[w] & m != 0 {
                    break;
                }
                if on_path[w] & m != 0 {
                    return Err(VerifyError::ProtoCycle { offset: o });
                }
                on_path[w] |= m;
                path.push(o);
                // Already checked: PROTO is null or an object cell.
                cur = self.arena().get(o, Object::PROTO).get();
            }
            for o in path.drain(..) {
                let (w, m) = bit(o);
                on_path[w] &= !m;
                done[w] |= m;
            }
        }
        Ok(())
    }

    /// Whether `t` is an (unfrozen) append buffer.
    fn is_append_buffer(&self, t: u32, kind_at: &impl Fn(u32) -> Option<CellKind>) -> bool {
        matches!(kind_at(t), Some(CellKind::Str8 | CellKind::Str16))
            && self.cell_flags(t) & flags::STR_APPEND_BUF != 0
    }

    /// Whether the cell's own length field fits its size.
    fn length_fits(&self, cell: u32, kind: CellKind, size: u32) -> bool {
        let a = self.arena();
        let need = match kind {
            CellKind::Str8 => Str8::DATA.cell_size(a.get(cell, Str8::LEN)),
            CellKind::Str16 => Str16::DATA.cell_size(a.get(cell, Str16::LEN)),
            CellKind::ValueArray => ValueArray::VALUES.cell_size(a.get(cell, ValueArray::LEN)),
            CellKind::EphemeronArray => {
                EphemeronArray::VALUES.cell_size(a.get(cell, EphemeronArray::LEN))
            }
            CellKind::U32Array => U32Array::DATA.cell_size(a.get(cell, U32Array::LEN)),
            // A closure reads its template as an empty elements array (D2448).
            CellKind::FuncTemplate if a.get(cell, FuncTemplate::NO_ELEMENTS) != 0 => None,
            // The code and the capture descriptors (the handler records fill the rest).
            CellKind::FuncTemplate => a
                .get(cell, FuncTemplate::CODE_LEN)
                .checked_add(4 * a.get(cell, FuncTemplate::CAPTURES) as u32)
                .and_then(|n| FuncTemplate::DATA.cell_size(n)),
            CellKind::FuncInfo => FuncInfo::CHILDREN.cell_size(a.get(cell, FuncInfo::CHILD_COUNT)),
            CellKind::BigInt => BigInt::LIMBS.cell_size(a.get(cell, BigInt::LEN)),
            _ => Some(0),
        };
        matches!(need, Some(n) if n <= size)
    }
}

/// Visits every traced slot of a cell of `kind` and `size` at `cell`, from the layout
/// table alone. It does not touch the arena, so the caller may mutate it in `f` (the GC
/// does).
pub fn for_each_slot(kind: CellKind, cell: u32, size: u32, mut f: impl FnMut(Slot)) {
    let p = &PLANS[kind as usize];
    let slot = |s: u32, is_ref: bool| if is_ref { Slot::Ref(s) } else { Slot::Value(s) };
    // Every cell is at least its layout's minimum size, which covers the fixed fields.
    for i in 0..p.n_fixed as usize {
        f(slot(cell + p.fixed[i] as u32, p.fixed_refs & (1 << i) != 0));
    }
    if p.n_tail == 0 {
        return;
    }
    let (elem, end) = (p.elem_size as u32, cell + size);
    let mut e = cell + p.tail_offset as u32;
    if p.n_tail == 1 && p.tail[0] == 0 && p.tail_refs == 0 && elem == 8 {
        // Arrays of values (ValueArray, PropValues, object slots): one word each.
        while e + 8 <= end {
            f(Slot::Value(e));
            e += 8;
        }
        return;
    }
    while e + elem <= end {
        for i in 0..p.n_tail as usize {
            f(slot(e + p.tail[i] as u32, p.tail_refs & (1 << i) != 0));
        }
        e += elem;
    }
}

/// [`for_each_slot`] straight from the layout table, for checking the plans.
#[cfg(test)]
pub(super) fn for_each_slot_by_layout(
    kind: CellKind,
    cell: u32,
    size: u32,
    mut f: impl FnMut(Slot),
) {
    let layout = kind.layout();
    for field in layout.fields {
        visit(field.ty, cell + field.offset, &mut f);
    }
    if let Some(tail) = &layout.tail {
        if !tail.elem.fields.iter().any(|fi| fi.ty.is_traced()) {
            return;
        }
        let n = layout.tail_capacity(size);
        for i in 0..n {
            let base = cell + tail.offset + i * tail.elem.size;
            for field in tail.elem.fields {
                visit(field.ty, base + field.offset, &mut f);
            }
        }
    }
}

/// The traced slots of a cell kind, precomputed from its layout: the fixed `Value`/`Ref`
/// fields (offset, is a `Ref`) in offset order, and those of the tail's element. What
/// [`for_each_slot`] and the compactor's old-region walk follow, instead of matching on
/// every field of the layout per cell (0224: a quarter of a collection's time).
///
/// Offsets are bytes from the cell (fixed fields) or the element (tail); bit `i` of a
/// mask is set when slot `i` is a `Ref`.
#[derive(Clone, Copy)]
pub(super) struct SlotPlan {
    pub(super) fixed: [u8; 12],
    pub(super) fixed_refs: u16,
    pub(super) n_fixed: u8,
    pub(super) n_tail: u8,
    pub(super) tail: [u8; 2],
    pub(super) tail_refs: u8,
    pub(super) tail_offset: u16,
    pub(super) elem_size: u16,
}

/// One more than the largest kind byte.
const KINDS: usize = max_kind() + 1;

const fn max_kind() -> usize {
    let (l, mut i, mut m) = (super::cells::LAYOUTS, 0, 0);
    while i < l.len() {
        if l[i].kind as usize > m {
            m = l[i].kind as usize;
        }
        i += 1;
    }
    m
}

/// [`SlotPlan`]s by kind byte (empty for unused kinds).
pub(super) static PLANS: [SlotPlan; KINDS] = plans();

const fn plans() -> [SlotPlan; KINDS] {
    let empty = SlotPlan {
        fixed: [0; 12],
        fixed_refs: 0,
        n_fixed: 0,
        n_tail: 0,
        tail: [0; 2],
        tail_refs: 0,
        tail_offset: 0,
        elem_size: 0,
    };
    let mut out = [empty; KINDS];
    let layouts = super::cells::LAYOUTS;
    let mut k = 0;
    while k < layouts.len() {
        let l = &layouts[k];
        let mut p = empty;
        let mut i = 0;
        while i < l.fields.len() {
            let f = &l.fields[i];
            let r = matches!(f.ty, FieldTy::Ref);
            if r || matches!(f.ty, FieldTy::Value) {
                assert!(
                    i == 0 || l.fields[i - 1].offset < f.offset,
                    "fields in offset order"
                );
                assert!(f.offset < 256);
                p.fixed[p.n_fixed as usize] = f.offset as u8;
                if r {
                    p.fixed_refs |= 1 << p.n_fixed;
                }
                p.n_fixed += 1;
            }
            i += 1;
        }
        if let Some(t) = &l.tail {
            p.tail_offset = t.offset as u16;
            p.elem_size = t.elem.size as u16;
            let mut i = 0;
            while i < t.elem.fields.len() {
                let f = &t.elem.fields[i];
                let r = matches!(f.ty, FieldTy::Ref);
                if r || matches!(f.ty, FieldTy::Value) {
                    p.tail[p.n_tail as usize] = f.offset as u8;
                    if r {
                        p.tail_refs |= 1 << p.n_tail;
                    }
                    p.n_tail += 1;
                }
                i += 1;
            }
        }
        out[l.kind as usize] = p;
        k += 1;
    }
    out
}

#[cfg(test)]
#[inline]
fn visit(ty: FieldTy, off: u32, f: &mut impl FnMut(Slot)) {
    match ty {
        FieldTy::Value => f(Slot::Value(off)),
        FieldTy::Ref => f(Slot::Ref(off)),
        _ => {}
    }
}

/// Whether a reference value with `tag` may point at a cell of `kind`. Internal references
/// (D007) may point at any cell that is not a JS value or the roots.
fn ref_fits(tag: Option<u16>, kind: Option<CellKind>) -> bool {
    matches!(
        (tag, kind),
        (
            Some(tag::STRING),
            Some(CellKind::Str8 | CellKind::Str16 | CellKind::Rope | CellKind::Slice)
        ) | (Some(tag::OBJECT), Some(CellKind::Object))
            | (Some(tag::SYMBOL), Some(CellKind::Symbol))
            | (Some(tag::BIGINT), Some(CellKind::BigInt))
    ) || (tag == Some(tag::INTERNAL)
        && kind.is_some_and(|k| {
            !matches!(
                k,
                CellKind::Roots
                    | CellKind::Str8
                    | CellKind::Str16
                    | CellKind::Rope
                    | CellKind::Slice
                    | CellKind::Object
                    | CellKind::Symbol
                    | CellKind::BigInt
            )
        }))
}
