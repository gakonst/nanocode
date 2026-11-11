//! Helpers for the generic array cells, [`ValueArray`] and [`U32Array`].

use super::cells::{U32Array, ValueArray};
use super::{CellKind, Heap, HeapError};
use crate::value::Value;

impl Heap {
    /// Allocates a [`ValueArray`] with room for `cap` values and `LEN` 0.
    pub(crate) fn alloc_value_array(&mut self, cap: u32) -> Result<u32, HeapError> {
        let size = ValueArray::VALUES
            .cell_size(cap)
            .ok_or(HeapError::OutOfMemory)?;
        self.alloc_cell(CellKind::ValueArray, size)
    }

    /// Slots a [`ValueArray`] can hold.
    #[inline]
    pub(crate) fn value_array_capacity(&self, arr: u32) -> u32 {
        ValueArray::LAYOUT.tail_capacity(self.cell_size(arr))
    }

    #[inline]
    pub(crate) fn value_array_len(&self, arr: u32) -> u32 {
        self.arena().get(arr, ValueArray::LEN)
    }

    #[inline]
    pub(crate) fn value_array_get(&self, arr: u32, i: u32) -> Value {
        debug_assert!(i < self.value_array_capacity(arr));
        self.arena().read(ValueArray::VALUES.at(arr, i))
    }

    #[inline]
    pub(crate) fn value_array_set(&mut self, arr: u32, i: u32, v: Value) {
        debug_assert!(i < self.value_array_capacity(arr));
        self.arena_mut().write(ValueArray::VALUES.at(arr, i), v)
    }

    /// Returns an array with the contents of `arr` (0 for none) and room for at least
    /// `min_cap` values: `arr` itself if it is big enough, else a new copy with doubled
    /// capacity. The old cell becomes garbage.
    pub(crate) fn value_array_reserve(&mut self, arr: u32, min_cap: u32) -> Result<u32, HeapError> {
        let (cap, len) = if arr == 0 {
            (0, 0)
        } else {
            (self.value_array_capacity(arr), self.value_array_len(arr))
        };
        if min_cap <= cap {
            return Ok(arr);
        }
        self.value_array_regrow(arr, len, min_cap.max(cap.saturating_mul(2)).max(8))
    }

    /// A copy of `arr`'s first `len` values in a new array with room for `new_cap`.
    pub(crate) fn value_array_regrow(
        &mut self,
        arr: u32,
        len: u32,
        new_cap: u32,
    ) -> Result<u32, HeapError> {
        let new = self.alloc_value_array(new_cap)?;
        if len > 0 {
            let src = ValueArray::VALUES.at(arr, 0);
            let dst = ValueArray::VALUES.at(new, 0);
            self.arena_mut().copy_within(src, len * 8, dst);
        }
        self.arena_mut().set(new, ValueArray::LEN, len);
        Ok(new)
    }

    /// Allocates a zeroed [`U32Array`] of `len` entries.
    pub(crate) fn alloc_u32_array(&mut self, len: u32) -> Result<u32, HeapError> {
        let size = U32Array::DATA
            .cell_size(len)
            .ok_or(HeapError::OutOfMemory)?;
        let a = self.alloc_cell(CellKind::U32Array, size)?;
        self.arena_mut().set(a, U32Array::LEN, len);
        Ok(a)
    }

    #[inline]
    pub(crate) fn u32_array_len(&self, arr: u32) -> u32 {
        self.arena().get(arr, U32Array::LEN)
    }

    #[inline]
    pub(crate) fn u32_array_get(&self, arr: u32, i: u32) -> u32 {
        debug_assert!(i < self.u32_array_len(arr));
        self.arena().read(U32Array::DATA.at(arr, i))
    }

    /// Linear probing over the hash index `index` (a [`U32Array`] whose length is a power
    /// of two), starting at bucket `hash & mask`: calls `f(bucket, value)` until it returns
    /// `Some`. Panics after visiting every bucket, which only a corrupted heap can cause,
    /// so a probe never loops forever.
    pub(crate) fn probe<R>(
        &self,
        index: u32,
        hash: u32,
        mut f: impl FnMut(u32, u32) -> Option<R>,
    ) -> R {
        let len = self.u32_array_len(index);
        if !len.is_power_of_two() {
            super::fail::bad_hash_index(index, len);
        }
        let mask = len - 1;
        let mut b = hash & mask;
        for _ in 0..len {
            if let Some(r) = f(b, self.u32_array_get(index, b)) {
                return r;
            }
            b = (b + 1) & mask;
        }
        super::fail::bad_hash_index(index, len)
    }

    /// The first empty bucket (value 0) for `hash`.
    pub(crate) fn probe_empty(&self, index: u32, hash: u32) -> u32 {
        self.probe(index, hash, |b, v| (v == 0).then_some(b))
    }

    #[inline]
    pub(crate) fn u32_array_set(&mut self, arr: u32, i: u32, v: u32) {
        debug_assert!(i < self.u32_array_len(arr));
        self.arena_mut().write(U32Array::DATA.at(arr, i), v)
    }
}
