// Map (ECMA-262 24.1) and Map Iterators (24.1.5). The table is heap/collection.rs.

/** The Map `this` of a Map.prototype method (qjs's message otherwise). */
function ThisMap(v: Value): Obj {
  if (CollectionClass(v) !== 0) {
    throw NewTypeError("Map object expected");
  }
  return AsObj(v);
}

/**
 * AddEntriesFromIterable ( target, iterable, adder ), 24.1.1.2: each entry must be an
 * object; the iterator is closed when reading it or calling the adder throws.
 * @intrinsic
 */
function AddEntriesFromIterable(target: Obj, iterable: Value, adder: Value): void {
  // The original Map.prototype.set on a Map is CollectionSet itself (0080).
  const direct: boolean = IsOriginalCollectionAdder(adder, 0) && CollectionClass(target) === 0;
  const iterator: Obj = GetIterator(iterable);
  const next: Value = Get(iterator, "next");
  while (true) {
    // Without a result object where that is unobservable (D1800).
    let nextItem: Value = undefined;
    if (IteratorFastReady(iterator, next)) {
      nextItem = IteratorFastValue(iterator, next);
    } else {
      const result: Obj = IteratorNextResult(iterator, next);
      if (ToBoolean(Get(result, "done"))) {
        return;
      }
      nextItem = Get(result, "value");
    }
    try {
      if (!IsObject(nextItem)) {
        throw NewTypeError("not an object");
      }
      const item: Obj = AsObj(nextItem);
      const k: Value = Get(item, "0");
      const v: Value = Get(item, "1");
      if (direct) {
        CollectionSet(target, k, v);
      } else {
        Call(adder, target, k, v);
      }
    } catch (e) {
      IteratorCloseThrow(iterator);
      throw e;
    }
  }
}

/**
 * Map ( [ iterable ] ), 24.1.1.1.
 * @constructor Map
 * @fast
 */
function MapConstructor(iterable?: Value): Obj {
  if (new.target === undefined) {
    throw NewTypeError("must be called with new");
  }
  const map: Obj = CollectionCreate(0, GetPrototypeFromConstructor(new.target, "MapPrototype"));
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
 * Map.groupBy ( items, callback ), 24.1.2.1.
 * @builtin Map.groupBy
 */
function MapGroupBy(items: Value, callback: Value): Obj {
  if (!IsCallable(callback)) {
    throw NewTypeError("not a function");
  }
  const map: Obj = CollectionCreate(0, RealmIntrinsic("MapPrototype"));
  const iterator: Obj = GetIterator(items);
  const next: Value = Get(iterator, "next");
  let k: f64 = 0;
  while (true) {
    // Without a result object where that is unobservable (D1800).
    let value: Value = undefined;
    if (IteratorFastReady(iterator, next)) {
      value = IteratorFastValue(iterator, next);
    } else {
      const result: Obj = IteratorNextResult(iterator, next);
      if (ToBoolean(Get(result, "done"))) {
        return map;
      }
      value = Get(result, "value");
    }
    let key: Value = undefined;
    try {
      key = Call(callback, undefined, value, k);
    } catch (e) {
      IteratorCloseThrow(iterator);
      throw e;
    }
    let group: Value = CollectionGet(map, key);
    if (group === undefined) {
      group = ArrayCreate(0);
      CollectionSet(map, key, group);
    }
    const g: Obj = AsObj(group);
    CreateDataPropertyOrThrow(g, LengthOfArrayLike(g), value);
    k++;
  }
}

/**
 * get Map [ %Symbol.species% ], 24.1.2.3.
 * @getter Map[Symbol.species]
 */
function MapSpecies(this: Value): Value {
  return this;
}

/**
 * Map.prototype.clear ( ), 24.1.3.1.
 * @builtin Map.prototype.clear
 */
function MapPrototypeClear(this: Value): void {
  CollectionClear(ThisMap(this));
}

/**
 * Map.prototype.delete ( key ), 24.1.3.3.
 * @builtin Map.prototype.delete
 */
function MapPrototypeDelete(this: Value, key: Value): boolean {
  return CollectionDelete(ThisMap(this), key);
}

/**
 * Map.prototype.entries ( ), 24.1.3.4; also Map.prototype [ %Symbol.iterator% ].
 * @builtin Map.prototype.entries
 * @builtin Map.prototype[Symbol.iterator]
 */
function MapPrototypeEntries(this: Value): Obj {
  return CollectionIterator(ThisMap(this), 2);
}

/**
 * Map.prototype.forEach ( callbackfn [ , thisArg ] ), 24.1.3.5.
 * @builtin Map.prototype.forEach
 */
function MapPrototypeForEach(this: Value, callbackfn: Value, thisArg?: Value): void {
  const m: Obj = ThisMap(this);
  if (!IsCallable(callbackfn)) {
    throw NewTypeError("not a function");
  }
  const it: Obj = CollectionIterator(m, 2);
  while (CollectionIteratorStep(it)) {
    Call(callbackfn, thisArg, CollectionIteratorValue(it), CollectionIteratorKey(it), m);
  }
}

/**
 * Map.prototype.get ( key ), 24.1.3.6.
 * @builtin Map.prototype.get
 * @fast
 */
function MapPrototypeGet(this: Value, key: Value): Value {
  return CollectionGet(ThisMap(this), key);
}

/** CanonicalizeKeyedCollectionKey ( key ), 24.5.1: -0 becomes +0. */
function CanonicalizeKeyedCollectionKey(key: Value): Value {
  if (IsNumber(key) && AsNumber(key) === 0) {
    return 0;
  }
  return key;
}

/**
 * Map.prototype.getOrInsert ( key, value ) (upsert).
 * @builtin Map.prototype.getOrInsert
 */
function MapPrototypeGetOrInsert(this: Value, key: Value, value: Value): Value {
  const m: Obj = ThisMap(this);
  if (CollectionHas(m, key)) {
    return CollectionGet(m, key);
  }
  CollectionSet(m, key, value);
  return value;
}

/**
 * Map.prototype.getOrInsertComputed ( key, callbackfn ) (upsert): the callback may
 * insert the key itself; its result overwrites that entry.
 * @builtin Map.prototype.getOrInsertComputed
 */
function MapPrototypeGetOrInsertComputed(this: Value, key: Value, callbackfn: Value): Value {
  const m: Obj = ThisMap(this);
  if (!IsCallable(callbackfn)) {
    throw NewTypeError("not a function");
  }
  const k: Value = CanonicalizeKeyedCollectionKey(key);
  if (CollectionHas(m, k)) {
    return CollectionGet(m, k);
  }
  const value: Value = Call(callbackfn, undefined, k);
  CollectionSet(m, k, value);
  return value;
}

/**
 * Map.prototype.has ( key ), 24.1.3.7.
 * @builtin Map.prototype.has
 * @fast
 */
function MapPrototypeHas(this: Value, key: Value): boolean {
  return CollectionHas(ThisMap(this), key);
}

/**
 * Map.prototype.keys ( ), 24.1.3.8.
 * @builtin Map.prototype.keys
 */
function MapPrototypeKeys(this: Value): Obj {
  return CollectionIterator(ThisMap(this), 0);
}

/**
 * Map.prototype.set ( key, value ), 24.1.3.9.
 * @builtin Map.prototype.set
 * @fast
 * @intrinsic
 */
function MapPrototypeSet(this: Value, key: Value, value: Value): Value {
  CollectionSet(ThisMap(this), key, value);
  return this;
}

/**
 * get Map.prototype.size, 24.1.3.10.
 * @getter Map.prototype.size
 */
function MapPrototypeSize(this: Value): f64 {
  return CollectionSize(ThisMap(this));
}

/**
 * Map.prototype.values ( ), 24.1.3.11.
 * @builtin Map.prototype.values
 */
function MapPrototypeValues(this: Value): Obj {
  return CollectionIterator(ThisMap(this), 1);
}

/**
 * Map.prototype [ %Symbol.toStringTag% ], 24.1.3.13.
 * @value Map.prototype[Symbol.toStringTag]
 * @attributes configurable
 */
function MapPrototypeToStringTag(): Str {
  return "Map";
}

/**
 * %MapIteratorPrototype%.next ( ), 24.1.5.2.1.
 * @builtin %MapIteratorPrototype%.next
 * @intrinsic
 */
function MapIteratorPrototypeNext(this: Value): Obj {
  return CollectionIteratorNext(this, false);
}

/**
 * %MapIteratorPrototype% [ %Symbol.toStringTag% ], 24.1.5.2.2.
 * @value %MapIteratorPrototype%[Symbol.toStringTag]
 * @attributes configurable
 */
function MapIteratorToStringTag(): Str {
  return "Map Iterator";
}
