//! Resolution and template creation (docs/compiler.md 5.3 to 5.9, D1081).
//!
//! After the whole unit is parsed:
//!
//! 1. every name reference of every function is classified, children first: a binding
//!    of the function itself, a capture (the walk through the enclosing functions marks
//!    the binding captured and adds capture slots to every function in between), or a
//!    global; TDZ checks are decided (5.6);
//! 2. every binding gets storage: an argument slot, a local slot, a local slot holding a
//!    Box (captured bindings, D1083), or global;
//! 3. each function's phase-1 code is re-emitted into final code and written as a
//!    template, children before parents. The functions share one constant pool, the
//!    unit's (D2446).

use alloc::borrow::Cow;
use alloc::vec;
use alloc::vec::Vec;

use super::func::{Capture, FuncKind, FuncState, ScopeKind, VarId, VarKind};
use super::ts_decl::{DeclKind, EnumVal, TsDecl};
use super::{CompileOptions, PErr, PResult};
use crate::bytecode::global_decls::GlobalDecls;
use crate::bytecode::{
    CodeBuf, Label, LineTableBuilder, Op, TemplateError, TemplateParts, alloc_script,
    alloc_template_in, capture_desc, func_flags, special,
};
use crate::heap::cells::FuncTemplate;
use crate::heap::{Atom, Heap, Ref};
use crate::lexer::LineTable;
use crate::value::Value;

/// The constant pool of a compilation unit (D2446): every template of the unit refers to
/// the same `ValueArray`, so a string or number used by many functions (an error message,
/// a property name passed as a string) is stored once, and the unit pays one cell header.
/// Strings are shared by content, numbers by bits; the runs of a tagged template site
/// and the global declaration list stay private and contiguous.
#[derive(Default)]
struct UnitPool {
    values: Vec<Value>,
    /// Open addressing over the shared constants' hashes: `(hash, index + 1)`, index 0
    /// empty. The hash is kept so that growing the table hashes nothing again: a
    /// megabyte string constant (a dump's data) was hashed again at every doubling
    /// (0198). (A `BTreeMap` here cost 16 KB of wasm.)
    table: Vec<(u32, u32)>,
    shared: u32,
}

impl UnitPool {
    fn hash(heap: &Heap, v: Value) -> u32 {
        match v.as_string() {
            // A long flat string by its length and ~256 sampled units: a dump's megabyte
            // literal is compared on a match instead of hashed whole (0198).
            Some(s) if heap.str_len(s) > 1024 => match heap.str_view(s) {
                Some(view) => (0..view.len())
                    .step_by(view.len() / 256)
                    .fold(view.len() as u32, |h, i| {
                        (h ^ u32::from(view.get(i))).wrapping_mul(0x0100_0193)
                    }),
                None => heap.str_hash_uncached(s),
            },
            Some(s) => heap.str_hash_uncached(s),
            None => {
                let b = v.to_bits();
                ((b ^ (b >> 29)).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 32) as u32
            }
        }
    }

    fn same(heap: &Heap, a: Value, b: Value) -> bool {
        match (a.as_string(), b.as_string()) {
            (Some(x), Some(y)) => heap.str_eq(x, y),
            (None, None) => a.to_bits() == b.to_bits(),
            _ => false,
        }
    }

    /// The index of a shared constant (a string or a number).
    fn intern(&mut self, heap: &Heap, v: Value) -> u32 {
        if 2 * (self.shared as usize + 1) > self.table.len() {
            let mut bigger = vec![(0u32, 0u32); (self.table.len() * 2).max(64)];
            for &(h, e) in self.table.iter().filter(|e| e.1 != 0) {
                let mask = bigger.len() - 1;
                let mut b = h as usize & mask;
                while bigger[b].1 != 0 {
                    b = (b + 1) & mask;
                }
                bigger[b] = (h, e);
            }
            self.table = bigger;
        }
        let mask = self.table.len() - 1;
        let h = Self::hash(heap, v);
        let mut b = h as usize & mask;
        loop {
            match self.table[b] {
                (_, 0) => break,
                (h2, e) if h2 == h && Self::same(heap, self.values[e as usize - 1], v) => {
                    return e - 1;
                }
                _ => b = (b + 1) & mask,
            }
        }
        self.values.push(v);
        self.table[b] = (h, self.values.len() as u32);
        self.shared += 1;
        self.values.len() as u32 - 1
    }

    /// Appends private constants; returns the index of the first.
    fn append(&mut self, vs: &[Value]) -> u32 {
        let start = self.values.len() as u32;
        self.values.extend_from_slice(vs);
        start
    }
}

/// Where a binding lives.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Storage {
    /// Never referenced pseudo-bindings.
    None,
    Local(u16),
    /// A local slot holding the Box of a captured binding.
    Box(u16),
    Arg(u16),
    /// Script top-level declarations.
    Global,
    /// A module's top-level binding: capture `.0` of the module closure, its environment
    /// entry (compiler/module.rs).
    Env(u16),
    /// A prelude function in the realm's prelude function table, which has `len` entries
    /// (D2440).
    Builtin {
        idx: u16,
        len: u16,
    },
}

/// What a name reference resolved to.
#[derive(Clone, Copy, Debug)]
enum Target {
    Var(VarId),
    Capture(u16),
    /// A prelude function of function `func` (`Storage::Builtin`), from an inner one.
    Builtin {
        func: u32,
        var: VarId,
    },
    Global,
    /// A member of a TypeScript enum or namespace (compiler/ts_decl.rs): a property of
    /// the object that reference `.0` (to the closure's parameter) loads.
    Member(u32),
    /// Slot `.0` of the prelude function table (a resolved `Builtin`).
    Slot(u16),
}

#[derive(Clone, Copy, Debug)]
struct Resolved {
    target: Target,
    /// The binding is immutable (`const`, a named function expression's name).
    immutable: bool,
    /// A TDZ check is needed (5.6).
    check: bool,
    /// `let`/`const`: initialization of a global one is `init_global_lex`.
    lexical: bool,
    /// The binding holds this TypeScript enum or namespace (const enum folding).
    decl: Option<u32>,
}

pub(super) fn internal(what: &str) -> PErr {
    PErr::syntax(
        0,
        Cow::Owned(crate::text::cat!("internal compiler error: ", what)),
    )
}

/// Step 1 of [`finish`] alone: every function's free names ([`super::free_names`]), its
/// globals with those of the functions inside it. The global `undefined`, which reads
/// compile to `undefined`, is left out.
#[cfg(feature = "dump")]
pub(super) fn free_names(
    mut funcs: Vec<FuncState>,
    decls: &[TsDecl],
    privileged: bool,
    atom_undefined: Atom,
) -> Vec<super::FreeNames> {
    let mut resolved: Vec<Vec<Resolved>> = (0..funcs.len()).map(|_| Vec::new()).collect();
    for id in (0..funcs.len()).rev() {
        let mut r = 0;
        while r < funcs[id].refs.len() {
            let res = classify(&mut funcs, id, r, decls, privileged);
            resolved[id].push(res);
            r += 1;
        }
    }
    let mut globals: Vec<Vec<Atom>> = vec![Vec::new(); funcs.len()];
    // Children have higher ids than their parents: theirs are complete first.
    for id in (0..funcs.len()).rev() {
        let mut own = core::mem::take(&mut globals[id]);
        for (r, res) in resolved[id].iter().enumerate() {
            let a = funcs[id].refs[r].atom;
            if matches!(res.target, Target::Global) && a != atom_undefined && !own.contains(&a) {
                own.push(a);
            }
        }
        if let Some((p, _)) = funcs[id].parent {
            let up = &mut globals[p as usize];
            for &g in &own {
                if !up.contains(&g) {
                    up.push(g);
                }
            }
        }
        globals[id] = own;
    }
    funcs
        .iter()
        .zip(globals)
        .map(|(f, globals)| super::FreeNames {
            start: f.src_start,
            end: f.src_end,
            captures: f.capture_names.clone(),
            globals,
        })
        .collect()
}

/// How a unit is compiled (D2990).
#[derive(Clone, Copy, Debug)]
pub(super) struct Unit {
    /// Lazy functions get lazy templates (`lazy_roots`), compiled at their first call.
    #[cfg_attr(not(feature = "lazy"), allow(dead_code))]
    pub lazy: bool,
    /// A lazy function compiled on its own (compiler/lazy.rs): function 0 is the
    /// stand-in for its enclosing function and is not emitted, function 1 is the result.
    pub root: bool,
    /// With `root`: the Script, the source offset of the parsed text, and the line and
    /// column there.
    pub script: Ref,
    pub base: u32,
    pub line: u32,
    pub col: u32,
}

impl Unit {
    pub const EAGER: Unit = Unit {
        lazy: false,
        root: false,
        script: Ref::NULL,
        base: 0,
        line: 1,
        col: 1,
    };

    /// The source offset of the parsed text (0 without lazy functions).
    fn base(&self) -> u32 {
        if cfg!(feature = "lazy") { self.base } else { 0 }
    }

    /// The line and column of source offset `pos` of the parsed text.
    fn line_col(
        &self,
        lt: &LineTable,
        src: &[u8],
        pos: u32,
        cursor: &mut crate::lexer::LineCursor,
    ) -> (u32, u32) {
        let (line, col) = lt.line_col_from(src, pos, cursor);
        if !cfg!(feature = "lazy") {
            return (line, col);
        }
        let col = if line == 1 { col + self.col - 1 } else { col };
        (line + self.line - 1, col)
    }
}

/// The functions that compile lazily when the unit is lazy (D2990). Eligibility depends
/// only on a function's own text and its parent's, so the compile of an enclosing lazy
/// function decides the same as the whole unit's, and every eligible function gets a
/// constant pool of its own in every compile: its code is the same either way, which the
/// suspension maps of an eager compile rely on.
#[cfg(feature = "lazy")]
fn lazy_roots(
    funcs: &[FuncState],
    resolved: &[Vec<Resolved>],
    unit: &Unit,
    privileged: bool,
) -> Vec<bool> {
    let mut lazy = vec![false; funcs.len()];
    if privileged {
        return lazy;
    }
    for (id, f) in funcs.iter().enumerate().skip(if unit.root { 2 } else { 1 }) {
        let Some((p, _)) = f.parent else { continue };
        lazy[id] = f.lazy_ok
            && matches!(
                f.kind,
                FuncKind::Normal | FuncKind::Arrow | FuncKind::Method | FuncKind::Getter | FuncKind::Setter
            )
            // Called where it is defined, as far as the parent's text shows.
            && (f.call_end == 0 || f.call_end > funcs[p as usize].src_end)
            && !f.is_async && !f.is_generator;
    }
    // Names resolved through TypeScript declarations (const enums, namespace members)
    // need the unit's declarations: such a function and all around it stay eager.
    for (id, res) in resolved.iter().enumerate() {
        if res
            .iter()
            .any(|r| r.decl.is_some() || matches!(r.target, Target::Member(_)))
        {
            let mut at = Some(id as u32);
            while let Some(i) = at {
                lazy[i as usize] = false;
                at = funcs[i as usize].parent.map(|(p, _)| p);
            }
        }
    }
    lazy
}

/// Which functions a unit emits as code or as lazy templates, and their constant pools
/// (D2446): the top level has its own (it runs once, and its constants need not live as
/// long as the functions it creates), every lazy-eligible function one shared with the
/// functions inside it that are not (D2990), and the rest share pool 1. A lazily
/// compiled function (`Unit::root`, id 1) is in pool 1.
#[cfg(feature = "lazy")]
struct Plan {
    pool_of: Vec<usize>,
    pools: usize,
    /// 0 nothing (inside a lazy function, or the stand-in of a root), 1 a lazy template,
    /// 2 code.
    emit: Vec<u8>,
    /// The attributes of every capture (`capture_desc::IMMUTABLE`, `CHECKED`) of the
    /// lazy templates.
    cap_attrs: Vec<Vec<u32>>,
}

#[cfg(feature = "lazy")]
impl Plan {
    fn new(funcs: &[FuncState], resolved: &[Vec<Resolved>], unit: &Unit, privileged: bool) -> Plan {
        let eligible = lazy_roots(funcs, resolved, unit, privileged);
        let mut pools = 2;
        let mut pool_of = vec![0usize; funcs.len()];
        let mut emit = vec![2u8; funcs.len()];
        if unit.root {
            emit[0] = 0;
        }
        for id in 1..funcs.len() {
            let p = funcs[id].parent.map_or(0, |(p, _)| p as usize);
            if emit[p] < 2 && !(unit.root && id == 1) {
                emit[id] = 0;
            } else if eligible[id] && unit.lazy {
                emit[id] = 1;
            }
            pool_of[id] = if eligible[id] {
                pools += 1;
                pools - 1
            } else if p == 0 {
                1
            } else {
                pool_of[p]
            };
        }
        // Parents first: an `Outer` capture has its parent's attributes.
        let mut cap_attrs: Vec<Vec<u32>> = vec![Vec::new(); funcs.len()];
        for id in (1..funcs.len()).filter(|_| unit.lazy) {
            let Some((p, _)) = funcs[id].parent else {
                continue;
            };
            let p = p as usize;
            let attrs = funcs[id]
                .captures
                .iter()
                .map(|c| match *c {
                    Capture::Local(v) => {
                        let var = &funcs[p].vars[v as usize];
                        let mut a = 0;
                        if var.kind.is_immutable() {
                            a |= capture_desc::IMMUTABLE;
                        }
                        if var.kind.is_lexical()
                            || (var.kind == VarKind::Param && !funcs[p].simple_params)
                        {
                            a |= capture_desc::CHECKED;
                        }
                        a
                    }
                    Capture::Outer(n) => cap_attrs[p].get(n as usize).copied().unwrap_or(0),
                })
                .collect();
            cap_attrs[id] = attrs;
        }
        Plan {
            pool_of,
            pools,
            emit,
            cap_attrs,
        }
    }

    fn pools(&self) -> Vec<UnitPool> {
        (0..self.pools).map(|_| UnitPool::default()).collect()
    }

    fn pool(&self, id: usize) -> usize {
        self.pool_of[id]
    }
}

/// A lazy function's span and the globals it and the functions inside it read or write,
/// first use first, the function before those inside it (D2993).
#[cfg(feature = "dump")]
pub type LazyGlobals = (u32, u32, Vec<Atom>);

/// The [`LazyGlobals`] of every function `plan` writes as a lazy template.
#[cfg(all(feature = "dump", feature = "lazy"))]
fn lazy_globals(
    funcs: &[FuncState],
    resolved: &[Vec<Resolved>],
    plan: &Plan,
    atom_undefined: Atom,
    out: &mut Vec<LazyGlobals>,
) {
    for (id, f) in funcs.iter().enumerate() {
        if plan.emit[id] != 1 {
            continue;
        }
        let mut globals: Vec<Atom> = Vec::new();
        let mut stack = vec![id];
        while let Some(i) = stack.pop() {
            for (r, res) in resolved[i].iter().enumerate() {
                let a = funcs[i].refs[r].atom;
                if matches!(res.target, Target::Global)
                    && a != atom_undefined
                    && !globals.contains(&a)
                {
                    globals.push(a);
                }
            }
            stack.extend(funcs[i].children.iter().rev().map(|&c| c as usize));
        }
        out.push((f.src_start, f.src_end, globals));
    }
}

/// Without lazy functions: every function emitted, the top level in pool 0, the rest in
/// pool 1.
#[cfg(not(feature = "lazy"))]
struct Plan;

#[cfg(not(feature = "lazy"))]
impl Plan {
    fn new(_: &[FuncState], _: &[Vec<Resolved>], _: &Unit, _: bool) -> Plan {
        Plan
    }

    fn pools(&self) -> [UnitPool; 2] {
        [UnitPool::default(), UnitPool::default()]
    }

    fn pool(&self, id: usize) -> usize {
        usize::from(id != 0)
    }
}

/// Resolves all functions of the unit and writes their templates. `funcs` is indexed
/// by function id; id 0 is the top level.
#[allow(clippy::too_many_arguments)]
pub(super) fn finish(
    heap: &mut Heap,
    src: &[u8],
    opts: &CompileOptions<'_>,
    unit: &Unit,
    atom_undefined: Atom,
    mut funcs: Vec<FuncState>,
    decls: &[TsDecl],
    builtin_names: Option<&mut Vec<Atom>>,
    #[cfg(feature = "dump")] mut maps: Option<&mut Vec<super::smap::FunctionMap>>,
    #[cfg(feature = "dump")] globals: Option<&mut Vec<LazyGlobals>>,
) -> PResult<Ref> {
    // 1. Classify references, children first. (Member references add a reference to
    // their namespace parameter, classified in turn.)
    let mut resolved: Vec<Vec<Resolved>> = (0..funcs.len()).map(|_| Vec::new()).collect();
    for id in (0..funcs.len()).rev() {
        let mut r = 0;
        while r < funcs[id].refs.len() {
            let res = classify(&mut funcs, id, r, decls, opts.privileged);
            resolved[id].push(res);
            r += 1;
        }
    }

    // 2. Storage.
    let builtins = funcs
        .iter()
        .flat_map(|f| f.vars.iter())
        .filter(|v| v.builtin)
        .count();
    let builtins = u16::try_from(builtins).map_err(|_| internal("too many prelude functions"))?;
    let mut next_builtin = 0;
    // The debug name of construct temporaries (D2732), which bind integer atoms.
    let temp_name = if funcs.iter().any(|f| !f.temps.is_empty()) {
        heap.atom_str("<temp>")?
    } else {
        Atom::NULL
    };
    let storages: Vec<(Vec<Storage>, Vec<Atom>)> = funcs
        .iter()
        .map(|f| assign_storage(f, &mut next_builtin, builtins, temp_name))
        .collect();
    for (s, _) in &storages {
        if s.len() > u16::MAX as usize {
            return Err(PErr::syntax(0, Cow::Borrowed("too many local variables")));
        }
    }
    if let Some(names) = builtin_names {
        names.clear();
        for (f, (s, _)) in funcs.iter().zip(&storages) {
            for (v, st) in f.vars.iter().zip(s) {
                if let Storage::Builtin { .. } = st {
                    names.push(v.name);
                }
            }
        }
    }
    // References to another function's prelude function name its table slot.
    for res in resolved.iter_mut().flatten() {
        if let Target::Builtin { func, var } = res.target {
            match storages[func as usize].0[var as usize] {
                Storage::Builtin { idx, .. } => res.target = Target::Slot(idx),
                _ => return Err(internal("prelude function without a table slot")),
            }
        }
    }

    // 3. Templates, children first.
    let script = if unit.root {
        unit.script
    } else {
        let name = heap.alloc_str(opts.filename)?;
        // Privileged code (the prelude) keeps no source text: its functions print as
        // native code, and the text would be a quarter of the base image (D1340).
        // UTF-8, as compiled: a lazy function's text is a slice of it (D2992).
        let source = (!opts.privileged && core::str::from_utf8(src).is_ok()).then_some(src);
        let params = if opts.goal == super::Goal::AsyncBody && !opts.params.is_empty() {
            // (Not `join`: its generic copy is a kilobyte of wasm.)
            let mut joined = alloc::string::String::new();
            for (i, p) in opts.params.iter().enumerate() {
                if i > 0 {
                    joined.push(',');
                }
                joined.push_str(p);
            }
            Some(heap.alloc_str(&joined)?)
        } else {
            None
        };
        alloc_script(heap, name, source, opts.bits(), params)?
    };
    let line_table = LineTable::new(src);
    let plan = Plan::new(&funcs, &resolved, unit, opts.privileged);
    #[cfg(all(feature = "dump", feature = "lazy"))]
    if let Some(out) = globals {
        lazy_globals(&funcs, &resolved, &plan, atom_undefined, out);
    }
    #[cfg(all(feature = "dump", not(feature = "lazy")))]
    let _ = globals;
    let mut pools = plan.pools();
    let mut templates: Vec<Ref> = vec![Ref::NULL; funcs.len()];
    let mut scratch = Scratch::default();
    for id in (0..funcs.len()).rev() {
        let f = &mut funcs[id];
        let parent_storage = f.parent.map(|(p, _)| &storages[p as usize].0);
        #[cfg(feature = "lazy")]
        match plan.emit[id] {
            0 => continue,
            1 => {
                templates[id] = emit_lazy(
                    heap,
                    f,
                    parent_storage,
                    &plan.cap_attrs[id],
                    &line_table,
                    src,
                    script,
                    unit,
                    &mut scratch,
                )?;
                continue;
            }
            _ => {}
        }
        let mut phase1 = core::mem::take(&mut f.code)
            .finish()
            .map_err(|e| internal(&crate::detail::detail("bytecode", &e)))?;
        let f = &funcs[id];
        let t = emit_function(
            heap,
            f,
            &mut phase1,
            &resolved[id],
            &storages[id],
            parent_storage,
            &templates,
            atom_undefined,
            decls,
            // Privileged code (the prelude) keeps no line tables or local names: its
            // frames show as `(native)` or not at all in stack traces, and it has no
            // TDZ errors to name (D1430).
            (!opts.privileged).then_some(&line_table),
            opts.privileged,
            src,
            script,
            unit,
            &mut pools[plan.pool(id)],
            &mut scratch,
            #[cfg(feature = "dump")]
            maps.as_deref_mut(),
        )?;
        templates[id] = t;
    }
    #[cfg(not(feature = "lazy"))]
    {
        set_pool(heap, &pools[1], &templates[1..])?;
        set_pool(heap, &pools[0], &templates[..1])?;
        Ok(templates[0])
    }
    #[cfg(feature = "lazy")]
    {
        for (i, pool) in pools.iter().enumerate() {
            if pool.values.is_empty() {
                continue;
            }
            let members: Vec<Ref> = (0..funcs.len())
                .filter(|&id| plan.pool_of[id] == i && plan.emit[id] == 2)
                .map(|id| templates[id])
                .collect();
            set_pool(heap, pool, &members)?;
        }
        Ok(templates[usize::from(unit.root)])
    }
}

/// The lazy template of `f` (D2990): its flags, name, parameters and captures as its
/// compiled template will have them, the code `lazy`, the capture names, and in place of
/// the line table its lazy record: a line table whose pc-0 entry is the function's start
/// (a stack trace through the frame names it), then at pc 1 the offset of its
/// parameters from its start and at pc 2 the context bits, as the columns of the next
/// two lines (compiler/lazy.rs reads them).
#[cfg(feature = "lazy")]
#[allow(clippy::too_many_arguments)]
fn emit_lazy(
    heap: &mut Heap,
    f: &FuncState,
    parent_storage: Option<&Vec<Storage>>,
    attrs: &[u32],
    line_table: &LineTable,
    src: &[u8],
    script: Ref,
    unit: &Unit,
    scratch: &mut Scratch,
) -> PResult<Ref> {
    let mut captures = Vec::new();
    capture_words(f, parent_storage, &mut captures)?;
    let captures: Vec<u32> = captures.iter().zip(attrs).map(|(&w, &a)| w | a).collect();
    let mut cursor = crate::lexer::LineCursor::default();
    let (line, col) = unit.line_col(line_table, src, f.src_start, &mut cursor);
    let mut record = LineTableBuilder::new();
    record.add(0, line, col);
    record.add(1, line + 1, f.head - f.src_start);
    let ctx =
        u32::from(f.func_name_var.is_some()) | u32::from(f.after_q) << 1 | (f.owner as u32) << 2;
    record.add(2, line + 2, ctx);
    let lines = record.finish();
    let parts = TemplateParts {
        flags: template_flags(f, false),
        name: f.name,
        params: f.param_count - u16::from(f.rest.is_some()),
        length: f.length,
        captures: &captures,
        code: &[Op::Lazy as u8],
        lines: &lines,
        names: &f.capture_names,
        script,
        src_start: f.src_start + unit.base,
        src_end: f.src_end + unit.base,
        ..TemplateParts::default()
    };
    alloc_template_in(heap, &parts, &mut scratch.stack).map_err(|e| match e {
        TemplateError::Heap(e) => e.into(),
        e => internal(&crate::detail::detail("template", &e)),
    })
}

/// Writes `pool` into a `ValueArray` and makes it the constants of `templates`.
fn set_pool(heap: &mut Heap, pool: &UnitPool, templates: &[Ref]) -> PResult<()> {
    if pool.values.is_empty() {
        return Ok(());
    }
    let n = pool.values.len() as u32;
    let consts = heap.alloc_value_array(n)?;
    heap.arena_mut()
        .set(consts, crate::heap::cells::ValueArray::LEN, n);
    for (i, &v) in pool.values.iter().enumerate() {
        heap.value_array_set(consts, i as u32, v);
    }
    for &t in templates {
        heap.arena_mut().set(t.0, FuncTemplate::CONSTS, Ref(consts));
    }
    Ok(())
}

/// Classifies reference `r` of function `id`, walking into enclosing functions for
/// free names (adding capture slots on the way).
fn classify(
    funcs: &mut [FuncState],
    id: usize,
    r: usize,
    decls: &[TsDecl],
    privileged: bool,
) -> Resolved {
    let nr = funcs[id].refs[r];
    // A member of the namespace of function `f`'s closure: a property of its
    // parameter, reached from the reference's own scope.
    let member = |funcs: &mut [FuncState], f: usize| -> Option<Resolved> {
        let decl = &decls[funcs[f].ns? as usize];
        let m = decl.members.iter().find(|m| m.name == nr.atom)?;
        funcs[id].refs.push(super::func::NameRef {
            atom: decl.param,
            scope: nr.scope,
            pos: nr.pos,
            init: false,
        });
        Some(Resolved {
            target: Target::Member((funcs[id].refs.len() - 1) as u32),
            immutable: false,
            check: false,
            lexical: false,
            decl: m.child,
        })
    };
    if let Some(v) = funcs[id].lookup(nr.scope, nr.atom) {
        let var = &mut funcs[id].vars[v as usize];
        var.used = true;
        let immutable = var.kind.is_immutable();
        let lexical = var.kind.is_lexical();
        let decl = var.ts_decl;
        if var.global {
            return Resolved {
                target: Target::Global,
                immutable,
                check: false,
                lexical,
                decl,
            };
        }
        let check = !nr.init && needs_check(&funcs[id], v, nr.pos);
        return Resolved {
            target: Target::Var(v),
            immutable,
            check,
            lexical,
            decl,
        };
    }
    if let Some(m) = member(funcs, id) {
        return m;
    }
    // A free name: find the enclosing function that binds it.
    let mut path = vec![id];
    let mut at = funcs[id].parent;
    while let Some((pf, ps)) = at {
        let pf = pf as usize;
        let Some(v) = funcs[pf].lookup(ps, nr.atom) else {
            if let Some(m) = member(funcs, pf) {
                return m;
            }
            path.push(pf);
            at = funcs[pf].parent;
            continue;
        };
        {
            let var = &mut funcs[pf].vars[v as usize];
            if var.global {
                return Resolved {
                    target: Target::Global,
                    immutable: var.kind.is_immutable(),
                    check: false,
                    lexical: var.kind.is_lexical(),
                    decl: var.ts_decl,
                };
            }
            let decl = var.ts_decl;
            var.used = true;
            // A function declared at the top of the prelude's wrapper function (a child
            // of the Script, run once): its closure is made once, so the realm's table
            // holds it instead of a Box every caller captures (D2440).
            if privileged
                && var.kind == VarKind::Func
                && funcs[pf].parent.is_some_and(|(p, _)| p == 0)
            {
                let var = &mut funcs[pf].vars[v as usize];
                var.builtin = true;
                return Resolved {
                    target: Target::Builtin {
                        func: pf as u32,
                        var: v,
                    },
                    immutable: false,
                    check: false,
                    lexical: false,
                    decl,
                };
            }
            let var = &mut funcs[pf].vars[v as usize];
            var.captured = true;
            let immutable = var.kind.is_immutable();
            // Closures may run at any time: lexical bindings are always checked
            // through captures (and parameters of non-simple lists, whose defaults can
            // create closures before later parameters are initialized).
            let check =
                var.kind.is_lexical() || (var.kind == VarKind::Param && !funcs[pf].simple_params);
            // The function directly inside `pf` takes the parent's Box (a module's
            // environment entry: the module closure's capture); each one below takes its
            // parent's capture.
            let mut cap = match var.env() {
                Some(e) => Capture::Outer(e),
                None => Capture::Local(v),
            };
            let mut slot = 0;
            for &f in path.iter().rev() {
                slot = funcs[f].capture_slot(cap, nr.atom);
                cap = Capture::Outer(slot);
            }
            return Resolved {
                target: Target::Capture(slot),
                immutable,
                check,
                lexical: false,
                decl,
            };
        }
    }
    Resolved {
        target: Target::Global,
        immutable: false,
        check: false,
        lexical: false,
        decl: None,
    }
}

/// Whether an access at `pos` to binding `v` in its own function needs a TDZ check
/// (docs/compiler.md 5.6).
fn needs_check(f: &FuncState, v: VarId, pos: u32) -> bool {
    let var = &f.vars[v as usize];
    let tdz = var.kind.is_lexical() || (var.kind == VarKind::Param && !f.simple_params);
    if !tdz || var.global {
        // Globals are checked at run time by the global ops.
        return false;
    }
    pos < var.init_end || f.scopes[var.scope as usize].kind == ScopeKind::Switch
}

/// Storage for every binding of `f`, and the local slot names. Prelude functions
/// (`Var::builtin`) take the next of the `builtins` table slots.
fn assign_storage(
    f: &FuncState,
    next_builtin: &mut u16,
    builtins: u16,
    temp_name: Atom,
) -> (Vec<Storage>, Vec<Atom>) {
    let mut storage = crate::buf::vec_with_capacity(f.vars.len());
    let mut names = Vec::new();
    for v in &f.vars {
        let pseudo = v.kind.is_pseudo();
        storage.push(if let Some(e) = v.env() {
            Storage::Env(e)
        } else if v.global {
            Storage::Global
        } else if v.builtin {
            *next_builtin += 1;
            Storage::Builtin {
                idx: *next_builtin - 1,
                len: builtins,
            }
        } else if pseudo && !v.used {
            Storage::None
        } else if v.kind == VarKind::Param && f.simple_params && !v.captured {
            Storage::Arg(v.param)
        } else {
            names.push(if v.kind == VarKind::Temp {
                temp_name
            } else {
                v.name
            });
            let slot = (names.len() - 1) as u16;
            if v.captured {
                Storage::Box(slot)
            } else {
                Storage::Local(slot)
            }
        });
    }
    (storage, names)
}

/// The suspension map of `f` (D2563) from its final code: where each recorded site
/// ended up and the operand depth there.
#[cfg(feature = "dump")]
#[allow(clippy::too_many_arguments)]
fn suspension_map(
    heap: &Heap,
    src: &[u8],
    f: &FuncState,
    rec: &super::smap::Recorder,
    storage: &[Storage],
    names: &[Atom],
    code: &crate::bytecode::Code,
    handlers: &[crate::bytecode::Handler],
    suspends: &[(u32, u32)],
    all: &[crate::bytecode::Insn],
    resolved: &[Resolved],
) -> PResult<super::smap::FunctionMap> {
    if rec.sites.is_empty() {
        // Only the computed keys: no stack or liveness analysis.
        return Ok(super::smap::build(
            heap,
            src,
            f,
            rec,
            &[],
            0,
            0,
            &[],
            &[],
            &[],
        ));
    }
    let decode = |pc: u32| {
        crate::bytecode::decode::decode(&code.code, pc)
            .map_err(|e| internal(&crate::detail::detail("decode", &e)))
    };
    let depths = crate::bytecode::stack_depths(&code.code, handlers)
        .map_err(|e| internal(&crate::detail::detail("stack depths", &e)))?;
    let mut at = Vec::with_capacity(rec.sites.len());
    let mut depth = Vec::with_capacity(rec.sites.len());
    for s in &rec.sites {
        let found = suspends
            .iter()
            .find(|e| e.0 == s.p1)
            .map(|e| code.map_pos(e.1));
        match found {
            Some(pc) => {
                let size = decode(pc)?.size();
                at.push(Some((pc, pc + size)));
                depth.push(
                    depths
                        .get(pc as usize)
                        .copied()
                        .filter(|&d| d != crate::bytecode::UNKNOWN_DEPTH),
                );
            }
            None => {
                at.push(None);
                depth.push(None);
            }
        }
    }
    let slots: Vec<Option<(bool, u16, bool)>> = storage
        .iter()
        .map(|s| match *s {
            Storage::Local(i) => Some((true, i, false)),
            Storage::Box(i) => Some((true, i, true)),
            Storage::Arg(i) => Some((false, i, false)),
            _ => None,
        })
        .collect();
    // Which construct temporaries hold a value at each site (D2732), from the phase-1
    // code: the site's instruction is the suspending op with its phase-1 index.
    let live = if f.temps.is_empty() {
        alloc::vec![Vec::new(); rec.sites.len()]
    } else {
        // (One pass over the code: the suspending instructions are few.)
        let suspending: Vec<(u32, usize)> = all
            .iter()
            .enumerate()
            .filter(|(_, i)| matches!(i.op, Op::Await | Op::Yield | Op::YieldStar))
            .map(|(k, i)| (i.pc, k))
            .collect();
        let index: Vec<usize> = rec
            .sites
            .iter()
            .map(|s| {
                suspending
                    .iter()
                    .find(|x| x.0 == s.p1)
                    .map_or(usize::MAX, |x| x.1)
            })
            .collect();
        let temp_of = |r: u32| match resolved.get(r as usize).map(|x| x.target) {
            Some(Target::Var(v)) => f.temps.iter().position(|t| t.0 == v),
            _ => None,
        };
        // A completion's kind is written after its value, or alone when the value is
        // dead (compiler/completion.rs): the write defines the value too.
        let also: Vec<Option<usize>> = f
            .temps
            .iter()
            .map(|&(_, c, at)| {
                (c == "completion.kind").then(|| {
                    f.temps
                        .iter()
                        .position(|t| t.1 == "completion.value" && t.2 == at)
                })?
            })
            .collect();
        super::temps::live_at(all, &f.handlers, &also, temp_of, &index)
    };
    let params = (f.param_count - u16::from(f.rest.is_some())) as usize;
    Ok(super::smap::build(
        heap,
        src,
        f,
        rec,
        &slots,
        names.len(),
        params,
        &at,
        &depth,
        &live,
    ))
}

/// Handlers whose depth is relative to a label (`HandlerLabels::depth_from`) get their
/// absolute depth from the final code. Reachability is found from the entry: a handler
/// becomes an entry point of the stack walk (at its depth, relative ones from their
/// anchor's) once code in its region is reached, which repeats until nothing changes (a
/// pattern in a catch block of a pattern default...). Handlers never reached are
/// dropped (D2960): they sit in dead code (a pattern after `return`, 0210), where an
/// anchor has no depth and a handler in dead code would make more dead code look live.
fn resolve_relative_depths(
    f: &FuncState,
    code: &crate::bytecode::Code,
    handlers: &mut Vec<crate::bytecode::Handler>,
) -> PResult<()> {
    let unknown = crate::bytecode::UNKNOWN_DEPTH;
    if f.handlers.iter().all(|h| h.depth_from.is_none()) {
        return Ok(());
    }
    for h in handlers.iter_mut() {
        h.depth = unknown;
    }
    let mut left = handlers.len();
    let mut changed = true;
    while left > 0 && changed {
        changed = false;
        let depths = crate::bytecode::stack_depths(&code.code, handlers)
            .map_err(|e| internal(&crate::detail::detail("stack depths", &e)))?;
        let known = |pc: u32| depths.get(pc as usize).copied().filter(|&d| d != unknown);
        for (out, h) in handlers.iter_mut().zip(&f.handlers) {
            if out.depth != unknown || (out.start..out.end).all(|pc| known(pc).is_none()) {
                continue;
            }
            // A relative handler's anchor precedes its region in the same code.
            let base = match h.depth_from {
                Some(l) => code
                    .label_pos(l)
                    .and_then(known)
                    .ok_or_else(|| internal("unreachable relative handler depth"))?,
                None => 0,
            };
            out.depth = base + h.depth;
            left -= 1;
            changed = true;
        }
    }
    handlers.retain(|h| h.depth != unknown);
    Ok(())
}

/// Stores the value on top of the stack into a binding being initialized.
fn emit_init(out: &mut CodeBuf, storage: Storage, atom: Atom, lexical: bool) {
    match storage {
        Storage::Local(s) => out.put_loc(s),
        Storage::Box(s) => out.put_box(s),
        Storage::Arg(a) => out.put_arg(a),
        Storage::Global if lexical => out.init_global_lex(atom),
        Storage::Global => out.put_global(atom, 0),
        Storage::None => out.drop(),
        Storage::Builtin { idx, len } => out.put_builtin(idx, len),
        Storage::Env(c) if crate::MODULES => out.put_cap(c),
        Storage::Env(_) => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_function(
    heap: &mut Heap,
    f: &FuncState,
    phase1: &mut crate::bytecode::Phase1Code,
    resolved: &[Resolved],
    (storage, names): &(Vec<Storage>, Vec<Atom>),
    parent_storage: Option<&Vec<Storage>>,
    templates: &[Ref],
    atom_undefined: Atom,
    decls: &[TsDecl],
    line_table: Option<&LineTable>,
    privileged: bool,
    src: &[u8],
    script: Ref,
    unit: &Unit,
    pool: &mut UnitPool,
    scratch: &mut Scratch,
    #[cfg(feature = "dump")] maps: Option<&mut Vec<super::smap::FunctionMap>>,
) -> PResult<Ref> {
    // Suspension sites (D2563): phase-1 index and position in the code before jump
    // shortening of each emitted `await`/`yield`.
    #[cfg(feature = "dump")]
    let mut suspends: Vec<(u32, u32)> = Vec::new();
    let mut needs_uninit = core::mem::take(&mut scratch.needs_uninit);
    needs_uninit.clear();
    needs_uninit.resize(f.vars.len(), false);
    for r in resolved {
        if let (Target::Var(v), true) = (r.target, r.check) {
            needs_uninit[v as usize] = true;
        }
    }

    let mut all = core::mem::take(&mut scratch.insns);
    all.clear();
    all.extend(
        phase1
            .insns
            .iter()
            .enumerate()
            .map(|(i, p)| p.insn(i as u32)),
    );
    let estimate = all.len() * 3;
    let mut label_count = phase1.label_count;
    super::peephole::simplify(&mut all, &mut label_count, &mut scratch.peephole);
    let mut out = core::mem::take(&mut scratch.out);
    out.reset(label_count);
    out.reserve(estimate + 16);

    // ---- Prologue (docs/compiler.md 5.5, 5.7).
    if f.kind == FuncKind::Script {
        let mut decls = GlobalDecls::default();
        for v in f.vars.iter().filter(|v| v.global) {
            match v.kind {
                VarKind::Var => decls.vars.push(v.name),
                VarKind::Func => decls.funcs.push(v.name),
                VarKind::Let => decls.lets.push(v.name),
                VarKind::Const => decls.consts.push(v.name),
                _ => {}
            }
        }
        if !decls.is_empty() {
            let idx = pool.append(&decls.encode());
            out.global_decls(idx);
        }
    }
    for &p in &f.params {
        match storage[p as usize] {
            Storage::Box(s) if f.simple_params => {
                out.get_arg(f.vars[p as usize].param);
                out.new_box_init(s);
            }
            // Non-simple lists: the phase-1 code initializes them in order.
            Storage::Box(s) => out.new_box(s),
            Storage::Local(s) if needs_uninit[p as usize] => out.set_loc_uninit(s),
            _ => {}
        }
    }
    let pseudo = [
        (f.this_var, special::THIS),
        (f.arguments_var, special::ARGUMENTS),
        (f.new_target_var, special::NEW_TARGET),
        (f.func_name_var, special::ACTIVE_FUNCTION),
        (f.home_var, special::HOME_OBJECT),
        (f.active_var, special::ACTIVE_FUNCTION),
    ];
    for (var, kind) in pseudo {
        if let Some(v) = var {
            let st = storage[v as usize];
            match st {
                Storage::Local(s) => {
                    out.special_object(kind);
                    out.put_loc(s);
                }
                Storage::Box(s) => {
                    out.special_object(kind);
                    out.new_box_init(s);
                }
                _ => {}
            }
        }
    }
    for (i, v) in f.vars.iter().enumerate() {
        // Captured `var`s of the function scope start as `undefined` in their Box.
        if v.kind == VarKind::Var
            && let Storage::Box(s) = storage[i]
        {
            out.undefined();
            out.new_box_init(s);
        }
    }

    // ---- Re-emit.
    let mut lines = core::mem::take(&mut scratch.lines);
    lines.clear();
    let mut li = 0;
    // The line table entries of phase-1 instructions up to `pc` start at the current
    // position of `out` (the last one wins).
    let flush = |out: &CodeBuf, li: &mut usize, lines: &mut Vec<(u32, u32)>, pc: u32| {
        while *li < f.lines.len() && f.lines[*li].0 <= pc {
            let entry = (out.pos(), f.lines[*li].1);
            match lines.last_mut() {
                // (`>`: the emitter may fold an instruction into the previous one,
                // D2443.)
                Some(last) if last.0 >= entry.0 => *last = entry,
                _ => lines.push(entry),
            }
            *li += 1;
        }
    };
    let mut i = 0;
    // After an instruction without fallthrough, code up to the next label is
    // unreachable (the `return_undef` after a function's last `return`): not emitted.
    let mut dead = false;
    while i < all.len() {
        let insn = all[i];
        i += 1;
        if dead && insn.op != Op::Label {
            continue;
        }
        dead = insn.info().no_fallthrough();
        flush(&out, &mut li, &mut lines, insn.pc);
        #[cfg(feature = "dump")]
        if f.smap.is_some() && matches!(insn.op, Op::Await | Op::Yield | Op::YieldStar) {
            suspends.push((insn.pc, out.pos()));
        }
        match insn.op {
            Op::NameGet | Op::NamePut | Op::NameSet | Op::NameInit | Op::NameTypeof => {
                let r = insn.imm(0) as usize;
                let atom = f.refs[r].atom;
                // A loop's step on its temporaries (D2732): `get it; get next; iter_next`
                // with `next` in the local after `it`'s is one `iter_next_loc`.
                if insn.op == Op::NameGet
                    && let [get, step, ..] = all[i..]
                    && get.op == Op::NameGet
                    && step.op == Op::IterNext
                    && let Some(s) = unchecked_local(storage, resolved[r])
                    && unchecked_local(storage, resolved[get.imm(0) as usize]) == Some(s + 1)
                {
                    // (Its position is marked before the first `get`.)
                    out.iter_next_loc(s, Label(step.imm(0)));
                    i += 2;
                    continue;
                }
                // `i++;` / `++i;` on an unchecked local: `get_loc; inc; put_loc` in place.
                if insn.op == Op::NameGet
                    && let [upd, put, ..] = all[i..]
                    && matches!(upd.op, Op::Inc | Op::Dec)
                    && matches!(put.op, Op::NamePut | Op::NameSet)
                    && let Some(s) = unchecked_local(storage, resolved[r])
                    && unchecked_local(storage, resolved[put.imm(0) as usize]) == Some(s)
                    && !resolved[put.imm(0) as usize].immutable
                {
                    // Errors point at the update (the operator), as for `inc`.
                    flush(&out, &mut li, &mut lines, upd.pc);
                    if upd.op == Op::Inc {
                        out.inc_loc(s);
                    } else {
                        out.dec_loc(s);
                    }
                    if put.op == Op::NameSet {
                        out.get_loc(s);
                    }
                    i += 2;
                    continue;
                }
                // `x = v; ... x`, `const p = f(); if (p ...)`: keep the stored value
                // (unless the read starts an `inc_loc`).
                if matches!(insn.op, Op::NamePut | Op::NameInit)
                    && let Some(get) = all.get(i)
                    && get.op == Op::NameGet
                    && !all
                        .get(i + 1)
                        .is_some_and(|u| matches!(u.op, Op::Inc | Op::Dec))
                    && (!resolved[r].immutable || insn.op == Op::NameInit)
                    && let Some(s) = unchecked_local(storage, resolved[r])
                    && unchecked_local(storage, resolved[get.imm(0) as usize]) == Some(s)
                {
                    flush(&out, &mut li, &mut lines, get.pc);
                    out.set_loc(s);
                    i += 1;
                    continue;
                }
                // `v === undefined`
                if insn.op == Op::NameGet
                    && atom == atom_undefined
                    && matches!(resolved[r].target, Target::Global)
                    && let Some(cmp) = all.get(i)
                    && matches!(cmp.op, Op::StrictEq | Op::StrictNe)
                {
                    let jump = all.get(i + 1).filter(|j| {
                        matches!(j.op, Op::JumpIfFalse | Op::JumpIfTrue) && cmp.op == Op::StrictNe
                    });
                    if cmp.op == Op::StrictEq || jump.is_some() {
                        flush(&out, &mut li, &mut lines, cmp.pc);
                        out.is_undefined();
                        i += 1;
                        if let Some(j) = jump {
                            // `!==`: the inverted jump.
                            let inv = if j.op == Op::JumpIfFalse {
                                Op::JumpIfTrue
                            } else {
                                Op::JumpIfFalse
                            };
                            out.emit_insn(&j.with_op(inv));
                            i += 1;
                        }
                        continue;
                    }
                }
                if insn.op == Op::NameGet
                    && let Some(d) = resolved[r].decl
                    && let Some((v, n)) = fold_const_enum(decls, d, &all[i..])
                {
                    // `CE.A`, `N.CE.A` of a const enum: the member's value (as tsc).
                    match v {
                        EnumVal::Num(x) => emit_number(heap, &mut out, pool, *x),
                        EnumVal::Str(units) => {
                            let s = if units.iter().all(|&u| u <= 0xFF) {
                                let b: Vec<u8> = units.iter().map(|&u| u as u8).collect();
                                heap.alloc_latin1(&b)?
                            } else {
                                heap.alloc_utf16(units)?
                            };
                            let idx = pool.intern(heap, Value::string(s));
                            out.push_const(idx);
                        }
                    }
                    i += n;
                    continue;
                }
                if let Target::Member(obj) = resolved[r].target {
                    let obj = obj as usize;
                    let load = |out: &mut CodeBuf| {
                        let a = f.refs[obj].atom;
                        emit_name(out, storage, Op::NameGet, a, resolved[obj], atom_undefined);
                    };
                    match insn.op {
                        Op::NameGet | Op::NameTypeof => {
                            load(&mut out);
                            out.get_field(atom, 0);
                        }
                        Op::NameSet => {
                            out.dup();
                            load(&mut out);
                            out.swap();
                            out.put_field(atom, 0);
                        }
                        _ => {
                            load(&mut out);
                            out.swap();
                            out.put_field(atom, 0);
                        }
                    }
                    continue;
                }
                if matches!(insn.op, Op::NamePut | Op::NameSet | Op::NameInit)
                    && builtin_slot(storage, resolved[r]).is_some()
                {
                    return Err(internal("assignment to a prelude function"));
                }
                emit_name(
                    &mut out,
                    storage,
                    insn.op,
                    atom,
                    resolved[r],
                    atom_undefined,
                );
            }
            Op::EnterScope => {
                let s = insn.imm(0) as usize;
                let scope = &f.scopes[s];
                for &v in &scope.vars {
                    let var = &f.vars[v as usize];
                    if matches!(
                        var.kind,
                        VarKind::PrivateName | VarKind::PrivateMethod | VarKind::PrivateAccessor
                    ) {
                        // A new private symbol per class evaluation (D1400).
                        use crate::heap::cells::flags::{SYM_PRIVATE_METHOD, SYM_PRIVATE_READONLY};
                        let method = match var.kind {
                            VarKind::PrivateName => 0,
                            VarKind::PrivateMethod => SYM_PRIVATE_METHOD | SYM_PRIVATE_READONLY,
                            _ => SYM_PRIVATE_METHOD,
                        } as u8;
                        match storage[v as usize] {
                            Storage::Box(slot) => {
                                out.private_symbol(var.name, method);
                                out.new_box_init(slot);
                            }
                            Storage::Local(slot) => {
                                out.private_symbol(var.name, method);
                                out.put_loc(slot);
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if var.kind == VarKind::Hidden {
                        if let Storage::Box(slot) = storage[v as usize] {
                            out.new_box(slot);
                        }
                        continue;
                    }
                    // Parameters, `var`s and the pseudo-bindings are set up in the prologue.
                    if !matches!(
                        var.kind,
                        VarKind::Let | VarKind::Const | VarKind::Func | VarKind::CatchParam
                    ) || var.global
                    {
                        continue;
                    }
                    match storage[v as usize] {
                        Storage::Box(slot)
                            if var.kind.is_lexical() || var.kind == VarKind::CatchParam =>
                        {
                            out.new_box(slot)
                        }
                        Storage::Box(slot) => {
                            out.undefined();
                            out.new_box_init(slot);
                        }
                        Storage::Local(slot) if needs_uninit[v as usize] => {
                            out.set_loc_uninit(slot)
                        }
                        _ => {}
                    }
                }
                // `var`s named like a parameter (a separate var scope, D1820).
                for &(v, p) in &scope.copies {
                    match storage[p as usize] {
                        Storage::Local(s) => out.get_loc(s),
                        Storage::Box(s) => out.get_box(s),
                        Storage::Arg(a) => out.get_arg(a),
                        _ => out.undefined(),
                    }
                    emit_init(
                        &mut out,
                        storage[v as usize],
                        f.vars[v as usize].name,
                        false,
                    );
                }
                // Hoisted function declarations (docs/compiler.md 5.5); a module's
                // top-level ones exist from linking on.
                for &(v, child) in &scope.funcs {
                    if crate::MODULES && matches!(storage[v as usize], Storage::Env(_)) {
                        continue;
                    }
                    out.fclosure(child);
                    let var = &f.vars[v as usize];
                    emit_init(&mut out, storage[v as usize], var.name, false);
                }
            }
            Op::IterScope => {
                // CreatePerIterationEnvironment: fresh Boxes for captured bindings.
                let s = insn.imm(0) as usize;
                for &v in &f.scopes[s].vars {
                    if let Storage::Box(slot) = storage[v as usize] {
                        out.rebox(slot);
                    }
                }
            }
            // The generator/async state local (docs/compiler.md 7.7): phase-1 code holds
            // a placeholder.
            Op::GeneratorStart
            | Op::Yield
            | Op::AsyncStart
            | Op::Await
            | Op::AsyncReturn
            | Op::AsyncReject => {
                let slot = match f.state_var.map(|v| storage[v as usize]) {
                    Some(Storage::Local(s)) => s,
                    _ => return Err(internal("state op without a state local")),
                };
                match insn.op {
                    Op::GeneratorStart => out.generator_start(slot),
                    Op::Yield => out.yield_(slot),
                    Op::AsyncStart => out.async_start(slot),
                    Op::Await => out.await_(slot),
                    Op::AsyncReturn => out.async_return(slot),
                    _ => out.async_reject(slot),
                }
            }
            Op::LeaveScope => {}
            Op::Label => out.place_label(Label(insn.imm(0))),
            // The function's constants become the unit's (D2446).
            Op::PushConst => out.push_const(pool.intern(heap, f.consts[insn.imm(0) as usize])),
            Op::GetTemplateObject => {
                // The site's slot holds the start of `n, cooked x n, raw x n` (D1362):
                // copied as one run, and the slot itself is private (it caches the
                // template object).
                let local = |i: u32| f.consts[i as usize];
                let start = local(insn.imm(0)).as_int().unwrap_or(0) as u32;
                let n = local(start).as_int().unwrap_or(0) as u32;
                let run: Vec<Value> = (start..start + 1 + 2 * n).map(local).collect();
                let run_start = pool.append(&run);
                let site = pool.append(&[Value::int(run_start as i32)]);
                out.get_template_object(site);
            }
            _ => out.emit_insn(&insn),
        }
    }
    let mut code = core::mem::take(&mut scratch.code);
    out.finish_in(&mut code, &mut scratch.finish)
        .map_err(|e| internal(&crate::detail::detail("bytecode", &e)))?;
    for e in &mut lines {
        e.0 = code.map_pos(e.0);
    }
    let mut handlers = core::mem::take(&mut scratch.handlers);
    handlers.clear();
    for h in &f.handlers {
        let pos = |l| {
            code.label_pos(l)
                .ok_or_else(|| internal("unplaced handler label"))
        };
        handlers.push(crate::bytecode::Handler {
            start: pos(h.start)?,
            end: pos(h.end)?,
            target: pos(h.target)?,
            depth: h.depth,
        });
    }
    resolve_relative_depths(f, &code, &mut handlers)?;
    #[cfg(feature = "dump")]
    if let (Some(rec), Some(maps)) = (&f.smap, maps) {
        maps.push(suspension_map(
            heap, src, f, rec, storage, names, &code, &handlers, &suspends, &all, resolved,
        )?);
    }

    // ---- Captures, children, line table, template.
    let mut capture_words = core::mem::take(&mut scratch.captures);
    self::capture_words(f, parent_storage, &mut capture_words)?;
    let mut children = core::mem::take(&mut scratch.children);
    children.clear();
    children.extend(f.children.iter().map(|&c| templates[c as usize]));
    let mut lt = LineTableBuilder::with_buffer(core::mem::take(&mut scratch.line_bytes));
    let mut all_names = core::mem::take(&mut scratch.names);
    all_names.clear();
    if let Some(line_table) = line_table {
        let mut cursor = crate::lexer::LineCursor::default();
        for &(pc, pos) in &lines {
            let (line, col) = unit.line_col(line_table, src, pos, &mut cursor);
            lt.add(pc, line, col);
        }
        all_names.extend_from_slice(names);
        all_names.extend_from_slice(&f.capture_names);
    }
    let line_bytes = lt.finish();
    let flags = template_flags(f, privileged);
    let params = f.param_count - u16::from(f.rest.is_some());
    let parts = TemplateParts {
        flags,
        name: f.name,
        params,
        length: f.length,
        locals: names.len() as u16,
        captures: &capture_words,
        code: &code.code,
        consts: &[],
        children: &children,
        handlers: &handlers,
        lines: &line_bytes,
        names: &all_names,
        script,
        src_start: f.src_start + unit.base(),
        src_end: f.src_end + unit.base(),
    };
    let t = alloc_template_in(heap, &parts, &mut scratch.stack).map_err(|e| match e {
        TemplateError::Heap(e) => e.into(),
        e => internal(&crate::detail::detail("template", &e)),
    });
    *scratch = Scratch {
        lines,
        stack: core::mem::take(&mut scratch.stack),
        insns: all,
        peephole: core::mem::take(&mut scratch.peephole),
        needs_uninit,
        out,
        code,
        finish: core::mem::take(&mut scratch.finish),
        handlers,
        captures: capture_words,
        children,
        names: all_names,
        line_bytes,
    };
    t
}

/// The capture descriptors of `f` (`capture_desc`), into `capture_words`.
#[inline(always)]
fn capture_words(
    f: &FuncState,
    parent_storage: Option<&Vec<Storage>>,
    capture_words: &mut Vec<u32>,
) -> PResult<()> {
    capture_words.clear();
    for &c in &f.captures {
        capture_words.push(match c {
            Capture::Outer(i) => capture_desc::outer(i),
            Capture::Local(v) => match parent_storage.map(|p| p[v as usize]) {
                Some(Storage::Box(slot)) => capture_desc::local(slot),
                other => {
                    return Err(internal(&crate::detail::detail(
                        "capture of a non-Box binding",
                        &other,
                    )));
                }
            },
        });
    }
    Ok(())
}

/// The `func_flags` of `f`'s template.
#[inline(always)]
fn template_flags(f: &FuncState, privileged: bool) -> u32 {
    let kind = match f.kind {
        // Eval code runs like a Script's top level (the embedder supplies `this`).
        FuncKind::Script | FuncKind::Eval | FuncKind::Module => func_flags::KIND_SCRIPT,
        FuncKind::AsyncBody => func_flags::KIND_ASYNC_BODY,
        FuncKind::Normal => func_flags::KIND_NORMAL,
        FuncKind::Arrow => func_flags::KIND_ARROW,
        FuncKind::Method => func_flags::KIND_METHOD,
        FuncKind::Getter => func_flags::KIND_GETTER,
        FuncKind::Setter => func_flags::KIND_SETTER,
        FuncKind::ClassCtor => func_flags::KIND_CLASS_CTOR,
        FuncKind::DerivedCtor => func_flags::KIND_DERIVED_CTOR,
        FuncKind::FieldInit | FuncKind::StaticBlock => func_flags::KIND_FIELD_INIT,
    };
    let mut flags = kind;
    if privileged {
        flags |= func_flags::PRIVILEGED;
    }
    if f.is_async {
        flags |= func_flags::ASYNC;
    }
    if f.is_generator {
        flags |= func_flags::GENERATOR;
    }
    if f.sloppy_this {
        flags |= func_flags::SLOPPY_THIS;
    }
    // Functions that use `super.x` (themselves or in arrows) get a home object slot.
    if f.home_var.is_some() {
        flags |= func_flags::HOME;
    }
    flags
}

/// Buffers reused across the functions of a unit (0096: per-function allocations were
/// a tenth of a compile).
#[derive(Default)]
struct Scratch {
    lines: Vec<(u32, u32)>,
    stack: crate::bytecode::StackScratch,
    insns: Vec<crate::bytecode::Insn>,
    peephole: super::peephole::Scratch,
    needs_uninit: Vec<bool>,
    out: CodeBuf,
    code: crate::bytecode::Code,
    finish: crate::bytecode::FinishScratch,
    handlers: Vec<crate::bytecode::Handler>,
    captures: Vec<u32>,
    children: Vec<Ref>,
    names: Vec<Atom>,
    line_bytes: Vec<u8>,
}

/// A const enum member read through the binding of declaration `d`: the
/// `get_field` chain at the start of `next` (through namespaces) and its value, with
/// the number of `get_field`s it replaces.
fn fold_const_enum<'d>(
    decls: &'d [TsDecl],
    d: u32,
    next: &[crate::bytecode::Insn],
) -> Option<(&'d EnumVal, usize)> {
    let mut cur = d as usize;
    for (n, insn) in next.iter().enumerate() {
        if insn.op != Op::GetField {
            return None;
        }
        let decl = &decls[cur];
        let m = decl.members.iter().find(|m| m.name == insn.atom(0))?;
        match decl.kind {
            DeclKind::ConstEnum => return m.value.as_ref().map(|v| (v, n + 1)),
            DeclKind::Namespace => cur = m.child? as usize,
            DeclKind::Enum => return None,
        }
    }
    None
}

/// The prelude function table slot of a reference, if it resolved to one.
fn builtin_slot(storage: &[Storage], res: Resolved) -> Option<u16> {
    match res.target {
        Target::Slot(i) => Some(i),
        Target::Var(v) => match storage[v as usize] {
            Storage::Builtin { idx, .. } => Some(idx),
            _ => None,
        },
        _ => None,
    }
}

/// The local slot of a reference that resolved to a local without a TDZ check.
fn unchecked_local(storage: &[Storage], res: Resolved) -> Option<u16> {
    match res.target {
        Target::Var(v) if !res.check => match storage[v as usize] {
            Storage::Local(s) => Some(s),
            _ => None,
        },
        _ => None,
    }
}

/// Pushes a number constant (small integers inline).
fn emit_number(heap: &Heap, out: &mut CodeBuf, pool: &mut UnitPool, n: f64) {
    let i = n as i32;
    if i as f64 == n && !(n == 0.0 && n.is_sign_negative()) {
        match i8::try_from(i) {
            Ok(b) => out.push_i8(b),
            Err(_) => out.push_i32(i),
        }
        return;
    }
    out.push_const(pool.intern(heap, Value::number(n)));
}

/// The resolved form of one `name_*` op.
fn emit_name(
    out: &mut CodeBuf,
    storage: &[Storage],
    op: Op,
    atom: Atom,
    res: Resolved,
    atom_undefined: Atom,
) {
    #[derive(Clone, Copy)]
    enum At {
        Loc(u16),
        Box(u16),
        Arg(u16),
        Cap(u16),
        Builtin(u16),
        Global,
    }
    let at = match res.target {
        Target::Var(v) => match storage[v as usize] {
            Storage::Local(s) => At::Loc(s),
            Storage::Box(s) => At::Box(s),
            Storage::Arg(a) => At::Arg(a),
            Storage::Builtin { idx, .. } => At::Builtin(idx),
            Storage::Env(c) if crate::MODULES => At::Cap(c),
            Storage::Global | Storage::None | Storage::Env(_) => At::Global,
        },
        Target::Capture(c) => At::Cap(c),
        Target::Slot(i) => At::Builtin(i),
        // Handled by the caller (a property access); `Builtin` is resolved to `Slot`.
        Target::Global | Target::Member(_) | Target::Builtin { .. } => At::Global,
    };
    let lexical = res.lexical;
    let chk = res.check;
    // Reads, with or without the TDZ check.
    let get = |out: &mut CodeBuf, typeof_: bool| match at {
        At::Loc(s) if chk => out.get_loc_chk(s),
        At::Loc(s) => out.get_loc(s),
        At::Box(s) if chk => out.get_box_chk(s),
        At::Box(s) => out.get_box(s),
        At::Arg(a) => out.get_arg(a),
        At::Cap(c) if chk => out.get_cap_chk(c),
        At::Cap(c) => out.get_cap(c),
        At::Builtin(i) => out.get_builtin(i),
        At::Global => {
            // The global `undefined` is non-writable and non-configurable.
            if atom == atom_undefined && matches!(res.target, Target::Global) {
                out.undefined()
            } else if typeof_ {
                out.get_global_typeof(atom, 0)
            } else {
                out.get_global(atom, 0)
            }
        }
    };
    if res.immutable && matches!(op, Op::NamePut | Op::NameSet) {
        // Assignment to a const: TDZ first (ReferenceError), then TypeError.
        if chk || matches!(at, At::Global) {
            get(out, false);
            out.drop();
        }
        out.throw_const_assign(atom);
        return;
    }
    match op {
        Op::NameGet => get(out, false),
        Op::NameTypeof => get(out, true),
        Op::NamePut | Op::NameSet => {
            let keep = op == Op::NameSet;
            match at {
                At::Loc(s) => match (chk, keep) {
                    (true, true) => out.set_loc_chk(s),
                    (true, false) => out.put_loc_chk(s),
                    (false, true) => out.set_loc(s),
                    (false, false) => out.put_loc(s),
                },
                At::Box(s) => match (chk, keep) {
                    (true, true) => out.set_box_chk(s),
                    (true, false) => out.put_box_chk(s),
                    (false, true) => out.set_box(s),
                    (false, false) => out.put_box(s),
                },
                At::Arg(a) => {
                    if keep {
                        out.set_arg(a)
                    } else {
                        out.put_arg(a)
                    }
                }
                At::Cap(c) => match (chk, keep) {
                    (true, true) => out.set_cap_chk(c),
                    (true, false) => out.put_cap_chk(c),
                    (false, true) => out.set_cap(c),
                    (false, false) => out.put_cap(c),
                },
                At::Global => {
                    if keep {
                        out.dup();
                    }
                    out.put_global(atom, 0);
                }
                // Rejected by the caller (prelude functions are never assigned).
                At::Builtin(_) => out.nop(),
            }
        }
        Op::NameInit => match at {
            At::Loc(s) => out.put_loc(s),
            At::Box(s) => out.put_box(s),
            At::Arg(a) => out.put_arg(a),
            At::Cap(c) => out.put_cap(c),
            At::Global if lexical => out.init_global_lex(atom),
            At::Global => out.put_global(atom, 0),
            At::Builtin(_) => out.nop(),
        },
        _ => out.nop(),
    }
}
