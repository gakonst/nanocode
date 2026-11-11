// BigInt (ECMA-262 21.2) and the BigInt conversions (7.1.13).
// @slim-omit: left out of the slim build (D2411).

/**
 * ToBigInt ( argument ), 7.1.13.
 * @intrinsic
 */
function ToBigInt(v: Value): Value {
  return ToBigIntPrimitive(ToPrimitive(v, "number"));
}

/**
 * BigInt ( value ), 21.2.1.1.
 * @constructor BigInt
 */
function BigIntConstructor(value: Value): Value {
  if (new.target !== undefined) {
    throw NewTypeError("BigInt is not a constructor");
  }
  const prim: Value = ToPrimitive(value, "number");
  if (IsNumber(prim)) {
    return NumberToBigInt(AsNumber(prim));
  }
  // qjs's constructor says "BigInt" where its ToBigInt says "bigint".
  if (prim === undefined || prim === null || IsSymbol(prim)) {
    throw NewTypeError("cannot convert to BigInt");
  }
  return ToBigIntPrimitive(prim);
}

/**
 * BigInt.asIntN ( bits, bigint ), 21.2.2.1.
 * @builtin BigInt.asIntN
 */
function BigIntAsIntNBuiltin(bits: Value, bigint: Value): Value {
  const n: f64 = ToIndex(bits);
  return BigIntAsIntN(n, ToBigInt(bigint));
}

/**
 * BigInt.asUintN ( bits, bigint ), 21.2.2.2.
 * @builtin BigInt.asUintN
 */
function BigIntAsUintNBuiltin(bits: Value, bigint: Value): Value {
  const n: f64 = ToIndex(bits);
  return BigIntAsUintN(n, ToBigInt(bigint));
}

/** ThisBigIntValue ( value ), 21.2.3.4.1. */
function ThisBigIntValue(value: Value): Value {
  const b: Value = PrimitiveValue(value);
  if (!IsBigInt(b)) {
    throw NewTypeError("not a BigInt");
  }
  return b;
}

/**
 * BigInt.prototype.toString ( [ radix ] ), 21.2.3.3.
 * @builtin BigInt.prototype.toString
 */
function BigIntPrototypeToString(this: Value, radix?: Value): Str {
  const x: Value = ThisBigIntValue(this);
  let r: f64 = 10;
  if (radix !== undefined) {
    r = ToIntegerOrInfinity(radix);
  }
  if (r < 2 || r > 36) {
    throw NewRangeError("radix must be between 2 and 36");
  }
  return BigIntToString(x, F64ToI32(r));
}

/**
 * BigInt.prototype.toLocaleString ( [ reserved1 [ , reserved2 ] ] ), 21.2.3.2. Without
 * Intl it is toString() (QuickJS passes the locale on as a radix; node does not).
 * @builtin BigInt.prototype.toLocaleString
 */
function BigIntPrototypeToLocaleString(this: Value): Str {
  return BigIntToString(ThisBigIntValue(this), 10);
}

/**
 * BigInt.prototype.valueOf ( ), 21.2.3.4.
 * @builtin BigInt.prototype.valueOf
 */
function BigIntPrototypeValueOf(this: Value): Value {
  return ThisBigIntValue(this);
}

/**
 * BigInt.prototype [ %Symbol.toStringTag% ], 21.2.3.5.
 * @value BigInt.prototype[Symbol.toStringTag]
 * @attributes configurable
 */
function BigIntPrototypeToStringTag(): Str {
  return "BigInt";
}
