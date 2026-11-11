// ArrayBuffer (ECMA-262 25.1). The bytes live in a Bytes cell (heap/typed.rs, D1390).
// @slim-omit: left out of the slim build (D2411).

/** ToIndex ( value ), 7.1.22 (qjs's message). */
function ToIndex(v: Value): f64 {
  const i: f64 = ToIntegerOrInfinity(v);
  if (!(i >= 0 && i <= 9007199254740991)) {
    throw NewRangeError("invalid array index");
  }
  return i;
}

/** The ArrayBuffer `this` of a prototype method. */
function ThisArrayBuffer(v: Value): Obj {
  BufferInfo(v, 0);
  return AsObj(v);
}

/** AllocateArrayBuffer ( constructor, byteLength [ , maxByteLength ] ), 25.1.3.1. */
function AllocateArrayBuffer(newTarget: Value, byteLength: f64, max: Value): Obj {
  if (max !== undefined && byteLength > AsNumber(max)) {
    throw NewRangeError("invalid array buffer max length");
  }
  const proto: Obj = GetPrototypeFromConstructor(newTarget, "ArrayBufferPrototype");
  return ArrayBufferCreate(proto, byteLength, max);
}

/**
 * ArrayBuffer ( length [ , options ] ), 25.1.4.1.
 * @constructor ArrayBuffer
 * @length 1
 */
function ArrayBufferConstructor(length: Value, options?: Value): Obj {
  if (new.target === undefined) {
    throw NewTypeError("must be called with new");
  }
  const byteLength: f64 = ToIndex(length);
  let max: Value = undefined;
  if (IsObject(options)) {
    const m: Value = Get(AsObj(options), "maxByteLength");
    if (m !== undefined) {
      max = ToIndex(m);
    }
  }
  return AllocateArrayBuffer(new.target, byteLength, max);
}

/**
 * ArrayBuffer.isView ( arg ), 25.1.5.1.
 * @builtin ArrayBuffer.isView
 */
function ArrayBufferIsView(arg: Value): boolean {
  return IsTypedArray(arg) || IsDataView(arg);
}

/**
 * get ArrayBuffer [ %Symbol.species% ], 25.1.5.3.
 * @getter ArrayBuffer[Symbol.species]
 */
function ArrayBufferSpecies(this: Value): Value {
  return this;
}

/**
 * get ArrayBuffer.prototype.byteLength, 25.1.6.2.
 * @getter ArrayBuffer.prototype.byteLength
 */
function ArrayBufferPrototypeByteLength(this: Value): Value {
  return BufferInfo(this, 0);
}

/**
 * get ArrayBuffer.prototype.maxByteLength, 25.1.6.5.
 * @getter ArrayBuffer.prototype.maxByteLength
 */
function ArrayBufferPrototypeMaxByteLength(this: Value): Value {
  return BufferInfo(this, 1);
}

/**
 * get ArrayBuffer.prototype.resizable, 25.1.6.6.
 * @getter ArrayBuffer.prototype.resizable
 */
function ArrayBufferPrototypeResizable(this: Value): Value {
  return BufferInfo(this, 2);
}

/**
 * get ArrayBuffer.prototype.detached, 25.1.6.3.
 * @getter ArrayBuffer.prototype.detached
 */
function ArrayBufferPrototypeDetached(this: Value): Value {
  return BufferInfo(this, 3);
}

/**
 * get ArrayBuffer.prototype.immutable (the immutable ArrayBuffer proposal, D1970).
 * @getter ArrayBuffer.prototype.immutable
 */
function ArrayBufferPrototypeImmutable(this: Value): Value {
  return BufferInfo(this, 4);
}

/** IsImmutableBuffer ( buffer ): a buffer whose bytes never change (D1970). */
function IsImmutableBuffer(b: Obj): boolean {
  return AsBool(BufferInfo(b, 4));
}

/** Whether a buffer is detached. */
function IsDetached(b: Obj): boolean {
  return AsBool(BufferInfo(b, 3));
}

/** The byte length of a buffer. */
function BufferLength(b: Obj): f64 {
  return AsNumber(BufferInfo(b, 0));
}

/**
 * ArrayBuffer.prototype.resize ( newLength ), 25.1.6.7.
 * @builtin ArrayBuffer.prototype.resize
 */
function ArrayBufferPrototypeResize(this: Value, newLength: Value): void {
  const o: Obj = ThisArrayBuffer(this);
  if (!AsBool(BufferInfo(o, 2))) {
    throw NewTypeError("array buffer is not resizable");
  }
  const newByteLength: f64 = ToIndex(newLength);
  if (IsDetached(o)) {
    throw NewTypeError("ArrayBuffer is detached");
  }
  if (newByteLength > AsNumber(BufferInfo(o, 1))) {
    throw NewRangeError("invalid array buffer length");
  }
  BufferResize(o, newByteLength);
}

/**
 * ArrayBuffer.prototype.slice ( start, end ), 25.1.6.8.
 * @builtin ArrayBuffer.prototype.slice
 */
function ArrayBufferPrototypeSlice(this: Value, start: Value, end: Value): Obj {
  const o: Obj = ThisArrayBuffer(this);
  if (IsDetached(o)) {
    throw NewTypeError("ArrayBuffer is detached");
  }
  const len: f64 = BufferLength(o);
  const first: f64 = RelativeIndex(start, len, 0);
  const fin: f64 = RelativeIndex(end, len, len);
  const newLen: f64 = fin > first ? fin - first : 0;
  const ctor: Value = SpeciesConstructor(o, RealmConstructor("ArrayBuffer"));
  const created: Obj = Construct(ctor, newLen);
  BufferInfo(created, 0);
  if (IsDetached(created)) {
    throw NewTypeError("ArrayBuffer is detached");
  }
  if (IsImmutableBuffer(created)) {
    throw NewTypeError("ArrayBuffer is immutable");
  }
  if (created === o) {
    throw NewTypeError("cannot use identical ArrayBuffer");
  }
  if (BufferLength(created) < newLen) {
    throw NewTypeError("new ArrayBuffer is too small");
  }
  if (IsDetached(o)) {
    throw NewTypeError("ArrayBuffer is detached");
  }
  const currentLen: f64 = BufferLength(o);
  if (first < currentLen) {
    const count: f64 = newLen < currentLen - first ? newLen : currentLen - first;
    BufferCopy(o, first, created, 0, count);
  }
  return created;
}

/**
 * ArrayBufferCopyAndDetach ( O, newLength, preserveResizability ), 25.1.3.3; mode 0
 * fixed length, 1 preserve resizability, 2 immutable (D1970).
 */
function ArrayBufferCopyAndDetach(thisValue: Value, newLength: Value, mode: i32): Obj {
  const o: Obj = ThisArrayBuffer(thisValue);
  let newByteLength: f64 = 0;
  if (newLength === undefined) {
    newByteLength = BufferLength(o);
  } else {
    newByteLength = ToIndex(newLength);
  }
  if (IsDetached(o)) {
    throw NewTypeError("ArrayBuffer is detached");
  }
  if (IsImmutableBuffer(o)) {
    throw NewTypeError("ArrayBuffer is immutable");
  }
  let max: Value = undefined;
  if (mode === 1 && AsBool(BufferInfo(o, 2))) {
    max = BufferInfo(o, 1);
  }
  if (mode === 2) {
    // BufferTransfer's `max` true: an immutable buffer.
    return BufferTransfer(o, newByteLength, true);
  }
  if (max !== undefined && newByteLength > AsNumber(max)) {
    throw NewRangeError("invalid array buffer length");
  }
  return BufferTransfer(o, newByteLength, max);
}

/**
 * ArrayBuffer.prototype.transfer ( [ newLength ] ), 25.1.6.9.
 * @builtin ArrayBuffer.prototype.transfer
 */
function ArrayBufferPrototypeTransfer(this: Value, newLength?: Value): Obj {
  return ArrayBufferCopyAndDetach(this, newLength, 1);
}

/**
 * ArrayBuffer.prototype.transferToFixedLength ( [ newLength ] ), 25.1.6.10.
 * @builtin ArrayBuffer.prototype.transferToFixedLength
 */
function ArrayBufferPrototypeTransferToFixedLength(this: Value, newLength?: Value): Obj {
  return ArrayBufferCopyAndDetach(this, newLength, 0);
}

/**
 * ArrayBuffer.prototype.transferToImmutable ( [ newLength ] ) (D1970).
 * @builtin ArrayBuffer.prototype.transferToImmutable
 */
function ArrayBufferPrototypeTransferToImmutable(this: Value, newLength?: Value): Obj {
  return ArrayBufferCopyAndDetach(this, newLength, 2);
}

/**
 * ArrayBuffer.prototype.sliceToImmutable ( start, end ) (D1970): a new immutable
 * buffer with a copy of the bytes; the species constructor is not used.
 * @builtin ArrayBuffer.prototype.sliceToImmutable
 */
function ArrayBufferPrototypeSliceToImmutable(this: Value, start: Value, end: Value): Obj {
  const o: Obj = ThisArrayBuffer(this);
  if (IsDetached(o)) {
    throw NewTypeError("ArrayBuffer is detached");
  }
  const len: f64 = BufferLength(o);
  const first: f64 = RelativeIndex(start, len, 0);
  const fin: f64 = RelativeIndex(end, len, len);
  const newLen: f64 = fin > first ? fin - first : 0;
  if (IsDetached(o)) {
    throw NewTypeError("ArrayBuffer is detached");
  }
  if (BufferLength(o) < fin) {
    throw NewRangeError("invalid array buffer length");
  }
  const created: Obj = ArrayBufferCreate(RealmIntrinsic("ArrayBufferPrototype"), newLen, true);
  BufferCopy(o, first, created, 0, newLen);
  return created;
}

/**
 * ArrayBuffer.prototype [ %Symbol.toStringTag% ], 25.1.6.11.
 * @value ArrayBuffer.prototype[Symbol.toStringTag]
 * @attributes configurable
 */
function ArrayBufferPrototypeToStringTag(): Str {
  return "ArrayBuffer";
}
