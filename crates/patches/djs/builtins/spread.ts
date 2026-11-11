// Spread over a general iterable: ArrayAccumulation of a SpreadElement (ECMA-262
// 13.2.4.1). The interpreter's `array_spread` does strings and arrays with unmodified
// iteration itself and calls %ArraySpread for everything else.

/**
 * Appends the values of iterating src to the array literal arr; returns arr.
 * @intrinsic
 */
function ArraySpread(arr: Obj, src: Value): Obj {
  const iterator: Obj = GetIterator(src);
  const next: Value = Get(iterator, "next");
  let n: f64 = LengthOfArrayLike(arr);
  while (true) {
    // Without a result object where that is unobservable (D1800): arrays with holes,
    // typed arrays, `arr.entries()`; keys and values runs in one step (D2305).
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
 * CopyDataProperties ( target, source, excludedItems ), 7.3.25, without excluded
 * items: object spread (`{...src}`). The interpreter's `copy_data_props` copies plain
 * data properties itself and calls this for sources with accessors. Returns target.
 * @intrinsic
 */
function CopyDataProperties(target: Obj, src: Value): Obj {
  if (src === undefined || src === null) {
    return target;
  }
  const from: Obj = ToObject(src);
  const keys: Obj = OwnPropertyKeys(from, 2);
  const count: f64 = LengthOfArrayLike(keys);
  for (let i: f64 = 0; i < count; i++) {
    const key: Value = Get(keys, i);
    const attrs: i32 = OwnPropertyAttributes(from, key);
    if (attrs >= 0 && (attrs & 2) !== 0) {
      CreateDataPropertyOrThrow(target, key, Get(from, key));
    }
  }
  return target;
}

/**
 * Object rest in destructuring (`{a, ...rest} = src`): a new object with the
 * enumerable own properties of src except the keys in the array excluded (strings,
 * symbols or numbers, as ToPropertyKey returns them). The caller did
 * RequireObjectCoercible(src).
 * @intrinsic
 */
function CopyDataPropertiesExcluding(src: Value, excluded: Obj): Obj {
  const target: Obj = ObjectCreate(RealmIntrinsic("ObjectPrototype"));
  const from: Obj = ToObject(src);
  const keys: Obj = OwnPropertyKeys(from, 2);
  const count: f64 = LengthOfArrayLike(keys);
  const n: f64 = LengthOfArrayLike(excluded);
  for (let i: f64 = 0; i < count; i++) {
    const key: Value = Get(keys, i);
    let skip: boolean = false;
    for (let j: f64 = 0; j < n; j++) {
      let e: Value = Get(excluded, j);
      if (IsNumber(e)) {
        e = ToStringPrimitive(e);
      }
      if (SameValue(e, key)) {
        skip = true;
        break;
      }
    }
    if (!skip) {
      const attrs: i32 = OwnPropertyAttributes(from, key);
      if (attrs >= 0 && (attrs & 2) !== 0) {
        CreateDataPropertyOrThrow(target, key, Get(from, key));
      }
    }
  }
  return target;
}
