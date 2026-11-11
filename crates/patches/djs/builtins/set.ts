// Set (ECMA-262 24.2), its ES2025 methods, and Set Iterators (24.2.6).

/** The Set `this` of a Set.prototype method (qjs's message otherwise). */
function ThisSet(v: Value): Obj {
  if (CollectionClass(v) !== 1) {
    throw NewTypeError("Set object expected");
  }
  return AsObj(v);
}

/**
 * Set ( [ iterable ] ), 24.2.2.1.
 * @constructor Set
 * @fast
 */
function SetConstructor(iterable?: Value): Obj {
  if (new.target === undefined) {
    throw NewTypeError("must be called with new");
  }
  const set: Obj = CollectionCreate(1, GetPrototypeFromConstructor(new.target, "SetPrototype"));
  if (iterable === undefined || iterable === null) {
    return set;
  }
  const adder: Value = Get(set, "add");
  if (!IsCallable(adder)) {
    throw NewTypeError("not a function");
  }
  // The original add on the new Set is CollectionSet itself (0080).
  const direct: boolean = IsOriginalCollectionAdder(adder, 1);
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
    if (direct) {
      CollectionSet(set, nextValue, true);
      continue;
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
 * get Set [ %Symbol.species% ], 24.2.3.2.
 * @getter Set[Symbol.species]
 */
function SetSpecies(this: Value): Value {
  return this;
}

/**
 * Set.prototype.add ( value ), 24.2.4.1.
 * @builtin Set.prototype.add
 * @fast
 * @intrinsic
 */
function SetPrototypeAdd(this: Value, value: Value): Value {
  CollectionSet(ThisSet(this), value, true);
  return this;
}

/**
 * Set.prototype.clear ( ), 24.2.4.2.
 * @builtin Set.prototype.clear
 */
function SetPrototypeClear(this: Value): void {
  CollectionClear(ThisSet(this));
}

/**
 * Set.prototype.delete ( value ), 24.2.4.4.
 * @builtin Set.prototype.delete
 */
function SetPrototypeDelete(this: Value, value: Value): boolean {
  return CollectionDelete(ThisSet(this), value);
}

/**
 * Set.prototype.entries ( ), 24.2.4.6.
 * @builtin Set.prototype.entries
 */
function SetPrototypeEntries(this: Value): Obj {
  return CollectionIterator(ThisSet(this), 2);
}

/**
 * Set.prototype.forEach ( callbackfn [ , thisArg ] ), 24.2.4.7.
 * @builtin Set.prototype.forEach
 */
function SetPrototypeForEach(this: Value, callbackfn: Value, thisArg?: Value): void {
  const s: Obj = ThisSet(this);
  if (!IsCallable(callbackfn)) {
    throw NewTypeError("not a function");
  }
  const it: Obj = CollectionIterator(s, 0);
  while (CollectionIteratorStep(it)) {
    const k: Value = CollectionIteratorKey(it);
    Call(callbackfn, thisArg, k, k, s);
  }
}

/**
 * Set.prototype.has ( value ), 24.2.4.8.
 * @builtin Set.prototype.has
 * @fast
 */
function SetPrototypeHas(this: Value, value: Value): boolean {
  return CollectionHas(ThisSet(this), value);
}

/**
 * get Set.prototype.size, 24.2.4.14.
 * @getter Set.prototype.size
 */
function SetPrototypeSize(this: Value): f64 {
  return CollectionSize(ThisSet(this));
}

/**
 * Set.prototype.values ( ), 24.2.4.17; also keys and [ %Symbol.iterator% ].
 * @builtin Set.prototype.values
 * @builtin Set.prototype.keys
 * @builtin Set.prototype[Symbol.iterator]
 */
function SetPrototypeValues(this: Value): Obj {
  return CollectionIterator(ThisSet(this), 0);
}

/**
 * Set.prototype [ %Symbol.toStringTag% ], 24.2.4.18.
 * @value Set.prototype[Symbol.toStringTag]
 * @attributes configurable
 */
function SetPrototypeToStringTag(): Str {
  return "Set";
}

/**
 * %SetIteratorPrototype%.next ( ), 24.2.6.2.1.
 * @builtin %SetIteratorPrototype%.next
 * @intrinsic
 */
function SetIteratorPrototypeNext(this: Value): Obj {
  return CollectionIteratorNext(this, true);
}

/**
 * %SetIteratorPrototype% [ %Symbol.toStringTag% ], 24.2.6.2.2.
 * @value %SetIteratorPrototype%[Symbol.toStringTag]
 * @attributes configurable
 */
function SetIteratorToStringTag(): Str {
  return "Set Iterator";
}

// ---- The Set methods (24.2.4, ES2025) ------------------------------------------------------

/**
 * GetSetRecord ( obj ), 24.2.1.2, as a record object: set, size, has, keys (prototype-
 * less, internal). qjs's messages.
 */
function GetSetRecord(obj: Value): Obj {
  if (!IsObject(obj)) {
    throw NewTypeError("not an object");
  }
  const o: Obj = AsObj(obj);
  const rawSize: Value = Get(o, "size");
  const numSize: f64 = ToNumber(rawSize);
  if (numSize !== numSize) {
    throw NewTypeError(".size is not a number");
  }
  const intSize: f64 = ToIntegerOrInfinityNumber(numSize);
  if (intSize < 0) {
    throw NewRangeError(".size must be positive");
  }
  const has: Value = Get(o, "has");
  if (!IsCallable(has)) {
    throw NewTypeError(".has is not a function");
  }
  const keys: Value = Get(o, "keys");
  if (!IsCallable(keys)) {
    throw NewTypeError(".keys is not a function");
  }
  const rec: Obj = ObjectCreate(null);
  CreateDataPropertyOrThrow(rec, "set", o);
  CreateDataPropertyOrThrow(rec, "size", intSize);
  CreateDataPropertyOrThrow(rec, "has", has);
  CreateDataPropertyOrThrow(rec, "keys", keys);
  return rec;
}

/** The size of a set record (a Number; may be +Infinity). */
function RecordSize(rec: Obj): f64 {
  return AsNumber(Get(rec, "size"));
}

/** Calls the record's `has` on the record's set. */
function RecordHas(rec: Obj, value: Value): boolean {
  return ToBoolean(Call(Get(rec, "has"), Get(rec, "set"), value));
}

/** GetIteratorFromMethod ( set, keys ) of a set record, 24.2.1.3 (GetKeysIterator). */
function RecordKeys(rec: Obj): Obj {
  const iterator: Value = Call(Get(rec, "keys"), Get(rec, "set"));
  if (!IsObject(iterator)) {
    throw NewTypeError("not an object");
  }
  return AsObj(iterator);
}

/** A new Set with the elements of s (the spec's copy of [[SetData]]). */
function SetCopy(s: Obj): Obj {
  const result: Obj = CollectionCreate(1, RealmIntrinsic("SetPrototype"));
  const it: Obj = CollectionIterator(s, 0);
  while (CollectionIteratorStep(it)) {
    CollectionSet(result, CollectionIteratorKey(it), true);
  }
  return result;
}

/**
 * The next value of a keys iterator, as a pair: done (true) or the value. A non-object
 * result is a TypeError (IteratorStepValue).
 */
function KeysNext(iterator: Obj, next: Value, out: Obj): boolean {
  const result: Obj = IteratorNextResult(iterator, next);
  if (ToBoolean(Get(result, "done"))) {
    return false;
  }
  CreateDataPropertyOrThrow(out, 0, Get(result, "value"));
  return true;
}

/**
 * Set.prototype.union ( other ), 24.2.4.19.
 * @builtin Set.prototype.union
 */
function SetPrototypeUnion(this: Value, other: Value): Obj {
  const s: Obj = ThisSet(this);
  const rec: Obj = GetSetRecord(other);
  const keys: Obj = RecordKeys(rec);
  const next: Value = Get(keys, "next");
  const result: Obj = SetCopy(s);
  const cell: Obj = ArrayCreate(1);
  while (KeysNext(keys, next, cell)) {
    CollectionSet(result, Get(cell, 0), true);
  }
  return result;
}

/**
 * Set.prototype.intersection ( other ), 24.2.4.9.
 * @builtin Set.prototype.intersection
 */
function SetPrototypeIntersection(this: Value, other: Value): Obj {
  const s: Obj = ThisSet(this);
  const rec: Obj = GetSetRecord(other);
  const result: Obj = CollectionCreate(1, RealmIntrinsic("SetPrototype"));
  if (CollectionSize(s) <= RecordSize(rec)) {
    const it: Obj = CollectionIterator(s, 0);
    while (CollectionIteratorStep(it)) {
      const e: Value = CollectionIteratorKey(it);
      if (RecordHas(rec, e)) {
        CollectionSet(result, e, true);
      }
    }
    return result;
  }
  const keys: Obj = RecordKeys(rec);
  const next: Value = Get(keys, "next");
  const cell: Obj = ArrayCreate(1);
  while (KeysNext(keys, next, cell)) {
    const v: Value = Get(cell, 0);
    if (CollectionHas(s, v)) {
      CollectionSet(result, v, true);
    }
  }
  return result;
}

/**
 * Set.prototype.difference ( other ), 24.2.4.5.
 * @builtin Set.prototype.difference
 */
function SetPrototypeDifference(this: Value, other: Value): Obj {
  const s: Obj = ThisSet(this);
  const rec: Obj = GetSetRecord(other);
  const result: Obj = SetCopy(s);
  if (CollectionSize(s) <= RecordSize(rec)) {
    const it: Obj = CollectionIterator(s, 0);
    while (CollectionIteratorStep(it)) {
      const e: Value = CollectionIteratorKey(it);
      if (RecordHas(rec, e)) {
        CollectionDelete(result, e);
      }
    }
    return result;
  }
  const keys: Obj = RecordKeys(rec);
  const next: Value = Get(keys, "next");
  const cell: Obj = ArrayCreate(1);
  while (KeysNext(keys, next, cell)) {
    CollectionDelete(result, Get(cell, 0));
  }
  return result;
}

/**
 * Set.prototype.symmetricDifference ( other ), 24.2.4.16.
 * @builtin Set.prototype.symmetricDifference
 */
function SetPrototypeSymmetricDifference(this: Value, other: Value): Obj {
  const s: Obj = ThisSet(this);
  const rec: Obj = GetSetRecord(other);
  const keys: Obj = RecordKeys(rec);
  const next: Value = Get(keys, "next");
  const result: Obj = SetCopy(s);
  const cell: Obj = ArrayCreate(1);
  while (KeysNext(keys, next, cell)) {
    const v: Value = Get(cell, 0);
    if (CollectionHas(s, v)) {
      CollectionDelete(result, v);
    } else {
      CollectionSet(result, v, true);
    }
  }
  return result;
}

/**
 * Set.prototype.isSubsetOf ( other ), 24.2.4.11.
 * @builtin Set.prototype.isSubsetOf
 */
function SetPrototypeIsSubsetOf(this: Value, other: Value): boolean {
  const s: Obj = ThisSet(this);
  const rec: Obj = GetSetRecord(other);
  if (CollectionSize(s) > RecordSize(rec)) {
    return false;
  }
  const it: Obj = CollectionIterator(s, 0);
  while (CollectionIteratorStep(it)) {
    if (!RecordHas(rec, CollectionIteratorKey(it))) {
      return false;
    }
  }
  return true;
}

/**
 * Set.prototype.isSupersetOf ( other ), 24.2.4.12.
 * @builtin Set.prototype.isSupersetOf
 */
function SetPrototypeIsSupersetOf(this: Value, other: Value): boolean {
  const s: Obj = ThisSet(this);
  const rec: Obj = GetSetRecord(other);
  if (CollectionSize(s) < RecordSize(rec)) {
    return false;
  }
  const keys: Obj = RecordKeys(rec);
  const next: Value = Get(keys, "next");
  const cell: Obj = ArrayCreate(1);
  while (KeysNext(keys, next, cell)) {
    if (!CollectionHas(s, Get(cell, 0))) {
      IteratorClose(keys);
      return false;
    }
  }
  return true;
}

/**
 * Set.prototype.isDisjointFrom ( other ), 24.2.4.10.
 * @builtin Set.prototype.isDisjointFrom
 */
function SetPrototypeIsDisjointFrom(this: Value, other: Value): boolean {
  const s: Obj = ThisSet(this);
  const rec: Obj = GetSetRecord(other);
  if (CollectionSize(s) <= RecordSize(rec)) {
    const it: Obj = CollectionIterator(s, 0);
    while (CollectionIteratorStep(it)) {
      if (RecordHas(rec, CollectionIteratorKey(it))) {
        return false;
      }
    }
    return true;
  }
  const keys: Obj = RecordKeys(rec);
  const next: Value = Get(keys, "next");
  const cell: Obj = ArrayCreate(1);
  while (KeysNext(keys, next, cell)) {
    if (CollectionHas(s, Get(cell, 0))) {
      IteratorClose(keys);
      return false;
    }
  }
  return true;
}
