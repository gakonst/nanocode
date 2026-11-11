// RegExp objects (ECMA-262 22.2.4 - 22.2.9). The matcher and the match arrays are leaves
// over crate::regexp (D1332); the protocol (lastIndex, flags, exec lookups, species) is
// here, in spec order, with QuickJS's messages.

/**
 * RegExp ( pattern, flags ), 22.2.4.1.
 * @constructor RegExp
 * @length 2
 */
function RegExpConstructor(pattern: Value, flags: Value): Value {
  let newTarget: Value = new.target;
  if (
    IsString(pattern) &&
    (flags === undefined || IsString(flags)) &&
    (newTarget === undefined || newTarget === RealmConstructor("RegExp"))
  ) {
    // The common case, without the steps that cannot observe anything here: IsRegExp of
    // a string, the prototype of %RegExp% (a fixed property), ToString of strings.
    return RegExpCreate(undefined, AsStr(pattern), flags === undefined ? "" : AsStr(flags));
  }
  const patternIsRegExp: boolean = IsRegExp(pattern);
  if (newTarget === undefined) {
    newTarget = RealmConstructor("RegExp");
    if (patternIsRegExp && flags === undefined) {
      const patternConstructor: Value = Get(AsObj(pattern), "constructor");
      if (patternConstructor === newTarget) {
        return pattern;
      }
    }
  }
  let p: Value = pattern;
  let f: Value = flags;
  if (IsRegExpObject(pattern)) {
    p = RegExpSource(AsObj(pattern));
    if (flags === undefined) {
      f = RegExpFlags(AsObj(pattern));
    }
  } else if (patternIsRegExp) {
    p = Get(AsObj(pattern), "source");
    if (flags === undefined) {
      f = Get(AsObj(pattern), "flags");
    }
  }
  const proto: Obj = GetPrototypeFromConstructor(newTarget, "RegExpPrototype");
  const pStr: Str = p === undefined ? "" : ToString(p);
  const fStr: Str = f === undefined ? "" : ToString(f);
  if (IsRegExpObject(pattern)) {
    // Its source is pStr: with the same flags, the new one shares its program.
    return RegExpCreate(proto, pattern, fStr);
  }
  return RegExpCreate(proto, pStr, fStr);
}

/**
 * RegExp.escape ( S ), 22.2.5.1.
 * @builtin RegExp.escape
 */
function RegExpEscape(s: Value): Str {
  if (!IsString(s)) {
    throw NewTypeError("not a string");
  }
  return RegExpEscapeString(AsStr(s));
}

/**
 * get RegExp [ %Symbol.species% ], 22.2.5.2.
 * @getter RegExp[Symbol.species]
 * @intrinsic
 */
function RegExpSpecies(this: Value): Value {
  return this;
}

/** `this` of the RegExp.prototype accessors and methods: an object, qjs's message. */
function ThisRegExpObject(v: Value): Obj {
  if (!IsObject(v)) {
    throw NewTypeError("not an object");
  }
  return AsObj(v);
}

/**
 * The flag getters (22.2.6.4.1 RegExpHasFlag): undefined on %RegExp.prototype% itself.
 */
function RegExpFlagGetter(r: Value, flag: Str): Value {
  const o: Obj = ThisRegExpObject(r);
  if (!IsRegExpObject(o)) {
    if (o === RealmIntrinsic("RegExpPrototype")) {
      return undefined;
    }
    throw NewTypeError("RegExp object expected");
  }
  return RegExpHasFlag(o, flag);
}

/**
 * get RegExp.prototype.hasIndices, 22.2.6.6.
 * @getter RegExp.prototype.hasIndices
 * @intrinsic
 */
function RegExpPrototypeHasIndices(this: Value): Value {
  return RegExpFlagGetter(this, "d");
}

/**
 * get RegExp.prototype.global, 22.2.6.6.
 * @getter RegExp.prototype.global
 * @intrinsic
 */
function RegExpPrototypeGlobal(this: Value): Value {
  return RegExpFlagGetter(this, "g");
}

/**
 * get RegExp.prototype.ignoreCase, 22.2.6.7.
 * @getter RegExp.prototype.ignoreCase
 * @intrinsic
 */
function RegExpPrototypeIgnoreCase(this: Value): Value {
  return RegExpFlagGetter(this, "i");
}

/**
 * get RegExp.prototype.multiline, 22.2.6.9.
 * @getter RegExp.prototype.multiline
 * @intrinsic
 */
function RegExpPrototypeMultiline(this: Value): Value {
  return RegExpFlagGetter(this, "m");
}

/**
 * get RegExp.prototype.dotAll, 22.2.6.3.
 * @getter RegExp.prototype.dotAll
 * @intrinsic
 */
function RegExpPrototypeDotAll(this: Value): Value {
  return RegExpFlagGetter(this, "s");
}

/**
 * get RegExp.prototype.unicode, 22.2.6.17.
 * @getter RegExp.prototype.unicode
 * @intrinsic
 */
function RegExpPrototypeUnicode(this: Value): Value {
  return RegExpFlagGetter(this, "u");
}

/**
 * get RegExp.prototype.unicodeSets, 22.2.6.18.
 * @getter RegExp.prototype.unicodeSets
 * @intrinsic
 */
function RegExpPrototypeUnicodeSets(this: Value): Value {
  return RegExpFlagGetter(this, "v");
}

/**
 * get RegExp.prototype.sticky, 22.2.6.15.
 * @getter RegExp.prototype.sticky
 * @intrinsic
 */
function RegExpPrototypeSticky(this: Value): Value {
  return RegExpFlagGetter(this, "y");
}

/**
 * get RegExp.prototype.flags, 22.2.6.4.
 * @getter RegExp.prototype.flags
 * @intrinsic
 */
function RegExpPrototypeFlags(this: Value): Str {
  const r: Obj = ThisRegExpObject(this);
  if (IsPristineRegExp(r)) {
    // The getters below are the originals: they read [[OriginalFlags]] (D3136).
    return RegExpFlags(r);
  }
  let result: Str = "";
  if (ToBoolean(Get(r, "hasIndices"))) {
    result += "d";
  }
  if (ToBoolean(Get(r, "global"))) {
    result += "g";
  }
  if (ToBoolean(Get(r, "ignoreCase"))) {
    result += "i";
  }
  if (ToBoolean(Get(r, "multiline"))) {
    result += "m";
  }
  if (ToBoolean(Get(r, "dotAll"))) {
    result += "s";
  }
  if (ToBoolean(Get(r, "unicode"))) {
    result += "u";
  }
  if (ToBoolean(Get(r, "unicodeSets"))) {
    result += "v";
  }
  if (ToBoolean(Get(r, "sticky"))) {
    result += "y";
  }
  return result;
}

/**
 * get RegExp.prototype.source, 22.2.6.13.
 * @getter RegExp.prototype.source
 */
function RegExpPrototypeSource(this: Value): Str {
  const r: Obj = ThisRegExpObject(this);
  if (!IsRegExpObject(r)) {
    if (r === RealmIntrinsic("RegExpPrototype")) {
      return "(?:)";
    }
    throw NewTypeError("RegExp object expected");
  }
  return RegExpEscapedSource(r);
}

/**
 * RegExp.prototype.toString ( ), 22.2.6.16.
 * @builtin RegExp.prototype.toString
 */
function RegExpPrototypeToString(this: Value): Str {
  const r: Obj = ThisRegExpObject(this);
  const pattern: Str = ToString(Get(r, "source"));
  const flags: Str = ToString(Get(r, "flags"));
  return "/" + pattern + "/" + flags;
}

/** RegExpBuiltinExec ( R, S ), 22.2.7.2: lastIndex here, the rest in the leaf. */
function RegExpBuiltinExec(r: Obj, s: Str): Value {
  const lastIndex: Value = Get(r, "lastIndex");
  return RegExpExecRaw(r, s, IsNumber(lastIndex) ? AsNumber(lastIndex) : ToLength(lastIndex), undefined);
}

/**
 * RegExp.prototype.exec ( string ), 22.2.6.2.
 * @builtin RegExp.prototype.exec
 * @intrinsic
 * @fast
 */
function RegExpPrototypeExec(this: Value, string: Value): Value {
  if (!IsRegExpObject(this)) {
    throw NewTypeError("RegExp object expected");
  }
  const r: Obj = AsObj(this);
  const s: Str = IsString(string) ? AsStr(string) : ToString(string);
  const fast: Value = RegExpExecOwn(r, s, false, undefined);
  if (fast !== undefined) {
    return fast;
  }
  // RegExpBuiltinExec, inlined: this is the hot path of every match (D1560).
  const lastIndex: Value = Get(r, "lastIndex");
  return RegExpExecRaw(r, s, IsNumber(lastIndex) ? AsNumber(lastIndex) : ToLength(lastIndex), undefined);
}

/** RegExpExec ( R, S ), 22.2.7.1. */
function RegExpExec(r: Obj, s: Str): Value {
  const fast: Value = RegExpExecOwn(r, s, false, undefined);
  if (fast !== undefined) {
    return fast;
  }
  const exec: Value = Get(r, "exec");
  // The original exec on a RegExp: call its body directly (no frame, no result check).
  if (IsOriginalRegExpExec(exec) && IsRegExpObject(r)) {
    return RegExpBuiltinExec(r, s);
  }
  if (IsCallable(exec)) {
    const result: Value = Call(exec, r, s);
    if (!IsObject(result) && result !== null) {
      throw NewTypeError("RegExp exec method must return an object or null");
    }
    return result;
  }
  if (!IsRegExpObject(r)) {
    throw NewTypeError("RegExp object expected");
  }
  return RegExpBuiltinExec(r, s);
}

/**
 * RegExp.prototype.test ( S ), 22.2.6.16.
 * @builtin RegExp.prototype.test
 * @fast
 */
function RegExpPrototypeTest(this: Value, s: Value): boolean {
  if (!IsObject(this)) {
    // qjs looks up `exec` on the primitive and ends in RegExp.prototype.exec.
    throw NewTypeError("RegExp object expected");
  }
  const r: Obj = AsObj(this);
  const str: Str = IsString(s) ? AsStr(s) : ToString(s);
  const fast: Value = RegExpExecOwn(r, str, true, undefined);
  if (fast !== undefined) {
    return AsBool(fast);
  }
  const exec: Value = Get(r, "exec");
  if (IsOriginalRegExpExec(exec) && IsRegExpObject(r)) {
    // No match array is built when only the outcome is needed (D1560).
    const lastIndex: Value = Get(r, "lastIndex");
    const li: f64 = IsNumber(lastIndex) ? AsNumber(lastIndex) : ToLength(lastIndex);
    return RegExpTestRaw(r, str, li, undefined);
  }
  return RegExpExec(r, str) !== null;
}

/** AdvanceStringIndex ( S, index, unicode ), 22.2.7.3. */
function AdvanceStringIndex(s: Str, index: f64, unicode: boolean): f64 {
  if (!unicode || index + 1 >= StrLen(s)) {
    return index + 1;
  }
  const cp: Value = StrCodePointAt(s, index);
  return IsNumber(cp) && AsNumber(cp) > 65535 ? index + 2 : index + 1;
}

/** Whether the flags string contains the letter c. */
function FlagsInclude(flags: Str, c: Str): boolean {
  return StrIndexOf(flags, c, 0) >= 0;
}

/** The ToString of match[0] of a match result. */
function MatchString(result: Value): Str {
  return ToString(Get(AsObj(result), 0));
}

/** Advances lastIndex past an empty match (RegExp.prototype[@@match] and friends). */
function AdvanceAfterEmptyMatch(rx: Obj, s: Str, fullUnicode: boolean): void {
  const thisIndex: f64 = ToLength(Get(rx, "lastIndex"));
  Set(rx, "lastIndex", AdvanceStringIndex(s, thisIndex, fullUnicode));
}

/**
 * RegExp.prototype [ %Symbol.match% ] ( string ), 22.2.6.8.
 * @builtin RegExp.prototype[Symbol.match]
 * @intrinsic
 */
function RegExpPrototypeMatch(this: Value, string: Value): Value {
  const rx: Obj = ThisRegExpObject(this);
  const s: Str = ToString(string);
  const flags: Str = ToString(Get(rx, "flags"));
  if (!FlagsInclude(flags, "g")) {
    return RegExpExec(rx, s);
  }
  const fullUnicode: boolean = FlagsInclude(flags, "u") || FlagsInclude(flags, "v");
  Set(rx, "lastIndex", 0);
  const a: Obj = ArrayCreate(0);
  let n: f64 = 0;
  while (true) {
    const result: Value = RegExpExec(rx, s);
    if (result === null) {
      if (n === 0) {
        return null;
      }
      return a;
    }
    const matchStr: Str = MatchString(result);
    CreateDataPropertyOrThrow(a, n, matchStr);
    if (matchStr === "") {
      AdvanceAfterEmptyMatch(rx, s, fullUnicode);
    }
    n++;
  }
}

/**
 * RegExp.prototype [ %Symbol.matchAll% ] ( string ), 22.2.6.9.
 * @builtin RegExp.prototype[Symbol.matchAll]
 */
function RegExpPrototypeMatchAll(this: Value, string: Value): Obj {
  const r: Obj = ThisRegExpObject(this);
  const s: Str = ToString(string);
  if (IsPristineRegExp(r)) {
    // SpeciesConstructor is %RegExp%, the flags and lastIndex are data: the steps
    // below without their lookups, and the matcher shares r's program (D3136).
    const flags0: Str = RegExpFlags(r);
    const matcher0: Obj = RegExpCreate(undefined, r, flags0);
    Set(matcher0, "lastIndex", ToLength(Get(r, "lastIndex")));
    return CreateRegExpStringIterator(
      matcher0,
      s,
      RegExpHasFlag(r, "g"),
      RegExpHasFlag(r, "u") || RegExpHasFlag(r, "v"),
    );
  }
  const c: Value = SpeciesConstructor(r, RealmConstructor("RegExp"));
  const flags: Str = ToString(Get(r, "flags"));
  const matcher: Obj = Construct(c, r, flags);
  const lastIndex: f64 = ToLength(Get(r, "lastIndex"));
  Set(matcher, "lastIndex", lastIndex);
  const global: boolean = FlagsInclude(flags, "g");
  const fullUnicode: boolean = FlagsInclude(flags, "u") || FlagsInclude(flags, "v");
  return CreateRegExpStringIterator(matcher, s, global, fullUnicode);
}

/**
 * %RegExpStringIteratorPrototype%.next ( ), 22.2.9.2.1.
 * @builtin %RegExpStringIteratorPrototype%.next
 * @fast
 */
function RegExpStringIteratorNext(this: Value): Obj {
  if (AsBool(RegExpStringIteratorSlot(this, 4))) {
    return CreateIterResultObject(undefined, true);
  }
  const r: Obj = AsObj(RegExpStringIteratorSlot(this, 0));
  const s: Str = AsStr(RegExpStringIteratorSlot(this, 1));
  const match: Value = RegExpExec(r, s);
  if (match === null) {
    RegExpStringIteratorFinish(this);
    return CreateIterResultObject(undefined, true);
  }
  if (AsBool(RegExpStringIteratorSlot(this, 2))) {
    if (MatchString(match) === "") {
      AdvanceAfterEmptyMatch(r, s, AsBool(RegExpStringIteratorSlot(this, 3)));
    }
  } else {
    RegExpStringIteratorFinish(this);
  }
  return CreateIterResultObject(match, false);
}

/**
 * %RegExpStringIteratorPrototype% [ %Symbol.toStringTag% ], 22.2.9.2.2.
 * @value %RegExpStringIteratorPrototype%[Symbol.toStringTag]
 * @attributes configurable
 */
function RegExpStringIteratorToStringTag(): Str {
  return "RegExp String Iterator";
}

/**
 * RegExp.prototype [ %Symbol.replace% ] ( string, replaceValue ), 22.2.6.11.
 * @builtin RegExp.prototype[Symbol.replace]
 * @intrinsic
 */
function RegExpPrototypeReplace(this: Value, string: Value, replaceValue: Value): Str {
  const rx: Obj = ThisRegExpObject(this);
  const s: Str = ToString(string);
  const lengthS: f64 = StrLen(s);
  const functionalReplace: boolean = IsCallable(replaceValue);
  let replaceStr: Str = "";
  if (!functionalReplace) {
    replaceStr = ToString(replaceValue);
  }
  // An unmodified global RegExp and a string replacement: the leaf (D1503).
  if (!functionalReplace && IsPristineRegExp(rx) && RegExpHasFlag(rx, "g")) {
    return RegExpReplaceRaw(rx, s, replaceStr, undefined);
  }
  // An unmodified RegExp and a replacer function: the matches from one leaf (nothing the
  // loop below does before its first call is observable then), the calls here.
  if (functionalReplace && IsPristineRegExp(rx)) {
    const spans: Obj = RegExpMatchSpans(rx, s, undefined);
    const matches: f64 = RegExpSpanCount(rx, s, spans);
    const texts: Value = RegExpSpanTexts(rx, s, spans);
    const replacements: Obj = RegExpSpanStarts(rx, s, spans);
    let k: f64 = 0;
    while (k < matches) {
      let r: Value = undefined;
      if (texts === undefined) {
        r = CallWithArray(replaceValue, undefined, RegExpReplaceArgs(rx, s, spans, k));
      } else {
        // Without groups: one frame for every call while the results are strings.
        const at: f64 = EachDense(9, AsObj(texts), k, matches, replaceValue, s, replacements);
        if (at === matches) {
          break;
        }
        if (at < 0) {
          k = -1 - at;
          r = Get(replacements, k);
        } else {
          k = at;
          r = Call(replaceValue, undefined, Get(AsObj(texts), k), Get(replacements, k), s);
        }
      }
      CreateDataPropertyOrThrow(replacements, k, IsString(r) ? AsStr(r) : ToString(r));
      k++;
    }
    return RegExpReplaceJoin(rx, s, spans, replacements);
  }
  const flags: Str = ToString(Get(rx, "flags"));
  const global: boolean = FlagsInclude(flags, "g");
  let fullUnicode: boolean = false;
  if (global) {
    fullUnicode = FlagsInclude(flags, "u") || FlagsInclude(flags, "v");
    Set(rx, "lastIndex", 0);
  }
  const results: Obj = ArrayCreate(0);
  let count: f64 = 0;
  while (true) {
    const result: Value = RegExpExec(rx, s);
    if (result === null) {
      break;
    }
    CreateDataPropertyOrThrow(results, count, result);
    count++;
    if (!global) {
      break;
    }
    if (MatchString(result) === "") {
      AdvanceAfterEmptyMatch(rx, s, fullUnicode);
    }
  }
  let accumulatedResult: Str = "";
  let nextSourcePosition: f64 = 0;
  for (let k: f64 = 0; k < count; k++) {
    const result: Obj = AsObj(Get(results, k));
    const resultLength: f64 = LengthOfArrayLike(result);
    const nCaptures: f64 = resultLength > 1 ? resultLength - 1 : 0;
    const matched: Str = ToString(Get(result, 0));
    const matchLength: f64 = StrLen(matched);
    let position: f64 = ToIntegerOrInfinity(Get(result, "index"));
    if (position < 0) {
      position = 0;
    } else if (position > lengthS) {
      position = lengthS;
    }
    const captures: Obj = ArrayCreate(0);
    for (let n: f64 = 1; n <= nCaptures; n++) {
      let capN: Value = Get(result, n);
      if (capN !== undefined) {
        capN = ToString(capN);
      }
      CreateDataPropertyOrThrow(captures, n - 1, capN);
    }
    let namedCaptures: Value = Get(result, "groups");
    let replacement: Str = "";
    if (functionalReplace) {
      const args: Obj = ArrayCreate(0);
      CreateDataPropertyOrThrow(args, 0, matched);
      for (let n: f64 = 0; n < nCaptures; n++) {
        CreateDataPropertyOrThrow(args, n + 1, Get(captures, n));
      }
      CreateDataPropertyOrThrow(args, nCaptures + 1, position);
      CreateDataPropertyOrThrow(args, nCaptures + 2, s);
      if (namedCaptures !== undefined) {
        CreateDataPropertyOrThrow(args, nCaptures + 3, namedCaptures);
      }
      replacement = ToString(CallWithArray(replaceValue, undefined, args));
    } else {
      if (namedCaptures !== undefined) {
        namedCaptures = ToObject(namedCaptures);
      }
      replacement = GetSubstitution(matched, s, position, captures, nCaptures, namedCaptures, replaceStr);
    }
    if (position >= nextSourcePosition) {
      accumulatedResult += StrSubstring(s, nextSourcePosition, position) + replacement;
      nextSourcePosition = position + matchLength;
    }
  }
  if (nextSourcePosition >= lengthS) {
    return accumulatedResult;
  }
  return accumulatedResult + StrSubstring(s, nextSourcePosition, lengthS);
}

/**
 * GetSubstitution ( matched, str, position, captures, namedCaptures, replacementTemplate ),
 * 22.1.3.19.1. `captures` is a dense array of m strings or undefined.
 */
function GetSubstitution(
  matched: Str,
  str: Str,
  position: f64,
  captures: Obj,
  m: f64,
  namedCaptures: Value,
  template: Str,
): Str {
  let i: f64 = StrIndexOf(template, "$", 0);
  if (i < 0) {
    return template;
  }
  const len: f64 = StrLen(template);
  const stringLength: f64 = StrLen(str);
  let result: Str = StrSubstring(template, 0, i);
  while (i < len) {
    const c: f64 = StrCharCodeAt(template, i);
    if (c !== 36 || i + 1 >= len) {
      // A run of text up to the next `$` (or a final lone `$`).
      let j: f64 = StrIndexOf(template, "$", i + 1);
      if (j < 0) {
        j = len;
      }
      result += StrSubstring(template, i, j);
      i = j;
      continue;
    }
    const d: f64 = StrCharCodeAt(template, i + 1);
    if (d === 36) {
      result += "$";
      i += 2;
    } else if (d === 38) {
      result += matched;
      i += 2;
    } else if (d === 96) {
      result += StrSubstring(str, 0, position);
      i += 2;
    } else if (d === 39) {
      const tailPos: f64 = position + StrLen(matched);
      if (tailPos < stringLength) {
        result += StrSubstring(str, tailPos, stringLength);
      }
      i += 2;
    } else if (d >= 48 && d <= 57) {
      let digits: f64 = 1;
      let index: f64 = d - 48;
      if (i + 2 < len) {
        const e: f64 = StrCharCodeAt(template, i + 2);
        if (e >= 48 && e <= 57) {
          const two: f64 = index * 10 + (e - 48);
          if (two >= 1 && two <= m) {
            index = two;
            digits = 2;
          }
        }
      }
      if (index >= 1 && index <= m) {
        const capture: Value = Get(captures, index - 1);
        if (capture !== undefined) {
          result += AsStr(capture);
        }
        i += 1 + digits;
      } else {
        result += "$";
        i += 1;
      }
    } else if (d === 60 && namedCaptures !== undefined) {
      const gtPos: f64 = StrIndexOf(template, ">", i);
      if (gtPos < 0) {
        result += "$<";
        i += 2;
      } else {
        const groupName: Str = StrSubstring(template, i + 2, gtPos);
        const capture: Value = Get(AsObj(namedCaptures), groupName);
        if (capture !== undefined) {
          result += ToString(capture);
        }
        i = gtPos + 1;
      }
    } else {
      result += "$";
      i += 1;
    }
  }
  return result;
}

/**
 * RegExp.prototype [ %Symbol.search% ] ( string ), 22.2.6.12.
 * @builtin RegExp.prototype[Symbol.search]
 * @intrinsic
 */
function RegExpPrototypeSearch(this: Value, string: Value): Value {
  const rx: Obj = ThisRegExpObject(this);
  const s: Str = ToString(string);
  const previousLastIndex: Value = Get(rx, "lastIndex");
  if (!SameValue(previousLastIndex, 0)) {
    Set(rx, "lastIndex", 0);
  }
  const result: Value = RegExpExec(rx, s);
  const currentLastIndex: Value = Get(rx, "lastIndex");
  if (!SameValue(currentLastIndex, previousLastIndex)) {
    Set(rx, "lastIndex", previousLastIndex);
  }
  if (result === null) {
    return -1;
  }
  return Get(AsObj(result), "index");
}

/**
 * RegExp.prototype [ %Symbol.split% ] ( string, limit ), 22.2.6.14.
 * @builtin RegExp.prototype[Symbol.split]
 * @intrinsic
 */
function RegExpPrototypeSplit(this: Value, string: Value, limit: Value): Obj {
  const rx: Obj = ThisRegExpObject(this);
  const s: Str = ToString(string);
  // An unmodified RegExp and a limit that runs no user code: the leaf (D1503).
  if ((limit === undefined || IsNumber(limit)) && StrLen(s) > 0 && IsPristineRegExp(rx)) {
    const fastLim: f64 = limit === undefined ? 4294967295 : NumberToUint32(AsNumber(limit));
    if (fastLim === 0) {
      return ArrayCreate(0);
    }
    return RegExpSplitRaw(rx, s, fastLim, undefined);
  }
  const c: Value = SpeciesConstructor(rx, RealmConstructor("RegExp"));
  const flags: Str = ToString(Get(rx, "flags"));
  const unicodeMatching: boolean = FlagsInclude(flags, "u") || FlagsInclude(flags, "v");
  const newFlags: Str = FlagsInclude(flags, "y") ? flags : flags + "y";
  const splitter: Obj = Construct(c, rx, newFlags);
  const a: Obj = ArrayCreate(0);
  let lengthA: f64 = 0;
  const lim: f64 = limit === undefined ? 4294967295 : NumberToUint32(ToNumber(limit));
  if (lim === 0) {
    return a;
  }
  const size: f64 = StrLen(s);
  if (size === 0) {
    const z: Value = RegExpExec(splitter, s);
    if (z !== null) {
      return a;
    }
    CreateDataPropertyOrThrow(a, 0, s);
    return a;
  }
  let p: f64 = 0;
  let q: f64 = p;
  while (q < size) {
    Set(splitter, "lastIndex", q);
    const z: Value = RegExpExec(splitter, s);
    if (z === null) {
      q = AdvanceStringIndex(s, q, unicodeMatching);
      continue;
    }
    let e: f64 = ToLength(Get(splitter, "lastIndex"));
    if (e > size) {
      e = size;
    }
    if (e === p) {
      q = AdvanceStringIndex(s, q, unicodeMatching);
      continue;
    }
    CreateDataPropertyOrThrow(a, lengthA, StrSubstring(s, p, q));
    lengthA++;
    if (lengthA === lim) {
      return a;
    }
    p = e;
    const zo: Obj = AsObj(z);
    const numberOfCaptures: f64 = LengthOfArrayLike(zo);
    for (let i: f64 = 1; i < numberOfCaptures; i++) {
      CreateDataPropertyOrThrow(a, lengthA, Get(zo, i));
      lengthA++;
      if (lengthA === lim) {
        return a;
      }
    }
    q = p;
  }
  CreateDataPropertyOrThrow(a, lengthA, StrSubstring(s, p, size));
  return a;
}
