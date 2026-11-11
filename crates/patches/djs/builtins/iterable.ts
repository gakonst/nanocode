// Builtins over the iteration protocol (0034, on 0029's builtins/iter.ts): IterableToList,
// Array.from of iterables, Object.fromEntries and groupBy, AggregateError's errors, and
// the String iterator (D1346).

/** GetIteratorFromMethod ( obj, method ), 7.4.2: the iterator object. */
function GetIteratorFromMethod(obj: Value, method: Value): Obj {
  const iterator: Value = Call(method, obj);
  if (!IsObject(iterator)) {
    throw NewTypeError("not an object");
  }
  return AsObj(iterator);
}

/**
 * IteratorStep's result object, 7.4.8: `next()` must return an object (qjs's message).
 */
function IteratorNextResult(iterator: Obj, next: Value): Obj {
  const result: Value = Call(next, iterator);
  if (!IsObject(result)) {
    throw NewTypeError("iterator must return an object");
  }
  return AsObj(result);
}

/** IteratorToList ( GetIterator ( items ) ), 7.4.13 and 7.4.12. */
function IterableToList(items: Value): Obj {
  const iterator: Obj = GetIterator(items);
  const next: Value = Get(iterator, "next");
  const list: Obj = ArrayCreate(0);
  let n: f64 = 0;
  while (true) {
    // Without a result object where that is unobservable (D1800), keys and values runs
    // in one step (D2305).
    n = IteratorFastDrain(iterator, next, list, n);
    if (IteratorFastReady(iterator, next)) {
      CreateDataPropertyOrThrow(list, n, IteratorFastValue(iterator, next));
    } else {
      const result: Obj = IteratorNextResult(iterator, next);
      if (ToBoolean(Get(result, "done"))) {
        return list;
      }
      CreateDataPropertyOrThrow(list, n, Get(result, "value"));
    }
    n++;
  }
}

/**
 * Array.from's iterable path, 23.1.2.1 step 5: `a` is the new array, the iterator
 * is closed when mapping or storing throws.
 */
function ArrayFromIterator(a: Obj, items: Value, method: Value, mapfn: Value, thisArg: Value): Obj {
  const iterator: Obj = GetIteratorFromMethod(items, method);
  const next: Value = Get(iterator, "next");
  let k: f64 = 0;
  while (true) {
    if (k >= 9007199254740991) {
      const error: Obj = NewTypeError("Array loo long");
      IteratorCloseThrow(iterator);
      throw error;
    }
    if (mapfn === undefined) {
      // Keys and values runs in one step (D2305).
      k = IteratorFastDrain(iterator, next, a, k);
    }
    // Without a result object where that is unobservable (D1800).
    let nextValue: Value = undefined;
    if (IteratorFastReady(iterator, next)) {
      nextValue = IteratorFastValue(iterator, next);
    } else {
      const result: Obj = IteratorNextResult(iterator, next);
      if (ToBoolean(Get(result, "done"))) {
        Set(a, "length", k);
        return a;
      }
      nextValue = Get(result, "value");
    }
    try {
      const mappedValue: Value = mapfn === undefined ? nextValue : Call(mapfn, thisArg, nextValue, k);
      CreateDataPropertyOrThrow(a, k, mappedValue);
    } catch (e) {
      IteratorCloseThrow(iterator);
      throw e;
    }
    k++;
  }
}

/**
 * Object.fromEntries ( iterable ), 20.1.2.7.
 * @builtin Object.fromEntries
 */
function ObjectFromEntries(iterable: Value): Obj {
  // RequireObjectCoercible is GetIterator's TypeError on null and undefined (qjs's
  // message); nothing observable happens in between.
  const obj: Obj = ObjectCreate(RealmIntrinsic("ObjectPrototype"));
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
        return obj;
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
      CreateDataPropertyOrThrow(obj, ToPropertyKey(k), v);
    } catch (e) {
      IteratorCloseThrow(iterator);
      throw e;
    }
  }
}

/**
 * GroupBy ( items, callback, property ), 7.3.35, for property keys. Returns a
 * prototype-less object whose keys are the group keys in first-seen order, each
 * holding an array.
 */
function GroupByProperty(items: Value, callback: Value): Obj {
  // RequireObjectCoercible ( items ) is GetIterator's TypeError, as above.
  if (!IsCallable(callback)) {
    throw NewTypeError("not a function");
  }
  const groups: Obj = ObjectCreate(null);
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
        return groups;
      }
      value = Get(result, "value");
    }
    let key: Value = undefined;
    try {
      key = ToPropertyKey(Call(callback, undefined, value, k));
    } catch (e) {
      IteratorCloseThrow(iterator);
      throw e;
    }
    // The groups object has no prototype and only arrays as values, so these
    // lookups and stores cannot run user code.
    let group: Value = Get(groups, key);
    if (group === undefined) {
      group = ArrayCreate(0);
      CreateDataPropertyOrThrow(groups, key, group);
    }
    const g: Obj = AsObj(group);
    CreateDataPropertyOrThrow(g, LengthOfArrayLike(g), value);
    k++;
  }
}

/**
 * Object.groupBy ( items, callback ), 20.1.2.12.
 * @builtin Object.groupBy
 */
function ObjectGroupBy(items: Value, callback: Value): Obj {
  return GroupByProperty(items, callback);
}

/**
 * String.prototype [ %Symbol.iterator% ] ( ), 22.1.3.36.
 * @builtin String.prototype[Symbol.iterator]
 * @intrinsic
 */
function StringPrototypeIterator(this: Value): Obj {
  return CreateStringIterator(ThisToString(this));
}

/**
 * %StringIteratorPrototype%.next ( ), 22.1.5.1.1.
 * @builtin %StringIteratorPrototype%.next
 * @intrinsic
 */
function StringIteratorPrototypeNext(this: Value): Obj {
  const target: Value = StringIteratorTarget(this);
  if (target === undefined) {
    return CreateIterResultObject(undefined, true);
  }
  const s: Str = AsStr(target);
  const position: f64 = ArrayIteratorIndex(this);
  if (position >= StrLen(s)) {
    ArrayIteratorFinish(this);
    return CreateIterResultObject(undefined, true);
  }
  const cp: Str = StrCodePointString(s, position);
  ArrayIteratorSetIndex(this, position + StrLen(cp));
  return CreateIterResultObject(cp, false);
}

/**
 * %StringIteratorPrototype% [ %Symbol.toStringTag% ], 22.1.5.1.2.
 * @value %StringIteratorPrototype%[Symbol.toStringTag]
 * @attributes configurable
 */
function StringIteratorToStringTag(): Str {
  return "String Iterator";
}
