//! The handle table: values held from outside the heap.
//!
//! A [`Handle`] is an index into the [`HandleTable`] cell, which hangs off
//! [`Roots::HANDLES`]. Handle slots are GC roots, and the GC updates them when it moves
//! cells. A `Handle` is therefore the only thing Rust code (or the embedder) may keep across
//! [`Heap::collect`](super::Heap::collect): raw `Obj`/`Str`/`Sym` values are arena offsets
//! and are stale after a collection.
//!
//! Free slots form a linked list through [`HandleSlot::NEXT`], so dropping and creating
//! handles does not grow the table. Indexes stay stable for the lifetime of a handle,
//! including across collections and snapshots.

use super::cells::{HANDLE_LIVE, HandleSlot, HandleTable, Roots};
use super::layout::Ref;
use super::{CellKind, Heap, HeapError};
use crate::value::Value;

/// A GC root holding one value. See the module docs.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Handle(u32);

impl Handle {
    /// The slot index (what the embedder stores).
    pub const fn index(self) -> u32 {
        self.0
    }

    /// Rebuilds a handle from its index. Using an index that is not a live handle panics.
    pub const fn from_index(i: u32) -> Handle {
        Handle(i)
    }
}

const INITIAL_HANDLES: u32 = 16;

impl Heap {
    #[inline]
    fn handle_table(&self) -> u32 {
        self.arena().get(0, Roots::HANDLES).0
    }

    #[inline]
    fn handle_slot(t: u32, i: u32) -> u32 {
        HandleTable::SLOTS.at(t, i)
    }

    fn alloc_handle_table(&mut self, cap: u32) -> Result<u32, HeapError> {
        let size = HandleTable::SLOTS
            .cell_size(cap)
            .ok_or(HeapError::OutOfMemory)?;
        self.alloc_cell(CellKind::HandleTable, size)
    }

    /// Returns the slot of live handle `h`, panicking if it is not live.
    fn live_slot(&self, h: Handle) -> u32 {
        let t = self.handle_table();
        if t == 0 || h.0 >= self.arena().get(t, HandleTable::LEN) {
            super::fail::invalid_handle(h, false);
        }
        let slot = Self::handle_slot(t, h.0);
        if self.arena().get(slot, HandleSlot::NEXT) != HANDLE_LIVE {
            super::fail::invalid_handle(h, true);
        }
        slot
    }

    /// Creates a handle holding `v`.
    pub fn new_handle(&mut self, v: Value) -> Result<Handle, HeapError> {
        let mut t = self.handle_table();
        if t == 0 {
            t = self.alloc_handle_table(INITIAL_HANDLES)?;
            self.arena_mut().set(0, Roots::HANDLES, Ref(t));
        }
        let free = self.arena().get(t, HandleTable::FREE);
        let i = if free != 0 {
            let i = free - 1;
            let next = self.arena().get(Self::handle_slot(t, i), HandleSlot::NEXT);
            self.arena_mut().set(t, HandleTable::FREE, next);
            i
        } else {
            let len = self.arena().get(t, HandleTable::LEN);
            if len == HandleTable::LAYOUT.tail_capacity(self.cell_size(t)) {
                t = self.grow_handle_table(t, len)?;
            }
            self.arena_mut().set(t, HandleTable::LEN, len + 1);
            len
        };
        let slot = Self::handle_slot(t, i);
        let a = self.arena_mut();
        a.set(slot, HandleSlot::VALUE, v);
        a.set(slot, HandleSlot::NEXT, HANDLE_LIVE);
        let live = a.get(t, HandleTable::LIVE);
        a.set(t, HandleTable::LIVE, live + 1);
        Ok(Handle(i))
    }

    /// Copies the table into one with twice the room. The old cell becomes garbage.
    fn grow_handle_table(&mut self, t: u32, len: u32) -> Result<u32, HeapError> {
        let new = self.alloc_handle_table(len.saturating_mul(2).max(INITIAL_HANDLES))?;
        let esize = HandleTable::SLOTS.elem_size();
        let a = self.arena_mut();
        a.copy_within(
            Self::handle_slot(t, 0),
            len * esize,
            Self::handle_slot(new, 0),
        );
        for f in [HandleTable::LEN, HandleTable::FREE, HandleTable::LIVE] {
            let v = a.get(t, f);
            a.set(new, f, v);
        }
        a.set(0, Roots::HANDLES, Ref(new));
        Ok(new)
    }

    /// The value of a live handle.
    pub fn get_handle(&self, h: Handle) -> Value {
        let slot = self.live_slot(h);
        self.arena().get(slot, HandleSlot::VALUE)
    }

    /// Replaces the value of a live handle.
    pub fn set_handle(&mut self, h: Handle, v: Value) {
        let slot = self.live_slot(h);
        self.arena_mut().set(slot, HandleSlot::VALUE, v);
    }

    /// Releases a handle. Its value is no longer a root. Dropping twice panics.
    pub fn drop_handle(&mut self, h: Handle) {
        let slot = self.live_slot(h);
        let t = self.handle_table();
        let a = self.arena_mut();
        let free = a.get(t, HandleTable::FREE);
        a.set(slot, HandleSlot::VALUE, Value::UNDEFINED);
        a.set(slot, HandleSlot::NEXT, free);
        a.set(t, HandleTable::FREE, h.0 + 1);
        let live = a.get(t, HandleTable::LIVE);
        a.set(t, HandleTable::LIVE, live - 1);
    }

    /// Number of live handles.
    pub fn handle_count(&self) -> u32 {
        match self.handle_table() {
            0 => 0,
            t => self.arena().get(t, HandleTable::LIVE),
        }
    }
}
