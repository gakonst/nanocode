// ES modules (0241, D3171): the evaluation of a linked module graph and the traps of
// module namespace objects. Loading and linking are leaf work in Rust (interp/module.rs);
// evaluation calls the module bodies, so it is here.
//
// A module record is an internal Record (interp/module.rs has the layout): slot 1 the
// status (2 linked, 3 evaluating, 4 evaluated, 5 evaluated with the error in slot 5, 6
// evaluating-async), 2 the module body, 3 the requested modules (a Record of records) and
// 4 their count, 6 and 7 [[DFSIndex]] and [[DFSAncestorIndex]], 10 the record under it on
// the evaluation stack, 12 flags (2: [[HasTLA]], the body is async), 13 [[CycleRoot]], 14
// [[AsyncEvaluationOrder]] (0 unset, -1 done), 15 [[PendingAsyncDependencies]], 16
// [[AsyncParentModules]] (an Array or null), 17 [[TopLevelCapability]] (a promise).
//
// @embed-only: left out of the preludes of builds without `embed` (D3175).

/**
 * Evaluate ( ), 16.2.1.5.3, of a linked module: the promise of the evaluation of its cycle
 * root, made once ([[TopLevelCapability]]). Sync modules settle it before it returns; a
 * graph with top-level await settles it when its last async module is done. An error is
 * kept by every module it reached and thrown again by later evaluations.
 */
function ModuleEvaluate(m: Obj): Obj {
  let module: Obj = m;
  const status: f64 = AsNumber(RecordGet(module, 1));
  if (status === 4 || status === 5 || status === 6) {
    const root: Value = RecordGet(module, 13);
    if (root !== null) {
      module = AsObj(root);
    }
  }
  const existing: Value = RecordGet(module, 17);
  if (existing !== undefined) {
    return AsObj(existing);
  }
  const promise: Obj = PromiseCreate(RealmIntrinsic("PromisePrototype"));
  RecordSet(module, 17, promise);
  // [the top of the stack, the next DFS index]
  const state: Obj = NewRecord(2);
  RecordSet(state, 0, null);
  RecordSet(state, 1, 0);
  try {
    InnerModuleEvaluation(module, state);
  } catch (e) {
    let top: Value = RecordGet(state, 0);
    while (top !== null) {
      const t: Obj = AsObj(top);
      RecordSet(t, 1, 5);
      RecordSet(t, 5, e);
      top = RecordGet(t, 10);
      RecordSet(t, 10, null);
    }
    RejectPromise(promise, e);
    return promise;
  }
  // Not async, or async and done already (an evaluated module evaluated again).
  if (AsNumber(RecordGet(module, 14)) <= 0) {
    FulfillPromise(promise, undefined);
  }
  return promise;
}

/** [[HasTLA]] of record m. */
function HasTLA(m: Obj): boolean {
  return (F64ToI32(AsNumber(RecordGet(m, 12))) & 2) !== 0;
}

/** A builtin closure of ModuleOp with op and its operand (module.ts's reactions). */
function ModuleClosure(op: i32, operand: Value, length: i32): Value {
  const data: Obj = NewRecord(2);
  RecordSet(data, 0, op);
  RecordSet(data, 1, operand);
  return NsOp(8, data, length, undefined);
}

/**
 * EvaluateImportCall ( specifierExpression, optionsExpression ), 13.3.10.2, in builds with
 * modules: the compiler calls this (through ModuleOp) with the referrer, the key of the
 * module or the name of the script the call is in. The checks reject as in
 * builtins/import.ts; then HostLoadImportedModule is a call of the VM's async host
 * function for imports (`vm::IMPORT_FN_ID`), which the host answers with the key of the
 * graph it loaded and linked (or an error, which rejects), and FinishImport evaluates it
 * (ContinueDynamicImport). A VM whose host does not answer imports rejects as without
 * modules.
 */
function ImportCall(specifier: Value, options: Value, referrer: Value): Obj {
  const load: Value = NsOp(4, undefined, undefined, undefined);
  if (load === undefined) {
    return DynamicImport(specifier, options);
  }
  const promise: Obj = PromiseCreate(RealmIntrinsic("PromisePrototype"));
  try {
    const name: Str = ToString(specifier);
    // The `type` attribute (the only one djs supports), or undefined.
    let type: Value = undefined;
    if (options !== undefined) {
      if (!IsObject(options)) {
        throw NewTypeError("options must be an object");
      }
      const attributes: Value = Get(AsObj(options), "with");
      if (attributes !== undefined) {
        if (!IsObject(attributes)) {
          throw NewTypeError("options.with must be an object");
        }
        const entries: Obj = EnumerableOwnProperties(AsObj(attributes), 2);
        const n: f64 = LengthOfArrayLike(entries);
        let unsupported: Value = undefined;
        for (let i: f64 = 0; i < n; i++) {
          const entry: Obj = AsObj(Get(entries, i));
          const key: Value = Get(entry, 0);
          const value: Value = Get(entry, 1);
          if (!IsString(value)) {
            throw NewTypeError("module attribute values must be strings");
          }
          if (key === "type") {
            type = value;
          } else if (unsupported === undefined) {
            unsupported = key;
          }
        }
        if (unsupported !== undefined) {
          // AllImportAttributesSupported.
          throw NewSyntaxError("unsupported import attribute");
        }
      }
    }
    const loaded: Value = Call(load, undefined, name, referrer, type);
    PerformPromiseThen(
      AsObj(loaded),
      ModuleClosure(13, promise, 1),
      ModuleClosure(14, promise, 1),
      undefined,
    );
  } catch (e) {
    RejectPromise(promise, e);
  }
  return promise;
}

/**
 * ContinueDynamicImport's fulfilled path, 16.2.1.10.1, once the host loaded and linked the
 * graph of key: evaluates it and resolves promise with its namespace (a thenable namespace
 * is adopted), or rejects it.
 */
function FinishImport(promise: Obj, key: Value): void {
  try {
    const m: Obj = AsObj(NsOp(5, key, undefined, undefined));
    const evaluated: Obj = ModuleEvaluate(m);
    PerformPromiseThen(evaluated, ModuleClosure(12, m, 0), undefined, promise);
  } catch (e) {
    RejectPromise(promise, e);
  }
}

/** InnerModuleEvaluation ( module, stack, index ), 16.2.1.5.3.1. */
function InnerModuleEvaluation(m: Obj, state: Obj): void {
  const status: f64 = AsNumber(RecordGet(m, 1));
  if (status === 5) {
    throw RecordGet(m, 5);
  }
  if (status !== 2) {
    // Evaluating (a cycle), evaluating-async or evaluated (synthetic modules too).
    return;
  }
  const index: f64 = AsNumber(RecordGet(state, 1));
  RecordSet(m, 1, 3);
  RecordSet(m, 6, index);
  RecordSet(m, 7, index);
  RecordSet(m, 15, 0);
  RecordSet(state, 1, index + 1);
  RecordSet(m, 10, RecordGet(state, 0));
  RecordSet(state, 0, m);
  const requested: Obj = AsObj(RecordGet(m, 3));
  const n: f64 = AsNumber(RecordGet(m, 4));
  for (let i: f64 = 0; i < n; i++) {
    let r: Obj = AsObj(RecordGet(requested, F64ToI32(i)));
    InnerModuleEvaluation(r, state);
    if (AsNumber(RecordGet(r, 1)) === 3) {
      const ancestor: f64 = AsNumber(RecordGet(r, 7));
      if (ancestor < AsNumber(RecordGet(m, 7))) {
        RecordSet(m, 7, ancestor);
      }
    } else {
      const root: Value = RecordGet(r, 13);
      if (root === null) {
        // A synthetic module: evaluated, never async.
        continue;
      }
      r = AsObj(root);
      if (AsNumber(RecordGet(r, 1)) === 5) {
        throw RecordGet(r, 5);
      }
    }
    if (AsNumber(RecordGet(r, 14)) > 0) {
      RecordSet(m, 15, AsNumber(RecordGet(m, 15)) + 1);
      let parents: Value = RecordGet(r, 16);
      if (parents === null) {
        parents = ArrayCreate(0);
        RecordSet(r, 16, parents);
      }
      ArrayPushDense(AsObj(parents), m);
    }
  }
  const pending: f64 = AsNumber(RecordGet(m, 15));
  if (pending > 0 || HasTLA(m)) {
    RecordSet(m, 14, NsOp(7, undefined, undefined, undefined));
    if (pending === 0) {
      ExecuteAsyncModule(m);
    }
  } else {
    Call(RecordGet(m, 2), undefined);
  }
  if (AsNumber(RecordGet(m, 7)) === index) {
    // The root of a strongly connected component: it and everything above it are done.
    let done: boolean = false;
    while (!done) {
      const t: Obj = AsObj(RecordGet(state, 0));
      RecordSet(state, 0, RecordGet(t, 10));
      RecordSet(t, 10, null);
      RecordSet(t, 1, AsNumber(RecordGet(t, 14)) === 0 ? 4 : 6);
      RecordSet(t, 13, m);
      done = t === m;
    }
  }
}

/** ExecuteAsyncModule ( module ), 16.2.1.5.3.2: runs the async body of m. */
function ExecuteAsyncModule(m: Obj): void {
  const p: Value = Call(RecordGet(m, 2), undefined);
  PerformPromiseThen(AsObj(p), ModuleClosure(10, m, 0), ModuleClosure(11, m, 1), undefined);
}

/**
 * GatherAvailableAncestors ( module, execList ), 16.2.1.5.3.3: the async parents of m that
 * have no pending dependency left, into list.
 */
function GatherAvailableAncestors(m: Obj, list: Obj): void {
  const parents: Value = RecordGet(m, 16);
  if (parents === null) {
    return;
  }
  const n: f64 = LengthOfArrayLike(AsObj(parents));
  for (let i: f64 = 0; i < n; i++) {
    const p: Obj = AsObj(Get(AsObj(parents), i));
    if (ListContains(list, p)) {
      continue;
    }
    if (AsNumber(RecordGet(AsObj(RecordGet(p, 13)), 1)) === 5) {
      continue;
    }
    const pending: f64 = AsNumber(RecordGet(p, 15)) - 1;
    RecordSet(p, 15, pending);
    if (pending === 0) {
      ArrayPushDense(list, p);
      if (!HasTLA(p)) {
        GatherAvailableAncestors(p, list);
      }
    }
  }
}

/** Whether array list holds v. */
function ListContains(list: Obj, v: Value): boolean {
  const n: f64 = LengthOfArrayLike(list);
  for (let i: f64 = 0; i < n; i++) {
    if (Get(list, i) === v) {
      return true;
    }
  }
  return false;
}

/** AsyncModuleExecutionFulfilled ( module ), 16.2.1.5.3.4. */
function AsyncModuleExecutionFulfilled(m: Obj): void {
  if (AsNumber(RecordGet(m, 1)) === 5) {
    return;
  }
  RecordSet(m, 14, -1);
  RecordSet(m, 1, 4);
  const capability: Value = RecordGet(m, 17);
  if (capability !== undefined) {
    FulfillPromise(AsObj(capability), undefined);
  }
  const list: Obj = ArrayCreate(0);
  GatherAvailableAncestors(m, list);
  // Sorted by [[AsyncEvaluationOrder]] (insertion sort: lists are short).
  const n: f64 = LengthOfArrayLike(list);
  for (let i: f64 = 1; i < n; i++) {
    const x: Value = Get(list, i);
    const order: f64 = AsNumber(RecordGet(AsObj(x), 14));
    let j: f64 = i - 1;
    while (j >= 0 && AsNumber(RecordGet(AsObj(Get(list, j)), 14)) > order) {
      CreateDataPropertyOrThrow(list, j + 1, Get(list, j));
      j = j - 1;
    }
    CreateDataPropertyOrThrow(list, j + 1, x);
  }
  for (let i: f64 = 0; i < n; i++) {
    const x: Obj = AsObj(Get(list, i));
    const status: f64 = AsNumber(RecordGet(x, 1));
    if (status === 4 || status === 5) {
      continue;
    }
    if (HasTLA(x)) {
      ExecuteAsyncModule(x);
      continue;
    }
    try {
      Call(RecordGet(x, 2), undefined);
    } catch (e) {
      AsyncModuleExecutionRejected(x, e);
      continue;
    }
    RecordSet(x, 14, -1);
    RecordSet(x, 1, 4);
    const c: Value = RecordGet(x, 17);
    if (c !== undefined) {
      FulfillPromise(AsObj(c), undefined);
    }
  }
}

/** AsyncModuleExecutionRejected ( module, error ), 16.2.1.5.3.5. */
function AsyncModuleExecutionRejected(m: Obj, error: Value): void {
  if (AsNumber(RecordGet(m, 1)) === 5) {
    return;
  }
  RecordSet(m, 5, error);
  RecordSet(m, 1, 5);
  RecordSet(m, 14, -1);
  // The module's own evaluation rejects before its parents' (the order test262's
  // rejection-order.js checks).
  const capability: Value = RecordGet(m, 17);
  if (capability !== undefined) {
    RejectPromise(AsObj(capability), error);
  }
  const parents: Value = RecordGet(m, 16);
  if (parents !== null) {
    const n: f64 = LengthOfArrayLike(AsObj(parents));
    for (let i: f64 = 0; i < n; i++) {
      AsyncModuleExecutionRejected(AsObj(Get(AsObj(parents), i)), error);
    }
  }
}

// A module namespace object (10.4.6) is a Proxy over a non-extensible target with the
// same keys (so that the proxy invariants hold), which also holds the namespace's data in
// internal slots. Its handler's traps get, set, getOwnPropertyDescriptor, defineProperty
// and ownKeys are builtin closures of ModuleOp; the other internal methods (has,
// deleteProperty, getPrototypeOf, ...) are the target's, which answers them as a
// namespace does.

/**
 * The module builtins' entry for Rust and the compiler (one intrinsic: each costs the
 * shipped module table space): `this` undefined is `import(target, key)` from referrer a
 * ([`ImportCall`], the compiler's call); `this` 5 evaluates module record target
 * ([`ModuleEvaluate`]); a Record [op, operand] is a reaction (ModuleClosure): 10 and 11 an
 * async module's body fulfilled or rejected, 12 an import's namespace, 13 an import's graph
 * loaded ([`FinishImport`]), 14 an import rejected; otherwise it is a namespace trap and
 * `this` (the builtin closure's data) the
 * internal method: 0 [[Get]] (10.4.6.8), 1 [[GetOwnProperty]] (10.4.6.5), 2
 * [[DefineOwnProperty]] (10.4.6.6), 3 [[OwnPropertyKeys]] (10.4.6.11), 4 [[Set]] (10.4.6.9:
 * always false).
 * @intrinsic
 */
function ModuleOp(this: Value, target: Value, key: Value, a: Value): Value {
  if (this === undefined) {
    return ImportCall(target, key, a);
  }
  if (IsObject(this)) {
    // A reaction: [op, operand].
    const r: Obj = AsObj(this);
    const op: f64 = AsNumber(RecordGet(r, 0));
    const operand: Value = RecordGet(r, 1);
    if (op === 10) {
      AsyncModuleExecutionFulfilled(AsObj(operand));
    } else if (op === 11) {
      AsyncModuleExecutionRejected(AsObj(operand), target);
    } else if (op === 12) {
      return NsOp(6, operand, undefined, undefined);
    } else if (op === 13) {
      FinishImport(AsObj(operand), target);
    } else {
      RejectPromise(AsObj(operand), target);
    }
    return undefined;
  }
  const op: i32 = F64ToI32(AsNumber(this));
  if (op === 5) {
    return ModuleEvaluate(AsObj(target));
  }
  if (op === 4) {
    return false;
  }
  return NsOp(op, AsObj(target), key, a);
}
