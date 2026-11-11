// Dynamic code (0059, D014): indirect eval and CreateDynamicFunction, whose leaves
// compile source text (crates/djs-core/src/builtins/eval.rs), and queueMicrotask.

/**
 * eval ( x ), 19.2.1: always indirect (D014: djs has no direct eval, so `eval(src)`
 * behaves as `(0, eval)(src)`). A non-string is returned as it is.
 * @builtin globalThis.eval
 */
function GlobalEval(x: Value): Value {
  if (!IsString(x)) {
    return x;
  }
  const script: Obj = CompileEval(AsStr(x));
  return Call(script, GlobalObject());
}

/**
 * CreateDynamicFunction ( constructor, newTarget, kind, parameterArgs, bodyArg ),
 * 20.2.1.1.1. `kind`: 0 normal, 1 generator, 2 async, 3 async generator; `fallback` the
 * realm prototype of that kind.
 */
function CreateDynamicFunction(newTarget: Value, kind: i32, fallback: Str, args: Obj): Obj {
  const argCount: f64 = LengthOfArrayLike(args);
  let p: Str = "";
  let body: Str = "";
  if (argCount > 0) {
    for (let k: f64 = 0; k < argCount - 1; k++) {
      const next: Str = ToString(Get(args, k));
      p = k === 0 ? next : p + "," + next;
    }
    body = ToString(Get(args, argCount - 1));
  }
  const script: Obj = CompileFunction(kind, p, body);
  const f: Obj = AsObj(Call(script, GlobalObject()));
  if (newTarget !== undefined) {
    const proto: Obj = GetPrototypeFromConstructor(newTarget, fallback);
    SetPrototypeOf(f, proto);
  }
  return f;
}

/**
 * queueMicrotask ( callback ) (HTML 8.6, and qjs's global): queues a job that calls
 * `callback` with no arguments.
 * @builtin globalThis.queueMicrotask
 */
function GlobalQueueMicrotask(callback: Value): void {
  if (!IsCallable(callback)) {
    throw NewTypeError("not a function");
  }
  EnqueueMicrotask(callback);
}

/**
 * The job of queueMicrotask. An exception is reported like an unhandled rejection
 * (D1592): the job queue goes on, as in browsers.
 * @intrinsic
 */
function MicrotaskJob(callback: Value): void {
  try {
    Call(callback, undefined);
  } catch (e) {
    RejectPromise(PromiseCreate(RealmIntrinsic("PromisePrototype")), e);
  }
}
