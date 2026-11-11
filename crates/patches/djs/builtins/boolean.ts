// Boolean (ECMA-262 20.3).

/**
 * Boolean ( value ), 20.3.1.1.
 * @constructor Boolean
 */
function BooleanConstructor(value: Value): Value {
  const b: boolean = ToBoolean(value);
  if (new.target === undefined) {
    return b;
  }
  return WrapPrimitive(b, GetPrototypeFromConstructor(new.target, "BooleanPrototype"));
}

/** ThisBooleanValue ( value ), 20.3.3.3.1. */
function ThisBooleanValue(value: Value): boolean {
  const b: Value = PrimitiveValue(value);
  if (!IsBoolean(b)) {
    throw NewTypeError("not a boolean");
  }
  return AsBool(b);
}

/**
 * Boolean.prototype.toString ( ), 20.3.3.2.
 * @builtin Boolean.prototype.toString
 */
function BooleanPrototypeToString(this: Value): Str {
  return ThisBooleanValue(this) ? "true" : "false";
}

/**
 * Boolean.prototype.valueOf ( ), 20.3.3.3.
 * @builtin Boolean.prototype.valueOf
 */
function BooleanPrototypeValueOf(this: Value): boolean {
  return ThisBooleanValue(this);
}
