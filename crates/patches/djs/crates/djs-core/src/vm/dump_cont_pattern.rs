//! Destructuring patterns around a continuation's site (D2874): the rest of a pattern
//! whose evaluation stopped is written as a pattern again, over a source that continues
//! where the original's stopped.
//!
//! - An array pattern continues over the saved iterator record: `$djs_L(it, next)`, whose
//!   first step is a placeholder (`undefined`, so the element's default runs: the site's
//!   default, or the rest of a nested pattern as that default), `$djs_I(it, next)` where
//!   the element's step is still to come (the site in a reference target), `$djs_E()`
//!   once the iterator is done. The language closes the iterator as the original would.
//! - An object pattern becomes `{ v0: …, v1: …, vR: rest }` over a source object of the
//!   dump's (`{ v0: undefined, get v1() { return src[key]; }, get vR() { … } }`): its
//!   getters read the saved source at the moments the original reads it, computed keys
//!   are evaluated in the pattern's own computed keys (`[(k = $djs_P(key), "v1")]`,
//!   converted once), and the rest property copies the source without the keys taken
//!   (`$djs_S(src, excluded)`), the excluded array growing as the original's does.

use alloc::string::String;
use alloc::vec::Vec;

use super::*;
use crate::compiler::Part;

impl G<'_, '_> {
    /// The rest of pattern node `i` (the site inside it): its target and its source.
    pub(super) fn pattern(&mut self, i: usize) -> R<(P, P)> {
        let n = self.node(i).clone();
        match n.kind {
            NodeKind::ObjectPattern => self.object_pattern(i, &n),
            NodeKind::ArrayPattern => self.array_pattern(i, &n),
            _ => Err(cat!(
                "plain dumps: a pattern target ",
                kind_name(n.kind),
                " around the site"
            )),
        }
    }

    /// A pattern element's target with its default, path node `j` inside it: the
    /// element, and whether its value was taken already (else the read is still to
    /// come: the site is in a reference target).
    fn pattern_element(&mut self, j: usize) -> R<(P, bool)> {
        let nd = self.node(j).clone();
        match nd.kind {
            NodeKind::Default => {
                let kd = self
                    .next_part(j)
                    .ok_or("plain dumps: a default at the site")?;
                let tp = nd.parts.iter().find(|p| p.role == Role::Target).copied();
                let ip = nd.parts.iter().find(|p| p.role == Role::Init).copied();
                let (Some(tp), Some(ip)) = (tp, ip) else {
                    return no("a default without its target");
                };
                if nd.parts[kd].role == Role::Init {
                    // The value was `undefined`: the default runs, the site in it.
                    let t = self.pattern_target(tp)?;
                    let e = self.expr(j + 1)?;
                    return Ok((P::L(alloc::vec![t, s(" = "), e]), true));
                }
                let (t, taken) = self.pattern_element(j + 1)?;
                if taken {
                    // A nested pattern: its rest is the element's default.
                    Ok((t, true))
                } else {
                    Ok((
                        P::L(alloc::vec![t, s(" = "), P::V(ip.start, ip.end)]),
                        false,
                    ))
                }
            }
            NodeKind::ObjectPattern | NodeKind::ArrayPattern => {
                let (t, src) = self.pattern(j)?;
                Ok((P::L(alloc::vec![t, s(" = "), src]), true))
            }
            NodeKind::Member { .. } => {
                // K's own reference target: the original's reference is K's.
                let mut marks = Vec::new();
                for c in ["assign.object", "assign.key"] {
                    let m = self.mark("tmp", 0);
                    if let P::Mark(x) = m {
                        self.scaf[x].3 = cat!(",", quote(&cat!(c, "@", nd.start)), ",", quote(c));
                    }
                    marks.push_out(m);
                }
                marks.push_out(self.expr(j)?);
                Ok((P::L(marks), false))
            }
            _ => Err(cat!(
                "plain dumps: a pattern target ",
                kind_name(nd.kind),
                " around the site"
            )),
        }
    }

    /// A target the site is not in, whose default holds the site: a name or pattern as
    /// it is, a reference target as the reference the original evaluated.
    fn pattern_target(&mut self, tp: Part) -> R<P> {
        let text = String::from(self.text(tp.start, tp.end).trim());
        if is_ident(&text) || text.starts_with('{') || text.starts_with('[') {
            return Ok(P::V(tp.start, tp.end));
        }
        let oname = cat!("assign.object@", tp.start);
        if !self.has(&oname) {
            return no("a reference target of a pattern without its reference");
        }
        let obj = self.tmp("assign.object", tp.start)?;
        if self.has(&cat!("assign.key@", tp.start)) {
            let key = self.tmp("assign.key", tp.start)?;
            return Ok(P::L(alloc::vec![obj, s("["), key, s("]")]));
        }
        match text.rfind('.') {
            Some(d) if is_ident(text[d + 1..].trim()) => Ok(P::L(alloc::vec![
                obj,
                P::S(cat!(".", text[d + 1..].trim()))
            ])),
            _ => no("a reference target of a pattern the exporter cannot read"),
        }
    }

    fn array_pattern(&mut self, i: usize, n: &Node) -> R<(P, P)> {
        let k = self
            .next_part(i)
            .ok_or("plain dumps: a pattern at the site")?;
        if n.parts[k].role != Role::Element {
            return no("a site in a rest element (0203)");
        }
        let iname = cat!("pattern.iterator@", n.start);
        let done = self.ctx.slot(&iname).is_none_or(|v| v == "undefined");
        let done = self.steer(u32::from(done)) == 1;
        let rec = if done {
            if self.has(&iname) {
                self.consume(&iname);
            }
            if self.has(&cat!("pattern.next@", n.start)) {
                self.consume(&cat!("pattern.next@", n.start));
            }
            None
        } else {
            let it = self.tmp("pattern.iterator", n.start)?;
            // (Dead after the last step: only the close is left.)
            let next = if self.has(&cat!("pattern.next@", n.start)) {
                self.tmp("pattern.next", n.start)?
            } else {
                s("undefined")
            };
            Some((it, next))
        };
        let (el, taken) = self.pattern_element(i + 1)?;
        let mut items = alloc::vec![el];
        for m in k + 1..n.parts.len() {
            let p = n.parts[m];
            items.push_out(if p.role == Role::Hole {
                s("")
            } else {
                P::V(p.start, p.end)
            });
        }
        let trailing_hole = n.parts.last().is_some_and(|p| p.role == Role::Hole);
        // K's own pattern: its iterator is the wrapper (`undefined` once done), which
        // origin tracking reads back as the original's.
        let m = self.mark("pattern", n.start);
        let mut target = alloc::vec![m, s("[")];
        push_joined(&mut target, items);
        target.push_out(s(if trailing_hole { ",]" } else { "]" }));
        let source = match rec {
            None => s("$djs_E()"),
            Some((it, next)) => P::L(alloc::vec![
                s(if taken { "$djs_L(" } else { "$djs_I(" }),
                it,
                s(", "),
                next,
                s(")")
            ]),
        };
        Ok((P::L(target), source))
    }

    fn object_pattern(&mut self, i: usize, n: &Node) -> R<(P, P)> {
        let k = self
            .next_part(i)
            .ok_or("plain dumps: a pattern at the site")?;
        // (Dead once the last property read it: then nothing reads it again.)
        let src = if self.has(&cat!("pattern.source@", n.start)) {
            self.tmp_text("pattern.source", n.start)?
        } else {
            String::from("undefined")
        };
        let excl = if self.has(&cat!("pattern.excluded@", n.start)) {
            Some(self.tmp_text("pattern.excluded", n.start)?)
        } else {
            None
        };
        // Appends key expression `key` to the excluded keys (at the key, as the
        // original does), in a computed key of K's pattern.
        let push = |key: &str| match &excl {
            Some(x) => cat!(x, "[", x, ".length] = ", key, ", "),
            None => String::new(),
        };
        let mut props: Vec<P> = Vec::new();
        let mut srcs: Vec<String> = Vec::new();
        let mut v = 0;
        // The property holding the site.
        let role = n.parts[k].role;
        let mut m = match role {
            Role::ComputedKey => {
                let kv = self.new_temp();
                self.lets.push_out(kv.clone());
                self.cmap
                    .push_out((cat!("pattern.key@", n.parts[k].start), kv.clone(), false));
                let e = self.expr(i + 1)?;
                let val = n
                    .parts
                    .get(k + 1)
                    .filter(|p| p.role == Role::Value)
                    .copied()
                    .ok_or("plain dumps: a computed key without its value")?;
                props.push_out(P::L(alloc::vec![
                    self.kmark(),
                    P::S(cat!("[(", kv, " = ")),
                    self.kmark(),
                    s("$djs_P("),
                    e,
                    P::S(cat!("), ", push(&kv), "\"v0\")]: ")),
                    P::V(val.start, val.end),
                ]));
                srcs.push_out(cat!("get v0() { return ", src, "[", kv, "]; }"));
                k + 2
            }
            Role::Value | Role::Shorthand => {
                let key = if role == Role::Shorthand {
                    quote(&shorthand_name(self.text(n.parts[k].start, n.parts[k].end)))
                } else {
                    let kp = k
                        .checked_sub(1)
                        .map(|x| n.parts[x])
                        .ok_or("plain dumps: a property without its key")?;
                    if kp.role == Role::ComputedKey {
                        self.tmp_text("pattern.key", kp.start)?
                    } else {
                        self.key_expr(kp)?
                    }
                };
                let (el, taken) = self.pattern_element(i + 1)?;
                props.push_out(P::L(alloc::vec![s("v0: "), el]));
                srcs.push_out(if taken {
                    String::from("v0: undefined")
                } else {
                    cat!("get v0() { return ", src, "[", key, "]; }")
                });
                k + 1
            }
            _ => return no("a site in a pattern's rest property (0203)"),
        };
        // The properties after it, as they are.
        while m < n.parts.len() {
            v += 1;
            let p = n.parts[m];
            let name = cat!("v", v);
            match p.role {
                Role::Key | Role::ComputedKey => {
                    let val = n
                        .parts
                        .get(m + 1)
                        .copied()
                        .ok_or("plain dumps: a property without its value")?;
                    let (kexpr, key) = if p.role == Role::ComputedKey {
                        let kv = self.new_temp();
                        self.lets.push_out(kv.clone());
                        // The original's converted key is K's variable.
                        self.cmap
                            .push_out((cat!("pattern.key@", p.start), kv.clone(), false));
                        let e = P::L(alloc::vec![
                            P::S(cat!("(", kv, " = ")),
                            self.kmark(),
                            s("$djs_P("),
                            self.rest(p.start + 1, p.end - 1),
                            P::S(cat!("), ", push(&kv), quote(&name), ")")),
                        ]);
                        (e, kv)
                    } else {
                        let key = self.key_expr(p)?;
                        let e = P::S(cat!("(", push(&key), quote(&name), ")"));
                        (e, key)
                    };
                    props.push_out(P::L(alloc::vec![
                        self.kmark(),
                        s("["),
                        kexpr,
                        s("]: "),
                        P::V(val.start, val.end),
                    ]));
                    srcs.push_out(cat!("get ", name, "() { return ", src, "[", key, "]; }"));
                    m += 2;
                }
                Role::Shorthand => {
                    let key = quote(&shorthand_name(self.text(p.start, p.end)));
                    props.push_out(P::L(alloc::vec![
                        self.kmark(),
                        P::S(cat!("[(", push(&key), quote(&name), ")]: ")),
                        P::V(p.start, p.end),
                    ]));
                    srcs.push_out(cat!("get ", name, "() { return ", src, "[", key, "]; }"));
                    m += 1;
                }
                Role::Rest => {
                    let Some(x) = &excl else {
                        return no("a rest property without its excluded keys");
                    };
                    props.push_out(P::L(alloc::vec![
                        P::S(cat!(name, ": ")),
                        P::V(p.start + 3, p.end),
                    ]));
                    srcs.push_out(cat!(
                        "get ",
                        name,
                        "() { return $djs_S(",
                        src,
                        ", ",
                        x,
                        "); }"
                    ));
                    m += 1;
                }
                _ => return no("a pattern property the exporter does not know"),
            }
        }
        let mut target = alloc::vec![self.kmark(), s("{ ")];
        push_joined(&mut target, props);
        target.push_out(s(" }"));
        let source = P::S(cat!("{ ", join(&srcs, ", "), " }"));
        Ok((P::L(target), source))
    }

    /// The key of a pattern property as an expression of the key (its name quoted, a
    /// string literal as it is, a number converted as a key).
    fn key_expr(&self, p: Part) -> R<String> {
        let t = self.text(p.start, p.end).trim();
        Ok(if is_ident(t) {
            quote(t)
        } else if t.starts_with('"') || t.starts_with('\'') {
            String::from(t)
        } else {
            cat!("$djs_P(", t, ")")
        })
    }

    /// Bindings of the declarator whose target is pattern path node `i` that the
    /// pattern initialized before the site: those declared in its parts before the one
    /// that holds the site, at every level down to the site.
    pub(super) fn pattern_done_bindings(&self, i: usize) -> Vec<String> {
        let mut spans: Vec<(u32, u32)> = Vec::new();
        for j in i..self.act.site.path.len() {
            let nd = self.node(j);
            if !matches!(nd.kind, NodeKind::ObjectPattern | NodeKind::ArrayPattern) {
                continue;
            }
            let Some(k) = self.next_part(j) else { continue };
            let mut first = k;
            if nd.parts[k].role == Role::Value
                && k > 0
                && matches!(nd.parts[k - 1].role, Role::Key | Role::ComputedKey)
            {
                first = k - 1;
            }
            for p in &nd.parts[..first] {
                spans.push_out((p.start, p.end));
            }
        }
        let mut out: Vec<String> = Vec::new();
        for j in 0..self.act.site.path.len() {
            for d in &self.node(j).decls {
                if spans.iter().any(|&(a, b)| a <= d.at && d.at < b) && !out.contains(&d.name) {
                    out.push_out(d.name.clone());
                }
            }
        }
        out
    }
}

/// The binding of a shorthand property (`a` of `a = 1`).
fn shorthand_name(t: &str) -> String {
    let t = t.trim();
    String::from(t.split(['=', ' ']).next().unwrap_or(t).trim())
}
