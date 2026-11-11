//! WeakRef and FinalizationRegistry storage (0051, D1513), on the ephemeron tables of
//! D1382: the collector clears a pair whose key died (HOLE key, undefined value), and
//! that is all the weakness these objects need.
//!
//! - A WeakRef (`ObjectClass::WeakRef`) has one slot, an [`EphemeronArray`] with the
//!   pair `[target, true]`. `deref` reads the key; HOLE means the target died.
//! - A FinalizationRegistry (`ObjectClass::FinalizationRegistry`) keeps its cells in
//!   three parallel arrays: slot 1 the targets (an EphemeronArray of `[target, true]`),
//!   slot 2 the unregister tokens (an EphemeronArray of `[token, true]`, or a HOLE pair
//!   for none), slot 3 the held values (a ValueArray; HOLE marks a free cell). Slot 0 is
//!   the cleanup callback, slot 4 an int: 1 while a cleanup job is queued. A cell whose
//!   held value is set but whose target is HOLE is dead and waits for its callback.
//! - The realm's list of registries (`Intrinsic::Registries`) is itself an
//!   EphemeronArray of `[registry, true]`, so a registry that died takes its pending
//!   callbacks with it (the spec allows that). After a collection the interpreter asks
//!   [`Heap::registries_needing_cleanup`] and queues the jobs (interp/run/late.rs).
//!
//! Everything iterates in index order, so it depends only on the object graph and the
//! collections stay deterministic (D1090).
//!
//! Strong mode (D1680, [`Heap::weak_refs_strong`]): a WeakRef's pair and a registry's
//! targets table are made as ValueArrays instead (the same layout: LEN, then the
//! values), which the collector traces like any array. Nothing they reference dies, so
//! `deref` always finds its target and no cleanup is ever due, whenever collections
//! run. A table keeps its kind when it grows.

use alloc::vec::Vec;

use super::cells::{EphemeronArray, ValueArray};
use super::object::ObjectClass;
use super::{CellKind, Heap, HeapError};
use crate::value::{Obj, Value};

const TARGETS: u32 = 1;
const TOKENS: u32 = 2;
const HELD: u32 = 3;
const FLAGS: u32 = 4;

impl Heap {
    /// An EphemeronArray with room for `pairs` pairs, LEN 0.
    fn alloc_pairs(&mut self, pairs: u32) -> Result<u32, HeapError> {
        self.alloc_pairs_of(CellKind::EphemeronArray, pairs)
    }

    /// A pair table of `kind`: an EphemeronArray (weak keys) or, in strong mode, a
    /// ValueArray with the same layout (D1680).
    fn alloc_pairs_of(&mut self, kind: CellKind, pairs: u32) -> Result<u32, HeapError> {
        // Strong tables rely on the two layouts being the same.
        const {
            assert!(EphemeronArray::VALUES.offset == ValueArray::VALUES.offset);
            assert!(EphemeronArray::LEN.offset == ValueArray::LEN.offset);
        }
        let size = EphemeronArray::VALUES
            .cell_size(pairs * 2)
            .ok_or(HeapError::OutOfMemory)?;
        let e = self.alloc_cell(kind, size)?;
        self.arena_mut().set(e, EphemeronArray::LEN, 0);
        Ok(e)
    }

    /// The kind of the tables that hold weak targets: weak, or strong in strong mode.
    fn weak_table_kind(&self) -> CellKind {
        if self.weak_refs_strong() {
            CellKind::ValueArray
        } else {
            CellKind::EphemeronArray
        }
    }

    fn pairs_len(&self, e: u32) -> u32 {
        self.arena().get(e, EphemeronArray::LEN) / 2
    }

    fn pair_key(&self, e: u32, i: u32) -> Value {
        self.arena().read(EphemeronArray::VALUES.at(e, 2 * i))
    }

    fn set_pair(&mut self, e: u32, i: u32, k: Value, v: Value) {
        self.arena_mut()
            .write(EphemeronArray::VALUES.at(e, 2 * i), k);
        self.arena_mut()
            .write(EphemeronArray::VALUES.at(e, 2 * i + 1), v);
    }

    /// Appends a pair, growing the table (a new cell, returned) when it is full.
    fn push_pair(&mut self, e: u32, k: Value, v: Value) -> Result<u32, HeapError> {
        let n = self.pairs_len(e);
        let cap = EphemeronArray::LAYOUT.tail_capacity(self.cell_size(e)) / 2;
        let e = if n < cap {
            e
        } else {
            let new = self.alloc_pairs_of(self.cell_kind(e), (cap * 2).max(4))?;
            let src = EphemeronArray::VALUES.at(e, 0);
            let dst = EphemeronArray::VALUES.at(new, 0);
            self.arena_mut().copy_within(src, n * 16, dst);
            new
        };
        self.set_pair(e, n, k, v);
        self.arena_mut().set(e, EphemeronArray::LEN, 2 * (n + 1));
        Ok(e)
    }

    /// A new WeakRef with prototype `proto` and target `target` (an object or a symbol
    /// that can be held weakly).
    pub fn weakref_new(&mut self, proto: Obj, target: Value) -> Result<Obj, HeapError> {
        let cell = self.alloc_pairs_of(self.weak_table_kind(), 1)?;
        self.push_pair(cell, target, Value::TRUE)?;
        let o = self.alloc_object(ObjectClass::WeakRef, Some(proto), 1)?;
        self.set_object_slot(o, 0, Value::internal(cell));
        Ok(o)
    }

    /// The target of a WeakRef, or `None` once it died.
    pub fn weakref_target(&self, o: Obj) -> Option<Value> {
        let cell = self.object_slot(o, 0).as_internal()?;
        let k = self.pair_key(cell, 0);
        (!k.is_hole()).then_some(k)
    }

    /// A new FinalizationRegistry with prototype `proto` and cleanup callback `cleanup`.
    /// The caller registers it in the realm's list ([`Heap::registry_list_push`]).
    pub fn registry_new(&mut self, proto: Obj, cleanup: Value) -> Result<Obj, HeapError> {
        let targets = self.alloc_pairs_of(self.weak_table_kind(), 4)?;
        let tokens = self.alloc_pairs(4)?;
        let held = self.alloc_value_array(4)?;
        self.arena_mut().set(held, ValueArray::LEN, 0);
        let o = self.alloc_object(ObjectClass::FinalizationRegistry, Some(proto), 5)?;
        self.set_object_slot(o, 0, cleanup);
        self.set_object_slot(o, TARGETS, Value::internal(targets));
        self.set_object_slot(o, TOKENS, Value::internal(tokens));
        self.set_object_slot(o, HELD, Value::internal(held));
        self.set_object_slot(o, FLAGS, Value::int(0));
        Ok(o)
    }

    fn registry_cells(&self, r: Obj) -> (u32, u32, u32) {
        let slot = |i| self.object_slot(r, i).as_internal().expect("registry cell");
        (slot(TARGETS), slot(TOKENS), slot(HELD))
    }

    /// Appends `r` to the realm's weak list of registries `list` (0 for none yet);
    /// returns the list cell (new when it grew).
    pub fn registry_list_push(&mut self, list: u32, r: Obj) -> Result<u32, HeapError> {
        let list = if list == 0 {
            self.alloc_pairs(4)?
        } else {
            // Drop the pairs of dead registries before growing.
            self.compact_pairs(list);
            list
        };
        self.push_pair(list, Value::object(r), Value::TRUE)
    }

    /// Removes cleared pairs (HOLE keys) from a list table, keeping the order.
    fn compact_pairs(&mut self, e: u32) {
        let n = self.pairs_len(e);
        let mut w = 0;
        for i in 0..n {
            let k = self.pair_key(e, i);
            if !k.is_hole() {
                self.set_pair(e, w, k, Value::TRUE);
                w += 1;
            }
        }
        for i in w..n {
            // Beyond LEN the slots are 0, as for a fresh table.
            let at = EphemeronArray::VALUES.at(e, 2 * i);
            self.arena_mut().write(at, Value::from_raw(0));
            self.arena_mut().write(at + 8, Value::from_raw(0));
        }
        self.arena_mut().set(e, EphemeronArray::LEN, 2 * w);
    }

    /// Adds a cell to registry `r` (`token` HOLE for none), reusing a free one.
    pub fn registry_register(
        &mut self,
        r: Obj,
        target: Value,
        held: Value,
        token: Value,
    ) -> Result<(), HeapError> {
        let (targets, tokens, held_arr) = self.registry_cells(r);
        let token_value = if token.is_hole() {
            Value::UNDEFINED
        } else {
            Value::TRUE
        };
        let n = self.value_array_len(held_arr);
        if let Some(i) = (0..n).find(|&i| self.value_array_get(held_arr, i).is_hole()) {
            self.set_pair(targets, i, target, Value::TRUE);
            self.set_pair(tokens, i, token, token_value);
            self.value_array_set(held_arr, i, held);
            return Ok(());
        }
        let targets = self.push_pair(targets, target, Value::TRUE)?;
        self.set_object_slot(r, TARGETS, Value::internal(targets));
        let tokens = self.push_pair(tokens, token, token_value)?;
        self.set_object_slot(r, TOKENS, Value::internal(tokens));
        let held_arr = self.value_array_reserve(held_arr, n + 1)?;
        self.value_array_set(held_arr, n, held);
        self.arena_mut().set(held_arr, ValueArray::LEN, n + 1);
        self.set_object_slot(r, HELD, Value::internal(held_arr));
        Ok(())
    }

    /// Frees cell i of a registry.
    fn registry_free(&mut self, r: Obj, i: u32) {
        let (targets, tokens, held) = self.registry_cells(r);
        self.set_pair(targets, i, Value::HOLE, Value::UNDEFINED);
        self.set_pair(tokens, i, Value::HOLE, Value::UNDEFINED);
        self.value_array_set(held, i, Value::HOLE);
    }

    /// Removes the cells whose unregister token is `token`; whether there were any.
    pub fn registry_unregister(&mut self, r: Obj, token: Value) -> bool {
        let (_, tokens, held) = self.registry_cells(r);
        let mut removed = false;
        for i in 0..self.value_array_len(held) {
            if !self.value_array_get(held, i).is_hole() && self.pair_key(tokens, i) == token {
                self.registry_free(r, i);
                removed = true;
            }
        }
        removed
    }

    /// The first dead cell of a registry (its target died, its callback is due).
    pub fn registry_next_dead(&self, r: Obj) -> Option<u32> {
        let (targets, _, held) = self.registry_cells(r);
        (0..self.value_array_len(held)).find(|&i| {
            !self.value_array_get(held, i).is_hole() && self.pair_key(targets, i).is_hole()
        })
    }

    /// Takes the held value of dead cell i and frees the cell; `None` if cell i is not
    /// dead.
    pub fn registry_take(&mut self, r: Obj, i: u32) -> Option<Value> {
        let (targets, _, held) = self.registry_cells(r);
        if i >= self.value_array_len(held) || !self.pair_key(targets, i).is_hole() {
            return None;
        }
        let v = self.value_array_get(held, i);
        if v.is_hole() {
            return None;
        }
        self.registry_free(r, i);
        Some(v)
    }

    /// Whether a cleanup job for `r` is queued.
    pub fn registry_queued(&self, r: Obj) -> bool {
        self.object_slot(r, FLAGS) == Value::int(1)
    }

    pub fn set_registry_queued(&mut self, r: Obj, queued: bool) {
        self.set_object_slot(r, FLAGS, Value::int(queued as i32));
    }

    /// The live registries of the list `list` (0 for none) that have dead cells and no
    /// queued cleanup job, in registration order.
    pub fn registries_needing_cleanup(&self, list: u32) -> Vec<Obj> {
        if list == 0 {
            return Vec::new();
        }
        (0..self.pairs_len(list))
            .filter_map(|i| self.pair_key(list, i).as_object())
            .filter(|&r| !self.registry_queued(r) && self.registry_next_dead(r).is_some())
            .collect()
    }
}
