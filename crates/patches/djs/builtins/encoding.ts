// TextEncoder and TextDecoder (WHATWG Encoding standard, UTF-8 only; 0244, D3172). The
// arguments are converted as WebIDL does, in order, before the steps; the labels are
// checked and a BufferSource's bytes found here (as a buffer, an offset and a length),
// and the leaves in builtins/encoding.rs transcode. Messages are node's. Methods and
// accessors are enumerable, as WebIDL's are.
// @embed-only: left out of the preludes of builds without `embed` (D3175).

/** The prototype for a new codec: new.target's, else the realm constructor `name`'s. */
function TextCodecPrototype(newTarget: Value, name: Str): Obj {
  const proto: Value = Get(AsObj(newTarget), "prototype");
  if (IsObject(proto)) {
    return AsObj(proto);
  }
  return AsObj(Get(RealmConstructor(name), "prototype"));
}

/**
 * Whether the WebIDL dictionary argument v has members to read: false for undefined
 * and null, true for an object, else TypeError.
 */
function TextDictionary(v: Value): boolean {
  if (v === undefined || v === null) {
    return false;
  }
  if (!IsObject(v)) {
    throw NewTypeError("The \"options\" argument must be of type object");
  }
  return true;
}

/** Whether c is ASCII whitespace (TAB, LF, FF, CR, SPACE). */
function TextIsAsciiWhitespace(c: f64): boolean {
  return c === 9 || c === 10 || c === 12 || c === 13 || c === 32;
}

/**
 * Whether label names UTF-8, the one encoding djs has: get an encoding (Encoding
 * standard, 4.2) trims ASCII whitespace and ignores ASCII case. (No other code point
 * lowercases to a letter of these labels.)
 */
function TextLabelIsUtf8(label: Str): boolean {
  let start: f64 = 0;
  let end: f64 = StrLength(label);
  while (start < end && TextIsAsciiWhitespace(StrCharCodeAt(label, start))) {
    start += 1;
  }
  while (end > start && TextIsAsciiWhitespace(StrCharCodeAt(label, end - 1))) {
    end -= 1;
  }
  const l: Str = StrToCase(StrSubstring(label, start, end), false);
  return l === "utf-8" || l === "utf8" || l === "unicode-1-1-utf-8" || l === "unicode11utf8" ||
    l === "unicode20utf8" || l === "x-unicode20utf8";
}

/**
 * TextEncoder ( ).
 * @constructor TextEncoder
 */
function TextEncoderConstructor(): Obj {
  if (new.target === undefined) {
    throw NewTypeError("must be called with new");
  }
  return TextCodecCreate(TextCodecPrototype(new.target, "TextEncoder"), 0);
}

/**
 * get TextEncoder.prototype.encoding.
 * @getter TextEncoder.prototype.encoding
 */
function TextEncoderPrototypeEncoding(this: Value): Str {
  TextCodecFlags(this, false);
  return "utf-8";
}

/**
 * TextEncoder.prototype.encode ( optional USVString input = "" ).
 * @builtin TextEncoder.prototype.encode
 */
function TextEncoderPrototypeEncode(this: Value, input?: Value): Obj {
  TextCodecFlags(this, false);
  return AsObj(Utf8Encode(input === undefined ? "" : ToString(input), undefined, 0, 0));
}

/**
 * TextEncoder.prototype.encodeInto ( USVString source, Uint8Array destination ).
 * @builtin TextEncoder.prototype.encodeInto
 * @length 2
 */
function TextEncoderPrototypeEncodeInto(this: Value, ...args: Value[]): Obj {
  TextCodecFlags(this, false);
  if (LengthOfArrayLike(args) < 2) {
    throw NewTypeError("2 arguments required");
  }
  const source: Str = ToString(Get(args, 0));
  const dest: Value = Get(args, 1);
  // Uint8Array is kind 1.
  if (!IsTypedArray(dest) || TypedArrayField(dest, 0) !== 1) {
    throw NewTypeError("The \"dest\" argument must be an instance of Uint8Array");
  }
  const at: Value = TypedArrayField(dest, 2);
  return AsObj(Utf8Encode(source, TypedArrayField(dest, 1), at, TypedArrayField(dest, 4)));
}

/**
 * TextEncoder.prototype [ %Symbol.toStringTag% ]; installed last, it makes the members
 * enumerable.
 * @value TextEncoder.prototype[Symbol.toStringTag]
 * @attributes configurable
 */
function TextEncoderToStringTag(): Str {
  const proto: Obj = AsObj(Get(RealmConstructor("TextEncoder"), "prototype"));
  DOMExceptionEnumerable(proto, "encoding");
  DOMExceptionEnumerable(proto, "encode");
  DOMExceptionEnumerable(proto, "encodeInto");
  return "TextEncoder";
}

/**
 * TextDecoder ( optional DOMString label = "utf-8", optional TextDecoderOptions options ).
 * @constructor TextDecoder
 * @length 0
 */
function TextDecoderConstructor(label?: Value, options?: Value): Obj {
  if (new.target === undefined) {
    throw NewTypeError("must be called with new");
  }
  const l: Str = label === undefined ? "utf-8" : ToString(label);
  let flags: i32 = 1;
  if (TextDictionary(options)) {
    if (ToBoolean(Get(AsObj(options), "fatal"))) {
      flags = flags | 2;
    }
    if (ToBoolean(Get(AsObj(options), "ignoreBOM"))) {
      flags = flags | 4;
    }
  }
  const proto: Obj = TextCodecPrototype(new.target, "TextDecoder");
  if (!TextLabelIsUtf8(l)) {
    throw NewRangeError("The \"" + l + "\" encoding is not supported");
  }
  return TextCodecCreate(proto, flags);
}

/**
 * get TextDecoder.prototype.encoding.
 * @getter TextDecoder.prototype.encoding
 */
function TextDecoderPrototypeEncoding(this: Value): Str {
  TextCodecFlags(this, true);
  return "utf-8";
}

/**
 * get TextDecoder.prototype.fatal.
 * @getter TextDecoder.prototype.fatal
 */
function TextDecoderPrototypeFatal(this: Value): boolean {
  return (TextCodecFlags(this, true) & 2) !== 0;
}

/**
 * get TextDecoder.prototype.ignoreBOM.
 * @getter TextDecoder.prototype.ignoreBOM
 */
function TextDecoderPrototypeIgnoreBOM(this: Value): boolean {
  return (TextCodecFlags(this, true) & 4) !== 0;
}

/**
 * TextDecoder.prototype.decode ( optional AllowSharedBufferSource input, optional
 * TextDecodeOptions options ).
 * @builtin TextDecoder.prototype.decode
 * @length 0
 */
function TextDecoderPrototypeDecode(this: Value, input?: Value, options?: Value): Str {
  TextCodecFlags(this, true);
  if (input !== undefined && !IsBufferSource(input)) {
    throw NewTypeError("The \"input\" argument must be an instance of ArrayBuffer or ArrayBufferView");
  }
  let stream: boolean = false;
  if (TextDictionary(options)) {
    stream = ToBoolean(Get(AsObj(options), "stream"));
  }
  // The bytes: an ArrayBuffer's all (length -1), a view's range (none when out of bounds).
  let buffer: Value = input;
  let offset: Value = 0;
  let length: Value = -1;
  if (IsTypedArray(input)) {
    buffer = TypedArrayField(input, 1);
    offset = TypedArrayField(input, 2);
    length = TypedArrayField(input, 4);
  } else if (IsDataView(input)) {
    buffer = DataViewField(input, 0);
    offset = DataViewField(input, 1);
    length = DataViewField(input, 2);
  }
  const r: Value = Utf8Decode(AsObj(this), buffer, offset, length, stream);
  if (r === undefined) {
    throw NewTypeError("The encoded data was not valid for encoding utf-8");
  }
  return AsStr(r);
}

/**
 * TextDecoder.prototype [ %Symbol.toStringTag% ]; installed last, it makes the members
 * enumerable.
 * @value TextDecoder.prototype[Symbol.toStringTag]
 * @attributes configurable
 */
function TextDecoderToStringTag(): Str {
  const proto: Obj = AsObj(Get(RealmConstructor("TextDecoder"), "prototype"));
  DOMExceptionEnumerable(proto, "encoding");
  DOMExceptionEnumerable(proto, "fatal");
  DOMExceptionEnumerable(proto, "ignoreBOM");
  DOMExceptionEnumerable(proto, "decode");
  return "TextDecoder";
}
