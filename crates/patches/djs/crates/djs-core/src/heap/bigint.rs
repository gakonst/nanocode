//! BigInt cells (0046, D1460): the heap form of [`crate::bigint::BigInt`].
//!
//! A BigInt value is always a reference to an immutable `BigInt` cell; there is no
//! inline small form. JS equality compares the values ([`Heap::bigint_eq`]), never the
//! cell offsets.

use core::cmp::Ordering;

use super::cells::{BigInt as Cell, flags};
use super::{CellKind, Heap, HeapError};
use crate::bigint::BigInt;
use crate::value::Value;

impl Heap {
    /// A new BigInt value.
    pub fn alloc_bigint(&mut self, b: &BigInt) -> Result<Value, HeapError> {
        debug_assert!(b.mag.last() != Some(&0) && !(b.neg && b.mag.is_empty()));
        let len = b.mag.len() as u32;
        let size = Cell::LIMBS.cell_size(len).ok_or(HeapError::OutOfMemory)?;
        let off = self.alloc_cell_from_iter(
            CellKind::BigInt,
            size,
            Cell::LIMBS.offset,
            len * 4,
            b.mag.iter().flat_map(|l| l.to_le_bytes()),
        )?;
        self.arena_mut().set(off, Cell::LEN, len);
        if b.neg {
            self.set_cell_flags(off, flags::BIGINT_NEG);
        }
        Ok(Value::bigint(off))
    }

    /// A BigInt value from an `i64`.
    pub fn alloc_bigint_i64(&mut self, v: i64) -> Result<Value, HeapError> {
        self.alloc_bigint(&BigInt::from_i64(v))
    }

    fn bigint_cell(v: Value) -> u32 {
        v.as_bigint().expect("a BigInt value")
    }

    /// Whether a BigInt is negative.
    pub fn bigint_neg(&self, v: Value) -> bool {
        self.cell_flags(Heap::bigint_cell(v)) & flags::BIGINT_NEG != 0
    }

    /// The limb count of a BigInt (0 for zero).
    pub fn bigint_len(&self, v: Value) -> u32 {
        self.arena().get(Heap::bigint_cell(v), Cell::LEN)
    }

    fn bigint_limbs(&self, v: Value) -> impl Iterator<Item = u32> + '_ {
        let c = Heap::bigint_cell(v);
        let len = self.arena().get(c, Cell::LEN);
        let bytes = self.arena().slice(Cell::LIMBS.at(c, 0), len * 4);
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&b| u32::from_le_bytes(b))
    }

    /// A copy of a BigInt's value.
    pub fn bigint(&self, v: Value) -> BigInt {
        BigInt {
            neg: self.bigint_neg(v),
            mag: self.bigint_limbs(v).collect(),
        }
    }

    /// Whether a BigInt is zero.
    pub fn bigint_is_zero(&self, v: Value) -> bool {
        self.bigint_len(v) == 0
    }

    /// Whether two BigInts are equal.
    pub fn bigint_eq(&self, a: Value, b: Value) -> bool {
        a == b
            || (self.bigint_neg(a) == self.bigint_neg(b)
                && self.bigint_len(a) == self.bigint_len(b)
                && self.bigint_limbs(a).eq(self.bigint_limbs(b)))
    }

    /// Compares two BigInts.
    pub fn bigint_cmp(&self, a: Value, b: Value) -> Ordering {
        self.bigint(a).cmp(&self.bigint(b))
    }

    /// A hash of a BigInt's value.
    pub fn bigint_hash(&self, v: Value) -> u64 {
        BigInt::hash_limbs(self.bigint_neg(v), self.bigint_limbs(v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_equality() {
        let mut h = Heap::new().unwrap();
        let x = BigInt::from_i64(-0x1234_5678_9abc);
        let a = h.alloc_bigint(&x).unwrap();
        let b = h.alloc_bigint(&x).unwrap();
        let z = h.alloc_bigint(&BigInt::zero()).unwrap();
        assert_ne!(a, b);
        assert_eq!(h.bigint(a), x);
        assert!(h.bigint_eq(a, b));
        assert!(!h.bigint_eq(a, z));
        assert!(h.bigint_is_zero(z));
        assert!(h.bigint_neg(a));
        assert_eq!(h.bigint_hash(a), h.bigint_hash(b));
        assert_eq!(h.bigint_cmp(a, z), Ordering::Less);
        assert_eq!(h.cell_size(z.as_bigint().unwrap()), 16);
        h.verify().unwrap();
    }
}
