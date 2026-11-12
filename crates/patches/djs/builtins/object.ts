// Object (ECMA-262 20.1) and property descriptors (6.2.6).

/**
 * Object ( [ value ] ), 20.1.1.1.
 * @constructor Object
 * @length 1
 */
function ObjectConstructor(value?: Value): Obj {
  // A subclass constructor (`super(value)`) ignores the value.
  if (new.target !== undefined && new.target !== RealmConstructor("Object")) {
    return ObjectCreate(GetPrototypeFromConstructor(new.target, "ObjectPrototype"));
  }
  if (value === undefined || value === null) {
    return ObjectCreate(GetPrototypeFromConstructor(new.target, "ObjectPrototype"));
  }
  return ToObject(value);
}

/**
 * ToPropertyDescriptor ( Obj ), 6.2.6.5. The result is a descriptor record: an object
 * without a prototype whose own data properties are the present fields.
 */
function ToPropertyDescriptor(attributes: Value): Obj {
  if (!IsObject(attributes)) {
    throw NewTypeError("not an object");
  }
  const o: Obj = AsObj(attributes);
  const desc: Obj = ObjectCreate(null);
  if (HasProperty(o, "enumerable")) {
    CreateDataPropertyOrThrow(desc, "enumerable", ToBoolean(Get(o, "enumerable")));
  }
  if (HasProperty(o, "configurable")) {
    CreateDataPropertyOrThrow(desc, "configurable", ToBoolean(Get(o, "configurable")));
  }
  const hasValue: boolean = HasProperty(o, "value");
  if (hasValue) {
    CreateDataPropertyOrThrow(desc, "value", Get(o, "value"));
  }
  const hasWritable: boolean = HasProperty(o, "writable");
  if (hasWritable) {
    CreateDataPropertyOrThrow(desc, "writable", ToBoolean(Get(o, "writable")));
  }
  const hasGet: boolean = HasProperty(o, "get");
  if (hasGet) {
    const getter: Value = Get(o, "get");
    if (!IsCallable(getter) && getter !== undefined) {
      throw NewTypeError("invalid getter");
    }
    CreateDataPropertyOrThrow(desc, "get", getter);
  }
  const hasSet: boolean = HasProperty(o, "set");
  if (hasSet) {
    const setter: Value = Get(o, "set");
    if (!IsCallable(setter) && setter !== undefined) {
      throw NewTypeError("invalid setter");
    }
    CreateDataPropertyOrThrow(desc, "set", setter);
  }
  if ((hasGet || hasSet) && (hasValue || hasWritable)) {
    throw NewTypeError("cannot have setter/getter and value or writable");
  }
  return desc;
}

/**
 * [[DefineOwnProperty]] with a descriptor record. An array's `length` value goes
 * through ArraySetLength's conversions first (10.4.2.4).
 */
function DefineOwnProperty(o: Obj, key: Value, desc: Obj): boolean {
  // A typed array element converts its value with ToNumber or ToBigInt (user code)
  // after the descriptor checks (IntegerIndexedDefineOwnProperty, 10.4.5.3); the leaf
  // only takes element values.
  // (An immutable buffer's element compares the value unconverted, D1970.)
  if (IsTypedArray(o) && HasOwnProperty(desc, "value") && IsCanonicalNumericKey(key) && !IsImmutableBuffer(AsObj(TypedArrayField(o, 1)))) {
    const value: Value = Get(desc, "value");
    if (!IsElementValue(o, value)) {
      DeleteProperty(desc, "value");
      if (!DefineOwnPropertyFromRecord(o, key, desc)) {
        return false;
      }
      TASet(o, ToNumber(key), value);
      return true;
    }
  }
  // A proxy's defineProperty trap sees the descriptor unconverted.
  if (!IsProxy(o) && IsArray(o) && key === "length" && HasOwnProperty(desc, "value")) {
    const v: Value = Get(desc, "value");
    const newLen: f64 = ToNumber(v);
    const numberLen: f64 = ToNumber(v);
    if (!(newLen >= 0 && newLen <= 4294967295 && ToIntegerOrInfinityNumber(newLen) === newLen)) {
      throw NewRangeError("invalid array length");
    }
    if (newLen !== numberLen) {
      throw NewRangeError("invalid array length");
    }
    CreateDataPropertyOrThrow(desc, "value", newLen);
  }
  return DefineOwnPropertyFromRecord(o, key, desc);
}

/** DefinePropertyOrThrow ( O, P, desc ), 7.3.8, with a descriptor record. */
function DefinePropertyOrThrow(o: Obj, key: Value, desc: Obj): void {
  if (!DefineOwnProperty(o, key, desc)) {
    // Probing a proxy for the message would run its getOwnPropertyDescriptor trap.
    if (IsProxy(o)) {
      throw NewTypeError("proxy: defineProperty exception");
    }
    if (OwnPropertyAttributes(o, key) < 0) {
      throw NewTypeError("object is not extensible");
    }
    throw NewTypeError("property is not configurable");
  }
}

/**
 * Object.defineProperty ( O, P, Attributes ), 20.1.2.4.
 * @builtin Object.defineProperty
 */
function ObjectDefineProperty(o: Value, p: Value, attributes: Value): Value {
  if (!IsObject(o)) {
    throw NewTypeError("not an object");
  }
  const key: Value = ToPropertyKey(p);
  const desc: Obj = ToPropertyDescriptor(attributes);
  DefinePropertyOrThrow(AsObj(o), key, desc);
  return o;
}

/**
 * Object.getOwnPropertyDescriptor ( O, P ), 20.1.2.8.
 * @builtin Object.getOwnPropertyDescriptor
 */
function ObjectGetOwnPropertyDescriptor(o: Value, p: Value): Value {
  const obj: Obj = ToObject(o);
  const key: Value = ToPropertyKey(p);
  return GetOwnPropertyDescriptorObject(obj, key);
}

/**
 * Object.getOwnPropertyNames ( O ), 20.1.2.10.
 * @builtin Object.getOwnPropertyNames
 */
function ObjectGetOwnPropertyNames(o: Value): Obj {
  return OwnPropertyKeys(ToObject(o), 0);
}

/**
 * Object.prototype.hasOwnProperty ( V ), 20.1.3.2.
 * @builtin Object.prototype.hasOwnProperty
 */
function ObjectPrototypeHasOwnProperty(this: Value, v: Value): boolean {
  const key: Value = ToPropertyKey(v);
  return HasOwnProperty(ToObject(this), key);
}

/**
 * Object.prototype.propertyIsEnumerable ( V ), 20.1.3.4.
 * @builtin Object.prototype.propertyIsEnumerable
 */
function ObjectPrototypePropertyIsEnumerable(this: Value, v: Value): boolean {
  const key: Value = ToPropertyKey(v);
  const attrs: i32 = OwnPropertyAttributes(ToObject(this), key);
  return attrs >= 0 && (attrs & 2) !== 0;
}

/**
 * Object.getPrototypeOf ( O ), 20.1.2.12.
 * @builtin Object.getPrototypeOf
 */
function ObjectGetPrototypeOf(o: Value): Value {
  if (o === undefined || o === null) {
    throw NewTypeError("not an object");
  }
  return GetPrototypeOf(ToObject(o));
}

/**
 * Object.setPrototypeOf ( O, proto ), 20.1.2.23.
 * @builtin Object.setPrototypeOf
 */
function ObjectSetPrototypeOf(o: Value, proto: Value): Value {
  if (o === undefined || o === null) {
    throw NewTypeError("not an object");
  }
  if (!IsObject(proto) && proto !== null) {
    throw NewTypeError("not an object");
  }
  if (!IsObject(o)) {
    return o;
  }
  SetPrototypeOrThrow(AsObj(o), proto);
  return o;
}

/** O.[[SetPrototypeOf]](proto), a TypeError with qjs's messages when refused. */
function SetPrototypeOrThrow(o: Obj, proto: Value): void {
  if (!SetPrototypeOf(o, proto)) {
    if (o === RealmIntrinsic("ObjectPrototype")) {
      throw NewTypeError("prototype is immutable");
    }
    if (!IsProxy(o) && !IsExtensible(o)) {
      throw NewTypeError("object is not extensible");
    }
    throw NewTypeError(IsProxy(o) ? "proxy: bad prototype" : "circular prototype chain");
  }
}

/**
 * get Object.prototype.__proto__, B.2.2.1.1.
 * @getter Object.prototype.__proto__
 */
function ObjectPrototypeGetProto(this: Value): Value {
  return GetPrototypeOf(ToObject(this));
}

/**
 * set Object.prototype.__proto__, B.2.2.1.2.
 * @setter Object.prototype.__proto__
 */
function ObjectPrototypeSetProto(this: Value, proto: Value): Value {
  if (this === undefined || this === null) {
    throw NewTypeError("not an object");
  }
  if ((!IsObject(proto) && proto !== null) || !IsObject(this)) {
    return undefined;
  }
  SetPrototypeOrThrow(AsObj(this), proto);
  return undefined;
}

/**
 * Object.create ( O, Properties ), 20.1.2.2.
 * @builtin Object.create
 */
function ObjectCreateBuiltin(o: Value, properties: Value): Obj {
  if (!IsObject(o) && o !== null) {
    throw NewTypeError("not a prototype");
  }
  const obj: Obj = ObjectCreate(o);
  if (properties !== undefined) {
    ObjectDefinePropertiesHelper(obj, properties);
  }
  return obj;
}

/** ObjectDefineProperties ( O, Properties ), 20.1.2.3.1. */
function ObjectDefinePropertiesHelper(o: Obj, properties: Value): Obj {
  const props: Obj = ToObject(properties);
  const keys: Obj = OwnPropertyKeys(props, 2);
  const n: f64 = LengthOfArrayLike(keys);
  // The descriptors are all read (and checked) before any is defined.
  const descKeys: Obj = ArrayCreate(0);
  const descs: Obj = ArrayCreate(0);
  let count: f64 = 0;
  for (let i: f64 = 0; i < n; i++) {
    const key: Value = Get(keys, i);
    const attrs: i32 = OwnPropertyAttributes(props, key);
    if (attrs >= 0 && (attrs & 2) !== 0) {
      const descObj: Value = Get(props, key);
      CreateDataPropertyOrThrow(descKeys, count, key);
      CreateDataPropertyOrThrow(descs, count, ToPropertyDescriptor(descObj));
      count++;
    }
  }
  for (let i: f64 = 0; i < count; i++) {
    DefinePropertyOrThrow(o, Get(descKeys, i), AsObj(Get(descs, i)));
  }
  return o;
}

/**
 * Object.defineProperties ( O, Properties ), 20.1.2.3.
 * @builtin Object.defineProperties
 */
function ObjectDefineProperties(o: Value, properties: Value): Obj {
  if (!IsObject(o)) {
    throw NewTypeError("not an object");
  }
  return ObjectDefinePropertiesHelper(AsObj(o), properties);
}

/**
 * EnumerableOwnProperties ( O, kind ), 7.3.23: kind 0 keys, 1 values, 2 entries. The
 * enumerable check happens per key, after the getters of earlier keys ran.
 */
function EnumerableOwnProperties(o: Obj, kind: i32): Obj {
  if (kind === 0) {
    return OwnPropertyKeys(o, 3);
  }
  const plain: Value = EnumerableOwnDataProperties(o, kind);
  if (plain !== undefined) {
    return AsObj(plain);
  }
  const keys: Obj = OwnPropertyKeys(o, 0);
  const n: f64 = LengthOfArrayLike(keys);
  const result: Obj = ArrayCreate(0);
  let count: f64 = 0;
  for (let i: f64 = 0; i < n; i++) {
    const key: Value = Get(keys, i);
    const attrs: i32 = OwnPropertyAttributes(o, key);
    if (attrs >= 0 && (attrs & 2) !== 0) {
      const value: Value = Get(o, key);
      if (kind === 1) {
        CreateDataPropertyOrThrow(result, count, value);
      } else {
        const entry: Obj = ArrayCreate(0);
        CreateDataPropertyOrThrow(entry, 0, key);
        CreateDataPropertyOrThrow(entry, 1, value);
        CreateDataPropertyOrThrow(result, count, entry);
      }
      count++;
    }
  }
  return result;
}

/**
 * Object.keys ( O ), 20.1.2.18.
 * @builtin Object.keys
 */
function ObjectKeys(o: Value): Obj {
  return EnumerableOwnProperties(ToObject(o), 0);
}

/**
 * Object.values ( O ), 20.1.2.24.
 * @builtin Object.values
 */
function ObjectValues(o: Value): Obj {
  return EnumerableOwnProperties(ToObject(o), 1);
}

/**
 * Object.entries ( O ), 20.1.2.5.
 * @builtin Object.entries
 */
function ObjectEntries(o: Value): Obj {
  return EnumerableOwnProperties(ToObject(o), 2);
}

/**
 * Object.assign ( target, ...sources ), 20.1.2.1.
 * @builtin Object.assign
 * @length 2
 * @fast
 */
function ObjectAssign(target: Value, ...sources: Value[]): Obj {
  const to: Obj = ToObject(target);
  const n: f64 = LengthOfArrayLike(sources);
  for (let s: f64 = 0; s < n; s++) {
    const nextSource: Value = Get(sources, s);
    if (nextSource !== undefined && nextSource !== null) {
      const from: Obj = ToObject(nextSource);
      const keys: Obj = OwnPropertyKeys(from, 2);
      const count: f64 = LengthOfArrayLike(keys);
      for (let i: f64 = 0; i < count; i++) {
        const key: Value = Get(keys, i);
        const attrs: i32 = OwnPropertyAttributes(from, key);
        if (attrs >= 0 && (attrs & 2) !== 0) {
          Set(to, key, Get(from, key));
        }
      }
    }
  }
  return to;
}

/**
 * Object.freeze ( O ), 20.1.2.6.
 * @builtin Object.freeze
 */
function ObjectFreeze(o: Value): Value {
  if (!IsObject(o)) {
    return o;
  }
  if (!SetIntegrityLevel(AsObj(o), true)) {
    throw NewTypeError("cannot freeze");
  }
  return o;
}

/**
 * Object.seal ( O ), 20.1.2.22.
 * @builtin Object.seal
 */
function ObjectSeal(o: Value): Value {
  if (!IsObject(o)) {
    return o;
  }
  if (!SetIntegrityLevel(AsObj(o), false)) {
    throw NewTypeError("cannot seal");
  }
  return o;
}

/**
 * Object.preventExtensions ( O ), 20.1.2.20.
 * @builtin Object.preventExtensions
 */
function ObjectPreventExtensions(o: Value): Value {
  if (!IsObject(o)) {
    return o;
  }
  if (!PreventExtensions(AsObj(o))) {
    throw NewTypeError("cannot prevent extensions");
  }
  return o;
}

/**
 * Object.isFrozen ( O ), 20.1.2.15.
 * @builtin Object.isFrozen
 */
function ObjectIsFrozen(o: Value): boolean {
  return !IsObject(o) || TestIntegrityLevel(AsObj(o), true);
}

/**
 * Object.isSealed ( O ), 20.1.2.16.
 * @builtin Object.isSealed
 */
function ObjectIsSealed(o: Value): boolean {
  return !IsObject(o) || TestIntegrityLevel(AsObj(o), false);
}

/**
 * Object.isExtensible ( O ), 20.1.2.14.
 * @builtin Object.isExtensible
 */
function ObjectIsExtensible(o: Value): boolean {
  return IsObject(o) && IsExtensible(AsObj(o));
}

/**
 * Object.getOwnPropertySymbols ( O ), 20.1.2.11.
 * @builtin Object.getOwnPropertySymbols
 */
function ObjectGetOwnPropertySymbols(o: Value): Obj {
  return OwnPropertyKeys(ToObject(o), 1);
}

/**
 * Object.getOwnPropertyDescriptors ( O ), 20.1.2.9.
 * @builtin Object.getOwnPropertyDescriptors
 */
function ObjectGetOwnPropertyDescriptors(o: Value): Obj {
  const obj: Obj = ToObject(o);
  const keys: Obj = OwnPropertyKeys(obj, 2);
  const n: f64 = LengthOfArrayLike(keys);
  const descriptors: Obj = ObjectCreate(RealmIntrinsic("ObjectPrototype"));
  for (let i: f64 = 0; i < n; i++) {
    const key: Value = Get(keys, i);
    const desc: Value = GetOwnPropertyDescriptorObject(obj, key);
    if (desc !== undefined) {
      CreateDataPropertyOrThrow(descriptors, key, desc);
    }
  }
  return descriptors;
}

/**
 * Object.hasOwn ( O, P ), 20.1.2.13.
 * @builtin Object.hasOwn
 */
function ObjectHasOwn(o: Value, p: Value): boolean {
  const obj: Obj = ToObject(o);
  return HasOwnProperty(obj, ToPropertyKey(p));
}

/**
 * Object.is ( value1, value2 ), 20.1.2.14.
 * @builtin Object.is
 */
function ObjectIs(value1: Value, value2: Value): boolean {
  return SameValue(value1, value2);
}

/**
 * Object.prototype.toString ( ), 20.1.3.6.
 * @builtin Object.prototype.toString
 */
function ObjectPrototypeToString(this: Value): Str {
  return ObjectToString(this);
}

/** The steps of Object.prototype.toString, for builtins that call it. */
function ObjectToString(v: Value): Str {
  if (v === undefined) {
    return "[object Undefined]";
  }
  if (v === null) {
    return "[object Null]";
  }
  const o: Obj = ToObject(v);
  // The builtinTag first: IsArray throws for a revoked proxy before the Get.
  const builtinTag: Str = ObjectClassTag(o);
  const tag: Value = Get(o, WellKnownSymbol("toStringTag"));
  if (IsString(tag)) {
    return "[object " + AsStr(tag) + "]";
  }
  return "[object " + builtinTag + "]";
}

/**
 * Object.prototype.toLocaleString ( [ reserved1 [ , reserved2 ] ] ), 20.1.3.5.
 * @builtin Object.prototype.toLocaleString
 */
function ObjectPrototypeToLocaleString(this: Value): Value {
  return Call(GetV(this, "toString"), this);
}

/**
 * Object.prototype.valueOf ( ), 20.1.3.7.
 * @builtin Object.prototype.valueOf
 */
function ObjectPrototypeValueOf(this: Value): Obj {
  return ToObject(this);
}

/**
 * Object.prototype.isPrototypeOf ( V ), 20.1.3.3.
 * @builtin Object.prototype.isPrototypeOf
 */
function ObjectPrototypeIsPrototypeOf(this: Value, v: Value): boolean {
  if (!IsObject(v)) {
    return false;
  }
  const o: Obj = ToObject(this);
  let p: Value = GetPrototypeOf(AsObj(v));
  while (p !== null) {
    if (p === o) {
      return true;
    }
    p = GetPrototypeOf(AsObj(p));
  }
  return false;
}
