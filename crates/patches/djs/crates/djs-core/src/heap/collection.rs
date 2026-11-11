//! Ordered hash tables for Map, Set, WeakMap and WeakSet (0038, D1381).
//!
//! A collection object (`ObjectClass::Map` / `Set` / `WeakMap` / `WeakSet`) has three
//! slots: the entries, a hash index and the live count.
//!
//! - Entries of a Map or Set: a [`ValueArray`] `[forward, removed, k0, v0, k1, v1, ...]`
//!   in insertion order (a Set stores `true` as the value). A deleted entry's key is a
//!   HOLE. When the array is replaced (it grew, or was compacted), the old one records
//!   `forward` (an internal reference to its successor) and `removed` (a [`U32Array`] of
//!   the entry numbers dropped, ascending, or undefined): an iterator that still holds
//!   the old array moves to the new one and shifts its position by the removed entries
//!   before it. So iteration sees entries added during iteration and skips deleted ones,
//!   as the spec requires, and compaction is safe.
//! - Entries of a WeakMap or WeakSet: an [`EphemeronArray`] `[k0, v0, ...]` (no
//!   iterators, so no forward/removed). The collector treats it as ephemerons (gc.rs).
//! - The index: a [`U32Array`] of power-of-two length, linear probing, holding entry
//!   number + 1 (0 = empty). Deleted entries stay in the index until a rebuild.
//!
//! Keys compare by SameValueZero; numbers are stored normalized (-0 → +0, one NaN), so
//! hashing and equality agree. Object and symbol keys hash by their offset, which the
//! copying collector changes: `collect` rebuilds the index of every collection it copied
//! (`Heap::collection_rehash`), without allocating.
use crate::PushOut;

use super::cells::{EphemeronArray, ValueArray};
use super::{CellKind, Heap, HeapError, ObjectClass};
use crate::value::{Obj, Value};

/// Slots of a collection object.
const ENTRIES: u32 = 0;
const INDEX: u32 = 1;
const LIVE: u32 = 2;

/// The first entry slot of a Map/Set entries array (after forward and removed).
const MAP_BASE: u32 = 2;
const FORWARD: u32 = 0;
const REMOVED: u32 = 1;

/// SameValueZero's canonical form of a key: -0 becomes +0, every NaN the same NaN, and
/// integral doubles in int range the int form.
pub fn normalize_key(v: Value) -> Value {
    match v.as_number() {
        Some(n) if n.is_nan() => Value::NAN,
        Some(n) => {
            let n = n + 0.0;
            let i = n as i32;
            if i as f64 == n {
                Value::int(i)
            } else {
                Value::number(n)
            }
        }
        None => v,
    }
}

/// Whether the collection object `o` (Map/Set/WeakMap/WeakSet) has an object or symbol
/// key at or above `old`: a key that a minor collection may move or (in a weak
/// collection) clear, so the index needs a rebuild (D2041). Strings and BigInts hash by
/// content and do not count.
pub(super) fn has_young_keys(arena: &super::Arena, o: u32, old: u32) -> bool {
    let entries: Value = arena.read(super::cells::Object::SLOTS.at(o, ENTRIES));
    let Some(e) = entries.as_internal() else {
        return false;
    };
    let weak = matches!(
        ObjectClass::from_u32(arena.get(o, super::cells::Object::CLASS)),
        Some(ObjectClass::WeakMap | ObjectClass::WeakSet)
    );
    let base = if weak { 0 } else { MAP_BASE };
    let len = arena.get(e, ValueArray::LEN);
    (base..len).step_by(2).any(|i| {
        let k: Value = arena.read(ValueArray::VALUES.at(e, i));
        k.ref_offset().is_some_and(|t| t >= old)
            && k.as_string().is_none()
            && k.as_bigint().is_none()
    })
}

impl Heap {
    fn is_weak_collection(&self, o: Obj) -> bool {
        matches!(
            self.object_class(o),
            ObjectClass::WeakMap | ObjectClass::WeakSet
        )
    }

    fn entries_base(&self, o: Obj) -> u32 {
        if self.is_weak_collection(o) {
            0
        } else {
            MAP_BASE
        }
    }

    pub(super) fn collection_entries(&self, o: Obj) -> u32 {
        self.object_slot(o, ENTRIES).as_internal().expect("entries")
    }

    pub(super) fn collection_index(&self, o: Obj) -> u32 {
        self.object_slot(o, INDEX).as_internal().expect("index")
    }

    /// Entry slots used (live and deleted).
    fn collection_used(&self, o: Obj) -> u32 {
        (self.value_array_len(self.collection_entries(o)) - self.entries_base(o)) / 2
    }

    /// The live entry count (`size`).
    pub fn collection_size(&self, o: Obj) -> u32 {
        self.object_slot(o, LIVE).as_int().unwrap_or(0) as u32
    }

    /// The hash of a normalized key, keyed by the VM's hash key (D2433), so crafted keys
    /// don't collide. Strings and BigInts hash by content; objects and symbols by offset
    /// (rebuilt after every collection).
    pub(super) fn key_hash(&self, k: Value) -> u32 {
        if let Some(s) = k.as_string() {
            return self.str_hash_uncached(s);
        }
        let key = self.hash_key();
        if let Some(n) = k.as_number() {
            return super::string::keyed_u64(key, n.to_bits());
        }
        if k.as_bigint().is_some() {
            return super::string::keyed_u64(key, self.bigint_hash(k));
        }
        super::string::keyed_u64(key, k.to_bits())
    }

    fn keys_equal(&self, a: Value, b: Value) -> bool {
        if let (Some(x), Some(y)) = (a.as_number(), b.as_number()) {
            return x == y || (x.is_nan() && y.is_nan());
        }
        if let (Some(x), Some(y)) = (a.as_string(), b.as_string()) {
            return self.str_eq(x, y);
        }
        if a.as_bigint().is_some() && b.as_bigint().is_some() {
            return self.bigint_eq(a, b);
        }
        a == b
    }

    pub(super) fn coll_key(&self, entries: u32, base: u32, n: u32) -> Value {
        self.value_array_get(entries, base + 2 * n)
    }

    /// A new, empty collection of `class` with prototype `proto`.
    pub fn collection_new(&mut self, class: ObjectClass, proto: Obj) -> Result<Obj, HeapError> {
        let o = self.alloc_object(class, Some(proto), class.slots())?;
        let weak = matches!(class, ObjectClass::WeakMap | ObjectClass::WeakSet);
        let entries = if weak {
            self.alloc_ephemerons(8)?
        } else {
            let e = self.alloc_value_array(MAP_BASE + 8)?;
            self.value_array_set(e, FORWARD, Value::UNDEFINED);
            self.value_array_set(e, REMOVED, Value::UNDEFINED);
            self.arena_mut().set(e, ValueArray::LEN, MAP_BASE);
            e
        };
        self.set_object_slot(o, ENTRIES, Value::internal(entries));
        let index = self.alloc_u32_array(8)?;
        self.set_object_slot(o, INDEX, Value::internal(index));
        self.set_object_slot(o, LIVE, Value::int(0));
        Ok(o)
    }

    fn alloc_ephemerons(&mut self, cap: u32) -> Result<u32, HeapError> {
        let size = EphemeronArray::VALUES
            .cell_size(cap)
            .ok_or(HeapError::OutOfMemory)?;
        let e = self.alloc_cell(CellKind::EphemeronArray, size)?;
        self.arena_mut().set(e, EphemeronArray::LEN, 0);
        Ok(e)
    }

    /// The entry number of `key` (normalized), if present.
    fn collection_find(&self, o: Obj, key: Value) -> Option<u32> {
        let entries = self.collection_entries(o);
        let base = self.entries_base(o);
        let index = self.collection_index(o);
        let h = self.key_hash(key);
        self.probe(index, h, |_, v| {
            if v == 0 {
                return Some(None);
            }
            let k = self.coll_key(entries, base, v - 1);
            (!k.is_hole() && self.keys_equal(k, key)).then_some(Some(v - 1))
        })
    }

    /// `m.get(key)`: the value, or `None`.
    pub fn collection_get(&self, o: Obj, key: Value) -> Option<Value> {
        let key = normalize_key(key);
        let n = self.collection_find(o, key)?;
        let entries = self.collection_entries(o);
        Some(self.value_array_get(entries, self.entries_base(o) + 2 * n + 1))
    }

    /// `m.has(key)`.
    pub fn collection_has(&self, o: Obj, key: Value) -> bool {
        self.collection_find(o, normalize_key(key)).is_some()
    }

    /// `m.set(key, value)` / `s.add(key)` (a Set passes `true` as the value). String
    /// keys should be flat (the caller flattens them) so their hash is cheap.
    pub fn collection_set(&mut self, o: Obj, key: Value, value: Value) -> Result<(), HeapError> {
        let key = normalize_key(key);
        if let Some(s) = key.as_string() {
            // Cached in the string (D2433): later lookups with it skip the keyed hash.
            self.str_hash(s);
        }
        let base = self.entries_base(o);
        if let Some(n) = self.collection_find(o, key) {
            let entries = self.collection_entries(o);
            self.value_array_set(entries, base + 2 * n + 1, value);
            return Ok(());
        }
        let used = self.collection_used(o);
        // Keep the index at most half full; rebuild it (bigger) before inserting.
        if 2 * (used + 1) > self.u32_array_len(self.collection_index(o)) {
            let live = self.collection_size(o);
            if live * 2 < used && used >= 8 {
                self.collection_compact(o)?;
            }
            let need = 2 * (self.collection_used(o) + 1);
            self.collection_reindex(o, need.next_power_of_two().max(8))?;
        }
        let used = self.collection_used(o);
        let entries = self.collection_entries(o);
        let len = base + 2 * used;
        let entries = if len + 2 > self.value_array_capacity(entries) {
            self.collection_grow(o, entries, len + 2)?
        } else {
            entries
        };
        self.value_array_set(entries, len, key);
        self.value_array_set(entries, len + 1, value);
        self.arena_mut().set(entries, ValueArray::LEN, len + 2);
        let index = self.collection_index(o);
        let b = self.probe_empty(index, self.key_hash(key));
        self.u32_array_set(index, b, used + 1);
        let live = self.collection_size(o);
        self.set_object_slot(o, LIVE, Value::int(live as i32 + 1));
        Ok(())
    }

    /// A bigger copy of the entries; the old Map/Set array forwards to it (nothing
    /// removed), so iterators holding it move over.
    fn collection_grow(&mut self, o: Obj, old: u32, min_cap: u32) -> Result<u32, HeapError> {
        let cap = min_cap.max(self.value_array_capacity(old) * 2);
        let len = self.value_array_len(old);
        let new = if self.is_weak_collection(o) {
            self.alloc_ephemerons(cap)?
        } else {
            self.alloc_value_array(cap)?
        };
        for i in 0..len {
            let v = self.value_array_get(old, i);
            self.value_array_set(new, i, v);
        }
        self.arena_mut().set(new, ValueArray::LEN, len);
        if !self.is_weak_collection(o) {
            self.value_array_set(new, FORWARD, Value::UNDEFINED);
            self.value_array_set(new, REMOVED, Value::UNDEFINED);
            self.value_array_set(old, FORWARD, Value::internal(new));
        }
        self.set_object_slot(o, ENTRIES, Value::internal(new));
        Ok(new)
    }

    /// Drops the deleted entries into a new array (the old one forwards to it with the
    /// removed entry numbers) and rebuilds the index.
    fn collection_compact(&mut self, o: Obj) -> Result<(), HeapError> {
        let old = self.collection_entries(o);
        let base = self.entries_base(o);
        let used = self.collection_used(o);
        let live = self.collection_size(o);
        let weak = self.is_weak_collection(o);
        let cap = base + 2 * live.max(4);
        let new = if weak {
            self.alloc_ephemerons(cap)?
        } else {
            self.alloc_value_array(cap)?
        };
        let mut removed = alloc::vec::Vec::new();
        let mut j = 0;
        for n in 0..used {
            let k = self.coll_key(old, base, n);
            if k.is_hole() {
                removed.push_out(n);
                continue;
            }
            let v = self.value_array_get(old, base + 2 * n + 1);
            self.value_array_set(new, base + 2 * j, k);
            self.value_array_set(new, base + 2 * j + 1, v);
            j += 1;
        }
        self.arena_mut().set(new, ValueArray::LEN, base + 2 * j);
        if !weak {
            self.value_array_set(new, FORWARD, Value::UNDEFINED);
            self.value_array_set(new, REMOVED, Value::UNDEFINED);
            let r = self.alloc_u32_array(removed.len() as u32)?;
            for (i, &n) in removed.iter().enumerate() {
                self.u32_array_set(r, i as u32, n);
            }
            self.value_array_set(old, REMOVED, Value::internal(r));
            self.value_array_set(old, FORWARD, Value::internal(new));
        }
        self.set_object_slot(o, ENTRIES, Value::internal(new));
        let cap = (2 * (j + 1)).next_power_of_two().max(8);
        self.collection_reindex(o, cap)
    }

    /// A new index of `cap` buckets.
    fn collection_reindex(&mut self, o: Obj, cap: u32) -> Result<(), HeapError> {
        let index = self.alloc_u32_array(cap)?;
        self.set_object_slot(o, INDEX, Value::internal(index));
        self.collection_rehash(o);
        Ok(())
    }

    /// Rebuilds the index in place (same size, no allocation) from the entries, and
    /// recounts the live entries. `collect` calls it for every collection it copied,
    /// since object and symbol keys hash by offset.
    #[inline(never)]
    pub(crate) fn collection_rehash(&mut self, o: Obj) {
        let entries = self.collection_entries(o);
        let base = self.entries_base(o);
        let index = self.collection_index(o);
        let len = self.u32_array_len(index);
        for b in 0..len {
            self.u32_array_set(index, b, 0);
        }
        let used = self.collection_used(o);
        let mut live = 0;
        for n in 0..used {
            let k = self.coll_key(entries, base, n);
            if k.is_hole() {
                continue;
            }
            let b = self.probe_empty(index, self.key_hash(k));
            self.u32_array_set(index, b, n + 1);
            live += 1;
        }
        // (Unchanged after a rekey: a Map object of the builtin prefix is not written.)
        if self.object_slot(o, LIVE) != Value::int(live) {
            self.set_object_slot(o, LIVE, Value::int(live));
        }
    }

    /// `m.delete(key)`.
    pub fn collection_delete(&mut self, o: Obj, key: Value) -> Result<bool, HeapError> {
        let Some(n) = self.collection_find(o, normalize_key(key)) else {
            return Ok(false);
        };
        let entries = self.collection_entries(o);
        let base = self.entries_base(o);
        self.value_array_set(entries, base + 2 * n, Value::HOLE);
        self.value_array_set(entries, base + 2 * n + 1, Value::UNDEFINED);
        let live = self.collection_size(o) - 1;
        self.set_object_slot(o, LIVE, Value::int(live as i32));
        let used = self.collection_used(o);
        if used >= 16 && live * 4 < used {
            self.collection_compact(o)?;
        }
        Ok(true)
    }

    /// `m.clear()`: every entry is removed (iterators continue with entries added later).
    pub fn collection_clear(&mut self, o: Obj) -> Result<(), HeapError> {
        let entries = self.collection_entries(o);
        let base = self.entries_base(o);
        for n in 0..self.collection_used(o) {
            self.value_array_set(entries, base + 2 * n, Value::HOLE);
            self.value_array_set(entries, base + 2 * n + 1, Value::UNDEFINED);
        }
        self.set_object_slot(o, LIVE, Value::int(0));
        self.collection_compact(o)
    }

    /// A new Map/Set iterator over `o` (kind 0 keys, 1 values, 2 entries).
    pub fn collection_iterator(&mut self, o: Obj, kind: i32, proto: Obj) -> Result<Obj, HeapError> {
        let is_set = self.object_class(o) == ObjectClass::Set;
        let it = self.alloc_object(
            ObjectClass::CollectionIterator,
            Some(proto),
            ObjectClass::CollectionIterator.slots(),
        )?;
        let entries = self.object_slot(o, ENTRIES);
        self.set_object_slot(it, 0, entries);
        self.set_object_slot(it, 1, Value::int(0));
        self.set_object_slot(it, 2, Value::int(kind));
        self.set_object_slot(it, 3, Value::int(is_set as i32));
        self.set_object_slot(it, 4, Value::UNDEFINED);
        self.set_object_slot(it, 5, Value::UNDEFINED);
        Ok(it)
    }

    /// The next live entry of an iterator as (key, value), following forwarded entries
    /// arrays; `None` once done (and from then on).
    pub fn collection_iterator_next(&mut self, it: Obj) -> Option<(Value, Value)> {
        let mut entries = self.object_slot(it, 0).as_internal()?;
        let mut n = self.object_slot(it, 1).as_int().unwrap_or(0) as u32;
        while let Some(next) = self.value_array_get(entries, FORWARD).as_internal() {
            if let Some(r) = self.value_array_get(entries, REMOVED).as_internal() {
                let len = self.u32_array_len(r);
                let before = (0..len).filter(|&i| self.u32_array_get(r, i) < n).count();
                n -= before as u32;
            }
            entries = next;
        }
        let used = (self.value_array_len(entries) - MAP_BASE) / 2;
        while n < used {
            let k = self.coll_key(entries, MAP_BASE, n);
            n += 1;
            if !k.is_hole() {
                let v = self.value_array_get(entries, MAP_BASE + 2 * (n - 1) + 1);
                self.set_object_slot(it, 0, Value::internal(entries));
                self.set_object_slot(it, 1, Value::int(n as i32));
                self.set_object_slot(it, 4, k);
                self.set_object_slot(it, 5, v);
                return Some((k, v));
            }
        }
        self.set_object_slot(it, 0, Value::UNDEFINED);
        None
    }

    /// Where a Map or Set iterator stands, for the dumper (0172): the entries array it
    /// walks now (forwarding followed) and how many live entries it has passed; `None`
    /// once done.
    #[cfg(feature = "dump")]
    pub fn collection_iterator_position(&self, it: Obj) -> Option<(u32, u32)> {
        let mut entries = self.object_slot(it, 0).as_internal()?;
        let mut n = self.object_slot(it, 1).as_int().unwrap_or(0) as u32;
        while let Some(next) = self.value_array_get(entries, FORWARD).as_internal() {
            if let Some(r) = self.value_array_get(entries, REMOVED).as_internal() {
                let len = self.u32_array_len(r);
                let before = (0..len).filter(|&i| self.u32_array_get(r, i) < n).count();
                n -= before as u32;
            }
            entries = next;
        }
        let live = (0..n)
            .filter(|&i| !self.coll_key(entries, MAP_BASE, i).is_hole())
            .count();
        Some((entries, live as u32))
    }

    /// The live entries of a Map or Set entries array.
    #[cfg(feature = "dump")]
    pub fn collection_entries_of(&self, entries: u32) -> alloc::vec::Vec<(Value, Value)> {
        let used = (self.value_array_len(entries) - MAP_BASE) / 2;
        (0..used)
            .filter_map(|n| {
                let k = self.coll_key(entries, MAP_BASE, n);
                (!k.is_hole()).then(|| (k, self.value_array_get(entries, MAP_BASE + 2 * n + 1)))
            })
            .collect()
    }

    /// The entries array of a Map or Set (the one its iterators lead to).
    #[cfg(feature = "dump")]
    pub fn collection_entries_ref(&self, o: Obj) -> Option<u32> {
        self.object_slot(o, ENTRIES).as_internal()
    }

    /// The live keys and values of a collection in order (for the builtins that read a
    /// snapshot, like the Set methods' receiver copies).
    pub fn collection_entries_list(&self, o: Obj) -> alloc::vec::Vec<(Value, Value)> {
        let entries = self.collection_entries(o);
        let base = self.entries_base(o);
        (0..self.collection_used(o))
            .filter_map(|n| {
                let k = self.coll_key(entries, base, n);
                (!k.is_hole()).then(|| (k, self.value_array_get(entries, base + 2 * n + 1)))
            })
            .collect()
    }
}
