// Generator objects (ECMA-262 27.5) and the await/return steps of async functions
// (27.7), over the suspendable frames of interp/run/suspend.rs (D1375). The bytecode
// ops generator_start/yield/await/async_return/async_reject suspend and finish frames;
// these builtins resume them and settle the result promises.

/**
 * GeneratorValidate ( generator, brand ), 27.5.3.2: the state (qjs's messages). States:
 * 0 at the start, 1 at a yield, 2 executing, 3 completed, 6 at a yield_star.
 */
function GeneratorValidate(generator: Value): i32 {
  const state: i32 = GeneratorState(generator);
  if (state < 0) {
    throw NewTypeError("not a generator");
  }
  if (state === 2) {
    throw NewTypeError("cannot invoke a running generator");
  }
  return state;
}

/**
 * Resumes generator g (suspended) with mode 0 next, 1 throw, 2 return, and makes the
 * iterator result: a yield leaves it suspended (done false); a return or a throw
 * completes it.
 */
function GeneratorStep(g: Obj, value: Value, mode: i32): Obj {
  let result: Value = undefined;
  try {
    result = GeneratorResume(g, value, mode);
  } catch (e) {
    GeneratorSetState(g, 3);
    throw e;
  }
  const state: i32 = GeneratorState(g);
  if (state === 1) {
    return CreateIterResultObject(result, false);
  }
  // yield_star: the inner iterator's result object as it is.
  if (state === 6) {
    return AsObj(result);
  }
  GeneratorSetState(g, 3);
  return CreateIterResultObject(result, true);
}

/**
 * %GeneratorPrototype%.next ( value ), 27.5.1.2.
 * @builtin %GeneratorPrototype%.next
 * @intrinsic
 */
function GeneratorPrototypeNext(this: Value, value: Value): Obj {
  const state: i32 = GeneratorValidate(this);
  if (state === 3) {
    return CreateIterResultObject(undefined, true);
  }
  return GeneratorStep(AsObj(this), value, 0);
}

/**
 * %GeneratorPrototype%.return ( value ), 27.5.1.3.
 * @builtin %GeneratorPrototype%.return
 */
function GeneratorPrototypeReturn(this: Value, value: Value): Obj {
  const state: i32 = GeneratorValidate(this);
  if (state === 0 || state === 3) {
    GeneratorSetState(AsObj(this), 3);
    return CreateIterResultObject(value, true);
  }
  return GeneratorStep(AsObj(this), value, 2);
}

/**
 * %GeneratorPrototype%.throw ( exception ), 27.5.1.4.
 * @builtin %GeneratorPrototype%.throw
 */
function GeneratorPrototypeThrow(this: Value, exception: Value): Obj {
  const state: i32 = GeneratorValidate(this);
  if (state === 0 || state === 3) {
    GeneratorSetState(AsObj(this), 3);
    throw exception;
  }
  return GeneratorStep(AsObj(this), exception, 1);
}

/**
 * %GeneratorPrototype% [ %Symbol.toStringTag% ], 27.5.1.5.
 * @value %GeneratorPrototype%[Symbol.toStringTag]
 * @attributes configurable
 */
function GeneratorPrototypeToStringTag(): Str {
  return "Generator";
}

/**
 * Await ( value ), 27.7.5.3, after the `await` op suspended the frame (of an async
 * function or an async generator, D1421): subscribes the state to the promise and
 * returns an async function's result promise to whoever
 * runs it (its caller the first time, the resuming job later). If PromiseResolve
 * throws, the function continues at once with that exception.
 * @intrinsic
 */
function AsyncAwait(a: Obj, v: Value): Value {
  let promise: Value = undefined;
  try {
    promise = PromiseResolve(IntrinsicPromise(), v);
  } catch (e) {
    return GeneratorResume(a, e, 1);
  }
  PerformPromiseThen(AsObj(promise), a, a, undefined);
  // An async generator's resumer (AsyncGeneratorStep) only looks at its state.
  if (IsAsyncFunctionState(a)) {
    return AsyncFunctionPromise(a);
  }
  return undefined;
}

/**
 * An async function returns v (`async_return`): resolves its promise, returns it. The
 * `async_return` op does that itself unless resolving calls `then` (D3130); an
 * `async_reject` rejects in the op.
 * @intrinsic
 */
function AsyncFunctionReturn(a: Obj, v: Value): Value {
  GeneratorSetState(a, 3);
  const promise: Obj = AsyncFunctionPromise(a);
  ResolvePromise(promise, v);
  return promise;
}

// ---- The function-kind constructors (27.3.1, 27.7.1, 27.4.1; D1420, 0059) --------------
//
// CreateDynamicFunction (builtins/eval.ts) compiles their source. Installing them
// (InstallIntrinsic) wires their `prototype` and `constructor` links in Rust.

/**
 * GeneratorFunction ( ...parameterArgs, bodyArg ), 27.3.1.1.
 * @intrinsic
 */
function GeneratorFunctionConstructor(...args: Value[]): Value {
  return CreateDynamicFunction(new.target, 1, "GeneratorFunctionPrototype", args);
}

/**
 * AsyncGeneratorFunction ( ...parameterArgs, bodyArg ), 27.4.1.1.
 * @intrinsic
 */
function AsyncGeneratorFunctionConstructor(...args: Value[]): Value {
  return CreateDynamicFunction(new.target, 3, "AsyncGeneratorFunctionPrototype", args);
}

/**
 * AsyncFunction ( ...parameterArgs, bodyArg ), 27.7.1.1.
 * @intrinsic
 */
function AsyncFunctionConstructor(...args: Value[]): Value {
  return CreateDynamicFunction(new.target, 2, "AsyncFunctionPrototype", args);
}
