// The Math object (ECMA-262 21.3). The functions are ToNumber plus a libm leaf (D1150).

/**
 * The Math object, 21.3: an ordinary object whose Symbol.toStringTag is "Math".
 * @value globalThis.Math
 * @attributes writable configurable
 */
function MathObject(): Obj {
  return ObjectCreate(RealmIntrinsic("ObjectPrototype"));
}

/**
 * Math [ %Symbol.toStringTag% ], 21.3.1.9.
 * @value Math[Symbol.toStringTag]
 * @attributes configurable
 */
function MathToStringTag(): Str {
  return "Math";
}

/**
 * Math.E, 21.3.1.1.
 * @value Math.E
 */
function MathE(): f64 {
  return 2.718281828459045;
}

/**
 * Math.LN10, 21.3.1.2.
 * @value Math.LN10
 */
function MathLN10(): f64 {
  return 2.302585092994046;
}

/**
 * Math.LN2, 21.3.1.3.
 * @value Math.LN2
 */
function MathLN2(): f64 {
  return 0.6931471805599453;
}

/**
 * Math.LOG10E, 21.3.1.4.
 * @value Math.LOG10E
 */
function MathLOG10E(): f64 {
  return 0.4342944819032518;
}

/**
 * Math.LOG2E, 21.3.1.5.
 * @value Math.LOG2E
 */
function MathLOG2E(): f64 {
  return 1.4426950408889634;
}

/**
 * Math.PI, 21.3.1.6.
 * @value Math.PI
 */
function MathPI(): f64 {
  return 3.141592653589793;
}

/**
 * Math.SQRT1_2, 21.3.1.7.
 * @value Math.SQRT1_2
 */
function MathSQRT12(): f64 {
  return 0.7071067811865476;
}

/**
 * Math.SQRT2, 21.3.1.8.
 * @value Math.SQRT2
 */
function MathSQRT2(): f64 {
  return 1.4142135623730951;
}

/**
 * Math.abs ( x ), 21.3.2.1.
 * @builtin Math.abs
 * @fast
 */
function MathAbs(x: Value): f64 {
  return F64Abs(ToNumber(x));
}

/**
 * Math.acos ( x ), 21.3.2.2.
 * @builtin Math.acos
 */
function MathAcos(x: Value): f64 {
  return F64Acos(ToNumber(x));
}

/**
 * Math.acosh ( x ), 21.3.2.3.
 * @builtin Math.acosh
 */
function MathAcosh(x: Value): f64 {
  return F64Acosh(ToNumber(x));
}

/**
 * Math.asin ( x ), 21.3.2.4.
 * @builtin Math.asin
 */
function MathAsin(x: Value): f64 {
  return F64Asin(ToNumber(x));
}

/**
 * Math.asinh ( x ), 21.3.2.5.
 * @builtin Math.asinh
 */
function MathAsinh(x: Value): f64 {
  return F64Asinh(ToNumber(x));
}

/**
 * Math.atan ( x ), 21.3.2.6.
 * @builtin Math.atan
 */
function MathAtan(x: Value): f64 {
  return F64Atan(ToNumber(x));
}

/**
 * Math.atanh ( x ), 21.3.2.7.
 * @builtin Math.atanh
 */
function MathAtanh(x: Value): f64 {
  return F64Atanh(ToNumber(x));
}

/**
 * Math.cbrt ( x ), 21.3.2.9.
 * @builtin Math.cbrt
 * @fast
 */
function MathCbrt(x: Value): f64 {
  return F64Cbrt(ToNumber(x));
}

/**
 * Math.ceil ( x ), 21.3.2.10.
 * @builtin Math.ceil
 * @fast
 */
function MathCeil(x: Value): f64 {
  return F64Ceil(ToNumber(x));
}

/**
 * Math.clz32 ( x ), 21.3.2.11.
 * @builtin Math.clz32
 */
function MathClz32(x: Value): f64 {
  return F64Clz32(ToNumber(x));
}

/**
 * Math.cos ( x ), 21.3.2.12.
 * @builtin Math.cos
 * @fast
 */
function MathCos(x: Value): f64 {
  return F64Cos(ToNumber(x));
}

/**
 * Math.cosh ( x ), 21.3.2.13.
 * @builtin Math.cosh
 */
function MathCosh(x: Value): f64 {
  return F64Cosh(ToNumber(x));
}

/**
 * Math.exp ( x ), 21.3.2.14.
 * @builtin Math.exp
 * @fast
 */
function MathExp(x: Value): f64 {
  return F64Exp(ToNumber(x));
}

/**
 * Math.expm1 ( x ), 21.3.2.15.
 * @builtin Math.expm1
 */
function MathExpm1(x: Value): f64 {
  return F64Expm1(ToNumber(x));
}

/**
 * Math.floor ( x ), 21.3.2.16.
 * @builtin Math.floor
 * @fast
 */
function MathFloor(x: Value): f64 {
  return F64Floor(ToNumber(x));
}

/**
 * Math.fround ( x ), 21.3.2.17.
 * @builtin Math.fround
 */
function MathFround(x: Value): f64 {
  return F64Fround(ToNumber(x));
}

/**
 * Math.f16round ( x ), 21.3.2.18.
 * @builtin Math.f16round
 */
function MathF16round(x: Value): f64 {
  return F64F16round(ToNumber(x));
}

/**
 * Math.sumPrecise ( items ) (proposal-math-sum): the exact sum of the Numbers of an
 * iterable, rounded once.
 * @builtin Math.sumPrecise
 */
function MathSumPrecise(items: Value): f64 {
  if (items === undefined || items === null) {
    throw NewTypeError("not an object");
  }
  const iterator: Obj = GetIterator(items);
  const next: Value = Get(iterator, "next");
  const list: Obj = ArrayCreate(0);
  let n: f64 = 0;
  while (true) {
    const result: Obj = IteratorNextResult(iterator, next);
    if (ToBoolean(Get(result, "done"))) {
      break;
    }
    const v: Value = Get(result, "value");
    if (!IsNumber(v)) {
      IteratorCloseThrow(iterator);
      throw NewTypeError("not a number");
    }
    CreateDataPropertyOrThrow(list, n, v);
    n = n + 1;
  }
  return F64SumPrecise(list);
}

/**
 * Math.log ( x ), 21.3.2.20.
 * @builtin Math.log
 * @fast
 */
function MathLog(x: Value): f64 {
  return F64Log(ToNumber(x));
}

/**
 * Math.log1p ( x ), 21.3.2.21.
 * @builtin Math.log1p
 */
function MathLog1p(x: Value): f64 {
  return F64Log1p(ToNumber(x));
}

/**
 * Math.log10 ( x ), 21.3.2.22.
 * @builtin Math.log10
 * @fast
 */
function MathLog10(x: Value): f64 {
  return F64Log10(ToNumber(x));
}

/**
 * Math.log2 ( x ), 21.3.2.23.
 * @builtin Math.log2
 * @fast
 */
function MathLog2(x: Value): f64 {
  return F64Log2(ToNumber(x));
}

/**
 * Math.round ( x ), 21.3.2.28.
 * @builtin Math.round
 * @fast
 */
function MathRound(x: Value): f64 {
  return F64Round(ToNumber(x));
}

/**
 * Math.sign ( x ), 21.3.2.29.
 * @builtin Math.sign
 * @fast
 */
function MathSign(x: Value): f64 {
  return F64Sign(ToNumber(x));
}

/**
 * Math.sin ( x ), 21.3.2.30.
 * @builtin Math.sin
 * @fast
 */
function MathSin(x: Value): f64 {
  return F64Sin(ToNumber(x));
}

/**
 * Math.sinh ( x ), 21.3.2.31.
 * @builtin Math.sinh
 */
function MathSinh(x: Value): f64 {
  return F64Sinh(ToNumber(x));
}

/**
 * Math.sqrt ( x ), 21.3.2.32.
 * @builtin Math.sqrt
 * @fast
 */
function MathSqrt(x: Value): f64 {
  return F64Sqrt(ToNumber(x));
}

/**
 * Math.tan ( x ), 21.3.2.33.
 * @builtin Math.tan
 */
function MathTan(x: Value): f64 {
  return F64Tan(ToNumber(x));
}

/**
 * Math.tanh ( x ), 21.3.2.34.
 * @builtin Math.tanh
 */
function MathTanh(x: Value): f64 {
  return F64Tanh(ToNumber(x));
}

/**
 * Math.trunc ( x ), 21.3.2.35.
 * @builtin Math.trunc
 * @fast
 */
function MathTrunc(x: Value): f64 {
  return F64Trunc(ToNumber(x));
}

/**
 * Math.atan2 ( y, x ), 21.3.2.8.
 * @builtin Math.atan2
 * @fast
 */
function MathAtan2(y: Value, x: Value): f64 {
  const ny: f64 = ToNumber(y);
  return F64Atan2(ny, ToNumber(x));
}

/**
 * Math.imul ( x, y ), 21.3.2.19.
 * @builtin Math.imul
 */
function MathImul(x: Value, y: Value): f64 {
  const a: f64 = ToNumber(x);
  return F64Imul(a, ToNumber(y));
}

/**
 * Math.pow ( base, exponent ), 21.3.2.26.
 * @builtin Math.pow
 * @fast
 */
function MathPow(base: Value, exponent: Value): f64 {
  const b: f64 = ToNumber(base);
  return NumberPow(b, ToNumber(exponent));
}

/** ToNumber of every element of `args` (a rest array), in place. */
function CoerceNumbers(args: Obj): f64 {
  const n: f64 = LengthOfArrayLike(args);
  if (AllNumbers(args)) {
    return n;
  }
  for (let i: f64 = 0; i < n; i++) {
    const v: Value = Get(args, i);
    if (!IsNumber(v)) {
      CreateDataPropertyOrThrow(args, i, ToNumber(v));
    }
  }
  return n;
}

/**
 * Math.max ( ...args ), 21.3.2.24.
 * @builtin Math.max
 * @length 2
 * @fast
 */
function MathMax(...args: Value[]): f64 {
  const n: f64 = CoerceNumbers(args);
  let highest: f64 = -1 / 0;
  for (let i: f64 = 0; i < n; i++) {
    const x: f64 = AsNumber(Get(args, i));
    if (x !== x) {
      return x;
    }
    // +0 is larger than -0.
    if (x > highest || (x === 0 && highest === 0 && 1 / x > 0)) {
      highest = x;
    }
  }
  return highest;
}

/**
 * Math.min ( ...args ), 21.3.2.25.
 * @builtin Math.min
 * @length 2
 * @fast
 */
function MathMin(...args: Value[]): f64 {
  const n: f64 = CoerceNumbers(args);
  let lowest: f64 = 1 / 0;
  for (let i: f64 = 0; i < n; i++) {
    const x: f64 = AsNumber(Get(args, i));
    if (x !== x) {
      return x;
    }
    // -0 is smaller than +0.
    if (x < lowest || (x === 0 && lowest === 0 && 1 / x < 0)) {
      lowest = x;
    }
  }
  return lowest;
}

/**
 * Math.hypot ( ...args ), 21.3.2.18 (pairwise libm hypot, as qjs does).
 * @builtin Math.hypot
 * @length 2
 */
function MathHypot(...args: Value[]): f64 {
  const n: f64 = CoerceNumbers(args);
  let r: f64 = 0;
  for (let i: f64 = 0; i < n; i++) {
    r = F64Hypot(r, AsNumber(Get(args, i)));
  }
  return r;
}

/**
 * Math.random ( ), 21.3.2.27.
 * @builtin Math.random
 */
function MathRandomBuiltin(): f64 {
  return MathRandom();
}
