//! Proxies in the interpreter (0045, D1450). The traps and their invariant checks are
//! builtins (builtins/proxy.ts, D1085): when a property operation or an internal-method
//! intrinsic meets a proxy, it calls the matching `%Proxy...` builtin instead, with a
//! continuation that completes the operation. The checks are class tests on the slow
//! paths only: a proxy has no own properties and no elements, so every fast path misses
//! on it first.

use super::intrinsics::id;
use super::*;
use crate::heap::proxy::{PROXY_CALLABLE, PROXY_CONSTRUCTOR};

pub(super) const PROXY_GET: u16 = id("ProxyGet");
pub(super) const PROXY_SET: u16 = id("ProxySet");
pub(super) const PROXY_SET_OR_THROW: u16 = id("ProxySetOrThrow");
pub(super) const PROXY_SET_ON_RECEIVER: u16 = id("ProxySetOnReceiver");
pub(super) const PROXY_HAS: u16 = id("ProxyHas");
pub(super) const PROXY_DELETE_OR_THROW: u16 = id("ProxyDeletePropertyOrThrow");
const FOR_IN_STEP: u16 = id("ForInStep");
pub(super) const PROXY_CONSTRUCT: u16 = id("ProxyConstruct");
pub(super) const ORDINARY_CONSTRUCT_SLOW: u16 = id("OrdinaryConstructSlow");
const PROXY_CALL: u16 = id("ProxyCall");
const IS_PROXY: u16 = id("IsProxy");
const NEW_PROXY: u16 = id("NewProxy");
const PROXY_TARGET: u16 = id("ProxyTarget");
const PROXY_HANDLER: u16 = id("ProxyHandler");
const PROXY_REVOKE: u16 = id("ProxyRevoke");
pub(super) const CONSTRUCT_WITH_THIS: u16 = id("ConstructWithThis");

/// The internal-method intrinsics whose first argument may be a proxy, and the builtin
/// of builtins/proxy.ts with the same parameters that runs the trap. (Get, Set,
/// HasProperty and the WithReceiver forms also meet proxies on the prototype chain; they
/// handle that themselves.)
const REDIRECTS: [(u16, u16); 16] = [
    (
        id("DefineDataPropertyOrThrow"),
        id("ProxyDefineDataPropertyOrThrow"),
    ),
    (
        id("DefineOwnPropertyFromRecord"),
        id("ProxyDefineOwnPropertyFromRecord"),
    ),
    (
        id("GetOwnPropertyDescriptorObject"),
        id("ProxyGetOwnPropertyDescriptorObject"),
    ),
    (id("HasOwnProperty"), id("ProxyHasOwnProperty")),
    (
        id("OwnPropertyAttributes"),
        id("ProxyOwnPropertyAttributes"),
    ),
    (id("OwnPropertyKeys"), id("ProxyOwnPropertyKeys")),
    (id("GetPrototypeOf"), id("ProxyGetPrototypeOf")),
    (id("SetPrototypeOf"), id("ProxySetPrototypeOf")),
    (id("PreventExtensions"), id("ProxyPreventExtensions")),
    (id("IsExtensible"), id("ProxyIsExtensible")),
    (id("SetIntegrityLevel"), id("ProxySetIntegrityLevel")),
    (id("TestIntegrityLevel"), id("ProxyTestIntegrityLevel")),
    (id("DeleteProperty"), id("ProxyDeleteProperty")),
    (
        id("DeletePropertyOrThrow"),
        id("ProxyDeletePropertyOrThrow"),
    ),
    (
        id("CreateDataPropertyOrThrow"),
        id("ProxyCreateDataPropertyOrThrow"),
    ),
    (id("CreateDataProperty"), id("ProxyCreateDataProperty")),
];

/// The smallest and largest intrinsic id in [`REDIRECTS`], for a cheap range test.
const REDIRECT_RANGE: (u16, u16) = {
    let (mut lo, mut hi) = (u16::MAX, 0);
    let mut i = 0;
    while i < REDIRECTS.len() {
        let id = REDIRECTS[i].0;
        if id < lo {
            lo = id;
        }
        if id > hi {
            hi = id;
        }
        i += 1;
    }
    (lo, hi)
};

impl Exec<'_> {
    /// Calls the installed builtin `id` with `args`, pushed above `sp`; the result goes
    /// where `cont` says and execution resumes at `self.pc`.
    pub(super) fn call_prelude(&mut self, id: u16, args: &[Value], cont: Continuation) -> Step {
        let f = realm::installed_intrinsic(self.heap, id);
        if f.is_undefined() {
            return type_error("proxies need the prelude");
        }
        let base = self.sp;
        self.ensure(base + 2 + args.len() as u32)?;
        self.push(f);
        self.push(Value::UNDEFINED);
        for &a in args {
            self.push(a);
        }
        let pc = self.pc;
        self.call(base, args.len() as u32, cont, pc)
            .inspect_err(|_| self.sp = base)
    }

    /// Calls the installed builtin `id` with the intrinsic arguments `a1 .. an` at
    /// `base` (they become `f undefined a1 .. an`); its result is the intrinsic's.
    pub(super) fn call_installed(&mut self, id: u16, base: u32, argc: u32, next: u32) -> Step {
        let f = realm::installed_intrinsic(self.heap, id);
        if f.is_undefined() {
            let name = crate::bytecode::intrinsics::intrinsic_name(id);
            return Err(Fault::error(
                ErrorKind::InternalError,
                crate::text::cat!("%", name, " is not installed"),
            ));
        }
        self.ensure(self.sp + 2)?;
        for i in (0..argc).rev() {
            let v = self.get(base + i);
            self.set(base + 2 + i, v);
        }
        self.set(base, f);
        self.set(base + 1, Value::UNDEFINED);
        self.sp = base + 2 + argc;
        self.call(base, argc, Continuation::PushResult, next)
    }

    /// An internal-method intrinsic whose first argument is a proxy: its trap builtin.
    /// `None` for anything else (the intrinsic runs as usual).
    #[inline]
    pub(super) fn proxy_intrinsic(
        &mut self,
        id: u16,
        base: u32,
        argc: u32,
        next: u32,
    ) -> Option<Step> {
        if id < REDIRECT_RANGE.0 || id > REDIRECT_RANGE.1 || argc == 0 {
            return None;
        }
        let o = self.get(base).as_object()?;
        if !self.heap.is_proxy(o) {
            return None;
        }
        let &(_, target) = REDIRECTS.iter().find(|r| r.0 == id)?;
        Some(self.call_installed(target, base, argc, next))
    }

    /// [[Call]] of the callable proxy at `stack[base]`: `p this a1 .. an` becomes
    /// `%ProxyCall undefined p this [a1 .. an]`.
    pub(super) fn proxy_call(
        &mut self,
        base: u32,
        argc: u32,
        cont: Continuation,
        ret_pc: u32,
    ) -> Step {
        let p = self.get(base);
        let this = self.get(base + 1);
        let args = self.args_array(base + 2, argc)?;
        let f = realm::installed_intrinsic(self.heap, PROXY_CALL);
        if f.is_undefined() {
            return type_error("proxies need the prelude");
        }
        self.ensure(base + 5)?;
        for (i, v) in [f, Value::UNDEFINED, p, this, args].into_iter().enumerate() {
            self.set(base + i as u32, v);
        }
        self.sp = base + 5;
        self.call(base, 3, cont, ret_pc)
    }

    /// `new` of `stack[base]` (`f a1 .. an`, new.target `new_target`) by the builtin
    /// `id(f, [a1 .. an], newTarget)`: %ProxyConstruct, or %OrdinaryConstructSlow when
    /// reading `prototype` runs user code.
    pub(super) fn construct_through(
        &mut self,
        id: u16,
        base: u32,
        argc: u32,
        new_target: Value,
        next: u32,
    ) -> Step {
        let f = self.get(base);
        let args = self.args_array(base + 1, argc)?;
        let g = realm::installed_intrinsic(self.heap, id);
        if g.is_undefined() {
            return type_error("proxies need the prelude");
        }
        self.ensure(base + 5)?;
        for (i, v) in [g, Value::UNDEFINED, f, args, new_target]
            .into_iter()
            .enumerate()
        {
            self.set(base + i as u32, v);
        }
        self.sp = base + 5;
        self.call(base, 3, Continuation::PushResult, next)
    }

    /// `%ConstructWithThis(f, this, args, newTarget)`: [[Construct]] of the ordinary
    /// function f with the object `this` made by the caller.
    pub(super) fn construct_with_this(&mut self, base: u32, next: u32) -> Step {
        let (f, this, args, nt) = (
            self.get(base),
            self.get(base + 1),
            self.get(base + 2),
            self.get(base + 3),
        );
        let Some(args) = args.as_object() else {
            return super::intrinsics::bad_argument(
                const { crate::bytecode::intrinsics::const_id("ConstructWithThis") },
            );
        };
        let n = self.heap.array_length(args);
        self.ensure(base + 2 + n)?;
        self.set(base, f);
        self.set(base + 1, this);
        for i in 0..n {
            let v = self
                .heap
                .get_element_fast(args, i)
                .unwrap_or(Value::UNDEFINED);
            self.set(base + 2 + i, v);
        }
        self.sp = base + 2 + n;
        self.construct_call(base, n, Some(nt), next)
    }

    /// The for-in iterator of an object with a proxy on its prototype chain: the Record
    /// that %ForInStep (builtins/proxy.ts) walks.
    pub(super) fn for_in_proxy_start(&mut self, o: Obj) -> Result<Value, Fault> {
        let proto = intrinsic_obj(self.heap, Intrinsic::ArrayProto);
        let visited = self.heap.new_array(Some(proto), 0)?;
        let rec = self.heap.alloc_object(ObjectClass::Record, None, 6)?;
        let slots = [
            Value::object(o),
            Value::UNDEFINED,
            Value::int(0),
            Value::object(visited),
            Value::UNDEFINED,
            Value::FALSE,
        ];
        for (i, v) in slots.into_iter().enumerate() {
            self.heap.set_object_slot(rec, i as u32, v);
        }
        Ok(Value::object(rec))
    }

    /// `for_in_next` on such a Record: the step's result if %ForInStep left one, else
    /// a call of %ForInStep, after which this instruction runs again.
    pub(super) fn for_in_proxy_next(&mut self, rec: Obj, next: u32) -> Step {
        if self.heap.object_slot(rec, 5) == Value::TRUE {
            let key = self.heap.object_slot(rec, 4);
            self.heap.set_object_slot(rec, 5, Value::FALSE);
            self.heap.set_object_slot(rec, 4, Value::UNDEFINED);
            self.pc = next;
            if key.is_null() {
                self.push(Value::UNDEFINED);
                self.push(Value::TRUE);
            } else {
                self.push(key);
                self.push(Value::FALSE);
            }
            return Ok(None);
        }
        self.call_prelude(
            FOR_IN_STEP,
            &[Value::object(rec)],
            Continuation::DiscardResult,
        )
    }

    /// A new Array of the `n` stack values from `from`.
    fn args_array(&mut self, from: u32, n: u32) -> Result<Value, Fault> {
        let vals: alloc::vec::Vec<Value> = (0..n).map(|i| self.get(from + i)).collect();
        let proto = intrinsic_obj(self.heap, Intrinsic::ArrayProto);
        Ok(Value::object(
            self.heap.new_array_from_values(Some(proto), &vals)?,
        ))
    }

    /// IsConstructor for a proxy.
    pub(super) fn proxy_is_constructor(&self, p: Obj) -> bool {
        self.heap.proxy_flags(p) & PROXY_CONSTRUCTOR != 0
    }

    /// The proxy leaves (IsProxy, NewProxy, ProxyTarget, ProxyHandler, ProxyRevoke).
    pub(super) fn proxy_leaf(
        &mut self,
        id: u16,
        a0: Value,
        a1: Value,
    ) -> Option<Result<Value, Fault>> {
        let proxy = || a0.as_object().filter(|&p| self.heap.is_proxy(p));
        Some(match id {
            IS_PROXY => Ok(Value::bool(proxy().is_some())),
            NEW_PROXY => (|| {
                let (Some(t), Some(h)) = (a0.as_object(), a1.as_object()) else {
                    return super::intrinsics::bad_argument(
                        const { crate::bytecode::intrinsics::const_id("NewProxy") },
                    );
                };
                let mut flags = 0;
                if self.heap.is_callable(t) {
                    flags |= PROXY_CALLABLE;
                    if self.is_constructor(t) {
                        flags |= PROXY_CONSTRUCTOR;
                    }
                }
                Ok(Value::object(self.heap.new_proxy(t, h, flags)?))
            })(),
            PROXY_TARGET | PROXY_HANDLER => match proxy() {
                None => super::intrinsics::bad_argument(
                    const { crate::bytecode::intrinsics::const_id("ProxyTarget") },
                ),
                Some(p) => {
                    let v = if id == PROXY_TARGET {
                        self.heap.proxy_target(p)
                    } else {
                        self.heap.proxy_handler(p)
                    };
                    match v {
                        Some(o) => Ok(Value::object(o)),
                        None => type_error("revoked proxy"),
                    }
                }
            },
            PROXY_REVOKE => match proxy() {
                None => super::intrinsics::bad_argument(
                    const { crate::bytecode::intrinsics::const_id("ProxyRevoke") },
                ),
                Some(p) => {
                    self.heap.proxy_revoke(p);
                    Ok(Value::UNDEFINED)
                }
            },
            _ => return None,
        })
    }
}
