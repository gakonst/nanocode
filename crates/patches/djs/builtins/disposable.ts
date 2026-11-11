// Explicit resource management, the runtime parts (ES2026 27.3, 27.4, 20.5.8, 0051):
// DisposableStack, AsyncDisposableStack, SuppressedError, and the dispose methods of
// %IteratorPrototype% and %AsyncIteratorPrototype%. The `using` syntax is a compiler
// issue of its own.
//
// A stack is a DisposableStack object (heap class 33, slots in disposable.ts's leaves):
// slot 0 async, 1 the state (0 pending, 1 disposed), 2 the resources, an internal
// Array of [value, method, kind] triples, oldest first. Kinds (D1512): 0 calls
// method with `this` = value, 1 calls method(value) (adopt's closure), 2 is an
// AsyncDisposableStack resource with only a Symbol.dispose method (GetDisposeMethod's
// closure: the call's outcome is awaited), 3 is an AsyncDisposableStack resource of
// null or undefined (no method; only forces an await).
// @slim-omit: left out of the slim build (D2411).

/** The SuppressedError DisposeResources throws when a disposal throws after another. */
function NewSuppressedError(error: Value, suppressed: Value): Obj {
  const o: Obj = ErrorCreate(RealmIntrinsic("SuppressedErrorPrototype"));
  DefineDataPropertyOrThrow(o, "error", error, 5);
  DefineDataPropertyOrThrow(o, "suppressed", suppressed, 5);
  RecordStack(o);
  return o;
}

/** The stack `this` of a method, still pending (qjs's ReferenceError otherwise). */
function PendingDisposableStack(v: Value, isAsync: boolean): Obj {
  const stack: Obj = ThisDisposableStack(v, isAsync);
  if (F64ToI32(AsNumber(RecordGet(stack, 1))) !== 0) {
    throw NewReferenceError(isAsync ? "AsyncDisposableStack is disposed" : "DisposableStack is disposed");
  }
  return stack;
}

/** Appends a resource triple to a stack. */
function AddResource(stack: Obj, value: Value, method: Value, kind: i32): void {
  const resources: Obj = AsObj(RecordGet(stack, 2));
  const n: f64 = LengthOfArrayLike(resources);
  CreateDataPropertyOrThrow(resources, n, value);
  CreateDataPropertyOrThrow(resources, n + 1, method);
  CreateDataPropertyOrThrow(resources, n + 2, kind);
}

/**
 * AddDisposableResource ( disposeCapability, V, hint ), 27.3.3.1 (without a method):
 * CreateDisposableResource and GetDisposeMethod for `use`.
 */
function AddDisposableValue(stack: Obj, v: Value, isAsync: boolean): void {
  if (v === undefined || v === null) {
    if (isAsync) {
      AddResource(stack, undefined, undefined, 3);
    }
    return;
  }
  if (!IsObject(v)) {
    throw NewTypeError("using: value is not an object");
  }
  let kind: i32 = 0;
  let method: Value = undefined;
  if (isAsync) {
    method = GetV(v, WellKnownSymbol("asyncDispose"));
  }
  if (method === undefined || method === null) {
    method = GetV(v, WellKnownSymbol("dispose"));
    if (isAsync) {
      kind = 2;
    }
    if (method === undefined || method === null) {
      throw NewTypeError(isAsync ? "using: value has no Symbol.asyncDispose or Symbol.dispose method" : "using: value has no Symbol.dispose method");
    }
  }
  if (!IsCallable(method)) {
    throw NewTypeError("using: dispose method is not a function");
  }
  AddResource(stack, v, method, kind);
}

/** Calls the disposal of resource triple i (kinds 0-2). */
function CallDisposeMethod(resources: Obj, i: f64): Value {
  const value: Value = Get(resources, i);
  const method: Value = Get(resources, i + 1);
  if (F64ToI32(AsNumber(Get(resources, i + 2))) === 1) {
    return Call(method, undefined, value);
  }
  return Call(method, value);
}

/**
 * DisposableStack ( ), 27.3.1.1.
 * @constructor DisposableStack
 */
function DisposableStackConstructor(): Obj {
  if (new.target === undefined) {
    throw NewTypeError("must be called with new");
  }
  return DisposableStackCreate(GetPrototypeFromConstructor(new.target, "DisposableStackPrototype"), false);
}

/**
 * get DisposableStack.prototype.disposed, 27.3.3.3.
 * @getter DisposableStack.prototype.disposed
 */
function DisposableStackPrototypeDisposed(this: Value): boolean {
  return F64ToI32(AsNumber(RecordGet(ThisDisposableStack(this, false), 1))) === 1;
}

/**
 * DisposableStack.prototype.dispose ( ), 27.3.3.2; also [ %Symbol.dispose% ].
 * DisposeResources ( disposeCapability, NormalCompletion ), 27.3.3.4.
 * @builtin DisposableStack.prototype.dispose
 * @builtin DisposableStack.prototype[Symbol.dispose]
 */
function DisposableStackPrototypeDispose(this: Value): void {
  const stack: Obj = ThisDisposableStack(this, false);
  if (F64ToI32(AsNumber(RecordGet(stack, 1))) === 1) {
    return;
  }
  RecordSet(stack, 1, 1);
  const resources: Obj = AsObj(RecordGet(stack, 2));
  RecordSet(stack, 2, ArrayCreate(0));
  let hasError: boolean = false;
  let error: Value = undefined;
  for (let i: f64 = LengthOfArrayLike(resources) - 3; i >= 0; i = i - 3) {
    try {
      CallDisposeMethod(resources, i);
    } catch (e) {
      error = hasError ? NewSuppressedError(e, error) : e;
      hasError = true;
    }
  }
  if (hasError) {
    throw error;
  }
}

/**
 * DisposableStack.prototype.use ( value ), 27.3.3.6.
 * @builtin DisposableStack.prototype.use
 */
function DisposableStackPrototypeUse(this: Value, value: Value): Value {
  AddDisposableValue(PendingDisposableStack(this, false), value, false);
  return value;
}

/**
 * DisposableStack.prototype.adopt ( value, onDispose ), 27.3.3.1.
 * @builtin DisposableStack.prototype.adopt
 */
function DisposableStackPrototypeAdopt(this: Value, value: Value, onDispose: Value): Value {
  const stack: Obj = PendingDisposableStack(this, false);
  if (!IsCallable(onDispose)) {
    throw NewTypeError("not a function");
  }
  AddResource(stack, value, onDispose, 1);
  return value;
}

/**
 * DisposableStack.prototype.defer ( onDispose ), 27.3.3.2.
 * @builtin DisposableStack.prototype.defer
 */
function DisposableStackPrototypeDefer(this: Value, onDispose: Value): void {
  const stack: Obj = PendingDisposableStack(this, false);
  if (!IsCallable(onDispose)) {
    throw NewTypeError("not a function");
  }
  AddResource(stack, undefined, onDispose, 0);
}

/** move ( ) of both stacks: a new stack takes the resources; this one is disposed. */
function DisposableStackMove(thisValue: Value, isAsync: boolean): Obj {
  const stack: Obj = PendingDisposableStack(thisValue, isAsync);
  const proto: Obj = RealmIntrinsic(isAsync ? "AsyncDisposableStackPrototype" : "DisposableStackPrototype");
  const newStack: Obj = DisposableStackCreate(proto, isAsync);
  RecordSet(newStack, 2, RecordGet(stack, 2));
  RecordSet(stack, 2, ArrayCreate(0));
  RecordSet(stack, 1, 1);
  return newStack;
}

/**
 * DisposableStack.prototype.move ( ), 27.3.3.5.
 * @builtin DisposableStack.prototype.move
 */
function DisposableStackPrototypeMove(this: Value): Obj {
  return DisposableStackMove(this, false);
}

/**
 * DisposableStack.prototype [ %Symbol.toStringTag% ], 27.3.3.8.
 * @value DisposableStack.prototype[Symbol.toStringTag]
 * @attributes configurable
 */
function DisposableStackPrototypeToStringTag(): Str {
  return "DisposableStack";
}

/**
 * AsyncDisposableStack ( ), 27.4.1.1.
 * @constructor AsyncDisposableStack
 */
function AsyncDisposableStackConstructor(): Obj {
  if (new.target === undefined) {
    throw NewTypeError("must be called with new");
  }
  return DisposableStackCreate(GetPrototypeFromConstructor(new.target, "AsyncDisposableStackPrototype"), true);
}

/**
 * get AsyncDisposableStack.prototype.disposed, 27.4.3.4.
 * @getter AsyncDisposableStack.prototype.disposed
 */
function AsyncDisposableStackPrototypeDisposed(this: Value): boolean {
  return F64ToI32(AsNumber(RecordGet(ThisDisposableStack(this, true), 1))) === 1;
}

/**
 * AsyncDisposableStack.prototype.use ( value ), 27.4.3.6.
 * @builtin AsyncDisposableStack.prototype.use
 */
function AsyncDisposableStackPrototypeUse(this: Value, value: Value): Value {
  AddDisposableValue(PendingDisposableStack(this, true), value, true);
  return value;
}

/**
 * AsyncDisposableStack.prototype.adopt ( value, onDisposeAsync ), 27.4.3.1.
 * @builtin AsyncDisposableStack.prototype.adopt
 */
function AsyncDisposableStackPrototypeAdopt(this: Value, value: Value, onDisposeAsync: Value): Value {
  const stack: Obj = PendingDisposableStack(this, true);
  if (!IsCallable(onDisposeAsync)) {
    throw NewTypeError("not a function");
  }
  AddResource(stack, value, onDisposeAsync, 1);
  return value;
}

/**
 * AsyncDisposableStack.prototype.defer ( onDisposeAsync ), 27.4.3.2.
 * @builtin AsyncDisposableStack.prototype.defer
 */
function AsyncDisposableStackPrototypeDefer(this: Value, onDisposeAsync: Value): void {
  const stack: Obj = PendingDisposableStack(this, true);
  if (!IsCallable(onDisposeAsync)) {
    throw NewTypeError("not a function");
  }
  AddResource(stack, undefined, onDisposeAsync, 0);
}

/**
 * AsyncDisposableStack.prototype.move ( ), 27.4.3.5.
 * @builtin AsyncDisposableStack.prototype.move
 */
function AsyncDisposableStackPrototypeMove(this: Value): Obj {
  return DisposableStackMove(this, true);
}

/**
 * AsyncDisposableStack.prototype [ %Symbol.toStringTag% ], 27.4.3.8.
 * @value AsyncDisposableStack.prototype[Symbol.toStringTag]
 * @attributes configurable
 */
function AsyncDisposableStackPrototypeToStringTag(): Str {
  return "AsyncDisposableStack";
}

// ---- AsyncDisposableStack.prototype.disposeAsync: DisposeResources with awaits ----------
//
// Like Array.fromAsync (D1511), a Record state machine resumed by builtin closures 18
// and 19: 0 the result promise, 1 the resources, 2 the index of the next triple to
// dispose (counting down), 3 the error, 4 whether there is one, 5 needsAwait, 6
// hasAwaited, 7 a kind-2 call's exception, to throw once the await is over.

/**
 * AsyncDisposableStack.prototype.disposeAsync ( ), 27.4.3.3; also
 * [ %Symbol.asyncDispose% ].
 * @builtin AsyncDisposableStack.prototype.disposeAsync
 * @builtin AsyncDisposableStack.prototype[Symbol.asyncDispose]
 */
function AsyncDisposableStackPrototypeDisposeAsync(this: Value): Obj {
  const promise: Obj = PromiseCreate(RealmIntrinsic("PromisePrototype"));
  let stack: Obj = promise;
  try {
    stack = ThisDisposableStack(this, true);
  } catch (e) {
    RejectPromise(promise, e);
    return promise;
  }
  if (F64ToI32(AsNumber(RecordGet(stack, 1))) === 1) {
    ResolvePromise(promise, undefined);
    return promise;
  }
  RecordSet(stack, 1, 1);
  const s: Obj = NewRecord(8);
  RecordSet(s, 0, promise);
  const resources: Obj = AsObj(RecordGet(stack, 2));
  RecordSet(stack, 2, ArrayCreate(0));
  RecordSet(s, 1, resources);
  RecordSet(s, 2, LengthOfArrayLike(resources) - 3);
  RecordSet(s, 4, false);
  RecordSet(s, 5, false);
  RecordSet(s, 6, false);
  RecordSet(s, 7, undefined);
  AsyncDisposeContinue(s);
  return promise;
}

/** Records a disposal error (step 2.c.iii: a SuppressedError over an earlier one). */
function AsyncDisposeError(s: Obj, e: Value): void {
  if (RecordGet(s, 4) === true) {
    RecordSet(s, 3, NewSuppressedError(e, RecordGet(s, 3)));
  } else {
    RecordSet(s, 3, e);
    RecordSet(s, 4, true);
  }
}

/** Await ( v ) of disposeAsync: resumes with AsyncDisposeResume. */
function AsyncDisposeAwait(s: Obj, v: Value): void {
  const promise: Value = PromiseResolve(IntrinsicPromise(), v);
  PerformPromiseThen(AsObj(promise), NewBuiltinClosure(18, s, 1), NewBuiltinClosure(19, s, 1), undefined);
}

/**
 * The loop of DisposeResources (27.3.3.4 step 2) from the current index: disposes
 * resources until one needs an await, then the final await (step 3) and the result.
 */
function AsyncDisposeContinue(s: Obj): void {
  const resources: Obj = AsObj(RecordGet(s, 1));
  while (true) {
    const i: f64 = AsNumber(RecordGet(s, 2));
    if (i < 0) {
      break;
    }
    RecordSet(s, 2, i - 3);
    const kind: i32 = F64ToI32(AsNumber(Get(resources, i + 2)));
    if (kind === 3) {
      RecordSet(s, 5, true);
      continue;
    }
    let result: Value = undefined;
    try {
      result = CallDisposeMethod(resources, i);
    } catch (e) {
      if (kind !== 2) {
        AsyncDisposeError(s, e);
        continue;
      }
      // GetDisposeMethod's closure returns a rejected promise: awaited, then thrown.
      RecordSet(s, 7, e);
      result = undefined;
    }
    RecordSet(s, 6, true);
    if (kind === 2) {
      result = undefined;
    }
    try {
      AsyncDisposeAwait(s, result);
    } catch (e) {
      AsyncDisposeError(s, e);
      continue;
    }
    return;
  }
  if (RecordGet(s, 5) === true && RecordGet(s, 6) !== true) {
    RecordSet(s, 6, true);
    AsyncDisposeAwait(s, undefined);
    return;
  }
  const promise: Obj = AsObj(RecordGet(s, 0));
  if (RecordGet(s, 4) === true) {
    RejectPromise(promise, RecordGet(s, 3));
  } else {
    ResolvePromise(promise, undefined);
  }
}

/**
 * Resumes disposeAsync after a fulfilled await: `this` is its Record.
 * @intrinsic
 */
function AsyncDisposeFulfilled(this: Value, value: Value): void {
  const s: Obj = AsObj(this);
  const pending: Value = RecordGet(s, 7);
  if (pending !== undefined) {
    RecordSet(s, 7, undefined);
    AsyncDisposeError(s, pending);
  }
  AsyncDisposeContinue(s);
}

/**
 * Resumes disposeAsync after a rejected await: `this` is its Record.
 * @intrinsic
 */
function AsyncDisposeRejected(this: Value, reason: Value): void {
  const s: Obj = AsObj(this);
  AsyncDisposeError(s, reason);
  AsyncDisposeContinue(s);
}

// ---- `using` and `await using` declarations (0058, docs/compiler.md 7.10) ---------------
//
// A scope with `using` declarations keeps a Record on the operand stack: 2 the resources
// (triples as in a DisposableStack, kinds 0, 2 and 3, plus kind 4: an `await using`
// resource with a Symbol.asyncDispose method, whose result is awaited), 3 the error, 4
// whether there is one, 5 needsAwait, 6 hasAwaited, 7 the index of the next triple to
// dispose (undefined before the first step), 8 the value to await, 9 a kind-2 call's
// exception to merge once the await is over, 10 whether there is one. Slots 0 and 1 are
// unused, so AddDisposableValue and AsyncDisposeError work on it unchanged.

/**
 * The resource Record of a scope's `using` declarations.
 * @intrinsic
 */
function UsingCreate(): Obj {
  const s: Obj = NewRecord(11);
  RecordSet(s, 2, ArrayCreate(0));
  return s;
}

/**
 * `using x = v`: AddDisposableResource ( s, v, sync-dispose ), 27.3.3.1. Returns v.
 * @intrinsic
 */
function UsingAdd(s: Obj, v: Value): Value {
  AddDisposableValue(s, v, false);
  return v;
}

/**
 * `await using x = v`: AddDisposableResource ( s, v, async-dispose ). Returns v.
 * @intrinsic
 */
function AwaitUsingAdd(s: Obj, v: Value): Value {
  AddDisposableValue(s, v, true);
  // An async add always appends one triple (kind 3 for null and undefined); kind 0
  // there is a Symbol.asyncDispose method, awaited unlike a `using` resource's.
  const resources: Obj = AsObj(RecordGet(s, 2));
  const k: f64 = LengthOfArrayLike(resources) - 1;
  if (F64ToI32(AsNumber(Get(resources, k))) === 0) {
    CreateDataPropertyOrThrow(resources, k, 4);
  }
  return v;
}

/**
 * Records an exception: the one the scope exits with (first), or one of a disposal
 * (as a SuppressedError over the earlier ones).
 * @intrinsic
 */
function UsingDisposeError(s: Obj, e: Value): void {
  AsyncDisposeError(s, e);
}

/**
 * DisposeResources ( s, completion ), 27.3.3.4, of a scope without `await using`: the
 * completion is the error recorded by UsingDisposeError, if any; throws the result.
 * @intrinsic
 */
function UsingDispose(s: Obj): void {
  const resources: Obj = AsObj(RecordGet(s, 2));
  for (let i: f64 = LengthOfArrayLike(resources) - 3; i >= 0; i = i - 3) {
    try {
      CallDisposeMethod(resources, i);
    } catch (e) {
      AsyncDisposeError(s, e);
    }
  }
  if (RecordGet(s, 4) === true) {
    throw RecordGet(s, 3);
  }
}

/**
 * DisposeResources of a scope with `await using`, up to its next Await: true when the
 * code must await UsingAwaitValue(s) (an exception from it goes to UsingDisposeError)
 * and step again, false when done; throws the result.
 * @intrinsic
 */
function UsingDisposeStep(s: Obj): boolean {
  if (RecordGet(s, 10) === true) {
    RecordSet(s, 10, false);
    AsyncDisposeError(s, RecordGet(s, 9));
  }
  const resources: Obj = AsObj(RecordGet(s, 2));
  let i: f64 = 0;
  if (RecordGet(s, 7) === undefined) {
    i = LengthOfArrayLike(resources) - 3;
  } else {
    i = AsNumber(RecordGet(s, 7));
  }
  while (i >= 0) {
    const kind: i32 = F64ToI32(AsNumber(Get(resources, i + 2)));
    if (kind === 0 && RecordGet(s, 5) === true && RecordGet(s, 6) !== true) {
      // A sync disposal after a null `await using`: the pending Await comes first.
      RecordSet(s, 5, false);
      RecordSet(s, 7, i);
      RecordSet(s, 8, undefined);
      return true;
    }
    RecordSet(s, 7, i - 3);
    if (kind === 3) {
      RecordSet(s, 5, true);
    } else if (kind === 0) {
      try {
        CallDisposeMethod(resources, i);
      } catch (e) {
        AsyncDisposeError(s, e);
      }
    } else {
      let result: Value = undefined;
      let threw: boolean = false;
      try {
        result = CallDisposeMethod(resources, i);
      } catch (e) {
        threw = true;
        if (kind === 2) {
          // GetDisposeMethod's closure returns a rejected promise: awaited, then thrown.
          RecordSet(s, 9, e);
          RecordSet(s, 10, true);
        } else {
          AsyncDisposeError(s, e);
        }
      }
      if (kind === 2 || !threw) {
        RecordSet(s, 6, true);
        RecordSet(s, 8, kind === 2 ? undefined : result);
        return true;
      }
    }
    i = i - 3;
  }
  if (RecordGet(s, 5) === true && RecordGet(s, 6) !== true) {
    RecordSet(s, 5, false);
    RecordSet(s, 8, undefined);
    return true;
  }
  if (RecordGet(s, 4) === true) {
    throw RecordGet(s, 3);
  }
  return false;
}

/**
 * The value UsingDisposeStep asks to await.
 * @intrinsic
 */
function UsingAwaitValue(s: Obj): Value {
  return RecordGet(s, 8);
}

/**
 * %IteratorPrototype% [ %Symbol.dispose% ] ( ), 27.1.4.15.
 * @builtin Iterator.prototype[Symbol.dispose]
 */
function IteratorPrototypeDispose(this: Value): void {
  const method: Value = GetMethod(this, "return");
  if (method !== undefined) {
    Call(method, this);
  }
}

/**
 * %AsyncIteratorPrototype% [ %Symbol.asyncDispose% ] ( ), 27.1.5.1.
 * @builtin %AsyncIteratorPrototype%[Symbol.asyncDispose]
 */
function AsyncIteratorPrototypeAsyncDispose(this: Value): Obj {
  const promise: Obj = PromiseCreate(RealmIntrinsic("PromisePrototype"));
  let wrapper: Value = undefined;
  try {
    const method: Value = GetMethod(this, "return");
    if (method === undefined) {
      ResolvePromise(promise, undefined);
      return promise;
    }
    wrapper = PromiseResolve(IntrinsicPromise(), Call(method, this));
  } catch (e) {
    RejectPromise(promise, e);
    return promise;
  }
  // The unwrap closure returns undefined: kind 9 returns its data.
  PerformPromiseThen(AsObj(wrapper), NewBuiltinClosure(9, undefined, 1), undefined, promise);
  return promise;
}

/**
 * Symbol.dispose, 20.4.2.
 * @value Symbol.dispose
 */
function SymbolDispose(): Sym {
  return WellKnownSymbol("dispose");
}

/**
 * Symbol.asyncDispose, 20.4.2.
 * @value Symbol.asyncDispose
 */
function SymbolAsyncDispose(): Sym {
  return WellKnownSymbol("asyncDispose");
}
