// Array.fromAsync (ES2026 23.1.2.2, 0051). The spec runs an abstract closure as an
// async function; the builtin language has no `await`, so the closure is a state
// machine over a Record (D1511) whose awaits subscribe two builtin closures (kinds 16
// and 17 of NewBuiltinClosure) that resume it:
//
//   0 the result promise, 1 mapfn (undefined: no mapping), 2 thisArg, 3 the iterator
//   (undefined for an array-like), 4 its `next`, 5 the array A, 6 k, 7 the phase the
//   pending await resumes, 8 the array-like, 9 its length, 10 the error that rejects
//   once the iterator is closed
//
// Phases: 1 awaiting `next()`'s result, 2 awaiting a mapped value, 3 awaiting the
// iterator's `return()` before rejecting, 4 awaiting an array-like element, 5 awaiting
// its mapped value.

/**
 * Array.fromAsync ( asyncItems [ , mapfn [ , thisArg ] ] ).
 * @builtin Array.fromAsync
 * @length 1
 */
function ArrayFromAsync(this: Value, asyncItems: Value, mapfn?: Value, thisArg?: Value): Obj {
  const promise: Obj = PromiseCreate(RealmIntrinsic("PromisePrototype"));
  try {
    FromAsyncStart(this, promise, asyncItems, mapfn, thisArg);
  } catch (e) {
    RejectPromise(promise, e);
  }
  return promise;
}

/** The closure up to its first await. */
function FromAsyncStart(c: Value, promise: Obj, asyncItems: Value, mapfn: Value, thisArg: Value): void {
  if (mapfn !== undefined && !IsCallable(mapfn)) {
    throw NewTypeError("mapfn is not callable");
  }
  const s: Obj = NewRecord(11);
  RecordSet(s, 0, promise);
  RecordSet(s, 1, mapfn);
  RecordSet(s, 2, thisArg);
  RecordSet(s, 6, 0);
  const usingAsyncIterator: Value = GetMethod(asyncItems, WellKnownSymbol("asyncIterator"));
  let usingSyncIterator: Value = undefined;
  if (usingAsyncIterator === undefined) {
    usingSyncIterator = GetMethod(asyncItems, WellKnownSymbol("iterator"));
  }
  if (usingAsyncIterator !== undefined || usingSyncIterator !== undefined) {
    let iterator: Obj = s;
    if (usingAsyncIterator !== undefined) {
      iterator = GetIteratorFromMethod(asyncItems, usingAsyncIterator);
    } else {
      const syncIterator: Obj = GetIteratorFromMethod(asyncItems, usingSyncIterator);
      iterator = CreateAsyncFromSyncIterator(syncIterator, Get(syncIterator, "next"));
    }
    RecordSet(s, 3, iterator);
    RecordSet(s, 4, Get(iterator, "next"));
    RecordSet(s, 5, IsConstructor(c) ? Construct(c) : ArrayCreate(0));
    FromAsyncIteratorNext(s);
    return;
  }
  const arrayLike: Obj = ToObject(asyncItems);
  const len: f64 = LengthOfArrayLike(arrayLike);
  RecordSet(s, 5, IsConstructor(c) ? Construct(c, len) : ArrayCreate(len));
  RecordSet(s, 8, arrayLike);
  RecordSet(s, 9, len);
  FromAsyncArrayLikeNext(s);
}

/** Await ( v ): resumes s at `phase` with the outcome. PromiseResolve may throw. */
function FromAsyncAwait(s: Obj, v: Value, phase: i32): void {
  RecordSet(s, 7, phase);
  const promise: Value = PromiseResolve(IntrinsicPromise(), v);
  PerformPromiseThen(AsObj(promise), NewBuiltinClosure(16, s, 1), NewBuiltinClosure(17, s, 1), undefined);
}

/** Rejects the result promise. */
function FromAsyncReject(s: Obj, e: Value): void {
  RejectPromise(AsObj(RecordGet(s, 0)), e);
}

/** Step iv.1-4 of the iterator loop: call `next` and await its result. */
function FromAsyncIteratorNext(s: Obj): void {
  try {
    if (AsNumber(RecordGet(s, 6)) >= 9007199254740991) {
      FromAsyncClose(s, NewTypeError("Array too long"));
      return;
    }
    const result: Value = Call(RecordGet(s, 4), RecordGet(s, 3));
    FromAsyncAwait(s, result, 1);
  } catch (e) {
    FromAsyncReject(s, e);
  }
}

/**
 * AsyncIteratorClose ( iteratorRecord, ThrowCompletion ( error ) ), 7.4.12: calls
 * `return` and awaits its result, ignoring errors, then rejects with error.
 */
function FromAsyncClose(s: Obj, error: Value): void {
  RecordSet(s, 10, error);
  try {
    const iterator: Obj = AsObj(RecordGet(s, 3));
    const method: Value = GetMethod(iterator, "return");
    if (method === undefined) {
      FromAsyncReject(s, error);
      return;
    }
    FromAsyncAwait(s, Call(method, iterator), 3);
  } catch (e) {
    FromAsyncReject(s, error);
  }
}

/** Steps iv.5-10 once `next()`'s result is in. */
function FromAsyncIteratorResult(s: Obj, nextResult: Value): void {
  let value: Value = undefined;
  try {
    if (!IsObject(nextResult)) {
      throw NewTypeError("iterator must return an object");
    }
    if (ToBoolean(Get(AsObj(nextResult), "done"))) {
      Set(AsObj(RecordGet(s, 5)), "length", RecordGet(s, 6));
      ResolvePromise(AsObj(RecordGet(s, 0)), RecordGet(s, 5));
      return;
    }
    value = Get(AsObj(nextResult), "value");
  } catch (e) {
    FromAsyncReject(s, e);
    return;
  }
  const mapfn: Value = RecordGet(s, 1);
  if (mapfn === undefined) {
    FromAsyncIteratorStore(s, value);
    return;
  }
  try {
    const mapped: Value = Call(mapfn, RecordGet(s, 2), value, RecordGet(s, 6));
    FromAsyncAwait(s, mapped, 2);
  } catch (e) {
    FromAsyncClose(s, e);
  }
}

/** Steps iv.11-13: store the (mapped) value, then the next iteration. */
function FromAsyncIteratorStore(s: Obj, value: Value): void {
  const k: f64 = AsNumber(RecordGet(s, 6));
  try {
    CreateDataPropertyOrThrow(AsObj(RecordGet(s, 5)), k, value);
  } catch (e) {
    FromAsyncClose(s, e);
    return;
  }
  RecordSet(s, 6, k + 1);
  FromAsyncIteratorNext(s);
}

/** The array-like loop (step 3.i.vii): await the next element, or finish. */
function FromAsyncArrayLikeNext(s: Obj): void {
  try {
    const k: f64 = AsNumber(RecordGet(s, 6));
    const len: f64 = AsNumber(RecordGet(s, 9));
    if (k < len) {
      FromAsyncAwait(s, Get(AsObj(RecordGet(s, 8)), k), 4);
      return;
    }
    Set(AsObj(RecordGet(s, 5)), "length", len);
    ResolvePromise(AsObj(RecordGet(s, 0)), RecordGet(s, 5));
  } catch (e) {
    FromAsyncReject(s, e);
  }
}

/** An array-like element (phase 4) or its mapped value (phase 5) is in. */
function FromAsyncArrayLikeValue(s: Obj, value: Value, phase: i32): void {
  try {
    const k: f64 = AsNumber(RecordGet(s, 6));
    const mapfn: Value = RecordGet(s, 1);
    if (phase === 4 && mapfn !== undefined) {
      FromAsyncAwait(s, Call(mapfn, RecordGet(s, 2), value, k), 5);
      return;
    }
    CreateDataPropertyOrThrow(AsObj(RecordGet(s, 5)), k, value);
    RecordSet(s, 6, k + 1);
  } catch (e) {
    FromAsyncReject(s, e);
    return;
  }
  FromAsyncArrayLikeNext(s);
}

/**
 * Resumes Array.fromAsync's closure after a fulfilled await: `this` is its Record.
 * @intrinsic
 */
function FromAsyncFulfilled(this: Value, value: Value): void {
  const s: Obj = AsObj(this);
  const phase: i32 = F64ToI32(AsNumber(RecordGet(s, 7)));
  if (phase === 1) {
    FromAsyncIteratorResult(s, value);
  } else if (phase === 2) {
    FromAsyncIteratorStore(s, value);
  } else if (phase === 3) {
    FromAsyncReject(s, RecordGet(s, 10));
  } else {
    FromAsyncArrayLikeValue(s, value, phase);
  }
}

/**
 * Resumes Array.fromAsync's closure after a rejected await: `this` is its Record. A
 * rejected mapped value closes the iterator (IfAbruptCloseAsyncIterator).
 * @intrinsic
 */
function FromAsyncRejected(this: Value, reason: Value): void {
  const s: Obj = AsObj(this);
  const phase: i32 = F64ToI32(AsNumber(RecordGet(s, 7)));
  if (phase === 2) {
    FromAsyncClose(s, reason);
  } else if (phase === 3) {
    FromAsyncReject(s, RecordGet(s, 10));
  } else {
    FromAsyncReject(s, reason);
  }
}
