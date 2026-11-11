//! The JSON and RegExp leaf intrinsics (0033), dispatched from `call_intrinsic` for the
//! ids its own table does not handle. Kept apart so the big match there stays someone
//! else's merge conflict.

use super::intrinsics::{bad_argument, id};
use super::*;
use crate::builtins::dense::index as dense_index;
use crate::builtins::{json, regexp as re};
use crate::heap::atoms;
use crate::regexp::{ExecLimits, ExecState, Flags, Outcome};
use crate::value::Str;

const JSON_PARSE: u16 = id("JsonParse");
const JSON_STRINGIFY_FAST: u16 = id("JsonStringifyFast");
const JSON_STRINGIFY_FAST_AT: u16 = id("JsonStringifyFastAt");
const JSON_QUOTE: u16 = id("JsonQuote");
const JSON_RAW_JSON: u16 = id("JsonRawJSON");
const IS_RAW_JSON: u16 = id("IsRawJSON");
const CREATE_DATA_PROPERTY: u16 = id("CreateDataProperty");
const IS_REGEXP_OBJECT: u16 = id("IsRegExpObject");
const REGEXP_CREATE: u16 = id("RegExpCreate");
const REGEXP_SOURCE: u16 = id("RegExpSource");
const REGEXP_FLAGS: u16 = id("RegExpFlags");
const REGEXP_HAS_FLAG: u16 = id("RegExpHasFlag");
const REGEXP_ESCAPED_SOURCE: u16 = id("RegExpEscapedSource");
const REGEXP_EXEC_RAW: u16 = id("RegExpExecRaw");
const CREATE_REGEXP_STRING_ITERATOR: u16 = id("CreateRegExpStringIterator");
const REGEXP_STRING_ITERATOR_SLOT: u16 = id("RegExpStringIteratorSlot");
const REGEXP_STRING_ITERATOR_FINISH: u16 = id("RegExpStringIteratorFinish");
const REGEXP_ESCAPE_STRING: u16 = id("RegExpEscapeString");
const IS_PRISTINE_REGEXP: u16 = id("IsPristineRegExp");
const IS_DENSE_ARRAY: u16 = id("IsDenseArray");
const ALL_NUMBERS: u16 = id("AllNumbers");
const ALL_CODE_POINTS: u16 = id("AllCodePoints");
const REGEXP_TEST_RAW: u16 = id("RegExpTestRaw");
const REGEXP_EXEC_OWN: u16 = id("RegExpExecOwn");
const IS_ORIGINAL_REGEXP_EXEC: u16 = id("IsOriginalRegExpExec");
const REGEXP_SPLIT_RAW: u16 = id("RegExpSplitRaw");
const REGEXP_REPLACE_RAW: u16 = id("RegExpReplaceRaw");
const REGEXP_PROTOTYPE_EXEC: u16 = id("RegExpPrototypeExec");
const REGEXP_MATCH_SPANS: u16 = id("RegExpMatchSpans");
const REGEXP_SPAN_COUNT: u16 = id("RegExpSpanCount");
const REGEXP_REPLACE_ARGS: u16 = id("RegExpReplaceArgs");
const REGEXP_SPAN_TEXTS: u16 = id("RegExpSpanTexts");
const REGEXP_SPAN_STARTS: u16 = id("RegExpSpanStarts");
const REGEXP_REPLACE_JOIN: u16 = id("RegExpReplaceJoin");

/// Fuel for work proportional to a string's length: one unit per 16 code units (the
/// leaves run to completion, so they pay afterwards).
fn length_fuel(len: u32) -> i64 {
    (len / 16) as i64
}

impl Exec<'_> {
    /// `call_intrinsic` for the JSON and RegExp intrinsics; `base` is the first
    /// argument's slot. Anything else is not supported.
    pub(super) fn json_regexp_intrinsic(
        &mut self,
        id: u16,
        base: u32,
        argc: u32,
        next: u32,
    ) -> Step {
        let arg = |ex: &Self, i: u32| {
            if i < argc {
                ex.get(base + i)
            } else {
                Value::UNDEFINED
            }
        };
        let a0 = arg(self, 0);
        let v = match id {
            JSON_PARSE => {
                let Some(s) = a0.as_string() else {
                    return bad_argument(id);
                };
                self.fuel -= length_fuel(self.heap.str_len(s));
                match json::parse(self.heap, s)? {
                    // With the second argument: `[value, record]` for the reviver (D1611).
                    Ok(v) if arg(self, 1).as_bool() == Some(true) => {
                        let rec = json::source_records(self.heap, s, v)?;
                        let proto = intrinsic_obj(self.heap, Intrinsic::ArrayProto);
                        let pair = self.heap.new_array_from_values(Some(proto), &[v, rec])?;
                        Value::object(pair)
                    }
                    Ok(v) => v,
                    Err(e) => {
                        // qjs puts the position in the stack, as a first `<input>` frame.
                        let err = realm::new_error(self.heap, ErrorKind::SyntaxError, &e.message)?;
                        self.set_stack_at(err, self.pc, Some((e.line, e.column)))?;
                        return Err(Fault::throw(err));
                    }
                }
            }
            JSON_STRINGIFY_FAST | JSON_STRINGIFY_FAST_AT => {
                let Some(gap) = arg(self, 1).as_string() else {
                    return bad_argument(id);
                };
                let r = if id == JSON_STRINGIFY_FAST {
                    json::stringify_fast(self.heap, a0, gap)?
                } else {
                    let Some(stack) = arg(self, 2).as_object() else {
                        return bad_argument(id);
                    };
                    json::stringify_fast_in(self.heap, a0, gap, stack, arg(self, 3))?
                };
                if let Some(s) = r.as_string() {
                    self.fuel -= length_fuel(self.heap.str_len(s));
                }
                r
            }
            JSON_RAW_JSON => {
                let Some(s) = a0.as_string() else {
                    return bad_argument(id);
                };
                json::raw_json(self.heap, s)?
            }
            IS_RAW_JSON => {
                Value::bool(a0.as_object().is_some_and(|o| {
                    self.heap.object_class(o) == crate::heap::ObjectClass::RawJSON
                }))
            }
            JSON_QUOTE => {
                let Some(s) = a0.as_string() else {
                    return bad_argument(id);
                };
                self.fuel -= length_fuel(self.heap.str_len(s));
                Value::string(json::quote(self.heap, s)?)
            }
            CREATE_DATA_PROPERTY => {
                let Some(o) = a0.as_object() else {
                    return bad_argument(id);
                };
                let key = conv::to_property_key(self.heap, arg(self, 1))?;
                Value::bool(self.heap.create_data_property(o, key, arg(self, 2))?)
            }
            IS_REGEXP_OBJECT => Value::bool(re::as_regexp(self.heap, a0).is_some()),
            REGEXP_CREATE => {
                // The first argument is a prototype, or a literal's compiled program.
                let (proto, cell) = (a0.as_object(), a0.as_internal());
                let Some(f) = arg(self, 2).as_string() else {
                    return bad_argument(id);
                };
                let pattern = arg(self, 1);
                if let Some(like) = re::as_regexp(self.heap, pattern) {
                    Value::object(re::create_like(self.heap, proto, like, f)?)
                } else {
                    let Some(p) = pattern.as_string() else {
                        return bad_argument(id);
                    };
                    Value::object(re::create(self.heap, proto, p, f, cell)?)
                }
            }
            REGEXP_SOURCE | REGEXP_FLAGS | REGEXP_HAS_FLAG | REGEXP_ESCAPED_SOURCE => {
                let Some(r) = re::as_regexp(self.heap, a0) else {
                    return bad_argument(id);
                };
                match id {
                    REGEXP_SOURCE => Value::string(re::source(self.heap, r)),
                    REGEXP_FLAGS => Value::string(re::flags_string(self.heap, r)?),
                    REGEXP_ESCAPED_SOURCE => Value::string(re::escaped_source(self.heap, r)?),
                    _ => {
                        let Some(c) = arg(self, 1)
                            .as_string()
                            .and_then(|f| self.heap.char_code_at(f, 0))
                        else {
                            return bad_argument(id);
                        };
                        Value::bool(re::has_flag(self.heap, r, c))
                    }
                }
            }
            REGEXP_EXEC_RAW if argc == 4 => return self.regexp_exec_raw(id, base, next, false),
            REGEXP_TEST_RAW if argc == 4 => return self.regexp_exec_raw(id, base, next, true),
            REGEXP_EXEC_OWN if argc == 4 => return self.regexp_exec_own(id, base, next),
            IS_ORIGINAL_REGEXP_EXEC => {
                Value::bool(a0 == realm::installed_intrinsic(self.heap, REGEXP_PROTOTYPE_EXEC))
            }
            REGEXP_SPLIT_RAW if argc == 4 => return self.regexp_split_raw(id, base, next),
            REGEXP_REPLACE_RAW if argc == 4 => return self.regexp_replace_raw(id, base, next),
            REGEXP_MATCH_SPANS if argc == 3 => return self.regexp_match_spans(id, base, next),
            REGEXP_SPAN_COUNT | REGEXP_SPAN_TEXTS | REGEXP_SPAN_STARTS | REGEXP_REPLACE_ARGS
            | REGEXP_REPLACE_JOIN => {
                let (Some(r), Some(s), Some(spans)) = (
                    re::as_regexp(self.heap, a0),
                    arg(self, 1).as_string(),
                    arg(self, 2).as_object(),
                ) else {
                    return bad_argument(id);
                };
                match id {
                    REGEXP_SPAN_COUNT => Value::int(re::span_count(self.heap, r, spans) as i32),
                    REGEXP_SPAN_TEXTS => re::span_texts(self.heap, r, s, spans)?,
                    REGEXP_SPAN_STARTS => Value::object(re::span_starts(self.heap, r, spans)?),
                    REGEXP_REPLACE_ARGS => match arg(self, 3).as_number().and_then(dense_index) {
                        Some(k) => Value::object(re::replace_args(self.heap, r, s, spans, k)?),
                        None => return bad_argument(id),
                    },
                    _ => match arg(self, 3).as_object() {
                        Some(to) => Value::string(re::replace_join(self.heap, r, s, spans, to)?),
                        None => return bad_argument(id),
                    },
                }
            }
            IS_PRISTINE_REGEXP => Value::bool(self.regexp_pristine(a0)?),
            // Bulk fast paths for builtins (D1561): apply, fromCodePoint, Math.max.
            IS_DENSE_ARRAY => Value::bool(
                a0.as_object()
                    .is_some_and(|a| self.dense_values(a).is_some()),
            ),
            ALL_NUMBERS | ALL_CODE_POINTS => {
                let Some(list) = a0.as_object() else {
                    return bad_argument(id);
                };
                let points = id == ALL_CODE_POINTS;
                // One pass over the dense store (holes are not Numbers), ints first.
                let ok = self.heap.dense_array_values(list).is_some_and(|mut vs| {
                    vs.all(|v| match v.as_int() {
                        Some(i) => !points || (0..=0x10FFFF).contains(&i),
                        None => match v.as_number() {
                            Some(x) if points => {
                                (0.0..=1114111.0).contains(&x) && djs_libm::trunc(x) == x
                            }
                            Some(_) => true,
                            None => false,
                        },
                    })
                });
                Value::bool(ok)
            }
            CREATE_REGEXP_STRING_ITERATOR => {
                let (Some(r), Some(s)) = (a0.as_object(), arg(self, 1).as_string()) else {
                    return bad_argument(id);
                };
                let mut flags = 0;
                if arg(self, 2).as_bool() == Some(true) {
                    flags |= re::IT_GLOBAL;
                }
                if arg(self, 3).as_bool() == Some(true) {
                    flags |= re::IT_UNICODE;
                }
                Value::object(re::string_iterator(self.heap, r, s, flags)?)
            }
            REGEXP_STRING_ITERATOR_SLOT => {
                let it = re::as_string_iterator(self.heap, a0)?;
                let flags = self
                    .heap
                    .object_slot(it, re::IT_FLAGS)
                    .as_int()
                    .unwrap_or(0);
                match arg(self, 1).as_int() {
                    Some(0) => self.heap.object_slot(it, re::IT_REGEXP),
                    Some(1) => self.heap.object_slot(it, re::IT_STRING),
                    Some(2) => Value::bool(flags & re::IT_GLOBAL != 0),
                    Some(3) => Value::bool(flags & re::IT_UNICODE != 0),
                    Some(4) => Value::bool(flags & re::IT_DONE != 0),
                    _ => return bad_argument(id),
                }
            }
            REGEXP_ESCAPE_STRING => {
                let Some(s) = a0.as_string() else {
                    return bad_argument(id);
                };
                Value::string(re::escape(self.heap, s)?)
            }
            REGEXP_STRING_ITERATOR_FINISH => {
                let it = re::as_string_iterator(self.heap, a0)?;
                let flags = self
                    .heap
                    .object_slot(it, re::IT_FLAGS)
                    .as_int()
                    .unwrap_or(0);
                self.heap
                    .set_object_slot(it, re::IT_FLAGS, Value::int(flags | re::IT_DONE));
                Value::UNDEFINED
            }
            _ => return self.bigint_intrinsic(id, base, argc, next),
        };
        self.sp = base;
        self.push(v);
        self.pc = next;
        Ok(None)
    }

    /// The length of an Array whose elements are all in its dense store (no holes), so
    /// that Get on them runs no user code; `None` for anything else.
    fn dense_values(&self, a: Obj) -> Option<u32> {
        let mut n = 0;
        self.heap
            .dense_array_values(a)?
            .all(|v| {
                n += 1;
                !v.is_hole()
            })
            .then_some(n)
    }

    /// Runs the matcher of the RegExp `r` on the flat string `flat` with the remaining fuel
    /// (at least one unit, so a resumed match always makes progress), from `start`, or
    /// continuing the match in `st` with `resume`.
    fn run_matcher(
        &mut self,
        r: Obj,
        flat: Str,
        start: usize,
        sticky: bool,
        resume: bool,
        st: &mut ExecState,
    ) -> Outcome {
        let budget = self.fuel.max(1);
        let mut fuel = budget;
        let limits = ExecLimits::default();
        let result = {
            let prog = re::program(self.heap, r);
            let view = self.heap.str_view(flat).expect("flat string");
            let input = re::input(view);
            if resume {
                crate::regexp::resume_in(prog, input, &mut fuel, limits, st)
            } else {
                crate::regexp::exec_in(prog, input, start, sticky, &mut fuel, limits, st)
            }
        };
        // A backreference can overdraw (D1711): the overdraft stays negative fuel, which
        // `run` carries as debt (D1740).
        self.fuel -= budget - fuel;
        result
    }

    /// A suspended matcher state as a U32Array cell.
    fn save_exec_state(&mut self, st: &ExecState) -> Result<Value, Fault> {
        let words = st.to_words();
        let cell = self.heap.alloc_u32_array(words.len() as u32)?;
        for (i, &w) in words.iter().enumerate() {
            self.heap.u32_array_set(cell, i as u32, w);
        }
        Ok(Value::internal(cell))
    }

    /// The matcher state saved in `v`, if there is one (into `st`): whether the match is
    /// to be resumed.
    fn load_exec_state(&self, v: Value, st: &mut ExecState) -> bool {
        let Some(cell) = v.as_internal() else {
            return false;
        };
        let n = self.heap.u32_array_len(cell);
        let words: alloc::vec::Vec<u32> =
            (0..n).map(|i| self.heap.u32_array_get(cell, i)).collect();
        match ExecState::from_words(&words) {
            Some(saved) => {
                *st = saved;
                true
            }
            None => false,
        }
    }

    /// Allocates a suspension record with `f` outside the memory cap and the GC threshold
    /// (D1710): it exists only because the slice ended here.
    fn unbudgeted<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T, Fault>) -> Result<T, Fault> {
        let saved = self.heap.begin_unbudgeted();
        let r = f(self);
        self.heap.end_unbudgeted(saved);
        r
    }

    /// Frees the suspension record in `slot` once the resumed intrinsic has read it
    /// (D1710): `first` is its first cell (the matcher state), the slot's value its last.
    /// Nothing else was allocated since, unless the host did between the two `run`s, so
    /// the arena ends up as if the slice had not ended inside the intrinsic.
    fn release_progress(&mut self, slot: u32, first: Value) {
        if let (Some(a), Some(b)) = (first.as_internal(), self.get(slot).as_internal()) {
            self.set(slot, Value::UNDEFINED);
            let end = b + self.heap.cell_size(b);
            self.heap.release_top(a, end);
        }
    }

    /// Stops `run` with the intrinsic's progress in its `state` argument slot; the next
    /// `run` executes the same `call_intrinsic` again and continues from there (D1332).
    fn suspend_in(&mut self, slot: u32, progress: Value) -> Step {
        self.set(slot, progress);
        // The resume dispatches this intrinsic again and charges it again (the
        // `call_intrinsic` instruction, or a native builtin's call): give back this
        // dispatch's charge, so a suspension costs nothing extra (D1740).
        self.fuel += if self.native != 0 {
            super::native::CALL_FUEL
        } else {
            crate::bytecode::OPS[crate::bytecode::Op::CallIntrinsic as usize]
                .cost
                .fuel() as i64
        };
        self.flush();
        Ok(Some(Status::OutOfFuel))
    }

    /// A progress record: an internal ValueArray of `values`.
    fn save_progress(&mut self, values: &[Value]) -> Result<Value, Fault> {
        let rec = self.heap.alloc_value_array(values.len() as u32)?;
        for (i, &v) in values.iter().enumerate() {
            self.heap.value_array_set(rec, i as u32, v);
        }
        self.heap.arena_mut().set(
            rec,
            crate::heap::cells::ValueArray::LEN,
            values.len() as u32,
        );
        Ok(Value::internal(rec))
    }

    fn progress(&self, v: Value, i: u32) -> Value {
        match v.as_internal() {
            Some(rec) => self.heap.value_array_get(rec, i),
            None => Value::UNDEFINED,
        }
    }

    /// `%RegExpExecRaw(r, s, lastIndex, state)`: RegExpBuiltinExec once `lastIndex` was
    /// read: a Number, ToLength'd here. Runs the matcher with the remaining fuel. When
    /// the fuel runs out mid-match, the matcher's state goes into a U32Array in the
    /// `state` argument slot (callers pass `undefined`) and `run` returns `OutOfFuel` with
    /// `pc` still at this instruction, so the next `run` executes it again and resumes the
    /// match from that state. Everything is in the arena: a heap image taken while
    /// suspended resumes the same match.
    /// With `test`, the result is just whether it matched (RegExp.prototype.test, D1560).
    fn regexp_exec_raw(&mut self, id: u16, base: u32, next: u32, test: bool) -> Step {
        let (Some(r), Some(s), Some(last)) = (
            re::as_regexp(self.heap, self.get(base)),
            self.get(base + 1).as_string(),
            self.get(base + 2).as_number(),
        ) else {
            return bad_argument(id);
        };
        self.regexp_exec(r, s, last, base, next, test)
    }

    /// `%RegExpExecOwn(r, s, test, state)`: RegExpExec when `exec` is the original and
    /// lastIndex an own Number (D3137); undefined otherwise. A resumed call (a state in
    /// its slot) passed the checks before, and nothing ran since.
    fn regexp_exec_own(&mut self, id: u16, base: u32, next: u32) -> Step {
        use crate::heap::Property;
        let Some(s) = self.get(base + 1).as_string() else {
            return bad_argument(id);
        };
        let test = self.get(base + 2) == Value::TRUE;
        let decline = |ex: &mut Self| {
            ex.sp = base;
            ex.push(Value::UNDEFINED);
            ex.pc = next;
            Ok(None)
        };
        let Some(r) = re::as_regexp(self.heap, self.get(base)) else {
            return decline(self);
        };
        let last = match self.heap.get_own_property(r, atoms::LAST_INDEX) {
            Some(Property::Data { value, .. }) if value.is_number() => value.as_number(),
            _ => None,
        };
        let proto = intrinsic_obj(self.heap, Intrinsic::RegExpProto);
        let original = || {
            self.heap.get_prototype_of(r) == Some(proto)
                && self.heap.get_own_property(r, atoms::EXEC).is_none()
                && matches!(
                    self.heap.get_own_property(proto, atoms::EXEC),
                    Some(Property::Data { value, .. })
                        if value == realm::installed_intrinsic(self.heap, REGEXP_PROTOTYPE_EXEC)
                )
        };
        match last {
            Some(last) if self.get(base + 3).as_internal().is_some() || original() => {
                self.regexp_exec(r, s, last, base, next, test)
            }
            _ => decline(self),
        }
    }

    /// RegExpBuiltinExec once lastIndex was read (`last`); the state slot is `base + 3`.
    fn regexp_exec(&mut self, r: Obj, s: Str, last: f64, base: u32, next: u32, test: bool) -> Step {
        let state_slot = base + 3;
        // ToLength.
        let last = if last > 0.0 {
            djs_libm::trunc(last)
        } else {
            0.0
        };
        let flags = re::flags(self.heap, r);
        let g_or_y = flags.contains(Flags::GLOBAL) || flags.contains(Flags::STICKY);
        let last = if g_or_y { last } else { 0.0 };
        let flat = self.heap.contiguous(s)?;
        let len = self.heap.str_len(flat) as f64;
        let v = if last > len {
            if g_or_y {
                re::set_last_index(self.heap, r, Value::int(0))?;
            }
            if test { Value::FALSE } else { Value::NULL }
        } else {
            let state = self.get(state_slot);
            let mut st = ExecState::new();
            let pending = self.load_exec_state(state, &mut st);
            self.release_progress(state_slot, state);
            let sticky = flags.contains(Flags::STICKY);
            match self.run_matcher(r, flat, last as usize, sticky, pending, &mut st) {
                Outcome::OutOfFuel => {
                    let saved = self.unbudgeted(|ex| ex.save_exec_state(&st))?;
                    return self.suspend_in(state_slot, saved);
                }
                Outcome::StackOverflow => return regexp_overflow(),
                Outcome::NoMatch => {
                    if g_or_y {
                        re::set_last_index(self.heap, r, Value::int(0))?;
                    }
                    if test { Value::FALSE } else { Value::NULL }
                }
                Outcome::Match => {
                    let caps = st.captures(re::program(self.heap, r));
                    if g_or_y {
                        let (_, end) = caps.get(0).expect("group 0 matched");
                        re::set_last_index(self.heap, r, Value::int(end as i32))?;
                    }
                    if test {
                        Value::TRUE
                    } else {
                        re::match_result(self.heap, r, s, caps)?
                    }
                }
            }
        };
        self.sp = base;
        self.push(v);
        self.pc = next;
        Ok(None)
    }

    /// `%RegExpMatchSpans(r, s, state)`: the matches RegExp.prototype[@@replace] collects
    /// from a pristine RegExp before it calls a replacer function: every match of a
    /// global one from lastIndex 0, else the first from lastIndex (0 unless sticky).
    /// The result is an Array of the capture positions of each match (the start and the
    /// end of every group, -1 for one that did not participate), and lastIndex is what
    /// that loop leaves: 0 for a global RegExp, the match's end or 0 for a sticky one.
    /// Suspendable like `RegExpSplitRaw`: the progress (the array, the next start, the
    /// matcher state) is kept in the state slot.
    fn regexp_match_spans(&mut self, id: u16, base: u32, next: u32) -> Step {
        let state_slot = base + 2;
        let (Some(r), Some(s)) = (
            re::as_regexp(self.heap, self.get(base)),
            self.get(base + 1).as_string(),
        ) else {
            return bad_argument(id);
        };
        let flat = self.heap.contiguous(s)?;
        let size = self.heap.str_len(flat) as usize;
        let flags = re::flags(self.heap, r);
        let (global, sticky) = (flags.contains(Flags::GLOBAL), flags.contains(Flags::STICKY));
        let unicode = flags.unicode_mode();
        let state = self.get(state_slot);
        let mut st = ExecState::new();
        let (a, mut last, mut pending) = match state.as_internal() {
            Some(_) => (
                self.progress(state, 0).as_object().expect("spans array"),
                self.progress(state, 1).as_int().unwrap_or(0) as usize,
                self.load_exec_state(self.progress(state, 2), &mut st),
            ),
            None => {
                let start = match re::last_index(self.heap, r) {
                    // ToLength; past the string is no match below.
                    Some(last) if sticky && !global && last > 0.0 => {
                        last.min(size as f64 + 1.0) as usize
                    }
                    _ => 0,
                };
                // Never a script's to see: no prototype.
                (self.heap.new_array(None, 0)?, start, false)
            }
        };
        self.release_progress(state_slot, self.progress(state, 2));
        // What lastIndex becomes, if it is written.
        let mut end = (global || sticky).then_some(0);
        loop {
            if last > size {
                break;
            }
            let resume = core::mem::take(&mut pending);
            match self.run_matcher(r, flat, last, sticky, resume, &mut st) {
                Outcome::OutOfFuel => {
                    let rec = self.unbudgeted(|ex| {
                        let saved = ex.save_exec_state(&st)?;
                        ex.save_progress(&[Value::object(a), Value::int(last as i32), saved])
                    })?;
                    return self.suspend_in(state_slot, rec);
                }
                Outcome::StackOverflow => return regexp_overflow(),
                Outcome::NoMatch => break,
                Outcome::Match => {
                    let caps = st.captures(re::program(self.heap, r));
                    let (m0, m1) = caps.get(0).expect("group 0 matched");
                    for g in 0..caps.len() {
                        let (x, y) = caps.get(g).map_or((-1, -1), |(x, y)| (x as i32, y as i32));
                        for v in [x, y] {
                            if self.heap.array_append_fast(a, Value::int(v))?.is_none() {
                                let key = self.heap.atom_from_index(self.heap.array_length(a))?;
                                self.heap.create_data_property(a, key, Value::int(v))?;
                            }
                        }
                    }
                    if !global {
                        end = sticky.then_some(m1);
                        break;
                    }
                    last = if m1 == m0 {
                        let view = self.heap.str_view(flat).expect("flat string");
                        re::advance(view, m1, unicode)
                    } else {
                        m1
                    };
                }
            }
        }
        if let Some(end) = end {
            re::set_last_index(self.heap, r, Value::int(end as i32))?;
        }
        self.sp = base;
        self.push(Value::object(a));
        self.pc = next;
        Ok(None)
    }

    /// Whether `v` is a pristine RegExp (`re::pristine`, D1503).
    fn regexp_pristine(&mut self, v: Value) -> Result<bool, Fault> {
        Ok(re::pristine(self.heap, v).is_some())
    }

    /// `%RegExpSplitRaw(r, s, lim, state)`: RegExp.prototype[@@split] for a pristine
    /// RegExp, a non-empty string and lim > 0 (D1503). The spec's sticky splitter tried at
    /// every position equals a search from that position. Suspendable like
    /// `RegExpExecRaw`: the progress (result array, p, q, matcher state) is kept in the
    /// state slot.
    fn regexp_split_raw(&mut self, id: u16, base: u32, next: u32) -> Step {
        let state_slot = base + 3;
        let (Some(r), Some(s), Some(lim)) = (
            re::as_regexp(self.heap, self.get(base)),
            self.get(base + 1).as_string(),
            self.get(base + 2).as_number(),
        ) else {
            return bad_argument(id);
        };
        let lim = lim as u32;
        let flat = self.heap.contiguous(s)?;
        let size = self.heap.str_len(flat) as usize;
        let unicode = re::program(self.heap, r).flags().unicode_mode();
        let state = self.get(state_slot);
        let mut st = ExecState::new();
        let (a, mut p, mut q, mut pending) = match state.as_internal() {
            Some(_) => (
                self.progress(state, 0).as_object().expect("split array"),
                self.progress(state, 1).as_int().unwrap_or(0) as usize,
                self.progress(state, 2).as_int().unwrap_or(0) as usize,
                self.load_exec_state(self.progress(state, 3), &mut st),
            ),
            None => {
                let proto = intrinsic_obj(self.heap, Intrinsic::ArrayProto);
                (self.heap.new_array(Some(proto), 0)?, 0, 0, false)
            }
        };
        self.release_progress(state_slot, self.progress(state, 3));
        let mut len = self.heap.array_length(a);
        let push = |ex: &mut Self, len: &mut u32, v: Value| -> Result<bool, Fault> {
            let key = crate::heap::Atom::int(*len)
                .ok_or(Fault::heap(crate::heap::HeapError::OutOfMemory))?;
            ex.heap.create_data_property(a, key, v)?;
            *len += 1;
            Ok(*len == lim)
        };
        let done = 'outer: loop {
            if q >= size {
                break false;
            }
            let resume = core::mem::take(&mut pending);
            match self.run_matcher(r, flat, q, false, resume, &mut st) {
                Outcome::OutOfFuel => {
                    let rec = self.unbudgeted(|ex| {
                        let saved = ex.save_exec_state(&st)?;
                        ex.save_progress(&[
                            Value::object(a),
                            Value::int(p as i32),
                            Value::int(q as i32),
                            saved,
                        ])
                    })?;
                    return self.suspend_in(state_slot, rec);
                }
                Outcome::StackOverflow => return regexp_overflow(),
                Outcome::NoMatch => break false,
                Outcome::Match => {
                    let caps = st.captures(re::program(self.heap, r));
                    let (at, e) = caps.get(0).expect("group 0 matched");
                    if at >= size {
                        break false;
                    }
                    if e == p {
                        let view = self.heap.str_view(flat).expect("flat string");
                        q = re::advance(view, at, unicode);
                        continue;
                    }
                    let piece = re::substring_value(self.heap, flat, p, at)?;
                    if push(self, &mut len, piece)? {
                        break true;
                    }
                    for g in 1..caps.len() {
                        let v = match caps.get(g) {
                            Some((x, y)) => re::substring_value(self.heap, flat, x, y)?,
                            None => Value::UNDEFINED,
                        };
                        if push(self, &mut len, v)? {
                            break 'outer true;
                        }
                    }
                    p = e;
                    q = p;
                }
            }
        };
        if !done {
            let tail = re::substring_value(self.heap, flat, p.min(size), size)?;
            push(self, &mut len, tail)?;
        }
        self.sp = base;
        self.push(Value::object(a));
        self.pc = next;
        Ok(None)
    }

    /// `%RegExpReplaceRaw(r, s, template, state)`: RegExp.prototype[@@replace] for a
    /// pristine global RegExp and a string replacement (D1503): every match from
    /// lastIndex 0, GetSubstitution in Rust, lastIndex 0 at the end. Suspendable: the
    /// output so far (as a string), the next source position, lastIndex and the matcher
    /// state are kept in the state slot.
    fn regexp_replace_raw(&mut self, id: u16, base: u32, next: u32) -> Step {
        let state_slot = base + 3;
        let (Some(r), Some(s), Some(t)) = (
            re::as_regexp(self.heap, self.get(base)),
            self.get(base + 1).as_string(),
            self.get(base + 2).as_string(),
        ) else {
            return bad_argument(id);
        };
        let flat = self.heap.contiguous(s)?;
        let size = self.heap.str_len(flat) as usize;
        let template = self.heap.str_to_utf16(t);
        // One byte per unit when the string and the template are: no widening and no
        // narrowing pass at the end.
        let narrow = !self.heap.str_view(flat).expect("flat string").is_two_byte()
            && template.iter().all(|&u| u <= 0xFF);
        let state = self.get(state_slot);
        let mut st = ExecState::new();
        let (acc, mut next_pos, mut last, mut resume) = match state.as_internal() {
            Some(_) => (
                self.progress(state, 0)
                    .as_string()
                    .map(|a| self.heap.str_to_utf16(a))
                    .unwrap_or_default(),
                self.progress(state, 1).as_int().unwrap_or(0) as usize,
                self.progress(state, 2).as_int().unwrap_or(0) as usize,
                self.load_exec_state(self.progress(state, 3), &mut st),
            ),
            None => (alloc::vec::Vec::new(), 0, 0, false),
        };
        self.release_progress(state_slot, self.progress(state, 3));
        // Room for an output as long as the input: most replacements are about as long
        // as what they replace.
        let mut out = if narrow {
            let mut v = crate::buf::vec_with_capacity(size.max(acc.len()));
            v.extend(acc.iter().map(|&u| u as u8));
            re::Units::Narrow(v)
        } else {
            let mut v = crate::buf::vec_with_capacity(size.max(acc.len()));
            v.extend_from_slice(&acc);
            re::Units::Wide(v)
        };
        // Every match in one borrow of the program and the string: nothing in the loop
        // allocates on the heap.
        let budget = self.fuel.max(1);
        let mut fuel = budget;
        let outcome = {
            let prog = re::program(self.heap, r);
            let view = self.heap.str_view(flat).expect("flat string");
            let input = re::input(view);
            let unicode = prog.flags().unicode_mode();
            let sticky = prog.flags().contains(Flags::STICKY);
            let limits = ExecLimits::default();
            loop {
                if last > size {
                    break Outcome::NoMatch;
                }
                let o = if core::mem::take(&mut resume) {
                    crate::regexp::resume_in(prog, input, &mut fuel, limits, &mut st)
                } else {
                    crate::regexp::exec_in(prog, input, last, sticky, &mut fuel, limits, &mut st)
                };
                if o != Outcome::Match {
                    break o;
                }
                let caps = st.captures(prog);
                let (m0, m1) = caps.get(0).expect("group 0 matched");
                out.range(view, next_pos, m0);
                re::substitute(&mut out, &template, view, caps, prog);
                next_pos = m1;
                last = if m1 == m0 {
                    re::advance(view, m1, unicode)
                } else {
                    m1
                };
            }
        };
        // A backreference can overdraw (D1711), as in `run_matcher`.
        self.fuel -= budget - fuel;
        match outcome {
            Outcome::OutOfFuel => {
                let rec = self.unbudgeted(|ex| {
                    let saved = ex.save_exec_state(&st)?;
                    // A fresh cell (never a shared one-character string): it is freed
                    // with the record.
                    let acc = ex.heap.alloc_utf16(&out.to_utf16())?;
                    ex.save_progress(&[
                        Value::string(acc),
                        Value::int(next_pos as i32),
                        Value::int(last as i32),
                        saved,
                    ])
                })?;
                return self.suspend_in(state_slot, rec);
            }
            Outcome::StackOverflow => return regexp_overflow(),
            _ => {}
        }
        {
            let view = self.heap.str_view(flat).expect("flat string");
            out.range(view, next_pos.min(size), size);
        }
        re::set_last_index(self.heap, r, Value::int(0))?;
        self.fuel -= length_fuel(out.len() as u32);
        let result = out.into_string(self.heap)?;
        self.sp = base;
        self.push(Value::string(result));
        self.pc = next;
        Ok(None)
    }
}

fn regexp_overflow<T>() -> Result<T, Fault> {
    Err(Fault::error(
        ErrorKind::InternalError,
        String::from("out of memory in regexp execution"),
    ))
}
