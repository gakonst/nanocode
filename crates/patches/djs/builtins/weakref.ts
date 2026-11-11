// WeakRef and FinalizationRegistry (ECMA-262 26.1, 26.2; 0051, D1513). The weak
// storage is heap/weakref.rs; collections only happen at safepoints, and the cleanup
// jobs are queued after a collection (or when the host asks, VmOptions).
// @slim-omit: left out of the slim build (D2411).

/**
 * WeakRef ( target ), 26.1.1.1.
 * @constructor WeakRef
 */
function WeakRefConstructor(target: Value): Obj {
  if (new.target === undefined) {
    throw NewTypeError("constructor requires 'new'");
  }
  if (!CanBeHeldWeakly(target)) {
    throw NewTypeError("invalid target");
  }
  return WeakRefCreate(GetPrototypeFromConstructor(new.target, "WeakRefPrototype"), target);
}

/**
 * WeakRef.prototype.deref ( ), 26.1.3.2.
 * @builtin WeakRef.prototype.deref
 */
function WeakRefPrototypeDeref(this: Value): Value {
  return WeakRefDeref(this);
}

/**
 * WeakRef.prototype [ %Symbol.toStringTag% ], 26.1.3.3.
 * @value WeakRef.prototype[Symbol.toStringTag]
 * @attributes configurable
 */
function WeakRefPrototypeToStringTag(): Str {
  return "WeakRef";
}

/**
 * FinalizationRegistry ( cleanupCallback ), 26.2.1.1.
 * @constructor FinalizationRegistry
 */
function FinalizationRegistryConstructor(cleanupCallback: Value): Obj {
  if (new.target === undefined) {
    throw NewTypeError("constructor requires 'new'");
  }
  if (!IsCallable(cleanupCallback)) {
    throw NewTypeError("argument must be a function");
  }
  const proto: Obj = GetPrototypeFromConstructor(new.target, "FinalizationRegistryPrototype");
  return FinalizationRegistryCreate(proto, cleanupCallback);
}

/**
 * FinalizationRegistry.prototype.register ( target, heldValue [ , unregisterToken ] ),
 * 26.2.3.2.
 * @builtin FinalizationRegistry.prototype.register
 * @length 2
 */
function FinalizationRegistryPrototypeRegister(this: Value, target: Value, heldValue: Value, unregisterToken?: Value): void {
  const r: Obj = ThisFinalizationRegistry(this);
  if (!CanBeHeldWeakly(target)) {
    throw NewTypeError("invalid target");
  }
  if (SameValue(target, heldValue)) {
    throw NewTypeError("held value cannot be the target");
  }
  if (unregisterToken !== undefined && !CanBeHeldWeakly(unregisterToken)) {
    throw NewTypeError("invalid unregister token");
  }
  FinalizationRegistryRegister(r, target, heldValue, unregisterToken);
}

/**
 * FinalizationRegistry.prototype.unregister ( unregisterToken ), 26.2.3.3.
 * @builtin FinalizationRegistry.prototype.unregister
 */
function FinalizationRegistryPrototypeUnregister(this: Value, unregisterToken: Value): boolean {
  const r: Obj = ThisFinalizationRegistry(this);
  if (!CanBeHeldWeakly(unregisterToken)) {
    throw NewTypeError("invalid unregister token");
  }
  return FinalizationRegistryUnregister(r, unregisterToken);
}

/**
 * FinalizationRegistry.prototype [ %Symbol.toStringTag% ], 26.2.3.4.
 * @value FinalizationRegistry.prototype[Symbol.toStringTag]
 * @attributes configurable
 */
function FinalizationRegistryPrototypeToStringTag(): Str {
  return "FinalizationRegistry";
}

/**
 * The job of HostEnqueueFinalizationRegistryCleanupJob: CleanupFinalizationRegistry
 * ( finalizationRegistry ), 9.13. A callback's exception ends the job; the spec leaves
 * reporting it to the host, and djs drops it (D1513). The other dead cells wait for the
 * next collection.
 * @intrinsic
 */
function FinalizationRegistryCleanupJob(r: Obj): void {
  FinalizationRegistrySetQueued(r, false);
  const callback: Value = RecordGet(r, 0);
  while (true) {
    const i: i32 = FinalizationRegistryNextDead(r);
    if (i < 0) {
      return;
    }
    const held: Value = FinalizationRegistryTake(r, i);
    try {
      Call(callback, undefined, held);
    } catch (e) {
      return;
    }
  }
}
