//! The single-pass compiler: source text to function templates (docs/compiler.md).
//!
//! Scope so far: expressions and statements (0014), functions, arrows and closures
//! (0024), objects (0027), exceptions (0025), for-in/of (0029), destructuring and
//! tagged templates (0036), async functions, generators and `for await` (0041), classes
//! (0040), TypeScript (0048), `using` declarations, completion values, eval code and
//! `import()` (0058), decorators and `accessor` fields, TypeScript's legacy decorators
//! (0065).
//!
//! ```ignore
//! let template = compile(&mut heap, src, &CompileOptions::script("main.js"))?;
//! ```
//!
//! The result is a `FuncTemplate` (no captures). The embedder instantiates it as a
//! closure and calls it with `this` = the global object (D008) and, in async-body mode,
//! the parameters.

mod class;
mod completion;
mod decorators;
mod dynamic;
mod expr;
mod for_in_of;
mod func;
mod function;
mod generator;
#[cfg(feature = "lazy")]
mod lazy;
mod module;
pub mod module_meta;
mod parser;
mod pattern;
mod peephole;
mod resolve;
mod scan;
mod smap;
#[cfg(all(feature = "dump", feature = "compile-cache"))]
pub(crate) mod smap_codec;
mod smap_syntax;
mod stmt;
mod temps;
mod ts;
mod ts_decl;
mod ts_meta;
mod using;

#[cfg(all(test, feature = "dump"))]
mod smap_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod ts_tests;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::heap::{Heap, HeapError, Ref};
use crate::lexer::{LexError, LineTable};

/// What the source is compiled as (D004, D008).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Goal {
    /// A Script: top-level `var` creates global object properties, top-level
    /// `let`/`const` go into the realm's global lexical record, `return` is an error.
    Script,
    /// A codemode body: pi runs `(async (tools, console) => { <code> })`, so top-level
    /// declarations are locals, the `params` are parameters, `return` ends the run.
    AsyncBody,
    /// Indirect eval code (strict, PerformEval): a sync body whose top-level
    /// declarations are locals of the call, free names are globals, `this` is what the
    /// embedder calls it with (the global object), and the result is the completion
    /// value. `return`, `new.target` and `super` are errors; `await` is an identifier,
    /// as in a Script.
    Eval,
    /// An ECMAScript module ([`compile_module`]): strict, `import`/`export`, top-level
    /// bindings in the module environment the linker sets up, `this` undefined.
    Module,
}

#[derive(Clone, Copy, Debug)]
pub struct CompileOptions<'a> {
    pub goal: Goal,
    /// For stack traces and error messages.
    pub filename: &'a str,
    /// Parameter names in [`Goal::AsyncBody`] mode (ignored for scripts).
    pub params: &'a [&'a str],
    /// Privileged mode (docs/compiler.md 7.3): enables `%Name(args)` intrinsic calls
    /// (`bytecode::intrinsics`). Only the embedder sets it, for the prelude; it is never
    /// reachable from user code. Privileged scripts also keep no source text (D1340).
    pub privileged: bool,
    /// TypeScript: the type skipper erases types while parsing, enums, namespaces and
    /// parameter properties compile to code (compiler/ts.rs).
    pub typescript: bool,
    /// TypeScript's legacy `experimentalDecorators` instead of the standard decorators
    /// (TypeScript only, compiler/decorators.rs): tsc's `__decorate` semantics and
    /// parameter decorators.
    pub experimental_decorators: bool,
    /// With `experimental_decorators`: `emitDecoratorMetadata`, the `design:type`,
    /// `design:paramtypes` and `design:returntype` metadata (compiler/ts_meta.rs).
    pub emit_decorator_metadata: bool,
    /// `debugger;` in an async function awaits a call of the global `debugger` (D3050):
    /// a host that binds an async function there makes the statement a suspension point,
    /// where the run can be dumped. Only builds with the `dump` feature compile it so;
    /// elsewhere, and outside async functions, `debugger` does nothing. (A field of dump
    /// builds only, so that the shipped module stays byte for byte the same.)
    #[cfg(feature = "dump")]
    pub debugger_awaits: bool,
}

impl<'a> CompileOptions<'a> {
    pub fn script(filename: &'a str) -> Self {
        CompileOptions {
            goal: Goal::Script,
            filename,
            params: &[],
            privileged: false,
            typescript: false,
            experimental_decorators: false,
            emit_decorator_metadata: false,
            #[cfg(feature = "dump")]
            debugger_awaits: false,
        }
    }

    /// Indirect eval code ([`Goal::Eval`]).
    pub fn eval(filename: &'a str) -> Self {
        CompileOptions {
            goal: Goal::Eval,
            ..Self::script(filename)
        }
    }

    /// A module ([`Goal::Module`]); `filename` is its key.
    pub fn module(filename: &'a str) -> Self {
        CompileOptions {
            goal: Goal::Module,
            ..Self::script(filename)
        }
    }

    pub fn async_body(filename: &'a str, params: &'a [&'a str]) -> Self {
        CompileOptions {
            goal: Goal::AsyncBody,
            filename,
            params,
            privileged: false,
            typescript: false,
            experimental_decorators: false,
            emit_decorator_metadata: false,
            #[cfg(feature = "dump")]
            debugger_awaits: false,
        }
    }

    /// The TypeScript bit of [`CompileOptions::bits`].
    #[cfg(feature = "dump")]
    pub(crate) const TYPESCRIPT_BIT: u32 = 1 << 2;

    /// The options that decide the code, as a Script cell keeps them (`Script::OPTIONS`):
    /// the goal in the low two bits, then TypeScript, legacy decorators, metadata (also
    /// what a dump's `$djs.source` hash counts, D2882).
    #[allow(clippy::let_and_return)]
    pub fn bits(&self) -> u32 {
        let goal = match self.goal {
            Goal::Script => 0,
            Goal::AsyncBody => 1,
            Goal::Eval => 2,
            Goal::Module => 3,
        };
        let bits = goal
            | u32::from(self.typescript) << 2
            | u32::from(self.experimental_decorators) << 3
            | u32::from(self.emit_decorator_metadata) << 4
            | u32::from(self.privileged) << 5;
        #[cfg(feature = "dump")]
        let bits = bits | u32::from(self.debugger_awaits) << 6;
        bits
    }

    /// `script` with the options of [`CompileOptions::bits`] `bits` (privileged code
    /// keeps no source: never compiled again).
    #[cfg(any(feature = "dump", feature = "lazy"))]
    pub(crate) fn from_bits(filename: &'a str, params: &'a [&'a str], bits: u32) -> Self {
        CompileOptions {
            goal: match bits & 3 {
                1 => Goal::AsyncBody,
                2 => Goal::Eval,
                3 => Goal::Module,
                _ => Goal::Script,
            },
            filename,
            params,
            privileged: bits & 1 << 5 != 0,
            typescript: bits & 1 << 2 != 0,
            experimental_decorators: bits & 1 << 3 != 0,
            emit_decorator_metadata: bits & 1 << 4 != 0,
            #[cfg(feature = "dump")]
            debugger_awaits: bits & 1 << 6 != 0,
        }
    }

    /// The same options with the TypeScript grammar.
    pub fn ts(self) -> Self {
        CompileOptions {
            typescript: true,
            ..self
        }
    }

    /// A privileged Script (the prelude, D1172).
    pub fn privileged_script(filename: &'a str) -> Self {
        CompileOptions {
            privileged: true,
            ..Self::script(filename)
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CompileErrorKind {
    /// A JS SyntaxError.
    Syntax,
    /// The heap ran out of memory while compiling.
    OutOfMemory,
    /// The host could not provide memory below the cap (D1690).
    HostOutOfMemory,
}

/// A compile error with its position: a byte offset and 1-based line/column (columns
/// count code points, D1032).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CompileError {
    pub kind: CompileErrorKind,
    /// QuickJS wording where QuickJS has one (docs/compiler.md 8.1).
    pub message: String,
    pub pos: u32,
    pub line: u32,
    pub col: u32,
}

impl CompileError {
    /// The error as text: `SyntaxError: <message> at <line>:<col>` (the Display form,
    /// built without core::fmt for the shipped module, D2339).
    pub fn text(&self) -> String {
        let mut out = String::from(match self.kind {
            CompileErrorKind::Syntax => "SyntaxError: ",
            CompileErrorKind::OutOfMemory => "out of memory",
            CompileErrorKind::HostOutOfMemory => "out of host memory",
        });
        if self.kind == CompileErrorKind::Syntax {
            out.push_str(&self.message);
        }
        crate::text::cat_to!(&mut out, " at ", self.line, ":", self.col);
        out
    }
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text())
    }
}

/// The parser's internal error, positioned in the source. Boxed so that a `PResult<T>`
/// with a small `T` is returned in registers: with the 16-byte error inline, each of the
/// compiler's ~800 `?` copied the result through memory (~27 KB of wasm, D1572).
#[derive(Clone, Debug)]
pub(super) struct PErr(Box<PErrKind>);

#[derive(Clone, Debug)]
pub(super) enum PErrKind {
    Syntax { pos: u32, msg: Cow<'static, str> },
    Oom,
    HostOom,
}

impl PErr {
    #[cold]
    #[inline(never)]
    pub(super) fn syntax(pos: u32, msg: Cow<'static, str>) -> PErr {
        PErr(Box::new(PErrKind::Syntax { pos, msg }))
    }

    #[cold]
    #[inline(never)]
    pub(super) fn oom() -> PErr {
        PErr(Box::new(PErrKind::Oom))
    }
}

impl From<LexError> for PErr {
    fn from(e: LexError) -> Self {
        PErr::syntax(e.pos, Cow::Borrowed(e.message))
    }
}

impl From<HeapError> for PErr {
    fn from(e: HeapError) -> Self {
        if e == HeapError::HostOutOfMemory {
            PErr(Box::new(PErrKind::HostOom))
        } else {
            PErr::oom()
        }
    }
}

pub(super) type PResult<T> = Result<T, PErr>;

/// Runs `f` on a parser of `src`. With the dumper, a scan for patterns stops at the
/// first literal that shows a group is none (compiler/scan.rs), which can change which
/// error a program that is not valid gets: after a syntax error the source is parsed
/// again with exact scans, so errors stay what they were. Valid programs compile the
/// same either way (0198).
#[cfg(feature = "dump")]
fn parse<T>(
    heap: &mut Heap,
    src: &[u8],
    opts: &CompileOptions<'_>,
    f: impl Fn(parser::Parser<'_, '_>) -> PResult<T>,
) -> PResult<T> {
    let mut exact = false;
    loop {
        let r = parser::Parser::new(heap, src, opts).and_then(|mut p| {
            p.pat_scan.exact = exact;
            f(p)
        });
        match r {
            Err(e) if !exact && matches!(*e.0, PErrKind::Syntax { .. }) => exact = true,
            r => return r,
        }
    }
}

/// Runs `f` on a parser of `src` (the scans are always exact without the dumper).
#[cfg(not(feature = "dump"))]
fn parse<T>(
    heap: &mut Heap,
    src: &[u8],
    opts: &CompileOptions<'_>,
    f: impl FnOnce(parser::Parser<'_, '_>) -> PResult<T>,
) -> PResult<T> {
    parser::Parser::new(heap, src, opts).and_then(f)
}

/// Compiles `src` (UTF-8) into a function template.
pub fn compile(
    heap: &mut Heap,
    src: &[u8],
    opts: &CompileOptions<'_>,
) -> Result<Ref, CompileError> {
    if crate::MODULES && opts.goal == Goal::Module {
        return Err(not_a_module_entry(src));
    }
    let result = parse(heap, src, opts, |p| p.compile());
    result.map(|(t, ..)| t).map_err(|e| error_at(src, e))
}

/// Compiles module source `src` ([`Goal::Module`]): the template of the module body and
/// its module record metadata, a U32Array (`compiler::module_meta` describes the words).
/// Without the `embed` feature (D3175) a SyntaxError.
pub fn compile_module(
    heap: &mut Heap,
    src: &[u8],
    opts: &CompileOptions<'_>,
) -> Result<(Ref, Ref), CompileError> {
    let opts = CompileOptions {
        goal: Goal::Module,
        ..*opts
    };
    if !crate::MODULES {
        return Err(error_at(
            src,
            PErr::syntax(0, Cow::Borrowed("modules are not available in this build")),
        ));
    }
    // A module with top-level await is an async body ([[HasTLA]]); which one is, the first
    // parse finds out (`Parser::module_await`): only those compile twice.
    let first = parser::Parser::new(heap, src, &opts)
        .map_err(|e| (e, false))
        .and_then(|p| p.compile_module(false));
    match first {
        Ok(r) => Ok(r),
        Err((_, true)) => parse(heap, src, &opts, |p| {
            p.compile_module(true).map_err(|e| e.0)
        })
        .map_err(|e| error_at(src, e)),
        Err((e, false)) => Err(error_at(src, e)),
    }
}

/// [`compile`] with lazy functions (D2990): the source is parsed whole (every early
/// error is reported), but a function that is not run at once gets a lazy template,
/// compiled at its first call ([`compile_lazy`]). Its code is the same either way.
#[cfg(feature = "lazy")]
pub fn compile_lazy_unit(
    heap: &mut Heap,
    src: &[u8],
    opts: &CompileOptions<'_>,
) -> Result<Ref, CompileError> {
    if crate::MODULES && opts.goal == Goal::Module {
        return Err(not_a_module_entry(src));
    }
    let result = parse(heap, src, opts, |mut p| {
        p.unit.lazy = true;
        p.compile()
    });
    result.map(|(t, ..)| t).map_err(|e| error_at(src, e))
}

/// The spans of the code that the outermost class of `src` (a class's text) runs when
/// it is evaluated: static blocks' bodies and static fields' initializers, in source
/// order. Plain dumps blank the static code, whose effects the dump
/// restores (0209, D2950).
#[cfg(feature = "dump")]
pub fn class_spans(
    heap: &mut Heap,
    src: &[u8],
    opts: &CompileOptions<'_>,
) -> Result<Vec<ClassSpan>, CompileError> {
    parse(heap, src, opts, |p| p.compile_class_spans()).map_err(|e| error_at(src, e))
}

/// The free names of every function of `src`, from parsing and name resolution alone (no
/// code is generated): what a dump's check compares with the original functions (D2888,
/// 0215; a full compile cost twice as much). In source order of their ends, the top level
/// first.
#[cfg(feature = "dump")]
pub fn free_names(
    heap: &mut Heap,
    src: &[u8],
    opts: &CompileOptions<'_>,
) -> Result<Vec<FreeNames>, CompileError> {
    parse(heap, src, opts, |p| p.free_names()).map_err(|e| error_at(src, e))
}

/// A function's free names ([`free_names`]): its source span, the names it captures,
/// and the globals it or a function inside it reads or writes by name (as the code it
/// compiles to does: `get_global`, `get_global_typeof`, `put_global`).
#[cfg(feature = "dump")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FreeNames {
    pub start: u32,
    pub end: u32,
    pub captures: Vec<crate::heap::Atom>,
    pub globals: Vec<crate::heap::Atom>,
}

/// The suspension maps (D2563, compiler/smap.rs) of every function of `src` that has an
/// `await` or `yield` or evaluates a computed property key (D2623), in source order: compiles `src` again with recording on (into
/// `heap`; the templates are garbage). The maps describe frames of this build's code for
/// the same source and options.
#[cfg(feature = "dump")]
pub fn suspension_maps(
    heap: &mut Heap,
    src: &[u8],
    opts: &CompileOptions<'_>,
) -> Result<Vec<smap::FunctionMap>, CompileError> {
    compile_with_maps(heap, src, opts).map(|(_, maps)| maps)
}

/// [`compile`] and [`suspension_maps`] in one compile: recording changes nothing in the
/// code (the pin test checks it), so a loader that runs a dump's text and resumes its
/// activations needs no second compile.
#[cfg(feature = "dump")]
pub fn compile_with_maps(
    heap: &mut Heap,
    src: &[u8],
    opts: &CompileOptions<'_>,
) -> Result<(Ref, Vec<smap::FunctionMap>), CompileError> {
    let result = parse(heap, src, opts, |p| p.compile_maps());
    result
        .map(|(t, maps)| {
            let maps = maps
                .into_iter()
                .filter(|m| !m.sites.is_empty() || !m.computed_keys.is_empty())
                .collect();
            (t, maps)
        })
        .map_err(|e| error_at(src, e))
}

/// [`compile_lazy_unit`] and the suspension maps of the functions it compiles (D2994):
/// lazy functions get theirs when asked for ([`lazy_function_maps`]). Unlike
/// [`compile_with_maps`] the maps of functions without sites are kept, so that a span
/// that is covered is known. Also the globals each lazy function reads or writes, for a
/// dump (D2993).
#[cfg(all(feature = "dump", feature = "lazy"))]
pub fn compile_lazy_with_maps(
    heap: &mut Heap,
    src: &[u8],
    opts: &CompileOptions<'_>,
) -> Result<(Ref, Vec<smap::FunctionMap>, Vec<LazyGlobals>), CompileError> {
    let result = parse(heap, src, opts, |p| p.compile_lazy_maps());
    result.map_err(|e| error_at(src, e))
}

/// [`compile_lazy_unit`] with the globals each lazy function reads or writes, for a dump
/// (D2993).
#[cfg(all(feature = "dump", feature = "lazy"))]
pub fn compile_lazy_with_globals(
    heap: &mut Heap,
    src: &[u8],
    opts: &CompileOptions<'_>,
) -> Result<(Ref, Vec<LazyGlobals>), CompileError> {
    let result = parse(heap, src, opts, |p| p.compile_lazy_globals());
    result.map_err(|e| error_at(src, e))
}

/// [`suspension_maps`] with the template, and a hook on the parser (tests).
#[cfg(all(test, feature = "dump"))]
fn suspension_maps_with(
    heap: &mut Heap,
    src: &[u8],
    opts: &CompileOptions<'_>,
    hook: impl FnOnce(&mut parser::Parser<'_, '_>),
) -> Result<(Ref, Vec<smap::FunctionMap>), CompileError> {
    let result = parser::Parser::new(heap, src, opts).and_then(|mut p| {
        hook(&mut p);
        p.compile_maps()
    });
    result.map_err(|e| error_at(src, e))
}

#[cfg(feature = "lazy")]
pub use lazy::compile_lazy;
#[cfg(all(feature = "dump", feature = "lazy"))]
pub use lazy::{lazy_function_code, lazy_function_maps};
pub use parser::{ClassSpan, ClassSpanKind};
#[cfg(feature = "dump")]
pub use resolve::LazyGlobals;
#[cfg(feature = "dump")]
pub use smap::{Decl, DeclKind, Node, NodeKind, Part, Role, Special, Step, Syntax};
#[cfg(feature = "dump")]
pub use smap::{FunctionMap, Site, SiteKind, Slot};
#[cfg(feature = "dump")]
pub use smap::{Holds, NamedSlot, Place};

/// The names of the prelude function table's slots (D2440), in slot order, for
/// privileged source `src`: the prelude functions that others refer to. The registry
/// names them by these (vm/registry.rs). Compiles into `heap`.
pub fn prelude_function_names(
    heap: &mut Heap,
    src: &[u8],
    opts: &CompileOptions<'_>,
) -> Result<Vec<crate::heap::Atom>, CompileError> {
    let result = parser::Parser::new(heap, src, opts).and_then(|mut p| {
        p.builtin_names = Some(Vec::new());
        p.compile()
    });
    result
        .map(|(.., names)| names.unwrap_or_default())
        .map_err(|e| error_at(src, e))
}

/// The kinds of CreateDynamicFunction (20.2.1.1.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DynamicKind {
    Normal,
    Generator,
    Async,
    AsyncGenerator,
}

impl DynamicKind {
    /// The sourceText prefix before ` anonymous(`.
    pub const fn prefix(self) -> &'static str {
        match self {
            DynamicKind::Normal => "function",
            DynamicKind::Generator => "function*",
            DynamicKind::Async => "async function",
            DynamicKind::AsyncGenerator => "async function*",
        }
    }
}

/// Compiles the sourceText of CreateDynamicFunction (`<prefix> anonymous(<P>\n) {\n<body>\n}`)
/// as a Script whose top-level code returns the function (compiler/dynamic.rs, D1590).
/// `params_close` is the offset of the `)` after P: a parameter list that ends anywhere
/// else, or anything after the function, is a SyntaxError.
pub fn compile_dynamic_function(
    heap: &mut Heap,
    src: &[u8],
    opts: &CompileOptions<'_>,
    kind: DynamicKind,
    params_close: u32,
) -> Result<Ref, CompileError> {
    let flavor = match kind {
        DynamicKind::Normal => (false, false),
        DynamicKind::Generator => (false, true),
        DynamicKind::Async => (true, false),
        DynamicKind::AsyncGenerator => (true, true),
    };
    let opts = CompileOptions {
        goal: Goal::Script,
        ..*opts
    };
    parse(heap, src, &opts, |p| {
        p.compile_dynamic(flavor, params_close)
    })
    .map_err(|e| error_at(src, e))
}

/// TypeScript erasure for the strip-types oracle (TESTING.md): compiles `src` with the
/// TypeScript grammar and returns it with every erased type range blanked (spaces;
/// line breaks stay, so positions do not move). The heap only holds the compiled
/// template, which the caller drops.
pub fn strip_types(
    heap: &mut Heap,
    src: &[u8],
    opts: &CompileOptions<'_>,
) -> Result<Vec<u8>, CompileError> {
    let opts = CompileOptions {
        typescript: true,
        ..*opts
    };
    let result = parse(heap, src, &opts, |mut p| {
        p.erased = Some(Vec::new());
        p.strip = true;
        p.compile()
    });
    let (_, ranges, _) = result.map_err(|e| error_at(src, e))?;
    Ok(ts::blank(src, ranges.unwrap_or_default()))
}

/// Module code compiles with [`compile_module`] only: it needs its metadata.
#[cold]
fn not_a_module_entry(src: &[u8]) -> CompileError {
    error_at(
        src,
        PErr::syntax(0, Cow::Borrowed("module code compiles with compile_module")),
    )
}

fn error_at(src: &[u8], e: PErr) -> CompileError {
    let (kind, pos, message) = match *e.0 {
        PErrKind::Syntax { pos, msg } => (CompileErrorKind::Syntax, pos, String::from(msg)),
        PErrKind::Oom => (
            CompileErrorKind::OutOfMemory,
            0,
            String::from("out of memory"),
        ),
        PErrKind::HostOom => (
            CompileErrorKind::HostOutOfMemory,
            0,
            String::from("out of host memory"),
        ),
    };
    let (line, col) = LineTable::new(src).line_col(src, pos);
    CompileError {
        kind,
        message,
        pos,
        line,
        col,
    }
}
