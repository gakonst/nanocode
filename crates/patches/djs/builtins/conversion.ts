// Type conversion abstract operations (ECMA-262 7.1) and friends.

/**
 * ToPrimitive ( input [ , preferredType ] ), 7.1.1. `hint` is "default", "string" or
 * "number". The interpreter calls it for objects in `+`, comparisons and so on.
 * @intrinsic
 */
function ToPrimitive(input: Value, hint: Str): Value {
  if (!IsObject(input)) {
    return input;
  }
  const o: Obj = AsObj(input);
  const exoticToPrim: Value = GetMethod(o, WellKnownSymbol("toPrimitive"));
  if (exoticToPrim !== undefined) {
    const result: Value = Call(exoticToPrim, o, hint);
    if (IsObject(result)) {
      throw NewTypeError("toPrimitive");
    }
    return result;
  }
  return OrdinaryToPrimitive(o, hint === "string" ? "string" : "number");
}

/** OrdinaryToPrimitive ( O, hint ), 7.1.1.1. */
function OrdinaryToPrimitive(o: Obj, hint: Str): Value {
  let name: Str = hint === "string" ? "toString" : "valueOf";
  for (let i: i32 = 0; i < 2; i++) {
    const method: Value = Get(o, name);
    if (IsCallable(method)) {
      const result: Value = Call(method, o);
      if (!IsObject(result)) {
        return result;
      }
    }
    name = name === "toString" ? "valueOf" : "toString";
  }
  throw NewTypeError("toPrimitive");
}

/** GetMethod ( V, P ), 7.3.11. */
function GetMethod(v: Value, key: Value): Value {
  const func: Value = GetV(v, key);
  if (func === undefined || func === null) {
    return undefined;
  }
  if (!IsCallable(func)) {
    throw NewTypeError("not a function");
  }
  return func;
}

/**
 * ToNumber ( argument ), 7.1.4.
 * @intrinsic
 */
function ToNumber(v: Value): f64 {
  if (IsObject(v)) {
    return ToNumberPrimitive(ToPrimitive(v, "number"));
  }
  return ToNumberPrimitive(v);
}

/**
 * ToString ( argument ), 7.1.17.
 * @intrinsic
 * @fast
 */
function ToString(v: Value): Str {
  if (IsObject(v)) {
    return ToStringPrimitive(ToPrimitive(v, "string"));
  }
  return ToStringPrimitive(v);
}

/**
 * ToIntegerOrInfinity ( argument ), 7.1.5.
 */
function ToIntegerOrInfinity(v: Value): f64 {
  return ToIntegerOrInfinityNumber(ToNumber(v));
}

/**
 * ToLength ( argument ), 7.1.20.
 */
function ToLength(v: Value): f64 {
  const len: f64 = ToIntegerOrInfinity(v);
  if (len <= 0) {
    return 0;
  }
  if (len > 9007199254740991) {
    return 9007199254740991;
  }
  return len;
}

/**
 * LengthOfArrayLike ( obj ), 7.3.18.
 * @intrinsic
 * @fast
 */
function LengthOfArrayLike(o: Obj): f64 {
  return ToLength(Get(o, "length"));
}

/**
 * ToPropertyKey ( argument ), 7.1.19: a string, a symbol, or a number key.
 * @intrinsic
 * @fast
 */
function ToPropertyKey(v: Value): Value {
  const key: Value = ToPrimitive(v, "string");
  if (IsSymbol(key) || IsNumber(key)) {
    // Numbers are valid keys for the intrinsics (canonical numeric strings).
    return key;
  }
  return ToStringPrimitive(key);
}

/**
 * GetPrototypeFromConstructor ( constructor, intrinsicDefaultProto ), 10.1.14. The leaf
 * %ConstructorPrototype answers unless reading `prototype` runs user code (0236).
 */
function GetPrototypeFromConstructor(newTarget: Value, fallback: Str): Obj {
  const proto: Value = ConstructorPrototype(newTarget, fallback);
  if (proto !== undefined) {
    return AsObj(proto);
  }
  return GetPrototypeFromConstructorSlow(newTarget, fallback);
}

/**
 * GetPrototypeFromConstructor's steps. `newTarget` undefined means the builtin was
 * called as a function, so the active function is the constructor and its prototype
 * is the realm's.
 * @intrinsic
 */
function GetPrototypeFromConstructorSlow(newTarget: Value, fallback: Str): Obj {
  if (newTarget === undefined) {
    return RealmIntrinsic(fallback);
  }
  const proto: Value = Get(AsObj(newTarget), "prototype");
  if (IsObject(proto)) {
    return AsObj(proto);
  }
  // GetFunctionRealm ( obj ), 7.3.24: one realm, but a revoked proxy is a TypeError.
  let c: Obj = AsObj(newTarget);
  while (IsProxy(c)) {
    c = ProxyTarget(c);
  }
  return RealmIntrinsic(fallback);
}

/**
 * ToNumeric ( value ), 7.1.3: a Number, or a BigInt. Here and
 * not in bigint.ts: the interpreter's numeric operators need it in every build (D2411).
 * @intrinsic
 */
function ToNumeric(v: Value): Value {
  const prim: Value = ToPrimitive(v, "number");
  if (IsBigInt(prim)) {
    return prim;
  }
  return ToNumberPrimitive(prim);
}
