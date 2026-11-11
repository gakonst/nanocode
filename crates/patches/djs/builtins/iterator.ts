// Iterator helpers (ECMA-262 27.1.3 and 27.1.2.1, ES2025) and Iterator.concat
// (iterator-sequencing). D1510: a helper object is an IteratorHelper object (heap
// class 28) whose generator-like state lives in slots, and each `next` runs one step of
// a hand-written state machine instead of a generator closure:
//
//   0 kind: 0 Iterator.from's wrapper, 1 map, 2 filter, 3 take, 4 drop, 5 flatMap,
//     6 concat, 7 chunks, 8 windows, 9 zip and zipKeyed
//   1 state: 0 suspended-start, 1 suspended-yield, 2 executing, 3 completed
//   2 the underlying iterator (concat: the current one), 3 its `next` method
//   4 the function (map, filter, flatMap), the remaining count (take, drop), the
//     iterables (concat: an array of [method, iterable] pairs), the size (chunks,
//     windows)
//   5 the counter (concat: the index of the next iterable; windows: allow-partial)
//   6 flatMap's inner iterator, 7 its `next` method; chunks and windows: 6 is true
//     once the underlying iterator is done (a last partial array was yielded), 7 the
//     window
//   zip: 2 the iterators (null once done in longest mode), 3 their `next` methods,
//   4 the padding, 5 the mode (0 shortest, 1 longest, 2 strict), 6 which iterators
//   are open (booleans), 7 zipKeyed's keys (undefined for zip)
//
// A step either yields (sets state 1 and returns the value) or returns with the state
// still executing, which completes the helper.
// @slim-omit: left out of the slim build (D2411).

/** The first slot of the per-kind state. */
function HelperState(o: Obj): i32 {
  return F64ToI32(AsNumber(RecordGet(o, 1)));
}

/** GetIteratorDirect ( obj ), 27.1.3.1.1 (ES2025 7.4.4): the `next` method. */
function GetIteratorDirectNext(o: Obj): Value {
  return Get(o, "next");
}

/**
 * IteratorStep on an iterator and its `next`, 7.4.9: the result object, or undefined
 * once it is done. The caller reads `value`.
 */
function IteratorStepResult(iterator: Obj, next: Value): Value {
  const result: Obj = IteratorNextResult(iterator, next);
  if (ToBoolean(Get(result, "done"))) {
    return undefined;
  }
  return result;
}

/**
 * GetIteratorFlattenable ( obj, primitiveHandling ), 7.4.5: the iterator; `strings`
 * selects iterate-string-primitives over reject-primitives.
 */
function GetIteratorFlattenable(obj: Value, strings: boolean): Obj {
  if (!IsObject(obj)) {
    if (!strings || !IsString(obj)) {
      throw NewTypeError("not an object");
    }
  }
  const method: Value = GetMethod(obj, WellKnownSymbol("iterator"));
  let iterator: Value = obj;
  if (method !== undefined) {
    iterator = Call(method, obj);
  }
  if (!IsObject(iterator)) {
    throw NewTypeError("not an object");
  }
  return AsObj(iterator);
}

/** The `this` of an Iterator.prototype method: an object (qjs's message). */
function ThisIteratorObject(v: Value): Obj {
  if (!IsObject(v)) {
    throw NewTypeError("not an object");
  }
  return AsObj(v);
}

/**
 * The callable argument of a helper or a consuming method: if it is not callable, the
 * receiver is closed (IteratorClose with a throw completion) and the TypeError thrown.
 */
function HelperCallable(o: Obj, f: Value): void {
  if (!IsCallable(f)) {
    const error: Obj = NewTypeError("not a function");
    IteratorCloseThrow(o);
    throw error;
  }
}

/**
 * The limit of take and drop, 27.1.4.4 and 27.1.4.12 steps 3-9: the receiver is
 * closed when the conversion throws or the limit is NaN, too large or negative.
 */
function HelperLimit(o: Obj, limit: Value): f64 {
  let numLimit: f64 = 0;
  try {
    numLimit = ToNumber(limit);
  } catch (e) {
    IteratorCloseThrow(o);
    throw e;
  }
  // NaN, or finite and above 2**53 - 1 (the 2025 spec).
  if (numLimit !== numLimit || (numLimit > 9007199254740991 && numLimit !== 1 / 0)) {
    const error: Obj = NewRangeError("must be positive");
    IteratorCloseThrow(o);
    throw error;
  }
  const integerLimit: f64 = ToIntegerOrInfinityNumber(numLimit);
  if (integerLimit < 0) {
    const error: Obj = NewRangeError("must be positive");
    IteratorCloseThrow(o);
    throw error;
  }
  return integerLimit;
}

/** A new helper of `kind` over iterator o (GetIteratorDirect) with slot 4 = data. */
function NewIteratorHelper(kind: i32, o: Obj, data: Value): Obj {
  const next: Value = GetIteratorDirectNext(o);
  const helper: Obj = CreateIteratorHelper(kind, o, next);
  RecordSet(helper, 4, data);
  RecordSet(helper, 5, 0);
  return helper;
}

/** Yield ( value ) of a step: suspends the helper at a yield. */
function HelperYield(o: Obj, value: Value): Value {
  RecordSet(o, 1, 1);
  return value;
}

/**
 * Calls the helper's function on (value, counter) and advances the counter; an
 * exception closes the underlying iterator (IfAbruptCloseIterator).
 */
function HelperCallFunction(o: Obj, iterator: Obj, value: Value): Value {
  const counter: f64 = AsNumber(RecordGet(o, 5));
  let result: Value = undefined;
  try {
    result = Call(RecordGet(o, 4), undefined, value, counter);
  } catch (e) {
    IteratorCloseThrow(iterator);
    throw e;
  }
  RecordSet(o, 5, counter + 1);
  return result;
}

/** One step of a helper (state executing): yields a value or completes. */
function IteratorHelperStep(o: Obj): Value {
  const kind: i32 = F64ToI32(AsNumber(RecordGet(o, 0)));
  if (kind === 6) {
    return ConcatStep(o);
  }
  if (kind === 9) {
    return ZipStep(o);
  }
  const iterator: Obj = AsObj(RecordGet(o, 2));
  const next: Value = RecordGet(o, 3);
  if (kind === 1) {
    const result: Value = IteratorStepResult(iterator, next);
    if (result === undefined) {
      return undefined;
    }
    const value: Value = Get(AsObj(result), "value");
    return HelperYield(o, HelperCallFunction(o, iterator, value));
  }
  if (kind === 2) {
    while (true) {
      const result: Value = IteratorStepResult(iterator, next);
      if (result === undefined) {
        return undefined;
      }
      const value: Value = Get(AsObj(result), "value");
      if (ToBoolean(HelperCallFunction(o, iterator, value))) {
        return HelperYield(o, value);
      }
    }
  }
  if (kind === 3) {
    const remaining: f64 = AsNumber(RecordGet(o, 4));
    if (remaining === 0) {
      IteratorClose(iterator);
      return undefined;
    }
    if (remaining !== 1 / 0) {
      RecordSet(o, 4, remaining - 1);
    }
    const result: Value = IteratorStepResult(iterator, next);
    if (result === undefined) {
      return undefined;
    }
    return HelperYield(o, Get(AsObj(result), "value"));
  }
  if (kind === 4) {
    let remaining: f64 = AsNumber(RecordGet(o, 4));
    while (remaining > 0) {
      if (remaining !== 1 / 0) {
        remaining = remaining - 1;
        RecordSet(o, 4, remaining);
      }
      if (IteratorStepResult(iterator, next) === undefined) {
        return undefined;
      }
    }
    const result: Value = IteratorStepResult(iterator, next);
    if (result === undefined) {
      return undefined;
    }
    return HelperYield(o, Get(AsObj(result), "value"));
  }
  if (kind === 5) {
    return FlatMapStep(o, iterator, next);
  }
  if (kind === 7) {
    return ChunksStep(o, iterator, next);
  }
  return WindowsStep(o, iterator, next);
}

/** A copy of the first n elements of the internal array a, as a new Array. */
function ArrayPrefixCopy(a: Obj, from: f64, n: f64): Obj {
  const copy: Obj = ArrayCreate(0);
  for (let i: f64 = 0; i < n; i++) {
    CreateDataPropertyOrThrow(copy, i, Get(a, from + i));
  }
  return copy;
}

/** A step of chunks (iterator-chunking): the next full chunk, or the partial last one. */
function ChunksStep(o: Obj, iterator: Obj, next: Value): Value {
  if (RecordGet(o, 6) === true) {
    return undefined;
  }
  const size: f64 = AsNumber(RecordGet(o, 4));
  const buffer: Obj = ArrayCreate(0);
  let n: f64 = 0;
  while (true) {
    const result: Value = IteratorStepResult(iterator, next);
    if (result === undefined) {
      if (n > 0) {
        RecordSet(o, 6, true);
        return HelperYield(o, buffer);
      }
      return undefined;
    }
    CreateDataPropertyOrThrow(buffer, n, Get(AsObj(result), "value"));
    n++;
    if (n === size) {
      return HelperYield(o, buffer);
    }
  }
}

/**
 * A step of windows (iterator-chunking): the next full window; with allow-partial, a
 * short last one if the iterator ended before the first window filled.
 */
function WindowsStep(o: Obj, iterator: Obj, next: Value): Value {
  if (RecordGet(o, 6) === true) {
    return undefined;
  }
  const size: f64 = AsNumber(RecordGet(o, 4));
  let window: Value = RecordGet(o, 7);
  if (window === undefined) {
    window = ArrayCreate(0);
    RecordSet(o, 7, window);
  }
  const w: Obj = AsObj(window);
  while (true) {
    let n: f64 = LengthOfArrayLike(w);
    const result: Value = IteratorStepResult(iterator, next);
    if (result === undefined) {
      if (RecordGet(o, 5) === true && n > 0 && n < size) {
        RecordSet(o, 6, true);
        return HelperYield(o, ArrayPrefixCopy(w, 0, n));
      }
      return undefined;
    }
    const value: Value = Get(AsObj(result), "value");
    if (n === size) {
      // Drop the oldest element: shift the window down by one.
      for (let i: f64 = 1; i < n; i++) {
        CreateDataPropertyOrThrow(w, i - 1, Get(w, i));
      }
      n = n - 1;
    }
    CreateDataPropertyOrThrow(w, n, value);
    Set(w, "length", n + 1);
    if (n + 1 === size) {
      return HelperYield(o, ArrayPrefixCopy(w, 0, size));
    }
  }
}

/**
 * IteratorCloseAll ( iters, completion ) (joint-iteration): closes the open iterators
 * of the list in reverse order, then completes with `error` if `isThrow` or if a
 * `return` threw (the first such error; later ones are ignored).
 */
function IteratorCloseAll(iters: Obj, open: Obj, error: Value, isThrow: boolean): void {
  let threw: boolean = isThrow;
  let err: Value = error;
  for (let i: f64 = LengthOfArrayLike(iters) - 1; i >= 0; i--) {
    if (Get(open, i) === true) {
      const iter: Obj = AsObj(Get(iters, i));
      if (threw) {
        IteratorCloseThrow(iter);
      } else {
        try {
          IteratorClose(iter);
        } catch (e) {
          threw = true;
          err = e;
        }
      }
    }
  }
  if (threw) {
    throw err;
  }
}

/** A step of IteratorZip (joint-iteration): the next results array or object. */
function ZipStep(o: Obj): Value {
  const iters: Obj = AsObj(RecordGet(o, 2));
  const nexts: Obj = AsObj(RecordGet(o, 3));
  const padding: Obj = AsObj(RecordGet(o, 4));
  const mode: i32 = F64ToI32(AsNumber(RecordGet(o, 5)));
  const open: Obj = AsObj(RecordGet(o, 6));
  const n: f64 = LengthOfArrayLike(iters);
  if (n === 0) {
    return undefined;
  }
  const results: Obj = ArrayCreate(0);
  for (let i: f64 = 0; i < n; i++) {
    const iter: Value = Get(iters, i);
    let value: Value = undefined;
    if (iter === null) {
      value = Get(padding, i);
    } else {
      let result: Value = undefined;
      try {
        result = IteratorStepResult(AsObj(iter), Get(nexts, i));
        if (result !== undefined) {
          value = Get(AsObj(result), "value");
        }
      } catch (e) {
        CreateDataPropertyOrThrow(open, i, false);
        IteratorCloseAll(iters, open, e, true);
      }
      if (result === undefined) {
        CreateDataPropertyOrThrow(open, i, false);
        if (mode === 0) {
          IteratorCloseAll(iters, open, undefined, false);
          return undefined;
        }
        if (mode === 2) {
          if (i !== 0) {
            IteratorCloseAll(iters, open, NewTypeError("iterators have different lengths"), true);
          }
          for (let k: f64 = 1; k < n; k++) {
            let rest: Value = undefined;
            try {
              rest = IteratorStepResult(AsObj(Get(iters, k)), Get(nexts, k));
            } catch (e) {
              CreateDataPropertyOrThrow(open, k, false);
              IteratorCloseAll(iters, open, e, true);
            }
            if (rest !== undefined) {
              IteratorCloseAll(iters, open, NewTypeError("iterators have different lengths"), true);
            }
            CreateDataPropertyOrThrow(open, k, false);
          }
          return undefined;
        }
        if (!ZipAnyOpen(open, n)) {
          return undefined;
        }
        CreateDataPropertyOrThrow(iters, i, null);
        value = Get(padding, i);
      }
    }
    CreateDataPropertyOrThrow(results, i, value);
  }
  const keys: Value = RecordGet(o, 7);
  if (keys === undefined) {
    return HelperYield(o, results);
  }
  const obj: Obj = ObjectCreate(null);
  for (let i: f64 = 0; i < n; i++) {
    CreateDataPropertyOrThrow(obj, Get(AsObj(keys), i), Get(results, i));
  }
  return HelperYield(o, obj);
}

/** Whether one of the n iterators of a zip is still open. */
function ZipAnyOpen(open: Obj, n: f64): boolean {
  for (let i: f64 = 0; i < n; i++) {
    if (Get(open, i) === true) {
      return true;
    }
  }
  return false;
}

/** A step of flatMap, 27.1.4.7: the inner iterator's next value, or the next mapping. */
function FlatMapStep(o: Obj, iterator: Obj, next: Value): Value {
  while (true) {
    const inner: Value = RecordGet(o, 6);
    if (inner !== undefined) {
      let result: Value = undefined;
      let value: Value = undefined;
      try {
        result = IteratorStepResult(AsObj(inner), RecordGet(o, 7));
        if (result !== undefined) {
          value = Get(AsObj(result), "value");
        }
      } catch (e) {
        IteratorCloseThrow(iterator);
        throw e;
      }
      if (result !== undefined) {
        return HelperYield(o, value);
      }
      RecordSet(o, 6, undefined);
      RecordSet(o, 7, undefined);
    }
    const outer: Value = IteratorStepResult(iterator, next);
    if (outer === undefined) {
      return undefined;
    }
    const mapped: Value = HelperCallFunction(o, iterator, Get(AsObj(outer), "value"));
    try {
      const innerIterator: Obj = GetIteratorFlattenable(mapped, false);
      const innerNext: Value = GetIteratorDirectNext(innerIterator);
      RecordSet(o, 6, innerIterator);
      RecordSet(o, 7, innerNext);
    } catch (e) {
      IteratorCloseThrow(iterator);
      throw e;
    }
  }
}

/** A step of Iterator.concat's closure, 27.1.3.2.3 (iterator-sequencing). */
function ConcatStep(o: Obj): Value {
  while (true) {
    const current: Value = RecordGet(o, 2);
    if (current !== undefined) {
      const result: Value = IteratorStepResult(AsObj(current), RecordGet(o, 3));
      if (result !== undefined) {
        return HelperYield(o, Get(AsObj(result), "value"));
      }
      RecordSet(o, 2, undefined);
      RecordSet(o, 3, undefined);
    }
    const iterables: Obj = AsObj(RecordGet(o, 4));
    const index: f64 = AsNumber(RecordGet(o, 5));
    if (index >= LengthOfArrayLike(iterables)) {
      return undefined;
    }
    RecordSet(o, 5, index + 1);
    const pair: Obj = AsObj(Get(iterables, index));
    const iter: Value = Call(Get(pair, 0), Get(pair, 1));
    if (!IsObject(iter)) {
      throw NewTypeError("not an object");
    }
    const next: Value = GetIteratorDirectNext(AsObj(iter));
    RecordSet(o, 2, iter);
    RecordSet(o, 3, next);
  }
}

/**
 * Resumes a suspended helper at its yield with a return completion: closes the
 * iterators the step was reading (flatMap: the inner one first, 27.1.4.7 step
 * 5.b.viii.4.b).
 */
function IteratorHelperReturnAtYield(o: Obj): void {
  const kind: i32 = F64ToI32(AsNumber(RecordGet(o, 0)));
  if (kind === 9) {
    IteratorCloseAll(AsObj(RecordGet(o, 2)), AsObj(RecordGet(o, 6)), undefined, false);
    return;
  }
  // The yield of a last partial chunk or window ignores the completion.
  if ((kind === 7 || kind === 8) && RecordGet(o, 6) === true) {
    return;
  }
  if (kind === 5) {
    const inner: Value = RecordGet(o, 6);
    if (inner !== undefined) {
      try {
        IteratorClose(AsObj(inner));
      } catch (e) {
        IteratorCloseThrow(AsObj(RecordGet(o, 2)));
        throw e;
      }
    }
  }
  IteratorClose(AsObj(RecordGet(o, 2)));
}

/**
 * %IteratorHelperPrototype%.next ( ), 27.1.2.1.1: GeneratorResume.
 * @builtin %IteratorHelperPrototype%.next
 */
function IteratorHelperPrototypeNext(this: Value): Obj {
  const o: Obj = ThisIteratorHelper(this, false);
  const state: i32 = HelperState(o);
  if (state === 2) {
    throw NewTypeError("cannot invoke a running iterator");
  }
  if (state === 3) {
    return CreateIterResultObject(undefined, true);
  }
  RecordSet(o, 1, 2);
  let value: Value = undefined;
  try {
    value = IteratorHelperStep(o);
  } catch (e) {
    RecordSet(o, 1, 3);
    throw e;
  }
  if (HelperState(o) === 1) {
    return CreateIterResultObject(value, false);
  }
  RecordSet(o, 1, 3);
  return CreateIterResultObject(undefined, true);
}

/**
 * %IteratorHelperPrototype%.return ( ), 27.1.2.1.2.
 * @builtin %IteratorHelperPrototype%.return
 */
function IteratorHelperPrototypeReturn(this: Value): Obj {
  const o: Obj = ThisIteratorHelper(this, false);
  const state: i32 = HelperState(o);
  if (state === 2) {
    throw NewTypeError("cannot invoke a running iterator");
  }
  if (state === 3) {
    return CreateIterResultObject(undefined, true);
  }
  if (state === 0) {
    RecordSet(o, 1, 3);
    if (F64ToI32(AsNumber(RecordGet(o, 0))) === 9) {
      IteratorCloseAll(AsObj(RecordGet(o, 2)), AsObj(RecordGet(o, 6)), undefined, false);
      return CreateIterResultObject(undefined, true);
    }
    // concat has no underlying iterator before it starts.
    const iterator: Value = RecordGet(o, 2);
    if (iterator !== undefined) {
      IteratorClose(AsObj(iterator));
    }
    return CreateIterResultObject(undefined, true);
  }
  RecordSet(o, 1, 2);
  try {
    IteratorHelperReturnAtYield(o);
  } finally {
    RecordSet(o, 1, 3);
  }
  return CreateIterResultObject(undefined, true);
}

/**
 * %IteratorHelperPrototype% [ %Symbol.toStringTag% ], 27.1.2.1.3.
 * @value %IteratorHelperPrototype%[Symbol.toStringTag]
 * @attributes configurable
 */
function IteratorHelperPrototypeToStringTag(): Str {
  return "Iterator Helper";
}

/**
 * Iterator ( ), 27.1.3.1: abstract.
 * @constructor Iterator
 */
function IteratorConstructor(): Obj {
  if (new.target === undefined) {
    throw NewTypeError("constructor requires 'new'");
  }
  if (new.target === RealmConstructor("Iterator")) {
    throw NewTypeError("abstract class not constructable");
  }
  return ObjectCreate(GetPrototypeFromConstructor(new.target, "IteratorPrototype"));
}

/**
 * Iterator.from ( O ), 27.1.3.2.2.
 * @builtin Iterator.from
 */
function IteratorFrom(o: Value): Obj {
  const iterator: Obj = GetIteratorFlattenable(o, true);
  const next: Value = GetIteratorDirectNext(iterator);
  if (OrdinaryHasInstance(RealmConstructor("Iterator"), iterator)) {
    return iterator;
  }
  return CreateIteratorHelper(0, iterator, next);
}

/**
 * %WrapForValidIteratorPrototype%.next ( ), 27.1.3.2.2.1.1.
 * @builtin %WrapForValidIteratorPrototype%.next
 */
function WrapForValidIteratorPrototypeNext(this: Value): Value {
  const o: Obj = ThisIteratorHelper(this, true);
  return Call(RecordGet(o, 3), RecordGet(o, 2));
}

/**
 * %WrapForValidIteratorPrototype%.return ( ), 27.1.3.2.2.1.2.
 * @builtin %WrapForValidIteratorPrototype%.return
 */
function WrapForValidIteratorPrototypeReturn(this: Value): Value {
  const o: Obj = ThisIteratorHelper(this, true);
  const iterator: Obj = AsObj(RecordGet(o, 2));
  const method: Value = GetMethod(iterator, "return");
  if (method === undefined) {
    return CreateIterResultObject(undefined, true);
  }
  return Call(method, iterator);
}

/**
 * Iterator.concat ( ...items ) (iterator-sequencing).
 * @builtin Iterator.concat
 */
function IteratorConcat(...items: Value[]): Obj {
  const n: f64 = LengthOfArrayLike(items);
  const iterables: Obj = ArrayCreate(0);
  for (let i: f64 = 0; i < n; i++) {
    const item: Value = Get(items, i);
    if (!IsObject(item)) {
      throw NewTypeError("not an object");
    }
    const method: Value = GetMethod(item, WellKnownSymbol("iterator"));
    if (method === undefined) {
      throw NewTypeError("not a function");
    }
    CreateDataPropertyOrThrow(iterables, i, CreateArrayFromPair(method, item));
  }
  const helper: Obj = CreateIteratorHelper(6, undefined, undefined);
  RecordSet(helper, 4, iterables);
  RecordSet(helper, 5, 0);
  return helper;
}

/**
 * Iterator.prototype.map ( mapper ), 27.1.4.8.
 * @builtin Iterator.prototype.map
 */
function IteratorPrototypeMap(this: Value, mapper: Value): Obj {
  const o: Obj = ThisIteratorObject(this);
  HelperCallable(o, mapper);
  return NewIteratorHelper(1, o, mapper);
}

/**
 * Iterator.prototype.filter ( predicate ), 27.1.4.5.
 * @builtin Iterator.prototype.filter
 */
function IteratorPrototypeFilter(this: Value, predicate: Value): Obj {
  const o: Obj = ThisIteratorObject(this);
  HelperCallable(o, predicate);
  return NewIteratorHelper(2, o, predicate);
}

/**
 * Iterator.prototype.take ( limit ), 27.1.4.12.
 * @builtin Iterator.prototype.take
 */
function IteratorPrototypeTake(this: Value, limit: Value): Obj {
  const o: Obj = ThisIteratorObject(this);
  return NewIteratorHelper(3, o, HelperLimit(o, limit));
}

/**
 * Iterator.prototype.drop ( limit ), 27.1.4.4.
 * @builtin Iterator.prototype.drop
 */
function IteratorPrototypeDrop(this: Value, limit: Value): Obj {
  const o: Obj = ThisIteratorObject(this);
  return NewIteratorHelper(4, o, HelperLimit(o, limit));
}

/**
 * Iterator.prototype.flatMap ( mapper ), 27.1.4.7.
 * @builtin Iterator.prototype.flatMap
 */
function IteratorPrototypeFlatMap(this: Value, mapper: Value): Obj {
  const o: Obj = ThisIteratorObject(this);
  HelperCallable(o, mapper);
  return NewIteratorHelper(5, o, mapper);
}

/**
 * Iterator.prototype.reduce ( reducer [ , initialValue ] ), 27.1.4.9.
 * @builtin Iterator.prototype.reduce
 * @length 1
 */
function IteratorPrototypeReduce(this: Value, reducer: Value, ...rest: Value[]): Value {
  const o: Obj = ThisIteratorObject(this);
  HelperCallable(o, reducer);
  const next: Value = GetIteratorDirectNext(o);
  let accumulator: Value = undefined;
  let counter: f64 = 0;
  if (LengthOfArrayLike(rest) === 0) {
    const first: Value = IteratorStepResult(o, next);
    if (first === undefined) {
      throw NewTypeError("empty iterator");
    }
    accumulator = Get(AsObj(first), "value");
    counter = 1;
  } else {
    accumulator = Get(rest, 0);
  }
  while (true) {
    const result: Value = IteratorStepResult(o, next);
    if (result === undefined) {
      return accumulator;
    }
    const value: Value = Get(AsObj(result), "value");
    try {
      accumulator = Call(reducer, undefined, accumulator, value, counter);
    } catch (e) {
      IteratorCloseThrow(o);
      throw e;
    }
    counter++;
  }
}

/**
 * Iterator.prototype.toArray ( ), 27.1.4.13.
 * @builtin Iterator.prototype.toArray
 */
function IteratorPrototypeToArray(this: Value): Obj {
  const o: Obj = ThisIteratorObject(this);
  const next: Value = GetIteratorDirectNext(o);
  const items: Obj = ArrayCreate(0);
  let n: f64 = 0;
  while (true) {
    const result: Value = IteratorStepResult(o, next);
    if (result === undefined) {
      return items;
    }
    CreateDataPropertyOrThrow(items, n, Get(AsObj(result), "value"));
    n++;
  }
}

/**
 * The loop of forEach, some, every and find (27.1.4.6, 27.1.4.11, 27.1.4.2, 27.1.4.3):
 * calls fn on each value; mode 0 forEach, 1 some, 2 every, 3 find decides when a
 * result stops the loop (which closes the iterator).
 */
function IteratorConsume(thisValue: Value, fn: Value, mode: i32): Value {
  const o: Obj = ThisIteratorObject(thisValue);
  HelperCallable(o, fn);
  const next: Value = GetIteratorDirectNext(o);
  let counter: f64 = 0;
  while (true) {
    const result: Value = IteratorStepResult(o, next);
    if (result === undefined) {
      if (mode === 1) {
        return false;
      }
      if (mode === 2) {
        return true;
      }
      return undefined;
    }
    const value: Value = Get(AsObj(result), "value");
    let r: Value = undefined;
    try {
      r = Call(fn, undefined, value, counter);
    } catch (e) {
      IteratorCloseThrow(o);
      throw e;
    }
    if (mode !== 0) {
      const b: boolean = ToBoolean(r);
      if (mode === 2 ? !b : b) {
        IteratorClose(o);
        if (mode === 3) {
          return value;
        }
        return mode === 1;
      }
    }
    counter++;
  }
}

/**
 * Iterator.prototype.forEach ( procedure ), 27.1.4.6.
 * @builtin Iterator.prototype.forEach
 */
function IteratorPrototypeForEach(this: Value, procedure: Value): Value {
  return IteratorConsume(this, procedure, 0);
}

/**
 * Iterator.prototype.some ( predicate ), 27.1.4.11.
 * @builtin Iterator.prototype.some
 */
function IteratorPrototypeSome(this: Value, predicate: Value): Value {
  return IteratorConsume(this, predicate, 1);
}

/**
 * Iterator.prototype.every ( predicate ), 27.1.4.2.
 * @builtin Iterator.prototype.every
 */
function IteratorPrototypeEvery(this: Value, predicate: Value): Value {
  return IteratorConsume(this, predicate, 2);
}

/**
 * Iterator.prototype.find ( predicate ), 27.1.4.3.
 * @builtin Iterator.prototype.find
 */
function IteratorPrototypeFind(this: Value, predicate: Value): Value {
  return IteratorConsume(this, predicate, 3);
}

/**
 * SetterThatIgnoresPrototypeProperties ( thisValue, home, p, v ), 27.1.4.1.1.
 */
function SetterThatIgnoresPrototypeProperties(thisValue: Value, home: Obj, p: Value, v: Value): void {
  if (!IsObject(thisValue)) {
    throw NewTypeError("not an object");
  }
  const o: Obj = AsObj(thisValue);
  if (o === home) {
    throw NewTypeError("Cannot assign to read only property");
  }
  if (OwnPropertyAttributes(o, p) < 0) {
    CreateDataPropertyOrThrow(o, p, v);
  } else {
    Set(o, p, v);
  }
}

/**
 * get Iterator.prototype.constructor, 27.1.4.1.
 * @getter Iterator.prototype.constructor
 */
function IteratorPrototypeConstructorGet(this: Value): Value {
  return RealmConstructor("Iterator");
}

/**
 * set Iterator.prototype.constructor, 27.1.4.1.
 * @setter Iterator.prototype.constructor
 */
function IteratorPrototypeConstructorSet(this: Value, v: Value): void {
  SetterThatIgnoresPrototypeProperties(this, RealmIntrinsic("IteratorPrototype"), "constructor", v);
}

/**
 * get Iterator.prototype [ %Symbol.toStringTag% ], 27.1.4.14.
 * @getter Iterator.prototype[Symbol.toStringTag]
 */
function IteratorPrototypeToStringTagGet(this: Value): Value {
  return "Iterator";
}

/**
 * set Iterator.prototype [ %Symbol.toStringTag% ], 27.1.4.14.
 * @setter Iterator.prototype[Symbol.toStringTag]
 */
function IteratorPrototypeToStringTagSet(this: Value, v: Value): void {
  SetterThatIgnoresPrototypeProperties(this, RealmIntrinsic("IteratorPrototype"), WellKnownSymbol("toStringTag"), v);
}

/**
 * The size argument of chunks and windows (iterator-chunking): an integral Number in
 * [1, 2**32 - 1], else the receiver is closed and a TypeError or RangeError thrown.
 */
function ChunkingSize(o: Obj, size: Value, name: Str): f64 {
  let error: Value = undefined;
  // Integral means finite too.
  if (!IsNumber(size) || ToIntegerOrInfinityNumber(AsNumber(size)) !== AsNumber(size) || AsNumber(size) - AsNumber(size) !== 0) {
    error = NewTypeError(name + " is not an integral number");
  } else if (AsNumber(size) < 1 || AsNumber(size) > 4294967295) {
    error = NewRangeError(name + " out of range");
  }
  if (error !== undefined) {
    IteratorCloseThrow(o);
    throw error;
  }
  return AsNumber(size);
}

/**
 * Iterator.prototype.chunks ( chunkSize ) (iterator-chunking).
 * @builtin Iterator.prototype.chunks
 */
function IteratorPrototypeChunks(this: Value, chunkSize: Value): Obj {
  const o: Obj = ThisIteratorObject(this);
  return NewIteratorHelper(7, o, ChunkingSize(o, chunkSize, "chunkSize"));
}

/**
 * Iterator.prototype.windows ( windowSize [ , undersized ] ) (iterator-chunking).
 * @builtin Iterator.prototype.windows
 * @length 1
 */
function IteratorPrototypeWindows(this: Value, windowSize: Value, undersized?: Value): Obj {
  const o: Obj = ThisIteratorObject(this);
  const size: f64 = ChunkingSize(o, windowSize, "windowSize");
  if (undersized !== undefined && undersized !== "only-full" && undersized !== "allow-partial") {
    const error: Obj = NewTypeError("bad undersized option");
    IteratorCloseThrow(o);
    throw error;
  }
  const helper: Obj = NewIteratorHelper(8, o, size);
  RecordSet(helper, 5, undersized === "allow-partial");
  return helper;
}

/**
 * Iterator.prototype.includes ( searchElement [ , skippedElements ] ) (iterator-includes).
 * @builtin Iterator.prototype.includes
 * @length 1
 */
function IteratorPrototypeIncludes(this: Value, searchElement: Value, skippedElements?: Value): boolean {
  const o: Obj = ThisIteratorObject(this);
  let toSkip: f64 = 0;
  if (skippedElements !== undefined) {
    let error: Value = undefined;
    if (!IsNumber(skippedElements)) {
      error = NewTypeError("not a number");
    } else {
      toSkip = AsNumber(skippedElements);
      if (toSkip !== toSkip || ToIntegerOrInfinityNumber(toSkip) !== toSkip) {
        error = NewTypeError("not an integral number");
      } else if (toSkip < 0 || (toSkip > 9007199254740991 && toSkip !== 1 / 0)) {
        error = NewRangeError("must be positive");
      }
    }
    if (error !== undefined) {
      IteratorCloseThrow(o);
      throw error;
    }
  }
  const next: Value = GetIteratorDirectNext(o);
  let skipped: f64 = 0;
  while (true) {
    const result: Value = IteratorStepResult(o, next);
    if (result === undefined) {
      return false;
    }
    const value: Value = Get(AsObj(result), "value");
    if (skipped < toSkip) {
      skipped++;
    } else if (SameValueZero(value, searchElement)) {
      IteratorClose(o);
      return true;
    }
  }
}

/**
 * Iterator.prototype.join ( separator ) (Iterator.prototype.join).
 * @builtin Iterator.prototype.join
 */
function IteratorPrototypeJoin(this: Value, separator: Value): Str {
  const o: Obj = ThisIteratorObject(this);
  let sep: Str = ",";
  if (separator !== undefined) {
    try {
      sep = ToString(separator);
    } catch (e) {
      IteratorCloseThrow(o);
      throw e;
    }
  }
  const next: Value = GetIteratorDirectNext(o);
  let r: Str = "";
  let first: boolean = true;
  while (true) {
    const result: Value = IteratorStepResult(o, next);
    if (result === undefined) {
      return r;
    }
    const value: Value = Get(AsObj(result), "value");
    if (!first) {
      r += sep;
    }
    first = false;
    if (value !== undefined && value !== null) {
      try {
        r += ToString(value);
      } catch (e) {
        IteratorCloseThrow(o);
        throw e;
      }
    }
  }
}

/**
 * The options of Iterator.zip and zipKeyed (joint-iteration): a Record [mode (0
 * shortest, 1 longest, 2 strict), padding option].
 */
function ZipOptions(options: Value): Obj {
  let opts: Obj = ObjectCreate(null);
  if (options !== undefined) {
    if (!IsObject(options)) {
      throw NewTypeError("options must be an object");
    }
    opts = AsObj(options);
  }
  const mode: Value = Get(opts, "mode");
  let m: i32 = 0;
  if (mode === "longest") {
    m = 1;
  } else if (mode === "strict") {
    m = 2;
  } else if (mode !== undefined && mode !== "shortest") {
    throw NewTypeError("invalid mode");
  }
  let padding: Value = undefined;
  if (m === 1) {
    padding = Get(opts, "padding");
    if (padding !== undefined && !IsObject(padding)) {
      throw NewTypeError("padding must be an object");
    }
  }
  const r: Obj = NewRecord(2);
  RecordSet(r, 0, m);
  RecordSet(r, 1, padding);
  return r;
}

/**
 * Appends the iterator of `value` (GetIteratorFlattenable, reject-primitives, and its
 * `next`) to the zip lists; on an exception, closes `outer` (if any) after the
 * iterators so far and rethrows.
 */
function ZipAddIterator(iters: Obj, nexts: Obj, open: Obj, value: Value, outer: Value): void {
  const n: f64 = LengthOfArrayLike(iters);
  try {
    const iter: Obj = GetIteratorFlattenable(value, false);
    const next: Value = GetIteratorDirectNext(iter);
    CreateDataPropertyOrThrow(iters, n, iter);
    CreateDataPropertyOrThrow(nexts, n, next);
    CreateDataPropertyOrThrow(open, n, true);
  } catch (e) {
    // IfAbruptCloseIterators over « outer » followed by the iterators: in reverse.
    try {
      IteratorCloseAll(iters, open, e, true);
    } finally {
      if (outer !== undefined) {
        IteratorCloseThrow(AsObj(outer));
      }
    }
  }
}

/** The helper of IteratorZip ( iters, mode, padding, finishResults ). */
function ZipCreate(iters: Obj, nexts: Obj, open: Obj, padding: Obj, mode: i32, keys: Value): Obj {
  const helper: Obj = CreateIteratorHelper(9, iters, nexts);
  RecordSet(helper, 4, padding);
  RecordSet(helper, 5, mode);
  RecordSet(helper, 6, open);
  RecordSet(helper, 7, keys);
  return helper;
}

/**
 * Iterator.zip ( iterables [ , options ] ) (joint-iteration).
 * @builtin Iterator.zip
 * @length 1
 */
function IteratorZip(iterables: Value, options?: Value): Obj {
  if (!IsObject(iterables)) {
    throw NewTypeError("not an object");
  }
  const opts: Obj = ZipOptions(options);
  const mode: i32 = F64ToI32(AsNumber(RecordGet(opts, 0)));
  const paddingOption: Value = RecordGet(opts, 1);
  const iters: Obj = ArrayCreate(0);
  const nexts: Obj = ArrayCreate(0);
  const open: Obj = ArrayCreate(0);
  const inputIter: Obj = GetIterator(iterables);
  const inputNext: Value = Get(inputIter, "next");
  while (true) {
    let result: Value = undefined;
    let value: Value = undefined;
    try {
      result = IteratorStepResult(inputIter, inputNext);
      if (result !== undefined) {
        value = Get(AsObj(result), "value");
      }
    } catch (e) {
      IteratorCloseAll(iters, open, e, true);
    }
    if (result === undefined) {
      break;
    }
    ZipAddIterator(iters, nexts, open, value, inputIter);
  }
  const n: f64 = LengthOfArrayLike(iters);
  const padding: Obj = ArrayCreate(0);
  if (mode === 1) {
    if (paddingOption === undefined) {
      for (let i: f64 = 0; i < n; i++) {
        CreateDataPropertyOrThrow(padding, i, undefined);
      }
    } else {
      let paddingIter: Obj = iters;
      let paddingNext: Value = undefined;
      try {
        paddingIter = GetIterator(paddingOption);
        paddingNext = Get(paddingIter, "next");
      } catch (e) {
        IteratorCloseAll(iters, open, e, true);
      }
      let usingIterator: boolean = true;
      for (let i: f64 = 0; i < n; i++) {
        let value: Value = undefined;
        if (usingIterator) {
          try {
            const result: Value = IteratorStepResult(paddingIter, paddingNext);
            if (result === undefined) {
              usingIterator = false;
            } else {
              value = Get(AsObj(result), "value");
            }
          } catch (e) {
            IteratorCloseAll(iters, open, e, true);
          }
        }
        CreateDataPropertyOrThrow(padding, i, value);
      }
      if (usingIterator) {
        try {
          IteratorClose(paddingIter);
        } catch (e) {
          IteratorCloseAll(iters, open, e, true);
        }
      }
    }
  }
  return ZipCreate(iters, nexts, open, padding, mode, undefined);
}

/**
 * Iterator.zipKeyed ( iterables [ , options ] ) (joint-iteration).
 * @builtin Iterator.zipKeyed
 * @length 1
 */
function IteratorZipKeyed(iterables: Value, options?: Value): Obj {
  if (!IsObject(iterables)) {
    throw NewTypeError("not an object");
  }
  const source: Obj = AsObj(iterables);
  const opts: Obj = ZipOptions(options);
  const mode: i32 = F64ToI32(AsNumber(RecordGet(opts, 0)));
  const paddingOption: Value = RecordGet(opts, 1);
  const iters: Obj = ArrayCreate(0);
  const nexts: Obj = ArrayCreate(0);
  const open: Obj = ArrayCreate(0);
  const keys: Obj = ArrayCreate(0);
  const allKeys: Obj = OwnPropertyKeys(source, 2);
  const count: f64 = LengthOfArrayLike(allKeys);
  for (let k: f64 = 0; k < count; k++) {
    const key: Value = Get(allKeys, k);
    let value: Value = undefined;
    try {
      const attrs: i32 = OwnPropertyAttributes(source, key);
      if (attrs >= 0 && (attrs & 2) !== 0) {
        value = Get(source, key);
      }
    } catch (e) {
      IteratorCloseAll(iters, open, e, true);
    }
    if (value !== undefined) {
      ZipAddIterator(iters, nexts, open, value, undefined);
      CreateDataPropertyOrThrow(keys, LengthOfArrayLike(keys), key);
    }
  }
  const n: f64 = LengthOfArrayLike(iters);
  const padding: Obj = ArrayCreate(0);
  if (mode === 1) {
    for (let i: f64 = 0; i < n; i++) {
      let value: Value = undefined;
      if (paddingOption !== undefined) {
        try {
          value = Get(AsObj(paddingOption), Get(keys, i));
        } catch (e) {
          IteratorCloseAll(iters, open, e, true);
        }
      }
      CreateDataPropertyOrThrow(padding, i, value);
    }
  }
  return ZipCreate(iters, nexts, open, padding, mode, keys);
}
