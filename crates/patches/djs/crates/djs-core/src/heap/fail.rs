//! The heap's invariant-violation panics, out of line (0098, D016): a formatted panic in
//! an inlined accessor (`cell_kind`, `object_slot`, ...) puts its argument formatting
//! at every call site. These are cold, never inlined, and say the same as before.

use super::{Atom, CellKind, Handle};

#[cold]
#[inline(never)]
pub(crate) fn bad_cell_kind(k: u8, off: u32) -> ! {
    panic!("invalid cell kind {k} at {off}")
}

#[cold]
#[inline(never)]
pub(crate) fn not_a_string(k: CellKind, off: u32) -> ! {
    panic!(
        "{} at {off}",
        crate::detail::detail("not a string cell", &k)
    )
}

#[cold]
#[inline(never)]
pub(crate) fn bad_object_class(c: u32) -> ! {
    panic!("invalid object class {c}")
}

#[cold]
#[inline(never)]
pub(crate) fn slot_out_of_range(i: u32) -> ! {
    panic!("slot {i} out of range")
}

#[cold]
#[inline(never)]
pub(crate) fn invalid_atom(a: Atom) -> ! {
    panic!("{}", crate::detail::detail("invalid atom", &a))
}

#[cold]
#[inline(never)]
pub(crate) fn invalid_handle(h: Handle, dropped: bool) -> ! {
    if dropped {
        panic!("{}", crate::detail::detail("dropped handle", &h))
    }
    panic!("{}", crate::detail::detail("invalid handle", &h))
}

#[cold]
#[inline(never)]
pub(crate) fn bad_hash_index(index: u32, len: u32) -> ! {
    if len.is_power_of_two() {
        panic!("hash index {index} is full")
    }
    panic!("hash index length {len}")
}

#[cold]
#[inline(never)]
pub(crate) fn bad_cell_size(size: u32, offset: u32) -> ! {
    panic!("bad cell size {size} at {offset}")
}
