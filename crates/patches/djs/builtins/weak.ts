// WeakMap (ECMA-262 24.3) and WeakSet (24.4). Entries live in an EphemeronArray, which
// the collector treats as ephemerons (D1382).

/** The WeakMap `this` of a WeakMap.prototype method. */
function ThisWeakMap(v: Value): Obj {
  if (CollectionClass(v) !== 2) {
    throw NewTypeError("WeakMap object expected");
  }
  return AsObj(v);
}

/** The WeakSet `this` of a WeakSet.prototype method. */
function ThisWeakSet(v: Value): Obj {
  if (CollectionClass(v) !== 3) {
    throw NewTypeError("WeakSet object expected");
  }
  return AsObj(v);
}

/**
 * WeakMap ( [ iterable ] ), 24.3.1.1.
 * @constructor WeakMap
 */
function WeakMapConstructor(iterable?: Value): Obj {
  if (new.target === undefined) {
    throw NewTypeError("must be called with new");
  }
  const map: Obj = CollectionCreate(2, GetPrototypeFromConstructor(new.target, "WeakMapPrototype"));
  if (iterable === undefined || iterable === null) {
    return map;
  }
  const adder: Value = Get(map, "set");
  if (!IsCallable(adder)) {
    throw NewTypeError("not a function");
  }
  AddEntriesFromIterable(map, iterable, adder);
  return map;
}

/**
 * WeakMap.prototype.delete ( key ), 24.3.3.2.
 * @builtin WeakMap.prototype.delete
 */
function WeakMapPrototypeDelete(this: Value, key: Value): boolean {
  const m: Obj = ThisWeakMap(this);
  return CanBeHeldWeakly(key) && CollectionDelete(m, key);
}

/**
 * WeakMap.prototype.get ( key ), 24.3.3.3.
 * @builtin WeakMap.prototype.get
 */
function WeakMapPrototypeGet(this: Value, key: Value): Value {
  const m: Obj = ThisWeakMap(this);
  if (!CanBeHeldWeakly(key)) {
    return undefined;
  }
  return CollectionGet(m, key);
}

/**
 * WeakMap.prototype.has ( key ), 24.3.3.4.
 * @builtin WeakMap.prototype.has
 */
function WeakMapPrototypeHas(this: Value, key: Value): boolean {
  const m: Obj = ThisWeakMap(this);
  return CanBeHeldWeakly(key) && CollectionHas(m, key);
}

/**
 * WeakMap.prototype.set ( key, value ), 24.3.3.5.
 * @builtin WeakMap.prototype.set
 */
function WeakMapPrototypeSet(this: Value, key: Value, value: Value): Value {
  const m: Obj = ThisWeakMap(this);
  if (!CanBeHeldWeakly(key)) {
    throw NewTypeError("invalid value used as WeakMap key");
  }
  CollectionSet(m, key, value);
  return this;
}

/**
 * WeakMap.prototype.getOrInsert ( key, value ) (upsert).
 * @builtin WeakMap.prototype.getOrInsert
 */
function WeakMapPrototypeGetOrInsert(this: Value, key: Value, value: Value): Value {
  const m: Obj = ThisWeakMap(this);
  if (!CanBeHeldWeakly(key)) {
    throw NewTypeError("invalid value used as WeakMap key");
  }
  if (CollectionHas(m, key)) {
    return CollectionGet(m, key);
  }
  CollectionSet(m, key, value);
  return value;
}

/**
 * WeakMap.prototype.getOrInsertComputed ( key, callbackfn ) (upsert).
 * @builtin WeakMap.prototype.getOrInsertComputed
 */
function WeakMapPrototypeGetOrInsertComputed(this: Value, key: Value, callbackfn: Value): Value {
  const m: Obj = ThisWeakMap(this);
  if (!IsCallable(callbackfn)) {
    throw NewTypeError("not a function");
  }
  if (!CanBeHeldWeakly(key)) {
    throw NewTypeError("invalid value used as WeakMap key");
  }
  if (CollectionHas(m, key)) {
    return CollectionGet(m, key);
  }
  const value: Value = Call(callbackfn, undefined, key);
  CollectionSet(m, key, value);
  return value;
}

/**
 * WeakMap.prototype [ %Symbol.toStringTag% ], 24.3.3.6.
 * @value WeakMap.prototype[Symbol.toStringTag]
 * @attributes configurable
 */
function WeakMapPrototypeToStringTag(): Str {
  return "WeakMap";
}

/**
 * WeakSet ( [ iterable ] ), 24.4.1.1.
 * @constructor WeakSet
 */
function WeakSetConstructor(iterable?: Value): Obj {
  if (new.target === undefined) {
    throw NewTypeError("must be called with new");
  }
  const set: Obj = CollectionCreate(3, GetPrototypeFromConstructor(new.target, "WeakSetPrototype"));
  if (iterable === undefined || iterable === null) {
    return set;
  }
  const adder: Value = Get(set, "add");
  if (!IsCallable(adder)) {
    throw NewTypeError("not a function");
  }
  const iterator: Obj = GetIterator(iterable);
  const next: Value = Get(iterator, "next");
  while (true) {
    // Without a result object where that is unobservable (D1800).
    let nextValue: Value = undefined;
    if (IteratorFastReady(iterator, next)) {
      nextValue = IteratorFastValue(iterator, next);
    } else {
      const result: Obj = IteratorNextResult(iterator, next);
      if (ToBoolean(Get(result, "done"))) {
        return set;
      }
      nextValue = Get(result, "value");
    }
    try {
      Call(adder, set, nextValue);
    } catch (e) {
      IteratorCloseThrow(iterator);
      throw e;
    }
  }
}

/**
 * WeakSet.prototype.add ( value ), 24.4.3.1.
 * @builtin WeakSet.prototype.add
 */
function WeakSetPrototypeAdd(this: Value, value: Value): Value {
  const s: Obj = ThisWeakSet(this);
  if (!CanBeHeldWeakly(value)) {
    throw NewTypeError("invalid value used as WeakSet key");
  }
  CollectionSet(s, value, true);
  return this;
}

/**
 * WeakSet.prototype.delete ( value ), 24.4.3.3.
 * @builtin WeakSet.prototype.delete
 */
function WeakSetPrototypeDelete(this: Value, value: Value): boolean {
  const s: Obj = ThisWeakSet(this);
  return CanBeHeldWeakly(value) && CollectionDelete(s, value);
}

/**
 * WeakSet.prototype.has ( value ), 24.4.3.4.
 * @builtin WeakSet.prototype.has
 */
function WeakSetPrototypeHas(this: Value, value: Value): boolean {
  const s: Obj = ThisWeakSet(this);
  return CanBeHeldWeakly(value) && CollectionHas(s, value);
}

/**
 * WeakSet.prototype [ %Symbol.toStringTag% ], 24.4.3.5.
 * @value WeakSet.prototype[Symbol.toStringTag]
 * @attributes configurable
 */
function WeakSetPrototypeToStringTag(): Str {
  return "WeakSet";
}
