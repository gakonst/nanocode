//! Where a run waits on an open host call (D3054): the position of the activation that
//! waits on it, for a host that shows where a run paused (the docs site's live examples
//! at `debugger;`). Dump builds only, like the pause itself (D3050).

use alloc::string::String;
use alloc::vec::Vec;

use super::Vm;
use crate::bytecode::lookup_line;
use crate::bytecode::template::{bytes_of, debug_info};
use crate::heap::cells::{HostCall, Script};
use crate::heap::{Heap, ObjectClass, Ref};
use crate::interp::run::suspend;
use crate::value::{Obj, Value};

/// A source position: the script's name (as compiled), line and column from 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallSite {
    pub file: String,
    pub line: u32,
    pub col: u32,
}

/// Promise slot 2: the `[capability, onFulfilled, onRejected]` reactions while pending
/// (interp/run/promise.rs).
const REACTIONS: u32 = 2;

impl Vm {
    /// Where the run stopped to wait on open call `call_id`: the `await` or `yield` of the
    /// innermost suspended activation waiting on the call's promise, directly or through
    /// other activations and promise combinators (`Promise.all`, a `then`). `None` when
    /// the call is not open, nothing waits on it, or the function has no positions.
    pub fn call_site(&self, call_id: u32) -> Option<CallSite> {
        let h = &self.heap;
        let cell = crate::interp::run::host_call_cell(h, call_id)?;
        let p = h
            .arena()
            .get::<Value>(cell, HostCall::PROMISE)
            .as_object()?;
        let g = first_waiter(h, p)?;
        let s = suspend::read_activation(h, g);
        let callee = s.frame.first()?.as_object()?;
        if h.object_class(callee) != ObjectClass::Function {
            return None;
        }
        let info = debug_info(h, h.closure_template(callee));
        let (line, col) = lookup_line(bytes_of(h, info.lines), s.pc.saturating_sub(1))?;
        let file = info
            .script
            .get()
            .and_then(|sc| h.arena().get::<Ref>(sc, Script::NAME).get())
            .map(|n| h.str_to_string(crate::value::Str(n)))
            .unwrap_or_default();
        Some(CallSite { file, line, col })
    }
}

/// The first suspended activation found waiting on `p`, breadth first: one awaiting `p`
/// itself before one awaiting a promise that waits on `p`.
fn first_waiter(h: &Heap, p: Obj) -> Option<Obj> {
    let mut seen = alloc::vec![p];
    let mut queue: Vec<Obj> = alloc::vec![p];
    let push = |v: Value, seen: &mut Vec<Obj>, queue: &mut Vec<Obj>| {
        if let Some(o) = v.as_object()
            && h.object_class(o) == ObjectClass::Promise
            && !seen.contains(&o)
        {
            seen.push(o);
            queue.push(o);
        }
    };
    // A capability Record `[promise, resolve, reject]`: its promise.
    let cap_promise = |v: Value| match v.as_object() {
        Some(o) if h.object_class(o) == ObjectClass::Record && h.object_slot_count(o) > 0 => {
            h.object_slot(o, 0)
        }
        _ => Value::UNDEFINED,
    };
    let mut i = 0;
    while i < queue.len() {
        let p = queue[i];
        i += 1;
        let Some(r) = h.object_slot(p, REACTIONS).as_internal() else {
            continue;
        };
        let n = h.value_array_len(r);
        for k in (0..n).step_by(3) {
            let (cap, on_f) = (h.value_array_get(r, k), h.value_array_get(r, k + 1));
            if let Some(f) = on_f.as_object() {
                match h.object_class(f) {
                    // An `await`: the activation itself is the reaction.
                    ObjectClass::Generator => return Some(f),
                    // A combinator's element function: its data record holds the
                    // combined promise's capability.
                    ObjectClass::BuiltinClosure => {
                        let data = h.object_slot(f, 1);
                        push(data, &mut seen, &mut queue);
                        if let Some(rec) = data.as_object()
                            && h.object_class(rec) == ObjectClass::Record
                        {
                            for s in 0..h.object_slot_count(rec) {
                                let v = h.object_slot(rec, s);
                                push(v, &mut seen, &mut queue);
                                push(cap_promise(v), &mut seen, &mut queue);
                            }
                        }
                    }
                    _ => {}
                }
            }
            push(cap, &mut seen, &mut queue);
            push(cap_promise(cap), &mut seen, &mut queue);
        }
    }
    None
}
