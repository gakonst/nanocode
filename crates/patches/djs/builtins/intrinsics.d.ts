// Types and intrinsics of the builtin language (ARCH.md "The builtin language").
//
// The builtin sources (builtins/*.ts) are scripts that see these declarations as
// globals. djs-bgen reads this file for the intrinsic signatures and tags; the `type`
// aliases are only for tsc and editors (bgen's types are fixed, and bgen is stricter:
// see crates/djs-bgen/src/check.rs for the numeric rules).
//
// Every intrinsic is tagged:
// - @leaf: plain Rust that may throw but never runs user code.
// - @reentrant: may run user code, but in a single step: its Rust fast path either
//   finishes or calls exactly one user function (a getter, a setter, the callee of
//   Call) and the result completes the intrinsic (D1085). Calls to reentrant intrinsics
//   are resume points. Anything that needs several steps around user code (ToPrimitive,
//   GetMethod, LengthOfArrayLike, ...) is written in the builtin language instead.
// Intrinsics are called as `Name(...)` here and emitted as `%Name(...)`.

/** Any JS value. */
type Value = unknown;
/** An object. */
type Obj = object;
/** A callable object. Only AsFn produces one. */
type Fn = Obj & { readonly __callable: true };
/** A string (WTF-16). */
type Str = string;
/** A symbol. */
type Sym = symbol;
/** Numbers whose kind bgen tracks; the Rust backend keeps them unboxed. */
type i32 = number & { readonly __num?: "i32" };
type u32 = number & { readonly __num?: "u32" };
type f64 = number;

// ---- Type tests ---------------------------------------------------------------------

/** @leaf */
declare function IsObject(v: Value): boolean;
/** IsCallable ( argument ). @leaf */
declare function IsCallable(v: Value): boolean;
/** IsConstructor ( argument ). @leaf */
declare function IsConstructor(v: Value): boolean;
/** IsArray ( argument ), 7.2.2, through proxies: TypeError for a revoked proxy. @leaf */
declare function IsArray(v: Value): boolean;
/** @leaf */
declare function IsString(v: Value): boolean;
/** @leaf */
declare function IsNumber(v: Value): boolean;
/** @leaf */
declare function IsSymbol(v: Value): boolean;
/** @leaf */
declare function IsBoolean(v: Value): boolean;
/**
 * How many arguments the calling builtin got (its frame's count, before padding), for an
 * optional parameter whose absence differs from undefined (reduce's initialValue)
 * without a rest array. @leaf
 */
declare function ArgumentCount(): i32;
/**
 * Throws `v`: `throw` as an expression, for helpers that the bytecode backend inlines
 * into their callers as one expression (0230). @leaf
 */
declare function Throw(v: Value): Value;

// ---- Casts (an internal error if the value has another type; test first) ------------

/** @leaf */
declare function AsObj(v: Value): Obj;
/** @leaf */
declare function AsFn(v: Value): Fn;
/** @leaf */
declare function AsStr(v: Value): Str;
/** @leaf */
declare function AsNumber(v: Value): f64;
/** @leaf */
declare function AsSym(v: Value): Sym;
/** @leaf */
declare function AsBool(v: Value): boolean;

// ---- Conversions that never run user code --------------------------------------------

/** ToBoolean ( argument ). @leaf */
declare function ToBoolean(v: Value): boolean;
/** ToNumber of a primitive (TypeError for symbols); objects go through ToPrimitive first. @leaf */
declare function ToNumberPrimitive(v: Value): f64;
/** ToString of a primitive (TypeError for symbols); objects go through ToPrimitive first. @leaf */
declare function ToStringPrimitive(v: Value): Str;
/** ToObject ( argument ): TypeError for undefined and null. @leaf */
declare function ToObject(v: Value): Obj;
/** ToIntegerOrInfinity of a number. @leaf */
declare function ToIntegerOrInfinityNumber(n: f64): f64;
/** An integral f64 in u32 range to u32 (an internal error otherwise). @leaf */
declare function F64ToU32(n: f64): u32;
/** An integral f64 in i32 range to i32 (an internal error otherwise). @leaf */
declare function F64ToI32(n: f64): i32;
/** The length of a string in code units. @leaf */
declare function StrLength(s: Str): u32;

// ---- Properties ----------------------------------------------------------------------
// Keys are primitive property keys (strings, symbols, numbers); an object key must go
// through ToPropertyKey first.

/** Get ( O, P ): may call a getter. @reentrant */
declare function Get(o: Obj, key: Value): Value;
/** GetV ( V, P ): Get with a primitive base looked up on its prototype. @reentrant */
declare function GetV(v: Value, key: Value): Value;
/** Set ( O, P, V, true ): may call a setter; throws on failure. @reentrant */
declare function Set(o: Obj, key: Value, v: Value): void;
/** HasProperty ( O, P ): a proxy's `has` trap. @reentrant */
declare function HasProperty(o: Obj, key: Value): boolean;
/** CreateDataPropertyOrThrow ( O, P, V ): a proxy's `defineProperty` trap. @reentrant */
declare function CreateDataPropertyOrThrow(o: Obj, key: Value, v: Value): void;
/** DeletePropertyOrThrow ( O, P ): a proxy's `deleteProperty` trap. @reentrant */
declare function DeletePropertyOrThrow(o: Obj, key: Value): void;

// ---- Calls ---------------------------------------------------------------------------

/** Call ( F, V, args ): TypeError if f is not callable. @reentrant */
declare function Call(f: Value, thisArg: Value, ...args: Value[]): Value;
/** Construct ( F, args ) with newTarget F: TypeError if f is not a constructor. @reentrant */
declare function Construct(f: Value, ...args: Value[]): Obj;

// ---- Objects, errors, symbols ----------------------------------------------------------

/** OrdinaryObjectCreate ( proto ). @leaf */
declare function ObjectCreate(proto: Value): Obj;
/** ArrayCreate ( length ): RangeError above 2**32 - 1. @leaf */
declare function ArrayCreate(length: f64): Obj;
/** A new TypeError of the current realm. @leaf */
declare function NewTypeError(message: Str): Obj;
/** A TypeError "<name> is not a constructor" with f's internal name, as QuickJS words it. @leaf */
declare function NewNotConstructorError(f: Value): Obj;
/** A new SyntaxError of the current realm (0039). @leaf */
declare function NewSyntaxError(message: Str): Obj;
/** A new ReferenceError of the current realm (0051). @leaf */
declare function NewReferenceError(message: Str): Obj;
/** A new RangeError of the current realm. @leaf */
declare function NewRangeError(message: Str): Obj;
/** The well-known symbol Symbol.<name>. @leaf */
declare function WellKnownSymbol(name: Str): Sym;

// ---- Realm objects, own properties (0031) ------------------------------------------------

/**
 * A realm intrinsic object by name: ObjectPrototype, FunctionPrototype, ArrayPrototype,
 * ErrorPrototype and the native error prototypes (TypeErrorPrototype, ...). @leaf
 */
declare function RealmIntrinsic(name: Str): Obj;
/**
 * GetPrototypeFromConstructor where it runs no user code (0236): newTarget's `prototype`
 * if that is an object, else the realm's prototype `fallback` (named as for
 * RealmIntrinsic), also for an undefined newTarget; undefined when reading `prototype`
 * would call a getter or a proxy trap. @leaf
 */
declare function ConstructorPrototype(newTarget: Value, fallback: Str): Value;
/** An object with [[ErrorData]] and prototype `proto`. @leaf */
declare function ErrorCreate(proto: Obj): Obj;
/** A DOMException: an object with [[ErrorData]], its name and message in slots (D1840). @leaf */
declare function DOMExceptionCreate(proto: Obj, name: Str, message: Str): Obj;
/** Slot i (0 name, 1 message) of a DOMException; undefined if v is not one. @leaf */
declare function DOMExceptionField(v: Value, i: i32): Value;
/** The legacy code of a DOMException name, 0 if it has none. @leaf */
declare function DOMExceptionCode(name: Str): i32;
/** The name of the legacy code constant i, 1 to 25 (`INDEX_SIZE_ERR`, ...). @leaf */
declare function DOMExceptionConstant(i: i32): Str;
/** btoa's base64 of the code units of s; undefined if one is above 0xFF. @leaf */
declare function Btoa(s: Str): Value;
/** atob's forgiving-base64 decoding of s into a string of bytes; undefined if invalid. @leaf */
declare function Atob(s: Str): Value;
/**
 * Records `o.stack` (D1282) from the current frames, without the prelude frames at the
 * top: an Error constructor does not show itself. With Error.prepareStackTrace it is
 * pending until FinishErrorStack (D2050). @leaf
 */
declare function CaptureStack(o: Obj): void;
/**
 * The stack an Error object recorded (D1971), a pending one too (D2050), or undefined
 * for any other value. @leaf
 */
declare function ErrorStack(v: Value): Value;
/**
 * The stack trace of the current frames (the calling builtin left out): a string, or a
 * pending stack with Error.prepareStackTrace (D2050). With a function `below`, only the
 * frames below its topmost call, none if it is not on the stack (Error.captureStackTrace,
 * D2010). @leaf
 */
declare function StackTraceValue(below: Value): Value;
/** Whether v is a stack waiting for Error.prepareStackTrace (D2050). @leaf */
declare function IsPendingStack(v: Value): boolean;
/** Sets the stack slot of an Error (an own `stack` of anything else). @leaf */
declare function SetErrorStack(o: Obj, v: Value): void;
/**
 * Slot i of a CallSite (0 function, 1 name, 2 file, 3 line, 4 column, 5 native,
 * 6 constructor); TypeError "CallSite object expected" for anything else. @leaf
 */
declare function CallSiteSlot(v: Value, i: i32): Value;
/**
 * DefinePropertyOrThrow ( O, P, desc ) with a complete data descriptor; `attrs` are
 * writable 1, enumerable 2, configurable 4. @reentrant
 */
declare function DefineDataPropertyOrThrow(o: Obj, key: Value, v: Value, attrs: i32): void;
/**
 * [[DefineOwnProperty]] with a descriptor record from ToPropertyDescriptor (an object
 * whose own data properties are the present fields). False if it was rejected. @reentrant
 */
declare function DefineOwnPropertyFromRecord(o: Obj, key: Value, desc: Obj): boolean;
/** FromPropertyDescriptor ( [[GetOwnProperty]](o, key) ): undefined if absent. @reentrant */
declare function GetOwnPropertyDescriptorObject(o: Obj, key: Value): Value;
/** HasOwnProperty ( O, P ). @reentrant */
declare function HasOwnProperty(o: Obj, key: Value): boolean;
/**
 * The attributes of an own property: -1 if absent, else writable 1, enumerable 2,
 * configurable 4, accessor 8. @reentrant
 */
declare function OwnPropertyAttributes(o: Obj, key: Value): i32;
/**
 * The own keys of o as a new array, in [[OwnPropertyKeys]] order: kind 0 strings,
 * 1 symbols, 2 both, 3 enumerable strings. @reentrant
 */
declare function OwnPropertyKeys(o: Obj, kind: i32): Obj;

/** [[GetPrototypeOf]] ( ): an object or null. @reentrant */
declare function GetPrototypeOf(o: Obj): Value;
/** [[SetPrototypeOf]] ( V ) with an object or null: false if refused. @reentrant */
declare function SetPrototypeOf(o: Obj, proto: Value): boolean;
/** [[PreventExtensions]] ( ). @reentrant */
declare function PreventExtensions(o: Obj): boolean;
/** [[IsExtensible]] ( ). @reentrant */
declare function IsExtensible(o: Obj): boolean;
/** SetIntegrityLevel ( O, level ): sealed, or frozen if `frozen`. @reentrant */
declare function SetIntegrityLevel(o: Obj, frozen: boolean): boolean;
/** TestIntegrityLevel ( O, level ): sealed, or frozen if `frozen`. @reentrant */
declare function TestIntegrityLevel(o: Obj, frozen: boolean): boolean;
/** SameValue ( x, y ). @leaf */
declare function SameValue(x: Value, y: Value): boolean;
/**
 * The builtinTag of Object.prototype.toString: Array, Arguments, Function, Error,
 * Boolean, Number, String, Date, RegExp, else Object. @leaf
 */
declare function ObjectClassTag(o: Obj): Str;
/** A Boolean, Number, String or Symbol wrapper of the primitive v with prototype proto. @leaf */
declare function WrapPrimitive(v: Value, proto: Obj): Obj;
/**
 * The primitive of a wrapper object ([[BooleanData]] and so on), a primitive itself,
 * or undefined for other objects. @leaf
 */
declare function PrimitiveValue(v: Value): Value;

// ---- Reflect (0038) ----------------------------------------------------------------------

/** Construct ( F, args, newTarget ) with the arguments in a dense array. @reentrant */
declare function ConstructWithArray(f: Value, args: Obj, newTarget: Value): Obj;
/**
 * [[Construct]] of an ordinary (non-builtin) function f with `thisArg` as the new
 * object (OrdinaryCreateFromConstructor done by the caller) and new.target. @reentrant
 */
declare function ConstructWithThis(f: Obj, thisArg: Obj, args: Obj, newTarget: Value): Obj;
/** [[Get]] ( P, Receiver ): a getter runs with `receiver` as this. @reentrant */
declare function GetWithReceiver(o: Obj, key: Value, receiver: Value): Value;
/** OrdinarySet ( O, P, V, Receiver ): false when refused; a setter runs with receiver. @reentrant */
declare function SetWithReceiver(o: Obj, key: Value, v: Value, receiver: Value): boolean;
/** [[Delete]] ( P ): false for a non-configurable property. @reentrant */
declare function DeleteProperty(o: Obj, key: Value): boolean;

// ---- Proxy (0045, heap/proxy.rs, builtins/proxy.ts) ---------------------------------------
//
// The internal-method intrinsics above (Get, Set, HasProperty, OwnPropertyKeys,
// GetPrototypeOf, ...) take proxies too: on one they call the builtin of proxy.ts that
// runs the trap (interp/run/proxy.rs), so they are reentrant.

/** Whether v is a Proxy exotic object (revoked or not). @leaf */
declare function IsProxy(v: Value): boolean;
/** A new proxy (ProxyCreate's object, 10.5.14): callable/constructor as target is. @leaf */
declare function NewProxy(target: Obj, handler: Obj): Obj;
/** [[ProxyTarget]] of a proxy: TypeError "revoked proxy" once revoked. @leaf */
declare function ProxyTarget(p: Obj): Obj;
/** [[ProxyHandler]] of a proxy: TypeError "revoked proxy" once revoked. @leaf */
declare function ProxyHandler(p: Obj): Obj;
/** Revokes a proxy (target and handler become null). @leaf */
declare function ProxyRevoke(p: Obj): void;

// ---- Map, Set, WeakMap, WeakSet (0038, heap/collection.rs) --------------------------

/** A new empty collection: class 0 Map, 1 Set, 2 WeakMap, 3 WeakSet. @leaf */
declare function CollectionCreate(cls: i32, proto: Obj): Obj;
/** The class of a collection object (0 Map, 1 Set, 2 WeakMap, 3 WeakSet), -1 otherwise. @leaf */
declare function CollectionClass(v: Value): i32;
/** The value of key (SameValueZero), or undefined. @leaf */
declare function CollectionGet(m: Obj, key: Value): Value;
/** Whether key is present. @leaf */
declare function CollectionHas(m: Obj, key: Value): boolean;
/** Adds or replaces key (-0 becomes +0); a Set passes true as the value. @leaf */
declare function CollectionSet(m: Obj, key: Value, value: Value): void;
/** Removes key; false if it was absent. @leaf */
declare function CollectionDelete(m: Obj, key: Value): boolean;
/** Removes every entry. @leaf */
declare function CollectionClear(m: Obj): void;
/** The number of entries. @leaf */
declare function CollectionSize(m: Obj): f64;
/**
 * A Map Iterator or Set Iterator over m (kind 0 keys, 1 values, 2 entries): it visits
 * entries added while iterating and skips deleted ones. @leaf
 */
declare function CollectionIterator(m: Obj, kind: i32): Obj;
/**
 * %MapIteratorPrototype%.next / %SetIteratorPrototype%.next: the iterator result.
 * TypeError "Map Iterator object expected" (or Set) for anything else. @leaf
 */
declare function CollectionIteratorNext(it: Value, isSet: boolean): Obj;
/**
 * Whether `adder` is the installed Map.prototype.set (kind 0) or Set.prototype.add (1):
 * the constructors then store directly (0080). @leaf
 */
declare function IsOriginalCollectionAdder(adder: Value, kind: i32): boolean;
/**
 * Whether `v` is the realm's %Array%: Array.from then makes its array itself, as
 * Construct(%Array%, ..) would (its `prototype` cannot change). @leaf
 */
declare function IsArrayConstructor(v: Value): boolean;
/** Advances an iterator to the next entry; false when done. @leaf */
declare function CollectionIteratorStep(it: Obj): boolean;
/** The key of the entry the last step reached. @leaf */
declare function CollectionIteratorKey(it: Obj): Value;
/** The value of the entry the last step reached. @leaf */
declare function CollectionIteratorValue(it: Obj): Value;
/** CanBeHeldWeakly ( v ): an object, or a symbol not in the Symbol.for registry. @leaf */
declare function CanBeHeldWeakly(v: Value): boolean;

// ---- Date (0038, builtins/date.rs; local time is the host's fixed offset, D1383) --------

/** The host's clock (ms since the epoch; Vm::set_now). @leaf */
declare function DateNow(): f64;
/** A new Date object with prototype proto and time value t. @leaf */
declare function DateCreate(proto: Obj, t: f64): Obj;
/** thisTimeValue: the time value of a Date, TypeError "not a Date object" otherwise. @leaf */
declare function DateValue(v: Value): f64;
/** Stores time value t in a Date (TypeError "not a Date object" otherwise); returns t. @leaf */
declare function DateSetValue(v: Value, t: f64): f64;
/**
 * A time value from the Numbers year, month[, date, hours, minutes, seconds, ms] in a
 * dense array (the constructor's and Date.UTC's form: years 0-99 are 1900-1999). @leaf
 */
declare function DateFromFields(fields: Obj, local: boolean): f64;
/**
 * Field `field` of t (0 year, 1 month, 2 date, 3 hours, 4 minutes, 5 seconds, 6 ms, 7
 * weekday), local or UTC; NaN for an invalid date. @leaf
 */
declare function DateGetField(t: f64, field: i32, local: boolean): f64;
/**
 * A setter's new time value: fields first.. of t replaced by the Numbers in args (a
 * dense array), local or UTC (qjs's set_date_field). @leaf
 */
declare function DateSetFields(t: f64, first: i32, args: Obj, local: boolean): f64;
/**
 * Formats t: fmt 0 toUTCString, 1 toString, 2 toISOString (RangeError for NaN), 3
 * toLocale*; part 1 date, 2 time, 3 both. "Invalid Date" for NaN. @leaf
 */
declare function DateFormat(t: f64, fmt: i32, part: i32): Str;
/** Date.parse of a string (ISO and the formats qjs's toString/toUTCString emit). @leaf */
declare function DateParse(s: Str): f64;
/** getTimezoneOffset of t: minutes (UTC - local), NaN for an invalid date. @leaf */
declare function DateTimezoneOffset(t: f64): f64;

// ---- ArrayBuffer, typed arrays, DataView (0039, heap/typed.rs) ----------------------------

/**
 * AllocateArrayBuffer: byteLength zeroed bytes; max is the maxByteLength of a resizable
 * buffer, true for an immutable one (D1970), or undefined. RangeError "invalid array
 * buffer length" (or "... max length"). @leaf
 */
declare function ArrayBufferCreate(proto: Obj, byteLength: f64, max: Value): Obj;
/**
 * A field of an ArrayBuffer: 0 byteLength, 1 maxByteLength, 2 resizable, 3 detached,
 * 4 immutable (D1970).
 * TypeError "ArrayBuffer object expected" for anything else. @leaf
 */
declare function BufferInfo(b: Value, field: i32): Value;
/** DetachArrayBuffer. @leaf */
declare function BufferDetach(b: Obj): void;
/** Resizes a resizable buffer in place (new bytes are 0). @leaf */
declare function BufferResize(b: Obj, byteLength: f64): void;
/**
 * A new buffer with the first bytes of b (max: a Number for resizable, true for
 * immutable), b detached; an immutable b is a TypeError. @leaf
 */
declare function BufferTransfer(b: Obj, byteLength: f64, max: Value): Obj;
/** Copies count bytes from src at from to dst at to (memmove). @leaf */
declare function BufferCopy(src: Obj, from: f64, dst: Obj, to: f64, count: f64): void;
/**
 * Sorts a dense array of Numbers numerically in place (a typed array's default order:
 * -0 before +0, NaN last; stable), or one of BigInts by value. @leaf
 */
declare function SortNumbers(list: Obj): void;
/** A DataView over buffer (byteLength -1: length-tracking). @leaf */
declare function DataViewCreate(proto: Obj, buffer: Obj, byteOffset: f64, byteLength: f64): Obj;
/**
 * A slot of a DataView: 0 buffer, 1 byteOffset, 2 byteLength (-1: length-tracking).
 * TypeError "DataView object expected" for anything else. @leaf
 */
declare function DataViewField(v: Value, field: i32): Value;
/**
 * The element of kind at byte `at` of the view's buffer (bounds checked by the caller): a
 * Number, or a BigInt for kinds 10 and 11. @leaf
 */
declare function DataViewGet(view: Obj, at: f64, kind: i32, little: boolean): Value;
/** Writes v (a Number, or a BigInt for kinds 10 and 11) as kind at byte `at` of the view's buffer. @leaf */
declare function DataViewSet(view: Obj, at: f64, kind: i32, v: Value, little: boolean): void;
/** Whether v is a DataView. @leaf */
declare function IsDataView(v: Value): boolean;
/** The constructor installed as `name` (the realm's %Name%, whatever the global is now). @leaf */
declare function RealmConstructor(name: Str): Obj;
/** Whether v is a typed array. @leaf */
declare function IsTypedArray(v: Value): boolean;
/** A typed array of `kind` over buffer (length -1: length-tracking). @leaf */
declare function TypedArrayCreate(proto: Obj, kind: i32, buffer: Obj, byteOffset: f64, length: f64): Obj;
/**
 * A field of a typed array: 0 kind, 1 buffer, 2 byteOffset, 3 length, 4 byteLength
 * (2-4 are 0 when out of bounds), 5 out of bounds or detached, 6 length-tracking, 7
 * [[ByteOffset]] (also when out of bounds).
 * TypeError "not a TypedArray" for anything else. @leaf
 */
declare function TypedArrayField(o: Value, field: i32): Value;
/** Element i (undefined if not a valid index). @leaf */
declare function TypedArrayGet(o: Obj, i: f64): Value;
/** Writes v (a Number, or a BigInt for a BigInt array) to element i (nothing if not a valid index). @leaf */
declare function TypedArraySet(o: Obj, i: f64, v: Value): void;
/** Copies count elements from src (at from) to dst (at to), converting; overlaps ok. @leaf */
declare function TypedArrayCopy(src: Obj, from: f64, dst: Obj, to: f64, count: f64): void;
/** The prototype of the typed array constructor for kind (the realm's). @leaf */
declare function TypedArrayKindProto(kind: i32): Obj;
/** The bytes of a Uint8Array as base64 (TypeError if detached or out of bounds). @leaf */
declare function Base64Encode(o: Obj, url: boolean, omitPadding: boolean): Str;
/** The bytes of a Uint8Array as lowercase hex (TypeError if detached). @leaf */
declare function HexEncode(o: Obj): Str;
/**
 * FromBase64 ( string, alphabet, lastChunkHandling, maxLength ): a record object
 * { read, bytes (an ArrayBuffer), error } (last: 0 loose, 1 strict, 2 stop-before-partial). @leaf
 */
declare function Base64Decode(s: Str, url: boolean, last: i32, max: f64): Obj;
/** FromHex ( string, maxLength ): the same record. @leaf */
declare function HexDecode(s: Str, max: f64): Obj;

// ---- Functions (0031) -----------------------------------------------------------------

/** Call ( F, V, args ) with the arguments in a dense array. @reentrant */
declare function CallWithArray(f: Value, thisArg: Value, args: Obj): Value;
/** BoundFunctionCreate ( target, boundThis, boundArgs ), without name and length. @leaf */
declare function BoundFunctionCreate(target: Fn, boundThis: Value, args: Obj): Obj;
/** [[BoundTargetFunction]] of a bound function, or undefined for anything else. @leaf */
declare function BoundTargetFunction(f: Value): Value;
/** The source text of a function (natives: `function name() { [native code] }`). @leaf */
declare function FunctionToString(f: Value): Str;

// ---- Numbers, symbols (0031) ------------------------------------------------------------

/** Number::exponentiate ( base, exponent ). @leaf */
declare function NumberPow(base: f64, exponent: f64): f64;
/** SymbolDescriptiveString ( sym ): `Symbol(description)`. @leaf */
declare function SymbolDescriptiveString(s: Sym): Str;
/**
 * Compares two strings by code units (IsLessThan for strings): -1, 0 or 1. Also
 * localeCompare's order (no Intl). @leaf
 */
declare function CompareStrings(a: Str, b: Str): f64;
/** The code points of s as an array of strings (the string iterator's values). @leaf */
declare function StringCodePoints(s: Str): Obj;
/**
 * Sorts the dense array `list` in place by SortCompare without a comparefn (undefined
 * last, then ToString order; stable) if every element is a primitive other than a
 * symbol. Returns false, leaving it unchanged, otherwise. @leaf
 */
declare function SortPrimitives(list: Obj): boolean;
/**
 * The state of a stable bottom-up merge sort of the dense array `list` (made by
 * SortList, never seen by user code) by SortCompare: its undefined elements move to the
 * end of the list (they sort last and are never compared), the others into the state.
 * @leaf
 */
declare function SortStart(list: Obj): Obj;
/**
 * Places the pending comparison's element by its result v, a Number (the right one if
 * v > 0), then merges on to the next comparison: true if there is one (SortOperand reads
 * it), false once the list is sorted (written back into it). @leaf
 */
declare function SortStep(st: Obj, v: Value): boolean;
/** The left (0) or right (1) value of the pending comparison of SortStep. @leaf */
declare function SortOperand(st: Obj, which: f64): Value;
/**
 * Splice's element moves in one step on a plain dense Array (no holes, default
 * attributes, extensible, writable length, no index properties on the prototype chain):
 * o[start .. start + deleteCount) becomes itemCount undefined slots and `length`
 * follows. False, with nothing changed, for any other object. @leaf
 */
declare function ArraySpliceDense(o: Obj, start: f64, deleteCount: f64, itemCount: f64): boolean;
/**
 * Whether `ArrayPopDense(o)` applies: an Array with only dense elements, a writable
 * length, its last element present and no index properties on its prototype chain,
 * where pop's Get, DeletePropertyOrThrow and Set observe nothing else (0080). @leaf
 */
declare function ArrayCanPopDense(o: Obj): boolean;
/** Array.prototype.pop of an array `ArrayCanPopDense` accepts. @leaf */
declare function ArrayPopDense(o: Obj): Value;
/**
 * fill's Sets on an Array while each is a plain store (an existing dense element, or an
 * append or hole fill of an extensible Array without index properties on its prototype
 * chain), from k up to fin: the index the general loop continues from (0080). @leaf
 */
declare function ArrayFillDense(o: Obj, value: Value, k: f64, fin: f64): f64;
/**
 * push(v)'s Set and length update in one step when the Set is a plain append to the
 * Array `o` (extensible, writable length, no holes at its end, no index properties on
 * its prototype chain). The new length, or -1 with nothing changed (0080). @leaf
 */
declare function ArrayPushDense(o: Obj, v: Value): f64;
/**
 * The search loops of indexOf (mode 0: `from` up to len, strict equality), lastIndexOf
 * (1: `from` down to 0) and includes (2: up to len, SameValueZero) over an Array whose
 * elements in the range are all present in dense storage, where HasProperty and Get
 * observe nothing: the index found or -1. -2, for the general loop, for anything else
 * (a hole, a shorter array, another object). @leaf
 */
declare function ArraySearchDense(o: Obj, search: Value, from: f64, len: f64, mode: i32): f64;
/**
 * slice's copy loop: `a[0 .. fin - k) = o[k .. fin)` when every element of `o` in the
 * range is present in dense storage and `a` is an extensible Array without index
 * properties (a fresh ArraySpeciesCreate result), so HasProperty, Get and
 * CreateDataPropertyOrThrow observe nothing. False, with nothing changed, otherwise.
 * @leaf
 */
declare function ArraySliceDense(o: Obj, k: f64, fin: f64, a: Obj): boolean;
/**
 * The dense part of the loops of the callback builtins (0137). From `k`, while `o` is an
 * Array whose element k is present in its dense store (so HasProperty and Get observe
 * nothing), calls `f(o[k], k, o)` with `thisArg` and completes the step by `mode`:
 * - 0 forEach;
 * - 1 map: CreateDataPropertyOrThrow(a, k, result);
 * - 2 filter: for a truthy result, CreateDataPropertyOrThrow(a, to, o[k]), with `to` the
 *   length of `a`, a fresh Array (the receiver's species is plain);
 * - 3 some, 4 every: a truthy (some) or falsy (every) result decides;
 * - 5 find: a truthy result decides, and o[k] goes to element 0 of the Array `a`;
 * - 6 findIndex: a truthy result decides;
 * - 7 reduce: calls `f(acc, o[k], k, o)` with `this` undefined; the accumulator starts as
 *   `thisArg`, and each result goes to element 0 of the Array `a` (so `a` holds the
 *   accumulator once the returned k is past the starting one);
 * - 8 Array.from of an array-like: calls `f(o[k], k)` and does
 *   CreateDataPropertyOrThrow(a, k, result); `o` is any ordinary object (or array or
 *   function) whose element k, along its prototype chain, is a data property or absent
 *   (undefined), so Get observes nothing (D2306);
 * - 9 a replacer function over the matches of a RegExp without groups: `o` holds the
 *   matched strings, `a` their positions and `thisArg` the string; calls
 *   `f(o[k], a[k], thisArg)` with `this` undefined and puts the result in `a` at k; a
 *   result that is not a string decides (the builtin converts it).
 * One frame for `f`, entered again per element. Returns the k the builtin's own loop goes
 * on from (`len`: done; `k` itself when `f` is not a plain closure or `a` is not as
 * required), or -1 - k when the result at k decided. @reentrant
 */
declare function EachDense(mode: i32, o: Obj, k: f64, len: f64, f: Value, thisArg: Value, a: Value): f64;
/**
 * join over o[0 .. len) when every element is present in dense storage and a primitive
 * other than a symbol (ToString runs no user code): the string, or undefined for the
 * general loop. @leaf
 */
declare function ArrayJoinDense(o: Obj, len: f64, sep: Str): Value;
/**
 * EnumerableOwnProperties ( o, kind ) for kind 1 (values) and 2 (entries) in one step
 * when no enumerable string-keyed own property is an accessor and `o` is not exotic
 * (no Proxy): the array, or undefined for the general loop (0080). @leaf
 */
declare function EnumerableOwnDataProperties(o: Obj, kind: i32): Value;
/**
 * Writes values[0 .. length) into the typed array `o` (at least that long) when
 * `values` is an Array whose elements are all present in dense storage and primitives
 * of `o`'s content type (ToNumber / ToBigInt run no user code). False, with nothing
 * written, otherwise (0080). @leaf
 */
declare function TypedArraySetFromDense(o: Obj, values: Obj): boolean;
// ---- Strings (0034): leaves on the heap representation -------------------------------

/**
 * localeCompare without Intl (as qjs): the difference of the first differing code
 * points, else -1, 0 or 1 by length. @leaf
 */
declare function StrLocaleCompare(a: Str, b: Str): f64;
/** The code unit at i as a Number, NaN out of range. @leaf */
declare function StrCharCodeAt(s: Str, i: f64): f64;
/** The code point at i (a surrogate pair combined), undefined out of range. @leaf */
declare function StrCodePointAt(s: Str, i: f64): Value;
/** s[start .. end) in code units; the indexes are integers, clamped by the caller. @leaf */
declare function StrSubstring(s: Str, start: f64, end: f64): Str;
/** The first index ≥ from where search occurs, or -1. @leaf */
declare function StrIndexOf(s: Str, search: Str, from: f64): f64;
/** The last index ≤ from where search occurs, or -1. @leaf */
declare function StrLastIndexOf(s: Str, search: Str, from: f64): f64;
/** Full Unicode upper- or lowercase (SpecialCasing, Final_Sigma; no locales). @leaf */
declare function StrToCase(s: Str, upper: boolean): Str;
/** Trims WhiteSpace and LineTerminators: where 0 both ends, 1 start, 2 end. @leaf */
declare function StrTrim(s: Str, where: i32): Str;
/** IsStringWellFormedUnicode. @leaf */
declare function StrIsWellFormed(s: Str): boolean;
/** Lone surrogates replaced by U+FFFD. @leaf */
declare function StrToWellFormed(s: Str): Str;
/** s repeated n times (an integer ≥ 0; RangeError past the string length limit). @leaf */
declare function StrRepeat(s: Str, n: f64): Str;
/** A string of the code units ToUint16(n) of the Numbers in a dense array. @leaf */
declare function StrFromCodeUnits(list: Obj): Str;
/** A string of the (valid) code points in a dense array. @leaf */
declare function StrFromCodePoints(list: Obj): Str;
/**
 * String.prototype.normalize for form 0 NFC, 1 NFD, 2 NFKC, 3 NFKD: the normalized string;
 * undefined only when djs is built without the `unicode-normalization` feature and the
 * string would need its tables. @leaf
 */
declare function StrNormalize(s: Str, form: i32): Value;
/** A new symbol; the description is a string or undefined. @leaf */
declare function NewSymbol(description: Value): Sym;
/** [[Description]] of a symbol: a string or undefined. @leaf */
declare function SymbolDescription(s: Sym): Value;
/** Symbol.for's GlobalSymbolRegistry lookup or insertion. @leaf */
declare function SymbolFor(key: Str): Sym;
/** Symbol.keyFor's registry lookup: the key, or undefined. @leaf */
declare function SymbolKeyFor(s: Sym): Value;
/**
 * Number::toString ( x, radix ) for an integral radix (RangeError outside 2..36),
 * Number.prototype.toString. @leaf
 */
declare function NumberToString(x: f64, radix: f64): Str;
/** Number.prototype.toFixed after ToIntegerOrInfinity(fractionDigits). @leaf */
declare function NumberToFixed(x: f64, digits: f64): Str;
/** Number.prototype.toExponential; digits is undefined or an integral Number. @leaf */
declare function NumberToExponential(x: f64, digits: Value): Str;
/** Number.prototype.toPrecision; precision is undefined or an integral Number. @leaf */
declare function NumberToPrecision(x: f64, precision: Value): Str;
/** The global parseFloat after ToString. @leaf */
declare function StringParseFloat(s: Str): f64;
/** The global parseInt after ToString; radix is ToInt32(radix). @leaf */
declare function StringParseInt(s: Str, radix: i32): f64;
/**
 * Encode ( string, extraUnescaped ), 19.2.6.5, after ToString: encodeURI keeps the
 * reserved characters (`component` false), encodeURIComponent escapes them. URIError for
 * a lone surrogate. @leaf
 */
declare function UriEncode(s: Str, component: boolean): Str;
/**
 * Decode ( string, preserveEscapeSet ), 19.2.6.6, after ToString: decodeURI keeps the
 * escapes of reserved characters (`component` false). URIError for a malformed escape or
 * UTF-8 sequence. @leaf
 */
declare function UriDecode(s: Str, component: boolean): Str;
/** escape ( string ), B.2.1.1, after ToString. @leaf */
declare function StrEscape(s: Str): Str;
/** unescape ( string ), B.2.1.2, after ToString. @leaf */
declare function StrUnescape(s: Str): Str;
/**
 * PerformEval's parse (19.2.1.1) for indirect eval: the source as a Script closure, to be
 * called with `this` = the global object. SyntaxError for invalid source. Charges fuel per
 * source byte. @leaf
 */
declare function CompileEval(src: Str): Obj;
/**
 * CreateDynamicFunction's parse (20.2.1.1.1): `kind` 0 function, 1 generator, 2 async, 3
 * async generator; `params` joined with ","; a Script closure whose call returns the new
 * function. SyntaxError for invalid source. Charges fuel per source byte. @leaf
 */
declare function CompileFunction(kind: i32, params: Str, body: Str): Obj;
/** The realm's global object. @leaf */
declare function GlobalObject(): Obj;
/** Queues the job MicrotaskJob(callback) (queueMicrotask). @leaf */
declare function EnqueueMicrotask(callback: Value): void;
/** ToInt32 of a number. @leaf */
declare function NumberToInt32(n: f64): i32;
/** ToUint32 of a number, as an f64. @leaf */
declare function NumberToUint32(n: f64): f64;

// ---- Math (0031): the libm functions (D1150) ---------------------------------------------

/** @leaf */
declare function F64Abs(x: f64): f64;
/** @leaf */
declare function F64Acos(x: f64): f64;
/** @leaf */
declare function F64Acosh(x: f64): f64;
/** @leaf */
declare function F64Asin(x: f64): f64;
/** @leaf */
declare function F64Asinh(x: f64): f64;
/** @leaf */
declare function F64Atan(x: f64): f64;
/** @leaf */
declare function F64Atanh(x: f64): f64;
/** @leaf */
declare function F64Atan2(y: f64, x: f64): f64;
/** @leaf */
declare function F64Cbrt(x: f64): f64;
/** @leaf */
declare function F64Hypot(x: f64, y: f64): f64;
/** @leaf */
declare function F64Ceil(x: f64): f64;
/** Math.clz32 of a number (ToUint32 inside). @leaf */
declare function F64Clz32(x: f64): f64;
/** @leaf */
declare function F64Cos(x: f64): f64;
/** @leaf */
declare function F64Cosh(x: f64): f64;
/** @leaf */
declare function F64Exp(x: f64): f64;
/** @leaf */
declare function F64Expm1(x: f64): f64;
/** @leaf */
declare function F64Floor(x: f64): f64;
/** Rounds to the nearest f32. @leaf */
declare function F64Fround(x: f64): f64;
/** Rounds to the nearest binary16 value (Math.f16round). @leaf */
declare function F64F16round(x: f64): f64;
/** The exactly rounded sum of the Numbers of the dense Array list (Math.sumPrecise). @leaf */
declare function F64SumPrecise(list: Obj): f64;
/** Math.imul of two numbers (ToUint32 inside). @leaf */
declare function F64Imul(a: f64, b: f64): f64;
/** @leaf */
declare function F64Log(x: f64): f64;
/** @leaf */
declare function F64Log1p(x: f64): f64;
/** @leaf */
declare function F64Log10(x: f64): f64;
/** @leaf */
declare function F64Log2(x: f64): f64;
/** Math.round: half up, -0 kept. @leaf */
declare function F64Round(x: f64): f64;
/** @leaf */
declare function F64Sign(x: f64): f64;
/** @leaf */
declare function F64Sin(x: f64): f64;
/** @leaf */
declare function F64Sinh(x: f64): f64;
/** @leaf */
declare function F64Sqrt(x: f64): f64;
/** @leaf */
declare function F64Tan(x: f64): f64;
/** @leaf */
declare function F64Tanh(x: f64): f64;
/** @leaf */
declare function F64Trunc(x: f64): f64;
/** The next number in [0, 1) of the realm's generator (xorshift128+, seeded by the host). @leaf */
declare function MathRandom(): f64;

// ---- Iterators ------------------------------------------------------------------------

/** A new Array Iterator over o (kind 0: keys, 1: values, 2: entries), 23.1.5.1. @leaf */
declare function CreateArrayIterator(o: Obj, kind: i32): Obj;
/**
 * The object an Array Iterator iterates, or undefined once it is exhausted. TypeError
 * "Array Iterator object expected" if it is not an Array Iterator.
 * @leaf
 */
declare function ArrayIteratorTarget(it: Value): Value;
/** The next index of an Array Iterator. @leaf */
declare function ArrayIteratorIndex(it: Value): f64;
/** @leaf */
declare function ArrayIteratorSetIndex(it: Value, index: f64): void;
/** Marks an Array Iterator exhausted (its target becomes undefined). @leaf */
declare function ArrayIteratorFinish(it: Value): void;
/** The kind of an Array Iterator (0: keys, 1: values, 2: entries). @leaf */
declare function ArrayIteratorKind(it: Value): i32;
/**
 * A String Iterator over s (CreateStringIterator, 22.1.5.1): an Array Iterator object of
 * kind 3 with %StringIteratorPrototype%; ArrayIteratorIndex/SetIndex/Finish apply. @leaf
 */
declare function CreateStringIterator(s: Str): Obj;
/**
 * The string a String Iterator iterates, or undefined once it is exhausted. TypeError
 * "String Iterator object expected" otherwise. @leaf
 */
declare function StringIteratorTarget(it: Value): Value;
/** The code point at index i of s as a string (one or two code units). @leaf */
declare function StrCodePointString(s: Str, i: f64): Str;
/**
 * Whether v is an Array or a string whose iteration is the original Symbol.iterator
 * method and `next` (data properties only): iterating it then equals reading it
 * directly. @leaf
 */
declare function HasOriginalIteration(v: Value): boolean;
/**
 * Whether the next step of `iterator` with `next` gives a value without running user
 * code: `next` is the original one of an Array or String Iterator and the value is at
 * hand (D1800). Then `IteratorFastValue` returns it; otherwise call `next`. @leaf
 */
declare function IteratorFastReady(iterator: Obj, next: Value): boolean;
/** The next value of an iterator `IteratorFastReady` accepts (it advances). @leaf */
declare function IteratorFastValue(iterator: Obj, next: Value): Value;
/**
 * The values `IteratorFastValue` would give one by one for a keys or values Array
 * Iterator, stored with CreateDataPropertyOrThrow into the Array `arr` at n, n + 1, ...
 * in one step, while they are plain dense stores (D2305): the next n. Stops before a
 * value only `next` can read (a hole), at the end (the next step finishes the iterator)
 * and where a store is not plain; gives n back unchanged for anything else. @leaf
 */
declare function IteratorFastDrain(iterator: Obj, next: Value, arr: Obj, n: f64): f64;
/** CreateIterResultObject ( value, done ), 7.4.14. @leaf */
declare function CreateIterResultObject(value: Value, done: boolean): Obj;
/** A new Array [a, b]. @leaf */
declare function CreateArrayFromPair(a: Value, b: Value): Obj;

// ---- Promises, records, builtin closures (0037, D1371/D1372) ----------------------------
//
// A promise capability is undefined (no result promise), a Promise (an internal
// capability of %Promise%, resolved directly), or a Record [promise, resolve, reject].

/** A new pending Promise with prototype proto. @leaf */
declare function PromiseCreate(proto: Obj): Obj;
/** Whether v is a Promise object (has [[PromiseState]]). @leaf */
declare function IsPromise(v: Value): boolean;
/** The realm's %Promise% constructor. @leaf */
declare function IntrinsicPromise(): Obj;
/** [[PromiseState]]: 0 pending, 1 fulfilled, 2 rejected. @leaf */
declare function PromiseState(p: Obj): i32;
/** [[PromiseResult]]. @leaf */
declare function PromiseResult(p: Obj): Value;
/** FulfillPromise ( promise, value ), 27.2.1.4: reactions become jobs. No-op unless pending. @leaf */
declare function FulfillPromise(p: Obj, v: Value): void;
/** RejectPromise ( promise, reason ), 27.2.1.7, with the rejection tracker. No-op unless pending. @leaf */
declare function RejectPromise(p: Obj, reason: Value): void;
/**
 * Sets the [[AlreadyResolved]] of p's own resolving functions (and internal capability);
 * false if it was already set.
 * @leaf
 */
declare function PromiseClaimResolve(p: Obj): boolean;
/**
 * PerformPromiseThen ( promise, onFulfilled, onRejected, resultCapability ), 27.2.5.4.1,
 * without the result: handlers that are not callable count as empty.
 * @leaf
 */
declare function PerformPromiseThen(p: Obj, onFulfilled: Value, onRejected: Value, capability: Value): void;
/**
 * The elements of PerformPromiseCombinator (Promise.all kind 0, allSettled 1, any 2, race
 * 3) that an Array values iterator gives without user code (as `IteratorFastValue` would),
 * one after another while their steps run no user code either (D3131): `fixed` is the
 * Record [C, promiseResolve, capability, values, remaining, resolveFn, rejectFn], C
 * %Promise% with the original `resolve`, and the element a Promise of %Promise% or a value
 * that fulfills one without a `then` call, whose `then`, `constructor` and
 * %Promise%[@@species] are the original ones. Each is done as the loop body would (values
 * slot, element Record, functions, `then`); returns the next index.
 * @leaf
 */
declare function PromiseCombinatorRun(fixed: Obj, kind: i32, index: f64, iterator: Obj, next: Value): f64;
/** Queues NewPromiseResolveThenableJob ( promise, thenable, then ), 27.2.2.2. @leaf */
declare function EnqueuePromiseResolveThenableJob(p: Obj, thenable: Obj, then: Value): void;
/** A new internal Record with n slots, all undefined. @leaf */
declare function NewRecord(n: i32): Obj;
/** Slot i of a Record, or of an object that keeps its state in slots (IteratorHelper, 0051). @leaf */
declare function RecordGet(r: Obj, i: i32): Value;
/** @leaf */
declare function RecordSet(r: Obj, i: i32, v: Value): void;
/**
 * A function (name "", `length`) that calls the builtin `kind` (the table in
 * interp/run/intrinsics.rs: 0 PromiseResolveFunction, 1 PromiseRejectFunction, 2
 * GetCapabilitiesExecutor, 3 PromiseAllResolveElement, 4 PromiseAllSettledResolveElement,
 * 5 PromiseAllSettledRejectElement, 6 PromiseAnyRejectElement, 7 PromiseThenFinally, 8
 * PromiseCatchFinally, 9 PromiseValueThunk, 10 PromiseThrower, 11
 * AsyncGeneratorAwaitReturnFulfilled, 12 AsyncGeneratorAwaitReturnRejected, 13
 * AsyncFromSyncIteratorUnwrap, 14 AsyncFromSyncIteratorCloseThrow, 15
 * ProxyRevokeFunction, 16 FromAsyncFulfilled, 17 FromAsyncRejected, 18
 * AsyncDisposeFulfilled, 19 AsyncDisposeRejected, 20 DecoratorAddInitializer, 21
 * DecoratorAccessGet, 22 DecoratorAccessSet, 23 DecoratorAccessHas, 24 DecoratorParamCall) with `this` = data and its own
 * arguments.
 * @leaf
 */
declare function NewBuiltinClosure(kind: i32, data: Value, length: i32): Fn;

// ---- Generators and async functions (0037, D1375) ---------------------------------------
//
// A Generator object is a suspendable activation: a generator, or an async function's
// state. States: 0 suspended at the start, 1 suspended at a yield, 2 executing, 4 at an await,
// 3 completed, 6 suspended at a yield_star (sync generators), D1421.

/** The state of a generator object (not an async state), or -1 for anything else. @leaf */
declare function GeneratorState(v: Value): i32;
/** @leaf */
declare function GeneratorSetState(g: Obj, state: i32): void;
/**
 * Resumes a suspended generator or async function (state 0 or 1) in a frame of its own:
 * mode 0 continues with value (a generator at a yield gets `value false`, an async
 * function the value), 1 throws value at the suspension point, 2 (generators at a
 * yield) returns value from there (`value true`). The result is what the activation
 * hands back when it next suspends or completes.
 * @reentrant
 */
declare function GeneratorResume(g: Obj, value: Value, mode: i32): Value;
/** Whether v is the state of an async function call. @leaf */
declare function IsAsyncFunctionState(v: Value): boolean;
/** The result promise of an async function call. @leaf */
declare function AsyncFunctionPromise(a: Obj): Obj;

// ---- Async generators and async iteration (0042, D1421) ---------------------------------
//
// Async generator states: GeneratorState's 0-3, 4 awaiting (inside an await), 5 awaiting
// a return() value (spec "draining-queue"/"awaiting-return"). A request is a Record
// [type (0 next, 1 throw, 2 return), value, promise].

/** The state of an async generator, or -1 for anything else. @leaf */
declare function AsyncGeneratorState(v: Value): i32;
/** Appends a request to the generator's queue. @leaf */
declare function AsyncGeneratorEnqueue(g: Obj, request: Obj): void;
/** The oldest request, or undefined. @leaf */
declare function AsyncGeneratorFirstRequest(g: Obj): Value;
/** Removes and returns the oldest request (undefined if none). @leaf */
declare function AsyncGeneratorDequeue(g: Obj): Value;
/**
 * CreateAsyncFromSyncIterator ( syncIteratorRecord ), 27.1.6.1: a Record [iterator,
 * next] with %AsyncFromSyncIteratorPrototype%.
 * @leaf
 */
declare function CreateAsyncFromSyncIterator(iterator: Obj, next: Value): Obj;

// ---- Iterator helpers (0051, builtins/iterator.ts, D1510) -------------------------------

/**
 * A new IteratorHelper object of `kind` (0: Iterator.from's wrapper, with
 * %WrapForValidIteratorPrototype%; else a helper, with %IteratorHelperPrototype%):
 * slot 0 kind, 1 state 0, 2 iterator, 3 next, the others undefined. @leaf
 */
declare function CreateIteratorHelper(kind: i32, iterator: Value, next: Value): Obj;
/**
 * v if it is an iterator helper (wrap false) or Iterator.from's wrapper (wrap true),
 * else TypeError "Iterator Helper object expected" / "Iterator Wrap object expected". @leaf
 */
declare function ThisIteratorHelper(v: Value, wrap: boolean): Obj;

// ---- WeakRef and FinalizationRegistry (0051, heap/weakref.rs, D1513) --------------------

/** A new WeakRef with prototype proto and target (AddToKeptObjects ( target ) too). @leaf */
declare function WeakRefCreate(proto: Obj, target: Value): Obj;
/**
 * WeakRefDeref of v (AddToKeptObjects of a live target): the target, or undefined once
 * it died. TypeError "WeakRef object expected" for anything else. @leaf
 */
declare function WeakRefDeref(v: Value): Value;
/** A new FinalizationRegistry with prototype proto and cleanupCallback. @leaf */
declare function FinalizationRegistryCreate(proto: Obj, cleanup: Value): Obj;
/** v if it is a FinalizationRegistry, else TypeError "FinalizationRegistry object expected". @leaf */
declare function ThisFinalizationRegistry(v: Value): Obj;
/** Appends a cell (token undefined for none). @leaf */
declare function FinalizationRegistryRegister(r: Obj, target: Value, held: Value, token: Value): void;
/** Removes the cells registered with token; whether there were any. @leaf */
declare function FinalizationRegistryUnregister(r: Obj, token: Value): boolean;
/** The index of the first cell whose target died, or -1. @leaf */
declare function FinalizationRegistryNextDead(r: Obj): i32;
/** The held value of dead cell i, removing the cell. @leaf */
declare function FinalizationRegistryTake(r: Obj, i: i32): Value;
/** Marks whether a cleanup job for r is queued. @leaf */
declare function FinalizationRegistrySetQueued(r: Obj, queued: boolean): void;

// ---- Disposable stacks (0051, builtins/disposable.ts) -----------------------------------

/**
 * A new DisposableStack (isAsync false) or AsyncDisposableStack with prototype proto:
 * slot 0 isAsync, 1 state 0 (pending), 2 a new empty Array of resources. @leaf
 */
declare function DisposableStackCreate(proto: Obj, isAsync: boolean): Obj;
/**
 * v if it is a DisposableStack (isAsync false) or an AsyncDisposableStack, else TypeError
 * "DisposableStack object expected" / "AsyncDisposableStack object expected". @leaf
 */
declare function ThisDisposableStack(v: Value, isAsync: boolean): Obj;
// ---- TextEncoder and TextDecoder (0244, builtins/encoding.rs, D3172) --------------------

/** A new TextEncoder (flags 0) or TextDecoder (flags 1 | fatal 2 | ignoreBOM 4). @leaf */
declare function TextCodecCreate(proto: Obj, flags: i32): Obj;
/** The flags of v, a TextDecoder (decoder) or a TextEncoder; else TypeError. @leaf */
declare function TextCodecFlags(v: Value, decoder: boolean): i32;
/** Whether v is an ArrayBuffer, a typed array or a DataView. @leaf */
declare function IsBufferSource(v: Value): boolean;
/**
 * buffer undefined: a new Uint8Array of the UTF-8 of s (lone surrogates as U+FFFD). Else
 * encodeInto into bytes offset..offset + length of the ArrayBuffer buffer: { read, written }. @leaf
 */
declare function Utf8Encode(s: Str, buffer: Value, offset: Value, length: Value): Value;
/**
 * The decoder's output for bytes offset..offset + length (length -1: to the end) of the
 * ArrayBuffer buffer (none if undefined, detached or out of range), keeping an incomplete
 * sequence for the next call if stream; undefined for a fatal error. @leaf
 */
declare function Utf8Decode(decoder: Obj, buffer: Value, offset: Value, length: Value, stream: boolean): Value;

// ---- JSON (0033, builtins/json.rs over crate::json) -------------------------------------

/**
 * JSON.parse of text without a reviver; qjs's SyntaxError for invalid JSON. With
 * withSource, an Array [value, record] where record is the JSON Parse Record tree for the
 * reviver's source text access: [value, source, elements, entries] (D1611). @leaf
 */
declare function JsonParse(text: Str, withSource: boolean): Value;
/**
 * JSON.rawJSON's work after ToString: a frozen, null-prototype RawJSON object with the
 * own property rawJSON = text; SyntaxError "invalid rawJSON string" unless text is a JSON
 * primitive without leading or trailing whitespace. @leaf
 */
declare function JsonRawJSON(text: Str): Obj;
/** Whether v has [[IsRawJSON]] (a JSON.rawJSON object). @leaf */
declare function IsRawJSON(v: Value): boolean;
/**
 * JSON.stringify(v, undefined, space) with gap already computed, for plain data: the
 * string, undefined for an undefined result, or null when the value needs the full
 * algorithm (accessors, toJSON, wrappers, exotic objects; nothing observable happened).
 * TypeError "circular reference" and RangeError "Maximum call stack size exceeded" are final. @leaf
 */
declare function JsonStringifyFast(v: Value, gap: Str): Value;
/**
 * JsonStringifyFast for an object the full algorithm reached, after its toJSON and
 * replacer (its own toJSON is not looked at again), nested in the objects of the
 * algorithm's `stack` (a cycle back to one throws; they set the indentation), with an
 * array replacer's PropertyList or undefined: the string, or null when it is not plain
 * data (0090). @leaf
 */
declare function JsonStringifyFastAt(v: Obj, gap: Str, stack: Obj, propertyList: Value): Value;
/** QuoteJSONString ( value ), 25.5.2.3. @leaf */
declare function JsonQuote(s: Str): Str;
/** CreateDataProperty ( O, P, V ): false if refused. @reentrant */
declare function CreateDataProperty(o: Obj, key: Value, v: Value): boolean;

// ---- BigInt (0046) ---------------------------------------------------------------------

/** @leaf */
declare function IsBigInt(v: Value): boolean;
/** ToBigInt of a primitive: TypeError for undefined, null, numbers and symbols; SyntaxError for a bad string. @leaf */
declare function ToBigIntPrimitive(v: Value): Value;
/** NumberToBigInt ( number ), 21.2.1.1.1: RangeError unless integral. @leaf */
declare function NumberToBigInt(n: f64): Value;
/** The digits of a BigInt in radix 2..36. @leaf */
declare function BigIntToString(b: Value, radix: i32): Str;
/** BigInt.asIntN on an index and a BigInt. @leaf */
declare function BigIntAsIntN(bits: f64, b: Value): Value;
/** BigInt.asUintN on an index and a BigInt. @leaf */
declare function BigIntAsUintN(bits: f64, b: Value): Value;
/** The Number value of a BigInt (rounded to nearest). @leaf */
declare function BigIntToNumber(b: Value): f64;

// ---- RegExp (0033, builtins/regexp.rs over crate::regexp; D1332) -----------------------

/** Whether v is a RegExp object (has [[RegExpMatcher]]). @leaf */
declare function IsRegExpObject(v: Value): boolean;
/**
 * RegExpAlloc + RegExpInitialize after the ToStrings: a new RegExp with prototype proto
 * (%RegExp.prototype% for undefined) and lastIndex 0. qjs's SyntaxError for a bad
 * pattern or flags. Regex literals compile to this. The pattern may be a RegExp object
 * instead of its source: with the same flags the new one shares its program (D3136).
 * @leaf
 */
declare function RegExpCreate(proto: Value, pattern: Value, flags: Str): Obj;
/** [[OriginalSource]] of a RegExp object. @leaf */
declare function RegExpSource(r: Obj): Str;
/** [[OriginalFlags]] of a RegExp object, in canonical order. @leaf */
declare function RegExpFlags(r: Obj): Str;
/** Whether a RegExp object has the flag letter `flag`. @leaf */
declare function RegExpHasFlag(r: Obj, flag: Str): boolean;
/** EscapeRegExpPattern of [[OriginalSource]] (`(?:)` when empty). @leaf */
declare function RegExpEscapedSource(r: Obj): Str;
/**
 * RegExpBuiltinExec ( R, S ) once lastIndex is Get(R, "lastIndex") as a Number (ToLength
 * is applied here): the match array or null, and lastIndex updated for g and y. Pass undefined as state: when the
 * fuel runs out mid-match the interpreter suspends here and keeps the matcher's state
 * in that argument. InternalError "out of memory in regexp execution" if the
 * backtracking stack overflows. @leaf
 */
declare function RegExpExecRaw(r: Obj, s: Str, lastIndex: f64, state: Value): Value;
/**
 * RegExpExecRaw without the match array: whether it matched (lastIndex is updated the
 * same way). Suspendable like RegExpExecRaw. @leaf
 */
declare function RegExpTestRaw(r: Obj, s: Str, lastIndex: f64, state: Value): boolean;
/**
 * RegExpExec ( R, S ) when its lookups read only data (D3137): R is a RegExp without an
 * own `exec`, whose prototype is %RegExp.prototype% with the original `exec`, and whose
 * own lastIndex is a Number. Then the match array or null (with `test`, true or false),
 * lastIndex updated; else undefined, with nothing done. Suspendable like RegExpExecRaw
 * (pass undefined as state). @leaf
 */
declare function RegExpExecOwn(r: Value, s: Str, test: boolean, state: Value): Value;
/** Whether v is the original RegExp.prototype.exec (the installed builtin). @leaf */
declare function IsOriginalRegExpExec(v: Value): boolean;
/** A RegExp String Iterator (CreateRegExpStringIterator, 22.2.9.1). @leaf */
declare function CreateRegExpStringIterator(r: Obj, s: Str, global: boolean, unicode: boolean): Obj;
/**
 * Slot i of a RegExp String Iterator: 0 the regexp, 1 the string, 2 global, 3 unicode,
 * 4 done. TypeError "RegExp String Iterator object expected" for anything else. @leaf
 */
declare function RegExpStringIteratorSlot(it: Value, i: i32): Value;
/** Marks a RegExp String Iterator done. @leaf */
declare function RegExpStringIteratorFinish(it: Value): void;
/** RegExp.escape of a string (EncodeForRegExpEscape, qjs's lowercase hex). @leaf */
declare function RegExpEscapeString(s: Str): Str;
/**
 * Whether v is a RegExp whose protocol lookups (exec, flags and the flag getters,
 * constructor, Symbol.species, Symbol.match, lastIndex) are all original data, so that skipping
 * them is unobservable (D1503). @leaf
 */
declare function IsPristineRegExp(v: Value): boolean;
/**
 * RegExp.prototype[Symbol.split] of a pristine RegExp on a non-empty string with 0 < lim:
 * the result array. Suspendable like RegExpExecRaw (pass undefined as state). @leaf
 */
declare function RegExpSplitRaw(r: Obj, s: Str, lim: f64, state: Value): Obj;
/**
 * RegExp.prototype[Symbol.replace] of a pristine global RegExp with a string replacement
 * template: the result, and lastIndex 0. Suspendable like RegExpExecRaw. @leaf
 */
declare function RegExpReplaceRaw(r: Obj, s: Str, template: Str, state: Value): Str;
/**
 * The matches RegExp.prototype[Symbol.replace] collects from a pristine RegExp before it
 * calls a replacer function: every match of a global one from lastIndex 0, else the first
 * from lastIndex (0 unless sticky). An Array of Numbers: for each match the start and the
 * end of every group (-1 for a group that did not participate). lastIndex ends as that
 * loop leaves it. Suspendable like RegExpExecRaw (pass undefined as state). @leaf
 */
declare function RegExpMatchSpans(r: Obj, s: Str, state: Value): Obj;
/** The number of matches in `spans` (RegExpMatchSpans of `r` in `s`). @leaf */
declare function RegExpSpanCount(r: Obj, s: Str, spans: Obj): f64;
/**
 * The arguments of a replacer function for match `k` of `spans`: the matched string,
 * the captures (strings or undefined), the position, `s`, and the groups object if `r`
 * has named groups. @leaf
 */
declare function RegExpReplaceArgs(r: Obj, s: Str, spans: Obj, k: f64): Obj;
/**
 * The matched strings of `spans` as an Array, for a RegExp without capture groups (a
 * replacer function then gets the string, the position and `s`); undefined otherwise.
 * @leaf
 */
declare function RegExpSpanTexts(r: Obj, s: Str, spans: Obj): Value;
/**
 * The Array the replacements of `spans` go to: for a RegExp without capture groups it
 * holds the position of each match (until its replacement is stored there), otherwise it
 * is empty. @leaf
 */
declare function RegExpSpanStarts(r: Obj, s: Str, spans: Obj): Obj;
/**
 * `s` with each match of `spans` replaced by the string at its index in the Array
 * `replacements`. @leaf
 */
declare function RegExpReplaceJoin(r: Obj, s: Str, spans: Obj, replacements: Obj): Str;

// ---- Bulk fast paths (0056, D1561) ------------------------------------------------------

/**
 * Whether v is an Array whose elements 0 .. length-1 are all present in its dense store
 * (no holes): reading them with Get then runs no user code. @leaf
 */
declare function IsDenseArray(v: Value): boolean;
/** Whether every element of a dense array (no holes) is a Number. @leaf */
declare function AllNumbers(list: Obj): boolean;
/** Whether every element of a dense array is an integral Number in [0, 0x10FFFF]. @leaf */
declare function AllCodePoints(list: Obj): boolean;

// ---- Used by the generated code (crates/djs-bgen/src/emit_js.rs) ------------------------

/**
 * Defines f as a method of the object at the dotted path `target` (from the global
 * object): writable, configurable, not enumerable, with `name` = key and `length`. The
 * key is a string or a symbol (`[Symbol.iterator]` paths).
 * @leaf
 */
declare function InstallMethod(target: Str, key: Value, f: Value, length: u32): void;
/**
 * `@constructor`: defines f as the global constructor `key` (writable, configurable, not
 * enumerable) and sets its `name` and `length`; with a parent (a dotted path), f's
 * [[Prototype]] is the parent constructor. Its statics follow, then InstallPrototype.
 * @leaf
 */
declare function InstallConstructor(target: Str, key: Str, f: Value, length: u32, parent: Value): void;
/**
 * The `prototype` of the constructor `key`, installed after its statics (D2052, qjs's
 * order): the realm's intrinsic prototype for that name if there is one, else a new
 * object inheriting from the parent's prototype (or Object.prototype). Sets
 * `f.prototype`.
 * @leaf
 */
declare function InstallPrototype(target: Str, key: Str, parent: Value): void;
/**
 * `prototype.constructor` of the constructor `key`, installed among the prototype's
 * methods where qjs's order puts it (D2150).
 * @leaf
 */
declare function InstallPrototypeConstructor(key: Str): void;
/** `@getter`/`@setter`: one half of an accessor (configurable, not enumerable). @leaf */
declare function InstallAccessor(target: Str, key: Value, f: Value, isSetter: boolean): void;
/** `@value`: a data property with `attrs` (writable 1, enumerable 2, configurable 4). @leaf */
declare function InstallValue(target: Str, key: Value, v: Value, attrs: i32): void;
/** Registers f so that privileged code can call it as %name(...). @leaf */
declare function InstallIntrinsic(name: Str, f: Value): void;
/**
 * Makes the builtin f native builtin `index` of the generated table (0063): with
 * native builtins on, its calls run the Rust state machine. @leaf
 */
declare function MarkNative(f: Value, index: i32): void;
/**
 * Gives the builtin f its leaf fast path (`@fast`, D1850): with native builtins on, a
 * call first tries the Rust function of the same name in builtins/fast.rs. @leaf
 */
declare function MarkFast(f: Value): void;

// ---- ES modules (0241, builtins/module.ts, interp/module.rs) -------------------------

/**
 * Internal method op (builtins/module.ts ModuleOp) of the module namespace whose proxy
 * target is target, on key: 0 [[Get]] (a symbol key: the target's; ReferenceError in a
 * binding's TDZ), 1 [[GetOwnProperty]] as a descriptor object or undefined, 2
 * [[DefineOwnProperty]] with descriptor object desc (its own fields), 3
 * [[OwnPropertyKeys]] as a new array. For import(): 4 the VM's async host function for
 * imports or undefined, 5 the linked module record of key target (an Error if none), 6
 * the namespace of module record target, 7 the reaction that finishes an import.
 * @leaf
 */
declare function NsOp(op: i32, target: Value, key: Value, desc: Value): Value;
