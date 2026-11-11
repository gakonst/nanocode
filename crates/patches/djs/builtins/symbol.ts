// Symbol (ECMA-262 20.4).

/**
 * Symbol ( [ description ] ), 20.4.1.1.
 * @constructor Symbol
 */
function SymbolConstructor(description?: Value): Sym {
  if (new.target !== undefined) {
    throw NewTypeError("Symbol is not a constructor");
  }
  if (description === undefined) {
    return NewSymbol(undefined);
  }
  return NewSymbol(ToString(description));
}

/**
 * Symbol.for ( key ), 20.4.2.2.
 * @builtin Symbol.for
 */
function SymbolForBuiltin(key: Value): Sym {
  return SymbolFor(ToString(key));
}

/**
 * Symbol.keyFor ( sym ), 20.4.2.6.
 * @builtin Symbol.keyFor
 */
function SymbolKeyForBuiltin(sym: Value): Value {
  if (!IsSymbol(sym)) {
    throw NewTypeError("not a symbol");
  }
  return SymbolKeyFor(AsSym(sym));
}

/** ThisSymbolValue ( value ), 20.4.3.4.1. */
function ThisSymbolValue(value: Value): Sym {
  const s: Value = PrimitiveValue(value);
  if (!IsSymbol(s)) {
    throw NewTypeError("not a symbol");
  }
  return AsSym(s);
}

/**
 * get Symbol.prototype.description, 20.4.3.2.
 * @getter Symbol.prototype.description
 */
function SymbolPrototypeDescription(this: Value): Value {
  return SymbolDescription(ThisSymbolValue(this));
}

/**
 * Symbol.prototype.toString ( ), 20.4.3.3.
 * @builtin Symbol.prototype.toString
 */
function SymbolPrototypeToString(this: Value): Str {
  return SymbolDescriptiveString(ThisSymbolValue(this));
}

/**
 * Symbol.prototype.valueOf ( ), 20.4.3.4.
 * @builtin Symbol.prototype.valueOf
 */
function SymbolPrototypeValueOf(this: Value): Sym {
  return ThisSymbolValue(this);
}

/**
 * Symbol.prototype [ %Symbol.toPrimitive% ] ( hint ), 20.4.3.5.
 * @builtin Symbol.prototype[Symbol.toPrimitive]
 * @attributes configurable
 */
function SymbolPrototypeToPrimitive(this: Value, hint: Value): Sym {
  return ThisSymbolValue(this);
}

/**
 * Symbol.prototype [ %Symbol.toStringTag% ], 20.4.3.6.
 * @value Symbol.prototype[Symbol.toStringTag]
 * @attributes configurable
 */
function SymbolPrototypeToStringTag(): Str {
  return "Symbol";
}

/**
 * Symbol.asyncIterator, 20.4.2.1.
 * @value Symbol.asyncIterator
 */
function SymbolAsyncIterator(): Sym {
  return WellKnownSymbol("asyncIterator");
}

/**
 * Symbol.hasInstance, 20.4.2.3.
 * @value Symbol.hasInstance
 */
function SymbolHasInstance(): Sym {
  return WellKnownSymbol("hasInstance");
}

/**
 * Symbol.isConcatSpreadable, 20.4.2.4.
 * @value Symbol.isConcatSpreadable
 */
function SymbolIsConcatSpreadable(): Sym {
  return WellKnownSymbol("isConcatSpreadable");
}

/**
 * Symbol.iterator, 20.4.2.5.
 * @value Symbol.iterator
 */
function SymbolIterator(): Sym {
  return WellKnownSymbol("iterator");
}

/**
 * Symbol.match, 20.4.2.7.
 * @value Symbol.match
 */
function SymbolMatch(): Sym {
  return WellKnownSymbol("match");
}

/**
 * Symbol.matchAll, 20.4.2.8.
 * @value Symbol.matchAll
 */
function SymbolMatchAll(): Sym {
  return WellKnownSymbol("matchAll");
}

/**
 * Symbol.replace, 20.4.2.10.
 * @value Symbol.replace
 */
function SymbolReplace(): Sym {
  return WellKnownSymbol("replace");
}

/**
 * Symbol.search, 20.4.2.11.
 * @value Symbol.search
 */
function SymbolSearch(): Sym {
  return WellKnownSymbol("search");
}

/**
 * Symbol.species, 20.4.2.12.
 * @value Symbol.species
 */
function SymbolSpecies(): Sym {
  return WellKnownSymbol("species");
}

/**
 * Symbol.split, 20.4.2.13.
 * @value Symbol.split
 */
function SymbolSplit(): Sym {
  return WellKnownSymbol("split");
}

/**
 * Symbol.toPrimitive, 20.4.2.14.
 * @value Symbol.toPrimitive
 */
function SymbolToPrimitive(): Sym {
  return WellKnownSymbol("toPrimitive");
}

/**
 * Symbol.toStringTag, 20.4.2.15.
 * @value Symbol.toStringTag
 */
function SymbolToStringTag(): Sym {
  return WellKnownSymbol("toStringTag");
}

/**
 * Symbol.unscopables, 20.4.2.16.
 * @value Symbol.unscopables
 */
function SymbolUnscopables(): Sym {
  return WellKnownSymbol("unscopables");
}
