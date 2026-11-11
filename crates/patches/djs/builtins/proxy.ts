// Proxy (ECMA-262 28.2) and the internal methods of Proxy exotic objects (10.5), with
// every invariant check.
//
// A proxy is a heap object of class Proxy (heap/proxy.rs) holding its target, handler
// and whether it is callable or a constructor. The interpreter and the internal-method
// intrinsics (Get, OwnPropertyKeys, ...) do not run traps themselves: when they meet a
// proxy they call the `@intrinsic` functions here instead, with the same arguments
// (interp/run/proxy.rs). Keys arrive as the intrinsics take them (strings, symbols, or
// Numbers for index keys); the traps see strings and symbols only.
//
// Property descriptors are descriptor records (ToPropertyDescriptor in object.ts):
// objects whose own data properties are the present fields. GetOwnPropertyDescriptorObject
// returns a complete descriptor as a plain object, which reads the same way.

/**
 * Proxy ( target, handler ), 28.2.1.1.
 * @constructor Proxy
 */
function ProxyConstructor(target: Value, handler: Value): Obj {
  if (new.target === undefined) {
    throw NewTypeError("must be called with new");
  }
  return ProxyCreate(target, handler);
}

/** ProxyCreate ( target, handler ), 10.5.14. */
function ProxyCreate(target: Value, handler: Value): Obj {
  if (!IsObject(target) || !IsObject(handler)) {
    throw NewTypeError("not an object");
  }
  return NewProxy(AsObj(target), AsObj(handler));
}

/**
 * Proxy.revocable ( target, handler ), 28.2.2.1.
 * @builtin Proxy.revocable
 */
function ProxyRevocable(target: Value, handler: Value): Obj {
  const p: Obj = ProxyCreate(target, handler);
  const revoker: Fn = NewBuiltinClosure(15, p, 0);
  const result: Obj = ObjectCreate(RealmIntrinsic("ObjectPrototype"));
  CreateDataPropertyOrThrow(result, "proxy", p);
  CreateDataPropertyOrThrow(result, "revoke", revoker);
  return result;
}

/**
 * A Proxy revocation function, 28.2.2.1.1: `this` is the proxy (its data). Revoking
 * twice does nothing.
 * @intrinsic
 */
function ProxyRevokeFunction(this: Value): Value {
  ProxyRevoke(AsObj(this));
  return undefined;
}

// ---- Helpers ---------------------------------------------------------------------------

/** The key as a trap sees it: index keys (Numbers here) become strings. */
function TrapKey(key: Value): Value {
  if (IsNumber(key)) {
    return ToStringPrimitive(key);
  }
  return key;
}

/** The trap `name` of the proxy's handler, or undefined (GetMethod, 7.3.11). */
function ProxyTrap(handler: Obj, name: Str): Value {
  return GetMethod(handler, name);
}

/** A boolean field of a descriptor (absent: false). */
function DescFlag(desc: Obj, field: Str): boolean {
  return HasOwnProperty(desc, field) && ToBoolean(Get(desc, field));
}

/** IsAccessorDescriptor ( Desc ), 6.2.6.1. */
function IsAccessorDescriptor(desc: Obj): boolean {
  return HasOwnProperty(desc, "get") || HasOwnProperty(desc, "set");
}

/** IsDataDescriptor ( Desc ), 6.2.6.2. */
function IsDataDescriptor(desc: Obj): boolean {
  return HasOwnProperty(desc, "value") || HasOwnProperty(desc, "writable");
}

/** CompletePropertyDescriptor ( Desc ), 6.2.6.6, in place. */
function CompletePropertyDescriptor(desc: Obj): void {
  if (!IsAccessorDescriptor(desc)) {
    if (!HasOwnProperty(desc, "value")) {
      CreateDataPropertyOrThrow(desc, "value", undefined);
    }
    if (!HasOwnProperty(desc, "writable")) {
      CreateDataPropertyOrThrow(desc, "writable", false);
    }
  } else {
    if (!HasOwnProperty(desc, "get")) {
      CreateDataPropertyOrThrow(desc, "get", undefined);
    }
    if (!HasOwnProperty(desc, "set")) {
      CreateDataPropertyOrThrow(desc, "set", undefined);
    }
  }
  if (!HasOwnProperty(desc, "enumerable")) {
    CreateDataPropertyOrThrow(desc, "enumerable", false);
  }
  if (!HasOwnProperty(desc, "configurable")) {
    CreateDataPropertyOrThrow(desc, "configurable", false);
  }
}

/**
 * IsCompatiblePropertyDescriptor ( Extensible, Desc, Current ), 10.1.6.2: whether
 * ValidateAndApplyPropertyDescriptor would accept Desc over the complete Current
 * (undefined if absent).
 */
function IsCompatiblePropertyDescriptor(extensible: boolean, desc: Obj, current: Value): boolean {
  if (current === undefined) {
    return extensible;
  }
  const cur: Obj = AsObj(current);
  if (DescFlag(cur, "configurable")) {
    return true;
  }
  if (DescFlag(desc, "configurable")) {
    return false;
  }
  if (HasOwnProperty(desc, "enumerable") && DescFlag(desc, "enumerable") !== DescFlag(cur, "enumerable")) {
    return false;
  }
  const generic: boolean = !IsAccessorDescriptor(desc) && !IsDataDescriptor(desc);
  if (!generic && IsAccessorDescriptor(desc) !== IsAccessorDescriptor(cur)) {
    return false;
  }
  if (IsAccessorDescriptor(cur)) {
    if (HasOwnProperty(desc, "get") && !SameValue(Get(desc, "get"), Get(cur, "get"))) {
      return false;
    }
    if (HasOwnProperty(desc, "set") && !SameValue(Get(desc, "set"), Get(cur, "set"))) {
      return false;
    }
  } else if (!DescFlag(cur, "writable")) {
    if (DescFlag(desc, "writable")) {
      return false;
    }
    if (HasOwnProperty(desc, "value") && !SameValue(Get(desc, "value"), Get(cur, "value"))) {
      return false;
    }
  }
  return true;
}

/** FromPropertyDescriptor ( Desc ), 6.2.6.4: a new ordinary object. */
function FromPropertyDescriptor(desc: Obj): Obj {
  const o: Obj = ObjectCreate(RealmIntrinsic("ObjectPrototype"));
  if (HasOwnProperty(desc, "value")) {
    CreateDataPropertyOrThrow(o, "value", Get(desc, "value"));
  }
  if (HasOwnProperty(desc, "writable")) {
    CreateDataPropertyOrThrow(o, "writable", Get(desc, "writable"));
  }
  if (HasOwnProperty(desc, "get")) {
    CreateDataPropertyOrThrow(o, "get", Get(desc, "get"));
  }
  if (HasOwnProperty(desc, "set")) {
    CreateDataPropertyOrThrow(o, "set", Get(desc, "set"));
  }
  if (HasOwnProperty(desc, "enumerable")) {
    CreateDataPropertyOrThrow(o, "enumerable", Get(desc, "enumerable"));
  }
  if (HasOwnProperty(desc, "configurable")) {
    CreateDataPropertyOrThrow(o, "configurable", Get(desc, "configurable"));
  }
  return o;
}

/** The OwnPropertyAttributes encoding of a descriptor (-1 for undefined). */
function DescriptorAttributes(desc: Value): i32 {
  if (desc === undefined) {
    return -1;
  }
  const d: Obj = AsObj(desc);
  let attrs: i32 = 0;
  if (DescFlag(d, "writable")) {
    attrs = attrs | 1;
  }
  if (DescFlag(d, "enumerable")) {
    attrs = attrs | 2;
  }
  if (DescFlag(d, "configurable")) {
    attrs = attrs | 4;
  }
  if (IsAccessorDescriptor(d)) {
    attrs = attrs | 8;
  }
  return attrs;
}

/** Whether the dense array `list` has an element SameValue to v (from `start`). */
function ListIncludes(list: Obj, v: Value): boolean {
  const n: f64 = LengthOfArrayLike(list);
  for (let i: f64 = 0; i < n; i++) {
    if (SameValue(Get(list, i), v)) {
      return true;
    }
  }
  return false;
}

// ---- The internal methods (10.5.1 - 10.5.13) ---------------------------------------------

/**
 * [[GetPrototypeOf]] ( ), 10.5.1.
 * @intrinsic
 */
function ProxyGetPrototypeOf(p: Obj): Value {
  const handler: Obj = ProxyHandler(p);
  const target: Obj = ProxyTarget(p);
  const trap: Value = ProxyTrap(handler, "getPrototypeOf");
  if (trap === undefined) {
    return GetPrototypeOf(target);
  }
  const handlerProto: Value = Call(trap, handler, target);
  if (!IsObject(handlerProto) && handlerProto !== null) {
    throw NewTypeError("proxy: inconsistent prototype");
  }
  if (IsExtensible(target)) {
    return handlerProto;
  }
  if (!SameValue(handlerProto, GetPrototypeOf(target))) {
    throw NewTypeError("proxy: inconsistent prototype");
  }
  return handlerProto;
}

/**
 * [[SetPrototypeOf]] ( V ), 10.5.2.
 * @intrinsic
 */
function ProxySetPrototypeOf(p: Obj, v: Value): boolean {
  const handler: Obj = ProxyHandler(p);
  const target: Obj = ProxyTarget(p);
  const trap: Value = ProxyTrap(handler, "setPrototypeOf");
  if (trap === undefined) {
    return SetPrototypeOf(target, v);
  }
  if (!ToBoolean(Call(trap, handler, target, v))) {
    return false;
  }
  if (IsExtensible(target)) {
    return true;
  }
  if (!SameValue(v, GetPrototypeOf(target))) {
    throw NewTypeError("proxy: inconsistent prototype");
  }
  return true;
}

/**
 * [[IsExtensible]] ( ), 10.5.3.
 * @intrinsic
 */
function ProxyIsExtensible(p: Obj): boolean {
  const handler: Obj = ProxyHandler(p);
  const target: Obj = ProxyTarget(p);
  const trap: Value = ProxyTrap(handler, "isExtensible");
  if (trap === undefined) {
    return IsExtensible(target);
  }
  const result: boolean = ToBoolean(Call(trap, handler, target));
  if (result !== IsExtensible(target)) {
    throw NewTypeError("proxy: inconsistent isExtensible");
  }
  return result;
}

/**
 * [[PreventExtensions]] ( ), 10.5.4.
 * @intrinsic
 */
function ProxyPreventExtensions(p: Obj): boolean {
  const handler: Obj = ProxyHandler(p);
  const target: Obj = ProxyTarget(p);
  const trap: Value = ProxyTrap(handler, "preventExtensions");
  if (trap === undefined) {
    return PreventExtensions(target);
  }
  const result: boolean = ToBoolean(Call(trap, handler, target));
  if (result && IsExtensible(target)) {
    throw NewTypeError("proxy: inconsistent preventExtensions");
  }
  return result;
}

/**
 * [[GetOwnProperty]] ( P ), 10.5.5: a complete descriptor record, or undefined.
 */
function ProxyGetOwnProperty(p: Obj, key: Value): Value {
  const handler: Obj = ProxyHandler(p);
  const target: Obj = ProxyTarget(p);
  const trap: Value = ProxyTrap(handler, "getOwnPropertyDescriptor");
  if (trap === undefined) {
    return GetOwnPropertyDescriptorObject(target, key);
  }
  const trapResultObj: Value = Call(trap, handler, target, TrapKey(key));
  if (!IsObject(trapResultObj) && trapResultObj !== undefined) {
    throw NewTypeError("proxy: inconsistent getOwnPropertyDescriptor");
  }
  const targetDesc: Value = GetOwnPropertyDescriptorObject(target, key);
  if (trapResultObj === undefined) {
    if (targetDesc === undefined) {
      return undefined;
    }
    if (!DescFlag(AsObj(targetDesc), "configurable")) {
      throw NewTypeError("proxy: inconsistent getOwnPropertyDescriptor");
    }
    if (!IsExtensible(target)) {
      throw NewTypeError("proxy: inconsistent getOwnPropertyDescriptor");
    }
    return undefined;
  }
  const extensibleTarget: boolean = IsExtensible(target);
  const resultDesc: Obj = ToPropertyDescriptor(trapResultObj);
  CompletePropertyDescriptor(resultDesc);
  if (!IsCompatiblePropertyDescriptor(extensibleTarget, resultDesc, targetDesc)) {
    throw NewTypeError("proxy: inconsistent getOwnPropertyDescriptor");
  }
  if (!DescFlag(resultDesc, "configurable")) {
    if (targetDesc === undefined || DescFlag(AsObj(targetDesc), "configurable")) {
      throw NewTypeError("proxy: inconsistent getOwnPropertyDescriptor");
    }
    if (HasOwnProperty(resultDesc, "writable") && !DescFlag(resultDesc, "writable")) {
      if (DescFlag(AsObj(targetDesc), "writable")) {
        throw NewTypeError("proxy: inconsistent getOwnPropertyDescriptor");
      }
    }
  }
  return resultDesc;
}

/**
 * GetOwnPropertyDescriptorObject on a proxy: FromPropertyDescriptor of
 * [[GetOwnProperty]].
 * @intrinsic
 */
function ProxyGetOwnPropertyDescriptorObject(p: Obj, key: Value): Value {
  const desc: Value = ProxyGetOwnProperty(p, key);
  if (desc === undefined) {
    return undefined;
  }
  return FromPropertyDescriptor(AsObj(desc));
}

/**
 * HasOwnProperty on a proxy.
 * @intrinsic
 */
function ProxyHasOwnProperty(p: Obj, key: Value): boolean {
  return ProxyGetOwnProperty(p, key) !== undefined;
}

/**
 * OwnPropertyAttributes on a proxy.
 * @intrinsic
 */
function ProxyOwnPropertyAttributes(p: Obj, key: Value): i32 {
  return DescriptorAttributes(ProxyGetOwnProperty(p, key));
}

/**
 * [[DefineOwnProperty]] ( P, Desc ), 10.5.6, with a descriptor record.
 * @intrinsic
 */
function ProxyDefineOwnPropertyFromRecord(p: Obj, key: Value, desc: Obj): boolean {
  const handler: Obj = ProxyHandler(p);
  const target: Obj = ProxyTarget(p);
  const trap: Value = ProxyTrap(handler, "defineProperty");
  if (trap === undefined) {
    return DefineOwnProperty(target, key, desc);
  }
  const descObj: Obj = FromPropertyDescriptor(desc);
  if (!ToBoolean(Call(trap, handler, target, TrapKey(key), descObj))) {
    return false;
  }
  const targetDesc: Value = GetOwnPropertyDescriptorObject(target, key);
  const extensibleTarget: boolean = IsExtensible(target);
  const settingConfigFalse: boolean = HasOwnProperty(desc, "configurable") && !DescFlag(desc, "configurable");
  if (targetDesc === undefined) {
    if (!extensibleTarget || settingConfigFalse) {
      throw NewTypeError("proxy: inconsistent defineProperty");
    }
    return true;
  }
  const td: Obj = AsObj(targetDesc);
  if (!IsCompatiblePropertyDescriptor(extensibleTarget, desc, targetDesc)) {
    throw NewTypeError("proxy: inconsistent defineProperty");
  }
  if (settingConfigFalse && DescFlag(td, "configurable")) {
    throw NewTypeError("proxy: inconsistent defineProperty");
  }
  if (IsDataDescriptor(td) && !DescFlag(td, "configurable") && DescFlag(td, "writable")) {
    if (HasOwnProperty(desc, "writable") && !DescFlag(desc, "writable")) {
      throw NewTypeError("proxy: inconsistent defineProperty");
    }
  }
  return true;
}

/** The descriptor record of a data property with OwnPropertyAttributes-style attrs. */
function DataDescriptorRecord(v: Value, attrs: i32): Obj {
  const desc: Obj = ObjectCreate(null);
  CreateDataPropertyOrThrow(desc, "value", v);
  CreateDataPropertyOrThrow(desc, "writable", (attrs & 1) !== 0);
  CreateDataPropertyOrThrow(desc, "enumerable", (attrs & 2) !== 0);
  CreateDataPropertyOrThrow(desc, "configurable", (attrs & 4) !== 0);
  return desc;
}

/**
 * DefineDataPropertyOrThrow on a proxy.
 * @intrinsic
 */
function ProxyDefineDataPropertyOrThrow(p: Obj, key: Value, v: Value, attrs: i32): void {
  if (!ProxyDefineOwnPropertyFromRecord(p, key, DataDescriptorRecord(v, attrs))) {
    throw NewTypeError("proxy: defineProperty exception");
  }
}

/**
 * CreateDataProperty ( O, P, V ), 7.3.5, on a proxy.
 * @intrinsic
 */
function ProxyCreateDataProperty(p: Obj, key: Value, v: Value): boolean {
  return ProxyDefineOwnPropertyFromRecord(p, key, DataDescriptorRecord(v, 7));
}

/**
 * CreateDataPropertyOrThrow ( O, P, V ), 7.3.7, on a proxy.
 * @intrinsic
 */
function ProxyCreateDataPropertyOrThrow(p: Obj, key: Value, v: Value): void {
  if (!ProxyCreateDataProperty(p, key, v)) {
    throw NewTypeError("proxy: defineProperty exception");
  }
}

/**
 * [[HasProperty]] ( P ), 10.5.7.
 * @intrinsic
 */
function ProxyHas(p: Obj, key: Value): boolean {
  const handler: Obj = ProxyHandler(p);
  const target: Obj = ProxyTarget(p);
  const trap: Value = ProxyTrap(handler, "has");
  if (trap === undefined) {
    return HasProperty(target, key);
  }
  const result: boolean = ToBoolean(Call(trap, handler, target, TrapKey(key)));
  if (!result) {
    const targetDesc: Value = GetOwnPropertyDescriptorObject(target, key);
    if (targetDesc !== undefined) {
      if (!DescFlag(AsObj(targetDesc), "configurable")) {
        throw NewTypeError("proxy: inconsistent has");
      }
      if (!IsExtensible(target)) {
        throw NewTypeError("proxy: inconsistent has");
      }
    }
  }
  return result;
}

/**
 * [[Get]] ( P, Receiver ), 10.5.8.
 * @intrinsic
 */
function ProxyGet(p: Obj, key: Value, receiver: Value): Value {
  const handler: Obj = ProxyHandler(p);
  const target: Obj = ProxyTarget(p);
  const trap: Value = ProxyTrap(handler, "get");
  if (trap === undefined) {
    return GetWithReceiver(target, key, receiver);
  }
  const trapResult: Value = Call(trap, handler, target, TrapKey(key), receiver);
  const targetDesc: Value = GetOwnPropertyDescriptorObject(target, key);
  if (targetDesc !== undefined) {
    const td: Obj = AsObj(targetDesc);
    if (!DescFlag(td, "configurable")) {
      if (IsDataDescriptor(td) && !DescFlag(td, "writable")) {
        if (!SameValue(trapResult, Get(td, "value"))) {
          throw NewTypeError("proxy: inconsistent get");
        }
      }
      if (IsAccessorDescriptor(td) && Get(td, "get") === undefined && trapResult !== undefined) {
        throw NewTypeError("proxy: inconsistent get");
      }
    }
  }
  return trapResult;
}

/**
 * [[Set]] ( P, V, Receiver ), 10.5.9.
 * @intrinsic
 */
function ProxySet(p: Obj, key: Value, v: Value, receiver: Value): boolean {
  const handler: Obj = ProxyHandler(p);
  const target: Obj = ProxyTarget(p);
  const trap: Value = ProxyTrap(handler, "set");
  if (trap === undefined) {
    return SetWithReceiver(target, key, v, receiver);
  }
  if (!ToBoolean(Call(trap, handler, target, TrapKey(key), v, receiver))) {
    return false;
  }
  const targetDesc: Value = GetOwnPropertyDescriptorObject(target, key);
  if (targetDesc !== undefined) {
    const td: Obj = AsObj(targetDesc);
    if (!DescFlag(td, "configurable")) {
      if (IsDataDescriptor(td) && !DescFlag(td, "writable")) {
        if (!SameValue(v, Get(td, "value"))) {
          throw NewTypeError("proxy: inconsistent set");
        }
      }
      if (IsAccessorDescriptor(td) && Get(td, "set") === undefined) {
        throw NewTypeError("proxy: inconsistent set");
      }
    }
  }
  return true;
}

/**
 * [[Set]] on a proxy from a strict-mode assignment or the Set intrinsic: false is a
 * TypeError.
 * @intrinsic
 */
function ProxySetOrThrow(p: Obj, key: Value, v: Value, receiver: Value): void {
  if (!ProxySet(p, key, v, receiver)) {
    throw NewTypeError("proxy: cannot set property");
  }
}

/**
 * The end of OrdinarySetWithOwnDescriptor ( O, P, V, Receiver, ownDesc ), 10.1.9.2
 * steps 2.c-e, for a receiver that is a proxy, once ownDesc turned out to be a writable
 * data property (or absent).
 * @intrinsic
 */
function ProxySetOnReceiver(receiver: Obj, key: Value, v: Value): boolean {
  const existing: Value = GetOwnPropertyDescriptorObject(receiver, key);
  if (existing !== undefined) {
    const e: Obj = AsObj(existing);
    if (IsAccessorDescriptor(e) || !DescFlag(e, "writable")) {
      return false;
    }
    const valueDesc: Obj = ObjectCreate(null);
    CreateDataPropertyOrThrow(valueDesc, "value", v);
    return DefineOwnPropertyFromRecord(receiver, key, valueDesc);
  }
  return CreateDataProperty(receiver, key, v);
}

/**
 * [[Delete]] ( P ), 10.5.10.
 * @intrinsic
 */
function ProxyDeleteProperty(p: Obj, key: Value): boolean {
  const handler: Obj = ProxyHandler(p);
  const target: Obj = ProxyTarget(p);
  const trap: Value = ProxyTrap(handler, "deleteProperty");
  if (trap === undefined) {
    return DeleteProperty(target, key);
  }
  if (!ToBoolean(Call(trap, handler, target, TrapKey(key)))) {
    return false;
  }
  const targetDesc: Value = GetOwnPropertyDescriptorObject(target, key);
  if (targetDesc === undefined) {
    return true;
  }
  if (!DescFlag(AsObj(targetDesc), "configurable")) {
    throw NewTypeError("proxy: inconsistent deleteProperty");
  }
  if (!IsExtensible(target)) {
    throw NewTypeError("proxy: inconsistent deleteProperty");
  }
  return true;
}

/**
 * DeletePropertyOrThrow ( O, P ), 7.3.10, on a proxy (and the strict `delete`
 * operator, which takes the result true).
 * @intrinsic
 */
function ProxyDeletePropertyOrThrow(p: Obj, key: Value): boolean {
  if (!ProxyDeleteProperty(p, key)) {
    throw NewTypeError("could not delete property");
  }
  return true;
}

/** [[OwnPropertyKeys]] ( ), 10.5.11: a dense array of strings and symbols. */
function ProxyOwnKeys(p: Obj): Obj {
  const handler: Obj = ProxyHandler(p);
  const target: Obj = ProxyTarget(p);
  const trap: Value = ProxyTrap(handler, "ownKeys");
  if (trap === undefined) {
    return OwnPropertyKeys(target, 2);
  }
  const trapResultArray: Value = Call(trap, handler, target);
  // CreateListFromArrayLike ( obj, « String, Symbol » ), with the duplicate check.
  if (!IsObject(trapResultArray)) {
    throw NewTypeError("proxy: ownKeys must return an object");
  }
  const arr: Obj = AsObj(trapResultArray);
  const len: f64 = LengthOfArrayLike(arr);
  const trapResult: Obj = ArrayCreate(0);
  for (let i: f64 = 0; i < len; i++) {
    const key: Value = Get(arr, i);
    if (!IsString(key) && !IsSymbol(key)) {
      throw NewTypeError("proxy: properties must be strings or symbols");
    }
    if (ListIncludes(trapResult, key)) {
      throw NewTypeError("proxy: duplicate property");
    }
    CreateDataPropertyOrThrow(trapResult, i, key);
  }
  const extensibleTarget: boolean = IsExtensible(target);
  const targetKeys: Obj = OwnPropertyKeys(target, 2);
  const n: f64 = LengthOfArrayLike(targetKeys);
  const configurable: Obj = ArrayCreate(0);
  const nonconfigurable: Obj = ArrayCreate(0);
  for (let i: f64 = 0; i < n; i++) {
    const key: Value = Get(targetKeys, i);
    const desc: Value = GetOwnPropertyDescriptorObject(target, key);
    if (desc !== undefined && !DescFlag(AsObj(desc), "configurable")) {
      CreateDataPropertyOrThrow(nonconfigurable, LengthOfArrayLike(nonconfigurable), key);
    } else {
      CreateDataPropertyOrThrow(configurable, LengthOfArrayLike(configurable), key);
    }
  }
  if (extensibleTarget && LengthOfArrayLike(nonconfigurable) === 0) {
    return trapResult;
  }
  // The unchecked keys: trapResult with each target key struck out once found.
  const unchecked: Obj = ArrayCreate(0);
  for (let i: f64 = 0; i < len; i++) {
    CreateDataPropertyOrThrow(unchecked, i, Get(trapResult, i));
  }
  let remaining: f64 = len;
  const nn: f64 = LengthOfArrayLike(nonconfigurable);
  for (let i: f64 = 0; i < nn; i++) {
    if (!StrikeKey(unchecked, Get(nonconfigurable, i))) {
      throw NewTypeError("proxy: target property must be present in proxy ownKeys");
    }
    remaining--;
  }
  if (extensibleTarget) {
    return trapResult;
  }
  const nc: f64 = LengthOfArrayLike(configurable);
  for (let i: f64 = 0; i < nc; i++) {
    if (!StrikeKey(unchecked, Get(configurable, i))) {
      throw NewTypeError("proxy: target property must be present in proxy ownKeys");
    }
    remaining--;
  }
  if (remaining !== 0) {
    throw NewTypeError("proxy: property not present in target were returned by non extensible proxy");
  }
  return trapResult;
}

/**
 * Replaces the element of `list` equal to key (a string or symbol) with a hole marker
 * (null, which no key is); false if there is none.
 */
function StrikeKey(list: Obj, key: Value): boolean {
  const n: f64 = LengthOfArrayLike(list);
  for (let i: f64 = 0; i < n; i++) {
    if (SameValue(Get(list, i), key)) {
      CreateDataPropertyOrThrow(list, i, null);
      return true;
    }
  }
  return false;
}

/**
 * OwnPropertyKeys on a proxy: kind 0 strings, 1 symbols, 2 both, 3 enumerable strings
 * ([[GetOwnProperty]] of each string key, as EnumerableOwnProperties does).
 * @intrinsic
 */
function ProxyOwnPropertyKeys(p: Obj, kind: i32): Obj {
  const keys: Obj = ProxyOwnKeys(p);
  if (kind === 2) {
    return keys;
  }
  const n: f64 = LengthOfArrayLike(keys);
  const result: Obj = ArrayCreate(0);
  let count: f64 = 0;
  for (let i: f64 = 0; i < n; i++) {
    const key: Value = Get(keys, i);
    let take: boolean = kind === 1 ? IsSymbol(key) : IsString(key);
    if (take && kind === 3) {
      const desc: Value = ProxyGetOwnProperty(p, key);
      take = desc !== undefined && DescFlag(AsObj(desc), "enumerable");
    }
    if (take) {
      CreateDataPropertyOrThrow(result, count, key);
      count++;
    }
  }
  return result;
}

/**
 * [[Call]] ( thisArgument, argumentsList ), 10.5.12; args is a fresh dense array.
 * @intrinsic
 */
function ProxyCall(p: Obj, thisArg: Value, args: Obj): Value {
  const handler: Obj = ProxyHandler(p);
  const target: Obj = ProxyTarget(p);
  const trap: Value = ProxyTrap(handler, "apply");
  if (trap === undefined) {
    return CallWithArray(target, thisArg, args);
  }
  return Call(trap, handler, target, thisArg, args);
}

/**
 * [[Construct]] ( argumentsList, newTarget ), 10.5.13; args is a fresh dense array.
 * @intrinsic
 */
function ProxyConstruct(p: Obj, args: Obj, newTarget: Value): Obj {
  const handler: Obj = ProxyHandler(p);
  const target: Obj = ProxyTarget(p);
  const trap: Value = ProxyTrap(handler, "construct");
  if (trap === undefined) {
    return ConstructWithArray(target, args, newTarget);
  }
  const newObj: Value = Call(trap, handler, target, args, newTarget);
  if (!IsObject(newObj)) {
    throw NewTypeError("not an object");
  }
  return AsObj(newObj);
}

// ---- Generic algorithms over the internal methods ---------------------------------------

/**
 * [[Construct]] of an ordinary function f when reading new.target's `prototype` runs
 * user code (a getter, a proxy): OrdinaryCreateFromConstructor, then the call.
 * @intrinsic
 */
function OrdinaryConstructSlow(f: Obj, args: Obj, newTarget: Value): Obj {
  const proto: Obj = GetPrototypeFromConstructor(newTarget, "ObjectPrototype");
  return ConstructWithThis(f, ObjectCreate(proto), args, newTarget);
}

/**
 * SetIntegrityLevel ( O, level ), 7.3.15, through the internal methods (for proxies).
 * @intrinsic
 */
function ProxySetIntegrityLevel(o: Obj, frozen: boolean): boolean {
  if (!PreventExtensions(o)) {
    return false;
  }
  const keys: Obj = OwnPropertyKeys(o, 2);
  const n: f64 = LengthOfArrayLike(keys);
  for (let i: f64 = 0; i < n; i++) {
    const key: Value = Get(keys, i);
    const desc: Obj = ObjectCreate(null);
    CreateDataPropertyOrThrow(desc, "configurable", false);
    if (frozen) {
      const current: Value = GetOwnPropertyDescriptorObject(o, key);
      if (current === undefined) {
        continue;
      }
      if (!IsAccessorDescriptor(AsObj(current))) {
        CreateDataPropertyOrThrow(desc, "writable", false);
      }
    }
    DefinePropertyOrThrow(o, key, desc);
  }
  return true;
}

/**
 * TestIntegrityLevel ( O, level ), 7.3.16, through the internal methods (for proxies).
 * @intrinsic
 */
function ProxyTestIntegrityLevel(o: Obj, frozen: boolean): boolean {
  if (IsExtensible(o)) {
    return false;
  }
  const keys: Obj = OwnPropertyKeys(o, 2);
  const n: f64 = LengthOfArrayLike(keys);
  for (let i: f64 = 0; i < n; i++) {
    const current: Value = GetOwnPropertyDescriptorObject(o, Get(keys, i));
    if (current !== undefined) {
      const c: Obj = AsObj(current);
      if (DescFlag(c, "configurable")) {
        return false;
      }
      if (frozen && IsDataDescriptor(c) && DescFlag(c, "writable")) {
        return false;
      }
    }
  }
  return true;
}

/**
 * One step of a for-in whose object has a proxy on its prototype chain
 * (EnumerateObjectProperties, 14.7.5.9, lazily, as qjs does it): the Record `it` made by
 * the interpreter holds 0 the object being walked (null when done), 1 its enumerable
 * string keys (undefined until read), 2 the next index, 3 the keys seen so far, 4 the
 * result, 5 whether there is one. Each object's keys are read when the walk reaches it
 * ([[OwnPropertyKeys]], then [[GetOwnProperty]] for each), and each key is checked
 * again ([[GetOwnProperty]]) just before it is visited, so a key deleted on the way is
 * skipped. The result is the next key, or null at the end.
 * @intrinsic
 */
function ForInStep(it: Obj): void {
  while (true) {
    const cur: Value = RecordGet(it, 0);
    if (cur === null) {
      RecordSet(it, 4, null);
      RecordSet(it, 5, true);
      return;
    }
    const o: Obj = AsObj(cur);
    const pending: Value = RecordGet(it, 1);
    if (pending === undefined) {
      const visited: Obj = AsObj(RecordGet(it, 3));
      const own: Obj = OwnPropertyKeys(o, 0);
      const n: f64 = LengthOfArrayLike(own);
      const keys: Obj = ArrayCreate(0);
      for (let i: f64 = 0; i < n; i++) {
        const key: Value = Get(own, i);
        if (!ListIncludes(visited, key)) {
          const attrs: i32 = OwnPropertyAttributes(o, key);
          if (attrs >= 0) {
            CreateDataPropertyOrThrow(visited, LengthOfArrayLike(visited), key);
            if ((attrs & 2) !== 0) {
              CreateDataPropertyOrThrow(keys, LengthOfArrayLike(keys), key);
            }
          }
        }
      }
      RecordSet(it, 1, keys);
      RecordSet(it, 2, 0);
    } else {
      const keys: Obj = AsObj(pending);
      const i: f64 = AsNumber(RecordGet(it, 2));
      if (i < LengthOfArrayLike(keys)) {
        RecordSet(it, 2, i + 1);
        const key: Value = Get(keys, i);
        if (OwnPropertyAttributes(o, key) >= 0) {
          RecordSet(it, 4, key);
          RecordSet(it, 5, true);
          return;
        }
      } else {
        RecordSet(it, 0, GetPrototypeOf(o));
        RecordSet(it, 1, undefined);
      }
    }
  }
}
