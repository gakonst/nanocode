//! The intrinsic id table: the immediate of `call_intrinsic id, argc` (docs/compiler.md
//! 7.3, D1270).
//!
//! The table is the contract between the compiler (privileged mode resolves `%Name` to
//! an id) and the interpreter (dispatches on the id). Its source of truth is
//! `builtins/intrinsics.d.ts` (the Rust intrinsics, in declaration order) followed by the
//! builtins tagged `@intrinsic` in `builtins/*.ts` (in file-name order, then declaration
//! order, the order of `builtins/gen/prelude.js`). Those are JS functions that the
//! prelude registers with `%InstallIntrinsic(name, fn)`. The `table_matches_sources` test
//! checks the table against those files and prints the expected Rust when they differ.
//!
//! Ids are indexes into [`INTRINSICS`]. They are part of the bytecode format, so the
//! table is hashed into [`INTRINSICS_HASH`] (combine it into the build hash with
//! `OPS_HASH`).

/// What runs for an intrinsic.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[repr(u8)]
pub enum IntrinsicKind {
    /// Rust that never runs user code (`@leaf`).
    Leaf,
    /// Rust that may call one user function whose result completes it (`@reentrant`,
    /// D1173).
    Reentrant,
    /// A JS builtin (`@intrinsic`), registered at prelude install time by
    /// `%InstallIntrinsic`; calling it is an ordinary JS call.
    Builtin,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct IntrinsicInfo {
    pub name: &'static str,
    pub kind: IntrinsicKind,
    /// Declared parameters (without a rest parameter, without `this`).
    pub params: u8,
    /// Takes `...args` after the declared parameters.
    pub variadic: bool,
}

const fn i(name: &'static str, kind: IntrinsicKind, params: u8, variadic: bool) -> IntrinsicInfo {
    IntrinsicInfo {
        name,
        kind,
        params,
        variadic,
    }
}

use IntrinsicKind::{Builtin as B, Leaf as L, Reentrant as R};

/// All intrinsics; the id is the index.
pub const INTRINSICS: &[IntrinsicInfo] = &[
    i("IsObject", L, 1, false),
    i("IsCallable", L, 1, false),
    i("IsConstructor", L, 1, false),
    i("IsArray", L, 1, false),
    i("IsString", L, 1, false),
    i("IsNumber", L, 1, false),
    i("IsSymbol", L, 1, false),
    i("IsBoolean", L, 1, false),
    i("ArgumentCount", L, 0, false),
    i("Throw", L, 1, false),
    i("AsObj", L, 1, false),
    i("AsFn", L, 1, false),
    i("AsStr", L, 1, false),
    i("AsNumber", L, 1, false),
    i("AsSym", L, 1, false),
    i("AsBool", L, 1, false),
    i("ToBoolean", L, 1, false),
    i("ToNumberPrimitive", L, 1, false),
    i("ToStringPrimitive", L, 1, false),
    i("ToObject", L, 1, false),
    i("ToIntegerOrInfinityNumber", L, 1, false),
    i("F64ToU32", L, 1, false),
    i("F64ToI32", L, 1, false),
    i("StrLength", L, 1, false),
    i("Get", R, 2, false),
    i("GetV", R, 2, false),
    i("Set", R, 3, false),
    i("HasProperty", R, 2, false),
    i("CreateDataPropertyOrThrow", R, 3, false),
    i("DeletePropertyOrThrow", R, 2, false),
    i("Call", R, 2, true),
    i("Construct", R, 1, true),
    i("ObjectCreate", L, 1, false),
    i("ArrayCreate", L, 1, false),
    i("NewTypeError", L, 1, false),
    i("NewNotConstructorError", L, 1, false),
    i("NewSyntaxError", L, 1, false),
    i("NewReferenceError", L, 1, false),
    i("NewRangeError", L, 1, false),
    i("WellKnownSymbol", L, 1, false),
    i("RealmIntrinsic", L, 1, false),
    i("ConstructorPrototype", L, 2, false),
    i("ErrorCreate", L, 1, false),
    i("DOMExceptionCreate", L, 3, false),
    i("DOMExceptionField", L, 2, false),
    i("DOMExceptionCode", L, 1, false),
    i("DOMExceptionConstant", L, 1, false),
    i("Btoa", L, 1, false),
    i("Atob", L, 1, false),
    i("CaptureStack", L, 1, false),
    i("ErrorStack", L, 1, false),
    i("StackTraceValue", L, 1, false),
    i("IsPendingStack", L, 1, false),
    i("SetErrorStack", L, 2, false),
    i("CallSiteSlot", L, 2, false),
    i("DefineDataPropertyOrThrow", R, 4, false),
    i("DefineOwnPropertyFromRecord", R, 3, false),
    i("GetOwnPropertyDescriptorObject", R, 2, false),
    i("HasOwnProperty", R, 2, false),
    i("OwnPropertyAttributes", R, 2, false),
    i("OwnPropertyKeys", R, 2, false),
    i("GetPrototypeOf", R, 1, false),
    i("SetPrototypeOf", R, 2, false),
    i("PreventExtensions", R, 1, false),
    i("IsExtensible", R, 1, false),
    i("SetIntegrityLevel", R, 2, false),
    i("TestIntegrityLevel", R, 2, false),
    i("SameValue", L, 2, false),
    i("ObjectClassTag", L, 1, false),
    i("WrapPrimitive", L, 2, false),
    i("PrimitiveValue", L, 1, false),
    i("ConstructWithArray", R, 3, false),
    i("ConstructWithThis", R, 4, false),
    i("GetWithReceiver", R, 3, false),
    i("SetWithReceiver", R, 4, false),
    i("DeleteProperty", R, 2, false),
    i("IsProxy", L, 1, false),
    i("NewProxy", L, 2, false),
    i("ProxyTarget", L, 1, false),
    i("ProxyHandler", L, 1, false),
    i("ProxyRevoke", L, 1, false),
    i("CollectionCreate", L, 2, false),
    i("CollectionClass", L, 1, false),
    i("CollectionGet", L, 2, false),
    i("CollectionHas", L, 2, false),
    i("CollectionSet", L, 3, false),
    i("CollectionDelete", L, 2, false),
    i("CollectionClear", L, 1, false),
    i("CollectionSize", L, 1, false),
    i("CollectionIterator", L, 2, false),
    i("CollectionIteratorNext", L, 2, false),
    i("IsOriginalCollectionAdder", L, 2, false),
    i("IsArrayConstructor", L, 1, false),
    i("CollectionIteratorStep", L, 1, false),
    i("CollectionIteratorKey", L, 1, false),
    i("CollectionIteratorValue", L, 1, false),
    i("CanBeHeldWeakly", L, 1, false),
    i("DateNow", L, 0, false),
    i("DateCreate", L, 2, false),
    i("DateValue", L, 1, false),
    i("DateSetValue", L, 2, false),
    i("DateFromFields", L, 2, false),
    i("DateGetField", L, 3, false),
    i("DateSetFields", L, 4, false),
    i("DateFormat", L, 3, false),
    i("DateParse", L, 1, false),
    i("DateTimezoneOffset", L, 1, false),
    i("ArrayBufferCreate", L, 3, false),
    i("BufferInfo", L, 2, false),
    i("BufferDetach", L, 1, false),
    i("BufferResize", L, 2, false),
    i("BufferTransfer", L, 3, false),
    i("BufferCopy", L, 5, false),
    i("SortNumbers", L, 1, false),
    i("DataViewCreate", L, 4, false),
    i("DataViewField", L, 2, false),
    i("DataViewGet", L, 4, false),
    i("DataViewSet", L, 5, false),
    i("IsDataView", L, 1, false),
    i("RealmConstructor", L, 1, false),
    i("IsTypedArray", L, 1, false),
    i("TypedArrayCreate", L, 5, false),
    i("TypedArrayField", L, 2, false),
    i("TypedArrayGet", L, 2, false),
    i("TypedArraySet", L, 3, false),
    i("TypedArrayCopy", L, 5, false),
    i("TypedArrayKindProto", L, 1, false),
    i("Base64Encode", L, 3, false),
    i("HexEncode", L, 1, false),
    i("Base64Decode", L, 4, false),
    i("HexDecode", L, 2, false),
    i("CallWithArray", R, 3, false),
    i("BoundFunctionCreate", L, 3, false),
    i("BoundTargetFunction", L, 1, false),
    i("FunctionToString", L, 1, false),
    i("NumberPow", L, 2, false),
    i("SymbolDescriptiveString", L, 1, false),
    i("CompareStrings", L, 2, false),
    i("StringCodePoints", L, 1, false),
    i("SortPrimitives", L, 1, false),
    i("SortStart", L, 1, false),
    i("SortStep", L, 2, false),
    i("SortOperand", L, 2, false),
    i("ArraySpliceDense", L, 4, false),
    i("ArrayCanPopDense", L, 1, false),
    i("ArrayPopDense", L, 1, false),
    i("ArrayFillDense", L, 4, false),
    i("ArrayPushDense", L, 2, false),
    i("ArraySearchDense", L, 5, false),
    i("ArraySliceDense", L, 4, false),
    i("EachDense", R, 7, false),
    i("ArrayJoinDense", L, 3, false),
    i("EnumerableOwnDataProperties", L, 2, false),
    i("TypedArraySetFromDense", L, 2, false),
    i("StrLocaleCompare", L, 2, false),
    i("StrCharCodeAt", L, 2, false),
    i("StrCodePointAt", L, 2, false),
    i("StrSubstring", L, 3, false),
    i("StrIndexOf", L, 3, false),
    i("StrLastIndexOf", L, 3, false),
    i("StrToCase", L, 2, false),
    i("StrTrim", L, 2, false),
    i("StrIsWellFormed", L, 1, false),
    i("StrToWellFormed", L, 1, false),
    i("StrRepeat", L, 2, false),
    i("StrFromCodeUnits", L, 1, false),
    i("StrFromCodePoints", L, 1, false),
    i("StrNormalize", L, 2, false),
    i("NewSymbol", L, 1, false),
    i("SymbolDescription", L, 1, false),
    i("SymbolFor", L, 1, false),
    i("SymbolKeyFor", L, 1, false),
    i("NumberToString", L, 2, false),
    i("NumberToFixed", L, 2, false),
    i("NumberToExponential", L, 2, false),
    i("NumberToPrecision", L, 2, false),
    i("StringParseFloat", L, 1, false),
    i("StringParseInt", L, 2, false),
    i("UriEncode", L, 2, false),
    i("UriDecode", L, 2, false),
    i("StrEscape", L, 1, false),
    i("StrUnescape", L, 1, false),
    i("CompileEval", L, 1, false),
    i("CompileFunction", L, 3, false),
    i("GlobalObject", L, 0, false),
    i("EnqueueMicrotask", L, 1, false),
    i("NumberToInt32", L, 1, false),
    i("NumberToUint32", L, 1, false),
    i("F64Abs", L, 1, false),
    i("F64Acos", L, 1, false),
    i("F64Acosh", L, 1, false),
    i("F64Asin", L, 1, false),
    i("F64Asinh", L, 1, false),
    i("F64Atan", L, 1, false),
    i("F64Atanh", L, 1, false),
    i("F64Atan2", L, 2, false),
    i("F64Cbrt", L, 1, false),
    i("F64Hypot", L, 2, false),
    i("F64Ceil", L, 1, false),
    i("F64Clz32", L, 1, false),
    i("F64Cos", L, 1, false),
    i("F64Cosh", L, 1, false),
    i("F64Exp", L, 1, false),
    i("F64Expm1", L, 1, false),
    i("F64Floor", L, 1, false),
    i("F64Fround", L, 1, false),
    i("F64F16round", L, 1, false),
    i("F64SumPrecise", L, 1, false),
    i("F64Imul", L, 2, false),
    i("F64Log", L, 1, false),
    i("F64Log1p", L, 1, false),
    i("F64Log10", L, 1, false),
    i("F64Log2", L, 1, false),
    i("F64Round", L, 1, false),
    i("F64Sign", L, 1, false),
    i("F64Sin", L, 1, false),
    i("F64Sinh", L, 1, false),
    i("F64Sqrt", L, 1, false),
    i("F64Tan", L, 1, false),
    i("F64Tanh", L, 1, false),
    i("F64Trunc", L, 1, false),
    i("MathRandom", L, 0, false),
    i("CreateArrayIterator", L, 2, false),
    i("ArrayIteratorTarget", L, 1, false),
    i("ArrayIteratorIndex", L, 1, false),
    i("ArrayIteratorSetIndex", L, 2, false),
    i("ArrayIteratorFinish", L, 1, false),
    i("ArrayIteratorKind", L, 1, false),
    i("CreateStringIterator", L, 1, false),
    i("StringIteratorTarget", L, 1, false),
    i("StrCodePointString", L, 2, false),
    i("HasOriginalIteration", L, 1, false),
    i("IteratorFastReady", L, 2, false),
    i("IteratorFastValue", L, 2, false),
    i("IteratorFastDrain", L, 4, false),
    i("CreateIterResultObject", L, 2, false),
    i("CreateArrayFromPair", L, 2, false),
    i("PromiseCreate", L, 1, false),
    i("IsPromise", L, 1, false),
    i("IntrinsicPromise", L, 0, false),
    i("PromiseState", L, 1, false),
    i("PromiseResult", L, 1, false),
    i("FulfillPromise", L, 2, false),
    i("RejectPromise", L, 2, false),
    i("PromiseClaimResolve", L, 1, false),
    i("PerformPromiseThen", L, 4, false),
    i("PromiseCombinatorRun", L, 5, false),
    i("EnqueuePromiseResolveThenableJob", L, 3, false),
    i("NewRecord", L, 1, false),
    i("RecordGet", L, 2, false),
    i("RecordSet", L, 3, false),
    i("NewBuiltinClosure", L, 3, false),
    i("GeneratorState", L, 1, false),
    i("GeneratorSetState", L, 2, false),
    i("GeneratorResume", R, 3, false),
    i("IsAsyncFunctionState", L, 1, false),
    i("AsyncFunctionPromise", L, 1, false),
    i("AsyncGeneratorState", L, 1, false),
    i("AsyncGeneratorEnqueue", L, 2, false),
    i("AsyncGeneratorFirstRequest", L, 1, false),
    i("AsyncGeneratorDequeue", L, 1, false),
    i("CreateAsyncFromSyncIterator", L, 2, false),
    i("CreateIteratorHelper", L, 3, false),
    i("ThisIteratorHelper", L, 2, false),
    i("WeakRefCreate", L, 2, false),
    i("WeakRefDeref", L, 1, false),
    i("FinalizationRegistryCreate", L, 2, false),
    i("ThisFinalizationRegistry", L, 1, false),
    i("FinalizationRegistryRegister", L, 4, false),
    i("FinalizationRegistryUnregister", L, 2, false),
    i("FinalizationRegistryNextDead", L, 1, false),
    i("FinalizationRegistryTake", L, 2, false),
    i("FinalizationRegistrySetQueued", L, 2, false),
    i("DisposableStackCreate", L, 2, false),
    i("ThisDisposableStack", L, 2, false),
    i("TextCodecCreate", L, 2, false),
    i("TextCodecFlags", L, 2, false),
    i("IsBufferSource", L, 1, false),
    i("Utf8Encode", L, 4, false),
    i("Utf8Decode", L, 5, false),
    i("JsonParse", L, 2, false),
    i("JsonRawJSON", L, 1, false),
    i("IsRawJSON", L, 1, false),
    i("JsonStringifyFast", L, 2, false),
    i("JsonStringifyFastAt", L, 4, false),
    i("JsonQuote", L, 1, false),
    i("CreateDataProperty", R, 3, false),
    i("IsBigInt", L, 1, false),
    i("ToBigIntPrimitive", L, 1, false),
    i("NumberToBigInt", L, 1, false),
    i("BigIntToString", L, 2, false),
    i("BigIntAsIntN", L, 2, false),
    i("BigIntAsUintN", L, 2, false),
    i("BigIntToNumber", L, 1, false),
    i("IsRegExpObject", L, 1, false),
    i("RegExpCreate", L, 3, false),
    i("RegExpSource", L, 1, false),
    i("RegExpFlags", L, 1, false),
    i("RegExpHasFlag", L, 2, false),
    i("RegExpEscapedSource", L, 1, false),
    i("RegExpExecRaw", L, 4, false),
    i("RegExpTestRaw", L, 4, false),
    i("RegExpExecOwn", L, 4, false),
    i("IsOriginalRegExpExec", L, 1, false),
    i("CreateRegExpStringIterator", L, 4, false),
    i("RegExpStringIteratorSlot", L, 2, false),
    i("RegExpStringIteratorFinish", L, 1, false),
    i("RegExpEscapeString", L, 1, false),
    i("IsPristineRegExp", L, 1, false),
    i("RegExpSplitRaw", L, 4, false),
    i("RegExpReplaceRaw", L, 4, false),
    i("IsDenseArray", L, 1, false),
    i("AllNumbers", L, 1, false),
    i("AllCodePoints", L, 1, false),
    i("InstallMethod", L, 4, false),
    i("InstallConstructor", L, 5, false),
    i("InstallPrototype", L, 3, false),
    i("InstallPrototypeConstructor", L, 1, false),
    i("InstallAccessor", L, 4, false),
    i("InstallValue", L, 4, false),
    i("InstallIntrinsic", L, 2, false),
    i("MarkNative", L, 2, false),
    i("MarkFast", L, 1, false),
    i("NsOp", L, 4, false),
    i("ArraySpeciesCreate", B, 2, false),
    i("ArrayConstructor", B, 0, true),
    i("ArraySpecies", B, 0, false),
    i("FindViaPredicate", B, 6, false),
    i("EveryOrSome", B, 4, false),
    i("SortList", B, 2, false),
    i("CollectPresent", B, 3, false),
    i("ArraySetLength", B, 3, false),
    i("AsyncGeneratorStep", B, 3, false),
    i("AsyncGeneratorAwaitReturnFulfilled", B, 1, false),
    i("AsyncGeneratorAwaitReturnRejected", B, 1, false),
    i("GetAsyncIterator", B, 1, false),
    i("AsyncIteratorReturnMethod", B, 1, false),
    i("AsyncIteratorCloseThrow", B, 1, false),
    i("AsyncFromSyncIteratorUnwrap", B, 1, false),
    i("AsyncFromSyncIteratorCloseThrow", B, 1, false),
    i("ToBigInt", B, 1, false),
    i("ToPrimitive", B, 2, false),
    i("ToNumber", B, 1, false),
    i("ToString", B, 1, false),
    i("LengthOfArrayLike", B, 1, false),
    i("ToPropertyKey", B, 1, false),
    i("GetPrototypeFromConstructorSlow", B, 2, false),
    i("ToNumeric", B, 1, false),
    i("DatePrototypeValueOf", B, 0, false),
    i("DatePrototypeToISOString", B, 0, false),
    i("DatePrototypeToJSON", B, 1, false),
    i("DatePrototypeToPrimitive", B, 1, false),
    i("DecoratorStart", B, 2, false),
    i("ApplyDecorators", B, 6, false),
    i("DecoratorRunInits", B, 3, false),
    i("DecoratorRunExtras", B, 3, false),
    i("DecoratorAddInitializer", B, 1, false),
    i("DecoratorAccessGet", B, 1, false),
    i("DecoratorAccessSet", B, 2, false),
    i("DecoratorAccessHas", B, 1, false),
    i("ApplyLegacyDecorators", B, 4, false),
    i("DecoratorParam", B, 2, false),
    i("DecoratorParamCall", B, 2, false),
    i("DecoratorMetadata", B, 2, false),
    i("AsyncDisposeFulfilled", B, 1, false),
    i("AsyncDisposeRejected", B, 1, false),
    i("UsingCreate", B, 0, false),
    i("UsingAdd", B, 2, false),
    i("AwaitUsingAdd", B, 2, false),
    i("UsingDisposeError", B, 2, false),
    i("UsingDispose", B, 1, false),
    i("UsingDisposeStep", B, 1, false),
    i("UsingAwaitValue", B, 1, false),
    i("CallPrepareStackTrace", B, 4, false),
    i("ThrowWithStack", B, 1, false),
    i("MicrotaskJob", B, 1, false),
    i("FromAsyncFulfilled", B, 1, false),
    i("FromAsyncRejected", B, 1, false),
    i("InstanceofOperator", B, 2, false),
    i("ClassHeritage", B, 1, false),
    i("ThrowTypeError", B, 1, false),
    i("FunctionPrototypeBehavior", B, 0, false),
    i("AsyncAwait", B, 2, false),
    i("AsyncFunctionReturn", B, 2, false),
    i("GeneratorFunctionConstructor", B, 0, true),
    i("AsyncGeneratorFunctionConstructor", B, 0, true),
    i("AsyncFunctionConstructor", B, 0, true),
    i("DynamicImport", B, 2, false),
    i("GetIterator", B, 1, false),
    i("IteratorClose", B, 1, false),
    i("IteratorCloseThrow", B, 1, false),
    i("IteratorRest", B, 2, false),
    i("ArrayPrototypeValues", B, 0, false),
    i("ArrayIteratorPrototypeNext", B, 0, false),
    i("StringPrototypeIterator", B, 0, false),
    i("StringIteratorPrototypeNext", B, 0, false),
    i("SerializeJSONProperty", B, 7, false),
    i("JSONStackIncludes", B, 2, false),
    i("JSONJoin", B, 7, false),
    i("SerializeJSONObject", B, 6, false),
    i("SerializeJSONArray", B, 6, false),
    i("AddEntriesFromIterable", B, 3, false),
    i("MapPrototypeSet", B, 2, false),
    i("MapIteratorPrototypeNext", B, 0, false),
    i("ModuleOp", B, 3, false),
    i("ResolvePromise", B, 2, false),
    i("PromiseResolveFunction", B, 1, false),
    i("PromiseRejectFunction", B, 1, false),
    i("GetCapabilitiesExecutor", B, 2, false),
    i("PromiseReactionJob", B, 4, false),
    i("PromiseResolveThenableJob", B, 3, false),
    i("PromiseConstructor", B, 1, false),
    i("PromiseSpecies", B, 0, false),
    i("PromiseStaticResolve", B, 1, false),
    i("PromiseAllResolveElement", B, 1, false),
    i("PromiseAllSettledResolveElement", B, 1, false),
    i("PromiseAllSettledRejectElement", B, 1, false),
    i("PromiseAnyRejectElement", B, 1, false),
    i("PromisePrototypeThen", B, 2, false),
    i("PromiseThenFinally", B, 1, false),
    i("PromiseCatchFinally", B, 1, false),
    i("PromiseValueThunk", B, 0, false),
    i("PromiseThrower", B, 0, false),
    i("HostCallSettle", B, 3, false),
    i("ProxyRevokeFunction", B, 0, false),
    i("ProxyGetPrototypeOf", B, 1, false),
    i("ProxySetPrototypeOf", B, 2, false),
    i("ProxyIsExtensible", B, 1, false),
    i("ProxyPreventExtensions", B, 1, false),
    i("ProxyGetOwnPropertyDescriptorObject", B, 2, false),
    i("ProxyHasOwnProperty", B, 2, false),
    i("ProxyOwnPropertyAttributes", B, 2, false),
    i("ProxyDefineOwnPropertyFromRecord", B, 3, false),
    i("ProxyDefineDataPropertyOrThrow", B, 4, false),
    i("ProxyCreateDataProperty", B, 3, false),
    i("ProxyCreateDataPropertyOrThrow", B, 3, false),
    i("ProxyHas", B, 2, false),
    i("ProxyGet", B, 3, false),
    i("ProxySet", B, 4, false),
    i("ProxySetOrThrow", B, 4, false),
    i("ProxySetOnReceiver", B, 3, false),
    i("ProxyDeleteProperty", B, 2, false),
    i("ProxyDeletePropertyOrThrow", B, 2, false),
    i("ProxyOwnPropertyKeys", B, 2, false),
    i("ProxyCall", B, 3, false),
    i("ProxyConstruct", B, 3, false),
    i("OrdinaryConstructSlow", B, 3, false),
    i("ProxySetIntegrityLevel", B, 2, false),
    i("ProxyTestIntegrityLevel", B, 2, false),
    i("ForInStep", B, 1, false),
    i("RegExpSpecies", B, 0, false),
    i("RegExpPrototypeHasIndices", B, 0, false),
    i("RegExpPrototypeGlobal", B, 0, false),
    i("RegExpPrototypeIgnoreCase", B, 0, false),
    i("RegExpPrototypeMultiline", B, 0, false),
    i("RegExpPrototypeDotAll", B, 0, false),
    i("RegExpPrototypeUnicode", B, 0, false),
    i("RegExpPrototypeUnicodeSets", B, 0, false),
    i("RegExpPrototypeSticky", B, 0, false),
    i("RegExpPrototypeFlags", B, 0, false),
    i("RegExpPrototypeExec", B, 1, false),
    i("RegExpPrototypeMatch", B, 1, false),
    i("SetPrototypeAdd", B, 1, false),
    i("SetIteratorPrototypeNext", B, 0, false),
    i("ArraySpread", B, 2, false),
    i("CopyDataProperties", B, 2, false),
    i("CopyDataPropertiesExcluding", B, 2, false),
    i("TypedArraySetElementSlow", B, 3, false),
    i("FinalizationRegistryCleanupJob", B, 1, false),
];

/// The id of the intrinsic `name`, at compile time in a `const` (a typo fails the
/// build).
pub const fn const_id(name: &str) -> u16 {
    let mut k = 0;
    while k < INTRINSICS.len() {
        let a = INTRINSICS[k].name.as_bytes();
        let b = name.as_bytes();
        if a.len() == b.len() {
            let mut j = 0;
            while j < a.len() && a[j] == b[j] {
                j += 1;
            }
            if j == a.len() {
                return k as u16;
            }
        }
        k += 1;
    }
    panic!("unknown intrinsic")
}

// The run-time tables (0225, D3091). [`INTRINSICS`] is a `const`: indexing it at run time
// would put the whole table in the module, 12 bytes per entry of which 8 are the name's
// pointer and length, and those pointer tables compress badly. The run time reads a byte
// per intrinsic for its signature and the names from one string with end offsets.

const COUNT: usize = INTRINSICS.len();

/// Per intrinsic: params in bits 0-4, variadic in bit 5, the kind in bits 6-7.
static SIGNATURES: [u8; COUNT] = {
    let mut t = [0u8; COUNT];
    let mut k = 0;
    while k < COUNT {
        let e = &INTRINSICS[k];
        assert!(e.params < 32);
        t[k] = e.params | (e.variadic as u8) << 5 | (e.kind as u8) << 6;
        k += 1;
    }
    t
};

const NAMES_LEN: usize = {
    let mut n = 0;
    let mut k = 0;
    while k < COUNT {
        n += INTRINSICS[k].name.len();
        k += 1;
    }
    n
};
const _: () = assert!(NAMES_LEN <= u16::MAX as usize);

/// All names, back to back.
static NAMES: [u8; NAMES_LEN] = {
    let mut t = [0u8; NAMES_LEN];
    let (mut k, mut at) = (0, 0);
    while k < COUNT {
        let n = INTRINSICS[k].name.as_bytes();
        let mut j = 0;
        while j < n.len() {
            t[at] = n[j];
            at += 1;
            j += 1;
        }
        k += 1;
    }
    t
};

/// Where each name ends in [`NAMES`].
static NAME_ENDS: [u16; COUNT] = {
    let mut t = [0u16; COUNT];
    let (mut k, mut at) = (0, 0);
    while k < COUNT {
        at += INTRINSICS[k].name.len();
        t[k] = at as u16;
        k += 1;
    }
    t
};

/// An intrinsic's signature, as the run time and the compiler read it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Signature {
    pub kind: IntrinsicKind,
    /// Declared parameters (without a rest parameter, without `this`).
    pub params: u8,
    /// Takes `...args` after the declared parameters.
    pub variadic: bool,
}

/// The signature of the intrinsic with id `id`.
#[inline]
pub fn intrinsic(id: u16) -> Option<Signature> {
    let b = *SIGNATURES.get(id as usize)?;
    let kind = match b >> 6 {
        0 => IntrinsicKind::Leaf,
        1 => IntrinsicKind::Reentrant,
        _ => IntrinsicKind::Builtin,
    };
    Some(Signature {
        kind,
        params: b & 31,
        variadic: b & 32 != 0,
    })
}

/// The declared parameters of intrinsic `id` (0 for an unknown id).
#[inline]
pub fn params(id: u32) -> u32 {
    SIGNATURES.get(id as usize).map_or(0, |b| (b & 31) as u32)
}

/// The name of the intrinsic with id `id` ("?" for an unknown id), for messages.
pub fn intrinsic_name(id: u16) -> &'static str {
    let i = id as usize;
    let end = NAME_ENDS.get(i).map_or(0, |&e| e as usize);
    let start = match i {
        0 => 0,
        _ => NAME_ENDS.get(i - 1).map_or(end, |&e| e as usize),
    };
    NAMES
        .get(start..end)
        .and_then(|b| core::str::from_utf8(b).ok())
        .filter(|n| !n.is_empty())
        .unwrap_or("?")
}

/// All names, by id (from the run-time tables).
pub fn names() -> impl Iterator<Item = &'static str> {
    (0..COUNT as u16).map(intrinsic_name)
}

/// The id of the intrinsic named by a string literal, computed at compile time (an
/// unknown name fails the build): for the compiler, which emits calls to intrinsics by
/// name, without a search of the names per emitted call (0225).
macro_rules! intrinsic_id_of {
    ($name:literal) => {
        const { $crate::bytecode::intrinsics::const_id($name) }
    };
}
pub(crate) use intrinsic_id_of;

/// The id of the intrinsic `name`.
pub fn intrinsic_id(name: &str) -> Option<u16> {
    // Compares the bytes of each name in place: going through `intrinsic_name` (a UTF-8
    // check per name) made compiling the builtins' and the prelude's source 1.6x slower
    // (0224; the compiler looks every intrinsic reference up here).
    let name = name.as_bytes();
    let mut start = 0;
    for (i, &end) in NAME_ENDS.iter().enumerate() {
        let end = end as usize;
        if end - start == name.len() && NAMES[start..end] == *name {
            return Some(i as u16);
        }
        start = end;
    }
    None
}

/// FNV-1a over the table (names, kinds, arities, order).
pub const INTRINSICS_HASH: u32 = hash_intrinsics(INTRINSICS);

/// FNV-1a over an intrinsic table: names, kinds, parameter counts and order.
pub const fn hash_intrinsics(table: &[IntrinsicInfo]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    let mut k = 0;
    while k < table.len() {
        let e = &table[k];
        let n = e.name.as_bytes();
        let mut j = 0;
        while j < n.len() {
            h = (h ^ n[j] as u32).wrapping_mul(0x0100_0193);
            j += 1;
        }
        let tail = [0u8, e.kind as u8, e.params, e.variadic as u8];
        let mut j = 0;
        while j < tail.len() {
            h = (h ^ tail[j] as u32).wrapping_mul(0x0100_0193);
            j += 1;
        }
        k += 1;
    }
    h
}

const _: () = assert!(INTRINSICS.len() <= u16::MAX as usize);
const _: () = assert!(
    IntrinsicKind::Leaf as u8 == 0
        && IntrinsicKind::Reentrant as u8 == 1
        && IntrinsicKind::Builtin as u8 == 2
);

#[cfg(test)]
mod tests {
    extern crate std;

    use alloc::format;
    use alloc::string::String;
    use alloc::vec::Vec;
    use std::path::PathBuf;

    use super::*;

    fn builtins_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../builtins")
    }

    /// `(name, params, variadic)` of a parameter list text (without parentheses).
    fn arity(params: &str) -> (u8, bool) {
        let mut n = 0;
        let mut variadic = false;
        for p in params.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            if p.starts_with("...") {
                variadic = true;
            } else if !p.starts_with("this:") && !p.starts_with("this :") {
                n += 1;
            }
        }
        (n, variadic)
    }

    /// The JSDoc comment right before `at` (only whitespace in between), if any.
    fn doc_before(src: &str, at: usize) -> &str {
        let before = src[..at].trim_end();
        if !before.ends_with("*/") {
            return "";
        }
        match before.rfind("/**") {
            Some(s) => &before[s..],
            None => "",
        }
    }

    /// The entries the sources describe.
    fn from_sources() -> Vec<IntrinsicInfo> {
        let mut out = Vec::new();
        let dts = std::fs::read_to_string(builtins_dir().join("intrinsics.d.ts")).unwrap();
        for (at, _) in dts.match_indices("declare function ") {
            let rest = &dts[at + "declare function ".len()..];
            let name = &rest[..rest.find('(').unwrap()];
            let params = &rest[name.len() + 1..rest.find(')').unwrap()];
            let doc = doc_before(&dts, at);
            let kind = if doc.contains("@reentrant") {
                IntrinsicKind::Reentrant
            } else {
                assert!(doc.contains("@leaf"), "{name}: no @leaf/@reentrant tag");
                IntrinsicKind::Leaf
            };
            let (n, variadic) = arity(params);
            out.push(IntrinsicInfo {
                name: std::boxed::Box::leak(String::from(name).into_boxed_str()),
                kind,
                params: n,
                variadic,
            });
        }
        let mut files: Vec<PathBuf> = std::fs::read_dir(builtins_dir())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.extension().is_some_and(|e| e == "ts") && !p.to_string_lossy().ends_with(".d.ts")
            })
            .collect();
        files.sort();
        for f in files {
            let src = std::fs::read_to_string(&f).unwrap();
            for (at, _) in src.match_indices("\nfunction ") {
                let at = at + 1;
                if !doc_before(&src, at).contains("@intrinsic") {
                    continue;
                }
                let rest = &src[at + "function ".len()..];
                let name = &rest[..rest.find('(').unwrap()];
                let params = &rest[name.len() + 1..rest.find(')').unwrap()];
                let (n, variadic) = arity(params);
                out.push(IntrinsicInfo {
                    name: std::boxed::Box::leak(String::from(name).into_boxed_str()),
                    kind: IntrinsicKind::Builtin,
                    params: n,
                    variadic,
                });
            }
        }
        out
    }

    #[test]
    fn table_matches_sources() {
        let expected = from_sources();
        if expected.as_slice() != INTRINSICS {
            let mut rust = String::new();
            for e in &expected {
                let k = match e.kind {
                    IntrinsicKind::Leaf => "L",
                    IntrinsicKind::Reentrant => "R",
                    IntrinsicKind::Builtin => "B",
                };
                rust.push_str(&format!(
                    "    i({:?}, {k}, {}, {}),\n",
                    e.name, e.params, e.variadic
                ));
            }
            panic!("INTRINSICS is out of date with builtins/; replace its entries with:\n{rust}");
        }
    }

    #[test]
    fn lookup() {
        assert_eq!(intrinsic_id("IsObject"), Some(0));
        let get = intrinsic_id("Get").unwrap();
        assert_eq!(intrinsic(get).unwrap().kind, IntrinsicKind::Reentrant);
        assert_eq!(intrinsic_id("Nope"), None);
        // The run-time tables agree with the table.
        for (k, e) in INTRINSICS.iter().enumerate() {
            let sig = intrinsic(k as u16).unwrap();
            assert_eq!(
                (sig.kind, sig.params, sig.variadic),
                (e.kind, e.params, e.variadic)
            );
            assert_eq!(intrinsic_name(k as u16), e.name);
            assert_eq!(params(k as u32), e.params as u32);
        }
        assert_eq!(intrinsic(COUNT as u16), None);
        assert_eq!(intrinsic_name(COUNT as u16), "?");
        assert_eq!(
            intrinsic(intrinsic_id("ToPrimitive").unwrap())
                .unwrap()
                .kind,
            IntrinsicKind::Builtin
        );
        assert_ne!(INTRINSICS_HASH, 0);
    }
}
