// Promise objects (ECMA-262 27.2), on the leaf intrinsics of interp/run/promise.rs
// (D1371): the Rust side keeps the slots, the reactions and the job queue; everything
// that may run user code (thenables, `then` lookups, species, iteration) is here.
//
// Capabilities (27.2.1.1) are undefined (nothing to resolve), a Promise (an internal
// capability of %Promise%: its resolving functions are never visible, so the promise is
// resolved directly, with PromiseClaimResolve as [[AlreadyResolved]]), or a Record
// [promise, resolve, reject] for any other constructor. Resolving functions and the
// other anonymous builtin functions are builtin closures (D1372) whose `this` is their
// data: a Promise (the constructor's pair: [[AlreadyResolved]] on the promise), or a
// Record.

// ---- Resolving and capabilities ---------------------------------------------------------

/**
 * The resolve steps of a promise resolve function, 27.2.1.3.2 steps 7-16, after the
 * [[AlreadyResolved]] check.
 * @intrinsic
 */
function ResolvePromise(promise: Obj, resolution: Value): void {
  if (resolution === promise) {
    RejectPromise(promise, NewTypeError("promise self resolution"));
    return;
  }
  if (!IsObject(resolution)) {
    FulfillPromise(promise, resolution);
    return;
  }
  let then: Value = undefined;
  try {
    then = Get(AsObj(resolution), "then");
  } catch (e) {
    RejectPromise(promise, e);
    return;
  }
  if (!IsCallable(then)) {
    FulfillPromise(promise, resolution);
    return;
  }
  EnqueuePromiseResolveThenableJob(promise, AsObj(resolution), then);
}

/**
 * The promise of a resolving function's data (a Promise, or a Record [promise,
 * alreadyResolved]) if it has not resolved yet (and marks it resolved), else undefined.
 */
function ClaimResolution(data: Value): Value {
  if (IsPromise(data)) {
    return PromiseClaimResolve(AsObj(data)) ? data : undefined;
  }
  const r: Obj = AsObj(data);
  if (RecordGet(r, 1) === true) {
    return undefined;
  }
  RecordSet(r, 1, true);
  return RecordGet(r, 0);
}

/**
 * Promise Resolve Functions, 27.2.1.3.2.
 * @intrinsic
 * @fast
 */
function PromiseResolveFunction(this: Value, resolution: Value): void {
  ResolveWithData(this, resolution);
}

/**
 * A resolve function with `data` (see ClaimResolution) called with resolution.
 * @fast
 */
function ResolveWithData(data: Value, resolution: Value): void {
  const promise: Value = ClaimResolution(data);
  if (promise !== undefined) {
    ResolvePromise(AsObj(promise), resolution);
  }
}

/**
 * A reject function with `data` called with reason.
 * @fast
 */
function RejectWithData(data: Value, reason: Value): void {
  const promise: Value = ClaimResolution(data);
  if (promise !== undefined) {
    RejectPromise(AsObj(promise), reason);
  }
}

/**
 * Promise Reject Functions, 27.2.1.3.1.
 * @intrinsic
 * @fast
 */
function PromiseRejectFunction(this: Value, reason: Value): void {
  RejectWithData(this, reason);
}

/** The promise of a capability. */
function CapabilityPromise(capability: Value): Value {
  if (IsPromise(capability)) {
    return capability;
  }
  return RecordGet(AsObj(capability), 0);
}

/** Calls capability.[[Resolve]] with v. */
function ResolveCapability(capability: Value, v: Value): void {
  if (IsPromise(capability)) {
    ResolveWithData(capability, v);
    return;
  }
  Call(RecordGet(AsObj(capability), 1), undefined, v);
}

/** Calls capability.[[Reject]] with reason. */
function RejectCapability(capability: Value, reason: Value): void {
  if (IsPromise(capability)) {
    RejectWithData(capability, reason);
    return;
  }
  Call(RecordGet(AsObj(capability), 2), undefined, reason);
}

/** capability.[[Resolve]] as a function (made for an internal capability). */
function CapabilityResolveFunction(capability: Value): Value {
  if (IsPromise(capability)) {
    return NewBuiltinClosure(0, capability, 1);
  }
  return RecordGet(AsObj(capability), 1);
}

/** capability.[[Reject]] as a function (made for an internal capability). */
function CapabilityRejectFunction(capability: Value): Value {
  if (IsPromise(capability)) {
    return NewBuiltinClosure(1, capability, 1);
  }
  return RecordGet(AsObj(capability), 2);
}

/**
 * GetCapabilitiesExecutor Functions, 27.2.1.5.1: `this` is the capability Record.
 * @intrinsic
 */
function GetCapabilitiesExecutor(this: Value, resolve: Value, reject: Value): void {
  const r: Obj = AsObj(this);
  if (RecordGet(r, 1) !== undefined || RecordGet(r, 2) !== undefined) {
    throw NewTypeError("resolving function already set");
  }
  RecordSet(r, 1, resolve);
  RecordSet(r, 2, reject);
}

/** NewPromiseCapability ( C ), 27.2.1.5. */
function NewPromiseCapability(c: Value): Value {
  if (c === IntrinsicPromise()) {
    // Unobservable: %Promise%.prototype is fixed and the executor is ours.
    return PromiseCreate(RealmIntrinsic("PromisePrototype"));
  }
  if (!IsConstructor(c)) {
    throw NewTypeError("not a constructor");
  }
  const r: Obj = NewRecord(3);
  const executor: Fn = NewBuiltinClosure(2, r, 2);
  const promise: Value = Construct(c, executor);
  if (!IsCallable(RecordGet(r, 1)) || !IsCallable(RecordGet(r, 2))) {
    throw NewTypeError("not a function");
  }
  RecordSet(r, 0, promise);
  return r;
}

/** PromiseResolve ( C, x ), 27.2.4.7.1. */
function PromiseResolve(c: Value, x: Value): Value {
  if (IsPromise(x)) {
    const xConstructor: Value = Get(AsObj(x), "constructor");
    if (xConstructor === c) {
      return x;
    }
  }
  const capability: Value = NewPromiseCapability(c);
  ResolveCapability(capability, x);
  return CapabilityPromise(capability);
}

/** SpeciesConstructor ( O, defaultConstructor ), 7.3.22. */
function SpeciesConstructor(o: Obj, defaultConstructor: Value): Value {
  const c: Value = Get(o, "constructor");
  if (c === undefined) {
    return defaultConstructor;
  }
  if (!IsObject(c)) {
    throw NewTypeError("not an object");
  }
  const s: Value = Get(AsObj(c), WellKnownSymbol("species"));
  if (s === undefined || s === null) {
    return defaultConstructor;
  }
  if (IsConstructor(s)) {
    return s;
  }
  throw NewTypeError("not a constructor");
}

/** Invoke ( V, "then", args ), 7.3.21. */
function InvokeThen(v: Value, onFulfilled: Value, onRejected: Value): Value {
  return Call(GetV(v, "then"), v, onFulfilled, onRejected);
}

// ---- Jobs -------------------------------------------------------------------------------

/**
 * NewPromiseReactionJob ( reaction, argument ), 27.2.2.1: the job. `handler` undefined is
 * an empty handler; an async function state is an await to resume.
 * @intrinsic
 */
function PromiseReactionJob(capability: Value, handler: Value, argument: Value, fulfilled: boolean): void {
  // An await (D1375): the handler is the async function's state; resume it.
  if (IsAsyncFunctionState(handler)) {
    GeneratorResume(AsObj(handler), argument, fulfilled ? 0 : 1);
    return;
  }
  if (AsyncGeneratorState(handler) >= 0) {
    AsyncGeneratorStep(AsObj(handler), argument, fulfilled ? 0 : 1);
    return;
  }
  let ok: boolean = fulfilled;
  let result: Value = argument;
  if (handler !== undefined) {
    try {
      result = Call(handler, undefined, argument);
      ok = true;
    } catch (e) {
      result = e;
      ok = false;
    }
  }
  if (capability === undefined) {
    return;
  }
  if (ok) {
    ResolveCapability(capability, result);
  } else {
    RejectCapability(capability, result);
  }
}

/**
 * NewPromiseResolveThenableJob ( promiseToResolve, thenable, then ), 27.2.2.2: the job,
 * with a fresh pair of resolving functions.
 * @intrinsic
 */
function PromiseResolveThenableJob(promise: Obj, thenable: Obj, then: Value): void {
  const r: Obj = NewRecord(2);
  RecordSet(r, 0, promise);
  RecordSet(r, 1, false);
  const resolve: Fn = NewBuiltinClosure(0, r, 1);
  const reject: Fn = NewBuiltinClosure(1, r, 1);
  try {
    Call(then, thenable, resolve, reject);
  } catch (e) {
    Call(reject, undefined, e);
  }
}

// ---- The constructor --------------------------------------------------------------------

/**
 * Promise ( executor ), 27.2.3.1.
 * @constructor Promise
 * @intrinsic
 */
function PromiseConstructor(executor: Value): Obj {
  if (new.target === undefined) {
    throw NewTypeError("must be called with new");
  }
  if (!IsCallable(executor)) {
    throw NewTypeError("not a function");
  }
  const promise: Obj = PromiseCreate(GetPrototypeFromConstructor(new.target, "PromisePrototype"));
  const resolve: Fn = NewBuiltinClosure(0, promise, 1);
  const reject: Fn = NewBuiltinClosure(1, promise, 1);
  try {
    Call(executor, undefined, resolve, reject);
  } catch (e) {
    Call(reject, undefined, e);
  }
  return promise;
}

/**
 * get Promise [ %Symbol.species% ], 27.2.4.9.
 * @getter Promise[Symbol.species]
 * @intrinsic
 */
function PromiseSpecies(this: Value): Value {
  return this;
}

/** The `this` of the static methods: qjs requires an object first. */
function PromiseThisConstructor(c: Value): Value {
  if (!IsObject(c)) {
    throw NewTypeError("not an object");
  }
  return c;
}

/**
 * Promise.resolve ( x ), 27.2.4.7.
 * @builtin Promise.resolve
 * @intrinsic
 * @fast
 */
function PromiseStaticResolve(this: Value, x: Value): Value {
  return PromiseResolve(PromiseThisConstructor(this), x);
}

/**
 * Promise.reject ( r ), 27.2.4.6.
 * @builtin Promise.reject
 */
function PromiseStaticReject(this: Value, r: Value): Value {
  const capability: Value = NewPromiseCapability(PromiseThisConstructor(this));
  RejectCapability(capability, r);
  return CapabilityPromise(capability);
}

// ---- The combinators --------------------------------------------------------------------
//
// Element functions get a Record [alreadyCalled, index, values, capability, remaining]
// (allSettled's two functions share one); `remaining` is a Record [count].

/** GetPromiseResolve ( promiseConstructor ), 27.2.4.1.1. */
function GetPromiseResolve(c: Value): Value {
  const promiseResolve: Value = Get(AsObj(c), "resolve");
  if (!IsCallable(promiseResolve)) {
    throw NewTypeError("not a function");
  }
  return promiseResolve;
}

/** remainingElementsCount.[[Value]] - 1, stored; returns the new count. */
function DecrementRemaining(remaining: Obj): f64 {
  const n: f64 = AsNumber(RecordGet(remaining, 0)) - 1;
  RecordSet(remaining, 0, n);
  return n;
}

/**
 * The Record of an element function: 0 called, 1 index, 2 values, 3 capability, 4 the
 * remaining count, 5 the keys of a keyed combinator (D1972) or undefined.
 */
function NewElementRecord(index: f64, values: Obj, capability: Value, remaining: Obj): Obj {
  const r: Obj = NewRecord(6);
  RecordSet(r, 0, false);
  RecordSet(r, 1, index);
  RecordSet(r, 2, values);
  RecordSet(r, 3, capability);
  RecordSet(r, 4, remaining);
  return r;
}

/**
 * The element step of an element function: stores x at the index unless the function
 * (or its allSettled sibling) already ran; resolves (or for `any`, rejects) the
 * capability once all elements are in.
 */
function SettleElement(data: Value, x: Value, reject: boolean): void {
  const r: Obj = AsObj(data);
  if (RecordGet(r, 0) === true) {
    return;
  }
  RecordSet(r, 0, true);
  const values: Obj = AsObj(RecordGet(r, 2));
  CreateDataPropertyOrThrow(values, RecordGet(r, 1), x);
  if (DecrementRemaining(AsObj(RecordGet(r, 4))) === 0) {
    const capability: Value = RecordGet(r, 3);
    const keys: Value = RecordGet(r, 5);
    if (reject) {
      RejectCapability(capability, NewAggregateError(values));
    } else if (keys !== undefined) {
      ResolveCapability(capability, KeyedResult(AsObj(keys), values));
    } else {
      ResolveCapability(capability, values);
    }
  }
}

/** CreateKeyedPromiseCombinatorResultObject ( keys, values ): a null-prototype object. */
function KeyedResult(keys: Obj, values: Obj): Obj {
  const o: Obj = ObjectCreate(null);
  const n: f64 = LengthOfArrayLike(keys);
  for (let i: f64 = 0; i < n; i++) {
    CreateDataPropertyOrThrow(o, Get(keys, i), Get(values, i));
  }
  return o;
}

/**
 * PerformPromiseKeyed: Promise.allKeyed (kind 0) and allSettledKeyed (1) of the
 * await-dictionary proposal (D1972). Each own enumerable property (strings and symbols,
 * in [[OwnPropertyKeys]] order) is resolved with C.resolve and subscribed like an
 * element of Promise.all / allSettled; the result is a null-prototype object with the
 * same keys. An abrupt completion rejects the capability.
 */
function PerformPromiseKeyed(thisValue: Value, promises: Value, kind: i32): Value {
  const c: Value = PromiseThisConstructor(thisValue);
  const capability: Value = NewPromiseCapability(c);
  try {
    const promiseResolve: Value = GetPromiseResolve(c);
    if (!IsObject(promises)) {
      throw NewTypeError("not an object");
    }
    const o: Obj = AsObj(promises);
    const allKeys: Obj = OwnPropertyKeys(o, 2);
    const count: f64 = LengthOfArrayLike(allKeys);
    const keys: Obj = ArrayCreate(0);
    const values: Obj = ArrayCreate(0);
    const remaining: Obj = NewRecord(1);
    RecordSet(remaining, 0, 1);
    const rejectFn: Value = kind === 0 ? CapabilityRejectFunction(capability) : undefined;
    let index: f64 = 0;
    for (let k: f64 = 0; k < count; k++) {
      const key: Value = Get(allKeys, k);
      const attrs: i32 = OwnPropertyAttributes(o, key);
      if (attrs >= 0 && (attrs & 2) !== 0) {
        const value: Value = Get(o, key);
        CreateDataPropertyOrThrow(keys, index, key);
        CreateDataPropertyOrThrow(values, index, undefined);
        const nextPromise: Value = Call(promiseResolve, c, value);
        const data: Obj = NewElementRecord(index, values, capability, remaining);
        RecordSet(data, 5, keys);
        RecordSet(remaining, 0, AsNumber(RecordGet(remaining, 0)) + 1);
        if (kind === 0) {
          InvokeThen(nextPromise, NewBuiltinClosure(3, data, 1), rejectFn);
        } else {
          InvokeThen(nextPromise, NewBuiltinClosure(4, data, 1), NewBuiltinClosure(5, data, 1));
        }
        index++;
      }
    }
    if (DecrementRemaining(remaining) === 0) {
      ResolveCapability(capability, KeyedResult(keys, values));
    }
  } catch (e) {
    RejectCapability(capability, e);
  }
  return CapabilityPromise(capability);
}

/**
 * Promise.allKeyed ( promises ) (the await-dictionary proposal, D1972).
 * @builtin Promise.allKeyed
 */
function PromiseAllKeyed(this: Value, promises: Value): Value {
  return PerformPromiseKeyed(this, promises, 0);
}

/**
 * Promise.allSettledKeyed ( promises ) (the await-dictionary proposal, D1972).
 * @builtin Promise.allSettledKeyed
 */
function PromiseAllSettledKeyed(this: Value, promises: Value): Value {
  return PerformPromiseKeyed(this, promises, 1);
}

/**
 * Promise.all Resolve Element Functions, 27.2.4.1.3.
 * @intrinsic
 */
function PromiseAllResolveElement(this: Value, x: Value): void {
  SettleElement(this, x, false);
}

/** An allSettled result object { status, value | reason }. */
function SettledRecord(fulfilled: boolean, x: Value): Obj {
  const o: Obj = ObjectCreate(RealmIntrinsic("ObjectPrototype"));
  CreateDataPropertyOrThrow(o, "status", fulfilled ? "fulfilled" : "rejected");
  CreateDataPropertyOrThrow(o, fulfilled ? "value" : "reason", x);
  return o;
}

/**
 * Promise.allSettled Resolve Element Functions, 27.2.4.2.2.
 * @intrinsic
 */
function PromiseAllSettledResolveElement(this: Value, x: Value): void {
  if (RecordGet(AsObj(this), 0) === true) {
    return;
  }
  SettleElement(this, SettledRecord(true, x), false);
}

/**
 * Promise.allSettled Reject Element Functions, 27.2.4.2.3.
 * @intrinsic
 */
function PromiseAllSettledRejectElement(this: Value, x: Value): void {
  if (RecordGet(AsObj(this), 0) === true) {
    return;
  }
  SettleElement(this, SettledRecord(false, x), false);
}

/**
 * Promise.any Reject Element Functions, 27.2.4.3.2.
 * @intrinsic
 */
function PromiseAnyRejectElement(this: Value, x: Value): void {
  SettleElement(this, x, true);
}

/** The AggregateError of Promise.any, with `errors` (qjs: an empty message). */
function NewAggregateError(errors: Obj): Obj {
  const e: Obj = ErrorCreate(RealmIntrinsic("AggregateErrorPrototype"));
  DefineDataPropertyOrThrow(e, "errors", CreateArrayFromList(errors), 5);
  CaptureStack(e);
  return e;
}

/** CreateArrayFromList of an internal list (a copy). */
function CreateArrayFromList(list: Obj): Obj {
  const len: f64 = LengthOfArrayLike(list);
  const a: Obj = ArrayCreate(0);
  for (let k: f64 = 0; k < len; k++) {
    CreateDataPropertyOrThrow(a, k, Get(list, k));
  }
  return a;
}

/**
 * The shared loop of Promise.all (kind 0), allSettled (1), any (2) and race (3),
 * 27.2.4.1.2 and the like: iterates, resolves each value with C.resolve and subscribes.
 * An abrupt completion rejects the capability, after closing the iterator unless the
 * iterator itself failed.
 */
function PerformPromiseCombinator(thisValue: Value, iterable: Value, kind: i32): Value {
  const c: Value = PromiseThisConstructor(thisValue);
  const capability: Value = NewPromiseCapability(c);
  let promiseResolve: Value = undefined;
  let iterator: Value = undefined;
  let next: Value = undefined;
  try {
    promiseResolve = GetPromiseResolve(c);
    iterator = GetIterator(iterable);
    next = Get(AsObj(iterator), "next");
  } catch (e) {
    RejectCapability(capability, e);
    return CapabilityPromise(capability);
  }
  const it: Obj = AsObj(iterator);
  // iteratorRecord.[[Done]]: the iterator's own failures (next, done, value) and
  // anything after it finished do not close it.
  let iteratorDone: boolean = false;
  try {
    const values: Obj = ArrayCreate(0);
    const remaining: Obj = NewRecord(1);
    RecordSet(remaining, 0, 1);
    let index: f64 = 0;
    // Race passes the capability's functions; the others pass reject (any: resolve).
    let resolveFn: Value = undefined;
    let rejectFn: Value = undefined;
    if (kind === 3 || kind === 2) {
      resolveFn = CapabilityResolveFunction(capability);
    }
    if (kind !== 2) {
      rejectFn = CapabilityRejectFunction(capability);
    }
    // What PromiseCombinatorRun needs to do elements' steps itself (D3131).
    const fixed: Obj = NewRecord(7);
    RecordSet(fixed, 0, c);
    RecordSet(fixed, 1, promiseResolve);
    RecordSet(fixed, 2, capability);
    RecordSet(fixed, 3, values);
    RecordSet(fixed, 4, remaining);
    RecordSet(fixed, 5, resolveFn);
    RecordSet(fixed, 6, rejectFn);
    while (true) {
      index = PromiseCombinatorRun(fixed, kind, index, it, next);
      iteratorDone = true;
      // Without a result object where that is unobservable (D1800).
      let nextValue: Value = undefined;
      if (IteratorFastReady(it, next)) {
        nextValue = IteratorFastValue(it, next);
      } else {
        const result: Obj = IteratorNextResult(it, next);
        if (ToBoolean(Get(result, "done"))) {
          if (kind !== 3 && DecrementRemaining(remaining) === 0) {
            if (kind === 2) {
              RejectCapability(capability, NewAggregateError(values));
            } else {
              ResolveCapability(capability, values);
            }
          }
          return CapabilityPromise(capability);
        }
        nextValue = Get(result, "value");
      }
      iteratorDone = false;
      if (kind !== 3) {
        CreateDataPropertyOrThrow(values, index, undefined);
      }
      const nextPromise: Value = Call(promiseResolve, c, nextValue);
      if (kind === 3) {
        InvokeThen(nextPromise, resolveFn, rejectFn);
      } else {
        const data: Obj = NewElementRecord(index, values, capability, remaining);
        RecordSet(remaining, 0, AsNumber(RecordGet(remaining, 0)) + 1);
        if (kind === 0) {
          InvokeThen(nextPromise, NewBuiltinClosure(3, data, 1), rejectFn);
        } else if (kind === 1) {
          InvokeThen(nextPromise, NewBuiltinClosure(4, data, 1), NewBuiltinClosure(5, data, 1));
        } else {
          InvokeThen(nextPromise, resolveFn, NewBuiltinClosure(6, data, 1));
        }
      }
      index++;
    }
  } catch (e) {
    if (!iteratorDone) {
      IteratorCloseThrow(it);
    }
    RejectCapability(capability, e);
    return CapabilityPromise(capability);
  }
}

/**
 * Promise.all ( iterable ), 27.2.4.1.
 * @builtin Promise.all
 */
function PromiseAll(this: Value, iterable: Value): Value {
  return PerformPromiseCombinator(this, iterable, 0);
}

/**
 * Promise.allSettled ( iterable ), 27.2.4.2.
 * @builtin Promise.allSettled
 */
function PromiseAllSettled(this: Value, iterable: Value): Value {
  return PerformPromiseCombinator(this, iterable, 1);
}

/**
 * Promise.any ( iterable ), 27.2.4.3.
 * @builtin Promise.any
 */
function PromiseAny(this: Value, iterable: Value): Value {
  return PerformPromiseCombinator(this, iterable, 2);
}

/**
 * Promise.try ( callback, ...args ), 27.2.4.8, as in qjs and the current draft: the
 * callback runs first, and a returned promise of the same constructor is not wrapped.
 * @builtin Promise.try
 */
function PromiseTry(this: Value, callback: Value, ...args: Value[]): Value {
  const c: Value = PromiseThisConstructor(this);
  let result: Value = undefined;
  try {
    result = CallWithArray(callback, undefined, args);
  } catch (e) {
    const capability: Value = NewPromiseCapability(c);
    RejectCapability(capability, e);
    return CapabilityPromise(capability);
  }
  return PromiseResolve(c, result);
}

/**
 * Promise.race ( iterable ), 27.2.4.5.
 * @builtin Promise.race
 */
function PromiseRace(this: Value, iterable: Value): Value {
  return PerformPromiseCombinator(this, iterable, 3);
}

/**
 * Promise.withResolvers ( ), 27.2.4.8.
 * @builtin Promise.withResolvers
 */
function PromiseWithResolvers(this: Value): Obj {
  const capability: Value = NewPromiseCapability(PromiseThisConstructor(this));
  const o: Obj = ObjectCreate(RealmIntrinsic("ObjectPrototype"));
  CreateDataPropertyOrThrow(o, "promise", CapabilityPromise(capability));
  CreateDataPropertyOrThrow(o, "resolve", CapabilityResolveFunction(capability));
  CreateDataPropertyOrThrow(o, "reject", CapabilityRejectFunction(capability));
  return o;
}

// ---- Promise.prototype ------------------------------------------------------------------

/**
 * Promise.prototype.then ( onFulfilled, onRejected ), 27.2.5.4.
 * @builtin Promise.prototype.then
 * @intrinsic
 * @fast
 */
function PromisePrototypeThen(this: Value, onFulfilled: Value, onRejected: Value): Value {
  if (!IsPromise(this)) {
    throw NewTypeError("Promise object expected");
  }
  const promise: Obj = AsObj(this);
  const c: Value = SpeciesConstructor(promise, IntrinsicPromise());
  const capability: Value = NewPromiseCapability(c);
  PerformPromiseThen(promise, onFulfilled, onRejected, capability);
  return CapabilityPromise(capability);
}

/**
 * Promise.prototype.catch ( onRejected ), 27.2.5.1.
 * @builtin Promise.prototype.catch
 */
function PromisePrototypeCatch(this: Value, onRejected: Value): Value {
  return InvokeThen(this, undefined, onRejected);
}

/**
 * Promise.prototype.finally ( onFinally ), 27.2.5.3.
 * @builtin Promise.prototype.finally
 */
function PromisePrototypeFinally(this: Value, onFinally: Value): Value {
  if (!IsObject(this)) {
    throw NewTypeError("not an object");
  }
  const c: Value = SpeciesConstructor(AsObj(this), IntrinsicPromise());
  if (!IsCallable(onFinally)) {
    return InvokeThen(this, onFinally, onFinally);
  }
  const data: Obj = NewRecord(2);
  RecordSet(data, 0, onFinally);
  RecordSet(data, 1, c);
  return InvokeThen(this, NewBuiltinClosure(7, data, 1), NewBuiltinClosure(8, data, 1));
}

/**
 * Then Finally Functions, 27.2.5.3.1: `this` is the Record [onFinally, C].
 * @intrinsic
 */
function PromiseThenFinally(this: Value, value: Value): Value {
  const data: Obj = AsObj(this);
  const result: Value = Call(RecordGet(data, 0), undefined);
  const p: Value = PromiseResolve(RecordGet(data, 1), result);
  return Call(GetV(p, "then"), p, NewBuiltinClosure(9, value, 0));
}

/**
 * Catch Finally Functions, 27.2.5.3.2.
 * @intrinsic
 */
function PromiseCatchFinally(this: Value, reason: Value): Value {
  const data: Obj = AsObj(this);
  const result: Value = Call(RecordGet(data, 0), undefined);
  const p: Value = PromiseResolve(RecordGet(data, 1), result);
  return Call(GetV(p, "then"), p, NewBuiltinClosure(10, reason, 0));
}

/**
 * The value thunk of Then Finally: returns its data.
 * @intrinsic
 */
function PromiseValueThunk(this: Value): Value {
  return this;
}

/**
 * The thrower of Catch Finally: throws its data.
 * @intrinsic
 */
function PromiseThrower(this: Value): Value {
  throw this;
}

/**
 * Promise.prototype [ %Symbol.toStringTag% ], 27.2.5.5.
 * @value Promise.prototype[Symbol.toStringTag]
 * @attributes configurable
 */
function PromisePrototypeToStringTag(): Str {
  return "Promise";
}

// ---- Async host calls -------------------------------------------------------------------

/**
 * Settles the promise of an async host call (D1374): the job that
 * `Vm::resolve_host_call` queues. A value is resolved like a resolve function would
 * (thenables are adopted); an error rejects.
 * @intrinsic
 */
function HostCallSettle(promise: Obj, ok: boolean, value: Value): void {
  if (!PromiseClaimResolve(promise)) {
    return;
  }
  if (ok) {
    ResolvePromise(promise, value);
  } else {
    RejectPromise(promise, value);
  }
}
