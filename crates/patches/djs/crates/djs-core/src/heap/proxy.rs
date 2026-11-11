//! Proxy exotic objects (0045, ECMA-262 10.5).
//!
//! A Proxy (`ObjectClass::Proxy`) has slots: 0 the target, 1 the handler (both `null`
//! once revoked), 2 flags ([`PROXY_CALLABLE`], [`PROXY_CONSTRUCTOR`]), fixed when it is
//! created, as the spec gives a proxy [[Call]] and [[Construct]] from its target.
//!
//! The heap knows nothing about traps: a proxy cell has no prototype and no own
//! properties, and it is not extensible, so heap code that does not know about proxies
//! sees an empty frozen object and can never store into one. It has
//! `OBJ_INDEXED_PROPS`, so the element fast paths do not look past it on a prototype
//! chain. The interpreter stops at a proxy ([`Heap::lookup_property`],
//! [`Heap::has_property_or_proxy`]) and the internal-method intrinsics hand it to the
//! traps in builtins/proxy.ts.

use super::cells::flags::{OBJ_DICTIONARY, OBJ_INDEXED_PROPS};
use super::{Heap, HeapError, ObjectClass};
use crate::value::{Obj, Value};

/// The proxy has [[Call]] (its target is callable).
pub const PROXY_CALLABLE: i32 = 1;
/// The proxy has [[Construct]] (its target is a constructor).
pub const PROXY_CONSTRUCTOR: i32 = 2;

impl Heap {
    /// ProxyCreate's object (10.5.14) for an object target and handler; `flags` are
    /// [`PROXY_CALLABLE`] and [`PROXY_CONSTRUCTOR`] as the caller found the target.
    pub fn new_proxy(&mut self, target: Obj, handler: Obj, flags: i32) -> Result<Obj, HeapError> {
        let p = self.alloc_object(ObjectClass::Proxy, None, ObjectClass::Proxy.slots())?;
        self.set_object_slot(p, 0, Value::object(target));
        self.set_object_slot(p, 1, Value::object(handler));
        self.set_object_slot(p, 2, Value::int(flags));
        // A dictionary without properties (0083): no inline cache ever matches its shape.
        self.set_cell_flags(p.0, OBJ_INDEXED_PROPS | OBJ_DICTIONARY);
        Ok(p)
    }

    #[inline]
    pub fn is_proxy(&self, o: Obj) -> bool {
        self.object_class(o) == ObjectClass::Proxy
    }

    /// The flags of a proxy.
    pub fn proxy_flags(&self, p: Obj) -> i32 {
        self.object_slot(p, 2).as_int().unwrap_or(0)
    }

    /// The target of a proxy, `None` once it is revoked.
    pub fn proxy_target(&self, p: Obj) -> Option<Obj> {
        self.object_slot(p, 0).as_object()
    }

    /// The handler of a proxy, `None` once it is revoked.
    pub fn proxy_handler(&self, p: Obj) -> Option<Obj> {
        self.object_slot(p, 1).as_object()
    }

    /// Revokes a proxy (its revoke function, 10.5.15.1).
    pub fn proxy_revoke(&mut self, p: Obj) {
        self.set_object_slot(p, 0, Value::NULL);
        self.set_object_slot(p, 1, Value::NULL);
    }

    /// IsCallable: callable classes, and proxies whose target is callable.
    #[inline]
    pub fn is_callable(&self, o: Obj) -> bool {
        match self.object_class(o) {
            ObjectClass::Proxy => self.proxy_flags(o) & PROXY_CALLABLE != 0,
            c => c.is_callable(),
        }
    }

    /// IsArray ( argument ), 7.2.2, through proxies: `None` for a revoked proxy (a
    /// TypeError).
    pub fn is_array(&self, o: Obj) -> Option<bool> {
        let mut o = o;
        loop {
            match self.object_class(o) {
                ObjectClass::Array => return Some(true),
                ObjectClass::Proxy => o = self.proxy_target(o)?,
                _ => return Some(false),
            }
        }
    }
}
