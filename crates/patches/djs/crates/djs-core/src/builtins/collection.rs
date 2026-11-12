//! Map/Set/WeakMap/WeakSet leaf intrinsics over `heap/collection.rs`.

use crate::heap::{Heap, ObjectClass};
use crate::interp::conv::{Fault, type_error};
use crate::interp::{Intrinsic, realm};
use crate::value::{Obj, Value};

pub(crate) fn class_of(i: i32) -> Option<ObjectClass> {
    Some(match i {
        0 => ObjectClass::Map,
        1 => ObjectClass::Set,
        2 => ObjectClass::WeakMap,
        3 => ObjectClass::WeakSet,
        _ => return None,
    })
}

/// 0 Map, 1 Set, 2 WeakMap, 3 WeakSet, -1 anything else.
pub(crate) fn class_index(heap: &Heap, v: Value) -> i32 {
    match v.as_object().map(|o| heap.object_class(o)) {
        Some(ObjectClass::Map) => 0,
        Some(ObjectClass::Set) => 1,
        Some(ObjectClass::WeakMap) => 2,
        Some(ObjectClass::WeakSet) => 3,
        _ => -1,
    }
}

/// A key as stored: strings flattened, so their content hash is cached and cheap.
pub(crate) fn stored_key(heap: &mut Heap, k: Value) -> Result<Value, Fault> {
    Ok(match k.as_string() {
        Some(s) => Value::string(heap.flatten(s)?),
        None => k,
    })
}

pub(crate) fn iterator(heap: &mut Heap, m: Obj, kind: i32) -> Result<Obj, Fault> {
    let proto = if heap.object_class(m) == ObjectClass::Set {
        Intrinsic::SetIteratorProto
    } else {
        Intrinsic::MapIteratorProto
    };
    let proto = realm::intrinsic_obj(heap, proto);
    Ok(heap.collection_iterator(m, kind, proto)?)
}

/// The next value of Map/Set iterator `o` (key, value or entry by its kind), or `None`
/// once done: `next` without the result object (D1800).
pub(crate) fn iterator_step_value(heap: &mut Heap, o: Obj) -> Result<Option<Value>, Fault> {
    let is_set = heap.object_slot(o, 3).as_int() == Some(1);
    let kind = heap.object_slot(o, 2).as_int().unwrap_or(0);
    let Some((k, v)) = heap.collection_iterator_next(o) else {
        return Ok(None);
    };
    Ok(Some(match kind {
        0 => k,
        1 => v,
        // A Set's entries are [value, value].
        _ => {
            let proto = realm::intrinsic_obj(heap, Intrinsic::ArrayProto);
            let pair = if is_set { [k, k] } else { [k, v] };
            Value::object(heap.new_array_from_values(Some(proto), &pair)?)
        }
    }))
}

/// `next()` of a Map or Set Iterator.
pub(crate) fn iterator_next(heap: &mut Heap, it: Value, is_set: bool) -> Result<Value, Fault> {
    let Some(o) = it.as_object().filter(|&o| {
        heap.object_class(o) == ObjectClass::CollectionIterator
            && heap.object_slot(o, 3).as_int() == Some(is_set as i32)
    }) else {
        return type_error(if is_set {
            "Set Iterator object expected"
        } else {
            "Map Iterator object expected"
        });
    };
    let (value, done) = match iterator_step_value(heap, o)? {
        Some(v) => (v, false),
        None => (Value::UNDEFINED, true),
    };
    let proto = realm::intrinsic_obj(heap, Intrinsic::ObjectProto);
    Ok(Value::object(heap.new_iter_result(
        Some(proto),
        value,
        done,
    )?))
}

/// CanBeHeldWeakly ( v ): objects, and symbols that are not registered.
pub(crate) fn can_be_held_weakly(heap: &mut Heap, v: Value) -> Result<bool, Fault> {
    if v.as_object().is_some() {
        return Ok(true);
    }
    let Some(sym) = v.as_symbol() else {
        return Ok(false);
    };
    let Some(d) = heap.symbol_description(sym).as_string() else {
        return Ok(true);
    };
    let registry = realm::intrinsic_obj(heap, Intrinsic::SymbolRegistry);
    let atom = heap.intern(d)?;
    let registered = heap
        .get_own_property(registry, atom)
        .and_then(|p| p.value());
    Ok(registered != Some(v))
}
