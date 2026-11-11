//! Leaf intrinsics of the remaining builtins (0051): iterator helpers, disposable
//! stacks, WeakRef and FinalizationRegistry (with the KeptAlive list and the cleanup
//! jobs after a collection, D1513). The intrinsic
//! fallback chain runs `call_intrinsic` → `json_regexp_intrinsic` → `bigint_intrinsic` →
//! [`Exec::late_intrinsic`].

use super::intrinsics::{bad_argument, id};
use super::*;
use crate::FULL;

const CREATE_ITERATOR_HELPER: u16 = id("CreateIteratorHelper");
const THIS_ITERATOR_HELPER: u16 = id("ThisIteratorHelper");
const DISPOSABLE_STACK_CREATE: u16 = id("DisposableStackCreate");
const WEAK_REF_CREATE: u16 = id("WeakRefCreate");
const WEAK_REF_DEREF: u16 = id("WeakRefDeref");
const FINALIZATION_REGISTRY_CREATE: u16 = id("FinalizationRegistryCreate");
const THIS_FINALIZATION_REGISTRY: u16 = id("ThisFinalizationRegistry");
const FINALIZATION_REGISTRY_REGISTER: u16 = id("FinalizationRegistryRegister");
const FINALIZATION_REGISTRY_UNREGISTER: u16 = id("FinalizationRegistryUnregister");
const FINALIZATION_REGISTRY_NEXT_DEAD: u16 = id("FinalizationRegistryNextDead");
const FINALIZATION_REGISTRY_TAKE: u16 = id("FinalizationRegistryTake");
const FINALIZATION_REGISTRY_SET_QUEUED: u16 = id("FinalizationRegistrySetQueued");
const CLEANUP_JOB: u16 = id("FinalizationRegistryCleanupJob");

/// AddToKeptObjects ( value ), 9.10.2.
fn add_kept_object(heap: &mut Heap, v: Value) -> Result<(), HeapError> {
    let list = intrinsic(heap, Intrinsic::KeptObjects)
        .as_internal()
        .unwrap_or(0);
    let n = if list == 0 {
        0
    } else {
        heap.value_array_len(list)
    };
    let list = heap.value_array_reserve(list, n + 1)?;
    heap.value_array_set(list, n, v);
    heap.arena_mut()
        .set(list, crate::heap::cells::ValueArray::LEN, n + 1);
    realm::set_intrinsic(heap, Intrinsic::KeptObjects, Value::internal(list));
    Ok(())
}

/// ClearKeptObjects ( ), 9.10.3.
pub(crate) fn clear_kept_objects(heap: &mut Heap) {
    if !intrinsic(heap, Intrinsic::KeptObjects).is_undefined() {
        realm::set_intrinsic(heap, Intrinsic::KeptObjects, Value::UNDEFINED);
    }
}

/// HostEnqueueFinalizationRegistryCleanupJob for every live registry with dead cells
/// and no queued job; the number of jobs queued. Called after a collection (or by the
/// host, D1513).
pub(crate) fn schedule_finalization_cleanup(heap: &mut Heap) -> Result<usize, HeapError> {
    let list = intrinsic(heap, Intrinsic::Registries)
        .as_internal()
        .unwrap_or(0);
    let due = heap.registries_needing_cleanup(list);
    let job = realm::installed_intrinsic(heap, CLEANUP_JOB);
    if job.is_undefined() {
        return Ok(0);
    }
    for &r in &due {
        let u = Value::UNDEFINED;
        promise::enqueue(heap, job, [Value::object(r), u, u, u])?;
        heap.set_registry_queued(r, true);
    }
    Ok(due.len())
}
const THIS_DISPOSABLE_STACK: u16 = id("ThisDisposableStack");

/// The kind (slot 0) of `Iterator.from`'s wrapper; the other kinds are helpers
/// (builtins/iterator.ts).
const KIND_WRAP: i32 = 0;

impl Exec<'_> {
    /// `call_intrinsic` for the leaves of 0051; the end of the fallback chain.
    pub(super) fn late_intrinsic(&mut self, id: u16, base: u32, argc: u32, next: u32) -> Step {
        let arg = |ex: &Self, i: u32| {
            if i < argc {
                ex.get(base + i)
            } else {
                Value::UNDEFINED
            }
        };
        let a0 = arg(self, 0);
        let v = match id {
            CREATE_ITERATOR_HELPER if FULL => {
                let Some(kind) = a0.as_int() else {
                    return bad_argument(id);
                };
                let proto = if kind == KIND_WRAP {
                    Intrinsic::WrapForValidIteratorProto
                } else {
                    Intrinsic::IteratorHelperProto
                };
                let proto = intrinsic_obj(self.heap, proto);
                let slots = ObjectClass::IteratorHelper.slots();
                let o = self
                    .heap
                    .alloc_object(ObjectClass::IteratorHelper, Some(proto), slots)?;
                // kind, state (suspended-start), iterator, next method, the rest unset.
                let init = [a0, Value::int(0), arg(self, 1), arg(self, 2)];
                for i in 0..slots {
                    let v = init.get(i as usize).copied().unwrap_or(Value::UNDEFINED);
                    self.heap.set_object_slot(o, i, v);
                }
                Value::object(o)
            }
            THIS_ITERATOR_HELPER if FULL => {
                let wrap = arg(self, 1) == Value::TRUE;
                let ok = a0.as_object().is_some_and(|o| {
                    self.heap.object_class(o) == ObjectClass::IteratorHelper
                        && (self.heap.object_slot(o, 0) == Value::int(KIND_WRAP)) == wrap
                });
                if !ok {
                    let msg = if wrap {
                        "Iterator Wrap object expected"
                    } else {
                        "Iterator Helper object expected"
                    };
                    return type_error(msg);
                }
                a0
            }
            DISPOSABLE_STACK_CREATE if FULL => {
                let Some(proto) = a0.as_object() else {
                    return bad_argument(id);
                };
                let o = self
                    .heap
                    .alloc_object(ObjectClass::DisposableStack, Some(proto), 3)?;
                let resources = self
                    .heap
                    .new_array(Some(intrinsic_obj(self.heap, Intrinsic::ArrayProto)), 0)?;
                self.heap
                    .set_object_slot(o, 0, Value::bool(arg(self, 1) == Value::TRUE));
                self.heap.set_object_slot(o, 1, Value::int(0));
                self.heap.set_object_slot(o, 2, Value::object(resources));
                Value::object(o)
            }
            WEAK_REF_CREATE if FULL => {
                let Some(proto) = a0.as_object() else {
                    return bad_argument(id);
                };
                let target = arg(self, 1);
                let o = self.heap.weakref_new(proto, target)?;
                add_kept_object(self.heap, target)?;
                Value::object(o)
            }
            WEAK_REF_DEREF if FULL => {
                let o = a0
                    .as_object()
                    .filter(|&o| self.heap.object_class(o) == ObjectClass::WeakRef);
                let Some(o) = o else {
                    return type_error("WeakRef object expected");
                };
                match self.heap.weakref_target(o) {
                    Some(t) => {
                        add_kept_object(self.heap, t)?;
                        t
                    }
                    None => Value::UNDEFINED,
                }
            }
            FINALIZATION_REGISTRY_CREATE if FULL => {
                let Some(proto) = a0.as_object() else {
                    return bad_argument(id);
                };
                let r = self.heap.registry_new(proto, arg(self, 1))?;
                let list = intrinsic(self.heap, Intrinsic::Registries)
                    .as_internal()
                    .unwrap_or(0);
                let list = self.heap.registry_list_push(list, r)?;
                realm::set_intrinsic(self.heap, Intrinsic::Registries, Value::internal(list));
                Value::object(r)
            }
            THIS_FINALIZATION_REGISTRY if FULL => {
                let ok = a0.as_object().is_some_and(|o| {
                    self.heap.object_class(o) == ObjectClass::FinalizationRegistry
                });
                if !ok {
                    return type_error("FinalizationRegistry object expected");
                }
                a0
            }
            FINALIZATION_REGISTRY_REGISTER
            | FINALIZATION_REGISTRY_UNREGISTER
            | FINALIZATION_REGISTRY_NEXT_DEAD
            | FINALIZATION_REGISTRY_TAKE
            | FINALIZATION_REGISTRY_SET_QUEUED
                if FULL =>
            {
                let r = a0
                    .as_object()
                    .filter(|&o| self.heap.object_class(o) == ObjectClass::FinalizationRegistry);
                let Some(r) = r else {
                    return bad_argument(id);
                };
                match id {
                    FINALIZATION_REGISTRY_REGISTER => {
                        let token = match arg(self, 3) {
                            t if t.is_undefined() => Value::HOLE,
                            t => t,
                        };
                        self.heap
                            .registry_register(r, arg(self, 1), arg(self, 2), token)?;
                        Value::UNDEFINED
                    }
                    FINALIZATION_REGISTRY_UNREGISTER => {
                        Value::bool(self.heap.registry_unregister(r, arg(self, 1)))
                    }
                    FINALIZATION_REGISTRY_NEXT_DEAD => {
                        Value::int(self.heap.registry_next_dead(r).map_or(-1, |i| i as i32))
                    }
                    FINALIZATION_REGISTRY_TAKE => {
                        let i = arg(self, 1).as_int().and_then(|i| u32::try_from(i).ok());
                        match i.and_then(|i| self.heap.registry_take(r, i)) {
                            Some(v) => v,
                            None => return bad_argument(id),
                        }
                    }
                    _ => {
                        self.heap
                            .set_registry_queued(r, arg(self, 1) == Value::TRUE);
                        Value::UNDEFINED
                    }
                }
            }
            THIS_DISPOSABLE_STACK if FULL => {
                let is_async = arg(self, 1) == Value::TRUE;
                let ok = a0.as_object().is_some_and(|o| {
                    self.heap.object_class(o) == ObjectClass::DisposableStack
                        && self.heap.object_slot(o, 0) == Value::bool(is_async)
                });
                if !ok {
                    let msg = if is_async {
                        "AsyncDisposableStack object expected"
                    } else {
                        "DisposableStack object expected"
                    };
                    return type_error(msg);
                }
                a0
            }
            // The slim build's arms of the families it leaves out end here too (D2411);
            // its prelude never calls them.
            _ if !FULL => {
                return Err(Fault::error(
                    ErrorKind::InternalError,
                    crate::text::cat!(
                        "%",
                        crate::bytecode::intrinsics::intrinsic_name(id),
                        " is not available in this build"
                    ),
                ));
            }
            _ => {
                return Err(Fault::error(
                    ErrorKind::InternalError,
                    crate::text::cat!(
                        "not supported yet: %",
                        crate::bytecode::intrinsics::intrinsic_name(id)
                    ),
                ));
            }
        };
        self.sp = base;
        self.push(v);
        self.pc = next;
        Ok(None)
    }
}
