//! Heap helpers for compiled code cells (`FuncTemplate`, `FuncInfo`, `Bytes`) and the
//! template disassembler. The cell layouts live in `heap/cells.rs`.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

use super::disasm::{DisasmContext, disasm};
use super::handlers::{Handler, HandlerTable, write_handlers};
use super::stack::{StackError, StackScratch, check_stack_in};
use crate::heap::cells::{Bytes, CellKind, FuncInfo, FuncTemplate, Script, U32Array, ValueArray};
use crate::heap::{Atom, Heap, HeapError, Ref};
use crate::value::{Str, Value};

/// `FuncTemplate::FLAGS`: the function kind in the low 4 bits, then flags.
pub mod func_flags {
    pub const KIND_MASK: u32 = 0xF;
    /// A Script's top level (D004): called with `this` = the global object.
    pub const KIND_SCRIPT: u32 = 0;
    /// A codemode body (D008): `(async (params) => { body })`, called with `this` = the
    /// global object and the host's arguments.
    pub const KIND_ASYNC_BODY: u32 = 1;
    pub const KIND_NORMAL: u32 = 2;
    pub const KIND_ARROW: u32 = 3;
    pub const KIND_METHOD: u32 = 4;
    pub const KIND_GETTER: u32 = 5;
    pub const KIND_SETTER: u32 = 6;
    pub const KIND_CLASS_CTOR: u32 = 7;
    pub const KIND_DERIVED_CTOR: u32 = 8;
    pub const KIND_FIELD_INIT: u32 = 9;
    pub const ASYNC: u32 = 1 << 4;
    pub const GENERATOR: u32 = 1 << 5;
    /// The closure has a home object slot after its captures (methods that use `super`
    /// properties, directly or in arrows; class constructors and field initializers).
    pub const HOME: u32 = 1 << 6;
    /// Compiled in privileged mode (the prelude): closures get no `length`, `name` or
    /// `prototype`, since the `%Install*` intrinsics define them on the builtins that
    /// user code can see (D1433).
    pub const PRIVILEGED: u32 = 1 << 8;
    /// D015: a function from a Function constructor whose body has no "use strict":
    /// OrdinaryCallBindThis is sloppy (undefined/null become the global object,
    /// primitives their wrappers). Nothing else of sloppy mode.
    pub const SLOPPY_THIS: u32 = 1 << 9;
    /// 0063: a `@native` builtin; `FuncTemplate::NATIVE` holds its index into the
    /// generated table (`builtins::gen`), set by `%MarkNative`. With
    /// `ExecOptions::native_builtins` its calls get native frames.
    pub const NATIVE: u32 = 1 << 10;
    /// D1850: a builtin with a leaf fast path; `FuncTemplate::FAST` holds its index into
    /// `builtins::fast::FAST`, set by `%MarkFast`. With `ExecOptions::native_builtins`
    /// a call tries the fast path first and runs the builtin only when it declines.
    pub const FAST: u32 = 1 << 11;
    /// D2445: a privileged template whose `NAME` and `LENGTH` are its builtin closure's
    /// lazy `name` and `length` (set at base-image build time): the base keeps the name.
    pub const LAZY_NAME: u32 = 1 << 12;

    pub const fn kind(flags: u32) -> u32 {
        flags & KIND_MASK
    }
}

/// The parts of a function template, for [`alloc_template`].
#[derive(Clone, Copy, Debug)]
pub struct TemplateParts<'a> {
    pub flags: u32,
    pub name: Atom,
    pub params: u16,
    pub length: u16,
    pub locals: u16,
    /// Capture descriptors (`ops::capture_desc`), one per capture.
    pub captures: &'a [u32],
    /// Final code (labels resolved).
    pub code: &'a [u8],
    pub consts: &'a [Value],
    /// Child templates.
    pub children: &'a [Ref],
    pub handlers: &'a [Handler],
    /// An encoded line table (`LineTableBuilder::finish`).
    pub lines: &'a [u8],
    /// Local names, then capture names; may be empty.
    pub names: &'a [Atom],
    pub script: Ref,
    pub src_start: u32,
    pub src_end: u32,
}

impl Default for TemplateParts<'_> {
    fn default() -> Self {
        TemplateParts {
            flags: 0,
            name: Atom::NULL,
            params: 0,
            length: 0,
            locals: 0,
            captures: &[],
            code: &[],
            consts: &[],
            children: &[],
            handlers: &[],
            lines: &[],
            names: &[],
            script: Ref::NULL,
            src_start: 0,
            src_end: 0,
        }
    }
}

/// Why [`alloc_template`] failed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TemplateError {
    Heap(HeapError),
    /// The code fails the stack checker (a compiler bug).
    Stack(StackError),
    /// More than `u16::MAX` captures.
    TooManyCaptures,
}

impl From<HeapError> for TemplateError {
    fn from(e: HeapError) -> Self {
        TemplateError::Heap(e)
    }
}

/// Checks the code with the stack checker (which also computes `STACK`) and writes a
/// `FuncTemplate` cell, its code, capture descriptors and handler table inline (D2444),
/// with its `ValueArray` of constants and a `FuncInfo` for the children and debug info.
/// Empty parts are stored as null.
pub fn alloc_template(heap: &mut Heap, p: &TemplateParts<'_>) -> Result<Ref, TemplateError> {
    alloc_template_in(heap, p, &mut StackScratch::default())
}

/// [`alloc_template`] with the stack checker's buffers reused (the compiler's loop over
/// the functions of a unit).
pub fn alloc_template_in(
    heap: &mut Heap,
    p: &TemplateParts<'_>,
    scratch: &mut StackScratch,
) -> Result<Ref, TemplateError> {
    let stack = check_stack_in(p.code, p.handlers, scratch).map_err(TemplateError::Stack)?;
    let cache_sites = &scratch.caches;
    let captures = u16::try_from(p.captures.len()).map_err(|_| TemplateError::TooManyCaptures)?;
    // The code, the capture descriptors and the handler records, written into the cell
    // (not gathered in a buffer first: a copy and two allocations per function).
    let handlers_at = p.code.len() + 4 * p.captures.len();
    let code_len = u32::try_from(p.code.len()).map_err(|_| HeapError::OutOfMemory)?;
    let data_len =
        u32::try_from(handlers_at + 16 * p.handlers.len()).map_err(|_| HeapError::OutOfMemory)?;
    let consts = nonempty(p.consts, |v| alloc_value_array(heap, v))?;
    let info = alloc_info(heap, p)?;
    let size = FuncTemplate::DATA
        .cell_size(data_len)
        .ok_or(HeapError::OutOfMemory)?;
    let off = heap.alloc_cell(CellKind::FuncTemplate, size)?;
    let a = heap.arena_mut();
    // Every flag is below bit 16 (D2448: a u16 field).
    debug_assert!(p.flags <= u32::from(u16::MAX));
    a.set(off, FuncTemplate::FLAGS, p.flags as u16);
    a.set(off, FuncTemplate::NAME, p.name.id());
    a.set(off, FuncTemplate::PARAMS, p.params);
    a.set(off, FuncTemplate::LENGTH, p.length);
    a.set(off, FuncTemplate::LOCALS, p.locals);
    a.set(off, FuncTemplate::STACK, stack);
    a.set(off, FuncTemplate::CAPTURES, captures);
    a.set(off, FuncTemplate::CODE_LEN, code_len);
    a.set(off, FuncTemplate::CONSTS, consts);
    a.set(off, FuncTemplate::INFO, info);
    let data = a.slice_mut(FuncTemplate::DATA.at(off, 0), data_len);
    let (code, rest) = data.split_at_mut(p.code.len());
    code.copy_from_slice(p.code);
    let ic_count = number_caches(code, cache_sites);
    let (words, records) = rest.split_at_mut(4 * p.captures.len());
    for (c, w) in p.captures.iter().zip(words.chunks_exact_mut(4)) {
        w.copy_from_slice(&c.to_le_bytes());
    }
    write_handlers(p.handlers, records);
    a.set(off, FuncTemplate::IC_COUNT, ic_count);
    Ok(Ref(off))
}

/// The `FuncInfo` of a template with parts `p`, or null when it needs none: a privileged
/// (prelude) function has no debug info, since its frames are hidden or `(native)` in
/// traces, its source is `[native code]`, and its locals are never named (D1980); it
/// usually has no children either.
fn alloc_info(heap: &mut Heap, p: &TemplateParts<'_>) -> Result<Ref, HeapError> {
    let debug = p.flags & func_flags::PRIVILEGED == 0 || !p.lines.is_empty() || !p.names.is_empty();
    if !debug && p.children.is_empty() {
        return Ok(Ref::NULL);
    }
    let (script, src_start, src_end) = if debug {
        (p.script, p.src_start, p.src_end)
    } else {
        (Ref::NULL, 0, 0)
    };
    let lines = nonempty(p.lines, |l| alloc_bytes(heap, l))?;
    let names = nonempty(p.names, |n| alloc_u32_array(heap, n))?;
    let n = u32::try_from(p.children.len()).map_err(|_| HeapError::OutOfMemory)?;
    let size = FuncInfo::CHILDREN
        .cell_size(n)
        .ok_or(HeapError::OutOfMemory)?;
    let off = heap.alloc_cell(CellKind::FuncInfo, size)?;
    let a = heap.arena_mut();
    a.set(off, FuncInfo::SCRIPT, script);
    a.set(off, FuncInfo::SRC_START, src_start);
    a.set(off, FuncInfo::SRC_END, src_end);
    a.set(off, FuncInfo::LINES, lines);
    a.set(off, FuncInfo::NAMES, names);
    a.set(off, FuncInfo::CHILD_COUNT, n);
    for (i, &c) in p.children.iter().enumerate() {
        a.write(FuncInfo::CHILDREN.at(off, i as u32), c);
    }
    Ok(Ref(off))
}

/// The bytecode of template `t`.
pub fn code(heap: &Heap, t: u32) -> &[u8] {
    let a = heap.arena();
    a.slice(
        FuncTemplate::DATA.at(t, 0),
        a.get(t, FuncTemplate::CODE_LEN),
    )
}

/// Capture descriptor `i` of template `t` (`ops::capture_desc`), after the code.
pub fn capture_desc(heap: &Heap, t: u32, i: u32) -> u32 {
    let a = heap.arena();
    a.read(FuncTemplate::DATA.at(t, a.get(t, FuncTemplate::CODE_LEN) + 4 * i))
}

/// The encoded handler table of template `t` (`bytecode::handlers`): the 16-byte
/// records after the code and the capture descriptors, to the end of the cell. The
/// cell's padding is shorter than a record, so the division finds the count.
pub fn handlers(heap: &Heap, t: u32) -> &[u8] {
    let a = heap.arena();
    let at = a.get(t, FuncTemplate::CODE_LEN) + 4 * a.get(t, FuncTemplate::CAPTURES) as u32;
    let start = FuncTemplate::DATA.at(t, at);
    let end = t + heap.cell_size(t);
    a.slice(start, (end - start) / 16 * 16)
}

/// The number of child templates of template `t`.
pub fn child_count(heap: &Heap, t: u32) -> u32 {
    let a = heap.arena();
    a.get::<Ref>(t, FuncTemplate::INFO)
        .get()
        .map_or(0, |i| a.get(i, FuncInfo::CHILD_COUNT))
}

/// Child template `i` of template `t` (`fclosure i`), if it has one.
pub fn child(heap: &Heap, t: u32, i: u32) -> Option<Ref> {
    let a = heap.arena();
    let info = a.get::<Ref>(t, FuncTemplate::INFO).get()?;
    (i < a.get(info, FuncInfo::CHILD_COUNT)).then(|| a.read(FuncInfo::CHILDREN.at(info, i)))
}

/// The debug info of template `t` (D1980): null and 0 for a privileged one, which has
/// none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DebugInfo {
    pub lines: Ref,
    pub script: Ref,
    pub src_start: u32,
    pub src_end: u32,
    pub names: Ref,
}

pub fn debug_script(heap: &Heap, t: u32) -> Ref {
    let a = heap.arena();
    match a.get::<Ref>(t, FuncTemplate::INFO).get() {
        Some(info) => a.get(info, FuncInfo::SCRIPT),
        None => Ref::NULL,
    }
}

pub fn debug_info(heap: &Heap, t: u32) -> DebugInfo {
    let a = heap.arena();
    let Some(info) = a.get::<Ref>(t, FuncTemplate::INFO).get() else {
        return DebugInfo {
            lines: Ref::NULL,
            script: Ref::NULL,
            src_start: 0,
            src_end: 0,
            names: Ref::NULL,
        };
    };
    DebugInfo {
        lines: a.get(info, FuncInfo::LINES),
        script: a.get(info, FuncInfo::SCRIPT),
        src_start: a.get(info, FuncInfo::SRC_START),
        src_end: a.get(info, FuncInfo::SRC_END),
        names: a.get(info, FuncInfo::NAMES),
    }
}

/// The `Cache` immediate of a function with more cache sites than slots (D1620).
pub const NO_CACHE: u16 = u16::MAX;

/// Numbers the `Cache` immediates of `code` at `sites` (in code order, D1620); returns
/// the count. Sites beyond `NO_CACHE - 1` get `NO_CACHE` (never cached).
fn number_caches(code: &mut [u8], sites: &[u32]) -> u16 {
    let mut n: u16 = 0;
    for &at in sites {
        let idx = if n < NO_CACHE { n } else { NO_CACHE };
        code[at as usize..at as usize + 2].copy_from_slice(&idx.to_le_bytes());
        n = n.saturating_add(1);
    }
    n
}

fn nonempty<T>(
    items: &[T],
    f: impl FnOnce(&[T]) -> Result<Ref, HeapError>,
) -> Result<Ref, HeapError> {
    if items.is_empty() {
        Ok(Ref::NULL)
    } else {
        f(items)
    }
}

/// Allocates a `Script` cell; `options` are `CompileOptions::bits`, `params` an async
/// body's parameter names joined by `,`.
pub fn alloc_script(
    heap: &mut Heap,
    name: Str,
    source: Option<&[u8]>,
    options: u32,
    params: Option<Str>,
) -> Result<Ref, HeapError> {
    let source = match source {
        Some(s) => alloc_bytes(heap, s)?,
        None => Ref::NULL,
    };
    let off = heap.alloc_cell(CellKind::Script, Script::LAYOUT.min_size)?;
    let a = heap.arena_mut();
    a.set(off, Script::OPTIONS, options);
    a.set(
        off,
        Script::PARAMS,
        params.map_or(Ref::NULL, |s| Ref(s.offset())),
    );
    a.set(off, Script::NAME, Ref(name.offset()));
    a.set(off, Script::SOURCE, source);
    Ok(Ref(off))
}

/// The source text of Script cell `script` as UTF-8 (D2992), or `None` when it keeps none.
pub fn script_source(heap: &Heap, script: u32) -> Option<&[u8]> {
    let r: Ref = heap.arena().get(script, Script::SOURCE);
    r.get()?;
    Some(bytes_of(heap, r))
}

/// [`script_source`] as a `String`.
pub fn script_text(heap: &Heap, script: u32) -> Option<alloc::string::String> {
    core::str::from_utf8(script_source(heap, script)?)
        .ok()
        .map(alloc::string::String::from)
}

fn alloc_value_array(heap: &mut Heap, values: &[Value]) -> Result<Ref, HeapError> {
    let len = u32::try_from(values.len()).map_err(|_| HeapError::OutOfMemory)?;
    let size = ValueArray::VALUES
        .cell_size(len)
        .ok_or(HeapError::OutOfMemory)?;
    let off = heap.alloc_cell(CellKind::ValueArray, size)?;
    let a = heap.arena_mut();
    a.set(off, ValueArray::LEN, len);
    for (i, &v) in values.iter().enumerate() {
        a.write(ValueArray::VALUES.at(off, i as u32), v);
    }
    Ok(Ref(off))
}

fn alloc_u32_array(heap: &mut Heap, words: &[Atom]) -> Result<Ref, HeapError> {
    let len = u32::try_from(words.len()).map_err(|_| HeapError::OutOfMemory)?;
    let size = U32Array::DATA
        .cell_size(len)
        .ok_or(HeapError::OutOfMemory)?;
    let off = heap.alloc_cell(CellKind::U32Array, size)?;
    let a = heap.arena_mut();
    a.set(off, U32Array::LEN, len);
    for (i, w) in words.iter().enumerate() {
        a.write(U32Array::DATA.at(off, i as u32), w.id());
    }
    Ok(Ref(off))
}

/// Allocates a `Bytes` cell holding `data`.
pub fn alloc_bytes(heap: &mut Heap, data: &[u8]) -> Result<Ref, HeapError> {
    let len = u32::try_from(data.len()).map_err(|_| HeapError::OutOfMemory)?;
    let size = Bytes::DATA.cell_size(len).ok_or(HeapError::OutOfMemory)?;
    let off = heap.alloc_cell(CellKind::Bytes, size)?;
    let a = heap.arena_mut();
    a.set(off, Bytes::LEN, len);
    a.slice_mut(Bytes::DATA.at(off, 0), len)
        .copy_from_slice(data);
    Ok(Ref(off))
}

/// The contents of a `Bytes` cell; empty for null.
pub fn bytes_of(heap: &Heap, r: Ref) -> &[u8] {
    match r.get() {
        None => &[],
        Some(off) => {
            debug_assert_eq!(heap.cell_kind(off), CellKind::Bytes);
            let len = heap.arena().get(off, Bytes::LEN);
            heap.arena().slice(Bytes::DATA.at(off, 0), len)
        }
    }
}

/// Template `t`'s constants.
pub fn consts(heap: &Heap, t: u32) -> Vec<Value> {
    value_array(heap, heap.arena().get(t, FuncTemplate::CONSTS))
}

fn value_array(heap: &Heap, r: Ref) -> Vec<Value> {
    let Some(off) = r.get() else {
        return Vec::new();
    };
    let len = heap.arena().get(off, ValueArray::LEN);
    (0..len)
        .map(|i| heap.arena().read(ValueArray::VALUES.at(off, i)))
        .collect()
}

fn u32_array(heap: &Heap, r: Ref) -> Vec<u32> {
    let Some(off) = r.get() else {
        return Vec::new();
    };
    let len = heap.arena().get(off, U32Array::LEN);
    (0..len)
        .map(|i| heap.arena().read(U32Array::DATA.at(off, i)))
        .collect()
}

/// Disassembles a function template and, after it, all its descendants: a header line
/// per function (name, sizes), the handler table, then the code with names and
/// constants resolved.
pub fn disasm_template(heap: &Heap, template: Ref) -> String {
    let mut out = String::new();
    // Explicit work list rather than recursion: templates nest as deep as the source.
    let mut work = alloc::vec![(template, String::from("0"))];
    while let Some((t, path)) = work.pop() {
        let Some(off) = t.get() else { continue };
        let a = heap.arena();
        let name = Atom::from_id(a.get(off, FuncTemplate::NAME));
        let locals = a.get(off, FuncTemplate::LOCALS);
        let consts = value_array(heap, a.get(off, FuncTemplate::CONSTS));
        let debug = debug_info(heap, off);
        let names: Vec<Atom> = u32_array(heap, debug.names)
            .into_iter()
            .map(Atom::from_id)
            .collect();
        let shown = if name.is_null() {
            String::from("<anonymous>")
        } else {
            heap.atom_name(name)
        };
        let _ = writeln!(
            out,
            "function {path} {shown}: params={} locals={locals} stack={} captures={}",
            a.get(off, FuncTemplate::PARAMS),
            a.get(off, FuncTemplate::STACK),
            a.get(off, FuncTemplate::CAPTURES),
        );
        if let Ok(h) = HandlerTable::new(handlers(heap, off)) {
            for h in h.iter() {
                let _ = writeln!(
                    out,
                    "  handler [{}, {}) -> {} depth {}",
                    h.start, h.end, h.target, h.depth
                );
            }
        }
        let cx = DisasmContext {
            heap: Some(heap),
            consts: &consts,
            names: &names,
            locals: locals as usize,
            phase1: false,
            lines: bytes_of(heap, debug.lines),
        };
        out.push_str(&disasm(code(heap, off), &cx));
        for i in (0..child_count(heap, off)).rev() {
            if let Some(c) = child(heap, off, i) {
                work.push((c, alloc::format!("{path}.{i}")));
            }
        }
    }
    out
}
