// The JSON object (ECMA-262 25.5). The parse and the plain-data stringify are leaves over
// crate::json (D1331); the reviver and the full stringify algorithm are here.

/**
 * The JSON object, 25.5.
 * @value globalThis.JSON
 * @attributes writable configurable
 */
function JSONObject(): Obj {
  return ObjectCreate(RealmIntrinsic("ObjectPrototype"));
}

/**
 * JSON [ %Symbol.toStringTag% ], 25.5.3.
 * @value JSON[Symbol.toStringTag]
 * @attributes configurable
 */
function JSONToStringTag(): Str {
  return "JSON";
}

/**
 * JSON.parse ( text [ , reviver ] ), 25.5.1.
 * @builtin JSON.parse
 */
function JSONParse(text: Value, reviver: Value): Value {
  const jsonString: Str = ToString(text);
  if (!IsCallable(reviver)) {
    return JsonParse(jsonString, false);
  }
  // [value, JSON Parse Record] for source text access (D1611).
  const pair: Obj = AsObj(JsonParse(jsonString, true));
  const root: Obj = ObjectCreate(RealmIntrinsic("ObjectPrototype"));
  CreateDataPropertyOrThrow(root, "", Get(pair, 0));
  return InternalizeJSONProperty(root, "", reviver, Get(pair, 1));
}

/**
 * InternalizeJSONProperty ( holder, name, reviver, valRecord ), 25.5.1.1 with JSON.parse
 * source text access: `rec` is the JSON Parse Record of the parsed value at this place
 * (an internal [value, source, elements, entries] array) or undefined. It applies only
 * while the value there is still the parsed one.
 */
function InternalizeJSONProperty(holder: Obj, name: Str, reviver: Value, rec: Value): Value {
  const val: Value = Get(holder, name);
  const context: Obj = ObjectCreate(RealmIntrinsic("ObjectPrototype"));
  let elements: Value = undefined;
  let entries: Value = undefined;
  if (rec !== undefined && SameValue(Get(AsObj(rec), 0), val)) {
    if (!IsObject(val)) {
      CreateDataPropertyOrThrow(context, "source", Get(AsObj(rec), 1));
    }
    elements = Get(AsObj(rec), 2);
    entries = Get(AsObj(rec), 3);
  }
  if (IsObject(val)) {
    const o: Obj = AsObj(val);
    if (IsArray(o)) {
      const len: f64 = LengthOfArrayLike(o);
      const known: f64 = elements === undefined ? 0 : LengthOfArrayLike(AsObj(elements));
      for (let i: f64 = 0; i < len; i++) {
        const elementRec: Value = i < known ? Get(AsObj(elements), i) : undefined;
        InternalizeJSONMember(o, ToStringPrimitive(i), reviver, elementRec);
      }
    } else {
      const keys: Obj = OwnPropertyKeys(o, 3);
      const n: f64 = LengthOfArrayLike(keys);
      for (let k: f64 = 0; k < n; k++) {
        const p: Str = AsStr(Get(keys, k));
        let entryRec: Value = undefined;
        if (entries !== undefined && HasOwnProperty(AsObj(entries), p)) {
          entryRec = Get(AsObj(entries), p);
        }
        InternalizeJSONMember(o, p, reviver, entryRec);
      }
    }
  }
  return Call(reviver, holder, name, val, context);
}

/** The loop body of InternalizeJSONProperty: revive o[p], delete it if undefined. */
function InternalizeJSONMember(o: Obj, p: Str, reviver: Value, rec: Value): void {
  const newElement: Value = InternalizeJSONProperty(o, p, reviver, rec);
  if (newElement === undefined) {
    DeleteProperty(o, p);
  } else {
    CreateDataProperty(o, p, newElement);
  }
}

/**
 * JSON.rawJSON ( text ), JSON.parse source text access.
 * @builtin JSON.rawJSON
 */
function JSONRawJSON(text: Value): Obj {
  return JsonRawJSON(ToString(text));
}

/**
 * JSON.isRawJSON ( O ), JSON.parse source text access.
 * @builtin JSON.isRawJSON
 */
function JSONIsRawJSON(o: Value): boolean {
  return IsRawJSON(o);
}

/**
 * JSON.stringify ( value [ , replacer [ , space ] ] ), 25.5.2. Without a replacer, plain
 * data takes the leaf fast path; everything else runs the full algorithm here.
 * @builtin JSON.stringify
 */
function JSONStringify(value: Value, replacer: Value, space: Value): Value {
  let replacerFunction: Value = undefined;
  let propertyList: Value = undefined;
  if (IsObject(replacer)) {
    if (IsCallable(replacer)) {
      replacerFunction = replacer;
    } else if (IsArray(replacer)) {
      propertyList = JSONPropertyList(AsObj(replacer));
    }
  }
  let sp: Value = space;
  if (IsObject(sp)) {
    const p: Value = PrimitiveValue(sp);
    if (IsNumber(p)) {
      sp = ToNumber(sp);
    } else if (IsString(p)) {
      sp = ToString(sp);
    }
  }
  let gap: Str = "";
  if (IsNumber(sp)) {
    const n: f64 = ToIntegerOrInfinityNumber(AsNumber(sp));
    for (let i: f64 = 0; i < n && i < 10; i++) {
      gap += " ";
    }
  } else if (IsString(sp)) {
    const s: Str = AsStr(sp);
    gap = StrLength(s) <= 10 ? s : StrSubstring(s, 0, 10);
  }
  if (replacerFunction === undefined && propertyList === undefined) {
    const fast: Value = JsonStringifyFast(value, gap);
    if (fast !== null) {
      return fast;
    }
  }
  const wrapper: Obj = ObjectCreate(RealmIntrinsic("ObjectPrototype"));
  CreateDataPropertyOrThrow(wrapper, "", value);
  return SerializeJSONProperty(replacerFunction, propertyList, gap, ArrayCreate(0), "", "", wrapper);
}

/** The PropertyList of an array replacer (25.5.2 step 4.b), without duplicates. */
function JSONPropertyList(r: Obj): Obj {
  const list: Obj = ArrayCreate(0);
  let n: f64 = 0;
  const len: f64 = LengthOfArrayLike(r);
  for (let k: f64 = 0; k < len; k++) {
    const v: Value = Get(r, k);
    let item: Value = undefined;
    if (IsString(v)) {
      item = v;
    } else if (IsNumber(v)) {
      item = ToStringPrimitive(v);
    } else if (IsObject(v)) {
      const p: Value = PrimitiveValue(v);
      if (IsString(p) || IsNumber(p)) {
        item = ToString(v);
      }
    }
    if (item !== undefined && !JSONListIncludes(list, n, item)) {
      CreateDataPropertyOrThrow(list, n, item);
      n++;
    }
  }
  return list;
}

/** Whether the first n elements of the internal array list include v. */
function JSONListIncludes(list: Obj, n: f64, v: Value): boolean {
  for (let i: f64 = 0; i < n; i++) {
    if (Get(list, i) === v) {
      return true;
    }
  }
  return false;
}

/**
 * SerializeJSONProperty ( state, key, holder ), 25.5.2.2: a string, or undefined. The
 * state is replacer, propertyList, gap, stack and indent.
 * @intrinsic
 * @native
 */
function SerializeJSONProperty(
  replacer: Value,
  propertyList: Value,
  gap: Str,
  stack: Obj,
  indent: Str,
  key: Str,
  holder: Obj,
): Value {
  let value: Value = Get(holder, key);
  if (IsObject(value) || IsBigInt(value)) {
    const toJSON: Value = GetV(value, "toJSON");
    if (IsCallable(toJSON)) {
      value = Call(toJSON, value, key);
    }
  }
  if (replacer !== undefined) {
    value = Call(replacer, holder, key, value);
  }
  if (IsRawJSON(value)) {
    // Its text, verbatim (source text access).
    return Get(AsObj(value), "rawJSON");
  }
  if (IsObject(value)) {
    const p: Value = PrimitiveValue(value);
    if (IsNumber(p)) {
      value = ToNumber(value);
    } else if (IsString(p)) {
      value = ToString(value);
    } else if (IsBoolean(p) || IsBigInt(p)) {
      value = p;
    }
  }
  if (value === null) {
    return "null";
  }
  if (value === true) {
    return "true";
  }
  if (value === false) {
    return "false";
  }
  if (IsString(value)) {
    return JsonQuote(AsStr(value));
  }
  if (IsNumber(value)) {
    const n: f64 = AsNumber(value);
    // Finite numbers only: NaN and the infinities give NaN here.
    return n - n === 0 ? ToStringPrimitive(value) : "null";
  }
  if (IsBigInt(value)) {
    throw NewTypeError("Do not know how to serialize a BigInt");
  }
  if (IsObject(value) && !IsCallable(value)) {
    const o: Obj = AsObj(value);
    // A plain subtree in one step, without a replacer function (D1901).
    if (replacer === undefined) {
      const fast: Value = JsonStringifyFastAt(o, gap, stack, propertyList);
      if (fast !== null) {
        return fast;
      }
    }
    if (JSONStackIncludes(stack, o)) {
      throw NewTypeError("circular reference");
    }
    const depth: f64 = LengthOfArrayLike(stack);
    CreateDataPropertyOrThrow(stack, depth, o);
    const r: Str = IsArray(o)
      ? SerializeJSONArray(replacer, propertyList, gap, stack, indent, o)
      : SerializeJSONObject(replacer, propertyList, gap, stack, indent, o);
    Set(stack, "length", depth);
    return r;
  }
  return undefined;
}

/**
 * Whether the internal stack array holds o.
 * @intrinsic
 * @native
 */
function JSONStackIncludes(stack: Obj, o: Obj): boolean {
  const n: f64 = LengthOfArrayLike(stack);
  for (let i: f64 = 0; i < n; i++) {
    if (Get(stack, i) === o) {
      return true;
    }
  }
  return false;
}

/**
 * The members joined: `{a,b}`, or with a gap, one per line at `indent`.
 * @intrinsic
 * @native
 */
function JSONJoin(open: Str, close: Str, partial: Obj, n: f64, gap: Str, stepback: Str, indent: Str): Str {
  if (n === 0) {
    return open + close;
  }
  const sep: Str = gap === "" ? "," : ",\n" + indent;
  let r: Str = gap === "" ? open : open + "\n" + indent;
  for (let i: f64 = 0; i < n; i++) {
    if (i > 0) {
      r += sep;
    }
    r += AsStr(Get(partial, i));
  }
  return gap === "" ? r + close : r + "\n" + stepback + close;
}

/**
 * SerializeJSONObject ( state, value ), 25.5.2.5, after the cycle check.
 * @intrinsic
 * @native
 */
function SerializeJSONObject(
  replacer: Value,
  propertyList: Value,
  gap: Str,
  stack: Obj,
  stepback: Str,
  value: Obj,
): Str {
  const indent: Str = stepback + gap;
  const keys: Obj = propertyList === undefined ? OwnPropertyKeys(value, 3) : AsObj(propertyList);
  const len: f64 = LengthOfArrayLike(keys);
  const partial: Obj = ArrayCreate(0);
  let n: f64 = 0;
  for (let k: f64 = 0; k < len; k++) {
    const p: Str = AsStr(Get(keys, k));
    const strP: Value = SerializeJSONProperty(replacer, propertyList, gap, stack, indent, p, value);
    if (strP !== undefined) {
      const member: Str = gap === "" ? JsonQuote(p) + ":" : JsonQuote(p) + ": ";
      CreateDataPropertyOrThrow(partial, n, member + AsStr(strP));
      n++;
    }
  }
  return JSONJoin("{", "}", partial, n, gap, stepback, indent);
}

/**
 * SerializeJSONArray ( state, value ), 25.5.2.6, after the cycle check.
 * @intrinsic
 * @native
 */
function SerializeJSONArray(
  replacer: Value,
  propertyList: Value,
  gap: Str,
  stack: Obj,
  stepback: Str,
  value: Obj,
): Str {
  const indent: Str = stepback + gap;
  const len: f64 = LengthOfArrayLike(value);
  const partial: Obj = ArrayCreate(0);
  for (let i: f64 = 0; i < len; i++) {
    const strP: Value = SerializeJSONProperty(
      replacer,
      propertyList,
      gap,
      stack,
      indent,
      ToStringPrimitive(i),
      value,
    );
    CreateDataPropertyOrThrow(partial, i, strP === undefined ? "null" : AsStr(strP));
  }
  return JSONJoin("[", "]", partial, len, gap, stepback, indent);
}
