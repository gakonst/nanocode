// The Reflect object (ECMA-262 28.1). No Proxy yet: the operations are the ordinary
// (and exotic) internal methods.

/**
 * The Reflect object, 28.1.
 * @value globalThis.Reflect
 * @attributes writable configurable
 */
function ReflectObject(): Obj {
  return ObjectCreate(RealmIntrinsic("ObjectPrototype"));
}

/**
 * Reflect [ %Symbol.toStringTag% ], 28.1.14.
 * @value Reflect[Symbol.toStringTag]
 * @attributes configurable
 */
function ReflectToStringTag(): Str {
  return "Reflect";
}

/** A TypeError unless f is a constructor, with qjs's messages. */
function RequireConstructor(f: Value): void {
  if (!IsObject(f)) {
    throw NewTypeError("not a function");
  }
  if (!IsCallable(f)) {
    throw NewTypeError("not a constructor");
  }
  if (!IsConstructor(f)) {
    throw NewNotConstructorError(f);
  }
}

/** The target argument: TypeError unless it is an object (qjs's message). */
function ReflectTarget(target: Value): Obj {
  if (!IsObject(target)) {
    throw NewTypeError("not an object");
  }
  return AsObj(target);
}

/**
 * Reflect.apply ( target, thisArgument, argumentsList ), 28.1.1.
 * @builtin Reflect.apply
 */
function ReflectApply(target: Value, thisArgument: Value, argumentsList: Value): Value {
  if (!IsCallable(target)) {
    throw NewTypeError("not a function");
  }
  return CallWithArray(target, thisArgument, CreateListFromArrayLike(argumentsList));
}

/**
 * Reflect.construct ( target, argumentsList [ , newTarget ] ), 28.1.2.
 * @builtin Reflect.construct
 */
function ReflectConstruct(target: Value, argumentsList: Value, ...rest: Value[]): Obj {
  RequireConstructor(target);
  let newTarget: Value = target;
  if (LengthOfArrayLike(rest) > 0) {
    newTarget = Get(rest, 0);
    if (!IsConstructor(newTarget)) {
      if (IsCallable(newTarget)) {
        throw NewNotConstructorError(newTarget);
      }
      throw NewTypeError("not a constructor");
    }
  }
  return ConstructWithArray(target, CreateListFromArrayLike(argumentsList), newTarget);
}

/**
 * Reflect.defineProperty ( target, propertyKey, attributes ), 28.1.3.
 * @builtin Reflect.defineProperty
 */
function ReflectDefineProperty(target: Value, propertyKey: Value, attributes: Value): boolean {
  const o: Obj = ReflectTarget(target);
  const key: Value = ToPropertyKey(propertyKey);
  const desc: Obj = ToPropertyDescriptor(attributes);
  return DefineOwnProperty(o, key, desc);
}

/**
 * Reflect.deleteProperty ( target, propertyKey ), 28.1.4.
 * @builtin Reflect.deleteProperty
 */
function ReflectDeleteProperty(target: Value, propertyKey: Value): boolean {
  const o: Obj = ReflectTarget(target);
  return DeleteProperty(o, ToPropertyKey(propertyKey));
}

/**
 * Reflect.get ( target, propertyKey [ , receiver ] ), 28.1.5.
 * @builtin Reflect.get
 */
function ReflectGet(target: Value, propertyKey: Value, ...rest: Value[]): Value {
  const o: Obj = ReflectTarget(target);
  const key: Value = ToPropertyKey(propertyKey);
  const receiver: Value = LengthOfArrayLike(rest) > 0 ? Get(rest, 0) : o;
  return GetWithReceiver(o, key, receiver);
}

/**
 * Reflect.getOwnPropertyDescriptor ( target, propertyKey ), 28.1.6.
 * @builtin Reflect.getOwnPropertyDescriptor
 */
function ReflectGetOwnPropertyDescriptor(target: Value, propertyKey: Value): Value {
  const o: Obj = ReflectTarget(target);
  return GetOwnPropertyDescriptorObject(o, ToPropertyKey(propertyKey));
}

/**
 * Reflect.getPrototypeOf ( target ), 28.1.7.
 * @builtin Reflect.getPrototypeOf
 */
function ReflectGetPrototypeOf(target: Value): Value {
  return GetPrototypeOf(ReflectTarget(target));
}

/**
 * Reflect.has ( target, propertyKey ), 28.1.8.
 * @builtin Reflect.has
 */
function ReflectHas(target: Value, propertyKey: Value): boolean {
  const o: Obj = ReflectTarget(target);
  return HasProperty(o, ToPropertyKey(propertyKey));
}

/**
 * Reflect.isExtensible ( target ), 28.1.9.
 * @builtin Reflect.isExtensible
 */
function ReflectIsExtensible(target: Value): boolean {
  return IsExtensible(ReflectTarget(target));
}

/**
 * Reflect.ownKeys ( target ), 28.1.10.
 * @builtin Reflect.ownKeys
 */
function ReflectOwnKeys(target: Value): Obj {
  return OwnPropertyKeys(ReflectTarget(target), 2);
}

/**
 * Reflect.preventExtensions ( target ), 28.1.11.
 * @builtin Reflect.preventExtensions
 */
function ReflectPreventExtensions(target: Value): boolean {
  return PreventExtensions(ReflectTarget(target));
}

/**
 * Reflect.set ( target, propertyKey, V [ , receiver ] ), 28.1.12.
 * @builtin Reflect.set
 */
function ReflectSet(target: Value, propertyKey: Value, v: Value, ...rest: Value[]): boolean {
  const o: Obj = ReflectTarget(target);
  const key: Value = ToPropertyKey(propertyKey);
  const receiver: Value = LengthOfArrayLike(rest) > 0 ? Get(rest, 0) : o;
  // A typed array's own element: TypedArraySetElement converts the value first (10.4.5.5),
  // which may run user code; the leaf only writes element values.
  // (Not for an immutable buffer, whose [[Set]] fails before converting, D1970.)
  if (IsTypedArray(receiver) && !IsElementValue(AsObj(receiver), v) && IsCanonicalNumericKey(key) && !IsImmutableBuffer(AsObj(TypedArrayField(receiver, 1))) && ReachesReceiver(o, key, AsObj(receiver))) {
    return SetWithReceiver(o, key, ToElementValue(AsObj(receiver), v), receiver);
  }
  return SetWithReceiver(o, key, v, receiver);
}

/**
 * Whether [[Set]] of the numeric `key` from o ends in a conversion of the value for the
 * typed array `receiver`: its own [[Set]] (TypedArraySetElement) or its
 * [[DefineOwnProperty]] of an existing element after OrdinarySet found a writable data
 * property (10.4.5.5, 10.1.9.2). Setters and failures see the value unconverted.
 */
function ReachesReceiver(o: Obj, key: Value, receiver: Obj): boolean {
  let cur: Value = o;
  while (cur !== null) {
    const c: Obj = AsObj(cur);
    if (c === receiver) {
      return true;
    }
    const attrs: i32 = OwnPropertyAttributes(c, key);
    if (IsTypedArray(c) && attrs < 0) {
      return false;
    }
    if (attrs >= 0) {
      if ((attrs & 8) !== 0 || (attrs & 1) === 0) {
        return false;
      }
      break;
    }
    cur = GetPrototypeOf(c);
  }
  return OwnPropertyAttributes(receiver, key) >= 0;
}

/**
 * Reflect.setPrototypeOf ( target, proto ), 28.1.13.
 * @builtin Reflect.setPrototypeOf
 */
function ReflectSetPrototypeOf(target: Value, proto: Value): boolean {
  const o: Obj = ReflectTarget(target);
  if (!IsObject(proto) && proto !== null) {
    throw NewTypeError("not an object");
  }
  return SetPrototypeOf(o, proto);
}
