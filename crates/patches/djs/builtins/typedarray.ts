// %TypedArray% and the typed array constructors (ECMA-262 23.2), and Uint8Array's base64
// and hex methods. Elements live in the buffer; the integer-indexed exotic behavior is in
// heap/typed.rs (D1390). Kinds: 0 Int8, 1 Uint8, 2 Uint8Clamped, 3 Int16, 4 Uint16,
// 5 Int32, 6 Uint32, 7 Float16, 8 Float32, 9 Float64, 10 BigInt64, 11 BigUint64.
// @slim-omit: left out of the slim build (D2411).

/** The element size of a kind. */
function ElementSize(kind: i32): f64 {
  if (kind <= 2) {
    return 1;
  }
  if (kind <= 4 || kind === 7) {
    return 2;
  }
  if (kind <= 6 || kind === 8) {
    return 4;
  }
  return 8;
}

/** The constructor name of a kind (for the realm's %Name% defaults). */
function KindName(kind: i32): Str {
  if (kind === 0) {
    return "Int8Array";
  }
  if (kind === 1) {
    return "Uint8Array";
  }
  if (kind === 2) {
    return "Uint8ClampedArray";
  }
  if (kind === 3) {
    return "Int16Array";
  }
  if (kind === 4) {
    return "Uint16Array";
  }
  if (kind === 5) {
    return "Int32Array";
  }
  if (kind === 6) {
    return "Uint32Array";
  }
  if (kind === 7) {
    return "Float16Array";
  }
  if (kind === 8) {
    return "Float32Array";
  }
  if (kind === 9) {
    return "Float64Array";
  }
  if (kind === 10) {
    return "BigInt64Array";
  }
  return "BigUint64Array";
}

/** The kind of a typed array. */
function KindOf(o: Obj): i32 {
  return F64ToI32(AsNumber(TypedArrayField(o, 0)));
}

/** The length of a typed array (0 if out of bounds or detached). */
function TALength(o: Obj): f64 {
  return AsNumber(TypedArrayField(o, 3));
}

/** Out of bounds or detached. */
function TAOutOfBounds(o: Obj): boolean {
  return AsBool(TypedArrayField(o, 5));
}

/**
 * ValidateTypedArray ( O, order ), 23.2.4.4: the typed array, TypeError if it is not
 * one or is out of bounds/detached.
 */
function ValidateTypedArray(v: Value): Obj {
  TypedArrayField(v, 0);
  const o: Obj = AsObj(v);
  if (TAOutOfBounds(o)) {
    throw NewTypeError("ArrayBuffer is detached or resized");
  }
  return o;
}

/** Whether two typed arrays have the same [[ContentType]] (Number or BigInt). */
function SameContentType(a: Obj, b: Obj): boolean {
  return (KindOf(a) >= 10) === (KindOf(b) >= 10);
}

/**
 * The element value of v for typed array o: ToBigInt for a BigInt array, ToNumber
 * otherwise (TypedArraySetElement, 10.4.5.16, step 1-2).
 */
function ToElementValue(o: Obj, v: Value): Value {
  if (KindOf(o) >= 10) {
    return ToBigInt(v);
  }
  return ToNumber(v);
}

/**
 * [[Set]] of element `index` (a Number; -1 for an invalid one) on typed array o with a
 * value that needs ToNumber / ToBigInt (TypedArraySetElement, 10.4.5.16): the
 * conversion may run user code that shrinks or detaches the buffer, then the store
 * happens only if the index is still valid. The interpreter's [[Set]] calls it (0090).
 * @intrinsic
 */
function TypedArraySetElementSlow(o: Obj, index: f64, v: Value): void {
  TypedArraySet(o, index, ToElementValue(o, v));
}

/** Whether v needs no conversion to be an element of o. */
function IsElementValue(o: Obj, v: Value): boolean {
  if (KindOf(o) >= 10) {
    return IsBigInt(v);
  }
  return IsNumber(v);
}

/** AllocateTypedArray ( ..., length ) with a fresh buffer of length elements. */
function AllocateTypedArrayOfLength(kind: i32, proto: Obj, length: f64): Obj {
  const size: f64 = ElementSize(kind);
  if (length * size > 2147483648) {
    throw NewRangeError("invalid array buffer length");
  }
  const buffer: Obj = ArrayBufferCreate(RealmIntrinsic("ArrayBufferPrototype"), length * size, undefined);
  return TypedArrayCreate(proto, kind, buffer, 0, length);
}

/** InitializeTypedArrayFromArrayBuffer, 23.2.5.1.3 (qjs's messages). */
function TypedArrayFromBuffer(kind: i32, proto: Obj, buffer: Obj, byteOffset: Value, length: Value): Obj {
  const size: f64 = ElementSize(kind);
  const offset: f64 = ToIndex(byteOffset);
  if (offset % size !== 0) {
    throw NewRangeError("invalid offset");
  }
  let newLength: f64 = 0;
  if (length !== undefined) {
    newLength = ToIndex(length);
  }
  if (IsDetached(buffer)) {
    throw NewTypeError("ArrayBuffer is detached");
  }
  const bufferByteLength: f64 = BufferLength(buffer);
  if (length === undefined && AsBool(BufferInfo(buffer, 2))) {
    if (offset > bufferByteLength) {
      throw NewRangeError("invalid offset");
    }
    return TypedArrayCreate(proto, kind, buffer, offset, -1);
  }
  if (length === undefined) {
    if (bufferByteLength % size !== 0) {
      throw NewRangeError("invalid length");
    }
    if (bufferByteLength - offset < 0) {
      throw NewRangeError("invalid offset");
    }
    return TypedArrayCreate(proto, kind, buffer, offset, (bufferByteLength - offset) / size);
  }
  if (offset + newLength * size > bufferByteLength) {
    throw NewRangeError("invalid length");
  }
  return TypedArrayCreate(proto, kind, buffer, offset, newLength);
}

/** A typed array of kind with the values of a list (an array) or array-like. */
function TypedArrayFromValues(kind: i32, proto: Obj, values: Obj): Obj {
  const len: f64 = LengthOfArrayLike(values);
  const o: Obj = AllocateTypedArrayOfLength(kind, proto, len);
  if (TypedArraySetFromDense(o, values)) {
    return o;
  }
  for (let k: f64 = 0; k < len; k++) {
    TypedArraySet(o, k, ToElementValue(o, Get(values, k)));
  }
  return o;
}

/** The constructors' shared steps (23.2.5.1). */
function TypedArrayConstruct(newTarget: Value, kind: i32, args: Obj): Obj {
  if (newTarget === undefined) {
    throw NewTypeError("must be called with new");
  }
  const n: f64 = LengthOfArrayLike(args);
  const first: Value = n > 0 ? Get(args, 0) : undefined;
  if (!IsObject(first)) {
    // ToIndex comes before the prototype lookup here (23.2.5.1 step 6.b).
    const elementLength: f64 = ToIndex(first);
    return AllocateTypedArrayOfLength(kind, GetPrototypeFromConstructor(newTarget, KindName(kind) + "Prototype"), elementLength);
  }
  const proto: Obj = GetPrototypeFromConstructor(newTarget, KindName(kind) + "Prototype");
  const src: Obj = AsObj(first);
  if (IsTypedArray(src)) {
    if (TAOutOfBounds(src)) {
      throw NewTypeError("ArrayBuffer is detached or resized");
    }
    if ((KindOf(src) >= 10) !== (kind >= 10)) {
      throw NewTypeError("incompatible content type");
    }
    const len: f64 = TALength(src);
    const o: Obj = AllocateTypedArrayOfLength(kind, proto, len);
    TypedArrayCopy(src, 0, o, 0, len);
    return o;
  }
  if (BufferInfoOrUndefined(src)) {
    return TypedArrayFromBuffer(kind, proto, src, n > 1 ? Get(args, 1) : undefined, n > 2 ? Get(args, 2) : undefined);
  }
  const usingIterator: Value = GetMethod(src, WellKnownSymbol("iterator"));
  // An array of Numbers iterated the original way: its elements are the list, and
  // converting them runs no user code (0080).
  if (HasOriginalIteration(src) && IsDenseArray(src) && AllNumbers(src)) {
    return TypedArrayFromValues(kind, proto, src);
  }
  if (usingIterator !== undefined) {
    const list: Obj = ArrayCreate(0);
    const target: Obj = ArrayFromIterator(list, src, usingIterator, undefined, undefined);
    return TypedArrayFromValues(kind, proto, target);
  }
  return TypedArrayFromValues(kind, proto, src);
}

/** Whether o is an ArrayBuffer. */
function BufferInfoOrUndefined(o: Obj): boolean {
  try {
    BufferInfo(o, 0);
    return true;
  } catch (e) {
    return false;
  }
}

/**
 * %TypedArray% ( ), 23.2.1.1: abstract.
 * @constructor %TypedArray%
 */
function TypedArrayAbstract(): Obj {
  throw NewTypeError("cannot be called");
}

/**
 * get %TypedArray% [ %Symbol.species% ], 23.2.2.4.
 * @getter %TypedArray%[Symbol.species]
 */
function TypedArraySpecies(this: Value): Value {
  return this;
}

/**
 * ValidateTypedArray's write mode (the immutable ArrayBuffer proposal, D1970): a
 * typed array over an immutable buffer cannot be written.
 */
function RequireMutableTypedArray(o: Obj): void {
  if (IsImmutableBuffer(AsObj(TypedArrayField(o, 1)))) {
    throw NewTypeError("ArrayBuffer is immutable");
  }
}

/** TypedArrayCreateFromConstructor with a length argument, validated (23.2.4.2). */
function TypedArrayCreateWithLength(c: Value, length: f64): Obj {
  const created: Obj = Construct(c, length);
  ValidateTypedArray(created);
  // The result is written: it must be mutable (D1970).
  RequireMutableTypedArray(created);
  if (TALength(created) < length) {
    throw NewTypeError("TypedArray length is too small");
  }
  return created;
}

/** TypedArraySpeciesCreate ( exemplar, « length » ), 23.2.4.1. */
function TypedArraySpeciesCreate(exemplar: Obj, length: f64): Obj {
  const c: Value = SpeciesConstructor(exemplar, RealmConstructor(KindName(KindOf(exemplar))));
  const r: Obj = TypedArrayCreateWithLength(c, length);
  if (!SameContentType(r, exemplar)) {
    throw NewTypeError("incompatible content type");
  }
  return r;
}

/**
 * %TypedArray%.from ( source [ , mapfn [ , thisArg ] ] ), 23.2.2.1.
 * @builtin %TypedArray%.from
 */
function TypedArrayFrom(this: Value, source: Value, mapfn?: Value, thisArg?: Value): Obj {
  if (!IsConstructor(this)) {
    throw NewTypeError("not a constructor");
  }
  const mapping: boolean = mapfn !== undefined;
  if (mapping && !IsCallable(mapfn)) {
    throw NewTypeError("not a function");
  }
  const usingIterator: Value = GetMethod(source, WellKnownSymbol("iterator"));
  let values: Obj = ArrayCreate(0);
  if (usingIterator !== undefined) {
    values = ArrayFromIterator(values, source, usingIterator, undefined, undefined);
  } else {
    values = ToObject(source);
  }
  const len: f64 = LengthOfArrayLike(values);
  const target: Obj = TypedArrayCreateWithLength(this, len);
  for (let k: f64 = 0; k < len; k++) {
    const kValue: Value = Get(values, k);
    const mapped: Value = mapping ? Call(mapfn, thisArg, kValue, k) : kValue;
    TASet(target, k, mapped);
  }
  return target;
}

/**
 * %TypedArray%.of ( ...items ), 23.2.2.2.
 * @builtin %TypedArray%.of
 */
function TypedArrayOf(this: Value, ...items: Value[]): Obj {
  if (!IsConstructor(this)) {
    throw NewTypeError("not a constructor");
  }
  const len: f64 = LengthOfArrayLike(items);
  const target: Obj = TypedArrayCreateWithLength(this, len);
  for (let k: f64 = 0; k < len; k++) {
    TASet(target, k, Get(items, k));
  }
  return target;
}

/**
 * get %TypedArray%.prototype.buffer, 23.2.3.2.
 * @getter %TypedArray%.prototype.buffer
 */
function TypedArrayPrototypeBuffer(this: Value): Value {
  return TypedArrayField(this, 1);
}

/**
 * get %TypedArray%.prototype.byteLength, 23.2.3.3.
 * @getter %TypedArray%.prototype.byteLength
 */
function TypedArrayPrototypeByteLength(this: Value): Value {
  return TypedArrayField(this, 4);
}

/**
 * get %TypedArray%.prototype.byteOffset, 23.2.3.4.
 * @getter %TypedArray%.prototype.byteOffset
 */
function TypedArrayPrototypeByteOffset(this: Value): Value {
  return TypedArrayField(this, 2);
}

/**
 * get %TypedArray%.prototype.length, 23.2.3.21.
 * @getter %TypedArray%.prototype.length
 */
function TypedArrayPrototypeLength(this: Value): Value {
  return TypedArrayField(this, 3);
}

/**
 * get %TypedArray%.prototype [ %Symbol.toStringTag% ], 23.2.3.38.
 * @getter %TypedArray%.prototype[Symbol.toStringTag]
 */
function TypedArrayPrototypeToStringTag(this: Value): Value {
  if (!IsTypedArray(this)) {
    return undefined;
  }
  return KindName(KindOf(AsObj(this)));
}

/**
 * %TypedArray%.prototype.set ( source [ , offset ] ), 23.2.3.26.
 * @builtin %TypedArray%.prototype.set
 */
function TypedArrayPrototypeSet(this: Value, source: Value, offset?: Value): void {
  TypedArrayField(this, 0);
  const target: Obj = AsObj(this);
  RequireMutableTypedArray(target);
  const targetOffset: f64 = ToIntegerOrInfinity(offset);
  if (targetOffset < 0) {
    throw NewRangeError("invalid array length");
  }
  if (TAOutOfBounds(target)) {
    throw NewTypeError("ArrayBuffer is detached");
  }
  const targetLength: f64 = TALength(target);
  if (IsTypedArray(source)) {
    const src: Obj = AsObj(source);
    if (TAOutOfBounds(src)) {
      throw NewTypeError("ArrayBuffer is detached or resized");
    }
    const srcLength: f64 = TALength(src);
    if (targetOffset === 1 / 0 || srcLength + targetOffset > targetLength) {
      throw NewRangeError("invalid array length");
    }
    if (!SameContentType(src, target)) {
      // qjs's words: it converts the first element.
      throw NewTypeError(KindOf(target) >= 10 ? "cannot convert to bigint" : "cannot convert bigint to number");
    }
    TypedArrayCopy(src, 0, target, targetOffset, srcLength);
    return;
  }
  const src: Obj = ToObject(source);
  const srcLength: f64 = LengthOfArrayLike(src);
  if (targetOffset === 1 / 0 || srcLength + targetOffset > targetLength) {
    throw NewRangeError("invalid array length");
  }
  for (let k: f64 = 0; k < srcLength; k++) {
    const value: Value = ToElementValue(target, Get(src, k));
    TypedArraySet(target, targetOffset + k, value);
  }
}

/**
 * %TypedArray%.prototype.subarray ( start, end ), 23.2.3.31.
 * @builtin %TypedArray%.prototype.subarray
 */
function TypedArrayPrototypeSubarray(this: Value, start: Value, end: Value): Obj {
  TypedArrayField(this, 0);
  const o: Obj = AsObj(this);
  const buffer: Obj = AsObj(TypedArrayField(o, 1));
  const srcLength: f64 = TALength(o);
  const kind: i32 = KindOf(o);
  const size: f64 = ElementSize(kind);
  const beginIndex: f64 = RelativeIndex(start, srcLength, 0);
  const tracking: boolean = AsBool(TypedArrayField(o, 6));
  // O.[[ByteOffset]] even when out of bounds: the new view then fails its bounds check.
  const beginByteOffset: f64 = AsNumber(TypedArrayField(o, 7)) + beginIndex * size;
  const c: Value = SpeciesConstructor(o, RealmConstructor(KindName(kind)));
  let created: Value = undefined;
  if (tracking && end === undefined) {
    created = Construct(c, buffer, beginByteOffset);
  } else {
    const endIndex: f64 = RelativeIndex(end, srcLength, srcLength);
    const newLength: f64 = endIndex > beginIndex ? endIndex - beginIndex : 0;
    created = Construct(c, buffer, beginByteOffset, newLength);
  }
  ValidateTypedArray(created);
  if (!SameContentType(AsObj(created), o)) {
    throw NewTypeError("incompatible content type");
  }
  return AsObj(created);
}

/**
 * %TypedArray%.prototype.slice ( start, end ), 23.2.3.27.
 * @builtin %TypedArray%.prototype.slice
 */
function TypedArrayPrototypeSlice(this: Value, start: Value, end: Value): Obj {
  const o: Obj = ValidateTypedArray(this);
  const srcLength: f64 = TALength(o);
  const startIndex: f64 = RelativeIndex(start, srcLength, 0);
  const endIndex: f64 = RelativeIndex(end, srcLength, srcLength);
  const count: f64 = endIndex > startIndex ? endIndex - startIndex : 0;
  const a: Obj = TypedArraySpeciesCreate(o, count);
  if (count > 0) {
    if (TAOutOfBounds(o)) {
      throw NewTypeError("ArrayBuffer is detached or resized");
    }
    const len: f64 = TALength(o);
    const fin: f64 = endIndex < len ? endIndex : len;
    const n: f64 = fin > startIndex ? fin - startIndex : 0;
    if (n === 0) {
      // A shrunk buffer: nothing left to copy.
    } else if (KindOf(a) === KindOf(o) && TypedArrayField(a, 0) === TypedArrayField(o, 0)) {
      // The same buffer: the spec copies byte by byte, forwards (23.2.3.27 step 14.g),
      // so an overlap repeats the first elements; elements are aligned, so per element.
      for (let k: f64 = 0; k < n; k++) {
        TypedArraySet(a, k, TypedArrayGet(o, startIndex + k));
      }
    } else if (KindOf(a) === KindOf(o)) {
      TypedArrayCopy(o, startIndex, a, 0, n);
    } else {
      for (let k: f64 = 0; k < n; k++) {
        TASet(a, k, TypedArrayGet(o, startIndex + k));
      }
    }
  }
  return a;
}

/**
 * %TypedArray%.prototype.fill ( value [ , start [ , end ] ] ), 23.2.3.9.
 * @builtin %TypedArray%.prototype.fill
 */
function TypedArrayPrototypeFill(this: Value, value: Value, start?: Value, end?: Value): Obj {
  const o: Obj = ValidateTypedArray(this);
  RequireMutableTypedArray(o);
  const len: f64 = TALength(o);
  const n: Value = ToElementValue(o, value);
  const k: f64 = RelativeIndex(start, len, 0);
  const fin: f64 = RelativeIndex(end, len, len);
  if (TAOutOfBounds(o)) {
    throw NewTypeError("ArrayBuffer is detached");
  }
  const len2: f64 = TALength(o);
  const last: f64 = fin < len2 ? fin : len2;
  for (let i: f64 = k; i < last; i++) {
    TypedArraySet(o, i, n);
  }
  return o;
}

// ---- Uint8Array base64 and hex (ES2026) ------------------------------------------------

/** ValidateUint8Array ( ta ). */
function ValidateUint8Array(v: Value): Obj {
  if (!IsTypedArray(v) || KindOf(AsObj(v)) !== 1) {
    throw NewTypeError("not a Uint8Array");
  }
  return AsObj(v);
}

/** GetOptionsObject ( options ). */
function GetOptionsObject(options: Value): Value {
  if (options === undefined) {
    return undefined;
  }
  if (!IsObject(options)) {
    throw NewTypeError("options must be an object");
  }
  return options;
}

/** The alphabet option: true for base64url. */
function Base64Alphabet(opts: Value): boolean {
  if (opts === undefined) {
    return false;
  }
  const alphabet: Value = Get(AsObj(opts), "alphabet");
  if (alphabet === undefined || alphabet === "base64") {
    return false;
  }
  if (alphabet === "base64url") {
    return true;
  }
  throw NewTypeError("invalid alphabet");
}

/** The lastChunkHandling option: 0 loose, 1 strict, 2 stop-before-partial. */
function LastChunkHandling(opts: Value): i32 {
  if (opts === undefined) {
    return 0;
  }
  const h: Value = Get(AsObj(opts), "lastChunkHandling");
  if (h === undefined || h === "loose") {
    return 0;
  }
  if (h === "strict") {
    return 1;
  }
  if (h === "stop-before-partial") {
    return 2;
  }
  throw NewTypeError("invalid lastChunkHandling option");
}

/** A Uint8Array holding the bytes of a decode record. */
function Uint8ArrayFromRecord(rec: Obj): Obj {
  const bytes: Obj = AsObj(Get(rec, "bytes"));
  const len: f64 = BufferLength(bytes);
  return TypedArrayCreate(RealmIntrinsic("Uint8ArrayPrototype"), 1, bytes, 0, len);
}

/**
 * Uint8Array.fromBase64 ( string [ , options ] ).
 * @builtin Uint8Array.fromBase64
 */
function Uint8ArrayFromBase64(string: Value, options?: Value): Obj {
  if (!IsString(string)) {
    throw NewTypeError("expected string");
  }
  const opts: Value = GetOptionsObject(options);
  const url: boolean = Base64Alphabet(opts);
  const last: i32 = LastChunkHandling(opts);
  const rec: Obj = Base64Decode(AsStr(string), url, last, 9007199254740991);
  if (AsBool(Get(rec, "error"))) {
    throw NewSyntaxError("invalid base64 string");
  }
  return Uint8ArrayFromRecord(rec);
}

/**
 * Uint8Array.fromHex ( string ).
 * @builtin Uint8Array.fromHex
 */
function Uint8ArrayFromHex(string: Value): Obj {
  if (!IsString(string)) {
    throw NewTypeError("expected string");
  }
  const rec: Obj = HexDecode(AsStr(string), 9007199254740991);
  if (AsBool(Get(rec, "error"))) {
    throw NewSyntaxError("invalid hex string");
  }
  return Uint8ArrayFromRecord(rec);
}

/**
 * Uint8Array.prototype.toBase64 ( [ options ] ).
 * @builtin Uint8Array.prototype.toBase64
 */
function Uint8ArrayPrototypeToBase64(this: Value, options?: Value): Str {
  const o: Obj = ValidateUint8Array(this);
  const opts: Value = GetOptionsObject(options);
  const url: boolean = Base64Alphabet(opts);
  let omit: boolean = false;
  if (opts !== undefined) {
    omit = ToBoolean(Get(AsObj(opts), "omitPadding"));
  }
  return Base64Encode(o, url, omit);
}

/**
 * Uint8Array.prototype.toHex ( ).
 * @builtin Uint8Array.prototype.toHex
 */
function Uint8ArrayPrototypeToHex(this: Value): Str {
  return HexEncode(ValidateUint8Array(this));
}

/** Writes a decode record's bytes into a Uint8Array; `{ read, written }`. */
function WriteDecoded(into: Obj, rec: Obj, isHex: boolean): Obj {
  const bytes: Obj = AsObj(Get(rec, "bytes"));
  const written: f64 = BufferLength(bytes);
  BufferCopy(bytes, 0, AsObj(TypedArrayField(into, 1)), AsNumber(TypedArrayField(into, 2)), written);
  if (AsBool(Get(rec, "error"))) {
    throw NewSyntaxError(isHex ? "invalid hex string" : "invalid base64 string");
  }
  const result: Obj = ObjectCreate(RealmIntrinsic("ObjectPrototype"));
  CreateDataPropertyOrThrow(result, "read", Get(rec, "read"));
  CreateDataPropertyOrThrow(result, "written", written);
  return result;
}

/**
 * Uint8Array.prototype.setFromBase64 ( string [ , options ] ).
 * @builtin Uint8Array.prototype.setFromBase64
 */
function Uint8ArrayPrototypeSetFromBase64(this: Value, string: Value, options?: Value): Obj {
  const into: Obj = ValidateUint8Array(this);
  RequireMutableTypedArray(into);
  if (!IsString(string)) {
    throw NewTypeError("expected string");
  }
  const opts: Value = GetOptionsObject(options);
  const url: boolean = Base64Alphabet(opts);
  const last: i32 = LastChunkHandling(opts);
  if (TAOutOfBounds(into)) {
    throw NewTypeError("ArrayBuffer is detached");
  }
  const rec: Obj = Base64Decode(AsStr(string), url, last, TALength(into));
  return WriteDecoded(into, rec, false);
}

/**
 * Uint8Array.prototype.setFromHex ( string ).
 * @builtin Uint8Array.prototype.setFromHex
 */
function Uint8ArrayPrototypeSetFromHex(this: Value, string: Value): Obj {
  const into: Obj = ValidateUint8Array(this);
  RequireMutableTypedArray(into);
  if (!IsString(string)) {
    throw NewTypeError("expected string");
  }
  if (TAOutOfBounds(into)) {
    throw NewTypeError("ArrayBuffer is detached");
  }
  const rec: Obj = HexDecode(AsStr(string), TALength(into));
  return WriteDecoded(into, rec, true);
}

/**
 * Int8Array ( ...args ), 23.2.5.1.
 * @constructor Int8Array
 * @extends %TypedArray%
 * @length 3
 */
function Int8ArrayConstructor(...args: Value[]): Obj {
  return TypedArrayConstruct(new.target, 0, args);
}

/**
 * Int8Array.BYTES_PER_ELEMENT, 23.2.6.1.
 * @value Int8Array.BYTES_PER_ELEMENT
 */
function Int8ArrayBytesPerElement(): f64 {
  return 1;
}

/**
 * Int8Array.prototype.BYTES_PER_ELEMENT, 23.2.7.1.
 * @value Int8Array.prototype.BYTES_PER_ELEMENT
 */
function Int8ArrayPrototypeBytesPerElement(): f64 {
  return 1;
}

/**
 * Uint8Array ( ...args ), 23.2.5.1.
 * @constructor Uint8Array
 * @extends %TypedArray%
 * @length 3
 */
function Uint8ArrayConstructor(...args: Value[]): Obj {
  return TypedArrayConstruct(new.target, 1, args);
}

/**
 * Uint8Array.BYTES_PER_ELEMENT, 23.2.6.1.
 * @value Uint8Array.BYTES_PER_ELEMENT
 */
function Uint8ArrayBytesPerElement(): f64 {
  return 1;
}

/**
 * Uint8Array.prototype.BYTES_PER_ELEMENT, 23.2.7.1.
 * @value Uint8Array.prototype.BYTES_PER_ELEMENT
 */
function Uint8ArrayPrototypeBytesPerElement(): f64 {
  return 1;
}

/**
 * Uint8ClampedArray ( ...args ), 23.2.5.1.
 * @constructor Uint8ClampedArray
 * @extends %TypedArray%
 * @length 3
 */
function Uint8ClampedArrayConstructor(...args: Value[]): Obj {
  return TypedArrayConstruct(new.target, 2, args);
}

/**
 * Uint8ClampedArray.BYTES_PER_ELEMENT, 23.2.6.1.
 * @value Uint8ClampedArray.BYTES_PER_ELEMENT
 */
function Uint8ClampedArrayBytesPerElement(): f64 {
  return 1;
}

/**
 * Uint8ClampedArray.prototype.BYTES_PER_ELEMENT, 23.2.7.1.
 * @value Uint8ClampedArray.prototype.BYTES_PER_ELEMENT
 */
function Uint8ClampedArrayPrototypeBytesPerElement(): f64 {
  return 1;
}

/**
 * Int16Array ( ...args ), 23.2.5.1.
 * @constructor Int16Array
 * @extends %TypedArray%
 * @length 3
 */
function Int16ArrayConstructor(...args: Value[]): Obj {
  return TypedArrayConstruct(new.target, 3, args);
}

/**
 * Int16Array.BYTES_PER_ELEMENT, 23.2.6.1.
 * @value Int16Array.BYTES_PER_ELEMENT
 */
function Int16ArrayBytesPerElement(): f64 {
  return 2;
}

/**
 * Int16Array.prototype.BYTES_PER_ELEMENT, 23.2.7.1.
 * @value Int16Array.prototype.BYTES_PER_ELEMENT
 */
function Int16ArrayPrototypeBytesPerElement(): f64 {
  return 2;
}

/**
 * Uint16Array ( ...args ), 23.2.5.1.
 * @constructor Uint16Array
 * @extends %TypedArray%
 * @length 3
 */
function Uint16ArrayConstructor(...args: Value[]): Obj {
  return TypedArrayConstruct(new.target, 4, args);
}

/**
 * Uint16Array.BYTES_PER_ELEMENT, 23.2.6.1.
 * @value Uint16Array.BYTES_PER_ELEMENT
 */
function Uint16ArrayBytesPerElement(): f64 {
  return 2;
}

/**
 * Uint16Array.prototype.BYTES_PER_ELEMENT, 23.2.7.1.
 * @value Uint16Array.prototype.BYTES_PER_ELEMENT
 */
function Uint16ArrayPrototypeBytesPerElement(): f64 {
  return 2;
}

/**
 * Int32Array ( ...args ), 23.2.5.1.
 * @constructor Int32Array
 * @extends %TypedArray%
 * @length 3
 */
function Int32ArrayConstructor(...args: Value[]): Obj {
  return TypedArrayConstruct(new.target, 5, args);
}

/**
 * Int32Array.BYTES_PER_ELEMENT, 23.2.6.1.
 * @value Int32Array.BYTES_PER_ELEMENT
 */
function Int32ArrayBytesPerElement(): f64 {
  return 4;
}

/**
 * Int32Array.prototype.BYTES_PER_ELEMENT, 23.2.7.1.
 * @value Int32Array.prototype.BYTES_PER_ELEMENT
 */
function Int32ArrayPrototypeBytesPerElement(): f64 {
  return 4;
}

/**
 * Uint32Array ( ...args ), 23.2.5.1.
 * @constructor Uint32Array
 * @extends %TypedArray%
 * @length 3
 */
function Uint32ArrayConstructor(...args: Value[]): Obj {
  return TypedArrayConstruct(new.target, 6, args);
}

/**
 * Uint32Array.BYTES_PER_ELEMENT, 23.2.6.1.
 * @value Uint32Array.BYTES_PER_ELEMENT
 */
function Uint32ArrayBytesPerElement(): f64 {
  return 4;
}

/**
 * Uint32Array.prototype.BYTES_PER_ELEMENT, 23.2.7.1.
 * @value Uint32Array.prototype.BYTES_PER_ELEMENT
 */
function Uint32ArrayPrototypeBytesPerElement(): f64 {
  return 4;
}

/**
 * Float16Array ( ...args ), 23.2.5.1.
 * @constructor Float16Array
 * @extends %TypedArray%
 * @length 3
 */
function Float16ArrayConstructor(...args: Value[]): Obj {
  return TypedArrayConstruct(new.target, 7, args);
}

/**
 * Float16Array.BYTES_PER_ELEMENT, 23.2.6.1.
 * @value Float16Array.BYTES_PER_ELEMENT
 */
function Float16ArrayBytesPerElement(): f64 {
  return 2;
}

/**
 * Float16Array.prototype.BYTES_PER_ELEMENT, 23.2.7.1.
 * @value Float16Array.prototype.BYTES_PER_ELEMENT
 */
function Float16ArrayPrototypeBytesPerElement(): f64 {
  return 2;
}

/**
 * Float32Array ( ...args ), 23.2.5.1.
 * @constructor Float32Array
 * @extends %TypedArray%
 * @length 3
 */
function Float32ArrayConstructor(...args: Value[]): Obj {
  return TypedArrayConstruct(new.target, 8, args);
}

/**
 * Float32Array.BYTES_PER_ELEMENT, 23.2.6.1.
 * @value Float32Array.BYTES_PER_ELEMENT
 */
function Float32ArrayBytesPerElement(): f64 {
  return 4;
}

/**
 * Float32Array.prototype.BYTES_PER_ELEMENT, 23.2.7.1.
 * @value Float32Array.prototype.BYTES_PER_ELEMENT
 */
function Float32ArrayPrototypeBytesPerElement(): f64 {
  return 4;
}

/**
 * Float64Array ( ...args ), 23.2.5.1.
 * @constructor Float64Array
 * @extends %TypedArray%
 * @length 3
 */
function Float64ArrayConstructor(...args: Value[]): Obj {
  return TypedArrayConstruct(new.target, 9, args);
}

/**
 * Float64Array.BYTES_PER_ELEMENT, 23.2.6.1.
 * @value Float64Array.BYTES_PER_ELEMENT
 */
function Float64ArrayBytesPerElement(): f64 {
  return 8;
}

/**
 * Float64Array.prototype.BYTES_PER_ELEMENT, 23.2.7.1.
 * @value Float64Array.prototype.BYTES_PER_ELEMENT
 */
function Float64ArrayPrototypeBytesPerElement(): f64 {
  return 8;
}

/**
 * BigInt64Array ( ...args ), 23.2.5.1.
 * @constructor BigInt64Array
 * @extends %TypedArray%
 * @length 3
 */
function BigInt64ArrayConstructor(...args: Value[]): Obj {
  return TypedArrayConstruct(new.target, 10, args);
}

/**
 * BigInt64Array.BYTES_PER_ELEMENT, 23.2.6.1.
 * @value BigInt64Array.BYTES_PER_ELEMENT
 */
function BigInt64ArrayBytesPerElement(): f64 {
  return 8;
}

/**
 * BigInt64Array.prototype.BYTES_PER_ELEMENT, 23.2.7.1.
 * @value BigInt64Array.prototype.BYTES_PER_ELEMENT
 */
function BigInt64ArrayPrototypeBytesPerElement(): f64 {
  return 8;
}

/**
 * BigUint64Array ( ...args ), 23.2.5.1.
 * @constructor BigUint64Array
 * @extends %TypedArray%
 * @length 3
 */
function BigUint64ArrayConstructor(...args: Value[]): Obj {
  return TypedArrayConstruct(new.target, 11, args);
}

/**
 * BigUint64Array.BYTES_PER_ELEMENT, 23.2.6.1.
 * @value BigUint64Array.BYTES_PER_ELEMENT
 */
function BigUint64ArrayBytesPerElement(): f64 {
  return 8;
}

/**
 * BigUint64Array.prototype.BYTES_PER_ELEMENT, 23.2.7.1.
 * @value BigUint64Array.prototype.BYTES_PER_ELEMENT
 */
function BigUint64ArrayPrototypeBytesPerElement(): f64 {
  return 8;
}

// ---- The rest of %TypedArray%.prototype (23.2.3) -----------------------------------------
// Like Array.prototype's, over the validated length; there are no holes.

/**
 * %TypedArray%.prototype.at ( index ), 23.2.3.1.
 * @builtin %TypedArray%.prototype.at
 */
function TypedArrayPrototypeAt(this: Value, index: Value): Value {
  const o: Obj = ValidateTypedArray(this);
  const len: f64 = TALength(o);
  const rel: f64 = ToIntegerOrInfinity(index);
  const k: f64 = rel >= 0 ? rel : len + rel;
  if (k < 0 || k >= len) {
    return undefined;
  }
  return TypedArrayGet(o, k);
}

/**
 * %TypedArray%.prototype.copyWithin ( target, start [ , end ] ), 23.2.3.6.
 * @builtin %TypedArray%.prototype.copyWithin
 */
function TypedArrayPrototypeCopyWithin(this: Value, target: Value, start: Value, end?: Value): Obj {
  const o: Obj = ValidateTypedArray(this);
  RequireMutableTypedArray(o);
  const len: f64 = TALength(o);
  const to: f64 = RelativeIndex(target, len, 0);
  const from: f64 = RelativeIndex(start, len, 0);
  const fin: f64 = RelativeIndex(end, len, len);
  let count: f64 = fin - from < len - to ? fin - from : len - to;
  if (count > 0) {
    if (TAOutOfBounds(o)) {
      throw NewTypeError("ArrayBuffer is detached or resized");
    }
    const len2: f64 = TALength(o);
    // A shrunk buffer bounds the copy (23.2.3.6 step 17).
    if (from + count > len2) {
      count = len2 - from;
    }
    if (to + count > len2) {
      count = len2 - to;
    }
    if (count > 0) {
      TypedArrayCopy(o, from, o, to, count);
    }
  }
  return o;
}

/**
 * %TypedArray%.prototype.entries ( ), 23.2.3.7.
 * @builtin %TypedArray%.prototype.entries
 */
function TypedArrayPrototypeEntries(this: Value): Obj {
  return CreateArrayIterator(ValidateTypedArray(this), 2);
}

/**
 * %TypedArray%.prototype.keys ( ), 23.2.3.19.
 * @builtin %TypedArray%.prototype.keys
 */
function TypedArrayPrototypeKeys(this: Value): Obj {
  return CreateArrayIterator(ValidateTypedArray(this), 0);
}

/**
 * %TypedArray%.prototype.values ( ), 23.2.3.35; also [ %Symbol.iterator% ].
 * @builtin %TypedArray%.prototype.values
 * @builtin %TypedArray%.prototype[Symbol.iterator]
 */
function TypedArrayPrototypeValues(this: Value): Obj {
  return CreateArrayIterator(ValidateTypedArray(this), 1);
}

/** every (`wantAll`) and some over a typed array. */
function TAEveryOrSome(o: Obj, callbackfn: Value, thisArg: Value, wantAll: boolean): boolean {
  const len: f64 = TALength(o);
  RequireCallable(callbackfn);
  for (let k: f64 = 0; k < len; k++) {
    const r: boolean = ToBoolean(Call(callbackfn, thisArg, TypedArrayGet(o, k), k, o));
    if (r !== wantAll) {
      return r;
    }
  }
  return wantAll;
}

/**
 * %TypedArray%.prototype.every ( callbackfn [ , thisArg ] ), 23.2.3.8.
 * @builtin %TypedArray%.prototype.every
 */
function TypedArrayPrototypeEvery(this: Value, callbackfn: Value, thisArg?: Value): boolean {
  return TAEveryOrSome(ValidateTypedArray(this), callbackfn, thisArg, true);
}

/**
 * %TypedArray%.prototype.some ( callbackfn [ , thisArg ] ), 23.2.3.29.
 * @builtin %TypedArray%.prototype.some
 */
function TypedArrayPrototypeSome(this: Value, callbackfn: Value, thisArg?: Value): boolean {
  return TAEveryOrSome(ValidateTypedArray(this), callbackfn, thisArg, false);
}

/**
 * %TypedArray%.prototype.filter ( callbackfn [ , thisArg ] ), 23.2.3.10.
 * @builtin %TypedArray%.prototype.filter
 */
function TypedArrayPrototypeFilter(this: Value, callbackfn: Value, thisArg?: Value): Obj {
  const o: Obj = ValidateTypedArray(this);
  const len: f64 = TALength(o);
  RequireCallable(callbackfn);
  const kept: Obj = ArrayCreate(0);
  let captured: f64 = 0;
  for (let k: f64 = 0; k < len; k++) {
    const kValue: Value = TypedArrayGet(o, k);
    if (ToBoolean(Call(callbackfn, thisArg, kValue, k, o))) {
      CreateDataPropertyOrThrow(kept, captured, kValue);
      captured++;
    }
  }
  const a: Obj = TypedArraySpeciesCreate(o, captured);
  for (let n: f64 = 0; n < captured; n++) {
    TASet(a, n, Get(kept, n));
  }
  return a;
}

/** The find family over a typed array. */
function TAFind(thisValue: Value, predicate: Value, thisArg: Value, fromEnd: boolean, wantIndex: boolean): Value {
  const o: Obj = ValidateTypedArray(thisValue);
  const len: f64 = TALength(o);
  RequireCallable(predicate);
  for (let i: f64 = 0; i < len; i++) {
    const k: f64 = fromEnd ? len - 1 - i : i;
    const kValue: Value = TypedArrayGet(o, k);
    if (ToBoolean(Call(predicate, thisArg, kValue, k, o))) {
      return wantIndex ? k : kValue;
    }
  }
  if (wantIndex) {
    return -1;
  }
  return undefined;
}

/**
 * %TypedArray%.prototype.find ( predicate [ , thisArg ] ), 23.2.3.11.
 * @builtin %TypedArray%.prototype.find
 */
function TypedArrayPrototypeFind(this: Value, predicate: Value, thisArg?: Value): Value {
  return TAFind(this, predicate, thisArg, false, false);
}

/**
 * %TypedArray%.prototype.findIndex ( predicate [ , thisArg ] ), 23.2.3.12.
 * @builtin %TypedArray%.prototype.findIndex
 */
function TypedArrayPrototypeFindIndex(this: Value, predicate: Value, thisArg?: Value): Value {
  return TAFind(this, predicate, thisArg, false, true);
}

/**
 * %TypedArray%.prototype.findLast ( predicate [ , thisArg ] ), 23.2.3.13.
 * @builtin %TypedArray%.prototype.findLast
 */
function TypedArrayPrototypeFindLast(this: Value, predicate: Value, thisArg?: Value): Value {
  return TAFind(this, predicate, thisArg, true, false);
}

/**
 * %TypedArray%.prototype.findLastIndex ( predicate [ , thisArg ] ), 23.2.3.14.
 * @builtin %TypedArray%.prototype.findLastIndex
 */
function TypedArrayPrototypeFindLastIndex(this: Value, predicate: Value, thisArg?: Value): Value {
  return TAFind(this, predicate, thisArg, true, true);
}

/**
 * %TypedArray%.prototype.forEach ( callbackfn [ , thisArg ] ), 23.2.3.15.
 * @builtin %TypedArray%.prototype.forEach
 */
function TypedArrayPrototypeForEach(this: Value, callbackfn: Value, thisArg?: Value): void {
  const o: Obj = ValidateTypedArray(this);
  const len: f64 = TALength(o);
  RequireCallable(callbackfn);
  for (let k: f64 = 0; k < len; k++) {
    Call(callbackfn, thisArg, TypedArrayGet(o, k), k, o);
  }
}

/**
 * %TypedArray%.prototype.includes ( searchElement [ , fromIndex ] ), 23.2.3.16.
 * @builtin %TypedArray%.prototype.includes
 */
function TypedArrayPrototypeIncludes(this: Value, searchElement: Value, fromIndex?: Value): boolean {
  const o: Obj = ValidateTypedArray(this);
  const len: f64 = TALength(o);
  if (len === 0) {
    return false;
  }
  let n: f64 = ToIntegerOrInfinity(fromIndex);
  if (n === 1 / 0) {
    return false;
  }
  if (n < 0) {
    n = len + n;
    if (n < 0) {
      n = 0;
    }
  }
  for (let k: f64 = n; k < len; k++) {
    if (SameValueZero(TypedArrayGet(o, k), searchElement)) {
      return true;
    }
  }
  return false;
}

/**
 * %TypedArray%.prototype.indexOf ( searchElement [ , fromIndex ] ), 23.2.3.17.
 * @builtin %TypedArray%.prototype.indexOf
 */
function TypedArrayPrototypeIndexOf(this: Value, searchElement: Value, fromIndex?: Value): f64 {
  const o: Obj = ValidateTypedArray(this);
  const len: f64 = TALength(o);
  if (len === 0) {
    return -1;
  }
  let n: f64 = ToIntegerOrInfinity(fromIndex);
  if (n === 1 / 0) {
    return -1;
  }
  if (n < 0) {
    n = len + n;
    if (n < 0) {
      n = 0;
    }
  }
  for (let k: f64 = n; k < len; k++) {
    // A shrunk buffer: missing elements are not present (HasProperty is false).
    const v: Value = TypedArrayGet(o, k);
    if (v !== undefined && v === searchElement) {
      return k;
    }
  }
  return -1;
}

/**
 * %TypedArray%.prototype.lastIndexOf ( searchElement [ , fromIndex ] ), 23.2.3.20.
 * @builtin %TypedArray%.prototype.lastIndexOf
 */
function TypedArrayPrototypeLastIndexOf(this: Value, searchElement: Value, ...rest: Value[]): f64 {
  const o: Obj = ValidateTypedArray(this);
  const len: f64 = TALength(o);
  if (len === 0) {
    return -1;
  }
  let n: f64 = len - 1;
  if (LengthOfArrayLike(rest) > 0) {
    n = ToIntegerOrInfinity(Get(rest, 0));
  }
  if (n === -1 / 0) {
    return -1;
  }
  let k: f64 = n >= 0 ? (n < len - 1 ? n : len - 1) : len + n;
  for (; k >= 0; k--) {
    const v: Value = TypedArrayGet(o, k);
    if (v !== undefined && v === searchElement) {
      return k;
    }
  }
  return -1;
}

/**
 * %TypedArray%.prototype.join ( separator ), 23.2.3.18.
 * @builtin %TypedArray%.prototype.join
 */
function TypedArrayPrototypeJoin(this: Value, separator: Value): Str {
  const o: Obj = ValidateTypedArray(this);
  const len: f64 = TALength(o);
  let sep: Str = ",";
  if (separator !== undefined) {
    sep = ToString(separator);
  }
  let r: Str = "";
  for (let k: f64 = 0; k < len; k++) {
    if (k > 0) {
      r += sep;
    }
    const element: Value = TypedArrayGet(o, k);
    if (element !== undefined) {
      r += ToString(element);
    }
  }
  return r;
}

/**
 * %TypedArray%.prototype.map ( callbackfn [ , thisArg ] ), 23.2.3.22.
 * @builtin %TypedArray%.prototype.map
 */
function TypedArrayPrototypeMap(this: Value, callbackfn: Value, thisArg?: Value): Obj {
  const o: Obj = ValidateTypedArray(this);
  const len: f64 = TALength(o);
  RequireCallable(callbackfn);
  const a: Obj = TypedArraySpeciesCreate(o, len);
  for (let k: f64 = 0; k < len; k++) {
    const mapped: Value = Call(callbackfn, thisArg, TypedArrayGet(o, k), k, o);
    TASet(a, k, mapped);
  }
  return a;
}

/** reduce and reduceRight over a typed array. */
function TAReduce(o: Obj, callbackfn: Value, rest: Obj, fromEnd: boolean): Value {
  const len: f64 = TALength(o);
  RequireCallable(callbackfn);
  let i: f64 = 0;
  let accumulator: Value = undefined;
  if (LengthOfArrayLike(rest) > 0) {
    accumulator = Get(rest, 0);
  } else {
    if (len === 0) {
      throw NewTypeError("empty array");
    }
    accumulator = TypedArrayGet(o, fromEnd ? len - 1 : 0);
    i = 1;
  }
  for (; i < len; i++) {
    const k: f64 = fromEnd ? len - 1 - i : i;
    accumulator = Call(callbackfn, undefined, accumulator, TypedArrayGet(o, k), k, o);
  }
  return accumulator;
}

/**
 * %TypedArray%.prototype.reduce ( callbackfn [ , initialValue ] ), 23.2.3.23.
 * @builtin %TypedArray%.prototype.reduce
 * @length 1
 */
function TypedArrayPrototypeReduce(this: Value, callbackfn: Value, ...rest: Value[]): Value {
  return TAReduce(ValidateTypedArray(this), callbackfn, rest, false);
}

/**
 * %TypedArray%.prototype.reduceRight ( callbackfn [ , initialValue ] ), 23.2.3.24.
 * @builtin %TypedArray%.prototype.reduceRight
 * @length 1
 */
function TypedArrayPrototypeReduceRight(this: Value, callbackfn: Value, ...rest: Value[]): Value {
  return TAReduce(ValidateTypedArray(this), callbackfn, rest, true);
}

/**
 * %TypedArray%.prototype.reverse ( ), 23.2.3.25.
 * @builtin %TypedArray%.prototype.reverse
 */
function TypedArrayPrototypeReverse(this: Value): Obj {
  const o: Obj = ValidateTypedArray(this);
  RequireMutableTypedArray(o);
  const len: f64 = TALength(o);
  for (let lower: f64 = 0; lower < F64Floor(len / 2); lower++) {
    const upper: f64 = len - lower - 1;
    const lv: Value = TypedArrayGet(o, lower);
    const uv: Value = TypedArrayGet(o, upper);
    TypedArraySet(o, lower, uv);
    TypedArraySet(o, upper, lv);
  }
  return o;
}

/** The elements of a typed array sorted with comparefn (or numerically), as a list. */
function TASortedList(o: Obj, len: f64, comparefn: Value): Obj {
  const list: Obj = ArrayCreate(0);
  for (let k: f64 = 0; k < len; k++) {
    CreateDataPropertyOrThrow(list, k, TypedArrayGet(o, k));
  }
  if (comparefn === undefined) {
    SortNumbers(list);
  } else {
    SortList(list, comparefn);
  }
  return list;
}

/**
 * %TypedArray%.prototype.sort ( comparefn ), 23.2.3.30.
 * @builtin %TypedArray%.prototype.sort
 */
function TypedArrayPrototypeSort(this: Value, comparefn: Value): Obj {
  if (comparefn !== undefined && !IsCallable(comparefn)) {
    throw NewTypeError("not a function");
  }
  const o: Obj = ValidateTypedArray(this);
  RequireMutableTypedArray(o);
  const len: f64 = TALength(o);
  const list: Obj = TASortedList(o, len, comparefn);
  for (let k: f64 = 0; k < len; k++) {
    TypedArraySet(o, k, Get(list, k));
  }
  return o;
}

/**
 * %TypedArray%.prototype.toSorted ( comparefn ), 23.2.3.34.
 * @builtin %TypedArray%.prototype.toSorted
 */
function TypedArrayPrototypeToSorted(this: Value, comparefn: Value): Obj {
  if (comparefn !== undefined && !IsCallable(comparefn)) {
    throw NewTypeError("not a function");
  }
  const o: Obj = ValidateTypedArray(this);
  const len: f64 = TALength(o);
  const a: Obj = AllocateTypedArrayOfLength(KindOf(o), TypedArrayKindProto(KindOf(o)), len);
  const list: Obj = TASortedList(o, len, comparefn);
  for (let k: f64 = 0; k < len; k++) {
    TypedArraySet(a, k, Get(list, k));
  }
  return a;
}

/**
 * %TypedArray%.prototype.toReversed ( ), 23.2.3.33.
 * @builtin %TypedArray%.prototype.toReversed
 */
function TypedArrayPrototypeToReversed(this: Value): Obj {
  const o: Obj = ValidateTypedArray(this);
  const len: f64 = TALength(o);
  const a: Obj = AllocateTypedArrayOfLength(KindOf(o), TypedArrayKindProto(KindOf(o)), len);
  for (let k: f64 = 0; k < len; k++) {
    TypedArraySet(a, k, TypedArrayGet(o, len - k - 1));
  }
  return a;
}

/**
 * %TypedArray%.prototype.with ( index, value ), 23.2.3.36.
 * @builtin %TypedArray%.prototype.with
 */
function TypedArrayPrototypeWith(this: Value, index: Value, value: Value): Obj {
  const o: Obj = ValidateTypedArray(this);
  const len: f64 = TALength(o);
  const rel: f64 = ToIntegerOrInfinity(index);
  const actual: f64 = rel >= 0 ? rel : len + rel;
  const n: Value = ToElementValue(o, value);
  if (!(actual >= 0 && actual < TALength(o))) {
    throw NewRangeError("invalid array index");
  }
  const a: Obj = AllocateTypedArrayOfLength(KindOf(o), TypedArrayKindProto(KindOf(o)), len);
  for (let k: f64 = 0; k < len; k++) {
    TypedArraySet(a, k, k === actual ? n : TypedArrayGet(o, k));
  }
  return a;
}

/**
 * %TypedArray%.prototype.toLocaleString ( [ reserved1 [ , reserved2 ] ] ), 23.2.3.32.
 * @builtin %TypedArray%.prototype.toLocaleString
 */
function TypedArrayPrototypeToLocaleString(this: Value): Str {
  const o: Obj = ValidateTypedArray(this);
  const len: f64 = TALength(o);
  let r: Str = "";
  for (let k: f64 = 0; k < len; k++) {
    if (k > 0) {
      r += ",";
    }
    const v: Value = TypedArrayGet(o, k);
    if (v !== undefined) {
      r += ToString(Call(GetV(v, "toLocaleString"), v));
    }
  }
  return r;
}

/** CanonicalNumericIndexString (7.1.21) is not undefined: `key` names an element slot. */
function IsCanonicalNumericKey(key: Value): boolean {
  // ToPropertyKey gives integer keys as Numbers.
  if (IsNumber(key)) {
    return true;
  }
  if (!IsString(key)) {
    return false;
  }
  return key === "-0" || ToString(ToNumber(key)) === key;
}

/**
 * TypedArraySetElement ( O, index, value ), 10.4.5.16, for builtins' Set(A, k, v, true)
 * on a typed array: the value is converted first (user code), then written if the index
 * is still valid.
 */
function TASet(o: Obj, k: f64, v: Value): void {
  TypedArraySet(o, k, ToElementValue(o, v));
}

