// Function.prototype (ECMA-262 20.2.3). The Function constructor is not supported (it
// would need eval-like compilation).

/**
 * Function.prototype.call ( thisArg, ...args ), 20.2.3.3.
 * @builtin Function.prototype.call
 * @length 1
 */
function FunctionPrototypeCall(this: Value, thisArg?: Value, ...args: Value[]): Value {
  if (!IsCallable(this)) {
    throw NewTypeError("not a function");
  }
  return CallWithArray(this, thisArg, args);
}

/** CreateListFromArrayLike ( obj ), 7.3.20, as a dense array. */
function CreateListFromArrayLike(obj: Value): Obj {
  if (!IsObject(obj)) {
    throw NewTypeError("not a object");
  }
  const o: Obj = AsObj(obj);
  const len: f64 = LengthOfArrayLike(o);
  const list: Obj = ArrayCreate(0);
  for (let k: f64 = 0; k < len; k++) {
    CreateDataPropertyOrThrow(list, k, Get(o, k));
  }
  return list;
}

/**
 * Function.prototype.apply ( thisArg, argArray ), 20.2.3.1.
 * @builtin Function.prototype.apply
 */
function FunctionPrototypeApply(this: Value, thisArg: Value, argArray: Value): Value {
  if (!IsCallable(this)) {
    throw NewTypeError("not a function");
  }
  if (argArray === undefined || argArray === null) {
    return Call(this, thisArg);
  }
  // A dense array is read without observable effects: pass it as is (D1561).
  if (IsDenseArray(argArray)) {
    return CallWithArray(this, thisArg, AsObj(argArray));
  }
  return CallWithArray(this, thisArg, CreateListFromArrayLike(argArray));
}

/**
 * Function.prototype.bind ( thisArg, ...args ), 20.2.3.2.
 * @builtin Function.prototype.bind
 * @length 1
 */
function FunctionPrototypeBind(this: Value, thisArg?: Value, ...args: Value[]): Obj {
  if (!IsCallable(this)) {
    throw NewTypeError("not a function");
  }
  const target: Fn = AsFn(this);
  const f: Obj = BoundFunctionCreate(target, thisArg, args);
  if (IsProxy(target)) {
    // BoundFunctionCreate's target.[[GetPrototypeOf]]() is a trap for a proxy; the leaf
    // took the proxy's own (null) prototype.
    SetPrototypeOf(f, GetPrototypeOf(target));
  }
  let l: f64 = 0;
  if (HasOwnProperty(target, "length")) {
    const targetLen: Value = Get(target, "length");
    if (IsNumber(targetLen)) {
      const n: f64 = AsNumber(targetLen);
      if (n === 1 / 0) {
        l = n;
      } else if (n !== -1 / 0) {
        l = ToIntegerOrInfinityNumber(n) - LengthOfArrayLike(args);
        if (l < 0) {
          l = 0;
        }
      }
    }
  }
  DefineDataPropertyOrThrow(f, "length", l, 4);
  const name: Value = Get(target, "name");
  const nameStr: Str = IsString(name) ? AsStr(name) : "";
  DefineDataPropertyOrThrow(f, "name", "bound " + nameStr, 4);
  return f;
}

/**
 * Function.prototype.toString ( ), 20.2.3.5.
 * @builtin Function.prototype.toString
 */
function FunctionPrototypeToString(this: Value): Str {
  if (IsProxy(this) && IsCallable(this)) {
    return "function () {\n    [native code]\n}";
  }
  return FunctionToString(this);
}

/**
 * Function ( ...parameterArgs, bodyArg ), 20.2.1.1 (0059).
 * @constructor Function
 * @length 1
 */
function FunctionConstructor(...args: Value[]): Value {
  return CreateDynamicFunction(new.target, 0, "FunctionPrototype", args);
}

/** OrdinaryHasInstance ( C, O ), 7.3.22. */
function OrdinaryHasInstance(c: Value, o: Value): boolean {
  if (!IsCallable(c)) {
    return false;
  }
  const target: Value = BoundTargetFunction(c);
  if (target !== undefined) {
    return InstanceofOperator(o, target);
  }
  if (!IsObject(o)) {
    return false;
  }
  const p: Value = Get(AsObj(c), "prototype");
  if (!IsObject(p)) {
    throw NewTypeError("operand 'prototype' property is not an object");
  }
  let v: Value = GetPrototypeOf(AsObj(o));
  while (v !== null) {
    if (v === p) {
      return true;
    }
    v = GetPrototypeOf(AsObj(v));
  }
  return false;
}

/**
 * InstanceofOperator ( V, target ), 13.10.2. The interpreter calls it for `instanceof`
 * when the target has its own Symbol.hasInstance or a `prototype` getter.
 * @intrinsic
 */
function InstanceofOperator(v: Value, target: Value): boolean {
  if (!IsObject(target)) {
    throw NewTypeError("invalid 'instanceof' right operand");
  }
  const instOfHandler: Value = GetMethod(target, WellKnownSymbol("hasInstance"));
  if (instOfHandler !== undefined) {
    return ToBoolean(Call(instOfHandler, target, v));
  }
  if (!IsCallable(target)) {
    throw NewTypeError("invalid 'instanceof' right operand");
  }
  return OrdinaryHasInstance(target, v);
}

/**
 * ClassDefinitionEvaluation steps 8.e-h, 15.7.14: checks a class heritage (`extends
 * parent`) and returns protoParent, the [[Prototype]] of the class's `prototype`.
 * `define_class` does the rest (0040).
 * @intrinsic
 */
function ClassHeritage(parent: Value): Value {
  if (parent === null) {
    return null;
  }
  if (!IsConstructor(parent)) {
    throw NewTypeError("parent class must be constructor");
  }
  const protoParent: Value = Get(AsObj(parent), "prototype");
  if (protoParent !== null && !IsObject(protoParent)) {
    throw NewTypeError("parent prototype must be an object or null");
  }
  return protoParent;
}

/**
 * Function.prototype [ %Symbol.hasInstance% ] ( V ), 20.2.3.6.
 * @builtin Function.prototype[Symbol.hasInstance]
 * @attributes none
 */
function FunctionPrototypeHasInstance(this: Value, v: Value): boolean {
  return OrdinaryHasInstance(this, v);
}

/**
 * %ThrowTypeError% ( ), 10.2.4.1: the `caller` and `arguments` accessors of
 * Function.prototype (AddRestrictedFunctionProperties, 10.2.4).
 * @getter Function.prototype.caller
 * @setter Function.prototype.caller
 * @getter Function.prototype.arguments
 * @setter Function.prototype.arguments
 * @intrinsic
 */
function ThrowTypeError(this: Value, v?: Value): void {
  throw NewTypeError("invalid property access");
}

/**
 * Function.prototype itself (20.2.3): a builtin function that accepts any arguments and
 * returns undefined. The realm creates Function.prototype as a builtin closure of it.
 * @intrinsic
 */
function FunctionPrototypeBehavior(this: Value): void {
  return;
}

/**
 * Function.prototype.length, 20.2.3: Function.prototype is a function of no parameters.
 * @value Function.prototype.length
 * @attributes configurable
 */
function FunctionPrototypeLength(): f64 {
  return 0;
}

/**
 * Function.prototype.name, 20.2.3: the empty string.
 * @value Function.prototype.name
 * @attributes configurable
 */
function FunctionPrototypeName(): Str {
  return "";
}
