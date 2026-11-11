//! Shapes (hidden classes, 0083, docs/objects.md 7): the shared key layout of shape-mode
//! objects.
//!
//! A [`Shape`] lists property keys and their attributes in insertion order; an object in
//! shape mode keeps only the values, in a [`PropValues`] block whose `SHAPE` is its
//! shape. Shapes form a tree rooted at [`Roots::ROOT_SHAPE`]: adding a key goes from a
//! shape to its child for `(key, flags)`, found in the parent's `TRANSITIONS` or created
//! there.
//!
//! Transitions are strong, so shapes never die (D1831). That keeps the tree a function of
//! the program's history alone, never of when collections ran, which the canonical hash
//! (D1730) needs. Its growth is bounded instead: at most [`SHAPE_MAX`] properties per
//! shape and [`SHAPE_BUDGET`] shapes per heap. Past either the object goes to dictionary
//! mode. A shape's children are sorted by key and flags, so finding a transition is a
//! binary search however wide the tree is (the root has a child for every first key).
//!
//! Descriptors (`DESCS`, a [`U32Array`] `[used, key0, flags0, ...]`) are shared along a
//! chain: a child of the shape that wrote the last used pair appends in place (if there is
//! room) and every other child copies the prefix. So a chain of n shapes costs O(n)
//! descriptor words.
use crate::PushOut;

use super::atom::Atom;
use super::cells::{RefArray, Roots, Shape, U32Array};
use super::layout::Ref;
use super::{CellKind, Heap, HeapError};

/// Shapes have at most this many properties; more go to dictionary mode.
pub const SHAPE_MAX: u32 = 32;

/// Shapes per heap, the root included (D1831). Once spent, new key sets go to dictionary
/// mode. A shape costs about 40 bytes (cell, its parent's transition slot, its share of
/// the descriptors), so the budget bounds the tree at about 160 KB typically and 2.3 MB
/// at worst (every shape copying a 32-property prefix).
pub const SHAPE_BUDGET: u32 = 4096;

impl Heap {
    /// The root shape, created on first use.
    pub(super) fn root_shape(&mut self) -> Result<u32, HeapError> {
        if let Some(s) = self.arena().get(0, Roots::ROOT_SHAPE).get() {
            return Ok(s);
        }
        let s = self.alloc_cell(CellKind::Shape, Shape::LAYOUT.min_size)?;
        let n = self.arena().get(0, Roots::SHAPES);
        let a = self.arena_mut();
        a.set(0, Roots::ROOT_SHAPE, Ref(s));
        a.set(0, Roots::SHAPES, n + 1);
        Ok(s)
    }

    /// Creates the shapes of the chain `keys` (key, stored flags) from the root shape, as
    /// adding those properties in that order to an empty object would (D2067).
    pub(crate) fn precreate_shapes(&mut self, keys: &[(&str, u32)]) -> Result<(), HeapError> {
        let mut s = self.root_shape()?;
        for &(k, flags) in keys {
            let key = self.atom_str(k)?;
            s = self
                .shape_child(s, key, flags)?
                .ok_or(HeapError::OutOfMemory)?;
        }
        Ok(())
    }

    /// The cells of the tree under the root shape: each shape, then its descriptors and
    /// transitions, depth-first in transition order. Empty without a root shape.
    pub(crate) fn root_shape_tree(&self) -> alloc::vec::Vec<u32> {
        let mut out = alloc::vec::Vec::new();
        let mut stack: alloc::vec::Vec<u32> = self
            .arena()
            .get::<Ref>(0, Roots::ROOT_SHAPE)
            .get()
            .into_iter()
            .collect();
        while let Some(s) = stack.pop() {
            out.push_out(s);
            let a = self.arena();
            if let Some(d) = a.get::<Ref>(s, Shape::DESCS).get() {
                out.push_out(d);
            }
            if let Some(t) = a.get::<Ref>(s, Shape::TRANSITIONS).get() {
                out.push_out(t);
                let len = a.get(t, RefArray::LEN);
                for i in (0..len).rev() {
                    stack.push_out(a.read::<Ref>(RefArray::REFS.at(t, i)).0);
                }
            }
        }
        // Descriptors are shared along a chain.
        let mut seen = alloc::collections::BTreeSet::new();
        out.retain(|&c| seen.insert(c));
        out
    }

    #[inline]
    pub(super) fn shape_count(&self, s: u32) -> u32 {
        self.arena().get(s, Shape::COUNT)
    }

    #[inline]
    pub(super) fn shape_parent(&self, s: u32) -> u32 {
        self.arena().get(s, Shape::PARENT).0
    }

    /// Descriptor `i` of shape `s` (`i < COUNT`): the key and the stored flags.
    #[inline]
    pub(super) fn shape_desc(&self, s: u32, i: u32) -> (Atom, u32) {
        let d = self.arena().get(s, Shape::DESCS).0;
        let a = self.arena();
        let at = U32Array::DATA.at(d, 1 + 2 * i);
        (Atom::from_id(a.read(at)), a.read(at + 4))
    }

    /// The slot of `key` in shape `s`.
    #[inline]
    pub(super) fn shape_find(&self, s: u32, key: Atom) -> Option<u32> {
        let n = self.shape_count(s);
        if n == 0 {
            return None;
        }
        let d = self.arena().get(s, Shape::DESCS).0;
        // One bounds check for the whole scan.
        let pairs = self.arena().slice(U32Array::DATA.at(d, 1), 8 * n);
        let id = key.id().to_le_bytes();
        pairs
            .as_chunks::<8>()
            .0
            .iter()
            .position(|p| p[..4] == id)
            .map(|i| i as u32)
    }

    /// The child of `s` that adds `key` with the stored `flags`: found, created, or `None`
    /// when a limit says the object should go to dictionary mode. Allocates everything
    /// before it links the child in, so a failed allocation leaves the tree as it was.
    pub(super) fn shape_child(
        &mut self,
        s: u32,
        key: Atom,
        flags: u32,
    ) -> Result<Option<u32>, HeapError> {
        let count = self.shape_count(s);
        let tr = self.arena().get(s, Shape::TRANSITIONS).0;
        let len = if tr == 0 {
            0
        } else {
            self.arena().get(tr, RefArray::LEN)
        };
        let lo = match self.transition_search(s, key, flags) {
            Ok(c) => return Ok(Some(c)),
            Err(lo) => lo,
        };
        let shapes = self.arena().get(0, Roots::SHAPES);
        if count >= SHAPE_MAX || shapes >= SHAPE_BUDGET {
            return Ok(None);
        }
        // The descriptors: shared if `s` wrote the last used pair and there is room.
        let d = self.arena().get(s, Shape::DESCS).0;
        let (used, cap) = if d == 0 {
            (0, 0)
        } else {
            (self.u32_array_get(d, 0), (self.u32_array_len(d) - 1) / 2)
        };
        let share = d != 0 && used == count && count < cap;
        let new_d = if share {
            d
        } else {
            let n = count + 1;
            // Room to grow a chain: double (at least 4 pairs).
            let cap = if used == count { 2 * n } else { n }.max(4);
            self.alloc_u32_array(1 + 2 * cap)?
        };
        let new_tr = if tr != 0 && len < self.ref_array_capacity(tr) {
            tr
        } else {
            let cap = (2 * len).max(2);
            let size = RefArray::REFS
                .cell_size(cap)
                .ok_or(HeapError::OutOfMemory)?;
            let t = self.alloc_cell(CellKind::RefArray, size)?;
            if len > 0 {
                let src = RefArray::REFS.at(tr, 0);
                let dst = RefArray::REFS.at(t, 0);
                self.arena_mut().copy_within(src, 4 * len, dst);
            }
            self.arena_mut().set(t, RefArray::LEN, len);
            t
        };
        let c = self.alloc_cell(CellKind::Shape, Shape::LAYOUT.min_size)?;
        // Everything is allocated: commit.
        let a = self.arena_mut();
        if !share && count > 0 {
            a.copy_within(
                U32Array::DATA.at(d, 1),
                8 * count,
                U32Array::DATA.at(new_d, 1),
            );
        }
        a.write(U32Array::DATA.at(new_d, 1 + 2 * count), key.id());
        a.write(U32Array::DATA.at(new_d, 2 + 2 * count), flags);
        a.write(U32Array::DATA.at(new_d, 0), count + 1);
        a.set(c, Shape::PARENT, Ref(s));
        a.set(c, Shape::DESCS, Ref(new_d));
        a.set(c, Shape::COUNT, count + 1);
        if lo < len {
            let at = RefArray::REFS.at(new_tr, lo);
            a.copy_within(at, 4 * (len - lo), at + 4);
        }
        a.write(RefArray::REFS.at(new_tr, lo), Ref(c));
        a.set(new_tr, RefArray::LEN, len + 1);
        a.set(s, Shape::TRANSITIONS, Ref(new_tr));
        a.set(0, Roots::SHAPES, shapes + 1);
        Ok(Some(c))
    }

    /// The existing child of `s` for `(key, flags)`, if any.
    pub(super) fn shape_transition(&self, s: u32, key: Atom, flags: u32) -> Option<u32> {
        self.transition_search(s, key, flags).ok()
    }

    /// Binary search of `s`'s children, which are sorted by the key and flags of their
    /// last descriptor: the child, or where it would go.
    fn transition_search(&self, s: u32, key: Atom, flags: u32) -> Result<u32, u32> {
        let count = self.shape_count(s);
        let tr = self.arena().get(s, Shape::TRANSITIONS).0;
        if tr == 0 {
            return Err(0);
        }
        let want = (key.id(), flags);
        let (mut lo, mut hi) = (0, self.arena().get(tr, RefArray::LEN));
        while lo < hi {
            let mid = (lo + hi) / 2;
            let c = self.arena().read::<Ref>(RefArray::REFS.at(tr, mid)).0;
            let (k, f) = self.shape_desc(c, count);
            match (k.id(), f).cmp(&want) {
                core::cmp::Ordering::Equal => return Ok(c),
                core::cmp::Ordering::Less => lo = mid + 1,
                core::cmp::Ordering::Greater => hi = mid,
            }
        }
        Err(lo)
    }

    fn ref_array_capacity(&self, r: u32) -> u32 {
        RefArray::LAYOUT.tail_capacity(self.cell_size(r))
    }
}
