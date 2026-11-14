//! Stack traces (D1282, D2030, D2050): the frames of the current call, as the default
//! `stack` string in QuickJS's format or, with `Error.prepareStackTrace` set, as the
//! CallSite objects it is called with.
//!
//! The VM cannot call JS while it creates an error, so with a prepare function a new
//! error's stack is first a *pending* record: an internal Record of `[callsites, default
//! string, prepare function, Error constructor]`. The prelude finishes it
//! (`%FinishStack`, builtins/error.ts): the Error constructors and
//! `Error.captureStackTrace` at once, the VM's own errors when they are thrown (the
//! throw calls `%ThrowWithStack`, see `Exec::throw_fault`), and any other error when its
//! `stack` is first read. Rust readers (`Vm::error_stack`, the REPL) see the default
//! string of a pending record.
use crate::PushOut;

use alloc::vec::Vec;

use super::*;
use crate::heap::atoms;
use crate::heap::cells::Script;
use crate::heap::cells::flags::OBJ_BUILTIN;
use crate::value::Str;

/// A name or file of a trace frame, kept where it is until it is written out.
enum Text {
    Str(Str),
    Atom(Atom),
    Lit(&'static str),
}

/// One frame of a trace.
struct TraceFrame {
    /// The function; `None` for the position of a parse error (`<input>`).
    func: Option<Obj>,
    /// The name shown; `None` for an anonymous function.
    name: Option<Text>,
    /// The file; `None` for a native frame.
    file: Option<Text>,
    /// Line and column.
    pos: Option<(u32, u32)>,
    native: bool,
    construct: bool,
}

impl Text {
    /// Appends the text to `out`.
    fn put(&self, heap: &Heap, out: &mut String) {
        match *self {
            Text::Str(s) => heap.push_str_to(s, out),
            Text::Atom(a) => match heap.atom_str_value(a) {
                Some(s) => heap.push_str_to(s, out),
                None => out.push_str(&heap.atom_name(a)),
            },
            Text::Lit(t) => out.push_str(t),
        }
    }

    /// The text as a JS string.
    fn value(&self, heap: &mut Heap) -> Result<Value, HeapError> {
        let s = match *self {
            Text::Str(s) => s,
            Text::Atom(a) => match heap.atom_str_value(a) {
                Some(s) => s,
                None => heap.alloc_str(&heap.atom_name(a))?,
            },
            Text::Lit(t) => heap.alloc_str(t)?,
        };
        Ok(Value::string(s))
    }
}

/// The bytes a trace line usually takes, reserved per frame for the default `stack` string.
const TRACE_LINE: usize = 64;

/// Slots of a pending stack record.
const PENDING_SITES: u32 = 0;
const PENDING_DEFAULT: u32 = 1;
const PENDING_PREPARE: u32 = 2;
const PENDING_CTOR: u32 = 3;
const PENDING_SLOTS: u32 = 4;

/// Whether `v` is a pending stack record (a stack slot holds nothing else of class Record).
pub(crate) fn is_pending_stack(heap: &Heap, v: Value) -> bool {
    v.as_object().is_some_and(|o| {
        heap.object_class(o) == ObjectClass::Record && heap.object_slot_count(o) == PENDING_SLOTS
    })
}

/// A stack value as Rust readers show it: the default string of a pending record.
pub(crate) fn settled_stack(heap: &Heap, v: Value) -> Value {
    match v.as_object() {
        Some(o) if is_pending_stack(heap, v) => heap.object_slot(o, PENDING_DEFAULT),
        _ => v,
    }
}

impl Exec<'_> {
    /// The frames of a trace, innermost first, at most `limit` (with the `<input>`
    /// position of a parse error first, counting as one).
    /// `skip_prelude`: leave out the prelude frames at the top (the builtin asking).
    /// `below`: leave out every frame up to and including the topmost call of that
    /// function (Error.captureStackTrace's constructorOpt, D2010); no frames if it is not
    /// on the stack.
    fn trace_frames(
        &self,
        pc: u32,
        skip_prelude: bool,
        below: Option<Obj>,
        limit: u32,
        input: Option<(u32, u32)>,
    ) -> Vec<TraceFrame> {
        let mut out = Vec::new();
        if limit == 0 {
            return out;
        }
        if let Some(pos) = input {
            out.push_out(TraceFrame {
                func: None,
                name: None,
                file: Some(Text::Lit("<input>")),
                pos: Some(pos),
                native: false,
                construct: false,
            });
        }
        let rest = limit - out.len() as u32;
        let mut raw = Vec::new();
        self.raw_frames(pc, skip_prelude, below, rest, &mut raw);
        for r in raw {
            out.push_out(describe_frame(self.heap, r));
        }
        out
    }

    /// The frames a trace shows (`trace_frames` without a parse error's position), before
    /// they are described.
    fn raw_frames(
        &self,
        pc: u32,
        mut skip_prelude: bool,
        mut below: Option<Obj>,
        limit: u32,
        out: &mut Vec<RawFrame>,
    ) {
        if limit == 0 || !self.has_frame() {
            return;
        }
        out.reserve(limit.min(RAW_RESERVE) as usize);
        let prelude = intrinsic(self.heap, Intrinsic::PreludeScript).as_internal();
        let (mut bp, mut hp, mut pc) = (self.bp, self.hp, pc);
        loop {
            let f = self.get(bp).as_object().expect("frame callee");
            let [prev_bp, prev_hp, ret_pc, _, flags] = self
                .heap
                .arena()
                .read_values(ValueArray::VALUES.at(self.stack, hp + frame::PREV_BP));
            let flags = flags.as_int().unwrap_or(0);
            let in_prelude = in_prelude(self.heap, f, prelude);
            skip_prelude &= in_prelude;
            let skipping = below.is_some();
            if below == Some(f) {
                below = None;
            }
            if !skip_prelude
                && !skipping
                && (!in_prelude || self.heap.cell_flags(f.offset()) & OBJ_BUILTIN != 0)
            {
                out.push_out((f, pc, flags & frame::FLAG_CONSTRUCT != 0));
            }
            let cont = Continuation::from_flags(flags);
            if cont.is_bottom() || out.len() as u32 >= limit {
                break;
            }
            let ret_pc = ret_pc.as_int().unwrap_or(1) as u32;
            pc = if cont.is_retry() {
                ret_pc
            } else {
                ret_pc.saturating_sub(1)
            };
            hp = prev_hp.as_int().unwrap_or(0) as u32;
            bp = prev_bp.as_int().unwrap_or(0) as u32;
        }
        if below.is_some() {
            // constructorOpt was not on the stack.
            out.clear();
        }
    }

    /// The default `stack` string: one `    at name (file:line:col)` line per frame.
    fn format_trace(&self, frames: &[TraceFrame]) -> String {
        format_trace(self.heap, frames)
    }

    /// The stack trace in QuickJS's format: one `    at name (file:line:col)` line per
    /// frame, innermost first, at most `limit` lines (`Error.stackTraceLimit`, D2030).
    /// Callers are positioned at their call (`ret_pc - 1`). Prelude frames are hidden,
    /// except installed builtin methods, which show as `at name (native)`.
    pub(crate) fn stack_trace(&self, pc: u32, limit: u32) -> String {
        self.format_trace(&self.trace_frames(pc, false, None, limit, None))
    }

    /// The `Error.prepareStackTrace` function and the Error constructor to call it with,
    /// if it applies now (qjs's js_get_prepare_stack_trace): an own data property of the
    /// realm's Error constructor (read without running code) that is callable, with JS
    /// frames on the stack and no call of it running already.
    fn prepare_stack_trace(&mut self, ctor: Option<Obj>) -> Option<(Value, Value)> {
        if !self.has_frame() {
            return None;
        }
        let ctor = ctor?;
        self.heap
            .own_entry_index(ctor, atoms::PREPARE_STACK_TRACE)?;
        let f = match self.heap.get_own_property(ctor, atoms::PREPARE_STACK_TRACE) {
            Some(Property::Data { value, .. }) => value,
            _ => return None,
        };
        if !f.as_object().is_some_and(|o| self.heap.is_callable(o)) {
            return None;
        }
        // Errors made while it runs get the default trace (qjs's in_prepare_stack_trace).
        let id = const { crate::bytecode::intrinsics::const_id("CallPrepareStackTrace") };
        let running = realm::installed_intrinsic(self.heap, id);
        let (mut bp, mut hp) = (self.bp, self.hp);
        loop {
            if self.get(bp) == running {
                return None;
            }
            let flags = self.get(hp + frame::FLAGS).as_int().unwrap_or(0);
            if Continuation::from_flags(flags).is_bottom() {
                break;
            }
            let prev_bp = self.get(hp + frame::PREV_BP).as_int().unwrap_or(0) as u32;
            hp = self.get(hp + frame::PREV_HP).as_int().unwrap_or(0) as u32;
            bp = prev_bp;
        }
        Some((f, Value::object(ctor)))
    }

    /// The `stack` of a new error from the current frames (the innermost at `pc`): the
    /// default string, or with `Error.prepareStackTrace` a pending record (module doc).
    /// `skip_prelude` and `below` as for [`Exec::trace_frames`]; `input` the position of
    /// a parse error in dynamic code (qjs's first `<input>` frame).
    pub(crate) fn new_stack(
        &mut self,
        pc: u32,
        skip_prelude: bool,
        below: Option<Obj>,
        input: Option<(u32, u32)>,
    ) -> Result<Value, HeapError> {
        let ctor = realm::error_constructor_cached(self.heap);
        let limit = realm::stack_trace_limit_cached(self.heap, ctor);
        let prepare = self.prepare_stack_trace(ctor);
        let frames = self.trace_frames(pc, skip_prelude, below, limit, input);
        let default = Value::string(self.heap.alloc_str(&self.format_trace(&frames))?);
        let Some((f, ctor)) = prepare else {
            return Ok(default);
        };
        let proto = intrinsic_obj(self.heap, Intrinsic::CallSiteProto);
        let mut sites = crate::buf::vec_with_capacity(frames.len());
        for fr in &frames {
            let site = self.heap.alloc_object(
                ObjectClass::CallSite,
                Some(proto),
                ObjectClass::CallSite.slots(),
            )?;
            let str_or_null = |heap: &mut Heap, s: &Option<Text>| -> Result<Value, HeapError> {
                match s {
                    Some(s) => s.value(heap),
                    None => Ok(Value::NULL),
                }
            };
            let name = str_or_null(self.heap, &fr.name)?;
            let file = str_or_null(self.heap, &fr.file)?;
            let (line, col) = fr.pos.unwrap_or((0, 0));
            let func = fr.func.map_or(Value::NULL, Value::object);
            for (i, v) in [
                func,
                name,
                file,
                Value::int(line as i32),
                Value::int(col as i32),
                Value::bool(fr.native),
                Value::bool(fr.construct),
            ]
            .into_iter()
            .enumerate()
            {
                self.heap.set_object_slot(site, i as u32, v);
            }
            sites.push_out(Value::object(site));
        }
        let array_proto = intrinsic_obj(self.heap, Intrinsic::ArrayProto);
        let sites = self.heap.new_array_from_values(Some(array_proto), &sites)?;
        let r = self
            .heap
            .alloc_object(ObjectClass::Record, None, PENDING_SLOTS)?;
        self.heap
            .set_object_slot(r, PENDING_SITES, Value::object(sites));
        self.heap.set_object_slot(r, PENDING_DEFAULT, default);
        self.heap.set_object_slot(r, PENDING_PREPARE, f);
        self.heap.set_object_slot(r, PENDING_CTOR, ctor);
        Ok(Value::object(r))
    }

    /// The `stack` of a new error from the current frames, recorded lazily (D1282): the
    /// frames' functions and pcs, formatted when the stack is first read
    /// (`lazy_stack_text`). With `Error.prepareStackTrace`, as `new_stack`.
    pub(crate) fn capture_stack(
        &mut self,
        pc: u32,
        skip_prelude: bool,
    ) -> Result<Value, HeapError> {
        let ctor = realm::error_constructor_cached(self.heap);
        if self.prepare_stack_trace(ctor).is_some() {
            return self.new_stack(pc, skip_prelude, None, None);
        }
        let limit = realm::stack_trace_limit_cached(self.heap, ctor);
        let mut frames = core::mem::take(&mut self.heap.frame_scratch);
        frames.clear();
        self.raw_frames(pc, skip_prelude, None, limit, &mut frames);
        let arr = self.frames_array(&frames);
        self.heap.frame_scratch = frames;
        arr
    }

    fn frames_array(&mut self, frames: &[RawFrame]) -> Result<Value, HeapError> {
        let arr = self.heap.alloc_value_array(2 * frames.len() as u32)?;
        for (i, &(f, pc, construct)) in frames.iter().enumerate() {
            let i = 2 * i as u32;
            self.heap.value_array_set(arr, i, Value::object(f));
            self.heap.value_array_set(
                arr,
                i + 1,
                Value::int((pc * 2 + u32::from(construct)) as i32),
            );
        }
        self.heap.arena_mut().set(
            arr,
            crate::heap::cells::ValueArray::LEN,
            2 * frames.len() as u32,
        );
        Ok(Value::internal(arr))
    }

    /// Records the stack of the error `e` from the current frames, the innermost at `pc`.
    pub(crate) fn set_stack(&mut self, e: Value, pc: u32) -> Result<(), HeapError> {
        self.set_stack_at(e, pc, None)
    }

    /// [`Exec::set_stack`], with the position of a parse error in dynamic code.
    pub(crate) fn set_stack_at(
        &mut self,
        e: Value,
        pc: u32,
        input: Option<(u32, u32)>,
    ) -> Result<(), HeapError> {
        let Some(o) = e.as_object() else {
            return Ok(());
        };
        let v = match input {
            Some(_) => self.new_stack(pc, false, None, input)?,
            None => self.capture_stack(pc, false)?,
        };
        realm::set_error_stack(self.heap, o, v)
    }

    /// Throws a fault at `pc` of the current frame. A thrown error whose stack is pending
    /// (D2050) first goes through `%ThrowWithStack(e)`, called as if from the faulting
    /// instruction, which runs `Error.prepareStackTrace` and throws `e` from there: the
    /// handlers that apply are those of `pc`, as for a direct throw.
    pub(super) fn throw_fault(&mut self, f: Fault, pc: u32) -> Option<Status> {
        let exc = self.fault_value(f, Some(pc));
        if is_pending_stack(self.heap, realm::error_stack_raw(self.heap, exc)) {
            let id = const { crate::bytecode::intrinsics::const_id("ThrowWithStack") };
            let throw = realm::installed_intrinsic(self.heap, id);
            let base = self.sp;
            let called = self.ensure(base + 3).and_then(|()| {
                self.push(throw);
                self.push(Value::UNDEFINED);
                self.push(exc);
                // `Retry`: unwinding back into this frame uses `pc` itself as the key.
                self.call(base, 1, Continuation::Retry(base), pc)
            });
            match called {
                Ok(_) => return None,
                Err(_) => {
                    // No room to call it: the default trace.
                    self.sp = base;
                    let v = realm::error_stack_slot(self.heap, exc);
                    if let Some(o) = exc.as_object() {
                        let _ = realm::set_error_stack(self.heap, o, v);
                    }
                }
            }
        }
        self.throw_value(exc, pc)
    }
}

/// The default `stack` string of `frames`: one `    at name (file:line:col)` line per frame.
fn format_trace(heap: &Heap, frames: &[TraceFrame]) -> String {
    let put = |t: &Option<Text>, missing: &'static str, out: &mut String| match t {
        Some(t) => t.put(heap, out),
        None => out.push_str(missing),
    };
    let mut out = String::with_capacity(frames.len() * TRACE_LINE);
    for fr in frames {
        out.push_str("    at ");
        match (&fr.func, &fr.file, fr.pos) {
            (None, file, Some((l, c))) => {
                put(file, "", &mut out);
                crate::text::cat_to!(&mut out, ":", l, ":", c, "\n")
            }
            (_, None, _) => {
                put(&fr.name, "", &mut out);
                out.push_str(" (native)\n");
            }
            (_, file, pos) => {
                put(&fr.name, "<anonymous>", &mut out);
                out.push_str(" (");
                put(file, "", &mut out);
                match pos {
                    Some((l, c)) => crate::text::cat_to!(&mut out, ":", l, ":", c, ")\n"),
                    None => out.push_str(")\n"),
                }
            }
        }
    }
    out
}
/// The frames [`Exec::raw_frames`] reserves room for: the default `Error.stackTraceLimit`.
const RAW_RESERVE: u32 = 10;

/// A frame a trace shows: the function, the pc and whether it was called with `new`.
type RawFrame = (Obj, u32, bool);

/// Whether `f` is a prelude function: a privileged template (D1980), or the prelude's
/// script (`prelude`).
fn in_prelude(heap: &Heap, f: Obj, prelude: Option<u32>) -> bool {
    let t = heap.closure_template(f);
    u32::from(heap.arena().get::<u16>(t, FuncTemplate::FLAGS)) & func_flags::PRIVILEGED != 0
        || (prelude.is_some() && prelude == crate::bytecode::template::debug_script(heap, t).get())
}

/// What a trace shows for a frame of `raw_frames`.
fn describe_frame(heap: &Heap, (f, pc, construct): RawFrame) -> TraceFrame {
    let t = heap.closure_template(f);
    let a = heap.arena();
    if heap.cell_flags(f.offset()) & OBJ_BUILTIN != 0 {
        // The installed name, not the prelude's internal function name.
        let name = heap
            .get_own_property(f, crate::heap::atoms::NAME)
            .and_then(|p| p.value())
            .and_then(|v| v.as_string())
            .filter(|&s| heap.str_len(s) > 0)
            .map(Text::Str);
        return TraceFrame {
            func: Some(f),
            name,
            file: None,
            pos: None,
            native: true,
            construct,
        };
    }
    let debug = crate::bytecode::template::debug_info(heap, t);
    let script = debug.script;
    let kind = func_flags::kind(u32::from(a.get::<u16>(t, FuncTemplate::FLAGS)));
    // A module body has no name (`<anonymous>`, as qjs prints it); a Script's top level
    // and a codemode body are `<eval>`.
    let module = crate::MODULES
        && kind == func_flags::KIND_SCRIPT
        && script
            .get()
            .is_some_and(|s| a.get::<u32>(s, Script::OPTIONS) & 3 == 3);
    let name = if module {
        None
    } else if matches!(kind, func_flags::KIND_SCRIPT | func_flags::KIND_ASYNC_BODY) {
        Some(Text::Lit("<eval>"))
    } else {
        let name = Atom::from_id(a.get(t, FuncTemplate::NAME));
        (!name.is_null() && name != atoms::EMPTY).then_some(Text::Atom(name))
    };
    let file = script
        .get()
        .and_then(|s| a.get(s, Script::NAME).get())
        .map_or(Text::Lit(""), |n| Text::Str(Str(n)));
    TraceFrame {
        func: Some(f),
        name,
        file: Some(file),
        pos: crate::bytecode::lookup_line(
            crate::bytecode::template::bytes_of(heap, debug.lines),
            pc,
        ),
        native: false,
        construct,
    }
}

/// The text of a stack recorded lazily (`Exec::capture_stack`, an internal ValueArray of
/// function and `pc * 2 + construct` pairs), or `None` for any other value.
pub(crate) fn lazy_stack_text(heap: &Heap, v: Value) -> Option<String> {
    let arr = v.as_internal()?;
    let frames: Vec<TraceFrame> = (0..heap.value_array_len(arr) / 2)
        .map(|i| {
            let f = heap
                .value_array_get(arr, 2 * i)
                .as_object()
                .expect("trace function");
            let code = heap.value_array_get(arr, 2 * i + 1).as_int().unwrap_or(0) as u32;
            describe_frame(heap, (f, code >> 1, code & 1 != 0))
        })
        .collect();
    Some(format_trace(heap, &frames))
}

/// Reads slot `i` of a CallSite (`%CallSiteSlot`): TypeError for anything else.
pub(crate) fn callsite_slot(heap: &Heap, v: Value, i: u32) -> Result<Value, Fault> {
    match v.as_object() {
        Some(o) if heap.object_class(o) == ObjectClass::CallSite && i < 7 => {
            Ok(heap.object_slot(o, i))
        }
        _ => type_error("CallSite object expected"),
    }
}
