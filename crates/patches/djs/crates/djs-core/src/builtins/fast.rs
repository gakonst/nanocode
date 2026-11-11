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

use crate::builtins::{collection, dense, number, string};
use crate::bytecode::intrinsics::INTRINSICS;
use crate::heap::{Heap, ObjectClass, Property, StrView, atoms};
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
    /// A constructor's `new F(args)` with new.target F, `new(heap, args)` (0236); `None`:
    /// constructs run the builtin.
    pub new: Option<fn(&mut Heap, &[Value]) -> Fast>,
}

/// The most arguments an entry looks at (calls with more decline).
pub(crate) const MAX_ARGS: usize = 6;

impl FastFn {
    pub(crate) fn call(&self, heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
        (self.run)(heap, this, args)
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
        new: None,
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
    f("StringPrototypeSplit", split),
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

/// String.prototype.split with a primitive separator (an object's @@split is a lookup).
fn split(heap: &mut Heap, this: Value, args: &[Value]) -> Fast {
    let (sep, limit) = (arg(args, 0), arg(args, 1));
    if sep.as_object().is_some() {
        return Ok(None);
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
const fn intrinsic_id(name: &str) -> u16 {
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
    match heap.get_own_property(array_proto, atoms::CONSTRUCTOR) {
        Some(Property::Data { value, .. }) if value == array => {}
        _ => return false,
    }
    let Some(array) = array.as_object() else {
        return false;
    };
    let species = realm::installed_intrinsic(heap, ARRAY_SPECIES);
    matches!(heap.get_own_property(array, atoms::SYM_SPECIES),
        Some(Property::Accessor { getter, .. }) if getter == species)
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
    let sep = match arg(args, 0) {
        v if v.is_undefined() => heap.alloc_latin1(b",")?,
        v => match str(heap, v)? {
            Some(s) => s,
            None => return Ok(None),
        },
    };
    Ok(dense::join(heap, o, len, sep)?.map(Value::string))
}

// ---- String.fromCharCode (0091) -----------------------------------------------------

/// String.fromCharCode ( ...codeUnits ) of primitives (ToUint16 of ToNumber).
fn from_char_code(heap: &mut Heap, _: Value, args: &[Value]) -> Fast {
    let mut units: Vec<u16> = crate::buf::vec_with_capacity(args.len());
    for &v in args {
        let Some(n) = num(heap, v)? else {
            return Ok(None);
        };
        units.push_out(conv::to_int32(n) as u16);
    }
    let s = if units.iter().all(|&u| u < 256) {
        let bytes: Vec<u8> = units.iter().map(|&u| u as u8).collect();
        heap.alloc_latin1(&bytes)?
    } else {
        heap.alloc_utf16(&units)?
    };
    some_str(s)
}

// ---- Map and Set ----------------------------------------------------------------------
// ThisMap/ThisSet decline for anything but a Map (Set); the collection leaves run no user
// code, and CollectionSet allocates (flattening the key, growing the table) before it
// changes anything observable.

/// `this` if it is an object of class `c`.
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
