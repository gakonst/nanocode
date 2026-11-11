// DataView (ECMA-262 25.3): a view of an ArrayBuffer's bytes, either endianness. Slots in
// builtins/typed.rs (`data_view_create`); the bytes go through heap/typed.rs's element
// encodings (D1390). BigInt64/BigUint64 read and write BigInts (0046).
// @slim-omit: left out of the slim build (D2411).

/** IsViewOutOfBounds ( viewRecord ), 25.3.1.3 (detached counts). */
function DVOutOfBounds(view: Obj): boolean {
  const buffer: Value = DataViewField(view, 0);
  if (AsBool(BufferInfo(buffer, 3))) {
    return true;
  }
  const bufferByteLength: f64 = AsNumber(BufferInfo(buffer, 0));
  const start: f64 = AsNumber(DataViewField(view, 1));
  const length: f64 = AsNumber(DataViewField(view, 2));
  const end: f64 = length < 0 ? bufferByteLength : start + length;
  return start > bufferByteLength || end > bufferByteLength;
}

/** GetViewByteLength ( viewRecord ), 25.3.1.2, of a view in bounds. */
function DVByteLength(view: Obj): f64 {
  const length: f64 = AsNumber(DataViewField(view, 2));
  if (length >= 0) {
    return length;
  }
  return AsNumber(BufferInfo(DataViewField(view, 0), 0)) - AsNumber(DataViewField(view, 1));
}

/**
 * DataView ( buffer [ , byteOffset [ , byteLength ] ] ), 25.3.2.1 (qjs's messages).
 * @constructor DataView
 * @length 1
 */
function DataViewConstructor(buffer: Value, byteOffset?: Value, byteLength?: Value): Obj {
  if (new.target === undefined) {
    throw NewTypeError("must be called with new");
  }
  BufferInfo(buffer, 0);
  const b: Obj = AsObj(buffer);
  const offset: f64 = ToIndex(byteOffset);
  if (AsBool(BufferInfo(b, 3))) {
    throw NewTypeError("ArrayBuffer is detached");
  }
  let bufferByteLength: f64 = AsNumber(BufferInfo(b, 0));
  if (offset > bufferByteLength) {
    throw NewRangeError("invalid byteOffset");
  }
  const tracking: boolean = byteLength === undefined && AsBool(BufferInfo(b, 2));
  let viewByteLength: f64 = -1;
  if (byteLength === undefined) {
    if (!tracking) {
      viewByteLength = bufferByteLength - offset;
    }
  } else {
    viewByteLength = ToIndex(byteLength);
    if (offset + viewByteLength > bufferByteLength) {
      throw NewRangeError("invalid byteLength");
    }
  }
  const proto: Obj = GetPrototypeFromConstructor(new.target, "DataViewPrototype");
  // The prototype lookup can run user code that detaches or shrinks the buffer.
  if (AsBool(BufferInfo(b, 3))) {
    throw NewTypeError("ArrayBuffer is detached");
  }
  bufferByteLength = AsNumber(BufferInfo(b, 0));
  if (offset > bufferByteLength) {
    throw NewRangeError("invalid byteOffset");
  }
  if (byteLength !== undefined && offset + viewByteLength > bufferByteLength) {
    throw NewRangeError("invalid byteLength");
  }
  return DataViewCreate(proto, b, offset, viewByteLength);
}

/**
 * get DataView.prototype.buffer, 25.3.4.1.
 * @getter DataView.prototype.buffer
 */
function DataViewPrototypeBuffer(this: Value): Value {
  return DataViewField(this, 0);
}

/**
 * get DataView.prototype.byteLength, 25.3.4.2.
 * @getter DataView.prototype.byteLength
 */
function DataViewPrototypeByteLength(this: Value): f64 {
  DataViewField(this, 0);
  const view: Obj = AsObj(this);
  if (DVOutOfBounds(view)) {
    throw NewTypeError("ArrayBuffer is detached or resized");
  }
  return DVByteLength(view);
}

/**
 * get DataView.prototype.byteOffset, 25.3.4.3.
 * @getter DataView.prototype.byteOffset
 */
function DataViewPrototypeByteOffset(this: Value): f64 {
  DataViewField(this, 0);
  const view: Obj = AsObj(this);
  if (DVOutOfBounds(view)) {
    throw NewTypeError("ArrayBuffer is detached or resized");
  }
  return AsNumber(DataViewField(view, 1));
}

/**
 * DataView.prototype [ %Symbol.toStringTag% ], 25.3.4.25: "DataView".
 * @value DataView.prototype[Symbol.toStringTag]
 * @attributes configurable
 */
function DataViewPrototypeToStringTag(): Str {
  return "DataView";
}

/** The byte of the buffer a get or set at requestIndex reaches, after the checks of 25.3.1.5/6. */
function DVByteIndex(view: Obj, getIndex: f64, kind: i32): f64 {
  if (DVOutOfBounds(view)) {
    if (AsBool(BufferInfo(DataViewField(view, 0), 3))) {
      throw NewTypeError("ArrayBuffer is detached");
    }
    throw NewTypeError("ArrayBuffer is detached or resized");
  }
  if (getIndex + ElementSize(kind) > DVByteLength(view)) {
    throw NewRangeError("out of bound");
  }
  return getIndex + AsNumber(DataViewField(view, 1));
}

/** GetViewValue ( view, requestIndex, isLittleEndian, type ), 25.3.1.5. */
function GetViewValue(view: Value, requestIndex: Value, isLittleEndian: Value, kind: i32): f64 {
  return AsNumber(GetViewElement(view, requestIndex, isLittleEndian, kind));
}

/** GetViewValue for any kind: a Number, or a BigInt for kinds 10 and 11. */
function GetViewElement(view: Value, requestIndex: Value, isLittleEndian: Value, kind: i32): Value {
  DataViewField(view, 0);
  const getIndex: f64 = ToIndex(requestIndex);
  const little: boolean = ToBoolean(isLittleEndian);
  const v: Obj = AsObj(view);
  return DataViewGet(v, DVByteIndex(v, getIndex, kind), kind, little);
}

/** SetViewValue ( view, requestIndex, isLittleEndian, type, value ), 25.3.1.6. */
function SetViewValue(view: Value, requestIndex: Value, isLittleEndian: Value, kind: i32, value: Value): void {
  // An immutable buffer is a TypeError before anything is converted (D1970).
  if (IsImmutableBuffer(AsObj(DataViewField(view, 0)))) {
    throw NewTypeError("ArrayBuffer is immutable");
  }
  const getIndex: f64 = ToIndex(requestIndex);
  let n: Value = undefined;
  if (kind >= 10) {
    n = ToBigInt(value);
  } else {
    n = ToNumber(value);
  }
  const little: boolean = ToBoolean(isLittleEndian);
  const v: Obj = AsObj(view);
  DataViewSet(v, DVByteIndex(v, getIndex, kind), kind, n, little);
}

/**
 * DataView.prototype.getInt8 ( byteOffset ).
 * @builtin DataView.prototype.getInt8
 */
function DataViewPrototypeGetInt8(this: Value, byteOffset: Value): f64 {
  return GetViewValue(this, byteOffset, false, 0);
}

/**
 * DataView.prototype.getUint8 ( byteOffset ).
 * @builtin DataView.prototype.getUint8
 */
function DataViewPrototypeGetUint8(this: Value, byteOffset: Value): f64 {
  return GetViewValue(this, byteOffset, false, 1);
}

/**
 * DataView.prototype.getInt16 ( byteOffset [ , littleEndian ] ).
 * @builtin DataView.prototype.getInt16
 */
function DataViewPrototypeGetInt16(this: Value, byteOffset: Value, littleEndian?: Value): f64 {
  return GetViewValue(this, byteOffset, littleEndian, 3);
}

/**
 * DataView.prototype.getUint16 ( byteOffset [ , littleEndian ] ).
 * @builtin DataView.prototype.getUint16
 */
function DataViewPrototypeGetUint16(this: Value, byteOffset: Value, littleEndian?: Value): f64 {
  return GetViewValue(this, byteOffset, littleEndian, 4);
}

/**
 * DataView.prototype.getInt32 ( byteOffset [ , littleEndian ] ).
 * @builtin DataView.prototype.getInt32
 */
function DataViewPrototypeGetInt32(this: Value, byteOffset: Value, littleEndian?: Value): f64 {
  return GetViewValue(this, byteOffset, littleEndian, 5);
}

/**
 * DataView.prototype.getUint32 ( byteOffset [ , littleEndian ] ).
 * @builtin DataView.prototype.getUint32
 */
function DataViewPrototypeGetUint32(this: Value, byteOffset: Value, littleEndian?: Value): f64 {
  return GetViewValue(this, byteOffset, littleEndian, 6);
}

/**
 * DataView.prototype.getBigInt64 ( byteOffset [ , littleEndian ] ).
 * @builtin DataView.prototype.getBigInt64
 */
function DataViewPrototypeGetBigInt64(this: Value, byteOffset: Value, littleEndian?: Value): Value {
  return GetViewElement(this, byteOffset, littleEndian, 10);
}

/**
 * DataView.prototype.getBigUint64 ( byteOffset [ , littleEndian ] ).
 * @builtin DataView.prototype.getBigUint64
 */
function DataViewPrototypeGetBigUint64(this: Value, byteOffset: Value, littleEndian?: Value): Value {
  return GetViewElement(this, byteOffset, littleEndian, 11);
}

/**
 * DataView.prototype.getFloat16 ( byteOffset [ , littleEndian ] ).
 * @builtin DataView.prototype.getFloat16
 */
function DataViewPrototypeGetFloat16(this: Value, byteOffset: Value, littleEndian?: Value): f64 {
  return GetViewValue(this, byteOffset, littleEndian, 7);
}

/**
 * DataView.prototype.getFloat32 ( byteOffset [ , littleEndian ] ).
 * @builtin DataView.prototype.getFloat32
 */
function DataViewPrototypeGetFloat32(this: Value, byteOffset: Value, littleEndian?: Value): f64 {
  return GetViewValue(this, byteOffset, littleEndian, 8);
}

/**
 * DataView.prototype.getFloat64 ( byteOffset [ , littleEndian ] ).
 * @builtin DataView.prototype.getFloat64
 */
function DataViewPrototypeGetFloat64(this: Value, byteOffset: Value, littleEndian?: Value): f64 {
  return GetViewValue(this, byteOffset, littleEndian, 9);
}

/**
 * DataView.prototype.setInt8 ( byteOffset, value ).
 * @builtin DataView.prototype.setInt8
 */
function DataViewPrototypeSetInt8(this: Value, byteOffset: Value, value: Value): void {
  SetViewValue(this, byteOffset, false, 0, value);
}

/**
 * DataView.prototype.setUint8 ( byteOffset, value ).
 * @builtin DataView.prototype.setUint8
 */
function DataViewPrototypeSetUint8(this: Value, byteOffset: Value, value: Value): void {
  SetViewValue(this, byteOffset, false, 1, value);
}

/**
 * DataView.prototype.setInt16 ( byteOffset, value [ , littleEndian ] ).
 * @builtin DataView.prototype.setInt16
 */
function DataViewPrototypeSetInt16(this: Value, byteOffset: Value, value: Value, littleEndian?: Value): void {
  SetViewValue(this, byteOffset, littleEndian, 3, value);
}

/**
 * DataView.prototype.setUint16 ( byteOffset, value [ , littleEndian ] ).
 * @builtin DataView.prototype.setUint16
 */
function DataViewPrototypeSetUint16(this: Value, byteOffset: Value, value: Value, littleEndian?: Value): void {
  SetViewValue(this, byteOffset, littleEndian, 4, value);
}

/**
 * DataView.prototype.setInt32 ( byteOffset, value [ , littleEndian ] ).
 * @builtin DataView.prototype.setInt32
 */
function DataViewPrototypeSetInt32(this: Value, byteOffset: Value, value: Value, littleEndian?: Value): void {
  SetViewValue(this, byteOffset, littleEndian, 5, value);
}

/**
 * DataView.prototype.setUint32 ( byteOffset, value [ , littleEndian ] ).
 * @builtin DataView.prototype.setUint32
 */
function DataViewPrototypeSetUint32(this: Value, byteOffset: Value, value: Value, littleEndian?: Value): void {
  SetViewValue(this, byteOffset, littleEndian, 6, value);
}

/**
 * DataView.prototype.setBigInt64 ( byteOffset, value [ , littleEndian ] ).
 * @builtin DataView.prototype.setBigInt64
 */
function DataViewPrototypeSetBigInt64(this: Value, byteOffset: Value, value: Value, littleEndian?: Value): void {
  SetViewValue(this, byteOffset, littleEndian, 10, value);
}

/**
 * DataView.prototype.setBigUint64 ( byteOffset, value [ , littleEndian ] ).
 * @builtin DataView.prototype.setBigUint64
 */
function DataViewPrototypeSetBigUint64(this: Value, byteOffset: Value, value: Value, littleEndian?: Value): void {
  SetViewValue(this, byteOffset, littleEndian, 11, value);
}

/**
 * DataView.prototype.setFloat16 ( byteOffset, value [ , littleEndian ] ).
 * @builtin DataView.prototype.setFloat16
 */
function DataViewPrototypeSetFloat16(this: Value, byteOffset: Value, value: Value, littleEndian?: Value): void {
  SetViewValue(this, byteOffset, littleEndian, 7, value);
}

/**
 * DataView.prototype.setFloat32 ( byteOffset, value [ , littleEndian ] ).
 * @builtin DataView.prototype.setFloat32
 */
function DataViewPrototypeSetFloat32(this: Value, byteOffset: Value, value: Value, littleEndian?: Value): void {
  SetViewValue(this, byteOffset, littleEndian, 8, value);
}

/**
 * DataView.prototype.setFloat64 ( byteOffset, value [ , littleEndian ] ).
 * @builtin DataView.prototype.setFloat64
 */
function DataViewPrototypeSetFloat64(this: Value, byteOffset: Value, value: Value, littleEndian?: Value): void {
  SetViewValue(this, byteOffset, littleEndian, 9, value);
}
