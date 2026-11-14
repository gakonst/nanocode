//! Leaf fast paths of everyday builtins (D1850, 0085).
//!
//! A builtin tagged `@fast` in `builtins/*.ts` has an entry here under its function
//! name; the prelude marks it with `%MarkFast(f)`. With native builtins on, a call of it
//! first runs the entry with `this` and the arguments. The entry either returns the
//! builtin's result, or declines (`Ok(None)`) and the builtin runs as usual.
//!
//! The rule that keeps this unobservable (the D1351 rule): an entry handles only
//! arguments for which the builtin's algorithm cannot run user code or look anything up
//! that user code could have changed. In practice: string, number, boolean, undefined
//! and null arguments (ToString, ToNumber, ToIntegerOrInfinity and ToPropertyKey of those
//! are pure), and objects only where the algorithm reads them with a leaf operation (an
//! own-property check of a non-proxy). Anything else, and every case where the builtin
//! would throw, declines, so errors and their stack traces come from the builtin itself.
//! Each entry mirrors its TS source step by step; keep them in sync.
use crate::PushOut;

use alloc::vec::Vec;

use crate::builtins::{collection, dense, number, regexp, string};
use crate::bytecode::intrinsics::INTRINSICS;
use crate::bytecode::template::func_flags;
use crate::heap::cells::FuncTemplate;
use crate::heap::{Heap, Lookup, ObjectClass, Property, StrView, atoms};
use crate::interp::conv::{self, Fault};
use crate::interp::run::promise;
use crate::interp::{Intrinsic, realm};
use crate::value::{Kind, Obj, Str, Value};

/// The result of an entry: `None` declines.
pub(crate) type Fast = Result<Option<Value>, Fault>;

pub(crate) struct FastFn {
    /// The builtin's function name in the prelude (checked by `%MarkFast`).
    #[cfg(djs_prelude)]
    pub name: &'static str,
    /// `run(heap, this, args)`.
    pub run: fn(&mut Heap, Value, &[Value]) -> Fast,
    /// An entry that runs the regexp matcher, in place of `run`: `metered(heap, this,
    /// args, fuel)` charges the matcher's steps to the run's fuel when it ends. It
    /// declines a match that needs more steps than an entry takes
    /// (`regexp::FAST_STEPS`): the builtin starts over and suspends.
    pub metered: Option<Metered>,
    /// A constructor's `new F(args)` with new.target F, `new(heap, args)` (0236); `None`:
    /// constructs run the builtin.
    pub new: Option<fn(&mut Heap, &[Value]) -> Fast>,
}

pub(crate) type Metered = fn(&mut Heap, Value, &[Value], &mut i64) -> Fast;

/// The most arguments an entry looks at (calls with more decline).
pub(crate) const MAX_ARGS: usize = 6;

impl FastFn {
    pub(crate) fn call(
        &self,
        heap: &mut Heap,
        this: Value,
        args: &[Value],
        fuel: &mut i64,
    ) -> Fast {
        match self.metered {
            Some(run) => run(heap, this, args, fuel),
            None => (self.run)(heap, this, args),
        }
    }
}

#[cfg(djs_prelude)]
/// The index of the entry for builtin `name`.
pub(crate) fn index(name: &str) -> Option<usize> {
    FAST.iter().position(|f| f.name == name)
}

const fn f(name: &'static str, run: fn(&mut Heap, Value, &[Value]) -> Fast) -> FastFn {
    #[cfg(not(djs_prelude))]
    let _ = name;
    FastFn {
        #[cfg(djs_prelude)]
        name,
        run,
        metered: None,
        new: None,
    }
}

/// An entry that runs the regexp matcher (`FastFn::metered`).
const fn m(name: &'static str, run: Metered) -> FastFn {
    FastFn {
        metered: Some(run),
        ..f(name, no_call)
    }
}

/// A constructor's entry: `run` for calls, `new` for constructs.
const fn ctor(
    name: &'static str,
    run: fn(&mut Heap, Value, &[Value]) -> Fast,
    new: fn(&mut Heap, &[Value]) -> Fast,
) -> FastFn {
    FastFn {
        new: Some(new),
        ..f(name, run)
    }
}

/// The index of `ArrayPrototypePush`'s entry: the interpreter appends for it directly
/// (interp/run/fastpath.rs, 0231; a test checks the index).
pub(crate) const ARRAY_PUSH: usize = 21;
pub(crate) const ARRAY_POP: usize = ARRAY_PUSH + 1;

/// The entries that pay their bytes (D2310: the rule and the measurements of every twin).
pub(crate) static FAST: &[FastFn] = &[
    // ---- conversion helpers of the builtin language (calls from bytecode builtins) ----
    f("ToString", to_string),
    f("LengthOfArrayLike", length_of_array_like),
    f("ToPropertyKey", to_property_key),
    // ---- String ----
    f("StringPrototypeIncludes", includes),
    f("StringPrototypeIndexOf", index_of),
    f("StringPrototypeSlice", slice),
    f("StringPrototypeCharAt", char_at),
    f("StringPrototypeCharCodeAt", char_code_at),
    m("StringPrototypeSplit", split),
    // 0230b: >2x QuickJS after the general fixes (D3120, D3121)
    f("StringPrototypeSubstring", substring),
    f("StringPrototypeAt", at),
    f("StringPrototypeRepeat", repeat),
    f("StringPrototypePadStart", pad_start),
    f("StringPrototypePadEnd", pad_end),
    // ---- Number ----
    f("NumberConstructor", number_constructor),
    f("NumberIsInteger", number_is_integer),
    f("NumberPrototypeToFixed", to_fixed),
    f("StringConstructor", string_constructor),
    // ---- Math (0230: `Math.max(m, x)` in reduce callbacks) ----
    f("MathMax", math_max),
    f("MathMin", math_min),
    // ---- Array ----
    f("ArraySpeciesCreate", array_species_create),
    // ---- Array element loops over dense arrays (0091, builtins/dense.rs) ----
    // (Its index is ARRAY_PUSH: entries added above it move it.)
    f("ArrayPrototypePush", array_push),
    f("ArrayPrototypePop", array_pop),
    f("ArrayPrototypeIndexOf", array_index_of),
    f("ArrayPrototypeIncludes", array_includes),
    f("ArrayPrototypeSlice", array_slice),
    f("ArrayPrototypeJoin", array_join),
    // ---- Map and Set (0231: the builtin's frames cost more than the lookup) ----
    f("MapPrototypeGet", map_get),
    f("MapPrototypeHas", map_has),
    f("MapPrototypeSet", map_set),
    f("SetPrototypeHas", set_has),
    f("SetPrototypeAdd", set_add),
    // ---- Promises (0232b, D3156: `new Promise` and `then` were 4-6x QuickJS) ----
    // (GetPrototypeFromConstructor is the hot loop's leaf %ConstructorPrototype, 0236.)
    f("PromiseResolveFunction", |h, this, a| {
        promise::fast_resolve(h, this, arg(a, 0))
    }),
    f("PromiseRejectFunction", |h, this, a| {
        promise::fast_reject(h, this, arg(a, 0))
    }),
    f("ResolveWithData", |h, _, a| {
        promise::fast_resolve(h, arg(a, 0), arg(a, 1))
    }),
    f("RejectWithData", |h, _, a| {
        promise::fast_reject(h, arg(a, 0), arg(a, 1))
    }),
    f("PromisePrototypeThen", promise::fast_then),
    f("PromiseStaticResolve", promise::fast_static_resolve),
    // ---- String.fromCharCode (0091; the URI functions run their builtins, D2256) ----
    f("StringFromCharCode", from_char_code),
    // ---- constructors (0236: `new Map()` was 2x QuickJS after the general fixes) ----
    ctor("ArrayConstructor", array_call, array_new),
    ctor("MapConstructor", no_call, map_new),
    ctor("SetConstructor", no_call, set_new),
    ctor("DateConstructor", no_call, date_new),
    // ---- the rest of the everyday String methods and two Array ones (nanocode: as
    // bytecode each was 3-10x QuickJS; the entries are at or below it) ----
    f("StringPrototypeStartsWith", starts_with),
    f("StringPrototypeEndsWith", ends_with),
    f("StringPrototypeLastIndexOf", last_index_of),
    f("StringPrototypeCodePointAt", code_point_at),
    f("StringPrototypeLocaleCompare", locale_compare),
    f("StringPrototypeConcat", concat),
    f("StringPrototypeToLowerCase", to_lower_case),
    f("StringPrototypeToUpperCase", to_upper_case),
    f("StringPrototypeToLocaleLowerCase", to_lower_case),
    f("StringPrototypeToLocaleUpperCase", to_upper_case),
    f("StringPrototypeTrim", trim),
    f("StringPrototypeTrimStart", trim_start),
    f("StringPrototypeTrimEnd", trim_end),
    f("StringPrototypeSubstr", substr),
    m("StringPrototypeReplace", replace),
    m("StringPrototypeReplaceAll", replace_all),
    f("ArrayPrototypeReverse", array_reverse),
    f("ArrayPrototypeConcat", array_concat),
    f("ArrayPrototypeFlat", array_flat),
    // ---- RegExps whose protocol is unobservable (D1503, D3137), matched in one go ----
    m("RegExpPrototypeTest", regexp_test),
    m("RegExpPrototypeExec", regexp_exec),
    m("RegExpStringIteratorNext", regexp_string_iterator_next),
    m("StringPrototypeMatch", string_match),
    m("StringPrototypeSearch", string_search),
    f("StringPrototypeMatchAll", string_match_all),
    // ---- Math and the number functions of a primitive (as bytecode a call cost a
    // frame for one libm call: 3.5x QuickJS) ----
    f("MathAbs", |h, _, a| math(h, a, djs_libm::fabs)),
    f("MathCeil", |h, _, a| math(h, a, djs_libm::ceil)),
    f("MathFloor", |h, _, a| math(h, a, djs_libm::floor)),
    f("MathRound", |h, _, a| math(h, a, number::round)),
    f("MathTrunc", |h, _, a| math(h, a, djs_libm::trunc)),
    f("MathSign", |h, _, a| math(h, a, number::sign)),
    f("MathSqrt", |h, _, a| math(h, a, djs_libm::sqrt)),
    f("MathCbrt", |h, _, a| math(h, a, djs_libm::cbrt)),
    f("MathExp", |h, _, a| math(h, a, djs_libm::exp)),
    f("MathLog", |h, _, a| math(h, a, djs_libm::log)),
    f("MathLog2", |h, _, a| math(h, a, djs_libm::log2)),
    f("MathLog10", |h, _, a| math(h, a, djs_libm::log10)),
    f("MathSin", |h, _, a| math(h, a, djs_libm::sin)),
    f("MathCos", |h, _, a| math(h, a, djs_libm::cos)),
    f("MathPow", |h, _, a| math2(h, a, conv::pow)),
    f("MathAtan2", |h, _, a| math2(h, a, djs_libm::atan2)),
    f("GlobalIsNaN", |h, _, a| {
        Ok(num(h, arg(a, 0))?.map(|n| Value::bool(n.is_nan())))
    }),
    f("GlobalIsFinite", |h, _, a| {
        Ok(num(h, arg(a, 0))?.map(|n| Value::bool(n.is_finite())))
    }),
    f("NumberIsNaN", |_, _, a| {
        Ok(Some(Value::bool(
            arg(a, 0).as_number().is_some_and(f64::is_nan),
        )))
    }),
    f("NumberIsFinite", |_, _, a| {
        Ok(Some(Value::bool(
            arg(a, 0).as_number().is_some_and(f64::is_finite),
        )))
    }),
    f("GlobalParseInt", parse_int),
    f("GlobalParseFloat", parse_float),
    f("NumberPrototypeToString", number_to_string),
    // ---- Array (the rest of what scripts call per element or per line) ----
    f("ArrayIsArray", array_is_array),
    f("ArrayPrototypeShift", array_shift),
    f("ArrayPrototypeUnshift", array_unshift),
    f("ArrayPrototypeSplice", array_splice),
    // ---- Date: the clock and a timestamp's text ----
    f("DateNowBuiltin", |h, _, _| {
        Ok(Some(realm::intrinsic(h, Intrinsic::Clock)))
    }),
    f("DatePrototypeGetTime", |h, this, _| {
        Ok(this_date(h, this).map(conv::number))
    }),
    f("DatePrototypeToISOString", date_to_iso_string),
    // ---- btoa and atob of a string ----
    f("GlobalBtoa", btoa),
    f("GlobalAtob", atob),
    f("ObjectAssign", object_assign),
];

// ---- argument helpers ----------------------------------------------------------------

fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).copied().unwrap_or(Value::UNDEFINED)
}

/// ToNumber of a primitive that cannot throw (not a symbol or bigint); `None` otherwise.
fn num(heap: &mut Heap, v: Value) -> Result<Option<f64>, Fault> {
    if let Some(n) = v.as_number() {
        return Ok(Some(n));
    }
    match v.kind() {
        Kind::Undefined | Kind::Null | Kind::Bool | Kind::String => {
            Ok(Some(conv::to_number(heap, v)?))
        }
        _ => Ok(None),
    }
}

/// ToString of a primitive that cannot throw (not a symbol); `None` otherwise.
fn str(heap: &mut Heap, v: Value) -> Result<Option<Str>, Fault> {
    if let Some(s) = v.as_string() {
        return Ok(Some(s));
    }
    match v.kind() {
        Kind::Undefined | Kind::Null | Kind::Bool | Kind::Number | Kind::BigInt => {
            Ok(Some(conv::to_string(heap, v)?))
        }
        _ => Ok(None),
    }
}

/// ThisToString / RequireObjectCoercible + ToString of a primitive `this`.
fn this_str(heap: &mut Heap, v: Value) -> Result<Option<Str>, Fault> {
    if v.is_undefined() || v.is_null() {
        return Ok(None);
    }
    str(heap, v)
}

/// ToIntegerOrInfinity of a Number.
fn integer(n: f64) -> f64 {
    if n.is_nan() {
        0.0
    } else {
        djs_libm::trunc(n) + 0.0
    }
}

/// `ToIntegerOrInfinity(v)` for a primitive `v`.
fn int_arg(heap: &mut Heap, v: Value) -> Result<Option<f64>, Fault> {
    if let Some(i) = v.as_int() {
        return Ok(Some(f64::from(i)));
    }
    Ok(num(heap, v)?.map(integer))
}

/// ClampPosition(v, len): ToIntegerOrInfinity, clamped to [0, len].
fn clamp(p: f64, len: f64) -> f64 {
    p.max(0.0).min(len)
}

/// RelativeIndex(v, len, dflt) of array.ts.
fn relative(heap: &mut Heap, v: Value, len: f64, dflt: f64) -> Result<Option<f64>, Fault> {
    if v.is_undefined() {
        return Ok(Some(dflt));
    }
    let Some(rel) = int_arg(heap, v)? else {
        return Ok(None);
    };
    Ok(Some(if rel < 0.0 {
        (len + rel).max(0.0)
    } else {
        rel.min(len)
    }))
}

fn len(heap: &Heap, s: Str) -> f64 {
    heap.str_len(s) as f64
}

fn some_str(s: Str) -> Fast {
    Ok(Some(Value::string(s)))
}

fn empty(heap: &mut Heap) -> Fast {
    some_str(heap.atom_string(atoms::EMPTY)?)
}

// ---- conversion helpers --------------------------------------------------------------

fn to_string(heap: &mut Heap, _: Value, args: &[Value]) -> Fast {
    Ok(str(heap, arg(args, 0))?.map(Value::string))
}

/// LengthOfArrayLike of an Array: its own `length`, which nothing can observe (0137:
/// the bytecode builtins call it once per call).
fn length_of_array_like(heap: &mut Heap, _: Value, args: &[Value]) -> Fast {
    Ok(arg(args, 0)
        .as_object()
        .filter(|&o| heap.object_class(o) == ObjectClass::Array)
        .map(|o| conv::number(f64::from(heap.array_length(o)))))
}

fn to_property_key(heap: &mut Heap, _: Value, args: &[Value]) -> Fast {
    let v = arg(args, 0);
    if v.as_symbol().is_some() || v.is_number() {
        return Ok(Some(v));
    }
    Ok(str(heap, v)?.map(Value::string))
}

// ---- String --------------------------------------------------------------------------

/// The receiver and the search string of includes/indexOf/startsWith/...
fn search(heap: &mut Heap, this: Value, search: Value) -> Result<Option<(Str, Str)>, Fault> {
    let Some(s) = this_str(heap, this)? else {
        return Ok(None);
    };
    // A primitive is never a RegExp (IsRegExp looks up @@match only on objects).
    Ok(str(heap, search)?.map(|t| (s, t)))
}

fn includes(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    let Some((s, t)) = search(heap, this, arg(args, 0))? else {
        return Ok(None);
    };
    let Some(p) = int_arg(heap, arg(args, 1))? else {
        return Ok(None);
    };
    let start = clamp(p, len(heap, s)) as u32;
    Ok(Some(Value::bool(
        string::index_of(heap, s, t, start, false)? >= 0.0,
    )))
}

fn index_of(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    let Some((s, t)) = search(heap, this, arg(args, 0))? else {
        return Ok(None);
    };
    let Some(p) = int_arg(heap, arg(args, 1))? else {
        return Ok(None);
    };
    let start = clamp(p, len(heap, s)) as u32;
    Ok(Some(conv::number(string::index_of(
        heap, s, t, start, false,
    )?)))
}

fn slice(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    let Some(s) = this_str(heap, this)? else {
        return Ok(None);
    };
    let n = len(heap, s);
    let (Some(from), Some(to)) = (
        relative(heap, arg(args, 0), n, 0.0)?,
        relative(heap, arg(args, 1), n, n)?,
    ) else {
        return Ok(None);
    };
    if from >= to {
        return empty(heap);
    }
    some_str(string::substring(heap, s, from as u32, to as u32)?)
}

/// String.prototype.substring: ClampPosition of both ends, then the smaller first.
fn substring(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    let Some(s) = this_str(heap, this)? else {
        return Ok(None);
    };
    let n = len(heap, s);
    let Some(a) = int_arg(heap, arg(args, 0))? else {
        return Ok(None);
    };
    let b = match arg(args, 1) {
        v if v.is_undefined() => n,
        v => match int_arg(heap, v)? {
            Some(b) => clamp(b, n),
            None => return Ok(None),
        },
    };
    let a = clamp(a, n);
    let (from, to) = if a < b { (a, b) } else { (b, a) };
    some_str(string::substring(heap, s, from as u32, to as u32)?)
}

/// String.prototype.at: a relative index; `undefined` out of range.
fn at(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    let Some(s) = this_str(heap, this)? else {
        return Ok(None);
    };
    let n = len(heap, s);
    let Some(rel) = int_arg(heap, arg(args, 0))? else {
        return Ok(None);
    };
    let k = if rel >= 0.0 { rel } else { n + rel };
    if k < 0.0 || k >= n {
        return Ok(Some(Value::UNDEFINED));
    }
    some_str(string::substring(heap, s, k as u32, k as u32 + 1)?)
}

/// String.prototype.repeat; a RangeError (a negative or infinite count) declines.
fn repeat(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    let Some(s) = this_str(heap, this)? else {
        return Ok(None);
    };
    let Some(n) = int_arg(heap, arg(args, 0))? else {
        return Ok(None);
    };
    if n < 0.0 || n == f64::INFINITY {
        return Ok(None);
    }
    if heap.str_len(s) == 0 || n == 0.0 {
        return empty(heap);
    }
    let s = heap.contiguous(s)?;
    some_str(string::repeat(heap, s, n.min(u32::MAX as f64) as u32)?)
}

/// StringPad of string.ts: ToLength of `maxLength`, then the filler only if needed.
fn pad(heap: &mut Heap, this: Value, args: &[Value], at_start: bool) -> Fast {
    let Some(s) = this_str(heap, this)? else {
        return Ok(None);
    };
    let Some(max) = int_arg(heap, arg(args, 0))? else {
        return Ok(None);
    };
    let n = len(heap, s);
    if max <= n {
        return some_str(s);
    }
    // `None`: the default filler, one space.
    let filler = match arg(args, 1) {
        v if v.is_undefined() => None,
        v => match str(heap, v)? {
            Some(f) => Some(f),
            None => return Ok(None),
        },
    };
    let fl = filler.map_or(1, |f| heap.str_len(f));
    if fl == 0 {
        return some_str(s);
    }
    // Long pads and two-byte fillers are the builtin's (by doubling, in StrRepeat).
    let fill = max - n;
    if fill > 65536.0 || filler.is_some_and(|f| heap.str_is_two_byte(f)) {
        return Ok(None);
    }
    let fill = fill as u32;
    // A short one-byte result is written once, from the stack.
    let mut short = [b' '; 128];
    let total = fill as usize + n as usize;
    if total <= short.len() && !heap.str_is_two_byte(s) {
        let s = heap.contiguous(s)?;
        let (pad_at, s_at) = if at_start {
            (0, fill as usize)
        } else {
            (n as usize, 0)
        };
        let unit = match filler {
            None => Some(b' '),
            Some(f) if fl == 1 => {
                let f = heap.contiguous(f)?;
                Some(heap.str_view(f).expect("contiguous").get(0) as u8)
            }
            Some(_) => None,
        };
        if let Some(unit) = unit
            && heap.cell_kind(s.0) == crate::heap::CellKind::Str8
        {
            let r = heap.alloc_str_uninit(total as u32, false)?;
            let data = Heap::str_data_offset(r);
            let a = heap.arena_mut();
            a.slice_mut(data + pad_at as u32, fill).fill(unit);
            a.copy_within(Heap::str_data_offset(s), n as u32, data + s_at as u32);
            return some_str(r);
        }
        if let Some(f) = filler {
            let f = heap.contiguous(f)?;
            let view = heap.str_view(f).expect("contiguous");
            for (i, d) in short[pad_at..pad_at + fill as usize].iter_mut().enumerate() {
                *d = view.get(i % fl as usize) as u8;
            }
        }
        if let Some(StrView::Latin1(b)) = heap.str_view(s) {
            short[s_at..s_at + b.len()].copy_from_slice(b);
            return some_str(heap.alloc_latin1(&short[..total])?);
        }
    }
    // The one-byte filler cycled into one buffer: one allocation for the run.
    let unit: Vec<u8> = match filler {
        Some(f) => heap.str_view_flat(f)?.units().map(|u| u as u8).collect(),
        None => alloc::vec![b' '],
    };
    let bytes: Vec<u8> = unit.iter().copied().cycle().take(fill as usize).collect();
    let run = heap.alloc_latin1(&bytes)?;
    let whole = n as u32;
    let mut pieces = if at_start {
        [(run, 0, fill), (s, 0, whole)]
    } else {
        [(s, 0, whole), (run, 0, fill)]
    };
    some_str(string::join(heap, &mut pieces)?)
}

fn pad_start(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    pad(heap, this, args, true)
}

fn pad_end(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    pad(heap, this, args, false)
}

/// The code unit index of charAt/charCodeAt/codePointAt: `None` declines, `Some(None)`
/// is out of range.
fn position(heap: &mut Heap, s: Str, v: Value) -> Result<Option<Option<u32>>, Fault> {
    let Some(p) = int_arg(heap, v)? else {
        return Ok(None);
    };
    Ok(Some((p >= 0.0 && p < len(heap, s)).then_some(p as u32)))
}

fn char_at(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    let Some(s) = this_str(heap, this)? else {
        return Ok(None);
    };
    match position(heap, s, arg(args, 0))? {
        None => Ok(None),
        Some(None) => empty(heap),
        Some(Some(i)) => some_str(string::substring(heap, s, i, i + 1)?),
    }
}

fn char_code_at(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    let Some(s) = this_str(heap, this)? else {
        return Ok(None);
    };
    Ok(match position(heap, s, arg(args, 0))? {
        None => None,
        Some(None) => Some(Value::NAN),
        Some(Some(i)) => {
            let s = heap.contiguous(s)?;
            Some(Value::int(heap.char_code_at(s, i).unwrap_or(0) as i32))
        }
    })
}

fn new_array(heap: &mut Heap, values: &[Value]) -> Fast {
    let proto = realm::intrinsic_obj(heap, Intrinsic::ArrayProto);
    Ok(Some(Value::object(
        heap.new_array_from_values(Some(proto), values)?,
    )))
}

/// String.prototype.split with a primitive separator, or a pristine RegExp whose
/// @@split is the original (any other object's @@split is a lookup).
fn split(heap: &mut Heap, this: Value, args: &[Value], fuel: &mut i64) -> Fast {
    let (sep, limit) = (arg(args, 0), arg(args, 1));
    if sep.as_object().is_some() {
        // RegExpPrototypeSplit's leaf case: a non-empty string and a limit that is
        // undefined or a Number, above 0.
        let Some(r) = symbol_method(heap, sep, regexp::SPLIT, atoms::SYM_SPLIT, REGEXP_SPLIT)
        else {
            return Ok(None);
        };
        let Some(s) = this_str(heap, this)? else {
            return Ok(None);
        };
        let lim = match limit.as_number() {
            Some(n) => number::to_uint32(n),
            None if limit.is_undefined() => u32::MAX,
            None => return Ok(None),
        };
        if lim == 0 || heap.str_len(s) == 0 {
            return Ok(None);
        }
        return regexp::split_now(heap, r, s, lim, fuel);
    }
    let Some(s) = this_str(heap, this)? else {
        return Ok(None);
    };
    let lim = if limit.is_undefined() {
        u32::MAX
    } else {
        match num(heap, limit)? {
            Some(n) => number::to_uint32(n),
            None => return Ok(None),
        }
    };
    let Some(r) = str(heap, sep)? else {
        return Ok(None);
    };
    if lim == 0 {
        return new_array(heap, &[]);
    }
    if sep.is_undefined() {
        return new_array(heap, &[Value::string(s)]);
    }
    let n = heap.str_len(s);
    let m = heap.str_len(r);
    let mut parts = Vec::new();
    if m == 0 {
        for i in 0..n.min(lim) {
            parts.push_out(Value::string(string::substring(heap, s, i, i + 1)?));
        }
        return new_array(heap, &parts);
    }
    // One-byte string and separator: find every piece in one pass over the bytes, then
    // copy them (0110: a search per piece repeated the setup of `index_of`).
    heap.str_view_flat(s)?;
    let r = heap.flatten(r)?;
    if let (Some(StrView::Latin1(h)), Some(StrView::Latin1(t))) =
        (heap.str_view(s), heap.str_view(r))
    {
        let mut cuts: Vec<(u32, u32)> = crate::buf::vec_with_capacity(16);
        let mut p = 0;
        while let Some(q) = string::find_latin1(h, t, p) {
            cuts.push_out((p as u32, q as u32));
            if cuts.len() as u32 == lim {
                break;
            }
            p = q + t.len();
        }
        if (cuts.len() as u32) < lim {
            cuts.push_out((p as u32, n));
        }
        // Each piece a slice of `s` or copied straight from it (flat, one-byte), the empty
        // ones shared (0129: through `substring` and `join` each paid their checks again).
        let empty = Value::string(heap.atom_string(atoms::EMPTY)?);
        parts.reserve_exact(cuts.len());
        for (a, b) in cuts {
            parts.push_out(if a == b {
                empty
            } else if a == 0 && b == n {
                Value::string(s)
            } else if let Some(piece) = heap.substring_slice(s, a, b)? {
                // Long lines of a long text share its units (D2542).
                Value::string(piece)
            } else {
                Value::string(heap.alloc_latin1_pieces(&[(s, a, b)], b - a)?)
            });
        }
        return new_array(heap, &parts);
    }
    let mut p = 0;
    let mut q = string::index_of(heap, s, r, 0, false)?;
    while q >= 0.0 {
        parts.push_out(Value::string(string::substring(heap, s, p, q as u32)?));
        if parts.len() as u32 == lim {
            return new_array(heap, &parts);
        }
        p = q as u32 + m;
        q = string::index_of(heap, s, r, p, false)?;
    }
    parts.push_out(Value::string(string::substring(heap, s, p, n)?));
    new_array(heap, &parts)
}

/// Whether `s[at .. at + len(t))` is `t`; both are contiguous and the range lies in `s`.
fn range_eq(heap: &Heap, s: Str, at: usize, t: Str) -> bool {
    match (heap.str_view(s), heap.str_view(t)) {
        (Some(StrView::Latin1(h)), Some(StrView::Latin1(n))) => h[at..at + n.len()] == *n,
        (Some(h), Some(n)) => (0..n.len()).all(|i| h.get(at + i) == n.get(i)),
        _ => false,
    }
}

/// String.prototype.startsWith ( searchString [ , position ] ).
fn starts_with(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    let Some((s, t)) = search(heap, this, arg(args, 0))? else {
        return Ok(None);
    };
    let Some(p) = int_arg(heap, arg(args, 1))? else {
        return Ok(None);
    };
    let (n, m) = (len(heap, s), len(heap, t));
    let start = clamp(p, n);
    if start + m > n {
        return Ok(Some(Value::FALSE));
    }
    let (s, t) = (heap.contiguous(s)?, heap.contiguous(t)?);
    Ok(Some(Value::bool(range_eq(heap, s, start as usize, t))))
}

/// String.prototype.endsWith ( searchString [ , endPosition ] ).
fn ends_with(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    let Some((s, t)) = search(heap, this, arg(args, 0))? else {
        return Ok(None);
    };
    let (n, m) = (len(heap, s), len(heap, t));
    let end = match arg(args, 1) {
        v if v.is_undefined() => n,
        v => match int_arg(heap, v)? {
            Some(p) => clamp(p, n),
            None => return Ok(None),
        },
    };
    if m > end {
        return Ok(Some(Value::FALSE));
    }
    let (s, t) = (heap.contiguous(s)?, heap.contiguous(t)?);
    Ok(Some(Value::bool(range_eq(heap, s, (end - m) as usize, t))))
}

/// String.prototype.lastIndexOf ( searchString [ , position ] ): a NaN position is the
/// end.
fn last_index_of(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    let Some((s, t)) = search(heap, this, arg(args, 0))? else {
        return Ok(None);
    };
    let Some(p) = num(heap, arg(args, 1))? else {
        return Ok(None);
    };
    let (n, m) = (len(heap, s), len(heap, t));
    let pos = if p.is_nan() { n } else { clamp(integer(p), n) };
    if m > n {
        return Ok(Some(Value::int(-1)));
    }
    let from = pos.min(n - m) as u32;
    Ok(Some(conv::number(string::index_of(
        heap, s, t, from, true,
    )?)))
}

/// String.prototype.codePointAt ( pos ).
fn code_point_at(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    let Some(s) = this_str(heap, this)? else {
        return Ok(None);
    };
    Ok(match position(heap, s, arg(args, 0))? {
        None => None,
        Some(None) => Some(Value::UNDEFINED),
        Some(Some(i)) => {
            let s = heap.contiguous(s)?;
            Some(string::code_point_at(heap, s, i).map_or(Value::UNDEFINED, conv::number))
        }
    })
}

/// String.prototype.localeCompare ( that ).
pub(crate) fn runs_locale_compare(heap: &Heap, f: Value) -> bool {
    let Some(o) = f.as_object() else {
        return false;
    };
    if heap.object_class(o) != ObjectClass::Function {
        return false;
    }
    let t = heap.closure_template(o);
    let a = heap.arena();
    let run = locale_compare as fn(&mut Heap, Value, &[Value]) -> Fast;
    u32::from(a.get::<u16>(t, FuncTemplate::FLAGS)) & func_flags::FAST != 0
        && FAST
            .get(usize::from(a.get::<u8>(t, FuncTemplate::FAST)))
            .is_some_and(|entry| entry.run as usize == run as usize)
}

fn locale_compare(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    let Some((s, t)) = search(heap, this, arg(args, 0))? else {
        return Ok(None);
    };
    Ok(Some(conv::number(string::locale_compare(heap, s, t)?)))
}

/// String.prototype.concat ( ...args ) of primitives.
fn concat(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    let Some(mut r) = this_str(heap, this)? else {
        return Ok(None);
    };
    let mut parts = [r; MAX_ARGS];
    for (p, &v) in parts.iter_mut().zip(args) {
        let Some(s) = str(heap, v)? else {
            return Ok(None);
        };
        *p = s;
    }
    for &p in &parts[..args.len()] {
        r = heap.concat(r, p)?;
    }
    some_str(r)
}

fn to_lower_case(heap: &mut Heap, this: Value, _: &[Value]) -> Fast {
    let Some(s) = this_str(heap, this)? else {
        return Ok(None);
    };
    some_str(string::to_case(heap, s, false)?)
}

fn to_upper_case(heap: &mut Heap, this: Value, _: &[Value]) -> Fast {
    let Some(s) = this_str(heap, this)? else {
        return Ok(None);
    };
    some_str(string::to_case(heap, s, true)?)
}

/// trim (`where_` 0), trimStart (1), trimEnd (2).
fn trim_at(heap: &mut Heap, this: Value, where_: i32) -> Fast {
    let Some(s) = this_str(heap, this)? else {
        return Ok(None);
    };
    some_str(string::trim(heap, s, where_)?)
}

fn trim(heap: &mut Heap, this: Value, _: &[Value]) -> Fast {
    trim_at(heap, this, 0)
}

fn trim_start(heap: &mut Heap, this: Value, _: &[Value]) -> Fast {
    trim_at(heap, this, 1)
}

fn trim_end(heap: &mut Heap, this: Value, _: &[Value]) -> Fast {
    trim_at(heap, this, 2)
}

/// String.prototype.substr ( start, length ).
fn substr(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    let Some(s) = this_str(heap, this)? else {
        return Ok(None);
    };
    let n = len(heap, s);
    let Some(start) = relative(heap, arg(args, 0), n, 0.0)? else {
        return Ok(None);
    };
    let count = match arg(args, 1) {
        v if v.is_undefined() => n,
        v => match int_arg(heap, v)? {
            Some(c) => c,
            None => return Ok(None),
        },
    };
    let end = (start + count).min(n);
    if start >= end {
        return empty(heap);
    }
    some_str(string::substring(heap, s, start as u32, end as u32)?)
}

const REGEXP_REPLACE: u16 = intrinsic_id("RegExpPrototypeReplace");
const REGEXP_SEARCH: u16 = intrinsic_id("RegExpPrototypeSearch");
const REGEXP_SPLIT: u16 = intrinsic_id("RegExpPrototypeSplit");
const REGEXP_MATCH: u16 = intrinsic_id("RegExpPrototypeMatch");
const REGEXP_MATCH_ALL: u16 = intrinsic_id("RegExpPrototypeMatchAll");

/// The pristine RegExp `v` (`regexp::pristine`: no own property but lastIndex, so its
/// methods are %RegExp.prototype%'s) when the method under the well-known symbol `key`
/// is the original builtin `id`: a String method that hands over to it can run its
/// leaf case directly.
fn symbol_method(
    heap: &Heap,
    v: Value,
    which: usize,
    key: crate::heap::Atom,
    id: u16,
) -> Option<Obj> {
    let r = regexp::pristine(heap, v)?;
    regexp::original_method(heap, which, key, id).then_some(r)
}

/// What replace and replaceAll search for.
enum Pattern {
    Text(Str),
    RegExp(Obj),
}

/// The string, the pattern and the replacement template of replace and replaceAll with a
/// primitive replacement (an object is a function or runs its toString) and a primitive
/// pattern or a pristine RegExp whose @@replace is the original (any other object is
/// asked for its @@replace).
fn replace_args(
    heap: &mut Heap,
    this: Value,
    args: &[Value],
) -> Result<Option<(Str, Pattern, Str)>, Fault> {
    let (pattern, replacement) = (arg(args, 0), arg(args, 1));
    if replacement.as_object().is_some() {
        return Ok(None);
    }
    let regexp = match pattern.as_object() {
        None => None,
        Some(_) => match symbol_method(
            heap,
            pattern,
            regexp::REPLACE,
            atoms::SYM_REPLACE,
            REGEXP_REPLACE,
        ) {
            Some(r) => Some(r),
            None => return Ok(None),
        },
    };
    let Some(s) = this_str(heap, this)? else {
        return Ok(None);
    };
    let pattern = match regexp {
        Some(r) => Pattern::RegExp(r),
        None => match str(heap, pattern)? {
            Some(t) => Pattern::Text(t),
            None => return Ok(None),
        },
    };
    Ok(str(heap, replacement)?.map(|r| (s, pattern, r)))
}

/// GetSubstitution of regexp.ts for a match `s[at .. at + m)` of a string pattern, as
/// pieces: without captures `$n` and `$<` stay as written.
fn substitution(
    heap: &mut Heap,
    pieces: &mut Vec<(Str, u32, u32)>,
    template: Str,
    s: Str,
    at: u32,
    m: u32,
) -> Result<(), Fault> {
    let t = heap.contiguous(template)?;
    let n = heap.str_len(s);
    let view = heap.str_view(t).expect("contiguous");
    let tl = view.len() as u32;
    let (mut run, mut k) = (0, 0);
    while k + 1 < tl {
        if view.get(k as usize) != 0x24 {
            k += 1;
            continue;
        }
        let piece = match view.get(k as usize + 1) {
            0x24 => (t, k, k + 1),
            0x26 => (s, at, at + m),
            0x60 => (s, 0, at),
            0x27 => (s, (at + m).min(n), n),
            _ => {
                k += 1;
                continue;
            }
        };
        pieces.push_out((t, run, k));
        pieces.push_out(piece);
        k += 2;
        run = k;
    }
    pieces.push_out((t, run, tl));
    Ok(())
}

/// String.prototype.replace ( searchValue, replaceValue ) with a string replacement.
fn replace(heap: &mut Heap, this: Value, args: &[Value], fuel: &mut i64) -> Fast {
    let Some((s, pattern, r)) = replace_args(heap, this, args)? else {
        return Ok(None);
    };
    let t = match pattern {
        Pattern::Text(t) => t,
        Pattern::RegExp(rx) => return regexp::replace_now(heap, rx, s, r, fuel),
    };
    let at = string::index_of(heap, s, t, 0, false)?;
    if at < 0.0 {
        return some_str(s);
    }
    let (at, m, n) = (at as u32, heap.str_len(t), heap.str_len(s));
    let mut pieces = crate::buf::vec_with_capacity(8);
    pieces.push_out((s, 0, at));
    substitution(heap, &mut pieces, r, s, at, m)?;
    pieces.push_out((s, at + m, n));
    some_str(string::join(heap, &mut pieces)?)
}

/// String.prototype.replaceAll ( searchValue, replaceValue ) with a string replacement; a
/// RegExp without the `g` flag is the builtin's TypeError.
fn replace_all(heap: &mut Heap, this: Value, args: &[Value], fuel: &mut i64) -> Fast {
    let Some((s, pattern, r)) = replace_args(heap, this, args)? else {
        return Ok(None);
    };
    let t = match pattern {
        Pattern::Text(t) => t,
        Pattern::RegExp(rx) if regexp::has_flag(heap, rx, u16::from(b'g')) => {
            return regexp::replace_now(heap, rx, s, r, fuel);
        }
        Pattern::RegExp(_) => return Ok(None),
    };
    let (m, n) = (heap.str_len(t), heap.str_len(s));
    let advance = m.max(1);
    let mut pieces = crate::buf::vec_with_capacity(16);
    let mut end = 0;
    let mut at = string::index_of(heap, s, t, 0, false)?;
    while at >= 0.0 {
        let p = at as u32;
        pieces.push_out((s, end, p));
        substitution(heap, &mut pieces, r, s, p, m)?;
        end = p + m;
        at = string::index_of(heap, s, t, p + advance, false)?;
    }
    if pieces.is_empty() {
        return some_str(s);
    }
    if end < n {
        pieces.push_out((s, end, n));
    }
    some_str(string::join(heap, &mut pieces)?)
}

// ---- RegExp --------------------------------------------------------------------------

/// The RegExp `this` and the string argument of test and exec.
fn regexp_and_string(
    heap: &mut Heap,
    this: Value,
    args: &[Value],
) -> Result<Option<(Obj, Str)>, Fault> {
    let Some(r) = regexp::as_regexp(heap, this) else {
        return Ok(None);
    };
    Ok(str(heap, arg(args, 0))?.map(|s| (r, s)))
}

/// RegExp.prototype.test ( S ) when RegExpExec is RegExpBuiltinExec (`%RegExpExecOwn`'s
/// case).
fn regexp_test(heap: &mut Heap, this: Value, args: &[Value], fuel: &mut i64) -> Fast {
    let Some((r, s)) = regexp_and_string(heap, this, args)? else {
        return Ok(None);
    };
    match regexp::last_index(heap, r) {
        Some(last) if regexp::exec_is_original(heap, r) => {
            regexp::exec_now(heap, r, s, last, true, fuel)
        }
        _ => Ok(None),
    }
}

/// RegExp.prototype.exec ( string ): RegExpBuiltinExec with a Number lastIndex.
fn regexp_exec(heap: &mut Heap, this: Value, args: &[Value], fuel: &mut i64) -> Fast {
    let Some((r, s)) = regexp_and_string(heap, this, args)? else {
        return Ok(None);
    };
    match regexp::last_index(heap, r) {
        Some(last) => regexp::exec_now(heap, r, s, last, false, fuel),
        None => Ok(None),
    }
}

/// %RegExpStringIteratorPrototype%.next ( ) over a RegExp whose RegExpExec is
/// RegExpBuiltinExec. The result object is made first: the steps after the match only
/// store.
fn regexp_string_iterator_next(heap: &mut Heap, this: Value, _: &[Value], fuel: &mut i64) -> Fast {
    let Some(it) = this
        .as_object()
        .filter(|&o| heap.object_class(o) == ObjectClass::RegExpStringIterator)
    else {
        return Ok(None);
    };
    let flags = heap.object_slot(it, regexp::IT_FLAGS).as_int().unwrap_or(0);
    let proto = realm::intrinsic_obj(heap, Intrinsic::ObjectProto);
    let result = heap.new_iter_result(Some(proto), Value::UNDEFINED, true)?;
    if flags & regexp::IT_DONE != 0 {
        return Ok(Some(Value::object(result)));
    }
    let (Some(r), Some(s)) = (
        regexp::as_regexp(heap, heap.object_slot(it, regexp::IT_REGEXP)),
        heap.object_slot(it, regexp::IT_STRING).as_string(),
    ) else {
        return Ok(None);
    };
    let Some(last) = regexp::last_index(heap, r).filter(|_| regexp::exec_is_original(heap, r))
    else {
        return Ok(None);
    };
    let Some(found) = regexp::exec_now(heap, r, s, last, false, fuel)? else {
        return Ok(None);
    };
    let finish = |heap: &mut Heap| {
        heap.set_object_slot(it, regexp::IT_FLAGS, Value::int(flags | regexp::IT_DONE));
    };
    let Some(matched) = found.as_object() else {
        finish(heap);
        return Ok(Some(Value::object(result)));
    };
    if flags & regexp::IT_GLOBAL == 0 {
        finish(heap);
    } else if heap
        .get_element_fast(matched, 0)
        .and_then(Value::as_string)
        .is_some_and(|m| heap.str_len(m) == 0)
    {
        // AdvanceAfterEmptyMatch: the match just set lastIndex (a global RegExp).
        let at = regexp::last_index(heap, r).unwrap_or(0.0) as usize;
        let s = heap.contiguous(s)?;
        let view = heap.str_view(s).expect("flat string");
        let next = regexp::advance(view, at, flags & regexp::IT_UNICODE != 0);
        heap.set_own_data_value(r, atoms::LAST_INDEX, Value::int(next as i32));
    }
    heap.set_own_data_value(result, atoms::VALUE, found);
    heap.set_own_data_value(result, atoms::DONE, Value::FALSE);
    Ok(Some(Value::object(result)))
}

/// String.prototype.match ( regexp ) of a pristine RegExp: every match of a global one,
/// else RegExpExec.
fn string_match(heap: &mut Heap, this: Value, args: &[Value], fuel: &mut i64) -> Fast {
    let Some(r) = symbol_method(
        heap,
        arg(args, 0),
        regexp::MATCH,
        atoms::SYM_MATCH,
        REGEXP_MATCH,
    ) else {
        return Ok(None);
    };
    let Some(s) = this_str(heap, this)? else {
        return Ok(None);
    };
    if regexp::has_flag(heap, r, u16::from(b'g')) {
        return regexp::match_all_now(heap, r, s, fuel);
    }
    match regexp::last_index(heap, r) {
        Some(last) => regexp::exec_now(heap, r, s, last, false, fuel),
        None => Ok(None),
    }
}

/// String.prototype.matchAll ( regexp ) of a pristine global RegExp (without `g` it is
/// the builtin's TypeError): the iterator over a copy of it that starts at its
/// lastIndex, as RegExpPrototypeMatchAll makes it.
fn string_match_all(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    let Some(r) = symbol_method(
        heap,
        arg(args, 0),
        regexp::MATCH_ALL,
        atoms::SYM_MATCH_ALL,
        REGEXP_MATCH_ALL,
    ) else {
        return Ok(None);
    };
    let (Some(last), true) = (
        regexp::last_index(heap, r),
        regexp::has_flag(heap, r, u16::from(b'g')),
    ) else {
        return Ok(None);
    };
    let Some(s) = this_str(heap, this)? else {
        return Ok(None);
    };
    let matcher = regexp::copy(heap, r, conv::number(regexp::to_length(last)))?;
    let mut flags = regexp::IT_GLOBAL;
    if regexp::flags(heap, r).unicode_mode() {
        flags |= regexp::IT_UNICODE;
    }
    Ok(Some(Value::object(regexp::string_iterator(
        heap, matcher, s, flags,
    )?)))
}

/// String.prototype.search ( regexp ) of a pristine RegExp.
fn string_search(heap: &mut Heap, this: Value, args: &[Value], fuel: &mut i64) -> Fast {
    let Some(r) = symbol_method(
        heap,
        arg(args, 0),
        regexp::SEARCH,
        atoms::SYM_SEARCH,
        REGEXP_SEARCH,
    ) else {
        return Ok(None);
    };
    let Some(s) = this_str(heap, this)? else {
        return Ok(None);
    };
    regexp::search_now(heap, r, s, fuel)
}

// ---- Number --------------------------------------------------------------------------

/// `Number(value)` called as a function: ToNumeric of a primitive (a BigInt converts).
fn number_constructor(heap: &mut Heap, _: Value, args: &[Value]) -> Fast {
    if args.is_empty() {
        return Ok(Some(Value::int(0)));
    }
    Ok(num(heap, args[0])?.map(conv::number))
}

/// Number.prototype.toFixed of a Number; a RangeError (digits out of 0..=100) declines.
fn to_fixed(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    let (Some(x), Some(d)) = (this.as_number(), int_arg(heap, arg(args, 0))?) else {
        return Ok(None);
    };
    some_str(number::format(heap, 1, x, conv::number(d))?)
}

/// String(v) called (not constructed: only calls take fast entries) on a primitive other
/// than a symbol (SymbolDescriptiveString stays with the builtin).
fn string_constructor(heap: &mut Heap, _: Value, args: &[Value]) -> Fast {
    match args.first() {
        None => empty(heap),
        Some(v) if v.as_symbol().is_some() => Ok(None),
        Some(&v) => Ok(str(heap, v)?.map(Value::string)),
    }
}

/// Number.isInteger: never throws, so every argument is answered.
fn number_is_integer(_: &mut Heap, _: Value, args: &[Value]) -> Fast {
    Ok(Some(Value::bool(arg(args, 0).as_number().is_some_and(
        |n| n.is_finite() && djs_libm::trunc(n) == n,
    ))))
}

// ---- Math ----------------------------------------------------------------------------

/// Math.f ( x ) of a primitive whose ToNumber is pure.
fn math(heap: &mut Heap, args: &[Value], f: fn(f64) -> f64) -> Fast {
    Ok(num(heap, arg(args, 0))?.map(|x| conv::number(f(x))))
}

/// Math.f ( x, y ) of two such primitives (both converted before `f` runs).
fn math2(heap: &mut Heap, args: &[Value], f: fn(f64, f64) -> f64) -> Fast {
    let (Some(x), Some(y)) = (num(heap, arg(args, 0))?, num(heap, arg(args, 1))?) else {
        return Ok(None);
    };
    Ok(Some(conv::number(f(x, y))))
}

/// parseInt ( string, radix ) of primitives.
fn parse_int(heap: &mut Heap, _: Value, args: &[Value]) -> Fast {
    let (Some(s), Some(radix)) = (str(heap, arg(args, 0))?, num(heap, arg(args, 1))?) else {
        return Ok(None);
    };
    let radix = conv::to_int32(radix);
    Ok(Some(conv::number(number::parse(heap, s, Some(radix))?)))
}

/// parseFloat ( string ) of a primitive.
fn parse_float(heap: &mut Heap, _: Value, args: &[Value]) -> Fast {
    let Some(s) = str(heap, arg(args, 0))? else {
        return Ok(None);
    };
    Ok(Some(conv::number(number::parse(heap, s, None)?)))
}

/// Number.prototype.toString ( [ radix ] ) of a Number; a RangeError (the radix)
/// declines.
fn number_to_string(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    let Some(x) = this.as_number() else {
        return Ok(None);
    };
    let radix = match arg(args, 0) {
        v if v.is_undefined() => 10.0,
        v => match int_arg(heap, v)? {
            Some(r) => r,
            None => return Ok(None),
        },
    };
    some_str(number::format(heap, 0, x, conv::number(radix))?)
}

/// Math.max (`max`) or Math.min of primitives whose ToNumber is pure. Every argument is
/// converted before the result is known (a NaN does not end the loop), so an object or
/// a symbol after a NaN still declines to the builtin, which runs or throws.
fn extremum(heap: &mut Heap, args: &[Value], max: bool) -> Fast {
    let mut r = if max {
        f64::NEG_INFINITY
    } else {
        f64::INFINITY
    };
    let mut nan = false;
    for &v in args {
        let Some(x) = num(heap, v)? else {
            return Ok(None);
        };
        nan |= x.is_nan();
        // +0 is larger than -0.
        let pick = if max {
            x > r || (x == 0.0 && r == 0.0 && x.is_sign_positive())
        } else {
            x < r || (x == 0.0 && r == 0.0 && x.is_sign_negative())
        };
        if pick {
            r = x;
        }
    }
    Ok(Some(if nan { Value::NAN } else { conv::number(r) }))
}

fn math_max(heap: &mut Heap, _: Value, args: &[Value]) -> Fast {
    extremum(heap, args, true)
}

fn math_min(heap: &mut Heap, _: Value, args: &[Value]) -> Fast {
    extremum(heap, args, false)
}

// ---- Object / Array ------------------------------------------------------------------

/// The id of an `@intrinsic` builtin, at compile time.
pub(crate) const fn intrinsic_id(name: &str) -> u16 {
    let mut k = 0;
    while k < INTRINSICS.len() {
        let (a, b) = (INTRINSICS[k].name.as_bytes(), name.as_bytes());
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

const ARRAY_CONSTRUCTOR: u16 = intrinsic_id("ArrayConstructor");
const ARRAY_SPECIES: u16 = intrinsic_id("ArraySpecies");

/// ArraySpeciesCreate(original, length) when nothing in it can run user code: `original`
/// is not an Array (and not a proxy), or an Array without an own `constructor` whose
/// prototype is %Array.prototype%, whose `constructor` is the data property %Array%,
/// whose @@species is the original getter (which returns %Array%). Then the result is
/// ArrayCreate(length), as Construct(%Array%, length) would make it. Anything else
/// (subclasses, patched constructors or species, proxies, a bad length) declines.
fn array_species_create(heap: &mut Heap, _: Value, args: &[Value]) -> Fast {
    let (Some(o), Some(n)) = (arg(args, 0).as_object(), arg(args, 1).as_number()) else {
        return Ok(None);
    };
    if !(n >= 0.0 && n <= u32::MAX as f64 && (n as u32) as f64 == n) || !plain_species(heap, o) {
        return Ok(None);
    }
    let array_proto = realm::intrinsic_obj(heap, Intrinsic::ArrayProto);
    let a = heap.new_array_with_length(Some(array_proto), n as u32)?;
    Ok(Some(Value::object(a)))
}

/// Whether ArraySpeciesCreate(o, n) is ArrayCreate(n) with %Array.prototype%, found
/// without running user code: `o` is no Array, or an Array whose `constructor` is the
/// untouched %Array% with the original @@species getter.
pub(crate) fn plain_species(heap: &Heap, o: Obj) -> bool {
    if heap.is_proxy(o) {
        return false;
    }
    if heap.object_class(o) != ObjectClass::Array {
        return true;
    }
    let array_proto = realm::intrinsic_obj(heap, Intrinsic::ArrayProto);
    if heap.has_own_property(o, atoms::CONSTRUCTOR) || heap.get_prototype_of(o) != Some(array_proto)
    {
        return false;
    }
    let array = realm::installed_intrinsic(heap, ARRAY_CONSTRUCTOR);
    // Frozen builtins (D2304): %Array.prototype%.constructor and %Array%[@@species] are
    // as the base made them, which is plain (0162).
    if let Some(a) = array.as_object()
        && heap.frozen(array_proto)
        && heap.frozen(a)
    {
        return true;
    }
    let ctor = atoms::CONSTRUCTOR;
    if heap
        .own_entry_index(array_proto, ctor)
        .and_then(|i| heap.data_at(array_proto, i, ctor))
        != Some(array)
    {
        return false;
    }
    let Some(array) = array.as_object() else {
        return false;
    };
    let species = realm::installed_intrinsic(heap, ARRAY_SPECIES);
    let key = atoms::SYM_SPECIES;
    heap.own_entry_index(array, key)
        .and_then(|i| heap.getter_at(array, i, key))
        == Some(species)
}

// ---- Array element loops (0091) ------------------------------------------------------
// `this` must be an Array (ToObject is the identity, LengthOfArrayLike its length);
// the loops are builtins/dense.rs's, which decline at a hole, a non-dense element or
// anything else that could run user code.

/// An Array `this` and its length.
fn this_array(heap: &Heap, this: Value) -> Option<(Obj, u32)> {
    let o = this.as_object()?;
    (heap.object_class(o) == ObjectClass::Array).then(|| (o, heap.array_length(o)))
}

/// Array.prototype.push ( item ): one item as a plain append.
fn array_push(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    let (Some((o, _)), [v]) = (this_array(heap, this), args) else {
        return Ok(None);
    };
    Ok(dense::push(heap, o, *v)?.map(|n| conv::number(n as f64)))
}

/// Array.prototype.pop ( ) of an array `Heap::array_pop_dense` takes.
fn array_pop(heap: &mut Heap, this: Value, _: &[Value]) -> Fast {
    let Some((o, _)) = this_array(heap, this) else {
        return Ok(None);
    };
    Ok(heap.array_pop_dense(o))
}

/// A search result of `dense::search` as the builtin's Number, `None` to decline.
fn found(r: f64) -> Option<f64> {
    (r != dense::DECLINE).then_some(r)
}

/// Array.prototype.indexOf ( searchElement [ , fromIndex ] ).
fn array_index_of(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    let Some((o, len)) = this_array(heap, this) else {
        return Ok(None);
    };
    if len == 0 {
        return Ok(Some(Value::int(-1)));
    }
    let Some(mut n) = int_arg(heap, arg(args, 1))? else {
        return Ok(None);
    };
    let len_f = len as f64;
    if n >= len_f {
        return Ok(Some(Value::int(-1)));
    }
    if n < 0.0 {
        n = (len_f + n).max(0.0);
    }
    let (r, _) = dense::search(heap, o, arg(args, 0), n as u32, len, 0);
    Ok(found(r).map(conv::number))
}

/// Array.prototype.includes ( searchElement [ , fromIndex ] ).
fn array_includes(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    let Some((o, len)) = this_array(heap, this) else {
        return Ok(None);
    };
    if len == 0 {
        return Ok(Some(Value::FALSE));
    }
    let Some(mut n) = int_arg(heap, arg(args, 1))? else {
        return Ok(None);
    };
    let len_f = len as f64;
    if n >= len_f {
        return Ok(Some(Value::FALSE));
    }
    if n < 0.0 {
        n = (len_f + n).max(0.0);
    }
    let (r, _) = dense::search(heap, o, arg(args, 0), n as u32, len, 2);
    Ok(found(r).map(|r| Value::bool(r >= 0.0)))
}

/// Array.prototype.slice ( start, end ) with an Array result (ArraySpeciesCreate's
/// entry) that takes plain appends.
fn array_slice(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    let Some((o, len)) = this_array(heap, this) else {
        return Ok(None);
    };
    let len_f = len as f64;
    let (Some(k), Some(fin)) = (
        relative(heap, arg(args, 0), len_f, 0.0)?,
        relative(heap, arg(args, 1), len_f, len_f)?,
    ) else {
        return Ok(None);
    };
    // A plain result, copied from dense elements in one step (0129). Anything else
    // declines: the builtin's own path (ArraySpeciesCreate, then `%ArraySliceDense`)
    // is what this did after it.
    if !plain_species(heap, o) {
        return Ok(None);
    }
    let proto = realm::intrinsic_obj(heap, Intrinsic::ArrayProto);
    let (k, fin) = (k as u32, fin.max(k) as u32);
    Ok(heap
        .new_array_from_dense(Some(proto), o, k, fin)?
        .map(Value::object))
}

/// Array.prototype.join ( separator ) with a primitive separator.
fn array_join(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    let Some((o, len)) = this_array(heap, this) else {
        return Ok(None);
    };
    if crate::builtins::array::join_active(heap, o) {
        return Ok(None);
    }
    let sep = match arg(args, 0) {
        v if v.is_undefined() => heap.alloc_latin1(b",")?,
        v => match str(heap, v)? {
            Some(s) => s,
            None => return Ok(None),
        },
    };
    Ok(dense::join(heap, o, len, sep)?.map(Value::string))
}

/// Array.prototype.reverse ( ) of an Array whose elements are all present in its dense
/// store: HasProperty and Get observe nothing, and each Set is a plain store.
fn array_reverse(heap: &mut Heap, this: Value, _: &[Value]) -> Fast {
    let Some((o, len)) = this_array(heap, this) else {
        return Ok(None);
    };
    if heap.frozen(o) {
        return Ok(None);
    }
    if heap.dense_len(o) != len
        || !heap
            .dense_array_values(o)
            .is_some_and(|mut vs| vs.all(|v| !v.is_hole()))
    {
        return Ok(None);
    }
    for i in 0..len / 2 {
        let j = len - 1 - i;
        if let (Some(a), Some(b)) = (heap.get_element_fast(o, i), heap.get_element_fast(o, j)) {
            heap.set_element_fast(o, i, b)?;
            heap.set_element_fast(o, j, a)?;
        }
    }
    Ok(Some(this))
}

/// Whether Get(o, @@isConcatSpreadable) is undefined without running user code: no object
/// on the prototype chain (none a proxy) has the property.
fn no_spreadable_key(heap: &Heap, o: Obj) -> bool {
    let mut c = Some(o);
    while let Some(q) = c {
        if heap.is_proxy(q) || heap.has_own_property(q, atoms::SYM_IS_CONCAT_SPREADABLE) {
            return false;
        }
        c = heap.get_prototype_of(q);
    }
    true
}

/// Array.prototype.concat ( ...items ) with a plain Array result: `this` and the items
/// that are Arrays spread their dense elements (all present), every other item (an
/// object only without @@isConcatSpreadable on its chain) is one element.
fn array_concat(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    let Some((o, _)) = this_array(heap, this) else {
        return Ok(None);
    };
    if !plain_species(heap, o) {
        return Ok(None);
    }
    let mut values: Vec<Value> = Vec::new();
    for &e in core::iter::once(&this).chain(args) {
        let Some(eo) = e.as_object() else {
            values.push_out(e);
            continue;
        };
        if !no_spreadable_key(heap, eo) {
            return Ok(None);
        }
        if heap.object_class(eo) != ObjectClass::Array {
            values.push_out(e);
            continue;
        }
        let Some(part) = dense::dense_range(heap, eo, 0, heap.array_length(eo)) else {
            return Ok(None);
        };
        values.extend_from_slice(&part);
    }
    if values.len() as u64 > u64::from(crate::heap::atom::MAX_INT_ATOM) {
        return Ok(None);
    }
    new_array(heap, &values)
}

const FLAT_MAX_NESTING: u32 = 64;

fn flatten_into(heap: &Heap, o: Obj, depth: f64, level: u32, out: &mut Vec<Value>) -> bool {
    let Some(part) = dense::dense_range(heap, o, 0, heap.array_length(o)) else {
        return false;
    };
    for e in part {
        match e.as_object() {
            Some(eo) if depth > 0.0 && heap.is_proxy(eo) => return false,
            Some(eo) if depth > 0.0 && heap.object_class(eo) == ObjectClass::Array => {
                if level >= FLAT_MAX_NESTING || !flatten_into(heap, eo, depth - 1.0, level + 1, out)
                {
                    return false;
                }
            }
            _ => out.push_out(e),
        }
    }
    true
}

fn array_flat(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    let Some((o, _)) = this_array(heap, this) else {
        return Ok(None);
    };
    let d = arg(args, 0);
    let depth = if d.is_undefined() {
        1.0
    } else {
        match int_arg(heap, d)? {
            Some(n) => n.max(0.0),
            None => return Ok(None),
        }
    };
    if !plain_species(heap, o) {
        return Ok(None);
    }
    let mut values: Vec<Value> = Vec::new();
    if !flatten_into(heap, o, depth, 0, &mut values)
        || values.len() as u64 > u64::from(crate::heap::atom::MAX_INT_ATOM)
    {
        return Ok(None);
    }
    new_array(heap, &values)
}

/// Array.isArray ( arg ); a revoked proxy is the builtin's TypeError.
fn array_is_array(heap: &mut Heap, _: Value, args: &[Value]) -> Fast {
    Ok(match arg(args, 0).as_object() {
        None => Some(Value::FALSE),
        Some(o) => heap.is_array(o).map(Value::bool),
    })
}

/// Array.prototype.shift ( ) of a non-empty Array that `array_splice_dense` takes: the
/// first element, the rest moved down in one step.
fn array_shift(heap: &mut Heap, this: Value, _: &[Value]) -> Fast {
    let Some((o, len)) = this_array(heap, this) else {
        return Ok(None);
    };
    let first = match heap.get_element_fast(o, 0) {
        Some(v) if len > 0 => v,
        _ => return Ok(None),
    };
    Ok(heap.array_splice_dense(o, 0, 1, 0)?.then_some(first))
}

/// Array.prototype.unshift ( ...items ) with items, on such an Array: room made in one
/// step, then plain stores.
fn array_unshift(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    let Some((o, len)) = this_array(heap, this) else {
        return Ok(None);
    };
    let n = args.len() as u32;
    if n == 0 || !heap.array_splice_dense(o, 0, 0, n)? {
        return Ok(None);
    }
    for (i, &v) in args.iter().enumerate() {
        heap.set_element_fast(o, i as u32, v)?;
    }
    Ok(Some(conv::number(f64::from(len) + f64::from(n))))
}

/// Array.prototype.splice ( start, deleteCount, ...items ) of an Array with a plain
/// species whose deleted elements are dense: they are copied out first, then the tail
/// moves in one step (`array_splice_dense`) and the items are plain stores. With as
/// many items as deleted elements nothing moves: the stores replace dense elements,
/// and the final Set of an unchanged `length` needs it writable.
fn array_splice(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    let Some((o, len)) = this_array(heap, this) else {
        return Ok(None);
    };
    if !plain_species(heap, o) || heap.frozen(o) {
        return Ok(None);
    }
    let len_f = f64::from(len);
    let Some(start) = relative(heap, arg(args, 0), len_f, 0.0)? else {
        return Ok(None);
    };
    let delete = match args {
        [] => 0.0,
        [_] => len_f - start,
        [_, count, ..] => match int_arg(heap, *count)? {
            Some(d) => d.max(0.0).min(len_f - start),
            None => return Ok(None),
        },
    };
    let items = args.get(2..).unwrap_or_default();
    let (start, delete, insert) = (start as u32, delete as u32, items.len() as u32);
    let Some(removed) = dense::dense_range(heap, o, start, start + delete) else {
        return Ok(None);
    };
    let proto = realm::intrinsic_obj(heap, Intrinsic::ArrayProto);
    let a = heap.new_array_from_values(Some(proto), &removed)?;
    let moved = if delete != insert {
        heap.array_splice_dense(o, start, delete, insert)?
    } else {
        matches!(heap.get_own_property(o, atoms::LENGTH),
            Some(Property::Data { flags, .. }) if flags.writable())
    };
    if !moved {
        return Ok(None);
    }
    for (k, &v) in items.iter().enumerate() {
        heap.set_element_fast(o, start + k as u32, v)?;
    }
    Ok(Some(Value::object(a)))
}

// ---- btoa and atob ----------------------------------------------------------------

/// btoa ( data ) of a primitive; a code unit above 0xFF is the builtin's DOMException.
fn btoa(heap: &mut Heap, _: Value, args: &[Value]) -> Fast {
    let [data] = args else {
        return Ok(None);
    };
    let Some(s) = str(heap, *data)? else {
        return Ok(None);
    };
    Ok(crate::builtins::dom::btoa(heap, s)?.map(Value::string))
}

/// atob ( data ) of a primitive; an invalid string is the builtin's DOMException.
fn atob(heap: &mut Heap, _: Value, args: &[Value]) -> Fast {
    let [data] = args else {
        return Ok(None);
    };
    let Some(s) = str(heap, *data)? else {
        return Ok(None);
    };
    Ok(crate::builtins::dom::atob(heap, s)?.map(Value::string))
}

// ---- String.fromCharCode (0091) -----------------------------------------------------

/// String.fromCharCode ( ...codeUnits ) of primitives (ToUint16 of ToNumber).
fn from_char_code(heap: &mut Heap, _: Value, args: &[Value]) -> Fast {
    // At most MAX_ARGS units: on the stack.
    let mut units = [0u16; MAX_ARGS];
    for (u, &v) in units.iter_mut().zip(args) {
        let Some(n) = num(heap, v)? else {
            return Ok(None);
        };
        *u = conv::to_int32(n) as u16;
    }
    let units = &units[..args.len()];
    let s = if units.iter().all(|&u| u < 256) {
        let mut bytes = [0u8; MAX_ARGS];
        for (b, &u) in bytes.iter_mut().zip(units) {
            *b = u as u8;
        }
        heap.alloc_latin1(&bytes[..units.len()])?
    } else {
        heap.alloc_utf16(units)?
    };
    some_str(s)
}

// ---- Map and Set ----------------------------------------------------------------------
// ThisMap/ThisSet decline for anything but a Map (Set); the collection leaves run no user
// code, and CollectionSet allocates (flattening the key, growing the table) before it
// changes anything observable.

/// `this` if it is an object of class `c`.
fn assignable(heap: &Heap, to: Obj, from: Value) -> bool {
    if from.as_string().is_some_and(|s| heap.str_len(s) > 0) {
        return false;
    }
    let Some(from) = from.as_object() else {
        return true;
    };
    if heap.is_proxy(from) {
        return false;
    }
    heap.own_keys(from)
        .into_iter()
        .all(|k| match heap.get_own_property(from, k) {
            Some(Property::Accessor { flags, .. }) => !flags.enumerable(),
            Some(Property::Data { flags, .. }) if flags.enumerable() => {
                match heap.lookup_property(to, k) {
                    Lookup::Found(holder, Property::Data { flags, .. }) => {
                        flags.writable() && (holder == to || heap.is_extensible(to))
                    }
                    Lookup::Found(_, Property::Accessor { .. }) | Lookup::Proxy(_) => false,
                    Lookup::Absent => heap.is_extensible(to),
                }
            }
            _ => true,
        })
}

const ASSIGN_INLINE: usize = 16;

fn object_assign(heap: &mut Heap, _: Value, args: &[Value]) -> Fast {
    let Some((&target, sources)) = args.split_first() else {
        return Ok(None);
    };
    let Some(to) = target.as_object() else {
        return Ok(None);
    };
    if heap.object_class(to) != ObjectClass::Ordinary
        || !sources.iter().all(|&from| assignable(heap, to, from))
    {
        return Ok(None);
    }
    for from in sources.iter().filter_map(|from| from.as_object()) {
        if heap.clone_plain_props(to, from)? {
            continue;
        }
        let mut keys = [atoms::EMPTY; ASSIGN_INLINE];
        let mut values = [Value::UNDEFINED; ASSIGN_INLINE];
        if let Some(n) = heap.plain_entries_into(from, &mut keys, &mut values) {
            for (&k, &value) in keys[..n].iter().zip(&values[..n]) {
                if !heap.set_own_data_value(to, k, value) {
                    heap.create_data_property(to, k, value)?;
                }
            }
            continue;
        }
        for k in heap.own_keys(from) {
            if let Some(Property::Data { value, flags }) = heap.get_own_property_alloc(from, k)?
                && flags.enumerable()
                && !heap.set_own_data_value(to, k, value)
            {
                heap.create_data_property(to, k, value)?;
            }
        }
    }
    Ok(Some(target))
}

fn this_collection(heap: &Heap, this: Value, c: ObjectClass) -> Option<Obj> {
    this.as_object().filter(|&o| heap.object_class(o) == c)
}

/// Map.prototype.get ( key ).
fn map_get(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    Ok(this_collection(heap, this, ObjectClass::Map).map(|m| {
        heap.collection_get(m, arg(args, 0))
            .unwrap_or(Value::UNDEFINED)
    }))
}

/// Map.prototype.has ( key ) and Set.prototype.has ( value ).
fn has(heap: &Heap, this: Value, args: &[Value], c: ObjectClass) -> Fast {
    Ok(this_collection(heap, this, c).map(|m| Value::bool(heap.collection_has(m, arg(args, 0)))))
}

fn map_has(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    has(heap, this, args, ObjectClass::Map)
}

fn set_has(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    has(heap, this, args, ObjectClass::Set)
}

/// Map.prototype.set ( key, value ) and Set.prototype.add ( value ): `this`.
fn set(heap: &mut Heap, this: Value, key: Value, v: Value, c: ObjectClass) -> Fast {
    let Some(m) = this_collection(heap, this, c) else {
        return Ok(None);
    };
    let key = collection::stored_key(heap, key)?;
    heap.collection_set(m, key, v)?;
    Ok(Some(this))
}

fn map_set(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    set(heap, this, arg(args, 0), arg(args, 1), ObjectClass::Map)
}

fn set_add(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    set(heap, this, arg(args, 0), Value::TRUE, ObjectClass::Set)
}

// ---- constructors (0236) ---------------------------------------------------------------
// A construct takes an entry only when new.target is the constructor itself, whose
// `prototype` is non-writable and non-configurable: GetPrototypeFromConstructor is the
// realm's prototype without a lookup.

/// A constructor whose calls run the builtin.
fn no_call(_: &mut Heap, _: Value, _: &[Value]) -> Fast {
    Ok(None)
}

/// Array(...values) and new Array(...values): one Number argument is a length (an
/// invalid one throws in the builtin), anything else the elements.
fn array_new(heap: &mut Heap, args: &[Value]) -> Fast {
    match args {
        [len] if len.is_number() => {
            let n = len.as_number().unwrap_or(f64::NAN);
            if !(n >= 0.0 && n <= u32::MAX as f64 && (n as u32) as f64 == n) {
                return Ok(None);
            }
            let proto = realm::intrinsic_obj(heap, Intrinsic::ArrayProto);
            Ok(Some(Value::object(
                heap.new_array_with_length(Some(proto), n as u32)?,
            )))
        }
        _ => new_array(heap, args),
    }
}

fn array_call(heap: &mut Heap, _: Value, args: &[Value]) -> Fast {
    array_new(heap, args)
}

/// `new Map()`, `new Map(undefined)`, `new Map(null)`: no entries to add.
fn map_new(heap: &mut Heap, args: &[Value]) -> Fast {
    empty_collection(heap, args, ObjectClass::Map, Intrinsic::MapProto)
}

fn set_new(heap: &mut Heap, args: &[Value]) -> Fast {
    empty_collection(heap, args, ObjectClass::Set, Intrinsic::SetProto)
}

fn empty_collection(heap: &mut Heap, args: &[Value], class: ObjectClass, proto: Intrinsic) -> Fast {
    let v = arg(args, 0);
    if !(v.is_undefined() || v.is_null()) {
        return Ok(None);
    }
    let proto = realm::intrinsic_obj(heap, proto);
    Ok(Some(Value::object(heap.collection_new(class, proto)?)))
}

/// The time value of a Date `this`.
fn this_date(heap: &Heap, this: Value) -> Option<f64> {
    this.as_object()
        .filter(|&o| heap.object_class(o) == ObjectClass::Date)
        .map(|o| heap.object_slot(o, 0).as_number().unwrap_or(f64::NAN))
}

/// Date.prototype.toISOString ( ); an invalid Date is the builtin's RangeError.
fn date_to_iso_string(heap: &mut Heap, this: Value, _: &[Value]) -> Fast {
    let Some(t) = this_date(heap, this) else {
        return Ok(None);
    };
    let offset = realm::intrinsic(heap, Intrinsic::TimeZoneOffset)
        .as_int()
        .unwrap_or(0) as i64;
    match crate::builtins::date::format(t, 2, 3, offset) {
        Some(s) => some_str(heap.alloc_str(&s)?),
        None => Ok(None),
    }
}

/// `new Date()`, and `new Date(v)` where DateValueFromArgs runs no user code: a Date
/// (its time value), a string (parsed), another primitive whose ToNumber cannot throw.
fn date_new(heap: &mut Heap, args: &[Value]) -> Fast {
    let tv = match args {
        [] => realm::intrinsic(heap, Intrinsic::Clock)
            .as_number()
            .unwrap_or(f64::NAN),
        [v] => {
            let t = if let Some(o) = v.as_object() {
                if heap.object_class(o) != ObjectClass::Date {
                    return Ok(None);
                }
                heap.object_slot(o, 0).as_number().unwrap_or(f64::NAN)
            } else if let Some(s) = v.as_string() {
                let offset = realm::intrinsic(heap, Intrinsic::TimeZoneOffset)
                    .as_int()
                    .unwrap_or(0) as i64;
                crate::builtins::date::parse(heap.str_units(s), offset)
            } else {
                let Some(n) = num(heap, *v)? else {
                    return Ok(None);
                };
                n
            };
            // TimeClip.
            if !(-8.64e15..=8.64e15).contains(&t) {
                f64::NAN
            } else {
                integer(t)
            }
        }
        _ => return Ok(None),
    };
    let proto = realm::intrinsic_obj(heap, Intrinsic::DateProto);
    let o = heap.alloc_object(ObjectClass::Date, Some(proto), 1)?;
    heap.set_object_slot(o, 0, Value::number(tv));
    Ok(Some(Value::object(o)))
}

#[cfg(test)]
mod tests {
    #[test]
    fn array_push_index() {
        let run = super::FAST[super::ARRAY_PUSH].run as usize;
        assert_eq!(run, super::array_push as fn(_, _, _) -> _ as usize);
    }
}
