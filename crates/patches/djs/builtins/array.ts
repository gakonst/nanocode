// Array.prototype methods (ECMA-262 23.1.3).

/**
 * ArraySpeciesCreate ( originalArray, length ), 10.4.2.3 (one realm).
 * @intrinsic
 * @fast
 */
function ArraySpeciesCreate(original: Obj, length: f64): Obj {
  if (!IsArray(original)) {
    return ArrayCreate(length);
  }
  let c: Value = Get(original, "constructor");
  if (IsObject(c)) {
    c = Get(AsObj(c), WellKnownSymbol("species"));
    if (c === null) {
      c = undefined;
    }
  }
  if (c === undefined) {
    return ArrayCreate(length);
  }
  // QuickJS's messages: it calls the constructor without checking first.
  if (!IsCallable(c)) {
    throw NewTypeError("not a function");
  }
  if (!IsConstructor(c)) {
    throw NewNotConstructorError(c);
  }
  return Construct(c, length);
}

/**
 * Array.prototype.forEach ( callbackfn [ , thisArg ] ), 23.1.3.15.
 * @builtin Array.prototype.forEach
 */
function ArrayPrototypeForEach(this: Value, callbackfn: Value, thisArg?: Value): void {
  const o: Obj = ToObject(this);
  const len: f64 = LengthOfArrayLike(o);
  if (!IsCallable(callbackfn)) {
    throw NewTypeError("not a function");
  }
  // The dense part in one intrinsic (0137), then the spec's loop from where it stopped.
  // An empty array skips the intrinsic: it costs an exit from the tier-0 loop (D2307).
  for (let k: f64 = len > 0 ? EachDense(0, o, 0, len, callbackfn, thisArg, undefined) : 0; k < len; k++) {
    if (HasProperty(o, k)) {
      Call(callbackfn, thisArg, Get(o, k), k, o);
    }
  }
}

/**
 * Array.prototype.map ( callbackfn [ , thisArg ] ), 23.1.3.21.
 * @builtin Array.prototype.map
 */
function ArrayPrototypeMap(this: Value, callbackfn: Value, thisArg?: Value): Obj {
  const o: Obj = ToObject(this);
  const len: f64 = LengthOfArrayLike(o);
  if (!IsCallable(callbackfn)) {
    throw NewTypeError("not a function");
  }
  const a: Obj = ArraySpeciesCreate(o, len);
  for (let k: f64 = len > 0 ? EachDense(1, o, 0, len, callbackfn, thisArg, a) : 0; k < len; k++) {
    if (HasProperty(o, k)) {
      const kValue: Value = Get(o, k);
      const mappedValue: Value = Call(callbackfn, thisArg, kValue, k, o);
      CreateDataPropertyOrThrow(a, k, mappedValue);
    }
  }
  return a;
}

/**
 * Array.prototype.indexOf ( searchElement [ , fromIndex ] ), 23.1.3.17.
 * @builtin Array.prototype.indexOf
 * @fast
 */
function ArrayPrototypeIndexOf(this: Value, searchElement: Value, fromIndex?: Value): f64 {
  const o: Obj = ToObject(this);
  const len: f64 = LengthOfArrayLike(o);
  if (len === 0) {
    return -1;
  }
  let n: f64 = ToIntegerOrInfinity(fromIndex);
  if (n >= len) {
    return -1;
  }
  if (n < 0) {
    n = len + n;
    if (n < 0) {
      n = 0;
    }
  }
  const found: f64 = ArraySearchDense(o, searchElement, n, len, 0);
  if (found !== -2) {
    return found;
  }
  for (let k: f64 = n; k < len; k++) {
    if (HasProperty(o, k)) {
      if (Get(o, k) === searchElement) {
        return k;
      }
    }
  }
  return -1;
}

/**
 * Array.prototype.join ( separator ), 23.1.3.18.
 * @builtin Array.prototype.join
 * @fast
 */
function ArrayPrototypeJoin(this: Value, separator: Value): Str {
  const o: Obj = ToObject(this);
  const len: f64 = LengthOfArrayLike(o);
  let sep: Str = ",";
  if (separator !== undefined) {
    sep = ToString(separator);
  }
  const dense: Value = ArrayJoinDense(o, len, sep);
  if (dense !== undefined) {
    return AsStr(dense);
  }
  let r: Str = "";
  for (let k: f64 = 0; k < len; k++) {
    if (k > 0) {
      r += sep;
    }
    const element: Value = Get(o, k);
    if (element !== undefined && element !== null) {
      r += ToString(element);
    }
  }
  return r;
}

/** SameValueZero ( x, y ), 7.2.11. */
function SameValueZero(x: Value, y: Value): boolean {
  if (IsNumber(x) && IsNumber(y)) {
    const a: f64 = AsNumber(x);
    const b: f64 = AsNumber(y);
    // NaN is the only value that is not equal to itself.
    return a === b || (a !== a && b !== b);
  }
  return x === y;
}

/**
 * Array.prototype.includes ( searchElement [ , fromIndex ] ), 23.1.3.16.
 * @builtin Array.prototype.includes
 * @fast
 */
function ArrayPrototypeIncludes(this: Value, searchElement: Value, fromIndex?: Value): boolean {
  const o: Obj = ToObject(this);
  const len: f64 = LengthOfArrayLike(o);
  if (len === 0) {
    return false;
  }
  let n: f64 = ToIntegerOrInfinity(fromIndex);
  if (n < 0) {
    n = len + n;
    if (n < 0) {
      n = 0;
    }
  }
  const found: f64 = ArraySearchDense(o, searchElement, n, len, 2);
  if (found !== -2) {
    return found !== -1;
  }
  for (let k: f64 = n; k < len; k++) {
    if (SameValueZero(Get(o, k), searchElement)) {
      return true;
    }
  }
  return false;
}

/**
 * Array ( ...values ), 23.1.1.1.
 * @constructor Array
 * @length 1
 * @intrinsic
 * @fast
 */
function ArrayConstructor(...values: Value[]): Obj {
  let a: Obj = values;
  if (LengthOfArrayLike(values) === 1) {
    const len: Value = Get(values, 0);
    if (IsNumber(len)) {
      const n: f64 = AsNumber(len);
      if (!(n >= 0 && n <= 4294967295 && ToIntegerOrInfinityNumber(n) === n)) {
        throw NewRangeError("invalid array length");
      }
      a = ArrayCreate(n);
    }
  }
  // The rest parameter is already a fresh array of the values; a new.target other than
  // Array (Reflect.construct, subclasses) gives it that constructor's prototype.
  if (new.target !== undefined) {
    const proto: Obj = GetPrototypeFromConstructor(new.target, "ArrayPrototype");
    if (proto !== RealmIntrinsic("ArrayPrototype")) {
      SetPrototypeOf(a, proto);
    }
  }
  return a;
}

/**
 * Array.isArray ( arg ), 23.1.2.2.
 * @builtin Array.isArray
 * @fast
 */
function ArrayIsArray(arg: Value): boolean {
  return IsArray(arg);
}

/**
 * Array.prototype.push ( ...items ), 23.1.3.23.
 * @builtin Array.prototype.push
 * @length 1
 * @native
 * @fast
 */
function ArrayPrototypePush(this: Value, ...items: Value[]): f64 {
  const o: Obj = ToObject(this);
  let len: f64 = LengthOfArrayLike(o);
  const argCount: f64 = LengthOfArrayLike(items);
  if (len + argCount > 9007199254740991) {
    throw NewTypeError("Array loo long");
  }
  if (argCount === 1) {
    const pushed: f64 = ArrayPushDense(o, Get(items, 0));
    if (pushed >= 0) {
      return pushed;
    }
  }
  for (let i: f64 = 0; i < argCount; i++) {
    Set(o, len, Get(items, i));
    len++;
  }
  Set(o, "length", len);
  return len;
}

// ---- Helpers ----------------------------------------------------------------------------

/**
 * A relative index (at, slice, fill, ...): ToIntegerOrInfinity(v), counted from the end
 * when negative, clamped to [0, len]. `undefined` gives `dflt`.
 */
function RelativeIndex(v: Value, len: f64, dflt: f64): f64 {
  if (v === undefined) {
    return dflt;
  }
  const rel: f64 = ToIntegerOrInfinity(v);
  if (rel < 0) {
    const k: f64 = len + rel;
    return k < 0 ? 0 : k;
  }
  return rel < len ? rel : len;
}

/** A callback argument: TypeError unless callable. */
function RequireCallable(f: Value): Fn {
  if (!IsCallable(f)) {
    throw NewTypeError("not a function");
  }
  return AsFn(f);
}

/** The 2^53 - 1 length limit of the methods that grow arrays (qjs's message). */
function CheckLength(len: f64): void {
  if (len > 9007199254740991) {
    throw NewTypeError("Array loo long");
  }
}

/** ArrayCreate for the copying methods (toReversed, with, ...): RangeError above 2^32 - 1. */
function ArrayCreateChecked(len: f64): Obj {
  if (len > 4294967295) {
    throw NewRangeError("invalid array length");
  }
  return ArrayCreate(len);
}

// ---- Array.of, Array.from, species ------------------------------------------------------

/**
 * Array.of ( ...items ), 23.1.2.3.
 * @builtin Array.of
 */
function ArrayOf(this: Value, ...items: Value[]): Obj {
  const len: f64 = LengthOfArrayLike(items);
  const a: Obj = IsConstructor(this) ? Construct(this, len) : ArrayCreate(len);
  for (let k: f64 = 0; k < len; k++) {
    CreateDataPropertyOrThrow(a, k, Get(items, k));
  }
  Set(a, "length", len);
  return a;
}

/**
 * Array.from ( items [ , mapfn [ , thisArg ] ] ), 23.1.2.1.
 * @builtin Array.from
 */
function ArrayFrom(this: Value, items: Value, mapfn?: Value, thisArg?: Value): Obj {
  const mapping: boolean = mapfn !== undefined;
  if (mapping && !IsCallable(mapfn)) {
    throw NewTypeError("not a function");
  }
  // From %Array%, Construct(%Array%) and Construct(%Array%, len) are ArrayCreate(0) and
  // ArrayCreate(len), and the final Set of `length` on that array changes nothing.
  const plain: boolean = IsArrayConstructor(this);
  const usingIterator: Value = GetMethod(items, WellKnownSymbol("iterator"));
  // A mapping function could grow an array while it is read; the iterator re-reads
  // the length, so only strings keep the direct path then.
  const direct: boolean = HasOriginalIteration(items) && (IsString(items) || !mapping);
  if (usingIterator !== undefined && !direct) {
    const target: Obj = !plain && IsConstructor(this) ? Construct(this) : ArrayCreate(0);
    return ArrayFromIterator(target, items, usingIterator, mapfn, thisArg);
  }
  // Arrays and strings with their original iteration are read directly: the same
  // elements (holes as undefined, strings by code points) without iterator objects.
  // The constructor gets no length for them, as for iterables.
  const iterable: boolean = usingIterator !== undefined;
  const arrayLike: Obj = IsString(items) ? StringCodePoints(AsStr(items)) : ToObject(items);
  const len: f64 = LengthOfArrayLike(arrayLike);
  let a: Obj = ArrayCreate(iterable ? 0 : len);
  if (!plain && IsConstructor(this)) {
    a = iterable ? Construct(this) : Construct(this, len);
  }
  // With a mapping function, the elements whose Get observes nothing in one callback
  // frame (D2306).
  const start: f64 = mapping && len > 0 ? EachDense(8, arrayLike, 0, len, mapfn, thisArg, a) : 0;
  for (let k: f64 = start; k < len; k++) {
    const kValue: Value = Get(arrayLike, k);
    const mappedValue: Value = mapping ? Call(mapfn, thisArg, kValue, k) : kValue;
    CreateDataPropertyOrThrow(a, k, mappedValue);
  }
  if (!plain) {
    Set(a, "length", len);
  }
  return a;
}

/**
 * get Array [ %Symbol.species% ], 23.1.2.5.
 * @getter Array[Symbol.species]
 * @intrinsic
 */
function ArraySpecies(this: Value): Value {
  return this;
}

// ---- Accessors and searches -----------------------------------------------------------

/**
 * Array.prototype.at ( index ), 23.1.3.1.
 * @builtin Array.prototype.at
 */
function ArrayPrototypeAt(this: Value, index: Value): Value {
  const o: Obj = ToObject(this);
  const len: f64 = LengthOfArrayLike(o);
  const rel: f64 = ToIntegerOrInfinity(index);
  const k: f64 = rel >= 0 ? rel : len + rel;
  if (k < 0 || k >= len) {
    return undefined;
  }
  return Get(o, k);
}

/**
 * Array.prototype.lastIndexOf ( searchElement [ , fromIndex ] ), 23.1.3.20.
 * @builtin Array.prototype.lastIndexOf
 */
function ArrayPrototypeLastIndexOf(this: Value, searchElement: Value, ...rest: Value[]): f64 {
  const o: Obj = ToObject(this);
  const len: f64 = LengthOfArrayLike(o);
  if (len === 0) {
    return -1;
  }
  let n: f64 = len - 1;
  if (LengthOfArrayLike(rest) > 0) {
    n = ToIntegerOrInfinity(Get(rest, 0));
  }
  let k: f64 = 0;
  if (n === -1 / 0) {
    return -1;
  }
  if (n >= 0) {
    k = n < len - 1 ? n : len - 1;
  } else {
    k = len + n;
  }
  const found: f64 = ArraySearchDense(o, searchElement, k, len, 1);
  if (found !== -2) {
    return found;
  }
  for (; k >= 0; k--) {
    if (HasProperty(o, k)) {
      if (Get(o, k) === searchElement) {
        return k;
      }
    }
  }
  return -1;
}

/**
 * The find family: `fromEnd` searches backwards, `wantIndex` returns the index (-1)
 * instead of the element (undefined). 23.1.3.9-12.
 * @intrinsic
 */
function FindViaPredicate(o: Obj, len: f64, fromEnd: boolean, predicate: Value, thisArg: Value, wantIndex: boolean): Value {
  if (!IsCallable(predicate)) {
    throw NewTypeError("not a function");
  }
  let i: f64 = 0;
  if (!fromEnd && len > 0) {
    // The dense part (0137): -1 - k when the result at k decided, o[k] in the box.
    const box: Obj = ArrayCreate(0);
    i = EachDense(wantIndex ? 6 : 5, o, 0, len, predicate, thisArg, box);
    if (i < 0) {
      return wantIndex ? -1 - i : Get(box, 0);
    }
  }
  for (; i < len; i++) {
    const k: f64 = fromEnd ? len - 1 - i : i;
    const kValue: Value = Get(o, k);
    if (ToBoolean(Call(predicate, thisArg, kValue, k, o))) {
      return wantIndex ? k : kValue;
    }
  }
  if (wantIndex) {
    return -1;
  }
  return undefined;
}

/**
 * Array.prototype.find ( predicate [ , thisArg ] ), 23.1.3.9.
 * @builtin Array.prototype.find
 */
function ArrayPrototypeFind(this: Value, predicate: Value, thisArg?: Value): Value {
  const o: Obj = ToObject(this);
  return FindViaPredicate(o, LengthOfArrayLike(o), false, predicate, thisArg, false);
}

/**
 * Array.prototype.findIndex ( predicate [ , thisArg ] ), 23.1.3.10.
 * @builtin Array.prototype.findIndex
 */
function ArrayPrototypeFindIndex(this: Value, predicate: Value, thisArg?: Value): Value {
  const o: Obj = ToObject(this);
  return FindViaPredicate(o, LengthOfArrayLike(o), false, predicate, thisArg, true);
}

/**
 * Array.prototype.findLast ( predicate [ , thisArg ] ), 23.1.3.11.
 * @builtin Array.prototype.findLast
 */
function ArrayPrototypeFindLast(this: Value, predicate: Value, thisArg?: Value): Value {
  const o: Obj = ToObject(this);
  return FindViaPredicate(o, LengthOfArrayLike(o), true, predicate, thisArg, false);
}

/**
 * Array.prototype.findLastIndex ( predicate [ , thisArg ] ), 23.1.3.12.
 * @builtin Array.prototype.findLastIndex
 */
function ArrayPrototypeFindLastIndex(this: Value, predicate: Value, thisArg?: Value): Value {
  const o: Obj = ToObject(this);
  return FindViaPredicate(o, LengthOfArrayLike(o), true, predicate, thisArg, true);
}

/**
 * every (`wantAll`) and some, 23.1.3.6 and 23.1.3.29.
 * @intrinsic
 */
function EveryOrSome(o: Obj, callbackfn: Value, thisArg: Value, wantAll: boolean): boolean {
  const len: f64 = LengthOfArrayLike(o);
  if (!IsCallable(callbackfn)) {
    throw NewTypeError("not a function");
  }
  // The dense part (0137): -1 - k when the result at k decided.
  let k: f64 = len > 0 ? EachDense(wantAll ? 4 : 3, o, 0, len, callbackfn, thisArg, undefined) : 0;
  if (k < 0) {
    return !wantAll;
  }
  for (; k < len; k++) {
    if (HasProperty(o, k)) {
      const r: boolean = ToBoolean(Call(callbackfn, thisArg, Get(o, k), k, o));
      if (r !== wantAll) {
        return r;
      }
    }
  }
  return wantAll;
}

/**
 * Array.prototype.every ( callbackfn [ , thisArg ] ), 23.1.3.6.
 * @builtin Array.prototype.every
 */
function ArrayPrototypeEvery(this: Value, callbackfn: Value, thisArg?: Value): boolean {
  return EveryOrSome(ToObject(this), callbackfn, thisArg, true);
}

/**
 * Array.prototype.some ( callbackfn [ , thisArg ] ), 23.1.3.29.
 * @builtin Array.prototype.some
 */
function ArrayPrototypeSome(this: Value, callbackfn: Value, thisArg?: Value): boolean {
  return EveryOrSome(ToObject(this), callbackfn, thisArg, false);
}

/**
 * Array.prototype.filter ( callbackfn [ , thisArg ] ), 23.1.3.8.
 * @builtin Array.prototype.filter
 */
function ArrayPrototypeFilter(this: Value, callbackfn: Value, thisArg?: Value): Obj {
  const o: Obj = ToObject(this);
  const len: f64 = LengthOfArrayLike(o);
  if (!IsCallable(callbackfn)) {
    throw NewTypeError("not a function");
  }
  const a: Obj = ArraySpeciesCreate(o, 0);
  // The dense part (0137) keeps elements in the fresh `a`: its length counts them.
  let k: f64 = len > 0 ? EachDense(2, o, 0, len, callbackfn, thisArg, a) : 0;
  let to: f64 = k > 0 ? LengthOfArrayLike(a) : 0;
  for (; k < len; k++) {
    if (HasProperty(o, k)) {
      const kValue: Value = Get(o, k);
      if (ToBoolean(Call(callbackfn, thisArg, kValue, k, o))) {
        CreateDataPropertyOrThrow(a, to, kValue);
        to++;
      }
    }
  }
  return a;
}

/**
 * Array.prototype.reduce ( callbackfn [ , initialValue ] ), 23.1.3.24. One frame, no rest
 * array: initialValue by ArgumentCount (0162).
 * @builtin Array.prototype.reduce
 * @length 1
 */
function ArrayPrototypeReduce(this: Value, callbackfn: Value, initialValue?: Value): Value {
  const o: Obj = ToObject(this);
  const len: f64 = LengthOfArrayLike(o);
  if (!IsCallable(callbackfn)) {
    throw NewTypeError("not a function");
  }
  let k: f64 = 0;
  let accumulator: Value = initialValue;
  if (ArgumentCount() < 2) {
    while (k < len && !HasProperty(o, k)) {
      k++;
    }
    if (k >= len) {
      throw NewTypeError("empty array");
    }
    accumulator = Get(o, k);
    k++;
  }
  if (k < len) {
    // The dense part (0137): the accumulator goes in as thisArg and comes out as
    // element 0 of the box once a step ran.
    const box: Obj = ArrayCreate(0);
    const next: f64 = EachDense(7, o, k, len, callbackfn, accumulator, box);
    if (next > k) {
      accumulator = Get(box, 0);
    }
    k = next;
  }
  for (; k < len; k++) {
    if (HasProperty(o, k)) {
      accumulator = Call(callbackfn, undefined, accumulator, Get(o, k), k, o);
    }
  }
  return accumulator;
}

/**
 * Array.prototype.reduceRight ( callbackfn [ , initialValue ] ), 23.1.3.25.
 * @builtin Array.prototype.reduceRight
 * @length 1
 */
function ArrayPrototypeReduceRight(this: Value, callbackfn: Value, initialValue?: Value): Value {
  const o: Obj = ToObject(this);
  const len: f64 = LengthOfArrayLike(o);
  if (!IsCallable(callbackfn)) {
    throw NewTypeError("not a function");
  }
  let k: f64 = len - 1;
  let accumulator: Value = initialValue;
  if (ArgumentCount() < 2) {
    while (k >= 0 && !HasProperty(o, k)) {
      k--;
    }
    if (k < 0) {
      throw NewTypeError("empty array");
    }
    accumulator = Get(o, k);
    k--;
  }
  for (; k >= 0; k--) {
    if (HasProperty(o, k)) {
      accumulator = Call(callbackfn, undefined, accumulator, Get(o, k), k, o);
    }
  }
  return accumulator;
}

// ---- Copies and concatenation -----------------------------------------------------------

/** IsConcatSpreadable ( O ), 23.1.3.2.1. */
function IsConcatSpreadable(o: Value): boolean {
  if (!IsObject(o)) {
    return false;
  }
  const spreadable: Value = Get(AsObj(o), WellKnownSymbol("isConcatSpreadable"));
  if (spreadable !== undefined) {
    return ToBoolean(spreadable);
  }
  return IsArray(o);
}

/**
 * Array.prototype.concat ( ...items ), 23.1.3.2.
 * @builtin Array.prototype.concat
 * @length 1
 * @fast
 */
function ArrayPrototypeConcat(this: Value, ...items: Value[]): Obj {
  const o: Obj = ToObject(this);
  const a: Obj = ArraySpeciesCreate(o, 0);
  let n: f64 = 0;
  const count: f64 = LengthOfArrayLike(items);
  for (let i: f64 = -1; i < count; i++) {
    const e: Value = i < 0 ? o : Get(items, i);
    if (IsConcatSpreadable(e)) {
      const eo: Obj = AsObj(e);
      const len: f64 = LengthOfArrayLike(eo);
      CheckLength(n + len);
      for (let k: f64 = 0; k < len; k++) {
        if (HasProperty(eo, k)) {
          CreateDataPropertyOrThrow(a, n, Get(eo, k));
        }
        n++;
      }
    } else {
      CheckLength(n + 1);
      CreateDataPropertyOrThrow(a, n, e);
      n++;
    }
  }
  Set(a, "length", n);
  return a;
}

/**
 * Array.prototype.slice ( start, end ), 23.1.3.28.
 * @builtin Array.prototype.slice
 * @fast
 */
function ArrayPrototypeSlice(this: Value, start: Value, end: Value): Obj {
  const o: Obj = ToObject(this);
  const len: f64 = LengthOfArrayLike(o);
  const k: f64 = RelativeIndex(start, len, 0);
  const fin: f64 = RelativeIndex(end, len, len);
  const count: f64 = fin > k ? fin - k : 0;
  const a: Obj = ArraySpeciesCreate(o, count);
  if (ArraySliceDense(o, k, fin, a)) {
    Set(a, "length", count);
    return a;
  }
  let n: f64 = 0;
  for (let i: f64 = k; i < fin; i++) {
    if (HasProperty(o, i)) {
      CreateDataPropertyOrThrow(a, n, Get(o, i));
    }
    n++;
  }
  Set(a, "length", n);
  return a;
}

/**
 * Array.prototype.splice ( start, deleteCount, ...items ), 23.1.3.31. The arguments
 * are a rest array: `splice()` and `splice(undefined)` differ.
 * @builtin Array.prototype.splice
 * @length 2
 * @fast
 */
function ArrayPrototypeSplice(this: Value, ...args: Value[]): Obj {
  const o: Obj = ToObject(this);
  const len: f64 = LengthOfArrayLike(o);
  const argCount: f64 = LengthOfArrayLike(args);
  const actualStart: f64 = RelativeIndex(Get(args, 0), len, 0);
  const itemCount: f64 = argCount > 2 ? argCount - 2 : 0;
  let actualDeleteCount: f64 = 0;
  if (argCount === 1) {
    actualDeleteCount = len - actualStart;
  } else if (argCount > 1) {
    const dc: f64 = ToIntegerOrInfinity(Get(args, 1));
    actualDeleteCount = dc < 0 ? 0 : dc;
    if (actualDeleteCount > len - actualStart) {
      actualDeleteCount = len - actualStart;
    }
  }
  CheckLength(len + itemCount - actualDeleteCount);
  const a: Obj = ArraySpeciesCreate(o, actualDeleteCount);
  for (let k: f64 = 0; k < actualDeleteCount; k++) {
    const from: f64 = actualStart + k;
    if (HasProperty(o, from)) {
      CreateDataPropertyOrThrow(a, k, Get(o, from));
    }
  }
  Set(a, "length", actualDeleteCount);
  MoveElements(o, len, actualStart, actualDeleteCount, itemCount);
  for (let k: f64 = 0; k < itemCount; k++) {
    Set(o, actualStart + k, Get(args, k + 2));
  }
  Set(o, "length", len - actualDeleteCount + itemCount);
  return a;
}

/**
 * Moves the elements after a deleted range (splice's steps 15-16, unshift's step 4):
 * o[start + deleteCount .. len) goes to start + itemCount, and the tail left over when
 * shrinking is deleted.
 */
function MoveElements(o: Obj, len: f64, start: f64, deleteCount: f64, itemCount: f64): void {
  // Plain dense arrays move in one leaf step: nothing observable happens in between.
  if (deleteCount !== itemCount && ArraySpliceDense(o, start, deleteCount, itemCount)) {
    return;
  }
  if (itemCount < deleteCount) {
    for (let k: f64 = start; k < len - deleteCount; k++) {
      const from: f64 = k + deleteCount;
      const to: f64 = k + itemCount;
      if (HasProperty(o, from)) {
        Set(o, to, Get(o, from));
      } else {
        DeletePropertyOrThrow(o, to);
      }
    }
    for (let k: f64 = len; k > len - deleteCount + itemCount; k--) {
      DeletePropertyOrThrow(o, k - 1);
    }
  } else if (itemCount > deleteCount) {
    for (let k: f64 = len - deleteCount; k > start; k--) {
      const from: f64 = k + deleteCount - 1;
      const to: f64 = k + itemCount - 1;
      if (HasProperty(o, from)) {
        Set(o, to, Get(o, from));
      } else {
        DeletePropertyOrThrow(o, to);
      }
    }
  }
}

/**
 * Array.prototype.copyWithin ( target, start [ , end ] ), 23.1.3.4.
 * @builtin Array.prototype.copyWithin
 */
function ArrayPrototypeCopyWithin(this: Value, target: Value, start: Value, end?: Value): Obj {
  const o: Obj = ToObject(this);
  const len: f64 = LengthOfArrayLike(o);
  let to: f64 = RelativeIndex(target, len, 0);
  let from: f64 = RelativeIndex(start, len, 0);
  const fin: f64 = RelativeIndex(end, len, len);
  let count: f64 = fin - from < len - to ? fin - from : len - to;
  let direction: f64 = 1;
  if (from < to && to < from + count) {
    direction = -1;
    from = from + count - 1;
    to = to + count - 1;
  }
  while (count > 0) {
    if (HasProperty(o, from)) {
      Set(o, to, Get(o, from));
    } else {
      DeletePropertyOrThrow(o, to);
    }
    from += direction;
    to += direction;
    count--;
  }
  return o;
}

/**
 * Array.prototype.fill ( value [ , start [ , end ] ] ), 23.1.3.7.
 * @builtin Array.prototype.fill
 */
function ArrayPrototypeFill(this: Value, value: Value, start?: Value, end?: Value): Obj {
  const o: Obj = ToObject(this);
  const len: f64 = LengthOfArrayLike(o);
  const k: f64 = RelativeIndex(start, len, 0);
  const fin: f64 = RelativeIndex(end, len, len);
  for (let i: f64 = ArrayFillDense(o, value, k, fin); i < fin; i++) {
    Set(o, i, value);
  }
  return o;
}

/**
 * Array.prototype.reverse ( ), 23.1.3.26.
 * @builtin Array.prototype.reverse
 * @fast
 */
function ArrayPrototypeReverse(this: Value): Obj {
  const o: Obj = ToObject(this);
  const len: f64 = LengthOfArrayLike(o);
  const middle: f64 = F64Floor(len / 2);
  for (let lower: f64 = 0; lower < middle; lower++) {
    const upper: f64 = len - lower - 1;
    const lowerExists: boolean = HasProperty(o, lower);
    let lowerValue: Value = undefined;
    if (lowerExists) {
      lowerValue = Get(o, lower);
    }
    const upperExists: boolean = HasProperty(o, upper);
    let upperValue: Value = undefined;
    if (upperExists) {
      upperValue = Get(o, upper);
    }
    if (upperExists) {
      Set(o, lower, upperValue);
    } else if (lowerExists) {
      DeletePropertyOrThrow(o, lower);
    }
    if (lowerExists) {
      Set(o, upper, lowerValue);
    } else if (upperExists) {
      DeletePropertyOrThrow(o, upper);
    }
  }
  return o;
}

/**
 * Array.prototype.pop ( ), 23.1.3.22.
 * @builtin Array.prototype.pop
 * @fast
 */
function ArrayPrototypePop(this: Value): Value {
  const o: Obj = ToObject(this);
  if (ArrayCanPopDense(o)) {
    return ArrayPopDense(o);
  }
  const len: f64 = LengthOfArrayLike(o);
  if (len === 0) {
    Set(o, "length", 0);
    return undefined;
  }
  const index: f64 = len - 1;
  const element: Value = Get(o, index);
  DeletePropertyOrThrow(o, index);
  Set(o, "length", index);
  return element;
}

/**
 * Array.prototype.shift ( ), 23.1.3.27.
 * @builtin Array.prototype.shift
 * @fast
 */
function ArrayPrototypeShift(this: Value): Value {
  const o: Obj = ToObject(this);
  const len: f64 = LengthOfArrayLike(o);
  if (len === 0) {
    Set(o, "length", 0);
    return undefined;
  }
  const first: Value = Get(o, 0);
  MoveElements(o, len, 0, 1, 0);
  Set(o, "length", len - 1);
  return first;
}

/**
 * Array.prototype.unshift ( ...items ), 23.1.3.34.
 * @builtin Array.prototype.unshift
 * @length 1
 * @fast
 */
function ArrayPrototypeUnshift(this: Value, ...items: Value[]): f64 {
  const o: Obj = ToObject(this);
  const len: f64 = LengthOfArrayLike(o);
  const argCount: f64 = LengthOfArrayLike(items);
  if (argCount > 0) {
    CheckLength(len + argCount);
    MoveElements(o, len, 0, 0, argCount);
    for (let j: f64 = 0; j < argCount; j++) {
      Set(o, j, Get(items, j));
    }
  }
  Set(o, "length", len + argCount);
  return len + argCount;
}

/** FlattenIntoArray ( target, source, sourceLen, start, depth [ , mapper, thisArg ] ), 23.1.3.13.1. */
function FlattenIntoArray(target: Obj, source: Obj, sourceLen: f64, start: f64, depth: f64, mapper: Value, thisArg: Value): f64 {
  let targetIndex: f64 = start;
  for (let sourceIndex: f64 = 0; sourceIndex < sourceLen; sourceIndex++) {
    if (HasProperty(source, sourceIndex)) {
      let element: Value = Get(source, sourceIndex);
      if (mapper !== undefined) {
        element = Call(mapper, thisArg, element, sourceIndex, source);
      }
      if (depth > 0 && IsArray(element)) {
        const e: Obj = AsObj(element);
        targetIndex = FlattenIntoArray(target, e, LengthOfArrayLike(e), targetIndex, depth - 1, undefined, undefined);
      } else {
        CheckLength(targetIndex + 1);
        CreateDataPropertyOrThrow(target, targetIndex, element);
        targetIndex++;
      }
    }
  }
  return targetIndex;
}

/**
 * Array.prototype.flat ( [ depth ] ), 23.1.3.13.
 * @builtin Array.prototype.flat
 */
function ArrayPrototypeFlat(this: Value, depth?: Value): Obj {
  const o: Obj = ToObject(this);
  const sourceLen: f64 = LengthOfArrayLike(o);
  let depthNum: f64 = 1;
  if (depth !== undefined) {
    depthNum = ToIntegerOrInfinity(depth);
    if (depthNum < 0) {
      depthNum = 0;
    }
  }
  const a: Obj = ArraySpeciesCreate(o, 0);
  FlattenIntoArray(a, o, sourceLen, 0, depthNum, undefined, undefined);
  return a;
}

/**
 * Array.prototype.flatMap ( mapperFunction [ , thisArg ] ), 23.1.3.14.
 * @builtin Array.prototype.flatMap
 */
function ArrayPrototypeFlatMap(this: Value, mapperFunction: Value, thisArg?: Value): Obj {
  const o: Obj = ToObject(this);
  const sourceLen: f64 = LengthOfArrayLike(o);
  RequireCallable(mapperFunction);
  const a: Obj = ArraySpeciesCreate(o, 0);
  FlattenIntoArray(a, o, sourceLen, 0, 1, mapperFunction, thisArg);
  return a;
}

// ---- Sorting ----------------------------------------------------------------------------

/**
 * Sorts the dense array `list` in place: stable, a bottom-up merge sort with comparefn
 * or ToString order. Lists of primitives without comparefn go to a leaf; otherwise the
 * merge runs in leaves between the comparisons (SortStep), and only the comparisons run
 * here.
 * @intrinsic
 * @native
 */
function SortList(list: Obj, comparefn: Value): void {
  if (comparefn === undefined && SortPrimitives(list)) {
    return;
  }
  const st: Obj = SortStart(list);
  // SortCompare's result, a Number (NaN counts as 0).
  let r: Value = 0;
  while (SortStep(st, r)) {
    if (comparefn === undefined) {
      const xs: Str = ToString(SortOperand(st, 0));
      r = CompareStrings(xs, ToString(SortOperand(st, 1)));
    } else {
      r = Call(comparefn, undefined, SortOperand(st, 0), SortOperand(st, 1));
      if (!IsNumber(r)) {
        r = ToNumber(r);
      }
    }
  }
}

/**
 * The present elements of o[0 .. len) as a dense list (SortIndexedProperties' read).
 * @intrinsic
 */
function CollectPresent(o: Obj, len: f64, skipHoles: boolean): Obj {
  const items: Obj = ArrayCreate(0);
  // A dense Array without holes in one copy (0137: in place of a native loop).
  if (ArraySliceDense(o, 0, len, items)) {
    return items;
  }
  let n: f64 = 0;
  for (let k: f64 = 0; k < len; k++) {
    if (!skipHoles || HasProperty(o, k)) {
      CreateDataPropertyOrThrow(items, n, Get(o, k));
      n++;
    }
  }
  return items;
}

/**
 * Array.prototype.sort ( comparefn ), 23.1.3.30.
 * @builtin Array.prototype.sort
 * @native
 */
function ArrayPrototypeSort(this: Value, comparefn: Value): Obj {
  if (comparefn !== undefined && !IsCallable(comparefn)) {
    throw NewTypeError("not a function");
  }
  const o: Obj = ToObject(this);
  const len: f64 = LengthOfArrayLike(o);
  const items: Obj = CollectPresent(o, len, true);
  const n: f64 = LengthOfArrayLike(items);
  SortList(items, comparefn);
  for (let k: f64 = 0; k < n; k++) {
    Set(o, k, Get(items, k));
  }
  // Holes go to the end.
  for (let k: f64 = n; k < len; k++) {
    DeletePropertyOrThrow(o, k);
  }
  return o;
}

/**
 * Array.prototype.toSorted ( comparefn ), 23.1.3.33.
 * @builtin Array.prototype.toSorted
 */
function ArrayPrototypeToSorted(this: Value, comparefn: Value): Obj {
  if (comparefn !== undefined && !IsCallable(comparefn)) {
    throw NewTypeError("not a function");
  }
  const o: Obj = ToObject(this);
  const len: f64 = LengthOfArrayLike(o);
  ArrayCreateChecked(0 + len);
  const items: Obj = CollectPresent(o, len, false);
  SortList(items, comparefn);
  return items;
}

// ---- Copying variants -------------------------------------------------------------------

/**
 * Array.prototype.toReversed ( ), 23.1.3.32.
 * @builtin Array.prototype.toReversed
 */
function ArrayPrototypeToReversed(this: Value): Obj {
  const o: Obj = ToObject(this);
  const len: f64 = LengthOfArrayLike(o);
  const a: Obj = ArrayCreateChecked(len);
  for (let k: f64 = 0; k < len; k++) {
    CreateDataPropertyOrThrow(a, k, Get(o, len - k - 1));
  }
  return a;
}

/**
 * Array.prototype.toSpliced ( start, skipCount, ...items ), 23.1.3.34.
 * @builtin Array.prototype.toSpliced
 * @length 2
 */
function ArrayPrototypeToSpliced(this: Value, ...args: Value[]): Obj {
  const o: Obj = ToObject(this);
  const len: f64 = LengthOfArrayLike(o);
  const argCount: f64 = LengthOfArrayLike(args);
  const actualStart: f64 = RelativeIndex(Get(args, 0), len, 0);
  const insertCount: f64 = argCount > 2 ? argCount - 2 : 0;
  let actualSkipCount: f64 = 0;
  if (argCount === 1) {
    actualSkipCount = len - actualStart;
  } else if (argCount > 1) {
    const sc: f64 = ToIntegerOrInfinity(Get(args, 1));
    actualSkipCount = sc < 0 ? 0 : sc;
    if (actualSkipCount > len - actualStart) {
      actualSkipCount = len - actualStart;
    }
  }
  const newLen: f64 = len + insertCount - actualSkipCount;
  CheckLength(newLen);
  const a: Obj = ArrayCreateChecked(newLen);
  let i: f64 = 0;
  for (; i < actualStart; i++) {
    CreateDataPropertyOrThrow(a, i, Get(o, i));
  }
  for (let k: f64 = 0; k < insertCount; k++) {
    CreateDataPropertyOrThrow(a, i, Get(args, k + 2));
    i++;
  }
  for (let r: f64 = actualStart + actualSkipCount; r < len; r++) {
    CreateDataPropertyOrThrow(a, i, Get(o, r));
    i++;
  }
  return a;
}

/**
 * Array.prototype.with ( index, value ), 23.1.3.39.
 * @builtin Array.prototype.with
 */
function ArrayPrototypeWith(this: Value, index: Value, value: Value): Obj {
  const o: Obj = ToObject(this);
  const len: f64 = LengthOfArrayLike(o);
  const rel: f64 = ToIntegerOrInfinity(index);
  const actual: f64 = rel >= 0 ? rel : len + rel;
  if (actual >= len || actual < 0) {
    throw NewRangeError("invalid array index: " + NumberToString(actual, 10));
  }
  const a: Obj = ArrayCreateChecked(len);
  for (let k: f64 = 0; k < len; k++) {
    CreateDataPropertyOrThrow(a, k, k === actual ? value : Get(o, k));
  }
  return a;
}

// ---- Conversions ------------------------------------------------------------------------

/**
 * Array.prototype.toString ( ), 23.1.3.36 (also %TypedArray%.prototype.toString, 23.2.3.32).
 * @builtin Array.prototype.toString
 * @builtin %TypedArray%.prototype.toString
 */
function ArrayPrototypeToString(this: Value): Value {
  const array: Obj = ToObject(this);
  const func: Value = Get(array, "join");
  if (IsCallable(func)) {
    return Call(func, array);
  }
  return ObjectToString(array);
}

/**
 * Array.prototype.toLocaleString ( [ reserved1 [ , reserved2 ] ] ), 23.1.3.35.
 * @builtin Array.prototype.toLocaleString
 */
function ArrayPrototypeToLocaleString(this: Value): Str {
  const array: Obj = ToObject(this);
  const len: f64 = LengthOfArrayLike(array);
  let r: Str = "";
  for (let k: f64 = 0; k < len; k++) {
    if (k > 0) {
      r += ",";
    }
    const nextElement: Value = Get(array, k);
    if (nextElement !== undefined && nextElement !== null) {
      r += ToString(Call(GetV(nextElement, "toLocaleString"), nextElement));
    }
  }
  return r;
}

/**
 * Array.prototype [ %Symbol.unscopables% ], 23.1.3.41.
 * @value Array.prototype[Symbol.unscopables]
 * @attributes configurable
 */
function ArrayPrototypeUnscopables(): Obj {
  const u: Obj = ObjectCreate(null);
  CreateDataPropertyOrThrow(u, "at", true);
  CreateDataPropertyOrThrow(u, "copyWithin", true);
  CreateDataPropertyOrThrow(u, "entries", true);
  CreateDataPropertyOrThrow(u, "fill", true);
  CreateDataPropertyOrThrow(u, "find", true);
  CreateDataPropertyOrThrow(u, "findIndex", true);
  CreateDataPropertyOrThrow(u, "findLast", true);
  CreateDataPropertyOrThrow(u, "findLastIndex", true);
  CreateDataPropertyOrThrow(u, "flat", true);
  CreateDataPropertyOrThrow(u, "flatMap", true);
  CreateDataPropertyOrThrow(u, "includes", true);
  CreateDataPropertyOrThrow(u, "keys", true);
  CreateDataPropertyOrThrow(u, "toReversed", true);
  CreateDataPropertyOrThrow(u, "toSorted", true);
  CreateDataPropertyOrThrow(u, "toSpliced", true);
  CreateDataPropertyOrThrow(u, "values", true);
  return u;
}

/**
 * [[Set]] of an Array's `length` to an object (OrdinarySetWithOwnDescriptor then
 * ArraySetLength, 10.4.2.4): both conversions (ToUint32 and ToNumber, each calling
 * ToPrimitive), then the definition, which may now be rejected (a non-writable
 * `length`). With `strict`, a rejection is a TypeError; else the result says it.
 * @intrinsic
 */
function ArraySetLength(a: Obj, v: Value, strict: boolean): boolean {
  // ToUint32(v) and ToNumber(v): equal only for a Number that is a valid length.
  const newLen: f64 = ToNumber(v);
  const numberLen: f64 = ToNumber(v);
  if (!(newLen >= 0 && newLen <= 4294967295 && ToIntegerOrInfinityNumber(newLen) === newLen)) {
    throw NewRangeError("invalid array length");
  }
  if (newLen !== numberLen) {
    throw NewRangeError("invalid array length");
  }
  const desc: Obj = ObjectCreate(null);
  CreateDataPropertyOrThrow(desc, "value", newLen);
  const ok: boolean = DefineOwnPropertyFromRecord(a, "length", desc);
  if (!ok && strict) {
    throw NewTypeError("'length' is read-only");
  }
  return ok;
}
