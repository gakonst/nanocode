//! Compiling a lazy function at its first call (D2990, work/research/0218).
//!
//! The unit's compile parsed the whole source, reported every early error, classified
//! every name and wrote a lazy template for each lazy function (`resolve::lazy_roots`):
//! its flags, name, parameters, capture descriptors and names, and a record of where its
//! parameters start and the context its parse needs. Here its source text is parsed
//! again on its own: a stand-in for the enclosing function binds one variable per
//! capture (with the binding's attributes from the descriptors), the function's capture
//! slots are those variables in the template's order, and `resolve::finish` emits the
//! function and the eager functions inside it. Positions are relative to the function's
//! start (`Unit::base`): only its text is copied, so a first call costs its size, not the
//! script's.

use alloc::string::String;
use alloc::vec::Vec;

use super::func::{BODY_SCOPE, Capture, FuncKind, FuncState, VarKind};
use super::parser::Parser;
use super::resolve::Unit;
use super::{CompileError, CompileOptions, PResult, error_at};
use crate::bytecode::capture_desc;
use crate::bytecode::lines::line_entries;
use crate::bytecode::template::{bytes_of, debug_info, func_flags, script_source};
use crate::heap::cells::{FuncTemplate, Script, U32Array};
use crate::heap::{Atom, Heap, Ref};
use crate::value::Str;

/// `FuncState::owner` as the lazy record stores it (`FuncKind as u32`).
const KINDS: [FuncKind; 13] = [
    FuncKind::Script,
    FuncKind::AsyncBody,
    FuncKind::Eval,
    FuncKind::Normal,
    FuncKind::Arrow,
    FuncKind::Method,
    FuncKind::Getter,
    FuncKind::Setter,
    FuncKind::ClassCtor,
    FuncKind::DerivedCtor,
    FuncKind::FieldInit,
    FuncKind::StaticBlock,
    FuncKind::Module,
];

/// Compiles lazy template `t` and returns its compiled template.
pub fn compile_lazy(heap: &mut Heap, t: Ref) -> Result<Ref, CompileError> {
    let f = LazyFn::read(heap, t);
    let compiled = f.compile(heap, f.script, true, None)?;
    // The capture descriptors say where closures of `t` take their Boxes from, which
    // only the enclosing unit knew.
    let a = heap.arena_mut();
    let n = u32::from(a.get::<u16>(compiled.0, FuncTemplate::CAPTURES));
    if n as usize != f.words.len() {
        return Err(error_at(
            &f.span,
            super::resolve::internal("lazy captures differ"),
        ));
    }
    let code_len: u32 = a.get(compiled.0, FuncTemplate::CODE_LEN);
    for (i, &w) in f.words.iter().enumerate() {
        a.write(
            FuncTemplate::DATA.at(compiled.0, code_len + 4 * i as u32),
            w,
        );
    }
    Ok(compiled)
}

/// The suspension maps of lazy template `t` of `from` and of the functions inside it
/// (D2563, D2994), compiled into `into` with every function eager: what an eager compile
/// of the whole script records for them (D2991). The maps of functions without sites
/// are kept too, so that a span that has none is known to have none.
#[cfg(feature = "dump")]
pub fn lazy_function_maps(
    from: &Heap,
    t: Ref,
    into: &mut Heap,
) -> Result<Vec<super::smap::FunctionMap>, CompileError> {
    let f = LazyFn::read(from, t);
    let mut maps = Vec::new();
    f.compile(into, Ref::NULL, false, Some(&mut maps))?;
    Ok(maps)
}

/// Lazy template `t` of `from` compiled into `into` with every function inside it
/// eager: the code it would have, for a dump to read (D2993).
#[cfg(feature = "dump")]
pub fn lazy_function_code(from: &Heap, t: Ref, into: &mut Heap) -> Result<Ref, CompileError> {
    LazyFn::read(from, t).compile(into, Ref::NULL, false, None)
}

/// A lazy template's function as its compile needs it, read from the heap that holds it
/// (names as text: the compile may go into another heap).
struct LazyFn {
    flags: u32,
    name: String,
    names: Vec<String>,
    words: Vec<u32>,
    /// The parameters' offset from the function's start, the context bits.
    head: u32,
    ctx: u32,
    /// The function's start in its script, and its line and column there.
    base: u32,
    line: u32,
    col: u32,
    /// The function's text.
    span: Vec<u8>,
    script: Ref,
    filename: String,
    params: String,
    bits: u32,
}

impl LazyFn {
    fn read(heap: &Heap, t: Ref) -> LazyFn {
        let a = heap.arena();
        let atom = |id: u32| heap.atom_name(Atom::from_id(id));
        let n_caps = u32::from(a.get::<u16>(t.0, FuncTemplate::CAPTURES));
        let code_len: u32 = a.get(t.0, FuncTemplate::CODE_LEN);
        let info = debug_info(heap, t.0);
        let mut record = [(1, 1); 3];
        for (i, e) in line_entries(bytes_of(heap, info.lines)).take(3).enumerate() {
            record[i] = (e.line, e.col);
        }
        let [(line, col), (_, head), (_, ctx)] = record;
        let text = |r: Ref| {
            r.get()
                .map(|s| heap.str_to_string(Str(s)))
                .unwrap_or_default()
        };
        let script = info.script;
        let (span, filename, params, bits) = match script.get() {
            Some(s) => (
                // Only the function's text is copied (the source is UTF-8, D2992).
                script_source(heap, s)
                    .and_then(|b| b.get(info.src_start as usize..info.src_end as usize))
                    .map(<[u8]>::to_vec)
                    .unwrap_or_default(),
                text(a.get(s, Script::NAME)),
                text(a.get(s, Script::PARAMS)),
                a.get::<u32>(s, Script::OPTIONS),
            ),
            None => Default::default(),
        };
        LazyFn {
            flags: u32::from(a.get::<u16>(t.0, FuncTemplate::FLAGS)),
            name: match a.get::<u32>(t.0, FuncTemplate::NAME) {
                0 => String::new(),
                id => atom(id),
            },
            names: match info.names.get() {
                Some(n) => (0..n_caps)
                    .map(|i| atom(a.read(U32Array::DATA.at(n, i))))
                    .collect(),
                None => Vec::new(),
            },
            words: (0..n_caps)
                .map(|i| a.read(FuncTemplate::DATA.at(t.0, code_len + 4 * i)))
                .collect(),
            head,
            ctx,
            base: info.src_start,
            line,
            col,
            span,
            script,
            filename,
            params,
            bits,
        }
    }

    /// Compiles the function into `heap`, its templates naming Script `script`, the
    /// functions inside it lazy or not, with recording into `maps`.
    fn compile(
        &self,
        heap: &mut Heap,
        script: Ref,
        lazy: bool,
        #[cfg(feature = "dump")] maps: Option<&mut Vec<super::smap::FunctionMap>>,
        #[cfg(not(feature = "dump"))] maps: Option<&mut Vec<()>>,
    ) -> Result<Ref, CompileError> {
        let params: Vec<&str> = if self.params.is_empty() {
            Vec::new()
        } else {
            self.params.split(',').collect()
        };
        let opts = CompileOptions::from_bits(&self.filename, &params, self.bits);
        let unit = Unit {
            lazy,
            root: true,
            script,
            base: self.base,
            line: self.line,
            col: self.col,
        };
        let name = if self.name.is_empty() {
            Atom::NULL
        } else {
            heap.atom_str(&self.name)
                .map_err(|e| error_at(&self.span, e.into()))?
        };
        let mut names = Vec::with_capacity(self.names.len());
        for n in &self.names {
            names.push(
                heap.atom_str(n)
                    .map_err(|e| error_at(&self.span, e.into()))?,
            );
        }
        let lazy = Lazy {
            flags: self.flags,
            name,
            names: &names,
            words: &self.words,
            head: self.head,
            ctx: self.ctx,
        };
        let span = &self.span[..];
        let base = self.base;
        Parser::new(heap, span, &opts)
            .and_then(|p| lazy.compile(p, unit, maps.map(|m| (m, base))))
            .map_err(|e| error_at(span, e))
    }
}

/// What a lazy compile reads from the template.
struct Lazy<'a> {
    flags: u32,
    name: Atom,
    names: &'a [Atom],
    words: &'a [u32],
    head: u32,
    ctx: u32,
}

impl Lazy<'_> {
    fn compile(
        &self,
        mut p: Parser<'_, '_>,
        unit: Unit,
        #[cfg(feature = "dump")] maps: Option<(&mut Vec<super::smap::FunctionMap>, u32)>,
        #[cfg(not(feature = "dump"))] maps: Option<(&mut Vec<()>, u32)>,
    ) -> PResult<Ref> {
        #[cfg(feature = "dump")]
        if maps.is_some() {
            p.smaps = true;
        }
        let kind = func_flags::kind(self.flags);
        // The stand-in for the enclosing function: for an arrow, the kind of the function
        // that owns its `this` (`super`, `arguments`, `new.target` resolve as there).
        let owner = if kind == func_flags::KIND_ARROW {
            KINDS[(self.ctx >> 2) as usize % KINDS.len()]
        } else {
            FuncKind::Normal
        };
        let mut outer = FuncState::reusing(0, owner, None, Default::default());
        outer.src_end = p.src.len() as u32;
        for (&n, &w) in self.names.iter().zip(self.words) {
            let kind = match (
                w & capture_desc::IMMUTABLE != 0,
                w & capture_desc::CHECKED != 0,
            ) {
                (true, true) => VarKind::Const,
                (true, false) => VarKind::FuncName,
                (false, true) => VarKind::Let,
                (false, false) => VarKind::Var,
            };
            let v = outer.add_var(n, kind, BODY_SCOPE);
            outer.vars[v as usize].init_end = 0;
            let slot = if n == p.atom_this {
                &mut outer.this_var
            } else if n == p.atom_arguments {
                &mut outer.arguments_var
            } else if n == p.atom_new_target {
                &mut outer.new_target_var
            } else if n == p.atom_home {
                &mut outer.home_var
            } else if n == p.atom_active {
                &mut outer.active_var
            } else {
                continue;
            };
            *slot = Some(v);
        }
        p.f = outer;
        p.completion = None;
        p.unit = unit;
        // Private names resolve through the captures; the unit checked them.
        p.classes.push(Default::default());
        p.lx.seek(self.head);
        p.tok = p.lx.next_token()?;
        p.prev_end = self.head;
        let flavor = (
            self.flags & func_flags::ASYNC != 0,
            self.flags & func_flags::GENERATOR != 0,
        );
        match kind {
            func_flags::KIND_ARROW => {
                p.parse_arrow(false, Atom::NULL, self.ctx & 2 != 0)?;
            }
            func_flags::KIND_NORMAL => {
                p.parse_function_rest(FuncKind::Normal, self.name, 0, self.ctx & 1 != 0, flavor)?;
            }
            _ => {
                let kind = match kind {
                    func_flags::KIND_GETTER => FuncKind::Getter,
                    func_flags::KIND_SETTER => FuncKind::Setter,
                    _ => FuncKind::Method,
                };
                p.parse_method_function(kind, self.name, 0, flavor)?;
            }
        }
        // The function's capture slots are the stand-in's variables, in order; its name
        // may come from the enclosing context.
        if let Some(Some(f)) = p.done.get_mut(1) {
            f.captures = (0..self.names.len() as u32).map(Capture::Local).collect();
            f.capture_names = self.names.to_vec();
            f.name = self.name;
        }
        #[cfg(feature = "dump")]
        if let Some((maps, base)) = maps {
            let (t, ..) = p.finish_unit_with(Some(&mut *maps), None)?;
            // Spans in the script, as an eager compile has them (the rest of a map is
            // relative to its function).
            for m in maps.iter_mut() {
                m.start += base;
                m.end += base;
                for k in &mut m.computed_keys {
                    k.0 += base;
                    k.1 += base;
                }
            }
            return Ok(t);
        }
        #[cfg(not(feature = "dump"))]
        let _ = maps;
        let (t, ..) = p.finish_unit()?;
        Ok(t)
    }
}
