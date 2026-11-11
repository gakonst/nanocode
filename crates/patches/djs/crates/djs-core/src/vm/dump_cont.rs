//! The continuation of a suspended activation (D2830, docs/plain-js-dumps.md §2): a
//! function of the original's kind whose body is the rest of the original from its
//! site, written from the original's text by the syntax the compiler recorded around
//! the site (D2850, compiler/smap_syntax.rs).
//!
//! The continuation's prefix declares the bindings live at the site with their values
//! (the dump's code for the frame's shared scopes goes into the block that declares
//! them, `Ctx::block`), then the site's statement rewritten: values evaluated before
//! the site come from the frame's temporaries, the site itself is `(await p)` (or a
//! placeholder `(yield)`), the rest is the original's text. Loops around the site are
//! scaffolds whose first iteration is the rest of the current one; `try`, `switch` and
//! labels are kept; then the rest of the function, verbatim.

use alloc::string::String;
use alloc::vec::Vec;

use super::plain::{Act, BDecl, Ctx, KText};
use super::{is_ident, join, quote};
use crate::buf::PushOut;
use crate::compiler::{DeclKind, Node, NodeKind, Role, Special};
use crate::interp::run::suspend;
use canon::Out;

#[path = "dump_cont_canon.rs"]
pub(super) mod canon;
#[path = "dump_cont_heads.rs"]
mod heads;
#[path = "dump_cont_pattern.rs"]
mod pattern;
#[path = "dump_cont_using.rs"]
mod using;
#[path = "dump_cont_yield.rs"]
mod yield_star;

type R<T> = Result<T, String>;

fn no<T>(why: &str) -> R<T> {
    Err(cat!("plain dumps: ", why))
}

/// Generated text: strings, verbatim spans of the original, marks.
#[derive(Clone)]
enum P {
    S(String),
    /// Text of K's own that is a constant (`s`): no allocation for it (0215).
    T(&'static str),
    V(u32, u32),
    /// A scaffold record (`scaf[i]`): its continuation offset is set here.
    Mark(usize),
    /// Where K's captured helpers are declared.
    Helpers,
    L(Vec<P>),
    /// A line that declares or assigns binding `.0` of the frame: left out where nothing
    /// in its scope reads the binding (liveness, D2911).
    Decl(String, String),
    /// A block of K: the names it declares, whether it is written in braces when one of
    /// them is left, its content.
    Scope(Vec<String>, bool, alloc::boxed::Box<P>),
    /// A temporary's literal written in place (`cmap[.0]`): a piece of its own, so that a
    /// re-dump reads the literal back from K's text (D2913).
    Lit(usize, String),
    /// A value of the dump written in K's text (the awaited operand): it reads none of K's
    /// bindings (D3001) but those the dump names there (`.1`, D3003), all liveness sees
    /// of it.
    Val(String, Vec<String>),
}

fn s(t: &'static str) -> P {
    P::T(t)
}

/// Writes activation `act`'s continuation, with what a re-dump needs to make its layout
/// again (D2913, dump_cont_canon.rs).
pub(super) fn generate(act: &Act, ctx: &mut dyn Ctx) -> R<KText> {
    canon::generate(act, ctx)
}

/// A continuation as pieces: generated text, copies of the original, marks. `replay`:
/// the tape of a first run, to write the same shape again.
fn run(act: &Act, ctx: &mut dyn Ctx, replay: Option<&[u32]>) -> R<Out> {
    if act.site.gap.is_some() {
        return no(
            "a site inside a construct the compiler does not record (a pattern, a class body)",
        );
    }
    if act.site.path.is_empty() {
        return no("no syntax recorded at the site");
    }
    if !matches!(
        act.site.kind,
        crate::compiler::SiteKind::Await
            | crate::compiler::SiteKind::Yield
            | crate::compiler::SiteKind::AsyncYield
            | crate::compiler::SiteKind::ForAwaitNext
            | crate::compiler::SiteKind::ForAwaitClose
            | crate::compiler::SiteKind::YieldStar
            | crate::compiler::SiteKind::YieldStarAwait
            | crate::compiler::SiteKind::AwaitUsing
    ) {
        return Err(cat!(
            "plain dumps: a site of kind ",
            act.site.name,
            " (0203)"
        ));
    }
    let arrow = act.fkind == crate::bytecode::template::func_flags::KIND_ARROW;
    for &(k, _) in &act.syntax.special {
        match k {
            // An async function or generator is never constructed: `new.target` is
            // undefined in K too. An arrow's is its context's.
            Special::NewTarget if arrow => return no("new.target in an arrow (0203)"),
            Special::Super if act.super_proto.is_none() => {
                return no("super outside a method (0203)");
            }
            _ => {}
        }
    }
    let mut g = G {
        act,
        ctx,
        used: Vec::new(),
        scaf: Vec::new(),
        cmap: Vec::new(),
        temps: Vec::new(),
        lets: Vec::new(),
        ntemps: 0,
        block_index: 0,
        uses_arguments: false,
        arrow: act.fkind == crate::bytecode::template::func_flags::KIND_ARROW,
        params: Vec::new(),
        deferred: Vec::new(),
        marks: true,
        tape: Vec::new(),
        replay: replay.map(|r| (r.to_vec(), 0)),
        aliases: Vec::new(),
        fdjs: core::cell::OnceCell::new(),
    };
    let mut body = g.body()?;
    if !g.deferred.is_empty() {
        return no("a value the dump's callback makes in a block the continuation does not reach");
    }
    // Liveness (D2911): bindings nothing in K reads are left out, unless K may read names
    // in ways its text does not show.
    let mut all = String::with_capacity(act.ftext.len() + 256);
    render(&body, &act.ftext, "", &mut all);
    if !has_ident(&all, "eval") && !has_ident(&all, "with") {
        prune(&mut body, &act.ftext);
        let mut text = String::with_capacity(all.len());
        render(&body, &act.ftext, "", &mut text);
        let params = core::mem::take(&mut g.params);
        for (n, v) in params {
            if has_ident(&text, &n) || !g.unique(&n) {
                g.params.push_out((n, v));
            }
        }
    }
    // The body's `using` resources are disposed at its end: a site there.
    let close = if g
        .node(0)
        .decls
        .iter()
        .any(|d| matches!(d.kind, DeclKind::Using | DeclKind::AwaitUsing))
    {
        let end = act.ftext.len() as u32;
        P::L(alloc::vec![s("\n}"), g.mark("site", end)])
    } else {
        s("\n}")
    };
    // The frame's values the body declares are K's parameters, its call's arguments
    // (D2909).
    let names: Vec<String> = g.params.iter().map(|p| p.0.clone()).collect();
    let ps = join(&names, ", ");
    let head = match (g.arrow, act.kind) {
        (true, suspend::ASYNC) => cat!("async (", ps, ") => {\n"),
        (true, _) => cat!("(", ps, ") => {\n"),
        (false, suspend::ASYNC) => cat!("async function (", ps, ") {\n"),
        (false, suspend::GENERATOR) => cat!("function* (", ps, ") {\n"),
        (false, _) => cat!("async function* (", ps, ") {\n"),
    };
    let args: Vec<String> = g.params.iter().map(|p| p.1.clone()).collect();
    let mut top = names.clone();
    top.extend(g.node(0).decls.iter().map(|d| d.name.clone()));
    if let Some((tape, at)) = &g.replay
        && *at < tape.len()
    {
        return Err(cat!(
            "a tape of ",
            tape.len(),
            " decisions, ",
            *at,
            " taken"
        ));
    }
    match &act.super_proto {
        // A method of an object whose prototype is the original's home object's: its
        // `super` reads what the original's does. Offsets (`segs`, `scaf`) are in the
        // method's own text, which is what origin tracking reads back.
        Some(p) => {
            let m = match act.kind {
                suspend::ASYNC => cat!("async K(", ps, ") {\n"),
                suspend::GENERATOR => cat!("*K(", ps, ") {\n"),
                _ => cat!("async *K(", ps, ") {\n"),
            };
            let mut k = g.flatten(P::L(alloc::vec![P::S(m), P::Helpers, body, close]));
            k.wrap = Some(String::from(p));
            k.args = args;
            k.top = top;
            Ok(k)
        }
        None => {
            let mut k = g.flatten(P::L(alloc::vec![P::S(head), P::Helpers, body, close]));
            k.args = args;
            k.top = top;
            Ok(k)
        }
    }
}

/// Whether `text` may assign the binding `name` (conservatively: an assignment or update
/// operator after or an update operator before an occurrence, an occurrence before `of`
/// or `in`, or inside brackets an assignment follows, a destructuring target).
fn may_assign(text: &str, name: &str) -> bool {
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'$';
    let b = text.as_bytes();
    let skip = |mut j: usize| {
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        j
    };
    // An `=` that assigns: not `==`, `===` or `=>`.
    let assigns_at =
        |j: usize| b.get(j) == Some(&b'=') && !matches!(b.get(j + 1), Some(b'=') | Some(b'>'));
    let mut from = 0;
    while let Some(at) = text.get(from..).and_then(|t| t.find(name)) {
        let (s, e) = (from + at, from + at + name.len());
        from = s + 1;
        if (s > 0 && ident(b[s - 1])) || (e < b.len() && ident(b[e])) {
            continue;
        }
        let mut p = s;
        while p > 0 && b[p - 1].is_ascii_whitespace() {
            p -= 1;
        }
        if p >= 2 && (&b[p - 2..p] == b"++" || &b[p - 2..p] == b"--") {
            return true;
        }
        // (A declaration's own initializer is no assignment.)
        let before = &text[..p];
        if ["const", "let", "var"].iter().any(|kw| {
            before.ends_with(kw)
                && !before[..before.len() - kw.len()]
                    .ends_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '$')
        }) {
            continue;
        }
        let j = skip(e);
        let rest = &b[j..];
        if assigns_at(j)
            || rest.starts_with(b"++")
            || rest.starts_with(b"--")
            || rest.starts_with(b"of")
            || rest.starts_with(b"in")
        {
            return true;
        }
        for op in [
            "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "<<=", ">>=", ">>>=", "**=", "&&=",
            "||=", "??=",
        ] {
            if rest.starts_with(op.as_bytes()) {
                return true;
            }
        }
        // Out of the brackets around it: an assignment after one is a pattern's.
        let mut depth = 0i32;
        let mut k = j;
        while k < b.len() && k < j + 4000 {
            match b[k] {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => {
                    depth -= 1;
                    if depth < 0 {
                        let n = skip(k + 1);
                        if assigns_at(n) {
                            return true;
                        }
                        if !matches!(b.get(n), Some(b',') | Some(b']') | Some(b'}') | Some(b')')) {
                            break;
                        }
                        depth = 0;
                        k = n;
                        continue;
                    }
                }
                b';' if depth == 0 => break,
                _ => {}
            }
            k += 1;
        }
    }
    false
}

/// Whether `text` holds the identifier `name` (not as part of a longer one).
fn has_ident(text: &str, name: &str) -> bool {
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'$';
    let (b, n) = (text.as_bytes(), name.as_bytes());
    let Some(&first) = n.first() else {
        return false;
    };
    // (By its first byte and a compare, not `str::find`: the texts and names are short,
    // and a searcher's setup was most of the time, 0215.)
    let mut from = 0;
    while from + n.len() <= b.len() {
        let Some(s) = super::plain::find_byte(b, from, first).filter(|&s| s + n.len() <= b.len())
        else {
            return false;
        };
        let e = s + n.len();
        if &b[s..e] == n && (s == 0 || !ident(b[s - 1])) && (e == b.len() || !ident(b[e])) {
            return true;
        }
        from = s + 1;
    }
    false
}

struct G<'a, 'c> {
    act: &'a Act,
    ctx: &'c mut dyn Ctx,
    used: Vec<String>,
    /// Scaffold constructs: kind, offset in the original, offset in K.
    /// Scaffold constructs: kind, offset in the original, offset in K, and what else
    /// origin tracking needs of it (a finally's completion: `, kind, "binding"`).
    scaf: Vec<(&'static str, u32, u32, String)>,
    /// The original's temporaries of the rewritten statement: slot, and the binding of K
    /// that holds the value or the literal written in its place.
    cmap: Vec<(String, String, bool)>,
    /// Temporaries of the site's statement not yet declared.
    temps: Vec<String>,
    /// Variables the site's statement assigns (`let`, declared with the temporaries).
    lets: Vec<String>,
    ntemps: usize,
    block_index: usize,
    uses_arguments: bool,
    arrow: bool,
    /// The body's bindings K takes as parameters: name, the argument (D2909).
    params: Vec<(String, String)>,
    /// Whether K can stop again in the expression being written after its site: only then
    /// does origin tracking need the records of K's own constructs (`k`, `tmp`).
    marks: bool,
    /// Assignments that wait for a part of the dump's callback (D2907): the part, the
    /// statement.
    deferred: Vec<(usize, Vec<String>, P)>,
    /// The decisions taken on values that shape K (D2913), in order: what a re-dump
    /// replays to write the same pieces again.
    tape: Vec<u32>,
    /// Writing again from a tape: it, and how far it was read.
    replay: Option<(Vec<u32>, usize)>,
    /// Temporaries whose literal is another's: `(cmap index, the other's)`.
    aliases: Vec<(usize, usize)>,
    /// Whether the function's text holds `$djs` at all (`G::f_names`): K's own names
    /// all start with it, and checking each against the text was a scan each (0215).
    fdjs: core::cell::OnceCell<bool>,
}

impl G<'_, '_> {
    /// [`has_ident`] of the function's text.
    fn f_names(&self, name: &str) -> bool {
        if name.starts_with("$djs") && !*self.fdjs.get_or_init(|| self.act.ftext.contains("$djs")) {
            return false;
        }
        has_ident(&self.act.ftext, name)
    }
}

impl G<'_, '_> {
    fn ftext(&self) -> &str {
        &self.act.ftext
    }

    /// `base`, or `base_k` if the function's text names it already (a continuation
    /// of a continuation has the first one's scaffold names).
    fn fresh(&self, base: String) -> String {
        if !self.f_names(&base) {
            return base;
        }
        let mut k = 1;
        loop {
            let n = cat!(base, "_", k);
            if !self.f_names(&n) {
                return n;
            }
            k += 1;
        }
    }

    fn node(&self, i: usize) -> &Node {
        let step = self.act.site.path[i];
        &self.act.syntax.nodes[step.node as usize]
    }

    /// The part of path node `i` that holds node `i + 1`.
    fn next_part(&self, i: usize) -> Option<usize> {
        self.act.site.path[i].part.map(usize::from)
    }

    fn is_site(&self, i: usize) -> bool {
        i + 1 == self.act.site.path.len()
    }

    fn text(&self, a: u32, b: u32) -> &str {
        self.ftext().get(a as usize..b as usize).unwrap_or("")
    }

    fn mark(&mut self, kind: &'static str, f: u32) -> P {
        // A record of K's own construct only where K can stop again inside it.
        if !self.marks && matches!(kind, "k" | "tmp") {
            return s("");
        }
        self.scaf.push_out((kind, f, 0, String::new()));
        P::Mark(self.scaf.len() - 1)
    }

    /// Decision `v`, taken on a value, that shapes K: recorded on the tape, or, writing
    /// again, the recorded one (D2913). Past the tape's end: 0 (the record leaves out
    /// the tape's trailing zeros).
    fn steer(&mut self, v: u32) -> u32 {
        match &mut self.replay {
            Some((tape, at)) => {
                let v = tape.get(*at).copied().unwrap_or(0);
                *at += 1;
                v
            }
            None => {
                self.tape.push_out(v);
                v
            }
        }
    }

    /// The site's awaited operand; the names K may bind where it stands go with the
    /// question (a value written there in place must not read them, D3001).
    fn awaiting(&mut self) -> String {
        let mut scope: Vec<String> = self.params.iter().map(|p| p.0.clone()).collect();
        for st in &self.act.site.path {
            for d in &self.act.syntax.nodes[st.node as usize].decls {
                scope.push_out(d.name.clone());
            }
        }
        for s in &self.act.slots {
            scope.push_out(String::from(s.name.split('@').next().unwrap_or(&s.name)));
        }
        scope.push_out(String::from("arguments"));
        self.ctx.awaiting(&scope)
    }

    /// A completion's kind (a number, D2733), a decision on the tape.
    fn steer_kind(&mut self, kind: &str) -> R<String> {
        let k = match kind.parse::<u32>() {
            Ok(k) if self.replay.is_none() => k,
            _ if self.replay.is_some() => 0,
            _ => return Err(cat!("plain dumps: a completion of kind ", kind)),
        };
        Ok(cat!(self.steer(k)))
    }

    /// Whether K evaluates temporary `slot` again (D2836), a decision on the tape.
    fn rereads(&mut self, slot: &str) -> bool {
        let v = self.replay.is_none() && self.ctx.rereads(slot);
        self.steer(u32::from(v)) == 1
    }

    fn has(&mut self, name: &str) -> bool {
        self.act.slots.iter().any(|s| s.name == name)
    }

    fn take(&mut self, name: &str) -> R<String> {
        match self.ctx.slot(name) {
            Some(v) => {
                if !self.used.iter().any(|u| u == name) {
                    self.used.push_out(String::from(name));
                }
                Ok(v)
            }
            None => Err(cat!("plain dumps: missing slot ", name)),
        }
    }

    /// Whether the dump's callback makes what `text` reads only in a part of a block
    /// below the current one (D2907).
    fn made_later(&self, text: &str) -> bool {
        let cur = self.block_index.saturating_sub(1);
        self.ctx.made_in(text).is_some_and(|p| p > cur)
    }

    /// A loop scaffold's constants, `const a = x, b = y;`: declared with `let` and
    /// assigned once the part of the dump's callback that makes what they read has run,
    /// where that is a block below the current one (D2907).
    fn scaffold_consts(&mut self, pairs: &[(String, String)]) -> String {
        let cur = self.block_index.saturating_sub(1);
        let later = pairs
            .iter()
            .filter_map(|(_, v)| self.ctx.made_in(v))
            .max()
            .filter(|&p| p > cur);
        let list: Vec<String> = pairs.iter().map(|(n, v)| cat!(n, " = ", v)).collect();
        match later {
            Some(p) => {
                let names: Vec<String> = pairs.iter().map(|(n, _)| n.clone()).collect();
                self.deferred
                    .push_out((p, names.clone(), P::S(cat!(join(&list, ", "), ";"))));
                cat!("let ", join(&names, ", "), ";")
            }
            // Values the dump has before K runs: parameters of K, its call's arguments
            // (D2917, as the body's bindings, D2909).
            None if pairs
                .iter()
                .all(|(_, v)| self.ctx.made_in(v).is_none() && !has_factory_ref(v)) =>
            {
                for (n, v) in pairs {
                    self.params.push_out((n.clone(), v.clone()));
                }
                String::new()
            }
            None => cat!("const ", join(&list, ", "), ";"),
        }
    }

    /// A loop's saved `next` method; `undefined` where the frame has none: the compiler
    /// leaves out a `next` no later step calls (a body that always leaves the loop), and
    /// the scaffold's helper then never calls it either.
    fn next_method(&mut self, slot: &str) -> R<String> {
        if self.has(slot) {
            self.take(slot)
        } else {
            Ok(String::from("undefined"))
        }
    }

    /// A temporary of the site's statement: a literal in place, else a const declared
    /// right before the statement (the dump's values are reachable only while it loads).
    /// (A re-dump reads a literal back from K's text, D2913: a string from K's
    /// constants.)
    fn tmp(&mut self, construct: &str, off: u32) -> R<P> {
        let slot = cat!(construct, "@", off);
        let v = self.take(&slot)?;
        // (Not an index's string: K may use it as an integer key, no string of its own.)
        let index = v.len() > 2 && v[1..v.len() - 1].bytes().all(|b| b.is_ascii_digit());
        let lit = literal(&v) && !(v.starts_with('"') && index);
        if self.steer(u32::from(lit)) == 1 {
            self.cmap.push_out((slot, v.clone(), true));
            return Ok(P::Lit(self.cmap.len() - 1, v));
        }
        Ok(P::S(self.temp_of(slot, v)))
    }

    /// [`G::tmp`] as text, which K uses inside text of its own: always a constant.
    fn tmp_text(&mut self, construct: &str, off: u32) -> R<String> {
        let slot = cat!(construct, "@", off);
        let v = self.take(&slot)?;
        Ok(self.temp_of(slot, v))
    }

    fn temp_of(&mut self, slot: String, v: String) -> String {
        let name = self.new_temp();
        self.temps.push_out(cat!(name, " = ", v));
        self.cmap.push_out((slot, name.clone(), false));
        name
    }

    /// A fresh name for a temporary of the site's statement.
    fn new_temp(&mut self) -> String {
        loop {
            let name = cat!("$djs_t", self.ntemps);
            self.ntemps += 1;
            let taken = |l: &Vec<String>| {
                l.iter()
                    .any(|t| t == &name || t.starts_with(&cat!(name, " ")))
            };
            if !self.f_names(&name) && !taken(&self.temps) && !taken(&self.lets) {
                return name;
            }
        }
    }

    fn flush_temps(&mut self) -> P {
        let mut t = String::new();
        if !self.temps.is_empty() {
            t = cat!("const ", join(&self.temps, ", "), ";\n");
        }
        if !self.lets.is_empty() {
            cat_to!(&mut t, "let ", join(&self.lets, ", "), ";\n");
        }
        self.temps.clear();
        self.lets.clear();
        if t.is_empty() { s("") } else { P::S(t) }
    }

    /// The statement at the site, after the temporaries. (The dump's callback ran at
    /// the ends of the blocks, D2907.)
    fn with_go(&mut self, parts: P) -> P {
        let t = self.flush_temps();
        P::L(alloc::vec![t, parts])
    }

    /// The original's text `[a, b)`, with `arguments` renamed (the continuation's own
    /// would be another object).
    fn rest(&mut self, a: u32, b: u32) -> P {
        if self.arrow {
            return P::V(a, b);
        }
        let mut out = Vec::new();
        let mut at = a;
        for &(k, off) in &self.act.syntax.special {
            if k == Special::Arguments && off >= a && off < b {
                out.push_out(P::V(at, off));
                out.push_out(s("$djs_arguments"));
                at = off + "arguments".len() as u32;
                self.uses_arguments = true;
            }
        }
        if out.is_empty() {
            return P::V(a, b);
        }
        out.push_out(P::V(at, b));
        P::L(out)
    }

    fn rest_part(&mut self, n: &Node, k: usize) -> P {
        let p = n.parts[k];
        self.rest(p.start, p.end)
    }

    // ---------------------------------------------------------------- bindings

    /// The slot of binding `name` declared by scope `scope`.
    fn binding_slot(&mut self, name: &str, scope: u32) -> Option<String> {
        let shadowed = cat!(name, "@", scope);
        if self.has(&shadowed) {
            return Some(shadowed);
        }
        self.has(name).then(|| String::from(name))
    }

    /// The declarations of a block of K: the live bindings with their values, the dump's
    /// code for this block, bindings whose values exist only after the dump's callback.
    fn declare(&mut self, decls: &[(String, DeclKind, u32, bool)]) -> R<P> {
        let mut early: Vec<P> = Vec::new();
        let mut late: Vec<P> = Vec::new();
        // (The names `late` binds and their values, by line; the place of each line of
        // `late` and of the bindings of `early` among the declarations.)
        let mut late_names: Vec<(String, String)> = Vec::new();
        let mut late_seq: Vec<usize> = Vec::new();
        let mut early_info: Vec<Option<(usize, String, String)>> = Vec::new();
        let mut seq = 0;
        let mut after: Vec<P> = Vec::new();
        let mut info = Vec::new();
        let block = self.block_index;
        // (A waiting assignment must reach the binding it was made for.)
        if self
            .deferred
            .iter()
            .any(|(_, names, _)| decls.iter().any(|d| names.contains(&d.0)))
        {
            return no("a binding the dump assigns late, shadowed in a block below");
        }
        for (name, kind, scope, live) in decls {
            seq += 1;
            let sname = self.binding_slot(name, *scope);
            info.push_out(BDecl {
                name: name.clone(),
                sname: sname.clone(),
            });
            let (true, Some(sname)) = (*live, sname) else {
                continue;
            };
            // A binding of the function's body with its value at hand: a parameter of K,
            // its value the call's argument (D2909). Not one shared with closures K's
            // blocks make (a cell), one taking what the dump makes later, a `using`
            // resource, or a constant the text may assign (which throws).
            let constant = matches!(kind, DeclKind::Const | DeclKind::FuncName);
            let made = self
                .ctx
                .cell(&sname)
                .flatten()
                .or_else(|| self.ctx.arg(&sname))
                .and_then(|v| self.ctx.made_in(&v));
            if block == 0
                && made.is_none()
                && name != "arguments"
                && !matches!(kind, DeclKind::Using | DeclKind::AwaitUsing)
                && !self.ctx.after_go(&sname)
                && self.ctx.cell(&sname).is_none()
                && !(constant && may_assign(self.ftext(), name))
                && let Some(v) = self.ctx.arg(&sname)
                && v != "$dump.TDZ"
                && !has_factory_ref(&v)
            {
                self.used.push_out(sname);
                self.params.push_out((name.clone(), v));
                continue;
            }
            let value = match self.ctx.cell(&sname) {
                Some(None) => {
                    self.used.push_out(sname);
                    continue;
                }
                Some(Some(init)) => init,
                None => match self.ctx.slot(&sname) {
                    Some(v) if v == "$dump.TDZ" => {
                        self.used.push_out(sname);
                        continue;
                    }
                    Some(v) => v,
                    None => continue,
                },
            };
            self.used.push_out(sname.clone());
            // (A `using` binding's resource is added again by the scope, D2876.)
            let kw = match kind {
                DeclKind::Const | DeclKind::FuncName | DeclKind::Using | DeclKind::AwaitUsing => {
                    "const"
                }
                // A function declared in a block is the block's (strict code), as K's
                // must be for origin tracking to find it there.
                DeclKind::Function if *scope != 0 => "let",
                DeclKind::Var | DeclKind::Param | DeclKind::Function => "var",
                _ => "let",
            };
            let target = if name == "arguments" {
                self.uses_arguments = true;
                "$djs_arguments"
            } else {
                name.as_str()
            };
            // (A `using` resource's scope ends where its block does: never left out.)
            let using = matches!(kind, DeclKind::Using | DeclKind::AwaitUsing);
            let line = if using {
                P::S(cat!(kw, " ", target, " = ", value, ";"))
            } else {
                self.decl_line(target, cat!(kw, " ", target, " = ", value, ";"))
            };
            if let Some(p) = made.filter(|&p| p > block) {
                // Made by a part of the dump's callback in a block below: declared now,
                // assigned once that part ran (D2907).
                if kw == "const" && may_assign(self.ftext(), name) {
                    return no("a constant whose value the dump makes in a block below its own");
                }
                let kw = if kw == "const" { "let" } else { kw };
                early.push_out(self.decl_line(target, cat!(kw, " ", target, ";")));
                early_info.push_out(None);
                let assign = self.decl_line(target, cat!(target, " = ", value, ";"));
                self.deferred
                    .push_out((p, alloc::vec![String::from(target)], assign));
            } else if self.ctx.after_go(&sname) || made == Some(block) {
                after.push_out(line);
            } else if has_factory_ref(&value) {
                late.push_out(line);
                late_names.push_out((String::from(target), value.clone()));
                late_seq.push_out(seq);
            } else {
                early.push_out(line);
                early_info.push_out(Some((seq, String::from(target), value)));
            }
        }
        let content = self.ctx.block(block, &info)?;
        self.block_index += 1;
        // A binding whose value reads a name the dump's code here declares (a class's
        // local, `$c30`, in `((C2) => class D3 extends C2 {…})($c30)`) is late too: after
        // that code, among the late ones in its order (0215).
        let declared = declared_names(&content);
        if !declared.is_empty() && early_info.iter().any(Option::is_some) {
            let mut kept = Vec::new();
            let mut all: Vec<(usize, P, (String, String))> = late
                .into_iter()
                .zip(late_seq)
                .zip(late_names)
                .map(|((p, s), n)| (s, p, n))
                .collect();
            for (p, i) in early.into_iter().zip(early_info) {
                match i {
                    Some((s, target, value))
                        if super::plain::eager_names(&value)
                            .iter()
                            .any(|n| declared.contains(&n.as_str())) =>
                    {
                        all.push_out((s, p, (target, value)));
                    }
                    _ => kept.push_out(p),
                }
            }
            crate::sort::sort_by(&mut all, |a, b| a.0 < b.0);
            early = kept;
            late = Vec::new();
            late_names = Vec::new();
            for (_, p, n) in all {
                late.push_out(p);
                late_names.push_out(n);
            }
        }
        let mut lines = early;
        // The dump's code and the late bindings, in an order where each binding comes after
        // the code that makes what it reads and a line of code after the bindings it reads
        // (D3150).
        let shown = content.iter().any(|l| !l.is_empty());
        let mut late: Vec<Option<P>> = late.into_iter().map(Some).collect();
        for item in block_order(&content, &late_names) {
            match item {
                Ok(k) if shown => lines.push_out(P::S(content[k].clone())),
                Ok(_) => {}
                Err(i) => {
                    if let Some(p) = late[i].take() {
                        lines.push_out(p);
                    }
                }
            }
        }
        // The part of the dump's callback that needs this block's code, before the
        // bindings that take what it makes and the code nested in the block (D2907).
        let go = self.ctx.go(block);
        if !go.is_empty() {
            lines.push_out(P::S(go));
        }
        // What waited for this part (scaffolds and bindings of the blocks above).
        for (p, names, st) in core::mem::take(&mut self.deferred) {
            if p == block {
                lines.push_out(st);
            } else {
                self.deferred.push_out((p, names, st));
            }
        }
        lines.extend(after);
        let mut out = Vec::new();
        for l in lines {
            if !out.is_empty() {
                out.push_out(s("\n"));
            }
            out.push_out(l);
        }
        Ok(P::L(out))
    }

    /// A line of binding `name`: prunable where the frame has one binding of the name
    /// (origin tracking reads a binding K leaves out as one not yet initialized).
    fn decl_line(&self, name: &str, line: String) -> P {
        if self.unique(name) {
            P::Decl(String::from(name), line)
        } else {
            P::S(line)
        }
    }

    /// Whether the frame has one binding of `name` (none of `name@scope`).
    fn unique(&self, name: &str) -> bool {
        let prefix = cat!(name, "@");
        name != "$djs_arguments"
            && !self.act.slots.iter().any(|s| {
                s.name
                    .strip_prefix(prefix.as_str())
                    .is_some_and(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit()))
            })
    }

    /// The bindings of the function body, statement `j` holding the site.
    fn body_decls(&mut self, n: &Node, j: Option<usize>) -> Vec<(String, DeclKind, u32, bool)> {
        let mut out: Vec<(String, DeclKind, u32, bool)> = Vec::new();
        for d in &n.decls {
            let live = match d.kind {
                DeclKind::Param | DeclKind::Var | DeclKind::Function | DeclKind::FuncName => true,
                _ => d
                    .part
                    .is_some_and(|p| j.is_some_and(|j| usize::from(p) < j)),
            };
            if !out.iter().any(|o| o.0 == d.name) {
                out.push_out((d.name.clone(), d.kind, d.scope, live));
            }
        }
        if !self.arrow && self.has("arguments") && !out.iter().any(|o| o.0 == "arguments") {
            out.push_out((String::from("arguments"), DeclKind::Var, 0, true));
        }
        out
    }

    fn block_decls(n: &Node, j: usize) -> Vec<(String, DeclKind, u32, bool)> {
        n.decls
            .iter()
            .map(|d| {
                let live =
                    d.kind == DeclKind::Function || d.part.is_some_and(|p| usize::from(p) < j);
                (d.name.clone(), d.kind, d.scope, live)
            })
            .collect()
    }

    // ---------------------------------------------------------------- statements

    fn body(&mut self) -> R<P> {
        let n = self.node(0).clone();
        let Some(k) = self.next_part(0) else {
            // The body's `using` resources being disposed.
            return self.using_disposal(0);
        };
        if n.parts[k].role == Role::Operand {
            // An arrow's concise body.
            let decls = self.body_decls(&n, None);
            let d = self.declare(&decls)?;
            let e = self.expr(1)?;
            let st = self.with_go(P::L(alloc::vec![s("return "), e, s(";")]));
            return Ok(P::L(alloc::vec![d, s("\n"), st]));
        }
        let decls = self.body_decls(&n, Some(k));
        self.list(&n, k, 1, &decls)
    }

    /// A statement list from part `k` (holding path node `i`) on.
    fn list(
        &mut self,
        n: &Node,
        k: usize,
        i: usize,
        decls: &[(String, DeclKind, u32, bool)],
    ) -> R<P> {
        let d = self.declare(decls)?;
        let u = self.using_readd(i - 1)?;
        let st = self.stmt(i, None)?;
        let mut out = alloc::vec![d, s("\n"), u, st];
        for m in k + 1..n.parts.len() {
            match n.parts[m].role {
                Role::Stmt => {
                    out.push_out(s("\n"));
                    out.push_out(self.rest_part(n, m));
                }
                // Hoisted: a binding of the prefix.
                Role::FuncDecl => {}
                // The next case clause of a switch: its statements follow (fall-through).
                Role::Case | Role::Default => {}
                _ => {}
            }
        }
        Ok(P::L(out))
    }

    fn stmt(&mut self, i: usize, label: Option<P>) -> R<P> {
        let n = self.node(i).clone();
        let k = self.next_part(i);
        let lbl = |l: Option<P>| l.unwrap_or_else(|| s(""));
        match n.kind {
            NodeKind::Block if self.is_site(i) => {
                // Its `using` resources being disposed.
                let d = self.using_disposal(i)?;
                Ok(P::L(alloc::vec![lbl(label), d]))
            }
            NodeKind::Block => {
                let k = k.ok_or("plain dumps: a block at the site")?;
                let decls = Self::block_decls(&n, k);
                let l = self.list(&n, k, i + 1, &decls)?;
                // After the `}`: a `using` scope's disposal is a site there.
                let end = if decls
                    .iter()
                    .any(|d| matches!(d.1, DeclKind::Using | DeclKind::AwaitUsing))
                {
                    self.mark("site", n.end)
                } else {
                    s("")
                };
                // A block that declares nothing and is no label's needs no braces where
                // K writes it among statements (not a `try`'s or a `catch`'s).
                let listed = i > 0
                    && matches!(
                        self.node(i - 1).kind,
                        NodeKind::Body
                            | NodeKind::Block
                            | NodeKind::If
                            | NodeKind::While
                            | NodeKind::DoWhile
                            | NodeKind::For
                            | NodeKind::ForIn
                            | NodeKind::ForOf
                            | NodeKind::ForAwait
                            | NodeKind::Switch
                    );
                let names: Vec<String> = decls.iter().map(|d| d.0.clone()).collect();
                let using = decls
                    .iter()
                    .any(|d| matches!(d.1, DeclKind::Using | DeclKind::AwaitUsing));
                if label.is_none() && listed && !using {
                    // (Braces only where something it declares is left, D2911.)
                    let b = P::Scope(names, true, alloc::boxed::Box::new(l));
                    return Ok(P::L(alloc::vec![b, end]));
                }
                let b = P::Scope(names, false, alloc::boxed::Box::new(l));
                Ok(P::L(alloc::vec![lbl(label), s("{\n"), b, s("\n}"), end]))
            }
            NodeKind::Expr => {
                let e = self.expr(i + 1)?;
                Ok(self.with_go(P::L(alloc::vec![e, s(";")])))
            }
            NodeKind::Return | NodeKind::Throw => {
                let e = self.expr(i + 1)?;
                let kw = if n.kind == NodeKind::Return {
                    "return "
                } else {
                    "throw "
                };
                Ok(self.with_go(P::L(alloc::vec![s(kw), e, s(";")])))
            }
            NodeKind::Decl(kind) => self.decl_stmt(i, &n, kind),
            NodeKind::If => {
                let k = k.unwrap_or(0);
                if n.parts[k].role != Role::Test {
                    return self.stmt(i + 1, label);
                }
                let e = self.expr(i + 1)?;
                let mut out = alloc::vec![s("if ("), e, s(") ")];
                for m in 0..n.parts.len() {
                    match n.parts[m].role {
                        Role::Then => out.push_out(self.rest_part(&n, m)),
                        Role::Else => {
                            out.push_out(s(" else "));
                            out.push_out(self.rest_part(&n, m));
                        }
                        _ => {}
                    }
                }
                Ok(self.with_go(P::L(out)))
            }
            NodeKind::Label => {
                let lp = n
                    .parts
                    .iter()
                    .find(|p| p.role == Role::Label)
                    .copied()
                    .ok_or("plain dumps: a label without its name")?;
                let l = P::L(alloc::vec![P::V(lp.start, lp.end), s(": ")]);
                let inner = self.node(i + 1).kind;
                if matches!(
                    inner,
                    NodeKind::While
                        | NodeKind::DoWhile
                        | NodeKind::For
                        | NodeKind::ForOf
                        | NodeKind::ForIn
                        | NodeKind::ForAwait
                ) {
                    // On the scaffold's loop, so `continue label` still works.
                    self.stmt(i + 1, Some(l))
                } else {
                    let st = self.stmt(i + 1, None)?;
                    Ok(P::L(alloc::vec![l, st]))
                }
            }
            NodeKind::While | NodeKind::DoWhile => {
                let k = k.unwrap_or(0);
                if n.parts[k].role == Role::Test {
                    return self.loop_test(i, &n, label);
                }
                if n.parts[k].role != Role::Body {
                    return no("a site in a loop's test (0203)");
                }
                // The rest of the iteration, then the loop as written, where nothing in the
                // body jumps to the loop (D2917, as D2908 for for-of).
                let bt = self.text(n.parts[k].start, n.parts[k].end);
                if !has_ident(bt, "break") && !has_ident(bt, "continue") {
                    let first = self.stmt(i + 1, None)?;
                    let test = n.parts.iter().position(|p| p.role == Role::Test);
                    let lp = match (n.kind, test) {
                        (NodeKind::While, _) => self.rest(n.start, n.end),
                        (_, Some(t)) => P::L(alloc::vec![
                            s("while ("),
                            self.rest_part(&n, t),
                            s(") "),
                            self.rest_part(&n, k)
                        ]),
                        (_, None) => return no("a do-while without its test"),
                    };
                    return Ok(P::L(alloc::vec![first, s("\n"), lbl(label), lp]));
                }
                let f = self.fresh(cat!("$djs_f", i));
                let test = n.parts.iter().position(|p| p.role == Role::Test);
                let test = match test {
                    Some(t) => self.rest_part(&n, t),
                    None => s("true"),
                };
                let body = self.rest_part(&n, k);
                let m = self.mark(
                    if n.kind == NodeKind::While {
                        "while"
                    } else {
                        "do"
                    },
                    n.start,
                );
                let first = self.stmt(i + 1, None)?;
                Ok(if n.kind == NodeKind::While {
                    P::L(alloc::vec![
                        P::S(cat!("{ let ", f, " = true; ")),
                        lbl(label),
                        m,
                        P::S(cat!("while (", f, " || (")),
                        test,
                        P::S(cat!(")) { if (", f, ") { ", f, " = false; ")),
                        first,
                        s(" } else "),
                        body,
                        s(" } }"),
                    ])
                } else {
                    P::L(alloc::vec![
                        P::S(cat!("{ let ", f, " = true; ")),
                        lbl(label),
                        m,
                        P::S(cat!("do { if (", f, ") { ", f, " = false; ")),
                        first,
                        s(" } else "),
                        body,
                        s(" } while ("),
                        test,
                        s("); }"),
                    ])
                })
            }
            NodeKind::For => self.for_stmt(i, &n, label),
            NodeKind::ForOf | NodeKind::ForIn => self.for_of(i, &n, label),
            NodeKind::ForAwait => self.for_await(i, &n, label),
            NodeKind::Try => self.try_stmt(i, &n),
            NodeKind::Switch => self.switch_stmt(i, &n),
            _ => Err(cat!(
                "plain dumps: a statement around the site: ",
                kind_name(n.kind)
            )),
        }
    }

    fn decl_stmt(&mut self, i: usize, n: &Node, kind: DeclKind) -> R<P> {
        let mut out = self.decl_list(i, n, kind)?;
        out.push_out(s(";"));
        Ok(self.with_go(P::L(out)))
    }

    /// Declaration path node `i` with the site in it, without its `;` (also a `for`'s
    /// head). A `var`'s names are declared by K's prefix: it is assignments.
    pub(super) fn decl_list(&mut self, i: usize, n: &Node, kind: DeclKind) -> R<Vec<P>> {
        if kind == DeclKind::Var {
            return self.var_list(i, n);
        }
        let kw = match kind {
            DeclKind::Let => "let ",
            DeclKind::Const => "const ",
            DeclKind::Using => "using ",
            DeclKind::AwaitUsing => "await using ",
            _ => return no("a declaration kind the exporter does not know"),
        };
        let using = matches!(kind, DeclKind::Using | DeclKind::AwaitUsing);
        // A `using` declaration's resources before the site are the scope's already (K
        // adds them again at its start): their bindings are constants of their own.
        let mut before: Vec<P> = Vec::new();
        let Some(k) = self.next_part(i) else {
            return no("a declaration at the site");
        };
        if !matches!(n.parts[k].role, Role::Init | Role::Target) {
            return no("a site in a declaration the exporter does not know");
        }
        let mut items: Vec<P> = Vec::new();
        let mut m = 0;
        while m < n.parts.len() {
            let t = n.parts[m];
            if t.role != Role::Target {
                m += 1;
                continue;
            }
            let init =
                (m + 1 < n.parts.len() && n.parts[m + 1].role == Role::Init).then_some(m + 1);
            let end = init.map_or(t.end, |x| n.parts[x].end);
            if m == k {
                // The site in this declarator's pattern: the bindings it initialized,
                // then the rest of it over its saved state.
                for name in self.pattern_done_bindings(i + 1) {
                    let v = self.decl_value(&name)?;
                    items.push_out(P::S(cat!(name, " = ", v)));
                }
                let (pt, ps) = self.pattern(i + 1)?;
                items.push_out(P::L(alloc::vec![pt, s(" = "), ps]));
            } else if init == Some(k) {
                let e = self.expr(i + 1)?;
                items.push_out(P::L(alloc::vec![P::V(t.start, t.end), s(" = "), e]));
            } else if t.start > n.parts[k].start {
                items.push_out(self.rest(t.start, end));
            } else {
                // Declared before the site: its values are in the frame.
                let name = String::from(self.text(t.start, t.end).trim());
                let names: Vec<String> = if is_ident(&name) {
                    alloc::vec![name]
                } else {
                    let mut v: Vec<String> = Vec::new();
                    for j in 0..self.act.site.path.len() {
                        for d in &self.node(j).decls {
                            if t.start <= d.at && d.at < t.end && !v.contains(&d.name) {
                                v.push_out(d.name.clone());
                            }
                        }
                    }
                    v
                };
                for name in names {
                    let v = self.decl_value(&name)?;
                    let item = P::S(cat!(name, " = ", v));
                    if using {
                        before.push_out(item);
                    } else {
                        items.push_out(item);
                    }
                }
            }
            m = init.map_or(m + 1, |x| x + 1);
        }
        // The scope's record, when this declaration made it (its first resource or a
        // loop head's): added again before the bindings.
        let mut out = if using {
            alloc::vec![self.using_readd(i)?]
        } else {
            Vec::new()
        };
        if !before.is_empty() {
            if i > 0 && self.node(i - 1).kind == NodeKind::For {
                return no("a site after another resource of a loop's using declaration");
            }
            out.push_out(s("const "));
            push_joined(&mut out, before);
            out.push_out(s("; "));
        }
        out.push_out(s(kw));
        push_joined(&mut out, items);
        Ok(out)
    }

    /// The value of binding `name` of the site's declaration, initialized already.
    fn decl_value(&mut self, name: &str) -> R<String> {
        let scope = self.enclosing_scope_of(name);
        Ok(match self.binding_slot(name, scope) {
            Some(sn) => {
                let v = match self.ctx.cell(&sn) {
                    Some(Some(init)) => init,
                    Some(None) => return no("a shared binding in its TDZ"),
                    None => self.take(&sn)?,
                };
                if !self.used.contains(&sn) {
                    self.used.push_out(sn);
                }
                v
            }
            None => String::from("undefined"),
        })
    }

    /// The scope offset of a binding declared by the statement list around path node
    /// `i`: the innermost enclosing node that declares it.
    fn enclosing_scope_of(&self, name: &str) -> u32 {
        for i in (0..self.act.site.path.len()).rev() {
            if let Some(d) = self.node(i).decls.iter().find(|d| d.name == name) {
                return d.scope;
            }
        }
        0
    }

    fn for_stmt(&mut self, i: usize, n: &Node, label: Option<P>) -> R<P> {
        let k = self.next_part(i).unwrap_or(0);
        if n.parts[k].role != Role::Body {
            return self.for_head(i, n, k, label);
        }
        let f = self.fresh(cat!("$djs_f", i));
        let mut init: Vec<P> = Vec::new();
        if !n.decls.is_empty() {
            let kw = match n.decls[0].kind {
                DeclKind::Const => "const ",
                DeclKind::Let => "let ",
                _ => return no("a for loop's var or using declaration (0203)"),
            };
            init.push_out(s(kw));
            for (x, d) in n.decls.clone().iter().enumerate() {
                let Some(sn) = self.binding_slot(&d.name, d.scope) else {
                    return Err(cat!("plain dumps: no slot for ", d.name));
                };
                let v = match self.ctx.cell(&sn) {
                    Some(Some(v)) => v,
                    Some(None) => return no("a loop binding in its TDZ"),
                    None => self.take(&sn)?,
                };
                if !self.used.contains(&sn) {
                    self.used.push_out(sn);
                }
                if x > 0 {
                    init.push_out(s(", "));
                }
                init.push_out(P::S(cat!(d.name, " = ", v)));
            }
        } else if n.parts.iter().any(|p| p.role == Role::Init)
            && self.text(n.start, n.parts[0].start).contains("var")
        {
            return no("a for loop's var declaration (0203)");
        }
        let (init, test, update) = self.for_parts(n, init);
        let body = self.rest_part(n, k);
        // The rest of the iteration, then the loop going on with its update, where nothing
        // in the body jumps to the loop and no closure may capture a `let` of the head
        // (one binding then stands for each iteration's) (D2917).
        let bt = self.text(n.parts[k].start, n.parts[k].end);
        let lt = self.text(n.start, n.end);
        let closures = lt.contains("=>") || has_ident(lt, "function") || has_ident(lt, "class");
        let lets = n.decls.iter().any(|d| d.kind == DeclKind::Let);
        if !has_ident(bt, "break") && !has_ident(bt, "continue") && !(lets && closures) {
            let own: Vec<_> = n
                .decls
                .iter()
                .map(|d| (d.name.clone(), d.kind, d.scope, false))
                .collect();
            let own = self.declare(&own)?;
            let first = self.stmt(i + 1, None)?;
            let open = if n.decls.is_empty() {
                s("{\n")
            } else {
                P::L(alloc::vec![s("{ "), init, s(";\n")])
            };
            return Ok(P::L(alloc::vec![
                open,
                own,
                s("\n"),
                first,
                s("\n"),
                label.unwrap_or_else(|| s("")),
                s("for ("),
                update.clone(),
                s("; "),
                test,
                s("; "),
                update,
                s(") "),
                body,
                s(" }"),
            ]));
        }
        let m = self.mark("for", n.start);
        // The per-iteration scope: its bindings are declared by the head; the code the
        // dump puts there runs in the first iteration.
        let own: Vec<_> = n
            .decls
            .iter()
            .map(|d| (d.name.clone(), d.kind, d.scope, false))
            .collect();
        let own = self.declare(&own)?;
        let first = self.stmt(i + 1, None)?;
        Ok(P::L(alloc::vec![
            P::S(cat!("{ let ", f, " = true; ")),
            // (At the loop's keyword: a jump's completion names the loop by it.)
            label.unwrap_or_else(|| s("")),
            m,
            s("for ("),
            init,
            P::S(cat!("; ", f, " || (")),
            test,
            s("); "),
            update,
            P::S(cat!(") { if (", f, ") { ", f, " = false; ")),
            own,
            s("\n"),
            first,
            s(" } else "),
            body,
            s(" } }"),
        ]))
    }

    /// A for-of or for-in around the site: the rest of this iteration runs first, then
    /// the loop goes on over the saved iterator record as a real loop (`break`,
    /// `return` and exceptions close the iterator as the original's do).
    fn for_of(&mut self, i: usize, n: &Node, label: Option<P>) -> R<P> {
        // At the loop itself: its head's `using` resources disposed after an iteration.
        let disposing = self.is_site(i);
        let using_head = n
            .decls
            .iter()
            .any(|d| matches!(d.kind, DeclKind::Using | DeclKind::AwaitUsing));
        let k = match self.next_part(i) {
            Some(k) => k,
            None => n
                .parts
                .iter()
                .position(|p| p.role == Role::Body)
                .ok_or("plain dumps: a loop without its body")?,
        };
        if n.parts[k].role == Role::Iterable {
            return self.loop_iterable(i, n, k, label);
        }
        if n.parts[k].role != Role::Body {
            return no("a site in a loop's head (0203)");
        }
        let c = if n.kind == NodeKind::ForIn {
            "for-in"
        } else {
            "for-of"
        };
        if !disposing
            && self.rereads(&cat!(c, ".closeless@", n.start))
            && !n
                .decls
                .iter()
                .any(|d| matches!(d.kind, DeclKind::Using | DeclKind::AwaitUsing))
        {
            let bt = self.text(n.parts[k].start, n.parts[k].end);
            if !has_ident(bt, "break") && !has_ident(bt, "continue") {
                return self.for_of_rest(i, n, k, c, label);
            }
        }
        let it = self.take(&cat!(c, ".iterator@", n.start))?;
        let next = if n.kind == NodeKind::ForIn {
            String::from("null")
        } else {
            self.next_method(&cat!(c, ".next@", n.start))?
        };
        // An iterator the dump's callback makes inside the loop's first iteration: the
        // wrapper takes it once that part ran (D2910).
        let late = if self.made_later(&it) || self.made_later(&next) {
            let w = self.fresh(cat!("$djs_w", i));
            let cur = self.block_index.saturating_sub(1);
            let p = [&it, &next]
                .iter()
                .filter_map(|v| self.ctx.made_in(v))
                .max()
                .filter(|&p| p > cur)
                .unwrap_or(cur + 1);
            self.deferred
                .push_out((p, Vec::new(), P::S(cat!(w, ".set(", it, ", ", next, ");"))));
            Some(w)
        } else {
            None
        };
        let head = n
            .parts
            .iter()
            .find(|p| p.role == Role::Head)
            .copied()
            .ok_or("plain dumps: a loop without its head")?;
        let f = self.fresh(cat!("$djs_f", i));
        let v = self.fresh(cat!("$djs_v", i));
        let head_text = String::from(self.text(head.start, head.end));
        let bind = if n.decls.is_empty() {
            // (A `var` head's names are K's: assigned.)
            let target = self.var_head_target(head, &head_text);
            P::L(alloc::vec![s("("), target, P::S(cat!(" = ", v, ");"))])
        } else {
            P::L(alloc::vec![
                P::V(head.start, head.end),
                P::S(cat!(" = ", v, ";"))
            ])
        };
        let decls: Vec<_> = n
            .decls
            .iter()
            .map(|d| (d.name.clone(), d.kind, d.scope, true))
            .collect();
        let body = self.rest_part(n, k);
        let m = self.mark(c, n.start);
        let (d, first) = if disposing {
            (s(""), self.using_disposal(i)?)
        } else {
            let d = self.declare(&decls)?;
            let u = self.using_readd(i)?;
            (P::L(alloc::vec![d, s("\n"), u]), self.stmt(i + 1, None)?)
        };
        let (pre, wrapper) = match &late {
            Some(w) => (cat!("let ", w, "; "), cat!(w, " = $djs_L()")),
            None => (String::new(), cat!("$djs_L(", it, ", ", next, ")")),
        };
        Ok(P::L(alloc::vec![
            P::S(cat!(pre, "{ let ", f, " = true; ")),
            label.unwrap_or_else(|| s("")),
            // (At the loop's own start: its temporaries are named by it.)
            m,
            P::S(cat!(
                "for (const ",
                v,
                " of ",
                wrapper,
                ") { if (",
                f,
                ") { ",
                f,
                " = false; {\n"
            )),
            // (The first iteration's block: its bindings live there only, D2911.)
            P::Scope(
                decls.iter().map(|d| d.0.clone()).collect(),
                false,
                alloc::boxed::Box::new(P::L(alloc::vec![d, s("\n"), first]))
            ),
            s("\n}"),
            // (The iteration's `using` scope ends here: its disposal is a site.)
            if using_head {
                self.mark("site", n.parts[k].end)
            } else {
                s("")
            },
            s(" } else { "),
            bind,
            s(" "),
            body,
            s(" } } }"),
        ]))
    }

    /// [`G::for_of`] where the rest of the current iteration can run before the loop
    /// (D2908): nothing in the body jumps to the loop (no `break` or `continue` in its
    /// text) and nothing closes the iterator (it has no `return`), so the rest, then
    /// `for (Head of $djs_I(it, next)) Body` over the saved record, is the loop going on.
    /// The record is bound to constants of K, which origin tracking reads back while K is
    /// in the rest (`["for-of", fo, ko, 0, it, next]`, ko the loop's start).
    fn for_of_rest(
        &mut self,
        i: usize,
        n: &Node,
        k: usize,
        c: &'static str,
        label: Option<P>,
    ) -> R<P> {
        let head = n
            .parts
            .iter()
            .find(|p| p.role == Role::Head)
            .copied()
            .ok_or("plain dumps: a loop without its head")?;
        let it = self.take(&cat!(c, ".iterator@", n.start))?;
        let nslot = cat!(c, ".next@", n.start);
        let next = if n.kind == NodeKind::ForIn || !self.has(&nslot) {
            None
        } else if self.rereads(&nslot) {
            self.consume(&nslot);
            None
        } else {
            Some(self.take(&nslot)?)
        };
        self.consume(&cat!(c, ".closeless@", n.start));
        let ib = self.fresh(cat!("$djs_i", i));
        let nb = self.fresh(cat!("$djs_n", i));
        let rec = self.mark(c, n.start);
        if let P::Mark(x) = rec {
            self.scaf[x].3 = match &next {
                Some(_) => cat!(",0,", quote(&ib), ",", quote(&nb)),
                None => cat!(",0,", quote(&ib)),
            };
        }
        let mut pairs = alloc::vec![(ib.clone(), it.clone())];
        if let Some(nx) = &next {
            pairs.push_out((nb.clone(), nx.clone()));
        }
        let consts = self.scaffold_consts(&pairs);
        let consts = if consts.is_empty() {
            consts
        } else {
            cat!(consts, "\n")
        };
        let decls: Vec<_> = n
            .decls
            .iter()
            .map(|d| (d.name.clone(), d.kind, d.scope, true))
            .collect();
        let d = self.declare(&decls)?;
        let first = self.stmt(i + 1, None)?;
        // (An iterator that is its own iterable, a builtin's, as it is, D2917.)
        let direct = n.kind == NodeKind::ForOf
            && next.is_none()
            && self.rereads(&cat!("for-of.direct@", n.start));
        let args = match &next {
            Some(_) => cat!(ib, ", ", nb),
            None => ib.clone(),
        };
        let of = if direct {
            cat!(" of ", ib, ") ")
        } else {
            cat!(" of $djs_I(", args, ")) ")
        };
        // (Braces only where the iteration declares something live, D2911.)
        let names = decls.iter().map(|d| d.0.clone()).collect();
        let body = alloc::boxed::Box::new(P::L(alloc::vec![d, s("\n"), first]));
        Ok(P::L(alloc::vec![
            P::S(consts),
            P::Scope(names, true, body),
            s("\n"),
            label.unwrap_or_else(|| s("")),
            rec,
            P::V(n.start, head.start),
            P::V(head.start, head.end),
            P::S(of),
            self.rest_part(n, k),
        ]))
    }

    /// A `for await` around the site or at it (its `next` or a close awaited). The loop
    /// goes on over the saved async iterator record (an async-from-sync one is the
    /// dump's `$dump.fromSync`), bound to constants of K that origin tracking reads back
    /// (the scaffold record `["for-await", fo, ko, 0, it, next]`).
    fn for_await(&mut self, i: usize, n: &Node, label: Option<P>) -> R<P> {
        let fo = n.start;
        let lbl = label.unwrap_or_else(|| s(""));
        if self.is_site(i) && self.act.site.kind == crate::compiler::SiteKind::ForAwaitClose {
            return self.for_await_close(n, &lbl);
        }
        let head = n
            .parts
            .iter()
            .find(|p| p.role == Role::Head)
            .copied()
            .ok_or("plain dumps: a loop without its head")?;
        let body = n
            .parts
            .iter()
            .position(|p| p.role == Role::Body)
            .ok_or("plain dumps: a loop without its body")?;
        if let Some(k) = self.next_part(i)
            && n.parts[k].role == Role::Iterable
        {
            return self.loop_iterable(i, n, k, Some(lbl));
        }
        // (A `var` head declares K's names again: harmless.)
        let it = self.take(&cat!("for-await.iterator@", fo))?;
        let next = self.next_method(&cat!("for-await.next@", fo))?;
        let ib = self.fresh(cat!("$djs_i", i));
        let nb = self.fresh(cat!("$djs_n", i));
        let rec = self.mark("for-await", fo);
        if let P::Mark(x) = rec {
            self.scaf[x].3 = cat!(",0,", quote(&ib), ",", quote(&nb));
        }
        let consts = P::S(cat!(
            "{ ",
            self.scaffold_consts(&[(ib.clone(), it.clone()), (nb.clone(), next.clone())]),
            " "
        ));
        // `for await (Head of <the record>) Body`, `first` the awaited promise of the
        // first step (a site at the loop's `next`).
        let lp = |first: &str| {
            P::L(alloc::vec![
                P::V(n.start, head.start),
                P::V(head.start, head.end),
                P::S(cat!(" of $djs_N(", ib, ", ", nb, first, ")) ")),
                P::V(n.parts[body].start, n.parts[body].end),
            ])
        };
        if self.is_site(i) {
            // At the loop's `next`: its first step is the awaited promise.
            let site = self.mark("site", fo);
            let p = cat!(", ", self.awaiting());
            let l = lp(&p);
            return Ok(self.with_go(P::L(alloc::vec![consts, rec, lbl, site, l, s(" }")])));
        }
        if self.next_part(i) != Some(body) {
            return no("a site in a for await's head (0203)");
        }
        // The rest of this iteration in a loop that runs once, so `continue` ends it and
        // `break` leaves it (state 2, else 1); the iterator is closed as the loop
        // would: after a `break` or a jump out (AsyncIteratorClose: `return()` awaited,
        // its result checked), on an exception ignoring what `return()` does.
        let st = self.fresh(cat!("$djs_s", i));
        let once = self.fresh(cat!("$djs_o", i));
        let exc = self.fresh(cat!("$djs_e", i));
        let m = self.fresh(cat!("$djs_m", i));
        let close = cat!(
            "{ const ",
            m,
            " = ",
            ib,
            ".return; if (",
            m,
            " != null) $djs_O(await $djs_R(",
            m,
            ", ",
            ib,
            ", [])); }"
        );
        let decls: Vec<_> = n
            .decls
            .iter()
            .map(|d| (d.name.clone(), d.kind, d.scope, true))
            .collect();
        let d = self.declare(&decls)?;
        let first = self.stmt(i + 1, None)?;
        let rest = lp("");
        // The loop that runs the rest of this iteration stands for the original's loop:
        // a `continue` or `break` pending in K's code (a `using` scope's disposal) names
        // it (origin tracking, D2733).
        let once_loop = self.mark("loop", fo);
        Ok(P::L(alloc::vec![
            consts,
            P::S(cat!("let ", st, " = 2;\ntry { ")),
            once_loop,
            lbl.clone(),
            P::S(cat!(
                "for (let ",
                once,
                " = 0; ",
                once,
                " < 1; ",
                once,
                "++, ",
                st,
                " = 1) {\n"
            )),
            d,
            s("\n"),
            first,
            P::S(cat!(
                "\n} ",
                st,
                " += 10; } catch (",
                exc,
                ") { ",
                st,
                " = 20; await $djs_X(",
                ib,
                "); throw ",
                exc,
                "; } finally { if (",
                st,
                " < 10) ",
                close,
                " }\nif (",
                st,
                " === 12) ",
                close,
                " else { "
            )),
            rec,
            lbl,
            rest,
            s(" } }"),
        ]))
    }

    /// A `for await` stopped while its iterator's `return()` is awaited: on an exception
    /// (rethrown after), a jump out of the loop (taken after) or a `break`.
    fn for_await_close(&mut self, n: &Node, lbl: &P) -> R<P> {
        let fo = n.start;
        let site = self.mark("site", fo);
        if let P::Mark(x) = site {
            self.scaf[x].3 = cat!(",", quote(&self.act.site.name));
        }
        let p = self.awaiting();
        let awaited = P::L(alloc::vec![s("("), site, P::S(cat!("await ", p)), s(")")]);
        let exc = cat!("for-await.exception@", fo);
        if self.has(&exc) {
            let e = self.tmp("for-await.exception", fo)?;
            return Ok(self.with_go(P::L(alloc::vec![
                lbl.clone(),
                s("{ "),
                awaited,
                s("; throw "),
                e,
                s("; }")
            ])));
        }
        let (kname, vname) = (cat!("completion.kind@", fo), cat!("completion.value@", fo));
        // Which close this is: the loop's close sites are the exception's, a jump out's
        // and a `break`'s (the last two where used).
        let closes = &self.act.peers;
        let nth = closes.iter().position(|c| *c == self.act.site.name);
        let jump = if self.has(&kname) || self.has(&vname) {
            true
        } else if closes.len() == 3 {
            nth == Some(1)
        } else {
            // One of them: a `break`, unless a labelled jump could leave the loop (its
            // completion is not in the frame then: the only exit, which the code knows).
            let body = n.parts.iter().find(|p| p.role == Role::Body).copied();
            let text = body.map_or("", |b| self.text(b.start, b.end));
            let path = &self.act.site.path;
            for j in 0..path.len().saturating_sub(2) {
                let o = self.node(j);
                if o.kind == NodeKind::Label
                    && let Some(lp) = o.parts.iter().find(|p| p.role == Role::Label)
                {
                    let name = self.text(lp.start, lp.end);
                    if has_ident(text, name) {
                        return no("a for await left by a labelled jump, closing (0203)");
                    }
                }
            }
            false
        };
        let exit = if !jump {
            s("")
        } else if self.has(&kname) {
            let k = self.take(&kname)?;
            let k = self.steer_kind(&k)?;
            // (The original's at this site, should K stop here again.)
            self.cmap.push_out((kname.clone(), k.clone(), true));
            match k.parse::<i32>() {
                Ok(2) => P::L(alloc::vec![
                    s(" return "),
                    self.tmp("completion.value", fo)?,
                    s(";")
                ]),
                Ok(c) if c >= 3 => {
                    if self.has(&vname) {
                        self.consume(&vname);
                    }
                    let l = self.label_at(((c - 3) / 2) as u32)?;
                    P::S(cat!(
                        if c % 2 == 1 { " break " } else { " continue " },
                        l,
                        ";"
                    ))
                }
                _ => return Err(cat!("plain dumps: a for await closed with completion ", k)),
            }
        } else if self.has(&vname) {
            P::L(alloc::vec![
                s(" return "),
                self.tmp("completion.value", fo)?,
                s(";")
            ])
        } else {
            return no("a for await left by a labelled jump, closing (0203)");
        };
        Ok(self.with_go(P::L(alloc::vec![
            lbl.clone(),
            s("{ $djs_O("),
            awaited,
            s(");"),
            exit,
            s(" }")
        ])))
    }

    /// The label of the statement at offset `o` on the site's path (a labelled
    /// statement's first label, or a loop's).
    fn label_at(&self, o: u32) -> R<String> {
        let path = &self.act.site.path;
        for j in 0..path.len() {
            let nd = self.node(j);
            if nd.start != o {
                continue;
            }
            for jj in (0..=j).rev() {
                let l = self.node(jj);
                if l.kind == NodeKind::Label
                    && let Some(lp) = l.parts.iter().find(|p| p.role == Role::Label)
                {
                    return Ok(String::from(self.text(lp.start, lp.end)));
                }
                if jj < j && l.kind != NodeKind::Label {
                    break;
                }
            }
        }
        no("a jump to a statement off the site's path")
    }

    fn try_stmt(&mut self, i: usize, n: &Node) -> R<P> {
        let k = self.next_part(i).unwrap_or(0);
        let part = |r: Role| n.parts.iter().position(|p| p.role == r);
        match n.parts[k].role {
            Role::Block => {
                let m = self.mark("try", n.start);
                let first = self.stmt(i + 1, None)?;
                let mut out = alloc::vec![m, s("try "), first];
                if let Some(cb) = part(Role::CatchBody) {
                    out.push_out(s(" catch"));
                    if let Some(cp) = part(Role::CatchParam) {
                        out.push_out(s(" ("));
                        out.push_out(self.rest_part(n, cp));
                        out.push_out(s(")"));
                    }
                    out.push_out(s(" "));
                    out.push_out(self.rest_part(n, cb));
                }
                if let Some(fi) = part(Role::Finally) {
                    out.push_out(s(" finally "));
                    out.push_out(self.rest_part(n, fi));
                }
                Ok(P::L(out))
            }
            Role::CatchBody => {
                let decls: Vec<_> = n
                    .decls
                    .iter()
                    .map(|d| (d.name.clone(), d.kind, d.scope, true))
                    .collect();
                let d = self.declare(&decls)?;
                let first = self.stmt(i + 1, None)?;
                let h = P::L(alloc::vec![s("{\n"), d, s("\n"), first, s("\n}")]);
                Ok(match part(Role::Finally) {
                    Some(fi) => {
                        let m = self.mark("try", n.start);
                        let fin = self.rest_part(n, fi);
                        P::L(alloc::vec![m, s("try "), h, s(" finally "), fin])
                    }
                    None => {
                        let m = self.mark("catch", n.start);
                        P::L(alloc::vec![m, h])
                    }
                })
            }
            Role::Finally => {
                // The rest of the finally block, then the completion that entered it.
                // (No completion where the block's end is dead, `if (true) continue;`:
                // the compiler keeps none, and K never reaches the end either.)
                let (kname, vname) = (
                    cat!("completion.kind@", n.start),
                    cat!("completion.value@", n.start),
                );
                let (kind, v) = if self.has(&kname) {
                    let kind = self.take(&kname)?;
                    (self.steer_kind(&kind)?, self.take(&vname)?)
                } else {
                    if self.has(&vname) {
                        self.consume(&vname);
                    }
                    (String::from("0"), String::from("undefined"))
                };
                let cv = self.fresh(cat!("$djs_c", i));
                let done = match kind.as_str() {
                    "0" => String::new(),
                    "1" => cat!("throw ", cv, ";"),
                    "2" => cat!("return ", cv, ";"),
                    c => match c.parse::<i32>() {
                        Ok(c) if c >= 3 => self.jump_to(i, ((c - 3) / 2) as u32, c % 2 == 0)?,
                        _ => return Err(cat!("plain dumps: a finally's completion ", c)),
                    },
                };
                let m = self.mark("finally", n.start);
                if let P::Mark(x) = m {
                    self.scaf[x].3 = cat!(",", kind, ",", quote(&cv));
                }
                let first = self.stmt(i + 1, None)?;
                Ok(P::L(alloc::vec![
                    m,
                    P::S(cat!("{ const ", cv, " = ", v, "; ")),
                    first,
                    P::S(cat!(" ", done, " }")),
                ]))
            }
            Role::CatchParam => self.catch_param(i, n),
            _ => no("a site in a try statement the exporter does not know"),
        }
    }

    fn switch_stmt(&mut self, i: usize, n: &Node) -> R<P> {
        let k = self.next_part(i).unwrap_or(0);
        match n.parts[k].role {
            Role::Discriminant => {
                let e = self.expr(i + 1)?;
                let d = n.parts[k];
                let open = self.text(d.end, n.end).find('{').map(|x| d.end + x as u32);
                let open = open.ok_or("plain dumps: a switch without its block")?;
                return Ok(self.with_go(P::L(alloc::vec![
                    P::V(n.start, d.start),
                    e,
                    s(") "),
                    P::V(open, n.end)
                ])));
            }
            Role::Case => return self.switch_case(i, n, k),
            Role::Stmt => {}
            _ => return no("a site in a switch's clause"),
        }
        // The clause holding the site: its statements from the site's on, then the
        // statements of the later clauses (fall-through); `break` leaves `switch (0)`.
        let decls = Self::block_decls(n, k);
        let m = self.mark("switch", n.start);
        let l = self.list(n, k, i + 1, &decls)?;
        Ok(P::L(alloc::vec![m, s("switch (0) { case 0: "), l, s(" }")]))
    }

    /// A site in case test `k`: the switch again with the saved discriminant, the tests
    /// that failed before it never matching (`case {}`: a fresh object), the site's
    /// test rewritten and the rest as they are.
    fn switch_case(&mut self, i: usize, n: &Node, k: usize) -> R<P> {
        if n.decls.iter().any(|d| d.kind == DeclKind::Function) {
            return no("a function declared in a switch whose case test awaits (0203)");
        }
        // A case's `case` keyword: before its test (the value it compares is named by it).
        let case_kw = |g: &Self, m: usize| -> R<u32> {
            let from = if m == 0 { n.start } else { n.parts[m - 1].end };
            let to = n.parts[m].start;
            g.text(from, to)
                .rfind("case")
                .map(|x| from + x as u32)
                .ok_or_else(|| String::from("plain dumps: a case without its keyword"))
        };
        let kw = case_kw(self, k)?;
        // The value the test compares: the discriminant (its temporary is dead once the
        // last test runs).
        let dname = cat!("switch.discriminant@", n.start);
        let disc = if self.has(&dname) {
            // (The test's left operand is the discriminant's value: K's one stands for
            // both, D2913.)
            let disc = self.tmp("switch.discriminant", n.start)?;
            let lname = cat!("binary.left@", kw);
            self.consume(&lname);
            let i = self.cmap.len() - 1;
            let (v, lit) = (self.cmap[i].1.clone(), self.cmap[i].2);
            self.cmap.push_out((lname, v, lit));
            if lit {
                self.aliases.push_out((self.cmap.len() - 1, i));
            }
            disc
        } else {
            self.tmp("binary.left", kw)?
        };
        let d = n
            .parts
            .iter()
            .find(|p| p.role == Role::Discriminant)
            .copied()
            .ok_or("plain dumps: a switch without its discriminant")?;
        let mut out = alloc::vec![P::V(n.start, d.start), disc, s(") {\n")];
        for m in 0..n.parts.len() {
            let p = n.parts[m];
            match p.role {
                Role::Case if m < k => out.push_out(s("case {}:\n")),
                Role::Case if m == k => {
                    out.push_out(P::V(kw, p.start));
                    out.push_out(self.expr(i + 1)?);
                    out.push_out(s(":\n"));
                }
                Role::Case => {
                    let c = case_kw(self, m)?;
                    out.push_out(P::V(c, p.end));
                    out.push_out(s(":\n"));
                }
                Role::Default => {
                    out.push_out(P::V(p.start, p.end));
                    out.push_out(s(":\n"));
                }
                Role::Stmt | Role::FuncDecl => {
                    out.push_out(P::V(p.start, p.end));
                    out.push_out(s("\n"));
                }
                _ => {}
            }
        }
        out.push_out(s("}"));
        let m = self.mark("switch", n.start);
        out.insert(0, m);
        Ok(self.with_go(P::L(out)))
    }

    /// `break`/`continue` to the statement at offset `o`, from inside path node `i`:
    /// without a label where the innermost loop (or switch, for `break`) is that
    /// statement, as K's scaffolds nest like the original's statements.
    fn jump_to(&self, i: usize, o: u32, cont: bool) -> R<String> {
        let kw = if cont { "continue" } else { "break" };
        for j in (0..i).rev() {
            let nd = self.node(j);
            let target = match nd.kind {
                NodeKind::While
                | NodeKind::DoWhile
                | NodeKind::For
                | NodeKind::ForOf
                | NodeKind::ForIn
                | NodeKind::ForAwait => true,
                NodeKind::Switch => !cont,
                _ => false,
            };
            if target {
                if nd.start == o {
                    return Ok(cat!(kw, ";"));
                }
                break;
            }
        }
        Ok(cat!(kw, " ", self.label_at(o)?, ";"))
    }

    // ---------------------------------------------------------------- expressions

    fn expr(&mut self, i: usize) -> R<P> {
        let n = self.node(i).clone();
        if self.is_site(i) && n.kind == NodeKind::YieldStar {
            return self.yield_star_site(&n);
        }
        if self.is_site(i) {
            let m = self.mark("site", n.start);
            // Parentheses only where the site's place does not take it bare.
            let (open, close) = if self.takes_bare(i) {
                ("", "")
            } else {
                ("(", ")")
            };
            return Ok(match n.kind {
                NodeKind::Await => P::L(alloc::vec![
                    s(open),
                    m,
                    s("await "),
                    P::Val(self.awaiting(), self.ctx.site_reads()),
                    s(close)
                ]),
                // A yield's operand was yielded before the dump.
                NodeKind::Yield => P::L(alloc::vec![s(open), m, s("yield"), s(close)]),
                _ => return Err(cat!("plain dumps: a site at ", kind_name(n.kind))),
            });
        }
        let k = self
            .next_part(i)
            .ok_or("plain dumps: a path that ends early")?;
        // (Can K stop again in this expression after the site? Its text after the site
        // has an `await` or a `yield`.)
        let last = self.act.site.path.len() - 1;
        let after = self.text(self.node(last).end.min(n.end), n.end);
        // (Or an `await`/`yield` between it and the site, which waits once the site's
        // operand is done.)
        let between = (i + 1..last).any(|j| {
            matches!(
                self.node(j).kind,
                NodeKind::Await | NodeKind::Yield | NodeKind::YieldStar
            )
        });
        let saved = self.marks;
        self.marks = between || has_ident(after, "await") || has_ident(after, "yield");
        let r = self.expr_at(i, &n, k);
        self.marks = saved;
        r
    }

    /// [`G::expr`] of node `n` (path node `i`, the site in its part `k`).
    fn expr_at(&mut self, i: usize, n: &Node, k: usize) -> R<P> {
        let n = n.clone();
        let role = n.parts[k].role;
        let pt = |r: Role| n.parts.iter().position(|p| p.role == r);
        match n.kind {
            // The keyword copied: K stopping there again maps to the original's site.
            NodeKind::Await | NodeKind::Yield => {
                let e = self.expr(i + 1)?;
                Ok(P::L(alloc::vec![
                    s("("),
                    P::V(n.start, n.start + 5),
                    s(" "),
                    e,
                    s(")")
                ]))
            }
            NodeKind::Call { .. } | NodeKind::New => self.call(i, &n, k),
            NodeKind::Binary | NodeKind::Logical => {
                let op = pt(Role::Op).ok_or("plain dumps: an operator without its token")?;
                let opt = String::from(self.text(n.parts[op].start, n.parts[op].end));
                if role == Role::Left {
                    // K's own operator: its left operand is the original's (D2885).
                    let m = self.ktemp(cat!("binary.left@", n.start), "binary.left");
                    let e = self.expr(i + 1)?;
                    let r = pt(Role::Right)
                        .map(|r| self.rest_part(&n, r))
                        .unwrap_or(s(""));
                    return Ok(P::L(alloc::vec![
                        s("("),
                        m,
                        e,
                        P::S(cat!(" ", opt, " ")),
                        r,
                        s(")")
                    ]));
                }
                if n.kind == NodeKind::Logical {
                    // The left side decided to evaluate the right.
                    return self.expr(i + 1);
                }
                // The left operand read again (D2914): the original's text.
                if self.rereads(&cat!("binary.left@", n.start)) {
                    self.consume(&cat!("binary.left@", n.start));
                    let r = n.parts[k];
                    let e = self.expr(i + 1)?;
                    return Ok(P::L(alloc::vec![
                        s("("),
                        self.rest(n.start, r.start),
                        e,
                        self.rest(r.end, n.end),
                        s(")")
                    ]));
                }
                let l = self.tmp("binary.left", n.start)?;
                let e = self.expr(i + 1)?;
                Ok(P::L(alloc::vec![
                    s("("),
                    self.kmark(),
                    l,
                    P::S(cat!(" ", opt, " ")),
                    e,
                    s(")")
                ]))
            }
            NodeKind::Conditional => {
                if role == Role::Test {
                    let e = self.expr(i + 1)?;
                    let t = pt(Role::Then)
                        .map(|x| self.rest_part(&n, x))
                        .unwrap_or(s(""));
                    let f = pt(Role::Else)
                        .map(|x| self.rest_part(&n, x))
                        .unwrap_or(s(""));
                    return Ok(P::L(alloc::vec![
                        s("("),
                        e,
                        s(" ? "),
                        t,
                        s(" : "),
                        f,
                        s(")")
                    ]));
                }
                self.expr(i + 1)
            }
            NodeKind::Unary => {
                let op = pt(Role::Op).ok_or("plain dumps: an operator without its token")?;
                let opt = String::from(self.text(n.parts[op].start, n.parts[op].end));
                let e = self.expr(i + 1)?;
                Ok(P::L(alloc::vec![P::S(cat!("(", opt, " ")), e, s(")")]))
            }
            NodeKind::Sequence => {
                let mut out = alloc::vec![s("("), self.expr(i + 1)?];
                for m in k + 1..n.parts.len() {
                    out.push_out(s(", "));
                    out.push_out(self.rest_part(&n, m));
                }
                out.push_out(s(")"));
                Ok(P::L(out))
            }
            NodeKind::Template => {
                // Substitutions before the site are strings already; the literal parts
                // before it are kept by djs too (`template.part@` of the delimiter).
                let mut out = Vec::new();
                let mut seen = false;
                for m in 0..n.parts.len() {
                    let p = n.parts[m];
                    match p.role {
                        Role::Quasi => {
                            let name = cat!("template.part@", p.start);
                            if !seen && self.has(&name) && !self.used.contains(&name) {
                                self.used.push_out(name);
                            }
                            out.push_out(P::V(p.start, p.end));
                        }
                        Role::Subst if m == k => {
                            // (The substitution's value is the original's part.)
                            out.push_out(
                                self.ktemp(cat!("template.part@", p.start), "template.part"),
                            );
                            out.push_out(self.expr(i + 1)?);
                            seen = true;
                        }
                        Role::Subst if seen => out.push_out(self.rest_part(&n, m)),
                        // (Read again, D2914: the original's text.)
                        Role::Subst if self.rereads(&cat!("template.part@", p.start)) => {
                            self.consume(&cat!("template.part@", p.start));
                            out.push_out(self.rest_part(&n, m));
                        }
                        // (K's part there is rebuilt from the original's frame.)
                        Role::Subst => {
                            out.push_out(self.kmark());
                            out.push_out(self.tmp("template.part", p.start)?);
                        }
                        _ => {}
                    }
                }
                Ok(P::L(out))
            }
            NodeKind::Array => self.array(i, &n, k),
            NodeKind::Member { computed, .. } => {
                if let Some(o) = pt(Role::Object)
                    && self.text(n.parts[o].start, n.parts[o].end) == "super"
                {
                    return self.super_member(i, &n, k);
                }
                if role == Role::Object {
                    let e = self.expr(i + 1)?;
                    let tail = self.rest(n.parts[k].end, n.end);
                    return Ok(P::L(alloc::vec![e, tail]));
                }
                if !computed {
                    return no("a site in a property name");
                }
                let o = self.tmp("member.object", n.start)?;
                let e = self.expr(i + 1)?;
                Ok(P::L(alloc::vec![o, s("["), e, s("]")]))
            }
            NodeKind::Assign => self.assign(i, &n, k),
            NodeKind::Update => self.update(i, &n, k),
            NodeKind::TaggedTemplate => self.tagged(i, &n, k),
            NodeKind::Object => self.object(i, &n, k),
            _ => Err(cat!(
                "plain dumps: an expression around the site: ",
                kind_name(n.kind)
            )),
        }
    }

    /// `super[<site>]`: K is a method whose home object's prototype is the base the
    /// original read (`Act::super_proto`), called with the original's `this`, so the
    /// `super` it evaluates again is the one the original evaluated.
    fn super_member(&mut self, i: usize, n: &Node, k: usize) -> R<P> {
        if n.parts[k].role != Role::Property {
            return no("a site in super's base");
        }
        // The exporter checked they are what K's `super` and `this` are.
        self.consume(&cat!("super.this@", n.start));
        self.consume(&cat!("super.base@", n.start));
        let e = self.expr(i + 1)?;
        Ok(P::L(alloc::vec![
            P::V(n.start, n.parts[k].start + 1),
            e,
            s("]")
        ]))
    }

    fn call(&mut self, i: usize, n: &Node, k: usize) -> R<P> {
        let new = n.kind == NodeKind::New;
        let callee = n.parts.iter().position(|p| p.role == Role::Callee);
        let open = n.parts.iter().position(|p| p.role == Role::Open);
        if n.parts[k].role == Role::Callee {
            if new {
                return no("a site in a new's callee");
            }
            // K's call: its callee and `this` are the original's.
            let mc = self.ktemp(cat!("call.callee@", n.start), "call.callee");
            let mt = self.ktemp(cat!("call.this@", n.start), "call.this");
            let e = self.expr(i + 1)?;
            let tail = self.rest(n.parts[k].end, n.end);
            return Ok(P::L(alloc::vec![mc, mt, e, tail]));
        }
        if let Some(c) = callee {
            let t = self.text(n.parts[c].start, n.parts[c].end);
            if t == "super" || t.starts_with("super.") || t.starts_with("super[") {
                return no("a super call around the site (0203)");
            }
        }
        let args: Vec<usize> = (0..n.parts.len())
            .filter(|&m| matches!(n.parts[m].role, Role::Arg | Role::Spread))
            .collect();
        let spread_before = args
            .iter()
            .any(|&m| m < k && n.parts[m].role == Role::Spread);
        let construct = if new { "new.arg" } else { "call.arg" };
        let (fc, ft) = (cat!("call.callee@", n.start), cat!("call.this@", n.start));
        if !new && !spread_before && self.rereads(&fc) && self.rereads(&ft) {
            // The call as written: its callee and `this` read again (D2836). Origin
            // tracking maps K's call to the original's through the copied text.
            self.consume(&fc);
            self.consume(&ft);
            let mut out = alloc::vec![self.rest(n.start, n.parts[args[0]].start)];
            for (j, &m) in args.iter().enumerate() {
                let p = n.parts[m];
                if j > 0 {
                    out.push_out(self.rest(n.parts[args[j - 1]].end, p.start));
                }
                let e = if m < k {
                    self.operand(construct, n, m)?
                } else if m == k {
                    self.arg(i, n, m)?
                } else {
                    self.rest_part(n, m)
                };
                let copied = m > k || (m < k && self.rereads(&cat!(construct, "@", p.start)));
                out.push_out(if copied {
                    e
                } else {
                    self.as_kslot(cat!(construct, "@", p.start), construct, e)
                });
            }
            out.push_out(self.rest(n.parts[*args.last().unwrap_or(&k)].end, n.end));
            return Ok(P::L(out));
        }
        let (ce, this) = if new {
            (self.tmp("new.callee", n.start)?, None)
        } else {
            (
                self.tmp("call.callee", n.start)?,
                Some(self.tmp("call.this", n.start)?),
            )
        };
        let mut list: Vec<P> = Vec::new();
        let head = if new { "$djs_C(" } else { "$djs_R(" };
        // K's own call: its callee, `this` and arguments are rebuilt from the
        // original's frame if K stops again inside it ("k" records, origin tracking).
        let mut out = alloc::vec![self.kmark(), s(head), self.kmark(), ce, s(", ")];
        if let Some(t) = this {
            out.push_out(self.kmark());
            out.push_out(t);
            out.push_out(s(", "));
        }
        if spread_before {
            // The argument list so far is one array: the rest is appended to it.
            let open = open.ok_or("plain dumps: a call without its parenthesis")?;
            let acc = self.tmp(
                if new { "new.args" } else { "call.args" },
                n.parts[open].start,
            )?;
            list.push_out(acc);
            for &m in &args {
                if m == k {
                    list.push_out(self.arg(i, n, m)?);
                } else if m > k {
                    list.push_out(self.rest_part(n, m));
                }
            }
            out.push_out(s("$djs_Q("));
            push_joined(&mut out, list);
            out.push_out(s("))"));
            return Ok(P::L(out));
        }
        let simple = !args.iter().any(|&m| n.parts[m].role == Role::Spread);
        // (Operands before the site are K's constants here, not read again: origin
        // tracking finds the original's temporaries in K's call only where K's call is
        // the original's.)
        for &m in &args {
            let p = n.parts[m];
            let e = if m < k {
                self.tmp(construct, n.parts[m].start)?
            } else if m == k {
                self.arg(i, n, m)?
            } else {
                self.rest_part(n, m)
            };
            list.push_out(if simple {
                self.as_element(cat!(construct, "@", p.start), e)
            } else {
                e
            });
        }
        out.push_out(self.kmark());
        out.push_out(s("["));
        push_joined(&mut out, list);
        out.push_out(s("])"));
        Ok(P::L(out))
    }

    /// An argument or element holding the site (`...` kept for a spread).
    fn arg(&mut self, i: usize, n: &Node, m: usize) -> R<P> {
        let e = self.expr(i + 1)?;
        Ok(if n.parts[m].role == Role::Spread {
            P::L(alloc::vec![s("..."), e])
        } else {
            e
        })
    }

    /// A mark where K's construct `kc` keeps what the original's temporary `fslot`
    /// keeps (origin tracking, should K stop again inside its rewritten statement).
    fn ktemp(&mut self, fslot: String, kc: &str) -> P {
        let m = self.mark("tmp", 0);
        if let P::Mark(x) = m {
            self.scaf[x].3 = cat!(",", quote(&fslot), ",", quote(kc));
        }
        m
    }

    /// Whether path node `i` (an `await` or `yield`) stands where an assignment
    /// expression does: an argument, an element, a property's value, an initializer, an
    /// expression statement, a `return`'s or `throw`'s operand, an assignment's right
    /// side, a template's substitution.
    fn takes_bare(&self, i: usize) -> bool {
        if i == 0 {
            return false;
        }
        let p = self.node(i - 1);
        let Some(k) = self.next_part(i - 1) else {
            return false;
        };
        matches!(
            (p.kind, p.parts[k].role),
            (NodeKind::Call { .. } | NodeKind::New, Role::Arg)
                | (NodeKind::Array, Role::Element)
                | (NodeKind::Object, Role::Value)
                | (NodeKind::Decl(_), Role::Init)
                | (
                    NodeKind::Expr | NodeKind::Return | NodeKind::Throw,
                    Role::Operand
                )
                | (NodeKind::Assign, Role::Right)
                | (NodeKind::Template, Role::Subst)
        )
    }

    /// Argument `m` of call `n`, evaluated before the site: its text again where it reads
    /// the saved value (D2836), else its temporary.
    fn operand(&mut self, construct: &str, n: &Node, m: usize) -> R<P> {
        let p = n.parts[m];
        let slot = cat!(construct, "@", p.start);
        if self.rereads(&slot) {
            self.consume(&slot);
            return Ok(self.rest_part(n, m));
        }
        self.tmp(construct, p.start)
    }

    /// Marks slot `name` consumed without writing its value: K evaluates it again.
    fn consume(&mut self, name: &str) {
        if !self.used.iter().any(|u| u == name) {
            self.used.push_out(String::from(name));
        }
    }

    /// A construct of K's own whose temporaries origin tracking rebuilds from the
    /// original's frame (K's `$djs_R(` call, its fixed arguments).
    fn kmark(&mut self) -> P {
        self.mark("k", 0)
    }

    /// Element `p` of K's own array literal standing for the original's temporary
    /// `fslot` (an argument or element evaluated at or after the site): recorded for
    /// origin tracking, should K stop again before the literal is complete.
    fn as_element(&mut self, fslot: String, p: P) -> P {
        self.as_kslot(fslot, "array.element", p)
    }

    /// [`G::as_element`] for K's construct `kc` at the mark (an argument of K's call
    /// written as the original's, `call.arg`).
    fn as_kslot(&mut self, fslot: String, kc: &str, p: P) -> P {
        let m = self.mark("tmp", 0);
        if let P::Mark(x) = m {
            self.scaf[x].3 = cat!(",", quote(&fslot), ",", quote(kc));
        }
        P::L(alloc::vec![m, p])
    }

    fn array(&mut self, i: usize, n: &Node, k: usize) -> R<P> {
        let lit = cat!("array.literal@", n.start);
        if self.has(&lit) {
            if n.parts[k + 1..].iter().any(|p| p.role == Role::Hole) {
                return no("a hole after the site in an array with a spread");
            }
            let v = self.tmp("array.literal", n.start)?;
            let mut list = alloc::vec![v, self.arg(i, n, k)?];
            for m in k + 1..n.parts.len() {
                list.push_out(self.rest_part(n, m));
            }
            let mut out = alloc::vec![s("$djs_Q(")];
            push_joined(&mut out, list);
            out.push_out(s(")"));
            return Ok(P::L(out));
        }
        let mut list = Vec::new();
        for m in 0..n.parts.len() {
            let p = n.parts[m];
            if p.role == Role::Hole {
                list.push_out(s(""));
            } else if m == k && p.role != Role::Spread {
                let e = self.arg(i, n, m)?;
                list.push_out(self.as_element(cat!("array.element@", p.start), e));
            } else if m == k {
                list.push_out(self.arg(i, n, m)?);
            } else if m > k {
                list.push_out(self.rest_part(n, m));
            } else if p.role == Role::Spread {
                return no("a spread before the site in an array");
            } else {
                let e = self.tmp("array.element", p.start)?;
                list.push_out(self.as_element(cat!("array.element@", p.start), e));
            }
        }
        let trailing_hole = n.parts.last().is_some_and(|p| p.role == Role::Hole);
        // K's literal has the original's elements: once it appends (a spread, many
        // elements), its array is the original's.
        let m = self.mark("tmp", 0);
        if let P::Mark(x) = m {
            self.scaf[x].3 = cat!(
                ",",
                quote(&cat!("array.literal@", n.start)),
                ",\"array.literal\""
            );
        }
        let mut out = alloc::vec![m, s("[")];
        push_joined(&mut out, list);
        out.push_out(s(if trailing_hole { ",]" } else { "]" }));
        Ok(P::L(out))
    }

    fn assign(&mut self, i: usize, n: &Node, k: usize) -> R<P> {
        if n.parts[k].role == Role::Left
            && matches!(
                self.node(i + 1).kind,
                NodeKind::ObjectPattern | NodeKind::ArrayPattern
            )
        {
            // The rest of the pattern; the assignment's value is its right side's.
            let (pt, ps) = self.pattern(i + 1)?;
            let v = self.tmp("assign.value", n.start)?;
            return Ok(P::L(alloc::vec![
                s("(("),
                pt,
                s(" = "),
                ps,
                s("), "),
                v,
                s(")")
            ]));
        }
        if matches!(n.parts[k].role, Role::Object | Role::Property) {
            return self.assign_target_site(i, n, k);
        }
        if n.parts[k].role != Role::Right {
            return no("a site in an assignment's target (0203)");
        }
        let op = n
            .parts
            .iter()
            .position(|p| p.role == Role::Op)
            .ok_or("plain dumps: an assignment without its operator")?;
        let opt = String::from(self.text(n.parts[op].start, n.parts[op].end));
        let logical = matches!(opt.as_str(), "&&=" | "||=" | "??=");
        let compound = opt != "=" && !logical;
        // The assignment as written where its target and old value read again (D2914).
        let member = n.parts.iter().any(|p| p.role == Role::Object);
        let computed = member
            && n.parts
                .iter()
                .find(|p| p.role == Role::Property)
                .is_some_and(|p| self.text(p.start, p.end).starts_with('['));
        let slot = |c: &str| cat!(c, "@", n.start);
        let wanted: Vec<String> = [
            (compound, "assign.old"),
            (member, "assign.object"),
            (computed && self.has(&slot("assign.key")), "assign.key"),
        ]
        .iter()
        .filter(|(on, c)| *on && self.has(&slot(c)))
        .map(|(_, c)| slot(c))
        .collect();
        let plain = !member
            || n.parts
                .iter()
                .any(|p| p.role == Role::Object && self.text(p.start, p.end) != "super");
        if plain && !logical && !wanted.is_empty() && wanted.iter().all(|w| self.rereads(w)) {
            for w in &wanted {
                self.consume(w);
            }
            let r = n.parts[k];
            let e = self.expr(i + 1)?;
            return Ok(P::L(alloc::vec![
                s("("),
                self.rest(n.start, r.start),
                e,
                self.rest(r.end, n.end),
                s(")")
            ]));
        }
        let bin = String::from(opt.strip_suffix('=').unwrap_or(&opt));
        let target: P = if let Some(l) = n.parts.iter().position(|p| p.role == Role::Left) {
            let t = self.text(n.parts[l].start, n.parts[l].end).trim();
            // A pattern destructures K's value as it would the original's: its targets
            // and defaults all come after the right side.
            let pattern = opt == "=" && (t.starts_with('{') || t.starts_with('['));
            if !is_ident(t) && !pattern {
                return no("an assignment to a pattern around the site (0203)");
            }
            P::V(n.parts[l].start, n.parts[l].end)
        } else {
            let o = n.parts.iter().position(|p| p.role == Role::Object);
            let pr = n.parts.iter().position(|p| p.role == Role::Property);
            let (Some(o), Some(pr)) = (o, pr) else {
                return no("an assignment target the compiler did not describe");
            };
            if self.text(n.parts[o].start, n.parts[o].end) == "super" {
                return no("an assignment to super around the site (0203)");
            }
            let obj = self.tmp("assign.object", n.start)?;
            let p = n.parts[pr];
            if self.text(p.start, p.end).starts_with('[') {
                let key = if self.has(&cat!("assign.key@", n.start)) {
                    self.tmp("assign.key", n.start)?
                } else {
                    self.rest(p.start + 1, p.end - 1)
                };
                P::L(alloc::vec![obj, s("["), key, s("]")])
            } else {
                P::L(alloc::vec![obj, s("."), P::V(p.start, p.end)])
            }
        };
        let e = self.expr(i + 1)?;
        // (K's own assignment and operator: rebuilt from the original's frame.)
        let target = P::L(alloc::vec![self.kmark(), target]);
        Ok(if compound {
            let old = self.tmp("assign.old", n.start)?;
            P::L(alloc::vec![
                s("("),
                target,
                s(" = "),
                self.kmark(),
                old,
                P::S(cat!(" ", bin, " ")),
                e,
                s(")")
            ])
        } else {
            P::L(alloc::vec![s("("), target, s(" = "), e, s(")")])
        })
    }

    /// The site in an assignment's member target (`(await o).p = v`, `o[await k] = v`):
    /// nothing of the store happened yet, so K's assignment is the original's from the
    /// site on, with its operator as written (a compound one reads the old value itself).
    fn assign_target_site(&mut self, i: usize, n: &Node, k: usize) -> R<P> {
        let role = |r: Role| n.parts.iter().position(|p| p.role == r);
        let (Some(o), Some(pr), Some(op), Some(r)) = (
            role(Role::Object),
            role(Role::Property),
            role(Role::Op),
            role(Role::Right),
        ) else {
            return no("an assignment target the compiler did not describe");
        };
        if self.text(n.parts[o].start, n.parts[o].end) == "super" {
            return no("an assignment to super around the site (0203)");
        }
        // (Before its key, the target is still the member expression the compiler read,
        // its object named where it starts.)
        let at = if k == o || self.has(&cat!("member.object@", n.parts[o].start)) {
            n.parts[o].start
        } else {
            n.start
        };
        let p = n.parts[pr];
        let computed = self.text(p.start, p.end).starts_with('[');
        let opt = String::from(self.text(n.parts[op].start, n.parts[op].end));
        // Should K stop again in its assignment, its member's object, its target's
        // reference and old value keep what the original's do.
        let mut marks = Vec::new();
        let mut kslots = alloc::vec![(cat!("assign.object@", n.start), "assign.object")];
        if computed {
            kslots.push_out((cat!("member.object@", at), "member.object"));
            kslots.push_out((cat!("assign.key@", n.start), "assign.key"));
        }
        if opt != "=" && !matches!(opt.as_str(), "&&=" | "||=" | "??=") {
            kslots.push_out((cat!("assign.old@", n.start), "assign.old"));
        }
        for (fslot, kc) in kslots {
            marks.push_out(self.ktemp(fslot, kc));
        }
        let obj = if k == o {
            self.expr(i + 1)?
        } else {
            self.tmp("member.object", at)?
        };
        let key = if !computed {
            P::L(alloc::vec![s("."), P::V(p.start, p.end)])
        } else if k == pr {
            P::L(alloc::vec![s("["), self.expr(i + 1)?, s("]")])
        } else {
            P::L(alloc::vec![
                s("["),
                self.rest(p.start + 1, p.end - 1),
                s("]")
            ])
        };
        let e = self.rest_part(n, r);
        Ok(P::L(alloc::vec![
            s("("),
            P::L(marks),
            obj,
            key,
            P::S(cat!(" ", opt, " ")),
            e,
            s(")")
        ]))
    }

    fn object(&mut self, i: usize, n: &Node, k: usize) -> R<P> {
        // The literal as written, where its properties before the site read again what
        // the object made so far holds (D2836, dump_reread.rs).
        let lit = cat!("object.literal@", n.start);
        if self.rereads(&lit) {
            self.consume(&lit);
            // (A spread's part has its `...`.)
            let at = n.parts[k].start
                + if n.parts[k].role == Role::Spread {
                    3
                } else {
                    0
                };
            let e = self.expr(i + 1)?;
            return Ok(P::L(alloc::vec![
                self.rest(n.start, at),
                e,
                self.rest(n.parts[k].end, n.end)
            ]));
        }
        // The object made so far; the rest of the literal is defined on it in order.
        let ot = self.tmp_text("object.literal", n.start)?;
        // The first part of the property holding the site.
        let mut first = k;
        if n.parts[k].role == Role::Value && k > 0 {
            first = k - 1;
        }
        let mut items: Vec<P> = Vec::new();
        let mut m = first;
        while m < n.parts.len() {
            let p = n.parts[m];
            match p.role {
                Role::Spread => {
                    let v = if m == k {
                        self.expr(i + 1)?
                    } else {
                        self.rest(p.start + 3, p.end)
                    };
                    items.push_out(P::L(alloc::vec![
                        self.kmark(),
                        s("$djs_A("),
                        self.kmark(),
                        P::S(cat!(ot, ", ")),
                        v,
                        s(")")
                    ]));
                    m += 1;
                }
                Role::Key | Role::ComputedKey => {
                    let vi = m + 1;
                    let key = if p.role == Role::Key {
                        let t = self.text(p.start, p.end);
                        if t == "__proto__" || t == "\"__proto__\"" || t == "'__proto__'" {
                            return no("__proto__ in an object literal around the site");
                        }
                        if is_ident(t) {
                            P::S(quote(t))
                        } else if t.bytes().all(|b| b.is_ascii_digit()) {
                            P::S(cat!("\"", t, "\""))
                        } else if t.starts_with('"') || t.starts_with('\'') {
                            P::V(p.start, p.end)
                        } else {
                            return no("a numeric key in an object literal around the site");
                        }
                    } else if m < k {
                        // Evaluated before the site, as a property key.
                        self.tmp("object.key", p.start)?
                    } else if m == k {
                        // The site in the key: converted as the original does.
                        let e = self.expr(i + 1)?;
                        P::L(alloc::vec![self.kmark(), s("$djs_P("), e, s(")")])
                    } else {
                        // Converted before the value, as the original does; the
                        // original's converted key is this argument of K's call.
                        let m = self.mark("tmp", 0);
                        if let P::Mark(x) = m {
                            self.scaf[x].3 =
                                cat!(",", quote(&cat!("object.key@", p.start)), ",\"call.arg\"");
                        }
                        P::L(alloc::vec![
                            m,
                            self.kmark(),
                            s("$djs_P("),
                            self.rest(p.start + 1, p.end - 1),
                            s(")")
                        ])
                    };
                    let v = if vi == k {
                        self.expr(i + 1)?
                    } else {
                        self.rest_part(n, vi)
                    };
                    // K's own call (origin tracking rebuilds its callee and
                    // fixed arguments from the original's frame).
                    items.push_out(P::L(alloc::vec![
                        self.kmark(),
                        s("$djs_D("),
                        self.kmark(),
                        P::S(cat!(ot, ", ")),
                        self.kmark(),
                        key,
                        s(", "),
                        v,
                        s(")")
                    ]));
                    m = vi + 1;
                }
                Role::Shorthand => {
                    let t = String::from(self.text(p.start, p.end));
                    items.push_out(P::L(alloc::vec![
                        self.kmark(),
                        s("$djs_D("),
                        self.kmark(),
                        P::S(cat!(ot, ", ")),
                        self.kmark(),
                        P::S(cat!(quote(&t), ", ")),
                        self.rest(p.start, p.end),
                        s(")"),
                    ]));
                    m += 1;
                }
                Role::Method => {
                    // Defined as the literal would: its descriptor from a literal of its
                    // own (a method, getter or setter, its key evaluated in order).
                    items.push_out(P::L(alloc::vec![
                        self.kmark(),
                        s("$djs_DD("),
                        self.kmark(),
                        P::S(cat!(ot, ", { ")),
                        P::V(p.start, p.end),
                        s(" })"),
                    ]));
                    m += 1;
                }
                _ => {
                    return no(
                        "a property after the site in an object literal the exporter does not know",
                    );
                }
            }
        }
        let mut out = alloc::vec![s("(")];
        push_joined(&mut out, items);
        out.push_out(P::S(cat!(", ", ot, ")")));
        Ok(P::L(out))
    }
}

/// `p` as text, without the lines of binding `skip` (for liveness, D2911).
fn render(p: &P, ftext: &str, skip: &str, out: &mut String) {
    match p {
        P::S(t) => out.push_str(t),
        P::T(t) => out.push_str(t),
        P::Decl(n, t) => {
            if n != skip {
                out.push_str(t)
            }
        }
        P::Lit(_, t) => out.push_str(t),
        // (Only the bindings of K the dump names there, D3003; apart from what it stands
        // between.)
        P::Val(_, reads) => {
            for n in reads {
                out.push(' ');
                out.push_str(n);
            }
            out.push(' ');
        }
        P::V(a, b) => out.push_str(ftext.get(*a as usize..*b as usize).unwrap_or("")),
        P::Mark(_) | P::Helpers => {}
        P::L(list) => {
            for q in list {
                render(q, ftext, skip, out);
            }
        }
        P::Scope(_, _, q) => {
            out.push(' ');
            render(q, ftext, skip, out);
            out.push(' ');
        }
    }
}

/// The bindings `p` declares itself (not in nested scopes).
fn decl_names(p: &P, out: &mut Vec<String>) {
    match p {
        P::Decl(n, _) => {
            if !out.contains(n) {
                out.push_out(n.clone());
            }
        }
        P::L(list) => {
            for q in list {
                decl_names(q, out);
            }
        }
        _ => {}
    }
}

/// Leaves out every line of binding `name` in `p`.
fn drop_decls(p: &mut P, name: &str) {
    match p {
        P::Decl(n, _) if n == name => *p = s(""),
        P::L(list) => {
            for q in list {
                drop_decls(q, name);
            }
        }
        P::Scope(names, _, q) => {
            names.retain(|n| n != name);
            drop_decls(q, name);
        }
        _ => {}
    }
}

/// Liveness (D2911): leaves out the frame bindings scope `p` declares (and the scopes in
/// it) that nothing in the scope reads: their declarations, assignments and, in a scope
/// whose names are all gone, its braces.
fn prune(p: &mut P, ftext: &str) {
    let mut names = Vec::new();
    match &*p {
        P::Scope(_, _, q) => decl_names(q, &mut names),
        _ => decl_names(p, &mut names),
    }
    if !names.is_empty() {
        // One rendering: a name is read where it occurs outside its own lines and those
        // of the names left out before it (in order, as if each were rendered without
        // them).
        let mut text = String::new();
        let mut lines = Vec::new();
        render_lines(p, ftext, &mut text, &mut lines);
        let mut dropped: Vec<&str> = Vec::new();
        for n in &names {
            let inside = |at: usize| {
                lines.iter().any(|(a, b, m)| {
                    *a <= at && at < *b && (m == n || dropped.contains(&m.as_str()))
                })
            };
            if !ident_at(&text, n).any(|at| !inside(at)) {
                dropped.push_out(n.as_str());
            }
        }
        for n in dropped {
            drop_decls(p, n);
        }
    }
    match p {
        P::Scope(_, _, q) => prune_nested(q, ftext),
        _ => prune_nested(p, ftext),
    }
}

/// [`render`] of all of `p`, with where each binding's line is: `[start, end, name]`.
fn render_lines(p: &P, ftext: &str, out: &mut String, lines: &mut Vec<(usize, usize, String)>) {
    match p {
        P::Decl(n, t) => {
            lines.push_out((out.len(), out.len() + t.len(), n.clone()));
            out.push_str(t)
        }
        P::L(list) => {
            for q in list {
                render_lines(q, ftext, out, lines);
            }
        }
        P::Scope(_, _, q) => {
            out.push(' ');
            render_lines(q, ftext, out, lines);
            out.push(' ');
        }
        _ => render(p, ftext, "", out),
    }
}

/// Where the identifier `name` occurs in `text` (not as part of a longer one).
fn ident_at<'t>(text: &'t str, name: &'t str) -> impl Iterator<Item = usize> + 't {
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'$';
    let b = text.as_bytes();
    text.match_indices(name).map(|(s, _)| s).filter(move |&s| {
        let e = s + name.len();
        (s == 0 || !ident(b[s - 1])) && (e == b.len() || !ident(b[e]))
    })
}

/// [`prune`] for the scopes in `p`.
fn prune_nested(p: &mut P, ftext: &str) {
    match p {
        P::L(list) => {
            for q in list {
                prune_nested(q, ftext);
            }
        }
        P::Scope(..) => prune(p, ftext),
        _ => {}
    }
}

fn push_joined(out: &mut Vec<P>, list: Vec<P>) {
    for (x, p) in list.into_iter().enumerate() {
        if x > 0 {
            out.push_out(s(", "));
        }
        out.push_out(p);
    }
}

/// A value that can stand in place of a temporary.
fn literal(v: &str) -> bool {
    matches!(v, "undefined" | "true" | "false" | "null")
        || v.parse::<f64>().is_ok()
            && !v.contains(['e', 'E', 'x', 'X', 'o', 'O', 'b', 'B', 'n', 'i', 'N'])
        || (v.len() >= 2
            && v.starts_with('"')
            && v.ends_with('"')
            && !v[1..v.len() - 1].contains(['"', '\\']))
}

/// A value the dump's factories make (`$dump.s3.f`): written after the block's code.
fn has_factory_ref(v: &str) -> bool {
    let b = v.as_bytes();
    let mut i = 0;
    // (By `$` and a compare: see `has_ident`.)
    while let Some(at) = b[i..].iter().position(|&c| c == b'$') {
        let s = i + at;
        let j = s + "$dump.s".len();
        if b.get(s..j) == Some(b"$dump.s") && b.get(j).is_some_and(u8::is_ascii_digit) {
            return true;
        }
        i = s + 1;
    }
    false
}

fn kind_name(k: NodeKind) -> &'static str {
    match k {
        NodeKind::Body => "a function body",
        NodeKind::Block => "a block",
        NodeKind::Expr => "an expression statement",
        NodeKind::Decl(_) => "a declaration",
        NodeKind::If => "if",
        NodeKind::Label => "a label",
        NodeKind::While => "while",
        NodeKind::DoWhile => "do-while",
        NodeKind::For => "for",
        NodeKind::ForIn => "for-in",
        NodeKind::ForOf => "for-of",
        NodeKind::ForAwait => "for await",
        NodeKind::Try => "try",
        NodeKind::Switch => "switch",
        NodeKind::Return => "return",
        NodeKind::Throw => "throw",
        NodeKind::Await => "await",
        NodeKind::Yield => "yield",
        NodeKind::YieldStar => "yield* (0203)",
        NodeKind::Call { .. } => "a call",
        NodeKind::New => "new",
        NodeKind::ImportCall => "import() (0203)",
        NodeKind::Member { .. } => "a member",
        NodeKind::Binary => "a binary operator",
        NodeKind::Logical => "a logical operator",
        NodeKind::Assign => "an assignment",
        NodeKind::Unary => "a unary operator",
        NodeKind::Update => "an update (0203)",
        NodeKind::Conditional => "a conditional",
        NodeKind::Sequence => "a sequence",
        NodeKind::Template => "a template literal",
        NodeKind::TaggedTemplate => "a tagged template (0203)",
        NodeKind::Array => "an array literal",
        NodeKind::Object => "an object literal",
        NodeKind::ArrayPattern => "an array pattern (0203)",
        NodeKind::ObjectPattern => "an object pattern (0203)",
        NodeKind::Default => "a default value (0203)",
    }
}

/// The names the dump's code `lines` declares (`let x = …`, `class C …`).
fn declared_names(lines: &[String]) -> Vec<&str> {
    let mut out = Vec::new();
    for l in lines {
        for kw in ["let ", "const ", "var ", "class ", "function "] {
            if let Some(r) = l.strip_prefix(kw) {
                let n = r
                    .bytes()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'$')
                    .count();
                if n > 0 {
                    out.push_out(&r[..n]);
                }
            }
        }
    }
    out
}

/// The order of a block's dump code `content` (`Ok(k)`) and its late bindings (`Err(i)`,
/// name and value each): as written, code first, where nothing below holds it back
/// (0215, D3150). A binding comes after the line that fills a factory member it reads
/// ([`waits_for`]), the line that declares a name it reads, and a binding whose name it
/// reads; a line of code after the lines before it and, where that can be, after the
/// bindings whose names it reads (those names err on the side of too many: a function's
/// own name, a parameter; where nothing can come next, they give way, and then the order
/// as written).
fn block_order(content: &[String], late: &[(String, String)]) -> Vec<Result<usize, usize>> {
    let (c, l) = (content.len(), late.len());
    if l == 0 {
        return (0..c).map(Ok).collect();
    }
    let declared: Vec<Vec<&str>> = content
        .iter()
        .map(|x| declared_names(core::slice::from_ref(x)))
        .collect();
    // A binding's hard predecessors: lines of code and other bindings.
    let mut late_after: Vec<(Vec<usize>, Vec<usize>)> = Vec::new();
    for (i, (_, v)) in late.iter().enumerate() {
        let names = super::plain::eager_names(v);
        let lines: Vec<usize> = (0..c)
            .filter(|&k| {
                waits_for(v, core::slice::from_ref(&content[k]))
                    || declared[k].iter().any(|d| names.iter().any(|n| n == d))
            })
            .collect();
        let binds: Vec<usize> = (0..l)
            .filter(|&j| j != i && names.contains(&late[j].0))
            .collect();
        late_after.push_out((lines, binds));
    }
    // A line's soft predecessors: the bindings whose names it reads.
    let reads: Vec<Vec<usize>> = content
        .iter()
        .map(|x| {
            let names = super::plain::eager_names(x);
            (0..l).filter(|&j| names.contains(&late[j].0)).collect()
        })
        .collect();
    let mut done_c = alloc::vec![false; c];
    let mut done_l = alloc::vec![false; l];
    let mut out = Vec::with_capacity(c + l);
    let mut next_c = 0;
    while out.len() < c + l {
        let late_ready = |i: usize, done_c: &[bool], done_l: &[bool]| {
            !done_l[i]
                && late_after[i].0.iter().all(|&k| done_c[k])
                && late_after[i].1.iter().all(|&j| done_l[j])
        };
        // (Strict, then without the soft predecessors, then as written.)
        let mut pick = None;
        for soft in [true, false] {
            if next_c < c && (!soft || reads[next_c].iter().all(|&j| done_l[j])) {
                pick = Some(Ok(next_c));
                break;
            }
            if let Some(i) = (0..l).find(|&i| late_ready(i, &done_c, &done_l)) {
                pick = Some(Err(i));
                break;
            }
        }
        let pick = pick.unwrap_or_else(|| match (0..l).find(|&i| !done_l[i]) {
            Some(i) if next_c >= c => Err(i),
            _ => Ok(next_c),
        });
        match pick {
            Ok(k) => {
                done_c[k] = true;
                next_c += 1;
            }
            Err(i) => done_l[i] = true,
        }
        out.push_out(pick);
    }
    out
}

/// Whether value `v` reads a member the dump's code `lines` makes: `$dump.F.K` (or `$dump.F`
/// itself) where a line assigns members to `F` and names `K` (by its text: a member it
/// does not make only keeps the order as it was).
fn waits_for(v: &str, lines: &[String]) -> bool {
    let refs = super::plain::refs(v).dump;
    if refs.is_empty() {
        return false;
    }
    lines.iter().any(|l| {
        l.match_indices("Object.assign($dump.").any(|(at, _)| {
            let r = &l[at + "Object.assign($dump.".len()..];
            let n = r
                .bytes()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == b'_')
                .count();
            let f = &r[..n];
            refs.iter()
                .any(|(name, key)| name == f && key.as_deref().is_none_or(|k| has_ident(l, k)))
        })
    })
}
