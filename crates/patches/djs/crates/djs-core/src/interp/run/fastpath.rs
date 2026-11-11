//! Calls of `@fast` builtins (D1850): the leaf fast path of `builtins::fast` runs in
//! place of the call when it accepts the arguments.

#[cfg(djs_prelude)]
use alloc::string::String;

use super::Exec;
use crate::builtins::fast::{FAST, MAX_ARGS};
use crate::bytecode::template::func_flags;
use crate::bytecode::{OPS, Op};
#[cfg(djs_prelude)]
use crate::heap::Atom;
use crate::heap::cells::FuncTemplate;
use crate::heap::{HeapError, ObjectClass};
#[cfg(djs_prelude)]
use crate::interp::ErrorKind;
use crate::interp::conv::{Fault, FaultKind};
use crate::interp::frame::Continuation;
use crate::value::Value;

/// Fuel per fast call on top of the call instruction's: a builtin's call runs a few
/// instructions at least.
const FAST_FUEL: i64 = 2;

impl Exec<'_> {
    /// The `call` instruction at `pc` when its callee is a `@fast` builtin whose fast path
    /// accepts the arguments: true when it completed. Declining leaves everything
    /// unchanged, and `step` makes the full call.
    #[inline]
    pub(super) fn fast_leaf_call(&mut self, op: u8) -> bool {
        // `call argc` or a short form (`call2`, D2443).
        let (argc, size) = if op == super::fast::CALL {
            (self.imm_u16(1), Op::Call.size() as u32)
        } else {
            ((op - super::fast::CALL0) as u32, Op::Call0.size() as u32)
        };
        let base = self.sp - 2 - argc;
        let Some(o) = self.get(base).as_object() else {
            return false;
        };
        if !self.is_class(o, ObjectClass::Function) {
            return false;
        }
        let t = self.template_of(o);
        if u32::from(self.heap.arena().get::<u16>(t, FuncTemplate::FLAGS))
            & self.slow_call_flags
            & func_flags::FAST
            == 0
        {
            return false;
        }
        // (`call0`..`call3` cost as much as `call`: the table's check.)
        let info = &OPS[Op::Call as usize];
        let next = self.pc + size;
        // `arr.push(x)`, the commonest @fast call, straight to the heap's append (0231):
        // what `try_fast_call` and its `array_push` entry do, without the generic steps.
        if argc == 1
            && self.heap.arena().get(t, FuncTemplate::FAST) as usize
                == crate::builtins::fast::ARRAY_PUSH
            && let Some(a) = self.get(base + 1).as_object()
            && self.is_class(a, ObjectClass::Array)
        {
            match self.heap.array_append_fast(a, self.get(base + 2)) {
                Ok(Some(n)) => {
                    self.sp = base;
                    self.push(crate::interp::conv::number(f64::from(n)));
                    self.pc = next;
                    self.fuel -= info.cost.fuel() as i64 + FAST_FUEL;
                    return true;
                }
                Ok(None) => {}
                Err(e) => {
                    if matches!(e, HeapError::HostOutOfMemory) {
                        self.host_out_of_memory();
                    }
                    return false;
                }
            }
        }
        match self.try_fast_call(t, base, argc, Continuation::PushResult, next) {
            Ok(true) => {
                self.fuel -= info.cost.fuel() as i64;
                true
            }
            Ok(false) => false,
            Err(f) => {
                // The host refused memory: never silently retried (D1690); the run
                // stops at the next instruction boundary. The cap (OutOfMemory) is
                // deterministic, so `step`'s retry throws it.
                if matches!(&*f.0, FaultKind::Heap(HeapError::HostOutOfMemory)) {
                    self.host_out_of_memory();
                }
                false
            }
        }
    }

    #[cfg(djs_prelude)]
    /// `%MarkFast(f)`: gives the builtin `f` the fast path named like it. A name without
    /// an entry fails the prelude (at base-image build time).
    pub(super) fn mark_fast(&mut self, f: Value) -> Result<(), Fault> {
        let bad = |msg: String| Err(Fault::error(ErrorKind::InternalError, msg));
        let Some(o) = f
            .as_object()
            .filter(|&o| self.is_class(o, ObjectClass::Function))
        else {
            return bad(String::from("%MarkFast: not a function"));
        };
        let t = self.template_of(o);
        let name = self
            .heap
            .atom_name(Atom::from_id(self.heap.arena().get(t, FuncTemplate::NAME)));
        let Some(i) = crate::builtins::fast::index(&name) else {
            return bad(crate::text::cat!(
                "%MarkFast: no fast path for ",
                name,
                " (builtins/fast.rs)"
            ));
        };
        let a = self.heap.arena_mut();
        let flags = u32::from(a.get::<u16>(t, FuncTemplate::FLAGS));
        a.set(t, FuncTemplate::FLAGS, (flags | func_flags::FAST) as u16);
        // The table is far below 2^8 entries (a u8 field, D2448; a test checks).
        a.set(t, FuncTemplate::FAST, i as u8);
        Ok(())
    }

    /// Tries the fast path of the `@fast` builtin template `t` for the call at `base`
    /// (`f this a1 .. an`). On success the call is complete: the result went where `cont`
    /// says and `pc` is `ret_pc`. No collection happens here (a native caller keeps Rust
    /// locals across an inline call); the allocation sets `gc_pending` for the next
    /// safepoint as usual.
    #[inline(never)]
    /// `new F(a1 .. an)` (`F a1 .. an` at `base`) with new.target F of a constructor with
    /// a `new` entry (0236): true when it completed, the object at `base` and pc at
    /// `next`. Declining leaves everything unchanged, as for a call.
    pub(super) fn try_fast_new(
        &mut self,
        t: u32,
        base: u32,
        argc: u32,
        next: u32,
    ) -> Result<bool, Fault> {
        let idx = self.heap.arena().get(t, FuncTemplate::FAST) as usize;
        let Some(new) = FAST.get(idx).and_then(|f| f.new) else {
            return Ok(false);
        };
        if argc as usize > MAX_ARGS {
            return Ok(false);
        }
        let mut args = [Value::UNDEFINED; MAX_ARGS];
        for (i, a) in args.iter_mut().enumerate().take(argc as usize) {
            *a = self.get(base + 1 + i as u32);
        }
        let v = match new(self.heap, &args[..argc as usize]) {
            Ok(Some(v)) => v,
            Ok(None) => return Ok(false),
            // As for a call: a host refusal stops the run, anything else is the builtin's.
            Err(f) if matches!(&*f.0, FaultKind::Heap(HeapError::HostOutOfMemory)) => {
                return Err(f);
            }
            Err(_) => return Ok(false),
        };
        self.fuel -= FAST_FUEL;
        self.sp = base;
        self.push(v);
        self.pc = next;
        Ok(true)
    }

    pub(super) fn try_fast_call(
        &mut self,
        t: u32,
        base: u32,
        argc: u32,
        cont: Continuation,
        ret_pc: u32,
    ) -> Result<bool, Fault> {
        if argc as usize > MAX_ARGS {
            return Ok(false);
        }
        let push = match cont {
            Continuation::PushResult => true,
            Continuation::DiscardResult | Continuation::Replace(_) => false,
            _ => return Ok(false),
        };
        let idx = self.heap.arena().get(t, FuncTemplate::FAST) as usize;
        let Some(ff) = FAST.get(idx) else {
            return Ok(false);
        };
        let mut args = [Value::UNDEFINED; MAX_ARGS];
        for (i, a) in args.iter_mut().enumerate().take(argc as usize) {
            *a = self.get(base + 2 + i as u32);
        }
        let this = self.get(base + 1);
        let v = match ff.call(self.heap, this, &args[..argc as usize]) {
            Ok(Some(v)) => v,
            Ok(None) => return Ok(false),
            // A host refusal stops the run (D1690). At the heap cap the entry declines
            // (D1910): the builtin's own allocations then collect and retry (D1700,
            // D1900). Entries allocate before they change anything, or only make stores
            // the builtin repeats. Any other error is the builtin's to throw, with its own
            // frame in the stack trace.
            Err(f) if matches!(&*f.0, FaultKind::Heap(HeapError::HostOutOfMemory)) => {
                return Err(f);
            }
            Err(_) => return Ok(false),
        };
        // Like the string leaf intrinsics the builtins call, no charge per code unit.
        self.fuel -= FAST_FUEL;
        self.sp = base;
        match cont {
            Continuation::Replace(slot) => self.set(slot, v),
            _ if push => self.push(v),
            _ => {}
        }
        self.pc = ret_pc;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use crate::builtins::fast::FAST;

    /// Every entry of the table is marked by the prelude (`@fast` in builtins/*.ts):
    /// `%MarkFast` already fails the prelude for a tag without an entry, this catches an
    /// entry without a tag (dead code).
    #[test]
    fn every_fast_entry_is_marked() {
        let prelude = include_str!("../../../../../builtins/gen/prelude.js");
        for f in FAST {
            assert!(
                prelude.contains(&alloc::format!("%MarkFast({});", f.name)),
                "{} has a fast path but no @fast tag",
                f.name
            );
        }
        let marked = prelude.matches("%MarkFast(").count();
        let mut names: alloc::vec::Vec<&str> = FAST.iter().map(|f| f.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(
            marked,
            names.len(),
            "names in FAST are unique and all marked"
        );
    }
}
