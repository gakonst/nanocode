// Number (ECMA-262 21.1) and the global number functions (19.2).

/**
 * Number ( value ), 21.1.1.1.
 * @constructor Number
 * @length 1
 * @fast
 */
function NumberConstructor(...args: Value[]): Value {
  let n: f64 = 0;
  if (LengthOfArrayLike(args) > 0) {
    const prim: Value = ToNumeric(Get(args, 0));
    if (IsBigInt(prim)) {
      n = BigIntToNumber(prim);
    } else {
      n = AsNumber(prim);
    }
  }
  if (new.target === undefined) {
    return n;
  }
  return WrapPrimitive(n, GetPrototypeFromConstructor(new.target, "NumberPrototype"));
}

/** ThisNumberValue ( value ), 21.1.3.7.1. */
function ThisNumberValue(value: Value): f64 {
  const n: Value = PrimitiveValue(value);
  if (!IsNumber(n)) {
    throw NewTypeError("not a number");
  }
  return AsNumber(n);
}

/**
 * Number.prototype.toString ( [ radix ] ), 21.1.3.6.
 * @builtin Number.prototype.toString
 * @length 1
 */
function NumberPrototypeToString(this: Value, radix?: Value): Str {
  const x: f64 = ThisNumberValue(this);
  let r: f64 = 10;
  if (radix !== undefined) {
    r = ToIntegerOrInfinity(radix);
  }
  return NumberToString(x, r);
}

/**
 * Number.prototype.toLocaleString ( [ reserved1 [ , reserved2 ] ] ), 21.1.3.4.
 * @builtin Number.prototype.toLocaleString
 */
function NumberPrototypeToLocaleString(this: Value): Str {
  return NumberToString(ThisNumberValue(this), 10);
}

/**
 * Number.prototype.valueOf ( ), 21.1.3.7.
 * @builtin Number.prototype.valueOf
 */
function NumberPrototypeValueOf(this: Value): f64 {
  return ThisNumberValue(this);
}

/**
 * Number.prototype.toFixed ( fractionDigits ), 21.1.3.3.
 * @builtin Number.prototype.toFixed
 * @fast
 */
function NumberPrototypeToFixed(this: Value, fractionDigits: Value): Str {
  const x: f64 = ThisNumberValue(this);
  return NumberToFixed(x, ToIntegerOrInfinity(fractionDigits));
}

/**
 * Number.prototype.toExponential ( fractionDigits ), 21.1.3.2.
 * @builtin Number.prototype.toExponential
 */
function NumberPrototypeToExponential(this: Value, fractionDigits: Value): Str {
  const x: f64 = ThisNumberValue(this);
  const f: f64 = ToIntegerOrInfinity(fractionDigits);
  if (fractionDigits === undefined) {
    return NumberToExponential(x, undefined);
  }
  return NumberToExponential(x, f);
}

/**
 * Number.prototype.toPrecision ( precision ), 21.1.3.5.
 * @builtin Number.prototype.toPrecision
 */
function NumberPrototypeToPrecision(this: Value, precision: Value): Str {
  const x: f64 = ThisNumberValue(this);
  if (precision === undefined) {
    return NumberToString(x, 10);
  }
  return NumberToPrecision(x, ToIntegerOrInfinity(precision));
}

/** Whether a Number is finite (not NaN or ±Infinity). */
function IsFiniteNumber(n: f64): boolean {
  return n === n && n !== 1 / 0 && n !== -1 / 0;
}

/**
 * Number.isFinite ( number ), 21.1.2.2.
 * @builtin Number.isFinite
 */
function NumberIsFinite(number: Value): boolean {
  return IsNumber(number) && IsFiniteNumber(AsNumber(number));
}

/**
 * Number.isInteger ( number ), 21.1.2.3.
 * @builtin Number.isInteger
 * @fast
 */
function NumberIsInteger(number: Value): boolean {
  if (!IsNumber(number)) {
    return false;
  }
  const n: f64 = AsNumber(number);
  return IsFiniteNumber(n) && ToIntegerOrInfinityNumber(n) === n;
}

/**
 * Number.isNaN ( number ), 21.1.2.4.
 * @builtin Number.isNaN
 */
function NumberIsNaN(number: Value): boolean {
  if (!IsNumber(number)) {
    return false;
  }
  const n: f64 = AsNumber(number);
  return n !== n;
}

/**
 * Number.isSafeInteger ( number ), 21.1.2.5.
 * @builtin Number.isSafeInteger
 */
function NumberIsSafeInteger(number: Value): boolean {
  if (!NumberIsInteger(number)) {
    return false;
  }
  const n: f64 = AsNumber(number);
  return n <= 9007199254740991 && n >= -9007199254740991;
}

/**
 * Number.EPSILON, 21.1.2.1.
 * @value Number.EPSILON
 */
function NumberEpsilon(): f64 {
  return 2.220446049250313e-16;
}

/**
 * Number.MAX_SAFE_INTEGER, 21.1.2.6.
 * @value Number.MAX_SAFE_INTEGER
 */
function NumberMaxSafeInteger(): f64 {
  return 9007199254740991;
}

/**
 * Number.MAX_VALUE, 21.1.2.7.
 * @value Number.MAX_VALUE
 */
function NumberMaxValue(): f64 {
  return 1.7976931348623157e308;
}

/**
 * Number.MIN_SAFE_INTEGER, 21.1.2.8.
 * @value Number.MIN_SAFE_INTEGER
 */
function NumberMinSafeInteger(): f64 {
  return -9007199254740991;
}

/**
 * Number.MIN_VALUE, 21.1.2.9.
 * @value Number.MIN_VALUE
 */
function NumberMinValue(): f64 {
  return 5e-324;
}

/**
 * Number.NaN, 21.1.2.10.
 * @value Number.NaN
 */
function NumberNaN(): f64 {
  return 0 / 0;
}

/**
 * Number.NEGATIVE_INFINITY, 21.1.2.11.
 * @value Number.NEGATIVE_INFINITY
 */
function NumberNegativeInfinity(): f64 {
  return -1 / 0;
}

/**
 * Number.POSITIVE_INFINITY, 21.1.2.14.
 * @value Number.POSITIVE_INFINITY
 */
function NumberPositiveInfinity(): f64 {
  return 1 / 0;
}

/**
 * parseFloat ( string ), 19.2.4 (also Number.parseFloat).
 * @builtin globalThis.parseFloat
 * @builtin Number.parseFloat
 */
function GlobalParseFloat(string: Value): f64 {
  return StringParseFloat(ToString(string));
}

/**
 * parseInt ( string, radix ), 19.2.5 (also Number.parseInt).
 * @builtin globalThis.parseInt
 * @builtin Number.parseInt
 */
function GlobalParseInt(string: Value, radix: Value): f64 {
  const s: Str = ToString(string);
  return StringParseInt(s, NumberToInt32(ToNumber(radix)));
}

/**
 * isNaN ( number ), 19.2.3.
 * @builtin globalThis.isNaN
 */
function GlobalIsNaN(number: Value): boolean {
  const n: f64 = ToNumber(number);
  return n !== n;
}

/**
 * isFinite ( number ), 19.2.2.
 * @builtin globalThis.isFinite
 */
function GlobalIsFinite(number: Value): boolean {
  return IsFiniteNumber(ToNumber(number));
}
