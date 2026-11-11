//! Own-property leaf intrinsics: descriptors, keys, attributes (ECMA-262 7.3, 10.1).
//!
//! They work on ordinary objects (and arrays) without running user code; the builtins
//! in `builtins/object.ts` do the conversions (ToPropertyKey, ToPropertyDescriptor)
//! first.
use crate::PushOut;

use crate::heap::{Atom, Heap, PropFlags, Property, PropertyDescriptor, atoms};
use crate::interp::conv::{self, Fault, type_error};
use crate::value::{Obj, Value};

/// A property key that the builtin already converted (a string, symbol or number).
pub(crate) fn key(heap: &mut Heap, v: Value) -> Result<Atom, Fault> {
    if v.as_object().is_some() {
        return Err(Fault::error(
            crate::interp::ErrorKind::InternalError,
            "bad property key for an intrinsic".into(),
        ));
    }
    conv::to_property_key(heap, v)
}

/// The `attrs` bits of the builtin language (writable 1, enumerable 2, configurable 4)
/// are PropFlags bits.
pub(crate) fn flags(attrs: Value) -> PropFlags {
    PropFlags::from_bits(attrs.as_int().unwrap_or(0) as u8 & 7)
}

/// DefinePropertyOrThrow with a complete data descriptor.
pub(crate) fn define_data_or_throw(
    heap: &mut Heap,
    o: Obj,
    key: Atom,
    v: Value,
    flags: PropFlags,
) -> Result<(), Fault> {
    if !heap.define_own_property(o, key, &PropertyDescriptor::data(v, flags))? {
        return cannot_define(heap, key);
    }
    Ok(())
}

pub(crate) fn cannot_define<T>(heap: &Heap, key: Atom) -> Result<T, Fault> {
    type_error(&crate::text::cat!(
        "cannot define property '",
        conv::describe_key(heap, key),
        "'"
    ))
}

/// [[DefineOwnProperty]] with a descriptor record: an object whose own data properties
/// are the present fields (ToPropertyDescriptor in `builtins/object.ts` made it).
pub(crate) fn define_from_record(
    heap: &mut Heap,
    o: Obj,
    key: Atom,
    rec: Obj,
) -> Result<bool, Fault> {
    let field = |heap: &Heap, k: Atom| heap.get_own_property(rec, k).and_then(|p| p.value());
    let flag = |heap: &Heap, k: Atom| field(heap, k).map(|v| conv::to_boolean(heap, v));
    let d = PropertyDescriptor {
        value: field(heap, atoms::VALUE),
        get: field(heap, atoms::GET),
        set: field(heap, atoms::SET),
        writable: flag(heap, atoms::WRITABLE),
        enumerable: flag(heap, atoms::ENUMERABLE),
        configurable: flag(heap, atoms::CONFIGURABLE),
    };
    Ok(heap.define_own_property(o, key, &d)?)
}

/// FromPropertyDescriptor of an own property, or `undefined`.
pub(crate) fn descriptor_object(heap: &mut Heap, o: Obj, key: Atom) -> Result<Value, Fault> {
    let Some(p) = heap.get_own_property_alloc(o, key)? else {
        return Ok(Value::UNDEFINED);
    };
    let proto = crate::interp::realm::intrinsic_obj(heap, crate::interp::Intrinsic::ObjectProto);
    let f = p.flags();
    let (keys, values) = match p {
        Property::Data { value, .. } => (
            [
                atoms::VALUE,
                atoms::WRITABLE,
                atoms::ENUMERABLE,
                atoms::CONFIGURABLE,
            ],
            [
                value,
                Value::bool(f.writable()),
                Value::bool(f.enumerable()),
                Value::bool(f.configurable()),
            ],
        ),
        Property::Accessor { getter, setter, .. } => (
            [
                atoms::GET,
                atoms::SET,
                atoms::ENUMERABLE,
                atoms::CONFIGURABLE,
            ],
            [
                getter,
                setter,
                Value::bool(f.enumerable()),
                Value::bool(f.configurable()),
            ],
        ),
    };
    Ok(Value::object(heap.new_object_from_entries(
        Some(proto),
        &keys,
        &values,
    )?))
}

/// -1 if absent, else writable 1, enumerable 2, configurable 4, accessor 8.
pub(crate) fn attributes(heap: &Heap, o: Obj, key: Atom) -> i32 {
    match heap.get_own_property(o, key) {
        None => -1,
        Some(p) => p.flags().bits() as i32 | if p.is_accessor() { 8 } else { 0 },
    }
}

/// The own keys as a new array: kind 0 strings, 1 symbols, 2 both, 3 enumerable strings.
pub(crate) fn own_keys(heap: &mut Heap, o: Obj, kind: i32) -> Result<Value, Fault> {
    let keys = heap.own_keys(o);
    let mut values = crate::buf::vec_with_capacity(keys.len());
    for k in keys {
        let keep = match kind {
            0 => !k.is_symbol(),
            1 => k.is_symbol(),
            2 => true,
            _ => {
                !k.is_symbol()
                    && heap
                        .get_own_property(o, k)
                        .is_some_and(|p| p.flags().enumerable())
            }
        };
        if keep {
            values.push_out(heap.atom_to_value(k)?);
        }
    }
    let proto = crate::interp::realm::intrinsic_obj(heap, crate::interp::Intrinsic::ArrayProto);
    Ok(Value::object(
        heap.new_array_from_values(Some(proto), &values)?,
    ))
}

/// SetIntegrityLevel ( O, sealed | frozen ), without proxies.
pub(crate) fn set_integrity_level(heap: &mut Heap, o: Obj, frozen: bool) -> Result<bool, Fault> {
    if !heap.prevent_extensions(o) {
        // A typed array of a resizable buffer (qjs's message).
        return type_error("cannot prevent extensions");
    }
    for k in heap.own_keys(o) {
        let accessor = heap.get_own_property(o, k).is_some_and(|p| p.is_accessor());
        let d = PropertyDescriptor {
            configurable: Some(false),
            writable: (frozen && !accessor).then_some(false),
            ..Default::default()
        };
        if !heap.define_own_property(o, k, &d)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// TestIntegrityLevel ( O, sealed | frozen ), without proxies.
pub(crate) fn test_integrity_level(heap: &Heap, o: Obj, frozen: bool) -> bool {
    if heap.is_extensible(o) {
        return false;
    }
    heap.own_keys(o)
        .into_iter()
        .all(|k| match heap.get_own_property(o, k) {
            None => true,
            Some(p) => {
                !p.flags().configurable()
                    && !(frozen && matches!(p, Property::Data { flags, .. } if flags.writable()))
            }
        })
}

/// The builtinTag of `Object.prototype.toString`.
pub(crate) fn class_tag(heap: &Heap, o: Obj) -> &'static str {
    use crate::heap::ObjectClass as C;
    match heap.object_class(o) {
        C::Array => "Array",
        C::Arguments => "Arguments",
        C::Function | C::HostFunction | C::BoundFunction | C::BuiltinClosure => "Function",
        C::Error => "Error",
        C::Boolean => "Boolean",
        C::Number => "Number",
        C::String => "String",
        C::Date => "Date",
        C::RegExp => "RegExp",
        _ => "Object",
    }
}
