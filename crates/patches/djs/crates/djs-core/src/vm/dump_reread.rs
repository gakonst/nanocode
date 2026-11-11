//! Operands a plain continuation evaluates again instead of keeping (D2836, 0207).
//!
//! When a statement suspends inside a call's arguments, the callee, `this` and the
//! arguments before the site were already evaluated: the frame holds them as
//! temporaries, which a continuation keeps as constants (`$djs_R($djs_t0, $djs_t1, [...])`).
//! Plain dumps are written only with frozen builtins, so an operand that is pure to
//! evaluate again and gives the saved value on the dumped state can be written as the
//! original text (`results.push(await p)`):
//!
//! - a literal (a string without escapes, a decimal number, `true`, `false`, `null`);
//! - an identifier whose binding (a frame binding of that name, unique at the site; else a
//!   variable the function captured; else a global's data property) holds the saved
//!   value now;
//! - a callee `b.name` whose base `b` is such an identifier holding the saved `this`, and
//!   whose property `name` is found on a frozen object (a builtin): the receiver has no
//!   own property of that name (or is frozen itself), and every object on the way is
//!   frozen, so nothing the dump restores later can change the lookup.
//!
//! Everything else keeps its temporary: user functions read through user objects, own
//! properties of user objects, getters, proxies.

use alloc::string::String;
use alloc::vec::Vec;

use super::*;
use crate::compiler::{Holds, NamedSlot, NodeKind, Role, Syntax};

/// An operand's text that may be read again.
enum Read<'t> {
    Ident(&'t str),
    /// `base.name`.
    Member(&'t str, &'t str),
}

fn parse(text: &str) -> Option<Read<'_>> {
    let t = text.trim();
    if is_ident(t) {
        return Some(Read::Ident(t));
    }
    let (b, k) = t.split_once('.')?;
    (is_ident(b) && is_ident(k)).then_some(Read::Member(b, k))
}

/// Whether `text` is a literal: a string without escapes, a decimal number, `true`,
/// `false`, `null`.
fn is_literal(text: &str) -> bool {
    let t = text.trim();
    let quoted = |q: char| {
        t.len() >= 2
            && t.starts_with(q)
            && t.ends_with(q)
            && !t[1..t.len() - 1].contains([q, '\\', '\n', '\r'])
    };
    matches!(t, "true" | "false" | "null")
        || quoted('"')
        || quoted('\'')
        || (t.bytes().all(|b| b.is_ascii_digit() || b == b'.') && t.parse::<f64>().is_ok())
}

/// Strict equality of a saved operand and a value read now.
fn same(h: &Heap, a: Value, b: Value) -> bool {
    if a == b {
        return true;
    }
    match (a.as_string(), b.as_string()) {
        (Some(x), Some(y)) => text_of(h, x) == text_of(h, y),
        _ => matches!((a.as_number(), b.as_number()), (Some(x), Some(y)) if x == y),
    }
}

impl Dumper<'_> {
    /// The temporaries of activation frame `named` (values by `val`) that its
    /// continuation may evaluate again from `ftext` (D2836): see the module comment.
    pub(super) fn rereads(
        &self,
        f: Option<Obj>,
        syntax: &Syntax,
        ftext: &str,
        named: &[NamedSlot],
        val: &dyn Fn(&NamedSlot) -> Value,
        path: &[crate::compiler::Step],
    ) -> Vec<String> {
        let slot = |name: &str| named.iter().find(|x| x.name == name).map(val);
        let text = |a: u32, b: u32| ftext.get(a as usize..b as usize).unwrap_or("");
        let mut out = Vec::new();
        for (ni, n) in syntax.nodes.iter().enumerate() {
            // The part of the node that holds the site, where it is on the site's path.
            let held = path
                .iter()
                .find(|s| s.node as usize == ni)
                .and_then(|s| s.part)
                .map(usize::from)
                .and_then(|k| n.parts.get(k).map(|p| (k, p.role)));
            // An object literal with the site in a property's value: written again as
            // the original's text where the properties before it read again what the
            // object made so far holds (nothing sees the object before it is complete).
            if n.kind == NodeKind::Object
                && let Some(ov) = slot(&cat!("object.literal@", n.start))
                && let Some((k, _)) = held
                && self.object_rereads(f, named, val, ftext, n, ov, k)
            {
                out.push(cat!("object.literal@", n.start));
            }
            // A loop over a saved iterator: its `next` read again from the iterator, and
            // whether it has no `return` (nothing closes it: the rest of the current
            // iteration may run outside the loop, D2908).
            if n.kind == NodeKind::ForIn {
                out.push(cat!("for-in.closeless@", n.start));
            }
            if n.kind == NodeKind::ForOf
                && let Some(itv) = slot(&cat!("for-of.iterator@", n.start))
            {
                let next = cat!("for-of.next@", n.start);
                if slot(&next).is_some_and(|nv| self.frozen_lookup(itv, "next", nv)) {
                    out.push(next);
                    // An iterator that is its own iterable (a builtin's): the loop goes on
                    // over it as it is.
                    let own = intrinsic(self.h, Intrinsic::IteratorProto)
                        .as_object()
                        .and_then(|p| self.h.get_own_property(p, crate::heap::atoms::SYM_ITERATOR))
                        .and_then(|p| p.value());
                    if own.is_some_and(|f| {
                        self.frozen_get(itv, crate::heap::atoms::SYM_ITERATOR) == Some(f)
                    }) {
                        out.push(cat!("for-of.direct@", n.start));
                    }
                }
                if self.frozen_lookup(itv, "return", Value::UNDEFINED) {
                    out.push(cat!("for-of.closeless@", n.start));
                }
            }
            // A binary operator's left operand; a template's substitution before the site
            // (its string: a name that holds that string now).
            // (A substitution's saved string: a string, or a number whose string it is.)
            let saved_text = |v: Value| v.as_string().map(|s| text_of(self.h, s));
            let as_part = |now: Value, saved: Value| match (now.as_string(), now.as_number()) {
                (Some(_), _) => same(self.h, now, saved),
                (None, Some(n)) => {
                    saved_text(saved).is_some_and(|t| crate::num::to_string(n).as_str() == t)
                }
                _ => false,
            };
            let simple = |t: &str, saved: Value, string: bool| {
                (!string && is_literal(t))
                    || matches!(parse(t), Some(Read::Ident(id)) if self
                        .read_name(f, named, val, id)
                        .is_some_and(|now| if string { as_part(now, saved) } else { same(self.h, now, saved) }))
            };
            // (Nested operators start where their left operands do: the slot is that of
            // the one whose right operand holds the site.)
            let right_holds = || held.is_some_and(|h| h.1 == Role::Right);
            if n.kind == NodeKind::Binary
                && let Some(v) = slot(&cat!("binary.left@", n.start))
                && right_holds()
                && let Some(l) = n.parts.iter().find(|p| p.role == Role::Left)
                && simple(text(l.start, l.end), v, false)
            {
                out.push(cat!("binary.left@", n.start));
            }
            // An assignment with the site on its right: its target read again (a name; a
            // member whose object is a name holding the saved object, its key a literal
            // or a name holding the saved primitive), and a compound one's old value where
            // the target holds it now (D2914).
            if n.kind == NodeKind::Assign && right_holds() {
                self.assign_rereads(f, named, val, ftext, n, &slot, &mut out);
            }
            if n.kind == NodeKind::Template {
                for p in n.parts.iter().filter(|p| p.role == Role::Subst) {
                    let name = cat!("template.part@", p.start);
                    if let Some(v) = slot(&name)
                        && simple(text(p.start, p.end), v, true)
                    {
                        out.push(name);
                    }
                }
            }
            let call = matches!(n.kind, NodeKind::Call { optional: false });
            if !call && n.kind != NodeKind::New {
                continue;
            }
            // The callee and `this` of a call.
            let callee = cat!("call.callee@", n.start);
            let this = cat!("call.this@", n.start);
            // (Of the call whose arguments hold the site: a call's callee starts where it
            // does, `f(a)(await b)`.)
            let args_hold = held.is_some_and(|h| matches!(h.1, Role::Arg | Role::Spread));
            if call
                && args_hold
                && let (Some(cv), Some(tv)) = (slot(&callee), slot(&this))
                && let Some(c) = n.parts.iter().find(|p| p.role == Role::Callee)
            {
                let ok = match parse(text(c.start, c.end)) {
                    Some(Read::Ident(name)) => {
                        tv.is_undefined()
                            && self
                                .read_name(f, named, val, name)
                                .is_some_and(|v| same(self.h, v, cv))
                    }
                    Some(Read::Member(b, k)) => {
                        self.read_name(f, named, val, b)
                            .is_some_and(|v| same(self.h, v, tv))
                            && self.frozen_lookup(tv, k, cv)
                    }
                    None => false,
                };
                if ok {
                    out.push(callee);
                    out.push(this);
                }
            }
            // Arguments evaluated before the site.
            let construct = if call { "call.arg@" } else { "new.arg@" };
            for p in n.parts.iter().filter(|p| p.role == Role::Arg) {
                let name = cat!(construct, p.start);
                let Some(v) = slot(&name) else { continue };
                // (A literal gives its value again.)
                if is_literal(text(p.start, p.end)) {
                    out.push(name);
                    continue;
                }
                if let Some(Read::Ident(id)) = parse(text(p.start, p.end))
                    && self
                        .read_name(f, named, val, id)
                        .is_some_and(|now| same(self.h, now, v))
                {
                    out.push(name);
                }
            }
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn assign_rereads(
        &self,
        f: Option<Obj>,
        named: &[NamedSlot],
        val: &dyn Fn(&NamedSlot) -> Value,
        ftext: &str,
        n: &crate::compiler::Node,
        slot: &dyn Fn(&str) -> Option<Value>,
        out: &mut Vec<String>,
    ) {
        let h = self.h;
        let text = |p: &crate::compiler::Part| {
            ftext
                .get(p.start as usize..p.end as usize)
                .unwrap_or("")
                .trim()
        };
        let part = |r: Role| n.parts.iter().find(|p| p.role == r);
        let name = |s: &str| cat!(s, "@", n.start);
        let old = slot(&name("assign.old"));
        let now = |id: &str| self.read_name(f, named, val, id);
        if let Some(l) = part(Role::Left) {
            // A name: only its old value is a temporary.
            if let (Some(ov), true) = (old, is_ident(text(l)))
                && now(text(l)).is_some_and(|v| same(h, v, ov))
            {
                out.push(name("assign.old"));
            }
            return;
        }
        let (Some(o), Some(pr), Some(ov)) = (
            part(Role::Object),
            part(Role::Property),
            slot(&name("assign.object")),
        ) else {
            return;
        };
        let Some(Read::Ident(oid)) = parse(text(o)) else {
            return;
        };
        let Some(obj) = now(oid)
            .filter(|&v| same(h, v, ov))
            .and_then(|v| v.as_object())
        else {
            return;
        };
        if h.object_class(obj) == ObjectClass::Proxy {
            return;
        }
        let pt = text(pr);
        let key = match pt.strip_prefix('[').and_then(|k| k.strip_suffix(']')) {
            Some(k) => {
                let Some(kv) = slot(&name("assign.key")) else {
                    return;
                };
                let k = k.trim();
                let ok = if is_literal(k) {
                    true
                } else if let Some(Read::Ident(id)) = parse(k) {
                    now(id).is_some_and(|v| {
                        (v.as_string().is_some() || v.as_number().is_some()) && same(h, v, kv)
                    })
                } else {
                    false
                };
                if !ok {
                    return;
                }
                kv.as_string().map(|s| text_of(h, s))
            }
            None => Some(String::from(pt.trim_start_matches('.').trim())),
        };
        if let Some(ov) = old {
            // (Its own data property holds the old value now.)
            let holds = key
                .as_deref()
                .and_then(|k| super::origin::own(h, obj, k))
                .is_some_and(|v| same(h, v, ov));
            if !holds {
                return;
            }
            out.push(name("assign.old"));
        }
        out.push(name("assign.object"));
        if pt.starts_with('[') {
            out.push(name("assign.key"));
        }
    }

    /// Whether object literal `n` (the object made so far `ov`, the site in part `k`) can
    /// be written as the original's text: the site in a property's value, every property
    /// before it a plain key (a name or a string) with a literal or a name that holds the
    /// object's value of it now, or a shorthand one.
    #[allow(clippy::too_many_arguments)]
    fn object_rereads(
        &self,
        f: Option<Obj>,
        named: &[NamedSlot],
        val: &dyn Fn(&NamedSlot) -> Value,
        ftext: &str,
        n: &crate::compiler::Node,
        ov: Value,
        k: usize,
    ) -> bool {
        let h = self.h;
        let text = |a: u32, b: u32| ftext.get(a as usize..b as usize).unwrap_or("");
        let Some(o) = ov.as_object() else {
            return false;
        };
        // The site in a spread, or in a value whose key is a plain one (a computed one
        // was evaluated already).
        let first = match n.parts[k].role {
            Role::Spread => k,
            Role::Value if k > 0 && n.parts[k - 1].role == Role::Key => k - 1,
            _ => return false,
        };
        let mut keys: Vec<String> = Vec::new();
        let mut m = 0;
        while m < first {
            let p = n.parts[m];
            let (key, value) = match p.role {
                Role::Shorthand => (String::from(text(p.start, p.end).trim()), p),
                Role::Key if n.parts[m + 1].role == Role::Value => {
                    let t = text(p.start, p.end).trim();
                    let key = if is_ident(t) {
                        String::from(t)
                    } else if is_literal(t) && (t.starts_with('"') || t.starts_with('\'')) {
                        String::from(&t[1..t.len() - 1])
                    } else {
                        return false;
                    };
                    m += 1;
                    (key, n.parts[m])
                }
                _ => return false,
            };
            m += 1;
            if key == "__proto__" || keys.contains(&key) || key.parse::<u32>().is_ok() {
                return false;
            }
            let Some(have) = super::origin::own(h, o, &key) else {
                return false;
            };
            let vt = text(value.start, value.end);
            let ok = if p.role != Role::Shorthand && is_literal(vt) {
                true
            } else if let Some(Read::Ident(id)) = parse(vt) {
                self.read_name(f, named, val, id)
                    .is_some_and(|now| same(h, now, have))
            } else {
                false
            };
            if !ok {
                return false;
            }
            keys.push(key);
        }
        // (Nothing else on it: its own keys are those.)
        m == first && h.own_keys(o).len() == keys.len()
    }

    /// The value identifier `name` reads in the continuation, if it reads one without
    /// side effects: the frame's binding, a variable `f` captured, a global.
    fn read_name(
        &self,
        f: Option<Obj>,
        named: &[NamedSlot],
        val: &dyn Fn(&NamedSlot) -> Value,
        name: &str,
    ) -> Option<Value> {
        let h = self.h;
        if name == "arguments" || name == "undefined" {
            return (name == "undefined").then_some(Value::UNDEFINED);
        }
        let bindings: Vec<&NamedSlot> = named
            .iter()
            .filter(|x| {
                matches!(x.holds, Holds::Binding)
                    && (x.name == name || x.name.rsplit_once('@').is_some_and(|(b, _)| b == name))
            })
            .collect();
        match bindings.as_slice() {
            [x] if x.name == name => {
                let v = val(x);
                return Some(match v.as_internal() {
                    Some(b) if h.cell_kind(b) == crate::heap::cells::BoxCell::KIND => {
                        h.arena().get(b, crate::heap::cells::BoxCell::VALUE)
                    }
                    _ => v,
                });
            }
            [] => {}
            // Several of the name in scope: which one the text reads is the compiler's.
            _ => return None,
        }
        if let Some(v) = f.and_then(|f| captured_value(h, f, name)) {
            return Some(v);
        }
        // A global: the global lexical record's binding, else the global object's data
        // property.
        let lex = self.vm.global_lexical();
        if let Some(v) = super::origin::own(h, lex, name) {
            return Some(v);
        }
        super::origin::own(h, self.vm.global(), name)
    }

    /// What `recv[key]` finds, where it finds a data property on a frozen object through
    /// frozen objects only (after `recv` itself, which has no own `key`).
    fn frozen_get(&self, recv: Value, key: crate::heap::Atom) -> Option<Value> {
        let h = self.h;
        let mut o = recv.as_object()?;
        let mut first = true;
        loop {
            if h.object_class(o) == ObjectClass::Proxy || (!first && !h.frozen(o)) {
                return None;
            }
            if h.own_keys(o).contains(&key) {
                return match h.get_own_property(o, key) {
                    Some(Property::Data { value, .. }) if !first && h.frozen(o) => Some(value),
                    _ => None,
                };
            }
            first = false;
            o = h.get_prototype_of(o)?;
        }
    }

    /// Whether `recv.key` finds `want` on a frozen object through frozen objects only
    /// (after `recv` itself, which may be the session's if it has no own `key`); `want`
    /// undefined: or finds nothing.
    fn frozen_lookup(&self, recv: Value, key: &str, want: Value) -> bool {
        let h = self.h;
        let Some(mut o) = recv.as_object() else {
            return false;
        };
        let mut first = true;
        loop {
            if h.object_class(o) == ObjectClass::Proxy || (!first && !h.frozen(o)) {
                return false;
            }
            let k = h
                .own_keys(o)
                .into_iter()
                .find(|&k| h.atom_symbol(k).is_none() && atom_text(h, k) == key);
            if let Some(k) = k {
                return h.frozen(o)
                    && matches!(h.get_own_property(o, k),
                        Some(Property::Data { value, .. }) if same(h, value, want));
            }
            first = false;
            match h.get_prototype_of(o) {
                Some(p) => o = p,
                None => return want.is_undefined(),
            }
        }
    }
}
