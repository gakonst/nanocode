//! Sites in loop heads and `var` declarations (D2880): a loop's test or update the
//! first time round, its init, its iterable, a `var` declarator.

use alloc::vec::Vec;

use super::*;
use crate::compiler::Part;

impl G<'_, '_> {
    /// The declarators of `var` declaration path node `i` from the site on, as
    /// assignments: K's prefix declares the names (with the values of those before).
    pub(super) fn var_list(&mut self, i: usize, n: &Node) -> R<Vec<P>> {
        let k = self
            .next_part(i)
            .ok_or("plain dumps: a declaration at the site")?;
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
            if m == k {
                let (pt, ps) = self.pattern(i + 1)?;
                items.push_out(P::L(alloc::vec![s("("), pt, s(" = "), ps, s(")")]));
            } else if init == Some(k) {
                let e = self.expr(i + 1)?;
                items.push_out(P::L(alloc::vec![
                    s("("),
                    P::V(t.start, t.end),
                    s(" = "),
                    e,
                    s(")")
                ]));
            } else if t.start > n.parts[k].start
                && let Some(x) = init
            {
                let v = n.parts[x];
                items.push_out(P::L(alloc::vec![
                    s("("),
                    P::V(t.start, t.end),
                    s(" = "),
                    self.rest(v.start, v.end),
                    s(")")
                ]));
            }
            m = init.map_or(m + 1, |x| x + 1);
        }
        let mut out = Vec::new();
        push_joined(&mut out, items);
        Ok(out)
    }

    /// A `for-in`/`for-of` head without its own bindings as an assignment target: a
    /// `var` head's names are K's.
    pub(super) fn var_head_target(&self, head: Part, text: &str) -> P {
        let t = text.trim_start();
        if let Some(rest) = t.strip_prefix("var")
            && rest.starts_with(|c: char| c.is_whitespace() || c == '[' || c == '{')
        {
            let skip = (text.len() - t.len() + 3) as u32;
            return P::V(head.start + skip, head.end);
        }
        P::V(head.start, head.end)
    }

    /// A `while`/`do` loop at its test: the rest of that test, then the loop as it is.
    pub(super) fn loop_test(&mut self, i: usize, n: &Node, label: Option<P>) -> R<P> {
        let t = n
            .parts
            .iter()
            .position(|p| p.role == Role::Test)
            .ok_or("plain dumps: a loop without its test")?;
        let b = n
            .parts
            .iter()
            .position(|p| p.role == Role::Body)
            .ok_or("plain dumps: a loop without its body")?;
        let f = self.fresh(cat!("$djs_f", i));
        let test = self.rest_part(n, t);
        let body = self.rest_part(n, b);
        let first = self.expr(i + 1)?;
        let m = self.mark("while", n.start);
        Ok(self.with_go(P::L(alloc::vec![
            P::S(cat!("{ let ", f, " = true; ")),
            label.unwrap_or_else(|| s("")),
            m,
            P::S(cat!("while (", f, " ? (", f, " = false, ")),
            first,
            s(") : ("),
            test,
            s(")) "),
            body,
            s(" }"),
        ])))
    }

    /// A `for (init; test; update)` loop's init, test and update as K writes them around
    /// a site in its body (`init`: its bindings from the frame).
    pub(super) fn for_parts(&mut self, n: &Node, init: Vec<P>) -> (P, P, P) {
        let test = match n.parts.iter().position(|p| p.role == Role::Test) {
            Some(t) => self.rest_part(n, t),
            None => s("true"),
        };
        let update = match n.parts.iter().position(|p| p.role == Role::Update) {
            Some(t) => self.rest_part(n, t),
            None => s(""),
        };
        (P::L(init), test, update)
    }

    /// A `for (init; test; update)` loop with the site in its head: in its init (the
    /// loop not begun), or its test or update (the rest of it, then the loop on).
    pub(super) fn for_head(&mut self, i: usize, n: &Node, k: usize, label: Option<P>) -> R<P> {
        let lbl = label.unwrap_or_else(|| s(""));
        let body = n
            .parts
            .iter()
            .position(|p| p.role == Role::Body)
            .ok_or("plain dumps: a loop without its body")?;
        let body = self.rest_part(n, body);
        let part = |r: Role| n.parts.iter().position(|p| p.role == r);
        match n.parts[k].role {
            Role::Init => {
                let nd = self.node(i + 1).clone();
                let init = match nd.kind {
                    NodeKind::Decl(kind) => P::L(self.decl_list(i + 1, &nd, kind)?),
                    _ => self.expr(i + 1)?,
                };
                let test = part(Role::Test).map_or(s(""), |t| self.rest_part(n, t));
                let update = part(Role::Update).map_or(s(""), |t| self.rest_part(n, t));
                let m = self.mark("for", n.start);
                Ok(self.with_go(P::L(alloc::vec![
                    lbl,
                    m,
                    s("for ("),
                    init,
                    s("; "),
                    test,
                    s("; "),
                    update,
                    s(") "),
                    body,
                ])))
            }
            Role::Test | Role::Update => {
                // The loop's bindings, from the frame; then the rest of this test (or
                // this update and the test), the loop going on after.
                let init = self.for_init_from_frame(n)?;
                let first = self.expr(i + 1)?;
                let f = self.fresh(cat!("$djs_f", i));
                let test = part(Role::Test).map_or(s("true"), |t| self.rest_part(n, t));
                let update = part(Role::Update).map_or(s(""), |t| self.rest_part(n, t));
                let now = match (n.parts[k].role, part(Role::Test)) {
                    (Role::Test, _) => first,
                    (_, Some(t)) => {
                        P::L(alloc::vec![first, s(", ("), self.rest_part(n, t), s(")")])
                    }
                    (_, None) => P::L(alloc::vec![first, s(", true")]),
                };
                let m = self.mark("for", n.start);
                Ok(self.with_go(P::L(alloc::vec![
                    P::S(cat!("{ let ", f, " = true; ")),
                    lbl,
                    m,
                    s("for ("),
                    init,
                    P::S(cat!("; ", f, " ? (", f, " = false, ")),
                    now,
                    s(") : ("),
                    test,
                    s("); "),
                    update,
                    s(") "),
                    body,
                    s(" }"),
                ])))
            }
            _ => no("a site in a for loop's head (0203)"),
        }
    }

    /// `for (let/const ...)`'s bindings with their values from the frame, as K's init.
    fn for_init_from_frame(&mut self, n: &Node) -> R<P> {
        let mut init: Vec<P> = Vec::new();
        if n.decls.is_empty() {
            // `var` or an expression init: K's prefix has the names.
            return Ok(s(""));
        }
        let kw = match n.decls[0].kind {
            DeclKind::Const => "const ",
            DeclKind::Let => "let ",
            _ => return no("a for loop's using declaration (0203)"),
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
        Ok(P::L(init))
    }

    /// A `for-in`/`for-of`/`for await` at its iterable: the loop with the rest of it.
    pub(super) fn loop_iterable(&mut self, i: usize, n: &Node, k: usize, label: Option<P>) -> R<P> {
        let it = n.parts[k];
        let e = self.expr(i + 1)?;
        let m = self.mark("loop", n.start);
        Ok(self.with_go(P::L(alloc::vec![
            label.unwrap_or_else(|| s("")),
            m,
            P::V(n.start, it.start),
            e,
            P::V(it.end, n.end),
        ])))
    }

    /// `++`/`--` of a member whose object or key holds the site: the reference again
    /// (its object saved), then the update.
    pub(super) fn update(&mut self, i: usize, n: &Node, k: usize) -> R<P> {
        let op = n
            .parts
            .iter()
            .position(|p| p.role == Role::Op)
            .ok_or("plain dumps: an update without its operator")?;
        let opt = P::V(n.parts[op].start, n.parts[op].end);
        let prefix = op == 0;
        let target = match n.parts[k].role {
            Role::Object => {
                let e = self.expr(i + 1)?;
                let pr = n
                    .parts
                    .iter()
                    .position(|p| p.role == Role::Property)
                    .ok_or("plain dumps: an update without its property")?;
                let p = n.parts[pr];
                let dot = if self.text(p.start, p.end).starts_with('[') {
                    s("")
                } else {
                    s(".")
                };
                P::L(alloc::vec![
                    s("("),
                    e,
                    s(")"),
                    dot,
                    self.rest(p.start, p.end)
                ])
            }
            Role::Property => {
                let o = n
                    .parts
                    .iter()
                    .find(|p| p.role == Role::Object)
                    .copied()
                    .ok_or("plain dumps: an update without its object")?;
                let obj = if self.has(&cat!("member.object@", o.start)) {
                    self.tmp("member.object", o.start)?
                } else {
                    self.tmp("assign.object", o.start)?
                };
                let e = self.expr(i + 1)?;
                P::L(alloc::vec![obj, s("["), e, s("]")])
            }
            _ => return no("a site in an update's operand"),
        };
        Ok(if prefix {
            P::L(alloc::vec![s("("), opt, target, s(")")])
        } else {
            P::L(alloc::vec![s("("), target, opt, s(")")])
        })
    }

    /// A tagged template with the site in a substitution: the call K makes with the
    /// saved tag, `this`, template object and substitutions before the site (the
    /// template object is the original's, as the dump wrote it).
    pub(super) fn tagged(&mut self, i: usize, n: &Node, k: usize) -> R<P> {
        if n.parts[k].role != Role::Subst {
            return no("a site in a tagged template's tag (0203)");
        }
        let ce = self.tmp("call.callee", n.start)?;
        let this = self.tmp("call.this", n.start)?;
        let first = n
            .parts
            .iter()
            .find(|p| p.role == Role::Quasi)
            .copied()
            .ok_or("plain dumps: a tagged template without its text")?;
        let tobj = self.tmp("call.arg", first.start)?;
        let mut list = alloc::vec![self.as_element(cat!("call.arg@", first.start), tobj)];
        for m in 0..n.parts.len() {
            let p = n.parts[m];
            if p.role != Role::Subst {
                continue;
            }
            let e = if m < k {
                self.tmp("call.arg", p.start)?
            } else if m == k {
                self.expr(i + 1)?
            } else {
                self.rest_part(n, m)
            };
            list.push_out(self.as_element(cat!("call.arg@", p.start), e));
        }
        let mut out = alloc::vec![
            self.kmark(),
            s("$djs_R("),
            self.kmark(),
            ce,
            s(", "),
            self.kmark(),
            this,
            s(", "),
            self.kmark(),
            s("[")
        ];
        push_joined(&mut out, list);
        out.push_out(s("])"));
        Ok(P::L(out))
    }

    /// A site in a `catch` parameter's pattern: the rest of the pattern declares the
    /// parameter's names in a block of K, the catch block after it (a `finally` around).
    pub(super) fn catch_param(&mut self, i: usize, n: &Node) -> R<P> {
        let part = |r: Role| n.parts.iter().position(|p| p.role == r);
        let cb = part(Role::CatchBody).ok_or("plain dumps: a catch without its block")?;
        let mut items: Vec<P> = Vec::new();
        for name in self.pattern_done_bindings(i + 1) {
            let v = self.decl_value(&name)?;
            items.push_out(P::S(cat!(name, " = ", v)));
        }
        let (pt, ps) = self.pattern(i + 1)?;
        items.push_out(P::L(alloc::vec![pt, s(" = "), ps]));
        let mut decl = alloc::vec![s("let ")];
        push_joined(&mut decl, items);
        decl.push_out(s(";"));
        let decl = self.with_go(P::L(decl));
        let body = self.rest_part(n, cb);
        let h = P::L(alloc::vec![s("{\n"), decl, s("\n"), body, s("\n}")]);
        Ok(match part(Role::Finally) {
            Some(fi) => {
                let m = self.mark("try", n.start);
                let fin = self.rest_part(n, fi);
                P::L(alloc::vec![m, s("try "), h, s(" finally "), fin])
            }
            None => h,
        })
    }
}
