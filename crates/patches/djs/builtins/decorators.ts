// Decorators (TC39 2023-11, 0065, D1650): the runtime of the code compiler/decorators.rs
// emits, after qjs's js_apply_decorators.
//
// The decoration state of a class is a Record: 0 the metadata object, 1 the extra
// initializers of the static methods and accessors, 2 those of the instance ones, 3
// those of the class (Arrays). A decorated field or auto-accessor has a Record of its
// own: 0 its initializers (Array, in application order: the last decorator's first),
// 1 its extra initializers, 2 and 3 an auto-accessor's getter and setter.
//
// Flags (the compiler's DEC_*): the kind in bits 0-2 (0 class, 1 method, 2 getter, 3
// setter, 4 accessor, 5 field), 8 static, 16 private.
// @slim-omit: left out of the slim build (D2411).

/** The `kind` of a context object, also in qjs's messages. */
function DecoratorKindName(kind: i32): Str {
  if (kind === 0) {
    return "class";
  }
  if (kind === 1) {
    return "method";
  }
  if (kind === 2) {
    return "getter";
  }
  if (kind === 3) {
    return "setter";
  }
  if (kind === 4) {
    return "accessor";
  }
  return "field";
}

/** Appends v to the Array list. */
function DecoratorPush(list: Obj, v: Value): void {
  CreateDataPropertyOrThrow(list, LengthOfArrayLike(list), v);
}

/**
 * The decoration state of class `cls`: its metadata object inherits from the parent
 * class's Symbol.metadata (`extends null` has none).
 * @intrinsic
 */
function DecoratorStart(cls: Value, heritage: boolean): Value {
  let parent: Value = null;
  if (heritage) {
    const superClass: Value = GetPrototypeOf(AsObj(cls));
    if (IsObject(superClass) && superClass !== RealmIntrinsic("FunctionPrototype")) {
      parent = Get(AsObj(superClass), WellKnownSymbol("metadata"));
      if (parent === undefined) {
        parent = null;
      } else if (!IsObject(parent) && parent !== null) {
        throw NewTypeError("invalid parent class metadata");
      }
    }
  }
  const state: Obj = NewRecord(4);
  RecordSet(state, 0, ObjectCreate(parent));
  RecordSet(state, 1, ArrayCreate(0));
  RecordSet(state, 2, ArrayCreate(0));
  RecordSet(state, 3, ArrayCreate(0));
  return state;
}

/**
 * context.access of a public element: get, set and has of the property `key`, as the
 * kind has them.
 */
function DecoratorNewAccess(kind: i32, key: Value): Obj {
  const access: Obj = ObjectCreate(RealmIntrinsic("ObjectPrototype"));
  if (kind !== 3) {
    CreateDataPropertyOrThrow(access, "get", NewBuiltinClosure(21, key, 1));
  }
  if (kind === 3 || kind === 4 || kind === 5) {
    CreateDataPropertyOrThrow(access, "set", NewBuiltinClosure(22, key, 2));
  }
  CreateDataPropertyOrThrow(access, "has", NewBuiltinClosure(23, key, 1));
  return access;
}

/**
 * The context object of one decorator call; `adder` is the Record [extras, done] of its
 * addInitializer.
 */
function DecoratorNewContext(flags: i32, name: Value, access: Value, metadata: Value, adder: Obj): Obj {
  const kind: i32 = flags & 7;
  const ctx: Obj = ObjectCreate(RealmIntrinsic("ObjectPrototype"));
  CreateDataPropertyOrThrow(ctx, "kind", DecoratorKindName(kind));
  if (kind !== 0) {
    CreateDataPropertyOrThrow(ctx, "access", access);
    CreateDataPropertyOrThrow(ctx, "static", (flags & 8) !== 0);
    CreateDataPropertyOrThrow(ctx, "private", (flags & 16) !== 0);
  }
  CreateDataPropertyOrThrow(ctx, "name", name);
  CreateDataPropertyOrThrow(ctx, "addInitializer", NewBuiltinClosure(20, adder, 1));
  CreateDataPropertyOrThrow(ctx, "metadata", metadata);
  return ctx;
}

/** A decorator's result that must be undefined or a function. */
function DecoratorCheckResult(v: Value, kind: i32): void {
  if (v !== undefined && !IsCallable(v)) {
    throw NewTypeError(DecoratorKindName(kind) + " decorator must return a function or undefined");
  }
}

/** Property `name` of an accessor decorator's result: a function or undefined. */
function DecoratorAccessorFn(o: Obj, name: Str): Value {
  const f: Value = Get(o, name);
  if (f !== undefined && !IsCallable(f)) {
    throw NewTypeError("accessor decorator: '" + name + "' must be a function");
  }
  return f;
}

/**
 * ApplyDecoratorsToElementDefinition and ApplyDecoratorsToClassDefinition. `decs` is an
 * Array of decorator and receiver pairs in source order; they apply from the last one.
 * A class: `target` is the class, `key` its name (undefined for an anonymous class:
 * its `name` property); the result is the new class, with Symbol.metadata. A public
 * element: `target` is the home object, which the element is read from and redefined
 * on. A private element: `target` is the access object, `key` the `#name`, `value` the
 * method, getter or setter, or an auto-accessor's { get, set }; the result is the new
 * value. Fields and auto-accessors give their Record.
 * @intrinsic
 */
function ApplyDecorators(target: Value, key: Value, value: Value, decs: Value, stateV: Value, flags: i32): Value {
  const kind: i32 = flags & 7;
  const isPrivate: boolean = (flags & 16) !== 0;
  const state: Obj = AsObj(stateV);
  let name: Value = key;
  let access: Value = undefined;
  let cur: Value = undefined;
  let curSet: Value = undefined;
  if (kind === 0) {
    cur = target;
    if (!IsString(key)) {
      name = Get(AsObj(target), "name");
      if (!IsString(name)) {
        name = "";
      }
    }
  } else if (isPrivate) {
    access = target;
    if (kind === 4) {
      cur = Get(AsObj(value), "get");
      curSet = Get(AsObj(value), "set");
    } else {
      cur = value;
    }
  } else {
    if (IsNumber(key)) {
      name = ToStringPrimitive(key);
    }
    access = DecoratorNewAccess(kind, key);
    if (kind !== 5) {
      const desc: Value = GetOwnPropertyDescriptorObject(AsObj(target), key);
      if (desc !== undefined) {
        if (kind === 1) {
          cur = Get(AsObj(desc), "value");
        } else if (kind === 3) {
          cur = Get(AsObj(desc), "set");
        } else {
          cur = Get(AsObj(desc), "get");
          if (kind === 4) {
            curSet = Get(AsObj(desc), "set");
          }
        }
      }
    }
  }
  const metadata: Value = RecordGet(state, 0);
  let rec: Value = undefined;
  let extras: Value = undefined;
  if (kind === 4 || kind === 5) {
    const r: Obj = NewRecord(4);
    RecordSet(r, 0, ArrayCreate(0));
    RecordSet(r, 1, ArrayCreate(0));
    rec = r;
    extras = RecordGet(r, 1);
  } else if (kind === 0) {
    extras = RecordGet(state, 3);
  } else if ((flags & 8) !== 0) {
    extras = RecordGet(state, 1);
  } else {
    extras = RecordGet(state, 2);
  }
  let i: f64 = 0;
  if (decs !== undefined) {
    i = LengthOfArrayLike(AsObj(decs)) / 2;
  }
  while (i > 0) {
    i -= 1;
    const dec: Value = Get(AsObj(decs), 2 * i);
    const recv: Value = Get(AsObj(decs), 2 * i + 1);
    const adder: Obj = NewRecord(2);
    RecordSet(adder, 0, extras);
    RecordSet(adder, 1, false);
    const ctx: Obj = DecoratorNewContext(flags, name, access, metadata, adder);
    let arg: Value = cur;
    if (kind === 4) {
      const pair: Obj = ObjectCreate(RealmIntrinsic("ObjectPrototype"));
      CreateDataPropertyOrThrow(pair, "get", cur);
      CreateDataPropertyOrThrow(pair, "set", curSet);
      arg = pair;
    }
    let ret: Value = undefined;
    try {
      ret = Call(dec, recv, arg, ctx);
    } finally {
      // The decoration is finished: addInitializer() is no longer allowed.
      RecordSet(adder, 1, true);
    }
    if (kind === 5) {
      DecoratorCheckResult(ret, kind);
      if (ret !== undefined) {
        DecoratorPush(AsObj(RecordGet(AsObj(rec), 0)), ret);
      }
    } else if (kind === 4) {
      if (IsObject(ret)) {
        const g: Value = DecoratorAccessorFn(AsObj(ret), "get");
        const s: Value = DecoratorAccessorFn(AsObj(ret), "set");
        const init: Value = DecoratorAccessorFn(AsObj(ret), "init");
        if (g !== undefined) {
          cur = g;
        }
        if (s !== undefined) {
          curSet = s;
        }
        if (init !== undefined) {
          DecoratorPush(AsObj(RecordGet(AsObj(rec), 0)), init);
        }
      } else if (ret !== undefined) {
        throw NewTypeError("accessor decorator must return an object or undefined");
      }
    } else {
      DecoratorCheckResult(ret, kind);
      if (ret !== undefined) {
        cur = ret;
      }
    }
  }
  if (kind === 0) {
    DefineDataPropertyOrThrow(AsObj(cur), WellKnownSymbol("metadata"), metadata, 7);
    return cur;
  }
  if (kind === 5) {
    return rec;
  }
  if (kind === 4) {
    if (!isPrivate) {
      const d: Obj = ObjectCreate(null);
      CreateDataPropertyOrThrow(d, "get", cur);
      CreateDataPropertyOrThrow(d, "set", curSet);
      DecoratorDefine(AsObj(target), key, d);
    }
    RecordSet(AsObj(rec), 2, cur);
    RecordSet(AsObj(rec), 3, curSet);
    return rec;
  }
  if (isPrivate) {
    return cur;
  }
  const d: Obj = ObjectCreate(null);
  if (kind === 1) {
    CreateDataPropertyOrThrow(d, "value", cur);
  } else if (kind === 2) {
    CreateDataPropertyOrThrow(d, "get", cur);
  } else {
    CreateDataPropertyOrThrow(d, "set", cur);
  }
  DecoratorDefine(AsObj(target), key, d);
  return undefined;
}

/** DefinePropertyOrThrow with a descriptor record. */
function DecoratorDefine(o: Obj, key: Value, desc: Obj): void {
  if (!DefineOwnPropertyFromRecord(o, key, desc)) {
    throw NewTypeError("cannot redefine property");
  }
}

/**
 * The initializers of a decorated field or auto-accessor on its initial value, the
 * first decorator's last.
 * @intrinsic
 */
function DecoratorRunInits(value: Value, rec: Value, thisValue: Value): Value {
  const inits: Obj = AsObj(RecordGet(AsObj(rec), 0));
  let v: Value = value;
  let i: f64 = LengthOfArrayLike(inits);
  while (i > 0) {
    i -= 1;
    v = Call(Get(inits, i), thisValue, v);
  }
  return v;
}

/**
 * Calls the extra initializers in slot i of Record r (a state or an element record)
 * with `this` = thisValue, in the order they were added.
 * @intrinsic
 */
function DecoratorRunExtras(r: Value, i: i32, thisValue: Value): void {
  const list: Obj = AsObj(RecordGet(AsObj(r), i));
  const n: f64 = LengthOfArrayLike(list);
  for (let j: f64 = 0; j < n; j++) {
    Call(Get(list, j), thisValue);
  }
}

/**
 * context.addInitializer ( initializer ): `this` is the Record [extras, done].
 * @intrinsic
 */
function DecoratorAddInitializer(this: Value, initializer: Value): void {
  const adder: Obj = AsObj(this);
  if (AsBool(RecordGet(adder, 1))) {
    throw NewTypeError("addInitializer() called after the decoration");
  }
  if (!IsCallable(initializer)) {
    throw NewTypeError("initializer must be a function");
  }
  DecoratorPush(AsObj(RecordGet(adder, 0)), initializer);
}

/**
 * context.access.get ( o ) of a public element: `this` is the key.
 * @intrinsic
 */
function DecoratorAccessGet(this: Value, o: Value): Value {
  if (!IsObject(o)) {
    throw NewTypeError("not an object");
  }
  return Get(AsObj(o), this);
}

/**
 * context.access.set ( o, value ) of a public element: `this` is the key.
 * @intrinsic
 */
function DecoratorAccessSet(this: Value, o: Value, value: Value): void {
  if (!IsObject(o)) {
    throw NewTypeError("not an object");
  }
  Set(AsObj(o), this, value);
}

/**
 * context.access.has ( o ) of a public element: `this` is the key.
 * @intrinsic
 */
function DecoratorAccessHas(this: Value, o: Value): boolean {
  if (!IsObject(o)) {
    throw NewTypeError("not an object");
  }
  return HasProperty(AsObj(o), this);
}

// ---- TypeScript legacy decorators (experimentalDecorators, 0065 part 2) --------------

/** Reflect[name] (the global Reflect) if it is a function, else undefined: tsc's helpers. */
function DecoratorReflectFn(reflect: Value, name: Str): Value {
  if (!IsObject(reflect)) {
    return undefined;
  }
  const f: Value = Get(AsObj(reflect), name);
  return IsCallable(f) ? f : undefined;
}

/**
 * tsc's __decorate(decs, target, key, desc) for one class element or the class
 * (`kind` 0 class, 5 field, else a method, getter, setter or auto-accessor): uses
 * Reflect.decorate if it exists, else applies the decorators from the last one; a
 * truthy result replaces the descriptor (the class), which is defined on the target.
 * Returns the new class (undefined for an element).
 * @intrinsic
 */
function ApplyLegacyDecorators(target: Value, key: Value, decs: Value, kind: i32): Value {
  let r: Value = undefined;
  if (kind === 0) {
    r = target;
  } else if (kind !== 5) {
    r = ObjectGetOwnPropertyDescriptor(target, key);
  }
  const reflect: Value = Get(GlobalObject(), "Reflect");
  const decorate: Value = DecoratorReflectFn(reflect, "decorate");
  if (decorate !== undefined) {
    if (kind === 0) {
      r = Call(decorate, reflect, decs, target);
    } else {
      r = Call(decorate, reflect, decs, target, key, r);
    }
  } else {
    const list: Obj = AsObj(decs);
    let i: f64 = LengthOfArrayLike(list);
    while (i > 0) {
      i -= 1;
      const dec: Value = Get(list, i);
      if (!ToBoolean(dec)) {
        continue;
      }
      const res: Value = kind === 0 ? Call(dec, undefined, r) : Call(dec, undefined, target, key, r);
      if (ToBoolean(res)) {
        r = res;
      }
    }
  }
  if (kind === 0) {
    return r;
  }
  if (ToBoolean(r)) {
    ObjectDefineProperty(target, key, r);
  }
  return undefined;
}

/**
 * tsc's __param(index, dec): the decorator of parameter `index` as a method decorator.
 * @intrinsic
 */
function DecoratorParam(dec: Value, index: Value): Value {
  const data: Obj = NewRecord(2);
  RecordSet(data, 0, dec);
  RecordSet(data, 1, index);
  return NewBuiltinClosure(24, data, 2);
}

/**
 * The function of DecoratorParam: calls the parameter decorator with (target, key,
 * index); `this` is the Record [dec, index].
 * @intrinsic
 */
function DecoratorParamCall(this: Value, target: Value, key: Value): void {
  const data: Obj = AsObj(this);
  Call(RecordGet(data, 0), undefined, target, key, RecordGet(data, 1));
}

/**
 * tsc's __metadata(key, value): Reflect.metadata(key, value) if it exists, else
 * undefined (a decorator that is skipped).
 * @intrinsic
 */
function DecoratorMetadata(key: Value, value: Value): Value {
  const reflect: Value = Get(GlobalObject(), "Reflect");
  const f: Value = DecoratorReflectFn(reflect, "metadata");
  if (f === undefined) {
    return undefined;
  }
  return Call(f, reflect, key, value);
}

/**
 * Symbol.metadata, 20.4.2 (decorators proposal).
 * @value Symbol.metadata
 */
function SymbolMetadata(): Sym {
  return WellKnownSymbol("metadata");
}
