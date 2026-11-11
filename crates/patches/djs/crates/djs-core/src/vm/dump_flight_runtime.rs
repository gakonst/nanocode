//! Objects the plain dump runtime made, read back as the helper calls that made them
//! (0209). A run that loaded a plain dump holds what `$djs.fromSync` and
//! `$djs.closeThrow` returned (a `for await` over a sync iterator, D2872). Dumped as they
//! are, they would be written with the runtime's code as closures of their own scopes
//! (~1.6 KB per re-dump, D2903's cost). The exporter recognizes them by their text, which
//! is the runtime's (one table, dump_runtime.rs), and writes the call again:
//!
//! - `fromSync`'s iterator `{ next, return, throw }` is `$djs.fromSync(it, next)`, read
//!   like the engine's async-from-sync Record ([`Dumper::is_from_sync`]);
//! - `closeThrow`'s handler is `$djs.closeThrow(it)`, like the engine's
//!   `FROM_SYNC_CLOSE` closure;
//! - a step of `fromSync` in flight (`w.then((v) => res({ value: v, done }), (e) => {
//!   q(); rej(e); })` resolving the step's own promise) is that promise made by a `then`
//!   on `w` with the engine's handlers again: `(v) => ({ value: v, done })` and
//!   `$djs.closeThrow(it)` (or none, where the step passes rejections on as they are).
//!   The derived promise the runtime's `then` made is dropped: nothing else holds it.

use super::*;
use crate::interp::run::{builtin_closure_kind, promise};

/// The texts of the runtime's closures, as the runtime defines them (a test checks them
/// against dump_runtime.rs).
pub(super) const FROM_SYNC_NEXT: &str = "(...a) => step(() => Reflect.apply(next, it, a), true)";
pub(super) const STEP_FULFILLED: &str = "(v) => res({ value: v, done })";
pub(super) const STEP_CLOSE: &str = "(e) => { q(); rej(e); }";
/// `fromAsync`'s handlers of its await (dump_runtime.rs).
pub(super) const FROM_ASYNC_OK: &str = "(v) => { if (phase === 1) result(v); else if (phase === 2) store(v); else if (phase === 3) reject(error); else value(v); }";
pub(super) const FROM_ASYNC_BAD: &str =
    "(e) => { if (phase === 2) close(e); else if (phase === 3) reject(error); else reject(e); }";
pub(super) const CLOSE_THROW: &str = "(e) => { try { const m = it.return; if (m != null) Reflect.apply(m, it, []); } catch {} throw e; }";

/// A step of `fromSync` in flight, read from the reaction on the awaited value.
pub(super) struct Step {
    /// The step's own promise.
    pub promise: Value,
    pub done: Value,
    /// The sync iterator, when a rejection closes it.
    pub close: Option<Value>,
}

impl Dumper<'_> {
    /// Whether `v` is a closure of the runtime whose text is `text`.
    fn runtime_closure(&self, v: Value, text: &str) -> Option<Obj> {
        let o = v.as_object()?;
        (self.h.object_class(o) == ObjectClass::Function
            && self.sources.fn_text(self.h, o).as_deref() == Some(text))
        .then_some(o)
    }

    /// `fromSync`'s iterator: its sync iterator and `next`.
    pub(super) fn runtime_from_sync(&self, o: Obj) -> Option<(Value, Value)> {
        let h = self.h;
        if h.object_class(o) != ObjectClass::Ordinary
            || !h.is_extensible(o)
            || h.get_prototype_of(o).map(Value::object)
                != Some(intrinsic(h, Intrinsic::ObjectProto))
        {
            return None;
        }
        let keys = h.own_keys(o);
        let names = ["next", "return", "throw"];
        if keys.len() != names.len() {
            return None;
        }
        let mut fns = [Value::UNDEFINED; 3];
        for (i, k) in keys.into_iter().enumerate() {
            match h.get_own_property(o, k) {
                Some(Property::Data { value, flags })
                    if flags == PropFlags::ALL
                        && h.atom_symbol(k).is_none()
                        && atom_text(h, k) == names[i] =>
                {
                    fns[i] = value;
                }
                _ => return None,
            }
        }
        let next = self.runtime_closure(fns[0], FROM_SYNC_NEXT)?;
        let it = captured_value(h, next, "it")?;
        let step = captured_value(h, next, "step")?;
        // `return` and `throw` are the same call's.
        for f in &fns[1..] {
            let f = f.as_object()?;
            if h.object_class(f) != ObjectClass::Function
                || captured_value(h, f, "it") != Some(it)
                || captured_value(h, f, "step") != Some(step)
            {
                return None;
            }
        }
        Some((it, captured_value(h, next, "next")?))
    }

    /// A reaction `(f, r)` that is the runtime's `fromAsync` waiting: its state in the
    /// slots of the engine's Record (the promise first, which its `resolve` settles).
    pub(super) fn runtime_from_async(&self, f: Value, r: Value) -> Option<[Value; 11]> {
        let h = self.h;
        let ok = self.runtime_closure(f, FROM_ASYNC_OK)?;
        let bad = self.runtime_closure(r, FROM_ASYNC_BAD)?;
        let phase = captured_value(h, ok, "phase")?;
        if captured_value(h, bad, "phase") != Some(phase) {
            return None;
        }
        // The helper's variables, each read from a closure of its that captures it.
        let fun = |o: Obj, name: &str| captured_value(h, o, name)?.as_object();
        let result = fun(ok, "result")?;
        let store = fun(ok, "store")?;
        let step = fun(store, "step")?;
        let value = fun(ok, "value")?;
        let element = fun(value, "element")?;
        let close = fun(store, "close")?;
        let var = |name: &str| {
            [ok, result, store, step, value, element, close]
                .iter()
                .find_map(|&o| captured_value(h, o, name))
        };
        let promise = self.resolving(var("resolve")?, 0)?;
        Some([
            promise,
            var("mapfn")?,
            var("thisArg")?,
            var("it")?,
            var("next")?,
            var("A")?,
            var("k")?,
            phase,
            var("items")?,
            var("len")?,
            var("error")?,
        ])
    }

    /// `closeThrow`'s handler: its sync iterator.
    pub(super) fn runtime_close_throw(&self, v: Value) -> Option<Value> {
        let f = self.runtime_closure(v, CLOSE_THROW)?;
        captured_value(self.h, f, "it")
    }

    /// A reaction `(f, r)` that is a step of `fromSync` in flight.
    pub(super) fn runtime_step(&self, f: Value, r: Value) -> Option<Step> {
        let h = self.h;
        let fo = self.runtime_closure(f, STEP_FULFILLED)?;
        let promise = captured_value(h, fo, "res").and_then(|res| self.resolving(res, 0))?;
        let done = captured_value(h, fo, "done").filter(|d| d.as_bool().is_some())?;
        let close = if let Some(ro) = self.runtime_closure(r, STEP_CLOSE) {
            let rej = captured_value(h, ro, "rej")?;
            if self.resolving(rej, 1) != Some(promise) {
                return None;
            }
            let q = captured_value(h, ro, "q")?.as_object()?;
            Some(captured_value(h, q, "it")?)
        } else if self.resolving(r, 1) == Some(promise) {
            None
        } else {
            return None;
        };
        Some(Step {
            promise,
            done,
            close,
        })
    }

    /// The pending promise that `v`, a resolve (`kind` 0) or reject (1) function of
    /// `new Promise`, settles, if it has not been resolved yet.
    fn resolving(&self, v: Value, kind: usize) -> Option<Value> {
        let h = self.h;
        let o = v.as_object()?;
        if h.object_class(o) != ObjectClass::BuiltinClosure
            || builtin_closure_kind(h, o) != Some(kind)
        {
            return None;
        }
        let p = h.object_slot(o, 1).as_object()?;
        (h.object_class(p) == ObjectClass::Promise
            && h.object_slot(p, 0).as_int() == Some(promise::PENDING)
            && promise::flags(h, p) & promise::FLAG_RESOLVED == 0)
            .then_some(Value::object(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The texts are the runtime's.
    #[test]
    fn texts_are_the_runtimes() {
        let helper = |name: &str| {
            crate::vm::dump_runtime::HELPERS
                .iter()
                .find(|h| h.0 == name)
                .map(|h| h.1)
                .unwrap()
        };
        for t in [FROM_SYNC_NEXT, STEP_FULFILLED, STEP_CLOSE] {
            assert!(helper("fromSync").contains(t), "{t}");
        }
        assert!(helper("closeThrow").contains(CLOSE_THROW));
        for t in [FROM_ASYNC_OK, FROM_ASYNC_BAD] {
            assert!(helper("fromAsync").contains(t), "{t}");
        }
    }
}
