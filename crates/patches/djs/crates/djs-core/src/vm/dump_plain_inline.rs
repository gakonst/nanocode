//! Single-use values written where they are used (0216's rules 3 and 4, D3001).
//!
//! A value the dump names once (`$dump.p1 = $djs.resume(…)`, read only by a
//! continuation's `await $dump.p1`) is written in place of that reference, its line left
//! out, where moving its evaluation there keeps the dump's order:
//!
//! - a *pure* value (a data literal, a function, a builtin iterator, a host value) may move
//!   to any later point of the load: making it again later gives the same value, nothing
//!   else holds it, and it reads no name a later statement could change;
//! - an *effectful* one (a host call reopened, an activation, a reaction, anything else)
//!   only to the first thing its use's line evaluates, with nothing effectful in between:
//!   a continuation's site operand where its prefix does nothing before the site, the
//!   first argument of a builtin call (`Promise.prototype.then.call($dump.a3, …)`), or the
//!   Script's completion value right after it.
//!
//! The uses are counted over everything the writer gets (lines, factories, activations,
//! handles) before it writes: a value counted more often than it is written stays named,
//! and a value read again after it was written in place is a refusal (a bug).

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;

use super::{IK, W, ident_part, ident_start, scan_spans};
use crate::buf::PushOut;

/// A value whose line is written and may still go where its one use is.
pub(super) struct Pend {
    name: String,
    rhs: String,
    /// The line list it was written in (its nesting) and its index there.
    depth: usize,
    idx: usize,
    effect: bool,
    /// The names its text binds itself (a continuation's): its reads of them are its own.
    bound: Vec<String>,
}

/// The names a value written in place may read.
#[derive(Clone, Copy)]
enum Reads<'a> {
    Any,
    Only(&'a [&'a str]),
    /// None that a continuation binds where the reference is (`scope`).
    Scope(&'a dyn Fn(&str) -> bool),
}

#[derive(Default)]
pub(super) struct Inline {
    /// `$dump.X` references over the writer's input, by name (a value's own `$dump.X = `
    /// among them).
    /// (By the names' hashes, `name_hash`, 0215: a collision only adds uses.)
    pub uses: Counts,
    /// `$dump.F.key` references (a factory's members), by factory and key.
    /// (By [`key_hash`], 0215.)
    pub key_uses: Counts,
    pub pend: Vec<Pend>,
    /// The lines of each list being written (a callback's inside the top level's).
    pub stack: Vec<Vec<String>>,
    /// Per list, the indexes of its effectful lines (those written in place left out).
    pub effects: Vec<Vec<usize>>,
    /// The values written in place.
    pub done: BTreeSet<String>,
    /// The names of `pend` (0215: a data-heavy dump has thousands).
    pub pend_names: NameSet,
    /// Reopened host calls used once whose lines wait for the line that reads them, in
    /// their order (D3004): list depth, name, line, value.
    pub hooks: Vec<(usize, String, String, String)>,
    /// The names of `hooks`.
    pub hook_names: NameSet,
    /// The names of K's the closures and continuations written in place read (liveness
    /// must see them in a site's operand, D3003).
    pub placed_reads: Vec<String>,
    /// Per list, whether deferred host calls are written into it (D3004).
    pub pull: Vec<bool>,
}

/// Builtins a pure value may name (frozen builtins: nothing replaces them).
const PURE_NAMES: &[&str] = &[
    "undefined",
    "true",
    "false",
    "null",
    "NaN",
    "Infinity",
    "Map",
    "Set",
    "Date",
    "Symbol",
    "Object",
    "Array",
    "RegExp",
    "BigInt",
    "Error",
    "TypeError",
    "RangeError",
    "$djs",
    // (`new Map(…)`: `shape` admits only builtins after it.)
    "new",
];

/// Whether evaluating value text `rhs` later during the load gives the same value and does
/// nothing else.
pub(super) fn pure(rhs: &str) -> bool {
    // (A function's creation reads nothing: its names are read when it runs, from where
    // it is written; `reads` checks those where that matters.)
    if super::is_fn_text(rhs) {
        return true;
    }
    // (A literal's other reads: values the dump wrote, builtins; its keys are none.)
    shape(rhs) && !super::reads_other(rhs.trim(), PURE_NAMES)
}

/// Whether evaluating value text `rhs` only makes a value (reading names, calling nothing).
pub(super) fn shape(rhs: &str) -> bool {
    let t = rhs.trim();
    let starts = |p: &str| t.starts_with(p);
    starts("[")
        || starts("{")
        || starts("\"")
        || starts("'")
        || starts("`")
        || t.as_bytes().first().is_some_and(u8::is_ascii_digit)
        || starts("-") && t[1..].starts_with(|c: char| c.is_ascii_digit() || c == 'I')
        || starts("function")
        || starts("async function")
        || starts("class ")
        || starts("$djs.host(")
        || starts("$djs.source(")
        || starts("$djs.bytes(")
        || starts("$djs.advance(")
        || starts("new Map(")
        || starts("new Set(")
        || starts("new Date(")
        || matches!(
            t,
            "true" | "false" | "null" | "undefined" | "NaN" | "Infinity"
        )
        // (A name read: no getter, unlike a property; a value the dump made, or its
        // factory's member.)
        || super::is_ident(t)
        || t.strip_prefix("$dump.")
            .is_some_and(|r| r.split('.').all(|p| !p.is_empty() && super::is_ident(p)))
}

/// Whether value text `rhs` is a function or class without a name of its own (a name
/// would be inferred where it is written as an initializer or a property's value).
fn anonymous(rhs: &str) -> bool {
    let t = rhs.trim();
    let t = t.strip_prefix("async").map_or(t, str::trim_start);
    if let Some(r) = t.strip_prefix("function") {
        let r = r.trim_start().trim_start_matches('*').trim_start();
        return r.starts_with('(');
    }
    if let Some(r) = t.strip_prefix("class") {
        let r = r.trim_start();
        return r.starts_with('{') || r.starts_with("extends");
    }
    super::is_fn_text(t)
}

/// `rhs` written in place of `text[s..e]`: a function as an element, an argument or an
/// initializer as it is (no parentheses: djs compiles it lazily, D2995), else as
/// [`operand`].
fn operand_at(text: &str, s: usize, e: usize, rhs: &str) -> String {
    let prev = text[..s].trim_end();
    let next = text[e..].trim_start();
    let list = matches!(prev.as_bytes().last(), Some(b'(' | b',' | b'[' | b'='))
        && !prev.ends_with("=>")
        && !prev.ends_with("==")
        && (next.is_empty() || matches!(next.as_bytes()[0], b',' | b')' | b']' | b';'));
    if list && super::is_fn_text(rhs) {
        String::from(rhs.trim())
    } else {
        operand(rhs)
    }
}

/// `rhs` as an operand: in parentheses unless it is one already.
pub(super) fn operand(rhs: &str) -> String {
    let t = rhs.trim();
    let b = t.as_bytes();
    let call_or_literal = b
        .first()
        .is_some_and(|&c| ident_start(c) || matches!(c, b'[' | b'{' | b'"' | b'\'' | b'`'))
        || b.first().is_some_and(u8::is_ascii_digit);
    let keyword = [
        "function", "async", "class", "new", "typeof", "void", "await", "yield",
    ]
    .iter()
    .any(|k| t.starts_with(k) && !t.as_bytes().get(k.len()).is_some_and(|&c| ident_part(c)));
    // (`new C(…)` is a member expression already: `new C(…).then`, `new C(…)(x)`.)
    let new_call = t.strip_prefix("new ").is_some_and(|r| {
        let n = r
            .bytes()
            .take_while(|&c| ident_part(c) || c == b'.')
            .count();
        n > 0 && r.as_bytes().get(n) == Some(&b'(') && r.ends_with(')')
    });
    if new_call || call_or_literal && !keyword && !super::is_fn_text(t) {
        String::from(t)
    } else {
        cat!("(", t, ")")
    }
}

impl W<'_> {
    /// Counts the `$dump.X` references of everything the writer writes from.
    pub(super) fn count_uses(&mut self) {
        let ir = self.ir;
        let mut texts: Vec<&str> = Vec::new();
        // (An activation's or a factory's line is the format 2 one: the writer writes them
        // from the activation and the factory, counted below.)
        texts.extend(
            self.items
                .iter()
                .filter(|it| !matches!(it.kind, IK::Act(_) | IK::Factory(_)))
                .map(|it| it.text.as_str()),
        );
        for (ai, a) in ir.acts.iter().enumerate() {
            let cells = &self.acts[ai].cells;
            texts.push_out(a.fexpr.as_str());
            texts.push_out(a.this.as_str());
            texts.extend(a.awaiting.as_deref());
            texts.extend(a.proto.as_deref());
            texts.extend(a.super_proto.as_deref());
            // (A temporary K evaluates again is not written, D2836, `kept_temp` where it
            // is after all; a variable shared with closures is written from its
            // factory's.)
            texts.extend(
                a.slots
                    .iter()
                    .filter(|s| {
                        !a.rereads.contains(&s.name) && !cells.iter().any(|c| c.0 == s.name)
                    })
                    .map(|s| s.value.as_str()),
            );
            for (_, v, p) in &a.queue {
                texts.push_out(v.as_str());
                texts.push_out(p.as_str());
            }
            for u in &a.usings {
                texts.extend(u.triples.iter().flat_map(|t| [t.0.as_str(), t.1.as_str()]));
                texts.extend(u.state.iter().map(|s| s.1.as_str()));
            }
        }
        for (fi, f) in ir.factories.iter().enumerate() {
            for (si, sc) in f.scopes.iter().enumerate() {
                texts.extend(sc.other.iter().map(String::as_str));
                for (vi, v) in sc.vars.iter().enumerate() {
                    // (A variable a frame takes is its continuation's binding, of its
                    // value; the others are their factory's lines.)
                    if self.is_merged((fi, si)) && !self.dead((fi, si), vi) {
                        texts.extend(v.init.as_deref());
                    } else {
                        texts.push_out(v.line.as_str());
                    }
                }
                texts.extend(sc.props.iter().map(|p| p.text.as_str()));
            }
        }
        texts.extend(ir.handles.iter().map(|h| h.1.as_str()));
        // (About one name per item: no growing, 0215.)
        let mut uses = Counts::with_capacity(self.items.len());
        let mut key_uses = Counts::default();
        // (By substring, not `scan`, 0215: one in a string counts too, which only keeps a
        // value named.)
        for t in texts {
            for (s, e) in dump_ref_spans(t) {
                let name = &t[s + 6..e];
                uses.add(name_hash(name));
                if t.as_bytes().get(e) == Some(&b'.') {
                    let (key, _) = super::ident_at(t, e + 1);
                    if !key.is_empty() {
                        key_uses.add(key_hash(name, key));
                    }
                }
            }
        }
        self.inl.uses = uses;
        self.inl.key_uses = key_uses;
    }

    /// Whether `$dump.name` is read once besides its own line (`own`: whether its line is
    /// among the counted texts).
    pub(super) fn single(&self, name: &str, own: bool) -> bool {
        self.inl.uses.get(&name_hash(name)).copied() == Some(1 + u32::from(own))
    }

    /// Whether `$dump.F.key` (a factory's member) is read once.
    fn single_key(&self, fac: &str, key: &str) -> bool {
        self.inl.key_uses.get(&key_hash(fac, key)).copied() == Some(1)
    }

    /// Member `key` of factory `fac` where it is a closure of a merged scope placed in a
    /// block of activation `a`'s continuation: its scope, index and the block.
    fn block_prop(&self, a: usize, fac: &str, key: &str) -> Option<(super::Sc, usize, usize)> {
        let &f = self.fac_by_name.get(fac)?;
        for (si, sc) in self.ir.factories[f].scopes.iter().enumerate() {
            let Some(i) = sc.props.iter().position(|p| p.key == key) else {
                continue;
            };
            let pl = self
                .placed
                .iter()
                .find(|x| x.1 == super::Put::Prop((f, si), i))?
                .0;
            return match pl {
                super::Pl::Block(a2, b) if a2 == a => Some(((f, si), i, b)),
                _ => None,
            };
        }
        None
    }

    /// The activation whose continuation is member `key` of factory `fac`, placed in a
    /// block of activation `a`'s continuation at or above `block`, its text written, and
    /// reading no name a block between declares.
    fn block_k(&self, a: usize, block: usize, fac: &str, key: &str) -> Option<usize> {
        let a2 = self.kkeys.iter().position(|k| k == key)?;
        let (hsc, _) = self.acts[a2].home?;
        if self.ir.factories[hsc.0].name != fac || self.taken_k.contains(&a2) {
            return None;
        }
        let kt = self.acts[a2].k.as_ref()?;
        let pb = self.placed.iter().find_map(|x| match (x.0, x.1) {
            (super::Pl::Block(a3, b), super::Put::K(k)) if a3 == a && k == a2 => Some(b),
            _ => None,
        })?;
        (pb <= block && !self.shadowed(a, pb, block, &kt.text)).then_some(a2)
    }

    /// Whether a block of activation `a`'s continuation in `(from, to]` declares one of
    /// `names` (a closure written there would see that binding instead).
    fn declared_between(&self, a: usize, from: usize, to: usize, names: &[String]) -> bool {
        let blocks = &self.acts[a].blocks;
        (from + 1..=to).any(|b| {
            blocks
                .get(b)
                .is_some_and(|ds| ds.iter().any(|d| names.contains(&d.name)))
        })
    }

    /// Whether a block of activation `a`'s continuation in `(from, to]` declares a name
    /// `text` reads (not only as an object literal's key): written there, `text` would
    /// see that binding instead. (By the few names declared, not a scan of `text`, 0215.)
    fn shadowed(&self, a: usize, from: usize, to: usize, text: &str) -> bool {
        let blocks = &self.acts[a].blocks;
        let b = text.as_bytes();
        (from + 1..=to).any(|i| {
            blocks.get(i).is_some_and(|ds| {
                ds.iter().any(|d| {
                    let n = d.name.as_str();
                    text.match_indices(n).any(|(at, _)| {
                        !(at > 0 && (ident_part(b[at - 1]) || b[at - 1] == b'.'))
                            && !b.get(at + n.len()).is_some_and(|&c| ident_part(c))
                    }) && !only_keys(text, n)
                })
            })
        })
    }

    /// The block of activation `a`'s continuation that declares slot `sname`.
    fn block_of(&self, a: usize, sname: &str) -> Option<usize> {
        self.acts[a]
            .blocks
            .iter()
            .position(|ds| ds.iter().any(|d| d.sname.as_deref() == Some(sname)))
    }

    /// `text`, a part of activation `a`'s continuation in its block `block`, with the
    /// closures that block or one above makes for this one use written here (D3003):
    /// `$dump.s2.todos` → `async function todos(path) {…}`, the member left out of its
    /// block's `Object.assign`. A function's creation evaluates nothing, so it may be
    /// later; the names it reads must be the same bindings here.
    pub(super) fn props_in_place(&mut self, a: usize, block: usize, text: &str) -> String {
        if !text.contains("$dump.") {
            return String::from(text);
        }
        let b = text.as_bytes();
        let mut out = String::new();
        let mut at = 0;
        for (s, e) in super::eager_spans(text) {
            if b.get(e) != Some(&b'.') {
                continue;
            }
            let fac = &text[s + 6..e];
            let (key, end) = super::ident_at(text, e + 1);
            if key.is_empty() {
                continue;
            }
            // A continuation its block makes, called here (its one use, `$djs.k(K, …)(…)`).
            if let Some(a2) = self.block_k(a, block, fac, key) {
                let kt = self.acts[a2]
                    .k
                    .as_ref()
                    .map(|k| k.text.clone())
                    .unwrap_or_default();
                self.note_reads(a, &kt);
                out.push_str(&text[at..s]);
                if matches!(b.get(end), Some(b'.' | b'(' | b'[' | b'`')) {
                    out.push_str(&operand(&kt));
                } else {
                    out.push_str(&kt);
                }
                at = end;
                self.taken_k.push_out(a2);
                continue;
            }
            if !self.single_key(fac, key) {
                continue;
            }
            let Some((sc, i, pb)) = self.block_prop(a, fac, key) else {
                continue;
            };
            if pb > block || self.taken.contains(&(sc, i)) {
                continue;
            }
            let ptext = &self.scope(sc).props[i].text;
            let Some(expr) = ptext.strip_prefix(key).and_then(|r| r.strip_prefix(": ")) else {
                continue;
            };
            if !super::is_fn_text(expr) {
                continue;
            }
            if self.shadowed(a, pb, block, expr) {
                continue;
            }
            // (`(0, f)` keeps it anonymous where a name would be inferred.)
            let prev = text[..s].trim_end();
            let infers = (prev.ends_with('=') && !prev.ends_with("==") && !prev.ends_with("=>"))
                || prev.ends_with(':');
            let expr = match expr.strip_prefix("(0, ").and_then(|r| r.strip_suffix(')')) {
                Some(inner) if !infers && super::is_fn_text(inner) => inner,
                _ => expr,
            };
            let expr = if matches!(b.get(end), Some(b'.' | b'(' | b'[' | b'`')) {
                operand(expr)
            } else {
                String::from(expr)
            };
            self.note_reads(a, &expr);
            out.push_str(&text[at..s]);
            out.push_str(&expr);
            at = end;
            self.taken.push_out((sc, i));
            if let Some(n) = self.inl.key_uses.get_mut(&key_hash(fac, key)) {
                *n -= 1;
            }
        }
        out.push_str(&text[at..]);
        out
    }

    /// K writes the value of temporary `name` of activation `a` that the count took as
    /// evaluated again (D2836): the values it names are read once more, and none goes in
    /// place any more.
    pub(super) fn kept_temp(&mut self, a: usize, name: &str) {
        let act = &self.ir.acts[a];
        if !act.rereads.iter().any(|r| r == name) {
            return;
        }
        let Some(sl) = act.slots.iter().find(|s| s.name == name) else {
            return;
        };
        for (s, e) in dump_refs(&sl.value) {
            let n = &sl.value[s + 6..e];
            self.inl.uses.add(name_hash(n));
            if let Some(p) = self.inl.pend.iter().position(|p| p.name == n) {
                self.inl.pend.remove(p);
                self.inl.pend_names.remove(n);
            }
        }
    }

    /// Notes the names of activation `a`'s continuation `text` (written in place) reads.
    fn note_reads(&mut self, a: usize, text: &str) {
        let scope = self.kscopes[a].clone();
        for n in super::refs(text).names {
            if scope.contains(&n) && !self.inl.placed_reads.contains(&n) {
                self.inl.placed_reads.push_out(n);
            }
        }
    }

    /// The value `v` of binding slot `sname` (`<cell>sname` for a shared one) of activation
    /// `a`'s continuation, with closures written in place ([`W::props_in_place`]), once.
    pub(super) fn props_in_binding(&mut self, a: usize, sname: &str, v: &str) -> String {
        let key = (a, cat!("<k>", sname));
        if let Some(s) = self.vcache.get(&key) {
            return s.clone();
        }
        let slot = sname.strip_prefix("<cell>").unwrap_or(sname);
        let s = match self.block_of(a, slot) {
            Some(block) => self.props_in_place(a, block, v),
            None => String::from(v),
        };
        self.vcache.insert(key, s.clone());
        s
    }

    /// `text`, a part of activation `a`'s continuation in its block `block`, with the
    /// members of merged scopes that a binding of K declared at or above `block` holds
    /// named by that binding (D3003): `$dump.s2.todos` → `todos`.
    pub(super) fn bindings_in_k(
        &mut self,
        a: usize,
        block: usize,
        text: &str,
        at_site: bool,
    ) -> (String, Vec<String>) {
        if !text.contains("$dump.") {
            return (String::from(text), Vec::new());
        }
        // The bindings whose value is a member: name, value, block.
        let mut held: Vec<(String, String, usize)> = Vec::new();
        for (bi, ds) in self.acts[a].blocks.iter().enumerate().take(block + 1) {
            for d in ds {
                let Some(sname) = &d.sname else { continue };
                if self.acts[a].after_go.contains(sname) {
                    continue;
                }
                let v = match self.acts[a].cells.iter().find(|c| c.0 == *sname) {
                    Some(&(_, sc, v)) => {
                        let var = &self.scope(sc).vars[v];
                        if var.tdz {
                            continue;
                        }
                        var.init.clone().unwrap_or_default()
                    }
                    None => match self.ir.acts[a].slots.iter().find(|s| s.name == *sname) {
                        Some(sl) => sl.value.clone(),
                        None => continue,
                    },
                };
                // (A member of a factory, or a value on `$dump`.)
                let held_value = v.strip_prefix("$dump.").is_some_and(|r| {
                    let mut ps = r.split('.');
                    ps.next().is_some_and(super::is_ident)
                        && ps.next().is_none_or(super::is_ident)
                        && ps.next().is_none()
                });
                // (Not, in a part of the callback, a value it makes there or below: the
                // binding takes it after that part ran. Not a binding the rest of the
                // original does not read: naming it would keep it.)
                let made = !at_site
                    && v.strip_prefix("$dump.").is_some_and(|r| {
                        let n = r.split('.').next().unwrap_or("");
                        self.acts[a].made.iter().any(|m| m.0 == n && m.1 >= block)
                    });
                // (A closure a member holds is kept named, D3003: its binding is
                // usually read.)
                let member = v.matches('.').count() == 2;
                if held_value && !made && (member || self.read_after_site(a, &d.name)) {
                    held.push_out((d.name.clone(), v, bi));
                }
            }
        }
        if held.is_empty() {
            return (String::from(text), Vec::new());
        }
        let b = text.as_bytes();
        let mut named = Vec::new();
        let mut out = String::new();
        let mut at = 0;
        for (s, e) in super::eager_spans(text) {
            let (key, end) = if b.get(e) == Some(&b'.') {
                super::ident_at(text, e + 1)
            } else {
                ("", e)
            };
            let found = held
                .iter()
                .find(|h| !key.is_empty() && h.1 == text[s..end])
                .map(|h| (h, key, end))
                .or_else(|| held.iter().find(|h| h.1 == text[s..e]).map(|h| (h, "", e)));
            let Some(((name, _, bb), key, end)) = found else {
                continue;
            };
            // (A value's own line, `$dump.X = …`, defines it.)
            let after = text[end..].trim_start();
            if after.starts_with('=') && !after.starts_with("==") {
                continue;
            }
            // (One binding of the name: no block between declares it again, and the
            // reference is no property key.)
            if self.declared_between(a, *bb, block, core::slice::from_ref(name))
                || held.iter().filter(|h| h.0 == *name).count() > 1
            {
                continue;
            }
            let fac = String::from(&text[s + 6..e]);
            out.push_str(&text[at..s]);
            out.push_str(name);
            named.push_out(name.clone());
            at = end;
            if key.is_empty() {
                if let Some(n) = self.inl.uses.get_mut(&name_hash(&fac)) {
                    *n -= 1;
                }
                self.repend(&fac);
            } else if let Some(n) = self.inl.key_uses.get_mut(&key_hash(&fac, key)) {
                *n -= 1;
            }
        }
        out.push_str(&text[at..]);
        (out, named)
    }

    /// Whether the original's text after activation `a`'s site names `name` (a binding
    /// K is likely to keep): after the site's construct, or in a loop around it (K copies
    /// the loop again, its operand too); not in the site's operand otherwise.
    fn read_after_site(&self, a: usize, name: &str) -> bool {
        use crate::compiler::NodeKind;
        let act = &self.ir.acts[a];
        let site = act
            .site
            .path
            .last()
            .and_then(|st| act.syntax.nodes.get(st.node as usize))
            .map_or((act.site.pos, act.site.pos), |n| (n.start, n.end));
        let mut ranges = alloc::vec![(site.1, act.ftext.len() as u32)];
        for st in &act.site.path {
            if let Some(n) = act.syntax.nodes.get(st.node as usize)
                && matches!(
                    n.kind,
                    NodeKind::While
                        | NodeKind::DoWhile
                        | NodeKind::For
                        | NodeKind::ForIn
                        | NodeKind::ForOf
                        | NodeKind::ForAwait
                )
            {
                ranges.push_out((n.start, n.end));
            }
        }
        let b = act.ftext.as_bytes();
        act.ftext.match_indices(name).any(|(at, _)| {
            let end = at + name.len();
            ranges
                .iter()
                .any(|r| (r.0 as usize..r.1 as usize).contains(&at))
                && !(at > 0 && (ident_part(b[at - 1]) || b[at - 1] == b'.'))
                && !b.get(end).is_some_and(|&c| ident_part(c))
        })
    }

    /// Makes `$dump.name`'s line a candidate for its one use again, now that a reference
    /// to it was written as a binding's name (D3003).
    fn repend(&mut self, name: &str) {
        if !self.single(name, true) || self.inl.pend.iter().any(|p| p.name == name) {
            return;
        }
        let head = cat!("$dump.", name, " = ");
        for depth in (0..self.inl.stack.len()).rev() {
            let Some(idx) = self.inl.stack[depth]
                .iter()
                .position(|l| l.starts_with(&head))
            else {
                continue;
            };
            let Some(rhs) = def_rhs(&self.inl.stack[depth][idx]).map(String::from) else {
                return;
            };
            let effect = !pure(&rhs);
            self.inl.pend_names.insert(String::from(name));
            self.inl.pend.push_out(Pend {
                name: String::from(name),
                rhs,
                depth,
                idx,
                effect,
                bound: Vec::new(),
            });
            return;
        }
    }

    /// The top-level lines `$dump.X = value;` whose value nothing reads in the end (a
    /// parameter liveness left out held it, a reference became a binding's name): the
    /// value's effect alone, or nothing (D3003). Fails where a value written in place is
    /// still read by name (a bug). One count, kept as lines go (0215).
    pub(super) fn drop_unread(
        &self,
        body: &mut [String],
        handles: &[(String, String)],
    ) -> Result<Counts, String> {
        // (By the names' hashes, 0215: integer keys. A collision only adds uses, which
        // keeps a line; a name in place seen where it is not only asks `scan` again.)
        let mut done = Counts::with_capacity(self.inl.done.len());
        for n in &self.inl.done {
            done.add(name_hash(n));
        }
        // The definitions by name (the line's index), and the lines to look at.
        let mut defs = Counts::with_capacity(body.len());
        for (i, l) in body.iter().enumerate() {
            if let Some(rest) = l.strip_prefix("$dump.") {
                let (name, _) = super::ident_at(rest, 0);
                if !name.is_empty() && rest[name.len()..].starts_with(" = ") {
                    defs.set(name_hash(name), i as u32);
                }
            }
        }
        // Every name's uses.
        let mut uses = Counts::with_capacity(body.len());
        let fns = self
            .items
            .iter()
            .filter(|it| it.kind == IK::FnDecl)
            .map(|it| it.text.as_str());
        for t in body
            .iter()
            .map(String::as_str)
            .chain(fns)
            .chain(handles.iter().map(|h| h.1.as_str()))
        {
            let mut seen_done = false;
            for_dump_refs(t, |name| {
                let h = name_hash(name);
                seen_done |= done.contains_key(&h);
                uses.add(h);
            });
            // (Checked by `scan`, which skips strings, only where it may be so.)
            if seen_done
                && let Some(name) = scan_spans(t)
                    .into_iter()
                    .map(|(s, e)| &t[s + 6..e])
                    .find(|n| self.inl.done.contains(*n))
            {
                return Err(cat!(
                    "plain dumps: $dump.",
                    name,
                    " is written in place and read again (a bug)"
                ));
            }
        }
        // (In the hashes' order, as a sorted map gave them: the lines dropped do not
        // depend on it, the statements' order is the body's.)
        let mut work: Vec<u64> = defs
            .keys()
            .filter(|h| uses.get(h).copied() == Some(1))
            .collect();
        crate::sort::sort_by(&mut work, |a, b| a < b);
        while let Some(h) = work.pop() {
            let Some(&i) = defs.get(&h) else { continue };
            let i = i as usize;
            if uses.get(&h).copied() != Some(1) {
                continue;
            }
            let Some(rhs) = def_rhs(&body[i]) else {
                continue;
            };
            // (A factory makes its scope's closures and nothing else.)
            let drop = shape(rhs) || rhs.starts_with("(() => {");
            let new = if drop { String::new() } else { statement(rhs) };
            // What the line read is read once less.
            let old = core::mem::replace(&mut body[i], new);
            defs.remove(&h);
            if let Some(c) = uses.get_mut(&h) {
                *c -= 1;
            }
            if drop {
                let mut first = true;
                for_dump_refs(&old, |n| {
                    // (Its own name first.)
                    if core::mem::take(&mut first) {
                        return;
                    }
                    let k = name_hash(n);
                    if let Some(c) = uses.get_mut(&k) {
                        *c -= 1;
                        if *c == 1 && defs.contains_key(&k) {
                            work.push_out(k);
                        }
                    }
                });
            }
        }
        Ok(uses)
    }

    /// Starts a list of lines (a callback's, or the top level).
    pub(super) fn open_list(&mut self) {
        self.inl.stack.push(Vec::new());
        self.inl.effects.push(Vec::new());
        self.inl.pull.push(false);
    }

    /// Starts the list of a callback's part `part`: host calls deferred outside may be
    /// written in it where every one waiting is read in it (so none is left to be written
    /// outside, before the part runs, though it comes after one written in it).
    pub(super) fn open_part(&mut self, part: &[super::Plan]) {
        let depth = self.inl.stack.len();
        let all = !self.inl.hooks.is_empty()
            && self
                .inl
                .hooks
                .iter()
                .all(|h| h.0 < depth && self.part_reads(part, &h.1));
        self.open_list();
        if let Some(p) = self.inl.pull.last_mut() {
            *p = all;
        }
    }

    /// Whether an entry of `part` reads `$dump.name`.
    fn part_reads(&self, part: &[super::Plan], name: &str) -> bool {
        let r = cat!("$dump.", name);
        let has = |t: &str| {
            super::dollar_indices(t, r.as_str()).any(|at| {
                !t.as_bytes()
                    .get(at + r.len())
                    .is_some_and(|&c| ident_part(c))
            })
        };
        part.iter().any(|e| match e {
            super::Plan::Item(i) => has(&self.items[*i].text),
            super::Plan::Call(a, cb) => {
                let act = &self.ir.acts[*a];
                act.awaiting.as_deref().is_some_and(has)
                    || has(&act.fexpr)
                    || has(&act.this)
                    || act.slots.iter().any(|s| has(&s.value))
                    || act.queue.iter().any(|q| has(&q.1))
                    || cb.as_deref().is_some_and(|cb| self.part_reads(cb, name))
            }
        })
    }

    /// Defers line `text` of reopened host call `name` (value `rhs`, read once) until the
    /// line that reads it (D3004).
    pub(super) fn defer_hook(&mut self, name: String, text: String, rhs: String) {
        let depth = self.inl.stack.len() - 1;
        self.inl.hook_names.insert(name.clone());
        self.inl.hooks.push_out((depth, name, text, rhs));
    }

    /// Writes the deferred host calls `text` reads, and those before them (their order is
    /// the host's).
    pub(super) fn flush_hooks(&mut self, text: &str) {
        if self.inl.hooks.is_empty() {
            return;
        }
        if !super::has_dollar(text, "$dump.") {
            return;
        }
        // (One pass over the text's `$dump.` names, 0215.)
        let mut last = None;
        for at in super::dollar_indices(text, "$dump.") {
            let (n, _) = super::ident_at(text, at + 6);
            if self.inl.hook_names.contains(n)
                && let Some(i) = self.inl.hooks.iter().position(|h| h.1 == n)
            {
                last = last.max(Some(i));
            }
        }
        if let Some(i) = last {
            self.flush_upto(i);
        }
    }

    /// Writes every deferred host call: a line with a host call of its own comes after
    /// them.
    pub(super) fn flush_all_hooks(&mut self) {
        if let Some(n) = self.inl.hooks.len().checked_sub(1) {
            self.flush_upto(n);
        }
    }

    /// (Each into its own list: a callback's lines run after the lines of the list
    /// around it that follow it, so a host call written there would come after the
    /// later ones written outside.)
    fn flush_upto(&mut self, i: usize) {
        let hooks: Vec<_> = self.inl.hooks.drain(..=i).collect();
        for h in &hooks {
            self.inl.hook_names.remove(&h.1);
        }
        let here = self.inl.stack.len() - 1;
        let pull = self.inl.pull.last() == Some(&true);
        for (depth, name, text, rhs) in hooks {
            let at = if pull { here } else { depth };
            self.put_line_at(at, text, true, Some((name, rhs, true)), Vec::new());
        }
    }

    /// Ends the current list: its lines (those written in place left out).
    pub(super) fn close_list(&mut self) -> Vec<String> {
        let depth = self.inl.stack.len() - 1;
        // (A deferred host call of this list, and those before it, are written in it.)
        if let Some(i) = self.inl.hooks.iter().rposition(|h| h.0 == depth) {
            self.flush_upto(i);
        }
        let names = &mut self.inl.pend_names;
        self.inl.pend.retain(|p| {
            if p.depth == depth {
                names.remove(&p.name);
            }
            p.depth != depth
        });
        self.inl.effects.pop();
        self.inl.pull.pop();
        let lines = self.inl.stack.pop().unwrap_or_default();
        lines.into_iter().filter(|l| !l.is_empty()).collect()
    }

    /// Writes line `text` of the current list; `effect`: whether it does more than make a
    /// value; `def`: the value it names and its text, a candidate for its one use.
    pub(super) fn put_line(
        &mut self,
        text: String,
        effect: bool,
        def: Option<(String, String, bool)>,
    ) {
        self.put_line_binding(text, effect, def, Vec::new());
    }

    /// [`W::put_line`] for a value whose text binds names `bound` itself.
    pub(super) fn put_line_binding(
        &mut self,
        text: String,
        effect: bool,
        def: Option<(String, String, bool)>,
        bound: Vec<String>,
    ) {
        // (The deferred host calls a line reads are written before it by `inline_with`,
        // which every line written from text goes through; `flush_hooks` for the others.)
        let depth = self.inl.stack.len() - 1;
        self.put_line_at(depth, text, effect, def, bound);
    }

    /// [`W::put_line_binding`] into the list at `depth`.
    fn put_line_at(
        &mut self,
        depth: usize,
        text: String,
        effect: bool,
        def: Option<(String, String, bool)>,
        bound: Vec<String>,
    ) {
        let idx = self.inl.stack[depth].len();
        self.inl.stack[depth].push_out(text);
        if effect {
            self.inl.effects[depth].push_out(idx);
        }
        if let Some((name, rhs, eff)) = def {
            self.inl.pend_names.insert(name.clone());
            self.inl.pend.push_out(Pend {
                name,
                rhs,
                depth,
                idx,
                effect: eff,
                bound,
            });
        }
    }

    /// `text` with the references to values waiting for their one use written in place
    /// where that keeps the order: `first` is where in `text` the line's first evaluation
    /// is (where effectful values may go, [`first_run`]), if anywhere.
    ///
    /// `in_k`: `text` is a value of a continuation's text, whose bindings could hide the
    /// names a value reads: only values that read none but builtins go there.
    pub(super) fn inline_refs(&mut self, text: &str, first: Option<usize>, in_k: bool) -> String {
        if in_k {
            self.inline_with(text, first, Reads::Only(PURE_NAMES))
        } else {
            self.inline_with(text, first, Reads::Any)
        }
    }

    /// A continuation's site operand `text`, its first evaluation at `first` (if so), where
    /// K may bind the names `scope`: a value may go there that reads, besides builtins and
    /// what it binds itself, no name of `scope` (and not `this`, `arguments`, `super`, or
    /// K's own `$djs_` helpers).
    pub(super) fn inline_site(
        &mut self,
        text: &str,
        first: Option<usize>,
        scope: &[String],
    ) -> String {
        let bound = |n: &str| scope.iter().any(|x| x == n);
        self.inline_with(text, first, Reads::Scope(&bound))
    }

    fn inline_with(&mut self, text: &str, first: Option<usize>, reads: Reads) -> String {
        if self.inl.pend.is_empty() && self.inl.hooks.is_empty() {
            return String::from(text);
        }
        // One pass over the names: deferred host calls to write first (D3004), pending
        // values (none: no scan, 0215).
        let mut last_hook = None;
        let mut named = false;
        for at in super::dollar_indices(text, "$dump.") {
            let (n, _) = super::ident_at(text, at + 6);
            named |= self.inl.pend_names.contains(n);
            if self.inl.hook_names.contains(n)
                && let Some(i) = self.inl.hooks.iter().position(|h| h.1 == n)
            {
                last_hook = last_hook.max(Some(i));
            }
        }
        if let Some(i) = last_hook {
            self.flush_upto(i);
            named = true;
        }
        if !named {
            return String::from(text);
        }
        let spans = scan_spans(text);
        let run = first.map_or_else(Vec::new, |f| first_run(text, f, &spans));
        let depth = self.inl.stack.len() - 1;
        // Per reference: the pending value written there, if one may be.
        let mut take: Vec<Option<usize>> = Vec::new();
        for &(s, e) in &spans {
            // (`$dump.X.key`: a factory's member, never a candidate.)
            let name = &text[s + 6..e];
            if !self.inl.pend_names.contains(name) {
                take.push_out(None);
                continue;
            }
            let Some(p) = self.inl.pend.iter().rposition(|p| p.name == name) else {
                take.push_out(None);
                continue;
            };
            let pd = &self.inl.pend[p];
            let reads_ok = match reads {
                Reads::Any => true,
                Reads::Only(only) => super::refs(&pd.rhs)
                    .names
                    .iter()
                    .all(|n| only.contains(&n.as_str())),
                Reads::Scope(scope) => {
                    !pd.rhs.contains("new.target")
                        && super::refs(&pd.rhs).names.iter().all(|n| {
                            !matches!(n.as_str(), "this" | "arguments" | "super")
                                && !n.starts_with("$djs_")
                                && (PURE_NAMES.contains(&n.as_str())
                                    || pd.bound.contains(n)
                                    || !scope(n)
                                    || only_keys(&pd.rhs, n))
                        })
                }
            };
            let ok = reads_ok
                && if pd.effect {
                    // (In the run of first evaluations, of the list being written; the
                    // run's order is checked below.)
                    run.contains(&s) && pd.depth == depth
                } else {
                    // A function without a name where one would be inferred keeps its
                    // line.
                    let prev = text[..s].trim_end();
                    let infers =
                        (prev.ends_with('=') && !prev.ends_with("==") && !prev.ends_with("=>"))
                            || prev.ends_with(':');
                    !(infers && anonymous(&pd.rhs))
                };
            take.push_out(ok.then_some(p));
        }
        // The effectful values of the run go there all or none: each must be the next of
        // the list's effects, the last one its last (nothing effectful comes between
        // their lines and this one).
        let mut eff: Vec<usize> = Vec::new();
        let mut all = true;
        for (k, &(s, e)) in spans.iter().enumerate() {
            if !run.contains(&s) {
                continue;
            }
            let name = &text[s + 6..e];
            if let Some(pd) = self.inl.pend.iter().find(|p| p.name == name && p.effect) {
                all &= take[k].is_some();
                eff.push_out(pd.idx);
            }
        }
        let tail: Vec<usize> = match eff.first() {
            Some(&f) => self.inl.effects[depth]
                .iter()
                .copied()
                .filter(|&i| i >= f)
                .collect(),
            None => Vec::new(),
        };
        if !all || tail != eff {
            for t in &mut take {
                if t.is_some_and(|p| self.inl.pend[p].effect) {
                    *t = None;
                }
            }
        }
        let names: Vec<Option<String>> = take
            .iter()
            .map(|t| t.map(|p| self.inl.pend[p].name.clone()))
            .collect();
        let mut out = String::new();
        let mut at = 0;
        for (k, &(s, e)) in spans.iter().enumerate() {
            let Some(name) = &names[k] else {
                continue;
            };
            let Some(p) = self.inl.pend.iter().position(|p| p.name == *name) else {
                continue;
            };
            let p = self.inl.pend.remove(p);
            self.inl.stack[p.depth][p.idx] = String::new();
            self.inl.effects[p.depth].retain(|&i| i != p.idx);
            out.push_str(&text[at..s]);
            out.push_str(&operand_at(text, s, e, &p.rhs));
            at = e;
            self.inl.pend_names.remove(&p.name);
            self.inl.done.insert(p.name);
        }
        out.push_str(&text[at..]);
        out
    }

    /// Activation `a`'s site operand where it is a value the last part of its callback
    /// (`b`, the list being written) makes for it alone (D3002): K runs that part right
    /// before its site, and where nothing effectful comes between (no code of K's blocks
    /// below `b`), the value is made at the site instead, as its first evaluation.
    pub(super) fn site_from_callback(&mut self, a: usize, b: usize) {
        let ir = self.ir;
        let act = &ir.acts[a];
        let Some(aw) = act.awaiting.as_deref() else {
            return;
        };
        let Some(name) = aw.strip_prefix("$dump.").filter(|n| super::is_ident(n)) else {
            return;
        };
        // (As `awaiting` asks for it: K called on its own line, its site its first
        // evaluation after its prefix.)
        if act.kind != crate::interp::run::suspend::ASYNC
            || self.acts[a].home.is_some()
            || self
                .placed
                .iter()
                .any(|x| matches!(x.0, super::Pl::Block(a2, i) if a2 == a && i > b))
        {
            return;
        }
        if !self.inl.pend.iter().any(|p| p.name == name) {
            return;
        }
        let scope = self.kscopes[a].clone();
        let bound = |n: &str| scope.contains(n);
        let v = self.inline_with(aw, Some(0), Reads::Scope(&bound));
        if v != aw {
            // (K's deepest block holds the site.)
            let last = self.acts[a].blocks.len().saturating_sub(1);
            let (v, mut reads) = self.bindings_in_k(a, last, &v, true);
            self.inl.placed_reads.clear();
            let v = self.props_in_place(a, last, &v);
            reads.append(&mut self.inl.placed_reads);
            self.acts[a].site_reads = reads;
            self.acts[a].site_val = Some(v);
        }
    }

    /// Activation `a`'s site operand worked out before its continuation is written, where
    /// K is async, called on its own line and does nothing before its site (no code of the
    /// dump's in its blocks, no callback): the operand is then the line's first
    /// evaluation (D3001), and K's bindings it names are known before K's arguments are
    /// (D3003: a function both K's parameter and named in the operand is written once).
    pub(super) fn site_first(&mut self, a: usize) {
        let ir = self.ir;
        let act = &ir.acts[a];
        let Some(aw) = act.awaiting.as_deref() else {
            return;
        };
        // (Not once its text is written: a factory's continuation may be, with its
        // factory.)
        if self.acts[a].site_val.is_some()
            || self.acts[a].k.is_some()
            || act.kind != crate::interp::run::suspend::ASYNC
            || !self.acts[a].go.iter().all(String::is_empty)
            || self
                .placed
                .iter()
                .any(|x| matches!(x.0, super::Pl::Block(a2, _) if a2 == a))
            || !aw.contains("$dump.")
        {
            return;
        }
        let scope = self.kscopes[a].clone();
        let bound = |n: &str| scope.contains(n);
        let v = self.inline_with(aw, Some(0), Reads::Scope(&bound));
        let last = self.acts[a].blocks.len().saturating_sub(1);
        let (v, mut reads) = self.bindings_in_k(a, last, &v, true);
        self.inl.placed_reads.clear();
        let v = self.props_in_place(a, last, &v);
        reads.append(&mut self.inl.placed_reads);
        if v != aw {
            self.acts[a].site_reads = reads;
            self.acts[a].site_val = Some(v);
        }
    }

    /// The names activation `a`'s continuation may bind anywhere: any declaration of its
    /// function, any slot's name (and its own `$djs_` helpers).
    pub(super) fn k_scope(&self, a: usize) -> &BTreeSet<String> {
        &self.kscopes[a]
    }

    /// Whether the lines of a part of activation `a`'s callback may be written in its
    /// continuation where it calls them (D3002): they read no name K may bind, and nothing
    /// that is K's own there (`this`, `arguments`, `super`, `new.target`).
    pub(super) fn callback_in_k(&self, a: usize, lines: &[String]) -> bool {
        let scope = self.k_scope(a);
        lines.iter().all(|l| {
            !l.contains("new.target")
                && super::refs(l).names.iter().all(|n| {
                    !matches!(n.as_str(), "this" | "arguments" | "super")
                        && !n.starts_with("$djs_")
                        && (!scope.contains(n) || only_keys(l, n))
                })
        })
    }

    /// Whether item `kind`'s line does more than make a value.
    pub(super) fn effectful(kind: &IK, text: &str) -> bool {
        match kind {
            IK::Def(_) => !shape(def_rhs(text).unwrap_or("")),
            IK::Decl(_) => {
                // (`const x = value;`.)
                let rhs = text
                    .split_once(" = ")
                    .map_or("", |x| x.1)
                    .trim_end_matches(';');
                !shape(rhs)
            }
            IK::Factory(_) | IK::FnDecl => false,
            IK::Stmt | IK::Act(_) => true,
        }
    }

    /// Slot `name`'s value `v` of activation `a`, with the values that wait for their use
    /// written in place (once per slot: K's generator may ask again).
    pub(super) fn inlined_slot(&mut self, a: usize, name: &str, v: &str) -> String {
        let key = (a, String::from(name));
        if let Some(s) = self.vcache.get(&key) {
            return s.clone();
        }
        // (A value that reads a name K binds would be read before the continuation,
        // `pre`.)
        let scope = self.kscopes[a].clone();
        let bound = |n: &str| scope.contains(n);
        let s = self.inline_with(v, None, Reads::Scope(&bound));
        self.vcache.insert(key, s.clone());
        s
    }
}

/// Counts by name hash ([`name_hash`]): an open-addressing table, not a tree (0215: a
/// data-heavy dump counts thousands of names, a lookup per reference). Hash 0 counts as 1
/// (the table's empty key): a collision only adds uses, as any other.
#[derive(Default)]
pub(super) struct Counts(super::super::Index);

impl Counts {
    fn key(h: u64) -> u64 {
        h.max(1)
    }

    /// Counts for about `n` names.
    pub(super) fn with_capacity(n: usize) -> Self {
        Counts(super::super::Index::with_capacity(n))
    }

    pub(super) fn get(&self, h: &u64) -> Option<&u32> {
        self.0.get_ref(Self::key(*h)).filter(|&&c| c != u32::MAX)
    }

    pub(super) fn get_mut(&mut self, h: &u64) -> Option<&mut u32> {
        self.0.get_mut(Self::key(*h)).filter(|c| **c != u32::MAX)
    }

    pub(super) fn contains_key(&self, h: &u64) -> bool {
        self.get(h).is_some()
    }

    /// Whether `h` is counted more than zero times.
    pub(super) fn contains(&self, h: &u64) -> bool {
        self.get(h).is_some_and(|&c| c > 0)
    }

    pub(super) fn add(&mut self, h: u64) {
        match self.get_mut(&h) {
            Some(c) => *c += 1,
            None => self.0.insert(Self::key(h), 1),
        }
    }

    pub(super) fn set(&mut self, h: u64, v: u32) {
        self.0.insert(Self::key(h), v);
    }

    /// Removes `h` (its value: `u32::MAX`, which no count reaches).
    pub(super) fn remove(&mut self, h: &u64) {
        if let Some(c) = self.get_mut(h) {
            *c = u32::MAX;
        }
    }

    /// The hashes present (not removed), in no particular order.
    pub(super) fn keys(&self) -> impl Iterator<Item = u64> + '_ {
        self.0.entries().filter(|e| e.1 != u32::MAX).map(|e| e.0)
    }
}

/// A set of names with their hashes counted beside it: a lookup of a name not in it (most
/// references, whose values are not waiting) is one probe, not a search of the tree, 0215.
/// (One count per name in the set: exact, a hash two of them share only goes to the tree.)
#[derive(Default)]
pub(super) struct NameSet {
    set: BTreeSet<String>,
    hashes: Counts,
}

impl NameSet {
    pub(super) fn insert(&mut self, name: String) -> bool {
        let h = name_hash(&name);
        let new = self.set.insert(name);
        if new {
            self.hashes.add(h);
        }
        new
    }

    pub(super) fn remove(&mut self, name: &str) -> bool {
        let gone = self.set.remove(name);
        if gone && let Some(c) = self.hashes.get_mut(&name_hash(name)) {
            *c -= 1;
        }
        gone
    }

    pub(super) fn contains(&self, name: &str) -> bool {
        self.hashes.contains(&name_hash(name)) && self.set.contains(name)
    }
}

/// A name's hash (FNV-1a), a map key cheaper than the name.
pub(super) fn name_hash(n: &str) -> u64 {
    n.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Calls `f` with the name of each `$dump.X` reference of `text`, by substring (as
/// [`dump_refs`]).
pub(super) fn for_dump_refs<'t>(text: &'t str, mut f: impl FnMut(&'t str)) {
    let b = text.as_bytes();
    for at in super::dollar_indices(text, "$dump.") {
        if at > 0 && (ident_part(b[at - 1]) || b[at - 1] == b'.') {
            continue;
        }
        let (n, _) = super::ident_at(text, at + 6);
        if !n.is_empty() {
            f(n);
        }
    }
}

/// The `$dump.X` references of `text` by substring (`[start, end)`, the name's end): every
/// one [`scan_spans`] finds, and those in strings and comments too.
pub(super) fn dump_refs(text: &str) -> Vec<(usize, usize)> {
    dump_ref_spans(text).collect()
}

/// [`dump_refs`] one by one.
fn dump_ref_spans(text: &str) -> impl Iterator<Item = (usize, usize)> + '_ {
    let b = text.as_bytes();
    super::dollar_indices(text, "$dump.")
        .filter(move |&at| !(at > 0 && (ident_part(b[at - 1]) || b[at - 1] == b'.')))
        .filter_map(move |at| {
            let (n, e) = super::ident_at(text, at + 6);
            (!n.is_empty()).then_some((at, e))
        })
}

/// A factory member's hash (`$dump.F.key`), for [`Inline::key_uses`]: a collision only
/// adds uses, as with names.
pub(super) fn key_hash(fac: &str, key: &str) -> u64 {
    name_hash(fac).rotate_left(17) ^ name_hash(key) ^ 0x2545_F491_4F6C_DD1D
}

/// Whether `text` names `name` only as an object literal's key (`{ path: "b.md" }`): the
/// references `refs` finds err on the side of keys.
pub(super) fn only_keys(text: &str, name: &str) -> bool {
    let b = text.as_bytes();
    let mut any = false;
    for (at, _) in text.match_indices(name) {
        let end = at + name.len();
        if at > 0 && ident_part(b[at - 1]) || b.get(end).is_some_and(|&c| ident_part(c)) {
            continue;
        }
        any = true;
        let before = text[..at].trim_end();
        let after = text[end..].trim_start();
        let key = (before.ends_with('{') || before.ends_with(','))
            && after.starts_with(':')
            && !after.starts_with("::");
        if !key {
            return false;
        }
    }
    any
}

/// Value text `rhs` as an expression statement (in parentheses where it would start
/// another statement).
pub(super) fn statement(rhs: &str) -> String {
    let t = rhs.trim();
    let other = ["{", "function", "class", "async", "let"]
        .iter()
        .any(|k| t.starts_with(k));
    if other {
        cat!("(", t, ");")
    } else {
        cat!(t, ";")
    }
}

/// The value of a `$dump.X = value;` line.
pub(super) fn def_rhs(text: &str) -> Option<&str> {
    text.strip_prefix("$dump.")?;
    let (_, v) = super::split_def(text)?;
    Some(v.strip_suffix(';').unwrap_or(v))
}

/// Where in a line `$dump.X = value;` or `void value;` its value starts (its first
/// evaluation, [`first_run`]).
pub(super) fn first_eval(text: &str) -> Option<usize> {
    let v = match text.strip_prefix("void ") {
        Some(v) => v,
        None => {
            text.strip_prefix("$dump.")?;
            super::split_def(text)?.1
        }
    };
    Some(text.len() - v.len())
}

/// The references of `text` its evaluation from `first` evaluates first, one after another
/// with nothing effectful between (`spans`: its references): the value itself
/// (`$dump.a3.then(…)`: its receiver), or a builtin call's or an array's leading operands
/// (`Promise.all([$dump.p4, $dump.p5])`) with literals among them. Effectful values may
/// go there, in this order.
pub(super) fn first_run(text: &str, first: usize, spans: &[(usize, usize)]) -> Vec<usize> {
    let b = text.as_bytes();
    let mut i = first;
    let ws = |i: &mut usize| {
        while b.get(*i).is_some_and(u8::is_ascii_whitespace) {
            *i += 1;
        }
    };
    // A builtin's call (not the dump's own): its callee read, its arguments in order.
    let n = b[i..]
        .iter()
        .take_while(|&&c| ident_part(c) || c == b'.')
        .count();
    if n > 0
        && b.get(i + n) == Some(&b'(')
        && ident_start(b[i])
        && !text[i..].starts_with("$dump")
        && !text[i..].starts_with("$djs")
        && text[i..i + n].split('.').all(|p| !p.is_empty())
    {
        i += n + 1;
    }
    let mut out = Vec::new();
    loop {
        // Literals and punctuation evaluate nothing.
        loop {
            ws(&mut i);
            match b.get(i) {
                Some(b'[' | b',') => i += 1,
                Some(b'"') => {
                    let mut j = i + 1;
                    while j < b.len() && b[j] != b'"' {
                        j += if b[j] == b'\\' { 2 } else { 1 };
                    }
                    i = j + 1;
                }
                Some(c)
                    if c.is_ascii_digit()
                        || *c == b'-' && b.get(i + 1).is_some_and(u8::is_ascii_digit) =>
                {
                    i += 1;
                    while b
                        .get(i)
                        .is_some_and(|&c| c.is_ascii_alphanumeric() || c == b'.' || c == b'_')
                    {
                        i += 1;
                    }
                }
                _ => {
                    let word = ["true", "false", "null", "undefined"].iter().find(|w| {
                        text[i..].starts_with(**w)
                            && !b.get(i + w.len()).is_some_and(|&c| ident_part(c))
                    });
                    match word {
                        Some(w) => i += w.len(),
                        None => break,
                    }
                }
            }
        }
        let Some(&(s, e)) = spans.iter().find(|x| x.0 == i) else {
            break;
        };
        out.push_out(s);
        i = e;
        // (Only a whole operand lets the next one follow.)
        ws(&mut i);
        if b.get(i) != Some(&b',') {
            break;
        }
    }
    out
}
