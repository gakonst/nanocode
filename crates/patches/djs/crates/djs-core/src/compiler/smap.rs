//! Suspension maps (D2563, docs/continuation-dumps.md "Suspension maps"): for every
//! `await` and `yield` of a function, what a frame suspended there holds, described by
//! source names rather than by this build's slots and pcs, so that a later
//! build can rebuild the frame from a dump and an exporter can name what it finds.
//!
//! Computed on demand by compiling again with recording on
//! ([`suspension_maps`]); nothing is stored in templates and a normal compile records
//! nothing. Without the `dump` feature the parser's hooks compile to nothing.
//!
//! The values a construct keeps on the operand stack while a nested expression is
//! parsed (a call's callee and `this`, a binary operator's left operand) are named by
//! the construct and the source offset of what keeps them (`call.callee@41`, D2721):
//! the parser pushes their names when it emits them and pops them when it consumes
//! them, so at a site the name stack is the operand stack below the awaited value.
//! Statement-level state lives in temporaries (locals of its own, D2732, D2733), so
//! only expressions keep values there. Resolution checks the names' count against the
//! stack depth of the final code: a mismatch (a compiler bug), or a suspension inside a
//! class definition (whose half-made class no dump can write), makes the site
//! unsupported instead of misnamed.

/// What kind of suspension point a site is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SiteKind {
    /// `await x`.
    Await,
    /// `yield x` of a generator.
    Yield,
    /// An async generator's `yield x`: the await of `x`, the yield itself, and the await
    /// of the value a `return(v)` resumes it with.
    AsyncYieldAwait,
    AsyncYield,
    AsyncYieldReturn,
    /// An async generator's `return x`: the await of `x`.
    AsyncReturn,
    /// `for await`: the await of each `next()` result, and of `return()` when the loop
    /// is left early.
    ForAwaitNext,
    ForAwaitClose,
    /// `await using`: the await of a disposal.
    AwaitUsing,
    /// `yield*`: its yields (a sync generator passes the inner result through, an async
    /// one yields its value) and, in async generators, the awaits of the inner results
    /// and of a `return(v)` value.
    YieldStar,
    YieldStarAwait,
}

impl SiteKind {
    /// The name's prefix: `await#1`, `yield#2`, `for-await.next#1`, …
    #[cfg_attr(not(feature = "dump"), allow(dead_code))]
    pub fn name(self) -> &'static str {
        match self {
            SiteKind::Await => "await",
            SiteKind::Yield => "yield",
            SiteKind::AsyncYieldAwait => "yield.await",
            SiteKind::AsyncYield => "yield",
            SiteKind::AsyncYieldReturn => "yield.return",
            SiteKind::AsyncReturn => "return.await",
            SiteKind::ForAwaitNext => "for-await.next",
            SiteKind::ForAwaitClose => "for-await.close",
            SiteKind::AwaitUsing => "await-using",
            SiteKind::YieldStar => "yield*",
            SiteKind::YieldStarAwait => "yield*.await",
        }
    }
}

#[cfg(feature = "dump")]
pub use super::smap_syntax::*;
#[cfg(feature = "dump")]
pub use maps::*;

#[cfg(feature = "dump")]
mod maps {
    use alloc::string::String;
    use alloc::vec::Vec;
    use core::fmt::Write;

    use super::super::func::{FuncState, ScopeId, VarKind};
    use super::SiteKind;
    use crate::heap::Heap;

    /// The parser's record of one function: the name stack and the sites so far.
    #[derive(Default)]
    pub(in crate::compiler) struct Recorder {
        /// The names of the values kept on the operand stack (their constructs), with
        /// the source offset that names their slot (D2721), bottom first.
        pub names: Vec<(&'static str, u32)>,
        /// The constructs being parsed whose suspensions are refused (class
        /// definitions), innermost last.
        pub unsupported: Vec<&'static str>,
        pub sites: Vec<SiteRec>,
        /// The computed property keys this function evaluates (`[` to `]`).
        pub computed_keys: Vec<(u32, u32)>,
        /// The syntax around the sites (D2850).
        pub syn: super::super::smap_syntax::SynRec,
    }

    /// A site as the parser saw it.
    pub(in crate::compiler) struct SiteRec {
        pub kind: SiteKind,
        /// Source offset of the construct (the `await`/`yield` keyword, the `for`).
        pub pos: u32,
        /// Phase-1 index of the `await`/`yield` instruction (kept up to date by loop
        /// rotation).
        pub p1: u32,
        /// The innermost scope at the site.
        pub scope: ScopeId,
        /// The name stack at the site: the values below the awaited value, each with
        /// the source offset its slot is named by.
        pub names: Vec<(&'static str, u32)>,
        /// The innermost construct whose suspensions are refused, if any.
        pub unsupported: Option<&'static str>,
    }

    /// What a frame slot holds at a site.
    #[derive(Clone, PartialEq, Eq, Debug)]
    pub enum Slot {
        /// A binding: its name and the source offset (from the function's start) of
        /// the scope that declares it.
        Binding { name: String, scope: u32 },
        /// A compiler-made local by its stable name (`<state>`, `this`, `<home>`, …).
        Hidden(String),
        /// An argument slot that no binding lives in (the call's argument `i`).
        Argument(u16),
        /// A construct temporary that holds a value here, by its slot name (D2732).
        Temp(String),
        /// Nothing at this site (a binding whose scope does not enclose it).
        Nothing,
    }

    /// One suspension point.
    #[derive(Clone, Debug)]
    pub struct Site {
        /// `await#1`, `yield.return#2`, …: the kind's prefix and the site's number among
        /// those of its kind in the function, in source order (from 1).
        pub name: String,
        pub kind: SiteKind,
        /// Source offset of the construct, from the function's start.
        pub pos: u32,
        /// Where execution resumes in this build's code (after the suspending
        /// instruction).
        pub resume_pc: u32,
        /// The argument slots (padded to the parameter count).
        pub args: Vec<Slot>,
        /// The local slots.
        pub locals: Vec<Slot>,
        /// Which local slots hold a Box at this site (a binding a closure captures): the
        /// loader puts the value in a Box there.
        pub boxed: Vec<bool>,
        /// The slot names of the values below the awaited value on the operand stack,
        /// bottom first (D2721): the construct and the source offset, from the
        /// function's start, of what keeps the value (`call.callee@41`).
        pub stack: Vec<String>,
        /// Why the frame cannot be described here: the construct, or `None`.
        pub unsupported: Option<String>,
        /// The syntax nodes around the site (D2850, `FunctionMap::syntax`), from the
        /// function's body (`path[0]`, a `NodeKind::Body`) down to the site's own
        /// construct: the `Await`/`Yield`/`YieldStar` node of an `await`, `yield`,
        /// `yield.await`, `yield.return`, `yield*` or `yield*.await` site; the
        /// `ForAwait` node of `for-await.next`/`for-await.close`; the `Return` node of
        /// `return.await`; for `await-using` the node whose end disposes: the
        /// statement list (`Block`, `Body` or `Switch`) or the loop (`For`, `ForOf`,
        /// `ForAwait`) whose head declares the resource. A `yield*.await` site's `pos`
        /// may be the end of its `YieldStar` node (the await of an inner result).
        pub path: Vec<super::Step>,
        /// The path misses a construct the recorder does not know (smap_syntax.rs): the
        /// offset of the first text in a step's part that is neither the next node nor
        /// parentheses. A continuation cannot be written from such a path.
        pub gap: Option<u32>,
    }

    /// Where a named slot is in a suspended frame: argument `i`, local `i`, or the
    /// `i`-th value the frame keeps past its locals (bottom first).
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Place {
        Arg(u16),
        Local(u16),
        Stack(u16),
    }

    /// What a named slot holds.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Holds {
        /// A binding of the source.
        Binding,
        /// A compiler-made local (`<state>`, `arguments`, `<fields 1>`, …).
        Hidden,
        /// A temporary of a construct in flight: in a local of its own (D2732) or on
        /// the operand stack.
        Temp,
    }

    /// One slot of a suspended frame by name (D2721, D2731): a dump's `slots` key.
    #[derive(Clone, Debug)]
    pub struct NamedSlot {
        /// The key: a binding's name, or `name@scope` when two of the name are in scope
        /// at the site; a hidden local's name; a temporary's `construct@offset`.
        pub name: String,
        pub place: Place,
        pub holds: Holds,
        /// The local holds a Box (a binding a closure captures).
        pub boxed: bool,
    }

    impl NamedSlot {
        /// A temporary's construct (`for-of.iterator` of `for-of.iterator@18`), else
        /// `""`.
        pub fn construct(&self) -> &str {
            match self.holds {
                Holds::Temp => self.name.split('@').next().unwrap_or(""),
                _ => "",
            }
        }

        /// A temporary's partner of the same construct: `for-of.iterator@18` for
        /// `for-of.next@18` and `want` `"iterator"`.
        pub fn partner(&self, want: &str) -> Option<String> {
            let (construct, at) = self.name.split_once('@')?;
            let (kind, _) = construct.rsplit_once('.')?;
            Some(alloc::format!("{kind}.{want}@{at}"))
        }
    }

    impl Site {
        /// The names of the construct temporaries at this site: those in locals of their
        /// own (frame order), then those on the operand stack (bottom first).
        pub fn temporaries(&self) -> Vec<&str> {
            let locals = self.locals.iter().filter_map(|x| match x {
                Slot::Temp(n) => Some(n.as_str()),
                _ => None,
            });
            locals
                .chain(self.stack.iter().map(String::as_str))
                .collect()
        }

        /// The frame's slots at this site by name, in frame order: the parameters'
        /// bindings, the locals in scope, the temporaries. Names are unique (the pin
        /// test checks the temporaries'; binding keys take their scope when shadowed;
        /// the three kinds of keys cannot meet: identifiers have no `@`, hidden names
        /// no `@`, a temporary's construct always has a `.`).
        pub fn named_slots(&self) -> Vec<NamedSlot> {
            let shadowed = |name: &str| {
                self.args
                    .iter()
                    .chain(&self.locals)
                    .filter(|s| matches!(s, Slot::Binding { name: n, .. } if n == name))
                    .count()
                    > 1
            };
            let key = |slot: &Slot| -> Option<(String, Holds)> {
                match slot {
                    Slot::Binding { name, scope } if shadowed(name) => {
                        Some((alloc::format!("{name}@{scope}"), Holds::Binding))
                    }
                    Slot::Binding { name, .. } => Some((name.clone(), Holds::Binding)),
                    Slot::Hidden(name) => Some((name.clone(), Holds::Hidden)),
                    Slot::Temp(name) => Some((name.clone(), Holds::Temp)),
                    _ => None,
                }
            };
            let mut out = Vec::new();
            for (i, slot) in self.args.iter().enumerate() {
                if let Some((name, holds)) = key(slot) {
                    out.push(NamedSlot {
                        name,
                        place: Place::Arg(i as u16),
                        holds,
                        boxed: false,
                    });
                }
            }
            for (i, slot) in self.locals.iter().enumerate() {
                if let Some((name, holds)) = key(slot) {
                    out.push(NamedSlot {
                        name,
                        place: Place::Local(i as u16),
                        holds,
                        boxed: self.boxed.get(i).copied().unwrap_or(false),
                    });
                }
            }
            for (k, name) in self.stack.iter().enumerate() {
                out.push(NamedSlot {
                    name: name.clone(),
                    place: Place::Stack(k as u16),
                    holds: Holds::Temp,
                    boxed: false,
                });
            }
            out
        }
    }

    /// The sites of one function.
    #[derive(Clone, Debug)]
    pub struct FunctionMap {
        /// The function's name (`<anonymous>` for none).
        pub name: String,
        /// Its source span in the compiled source (`Function.prototype.toString`'s).
        pub start: u32,
        pub end: u32,
        pub sites: Vec<Site>,
        /// The source spans (`[` to `]`) of the computed property keys its code
        /// evaluates: of its object literals, of the classes it defines, of its
        /// destructuring patterns. A dump that writes a class or method by its text
        /// evaluates such a key again (D2623).
        pub computed_keys: Vec<(u32, u32)>,
        /// The syntax nodes around the sites (D2850, compiler/smap_syntax.rs).
        pub syntax: super::Syntax,
    }

    impl FunctionMap {
        /// The map as text, for the pin test and for reading: a header with every slot
        /// that holds something at some site, then one line per site with its temporaries'
        /// names (in locals, then on the stack bottom first, D2721), and the slots that hold nothing there
        /// (`out`).
        pub fn text(&self, src: &[u8]) -> String {
            let mut out = String::new();
            let _ = writeln!(out, "function {} @{}..{}", self.name, self.start, self.end);
            // A slot holds the same thing wherever it holds something.
            let table = |pick: fn(&Site) -> &Vec<Slot>| -> Vec<Slot> {
                let n = self.sites.iter().map(|s| pick(s).len()).max().unwrap_or(0);
                (0..n)
                    .map(|i| {
                        self.sites
                            .iter()
                            .find_map(|s| {
                                pick(s)
                                    .get(i)
                                    .filter(|x| !matches!(x, Slot::Nothing | Slot::Temp(_)))
                            })
                            .cloned()
                            .unwrap_or(Slot::Nothing)
                    })
                    .collect()
            };
            let (args, locals) = (table(|s| &s.args), table(|s| &s.locals));
            for (kind, slots) in [("arg", &args), ("loc", &locals)] {
                let line: Vec<String> = slots
                    .iter()
                    .enumerate()
                    .filter_map(|(i, slot)| {
                        let text = match slot {
                            Slot::Binding { name, scope } => alloc::format!("{name}@{scope}"),
                            Slot::Hidden(n) => n.clone(),
                            Slot::Argument(a) => alloc::format!("argument {a}"),
                            Slot::Temp(_) | Slot::Nothing => return None,
                        };
                        Some(alloc::format!("{kind}{i} {text}"))
                    })
                    .collect();
                if !line.is_empty() {
                    let _ = writeln!(out, "  {}", line.join(", "));
                }
            }
            out.push_str(&self.syntax.text());
            for s in &self.sites {
                let line = line_of(src, self.start, s.pos);
                let _ = write!(out, "  {} @{} ({line}) pc {}:", s.name, s.pos, s.resume_pc);
                match &s.unsupported {
                    Some(u) => {
                        let _ = write!(out, " unsupported {u}");
                    }
                    None => {
                        let temps = s.temporaries();
                        if temps.is_empty() {
                            out.push_str(" -");
                        }
                        for n in temps {
                            let _ = write!(out, " {n}");
                        }
                    }
                }
                let mut dead = Vec::new();
                for (kind, slots, all) in [("arg", &s.args, &args), ("loc", &s.locals, &locals)] {
                    for (i, slot) in slots.iter().enumerate() {
                        if matches!(slot, Slot::Nothing | Slot::Temp(_))
                            && all.get(i).is_some_and(|x| *x != Slot::Nothing)
                        {
                            dead.push(alloc::format!("{kind}{i}"));
                        }
                    }
                }
                if !dead.is_empty() {
                    let _ = write!(out, " (out: {})", dead.join(" "));
                }
                out.push('\n');
                if !s.path.is_empty() {
                    out.push_str("    path");
                    for st in &s.path {
                        match st.part {
                            Some(p) => {
                                let _ = write!(out, " {}.{p}", st.node);
                            }
                            None => {
                                let _ = write!(out, " {}", st.node);
                            }
                        }
                    }
                    if let Some(g) = s.gap {
                        let _ = write!(out, " gap @{g}");
                    }
                    out.push('\n');
                }
            }
            out
        }
    }

    /// `line:col` of `pos` (from the function's start `base`) in `src`, 1-based.
    fn line_of(src: &[u8], base: u32, pos: u32) -> String {
        let at = ((base + pos) as usize).min(src.len());
        let line = src[..at].iter().filter(|&&b| b == b'\n').count() + 1;
        let col = at
            - src[..at]
                .iter()
                .rposition(|&b| b == b'\n')
                .map_or(0, |p| p + 1)
            + 1;
        alloc::format!("{line}:{col}")
    }

    /// Builds the map of function `f` from the parser's record and the final code.
    /// `slots[v]` is where binding `v` lives: `Some((false, i, _))` argument `i`,
    /// `Some((true, i, boxed))` local `i`, holding a Box if `boxed`; `n_locals` local slots; `params` argument slots;
    /// `at[k]` the final pc of site `k`'s instruction (`None`: not emitted, dead code);
    /// `depth[k]` the operand depth there, the suspended value included; `live[k][t]` whether
    /// temporary `t` (`FuncState::temps`) holds a value at site `k`.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::compiler) fn build(
        heap: &Heap,
        src: &[u8],
        f: &FuncState,
        rec: &Recorder,
        slots: &[Option<(bool, u16, bool)>],
        n_locals: usize,
        params: usize,
        at: &[Option<(u32, u32)>],
        depth: &[Option<u32>],
        live: &[Vec<bool>],
    ) -> FunctionMap {
        let base = f.src_start;
        let scope_pos = |s: ScopeId| f.scopes[s as usize].pos.saturating_sub(base);
        // Site numbers per kind, in source order (stable sort: emission order breaks
        // ties, as for the three sites of an async generator's `yield`).
        // (Sites in dead code are not emitted: they get no number.)
        let mut order: Vec<usize> = (0..rec.sites.len()).filter(|&k| at[k].is_some()).collect();
        crate::sort::sort_by(&mut order, |&a, &b| rec.sites[a].pos < rec.sites[b].pos);
        let mut number = alloc::vec![0u32; rec.sites.len()];
        let mut counts: Vec<(&'static str, u32)> = Vec::new();
        for &k in &order {
            let prefix = rec.sites[k].kind.name();
            let n = match counts.iter_mut().find(|c| c.0 == prefix) {
                Some(c) => {
                    c.1 += 1;
                    c.1
                }
                None => {
                    counts.push((prefix, 1));
                    1
                }
            };
            number[k] = n;
        }
        let (syntax, mut paths) =
            super::super::smap_syntax::build_syntax(heap, f, &rec.syn, src, &order);
        let mut sites = Vec::new();
        for (j, &k) in order.iter().enumerate() {
            let s = &rec.sites[k];
            let Some((_, resume)) = at[k] else { continue };
            // The scopes enclosing the site.
            let mut chain = Vec::new();
            let mut sc = Some(s.scope);
            while let Some(id) = sc {
                chain.push(id);
                sc = f.scopes[id as usize].parent;
            }
            let mut args = alloc::vec![None; params];
            let mut locals = alloc::vec![Slot::Nothing; n_locals];
            let mut boxed = alloc::vec![false; n_locals];
            for (i, a) in args.iter_mut().enumerate() {
                *a = Some(Slot::Argument(i as u16));
            }
            for (v, var) in f.vars.iter().enumerate() {
                let Some((local, i, in_box)) = slots[v] else {
                    continue;
                };
                let name = heap.atom_name(var.name);
                let slot = match var.kind {
                    VarKind::Temp => match f.temps.iter().position(|t| t.0 as usize == v) {
                        Some(t) if live[k].get(t).copied().unwrap_or(false) => {
                            let (_, construct, at) = f.temps[t];
                            Slot::Temp(alloc::format!("{construct}@{at}"))
                        }
                        _ => Slot::Nothing,
                    },
                    VarKind::This
                    | VarKind::Arguments
                    | VarKind::NewTarget
                    | VarKind::Internal
                    | VarKind::HomeObject
                    | VarKind::ActiveFunc
                    | VarKind::Hidden
                    | VarKind::PrivateName
                    | VarKind::PrivateMethod
                    | VarKind::PrivateAccessor => Slot::Hidden(name),
                    _ if var.kind == VarKind::FuncName || chain.contains(&var.scope) => {
                        Slot::Binding {
                            name,
                            scope: scope_pos(var.scope),
                        }
                    }
                    _ => Slot::Nothing,
                };
                if local {
                    // A slot shared by bindings of disjoint scopes holds the one in
                    // scope here.
                    if slot != Slot::Nothing || locals[i as usize] == Slot::Nothing {
                        boxed[i as usize] = in_box && slot != Slot::Nothing;
                        locals[i as usize] = slot;
                    }
                } else if let Some(a) = args.get_mut(i as usize) {
                    *a = Some(slot);
                }
            }
            let unsupported = match s.unsupported {
                Some(c) => Some(String::from(c)),
                // (A compiler bug: a construct that keeps a value without naming it.)
                None => match depth[k] {
                    Some(d) if d as usize == s.names.len() + 1 => None,
                    Some(d) => Some(alloc::format!(
                        "{} operands, {} named",
                        d.saturating_sub(1),
                        s.names.len()
                    )),
                    None => Some(String::from("unknown operand depth")),
                },
            };
            sites.push(Site {
                name: alloc::format!("{}#{}", s.kind.name(), number[k]),
                kind: s.kind,
                pos: s.pos.saturating_sub(base),
                resume_pc: resume,
                args: args
                    .into_iter()
                    .map(|a| a.unwrap_or(Slot::Nothing))
                    .collect(),
                locals,
                boxed,
                stack: if unsupported.is_some() {
                    Vec::new()
                } else {
                    s.names
                        .iter()
                        .map(|&(c, at)| alloc::format!("{c}@{}", at.saturating_sub(base)))
                        .collect()
                },
                unsupported,
                path: core::mem::take(&mut paths[j].0),
                gap: paths[j].1,
            });
        }
        let name = if f.name.is_null() {
            String::from("<anonymous>")
        } else {
            heap.atom_name(f.name)
        };
        FunctionMap {
            name,
            start: f.src_start,
            end: f.src_end,
            sites,
            computed_keys: rec.computed_keys.clone(),
            syntax,
        }
    }
}

// ---- The parser's hooks ----------------------------------------------------------------

#[cfg(feature = "dump")]
impl super::parser::Parser<'_, '_> {
    /// The construct keeps values named `cs` on the stack (pushed in this order; every
    /// construct has a `.`, so no name is a binding's `name@scope`); `at` is the source
    /// offset that names their slots (D2721): for a list's element (an argument, an
    /// array element, a template part) the element's own start, which stands for its
    /// index, else the start of the construct that keeps it.
    #[inline]
    pub(super) fn names_push(&mut self, at: u32, cs: &[&'static str]) {
        if let Some(r) = &mut self.f.smap {
            r.names.extend(cs.iter().map(|&c| (c, at)));
        }
    }

    /// The construct consumed the top `n` values it kept.
    #[inline]
    pub(super) fn names_pop(&mut self, n: usize) {
        if let Some(r) = &mut self.f.smap {
            let len = r.names.len().saturating_sub(n);
            r.names.truncate(len);
        }
    }

    /// Replaces the top `n` names by `cs` (an `array_from` collapsing its elements).
    #[inline]
    pub(super) fn names_replace(&mut self, n: usize, at: u32, cs: &[&'static str]) {
        self.names_pop(n);
        self.names_push(at, cs);
    }

    /// Suspensions are refused until `unsupported_end`: inside `construct`.
    #[inline]
    pub(super) fn unsupported_begin(&mut self, construct: &'static str) {
        if let Some(r) = &mut self.f.smap {
            r.unsupported.push(construct);
        }
    }

    #[inline]
    pub(super) fn unsupported_end(&mut self) {
        if let Some(r) = &mut self.f.smap {
            r.unsupported.pop();
        }
    }

    /// A suspension point: the `await`/`yield` instruction is emitted next.
    #[inline]
    pub(super) fn site(&mut self, kind: SiteKind, pos: u32) {
        let p1 = self.f.code.pos();
        let scope = self.scope;
        if let Some(r) = &mut self.f.smap {
            let names = r.names.clone();
            let unsupported = r.unsupported.last().copied();
            r.syn.site();
            r.sites.push(maps::SiteRec {
                kind,
                pos,
                p1,
                scope,
                names,
                unsupported,
            });
        }
    }
}

#[cfg(not(feature = "dump"))]
impl super::parser::Parser<'_, '_> {
    #[inline(always)]
    pub(super) fn names_push(&mut self, _at: u32, _cs: &[&'static str]) {}
    #[inline(always)]
    pub(super) fn names_pop(&mut self, _n: usize) {}
    #[inline(always)]
    pub(super) fn names_replace(&mut self, _n: usize, _at: u32, _cs: &[&'static str]) {}
    #[inline(always)]
    pub(super) fn unsupported_begin(&mut self, _construct: &'static str) {}
    #[inline(always)]
    pub(super) fn unsupported_end(&mut self) {}
    #[inline(always)]
    pub(super) fn site(&mut self, _kind: SiteKind, _pos: u32) {}
}
