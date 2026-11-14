//! The parser core: state, token helpers, errors, names, constants and declarations
//! (docs/compiler.md 2). Statements are in `stmt.rs`, expressions in `expr.rs`.
use crate::PushOut;

use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec::Vec;

use super::func::{
    BODY_SCOPE, FuncId, FuncKind, FuncState, NameRef, ScopeId, ScopeKind, VarId, VarKind,
};
use super::{CompileOptions, Goal, PErr, PResult};
use crate::bytecode::{CodeBuf, Label, Phase1};
use crate::heap::{Atom, Heap, Ref};
use crate::lexer::{Kw, Lexer, Token, TokenKind};
use crate::value::Value;

/// Nesting limit for the recursive descent (docs/compiler.md 2.4). The value is pinned
/// by the `depth_limit_fits_wasm_stack` test.
pub(super) const MAX_DEPTH: u32 = 550;

/// Extra depth units of a class body, a C-style `for` and a destructuring pattern level:
/// in wasm one nesting level of each takes ~1.4 KiB, ~210 B and ~230 B of stack, against
/// 130–160 B for a plain statement level. With them the deepest accepted nesting of every
/// construct fits in djs-wasm's 128 KiB stack with at least 10% to spare (D1941, D2540).
pub(super) const CLASS_WEIGHT: u32 = 7;
pub(super) const FOR_WEIGHT: u32 = 1;
pub(super) const PATTERN_WEIGHT: u32 = 1;

/// A break/continue target (QuickJS's `BlockEnv`, docs/compiler.md 4.3).
pub(super) struct BlockEnv {
    pub labels: Vec<Atom>,
    pub kind: BlockKind,
    /// The break target (for `Finally` and `Iter`: the block a completion enters,
    /// compiler/completion.rs).
    pub break_label: Label,
    pub continue_label: Option<Label>,
    /// Whether a jump used `break_label`.
    pub used: bool,
    /// Source offset of the statement (its keyword; a labeled statement's first label):
    /// it names the jumps to its targets in a completion kind.
    pub pos: u32,
    /// `Finally` and `Iter`: the completion that enters the block.
    pub completion: Option<super::completion::Completion>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum BlockKind {
    Loop,
    Switch,
    /// A labeled statement that is not a loop: only `break label` targets it.
    Labeled,
    /// A `try` with a `finally` block (or a `using` list's disposal): `break`,
    /// `continue` and `return` that leave it run the finally block first, entering it
    /// with their completion (compiler/completion.rs, docs/compiler.md 7.4).
    Finally,
    /// The iterator of a for-of: `break`, `continue` and `return` that leave the loop
    /// close it first, in the block at `break_label` (entered with a completion like a
    /// finally block, see compiler/for_in_of.rs).
    Iter,
}

/// The state of an enclosing function while a nested one is parsed.
pub(super) struct Suspended {
    pub f: FuncState,
    pub scope: ScopeId,
    pub blocks: Vec<BlockEnv>,
    pub open_iters: Vec<OpenIter>,
    pub using: super::using::UsingList,
}

/// An iterator that a `return` from inside an expression must close (a resumed
/// `yield`, compiler/generator.rs): an array pattern's being destructured, with the
/// depth at `base` and its iterator's temporary (`undefined` once done, D2732). For-of
/// iterators and finally blocks are left through `BlockEnv`s instead.
#[derive(Clone, Copy)]
pub(super) struct OpenIter {
    pub base: Label,
    pub it: crate::heap::Atom,
}

#[derive(Default)]
pub(super) struct ParserSpare {
    stack: Vec<Suspended>,
    done: Vec<Option<FuncState>>,
    blocks: Vec<BlockEnv>,
    buf8: Vec<u8>,
    pattern_vars: Vec<VarId>,
    open_iters: Vec<OpenIter>,
    str_pool: Vec<(u32, u32, crate::value::Str)>,
    str_units: Vec<u16>,
    units: Vec<u16>,
    latin1: Vec<u8>,
}

pub(super) struct Parser<'a, 'h> {
    pub lx: Lexer<'a>,
    pub tok: Token,
    pub prev_end: u32,
    pub src: &'a [u8],
    pub heap: &'h mut Heap,
    pub opts: &'a CompileOptions<'a>,
    /// The function being parsed.
    pub f: FuncState,
    /// Its enclosing functions, innermost last.
    pub stack: Vec<Suspended>,
    /// Finished functions by id (the unit's top level is id 0, finished last).
    pub done: Vec<Option<FuncState>>,
    /// The innermost scope of `f`.
    pub scope: ScopeId,
    pub depth: u32,
    /// Break/continue targets of `f` (they never cross functions).
    pub blocks: Vec<BlockEnv>,
    pub buf8: Vec<u8>,
    pub atom_undefined: Atom,
    pub atom_this: Atom,
    pub atom_arguments: Atom,
    pub atom_new_target: Atom,
    pub atom_home: Atom,
    pub atom_active: Atom,
    /// The classes being parsed, innermost last (compiler/class.rs).
    pub classes: Vec<super::class::ClassCtx>,
    /// The name for an anonymous function or arrow that starts at the given offset
    /// (NamedEvaluation: `let f = () => 1` names it `f`).
    pub name_hint: Option<(Atom, u32)>,
    /// The span of the last anonymous function or arrow expression, so a computed
    /// property can tell whether its value is one (runtime naming).
    pub last_anon_fn: Option<(u32, u32)>,
    /// The function (or class constructor) that took the current name hint, for
    /// [`Parser::parse_named`] to take the name back if the initializer goes on.
    pub named_fn: Option<FuncId>,
    /// Decorators were parsed for the next class (their array is on the stack): the
    /// offset of the first `@` (compiler/decorators.rs).
    pub class_decs: Option<u32>,
    /// Legacy decorators were parsed for the next class: their thunk.
    pub class_thunk: Option<super::decorators::Thunk>,
    /// Where the content of the innermost parentheses being parsed starts (`u32::MAX`:
    /// none), so an optional chain knows it is all of it (`(a?.b)()`).
    pub paren_inner: u32,
    /// The start of the call argument being parsed: a function there is eager (D2995).
    #[cfg(feature = "lazy")]
    pub arg_start: u32,
    /// The next unary operand is the operand of `delete`.
    pub delete_operand: bool,
    /// The memoized pattern-or-literal scan (docs/compiler.md 3.3).
    pub pat_scan: super::scan::PatternScan,
    /// Bindings declared by destructuring patterns, for their callers to set
    /// `init_end` (a stack: nested patterns in defaults push and take their own).
    pub pattern_vars: Vec<VarId>,
    /// Iterators to close on a return from inside an expression, innermost last.
    pub open_iters: Vec<OpenIter>,
    /// String constants of the whole unit: functions share them
    /// (strings are immutable, so sharing is invisible; the prelude repeats its error
    /// messages and keys across hundreds of functions, 0043).
    pub str_pool: Vec<(u32, u32, crate::value::Str)>,
    pub str_units: Vec<u16>,
    pub units: Vec<u16>,
    pub latin1: Vec<u8>,
    /// `str_pool` by the hash of the units: `(hash, index)`, hash 0 empty.
    pub str_table: Vec<(u32, u32)>,
    /// The TypeScript grammar is on (`CompileOptions::typescript`).
    pub ts: bool,
    /// `CompileOptions::debugger_awaits` (D3050).
    #[cfg(feature = "dump")]
    pub debugger_awaits: bool,
    /// Nesting of TypeScript speculation (compiler/ts.rs): errors inside are cheap and
    /// only mean "not this".
    pub spec: u32,
    /// The erased type ranges, for `strip_types` (`None`: not recorded).
    pub erased: Option<Erased>,
    /// The spans of the outermost class's code that runs when the class is evaluated,
    /// for `class_spans` (`None`: not recorded), and the class nesting depth.
    pub class_spans: Option<Vec<ClassSpan>>,
    pub class_depth: u32,
    /// When set, `finish_unit` fills it with the names of the prelude function table's
    /// slots, in slot order (privileged code, D2440).
    pub builtin_names: Option<Vec<crate::heap::Atom>>,
    /// The next assignment expression is the `b` of `a ? b : c` (TypeScript arrows with
    /// return types, compiler/ts.rs).
    pub after_q: bool,
    /// TypeScript enums and namespaces (compiler/ts_decl.rs), by id.
    pub ts_decls: Vec<super::ts_decl::TsDecl>,
    /// The last statement was a TypeScript declaration without runtime code (for
    /// type-only namespaces).
    pub ts_type_only: bool,
    /// strip-types: the last JS statement did not end with `;` (`ts_note_stmt_end`).
    pub ts_semi: bool,
    /// strip-types: module syntax (`import`, `export`) is parsed (and compiles to
    /// nothing useful): the corpus is modules.
    pub strip: bool,
    /// Module code (`Goal::Module`): the import and export entries parsed so far
    /// (compiler/module.rs).
    pub module: super::module::ModuleSlot,
    /// Module code that is not compiled as async met an `await` where it would be the
    /// operator (top-level await): `compile_module` compiles it again as an async body.
    pub module_await: core::cell::Cell<bool>,
    /// strip-types: where the last formal parameter list's contents ended (tsc's
    /// `parameters.end`).
    pub ts_params_end: u32,
    /// The `using` declarations of the statement list being parsed (compiler/using.rs).
    pub using: super::using::UsingList,
    /// The local holding the completion value of Script and eval code
    /// (`Parser::emit_completion`).
    pub completion: Option<Atom>,
    /// Lazy or eager, and a lazy function's own compile (D2990).
    #[cfg(feature = "lazy")]
    pub unit: super::resolve::Unit,
    /// Record suspension maps (compiler/smap.rs): every function gets a recorder.
    #[cfg(feature = "dump")]
    pub smaps: bool,
    /// Tests only: a plain call pushes `this` under the callee (a lowering whose
    /// suspension maps must differ, compiler/smap_tests.rs). The code does not run.
    #[cfg(test)]
    pub test_swap_call: bool,
}

/// The atoms of unescaped identifiers by their text in the source: most names repeat,
/// and interning through the heap's table (hash, array-index check, probe, compare
/// through the heap) cost 5% of a compile (0096). Open addressing, keyed by the hash of
/// the bytes; an entry is `(hash, start, len, atom)` with the text at `text[start..]`.
#[derive(Default)]
pub(crate) struct IdentAtoms {
    slots: Vec<(u32, u32, u32, Atom)>,
    text: Vec<u8>,
    len: usize,
}

impl IdentAtoms {
    fn hash(b: &[u8]) -> u32 {
        let mut h: u32 = 0x811C_9DC5;
        for &c in b {
            h = (h ^ u32::from(c)).wrapping_mul(0x0100_0193);
        }
        h | 1
    }

    fn get(&self, b: &[u8], h: u32) -> Option<Atom> {
        if self.slots.is_empty() {
            return None;
        }
        let mask = self.slots.len() - 1;
        let mut i = h as usize & mask;
        loop {
            let (k, start, len, a) = self.slots[i];
            if k == 0 {
                return None;
            }
            if k == h && self.text.get(start as usize..(start + len) as usize) == Some(b) {
                return Some(a);
            }
            i = (i + 1) & mask;
        }
    }

    fn insert(&mut self, h: u32, b: &[u8], a: Atom) {
        let start = self.text.len() as u32;
        self.text.extend_from_slice(b);
        let len = b.len() as u32;
        if (self.len + 1) * 2 > self.slots.len() {
            let cap = (self.slots.len() * 2).max(256);
            let old = core::mem::replace(&mut self.slots, alloc::vec![(0, 0, 0, Atom::NULL); cap]);
            for e in old.into_iter().filter(|e| e.0 != 0) {
                self.put(e);
            }
        }
        self.put((h, start, len, a));
        self.len += 1;
    }

    fn put(&mut self, e: (u32, u32, u32, Atom)) {
        let mask = self.slots.len() - 1;
        let mut i = e.0 as usize & mask;
        while self.slots[i].0 != 0 {
            i = (i + 1) & mask;
        }
        self.slots[i] = e;
    }
}

/// A span of class code that runs when the class is evaluated (`compiler::class_spans`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClassSpan {
    pub start: u32,
    pub end: u32,
    pub kind: ClassSpanKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassSpanKind {
    /// A static block's body, between its braces.
    StaticBlock,
    /// A static field's ` = initializer`, from the `=`.
    StaticInit,
    /// The same of a static private field or auto-accessor (only class code sets it).
    StaticPrivateInit,
}

/// Erased TypeScript ranges: `start..end` byte offsets and the kind (`ts::ER_*`).
pub(super) type Erased = Vec<(u32, u32, u8)>;

/// Parser position for emission-free lookahead and deferred parsing (docs/compiler.md
/// 3, D1080): the lexer state and the current token.
#[derive(Clone, Copy)]
pub(super) struct Checkpoint<'a> {
    lx: Lexer<'a>,
    tok: Token,
    prev_end: u32,
}

impl Checkpoint<'_> {
    pub fn tok_start(&self) -> u32 {
        self.tok.start
    }
}

pub(super) fn syntax<T>(pos: u32, msg: impl Into<Cow<'static, str>>) -> PResult<T> {
    Err(syntax_error(pos, msg))
}

/// Out of line, and generic only over the message type: an error site is one call with
/// the position and the message (0092: inlined, the `Cow` construction at every site
/// was ~8 KB of wasm).
#[cold]
#[inline(never)]
fn syntax_error(pos: u32, msg: impl Into<Cow<'static, str>>) -> PErr {
    PErr::syntax(pos, msg.into())
}

impl Drop for Parser<'_, '_> {
    fn drop(&mut self) {
        fn take<T>(v: &mut Vec<T>) -> Vec<T> {
            v.clear();
            core::mem::take(v)
        }
        let spare = ParserSpare {
            stack: take(&mut self.stack),
            done: take(&mut self.done),
            blocks: take(&mut self.blocks),
            buf8: take(&mut self.buf8),
            pattern_vars: take(&mut self.pattern_vars),
            open_iters: take(&mut self.open_iters),
            str_pool: take(&mut self.str_pool),
            str_units: take(&mut self.str_units),
            units: take(&mut self.units),
            latin1: take(&mut self.latin1),
        };
        self.heap.compile_scratch.parser = spare;
    }
}

impl<'a, 'h> Parser<'a, 'h> {
    pub fn new(
        heap: &'h mut Heap,
        src: &'a [u8],
        opts: &'a CompileOptions<'a>,
    ) -> PResult<Parser<'a, 'h>> {
        let mut lx = Lexer::new(src)?;
        let tok = lx.next_token()?;
        let kind = match opts.goal {
            Goal::Script => FuncKind::Script,
            Goal::AsyncBody => FuncKind::AsyncBody,
            Goal::Eval => FuncKind::Eval,
            Goal::Module => FuncKind::Module,
        };
        let atom_undefined = crate::heap::atoms::UNDEFINED;
        let atom_this = crate::heap::atoms::THIS;
        let atom_arguments = crate::heap::atoms::ARGUMENTS;
        // Not an identifier, so it cannot collide with a user binding.
        let atom_new_target = crate::heap::atoms::NEW_TARGET;
        let atom_home = crate::heap::atoms::HOME;
        let atom_active = crate::heap::atoms::ACTIVE;
        let f = FuncState::reusing(0, kind, None, heap.compile_scratch.spare());
        let ParserSpare {
            stack,
            mut done,
            blocks,
            buf8,
            pattern_vars,
            open_iters,
            str_pool,
            str_units,
            units,
            latin1,
        } = core::mem::take(&mut heap.compile_scratch.parser);
        done.push(None);
        let mut p = Parser {
            lx,
            tok,
            prev_end: 0,
            src,
            heap,
            opts,
            f,
            stack,
            done,
            scope: BODY_SCOPE,
            depth: 0,
            blocks,
            buf8,
            atom_undefined,
            atom_this,
            atom_arguments,
            atom_new_target,
            atom_home,
            atom_active,
            classes: Vec::new(),
            name_hint: None,
            last_anon_fn: None,
            named_fn: None,
            class_decs: None,
            class_thunk: None,
            delete_operand: false,
            paren_inner: u32::MAX,
            #[cfg(feature = "lazy")]
            arg_start: u32::MAX,
            pat_scan: Default::default(),
            pattern_vars,
            open_iters,
            str_pool,
            str_units,
            units,
            latin1,
            str_table: Vec::new(),
            ts: opts.typescript,
            #[cfg(feature = "dump")]
            debugger_awaits: opts.debugger_awaits,
            spec: 0,
            erased: None,
            class_spans: None,
            class_depth: 0,
            builtin_names: None,
            after_q: false,
            ts_decls: Vec::new(),
            ts_type_only: false,
            ts_semi: false,
            strip: false,
            module: Default::default(),
            module_await: core::cell::Cell::new(false),
            ts_params_end: 0,
            using: Default::default(),
            completion: None,
            #[cfg(feature = "lazy")]
            unit: super::resolve::Unit::EAGER,
            #[cfg(feature = "dump")]
            smaps: false,
            #[cfg(test)]
            test_swap_call: false,
        };
        p.f.src_end = src.len() as u32;
        if crate::MODULES && kind == FuncKind::Module {
            p.module = super::module::ModuleSlot::new(super::module::ModuleBuild::new(p.heap)?);
        }
        if kind == FuncKind::AsyncBody {
            // Before the parameters, as for an async function: a body's frame is laid out
            // as the `async function (params) {...}` a dump writes for it (D2617, D2620).
            p.start_flavor((true, false))?;
            for name in opts.params.iter() {
                let atom = p.heap.atom_str(name)?;
                let v = p.declare_param(atom, 0)?;
                p.f.vars[v as usize].init_end = 0;
            }
            p.f.length = p.f.param_count;
        }
        // The prelude's completion value is never used.
        if kind == FuncKind::Eval || (kind == FuncKind::Script && !opts.privileged) {
            let atom = crate::heap::atoms::COMPLETION;
            let v = p.f.add_var(atom, VarKind::Internal, BODY_SCOPE);
            p.f.vars[v as usize].init_end = 0;
            p.completion = Some(atom);
        }
        Ok(p)
    }

    /// Parses the whole source and builds the templates. Also returns the erased type
    /// ranges if they were recorded.
    pub fn compile(mut self) -> PResult<(Ref, Option<Erased>, Option<Vec<crate::heap::Atom>>)> {
        self.parse_top()?;
        self.finish_unit()
    }

    /// Parses the whole source and resolves its names: [`super::free_names`].
    #[cfg(feature = "dump")]
    pub fn free_names(mut self) -> PResult<Vec<super::FreeNames>> {
        self.parse_top()?;
        self.end_async_region();
        let top = core::mem::replace(&mut self.f, FuncState::new(0, FuncKind::Script, None));
        self.done[0] = Some(top);
        let funcs = core::mem::take(&mut self.done)
            .into_iter()
            .map(|f| f.ok_or_else(|| super::resolve::internal("unfinished function")))
            .collect::<PResult<Vec<_>>>()?;
        Ok(super::resolve::free_names(
            funcs,
            &self.ts_decls,
            self.opts.privileged,
            self.atom_undefined,
        ))
    }

    /// Parses the whole source and returns the class spans it recorded
    /// (`compiler::class_spans`); the templates are not built.
    #[cfg(feature = "dump")]
    pub fn compile_class_spans(mut self) -> PResult<Vec<ClassSpan>> {
        self.class_spans = Some(Vec::new());
        self.parse_top()?;
        Ok(self.class_spans.take().unwrap_or_default())
    }

    /// Parses the whole source and returns the suspension maps of its functions
    /// (compiler/smap.rs), in source order. The templates are built too, and are garbage.
    #[cfg(feature = "dump")]
    pub fn compile_maps(mut self) -> PResult<(Ref, Vec<super::smap::FunctionMap>)> {
        self.smaps = true;
        self.f.smap = Some(Default::default());
        self.parse_top()?;
        let mut maps = Vec::new();
        let (t, ..) = self.finish_unit_with(Some(&mut maps), None)?;
        crate::sort::sort_by(&mut maps, |a, b| (a.start, a.end) < (b.start, b.end));
        Ok((t, maps))
    }

    /// A lazy unit's compile with the globals of each of its lazy functions (D2993).
    #[cfg(all(feature = "dump", feature = "lazy"))]
    pub fn compile_lazy_globals(mut self) -> PResult<(Ref, Vec<super::resolve::LazyGlobals>)> {
        self.unit.lazy = true;
        self.parse_top()?;
        let mut globals = Vec::new();
        let (t, ..) = self.finish_unit_with(None, Some(&mut globals))?;
        Ok((t, globals))
    }

    /// [`Parser::compile_maps`] of a lazy unit (D2994), with the globals of each of its
    /// lazy functions.
    #[cfg(all(feature = "dump", feature = "lazy"))]
    pub fn compile_lazy_maps(
        mut self,
    ) -> PResult<(
        Ref,
        Vec<super::smap::FunctionMap>,
        Vec<super::resolve::LazyGlobals>,
    )> {
        self.unit.lazy = true;
        self.smaps = true;
        self.f.smap = Some(Default::default());
        self.parse_top()?;
        let (mut maps, mut globals) = (Vec::new(), Vec::new());
        let (t, ..) = self.finish_unit_with(Some(&mut maps), Some(&mut globals))?;
        crate::sort::sort_by(&mut maps, |a, b| (a.start, a.end) < (b.start, b.end));
        Ok((t, maps, globals))
    }

    /// The top-level code, up to its return.
    pub(super) fn parse_top(&mut self) -> PResult<()> {
        self.code().enter_scope(BODY_SCOPE as u16);
        self.reset_completion();
        // `using` is not allowed at the top level of a Script, but in a codemode body
        // (an async function's) and a module (strip-types).
        let using = self.begin_using_list(
            self.f.kind == FuncKind::AsyncBody
                || (crate::MODULES && self.f.kind == FuncKind::Module)
                || self.strip,
        );
        while self.tok.kind != TokenKind::Eof {
            self.parse_statement_list_item()?;
        }
        self.end_using_list(using);
        self.code().leave_scope(BODY_SCOPE as u16);
        if let Some(a) = self.completion {
            let r = self.name_ref(a, self.prev_end);
            self.code().name_get(r);
            self.code().return_();
        } else {
            self.emit_return_undef();
        }
        Ok(())
    }

    /// Finishes the unit after its top-level code: resolves every function into
    /// templates (also used by `compile_dynamic`, compiler/dynamic.rs).
    pub(super) fn finish_unit(
        self,
    ) -> PResult<(Ref, Option<Erased>, Option<Vec<crate::heap::Atom>>)> {
        self.finish_unit_with(
            #[cfg(feature = "dump")]
            None,
            #[cfg(feature = "dump")]
            None,
        )
    }

    /// [`Parser::finish_unit`], filling `maps` with the suspension maps when recorded,
    /// and `globals` with the globals of each lazy function (`resolve::LazyGlobals`).
    pub(super) fn finish_unit_with(
        mut self,
        #[cfg(feature = "dump")] maps: Option<&mut Vec<super::smap::FunctionMap>>,
        #[cfg(feature = "dump")] globals: Option<&mut Vec<super::resolve::LazyGlobals>>,
    ) -> PResult<(Ref, Option<Erased>, Option<Vec<crate::heap::Atom>>)> {
        self.end_async_region();
        let top = core::mem::replace(&mut self.f, FuncState::new(0, FuncKind::Script, None));
        self.done[0] = Some(top);
        let mut funcs = core::mem::take(&mut self.heap.compile_scratch.funcs);
        funcs.clear();
        for f in self.done.drain(..) {
            funcs.push(f.ok_or_else(|| super::resolve::internal("unfinished function"))?);
        }
        let erased = self.erased.take();
        let mut names = self.builtin_names.take();
        let t = super::resolve::finish(
            self.heap,
            self.src,
            self.opts,
            #[cfg(feature = "lazy")]
            &self.unit,
            #[cfg(not(feature = "lazy"))]
            &super::resolve::Unit::EAGER,
            self.atom_undefined,
            funcs,
            &self.ts_decls,
            names.as_mut(),
            #[cfg(feature = "dump")]
            maps,
            #[cfg(feature = "dump")]
            globals,
        )?;
        Ok((t, erased, names))
    }

    // ---- Functions ----------------------------------------------------------------

    /// Starts parsing a nested function: `f` becomes the new function.
    pub fn begin_function(&mut self, kind: FuncKind) -> PResult<()> {
        let id = self.done.len() as FuncId;
        self.done.push(None);
        let spare = self.heap.compile_scratch.spare();
        #[allow(unused_mut)]
        let mut child = FuncState::reusing(id, kind, Some((self.f.id, self.scope)), spare);
        #[cfg(feature = "dump")]
        if self.smaps {
            child.smap = Some(Default::default());
        }
        let parent = core::mem::replace(&mut self.f, child);
        self.stack.push(Suspended {
            f: parent,
            scope: self.scope,
            blocks: core::mem::take(&mut self.blocks),
            open_iters: core::mem::take(&mut self.open_iters),
            using: core::mem::take(&mut self.using),
        });
        self.scope = BODY_SCOPE;
        Ok(())
    }

    /// Finishes the current function and returns to its parent. Returns the child
    /// index for `fclosure`.
    pub fn end_function(&mut self) -> PResult<u32> {
        self.end_function_at(None)
    }

    /// [`Parser::end_function`] into a child index reserved earlier with
    /// [`Parser::reserve_child`] (a class constructor, whose `fclosure` comes first).
    pub fn end_function_at(&mut self, reserved: Option<u32>) -> PResult<u32> {
        self.end_async_region();
        let s = self
            .stack
            .pop()
            .ok_or_else(|| super::resolve::internal("no parent function"))?;
        let child = core::mem::replace(&mut self.f, s.f);
        self.scope = s.scope;
        self.blocks = s.blocks;
        self.open_iters = s.open_iters;
        self.using = s.using;
        let id = child.id;
        self.done[id as usize] = Some(child);
        if let Some(i) = reserved {
            self.f.children[i as usize] = id;
            return Ok(i);
        }
        self.f.children.push_out(id);
        Ok((self.f.children.len() - 1) as u32)
    }

    /// Finishes the current function without creating it anywhere (a type-only
    /// namespace's closure): returns to the parent.
    pub fn discard_function(&mut self) -> PResult<()> {
        self.end_async_region();
        let s = self
            .stack
            .pop()
            .ok_or_else(|| super::resolve::internal("no parent function"))?;
        let child = core::mem::replace(&mut self.f, s.f);
        self.scope = s.scope;
        self.blocks = s.blocks;
        self.open_iters = s.open_iters;
        self.using = s.using;
        let id = child.id;
        self.done[id as usize] = Some(child);
        Ok(())
    }

    /// Reserves the child index of a function that is parsed later.
    pub fn reserve_child(&mut self) -> u32 {
        self.f.children.push_out(u32::MAX);
        (self.f.children.len() - 1) as u32
    }

    /// Makes a parked function (a class's field initializer, compiler/class.rs) the
    /// current one again, with the current function suspended as its parent.
    pub fn resume_function(&mut self, f: FuncState) {
        let parent = core::mem::replace(&mut self.f, f);
        self.stack.push(Suspended {
            f: parent,
            scope: self.scope,
            blocks: core::mem::take(&mut self.blocks),
            open_iters: core::mem::take(&mut self.open_iters),
            using: core::mem::take(&mut self.using),
        });
        self.scope = BODY_SCOPE;
    }

    /// Parks the current function (resumed with [`Parser::resume_function`]) and
    /// returns to its parent.
    pub fn park_function(&mut self) -> PResult<FuncState> {
        let s = self
            .stack
            .pop()
            .ok_or_else(|| super::resolve::internal("no parent function"))?;
        let f = core::mem::replace(&mut self.f, s.f);
        self.scope = s.scope;
        self.blocks = s.blocks;
        self.open_iters = s.open_iters;
        self.using = s.using;
        Ok(f)
    }

    /// Declares the next parameter of the current function. Its `init_end` is set by
    /// the caller once the parameter (and its default) has been parsed.
    pub fn declare_param(&mut self, name: Atom, pos: u32) -> PResult<VarId> {
        if self.f.find_in_scope(BODY_SCOPE, name).is_some() {
            return syntax(pos, "duplicate argument names not allowed in this context");
        }
        if self.f.param_count == u16::MAX {
            return syntax(pos, "too many parameters");
        }
        let v = self.f.add_var(name, VarKind::Param, BODY_SCOPE);
        self.f.vars[v as usize].param = self.f.param_count;
        self.f.params.push_out(v);
        self.f.param_count += 1;
        self.syn_decl(v, super::smap_syntax::DeclKind::Param);
        Ok(v)
    }

    // ---- Tokens ---------------------------------------------------------------------

    pub fn checkpoint(&self) -> Checkpoint<'a> {
        Checkpoint {
            lx: self.lx,
            tok: self.tok,
            prev_end: self.prev_end,
        }
    }

    pub fn rewind(&mut self, cp: Checkpoint<'a>) {
        self.lx = cp.lx;
        self.tok = cp.tok;
        self.prev_end = cp.prev_end;
    }

    pub fn next(&mut self) -> PResult<()> {
        self.prev_end = self.tok.end;
        self.tok = self.lx.next_token()?;
        Ok(())
    }

    /// The token after the current one (the lexer is `Copy`, so this is a scan on a
    /// copy). `Eof` on a lexing error: the real scan reports it.
    pub fn peek(&self) -> Token {
        let mut lx = self.lx;
        lx.next_token().unwrap_or(Token {
            kind: TokenKind::Eof,
            kw: Kw::None,
            flags: 0,
            start: self.tok.end,
            end: self.tok.end,
        })
    }

    /// The two tokens after the current one.
    pub fn peek2(&self) -> (Token, Token) {
        let mut lx = self.lx;
        let eof = Token {
            kind: TokenKind::Eof,
            kw: Kw::None,
            flags: 0,
            start: self.tok.end,
            end: self.tok.end,
        };
        let t1 = lx.next_token().unwrap_or(eof);
        let t2 = lx.next_token().unwrap_or(eof);
        (t1, t2)
    }

    pub fn is(&self, kind: TokenKind) -> bool {
        self.tok.kind == kind
    }

    pub fn eat(&mut self, kind: TokenKind) -> PResult<bool> {
        if self.tok.kind == kind {
            self.next()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Consumes `kind` or fails with `expecting '<text>'`.
    pub fn expect(&mut self, kind: TokenKind, text: &str) -> PResult<()> {
        if self.tok.kind == kind {
            self.next()
        } else {
            syntax(self.tok.start, crate::text::cat!("expecting '", text, "'"))
        }
    }

    /// Automatic semicolon insertion (docs/compiler.md 3.5).
    pub fn expect_semicolon(&mut self) -> PResult<()> {
        if self.tok.kind == TokenKind::Semicolon {
            return self.next();
        }
        if matches!(self.tok.kind, TokenKind::RBrace | TokenKind::Eof) || self.tok.newline_before()
        {
            if self.erased.is_some() {
                self.ts_stmt_end_asi();
            }
            return Ok(());
        }
        syntax(self.tok.start, "expecting ';'")
    }

    /// In operator position, `>` merges with what follows (`>>`, `>=`, ...).
    pub fn operator_tok(&mut self) {
        if self.tok.kind == TokenKind::Gt {
            self.tok = self.lx.rescan_greater(self.tok);
        }
    }

    pub fn enter(&mut self) -> PResult<()> {
        self.enter_n(1)
    }

    pub fn leave(&mut self) {
        self.leave_n(1);
    }

    /// Like [`Parser::enter`] for `n` depth units: a construct whose recursion takes
    /// several times the stack of a plain level (docs/compiler.md 2.4, D1941).
    pub fn enter_n(&mut self, n: u32) -> PResult<()> {
        self.depth += n;
        if self.depth > MAX_DEPTH {
            return syntax(self.tok.start, "stack overflow");
        }
        Ok(())
    }

    pub fn leave_n(&mut self, n: u32) {
        self.depth -= n;
    }

    /// The token text for messages: the cooked name for identifiers and keywords,
    /// otherwise the source.
    pub fn tok_text(&mut self) -> String {
        if self.tok.kind == TokenKind::Ident && self.tok.has_escape() {
            let v = self.lx.ident_value(&self.tok, &mut self.buf8);
            return String::from_utf8_lossy(v).into_owned();
        }
        // qjs shows the end of input as an empty token (D2071).
        if self.tok.kind == TokenKind::Eof {
            return String::new();
        }
        String::from_utf8_lossy(self.lx.text(&self.tok)).into_owned()
    }

    pub fn unexpected<T>(&mut self) -> PResult<T> {
        let text = self.tok_text();
        syntax(
            self.tok.start,
            crate::text::cat!("unexpected token in expression: '", text, "'"),
        )
    }

    // ---- Code ---------------------------------------------------------------------

    pub fn code(&mut self) -> &mut CodeBuf<Phase1> {
        &mut self.f.code
    }

    pub fn new_label(&mut self) -> Label {
        self.f.code.new_label()
    }

    pub fn place(&mut self, l: Label) {
        self.f.code.place_label(l);
    }

    /// Records that the next instruction comes from source offset `pos` (for runtime
    /// error positions).
    pub fn mark(&mut self, pos: u32) {
        let pc = self.f.code.pos();
        if let Some(last) = self.f.lines.last_mut() {
            if last.0 == pc {
                last.1 = pos;
                return;
            }
            if last.1 == pos {
                return;
            }
        }
        self.f.lines.push_out((pc, pos));
    }

    /// Moves the code in `mid..end` in front of `start..mid` (loop rotation, 0086), with
    /// the line table entries. The moved blocks and the code after `end` keep the
    /// positions they started with.
    pub fn rotate_code(&mut self, start: u32, mid: u32, end: u32) {
        if start == mid || mid == end {
            return;
        }
        self.f.code.rotate(start, mid, end);
        // Suspension sites in the moved blocks move with them.
        #[cfg(feature = "dump")]
        if let Some(r) = &mut self.f.smap {
            for s in &mut r.sites {
                if (start..mid).contains(&s.p1) {
                    s.p1 += end - mid;
                } else if (mid..end).contains(&s.p1) {
                    s.p1 -= mid - start;
                }
            }
        }
        // The entries are sorted by pc: before `start`, the moved `start..mid` and
        // `mid..end`, the rest. Each block starts with the entry in effect where it
        // started (unless it has its own), and so does the code after `end`.
        // Only the entries from `start` on move (0092: binary search, not a scan of
        // the whole function per loop).
        let lines = &mut self.f.lines;
        let idx = |pc: u32| lines.partition_point(|l| l.0 < pc);
        let (i0, i1, i2) = (idx(start), idx(mid), idx(end));
        // The entry in effect at `pc` (index `i`), unless one starts there.
        let eff = |i: usize, pc: u32| match lines.get(i) {
            Some(l) if l.0 == pc => None,
            _ => i.checked_sub(1).map(|j| lines[j].1),
        };
        let (e_start, e_mid, e_end) = (eff(i0, start), eff(i1, mid), eff(i2, end));
        let tail = lines.split_off(i0);
        let (a, rest) = tail.split_at(i1 - i0);
        let (b, after) = rest.split_at(i2 - i1);
        let (shift_b, shift_a) = (mid - start, end - mid);
        lines.extend(e_mid.map(|p| (start, p)));
        lines.extend(b.iter().map(|&(pc, p)| (pc - shift_b, p)));
        lines.extend(e_start.map(|p| (start + shift_a, p)));
        lines.extend(a.iter().map(|&(pc, p)| (pc + shift_a, p)));
        lines.extend(e_end.map(|p| (end, p)));
        lines.extend_from_slice(after);
    }

    /// Removes the last instruction (lvalue conversion, docs/compiler.md 3.7).
    pub fn truncate_last(&mut self) {
        self.f.code.truncate_last();
        let pc = self.f.code.pos();
        while self.f.lines.last().is_some_and(|l| l.0 > pc) {
            self.f.lines.pop();
        }
    }

    /// A name reference at `pos` in the current scope.
    pub fn name_ref(&mut self, atom: Atom, pos: u32) -> u32 {
        self.f.refs.push_out(NameRef {
            atom,
            scope: self.scope,
            pos,
            init: false,
        });
        (self.f.refs.len() - 1) as u32
    }

    /// A reference that initializes a declaration (`name_init`).
    pub fn init_ref(&mut self, atom: Atom, pos: u32) -> u32 {
        let r = self.name_ref(atom, pos);
        self.f.refs[r as usize].init = true;
        r
    }

    // ---- Constants -----------------------------------------------------------------

    /// Pushes a number: small integers inline, everything else through the pool.
    pub fn emit_number(&mut self, n: f64) {
        // `trunc` is not in core; an exact round trip through i32 says the same.
        let i = n as i32;
        if i as f64 == n && !(n == 0.0 && n.is_sign_negative()) {
            if let Ok(b) = i8::try_from(i) {
                self.code().push_i8(b);
            } else {
                self.code().push_i32(i);
            }
            return;
        }
        // Not deduplicated here: resolution interns every constant of the unit by value
        // (D2446), and a sorted list made a compile quadratic in distinct constants (0198).
        let idx = self.add_const(Value::number(n));
        self.code().push_const(idx);
    }

    pub fn add_const(&mut self, v: Value) -> u32 {
        self.f.consts.push_out(v);
        (self.f.consts.len() - 1) as u32
    }

    /// The constant index of a string. Its heap string is shared by the unit's
    /// functions; resolution interns the constants by value (D2446).
    pub fn string_const(&mut self, units: &[u16]) -> PResult<u32> {
        let s = self.str_pool_get(units)?;
        Ok(self.add_const(Value::string(s)))
    }

    /// The unit's heap string of `units`: open addressing over their hash, compared
    /// by their units (a sorted list, inserted into, made a compile quadratic in
    /// distinct strings: a dump's data has thousands, 0198).
    fn str_pool_get(&mut self, units: &[u16]) -> PResult<crate::value::Str> {
        // A long string by its length and a sample of ~256 units: a dump's megabyte
        // literal is compared once on a match instead of hashed whole.
        let h = units
            .iter()
            .step_by(units.len() / 256 + 1)
            .fold(units.len() as u32, |h, &u| {
                (h ^ u32::from(u)).wrapping_mul(0x0100_0193)
            })
            | 1;
        if 2 * (self.str_pool.len() + 1) > self.str_table.len() {
            let mut bigger = alloc::vec![(0u32, 0u32); (self.str_table.len() * 2).max(64)];
            let mask = bigger.len() - 1;
            for &(k, e) in self.str_table.iter().filter(|e| e.0 != 0) {
                let mut i = k as usize & mask;
                while bigger[i].0 != 0 {
                    i = (i + 1) & mask;
                }
                bigger[i] = (k, e);
            }
            self.str_table = bigger;
        }
        let mask = self.str_table.len() - 1;
        let mut i = h as usize & mask;
        loop {
            match self.str_table[i] {
                (0, _) => break,
                (k, e) if k == h => {
                    let (at, len, s) = self.str_pool[e as usize];
                    if self.str_units[at as usize..(at + len) as usize] == *units {
                        return Ok(s);
                    }
                }
                _ => {}
            }
            i = (i + 1) & mask;
        }
        let s = self.alloc_units(units)?;
        self.str_table[i] = (h, self.str_pool.len() as u32);
        let at = self.str_units.len() as u32;
        self.str_units.extend_from_slice(units);
        self.str_pool.push_out((at, units.len() as u32, s));
        Ok(s)
    }

    /// A heap string from UTF-16 units (one byte per unit when they are Latin-1).
    pub fn alloc_units(&mut self, units: &[u16]) -> PResult<crate::value::Str> {
        Ok(if units.iter().all(|&u| u <= 0xFF) {
            self.latin1.clear();
            self.latin1.extend(units.iter().map(|&u| u as u8));
            self.heap.alloc_latin1(&self.latin1)?
        } else {
            self.heap.alloc_utf16(units)?
        })
    }

    pub fn cooked_string_const(&mut self) -> PResult<u32> {
        let mut units = core::mem::take(&mut self.units);
        units.clear();
        self.lx.cook_string(&self.tok, &mut units);
        let idx = self.string_const(&units);
        self.units = units;
        idx
    }

    pub fn cooked_string_atom(&mut self) -> PResult<Atom> {
        let mut units = core::mem::take(&mut self.units);
        units.clear();
        self.lx.cook_string(&self.tok, &mut units);
        let atom = self.atom_of_units(&units);
        self.units = units;
        atom
    }

    /// The cooked value of the current `String` token as UTF-16.
    pub fn cooked_string(&mut self) -> Vec<u16> {
        let mut out = Vec::new();
        self.lx.cook_string(&self.tok, &mut out);
        out
    }

    /// The atom of the current identifier-name token (identifier, keyword, or private
    /// name with its `#`).
    pub fn ident_atom(&mut self) -> PResult<Atom> {
        let tok = self.tok;
        if !tok.has_escape() {
            let b = self.lx.text(&tok);
            #[cfg(feature = "compile-cache")]
            if self.heap.intern_log.is_some() {
                let s = core::str::from_utf8(b).unwrap_or("\u{FFFD}");
                return Ok(self.heap.atom_str(s)?);
            }
            let h = IdentAtoms::hash(b);
            if let Some(a) = self.heap.ident_atoms.get(b, h) {
                return Ok(a);
            }
            let s = core::str::from_utf8(b).unwrap_or("\u{FFFD}");
            let a = self.heap.atom_str(s)?;
            self.heap.ident_atoms.insert(h, b, a);
            return Ok(a);
        }
        let bytes = self.lx.ident_value(&tok, &mut self.buf8);
        let s = core::str::from_utf8(bytes).unwrap_or("\u{FFFD}");
        Ok(self.heap.atom_str(s)?)
    }

    /// The atom of a string given as UTF-16 (property keys).
    pub fn atom_of_units(&mut self, units: &[u16]) -> PResult<Atom> {
        let s = self.alloc_units(units)?;
        Ok(self.heap.intern(s)?)
    }

    /// The atom of a number used as a property key (`{ 1.5: x }`).
    /// The value of the current BigInt literal token (D1460: a constant).
    pub fn bigint_literal(&mut self) -> PResult<crate::bigint::BigInt> {
        let tok = self.tok;
        // The slim build has no BigInt (D2411).
        if !crate::FULL {
            return syntax(tok.start, "BigInt literals are not available in this build");
        }
        let lit = self.lx.number_literal(&tok);
        let mut digits = Vec::new();
        crate::lexer::strip_separators(lit.digits, &mut digits);
        let b = crate::bigint::BigInt::parse_digits(
            digits.iter().map(|&d| u32::from(d)),
            lit.base.radix(),
        );
        match b {
            Some(b) if b.bits() <= crate::bigint::MAX_BITS => Ok(b),
            Some(_) => syntax(tok.start, "BigInt literal is too large"),
            None => syntax(tok.start, "invalid number literal"),
        }
    }

    /// Pushes the current BigInt literal token from the constant pool.
    pub fn emit_bigint_literal(&mut self) -> PResult<()> {
        let b = self.bigint_literal()?;
        let v = self.heap.alloc_bigint(&b)?;
        let idx = self.add_const(v);
        self.code().push_const(idx);
        Ok(())
    }

    pub fn atom_of_number(&mut self, n: f64) -> PResult<Atom> {
        let s = crate::num::to_string(n);
        Ok(self.heap.atom_str(s.as_str())?)
    }

    // ---- Names and declarations ----------------------------------------------------

    /// Validates the current token as a binding identifier and returns its atom
    /// (docs/compiler.md 2.5).
    pub fn binding_name(&mut self) -> PResult<Atom> {
        let tok = self.tok;
        if tok.kind != TokenKind::Ident
            || tok.is_escaped_reserved()
            || tok.kw.is_strict_reserved()
            || (tok.kw == Kw::Await && self.await_reserved())
        {
            return syntax(tok.start, "variable name expected");
        }
        if matches!(tok.kw, Kw::Eval | Kw::Arguments) {
            return syntax(tok.start, "invalid variable name in strict mode");
        }
        self.ident_atom()
    }

    /// Declares `var name` from the current scope: it binds in the body scope.
    pub fn declare_var(&mut self, name: Atom, pos: u32) -> PResult<VarId> {
        // A `var` must not cross a scope with a lexical binding of the same name.
        let mut s = self.scope;
        loop {
            if let Some(v) = self.f.find_in_scope(s, name) {
                let kind = self.f.vars[v as usize].kind;
                // Block-level function declarations are lexical too, and a module's
                // top-level ones (16.2.1.1: LexicallyDeclaredNames of a ModuleItemList).
                if kind.is_lexical()
                    || (kind == VarKind::Func && (s != self.f.var_scope || self.at_module_top()))
                {
                    return syntax(pos, "invalid redefinition of lexical identifier");
                }
            }
            match self.f.scopes[s as usize].parent {
                Some(p) => s = p,
                None => break,
            }
        }
        self.f.var_decls.push_out((name, self.scope));
        let vs = self.f.var_scope;
        if let Some(v) = self.f.find_in_scope(vs, name) {
            // `var` after `var`, or a parameter: the same binding.
            return Ok(v);
        }
        let v = self.f.add_var(name, VarKind::Var, vs);
        self.f.vars[v as usize].global = self.f.kind == FuncKind::Script;
        self.syn_decl(v, super::smap_syntax::DeclKind::Var);
        self.note_export(name, pos);
        Ok(v)
    }

    /// Declares a `let`/`const` in the current scope.
    pub fn declare_lexical(&mut self, name: Atom, kind: VarKind, pos: u32) -> PResult<VarId> {
        // In a separate var scope, the parameters are still the body's names.
        let params_too = self.scope == self.f.var_scope && self.scope != BODY_SCOPE;
        let found = self.f.find_in_scope(self.scope, name).or_else(|| {
            params_too
                .then(|| self.f.find_in_scope(BODY_SCOPE, name))
                .flatten()
                .filter(|&v| self.f.vars[v as usize].kind == VarKind::Param)
        });
        let found = found.or_else(|| self.catch_param(name));
        if let Some(v) = found {
            let msg = match self.f.vars[v as usize].kind {
                // Function declarations at the function level behave like `var`.
                VarKind::Var => self.var_conflict_message(),
                VarKind::Func if self.scope == self.f.var_scope => self.var_conflict_message(),
                VarKind::Param => "invalid redefinition of parameter name",
                _ => "invalid redefinition of lexical identifier",
            };
            return syntax(pos, msg);
        }
        let scope = self.scope;
        if self
            .f
            .var_decls
            .iter()
            .any(|&(n, d)| n == name && self.f.scope_within(d, scope))
        {
            return syntax(pos, self.var_conflict_message());
        }
        let v = self.f.add_var(name, kind, scope);
        self.f.vars[v as usize].global = self.f.kind == FuncKind::Script && scope == BODY_SCOPE;
        if scope == BODY_SCOPE {
            self.note_export(name, pos);
        }
        let dk = if kind == VarKind::Const {
            super::smap_syntax::DeclKind::Const
        } else {
            super::smap_syntax::DeclKind::Let
        };
        self.syn_decl(v, dk);
        Ok(v)
    }

    /// In the block of a `catch` with a pattern: the parameter's binding `name`.
    fn catch_param(&self, name: Atom) -> Option<VarId> {
        let s = &self.f.scopes[self.scope as usize];
        if s.kind != ScopeKind::CatchBody {
            return None;
        }
        self.f.find_in_scope(s.parent?, name)
    }

    fn var_conflict_message(&self) -> &'static str {
        match self.f.kind {
            FuncKind::Script => "invalid redefinition of global identifier",
            _ => "invalid redefinition of a variable",
        }
    }

    /// Declares a function declaration's name in the current scope (hoisted to the
    /// scope's entry, docs/compiler.md 5.5) and registers the child that initializes it.
    pub fn declare_function(&mut self, name: Atom, child: u32, pos: u32) -> PResult<()> {
        let scope = self.scope;
        let v = if scope == self.f.var_scope {
            // Like `var` at the function level: it may share a binding with a `var`,
            // a parameter or an earlier function declaration (the last one wins).
            match self.f.find_in_scope(scope, name) {
                Some(v) if self.f.vars[v as usize].kind.is_lexical() || self.at_module_top() => {
                    return syntax(pos, "invalid redefinition of lexical identifier");
                }
                Some(v) => v,
                None => {
                    let v = self.f.add_var(name, VarKind::Func, scope);
                    self.f.vars[v as usize].global = self.f.kind == FuncKind::Script;
                    v
                }
            }
        } else {
            // In a block: lexical (strict mode, no Annex B).
            if self.f.find_in_scope(scope, name).is_some()
                || self.catch_param(name).is_some()
                || self
                    .f
                    .var_decls
                    .iter()
                    .any(|&(n, d)| n == name && self.f.scope_within(d, scope))
            {
                return syntax(pos, "invalid redefinition of lexical identifier");
            }
            self.f.add_var(name, VarKind::Func, scope)
        };
        let var = &mut self.f.vars[v as usize];
        if var.kind == VarKind::Var {
            // `var f; function f() {}`: the function now initializes it.
            var.kind = VarKind::Func;
        }
        if var.kind == VarKind::Func {
            self.syn_decl(v, super::smap_syntax::DeclKind::Function);
        }
        self.f.scopes[scope as usize].funcs.push_out((v, child));
        if scope == BODY_SCOPE {
            self.note_export(name, pos);
        }
        Ok(())
    }

    /// The innermost function with its own `this`/`arguments`/`new.target` (not an
    /// arrow): `None` = the current one, `Some(i)` = `stack[i]`. `Err` if none.
    pub fn own_this_function(&self) -> Result<Option<usize>, ()> {
        if self.f.kind.has_own_this() {
            return Ok(None);
        }
        for (i, s) in self.stack.iter().enumerate().rev() {
            if s.f.kind.has_own_this() {
                return Ok(Some(i));
            }
        }
        Err(())
    }

    /// The kind of the innermost function with its own `this` (not an arrow).
    pub fn own_this_kind(&self) -> Option<FuncKind> {
        match self.own_this_function() {
            Ok(None) => Some(self.f.kind),
            Ok(Some(i)) => Some(self.stack[i].f.kind),
            Err(()) => None,
        }
    }

    /// Declares a pseudo-binding (`this`, `arguments`, `new.target`, the home object, the
    /// active function) in the function that owns it. Returns false if there is none
    /// (top-level code has a `this` but none of the others).
    pub fn declare_pseudo(&mut self, kind: VarKind) -> bool {
        let Ok(owner) = self.own_this_function() else {
            return false;
        };
        let atom = match kind {
            VarKind::This => self.atom_this,
            VarKind::Arguments => self.atom_arguments,
            VarKind::HomeObject => self.atom_home,
            VarKind::ActiveFunc => self.atom_active,
            _ => self.atom_new_target,
        };
        let f = match owner {
            None => &mut self.f,
            Some(i) => &mut self.stack[i].f,
        };
        if f.kind.is_top_level() && kind != VarKind::This {
            return false;
        }
        let slot = match kind {
            VarKind::This => &mut f.this_var,
            VarKind::Arguments => &mut f.arguments_var,
            VarKind::HomeObject => &mut f.home_var,
            VarKind::ActiveFunc => &mut f.active_var,
            _ => &mut f.new_target_var,
        };
        if slot.is_none() {
            *slot = Some(f.vars.len() as VarId);
            f.add_var(atom, kind, BODY_SCOPE);
        }
        true
    }

    /// Whether `await` is the operator here (an async function). At the top level of
    /// module code that is not compiled as async yet it would be (top-level await): that
    /// is noted, and the module is compiled again as an async body.
    pub fn await_operator(&self) -> bool {
        if self.f.is_async {
            return true;
        }
        if crate::MODULES && self.f.kind == FuncKind::Module {
            self.module_await.set(true);
        }
        false
    }

    /// `await` is reserved here: in async functions, and in the parameters of arrows
    /// nested in their parameters or bodies (arrow parameters see the enclosing
    /// context, ECMA-262 15.3).
    pub fn await_reserved(&self) -> bool {
        // Module code reserves `await` everywhere (ECMA-262 13.1.1).
        if self.f.is_async
            || self.f.kind == FuncKind::StaticBlock
            || (crate::MODULES && self.module.is_some())
        {
            return true;
        }
        if !(self.f.kind == FuncKind::Arrow && self.f.in_params) {
            return false;
        }
        for s in self.stack.iter().rev() {
            if s.f.is_async || s.f.kind == FuncKind::StaticBlock {
                return true;
            }
            if !(s.f.kind == FuncKind::Arrow && s.f.in_params) {
                return false;
            }
        }
        false
    }

    /// NamedEvaluation (D2070): parses an initializer with `name` for an anonymous
    /// function, arrow or class that *is* the initializer, parenthesized or not. The
    /// name hint is taken by a function at the initializer's start (or inside its
    /// parentheses) before it is known what follows; if the initializer goes on
    /// (`(function () {}).name`, `function () {}.bind(x)`, `(() => 0)()`, `f || x`),
    /// the function loses the name again: it is not the whole initializer.
    pub fn parse_named<T>(
        &mut self,
        name: Atom,
        parse: impl FnOnce(&mut Self) -> PResult<T>,
    ) -> PResult<T> {
        let start = self.tok.start;
        let outer = self.named_fn.take();
        self.name_hint = Some((name, start));
        let r = parse(self);
        let named = core::mem::replace(&mut self.named_fn, outer);
        if self.name_hint.is_some_and(|(_, p)| p == start) {
            self.name_hint = None;
        }
        let r = r?;
        if let Some(id) = named
            && self.last_anon_fn != Some((start, self.prev_end))
            && let Some(f) = self.done[id as usize].as_mut()
        {
            f.name = Atom::NULL;
        }
        Ok(r)
    }

    /// The hinted name if the function or arrow at `pos` is the hinted expression.
    pub fn take_name_hint(&mut self, pos: u32) -> Atom {
        match self.name_hint.take() {
            Some((a, p)) if p == pos => a,
            _ => Atom::NULL,
        }
    }

    /// A reference to `this` (docs/compiler.md 5.7). In arrows it resolves to the
    /// enclosing function's binding through a capture.
    pub fn this_ref(&mut self, pos: u32) -> u32 {
        self.declare_pseudo(VarKind::This);
        let atom = self.atom_this;
        self.name_ref(atom, pos)
    }

    /// Reads `this`. In a derived constructor (or an arrow in one) it is checked: it is
    /// uninitialized until `super()` returns.
    pub fn emit_this(&mut self, pos: u32) {
        let r = self.this_ref(pos);
        self.mark(pos);
        self.code().name_get(r);
        if self.own_this_kind() == Some(FuncKind::DerivedCtor) {
            self.code().check_this();
        }
    }

    /// Before a reference to the name `arguments`: makes sure the owning function has
    /// the binding. In top-level code `arguments` is an ordinary (global) name. Class
    /// field initializers and static blocks have none (an early error).
    pub fn note_arguments(&mut self, pos: u32) -> PResult<()> {
        self.syn_special(super::smap_syntax::Special::Arguments, pos);
        if self.own_this_kind().is_some_and(FuncKind::is_class_init) {
            return syntax(
                pos,
                "'arguments' identifier is not allowed in class field initializer",
            );
        }
        self.declare_pseudo(VarKind::Arguments);
        Ok(())
    }

    /// A reference to `new.target`, or `None` outside functions.
    pub fn new_target_ref(&mut self, pos: u32) -> Option<u32> {
        if !self.declare_pseudo(VarKind::NewTarget) {
            return None;
        }
        let atom = self.atom_new_target;
        Some(self.name_ref(atom, pos))
    }

    // ---- Scopes ----------------------------------------------------------------------

    pub fn push_scope(&mut self, kind: ScopeKind) -> PResult<ScopeId> {
        if self.f.scopes.len() > u16::MAX as usize {
            return syntax(self.tok.start, "too many nested scopes");
        }
        let s = self.f.new_scope(self.scope, kind, self.tok.start);
        self.scope = s;
        self.f.code.enter_scope(s as u16);
        Ok(s)
    }

    pub fn pop_scope(&mut self) {
        let s = self.scope;
        self.f.code.leave_scope(s as u16);
        self.scope = self.f.scopes[s as usize].parent.unwrap_or(BODY_SCOPE);
    }
}
