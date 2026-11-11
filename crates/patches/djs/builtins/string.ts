// String (ECMA-262 22.1): the constructor and the String object basics so far.

/**
 * String ( value ), 22.1.1.1.
 * @constructor String
 * @length 1
 * @fast
 */
function StringConstructor(value?: Value): Value {
  // The argument count, not a rest array: `String(x)` allocates nothing (0230b).
  const nt: Value = new.target;
  let s: Str = "";
  if (ArgumentCount() > 0) {
    if (nt === undefined && IsSymbol(value)) {
      return SymbolDescriptiveString(AsSym(value));
    }
    s = ToString(value);
  }
  if (nt === undefined) {
    return s;
  }
  return WrapPrimitive(s, GetPrototypeFromConstructor(nt, "StringPrototype"));
}

/** ThisStringValue ( value ), 22.1.3.35.1. */
function ThisStringValue(value: Value): Str {
  const s: Value = PrimitiveValue(value);
  if (!IsString(s)) {
    throw NewTypeError("not a string");
  }
  return AsStr(s);
}

/**
 * String.prototype.toString ( ), 22.1.3.32.
 * @builtin String.prototype.toString
 */
function StringPrototypeToString(this: Value): Str {
  return ThisStringValue(this);
}

/**
 * String.prototype.valueOf ( ), 22.1.3.35.
 * @builtin String.prototype.valueOf
 */
function StringPrototypeValueOf(this: Value): Str {
  return ThisStringValue(this);
}

// ---- Helpers ----------------------------------------------------------------------------

/**
 * The length of a string in code units, as a Number.
 */
function StrLen(s: Str): f64 {
  return StrLength(s);
}

/**
 * RequireObjectCoercible ( this ) and ToString, QuickJS's message.
 */
function ThisToString(v: Value): Str {
  if (v === undefined || v === null) {
    throw NewTypeError("null or undefined are forbidden");
  }
  return ToString(v);
}

/**
 * IsRegExp ( argument ), 7.2.6.
 */
function IsRegExp(v: Value): boolean {
  if (!IsObject(v)) {
    return false;
  }
  const matcher: Value = Get(AsObj(v), WellKnownSymbol("match"));
  if (matcher !== undefined) {
    return ToBoolean(matcher);
  }
  return IsRegExpObject(v);
}

/**
 * A position argument clamped to [0, len] (ToIntegerOrInfinity first).
 */
function ClampPosition(v: Value, len: f64): f64 {
  const p: f64 = ToIntegerOrInfinity(v);
  if (p < 0) {
    return 0;
  }
  return p > len ? len : p;
}

// ---- Character access ---------------------------------------------------------------------

/**
 * String.prototype.at ( index ), 22.1.3.1.
 * @builtin String.prototype.at
 * @fast
 */
function StringPrototypeAt(this: Value, index: Value): Value {
  const s: Str = ThisToString(this);
  const len: f64 = StrLen(s);
  const rel: f64 = ToIntegerOrInfinity(index);
  const k: f64 = rel >= 0 ? rel : len + rel;
  if (k < 0 || k >= len) {
    return undefined;
  }
  return StrSubstring(s, k, k + 1);
}

/**
 * String.prototype.charAt ( pos ), 22.1.3.2.
 * @builtin String.prototype.charAt
 * @fast
 */
function StringPrototypeCharAt(this: Value, pos: Value): Str {
  const s: Str = ThisToString(this);
  const p: f64 = ToIntegerOrInfinity(pos);
  if (p < 0 || p >= StrLen(s)) {
    return "";
  }
  return StrSubstring(s, p, p + 1);
}

/**
 * String.prototype.charCodeAt ( pos ), 22.1.3.3.
 * @builtin String.prototype.charCodeAt
 * @fast
 */
function StringPrototypeCharCodeAt(this: Value, pos: Value): f64 {
  const s: Str = ThisToString(this);
  return StrCharCodeAt(s, ToIntegerOrInfinity(pos));
}

/**
 * String.prototype.codePointAt ( pos ), 22.1.3.4.
 * @builtin String.prototype.codePointAt
 * @fast
 */
function StringPrototypeCodePointAt(this: Value, pos: Value): Value {
  const s: Str = ThisToString(this);
  return StrCodePointAt(s, ToIntegerOrInfinity(pos));
}

// ---- Searching --------------------------------------------------------------------------

/**
 * String.prototype.indexOf ( searchString [ , position ] ), 22.1.3.9.
 * @builtin String.prototype.indexOf
 * @fast
 */
function StringPrototypeIndexOf(this: Value, searchString: Value, position?: Value): f64 {
  const s: Str = ThisToString(this);
  const search: Str = ToString(searchString);
  const start: f64 = ClampPosition(position, StrLen(s));
  return StrIndexOf(s, search, start);
}

/**
 * String.prototype.lastIndexOf ( searchString [ , position ] ), 22.1.3.11.
 * @builtin String.prototype.lastIndexOf
 * @fast
 */
function StringPrototypeLastIndexOf(this: Value, searchString: Value, position?: Value): f64 {
  const s: Str = ThisToString(this);
  const search: Str = ToString(searchString);
  const numPos: f64 = ToNumber(position);
  const len: f64 = StrLen(s);
  // NaN means +Infinity here.
  const pos: f64 = numPos !== numPos ? len : ClampPosition(numPos, len);
  const searchLen: f64 = StrLen(search);
  if (searchLen > len) {
    return -1;
  }
  return StrLastIndexOf(s, search, pos < len - searchLen ? pos : len - searchLen);
}

/**
 * String.prototype.includes ( searchString [ , position ] ), 22.1.3.8.
 * @builtin String.prototype.includes
 * @fast
 */
function StringPrototypeIncludes(this: Value, searchString: Value, position?: Value): boolean {
  const s: Str = ThisToString(this);
  if (IsRegExp(searchString)) {
    throw NewTypeError("regexp not supported");
  }
  const search: Str = ToString(searchString);
  const start: f64 = ClampPosition(position, StrLen(s));
  return StrIndexOf(s, search, start) >= 0;
}

/**
 * String.prototype.startsWith ( searchString [ , position ] ), 22.1.3.24.
 * @builtin String.prototype.startsWith
 * @fast
 */
function StringPrototypeStartsWith(this: Value, searchString: Value, position?: Value): boolean {
  const s: Str = ThisToString(this);
  if (IsRegExp(searchString)) {
    throw NewTypeError("regexp not supported");
  }
  const search: Str = ToString(searchString);
  const len: f64 = StrLen(s);
  const start: f64 = ClampPosition(position, len);
  const searchLength: f64 = StrLen(search);
  if (start + searchLength > len) {
    return false;
  }
  return StrSubstring(s, start, start + searchLength) === search;
}

/**
 * String.prototype.endsWith ( searchString [ , endPosition ] ), 22.1.3.7.
 * @builtin String.prototype.endsWith
 * @fast
 */
function StringPrototypeEndsWith(this: Value, searchString: Value, endPosition?: Value): boolean {
  const s: Str = ThisToString(this);
  if (IsRegExp(searchString)) {
    throw NewTypeError("regexp not supported");
  }
  const search: Str = ToString(searchString);
  const len: f64 = StrLen(s);
  const end: f64 = endPosition === undefined ? len : ClampPosition(endPosition, len);
  const searchLength: f64 = StrLen(search);
  if (searchLength > end) {
    return false;
  }
  return StrSubstring(s, end - searchLength, end) === search;
}

/**
 * String.prototype.localeCompare ( that ), 22.1.3.12: code point order (no Intl).
 * @builtin String.prototype.localeCompare
 * @fast
 */
function StringPrototypeLocaleCompare(this: Value, that: Value): f64 {
  const s: Str = ThisToString(this);
  return StrLocaleCompare(s, ToString(that));
}

// ---- Substrings -------------------------------------------------------------------------

/**
 * String.prototype.slice ( start, end ), 22.1.3.22.
 * @builtin String.prototype.slice
 * @fast
 */
function StringPrototypeSlice(this: Value, start: Value, end: Value): Str {
  const s: Str = ThisToString(this);
  const len: f64 = StrLen(s);
  const from: f64 = RelativeIndex(start, len, 0);
  const to: f64 = RelativeIndex(end, len, len);
  if (from >= to) {
    return "";
  }
  return StrSubstring(s, from, to);
}

/**
 * String.prototype.substring ( start, end ), 22.1.3.25.
 * @builtin String.prototype.substring
 * @fast
 */
function StringPrototypeSubstring(this: Value, start: Value, end: Value): Str {
  const s: Str = ThisToString(this);
  const len: f64 = StrLen(s);
  const a: f64 = ClampPosition(start, len);
  const b: f64 = end === undefined ? len : ClampPosition(end, len);
  return a < b ? StrSubstring(s, a, b) : StrSubstring(s, b, a);
}

/**
 * String.prototype.substr ( start, length ), B.2.2.1.
 * @builtin String.prototype.substr
 * @fast
 */
function StringPrototypeSubstr(this: Value, start: Value, length: Value): Str {
  const s: Str = ThisToString(this);
  const size: f64 = StrLen(s);
  const intStart: f64 = RelativeIndex(start, size, 0);
  let intLength: f64 = size;
  if (length !== undefined) {
    intLength = ToIntegerOrInfinity(length);
  }
  let intEnd: f64 = intStart + intLength;
  if (intEnd > size) {
    intEnd = size;
  }
  if (intStart >= intEnd) {
    return "";
  }
  return StrSubstring(s, intStart, intEnd);
}

/**
 * String.prototype.concat ( ...args ), 22.1.3.5.
 * @builtin String.prototype.concat
 * @length 1
 * @fast
 */
function StringPrototypeConcat(this: Value, ...args: Value[]): Str {
  let r: Str = ThisToString(this);
  const n: f64 = LengthOfArrayLike(args);
  for (let i: f64 = 0; i < n; i++) {
    r += ToString(Get(args, i));
  }
  return r;
}

/**
 * String.prototype.repeat ( count ), 22.1.3.19.
 * @builtin String.prototype.repeat
 * @fast
 */
function StringPrototypeRepeat(this: Value, count: Value): Str {
  const s: Str = ThisToString(this);
  const n: f64 = ToIntegerOrInfinity(count);
  if (n < 0 || n === 1 / 0) {
    throw NewRangeError("invalid repeat count");
  }
  if (StrLen(s) === 0 || n === 0) {
    return "";
  }
  return StrRepeat(s, n);
}

/** StringPad ( S, maxLength, fillString, placement ), 22.1.3.17.2. */
function StringPad(s: Str, maxLength: Value, fillString: Value, atStart: boolean): Str {
  const intMaxLength: f64 = ToLength(maxLength);
  const stringLength: f64 = StrLen(s);
  if (intMaxLength <= stringLength) {
    return s;
  }
  const filler: Str = fillString === undefined ? " " : ToString(fillString);
  const fillerLength: f64 = StrLen(filler);
  if (fillerLength === 0) {
    return s;
  }
  const fillLen: f64 = intMaxLength - stringLength;
  const truncated: Str = StrSubstring(StrRepeat(filler, F64Ceil(fillLen / fillerLength)), 0, fillLen);
  return atStart ? truncated + s : s + truncated;
}

/**
 * String.prototype.padEnd ( maxLength [ , fillString ] ), 22.1.3.16.
 * @builtin String.prototype.padEnd
 * @fast
 */
function StringPrototypePadEnd(this: Value, maxLength: Value, fillString?: Value): Str {
  return StringPad(ThisToString(this), maxLength, fillString, false);
}

/**
 * String.prototype.padStart ( maxLength [ , fillString ] ), 22.1.3.17.
 * @builtin String.prototype.padStart
 * @fast
 */
function StringPrototypePadStart(this: Value, maxLength: Value, fillString?: Value): Str {
  return StringPad(ThisToString(this), maxLength, fillString, true);
}

// ---- Case and white space ---------------------------------------------------------------

/**
 * String.prototype.toLowerCase ( ), 22.1.3.28.
 * @builtin String.prototype.toLowerCase
 * @fast
 */
function StringPrototypeToLowerCase(this: Value): Str {
  return StrToCase(ThisToString(this), false);
}

/**
 * String.prototype.toUpperCase ( ), 22.1.3.30.
 * @builtin String.prototype.toUpperCase
 * @fast
 */
function StringPrototypeToUpperCase(this: Value): Str {
  return StrToCase(ThisToString(this), true);
}

/**
 * String.prototype.toLocaleLowerCase ( ), 22.1.3.26 (no locales: toLowerCase).
 * @builtin String.prototype.toLocaleLowerCase
 * @fast
 */
function StringPrototypeToLocaleLowerCase(this: Value): Str {
  return StrToCase(ThisToString(this), false);
}

/**
 * String.prototype.toLocaleUpperCase ( ), 22.1.3.27 (no locales: toUpperCase).
 * @builtin String.prototype.toLocaleUpperCase
 * @fast
 */
function StringPrototypeToLocaleUpperCase(this: Value): Str {
  return StrToCase(ThisToString(this), true);
}

/**
 * String.prototype.trim ( ), 22.1.3.32.
 * @builtin String.prototype.trim
 * @fast
 */
function StringPrototypeTrim(this: Value): Str {
  return StrTrim(ThisToString(this), 0);
}

/**
 * String.prototype.trimStart ( ), 22.1.3.34 (trimLeft is the same function, B.2.2.15).
 * @builtin String.prototype.trimStart
 * @builtin String.prototype.trimLeft
 * @fast
 */
function StringPrototypeTrimStart(this: Value): Str {
  return StrTrim(ThisToString(this), 1);
}

/**
 * String.prototype.trimEnd ( ), 22.1.3.33 (trimRight is the same function, B.2.2.16).
 * @builtin String.prototype.trimEnd
 * @builtin String.prototype.trimRight
 * @fast
 */
function StringPrototypeTrimEnd(this: Value): Str {
  return StrTrim(ThisToString(this), 2);
}

/**
 * String.prototype.isWellFormed ( ), 22.1.3.10.
 * @builtin String.prototype.isWellFormed
 */
function StringPrototypeIsWellFormed(this: Value): boolean {
  return StrIsWellFormed(ThisToString(this));
}

/**
 * String.prototype.toWellFormed ( ), 22.1.3.31.
 * @builtin String.prototype.toWellFormed
 */
function StringPrototypeToWellFormed(this: Value): Str {
  return StrToWellFormed(ThisToString(this));
}

/**
 * String.prototype.normalize ( [ form ] ), 22.1.3.15. A build without the
 * `unicode-normalization` feature throws for strings that would need its tables (D1760).
 * @builtin String.prototype.normalize
 */
function StringPrototypeNormalize(this: Value, form?: Value): Str {
  const s: Str = ThisToString(this);
  let f: Str = "NFC";
  if (form !== undefined) {
    f = ToString(form);
  }
  let code: i32 = 0;
  if (f === "NFC") {
    code = 0;
  } else if (f === "NFD") {
    code = 1;
  } else if (f === "NFKC") {
    code = 2;
  } else if (f === "NFKD") {
    code = 3;
  } else {
    throw NewRangeError("bad normalization form");
  }
  const r: Value = StrNormalize(s, code);
  if (r === undefined) {
    throw NewTypeError("not supported yet: normalize of this string (no Unicode normalization tables)");
  }
  return AsStr(r);
}

// ---- String statics ---------------------------------------------------------------------

/**
 * String.fromCharCode ( ...codeUnits ), 22.1.2.1.
 * @builtin String.fromCharCode
 * @length 1
 * @fast
 */
function StringFromCharCode(...codeUnits: Value[]): Str {
  CoerceNumbers(codeUnits);
  return StrFromCodeUnits(codeUnits);
}

/**
 * String.fromCodePoint ( ...codePoints ), 22.1.2.2.
 * @builtin String.fromCodePoint
 * @length 1
 */
function StringFromCodePoint(...codePoints: Value[]): Str {
  if (AllCodePoints(codePoints)) {
    return StrFromCodePoints(codePoints);
  }
  const n: f64 = LengthOfArrayLike(codePoints);
  for (let i: f64 = 0; i < n; i++) {
    const next: Value = Get(codePoints, i);
    const cp: f64 = IsNumber(next) ? AsNumber(next) : ToNumber(next);
    if (ToIntegerOrInfinityNumber(cp) !== cp || cp < 0 || cp > 1114111) {
      throw NewRangeError("invalid code point");
    }
    CreateDataPropertyOrThrow(codePoints, i, cp);
  }
  return StrFromCodePoints(codePoints);
}

/**
 * String.raw ( template, ...substitutions ), 22.1.2.4.
 * @builtin String.raw
 * @length 1
 */
function StringRaw(template: Value, ...substitutions: Value[]): Str {
  const cooked: Obj = ToObject(template);
  const literals: Obj = ToObject(Get(cooked, "raw"));
  const literalCount: f64 = LengthOfArrayLike(literals);
  if (literalCount <= 0) {
    return "";
  }
  const subCount: f64 = LengthOfArrayLike(substitutions);
  let r: Str = "";
  for (let nextIndex: f64 = 0; ; nextIndex++) {
    r += ToString(Get(literals, nextIndex));
    if (nextIndex + 1 === literalCount) {
      return r;
    }
    if (nextIndex < subCount) {
      r += ToString(Get(substitutions, nextIndex));
    }
  }
}
