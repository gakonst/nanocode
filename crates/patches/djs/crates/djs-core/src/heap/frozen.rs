//! The frozen-builtins mode (D022, D2304): with `Roots::FLAGS` bit
//! [`ROOT_FROZEN_BUILTINS`](super::cells::ROOT_FROZEN_BUILTINS) set, every object of the
//! builtin prefix (below `Roots::BASE_END`) behaves as frozen without being frozen: it is
//! not extensible, its own properties read as non-writable and non-configurable, and the
//! property operations refuse to change it. Nothing in the objects changes, so one base
//! image serves both modes. Internal state the engine keeps in those cells (header bits,
//! shape counts, realm slots) is written as before: only the JS-visible property
//! operations of `object.rs` consult [`Heap::frozen`].
//!
//! No object is exempt: the per-VM objects (the global object, the global lexical record,
//! `Symbol.for`'s registry) live above the prefix (D2063). With the mode on, the prefix
//! write barrier (D2065) is armed in debug builds.

use super::Heap;
use super::cells::{self, Roots};
use crate::value::Obj;

impl Heap {
    /// Whether the frozen-builtins mode is on (part of the image).
    pub fn frozen_builtins(&self) -> bool {
        self.arena.len() >= Roots::LAYOUT.min_size
            && self.arena.get(0, Roots::FLAGS) & cells::ROOT_FROZEN_BUILTINS != 0
    }

    /// Turns the frozen-builtins mode on or off. It has no effect without a builtin prefix
    /// (`Roots::BASE_END` 0).
    pub fn set_frozen_builtins(&mut self, on: bool) {
        let f = self.arena.get(0, Roots::FLAGS);
        let f = if on {
            f | cells::ROOT_FROZEN_BUILTINS
        } else {
            f & !cells::ROOT_FROZEN_BUILTINS
        };
        self.arena.set(0, Roots::FLAGS, f);
        self.refresh_frozen();
    }

    /// Recomputes the cached boundary after anything that can move `Roots::BASE_END` or
    /// the flag: loading, collections, truncation.
    pub(crate) fn refresh_frozen(&mut self) {
        self.frozen_end = if self.frozen_builtins() {
            self.arena.get(0, Roots::BASE_END)
        } else {
            0
        };
        self.refresh_barrier();
    }

    /// Whether `o` is a frozen builtin: in the prefix with the mode on. The per-VM
    /// objects (the global object, the global lexical record, the `Symbol.for` registry)
    /// are above the prefix (D2063), so there are no exemptions.
    #[inline(always)]
    pub fn frozen(&self, o: Obj) -> bool {
        o.0 < self.frozen_end
    }
}
