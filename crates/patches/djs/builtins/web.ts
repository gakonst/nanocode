// DOMException (WebIDL) and atob/btoa (HTML), as qjs has them (0084, D1840). A
// DOMException is an object with [[ErrorData]] whose name and message are internal
// slots; the prototype's getters read them. Codemode moves images as base64, so atob
// and btoa are native rather than polyfilled. (The file sorts after error.ts: the
// DOMException constructor needs Error installed.)

/** The prototype of the realm's DOMException. */
function DOMExceptionPrototypeObject(): Obj {
  return AsObj(Get(RealmConstructor("DOMException"), "prototype"));
}

/** A new DOMException (the errors of atob and btoa). */
function NewDOMException(message: Str, name: Str): Obj {
  const o: Obj = DOMExceptionCreate(DOMExceptionPrototypeObject(), name, message);
  RecordStack(o);
  return o;
}

/**
 * DOMException ( [ message [ , name ] ] ) (WebIDL): the arguments are converted before
 * the object is created.
 * @constructor DOMException
 * @extends Error
 * @length 0
 */
function DOMExceptionConstructor(message?: Value, name?: Value): Obj {
  if (new.target === undefined) {
    throw NewTypeError("must be called with new");
  }
  const msg: Str = message === undefined ? "" : ToString(message);
  const nm: Str = name === undefined ? "Error" : ToString(name);
  let proto: Value = Get(AsObj(new.target), "prototype");
  if (!IsObject(proto)) {
    proto = DOMExceptionPrototypeObject();
  }
  const o: Obj = DOMExceptionCreate(AsObj(proto), nm, msg);
  RecordStack(o);
  return o;
}

/** Internal slot i of `this`, a DOMException. */
function DOMExceptionThisField(v: Value, i: i32): Str {
  const f: Value = DOMExceptionField(v, i);
  if (f === undefined) {
    throw NewTypeError("DOMException object expected");
  }
  return AsStr(f);
}

/**
 * get DOMException.prototype.name.
 * @getter DOMException.prototype.name
 */
function DOMExceptionPrototypeName(this: Value): Str {
  return DOMExceptionThisField(this, 0);
}

/**
 * get DOMException.prototype.message.
 * @getter DOMException.prototype.message
 */
function DOMExceptionPrototypeMessage(this: Value): Str {
  return DOMExceptionThisField(this, 1);
}

/**
 * get DOMException.prototype.code: the legacy code of the name (0 if none).
 * @getter DOMException.prototype.code
 */
function DOMExceptionPrototypeCode(this: Value): i32 {
  return DOMExceptionCode(DOMExceptionThisField(this, 0));
}

/** Makes the accessor `key` of o enumerable (WebIDL attributes are). */
function DOMExceptionEnumerable(o: Obj, key: Str): void {
  const desc: Obj = ObjectCreate(null);
  CreateDataPropertyOrThrow(desc, "enumerable", true);
  DefineOwnPropertyFromRecord(o, key, desc);
}

/**
 * DOMException.prototype [ %Symbol.toStringTag% ]. Installed after the getters, it also
 * makes them enumerable and defines the legacy code constants (INDEX_SIZE_ERR, ...) on
 * the constructor and the prototype: enumerable, read-only.
 * @value DOMException.prototype[Symbol.toStringTag]
 * @attributes configurable
 */
function DOMExceptionToStringTag(): Str {
  const ctor: Obj = RealmConstructor("DOMException");
  const proto: Obj = DOMExceptionPrototypeObject();
  DOMExceptionEnumerable(proto, "name");
  DOMExceptionEnumerable(proto, "message");
  DOMExceptionEnumerable(proto, "code");
  for (let i: i32 = 1; i <= 25; i++) {
    const key: Str = DOMExceptionConstant(i);
    DefineDataPropertyOrThrow(ctor, key, i, 2);
    DefineDataPropertyOrThrow(proto, key, i, 2);
  }
  return "DOMException";
}

/**
 * btoa ( data ) (HTML): base64 of a string of bytes (code units up to 0xFF).
 * @builtin btoa
 * @length 1
 * @fast
 */
function GlobalBtoa(...args: Value[]): Str {
  if (LengthOfArrayLike(args) === 0) {
    throw NewTypeError("1 argument required");
  }
  const r: Value = Btoa(ToString(Get(args, 0)));
  if (r === undefined) {
    throw NewDOMException("string contains an invalid character", "InvalidCharacterError");
  }
  return AsStr(r);
}

/**
 * atob ( data ) (HTML): forgiving-base64 decoding into a string of bytes.
 * @builtin atob
 * @length 1
 * @fast
 */
function GlobalAtob(...args: Value[]): Str {
  if (LengthOfArrayLike(args) === 0) {
    throw NewTypeError("1 argument required");
  }
  const r: Value = Atob(ToString(Get(args, 0)));
  if (r === undefined) {
    throw NewDOMException("string is not correctly encoded", "InvalidCharacterError");
  }
  return AsStr(r);
}
