//! The realm: the global object and the intrinsics, and Error objects.

use alloc::string::String;

use crate::bytecode::intrinsics::INTRINSICS;
use crate::heap::cells::{Realm, Roots};
use crate::heap::{
    Atom, CellKind, Heap, HeapError, ObjectClass, PropFlags, Property, PropertyDescriptor, atoms,
};
use crate::value::{Obj, Value};

/// The globals of the builtin families the slim build leaves out (D2411): their
/// ReferenceError says so.
pub const ABSENT_GLOBALS: &[&str] = &[
    "ArrayBuffer",
    "AsyncDisposableStack",
    "BigInt",
    "BigInt64Array",
    "BigUint64Array",
    "DataView",
    "DisposableStack",
    "FinalizationRegistry",
    "Float16Array",
    "Float32Array",
    "Float64Array",
    "Int16Array",
    "Int32Array",
    "Int8Array",
    "Iterator",
    "SuppressedError",
    "Uint16Array",
    "Uint32Array",
    "Uint8Array",
    "Uint8ClampedArray",
    "WeakRef",
];

/// The globals of the builtins only `embed` builds have (`// @embed-only`, D3175): full
/// builds without `embed` (the wasm modules) name them like [`ABSENT_GLOBALS`].
pub const EMBED_GLOBALS: &[&str] = &["TextDecoder", "TextEncoder"];

/// The realm slots of the families the slim build leaves out (D2411): `undefined` there.
const SLIM_LEAVES_OUT: &[Intrinsic] = &[
    Intrinsic::IteratorHelperProto,
    Intrinsic::WrapForValidIteratorProto,
    Intrinsic::ArrayBufferProto,
    Intrinsic::DataViewProto,
    Intrinsic::TypedArrayProto,
    Intrinsic::Int8ArrayProto,
    Intrinsic::Uint8ArrayProto,
    Intrinsic::Uint8ClampedArrayProto,
    Intrinsic::Int16ArrayProto,
    Intrinsic::Uint16ArrayProto,
    Intrinsic::Int32ArrayProto,
    Intrinsic::Uint32ArrayProto,
    Intrinsic::Float16ArrayProto,
    Intrinsic::Float32ArrayProto,
    Intrinsic::Float64ArrayProto,
    Intrinsic::BigInt64ArrayProto,
    Intrinsic::BigUint64ArrayProto,
    Intrinsic::DisposableStackProto,
    Intrinsic::AsyncDisposableStackProto,
    Intrinsic::SuppressedErrorProto,
    Intrinsic::WeakRefProto,
    Intrinsic::FinalizationRegistryProto,
    Intrinsic::BigIntProto,
];

/// Intrinsics, by index into `Realm::INTRINSICS`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum Intrinsic {
    Global,
    ObjectProto,
    FunctionProto,
    ArrayProto,
    ErrorProto,
    TypeErrorProto,
    RangeErrorProto,
    ReferenceErrorProto,
    SyntaxErrorProto,
    InternalErrorProto,
    /// A preallocated `InternalError: out of memory`, thrown when even the error object
    /// cannot be allocated.
    OutOfMemoryError,
    /// The prelude's `Script` cell (an internal reference), or `undefined`: its frames
    /// are builtins in stack traces.
    PreludeScript,
    EvalErrorProto,
    UriErrorProto,
    AggregateErrorProto,
    BooleanProto,
    NumberProto,
    StringProto,
    SymbolProto,
    /// `Symbol.for`'s registry: a prototype-less object from keys to symbols.
    SymbolRegistry,
    /// `Math.random`'s state: an internal ValueArray of two int32 halves.
    RandomState,
    /// %IteratorPrototype%.
    IteratorProto,
    /// %ArrayIteratorPrototype% ([[Prototype]] = %IteratorPrototype%).
    ArrayIteratorProto,
    /// %StringIteratorPrototype% ([[Prototype]] = %IteratorPrototype%). String Iterators
    /// are Array Iterator objects of kind 3 (D1346).
    StringIteratorProto,
    MapProto,
    SetProto,
    WeakMapProto,
    WeakSetProto,
    /// %MapIteratorPrototype% and %SetIteratorPrototype% ([[Prototype]]
    /// %IteratorPrototype%).
    MapIteratorProto,
    SetIteratorProto,
    /// %Promise.prototype% (an ordinary object).
    PromiseProto,
    DateProto,
    /// The host's clock for `Date.now()` (ms since the epoch, a Number; D1383).
    Clock,
    /// The local time zone offset in minutes east of UTC (an int; D1383).
    TimeZoneOffset,
    /// %GeneratorPrototype% (%GeneratorFunction.prototype.prototype%, [[Prototype]]
    /// %IteratorPrototype%): the default prototype of generator objects (D1375).
    GeneratorProto,
    ArrayBufferProto,
    /// %TypedArray% (the constructor, installed by the prelude, D1391) and
    /// %TypedArray.prototype%.
    TypedArrayCtor,
    TypedArrayProto,
    Int8ArrayProto,
    Uint8ArrayProto,
    Uint8ClampedArrayProto,
    Int16ArrayProto,
    Uint16ArrayProto,
    Int32ArrayProto,
    Uint32ArrayProto,
    Float16ArrayProto,
    Float32ArrayProto,
    Float64ArrayProto,
    BigInt64ArrayProto,
    BigUint64ArrayProto,
    DataViewProto,
    /// The constructors as installed (a prototype-less object from names to functions),
    /// for the spec's %Name% defaults (SpeciesConstructor and friends): user code can
    /// replace the globals but not these.
    Constructors,
    /// %GeneratorFunction.prototype%, %AsyncFunction.prototype%,
    /// %AsyncGeneratorFunction.prototype%: the [[Prototype]] of closures by their
    /// ASYNC/GENERATOR flags (D1420).
    GeneratorFunctionProto,
    AsyncFunctionProto,
    AsyncGeneratorFunctionProto,
    /// %AsyncIteratorPrototype%, %AsyncGeneratorPrototype% and
    /// %AsyncFromSyncIteratorPrototype% (D1421).
    AsyncIteratorProto,
    AsyncGeneratorProto,
    AsyncFromSyncIteratorProto,
    /// %BigInt.prototype% (an ordinary object, 0046).
    BigIntProto,
    /// %RegExp.prototype% and %RegExpStringIteratorPrototype% (0033).
    RegExpProto,
    RegExpStringIteratorProto,
    /// %IteratorHelperPrototype% and %WrapForValidIteratorPrototype% ([[Prototype]]
    /// %IteratorPrototype%, 0051).
    IteratorHelperProto,
    WrapForValidIteratorProto,
    /// %DisposableStack.prototype%, %AsyncDisposableStack.prototype% and
    /// %SuppressedError.prototype% (0051).
    DisposableStackProto,
    AsyncDisposableStackProto,
    SuppressedErrorProto,
    /// %WeakRef.prototype% and %FinalizationRegistry.prototype% (0051).
    WeakRefProto,
    FinalizationRegistryProto,
    /// [[KeptAlive]] (9.10, the agent's KeptAlive list): an internal ValueArray of the
    /// WeakRef targets created or dereferenced since the last ClearKeptObjects, or
    /// undefined (D1513).
    KeptObjects,
    JoinStack,
    /// The live FinalizationRegistries: an internal EphemeronArray of `[registry,
    /// true]`, or undefined (heap/weakref.rs).
    Registries,
    /// %CallSitePrototype%: the [[Prototype]] of the CallSite objects given to
    /// `Error.prepareStackTrace` (0106, D2050).
    CallSiteProto,
    /// An int bumped whenever a Script declares a global lexical binding: global
    /// inline caches of global object properties check it (D1620).
    GlobalLexEpoch,
    /// The prelude function table (D2440): an internal ValueArray of the prelude's
    /// functions that other prelude functions refer to (`get_builtin`), or undefined
    /// before the prelude runs.
    PreludeFunctions,
    /// The RegExp compile cache (D3138): an internal ValueArray of `CACHE_ENTRIES`
    /// `[source, flags, program cell]` triples, most recently used first, or undefined.
    /// No user code reaches it.
    RegExpCache,
    /// ES modules (D3171, interp/module.rs): an internal Record `[registry, traps]`, the
    /// registry a hidden object from module key to module record and the traps the
    /// prototype of the namespace handlers; undefined until the first module loads.
    Modules,
}

const INTRINSIC_COUNT: u32 = Intrinsic::Modules as u32 + 1;

impl Intrinsic {
    /// The intrinsic prototype of the constructor `name`, if the realm has one: the
    /// constructors installed by the prelude (`%InstallConstructor`) reuse it.
    pub(crate) fn prototype_of(name: &str) -> Option<Intrinsic> {
        Some(match name {
            "Object" => Intrinsic::ObjectProto,
            "Function" => Intrinsic::FunctionProto,
            "Array" => Intrinsic::ArrayProto,
            "Error" => Intrinsic::ErrorProto,
            "TypeError" => Intrinsic::TypeErrorProto,
            "RangeError" => Intrinsic::RangeErrorProto,
            "ReferenceError" => Intrinsic::ReferenceErrorProto,
            "SyntaxError" => Intrinsic::SyntaxErrorProto,
            "EvalError" => Intrinsic::EvalErrorProto,
            "URIError" => Intrinsic::UriErrorProto,
            "AggregateError" => Intrinsic::AggregateErrorProto,
            "InternalError" => Intrinsic::InternalErrorProto,
            "Boolean" => Intrinsic::BooleanProto,
            "Number" => Intrinsic::NumberProto,
            "String" => Intrinsic::StringProto,
            "Symbol" => Intrinsic::SymbolProto,
            "BigInt" => Intrinsic::BigIntProto,
            "Map" => Intrinsic::MapProto,
            "Set" => Intrinsic::SetProto,
            "WeakMap" => Intrinsic::WeakMapProto,
            "WeakSet" => Intrinsic::WeakSetProto,
            "Promise" => Intrinsic::PromiseProto,
            "Date" => Intrinsic::DateProto,
            "ArrayBuffer" => Intrinsic::ArrayBufferProto,
            "%TypedArray%" => Intrinsic::TypedArrayProto,
            "Int8Array" => Intrinsic::Int8ArrayProto,
            "Uint8Array" => Intrinsic::Uint8ArrayProto,
            "Uint8ClampedArray" => Intrinsic::Uint8ClampedArrayProto,
            "Int16Array" => Intrinsic::Int16ArrayProto,
            "Uint16Array" => Intrinsic::Uint16ArrayProto,
            "Int32Array" => Intrinsic::Int32ArrayProto,
            "Uint32Array" => Intrinsic::Uint32ArrayProto,
            "Float16Array" => Intrinsic::Float16ArrayProto,
            "Float32Array" => Intrinsic::Float32ArrayProto,
            "Float64Array" => Intrinsic::Float64ArrayProto,
            "BigInt64Array" => Intrinsic::BigInt64ArrayProto,
            "BigUint64Array" => Intrinsic::BigUint64ArrayProto,
            "DataView" => Intrinsic::DataViewProto,
            "RegExp" => Intrinsic::RegExpProto,
            "Iterator" => Intrinsic::IteratorProto,
            "IteratorHelper" => Intrinsic::IteratorHelperProto,
            "WrapForValidIterator" => Intrinsic::WrapForValidIteratorProto,
            "DisposableStack" => Intrinsic::DisposableStackProto,
            "AsyncDisposableStack" => Intrinsic::AsyncDisposableStackProto,
            "SuppressedError" => Intrinsic::SuppressedErrorProto,
            "WeakRef" => Intrinsic::WeakRefProto,
            "FinalizationRegistry" => Intrinsic::FinalizationRegistryProto,
            "GeneratorFunction" => Intrinsic::GeneratorFunctionProto,
            "AsyncGeneratorFunction" => Intrinsic::AsyncGeneratorFunctionProto,
            "AsyncFunction" => Intrinsic::AsyncFunctionProto,
            _ => return None,
        })
    }

    /// `%RealmIntrinsic(name)` of the builtin language: `<Constructor>Prototype`.
    pub(crate) fn by_name(name: &str) -> Option<Intrinsic> {
        Intrinsic::prototype_of(name.strip_suffix("Prototype")?)
    }
}

/// The native error types the VM throws itself.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ErrorKind {
    Error,
    TypeError,
    RangeError,
    ReferenceError,
    SyntaxError,
    InternalError,
    UriError,
}

impl ErrorKind {
    pub const fn name(self) -> &'static str {
        match self {
            ErrorKind::Error => "Error",
            ErrorKind::TypeError => "TypeError",
            ErrorKind::RangeError => "RangeError",
            ErrorKind::ReferenceError => "ReferenceError",
            ErrorKind::SyntaxError => "SyntaxError",
            ErrorKind::InternalError => "InternalError",
            ErrorKind::UriError => "URIError",
        }
    }

    const fn proto(self) -> Intrinsic {
        match self {
            ErrorKind::Error => Intrinsic::ErrorProto,
            ErrorKind::TypeError => Intrinsic::TypeErrorProto,
            ErrorKind::RangeError => Intrinsic::RangeErrorProto,
            ErrorKind::ReferenceError => Intrinsic::ReferenceErrorProto,
            ErrorKind::SyntaxError => Intrinsic::SyntaxErrorProto,
            ErrorKind::InternalError => Intrinsic::InternalErrorProto,
            ErrorKind::UriError => Intrinsic::UriErrorProto,
        }
    }
}

/// The realm cell.
#[inline]
pub(crate) fn realm(heap: &Heap) -> u32 {
    heap.realm().as_internal().expect("no realm")
}

/// An intrinsic of the realm.
#[inline]
pub fn intrinsic(heap: &Heap, i: Intrinsic) -> Value {
    heap.arena()
        .read(Realm::INTRINSICS.at(realm(heap), i as u32))
}

/// Replaces a value intrinsic (the clock, the time zone offset).
pub(crate) fn set_intrinsic(heap: &mut Heap, i: Intrinsic, v: Value) {
    let r = realm(heap);
    heap.arena_mut().write(Realm::INTRINSICS.at(r, i as u32), v)
}

/// An intrinsic that is an object.
pub fn intrinsic_obj(heap: &Heap, i: Intrinsic) -> Obj {
    intrinsic(heap, i).as_object().expect("object intrinsic")
}

/// A non-enumerable data property (how builtins define their properties).
pub(crate) fn define_hidden(heap: &mut Heap, o: Obj, key: Atom, v: Value) -> Result<(), HeapError> {
    let d = PropertyDescriptor::data(v, PropFlags::WRITABLE | PropFlags::CONFIGURABLE);
    heap.define_own_property(o, key, &d)?;
    Ok(())
}

/// Creates the realm: prototypes, error prototypes, the global object.
pub(crate) fn init(heap: &mut Heap) -> Result<(), HeapError> {
    let size = Realm::INTRINSICS
        .cell_size(INTRINSIC_COUNT)
        .ok_or(HeapError::OutOfMemory)?;
    let r = heap.alloc_cell(CellKind::Realm, size)?;
    heap.set_realm(Value::internal(r));
    let set = |heap: &mut Heap, i: Intrinsic, v: Value| {
        heap.arena_mut().write(Realm::INTRINSICS.at(r, i as u32), v)
    };
    let object_proto = heap.new_object(None)?;
    set(heap, Intrinsic::ObjectProto, Value::object(object_proto));
    // Function.prototype is itself a function that returns undefined (20.2.3): a
    // builtin closure of %FunctionPrototypeBehavior%, callable once the prelude has
    // installed it (D1840).
    let class = ObjectClass::BuiltinClosure;
    let function_proto = heap.alloc_object(class, Some(object_proto), class.slots())?;
    let behavior = const { crate::bytecode::intrinsics::const_id("FunctionPrototypeBehavior") };
    heap.set_object_slot(function_proto, 0, Value::int(i32::from(behavior)));
    heap.set_object_slot(function_proto, 1, Value::UNDEFINED);
    heap.set_object_slot(function_proto, 2, Value::UNDEFINED);
    set(
        heap,
        Intrinsic::FunctionProto,
        Value::object(function_proto),
    );
    let array_proto = heap.new_array(Some(object_proto), 0)?;
    set(heap, Intrinsic::ArrayProto, Value::object(array_proto));
    let iterator_proto = heap.new_object(Some(object_proto))?;
    set(
        heap,
        Intrinsic::IteratorProto,
        Value::object(iterator_proto),
    );
    let array_iterator_proto = heap.new_object(Some(iterator_proto))?;
    set(
        heap,
        Intrinsic::ArrayIteratorProto,
        Value::object(array_iterator_proto),
    );
    let string_iterator_proto = heap.new_object(Some(iterator_proto))?;
    set(
        heap,
        Intrinsic::StringIteratorProto,
        Value::object(string_iterator_proto),
    );
    for i in [
        Intrinsic::MapIteratorProto,
        Intrinsic::SetIteratorProto,
        Intrinsic::RegExpStringIteratorProto,
        Intrinsic::IteratorHelperProto,
        Intrinsic::WrapForValidIteratorProto,
    ] {
        let p = heap.new_object(Some(iterator_proto))?;
        set(heap, i, Value::object(p));
    }
    for i in [
        Intrinsic::ArrayBufferProto,
        Intrinsic::DataViewProto,
        Intrinsic::TypedArrayProto,
        Intrinsic::CallSiteProto,
    ] {
        let p = heap.new_object(Some(object_proto))?;
        set(heap, i, Value::object(p));
    }
    set(heap, Intrinsic::TypedArrayCtor, Value::UNDEFINED);
    set(heap, Intrinsic::KeptObjects, Value::UNDEFINED);
    set(heap, Intrinsic::JoinStack, Value::UNDEFINED);
    set(heap, Intrinsic::RegExpCache, Value::UNDEFINED);
    set(heap, Intrinsic::Registries, Value::UNDEFINED);
    set(heap, Intrinsic::GlobalLexEpoch, Value::int(0));
    let ctors = heap.new_object(None)?;
    heap.make_dictionary(ctors)?;
    set(heap, Intrinsic::Constructors, Value::object(ctors));
    let typed_array_proto = intrinsic_obj(heap, Intrinsic::TypedArrayProto);
    for i in [
        Intrinsic::Int8ArrayProto,
        Intrinsic::Uint8ArrayProto,
        Intrinsic::Uint8ClampedArrayProto,
        Intrinsic::Int16ArrayProto,
        Intrinsic::Uint16ArrayProto,
        Intrinsic::Int32ArrayProto,
        Intrinsic::Uint32ArrayProto,
        Intrinsic::Float16ArrayProto,
        Intrinsic::Float32ArrayProto,
        Intrinsic::Float64ArrayProto,
        Intrinsic::BigInt64ArrayProto,
        Intrinsic::BigUint64ArrayProto,
    ] {
        let p = heap.new_object(Some(typed_array_proto))?;
        set(heap, i, Value::object(p));
    }
    set(heap, Intrinsic::Clock, Value::int(0));
    set(heap, Intrinsic::TimeZoneOffset, Value::int(0));
    for i in [
        Intrinsic::MapProto,
        Intrinsic::SetProto,
        Intrinsic::WeakMapProto,
        Intrinsic::WeakSetProto,
        Intrinsic::DateProto,
        Intrinsic::RegExpProto,
        Intrinsic::DisposableStackProto,
        Intrinsic::AsyncDisposableStackProto,
        Intrinsic::WeakRefProto,
        Intrinsic::FinalizationRegistryProto,
    ] {
        let p = heap.new_object(Some(object_proto))?;
        set(heap, i, Value::object(p));
    }
    let error_proto = heap.new_object(Some(object_proto))?;
    set(heap, Intrinsic::ErrorProto, Value::object(error_proto));
    // Error.prototype's `name` and `message` come from the prelude (builtins/error.ts),
    // after `toString`, in qjs's order (D2150).
    let empty = Value::string(heap.atom_string(atoms::EMPTY)?);
    for kind in [
        ErrorKind::TypeError,
        ErrorKind::RangeError,
        ErrorKind::ReferenceError,
        ErrorKind::SyntaxError,
        ErrorKind::InternalError,
    ] {
        let p = heap.new_object(Some(error_proto))?;
        let name = heap.alloc_str(kind.name())?;
        define_hidden(heap, p, atoms::NAME, Value::string(name))?;
        define_hidden(heap, p, atoms::MESSAGE, empty)?;
        set(heap, kind.proto(), Value::object(p));
    }
    for (i, name) in [
        (Intrinsic::EvalErrorProto, "EvalError"),
        (Intrinsic::UriErrorProto, "URIError"),
        (Intrinsic::AggregateErrorProto, "AggregateError"),
        (Intrinsic::SuppressedErrorProto, "SuppressedError"),
    ] {
        let p = heap.new_object(Some(error_proto))?;
        let name = heap.alloc_str(name)?;
        define_hidden(heap, p, atoms::NAME, Value::string(name))?;
        define_hidden(heap, p, atoms::MESSAGE, empty)?;
        set(heap, i, Value::object(p));
    }
    // Boolean.prototype, Number.prototype and String.prototype are wrapper objects
    // themselves (false, +0, "").
    for (i, class, v) in [
        (Intrinsic::BooleanProto, ObjectClass::Boolean, Value::FALSE),
        (Intrinsic::NumberProto, ObjectClass::Number, Value::int(0)),
        (Intrinsic::StringProto, ObjectClass::String, empty),
    ] {
        let p = heap.alloc_object(class, Some(object_proto), 1)?;
        heap.set_object_slot(p, 0, v);
        set(heap, i, Value::object(p));
    }
    let string_proto = intrinsic_obj(heap, Intrinsic::StringProto);
    let d = PropertyDescriptor::data(Value::int(0), PropFlags::NONE);
    heap.define_own_property(string_proto, atoms::LENGTH, &d)?;
    let symbol_proto = heap.new_object(Some(object_proto))?;
    set(heap, Intrinsic::SymbolProto, Value::object(symbol_proto));
    let bigint_proto = heap.new_object(Some(object_proto))?;
    set(heap, Intrinsic::BigIntProto, Value::object(bigint_proto));
    let promise_proto = heap.new_object(Some(object_proto))?;
    set(heap, Intrinsic::PromiseProto, Value::object(promise_proto));
    let iterator_proto = intrinsic_obj(heap, Intrinsic::IteratorProto);
    let generator_proto = heap.new_object(Some(iterator_proto))?;
    set(
        heap,
        Intrinsic::GeneratorProto,
        Value::object(generator_proto),
    );
    let async_iterator_proto = heap.new_object(Some(object_proto))?;
    set(
        heap,
        Intrinsic::AsyncIteratorProto,
        Value::object(async_iterator_proto),
    );
    let async_generator_proto = heap.new_object(Some(async_iterator_proto))?;
    set(
        heap,
        Intrinsic::AsyncGeneratorProto,
        Value::object(async_generator_proto),
    );
    let from_sync = heap.new_object(Some(async_iterator_proto))?;
    set(
        heap,
        Intrinsic::AsyncFromSyncIteratorProto,
        Value::object(from_sync),
    );
    // The function-kind prototypes (27.3.3, 27.4.3, 27.7.3), tagged like qjs. Their
    // `constructor` and `prototype` links are made when the prelude installs the
    // constructors, so that they come in qjs's order (D2150).
    let function_proto = intrinsic_obj(heap, Intrinsic::FunctionProto);
    let tag = |heap: &mut Heap, o: Obj, s: &str| -> Result<(), HeapError> {
        let v = Value::string(heap.alloc_str(s)?);
        let d = PropertyDescriptor::data(v, PropFlags::CONFIGURABLE);
        heap.define_own_property(o, atoms::SYM_TO_STRING_TAG, &d)
            .map(|_| ())
    };
    for (i, name) in [
        (Intrinsic::GeneratorFunctionProto, "GeneratorFunction"),
        (Intrinsic::AsyncFunctionProto, "AsyncFunction"),
        (
            Intrinsic::AsyncGeneratorFunctionProto,
            "AsyncGeneratorFunction",
        ),
    ] {
        let p = heap.new_object(Some(function_proto))?;
        set(heap, i, Value::object(p));
        tag(heap, p, name)?;
    }
    tag(heap, async_generator_proto, "AsyncGenerator")?;
    let registry = heap.new_object(None)?;
    heap.make_dictionary(registry)?;
    set(heap, Intrinsic::SymbolRegistry, Value::object(registry));
    // (The state's two int32 halves, and its seed's: `Math.random` ran where they
    // differ, and only then does a dump write the state, D2837.)
    let random = heap.alloc_value_array(4)?;
    heap.arena_mut()
        .set(random, crate::heap::cells::ValueArray::LEN, 4);
    set(heap, Intrinsic::RandomState, Value::internal(random));
    crate::builtins::number::set_seed(heap, crate::builtins::number::DEFAULT_RANDOM_SEED);
    let global = heap.new_object(Some(object_proto))?;
    // The global object and the lexical record are dictionaries (0083): one-off, with
    // many keys, and the global caches (D1620) keep their table indexes.
    heap.make_dictionary(global)?;
    set(heap, Intrinsic::Global, Value::object(global));
    define_hidden(heap, global, atoms::GLOBAL_THIS, Value::object(global))?;
    // The value properties of the global object: non-writable, non-configurable.
    for (k, v) in [
        (atoms::NAN, Value::NAN),
        (atoms::INFINITY, Value::number(f64::INFINITY)),
        (atoms::UNDEFINED, Value::UNDEFINED),
    ] {
        heap.define_own_property(global, k, &PropertyDescriptor::data(v, PropFlags::NONE))?;
    }
    // The global lexical record: a hidden object, never visible to JS. Bindings are
    // configurable data properties (so `init_global_lex` can always overwrite them);
    // `writable` is false for `const` (D1220).
    let installed = heap.alloc_value_array(INTRINSICS.len() as u32)?;
    for i in 0..INTRINSICS.len() as u32 {
        heap.value_array_set(installed, i, Value::UNDEFINED);
    }
    heap.arena_mut().set(
        installed,
        crate::heap::cells::ValueArray::LEN,
        INTRINSICS.len() as u32,
    );
    heap.arena_mut()
        .set(r, Realm::INSTALLED, crate::heap::Ref(installed));
    let lex = heap.new_object(None)?;
    heap.make_dictionary(lex)?;
    heap.arena_mut()
        .set(r, Realm::GLOBAL_LEX, crate::heap::Ref(lex.offset()));
    let oom = new_error(heap, ErrorKind::InternalError, "out of memory")?;
    set(heap, Intrinsic::OutOfMemoryError, oom);
    // The slim build has no objects of these families (D2411): their prototypes go, and
    // the base image's canonical collection drops them.
    if !crate::FULL {
        for i in SLIM_LEAVES_OUT {
            set(heap, *i, Value::UNDEFINED);
        }
    }
    debug_assert_eq!(heap.arena().get(0, Roots::REALM).as_internal(), Some(r));
    Ok(())
}

/// The JS function installed for intrinsic `id`, or `undefined`.
#[inline]
pub(crate) fn installed_intrinsic(heap: &Heap, id: u16) -> Value {
    let t: crate::heap::Ref = heap.arena().get(realm(heap), Realm::INSTALLED);
    if (id as u32) < heap.value_array_len(t.0) {
        heap.value_array_get(t.0, id as u32)
    } else {
        Value::UNDEFINED
    }
}

#[cfg(djs_prelude)]
pub(crate) fn install_intrinsic(heap: &mut Heap, id: u16, f: Value) {
    let t: crate::heap::Ref = heap.arena().get(realm(heap), Realm::INSTALLED);
    heap.value_array_set(t.0, id as u32, f);
}

/// The global lexical record (Script-mode `let`/`const`, D1220).
pub(crate) fn global_lex(heap: &Heap) -> Obj {
    let r: crate::heap::Ref = heap.arena().get(realm(heap), Realm::GLOBAL_LEX);
    Obj(r.0)
}

/// Declares an uninitialized global lexical binding.
pub(crate) fn declare_global_lexical(
    heap: &mut Heap,
    key: Atom,
    writable: bool,
) -> Result<(), HeapError> {
    let lex = global_lex(heap);
    let d = PropertyDescriptor::data(Value::UNINITIALIZED, PropFlags::new(writable, true, true));
    heap.define_own_property(lex, key, &d)?;
    // A new lexical binding may shadow a global object property a cache remembers.
    let epoch = intrinsic(heap, Intrinsic::GlobalLexEpoch)
        .as_int()
        .unwrap_or(0);
    set_intrinsic(
        heap,
        Intrinsic::GlobalLexEpoch,
        Value::int(epoch.wrapping_add(1)),
    );
    Ok(())
}

/// Sets the value of a declared global lexical binding.
pub(crate) fn set_global_lexical(heap: &mut Heap, key: Atom, v: Value) -> Result<(), HeapError> {
    let lex = global_lex(heap);
    let d = PropertyDescriptor {
        value: Some(v),
        ..Default::default()
    };
    heap.define_own_property(lex, key, &d)?;
    Ok(())
}

/// ToObject of a primitive (`proto` None: the realm's prototype for its type): a
/// Boolean, Number, String, Symbol or BigInt wrapper. `None` for undefined and null.
pub(crate) fn wrap_primitive(
    heap: &mut Heap,
    v: Value,
    proto: Option<Obj>,
) -> Result<Option<Obj>, HeapError> {
    let (class, default) = match v.kind() {
        crate::value::Kind::Bool => (ObjectClass::Boolean, Intrinsic::BooleanProto),
        crate::value::Kind::Number => (ObjectClass::Number, Intrinsic::NumberProto),
        crate::value::Kind::String => (ObjectClass::String, Intrinsic::StringProto),
        crate::value::Kind::Symbol => (ObjectClass::Symbol, Intrinsic::SymbolProto),
        crate::value::Kind::BigInt => (ObjectClass::BigInt, Intrinsic::BigIntProto),
        _ => return Ok(None),
    };
    let proto = proto.unwrap_or_else(|| intrinsic_obj(heap, default));
    let o = heap.alloc_object(class, Some(proto), 1)?;
    heap.set_object_slot(o, 0, v);
    if let Some(s) = v.as_string() {
        // The String exotic object's own properties: the code units (read-only,
        // enumerable), then `length`.
        let len = heap.str_len(s);
        for i in 0..len {
            let u = heap.char_code_at(s, i).expect("index in range");
            let c = heap.alloc_utf16(&[u])?;
            let key = heap.atom_from_index(i)?;
            let d = PropertyDescriptor::data(Value::string(c), PropFlags::ENUMERABLE);
            heap.define_own_property(o, key, &d)?;
        }
        let d = PropertyDescriptor::data(Value::int(len as i32), PropFlags::NONE);
        heap.define_own_property(o, atoms::LENGTH, &d)?;
    }
    Ok(Some(o))
}

/// The realm prototype that property lookups on a primitive go through.
pub(crate) fn primitive_proto(heap: &Heap, v: Value) -> Option<Obj> {
    let i = match v.kind() {
        crate::value::Kind::Bool => Intrinsic::BooleanProto,
        crate::value::Kind::Number => Intrinsic::NumberProto,
        crate::value::Kind::String => Intrinsic::StringProto,
        crate::value::Kind::Symbol => Intrinsic::SymbolProto,
        crate::value::Kind::BigInt => Intrinsic::BigIntProto,
        _ => return None,
    };
    Some(intrinsic_obj(heap, i))
}

/// A new Error object of `kind` with `message`.
pub fn new_error(heap: &mut Heap, kind: ErrorKind, message: &str) -> Result<Value, HeapError> {
    let proto = intrinsic_obj(heap, kind.proto());
    let e = heap.alloc_object(ObjectClass::Error, Some(proto), ObjectClass::Error.slots())?;
    let m = heap.alloc_str(message)?;
    define_hidden(heap, e, atoms::MESSAGE, Value::string(m))?;
    Ok(Value::object(e))
}

/// Records the stack trace of an Error in its slot, which `Error.prototype.stack` reads
/// (D1971); another object gets a non-enumerable own `stack` as before.
pub fn set_error_stack(heap: &mut Heap, o: Obj, stack: Value) -> Result<(), HeapError> {
    if heap.object_class(o) == ObjectClass::Error && heap.object_slot_count(o) > 0 {
        heap.set_object_slot(o, 0, stack);
        return Ok(());
    }
    define_hidden(heap, o, atoms::STACK, stack)
}

/// The realm's original Error constructor, if it is still installed.
pub fn error_constructor(heap: &Heap) -> Option<Obj> {
    let ctors = intrinsic_obj(heap, Intrinsic::Constructors);
    heap.get_own_property(ctors, atoms::ERROR)
        .and_then(|p| p.value())
        .and_then(|v| v.as_object())
}

/// The number of frames a stack trace records: the realm's `Error.stackTraceLimit`
/// (D2030), as qjs reads it: an own data property of `ctor`, the original Error
/// constructor ([`error_constructor`]), read without running code. A number is truncated and clamped to `0..=i32::MAX`
/// (NaN is 0); anything else, an accessor or no property gives 0.
pub fn stack_trace_limit(heap: &Heap, ctor: Option<Obj>) -> u32 {
    let Some(ctor) = ctor else {
        return crate::interp::run::STACK_TRACE_LIMIT;
    };
    match heap.get_own_property(ctor, atoms::STACK_TRACE_LIMIT) {
        Some(Property::Data { value, .. }) => frame_limit(value),
        _ => 0,
    }
}

pub(crate) fn error_constructor_cached(heap: &mut Heap) -> Option<Obj> {
    let ctors = intrinsic_obj(heap, Intrinsic::Constructors);
    if let Some(v) = heap.data_at(ctors, heap.error_ctor_slot, atoms::ERROR) {
        return v.as_object();
    }
    if let Some(i) = heap.own_entry_index(ctors, atoms::ERROR) {
        heap.error_ctor_slot = i;
    }
    error_constructor(heap)
}

pub(crate) fn stack_trace_limit_cached(heap: &mut Heap, ctor: Option<Obj>) -> u32 {
    let Some(c) = ctor else {
        return crate::interp::run::STACK_TRACE_LIMIT;
    };
    if let Some(v) = heap.data_at(c, heap.stack_limit_slot, atoms::STACK_TRACE_LIMIT) {
        return frame_limit(v);
    }
    if let Some(i) = heap.own_entry_index(c, atoms::STACK_TRACE_LIMIT) {
        heap.stack_limit_slot = i;
    }
    stack_trace_limit(heap, ctor)
}

fn frame_limit(v: Value) -> u32 {
    match v.as_number() {
        Some(d) if d > 0.0 => {
            if d >= i32::MAX as f64 {
                i32::MAX as u32
            } else {
                d as u32
            }
        }
        _ => 0,
    }
}

/// The stack trace an Error recorded (`undefined` for anything else, D1971), as Rust
/// readers show it: a stack waiting for `Error.prepareStackTrace` reads as its default
/// string (D2050).
pub fn error_stack_slot(heap: &Heap, v: Value) -> Value {
    super::run::settled_stack(heap, error_stack_raw(heap, v))
}

/// The value in an Error's stack slot, a pending record included (`undefined` for
/// anything else).
pub(crate) fn error_stack_raw(heap: &Heap, v: Value) -> Value {
    match v.as_object() {
        Some(o) if heap.object_class(o) == ObjectClass::Error && heap.object_slot_count(o) > 0 => {
            heap.object_slot(o, 0)
        }
        _ => Value::UNDEFINED,
    }
}

/// The name of the first native error prototype on `o`'s prototype chain.
fn native_error_name(heap: &Heap, o: Obj) -> Option<&'static str> {
    const PROTOS: [(Intrinsic, &str); 9] = [
        (Intrinsic::TypeErrorProto, "TypeError"),
        (Intrinsic::RangeErrorProto, "RangeError"),
        (Intrinsic::ReferenceErrorProto, "ReferenceError"),
        (Intrinsic::SyntaxErrorProto, "SyntaxError"),
        (Intrinsic::InternalErrorProto, "InternalError"),
        (Intrinsic::EvalErrorProto, "EvalError"),
        (Intrinsic::UriErrorProto, "URIError"),
        (Intrinsic::AggregateErrorProto, "AggregateError"),
        (Intrinsic::ErrorProto, "Error"),
    ];
    let mut p = heap.get_prototype_of(o);
    // Prototype chains are acyclic; the bound only guards against a corrupt heap.
    for _ in 0..10_000 {
        let q = p?;
        if let Some((_, name)) = PROTOS
            .iter()
            .find(|(i, _)| intrinsic(heap, *i).as_object() == Some(q))
        {
            return Some(name);
        }
        p = heap.get_prototype_of(q);
    }
    None
}

/// `name: message` of an Error object (via its prototype chain), for host output. `None`
/// if `v` is not an Error.
pub fn error_summary(heap: &Heap, v: Value) -> Option<String> {
    let o = v.as_object()?;
    if heap.object_class(o) != ObjectClass::Error {
        return None;
    }
    // Data properties only: reading an accessor would run JS. A `name` that is an
    // accessor (a frozen realm, such as after pi's lockdown, which turns prototype
    // `name`s into getters) falls back to the nearest native error prototype's name.
    let get = |k: Atom| {
        heap.find_property(o, k)
            .and_then(|(_, p)| p.value())
            .and_then(|v| v.as_string())
            .map(|s| heap.str_to_string(s))
    };
    // A DOMException keeps them in its two slots (D1840): its `name` and `message`
    // are getters.
    if heap.object_slot_count(o) == 2 {
        let slot = |i| {
            heap.object_slot(o, i)
                .as_string()
                .map(|s| heap.str_to_string(s))
                .unwrap_or_default()
        };
        let (name, message) = (slot(0), slot(1));
        return Some(if message.is_empty() {
            name
        } else {
            crate::text::cat!(name, ": ", message)
        });
    }
    let name = get(atoms::NAME)
        .or_else(|| native_error_name(heap, o).map(String::from))
        .unwrap_or_default();
    let message = get(atoms::MESSAGE).unwrap_or_default();
    Some(if message.is_empty() {
        name
    } else {
        crate::text::cat!(name, ": ", message)
    })
}
