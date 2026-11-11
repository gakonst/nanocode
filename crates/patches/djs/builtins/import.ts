// `import()` in Scripts and function bodies (0058). djs has no modules and no module
// loader, so every import is rejected, as by qjs without a loader (pi's codemode needs a
// runtime rejection, not a SyntaxError of the whole script).

/**
 * EvaluateImportCall ( specifierExpression, optionsExpression ), 13.3.10.2, from step
 * 6: the specifier and options checks reject the promise; then the host's module load
 * fails in a job, with qjs's "could not load module" ReferenceError.
 * @intrinsic
 */
function DynamicImport(specifier: Value, options: Value): Obj {
  const promise: Obj = PromiseCreate(RealmIntrinsic("PromisePrototype"));
  try {
    const name: Str = ToString(specifier);
    if (options !== undefined) {
      if (!IsObject(options)) {
        throw NewTypeError("options must be an object");
      }
      const attributes: Value = Get(AsObj(options), "with");
      if (attributes !== undefined) {
        if (!IsObject(attributes)) {
          throw NewTypeError("options.with must be an object");
        }
        const entries: Obj = EnumerableOwnProperties(AsObj(attributes), 1);
        const n: f64 = LengthOfArrayLike(entries);
        for (let i: f64 = 0; i < n; i++) {
          if (!IsString(Get(entries, i))) {
            throw NewTypeError("module attribute values must be strings");
          }
        }
      }
    }
    // The load fails one job later, as qjs's import job does: a fulfilled promise's
    // reaction throws into this one.
    const error: Obj = NewReferenceError("could not load module '" + name + "'");
    const ready: Value = PromiseResolve(IntrinsicPromise(), undefined);
    PerformPromiseThen(AsObj(ready), NewBuiltinClosure(10, error, 0), undefined, promise);
  } catch (e) {
    RejectPromise(promise, e);
  }
  return promise;
}
