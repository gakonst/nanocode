// The String.prototype methods that delegate to RegExp objects (ECMA-262 22.1.3): match,
// matchAll, replace, replaceAll, search and split. Only objects are asked for their
// Symbol.* methods (as in the current spec and QuickJS); primitives are searched as
// strings or wrapped in a new RegExp.

/**
 * RequireObjectCoercible ( this ), with QuickJS's message for these methods.
 */
function RequireThisCoercible(v: Value): void {
  if (v === undefined || v === null) {
    throw NewTypeError("cannot convert to object");
  }
}

/** Invoke ( V, P, « arg » ) for a well-known symbol method. */
function InvokeSymbol(v: Obj, name: Str, arg: Value): Value {
  return Call(GetV(v, WellKnownSymbol(name)), v, arg);
}

/** The `g` flag check of matchAll and replaceAll on a regexp argument. */
function RequireGlobalFlag(regexp: Obj): void {
  if (IsRegExp(regexp)) {
    const flags: Value = Get(regexp, "flags");
    if (flags === undefined || flags === null) {
      throw NewTypeError("cannot convert to object");
    }
    if (StrIndexOf(ToString(flags), "g", 0) < 0) {
      throw NewTypeError("regexp must have the 'g' flag");
    }
  }
}

/** RegExpCreate ( P, F ) with P converted here (undefined is the empty pattern). */
function RegExpFromPattern(pattern: Value, flags: Str): Obj {
  return RegExpCreate(undefined, pattern === undefined ? "" : ToString(pattern), flags);
}

/**
 * String.prototype.match ( regexp ), 22.1.3.13.
 * @builtin String.prototype.match
 * @fast
 */
function StringPrototypeMatch(this: Value, regexp: Value): Value {
  RequireThisCoercible(this);
  if (IsObject(regexp)) {
    const matcher: Value = GetMethod(regexp, WellKnownSymbol("match"));
    if (matcher !== undefined) {
      return Call(matcher, regexp, this);
    }
  }
  const s: Str = ToString(this);
  return InvokeSymbol(RegExpFromPattern(regexp, ""), "match", s);
}

/**
 * String.prototype.matchAll ( regexp ), 22.1.3.14.
 * @builtin String.prototype.matchAll
 */
function StringPrototypeMatchAll(this: Value, regexp: Value): Value {
  RequireThisCoercible(this);
  if (IsObject(regexp)) {
    RequireGlobalFlag(AsObj(regexp));
    const matcher: Value = GetMethod(regexp, WellKnownSymbol("matchAll"));
    if (matcher !== undefined) {
      return Call(matcher, regexp, this);
    }
  }
  const s: Str = ToString(this);
  return InvokeSymbol(RegExpFromPattern(regexp, "g"), "matchAll", s);
}

/**
 * String.prototype.search ( regexp ), 22.1.3.23.
 * @builtin String.prototype.search
 * @fast
 */
function StringPrototypeSearch(this: Value, regexp: Value): Value {
  RequireThisCoercible(this);
  if (IsObject(regexp)) {
    const searcher: Value = GetMethod(regexp, WellKnownSymbol("search"));
    if (searcher !== undefined) {
      return Call(searcher, regexp, this);
    }
  }
  const s: Str = ToString(this);
  return InvokeSymbol(RegExpFromPattern(regexp, ""), "search", s);
}

/** The replacement for one match of a string pattern (replace and replaceAll). */
function StringReplacement(
  functionalReplace: boolean,
  replaceValue: Value,
  replaceStr: Str,
  searchString: Str,
  position: f64,
  string: Str,
): Str {
  if (functionalReplace) {
    return ToString(Call(replaceValue, undefined, searchString, position, string));
  }
  return GetSubstitution(searchString, string, position, ArrayCreate(0), 0, undefined, replaceStr);
}

/**
 * String.prototype.replace ( searchValue, replaceValue ), 22.1.3.19.
 * @builtin String.prototype.replace
 * @fast
 */
function StringPrototypeReplace(this: Value, searchValue: Value, replaceValue: Value): Value {
  RequireThisCoercible(this);
  if (IsObject(searchValue)) {
    const replacer: Value = GetMethod(searchValue, WellKnownSymbol("replace"));
    if (replacer !== undefined) {
      return Call(replacer, searchValue, this, replaceValue);
    }
  }
  const string: Str = ToString(this);
  const searchString: Str = ToString(searchValue);
  const functionalReplace: boolean = IsCallable(replaceValue);
  let replaceStr: Str = "";
  if (!functionalReplace) {
    replaceStr = ToString(replaceValue);
  }
  const position: f64 = StrIndexOf(string, searchString, 0);
  if (position < 0) {
    return string;
  }
  const replacement: Str = StringReplacement(
    functionalReplace,
    replaceValue,
    replaceStr,
    searchString,
    position,
    string,
  );
  const following: f64 = position + StrLen(searchString);
  return StrSubstring(string, 0, position) + replacement + StrSubstring(string, following, StrLen(string));
}

/**
 * String.prototype.replaceAll ( searchValue, replaceValue ), 22.1.3.20.
 * @builtin String.prototype.replaceAll
 * @fast
 */
function StringPrototypeReplaceAll(this: Value, searchValue: Value, replaceValue: Value): Value {
  RequireThisCoercible(this);
  if (IsObject(searchValue)) {
    RequireGlobalFlag(AsObj(searchValue));
    const replacer: Value = GetMethod(searchValue, WellKnownSymbol("replace"));
    if (replacer !== undefined) {
      return Call(replacer, searchValue, this, replaceValue);
    }
  }
  const string: Str = ToString(this);
  const searchString: Str = ToString(searchValue);
  const functionalReplace: boolean = IsCallable(replaceValue);
  let replaceStr: Str = "";
  if (!functionalReplace) {
    replaceStr = ToString(replaceValue);
  }
  const searchLength: f64 = StrLen(searchString);
  const advanceBy: f64 = searchLength > 1 ? searchLength : 1;
  const positions: Obj = ArrayCreate(0);
  let count: f64 = 0;
  let position: f64 = StrIndexOf(string, searchString, 0);
  while (position >= 0) {
    CreateDataPropertyOrThrow(positions, count, position);
    count++;
    position = StrIndexOf(string, searchString, position + advanceBy);
  }
  let endOfLastMatch: f64 = 0;
  let result: Str = "";
  for (let k: f64 = 0; k < count; k++) {
    const p: f64 = AsNumber(Get(positions, k));
    const replacement: Str = StringReplacement(
      functionalReplace,
      replaceValue,
      replaceStr,
      searchString,
      p,
      string,
    );
    result += StrSubstring(string, endOfLastMatch, p) + replacement;
    endOfLastMatch = p + searchLength;
  }
  const len: f64 = StrLen(string);
  if (endOfLastMatch < len) {
    result += StrSubstring(string, endOfLastMatch, len);
  }
  return result;
}

/**
 * String.prototype.split ( separator, limit ), 22.1.3.23.
 * @builtin String.prototype.split
 * @fast
 */
function StringPrototypeSplit(this: Value, separator: Value, limit: Value): Value {
  RequireThisCoercible(this);
  if (IsObject(separator)) {
    const splitter: Value = GetMethod(separator, WellKnownSymbol("split"));
    if (splitter !== undefined) {
      return Call(splitter, separator, this, limit);
    }
  }
  const s: Str = ToString(this);
  const lim: f64 = limit === undefined ? 4294967295 : NumberToUint32(ToNumber(limit));
  const r: Str = ToString(separator);
  const a: Obj = ArrayCreate(0);
  if (lim === 0) {
    return a;
  }
  if (separator === undefined) {
    CreateDataPropertyOrThrow(a, 0, s);
    return a;
  }
  const len: f64 = StrLen(s);
  const separatorLength: f64 = StrLen(r);
  if (separatorLength === 0) {
    const n: f64 = len < lim ? len : lim;
    for (let i: f64 = 0; i < n; i++) {
      CreateDataPropertyOrThrow(a, i, StrSubstring(s, i, i + 1));
    }
    return a;
  }
  let lengthA: f64 = 0;
  let p: f64 = 0;
  let q: f64 = StrIndexOf(s, r, 0);
  while (q >= 0) {
    CreateDataPropertyOrThrow(a, lengthA, StrSubstring(s, p, q));
    lengthA++;
    if (lengthA === lim) {
      return a;
    }
    p = q + separatorLength;
    q = StrIndexOf(s, r, p);
  }
  CreateDataPropertyOrThrow(a, lengthA, StrSubstring(s, p, len));
  return a;
}
