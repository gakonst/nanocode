// Iteration (ECMA-262 7.4 and 23.1.5): the iterator protocol for for-of and spread,
// and Array Iterators. The compiler calls %GetIterator, %IteratorClose and
// %IteratorCloseThrow; the step (`next`, `done`, `value`) is plain bytecode.

/**
 * GetIterator ( obj, sync ), 7.4.3, returning only the iterator: the caller reads
 * `next` once, right after.
 * @intrinsic
 */
function GetIterator(obj: Value): Obj {
  const method: Value = GetV(obj, WellKnownSymbol("iterator"));
  if (!IsCallable(method)) {
    throw NewTypeError("value is not iterable");
  }
  const iterator: Value = Call(method, obj);
  if (!IsObject(iterator)) {
    throw NewTypeError("not an object");
  }
  return AsObj(iterator);
}

/**
 * IteratorClose ( iteratorRecord, completion ), 7.4.11, for a break, continue or
 * return completion: errors from `return` propagate.
 * @intrinsic
 */
function IteratorClose(iterator: Obj): void {
  const method: Value = Get(iterator, "return");
  if (method === undefined || method === null) {
    return;
  }
  const result: Value = Call(method, iterator);
  if (!IsObject(result)) {
    throw NewTypeError("not an object");
  }
}

/**
 * IteratorClose for a throw completion: `return` is still called, but its errors and
 * result are ignored, so the original exception wins.
 * @intrinsic
 */
function IteratorCloseThrow(iterator: Obj): void {
  try {
    const method: Value = Get(iterator, "return");
    if (method === undefined || method === null) {
      return;
    }
    Call(method, iterator);
  } catch (e) {
    return;
  }
}

/**
 * The rest element of array destructuring (`[a, ...rest] = it`): the remaining values
 * of the iterator, in a new array. Errors come from the iterator itself, so the caller
 * does not close it.
 * @intrinsic
 */
function IteratorRest(iterator: Obj, next: Value): Obj {
  const arr: Obj = ArrayCreate(0);
  let n: f64 = 0;
  while (true) {
    // Without a result object where that is unobservable (D1800), keys and values runs
    // in one step (D2305).
    n = IteratorFastDrain(iterator, next, arr, n);
    if (IteratorFastReady(iterator, next)) {
      CreateDataPropertyOrThrow(arr, n, IteratorFastValue(iterator, next));
    } else {
      const result: Value = Call(next, iterator);
      if (!IsObject(result)) {
        throw NewTypeError("iterator must return an object");
      }
      if (ToBoolean(Get(AsObj(result), "done"))) {
        return arr;
      }
      CreateDataPropertyOrThrow(arr, n, Get(AsObj(result), "value"));
    }
    n = n + 1;
  }
}

/**
 * Array.prototype.values ( ), 23.1.3.38; also Array.prototype [ %Symbol.iterator% ].
 * @builtin Array.prototype.values
 * @builtin Array.prototype[Symbol.iterator]
 * @intrinsic
 */
function ArrayPrototypeValues(this: Value): Obj {
  return CreateArrayIterator(ToObject(this), 1);
}

/**
 * Array.prototype.keys ( ), 23.1.3.19.
 * @builtin Array.prototype.keys
 */
function ArrayPrototypeKeys(this: Value): Obj {
  return CreateArrayIterator(ToObject(this), 0);
}

/**
 * Array.prototype.entries ( ), 23.1.3.7.
 * @builtin Array.prototype.entries
 */
function ArrayPrototypeEntries(this: Value): Obj {
  return CreateArrayIterator(ToObject(this), 2);
}

/**
 * %ArrayIteratorPrototype%.next ( ), 23.1.5.2.1 (no typed arrays yet).
 * @builtin %ArrayIteratorPrototype%.next
 * @intrinsic
 */
function ArrayIteratorPrototypeNext(this: Value): Obj {
  const target: Value = ArrayIteratorTarget(this);
  if (target === undefined) {
    return CreateIterResultObject(undefined, true);
  }
  const o: Obj = AsObj(target);
  const index: f64 = ArrayIteratorIndex(this);
  // A typed array: its own length, and a TypeError once detached or out of bounds
  // (23.1.5.1 step 2.b.ii).
  const len: f64 = IsTypedArray(o) ? TALength(ValidateTypedArray(o)) : LengthOfArrayLike(o);
  if (index >= len) {
    ArrayIteratorFinish(this);
    return CreateIterResultObject(undefined, true);
  }
  ArrayIteratorSetIndex(this, index + 1);
  const kind: i32 = ArrayIteratorKind(this);
  if (kind === 0) {
    return CreateIterResultObject(index, false);
  }
  const value: Value = Get(o, index);
  if (kind === 1) {
    return CreateIterResultObject(value, false);
  }
  return CreateIterResultObject(CreateArrayFromPair(index, value), false);
}

/**
 * %IteratorPrototype% [ %Symbol.iterator% ] ( ), 27.1.2.1.
 * @builtin %IteratorPrototype%[Symbol.iterator]
 */
function IteratorPrototypeIterator(this: Value): Value {
  return this;
}

/**
 * %ArrayIteratorPrototype% [ %Symbol.toStringTag% ], 23.1.5.2.2.
 * @value %ArrayIteratorPrototype%[Symbol.toStringTag]
 * @attributes configurable
 */
function ArrayIteratorPrototypeToStringTag(): Str {
  return "Array Iterator";
}
