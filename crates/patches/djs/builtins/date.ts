// Date (ECMA-262 21.4). Time comes from the host (Vm::set_now) and local time is the
// host's fixed offset (D1383); the arithmetic, formatting and parsing are QuickJS's, in
// crates/djs-core/src/builtins/date.rs.

/** The time value from the constructor's arguments (21.4.2.1 steps 3-5). */
function DateValueFromArgs(args: Obj): f64 {
  const n: f64 = LengthOfArrayLike(args);
  if (n === 0) {
    return DateNow();
  }
  if (n === 1) {
    const value: Value = Get(args, 0);
    if (IsObject(value) && IsDateObject(AsObj(value))) {
      return TimeClip(DateValue(value));
    }
    const v: Value = ToPrimitive(value, "default");
    if (IsString(v)) {
      return TimeClip(DateParse(AsStr(v)));
    }
    return TimeClip(ToNumber(v));
  }
  const fields: Obj = ArrayCreate(0);
  const count: f64 = n < 7 ? n : 7;
  for (let i: f64 = 0; i < count; i++) {
    CreateDataPropertyOrThrow(fields, i, ToNumber(Get(args, i)));
  }
  return DateFromFields(fields, true);
}

/** Whether o has [[DateValue]]. */
function IsDateObject(o: Obj): boolean {
  return ObjectClassTag(o) === "Date";
}

/** TimeClip ( time ), 21.4.1.31. */
function TimeClip(t: f64): f64 {
  if (!(t >= -8.64e15 && t <= 8.64e15)) {
    return 0 / 0;
  }
  return ToIntegerOrInfinityNumber(t);
}

/**
 * Date ( ...values ), 21.4.2.1.
 * @constructor Date
 * @length 7
 * @fast
 */
function DateConstructor(...values: Value[]): Value {
  if (new.target === undefined) {
    return DateFormat(DateNow(), 1, 3);
  }
  const tv: f64 = DateValueFromArgs(values);
  return DateCreate(GetPrototypeFromConstructor(new.target, "DatePrototype"), tv);
}

/**
 * Date.now ( ), 21.4.3.1.
 * @builtin Date.now
 */
function DateNowBuiltin(): f64 {
  return DateNow();
}

/**
 * Date.parse ( string ), 21.4.3.2.
 * @builtin Date.parse
 */
function DateParseBuiltin(string: Value): f64 {
  return DateParse(ToString(string));
}

/**
 * Date.UTC ( year [ , month [ , date [ , hours [ , minutes [ , seconds [ , ms ] ] ] ] ] ] ), 21.4.3.4.
 * @builtin Date.UTC
 * @length 7
 */
function DateUTC(...values: Value[]): f64 {
  const n: f64 = LengthOfArrayLike(values);
  if (n === 0) {
    return 0 / 0;
  }
  const fields: Obj = ArrayCreate(0);
  const count: f64 = n < 7 ? n : 7;
  for (let i: f64 = 0; i < count; i++) {
    CreateDataPropertyOrThrow(fields, i, ToNumber(Get(values, i)));
  }
  return DateFromFields(fields, false);
}

/**
 * Date.prototype.getFullYear ( ).
 * @builtin Date.prototype.getFullYear
 */
function DatePrototypeGetFullYear(this: Value): f64 {
  return DateGetField(DateValue(this), 0, true);
}

/**
 * Date.prototype.getUTCFullYear ( ).
 * @builtin Date.prototype.getUTCFullYear
 */
function DatePrototypeGetUTCFullYear(this: Value): f64 {
  return DateGetField(DateValue(this), 0, false);
}

/**
 * Date.prototype.getMonth ( ).
 * @builtin Date.prototype.getMonth
 */
function DatePrototypeGetMonth(this: Value): f64 {
  return DateGetField(DateValue(this), 1, true);
}

/**
 * Date.prototype.getUTCMonth ( ).
 * @builtin Date.prototype.getUTCMonth
 */
function DatePrototypeGetUTCMonth(this: Value): f64 {
  return DateGetField(DateValue(this), 1, false);
}

/**
 * Date.prototype.getDate ( ).
 * @builtin Date.prototype.getDate
 */
function DatePrototypeGetDate(this: Value): f64 {
  return DateGetField(DateValue(this), 2, true);
}

/**
 * Date.prototype.getUTCDate ( ).
 * @builtin Date.prototype.getUTCDate
 */
function DatePrototypeGetUTCDate(this: Value): f64 {
  return DateGetField(DateValue(this), 2, false);
}

/**
 * Date.prototype.getDay ( ).
 * @builtin Date.prototype.getDay
 */
function DatePrototypeGetDay(this: Value): f64 {
  return DateGetField(DateValue(this), 7, true);
}

/**
 * Date.prototype.getUTCDay ( ).
 * @builtin Date.prototype.getUTCDay
 */
function DatePrototypeGetUTCDay(this: Value): f64 {
  return DateGetField(DateValue(this), 7, false);
}

/**
 * Date.prototype.getHours ( ).
 * @builtin Date.prototype.getHours
 */
function DatePrototypeGetHours(this: Value): f64 {
  return DateGetField(DateValue(this), 3, true);
}

/**
 * Date.prototype.getUTCHours ( ).
 * @builtin Date.prototype.getUTCHours
 */
function DatePrototypeGetUTCHours(this: Value): f64 {
  return DateGetField(DateValue(this), 3, false);
}

/**
 * Date.prototype.getMinutes ( ).
 * @builtin Date.prototype.getMinutes
 */
function DatePrototypeGetMinutes(this: Value): f64 {
  return DateGetField(DateValue(this), 4, true);
}

/**
 * Date.prototype.getUTCMinutes ( ).
 * @builtin Date.prototype.getUTCMinutes
 */
function DatePrototypeGetUTCMinutes(this: Value): f64 {
  return DateGetField(DateValue(this), 4, false);
}

/**
 * Date.prototype.getSeconds ( ).
 * @builtin Date.prototype.getSeconds
 */
function DatePrototypeGetSeconds(this: Value): f64 {
  return DateGetField(DateValue(this), 5, true);
}

/**
 * Date.prototype.getUTCSeconds ( ).
 * @builtin Date.prototype.getUTCSeconds
 */
function DatePrototypeGetUTCSeconds(this: Value): f64 {
  return DateGetField(DateValue(this), 5, false);
}

/**
 * Date.prototype.getMilliseconds ( ).
 * @builtin Date.prototype.getMilliseconds
 */
function DatePrototypeGetMilliseconds(this: Value): f64 {
  return DateGetField(DateValue(this), 6, true);
}

/**
 * Date.prototype.getUTCMilliseconds ( ).
 * @builtin Date.prototype.getUTCMilliseconds
 */
function DatePrototypeGetUTCMilliseconds(this: Value): f64 {
  return DateGetField(DateValue(this), 6, false);
}

/**
 * Date.prototype.setMilliseconds ( ... ).
 * @builtin Date.prototype.setMilliseconds
 * @length 1
 */
function DatePrototypeSetMilliseconds(this: Value, ...args: Value[]): f64 {
  return DateSetter(this, args, 6, 1, true);
}

/**
 * Date.prototype.setUTCMilliseconds ( ... ).
 * @builtin Date.prototype.setUTCMilliseconds
 * @length 1
 */
function DatePrototypeSetUTCMilliseconds(this: Value, ...args: Value[]): f64 {
  return DateSetter(this, args, 6, 1, false);
}

/**
 * Date.prototype.setSeconds ( ... ).
 * @builtin Date.prototype.setSeconds
 * @length 2
 */
function DatePrototypeSetSeconds(this: Value, ...args: Value[]): f64 {
  return DateSetter(this, args, 5, 2, true);
}

/**
 * Date.prototype.setUTCSeconds ( ... ).
 * @builtin Date.prototype.setUTCSeconds
 * @length 2
 */
function DatePrototypeSetUTCSeconds(this: Value, ...args: Value[]): f64 {
  return DateSetter(this, args, 5, 2, false);
}

/**
 * Date.prototype.setMinutes ( ... ).
 * @builtin Date.prototype.setMinutes
 * @length 3
 */
function DatePrototypeSetMinutes(this: Value, ...args: Value[]): f64 {
  return DateSetter(this, args, 4, 3, true);
}

/**
 * Date.prototype.setUTCMinutes ( ... ).
 * @builtin Date.prototype.setUTCMinutes
 * @length 3
 */
function DatePrototypeSetUTCMinutes(this: Value, ...args: Value[]): f64 {
  return DateSetter(this, args, 4, 3, false);
}

/**
 * Date.prototype.setHours ( ... ).
 * @builtin Date.prototype.setHours
 * @length 4
 */
function DatePrototypeSetHours(this: Value, ...args: Value[]): f64 {
  return DateSetter(this, args, 3, 4, true);
}

/**
 * Date.prototype.setUTCHours ( ... ).
 * @builtin Date.prototype.setUTCHours
 * @length 4
 */
function DatePrototypeSetUTCHours(this: Value, ...args: Value[]): f64 {
  return DateSetter(this, args, 3, 4, false);
}

/**
 * Date.prototype.setDate ( ... ).
 * @builtin Date.prototype.setDate
 * @length 1
 */
function DatePrototypeSetDate(this: Value, ...args: Value[]): f64 {
  return DateSetter(this, args, 2, 1, true);
}

/**
 * Date.prototype.setUTCDate ( ... ).
 * @builtin Date.prototype.setUTCDate
 * @length 1
 */
function DatePrototypeSetUTCDate(this: Value, ...args: Value[]): f64 {
  return DateSetter(this, args, 2, 1, false);
}

/**
 * Date.prototype.setMonth ( ... ).
 * @builtin Date.prototype.setMonth
 * @length 2
 */
function DatePrototypeSetMonth(this: Value, ...args: Value[]): f64 {
  return DateSetter(this, args, 1, 2, true);
}

/**
 * Date.prototype.setUTCMonth ( ... ).
 * @builtin Date.prototype.setUTCMonth
 * @length 2
 */
function DatePrototypeSetUTCMonth(this: Value, ...args: Value[]): f64 {
  return DateSetter(this, args, 1, 2, false);
}

/**
 * Date.prototype.setFullYear ( ... ).
 * @builtin Date.prototype.setFullYear
 * @length 3
 */
function DatePrototypeSetFullYear(this: Value, ...args: Value[]): f64 {
  return DateSetter(this, args, 0, 3, true);
}

/**
 * Date.prototype.setUTCFullYear ( ... ).
 * @builtin Date.prototype.setUTCFullYear
 * @length 3
 */
function DatePrototypeSetUTCFullYear(this: Value, ...args: Value[]): f64 {
  return DateSetter(this, args, 0, 3, false);
}

/**
 * The setters (21.4.4.20-28): thisTimeValue first, then ToNumber of the arguments in
 * order (observable, always done), then the new time value.
 */
function DateSetter(date: Value, args: Obj, first: i32, maxCount: f64, local: boolean): f64 {
  const t: f64 = DateValue(date);
  const n: f64 = LengthOfArrayLike(args);
  const count: f64 = n < maxCount ? n : maxCount;
  const nums: Obj = ArrayCreate(0);
  for (let i: f64 = 0; i < count; i++) {
    CreateDataPropertyOrThrow(nums, i, ToNumber(Get(args, i)));
  }
  // An invalid date stays as it is (only setFullYear starts over from +0); the value
  // may have changed while the arguments were converted.
  if (t !== t && first !== 0) {
    return t;
  }
  return DateSetValue(date, DateSetFields(t, first, nums, local));
}

/**
 * Date.prototype.getTime ( ), 21.4.4.10.
 * @builtin Date.prototype.getTime
 */
function DatePrototypeGetTime(this: Value): f64 {
  return DateValue(this);
}

/**
 * Date.prototype.valueOf ( ), 21.4.4.44.
 * @builtin Date.prototype.valueOf
 * @intrinsic
 */
function DatePrototypeValueOf(this: Value): f64 {
  return DateValue(this);
}

/**
 * Date.prototype.getTimezoneOffset ( ), 21.4.4.11.
 * @builtin Date.prototype.getTimezoneOffset
 */
function DatePrototypeGetTimezoneOffset(this: Value): f64 {
  return DateTimezoneOffset(DateValue(this));
}

/**
 * Date.prototype.setTime ( time ), 21.4.4.27.
 * @builtin Date.prototype.setTime
 */
function DatePrototypeSetTime(this: Value, time: Value): f64 {
  DateValue(this);
  const t: f64 = ToNumber(time);
  return DateSetValue(this, TimeClip(t));
}

/**
 * Date.prototype.toString ( ).
 * @builtin Date.prototype.toString
 */
function DatePrototypeToString(this: Value): Str {
  return DateFormat(DateValue(this), 1, 3);
}

/**
 * Date.prototype.toDateString ( ).
 * @builtin Date.prototype.toDateString
 */
function DatePrototypeToDateString(this: Value): Str {
  return DateFormat(DateValue(this), 1, 1);
}

/**
 * Date.prototype.toTimeString ( ).
 * @builtin Date.prototype.toTimeString
 */
function DatePrototypeToTimeString(this: Value): Str {
  return DateFormat(DateValue(this), 1, 2);
}

/**
 * Date.prototype.toISOString ( ).
 * @builtin Date.prototype.toISOString
 * @intrinsic
 */
function DatePrototypeToISOString(this: Value): Str {
  return DateFormat(DateValue(this), 2, 3);
}

/**
 * Date.prototype.toLocaleString ( ).
 * @builtin Date.prototype.toLocaleString
 */
function DatePrototypeToLocaleString(this: Value): Str {
  return DateFormat(DateValue(this), 3, 3);
}

/**
 * Date.prototype.toLocaleDateString ( ).
 * @builtin Date.prototype.toLocaleDateString
 */
function DatePrototypeToLocaleDateString(this: Value): Str {
  return DateFormat(DateValue(this), 3, 1);
}

/**
 * Date.prototype.toLocaleTimeString ( ).
 * @builtin Date.prototype.toLocaleTimeString
 */
function DatePrototypeToLocaleTimeString(this: Value): Str {
  return DateFormat(DateValue(this), 3, 2);
}

/**
 * Date.prototype.toUTCString ( ), 21.4.4.43 (toGMTString is the same function, B.2.3.1).
 * @builtin Date.prototype.toUTCString
 * @builtin Date.prototype.toGMTString
 */
function DatePrototypeToUTCString(this: Value): Str {
  return DateFormat(DateValue(this), 0, 3);
}

/**
 * Date.prototype.getYear ( ), B.2.3.1.
 * @builtin Date.prototype.getYear
 */
function DatePrototypeGetYear(this: Value): f64 {
  const y: f64 = DateGetField(DateValue(this), 0, true);
  return y - 1900;
}

/**
 * Date.prototype.toJSON ( key ), 21.4.4.37.
 * @builtin Date.prototype.toJSON
 * @intrinsic
 */
function DatePrototypeToJSON(this: Value, key: Value): Value {
  const o: Obj = ToObject(this);
  const tv: Value = ToPrimitive(o, "number");
  if (IsNumber(tv)) {
    const n: f64 = AsNumber(tv);
    if (n !== n || n === 1 / 0 || n === -1 / 0) {
      return null;
    }
  }
  return Call(Get(o, "toISOString"), o);
}

/**
 * Date.prototype [ %Symbol.toPrimitive% ] ( hint ), 21.4.4.45.
 * @builtin Date.prototype[Symbol.toPrimitive]
 * @attributes configurable
 * @intrinsic
 */
function DatePrototypeToPrimitive(this: Value, hint: Value): Value {
  if (!IsObject(this)) {
    throw NewTypeError("not an object");
  }
  if (hint === "string" || hint === "default") {
    return OrdinaryToPrimitive(AsObj(this), "string");
  }
  if (hint === "number") {
    return OrdinaryToPrimitive(AsObj(this), "number");
  }
  throw NewTypeError("invalid hint");
}
