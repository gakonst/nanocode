//! Objects: properties, dense elements, prototypes. All operations here are leaf: they never
//! run user code. Getters and setters are returned to the caller, which decides how to call
//! them.
//!
//! # Storage
//!
//! An [`Object`] cell has a class, a prototype, a property table ref and an elements ref.
//!
//! - The [`PropTable`] keeps entries in insertion order. Up to [`LINEAR_MAX`] entries it is
//!   searched linearly; above that a hash index over the entries is built (like a
//!   compact dict). Entry keys are atoms, so lookups compare `u32`s.
//! - Dense elements (arrays only, for now) are a [`ValueArray`] whose `LEN` is the dense
//!   length. A dense slot holds a data property with all of writable, enumerable and
//!   configurable set, or a hole.
//!
//! Invariant: an array-index key lives in exactly one place. If its dense slot is not a
//! hole, the property table has no entry for it. Index keys in the table (sparse elements,
//! elements with non-default attributes, index keys of non-arrays) set the
//! [`OBJ_INDEXED_PROPS`] flag, so objects without it never search the table for an index.
//!
//! Classes with exotic integer-keyed behavior (String wrappers, typed arrays, proxies)
//! must also set [`OBJ_INDEXED_PROPS`]: the element fast paths rely on it to know
//! that a prototype chain cannot intercept an index.
//!
//! An array's `length` is internal slot 0 (a Number), not a table entry. It is reported as
//! an own data property `{writable: !OBJ_LENGTH_READONLY, enumerable: false,
//! configurable: false}`.
use crate::PushOut;

use alloc::vec::Vec;

use super::atom::{Atom, atoms};
use super::cells::{
    AccessorPair, FuncTemplate, Object, PropEntry, PropTable, PropValues, Roots, ValueArray,
    flags::{
        OBJ_DICTIONARY, OBJ_EXTENSIBLE, OBJ_INDEXED_PROPS, OBJ_IS_PROTO, OBJ_LAZY_LENGTH,
        OBJ_LAZY_NAME, OBJ_LENGTH_READONLY,
    },
};
use super::layout::Ref;
use super::shape::SHAPE_MAX;
use super::{CellKind, Heap, HeapError};
use crate::value::{Obj, Value};

/// Tables with at most this many entries are searched linearly.
pub const LINEAR_MAX: u32 = 8;

/// A write may extend the dense elements by at most this many holes; writes further out
/// go to the property table (sparse).
pub const DENSE_GAP: u32 = 64;

/// Bucket value for a deleted entry in a property hash index.
const TOMBSTONE: u32 = u32::MAX;

/// [`Heap::receiver_shape`] of a shape-mode object without a values block (no cell is
/// at offset 1).
pub const EMPTY_SHAPE: u32 = 1;

/// Storage-only flag bit: the entry is an accessor.
pub(crate) const STORE_ACCESSOR: u32 = 8;

/// The object class, stored in [`Object::CLASS`]. Issue 0006 adds more.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum ObjectClass {
    Ordinary = 0,
    /// An Array exotic object: dense elements and a `length` in slot 0.
    Array = 1,
    /// A JS closure: its `FuncTemplate` is in the object's `ELEMENTS` field (D2448: a
    /// closure has no dense elements), its slots are the internal references to its
    /// captured Boxes, then the home object if the template has `HOME`.
    Function = 2,
    /// A function implemented by the embedder: slot 0 is its host function id (int32).
    HostFunction = 3,
    /// An Error instance (`[[ErrorData]]`).
    Error = 4,
    /// An unmapped (strict) arguments object: ordinary storage, its own class for
    /// `Object.prototype.toString`.
    Arguments = 5,
    /// An Array Iterator: slot 0 the target (undefined once exhausted), slot 1 the next
    /// index, slot 2 the kind (0 keys, 1 values, 2 entries).
    ArrayIterator = 6,
    /// A bound function exotic object: slot 0 the target function, slot 1 the bound
    /// `this`, slot 2 an Array of the bound arguments.
    BoundFunction = 7,
    /// Primitive wrapper objects (`[[BooleanData]]`, `[[NumberData]]`,
    /// `[[StringData]]`, `[[SymbolData]]`): slot 0 is the primitive. A String object's
    /// `length` and index properties are ordinary own properties defined at creation.
    Boolean = 8,
    Number = 9,
    String = 10,
    Symbol = 11,
    /// Map, Set, WeakMap, WeakSet (0038, `heap/collection.rs`): slot 0 the entries (a
    /// ValueArray, or an EphemeronArray for the weak ones), slot 1 the hash index (a
    /// U32Array), slot 2 the number of live entries.
    Map = 12,
    Set = 13,
    WeakMap = 14,
    WeakSet = 15,
    /// A Map or Set Iterator: slot 0 the entries it walks (undefined once done), slot 1
    /// the next entry number, slot 2 the kind (0 keys, 1 values, 2 entries), slot 3 0
    /// for a Map, 1 for a Set, slots 4 and 5 the key and value of the last step.
    CollectionIterator = 16,
    /// A Promise (`[[PromiseState]]` ...): slot 0 the state (int32: 0 pending, 1
    /// fulfilled, 2 rejected), slot 1 the result, slot 2 the pending reactions (an
    /// internal `ValueArray` of `[capability, onFulfilled, onRejected]` triples, or
    /// undefined), slot 3 flags (int32, `interp::run::promise`). D1371.
    Promise = 17,
    /// A builtin closure (D1372): calling it calls the installed intrinsic builtin whose
    /// id is slot 0 (int32) with `this` = slot 1 (its data) and the call's arguments.
    /// Promise resolving functions, `Promise.all` element functions, ...
    BuiltinClosure = 18,
    /// An internal record (D1372): a prototype-less object with only internal slots,
    /// never handed to user code; the builtins read it with `%RecordGet`/`%RecordSet`.
    Record = 19,
    /// A Date: slot 0 the time value (a Number, NaN for an invalid date).
    Date = 20,
    /// A suspendable function activation (D1375): a generator object, or the internal
    /// state of an async function call. Slot 0 the state (`interp::run::suspend`), slot
    /// 1 the saved frame (an internal ValueArray) while suspended, slot 2 the pc to
    /// resume at, slot 3 the frame's `hp - bp`, slot 4 the result promise (async), slot
    /// 5 the kind (0 generator, 1 async function, 2 async generator), slot 6 an async
    /// generator's request queue (D1421).
    Generator = 21,
    /// An ArrayBuffer, a typed array, a DataView (0039, `heap/typed.rs`).
    ArrayBuffer = 22,
    TypedArray = 23,
    DataView = 24,
    /// A RegExp (0033, `builtins/regexp.rs`): slot 0 the compiled program (an internal
    /// U32Array), slot 1 the source string, slot 2 the flag bits (int32).
    RegExp = 25,
    /// A RegExp String Iterator (matchAll): slot 0 the regexp, slot 1 the string, slot
    /// 2 flags (int32: 1 global, 2 unicode, 4 done).
    RegExpStringIterator = 26,
    /// A Proxy exotic object (0045, `heap/proxy.rs`): slot 0 the target, slot 1 the
    /// handler (both null once revoked), slot 2 flags (int32: 1 callable, 2
    /// constructor, fixed at creation from the target). No prototype, no own
    /// properties, not extensible as far as the heap is concerned: the interpreter and
    /// the builtins send every internal method to the traps (builtins/proxy.ts).
    Proxy = 27,
    /// An iterator helper object (0051, builtins/iterator.ts): the objects of
    /// `Iterator.prototype.map` and friends, `Iterator.concat` and `Iterator.from`'s
    /// wrapper, told apart by slot 0 (the kind). The builtins keep their state in slots
    /// 0..8 with `%RecordGet`/`%RecordSet`.
    IteratorHelper = 28,
    /// A WeakRef and a FinalizationRegistry (0051, `heap/weakref.rs`, D1513).
    WeakRef = 31,
    FinalizationRegistry = 32,
    /// A DisposableStack or AsyncDisposableStack (0051, builtins/disposable.ts): slot 0
    /// true for an AsyncDisposableStack, slot 1 the state (0 pending, 1 disposed), slot 2
    /// the resources (an internal Array of [value, method, kind] triples).
    DisposableStack = 33,
    /// A BigInt wrapper (`[[BigIntData]]`, 0046): slot 0 is the BigInt, like the other
    /// primitive wrappers.
    BigInt = 30,
    /// A raw JSON object (`[[IsRawJSON]]`, JSON.rawJSON, D1611): frozen, null prototype,
    /// one own `rawJSON` string property, no slots.
    RawJSON = 29,
    /// A stack frame given to `Error.prepareStackTrace` (qjs's CallSite, 0106, D2050):
    /// slot 0 the function (null for a parse error position), 1 its name (a string or
    /// null), 2 the file name (a string, null for a native frame), 3 the line and 4 the
    /// column (int32, line 0 without a position), 5 whether it is native and 6 whether
    /// it was called as a constructor (booleans).
    CallSite = 34,
    /// A TextEncoder or TextDecoder (0244, `builtins/encoding.rs`): slot 0 the flags,
    /// slot 1 a decoder's stream state (both int32).
    TextCodec = 35,
}

impl ObjectClass {
    pub const fn from_u32(c: u32) -> Option<ObjectClass> {
        match c {
            0 => Some(ObjectClass::Ordinary),
            1 => Some(ObjectClass::Array),
            2 => Some(ObjectClass::Function),
            3 => Some(ObjectClass::HostFunction),
            4 => Some(ObjectClass::Error),
            5 => Some(ObjectClass::Arguments),
            6 => Some(ObjectClass::ArrayIterator),
            7 => Some(ObjectClass::BoundFunction),
            8 => Some(ObjectClass::Boolean),
            9 => Some(ObjectClass::Number),
            10 => Some(ObjectClass::String),
            11 => Some(ObjectClass::Symbol),
            12 => Some(ObjectClass::Map),
            13 => Some(ObjectClass::Set),
            14 => Some(ObjectClass::WeakMap),
            15 => Some(ObjectClass::WeakSet),
            16 => Some(ObjectClass::CollectionIterator),
            17 => Some(ObjectClass::Promise),
            18 => Some(ObjectClass::BuiltinClosure),
            19 => Some(ObjectClass::Record),
            20 => Some(ObjectClass::Date),
            21 => Some(ObjectClass::Generator),
            22 => Some(ObjectClass::ArrayBuffer),
            23 => Some(ObjectClass::TypedArray),
            24 => Some(ObjectClass::DataView),
            25 => Some(ObjectClass::RegExp),
            26 => Some(ObjectClass::RegExpStringIterator),
            27 => Some(ObjectClass::Proxy),
            28 => Some(ObjectClass::IteratorHelper),
            31 => Some(ObjectClass::WeakRef),
            32 => Some(ObjectClass::FinalizationRegistry),
            33 => Some(ObjectClass::DisposableStack),
            30 => Some(ObjectClass::BigInt),
            29 => Some(ObjectClass::RawJSON),
            34 => Some(ObjectClass::CallSite),
            35 => Some(ObjectClass::TextCodec),
            _ => None,
        }
    }

    /// Number of internal slots the class needs.
    pub const fn slots(self) -> u32 {
        match self {
            ObjectClass::Ordinary
            | ObjectClass::Arguments
            | ObjectClass::Record
            | ObjectClass::RawJSON => 0,
            ObjectClass::Promise => 4,
            // The builtin, its data and its `length` (D3155).
            ObjectClass::BuiltinClosure => 3,
            ObjectClass::Function => 0,
            ObjectClass::Array | ObjectClass::HostFunction => 1,
            // The stack trace (D1971); a DOMException has two more (D1840).
            ObjectClass::Error => 1,
            ObjectClass::ArrayIterator => 3,
            ObjectClass::BoundFunction => 3,
            ObjectClass::Boolean
            | ObjectClass::Number
            | ObjectClass::String
            | ObjectClass::Symbol
            | ObjectClass::BigInt => 1,
            ObjectClass::Map | ObjectClass::Set | ObjectClass::WeakMap | ObjectClass::WeakSet => 3,
            ObjectClass::CollectionIterator => 6,
            ObjectClass::Date => 1,
            ObjectClass::Generator => 7,
            ObjectClass::ArrayBuffer => 2,
            ObjectClass::TypedArray => 4,
            ObjectClass::DataView => 3,
            ObjectClass::RegExp | ObjectClass::RegExpStringIterator => 3,
            ObjectClass::Proxy => 3,
            ObjectClass::IteratorHelper => 8,
            ObjectClass::DisposableStack => 3,
            ObjectClass::WeakRef => 1,
            ObjectClass::FinalizationRegistry => 5,
            ObjectClass::CallSite => 7,
            ObjectClass::TextCodec => 2,
        }
    }

    /// Callable objects of this class. A Proxy is callable when its target is: see
    /// [`Heap::is_callable`].
    pub const fn is_callable(self) -> bool {
        matches!(
            self,
            ObjectClass::Function
                | ObjectClass::HostFunction
                | ObjectClass::BoundFunction
                | ObjectClass::BuiltinClosure
        )
    }
}

/// Property attributes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct PropFlags(u8);

impl PropFlags {
    pub const NONE: PropFlags = PropFlags(0);
    pub const WRITABLE: PropFlags = PropFlags(1);
    pub const ENUMERABLE: PropFlags = PropFlags(2);
    pub const CONFIGURABLE: PropFlags = PropFlags(4);
    /// Writable, enumerable and configurable: what assignment and dense elements use.
    pub const ALL: PropFlags = PropFlags(7);

    pub const fn new(writable: bool, enumerable: bool, configurable: bool) -> PropFlags {
        PropFlags(writable as u8 | (enumerable as u8) << 1 | (configurable as u8) << 2)
    }

    #[inline]
    pub const fn bits(self) -> u8 {
        self.0
    }

    #[inline]
    pub const fn from_bits(bits: u8) -> PropFlags {
        PropFlags(bits & 7)
    }

    /// Meaningless for accessors (always false there).
    #[inline]
    pub const fn writable(self) -> bool {
        self.0 & 1 != 0
    }

    #[inline]
    pub const fn enumerable(self) -> bool {
        self.0 & 2 != 0
    }

    #[inline]
    pub const fn configurable(self) -> bool {
        self.0 & 4 != 0
    }
}

impl core::ops::BitOr for PropFlags {
    type Output = PropFlags;
    fn bitor(self, o: PropFlags) -> PropFlags {
        PropFlags(self.0 | o.0)
    }
}

/// A complete own property, as stored.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Property {
    Data {
        value: Value,
        flags: PropFlags,
    },
    /// `getter`/`setter` are function objects or `undefined`. `flags` has no writable bit.
    Accessor {
        getter: Value,
        setter: Value,
        flags: PropFlags,
    },
}

impl Property {
    pub const fn data(value: Value, flags: PropFlags) -> Property {
        Property::Data { value, flags }
    }

    pub const fn flags(&self) -> PropFlags {
        match self {
            Property::Data { flags, .. } | Property::Accessor { flags, .. } => *flags,
        }
    }

    pub const fn is_accessor(&self) -> bool {
        matches!(self, Property::Accessor { .. })
    }

    /// The value of a data property.
    pub const fn value(&self) -> Option<Value> {
        match self {
            Property::Data { value, .. } => Some(*value),
            Property::Accessor { .. } => None,
        }
    }
}

/// A property descriptor with optional fields (the spec's Property Descriptor). A
/// descriptor must not have both data fields (`value`, `writable`) and accessor fields
/// (`get`, `set`); `ToPropertyDescriptor` (caller side) rejects that.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct PropertyDescriptor {
    pub value: Option<Value>,
    pub get: Option<Value>,
    pub set: Option<Value>,
    pub writable: Option<bool>,
    pub enumerable: Option<bool>,
    pub configurable: Option<bool>,
}

impl PropertyDescriptor {
    /// A complete data descriptor.
    pub const fn data(value: Value, flags: PropFlags) -> PropertyDescriptor {
        PropertyDescriptor {
            value: Some(value),
            get: None,
            set: None,
            writable: Some(flags.writable()),
            enumerable: Some(flags.enumerable()),
            configurable: Some(flags.configurable()),
        }
    }

    /// A complete accessor descriptor.
    pub const fn accessor(getter: Value, setter: Value, flags: PropFlags) -> PropertyDescriptor {
        PropertyDescriptor {
            value: None,
            get: Some(getter),
            set: Some(setter),
            writable: None,
            enumerable: Some(flags.enumerable()),
            configurable: Some(flags.configurable()),
        }
    }

    pub const fn is_accessor(&self) -> bool {
        self.get.is_some() || self.set.is_some()
    }

    pub const fn is_data(&self) -> bool {
        self.value.is_some() || self.writable.is_some()
    }

    pub const fn is_generic(&self) -> bool {
        !self.is_accessor() && !self.is_data()
    }

    const fn is_empty(&self) -> bool {
        self.is_generic() && self.enumerable.is_none() && self.configurable.is_none()
    }
}

/// The result of [`Heap::lookup_property`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lookup {
    /// The object on the chain that has the key, and the property.
    Found(Obj, Property),
    Absent,
    /// A Proxy reached before the key was found.
    Proxy(Obj),
}

/// Where an own property is stored.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Loc {
    None,
    Dense(u32),
    /// A named property slot (`prop_*`).
    Prop(u32),
    ArrayLength,
    /// A closure's unstored `length` and `name` (D2445, `OBJ_LAZY_LENGTH`/`_NAME`).
    LazyLength,
    LazyName,
}

/// Both lazy-property flags of a closure (D2445).
const OBJ_LAZY: u16 = OBJ_LAZY_LENGTH | OBJ_LAZY_NAME;

/// Why an object became a dictionary (test statistics, `Heap::dict_reasons`).
#[derive(Clone, Copy)]
enum Dict {
    /// More than `SHAPE_MAX` properties.
    TooMany = 0,
    /// The shape budget is spent.
    Budget = 1,
    /// A key other than the last was deleted.
    Delete = 2,
    /// An attribute change.
    Attributes = 3,
    /// An array-index key.
    Index = 4,
    /// A builtin namespace or prototype, the global object.
    Builtin = 5,
}

/// The result of ValidateAndApplyPropertyDescriptor.
enum Outcome {
    Reject,
    Keep,
    Write(Property),
}

#[inline]
pub(crate) fn key_hash(a: Atom) -> u32 {
    let h = a.id().wrapping_mul(0x9E37_79B9);
    h ^ (h >> 15)
}

impl Heap {
    // ---------------------------------------------------------------- objects

    /// Allocates an object with `slots` internal slots (set to `undefined`).
    pub fn alloc_object(
        &mut self,
        class: ObjectClass,
        proto: Option<Obj>,
        slots: u32,
    ) -> Result<Obj, HeapError> {
        let size = Object::SLOTS
            .cell_size(slots)
            .ok_or(HeapError::OutOfMemory)?;
        let o = self.alloc_cell(CellKind::Object, size)?;
        // (A new cell: its writes need no note in a tracked prefix.)
        let a = self.arena_mut();
        a.set_untracked(o, Object::CLASS, class as u32);
        a.set_untracked(o, Object::PROTO, Ref(proto.map_or(0, |p| p.0)));
        for i in 0..slots {
            a.write_untracked(Object::SLOTS.at(o, i), Value::UNDEFINED);
        }
        a.set_untracked(o, crate::heap::Header::FLAGS, OBJ_EXTENSIBLE);
        Ok(Obj(o))
    }

    /// A new ordinary object.
    pub fn new_object(&mut self, proto: Option<Obj>) -> Result<Obj, HeapError> {
        self.alloc_object(ObjectClass::Ordinary, proto, 0)
    }

    /// A new empty array with room for `capacity` dense elements.
    pub fn new_array(&mut self, proto: Option<Obj>, capacity: u32) -> Result<Obj, HeapError> {
        let o = self.alloc_object(ObjectClass::Array, proto, ObjectClass::Array.slots())?;
        self.set_array_length_raw(o, 0);
        if capacity > 0 {
            let el = self.alloc_value_array(capacity)?;
            self.arena_mut().set(o.0, Object::ELEMENTS, Ref(el));
        }
        Ok(o)
    }

    /// ArrayCreate(len) with prototype `proto`: an empty array whose `length` is `len`
    /// (all holes), without ArraySetLength's descriptor checks, which a fresh array always
    /// passes (0162).
    pub fn new_array_with_length(
        &mut self,
        proto: Option<Obj>,
        len: u32,
    ) -> Result<Obj, HeapError> {
        let o = self.new_array(proto, 0)?;
        self.set_array_length_raw(o, len);
        Ok(o)
    }

    /// A new ordinary object with the data properties `keys[i]: values[i]` (writable,
    /// enumerable, configurable), defined in order: a repeated key keeps its first
    /// position and its last value, as repeated CreateDataProperty calls would. The table is
    /// allocated once at its final size. Used by JSON.parse and object literals.
    pub fn new_object_from_entries(
        &mut self,
        proto: Option<Obj>,
        keys: &[Atom],
        values: &[Value],
    ) -> Result<Obj, HeapError> {
        assert_eq!(keys.len(), values.len());
        let o = self.new_object(proto)?;
        if keys.is_empty() {
            return Ok(o);
        }
        let n = u32::try_from(keys.len()).map_err(|_| HeapError::OutOfMemory)?;
        let index = keys.iter().any(|&k| self.atom_array_index(k).is_some());
        if n <= SHAPE_MAX && !index && self.shaped_from_entries(o, keys, values)? {
            return Ok(o);
        }
        #[cfg(test)]
        {
            let why = match () {
                _ if index => Dict::Index,
                _ if n > SHAPE_MAX => Dict::TooMany,
                _ => Dict::Budget,
            };
            self.dict_reasons[why as usize] += 1;
        }
        self.set_obj_flag(o, OBJ_DICTIONARY, true);
        let t = self.alloc_table(n.max(4))?;
        self.arena_mut().set(o.0, Object::PROPS, Ref(t));
        for (&key, &value) in keys.iter().zip(values) {
            let p = Property::data(value, PropFlags::ALL);
            let t = self.props(o);
            match self.table_find(t, key) {
                Some(i) => self.write_entry(t, i, key, &p)?,
                None => self.prop_insert(o, key, &p)?,
            }
        }
        Ok(o)
    }

    /// [`Heap::new_object_from_entries`] in shape mode (at most [`SHAPE_MAX`] keys, none
    /// an array index): one walk down the shape tree, and the values block at its exact
    /// size. False, with `o` unchanged, if a shape limit is reached.
    fn shaped_from_entries(
        &mut self,
        o: Obj,
        keys: &[Atom],
        values: &[Value],
    ) -> Result<bool, HeapError> {
        let mut slots = [0u32; SHAPE_MAX as usize];
        let mut s = self.root_shape()?;
        for (k, &key) in keys.iter().enumerate() {
            slots[k] = match self.shape_find(s, key) {
                Some(i) => i,
                None => {
                    let flags = PropFlags::ALL.bits() as u32;
                    let Some(c) = self.shape_child(s, key, flags)? else {
                        return Ok(false);
                    };
                    let i = self.shape_count(s);
                    s = c;
                    i
                }
            };
        }
        let count = self.shape_count(s);
        let v = self.alloc_values(count)?;
        let a = self.arena_mut();
        // A repeated key keeps its first slot and its last value.
        for (&i, &value) in slots.iter().zip(values) {
            a.write(PropValues::VALUES.at(v, i), value);
        }
        a.set(v, PropValues::LEN, count);
        a.set(v, PropValues::SHAPE, Ref(s));
        a.set(o.0, Object::PROPS, Ref(v));
        Ok(true)
    }

    pub(crate) fn plain_shape(&self, o: Obj) -> Option<u32> {
        if self.object_class(o) != ObjectClass::Ordinary
            || self.obj_flags(o) & (OBJ_DICTIONARY | OBJ_INDEXED_PROPS | OBJ_IS_PROTO) != 0
            || self.elements(o) != 0
        {
            return None;
        }
        let v = self.props(o);
        if v == 0 {
            return Some(0);
        }
        let s = self.arena().get(v, PropValues::SHAPE).0;
        let flags = PropFlags::ALL.bits() as u32;
        (0..self.shape_count(s))
            .all(|i| {
                let (k, f) = self.shape_desc(s, i);
                k.is_string() && f == flags
            })
            .then_some(s)
    }

    pub(crate) fn plain_entries(&self, o: Obj) -> Option<(Vec<Atom>, Vec<Value>)> {
        let s = self.plain_shape(o)?;
        if s == 0 {
            return Some((Vec::new(), Vec::new()));
        }
        let v = self.props(o);
        let count = self.shape_count(s);
        let keys = (0..count).map(|i| self.shape_desc(s, i).0).collect();
        let values = (0..count)
            .map(|i| self.arena().read(PropValues::VALUES.at(v, i)))
            .collect();
        Some((keys, values))
    }

    pub(crate) fn plain_entries_into(
        &self,
        o: Obj,
        keys: &mut [Atom],
        values: &mut [Value],
    ) -> Option<usize> {
        let s = self.plain_shape(o)?;
        if s == 0 {
            return Some(0);
        }
        let v = self.props(o);
        let count = self.shape_count(s) as usize;
        if count > keys.len().min(values.len()) {
            return None;
        }
        for i in 0..count {
            keys[i] = self.shape_desc(s, i as u32).0;
            values[i] = self.arena().read(PropValues::VALUES.at(v, i as u32));
        }
        Some(count)
    }

    pub(crate) fn clone_plain_props(&mut self, o: Obj, s: Obj) -> Result<bool, HeapError> {
        if self.plain_shape(o) != Some(0) || self.obj_flags(o) & OBJ_EXTENSIBLE == 0 {
            return Ok(false);
        }
        let Some(shape) = self.plain_shape(s) else {
            return Ok(false);
        };
        if shape == 0 {
            return Ok(true);
        }
        let count = self.shape_count(shape);
        let from = self.props(s);
        let v = self.alloc_values(count)?;
        let a = self.arena_mut();
        for i in 0..count {
            let value: Value = a.read(PropValues::VALUES.at(from, i));
            a.write(PropValues::VALUES.at(v, i), value);
        }
        a.set(v, PropValues::LEN, count);
        a.set(v, PropValues::SHAPE, Ref(shape));
        a.set(o.0, Object::PROPS, Ref(v));
        Ok(true)
    }

    pub(crate) fn new_object_from_entries_like(
        &mut self,
        proto: Option<Obj>,
        keys: &[Atom],
        values: &[Value],
        last: &mut u32,
    ) -> Result<Obj, HeapError> {
        let s = *last;
        if s != 0 && self.shape_is(s, keys, PropFlags::ALL.bits() as u32) {
            let o = self.new_object(proto)?;
            self.adopt_shape(o, s, values)?;
            return Ok(o);
        }
        let o = self.new_object_from_entries(proto, keys, values)?;
        *last = if keys.is_empty() {
            0
        } else {
            self.plain_props_shape(o)
        };
        Ok(o)
    }

    pub(crate) fn new_iter_result(
        &mut self,
        proto: Option<Obj>,
        value: Value,
        done: bool,
    ) -> Result<Obj, HeapError> {
        let mut last = self.iter_result_shape;
        let keys = [atoms::VALUE, atoms::DONE];
        let o = self.new_object_from_entries_like(
            proto,
            &keys,
            &[value, Value::bool(done)],
            &mut last,
        )?;
        self.iter_result_shape = last;
        Ok(o)
    }

    pub(crate) fn adopt_shape(
        &mut self,
        o: Obj,
        s: u32,
        values: &[Value],
    ) -> Result<(), HeapError> {
        let count = self.shape_count(s);
        let v = self.alloc_values(count)?;
        let a = self.arena_mut();
        for (i, &value) in (0..count).zip(values) {
            a.write(PropValues::VALUES.at(v, i), value);
        }
        a.set(v, PropValues::LEN, count);
        a.set(v, PropValues::SHAPE, Ref(s));
        a.set(o.0, Object::PROPS, Ref(v));
        Ok(())
    }

    pub(crate) fn plain_props_shape(&self, o: Obj) -> u32 {
        let v = self.props(o);
        if self.is_dictionary(o) || v == 0 {
            0
        } else {
            self.arena().get(v, PropValues::SHAPE).0
        }
    }

    /// A new array with `values` as its dense elements (`length` = `values.len()`).
    /// Values may be holes.
    pub fn new_array_from_values(
        &mut self,
        proto: Option<Obj>,
        values: &[Value],
    ) -> Result<Obj, HeapError> {
        let n = u32::try_from(values.len())
            .ok()
            .filter(|&n| n <= atom_int_max())
            .ok_or(HeapError::OutOfMemory)?;
        let o = self.new_array(proto, n)?;
        if n > 0 {
            let el = self.elements(o);
            let a = self.arena_mut();
            let base = ValueArray::VALUES.at(el, 0);
            let dst = a.slice_mut(base, n * 8);
            for (d, v) in dst.as_chunks_mut::<8>().0.iter_mut().zip(values) {
                *d = v.to_bits().to_le_bytes();
            }
            a.set(el, ValueArray::LEN, n);
            self.set_array_length_raw(o, n);
        }
        Ok(o)
    }

    /// A new array of `src[from .. to)`, copied in one step from `src`'s dense store
    /// (slice's fast path, 0129), or `None` (nothing allocated) unless `src` is an Array
    /// whose slots there are all dense, non-hole elements.
    pub(crate) fn new_array_from_dense(
        &mut self,
        proto: Option<Obj>,
        src: Obj,
        from: u32,
        to: u32,
    ) -> Result<Option<Obj>, HeapError> {
        if self.object_class(src) != ObjectClass::Array || from > to || to > self.dense_len(src) {
            return Ok(None);
        }
        let n = to - from;
        if n == 0 {
            return Ok(Some(self.new_array(proto, 0)?));
        }
        let el = self.elements(src);
        if (from..to).any(|i| self.value_array_get(el, i).is_hole()) {
            return Ok(None);
        }
        // (Allocating moves no cell: `el` stays valid.)
        let o = self.new_array(proto, n)?;
        let dst = self.elements(o);
        let a = self.arena_mut();
        a.copy_within(
            ValueArray::VALUES.at(el, from),
            n * 8,
            ValueArray::VALUES.at(dst, 0),
        );
        a.set(dst, ValueArray::LEN, n);
        self.set_array_length_raw(o, n);
        Ok(Some(o))
    }

    pub fn object_class(&self, o: Obj) -> ObjectClass {
        let c = self.arena().get(o.0, Object::CLASS);
        ObjectClass::from_u32(c).unwrap_or_else(|| super::fail::bad_object_class(c))
    }

    /// Number of internal slots.
    #[inline]
    pub fn object_slot_count(&self, o: Obj) -> u32 {
        Object::LAYOUT.tail_capacity(self.cell_size(o.0))
    }

    pub fn object_slot(&self, o: Obj, i: u32) -> Value {
        if i >= self.object_slot_count(o) {
            super::fail::slot_out_of_range(i);
        }
        self.arena().read(Object::SLOTS.at(o.0, i))
    }

    pub fn set_object_slot(&mut self, o: Obj, i: u32, v: Value) {
        if i >= self.object_slot_count(o) {
            super::fail::slot_out_of_range(i);
        }
        self.arena_mut().write(Object::SLOTS.at(o.0, i), v)
    }

    #[inline]
    fn obj_flags(&self, o: Obj) -> u16 {
        self.cell_flags(o.0)
    }

    fn set_obj_flag(&mut self, o: Obj, flag: u16, on: bool) {
        let f = self.cell_flags(o.0);
        let n = if on { f | flag } else { f & !flag };
        // Not written when unchanged: a builtin's header stays as the prefix has it
        // (D2063).
        if n != f {
            self.set_cell_flags(o.0, n);
        }
    }

    pub fn get_prototype_of(&self, o: Obj) -> Option<Obj> {
        self.arena().get(o.0, Object::PROTO).get().map(Obj)
    }

    /// OrdinarySetPrototypeOf: fails on a non-extensible object or a cycle.
    pub fn set_prototype_of(&mut self, o: Obj, proto: Option<Obj>) -> bool {
        if self.get_prototype_of(o) == proto {
            return true;
        }
        if !self.is_extensible(o) {
            return false;
        }
        let mut p = proto;
        while let Some(q) = p {
            if q == o {
                return false;
            }
            p = self.get_prototype_of(q);
        }
        self.arena_mut()
            .set(o.0, Object::PROTO, Ref(proto.map_or(0, |p| p.0)));
        self.proto_changed(o);
        true
    }

    /// The prototype epoch of the inline caches (D1621).
    #[inline(always)]
    pub fn proto_epoch(&self) -> u32 {
        self.arena().get(0, Roots::PROTO_EPOCH)
    }

    pub(crate) fn bump_proto_epoch(&mut self) {
        let e = self.proto_epoch();
        self.arena_mut()
            .set(0, Roots::PROTO_EPOCH, e.wrapping_add(1));
    }

    /// Marks `o` as on the chain of a cached prototype property (D1621).
    pub fn mark_proto(&mut self, o: Obj) {
        self.set_obj_flag(o, OBJ_IS_PROTO, true);
    }

    /// A key was added to `o` or its prototype changed: caches through it are stale.
    #[inline]
    fn proto_changed(&mut self, o: Obj) {
        if self.obj_flags(o) & OBJ_IS_PROTO != 0 {
            self.bump_proto_epoch();
        }
    }

    pub fn is_extensible(&self, o: Obj) -> bool {
        self.obj_flags(o) & OBJ_EXTENSIBLE != 0 && !self.frozen(o)
    }

    /// [[PreventExtensions]]: false for a typed array whose length can change (a view
    /// of a resizable buffer, 10.4.5.3 / IsTypedArrayFixedLength), which stays
    /// extensible.
    pub fn prevent_extensions(&mut self, o: Obj) -> bool {
        // A frozen builtin is not extensible already (D2304): nothing to write.
        if self.frozen(o) {
            return true;
        }
        if self.object_class(o) == ObjectClass::TypedArray && !self.ta_fixed_length(o) {
            return false;
        }
        self.set_obj_flag(o, OBJ_EXTENSIBLE, false);
        true
    }

    // ---------------------------------------------------------------- arrays

    /// The `length` of an array.
    #[inline]
    pub fn array_length(&self, o: Obj) -> u32 {
        debug_assert_eq!(self.object_class(o), ObjectClass::Array);
        let n = self.object_slot(o, 0).as_number().unwrap_or(0.0);
        n as u32
    }

    fn set_array_length_raw(&mut self, o: Obj, len: u32) {
        let v = if len <= i32::MAX as u32 {
            Value::int(len as i32)
        } else {
            Value::number(len as f64)
        };
        self.set_object_slot(o, 0, v);
    }

    /// The dense elements (0 for none). A closure's `ELEMENTS` field is its template
    /// (D2448), whose word where a `ValueArray` has its `LEN` is always 0
    /// (`FuncTemplate::NO_ELEMENTS`): read as elements, it is an empty array, so the
    /// element paths need no class check. Nothing writes a closure's elements: only
    /// arrays get dense elements (`dense_accepts`).
    #[inline]
    fn elements(&self, o: Obj) -> u32 {
        let el = self.arena().get::<Ref>(o.0, Object::ELEMENTS).0;
        debug_assert!(
            el == 0
                || self.cell_kind(el) == CellKind::ValueArray
                || (self.cell_kind(el) == CellKind::FuncTemplate
                    && self.object_class(o) == ObjectClass::Function
                    && self.value_array_len(el) == 0),
            "the elements of object {} are a {:?}",
            o.0,
            self.cell_kind(el)
        );
        el
    }

    /// A new closure of template `t` with `slots` slots (captures, home object), set to
    /// `undefined` (D2448: the template is in the `ELEMENTS` field).
    pub fn alloc_closure(
        &mut self,
        proto: Option<Obj>,
        t: u32,
        slots: u32,
    ) -> Result<Obj, HeapError> {
        let f = self.alloc_object(ObjectClass::Function, proto, slots)?;
        self.arena_mut()
            .set_untracked(f.0, Object::ELEMENTS, Ref(t));
        Ok(f)
    }

    /// The template of closure `o` (D2448).
    #[inline]
    pub fn closure_template(&self, o: Obj) -> u32 {
        debug_assert_eq!(self.object_class(o), ObjectClass::Function);
        self.arena().get::<Ref>(o.0, Object::ELEMENTS).0
    }

    /// The dense store of `o` (a `ValueArray` cell, 0 for none), for leaves that keep
    /// their working data in an internal Array they made (the comparator sort, D3125)
    /// and read it with `value_array_get`/`value_array_set`.
    #[inline]
    pub(crate) fn dense_store(&self, o: Obj) -> u32 {
        self.elements(o)
    }

    /// The dense length (0 without elements).
    #[inline]
    pub fn dense_len(&self, o: Obj) -> u32 {
        match self.elements(o) {
            0 => 0,
            el => self.value_array_len(el),
        }
    }

    /// The first `length` slots of an Array's dense store, if they cover its length
    /// (holes included: callers check `is_hole`), read straight from one slice. For bulk
    /// readers (`String.fromCodePoint`, `apply`): no per-element class or bounds checks.
    /// `None` for anything else.
    pub(crate) fn dense_array_values(&self, o: Obj) -> Option<impl Iterator<Item = Value> + '_> {
        if self.object_class(o) != ObjectClass::Array {
            return None;
        }
        let n = self.array_length(o);
        let el = self.elements(o);
        let bytes: &[u8] = if n == 0 {
            &[]
        } else if el != 0 && self.value_array_len(el) >= n {
            self.arena()
                .slice(ValueArray::VALUES.at(el, 0), n.checked_mul(8)?)
        } else {
            return None;
        };
        Some(
            bytes
                .as_chunks::<8>()
                .0
                .iter()
                .map(|b| Value::from_raw(u64::from_le_bytes(*b))),
        )
    }

    /// Fast path for reading element `i`: `Some` if it is a dense, non-hole element.
    /// `None` means "take the generic path" (property table, prototype chain).
    #[inline]
    pub fn get_element_fast(&self, o: Obj, i: u32) -> Option<Value> {
        let el = self.elements(o);
        if el == 0 {
            // Typed arrays have no elements store: their elements live in the buffer.
            // A BigInt element (a placeholder here) takes the generic path.
            return if self.object_class(o) == ObjectClass::TypedArray {
                self.ta_get(o, i).filter(|v| !v.is_hole())
            } else {
                None
            };
        }
        if i >= self.value_array_len(el) {
            return None;
        }
        let v = self.value_array_get(el, i);
        if v.is_hole() { None } else { Some(v) }
    }

    /// Fast path for `o[i] = v` with `o` as receiver. Returns `Ok(true)` when done:
    /// overwriting a dense element, or adding one (filling a hole or appending) to an
    /// extensible array whose prototype chain has no index properties. `Ok(false)` means
    /// "take the generic `[[Set]]` path".
    /// splice's element moves on a plain dense array in one step: `o[start .. start +
    /// delete)` is replaced by `insert` undefined slots (the caller writes them), the
    /// tail moves, and `length` follows. Only for an extensible Array with a writable
    /// length, no holes, every element dense (default attributes) and no index
    /// properties on its prototype chain, where the generic moves (HasProperty, Get,
    /// Set, DeletePropertyOrThrow) could not observe anything else. `Ok(false)`: not
    /// such an array; nothing changed.
    pub(crate) fn array_splice_dense(
        &mut self,
        o: Obj,
        start: u32,
        delete: u32,
        insert: u32,
    ) -> Result<bool, HeapError> {
        if self.object_class(o) != ObjectClass::Array || self.frozen(o) {
            return Ok(false);
        }
        let f = self.obj_flags(o);
        if f & OBJ_EXTENSIBLE == 0 || f & (OBJ_INDEXED_PROPS | OBJ_LENGTH_READONLY) != 0 {
            return Ok(false);
        }
        let len = self.array_length(o);
        if self.dense_len(o) != len || start > len || delete > len - start {
            return Ok(false);
        }
        let mut p = self.get_prototype_of(o);
        while let Some(q) = p {
            if self.dense_len(q) != 0 || self.obj_flags(q) & OBJ_INDEXED_PROPS != 0 {
                return Ok(false);
            }
            p = self.get_prototype_of(q);
        }
        let el = self.elements(o);
        let mut values: Vec<Value> =
            crate::buf::vec_with_capacity((len - delete + insert) as usize);
        for i in 0..len {
            let v = self.value_array_get(el, i);
            if v.is_hole() {
                return Ok(false);
            }
            if i == start {
                values.extend(core::iter::repeat_n(Value::UNDEFINED, insert as usize));
            }
            if i < start || i >= start + delete {
                values.push_out(v);
            }
        }
        if start == len {
            values.extend(core::iter::repeat_n(Value::UNDEFINED, insert as usize));
        }
        let new_len = u32::try_from(values.len()).map_err(|_| HeapError::OutOfMemory)?;
        if new_len > atom_int_max() {
            return Ok(false);
        }
        for (i, v) in values.into_iter().enumerate() {
            self.dense_put(o, i as u32, v)?;
        }
        if new_len < len {
            self.truncate_elements(o, new_len)?;
        }
        self.set_array_length_raw(o, new_len);
        Ok(true)
    }

    pub fn set_element_fast(&mut self, o: Obj, i: u32, v: Value) -> Result<bool, HeapError> {
        if self.frozen(o) {
            return Ok(false);
        }
        let el = self.elements(o);
        let len = if el == 0 { 0 } else { self.value_array_len(el) };
        if i < len && !self.value_array_get(el, i).is_hole() {
            self.value_array_set(el, i, v);
            return Ok(true);
        }
        if i > len || i > atom_int_max() || self.object_class(o) != ObjectClass::Array {
            // A typed array element: [[Set]] with the array as receiver always succeeds
            // (an invalid index writes nothing), once the value is a primitive.
            if el == 0 && self.object_class(o) == ObjectClass::TypedArray {
                // An immutable one: the generic [[Set]] fails (D1970).
                if self.ta_immutable(o) {
                    return Ok(false);
                }
                return Ok(match self.element_of(self.ta_kind(o), v) {
                    Some(x) => {
                        self.ta_set(o, i, x);
                        true
                    }
                    None => false,
                });
            }
            return Ok(false);
        }
        let f = self.obj_flags(o);
        if f & OBJ_EXTENSIBLE == 0 || f & OBJ_INDEXED_PROPS != 0 {
            return Ok(false);
        }
        let length = self.array_length(o);
        if i >= length && f & OBJ_LENGTH_READONLY != 0 {
            return Ok(false);
        }
        let mut p = self.get_prototype_of(o);
        while let Some(q) = p {
            if self.dense_len(q) != 0 || self.obj_flags(q) & OBJ_INDEXED_PROPS != 0 {
                return Ok(false);
            }
            p = self.get_prototype_of(q);
        }
        self.dense_put(o, i, v)?;
        if i >= length {
            self.set_array_length_raw(o, i + 1);
        }
        Ok(true)
    }

    /// `o.push(v)` of one value on an Array as a plain append: what `set_element_fast(o,
    /// length, v)` does when the dense store ends at `length`, in one pass over the
    /// object's flags (0231: push's leaf was a third of a push loop). The new length, or
    /// `None` with nothing changed (the caller takes the general path).
    pub(crate) fn array_append_fast(&mut self, o: Obj, v: Value) -> Result<Option<u32>, HeapError> {
        let f = self.obj_flags(o);
        if f & (OBJ_INDEXED_PROPS | OBJ_LENGTH_READONLY) != 0
            || f & OBJ_EXTENSIBLE == 0
            || self.frozen(o)
            || self.object_class(o) != ObjectClass::Array
        {
            return Ok(None);
        }
        let el = self.elements(o);
        let len = if el == 0 { 0 } else { self.value_array_len(el) };
        if len != self.array_length(o) || len >= atom_int_max() {
            return Ok(None);
        }
        // [[Set]] of a new index looks at the prototypes for a setter.
        let mut p = self.get_prototype_of(o);
        while let Some(q) = p {
            if self.dense_len(q) != 0 || self.obj_flags(q) & OBJ_INDEXED_PROPS != 0 {
                return Ok(None);
            }
            p = self.get_prototype_of(q);
        }
        if el != 0 && len < self.value_array_capacity(el) {
            self.value_array_set(el, len, v);
            self.arena_mut().set(el, ValueArray::LEN, len + 1);
        } else {
            self.dense_put(o, len, v)?;
        }
        self.set_array_length_raw(o, len + 1);
        Ok(Some(len + 1))
    }

    /// CreateDataProperty(o, i, v) on an Array as a dense store: an existing dense element
    /// (default attributes) is overwritten, a hole filled or the next element appended
    /// when `o` is extensible, has no index properties outside the dense store and, past
    /// `length`, a writable `length`. Unlike [[Set]] (`set_element_fast`) the prototype
    /// chain plays no part: the property is defined on `o` itself (0129: map's and
    /// filter's results walked it per element). `Ok(false)`: the generic path; nothing
    /// changed.
    pub(crate) fn create_element_fast(
        &mut self,
        o: Obj,
        i: u32,
        v: Value,
    ) -> Result<bool, HeapError> {
        debug_assert_eq!(self.object_class(o), ObjectClass::Array);
        if self.frozen(o) {
            return Ok(false);
        }
        let el = self.elements(o);
        let len = if el == 0 { 0 } else { self.value_array_len(el) };
        if i < len && !self.value_array_get(el, i).is_hole() {
            self.value_array_set(el, i, v);
            return Ok(true);
        }
        if i > len || i > atom_int_max() {
            return Ok(false);
        }
        let f = self.obj_flags(o);
        if f & OBJ_EXTENSIBLE == 0 || f & OBJ_INDEXED_PROPS != 0 {
            return Ok(false);
        }
        let length = self.array_length(o);
        if i >= length && f & OBJ_LENGTH_READONLY != 0 {
            return Ok(false);
        }
        self.dense_put(o, i, v)?;
        if i >= length {
            self.set_array_length_raw(o, i + 1);
        }
        Ok(true)
    }

    /// Stores `v` in dense slot `i`, growing the elements (with holes) as needed.
    fn dense_put(&mut self, o: Obj, i: u32, v: Value) -> Result<(), HeapError> {
        debug_assert_ne!(self.object_class(o), ObjectClass::Function);
        let mut el = self.elements(o);
        let len = if el == 0 { 0 } else { self.value_array_len(el) };
        if i >= len {
            let grown = self.value_array_reserve(el, i + 1)?;
            if grown != el {
                el = grown;
                self.arena_mut().set(o.0, Object::ELEMENTS, Ref(el));
            }
            for j in len..i {
                self.value_array_set(el, j, Value::HOLE);
            }
            self.arena_mut().set(el, ValueArray::LEN, i + 1);
        }
        self.value_array_set(el, i, v);
        Ok(())
    }

    /// Whether a new element at `i` should go into dense storage.
    fn dense_accepts(&self, o: Obj, i: u32) -> bool {
        self.object_class(o) == ObjectClass::Array
            && i <= atom_int_max()
            && i <= self.dense_len(o).saturating_add(DENSE_GAP)
    }

    // ---------------------------------------------------------------- property storage
    //
    // Named properties live in one of two modes (0083, docs/objects.md 7):
    // - shape mode: `PROPS` is a `PropValues` block (or null, the root shape); keys and
    //   flags are the block's shape's descriptors;
    // - dictionary mode (`OBJ_DICTIONARY`): `PROPS` is a `PropTable` (or null, empty).
    // The `prop_*` functions address properties by slot in either mode: a shape slot is
    // a descriptor index, a dictionary slot a table entry (deleted ones included).
    // Converting to dictionary mode keeps the slots.

    #[inline]
    fn props(&self, o: Obj) -> u32 {
        self.arena().get(o.0, Object::PROPS).0
    }

    #[inline]
    fn is_dictionary(&self, o: Obj) -> bool {
        self.obj_flags(o) & OBJ_DICTIONARY != 0
    }

    /// The flags word a property is stored with (shape descriptor or table entry).
    #[inline]
    fn stored_flags(p: &Property) -> u32 {
        match *p {
            Property::Data { flags, .. } => flags.bits() as u32,
            // Accessors have no writable attribute.
            Property::Accessor { flags, .. } => {
                (flags.bits() & !PropFlags::WRITABLE.bits()) as u32 | STORE_ACCESSOR
            }
        }
    }

    /// A property from its stored flags and value slot.
    #[inline]
    fn decode_property(&self, fl: u32, v: Value) -> Property {
        let flags = PropFlags::from_bits(fl as u8);
        if fl & STORE_ACCESSOR != 0 {
            let pair = v.as_internal().expect("accessor pair");
            let a = self.arena();
            Property::Accessor {
                getter: a.get(pair, AccessorPair::GETTER),
                setter: a.get(pair, AccessorPair::SETTER),
                flags,
            }
        } else {
            Property::Data { value: v, flags }
        }
    }

    /// Property slots of `o` (dictionary slots include deleted entries).
    #[inline]
    fn prop_slots(&self, o: Obj) -> u32 {
        match self.props(o) {
            0 => 0,
            p if self.is_dictionary(o) => self.arena().get(p, PropTable::COUNT),
            p => self.arena().get(p, PropValues::LEN),
        }
    }

    /// The key of slot `i` (null for a deleted dictionary entry).
    #[inline]
    fn prop_key(&self, o: Obj, i: u32) -> Atom {
        let p = self.props(o);
        if self.is_dictionary(o) {
            self.entry_key(p, i)
        } else {
            self.shape_desc(self.arena().get(p, PropValues::SHAPE).0, i)
                .0
        }
    }

    fn prop_read(&self, o: Obj, i: u32) -> Property {
        let p = self.props(o);
        if self.is_dictionary(o) {
            return self.read_entry(p, i);
        }
        let (_, fl) = self.shape_desc(self.arena().get(p, PropValues::SHAPE).0, i);
        self.decode_property(fl, self.arena().read(PropValues::VALUES.at(p, i)))
    }

    #[inline]
    fn prop_find(&self, o: Obj, key: Atom) -> Option<u32> {
        match self.props(o) {
            0 => None,
            t if self.is_dictionary(o) => self.table_find(t, key),
            v => self.shape_find(self.arena().get(v, PropValues::SHAPE).0, key),
        }
    }

    /// Replaces the value of slot `i` if it is a writable data property; false with
    /// nothing changed otherwise.
    #[inline]
    fn prop_set_value(&mut self, o: Obj, i: u32, v: Value) -> bool {
        if self.frozen(o) {
            return false;
        }
        let p = self.props(o);
        let (fl, at) = if self.is_dictionary(o) {
            let e = PropTable::ENTRIES.at(p, i);
            (
                self.arena().get(e, PropEntry::FLAGS),
                e + PropEntry::VALUE.offset,
            )
        } else {
            let s = self.arena().get(p, PropValues::SHAPE).0;
            (self.shape_desc(s, i).1, PropValues::VALUES.at(p, i))
        };
        if fl & STORE_ACCESSOR != 0 || !PropFlags::from_bits(fl as u8).writable() {
            return false;
        }
        self.arena_mut().write(at, v);
        true
    }

    /// Writes slot `i` (holding `key`). In shape mode a change of the attributes (or of
    /// data to accessor) makes the object a dictionary first.
    fn prop_write(&mut self, o: Obj, i: u32, key: Atom, p: &Property) -> Result<(), HeapError> {
        if !self.is_dictionary(o) {
            let v = self.props(o);
            let (_, fl) = self.shape_desc(self.arena().get(v, PropValues::SHAPE).0, i);
            if fl == Self::stored_flags(p) {
                let at = PropValues::VALUES.at(v, i);
                match *p {
                    Property::Data { value, .. } => self.arena_mut().write(at, value),
                    Property::Accessor { getter, setter, .. } => {
                        let pair = self
                            .arena()
                            .read::<Value>(at)
                            .as_internal()
                            .expect("accessor pair");
                        let a = self.arena_mut();
                        a.set(pair, AccessorPair::GETTER, getter);
                        a.set(pair, AccessorPair::SETTER, setter);
                    }
                }
                return Ok(());
            }
            self.become_dictionary(o, 0, Dict::Attributes)?;
        }
        self.write_entry(self.props(o), i, key, p)
    }

    /// Adds a property (the key must not be present): a shape transition, or a table
    /// entry in dictionary mode. Array-index keys make the object a dictionary and set
    /// `OBJ_INDEXED_PROPS`.
    fn prop_insert(&mut self, o: Obj, key: Atom, p: &Property) -> Result<(), HeapError> {
        self.proto_changed(o);
        let index = self.atom_array_index(key).is_some();
        if !self.is_dictionary(o) {
            if !index && self.shape_insert(o, key, p)? {
                return Ok(());
            }
            let why = if index {
                Dict::Index
            } else if self.prop_slots(o) >= SHAPE_MAX {
                Dict::TooMany
            } else {
                Dict::Budget
            };
            self.become_dictionary(o, 1, why)?;
        }
        self.table_insert(o, key, p)?;
        if index {
            self.set_obj_flag(o, OBJ_INDEXED_PROPS, true);
        }
        Ok(())
    }

    /// Adds a property to a shape-mode object; false (nothing changed) when a shape
    /// limit says it should become a dictionary. Allocates before it commits (D1700).
    fn shape_insert(&mut self, o: Obj, key: Atom, p: &Property) -> Result<bool, HeapError> {
        let v = self.props(o);
        let s = match v {
            0 => self.root_shape()?,
            v => self.arena().get(v, PropValues::SHAPE).0,
        };
        let Some(child) = self.shape_child(s, key, Self::stored_flags(p))? else {
            return Ok(false);
        };
        let len = if v == 0 {
            0
        } else {
            self.arena().get(v, PropValues::LEN)
        };
        let nv = if v != 0 && len < PropValues::LAYOUT.tail_capacity(self.cell_size(v)) {
            v
        } else {
            let nv = self.alloc_values((2 * len).max(4))?;
            if len > 0 {
                let src = PropValues::VALUES.at(v, 0);
                let dst = PropValues::VALUES.at(nv, 0);
                self.arena_mut().copy_within(src, 8 * len, dst);
            }
            nv
        };
        let value = self.store_value(p)?;
        let a = self.arena_mut();
        a.write(PropValues::VALUES.at(nv, len), value);
        a.set(nv, PropValues::LEN, len + 1);
        a.set(nv, PropValues::SHAPE, Ref(child));
        a.set(o.0, Object::PROPS, Ref(nv));
        Ok(true)
    }

    /// The value slot of a new property: the value, or a new accessor pair.
    fn store_value(&mut self, p: &Property) -> Result<Value, HeapError> {
        Ok(match *p {
            Property::Data { value, .. } => value,
            Property::Accessor { getter, setter, .. } => {
                let pair =
                    self.alloc_cell(CellKind::AccessorPair, AccessorPair::LAYOUT.min_size)?;
                let a = self.arena_mut();
                a.set(pair, AccessorPair::GETTER, getter);
                a.set(pair, AccessorPair::SETTER, setter);
                Value::internal(pair)
            }
        })
    }

    fn alloc_values(&mut self, cap: u32) -> Result<u32, HeapError> {
        let size = PropValues::VALUES
            .cell_size(cap)
            .ok_or(HeapError::OutOfMemory)?;
        self.alloc_cell(CellKind::PropValues, size)
    }

    /// Removes slot `i`. A shape-mode object goes back to the parent shape if `i` is its
    /// last property, else it becomes a dictionary first.
    fn prop_remove(&mut self, o: Obj, i: u32) -> Result<(), HeapError> {
        if !self.is_dictionary(o) {
            let v = self.props(o);
            let len = self.arena().get(v, PropValues::LEN);
            if i + 1 == len {
                let s = self.arena().get(v, PropValues::SHAPE).0;
                let parent = self.shape_parent(s);
                let a = self.arena_mut();
                a.write(PropValues::VALUES.at(v, i), Value::ZERO);
                a.set(v, PropValues::LEN, i);
                a.set(v, PropValues::SHAPE, Ref(parent));
                return Ok(());
            }
            if let Some(s) = self.shape_without(o, i)? {
                let a = self.arena_mut();
                let at = PropValues::VALUES.at(v, i);
                a.copy_within(at + 8, 8 * (len - i - 1), at);
                a.write(PropValues::VALUES.at(v, len - 1), Value::ZERO);
                a.set(v, PropValues::LEN, len - 1);
                a.set(v, PropValues::SHAPE, Ref(s));
                return Ok(());
            }
            self.become_dictionary(o, 0, Dict::Delete)?;
        }
        self.table_remove(o, i);
        Ok(())
    }

    /// The shape of shape-mode `o` without its property `i` (not the last): the
    /// ancestor before `i`, then the transitions for the keys after it. At most one of
    /// them may be new, so deleting keys of map-like objects does not spend the shape
    /// budget on every subset of their keys; `None` means "become a dictionary".
    fn shape_without(&mut self, o: Obj, i: u32) -> Result<Option<u32>, HeapError> {
        let s = self.arena().get(self.props(o), PropValues::SHAPE).0;
        let count = self.shape_count(s);
        let mut base = s;
        for _ in i..count {
            base = self.shape_parent(base);
        }
        let mut created = false;
        for j in i + 1..count {
            let (key, flags) = self.shape_desc(s, j);
            base = match self.shape_transition(base, key, flags) {
                Some(c) => c,
                None if !created => {
                    created = true;
                    match self.shape_child(base, key, flags)? {
                        Some(c) => c,
                        None => return Ok(None),
                    }
                }
                None => return Ok(None),
            };
        }
        Ok(Some(base))
    }

    /// Makes `o` a dictionary-mode object (builtin namespaces and prototypes: one-off
    /// objects, for which a private table is smaller than a chain of unshared shapes).
    pub(crate) fn make_dictionary(&mut self, o: Obj) -> Result<(), HeapError> {
        if self.is_dictionary(o) {
            return Ok(());
        }
        self.become_dictionary(o, 0, Dict::Builtin)
    }

    /// Converts a shape-mode object to dictionary mode with room for `extra` more
    /// entries. The slots stay the same.
    fn become_dictionary(&mut self, o: Obj, extra: u32, why: Dict) -> Result<(), HeapError> {
        debug_assert!(!self.is_dictionary(o));
        #[cfg(test)]
        {
            self.dict_reasons[why as usize] += 1;
        }
        let _ = why;
        let v = self.props(o);
        let n = if v == 0 {
            0
        } else {
            self.arena().get(v, PropValues::LEN)
        };
        let mut t = 0;
        if n > 0 {
            t = self.alloc_table((n + extra).max(4))?;
            let s = self.arena().get(v, PropValues::SHAPE).0;
            for i in 0..n {
                let (key, fl) = self.shape_desc(s, i);
                let value: Value = self.arena().read(PropValues::VALUES.at(v, i));
                let e = PropTable::ENTRIES.at(t, i);
                let a = self.arena_mut();
                a.set(e, PropEntry::KEY, key.id());
                a.set(e, PropEntry::FLAGS, fl);
                a.set(e, PropEntry::VALUE, value);
            }
            self.arena_mut().set(t, PropTable::COUNT, n);
            if n > LINEAR_MAX {
                self.build_index(t)?;
            }
        }
        self.arena_mut().set(o.0, Object::PROPS, Ref(t));
        self.set_obj_flag(o, OBJ_DICTIONARY, true);
        Ok(())
    }

    // ---------------------------------------------------------------- dictionaries

    #[inline]
    fn entry_key(&self, t: u32, i: u32) -> Atom {
        Atom::from_id(
            self.arena()
                .get(PropTable::ENTRIES.at(t, i), PropEntry::KEY),
        )
    }

    fn table_capacity(&self, t: u32) -> u32 {
        PropTable::LAYOUT.tail_capacity(self.cell_size(t))
    }

    fn table_find(&self, t: u32, key: Atom) -> Option<u32> {
        let a = self.arena();
        let count = a.get(t, PropTable::COUNT);
        let index = a.get(t, PropTable::INDEX).0;
        if index == 0 {
            return (0..count).find(|&i| self.entry_key(t, i) == key);
        }
        self.probe(index, key_hash(key), |_, e| match e {
            0 => Some(None),
            TOMBSTONE => None,
            e => (self.entry_key(t, e - 1) == key).then_some(Some(e - 1)),
        })
    }

    // ---------------------------------------------------------------- inline caches

    /// The slot of own property `key` of `o`, if it is stored as a named property (not a
    /// dense element, not an array's `length`): the index an inline cache remembers
    /// (D1620). Only ordinary storage: callers check the class.
    #[inline]
    pub fn own_entry_index(&self, o: Obj, key: Atom) -> Option<u32> {
        self.prop_find(o, key)
    }

    /// The value of slot `i` of `o` if that slot is the data property `key` (an inline
    /// cache hit, D1620); `None` for anything else, including a stale index. Keys are
    /// never array indexes of arrays here, since those callers only cache non-index atoms.
    #[inline]
    pub fn data_at(&self, o: Obj, i: u32, key: Atom) -> Option<Value> {
        let p = self.props(o);
        if p == 0 {
            return None;
        }
        let a = self.arena();
        let (k, fl, at) = if self.is_dictionary(o) {
            if i >= a.get(p, PropTable::COUNT) {
                return None;
            }
            let e = PropTable::ENTRIES.at(p, i);
            (
                a.get(e, PropEntry::KEY),
                a.get(e, PropEntry::FLAGS),
                e + PropEntry::VALUE.offset,
            )
        } else {
            if i >= a.get(p, PropValues::LEN) {
                return None;
            }
            let (k, fl) = self.shape_desc(a.get(p, PropValues::SHAPE).0, i);
            (k.id(), fl, PropValues::VALUES.at(p, i))
        };
        if k != key.id() || fl & STORE_ACCESSOR != 0 {
            return None;
        }
        Some(a.read(at))
    }

    pub(crate) fn getter_at(&self, o: Obj, i: u32, key: Atom) -> Option<Value> {
        let p = self.props(o);
        if p == 0 {
            return None;
        }
        let a = self.arena();
        let (k, fl, at) = if self.is_dictionary(o) {
            if i >= a.get(p, PropTable::COUNT) {
                return None;
            }
            let e = PropTable::ENTRIES.at(p, i);
            (
                a.get(e, PropEntry::KEY),
                a.get(e, PropEntry::FLAGS),
                e + PropEntry::VALUE.offset,
            )
        } else {
            if i >= a.get(p, PropValues::LEN) {
                return None;
            }
            let (k, fl) = self.shape_desc(a.get(p, PropValues::SHAPE).0, i);
            (k.id(), fl, PropValues::VALUES.at(p, i))
        };
        if k != key.id() || fl & STORE_ACCESSOR == 0 {
            return None;
        }
        let pair = a.read::<Value>(at).as_internal()?;
        Some(a.get(pair, AccessorPair::GETTER))
    }

    /// Writes `v` to slot `i` of `o` if it is the writable data property `key` (an inline
    /// cache hit of a store, D1620); false with nothing changed otherwise.
    #[inline]
    pub fn set_data_at(&mut self, o: Obj, i: u32, key: Atom, v: Value) -> bool {
        if i >= self.prop_slots(o) || self.prop_key(o, i) != key {
            return false;
        }
        self.prop_set_value(o, i, v)
    }

    /// For a shape-mode object whose own `key` is a data property: its shape, its slot
    /// and whether it is writable (what an own-property inline cache remembers, 0083).
    pub fn shape_slot(&self, o: Obj, key: Atom) -> Option<(u32, u32, bool)> {
        let v = self.props(o);
        if v == 0 || self.is_dictionary(o) {
            return None;
        }
        let s = self.arena().get(v, PropValues::SHAPE).0;
        let i = self.shape_find(s, key)?;
        let fl = self.shape_desc(s, i).1;
        (fl & STORE_ACCESSOR == 0).then(|| (s, i, PropFlags::from_bits(fl as u8).writable()))
    }

    /// The receiver shape word of an inline cache (0083): 0 for a dictionary-mode object
    /// (nothing to compare), [`EMPTY_SHAPE`] for a shape-mode object without a values
    /// block, else its shape. Equal words mean equal named keys and attributes.
    #[inline(always)]
    pub fn receiver_shape(&self, o: Obj) -> u32 {
        if self.is_dictionary(o) {
            return 0;
        }
        match self.props(o) {
            0 => EMPTY_SHAPE,
            v => self.arena().get(v, PropValues::SHAPE).0,
        }
    }

    /// After a slow path added `key` to `o`, whose receiver shape word was `old`: the new
    /// shape if the addition was the transition from `old` by `key` as a writable,
    /// enumerable, configurable data property, the one a cached addition can repeat.
    /// `None` for anything else, and for objects marked as prototypes (their additions
    /// must bump the prototype epoch).
    pub fn added_shape(&self, o: Obj, key: Atom, old: u32) -> Option<u32> {
        if old == 0 || self.obj_flags(o) & OBJ_IS_PROTO != 0 {
            return None;
        }
        let new = self.receiver_shape(o);
        if new <= EMPTY_SHAPE {
            return None;
        }
        let count = self.shape_count(new);
        let from_old = if old == EMPTY_SHAPE {
            count == 1
        } else {
            self.shape_parent(new) == old
        };
        (from_old && self.shape_desc(new, count - 1) == (key, PropFlags::ALL.bits() as u32))
            .then_some(new)
    }

    /// A cached addition (0083): gives `o` the shape `new` (a child of `old` by one
    /// writable, enumerable, configurable data property) with value `v` in the new slot,
    /// if `o` has receiver shape word `old`, is extensible and not marked as a prototype.
    /// The caller has checked what lies outside the object (the prototype chain for
    /// [[Set]]). Grows the values block if it is full; false, with nothing changed, if
    /// that allocation fails (the slow path then fails and retries, D1700) or `o` does not
    /// match.
    #[inline]
    pub fn shaped_add(&mut self, o: Obj, old: u32, new: u32, v: Value) -> bool {
        let f = self.obj_flags(o);
        // A lazy `length` or `name` (D2445) is not in the shape: the cached key could be
        // one of them.
        if f & (OBJ_DICTIONARY | OBJ_IS_PROTO | OBJ_LAZY) != 0
            || f & OBJ_EXTENSIBLE == 0
            || self.frozen(o)
        {
            return false;
        }
        let p = self.props(o);
        let (len, cur) = if p == 0 {
            (0, EMPTY_SHAPE)
        } else {
            let a = self.arena();
            (a.get(p, PropValues::LEN), a.get(p, PropValues::SHAPE).0)
        };
        if cur != old {
            return false;
        }
        let nv = if p != 0 && len < PropValues::LAYOUT.tail_capacity(self.cell_size(p)) {
            p
        } else {
            let Ok(nv) = self.alloc_values((2 * len).max(4)) else {
                return false;
            };
            if len > 0 {
                let src = PropValues::VALUES.at(p, 0);
                let dst = PropValues::VALUES.at(nv, 0);
                self.arena_mut().copy_within(src, 8 * len, dst);
            }
            nv
        };
        let a = self.arena_mut();
        a.write(PropValues::VALUES.at(nv, len), v);
        a.set(nv, PropValues::LEN, len + 1);
        a.set(nv, PropValues::SHAPE, Ref(new));
        a.set(o.0, Object::PROPS, Ref(nv));
        true
    }

    /// The values block of `o` if it is in shape mode with shape `shape`.
    #[inline(always)]
    fn values_if_shape(&self, o: Obj, shape: u32) -> Option<u32> {
        if self.is_dictionary(o) {
            return None;
        }
        let v = self.props(o);
        (v != 0 && self.arena().get(v, PropValues::SHAPE).0 == shape).then_some(v)
    }

    /// Value slot `slot` of `o` if `o` has shape `shape` (an inline cache hit: the shape
    /// fixes the key and its attributes).
    #[inline(always)]
    pub fn shaped_get(&self, o: Obj, shape: u32, slot: u32) -> Option<Value> {
        let v = self.values_if_shape(o, shape)?;
        Some(self.arena().read(PropValues::VALUES.at(v, slot)))
    }

    /// Writes value slot `slot` of `o` if `o` has shape `shape`; the caller knows from the
    /// shape that the slot is a writable data property.
    #[inline(always)]
    pub fn shaped_set(&mut self, o: Obj, shape: u32, slot: u32, value: Value) -> bool {
        // A frozen builtin can share its shape with a session object (D2304).
        if self.frozen(o) {
            return false;
        }
        let Some(v) = self.values_if_shape(o, shape) else {
            return false;
        };
        self.arena_mut()
            .write(PropValues::VALUES.at(v, slot), value);
        true
    }

    fn read_entry(&self, t: u32, i: u32) -> Property {
        let e = PropTable::ENTRIES.at(t, i);
        let a = self.arena();
        self.decode_property(a.get(e, PropEntry::FLAGS), a.get(e, PropEntry::VALUE))
    }

    /// Writes entry i. An accessor reuses the entry's pair if it has one, else allocates
    /// one first, so a failed allocation changes nothing (D1700).
    fn write_entry(&mut self, t: u32, i: u32, key: Atom, p: &Property) -> Result<(), HeapError> {
        let e = PropTable::ENTRIES.at(t, i);
        let value = match *p {
            Property::Data { value, .. } => value,
            Property::Accessor { getter, setter, .. } => {
                let a = self.arena();
                let old = (a.get(e, PropEntry::KEY) == key.id()
                    && a.get(e, PropEntry::FLAGS) & STORE_ACCESSOR != 0)
                    .then(|| a.get::<Value>(e, PropEntry::VALUE).as_internal())
                    .flatten();
                match old {
                    Some(pair) => {
                        let a = self.arena_mut();
                        a.set(pair, AccessorPair::GETTER, getter);
                        a.set(pair, AccessorPair::SETTER, setter);
                        Value::internal(pair)
                    }
                    None => self.store_value(p)?,
                }
            }
        };
        let a = self.arena_mut();
        a.set(e, PropEntry::KEY, key.id());
        a.set(e, PropEntry::FLAGS, Self::stored_flags(p));
        a.set(e, PropEntry::VALUE, value);
        Ok(())
    }

    fn alloc_table(&mut self, cap: u32) -> Result<u32, HeapError> {
        let size = PropTable::ENTRIES
            .cell_size(cap)
            .ok_or(HeapError::OutOfMemory)?;
        self.alloc_cell(CellKind::PropTable, size)
    }

    /// A hash index for the base image's table `t`, which is full and rarely grows (D2450):
    /// sized for a load of at most 3/4 rather than 1/2 of the capacity.
    pub(super) fn build_table_index_dense(&mut self, t: u32) -> Result<(), HeapError> {
        let count = self.arena().get(t, PropTable::COUNT);
        self.build_index_with(t, (4 * count).div_ceil(3).next_power_of_two())
    }

    fn build_index(&mut self, t: u32) -> Result<(), HeapError> {
        self.build_index_with(t, (self.table_capacity(t) * 2).next_power_of_two())
    }

    fn build_index_with(&mut self, t: u32, buckets: u32) -> Result<(), HeapError> {
        let index = self.alloc_u32_array(buckets)?;
        for i in 0..self.arena().get(t, PropTable::COUNT) {
            let key = self.entry_key(t, i);
            if key.is_null() {
                continue;
            }
            let b = self.probe_empty(index, key_hash(key));
            self.u32_array_set(index, b, i + 1);
        }
        self.arena_mut().set(t, PropTable::INDEX, Ref(index));
        Ok(())
    }

    /// Copies the live entries of `t` (or nothing) into a new, larger table for `o`.
    fn grow_table(&mut self, o: Obj, t: u32) -> Result<u32, HeapError> {
        let (count, deleted) = if t == 0 {
            (0, 0)
        } else {
            let a = self.arena();
            (a.get(t, PropTable::COUNT), a.get(t, PropTable::DELETED))
        };
        let live = count - deleted;
        let cap = (live * 2).max(4);
        let new = self.alloc_table(cap)?;
        let mut n = 0;
        for i in 0..count {
            if self.entry_key(t, i).is_null() {
                continue;
            }
            let src = PropTable::ENTRIES.at(t, i);
            let dst = PropTable::ENTRIES.at(new, n);
            self.arena_mut()
                .copy_within(src, PropTable::ENTRIES.elem_size(), dst);
            n += 1;
        }
        self.arena_mut().set(new, PropTable::COUNT, n);
        if n > LINEAR_MAX {
            self.build_index(new)?;
        }
        self.arena_mut().set(o.0, Object::PROPS, Ref(new));
        Ok(new)
    }

    /// Appends a property (the key must not be present).
    fn table_insert(&mut self, o: Obj, key: Atom, p: &Property) -> Result<(), HeapError> {
        let mut t = self.props(o);
        if t == 0 || self.arena().get(t, PropTable::COUNT) == self.table_capacity(t) {
            t = self.grow_table(o, t)?;
        }
        let count = self.arena().get(t, PropTable::COUNT);
        let mut index = self.arena().get(t, PropTable::INDEX).0;
        if index == 0 && count + 1 > LINEAR_MAX {
            self.build_index(t)?;
            index = self.arena().get(t, PropTable::INDEX).0;
        }
        self.write_entry(t, count, key, p)?;
        self.arena_mut().set(t, PropTable::COUNT, count + 1);
        if index != 0 {
            let b = self.probe(index, key_hash(key), |b, v| {
                matches!(v, 0 | TOMBSTONE).then_some(b)
            });
            self.u32_array_set(index, b, count + 1);
        }
        Ok(())
    }

    /// Removes entry `i` of `o`'s table.
    fn table_remove(&mut self, o: Obj, i: u32) {
        let t = self.props(o);
        let count = self.arena().get(t, PropTable::COUNT);
        let index = self.arena().get(t, PropTable::INDEX).0;
        let esize = PropTable::ENTRIES.elem_size();
        if index == 0 {
            // Linear mode stays compact, preserving insertion order.
            let src = PropTable::ENTRIES.at(t, i + 1);
            let dst = PropTable::ENTRIES.at(t, i);
            self.arena_mut()
                .copy_within(src, (count - i - 1) * esize, dst);
            self.arena_mut()
                .zero(PropTable::ENTRIES.at(t, count - 1), esize);
            self.arena_mut().set(t, PropTable::COUNT, count - 1);
            return;
        }
        let key = self.entry_key(t, i);
        let b = self.probe(index, key_hash(key), |b, v| (v == i + 1).then_some(b));
        self.u32_array_set(index, b, TOMBSTONE);
        self.arena_mut().zero(PropTable::ENTRIES.at(t, i), esize);
        let deleted = self.arena().get(t, PropTable::DELETED);
        self.arena_mut().set(t, PropTable::DELETED, deleted + 1);
    }

    // ---------------------------------------------------------------- own properties

    fn find_own(&self, o: Obj, key: Atom) -> Loc {
        if let Some(i) = key.as_int() {
            if self.get_element_fast(o, i).is_some() {
                return Loc::Dense(i);
            }
            if self.obj_flags(o) & OBJ_INDEXED_PROPS == 0 {
                return Loc::None;
            }
        } else if (key == atoms::LENGTH || key == atoms::NAME)
            && let Some(loc) = self.unstored_loc(o, key)
        {
            return loc;
        }
        self.prop_find(o, key).map_or(Loc::None, Loc::Prop)
    }

    /// The location of an own `length` or `name` that has no table entry: an array's
    /// `length`, a closure's lazy ones (D2445). Out of line: `find_own` is inlined into
    /// many callers, and only these two keys get here.
    #[inline(never)]
    fn unstored_loc(&self, o: Obj, key: Atom) -> Option<Loc> {
        let f = self.obj_flags(o);
        if key == atoms::LENGTH {
            if f & OBJ_LAZY_LENGTH != 0 {
                return Some(Loc::LazyLength);
            }
            (self.object_class(o) == ObjectClass::Array).then_some(Loc::ArrayLength)
        } else {
            (f & OBJ_LAZY_NAME != 0).then_some(Loc::LazyName)
        }
    }

    /// The value of a closure's lazy `length` or `name` (D2445): the template's `LENGTH`,
    /// the string of its `NAME` atom (empty for none).
    fn lazy_value(&self, o: Obj, loc: Loc) -> Value {
        // A builtin closure's `length` is its slot 2, its `name` empty (D3155).
        if self.object_class(o) == ObjectClass::BuiltinClosure {
            if loc == Loc::LazyLength {
                return self.object_slot(o, 2);
            }
            return Value::string(self.atom_str_value(atoms::EMPTY).expect("the empty atom"));
        }
        let t = self.closure_template(o);
        if loc == Loc::LazyLength {
            return Value::int(self.arena().get(t, FuncTemplate::LENGTH) as i32);
        }
        let name = Atom::from_id(self.arena().get(t, FuncTemplate::NAME));
        let name = if name.is_null() { atoms::EMPTY } else { name };
        Value::string(self.atom_str_value(name).expect("a string atom name"))
    }

    /// Gives closure `o` its `length` and `name` as lazy properties (D2445), if its
    /// template's name is a string atom (or none): what defining them as
    /// `{writable: false, enumerable: false, configurable: true}` would do, before any
    /// other key. False, with nothing changed, otherwise.
    pub(crate) fn set_lazy_fn_props(&mut self, o: Obj) -> bool {
        debug_assert!(self.object_class(o) == ObjectClass::Function && self.prop_slots(o) == 0);
        let t = self.closure_template(o);
        let name = Atom::from_id(self.arena().get(t, FuncTemplate::NAME));
        if !name.is_null() && self.atom_str_value(name).is_none() {
            return false;
        }
        self.set_obj_flag(o, OBJ_LAZY, true);
        true
    }

    /// Gives builtin closure `o` (no stored keys yet) its `length` (slot 2) and empty
    /// `name` as lazy properties (D3155), as defining them non-writable, non-enumerable
    /// and configurable before any other key would.
    pub(crate) fn set_lazy_builtin_closure_props(&mut self, o: Obj) {
        debug_assert!(
            self.object_class(o) == ObjectClass::BuiltinClosure && self.prop_slots(o) == 0
        );
        self.set_obj_flag(o, OBJ_LAZY, true);
    }

    /// Stores a closure's lazy `length` and `name` (D2445) as ordinary properties, first
    /// in key order as they are: before a change to one of them other than deleting it.
    /// The object becomes a dictionary (rare: a redefined `name` or `length`), whose
    /// entries move up to make room at the front. Allocates before it commits (D1700).
    #[cold]
    #[inline(never)]
    fn materialize_lazy(&mut self, o: Obj) -> Result<(), HeapError> {
        let f = self.obj_flags(o);
        let configurable = PropFlags::CONFIGURABLE.bits() as u32;
        let mut first = [(Atom::NULL, Value::UNDEFINED); 2];
        let mut k = 0;
        if f & OBJ_LAZY_LENGTH != 0 {
            first[0] = (atoms::LENGTH, self.lazy_value(o, Loc::LazyLength));
            k = 1;
        }
        if f & OBJ_LAZY_NAME != 0 {
            first[k as usize] = (atoms::NAME, self.lazy_value(o, Loc::LazyName));
            k += 1;
        }
        if !self.is_dictionary(o) {
            self.become_dictionary(o, k, Dict::Attributes)?;
        }
        let mut t = self.props(o);
        if t == 0 || self.arena().get(t, PropTable::COUNT) + k > self.table_capacity(t) {
            t = self.grow_table(o, t)?;
        }
        let count = self.arena().get(t, PropTable::COUNT);
        if self.arena().get::<Ref>(t, PropTable::INDEX).is_null() && count + k > LINEAR_MAX {
            self.build_index(t)?;
        }
        // Nothing allocates from here on.
        let esize = PropTable::ENTRIES.elem_size();
        let (src, dst) = (PropTable::ENTRIES.at(t, 0), PropTable::ENTRIES.at(t, k));
        self.arena_mut().copy_within(src, esize * count, dst);
        for (i, &(key, value)) in first[..k as usize].iter().enumerate() {
            let e = PropTable::ENTRIES.at(t, i as u32);
            let a = self.arena_mut();
            a.set(e, PropEntry::KEY, key.id());
            a.set(e, PropEntry::FLAGS, configurable);
            a.set(e, PropEntry::VALUE, value);
        }
        self.arena_mut().set(t, PropTable::COUNT, count + k);
        if let Some(index) = self.arena().get::<Ref>(t, PropTable::INDEX).get() {
            for b in 0..self.u32_array_len(index) {
                let v = self.u32_array_get(index, b);
                if v != 0 && v != TOMBSTONE {
                    self.u32_array_set(index, b, v + k);
                }
            }
            for (i, &(key, _)) in first[..k as usize].iter().enumerate() {
                let b = self.probe(index, key_hash(key), |b, v| {
                    matches!(v, 0 | TOMBSTONE).then_some(b)
                });
                self.u32_array_set(index, b, i as u32 + 1);
            }
        }
        self.set_obj_flag(o, OBJ_LAZY, false);
        Ok(())
    }

    /// For the base image (D2445): if builtin closure `o`'s first two stored properties
    /// are the `length` and `name` that `%Install*` defined (non-writable,
    /// non-enumerable, configurable; an int32 length up to `u16::MAX`, a string name
    /// that is a string atom already), moves them to its template (`LENGTH`, `NAME` and
    /// `func_flags::LAZY_NAME`) and makes them lazy. The caller checked that no other
    /// closure shares the template. False, with nothing changed, otherwise.
    pub(crate) fn make_builtin_props_lazy(&mut self, o: Obj) -> Result<bool, HeapError> {
        use crate::bytecode::template::func_flags;
        if self.object_class(o) != ObjectClass::Function
            || self.obj_flags(o) & OBJ_LAZY != 0
            || self.prop_slots(o) < 2
            || self.prop_key(o, 0) != atoms::LENGTH
            || self.prop_key(o, 1) != atoms::NAME
        {
            return Ok(false);
        }
        let (
            Property::Data {
                value: v0,
                flags: f0,
            },
            Property::Data {
                value: v1,
                flags: f1,
            },
        ) = (self.prop_read(o, 0), self.prop_read(o, 1))
        else {
            return Ok(false);
        };
        let length = v0.as_int().and_then(|l| u16::try_from(l).ok());
        let (Some(length), Some(name), true) = (
            length,
            v1.as_string(),
            f0 == PropFlags::CONFIGURABLE && f1 == PropFlags::CONFIGURABLE,
        ) else {
            return Ok(false);
        };
        // Only names that are atoms already (property keys, mostly): a new atom costs
        // more than the property it saves, and can grow the atom index.
        let Some(atom) = self.find_string_atom_of(name) else {
            return Ok(false);
        };
        for key in [atoms::LENGTH, atoms::NAME] {
            let i = self.prop_find(o, key).expect("checked above");
            self.prop_remove(o, i)?;
        }
        if !self.is_dictionary(o) && self.prop_slots(o) == 0 {
            // No stored keys left: no values block either (the root shape).
            self.arena_mut().set(o.0, Object::PROPS, Ref::NULL);
        }
        let t = self.closure_template(o);
        let a = self.arena_mut();
        let flags = u32::from(a.get::<u16>(t, FuncTemplate::FLAGS));
        a.set(
            t,
            FuncTemplate::FLAGS,
            (flags | func_flags::LAZY_NAME) as u16,
        );
        a.set(t, FuncTemplate::NAME, atom.id());
        a.set(t, FuncTemplate::LENGTH, length);
        self.set_obj_flag(o, OBJ_LAZY, true);
        Ok(true)
    }

    fn read_loc(&self, o: Obj, loc: Loc) -> Option<Property> {
        let p = self.read_loc_stored(o, loc)?;
        if !self.frozen(o) {
            return Some(p);
        }
        // A frozen builtin's properties read as non-writable and non-configurable (D2304).
        let e = p.flags().enumerable();
        Some(match p {
            Property::Data { value, .. } => Property::data(value, PropFlags::new(false, e, false)),
            Property::Accessor { getter, setter, .. } => Property::Accessor {
                getter,
                setter,
                flags: PropFlags::new(false, e, false),
            },
        })
    }

    /// [`Heap::read_loc`] as stored.
    #[inline(always)]
    fn read_loc_stored(&self, o: Obj, loc: Loc) -> Option<Property> {
        match loc {
            Loc::None => None,
            Loc::Dense(i) => Some(Property::data(
                self.value_array_get(self.elements(o), i),
                PropFlags::ALL,
            )),
            Loc::Prop(i) => Some(self.prop_read(o, i)),
            Loc::ArrayLength => Some(self.array_length_property(o)),
            Loc::LazyLength | Loc::LazyName => Some(Property::data(
                self.lazy_value(o, loc),
                PropFlags::CONFIGURABLE,
            )),
        }
    }

    fn array_length_property(&self, o: Obj) -> Property {
        let writable = self.obj_flags(o) & OBJ_LENGTH_READONLY == 0;
        Property::data(
            Value::number(self.array_length(o) as f64),
            PropFlags::new(writable, false, false),
        )
    }

    /// [[Set]] on an own writable data property, which only replaces its value
    /// (OrdinarySet's DefineOwnProperty with `{ [[Value]]: v }`). Returns false, with
    /// nothing changed, for anything else (absent, an accessor, read-only, an array's
    /// `length`), which needs the full algorithm. The interpreter's hot path (0035).
    pub fn set_own_data_value(&mut self, o: Obj, key: Atom, v: Value) -> bool {
        if self.object_class(o) == ObjectClass::TypedArray
            && self.canonical_numeric_index(key).is_some()
        {
            // [[Set]] on a typed array: true even for an invalid index (nothing written),
            // once the value is a primitive. An immutable one fails (the caller's error).
            if !self.ta_immutable(o) && self.element_of(self.ta_kind(o), v).is_some() {
                self.ta_numeric_set(o, key, v);
                return true;
            }
            return false;
        }
        if self.frozen(o) {
            return false;
        }
        match self.find_own(o, key) {
            Loc::Dense(i) => {
                let el = self.elements(o);
                self.value_array_set(el, i, v);
                true
            }
            Loc::Prop(i) => self.prop_set_value(o, i, v),
            Loc::None | Loc::ArrayLength | Loc::LazyLength | Loc::LazyName => false,
        }
    }

    /// `[[GetOwnProperty]]`.
    pub fn get_own_property(&self, o: Obj, key: Atom) -> Option<Property> {
        if self.object_class(o) == ObjectClass::TypedArray
            && let Some(r) = self.ta_numeric_get(o, key)
        {
            // An immutable buffer's elements are read-only and non-configurable (D1970).
            let flags = if self.ta_immutable(o) {
                PropFlags::new(false, true, false)
            } else {
                PropFlags::ALL
            };
            return r.map(|v| Property::data(v, flags));
        }
        self.read_loc(o, self.find_own(o, key))
    }

    pub fn has_own_property(&self, o: Obj, key: Atom) -> bool {
        if self.object_class(o) == ObjectClass::TypedArray
            && let Some(r) = self.ta_numeric_get(o, key)
        {
            return r.is_some();
        }
        self.find_own(o, key) != Loc::None
    }

    /// Looks `key` up along the prototype chain. Returns the object that has it and the
    /// property; for an accessor the caller calls the getter with the original receiver.
    /// A Proxy on the chain ends the walk as if it were an empty object without a
    /// prototype: callers that can meet one use [`Heap::lookup_property`].
    pub fn find_property(&self, o: Obj, key: Atom) -> Option<(Obj, Property)> {
        match self.lookup_property(o, key) {
            Lookup::Found(h, p) => Some((h, p)),
            Lookup::Absent | Lookup::Proxy(_) => None,
        }
    }

    /// [`Heap::find_property`] that reports a Proxy on the chain (`o` itself or a
    /// prototype) before the key was found: the rest of the lookup is its traps'.
    #[inline]
    pub fn lookup_property(&self, o: Obj, key: Atom) -> Lookup {
        let mut c = o;
        loop {
            if let Some(p) = self.get_own_property(c, key) {
                return Lookup::Found(c, p);
            }
            match self.object_class(c) {
                // A typed array answers numeric keys itself: no prototype lookup.
                ObjectClass::TypedArray if self.canonical_numeric_index(key).is_some() => {
                    return Lookup::Absent;
                }
                ObjectClass::Proxy => return Lookup::Proxy(c),
                _ => {}
            }
            match self.get_prototype_of(c) {
                Some(p) => c = p,
                None => return Lookup::Absent,
            }
        }
    }

    /// `[[HasProperty]]`, with a Proxy on the chain counting as absent: callers that can
    /// meet one use [`Heap::has_property_or_proxy`].
    pub fn has_property(&self, o: Obj, key: Atom) -> bool {
        self.has_property_or_proxy(o, key) == Ok(true)
    }

    /// `[[HasProperty]]` up to a Proxy on the chain: `Err(proxy)` when the answer is its
    /// `has` trap's.
    #[inline]
    pub fn has_property_or_proxy(&self, o: Obj, key: Atom) -> Result<bool, Obj> {
        let mut c = o;
        loop {
            if self.has_own_property(c, key) {
                return Ok(true);
            }
            match self.object_class(c) {
                ObjectClass::TypedArray if self.canonical_numeric_index(key).is_some() => {
                    return Ok(false);
                }
                ObjectClass::Proxy => return Err(c),
                _ => {}
            }
            match self.get_prototype_of(c) {
                Some(p) => c = p,
                None => return Ok(false),
            }
        }
    }

    /// Stores a complete property at `loc` (where `key` currently is, or `Loc::None`).
    fn write_property(
        &mut self,
        o: Obj,
        key: Atom,
        loc: Loc,
        p: Property,
    ) -> Result<(), HeapError> {
        let dense_ok = matches!(p, Property::Data { flags, .. } if flags == PropFlags::ALL);
        match loc {
            Loc::Dense(i) => {
                if let (true, Property::Data { value, .. }) = (dense_ok, p) {
                    self.value_array_set(self.elements(o), i, value);
                } else {
                    // Non-default attributes: move the element to the table.
                    self.prop_insert(o, key, &p)?;
                    self.value_array_set(self.elements(o), i, Value::HOLE);
                }
            }
            Loc::Prop(i) => self.prop_write(o, i, key, &p)?,
            Loc::ArrayLength => unreachable!("array length is written by array_set_length"),
            Loc::LazyLength | Loc::LazyName => {
                self.materialize_lazy(o)?;
                let Loc::Prop(i) = self.find_own(o, key) else {
                    unreachable!("materialized")
                };
                self.prop_write(o, i, key, &p)?;
            }
            Loc::None => {
                if let (true, Some(i), Property::Data { value, .. }) = (dense_ok, key.as_int(), p)
                    && self.dense_accepts(o, i)
                {
                    self.dense_put(o, i, value)?;
                } else {
                    self.prop_insert(o, key, &p)?;
                }
            }
        }
        Ok(())
    }

    /// SameValue.
    pub fn same_value(&self, a: Value, b: Value) -> bool {
        if let (Some(x), Some(y)) = (a.as_number(), b.as_number()) {
            return (x.is_nan() && y.is_nan())
                || (x == y && x.is_sign_negative() == y.is_sign_negative());
        }
        if let (Some(x), Some(y)) = (a.as_string(), b.as_string()) {
            return self.str_eq(x, y);
        }
        if a.as_bigint().is_some() && b.as_bigint().is_some() {
            return self.bigint_eq(a, b);
        }
        a == b
    }

    /// ValidateAndApplyPropertyDescriptor, without the write.
    fn validate_descriptor(
        &self,
        current: Option<Property>,
        extensible: bool,
        d: &PropertyDescriptor,
    ) -> Outcome {
        debug_assert!(!(d.is_accessor() && d.is_data()));
        let undef = Value::UNDEFINED;
        let Some(cur) = current else {
            if !extensible {
                return Outcome::Reject;
            }
            let e = d.enumerable.unwrap_or(false);
            let c = d.configurable.unwrap_or(false);
            return Outcome::Write(if d.is_accessor() {
                Property::Accessor {
                    getter: d.get.unwrap_or(undef),
                    setter: d.set.unwrap_or(undef),
                    flags: PropFlags::new(false, e, c),
                }
            } else {
                Property::Data {
                    value: d.value.unwrap_or(undef),
                    flags: PropFlags::new(d.writable.unwrap_or(false), e, c),
                }
            });
        };
        if d.is_empty() {
            return Outcome::Keep;
        }
        let cf = cur.flags();
        if !cf.configurable() {
            if d.configurable == Some(true) {
                return Outcome::Reject;
            }
            if d.enumerable.is_some_and(|e| e != cf.enumerable()) {
                return Outcome::Reject;
            }
            if !d.is_generic() && d.is_accessor() != cur.is_accessor() {
                return Outcome::Reject;
            }
            match cur {
                Property::Accessor { getter, setter, .. } => {
                    if d.get.is_some_and(|g| !self.same_value(g, getter))
                        || d.set.is_some_and(|s| !self.same_value(s, setter))
                    {
                        return Outcome::Reject;
                    }
                }
                Property::Data { value, flags } if !flags.writable() => {
                    if d.writable == Some(true)
                        || d.value.is_some_and(|v| !self.same_value(v, value))
                    {
                        return Outcome::Reject;
                    }
                }
                Property::Data { .. } => {}
            }
        }
        let e = d.enumerable.unwrap_or(cf.enumerable());
        let c = d.configurable.unwrap_or(cf.configurable());
        Outcome::Write(match cur {
            Property::Data { .. } if d.is_accessor() => Property::Accessor {
                getter: d.get.unwrap_or(undef),
                setter: d.set.unwrap_or(undef),
                flags: PropFlags::new(false, e, c),
            },
            Property::Accessor { .. } if d.is_data() => Property::Data {
                value: d.value.unwrap_or(undef),
                flags: PropFlags::new(d.writable.unwrap_or(false), e, c),
            },
            Property::Data { value, flags } => Property::Data {
                value: d.value.unwrap_or(value),
                flags: PropFlags::new(d.writable.unwrap_or(flags.writable()), e, c),
            },
            Property::Accessor { getter, setter, .. } => Property::Accessor {
                getter: d.get.unwrap_or(getter),
                setter: d.set.unwrap_or(setter),
                flags: PropFlags::new(false, e, c),
            },
        })
    }

    /// OrdinaryDefineOwnProperty.
    fn ordinary_define(
        &mut self,
        o: Obj,
        key: Atom,
        d: &PropertyDescriptor,
    ) -> Result<bool, HeapError> {
        let loc = self.find_own(o, key);
        let current = self.read_loc(o, loc);
        match self.validate_descriptor(current, self.is_extensible(o), d) {
            Outcome::Reject => Ok(false),
            Outcome::Keep => Ok(true),
            Outcome::Write(p) => {
                self.write_property(o, key, loc, p)?;
                Ok(true)
            }
        }
    }

    /// `[[DefineOwnProperty]]` (ordinary, plus the Array exotic behavior). `Ok(false)` means
    /// the spec returns false (the caller throws a TypeError in strict code). For an
    /// array's `length`, `d.value` must already be a Number (the caller does the
    /// ToNumber/ToUint32 conversions); a value that is not a `u32` gives
    /// [`HeapError::InvalidArrayLength`].
    pub fn define_own_property(
        &mut self,
        o: Obj,
        key: Atom,
        d: &PropertyDescriptor,
    ) -> Result<bool, HeapError> {
        if self.frozen(o) {
            // ValidateAndApplyPropertyDescriptor against the frozen view (D2304): what it
            // accepts changes nothing, so nothing is written.
            let current = self.get_own_property(o, key);
            return Ok(!matches!(
                self.validate_descriptor(current, false, d),
                Outcome::Reject
            ));
        }
        if self.object_class(o) == ObjectClass::TypedArray
            && let Some(n) = self.canonical_numeric_index(key)
        {
            // IntegerIndexedDefineOwnProperty (10.4.5.3); an immutable buffer's elements
            // are non-writable and non-configurable, and keep their value (D1970).
            let valid = self.ta_get_valid(o, n);
            let mutable = !self.ta_immutable(o);
            if !valid
                || d.configurable.is_some_and(|c| c != mutable)
                || d.enumerable == Some(false)
                || d.is_accessor()
                || d.writable.is_some_and(|w| w != mutable)
            {
                return Ok(false);
            }
            if let Some(v) = d.value {
                if !mutable {
                    let current = self.ta_numeric_get(o, key).flatten();
                    let current = match current {
                        Some(c) => self.materialize_value(o, key, c)?,
                        None => return Ok(false),
                    };
                    return Ok(self.same_value(v, current));
                }
                return Ok(self.ta_numeric_set(o, key, v) == Some(true));
            }
            return Ok(true);
        }
        if self.object_class(o) == ObjectClass::Array {
            if key == atoms::LENGTH {
                return self.array_set_length(o, d);
            }
            if let Some(i) = self.atom_array_index(key) {
                let len = self.array_length(o);
                if i >= len && self.obj_flags(o) & OBJ_LENGTH_READONLY != 0 {
                    return Ok(false);
                }
                if !self.ordinary_define(o, key, d)? {
                    return Ok(false);
                }
                if i >= len {
                    self.set_array_length_raw(o, i + 1);
                }
                return Ok(true);
            }
        }
        self.ordinary_define(o, key, d)
    }

    /// CreateDataProperty: define `key` as a writable, enumerable, configurable data
    /// property.
    pub fn create_data_property(
        &mut self,
        o: Obj,
        key: Atom,
        value: Value,
    ) -> Result<bool, HeapError> {
        // A new key on an extensible ordinary object (object literals, the common case):
        // what ordinary_define would do, without building and validating a descriptor.
        // An Array takes it for a key that is no index (a match array's `index`,
        // `input` and `groups`; `length` is never absent).
        let class = self.object_class(o);
        if class != ObjectClass::TypedArray
            && key.as_int().is_none()
            && (class != ObjectClass::Array || self.atom_array_index(key).is_none())
            && self.find_own(o, key) == Loc::None
            && self.is_extensible(o)
        {
            self.prop_insert(o, key, &Property::data(value, PropFlags::ALL))?;
            return Ok(true);
        }
        self.define_own_property(o, key, &PropertyDescriptor::data(value, PropFlags::ALL))
    }

    /// ArraySetLength.
    fn array_set_length(&mut self, o: Obj, d: &PropertyDescriptor) -> Result<bool, HeapError> {
        let current = self.array_length_property(o);
        let old_len = self.array_length(o);
        let Some(v) = d.value else {
            return Ok(match self.validate_descriptor(Some(current), true, d) {
                Outcome::Reject => false,
                Outcome::Keep => true,
                Outcome::Write(p) => {
                    if !p.flags().writable() {
                        self.set_obj_flag(o, OBJ_LENGTH_READONLY, true);
                    }
                    true
                }
            });
        };
        let new_len = match v.as_number() {
            // `as` saturates and maps NaN to 0, so the round trip rejects everything else.
            Some(n) if (n as u32) as f64 == n => n as u32,
            _ => return Err(HeapError::InvalidArrayLength),
        };
        let mut nd = *d;
        nd.value = Some(Value::number(new_len as f64));
        if new_len >= old_len {
            return Ok(match self.validate_descriptor(Some(current), true, &nd) {
                Outcome::Reject => false,
                Outcome::Keep => true,
                Outcome::Write(p) => {
                    self.set_array_length_raw(o, new_len);
                    if !p.flags().writable() {
                        self.set_obj_flag(o, OBJ_LENGTH_READONLY, true);
                    }
                    true
                }
            });
        }
        if !current.flags().writable() {
            return Ok(false);
        }
        let new_writable = d.writable != Some(false);
        nd.writable = Some(true);
        if let Outcome::Reject = self.validate_descriptor(Some(current), true, &nd) {
            return Ok(false);
        }
        let (ok, final_len) = self.truncate_elements(o, new_len)?;
        self.set_array_length_raw(o, final_len);
        if !new_writable {
            self.set_obj_flag(o, OBJ_LENGTH_READONLY, true);
        }
        Ok(ok)
    }

    /// Deletes every element at or above `new_len`, as ArraySetLength does from the top
    /// down: it stops above the highest non-configurable one. Returns whether everything
    /// was deleted, and the resulting length.
    fn truncate_elements(&mut self, o: Obj, new_len: u32) -> Result<(bool, u32), HeapError> {
        let mut stop = new_len;
        let mut ok = true;
        if self.obj_flags(o) & OBJ_INDEXED_PROPS != 0 {
            let mut doomed = Vec::new();
            for i in 0..self.prop_slots(o) {
                let key = self.prop_key(o, i);
                if key.is_null() {
                    continue;
                }
                if let Some(idx) = self.atom_array_index(key)
                    && idx >= new_len
                {
                    if !self.prop_read(o, i).flags().configurable() && idx + 1 > stop {
                        stop = idx + 1;
                        ok = false;
                    }
                    doomed.push_out((idx, key));
                }
            }
            for (idx, key) in doomed {
                if idx >= stop
                    && let Some(i) = self.prop_find(o, key)
                {
                    self.prop_remove(o, i)?;
                }
            }
        }
        let el = self.elements(o);
        if el != 0 {
            let len = self.value_array_len(el);
            if len > stop {
                let start = ValueArray::VALUES.at(el, stop);
                self.arena_mut().zero(start, (len - stop) * 8);
                self.arena_mut().set(el, ValueArray::LEN, stop);
            }
        }
        Ok((ok, stop))
    }

    /// Array.prototype.pop's moves in one step (0080): for an Array with only dense
    /// elements (no index properties in the table, so none is non-configurable, as in a
    /// frozen array), a writable `length`, its last element present in dense storage and
    /// no index properties on its prototype chain, removes that element, shrinks
    /// `length` by one and returns the element. There `Get`, `DeletePropertyOrThrow` and
    /// `Set(length)` observe nothing else. `None`, with nothing changed, otherwise.
    pub fn array_pop_dense(&mut self, o: Obj) -> Option<Value> {
        if !self.array_can_pop_dense(o) {
            return None;
        }
        let len = self.array_length(o);
        let el = self.elements(o);
        let v = self.value_array_get(el, len - 1);
        // As `truncate_elements`: the slots past the length are zero.
        let at = ValueArray::VALUES.at(el, len - 1);
        self.arena_mut().zero(at, 8);
        self.arena_mut().set(el, ValueArray::LEN, len - 1);
        self.set_array_length_raw(o, len - 1);
        Some(v)
    }

    /// Whether [`Heap::array_pop_dense`] applies to `o`.
    pub fn array_can_pop_dense(&self, o: Obj) -> bool {
        if self.object_class(o) != ObjectClass::Array
            || self.frozen(o)
            || self.obj_flags(o) & (OBJ_INDEXED_PROPS | OBJ_LENGTH_READONLY) != 0
        {
            return false;
        }
        let len = self.array_length(o);
        if len == 0 || self.dense_len(o) != len {
            return false;
        }
        let mut p = self.get_prototype_of(o);
        while let Some(q) = p {
            if self.dense_len(q) != 0 || self.obj_flags(q) & OBJ_INDEXED_PROPS != 0 {
                return false;
            }
            p = self.get_prototype_of(q);
        }
        !self.value_array_get(self.elements(o), len - 1).is_hole()
    }

    /// `[[Delete]]`: `false` if the property is non-configurable.
    ///
    /// Deleting a property of a shape-mode object other than its last may allocate (the
    /// object becomes a dictionary), so this can fail with `OutOfMemory`, with nothing
    /// changed.
    pub fn delete_own_property(&mut self, o: Obj, key: Atom) -> Result<bool, HeapError> {
        // A frozen builtin's properties are non-configurable (D2304).
        if self.frozen(o) {
            return Ok(!self.has_own_property(o, key));
        }
        if self.object_class(o) == ObjectClass::TypedArray
            && let Some(r) = self.ta_numeric_get(o, key)
        {
            return Ok(r.is_none());
        }
        Ok(match self.find_own(o, key) {
            Loc::None => true,
            Loc::Dense(i) => {
                let el = self.elements(o);
                self.value_array_set(el, i, Value::HOLE);
                true
            }
            Loc::ArrayLength => false,
            // Configurable; the other keys keep their order.
            Loc::LazyLength => {
                self.set_obj_flag(o, OBJ_LAZY_LENGTH, false);
                true
            }
            Loc::LazyName => {
                self.set_obj_flag(o, OBJ_LAZY_NAME, false);
                true
            }
            Loc::Prop(i) => {
                if !self.prop_read(o, i).flags().configurable() {
                    return Ok(false);
                }
                self.prop_remove(o, i)?;
                true
            }
        })
    }

    #[cfg(djs_prelude)]
    /// Removes an own table property even if it is non-configurable. Only for setting up
    /// builtins (a prelude function loses the `prototype` every ordinary function gets).
    pub(crate) fn remove_own_property_unchecked(
        &mut self,
        o: Obj,
        key: Atom,
    ) -> Result<(), HeapError> {
        match self.find_own(o, key) {
            Loc::Prop(i) => self.prop_remove(o, i)?,
            Loc::LazyLength => self.set_obj_flag(o, OBJ_LAZY_LENGTH, false),
            Loc::LazyName => self.set_obj_flag(o, OBJ_LAZY_NAME, false),
            _ => {}
        }
        Ok(())
    }

    /// For an ordinary object whose own properties are exactly its property table entries
    /// in `[[OwnPropertyKeys]]` order (no elements, no array-index keys), the number of
    /// entry slots to pass to [`Heap::plain_entry`]; `None` otherwise. Lets hot loops (JSON
    /// stringify) walk properties without collecting keys and looking each one up.
    pub fn plain_entry_slots(&self, o: Obj) -> Option<u32> {
        if self.object_class(o) != ObjectClass::Ordinary
            || self.obj_flags(o) & OBJ_INDEXED_PROPS != 0
            || self.dense_len(o) != 0
        {
            return None;
        }
        Some(self.prop_slots(o))
    }

    /// Entry slot `i` of a plain object (see [`Heap::plain_entry_slots`]): its key and
    /// property, or `None` for a deleted slot. Symbol keys are included; string keys come
    /// in creation order.
    pub fn plain_entry(&self, o: Obj, i: u32) -> Option<(Atom, Property)> {
        let key = self.prop_key(o, i);
        if key.is_null() {
            None
        } else {
            Some((key, self.prop_read(o, i)))
        }
    }

    /// Adds an own property under a private name (D1400), whether or not the object is
    /// extensible (PrivateFieldAdd, PrivateMethodOrAccessorAdd). The caller checked that
    /// it is absent.
    pub fn add_private_property(
        &mut self,
        o: Obj,
        key: Atom,
        p: &Property,
    ) -> Result<(), HeapError> {
        debug_assert!(self.is_private_atom(key) && self.find_own(o, key) == Loc::None);
        self.prop_insert(o, key, p)
    }

    /// Whether `o` has no elements and one property slot, which holds `key`: then `key`
    /// is its only own property (an object that had more and lost them may say false).
    pub(crate) fn only_own_key(&self, o: Obj, key: Atom) -> bool {
        self.elements(o) == 0 && self.prop_slots(o) == 1 && self.prop_key(o, 0) == key
    }

    /// `[[OwnPropertyKeys]]` in spec order: array indexes ascending, then string keys in
    /// creation order, then symbols in creation order.
    pub fn own_keys(&self, o: Obj) -> Vec<Atom> {
        if self.object_class(o) == ObjectClass::TypedArray {
            // The element indexes, then the ordinary keys (never numeric).
            let mut keys = self.ta_keys(o);
            keys.extend(self.ordinary_own_keys(o));
            return keys;
        }
        self.ordinary_own_keys(o)
    }

    /// The private names (D1400) `o` holds, in table order, with their properties:
    /// what [[OwnPropertyKeys]] leaves out. For dumps (vm/dump.rs).
    pub fn private_entries(&self, o: Obj) -> Vec<(Atom, Property)> {
        let mut out = Vec::new();
        for i in 0..self.prop_slots(o) {
            let key = self.prop_key(o, i);
            if !key.is_null() && self.is_private_atom(key) {
                out.push_out((key, self.prop_read(o, i)));
            }
        }
        out
    }

    pub(crate) fn ordinary_own_keys(&self, o: Obj) -> Vec<Atom> {
        let mut indexes: Vec<(u32, Atom)> = Vec::new();
        let mut strings = Vec::new();
        let mut symbols = Vec::new();
        let el = self.elements(o);
        if el != 0 {
            for i in 0..self.value_array_len(el) {
                if !self.value_array_get(el, i).is_hole() {
                    indexes.push_out((i, Atom::int(i).expect("dense index fits an int atom")));
                }
            }
        }
        let indexed = self.obj_flags(o) & OBJ_INDEXED_PROPS != 0;
        let mut sort = false;
        {
            for i in 0..self.prop_slots(o) {
                let key = self.prop_key(o, i);
                if key.is_null() {
                    continue;
                }
                if key.is_symbol() {
                    // Private names are not property keys to the language (D1400).
                    if !self.is_private_atom(key) {
                        symbols.push_out(key);
                    }
                } else if let Some(idx) = indexed.then(|| self.atom_array_index(key)).flatten() {
                    indexes.push_out((idx, key));
                    sort = true;
                } else {
                    strings.push_out(key);
                }
            }
        }
        if sort {
            crate::sort::sort_by(&mut indexes, |a, b| a.0 < b.0);
        }
        let mut out =
            crate::buf::vec_with_capacity(indexes.len() + strings.len() + symbols.len() + 3);
        out.extend(indexes.into_iter().map(|(_, k)| k));
        if self.object_class(o) == ObjectClass::Array {
            out.push_out(atoms::LENGTH);
        }
        // A closure's lazy `length` and `name` come before its stored keys (D2445).
        let f = self.obj_flags(o);
        if f & OBJ_LAZY_LENGTH != 0 {
            out.push(atoms::LENGTH);
        }
        if f & OBJ_LAZY_NAME != 0 {
            out.push(atoms::NAME);
        }
        out.extend(strings);
        out.extend(symbols);
        out
    }
}

/// Dense elements are addressed by integer atoms, so they stop at the int atom range.
#[inline]
const fn atom_int_max() -> u32 {
    super::atom::MAX_INT_ATOM
}
